//! Networking: one task owns the virtio-net device and the smoltcp stack.
//!
//! Everyone else asks it for work through `fetch` / `fetch_with` (HTTP(S)
//! requests) and `ping`, which return a `Pending` result to poll (the
//! Browser) or block on (the shell). Plain TCP connections and UDP sockets
//! (see "TCP connections" and "UDP sockets" below) are for the kernel,
//! through `std::net::{TcpStream, UdpSocket}`, and for programs, through
//! the `tcp_*` / `udp_*` calls. The task configures itself over DHCP, then sleeps until a packet
//! arrives, a smoltcp timer is due, or a new request comes in. Each request
//! is a small state machine (resolve the host, connect, send, receive), so
//! one slow request never holds up the others. https:// requests hand
//! their encryption to the `tls` task (see `tls.rs`).

pub mod http;
pub mod ntp;
pub mod tls;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use core::net::{SocketAddr, SocketAddrV4};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::{dhcpv4, dns, icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    DnsQueryType, EthernetAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr,
};
use crate::arch::{gic, timer};
use crate::drivers::virtio::net::VirtioNet;
use crate::println;
use crate::scheduler::{self, wait::WaitQueue, TaskId};
use crate::std::lock::irq_mutex::IrqMutex;

pub use http::{HttpResponse, RequestOptions, Url};
pub use tls::tls_task;

/// Give up on a request after this long.
const REQUEST_TIMEOUT_MS: u64 = 15_000;
const PING_TIMEOUT_MS: u64 = 3_000;
/// Responses are cut off beyond this size.
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const MAX_REDIRECTS: u8 = 5;

// ---- public API -------------------------------------------------------------

pub type FetchResult = Result<HttpResponse, String>;
/// Round-trip time in milliseconds.
pub type PingResult = Result<u64, String>;
pub type ResolveResult = Result<Ipv4Address, String>;

struct Slot<T> {
    result: IrqMutex<Option<T>>,
}

/// A request in flight. Take the result with `poll` or `wait`.
pub struct Pending<T>(Arc<Slot<T>>);

static RESULT_WAIT: WaitQueue = WaitQueue::new();

impl<T> Pending<T> {
    fn new() -> (Self, Arc<Slot<T>>) {
        let slot = Arc::new(Slot { result: IrqMutex::new(None) });
        (Self(slot.clone()), slot)
    }

    /// The result, once it's ready. Returns it only once.
    pub fn poll(&self) -> Option<T> {
        self.0.result.lock().take()
    }

    /// Blocks until the result is ready.
    pub fn wait(self) -> T {
        let mut result = None;
        RESULT_WAIT.wait_until(|| {
            result = self.0.result.lock().take();
            result.is_some()
        });
        result.unwrap()
    }

    /// Like `wait`, but gives up (returning None) if the waiting task is
    /// killed: for programs, which can be ended mid-request.
    pub fn wait_unless_killed(self) -> Option<T> {
        let mut result = None;
        RESULT_WAIT.wait_until(|| {
            result = self.0.result.lock().take();
            result.is_some() || scheduler::current_killed()
        });
        result
    }
}

fn complete<T>(slot: &Slot<T>, value: T) {
    *slot.result.lock() = Some(value);
    RESULT_WAIT.wake_all();
}

enum Request {
    Fetch { url: String, opts: RequestOptions, slot: Arc<Slot<FetchResult>> },
    Ping { host: String, slot: Arc<Slot<PingResult>> },
    Resolve { host: String, slot: Arc<Slot<ResolveResult>> },
    Tcp { conn: Arc<IrqMutex<Conn>>, host: String, port: u16 },
    Udp { sock: Arc<IrqMutex<UdpSock>>, port: u16 },
}

static REQUESTS: IrqMutex<VecDeque<Request>> = IrqMutex::new(VecDeque::new());
static NET_WAIT: WaitQueue = WaitQueue::new();
/// Set when another task (tls) has bytes for the net task to deal with.
static KICKED: AtomicBool = AtomicBool::new(false);

/// Wakes the net task to look at its requests again.
fn kick() {
    KICKED.store(true, Ordering::Release);
    NET_WAIT.wake_all();
}

fn request(req: Request) {
    REQUESTS.lock().push_back(req);
    NET_WAIT.wake_all();
}

/// Starts an HTTP GET of `url` (http:// or https://).
pub fn fetch(url: &str) -> Pending<FetchResult> {
    fetch_with(url, RequestOptions::default())
}

/// Starts an HTTP request of `url` with another method, headers or a body.
pub fn fetch_with(url: &str, opts: RequestOptions) -> Pending<FetchResult> {
    let (pending, slot) = Pending::new();
    request(Request::Fetch { url: url.to_string(), opts, slot });
    pending
}

/// Looks up `host`'s IPv4 address (an address is returned as it is).
pub fn resolve(host: &str) -> Pending<ResolveResult> {
    let (pending, slot) = Pending::new();
    request(Request::Resolve { host: host.to_string(), slot });
    pending
}

/// Sends one ICMP echo request to `host` (a name or an IPv4 address).
pub fn ping(host: &str) -> Pending<PingResult> {
    let (pending, slot) = Pending::new();
    request(Request::Ping { host: host.to_string(), slot });
    pending
}

#[derive(Clone, Default)]
pub struct Status {
    pub device: bool,
    pub mac: [u8; 6],
    pub address: Option<Ipv4Cidr>,
    pub gateway: Option<Ipv4Address>,
    pub dns: Vec<Ipv4Address>,
}

static STATUS: IrqMutex<Option<Status>> = IrqMutex::new(None);

/// The interface's current configuration.
pub fn status() -> Status {
    STATUS.lock().clone().unwrap_or_default()
}

