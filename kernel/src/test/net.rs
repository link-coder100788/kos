//! std::net over the network task. Only what QEMU's user-mode network
//! answers by itself, so `make test` doesn't need the internet.
//!
//! `make test` forwards this computer's port HOST_PORT to KOS's
//! LISTEN_PORT, so connecting to 10.0.2.2:HOST_PORT (this computer, as
//! QEMU shows it to KOS) comes back in to a listener here.

use core::time::Duration;
use crate::arch::timer::uptime_ms;
use crate::scheduler::wait::sleep_ms;
use crate::std::io::{ErrorKind, Read, Write};
use crate::std::net::{SocketAddr, SocketAddrV4, Ipv4Addr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket, UDP_MAX_DATAGRAM};

/// The Makefile's TEST_PORT and TEST_HOST_PORT.
const LISTEN_PORT: u16 = 7321;
const HOST_PORT: u16 = 47321;

fn addr(a: [u8; 4], port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(a), port))
}

fn test_addresses_parse() {
    let mut it = "10.0.2.2:80".to_socket_addrs().unwrap();
    assert_eq!(it.next(), Some(addr([10, 0, 2, 2], 80)));
    assert_eq!(("1.2.3.4", 5).to_socket_addrs().unwrap().next(), Some(addr([1, 2, 3, 4], 5)));
    assert_eq!("no port".to_socket_addrs().unwrap_err().kind(), ErrorKind::InvalidInput);
}

fn test_udp_bind_picks_a_port() {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    let SocketAddr::V4(local) = socket.local_addr().unwrap() else { panic!() };
    assert_ne!(local.port(), 0);
    assert_eq!(*local.ip(), Ipv4Addr::new(10, 0, 2, 15));
}

fn test_udp_port_in_use() {
    let first = UdpSocket::bind("0.0.0.0:7001").unwrap();
    assert_eq!(UdpSocket::bind("0.0.0.0:7001").err().unwrap().kind(), ErrorKind::AddrInUse);
    drop(first);
    // Closing gives the port back (once the network task has seen it).
    sleep_ms(100);
    UdpSocket::bind("0.0.0.0:7001").unwrap();
}

fn test_udp_dhcp_port_refused() {
    assert_eq!(UdpSocket::bind("0.0.0.0:68").err().unwrap().kind(), ErrorKind::InvalidInput);
}

fn test_udp_read_timeout_and_nonblocking() {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    let mut buf = [0u8; 16];
    socket.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let start = uptime_ms();
    assert_eq!(socket.recv_from(&mut buf).unwrap_err().kind(), ErrorKind::TimedOut);
    assert!(uptime_ms() - start >= 90);
    socket.set_nonblocking(true).unwrap();
    assert_eq!(socket.recv_from(&mut buf).unwrap_err().kind(), ErrorKind::WouldBlock);
}

fn test_udp_send_errors() {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    let big = [0u8; UDP_MAX_DATAGRAM + 1];
    assert_eq!(socket.send_to(&big, "10.0.2.2:9").unwrap_err().kind(), ErrorKind::InvalidInput);
    assert_eq!(socket.send(b"x").unwrap_err().kind(), ErrorKind::NotConnected);
    assert_eq!(socket.send_to(b"x", "10.0.2.2:9").unwrap(), 1);
    socket.connect("10.0.2.2:9").unwrap();
    assert_eq!(socket.peer_addr().unwrap(), addr([10, 0, 2, 2], 9));
    assert_eq!(socket.send(b"xy").unwrap(), 2);
}

fn test_tcp_refused() {
    // QEMU passes the connection on to this computer's port 1, where
    // nothing listens.
    assert!(TcpStream::connect("10.0.2.2:1").is_err());
}

fn test_tcp_listen_ports() {
    let any = TcpListener::bind("0.0.0.0:0").unwrap();
    let SocketAddr::V4(local) = any.local_addr().unwrap() else { panic!() };
    assert_ne!(local.port(), 0);
    let first = TcpListener::bind("0.0.0.0:7002").unwrap();
    assert_eq!(TcpListener::bind("0.0.0.0:7002").err().unwrap().kind(), ErrorKind::AddrInUse);
    drop(first);
    sleep_ms(100);
    TcpListener::bind("0.0.0.0:7002").unwrap();
}

fn test_tcp_accept_nonblocking_and_timeout() {
    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().err().unwrap().kind(), ErrorKind::WouldBlock);
    listener.set_nonblocking(false).unwrap();
    listener.set_accept_timeout(Some(Duration::from_millis(100))).unwrap();
    assert_eq!(listener.accept().err().unwrap().kind(), ErrorKind::TimedOut);
}

fn test_tcp_accept_round_trip() {
    let listener = TcpListener::bind(("0.0.0.0", LISTEN_PORT)).unwrap();
    listener.set_accept_timeout(Some(Duration::from_secs(5))).unwrap();
    // Two clients at once, each to its own accepted connection.
    let mut clients = [
        TcpStream::connect(("10.0.2.2", HOST_PORT)).unwrap(),
        TcpStream::connect(("10.0.2.2", HOST_PORT)).unwrap(),
    ];
    let mut servers = [listener.accept().unwrap().0, listener.accept().unwrap().0];
    for (i, (client, server)) in clients.iter_mut().zip(servers.iter_mut()).enumerate() {
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        // QEMU makes the connection, so it comes from this computer.
        let SocketAddr::V4(peer) = server.peer_addr().unwrap() else { panic!() };
        assert_eq!(*peer.ip(), Ipv4Addr::new(10, 0, 2, 2));
        assert_eq!(server.local_addr().unwrap().port(), LISTEN_PORT);
        let ping = [b'p', b'0' + i as u8];
        client.write_all(&ping).unwrap();
        let mut got = [0u8; 2];
        server.read_exact(&mut got).unwrap();
        assert_eq!(got, ping);
        server.write_all(b"pong").unwrap();
        let mut got = [0u8; 4];
        client.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"pong");
    }
    // Closing the server's end ends the client's reads.
    drop(servers);
    let mut rest = alloc::vec::Vec::new();
    clients[0].read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty());
}

pub fn run() {
    // Sockets need an address from DHCP.
    let start = uptime_ms();
    while crate::net::status().address.is_none() {
        assert!(uptime_ms() - start < 15_000, "no address from DHCP");
        sleep_ms(50);
    }
    crate::test::run_all(&[
        &test_addresses_parse,
        &test_udp_bind_picks_a_port,
        &test_udp_port_in_use,
        &test_udp_dhcp_port_refused,
        &test_udp_read_timeout_and_nonblocking,
        &test_udp_send_errors,
        &test_tcp_refused,
        &test_tcp_listen_ports,
        &test_tcp_accept_nonblocking_and_timeout,
        &test_tcp_accept_round_trip,
    ]);
}
