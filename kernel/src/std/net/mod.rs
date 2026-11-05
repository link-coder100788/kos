//! std::net for the kernel: `TcpStream`, `TcpListener` and `UdpSocket`, with std's names,
//! signatures and behaviour, on top of the network task (crate::net).
//!
//! ```ignore
//! use crate::std::io::{Read, Write};
//! use crate::std::net::{TcpListener, TcpStream, UdpSocket};
//!
//! let mut stream = TcpStream::connect("example.com:80")?;
//! stream.write_all(b"GET / HTTP/1.0\r\nHost: example.com\r\n\r\n")?;
//! let mut reply = Vec::new();
//! stream.read_to_end(&mut reply)?;
//!
//! let listener = TcpListener::bind("0.0.0.0:8080")?;
//! for stream in listener.incoming() {
//!     stream?.write_all(b"hello\n")?;
//! }
//!
//! let socket = UdpSocket::bind("0.0.0.0:0")?;
//! socket.send_to(&query, ("10.0.2.3", 53))?;
//! let (n, from) = socket.recv_from(&mut buf)?;
//! ```
//!
//! Differences from std: IPv4 only; there's no `shutdown`; a UdpSocket or
//! TcpListener binds a port on every interface whatever address it's
//! given; and a read (or accept) that times out fails with
//! `ErrorKind::TimedOut`. A datagram is at most
//! `UDP_MAX_DATAGRAM` bytes. Sockets close when dropped.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use crate::arch::timer;
use crate::net::{self as kernel_net, ConnRef, ListenerRef, SockRef, Wait};
use crate::std::io::{self, Error, ErrorKind, Read, Write};
use crate::std::lock::irq_mutex::IrqMutex;

pub use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
pub use crate::net::UDP_MAX_DATAGRAM;

/// Turns the network task's error text into an io::Error of the right kind.
fn error(why: String) -> Error {
    let kind = if why.contains("timed out") {
        ErrorKind::TimedOut
    } else if why.contains("in use") {
        ErrorKind::AddrInUse
    } else if why.contains("couldn't find host") || why.contains("has no address") {
        ErrorKind::NotFound
    } else if why.contains("couldn't connect") {
        ErrorKind::ConnectionRefused
    } else if why.contains("closed") {
        ErrorKind::ConnectionReset
    } else if why.contains("too big") || why.contains("bad port") || why.contains("is DHCP's") {
        ErrorKind::InvalidInput
    } else {
        ErrorKind::Other
    };
    Error::new(kind, why)
}

fn v4(addr: SocketAddr) -> io::Result<SocketAddrV4> {
    match addr {
        SocketAddr::V4(a) => Ok(a),
        SocketAddr::V6(_) => Err(Error::new(ErrorKind::InvalidInput, "IPv6 isn't supported")),
    }
}

// ---- addresses ------------------------------------------------------------------

/// Things that name socket addresses: a `SocketAddr`, `(ip, port)`,
/// `(host, port)` or `"host:port"`. Host names are looked up (over DNS).
pub trait ToSocketAddrs {
    type Iter: Iterator<Item = SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter>;
}

impl ToSocketAddrs for SocketAddr {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        Ok(Some(*self).into_iter())
    }
}

impl ToSocketAddrs for SocketAddrV4 {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        SocketAddr::V4(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for (IpAddr, u16) {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        SocketAddr::new(self.0, self.1).to_socket_addrs()
    }
}

impl ToSocketAddrs for (Ipv4Addr, u16) {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        SocketAddrV4::new(self.0, self.1).to_socket_addrs()
    }
}

/// Looks up the IPv4 address of a host name, or of the host in a URL
/// (`"example.com"`, `"example.com:8080"`, `"https://example.com/page"`), waiting
/// for the answer. An address is returned as it is.
pub fn lookup_host(name: &str) -> io::Result<Ipv4Addr> {
    let url = kernel_net::Url::parse(name).map_err(|e| Error::new(ErrorKind::InvalidInput, e))?;
    if let Ok(ip) = url.host.parse::<Ipv4Addr>() {
        return Ok(ip);
    }
    kernel_net::resolve(&url.host).wait_unless_killed()
        .unwrap_or_else(|| Err("killed".to_string()))
        .map_err(error)
}

fn lookup(host: &str, port: u16) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
    let ip = lookup_host(host)?;
    Ok(alloc::vec![SocketAddr::V4(SocketAddrV4::new(ip, port))].into_iter())
}

impl ToSocketAddrs for (&str, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        lookup(self.0, self.1)
    }
}

impl ToSocketAddrs for (String, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        lookup(&self.0, self.1)
    }
}