// ---- waiting -------------------------------------------------------------------

/// How long reading from a connection or socket may wait for data.
#[derive(Clone, Copy)]
pub enum Wait {
    /// Not at all.
    No,
    Forever,
    /// Until the timer counter reaches this (see `timer::deadline_after_ms`).
    Until(u64),
}

/// Waits as `wait` says until `ready`, or the waiting task is killed.
fn wait_for(wait: Wait, mut ready: impl FnMut() -> bool) {
    let deadline = match wait {
        Wait::No => return,
        Wait::Forever => None,
        Wait::Until(d) => Some(d),
    };
    RESULT_WAIT.wait_until_deadline(deadline, || ready() || scheduler::current_killed());
}

// ---- TCP connections ----------------------------------------------------------
// A connection: its user fills `tx` and drains `rx`; the network task moves
// bytes between them and the smoltcp socket. The kernel holds one as a
// `ConnRef` (see std::net::TcpStream); programs get an id, and their
// connections are closed when they end.

/// Stop reading from the network while this much is waiting to be read.
const TCP_RX_MAX: usize = 256 * 1024;
const TCP_TX_MAX: usize = 256 * 1024;
/// How long a closing connection gets to say goodbye.
const TCP_CLOSE_MS: u64 = 3_000;

pub type ConnId = u64;
pub type ConnRef = Arc<IrqMutex<Conn>>;

pub struct Conn {
    /// The program it belongs to, if it's a program's.
    owner: Option<TaskId>,
    connected: bool,
    /// Who it's connected to, once the host has been looked up.
    peer: Option<SocketAddr>,
    /// Our port, from then too.
    local_port: u16,
    /// The connection is over: None if the other side closed it, or why it failed.
    ended: Option<Option<String>>,
    /// Its user is done with it.
    closing: bool,
    rx: VecDeque<u8>,
    tx: VecDeque<u8>,
}

/// Opens a TCP connection to `host:port` (a name or an IPv4 address), and
/// waits until it's connected (or has failed).
pub fn conn_open(host: &str, port: u16, owner: Option<TaskId>) -> Result<ConnRef, String> {
    let conn = Arc::new(IrqMutex::new(Conn {
        owner,
        connected: false,
        peer: None,
        local_port: 0,
        ended: None,
        closing: false,
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    }));
    request(Request::Tcp { conn: conn.clone(), host: host.to_string(), port });
    wait_for(Wait::Forever, || {
        let c = conn.lock();
        c.connected || c.ended.is_some()
    });
    let failed = {
        let c = conn.lock();
        if c.connected { None } else { Some(c.ended.clone().flatten().unwrap_or_else(|| "connection closed".to_string())) }
    };
    match failed {
        None => Ok(conn),
        Some(why) => {
            conn_close(&conn);
            Err(why)
        }
    }
}

/// Queues `data` to send, waiting while too much is already queued.
/// Returns how much was queued.
pub fn conn_send(conn: &IrqMutex<Conn>, data: &[u8]) -> Result<usize, String> {
    let mut sent = 0;
    while sent < data.len() {
        wait_for(Wait::Forever, || {
            let c = conn.lock();
            c.tx.len() < TCP_TX_MAX || c.ended.is_some()
        });
        let mut c = conn.lock();
        if let Some(ended) = &c.ended {
            return Err(ended.clone().unwrap_or_else(|| "the other side closed the connection".to_string()));
        }
        if scheduler::current_killed() {
            break;
        }
        let n = (TCP_TX_MAX - c.tx.len()).min(data.len() - sent);
        c.tx.extend(&data[sent..sent + n]);
        sent += n;
        drop(c);
        kick();
    }
    Ok(sent)
}

/// Reads what has arrived into `buf`, waiting as `wait` says for something
/// to. Ok(None) means nothing has; Ok(Some(0)) that the other side has
/// closed the connection and everything has been read.
pub fn conn_recv(conn: &IrqMutex<Conn>, buf: &mut [u8], wait: Wait) -> Result<Option<usize>, String> {
    wait_for(wait, || {
        let c = conn.lock();
        !c.rx.is_empty() || c.ended.is_some()
    });
    let mut c = conn.lock();
    if c.rx.is_empty() {
        return match &c.ended {
            Some(None) => Ok(Some(0)),
            Some(Some(why)) => Err(why.clone()),
            None => Ok(None),
        };
    }
    let n = c.rx.len().min(buf.len());
    for (dst, src) in buf.iter_mut().zip(c.rx.drain(..n)) {
        *dst = src;
    }
    drop(c);
    // There may be room to read more from the network now.
    kick();
    Ok(Some(n))
}

/// Who the connection is to, and our port.
pub fn conn_ends(conn: &IrqMutex<Conn>) -> (Option<SocketAddr>, u16) {
    let c = conn.lock();
    (c.peer, c.local_port)
}

/// Closes the connection, letting the other side know.
pub fn conn_close(conn: &IrqMutex<Conn>) {
    conn.lock().closing = true;
    kick();
}

// Programs' connections, by id.

static CONNS: IrqMutex<BTreeMap<ConnId, ConnRef>> = IrqMutex::new(BTreeMap::new());
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// The calling program's connection `id`.
fn own_conn(id: ConnId) -> Result<ConnRef, String> {
    let conn = CONNS.lock().get(&id).cloned().ok_or("no such connection")?;
    if conn.lock().owner != Some(scheduler::current_id()) {
        return Err("not your connection".to_string());
    }
    Ok(conn)
}

