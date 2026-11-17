//! Discovery over UDP with `std::net`.
//!
//! Non-blocking sockets, read once per frame, no thread. The host's beacon listens on `<ip>:<discovery port>` (0.0.0.0,
//! with `--net-loopback` 127.0.0.1); the seeker sits on an ephemeral port of all interfaces with `SO_BROADCAST` and
//! queries `255.255.255.255:<discovery port>`, with `loopback` also `127.0.0.1:<discovery port>`: on Windows a socket
//! bound to 127.0.0.1 does not receive a broadcast to 255.255.255.255.

use super::{Announcement, Datagram, MAX_DATAGRAM, announce_bytes, announcements_in, queries_in};
use crate::transport::{Beacon, Seeker};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// At most this many datagrams per poll (against a flood; the rest waits until the next frame).
const MAX_PER_POLL: usize = 256;

/// `WSAEMSGSIZE`: on Windows `recv_from` reports a datagram above the buffer size as an error (and discards it);
/// other systems truncate silently, and then the length check of `decode_datagram` catches it.
#[cfg(windows)]
const MSG_SIZE_ERROR: i32 = 10_040;
#[cfg(not(windows))]
const MSG_SIZE_ERROR: i32 = i32::MIN;

/// Reads all waiting datagrams without blocking.
///
/// An oversized datagram is read with one byte more than [`MAX_DATAGRAM`] and then rejected. `ConnectionReset`
/// (Windows reports the ICMP response "port unreachable" to an earlier `send_to` this way) and oversized datagrams
/// are ignored, any other error ends the polling for this frame.
fn receive(socket: &UdpSocket) -> Vec<(SocketAddr, Vec<u8>)> {
    let mut buf = [0u8; MAX_DATAGRAM + 1];
    let mut out = Vec::new();
    for _ in 0..MAX_PER_POLL {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => out.push((from, buf[..n].to_vec())),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::Interrupted
                ) || e.raw_os_error() == Some(MSG_SIZE_ERROR) =>
            {
                log::debug!("Network (discovery): reception ignored: {e}");
            }
            Err(e) => {
                log::debug!("Network (discovery): reception failed: {e}");
                break;
            }
        }
    }
    out
}

/// The host's beacon: answers discovery queries.
pub(crate) struct UdpBeacon {
    socket: UdpSocket,
}

impl UdpBeacon {
    /// Listens on `addr`; occupied (e.g. a second host on the same machine) -> `Err`, and the game host then opens the
    /// lobby without an announcement.
    pub(crate) fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        log::info!("Network (discovery): beacon listening on {addr}");
        Ok(Self { socket })
    }
}

impl Beacon for UdpBeacon {
    fn poll_queries(&mut self) -> Vec<(SocketAddr, u32)> {
        queries_in(receive(&self.socket))
    }

    fn answer(&mut self, to: SocketAddr, a: &Announcement) {
        let Some(bytes) = announce_bytes(a) else {
            return;
        };
        if let Err(e) = self.socket.send_to(&bytes, to) {
            log::debug!("Network (discovery): announcement to {to} not sent: {e}");
        }
    }
}

/// The client's seeker: queries by broadcast and collects announcements.
pub(crate) struct UdpSeeker {
    socket: UdpSocket,
    port: u16,
    loopback: bool,
}

impl UdpSeeker {
    /// Ephemeral port of all interfaces with `SO_BROADCAST`; queries `port`, with `loopback` also on 127.0.0.1.
    pub(crate) fn bind(port: u16, loopback: bool) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))?;
        socket.set_broadcast(true)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            port,
            loopback,
        })
    }
}

impl Seeker for UdpSeeker {
    fn query(&mut self, nonce: u32) {
        let Some(bytes) = super::encode_datagram(&Datagram::Query { nonce }) else {
            return;
        };
        let mut targets = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), self.port)];
        if self.loopback {
            targets.push(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port));
        }
        for to in targets {
            // Without a network (no broadcast possible) only the error goes to the debug log; the discovery continues.
            if let Err(e) = self.socket.send_to(&bytes, to) {
                log::debug!("Network (discovery): query to {to} not sent: {e}");
            }
        }
    }

    fn poll(&mut self) -> Vec<(SocketAddr, Announcement)> {
        announcements_in(receive(&self.socket))
    }
}