impl ToSocketAddrs for str {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        let invalid = || Error::new(ErrorKind::InvalidInput, "invalid socket address");
        let (host, port) = self.rsplit_once(':').ok_or_else(invalid)?;
        lookup(host, port.parse().map_err(|_| invalid())?)
    }
}

impl ToSocketAddrs for String {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        self.as_str().to_socket_addrs()
    }
}

impl<T: ToSocketAddrs + ?Sized> ToSocketAddrs for &T {
    type Iter = T::Iter;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        (**self).to_socket_addrs()
    }
}

/// Tries `f` on each of `addr`'s addresses, returning the first success
/// (or the last failure), like std does.
fn each_addr<A: ToSocketAddrs, T>(addr: A, mut f: impl FnMut(SocketAddrV4) -> io::Result<T>) -> io::Result<T> {
    let mut last = None;
    for a in addr.to_socket_addrs()? {
        match v4(a).and_then(&mut f) {
            Ok(t) => return Ok(t),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| Error::new(ErrorKind::InvalidInput, "could not resolve to any addresses")))
}

/// This machine's address, or 0.0.0.0 before DHCP has given it one.
fn our_ip() -> Ipv4Addr {
    kernel_net::status().address.map_or(Ipv4Addr::UNSPECIFIED, |cidr| cidr.address())
}

// ---- reading options ----------------------------------------------------------------

/// `set_nonblocking` and `set_read_timeout`, shared by both kinds of socket.
struct ReadOptions {
    nonblocking: AtomicBool,
    /// 0 for none.
    timeout_ms: AtomicU64,
}

impl ReadOptions {
    const fn new() -> Self {
        Self { nonblocking: AtomicBool::new(false), timeout_ms: AtomicU64::new(0) }
    }

    fn set_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        let ms = match dur {
            Some(d) if d.is_zero() => return Err(Error::new(ErrorKind::InvalidInput, "cannot set a 0 duration timeout")),
            Some(d) => (d.as_millis() as u64).max(1),
            None => 0,
        };
        self.timeout_ms.store(ms, Ordering::Relaxed);
        Ok(())
    }

    fn timeout(&self) -> Option<Duration> {
        match self.timeout_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }

    fn wait(&self) -> Wait {
        if self.nonblocking.load(Ordering::Relaxed) {
            return Wait::No;
        }
        match self.timeout_ms.load(Ordering::Relaxed) {
            0 => Wait::Forever,
            ms => Wait::Until(timer::deadline_after_ms(ms)),
        }
    }

    /// The error for a read that found nothing.
    fn nothing(&self) -> Error {
        if self.nonblocking.load(Ordering::Relaxed) {
            Error::new(ErrorKind::WouldBlock, "nothing to read yet")
        } else {
            Error::new(ErrorKind::TimedOut, "timed out")
        }
    }
}

// ---- TCP ----------------------------------------------------------------------------

/// A TCP connection. Closed when dropped.
pub struct TcpStream {
    conn: ConnRef,
    options: ReadOptions,
}

impl TcpStream {
    /// Connects to `addr`, waiting until it's connected.
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        each_addr(addr, |a| {
            let conn = kernel_net::conn_open(&a.ip().to_string(), a.port(), None).map_err(error)?;
            Ok(TcpStream { conn, options: ReadOptions::new() })
        })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        kernel_net::conn_ends(&self.conn).0.ok_or_else(|| ErrorKind::NotConnected.into())
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let (_, port) = kernel_net::conn_ends(&self.conn);
        Ok(SocketAddr::V4(SocketAddrV4::new(our_ip(), port)))
    }

    /// Makes reads fail with `WouldBlock` instead of waiting.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.options.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }

    /// Makes reads give up (with `TimedOut`) after `dur`; None waits forever.
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.options.set_timeout(dur)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(self.options.timeout())
    }
}

impl Read for &TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match kernel_net::conn_recv(&self.conn, buf, self.options.wait()).map_err(error)? {
            Some(n) => Ok(n),
            None => Err(self.options.nothing()),
        }
    }
}

impl Write for &TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        kernel_net::conn_send(&self.conn, buf).map_err(error)
    }

    /// Everything written is already on its way.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        kernel_net::conn_close(&self.conn);
    }
}

/// Listens for TCP connections. Closed when dropped, along with any
/// connections that came in but weren't accepted.
pub struct TcpListener {
    listener: ListenerRef,
    options: ReadOptions,
}