/// Opens a connection for the calling program (see `conn_open`).
pub fn tcp_connect(host: &str, port: u16) -> Result<ConnId, String> {
    let conn = conn_open(host, port, Some(scheduler::current_id()))?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) as ConnId;
    CONNS.lock().insert(id, conn);
    Ok(id)
}

pub fn tcp_send(id: ConnId, data: &[u8]) -> Result<usize, String> {
    conn_send(&*own_conn(id)?, data)
}

pub fn tcp_recv(id: ConnId, buf: &mut [u8], wait: bool) -> Result<Option<usize>, String> {
    conn_recv(&*own_conn(id)?, buf, if wait { Wait::Forever } else { Wait::No })
}

/// Closes connection `id`, if it's the caller's.
pub fn tcp_close(id: ConnId) {
    if let Ok(conn) = own_conn(id) {
        CONNS.lock().remove(&id);
        conn_close(&conn);
    }
}

/// Closes every connection and UDP socket program `owner` has open (it has ended).
pub fn close_owned_by(owner: TaskId) {
    let mut conns = CONNS.lock();
    let mine: Vec<ConnId> = conns.iter().filter(|(_, c)| c.lock().owner == Some(owner)).map(|(id, _)| *id).collect();
    for id in mine {
        if let Some(conn) = conns.remove(&id) {
            conn.lock().closing = true;
        }
    }
    drop(conns);
    let mut socks = UDP_SOCKS.lock();
    let mine: Vec<SockId> = socks.iter().filter(|(_, s)| s.lock().owner == Some(owner)).map(|(id, _)| *id).collect();
    for id in mine {
        if let Some(sock) = socks.remove(&id) {
            sock.lock().closing = true;
        }
    }
    drop(socks);
    kick();
}

// ---- UDP sockets ----------------------------------------------------------------
// A UDP socket is bound to one local port. Sending waits until the datagram
// has been handed to smoltcp (after looking up the host), so a bad host
// name comes back as an error; received datagrams wait in `rx` with who
// sent them. The kernel holds one as a `SockRef` (see std::net::UdpSocket);
// programs get an id, and their sockets are closed when they end.

/// The most one datagram can carry without IP fragmentation (a 1500 byte
/// Ethernet frame, less the IPv4 and UDP headers).
pub const UDP_MAX_DATAGRAM: usize = 1472;
/// Datagrams waiting to be read; more are dropped, as UDP allows.
const UDP_RX_MAX: usize = 64;
/// DHCP's client port: smoltcp's DHCP socket owns it.
const DHCP_CLIENT_PORT: u16 = 68;

pub type SockId = u64;
pub type SockRef = Arc<IrqMutex<UdpSock>>;

/// One received datagram and where it came from.
pub struct Datagram {
    pub from: SocketAddr,
    pub data: Vec<u8>,
}

struct Outgoing {
    host: String,
    port: u16,
    data: Vec<u8>,
    slot: Arc<Slot<Result<(), String>>>,
}

pub struct UdpSock {
    /// The program it belongs to, if it's a program's.
    owner: Option<TaskId>,
    /// The local port, once bound.
    port: Option<u16>,
    /// Why it couldn't be opened.
    failed: Option<String>,
    /// Its user is done with it.
    closing: bool,
    rx: VecDeque<Datagram>,
    tx: VecDeque<Outgoing>,
}

/// Opens a UDP socket on local `port` (0 picks a free one).
pub fn sock_open(port: u16, owner: Option<TaskId>) -> Result<SockRef, String> {
    let sock = Arc::new(IrqMutex::new(UdpSock {
        owner,
        port: None,
        failed: None,
        closing: false,
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    }));
    request(Request::Udp { sock: sock.clone(), port });
    wait_for(Wait::Forever, || {
        let s = sock.lock();
        s.port.is_some() || s.failed.is_some()
    });
    let failed = {
        let s = sock.lock();
        match s.port {
            Some(_) => None,
            None => Some(s.failed.clone().unwrap_or_else(|| "socket closed".to_string())),
        }
    };
    match failed {
        None => Ok(sock),
        Some(why) => {
            sock_close(&sock);
            Err(why)
        }
    }
}

/// Sends `data` as one datagram to `host:port` (a name or an IPv4 address),
/// waiting until it's on its way.
pub fn sock_send(sock: &IrqMutex<UdpSock>, host: &str, port: u16, data: &[u8]) -> Result<(), String> {
    if data.len() > UDP_MAX_DATAGRAM {
        return Err(format!("datagram too big ({} bytes; at most {})", data.len(), UDP_MAX_DATAGRAM));
    }
    if port == 0 {
        return Err("bad port".to_string());
    }
    let (pending, slot) = Pending::new();
    sock.lock().tx.push_back(Outgoing { host: host.to_string(), port, data: data.to_vec(), slot });
    kick();
    pending.wait_unless_killed().unwrap_or_else(|| Err("killed".to_string()))
}

/// Takes the next datagram that has arrived, waiting as `wait` says for one
/// to. Ok(None) means none has.
pub fn sock_recv(sock: &IrqMutex<UdpSock>, wait: Wait) -> Result<Option<Datagram>, String> {
    wait_for(wait, || !sock.lock().rx.is_empty());
    let mut s = sock.lock();
    match s.rx.pop_front() {
        Some(d) => Ok(Some(d)),
        None if s.closing => Err("socket closed".to_string()),
        None => Ok(None),
    }
}

/// The local port it's bound to.
pub fn sock_port(sock: &IrqMutex<UdpSock>) -> u16 {
    sock.lock().port.unwrap_or(0)
}

pub fn sock_close(sock: &IrqMutex<UdpSock>) {
    sock.lock().closing = true;
    kick();
}

// Programs' sockets, by id.

