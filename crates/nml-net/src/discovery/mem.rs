//! In-process discovery over [`MemNet`] (for tests and `UI_GOLDEN`).
//!
//! The same datagrams as over UDP (encoded and checked), with played time: arrival after the delay of the sending
//! [`crate::transport::MemTransport`]. The broadcast reaches all beacons of the same `MemNet`.

use super::{
    Announcement, DISCOVERY_PORT, Datagram, announce_bytes, announcements_in, encode_datagram,
    queries_in,
};
use crate::link::mem::MemNet;
use crate::transport::{Beacon, Seeker};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// In-process beacon on `<ip>:2303`.
pub(crate) struct MemBeacon {
    net: MemNet,
    addr: SocketAddr,
    delay_ms: u64,
}

impl MemBeacon {
    /// Occupies `<ip>:2303`; already occupied -> `AddrInUse`.
    pub(crate) fn bind(net: &MemNet, ip: IpAddr, delay_ms: u64) -> std::io::Result<Self> {
        let addr = net.bind_socket(SocketAddr::new(ip, DISCOVERY_PORT))?;
        Ok(Self {
            net: net.clone(),
            addr,
            delay_ms,
        })
    }
}

impl Beacon for MemBeacon {
    fn poll_queries(&mut self) -> Vec<(SocketAddr, u32)> {
        queries_in(self.net.recv_datagrams(self.addr))
    }

    fn answer(&mut self, to: SocketAddr, a: &Announcement) {
        if let Some(bytes) = announce_bytes(a) {
            self.net.send_datagram(self.addr, to, &bytes, self.delay_ms);
        }
    }
}

impl Drop for MemBeacon {
    fn drop(&mut self) {
        self.net.unbind_socket(self.addr);
    }
}

/// In-process seeker on an ephemeral port; queries by broadcast to `255.255.255.255:2303`.
pub(crate) struct MemSeeker {
    net: MemNet,
    addr: SocketAddr,
    delay_ms: u64,
}

impl MemSeeker {
    pub(crate) fn bind(net: &MemNet, ip: IpAddr, delay_ms: u64) -> std::io::Result<Self> {
        let addr = net.bind_socket(SocketAddr::new(ip, 0))?;
        Ok(Self {
            net: net.clone(),
            addr,
            delay_ms,
        })
    }
}

impl Seeker for MemSeeker {
    fn query(&mut self, nonce: u32) {
        if let Some(bytes) = encode_datagram(&Datagram::Query { nonce }) {
            let to = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DISCOVERY_PORT);
            self.net.send_datagram(self.addr, to, &bytes, self.delay_ms);
        }
    }

    fn poll(&mut self) -> Vec<(SocketAddr, Announcement)> {
        announcements_in(self.net.recv_datagrams(self.addr))
    }
}

impl Drop for MemSeeker {
    fn drop(&mut self) {
        self.net.unbind_socket(self.addr);
    }
}