impl TcpListener {
    /// Listens on `addr`'s port (0 picks a free one).
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        each_addr(addr, |a| {
            let listener = kernel_net::listen_open(a.port(), None).map_err(error)?;
            Ok(TcpListener { listener, options: ReadOptions::new() })
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(our_ip(), kernel_net::listen_port(&self.listener))))
    }

    /// Waits for a connection; returns it and who it's from.
    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let Some(conn) = kernel_net::listen_accept(&self.listener, self.options.wait()).map_err(error)? else {
            return Err(self.options.nothing());
        };
        let stream = TcpStream { conn, options: ReadOptions::new() };
        let peer = stream.peer_addr()?;
        Ok((stream, peer))
    }

    /// Accepts connections, one per step. Never ends.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    /// Makes `accept` fail with `WouldBlock` instead of waiting.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.options.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }

    /// Not in std: makes `accept` give up (with `TimedOut`) after `dur`;
    /// None waits forever.
    pub fn set_accept_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.options.set_timeout(dur)
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        kernel_net::listen_close(&self.listener);
    }
}

/// See `TcpListener::incoming`.
pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

impl Iterator for Incoming<'_> {
    type Item = io::Result<TcpStream>;

    fn next(&mut self) -> Option<io::Result<TcpStream>> {
        Some(self.listener.accept().map(|(stream, _)| stream))
    }
}

// ---- UDP ----------------------------------------------------------------------------

/// A UDP socket. Closed when dropped.
pub struct UdpSocket {
    sock: SockRef,
    options: ReadOptions,
    /// Set by `connect`: where `send` goes, and the only sender `recv` takes.
    peer: IrqMutex<Option<SocketAddrV4>>,
}

impl UdpSocket {
    /// Opens a socket on `addr`'s port (0 picks a free one).
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<UdpSocket> {
        each_addr(addr, |a| {
            let sock = kernel_net::sock_open(a.port(), None).map_err(error)?;
            Ok(UdpSocket { sock, options: ReadOptions::new(), peer: IrqMutex::new(None) })
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(SocketAddrV4::new(our_ip(), kernel_net::sock_port(&self.sock))))
    }

    /// Sends `buf` as one datagram to `addr`.
    pub fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> io::Result<usize> {
        each_addr(addr, |a| self.send_v4(buf, a))
    }

    fn send_v4(&self, buf: &[u8], to: SocketAddrV4) -> io::Result<usize> {
        kernel_net::sock_send(&self.sock, &to.ip().to_string(), to.port(), buf).map_err(error)?;
        Ok(buf.len())
    }

    /// Receives one datagram into `buf` (cut short if it doesn't fit).
    /// Returns its length and who sent it.
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let wait = self.options.wait();
        loop {
            let Some(d) = kernel_net::sock_recv(&self.sock, wait).map_err(error)? else {
                return Err(self.options.nothing());
            };
            // A connected socket only takes datagrams from its peer.
            if let Some(peer) = *self.peer.lock() {
                if d.from != SocketAddr::V4(peer) {
                    continue;
                }
            }
            let n = d.data.len().min(buf.len());
            buf[..n].copy_from_slice(&d.data[..n]);
            return Ok((n, d.from));
        }
    }

    /// Makes `send` go to `addr`, and `recv` take datagrams only from it.
    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        each_addr(addr, |a| {
            *self.peer.lock() = Some(a);
            Ok(())
        })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.peer.lock().map(SocketAddr::V4).ok_or_else(|| ErrorKind::NotConnected.into())
    }

    /// Sends to the address given to `connect`.
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let peer = self.peer.lock().ok_or_else(|| Error::from(ErrorKind::NotConnected))?;
        self.send_v4(buf, peer)
    }

    /// Receives from the address given to `connect`.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        if self.peer.lock().is_none() {
            return Err(ErrorKind::NotConnected.into());
        }
        self.recv_from(buf).map(|(n, _)| n)
    }

    /// Makes receiving fail with `WouldBlock` instead of waiting.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.options.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }

    /// Makes receiving give up (with `TimedOut`) after `dur`; None waits forever.
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.options.set_timeout(dur)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(self.options.timeout())
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        kernel_net::sock_close(&self.sock);
    }
}

use crate::net::ntp::{NtpUdpRecv, NtpUdpSend};
use crate::std;

impl NtpUdpRecv for UdpSocket {
    type Error = std::io::Error;

    fn recv_from(&self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.recv_from(buf).map(|(read, addr)| read)
    }
}

impl NtpUdpSend for UdpSocket {
    type Error = std::io::Error;

    fn send_to(&self, buf: &[u8], addr: (Ipv4Addr, u16)) -> Result<usize, Self::Error> {
        self.send_to(buf, addr)
    }
}