static UDP_SOCKS: IrqMutex<BTreeMap<SockId, SockRef>> = IrqMutex::new(BTreeMap::new());

/// The calling program's socket `id`.
fn own_sock(id: SockId) -> Result<SockRef, String> {
    let sock = UDP_SOCKS.lock().get(&id).cloned().ok_or("no such socket")?;
    if sock.lock().owner != Some(scheduler::current_id()) {
        return Err("not your socket".to_string());
    }
    Ok(sock)
}

/// Opens a socket for the calling program (see `sock_open`).
pub fn udp_open(port: u16) -> Result<SockId, String> {
    let sock = sock_open(port, Some(scheduler::current_id()))?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) as SockId;
    UDP_SOCKS.lock().insert(id, sock);
    Ok(id)
}

pub fn udp_send(id: SockId, host: &str, port: u16, data: &[u8]) -> Result<(), String> {
    sock_send(&*own_sock(id)?, host, port, data)
}

pub fn udp_recv(id: SockId, wait: bool) -> Result<Option<Datagram>, String> {
    sock_recv(&*own_sock(id)?, if wait { Wait::Forever } else { Wait::No })
}

/// Closes socket `id`, if it's the caller's.
pub fn udp_close(id: SockId) {
    if let Ok(sock) = own_sock(id) {
        UDP_SOCKS.lock().remove(&id);
        sock_close(&sock);
    }
}

// ---- the network task ---------------------------------------------------------

static NET_BASE: AtomicUsize = AtomicUsize::new(0);
static IRQ_PENDING: AtomicBool = AtomicBool::new(false);

fn net_irq() {
    let base = NET_BASE.load(Ordering::Acquire);
    unsafe {
        let status = core::ptr::read_volatile((base + 0x060) as *const u32);
        core::ptr::write_volatile((base + 0x064) as *mut u32, status);
    }
    IRQ_PENDING.store(true, Ordering::Release);
    NET_WAIT.wake_all();
}

fn smol_now() -> Instant {
    Instant::from_millis(timer::uptime_ms() as i64)
}

/// How a request's host is being turned into an address.
enum Resolve {
    /// Waiting for DHCP to give us a DNS server.
    NotStarted,
    Query(dns::QueryHandle),
    Done(IpAddress),
}

struct FetchOp {
    slot: Arc<Slot<FetchResult>>,
    url: Url,
    opts: RequestOptions,
    redirects: u8,
    resolve: Resolve,
    tcp: Option<SocketHandle>,
    request: Vec<u8>,
    sent: usize,
    established: bool,
    response: Vec<u8>,
    /// For https://, the link to the tls task once connected.
    tls: Option<Arc<tls::Pipe>>,
    deadline: u64,
}

struct PingOp {
    slot: Arc<Slot<PingResult>>,
    host: String,
    resolve: Resolve,
    icmp: Option<SocketHandle>,
    sent_at: Option<u64>,
    deadline: u64,
}

struct ResolveOp {
    slot: Arc<Slot<ResolveResult>>,
    host: String,
    resolve: Resolve,
    deadline: u64,
}

struct TcpOp {
    conn: Arc<IrqMutex<Conn>>,
    host: String,
    port: u16,
    resolve: Resolve,
    tcp: Option<SocketHandle>,
    /// Until it connects: when to give up.
    deadline: u64,
    /// Once the owner is done: when to stop waiting for a clean close.
    close_by: Option<u64>,
}

struct UdpOp {
    sock: Arc<IrqMutex<UdpSock>>,
    /// The port asked for (0 = any).
    port: u16,
    udp: Option<SocketHandle>,
    /// The datagram being sent: its host's lookup, and when to give up.
    sending: Option<(Outgoing, Resolve, u64)>,
}

enum Op {
    Fetch(FetchOp),
    Ping(PingOp),
    Resolve(ResolveOp),
    Tcp(TcpOp),
    Udp(UdpOp),
}

struct Stack {
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    dns: SocketHandle,
    have_dns: bool,
    next_port: u16,
    ping_ident: u16,
    /// Local ports programs' UDP sockets are bound to.
    udp_ports: BTreeSet<u16>,
}

fn host_literal(host: &str) -> Option<IpAddress> {
    let mut parts = host.split('.');
    let mut octets = [0u8; 4];
    for o in octets.iter_mut() {
        *o = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then(|| IpAddress::Ipv4(Ipv4Address::new(octets[0], octets[1], octets[2], octets[3])))
}

impl Stack {
    fn local_port(&mut self) -> u16 {
        self.next_port = if self.next_port >= 65000 { 49152 } else { self.next_port + 1 };
        self.next_port
    }

    /// Advances name resolution. Ok(Some(addr)) when done.
    fn resolve(&mut self, host: &str, state: &mut Resolve) -> Result<Option<IpAddress>, String> {
        loop {
            match state {
                Resolve::Done(addr) => return Ok(Some(*addr)),
                Resolve::NotStarted => {
                    if let Some(addr) = host_literal(host) {
                        *state = Resolve::Done(addr);
                        continue;
                    }
                    if !self.have_dns {
                        return Ok(None);
                    }
                    let dns = self.sockets.get_mut::<dns::Socket>(self.dns);
                    let query = dns
                        .start_query(self.iface.context(), host, DnsQueryType::A)
                        .map_err(|e| format!("can't look up {}: {:?}", host, e))?;
                    log::debug!("dns: looking up {}", host);
                    *state = Resolve::Query(query);
                }
                Resolve::Query(query) => {
                    let dns = self.sockets.get_mut::<dns::Socket>(self.dns);
                    match dns.get_query_result(*query) {
                        Ok(addrs) => match addrs.first() {
                            Some(addr) => {
                                log::debug!("dns: {} is {} ({} addresses)", host, addr, addrs.len());
                                *state = Resolve::Done(*addr)
                            }
                            None => return Err(format!("{} has no address", host)),
                        },
                        Err(dns::GetQueryResultError::Pending) => return Ok(None),
                        Err(e) => {
                            log::debug!("dns: {} failed: {:?}", host, e);
                            return Err(format!("couldn't find host {}", host));
                        }
                    }
                }
            }
        }
    }

    fn close_tcp(&mut self, handle: Option<SocketHandle>) {
        if let Some(h) = handle {
            self.sockets.get_mut::<tcp::Socket>(h).abort();
            self.sockets.remove(h);
        }
    }

    /// Advances a fetch. Returns true when it's finished (successfully or not).
    fn step_fetch(&mut self, op: &mut FetchOp, now: u64) -> bool {
        if now >= op.deadline {
            log::warn!("{} {} timed out", op.opts.method, op.url.to_string());
            if let Some(pipe) = op.tls.take() {
                pipe.abort();
            }
            self.close_tcp(op.tcp.take());
            let why = if self.have_dns { "timed out" } else { "timed out (no network configuration from DHCP)" };
            complete(&op.slot, Err(why.to_string()));
            return true;
        }
        let addr = match self.resolve(&op.url.host.clone(), &mut op.resolve) {
            Ok(Some(addr)) => addr,
            Ok(None) => return false,
            Err(e) => {
                complete(&op.slot, Err(e));
                return true;
            }
        };
        let handle = match op.tcp {
            Some(h) => h,
            None => {
                let socket = tcp::Socket::new(
                    tcp::SocketBuffer::new(vec![0; 64 * 1024]),
                    tcp::SocketBuffer::new(vec![0; 4 * 1024]),
                );
                let h = self.sockets.add(socket);
                let port = self.local_port();
                let result = self.sockets.get_mut::<tcp::Socket>(h).connect(self.iface.context(), (addr, op.url.port), port);
                if let Err(e) = result {
                    self.sockets.remove(h);
                    complete(&op.slot, Err(format!("can't connect: {:?}", e)));
                    return true;
                }
                log::debug!("tcp: connecting to {}:{} from port {}", addr, op.url.port, port);
                op.tcp = Some(h);
                h
            }
        };
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket.state() == tcp::State::Established && !op.established {
            log::debug!("tcp: connected to {}:{}; sending {} byte request", addr, op.url.port, op.request.len());
            op.established = true;
        }
        let closed = if op.url.tls {
            // The tls task does the talking; we just carry its encrypted
            // bytes, and finish when it hands back the decrypted response.
            if op.established && op.tls.is_none() {
                op.tls = Some(tls::start(&op.url.host, op.request.clone()));
            }
            match &op.tls {
                Some(pipe) => match pipe.pump(socket) {
                    None => false,
                    Some(Ok(plain)) => {
                        op.response = plain;
                        true
                    }
                    Some(Err(e)) => {
                        op.tls = None;
                        self.close_tcp(op.tcp.take());
                        complete(&op.slot, Err(e));
                        return true;
                    }
                },
                // Not connected yet, or the connection was refused.
                None => !socket.is_active(),
            }
        } else {
            if socket.may_send() && op.sent < op.request.len() {
                if let Ok(n) = socket.send_slice(&op.request[op.sent..]) {
                    op.sent += n;
                }
            }
            while socket.can_recv() {
                let mut chunk = [0u8; 2048];
                match socket.recv_slice(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if op.response.len() + n <= MAX_RESPONSE {
                            op.response.extend_from_slice(&chunk[..n]);
                        }
                    }
                }
            }
            // Finished once the server has closed its side and we've read everything.
            !socket.is_active() || (op.established && !socket.may_recv())
        };
        if !closed {
            return false;
        }
        let established = op.established;
        log::debug!("tcp: {}:{} closed after {} bytes received", addr, op.url.port, op.response.len());
        op.tls = None;
        self.close_tcp(op.tcp.take());
        if !established {
            complete(&op.slot, Err(format!("couldn't connect to {}:{}", op.url.host, op.url.port)));
            return true;
        }
        let mut response = match http::parse_response(&core::mem::take(&mut op.response)) {
            Ok(r) => r,
            Err(e) => {
                complete(&op.slot, Err(e));
                return true;
            }
        };
        // Follow redirects, to http:// or https://.
        if (300..400).contains(&response.status) && op.redirects < MAX_REDIRECTS {
            if let Some(location) = response.header("location") {
                match op.url.join(location) {
                    Ok(next) => {
                        log::debug!("http: {} redirects to {}", op.url.to_string(), next.to_string());
                        op.redirects += 1;
                        op.opts = op.opts.redirected(response.status);
                        op.request = http::request_bytes(&next, &op.opts);
                        if next.host != op.url.host {
                            op.resolve = Resolve::NotStarted;
                        }
                        op.url = next;
                        op.sent = 0;
                        op.established = false;
                        return false;
                    }
                    Err(e) => {
                        complete(&op.slot, Err(format!("redirected to {}: {}", location, e)));
                        return true;
                    }
                }
            }
        }
        log::info!("{} {} -> {} {} ({} bytes, {} ms)", op.opts.method, op.url.to_string(), response.status, response.reason,
            response.body.len(), now + REQUEST_TIMEOUT_MS - op.deadline);
        response.url = op.url.to_string();
        complete(&op.slot, Ok(response));
        true
    }

    /// Advances a lookup. Returns true when it's finished.
    fn step_resolve(&mut self, op: &mut ResolveOp, now: u64) -> bool {
        let result = if now >= op.deadline {
            Err(if self.have_dns { "timed out" } else { "timed out (no network configuration from DHCP)" }.to_string())
        } else {
            match self.resolve(&op.host.clone(), &mut op.resolve) {
                Ok(None) => return false,
                Ok(Some(IpAddress::Ipv4(ip))) => Ok(ip),
                Err(e) => Err(e),
            }
        };
        complete(&op.slot, result);
        true
    }

    /// Advances a TCP connection. Returns true once it's over.
    fn step_tcp(&mut self, op: &mut TcpOp, now: u64) -> bool {
        let finish = |op: &mut TcpOp, why: Option<String>| {
            let mut c = op.conn.lock();
            if c.ended.is_none() {
                c.ended = Some(why);
            }
            drop(c);
            RESULT_WAIT.wake_all();
            true
        };
        let closing = op.conn.lock().closing;
        if closing {
            // The owner is done: close our side, and give the other a
            // moment to see it.
            let Some(h) = op.tcp else { return finish(op, None) };
            let close_by = *op.close_by.get_or_insert_with(|| {
                self.sockets.get_mut::<tcp::Socket>(h).close();
                now + TCP_CLOSE_MS
            });
            if self.sockets.get_mut::<tcp::Socket>(h).is_open() && now < close_by {
                return false;
            }
            self.close_tcp(op.tcp.take());
            return finish(op, None);
        }
        let connected = op.conn.lock().connected;
        if !connected && now >= op.deadline {
            self.close_tcp(op.tcp.take());
            return finish(op, Some(format!("couldn't connect to {}:{} (timed out)", op.host, op.port)));
        }
        let addr = match self.resolve(&op.host.clone(), &mut op.resolve) {
            Ok(Some(addr)) => addr,
            Ok(None) => return false,
            Err(e) => return finish(op, Some(e)),
        };
        let handle = match op.tcp {
            Some(h) => h,
            None => {
                let socket = tcp::Socket::new(
                    tcp::SocketBuffer::new(vec![0; 64 * 1024]),
                    tcp::SocketBuffer::new(vec![0; 16 * 1024]),
                );
                let h = self.sockets.add(socket);
                let port = self.local_port();
                if let Err(e) = self.sockets.get_mut::<tcp::Socket>(h).connect(self.iface.context(), (addr, op.port), port) {
                    self.sockets.remove(h);
                    return finish(op, Some(format!("can't connect: {:?}", e)));
                }
                log::debug!("tcp: connecting to {}:{} from port {}", addr, op.port, port);
                let IpAddress::Ipv4(ip) = addr;
                let mut c = op.conn.lock();
                c.peer = Some(SocketAddr::V4(SocketAddrV4::new(ip, op.port)));
                c.local_port = port;
                drop(c);
                op.tcp = Some(h);
                h
            }
        };
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        let mut c = op.conn.lock();
        let mut changed = false;
        if socket.state() == tcp::State::Established && !c.connected {
            c.connected = true;
            changed = true;
        }
        while socket.can_send() && !c.tx.is_empty() {
            let (front, _) = c.tx.as_slices();
            match socket.send_slice(front) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    c.tx.drain(..n);
                    changed = true;
                }
            }
        }
        while socket.can_recv() && c.rx.len() < TCP_RX_MAX {
            let mut chunk = [0u8; 2048];
            let room = chunk.len().min(TCP_RX_MAX - c.rx.len());
            match socket.recv_slice(&mut chunk[..room]) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    c.rx.extend(&chunk[..n]);
                    changed = true;
                }
            }
        }
        // Over once it's reset, or the other side has closed it and
        // everything it sent has been taken from the socket.
        let over = !socket.is_active() || (c.connected && !socket.may_recv() && !socket.can_recv());
        let was_connected = c.connected;
        drop(c);
        if changed {
            RESULT_WAIT.wake_all();
        }
        if !over {
            return false;
        }
        // Whatever was received stays readable after the socket goes.
        log::debug!("tcp: {}:{} closed", op.host, op.port);
        self.close_tcp(op.tcp.take());
        finish(op, if was_connected { None } else { Some(format!("couldn't connect to {}:{}", op.host, op.port)) })
    }

    /// Advances a UDP socket. Returns true once it's closed.
    fn step_udp(&mut self, op: &mut UdpOp, now: u64) -> bool {
        let closing = op.sock.lock().closing;
        if closing {
            let unsent = core::mem::take(&mut op.sock.lock().tx);
            for d in op.sending.take().map(|(d, _, _)| d).into_iter().chain(unsent) {
                complete(&d.slot, Err("socket closed".to_string()));
            }
            if let Some(h) = op.udp.take() {
                let port = self.sockets.get::<udp::Socket>(h).endpoint().port;
                self.udp_ports.remove(&port);
                self.sockets.remove(h);
                log::debug!("udp: closed port {}", port);
            }
            return true;
        }
        let handle = match op.udp {
            Some(h) => h,
            None => {
                let fail = |op: &mut UdpOp, why: String| {
                    op.sock.lock().failed = Some(why);
                    RESULT_WAIT.wake_all();
                    true
                };
                let port = match op.port {
                    0 => loop {
                        let p = self.local_port();
                        if !self.udp_ports.contains(&p) {
                            break p;
                        }
                    },
                    DHCP_CLIENT_PORT => return fail(op, format!("port {} is DHCP's", DHCP_CLIENT_PORT)),
                    p if self.udp_ports.contains(&p) => return fail(op, format!("port {} is in use", p)),
                    p => p,
                };
                let mut socket = udp::Socket::new(
                    udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 16], vec![0; 16 * 1024]),
                    udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4 * 1024]),
                );
                if let Err(e) = socket.bind(port) {
                    return fail(op, format!("can't use port {}: {:?}", port, e));
                }
                let h = self.sockets.add(socket);
                self.udp_ports.insert(port);
                log::debug!("udp: bound port {}", port);
                op.udp = Some(h);
                op.sock.lock().port = Some(port);
                RESULT_WAIT.wake_all();
                h
            }
        };
        // Send what's queued, one datagram at a time, in order.
        loop {
            if op.sending.is_none() {
                let Some(d) = op.sock.lock().tx.pop_front() else { break };
                op.sending = Some((d, Resolve::NotStarted, now + REQUEST_TIMEOUT_MS));
            }
            let (d, resolve, deadline) = op.sending.as_mut().unwrap();
            let result = if now >= *deadline {
                Err(if self.have_dns { "timed out" } else { "timed out (no network configuration from DHCP)" }.to_string())
            } else {
                match self.resolve(&d.host.clone(), resolve) {
                    Ok(None) => break,
                    Err(e) => Err(e),
                    Ok(Some(addr)) => match self.sockets.get_mut::<udp::Socket>(handle).send_slice(&d.data, (addr, d.port)) {
                        Ok(()) => {
                            log::debug!("udp: {} bytes to {}:{}", d.data.len(), addr, d.port);
                            Ok(())
                        }
                        // Try again once smoltcp has sent what it has.
                        Err(udp::SendError::BufferFull) => break,
                        Err(e) => Err(format!("can't send to {}:{}: {:?}", d.host, d.port, e)),
                    },
                }
            };
            let (d, _, _) = op.sending.take().unwrap();
            complete(&d.slot, result);
        }
        // Hand over what has arrived.
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        let mut s = op.sock.lock();
        let mut got = false;
        while let Ok((data, meta)) = socket.recv() {
            if s.rx.len() < UDP_RX_MAX {
                let IpAddress::Ipv4(ip) = meta.endpoint.addr;
                s.rx.push_back(Datagram { from: SocketAddr::V4(SocketAddrV4::new(ip, meta.endpoint.port)), data: data.to_vec() });
                got = true;
            }
        }
        drop(s);
        if got {
            RESULT_WAIT.wake_all();
        }
        false
    }

    fn close_icmp(&mut self, handle: Option<SocketHandle>) {
        if let Some(h) = handle {
            self.sockets.remove(h);
        }
    }

    fn step_ping(&mut self, op: &mut PingOp, now: u64) -> bool {
        if now >= op.deadline {
            self.close_icmp(op.icmp.take());
            complete(&op.slot, Err("no reply".to_string()));
            return true;
        }
        let addr = match self.resolve(&op.host.clone(), &mut op.resolve) {
            Ok(Some(addr)) => addr,
            Ok(None) => return false,
            Err(e) => {
                complete(&op.slot, Err(e));
                return true;
            }
        };
        let handle = match op.icmp {
            Some(h) => h,
            None => {
                let socket = icmp::Socket::new(
                    icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 2], vec![0; 512]),
                    icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 2], vec![0; 512]),
                );
                let h = self.sockets.add(socket);
                self.ping_ident = self.ping_ident.wrapping_add(1);
                let _ = self.sockets.get_mut::<icmp::Socket>(h).bind(icmp::Endpoint::Ident(self.ping_ident));
                op.icmp = Some(h);
                h
            }
        };
        let ident = self.ping_ident;
        let socket = self.sockets.get_mut::<icmp::Socket>(handle);
        if op.sent_at.is_none() && socket.can_send() {
            let repr = Icmpv4Repr::EchoRequest { ident, seq_no: 1, data: b"KOS ping" };
            let mut buf = vec![0u8; repr.buffer_len()];
            repr.emit(&mut Icmpv4Packet::new_unchecked(&mut buf), &Default::default());
            if socket.send_slice(&buf, addr).is_ok() {
                op.sent_at = Some(now);
            }
        }
        while let Ok((data, from)) = socket.recv() {
            let Ok(packet) = Icmpv4Packet::new_checked(data) else { continue };
            if let Ok(Icmpv4Repr::EchoReply { ident: id, .. }) = Icmpv4Repr::parse(&packet, &Default::default()) {
                if id == ident && from == addr {
                    let rtt = now - op.sent_at.unwrap_or(now);
                    log::debug!("ping {} ({}): reply in {} ms", op.host, addr, rtt);
                    self.close_icmp(op.icmp.take());
                    complete(&op.slot, Ok(rtt));
                    return true;
                }
            }
        }
        false
    }

    fn handle_dhcp(&mut self, status: &mut Status) {
        let event = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll();
        match event {
            Some(dhcpv4::Event::Configured(config)) => {
                let address = config.address;
                let router = config.router;
                let servers: Vec<IpAddress> = config.dns_servers.iter().map(|&a| IpAddress::Ipv4(a)).collect();
                status.dns = config.dns_servers.iter().copied().collect();
                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(IpCidr::Ipv4(address));
                });
                match router {
                    Some(r) => { let _ = self.iface.routes_mut().add_default_ipv4_route(r); }
                    None => { self.iface.routes_mut().remove_default_ipv4_route(); }
                }
                self.sockets.get_mut::<dns::Socket>(self.dns).update_servers(&servers);
                self.have_dns = !servers.is_empty();
                status.address = Some(address);
                status.gateway = router;
                log::info!("DHCP: address {}, gateway {:?}, dns {:?}", address, router, status.dns);
                *STATUS.lock() = Some(status.clone());
            }
            // smoltcp reports Deconfigured once at startup too; only act on a real loss.
            Some(dhcpv4::Event::Deconfigured) if status.address.is_some() => {
                self.iface.update_ip_addrs(|addrs| addrs.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                self.have_dns = false;
                status.address = None;
                status.gateway = None;
                status.dns.clear();
                log::warn!("lost DHCP configuration");
                *STATUS.lock() = Some(status.clone());
            }
            _ => {}
        }
    }
}

fn start_op(req: Request, now: u64) -> Option<Op> {
    match req {
        Request::Fetch { url, opts, slot } => match Url::parse(&url) {
            Ok(url) => {
                log::debug!("http: {} {}", opts.method, url.to_string());
                crate::stats::inc(&crate::stats::HTTP_REQUESTS);
                Some(Op::Fetch(FetchOp {
                    slot,
                    request: http::request_bytes(&url, &opts),
                    url,
                    opts,
                    redirects: 0,
                    resolve: Resolve::NotStarted,
                    tcp: None,
                    sent: 0,
                    established: false,
                    response: Vec::new(),
                    tls: None,
                    deadline: now + REQUEST_TIMEOUT_MS,
                }))
            }
            Err(e) => {
                log::debug!("http: bad URL {}: {}", url, e);
                complete(&slot, Err(e));
                None
            }
        },
        Request::Ping { host, slot } => Some(Op::Ping(PingOp {
            slot,
            host,
            resolve: Resolve::NotStarted,
            icmp: None,
            sent_at: None,
            deadline: now + PING_TIMEOUT_MS,
        })),
        Request::Resolve { host, slot } => Some(Op::Resolve(ResolveOp {
            slot,
            host,
            resolve: Resolve::NotStarted,
            deadline: now + REQUEST_TIMEOUT_MS,
        })),
        Request::Tcp { conn, host, port } => Some(Op::Tcp(TcpOp {
            conn,
            host,
            port,
            resolve: Resolve::NotStarted,
            tcp: None,
            deadline: now + REQUEST_TIMEOUT_MS,
            close_by: None,
        })),
        Request::Udp { sock, port } => Some(Op::Udp(UdpOp { sock, port, udp: None, sending: None })),
    }
}

/// Fails every request: there's no network device.
fn serve_without_device() -> ! {
    loop {
        NET_WAIT.wait_until(|| !REQUESTS.lock().is_empty());
        let requests = core::mem::take(&mut *REQUESTS.lock());
        for req in requests {
            match req {
                Request::Fetch { slot, .. } => complete(&slot, Err("no network device".to_string())),
                Request::Ping { slot, .. } => complete(&slot, Err("no network device".to_string())),
                Request::Resolve { slot, .. } => complete(&slot, Err("no network device".to_string())),
                Request::Tcp { conn, .. } => {
                    conn.lock().ended = Some(Some("no network device".to_string()));
                    RESULT_WAIT.wake_all();
                }
                Request::Udp { sock, .. } => {
                    sock.lock().failed = Some("no network device".to_string());
                    RESULT_WAIT.wake_all();
                }
            }
        }
    }
}

pub extern "C" fn net_task() -> ! {
    let Some(mut device) = VirtioNet::init() else {
        log::info!("no virtio-net device; networking disabled");
        *STATUS.lock() = Some(Status::default());
        serve_without_device();
    };
    let mac = device.mac_address();
    NET_BASE.store(device.mmio_base(), Ordering::Release);
    let slot = ((device.mmio_base() - crate::drivers::virtio::mmio::MMIO_BASE) / crate::drivers::virtio::mmio::MMIO_STRIDE) as u32;
    gic::register(gic::VIRTIO_IRQ_BASE + slot, net_irq);
    log::info!("virtio-net at slot {}, mac {:02x?}", slot, mac);

    let config = Config::new(EthernetAddress(mac).into());
    let iface = Interface::new(config, &mut device, smol_now());
    let mut sockets = SocketSet::new(Vec::new());
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let dns = sockets.add(dns::Socket::new(&[], Vec::from([None, None, None, None])));
    let mut stack = Stack { iface, sockets, dhcp, dns, have_dns: false, next_port: 49152, ping_ident: 0x4b00, udp_ports: BTreeSet::new() };
    let mut status = Status { device: true, mac, ..Default::default() };
    *STATUS.lock() = Some(status.clone());

    let mut ops: Vec<Op> = Vec::new();
    loop {
        stack.iface.poll(smol_now(), &mut device, &mut stack.sockets);
        stack.handle_dhcp(&mut status);

        let now = timer::uptime_ms();
        let new_requests = core::mem::take(&mut *REQUESTS.lock());
        ops.extend(new_requests.into_iter().filter_map(|r| start_op(r, now)));
        ops.retain_mut(|op| !match op {
            Op::Fetch(f) => stack.step_fetch(f, now),
            Op::Ping(p) => stack.step_ping(p, now),
            Op::Resolve(r) => stack.step_resolve(r, now),
            Op::Tcp(t) => stack.step_tcp(t, now),
            Op::Udp(u) => stack.step_udp(u, now),
        });
        // Send whatever the requests just queued.
        stack.iface.poll(smol_now(), &mut device, &mut stack.sockets);

        // Sleep until a packet arrives, a request comes in, or smoltcp (or
        // an in-flight request) needs attention.
        let mut delay = stack.iface.poll_delay(smol_now(), &stack.sockets).map_or(1000, |d| d.total_millis());
        if !ops.is_empty() {
            delay = delay.min(50);
        }
        let deadline = timer::deadline_after_ms(delay.max(1));
        NET_WAIT.wait_until_deadline(Some(deadline), || {
            IRQ_PENDING.swap(false, Ordering::AcqRel) || KICKED.swap(false, Ordering::AcqRel) || !REQUESTS.lock().is_empty()
        });
    }
}
