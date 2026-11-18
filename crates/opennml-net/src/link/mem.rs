//! Connections in the same process with played time (for tests of the turn core).
//!
//! Without threads and without a clock: [`MemNet`] holds the played time and all wires. A message arrives as soon as
//! the played time reaches `send time + delay_ms`, in send order. [`MemNet::advance_to`] generates heartbeats itself
//! (per side, if nothing went out for `HEARTBEAT_MS` of played time), unless the side is muted.
//! Large contents go onto the wire whole at once (no bandwidth); they arrive as pieces and are reassembled by the
//! `Assembler` as with TCP.
//!
//! For [`crate::transport::MemTransport`] `MemNet` also keeps listeners (address -> accepted wires not yet collected)
//! and datagram endpoints (address -> waiting datagrams). A datagram to `255.255.255.255:<port>` goes to all other
//! endpoints of this port in the same `MemNet` (broadcast of the discovery), otherwise only to the exact address;
//! without a receiver it is lost, as with UDP.

use super::tcp::HEARTBEAT_MS;
use super::{Link, LinkEvent, PEER_CLOSED, deliver};
use crate::msg::NetMessage;
use crate::wire::{Assembler, split};
use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

/// Reason in `Closed` after [`MemLink::cut`].
pub const CUT_REASON: &str = "Connection interrupted";

/// Something on the wire toward one side.
#[derive(Debug)]
enum Item {
    Msg(NetMessage),
    Closed(String),
}

/// One side of a wire: what is on its way to it and what it has sent.
#[derive(Debug, Default)]
struct End {
    /// On its way to this side: (arrival time, content), sorted by arrival.
    inbox: VecDeque<(u64, Item)>,
    assembler: Assembler,
    /// Played time of the last processed reception.
    last_rx: u64,
    /// Played time of the last send (also heartbeats).
    last_sent: u64,
    muted: bool,
    /// Closed or `Closed` reported: delivers nothing more, sends nothing more.
    gone: bool,
}

#[derive(Debug)]
struct Pipe {
    delay: u64,
    created: u64,
    cut: bool,
    ends: [End; 2],
    /// Machine addresses of the two ends (`None`: via `MemNet::pair` without an address); side 0 = listener, side 1 =
    /// caller.
    ips: [Option<IpAddr>; 2],
}

impl Pipe {
    /// Cuts the wire (details at [`MemLink::cut`]); already cut: nothing.
    fn cut_now(&mut self, now: u64) {
        if self.cut {
            return;
        }
        self.cut = true;
        for end in &mut self.ends {
            end.inbox.clear();
            if !end.gone {
                end.inbox.push_back((now, Item::Closed(CUT_REASON.into())));
            }
        }
    }
}

/// A datagram on its way to an endpoint: (arrival time, sender, bytes).
type QueuedDatagram = (u64, SocketAddr, Vec<u8>);

/// First port assigned for port 0 (like the ephemeral ports of the systems).
const EPHEMERAL_FIRST: u16 = 49_152;

#[derive(Debug, Default)]
struct Inner {
    now: u64,
    pipes: Vec<Pipe>,
    /// Listeners: address -> accepted wires (arrival time, wire; side 0 = host), not yet collected.
    listeners: BTreeMap<SocketAddr, VecDeque<(u64, usize)>>,
    /// Datagram endpoints: address -> waiting datagrams.
    sockets: BTreeMap<SocketAddr, Vec<QueuedDatagram>>,
    /// Assigned machine addresses (`10.0.x.y`).
    hosts: u32,
    /// Last assigned ephemeral port (0: none yet).
    last_port: u16,
}

/// Shared played time and all wires in the process (`Clone` shares the same state).
#[derive(Clone, Default)]
pub struct MemNet {
    inner: Arc<Mutex<Inner>>,
}

/// One side of a wire in the process.
///
/// `last_received_ms` and `age_ms` count played time since `MemNet::pair`. `Drop` closes like `close`.
pub struct MemLink {
    net: MemNet,
    pipe: usize,
    side: usize,
}

impl Inner {
    /// Puts something from side `from` onto the wire (arrival `now + delay`) if the wire is open.
    fn transmit(&mut self, pipe: usize, from: usize, item: Item) {
        let now = self.now;
        let p = &mut self.pipes[pipe];
        let to = 1 - from;
        if p.cut || p.ends[from].gone || p.ends[from].muted || p.ends[to].gone {
            return;
        }
        p.ends[to]
            .inbox
            .push_back((now.saturating_add(p.delay), item));
        p.ends[from].last_sent = now;
    }

    /// New wire with a fixed delay per direction, created at the current played time.
    fn new_pipe(&mut self, delay_ms: u64) -> usize {
        let now = self.now;
        let end = || End {
            last_rx: now,
            last_sent: now,
            ..End::default()
        };
        self.pipes.push(Pipe {
            delay: delay_ms,
            created: now,
            cut: false,
            ends: [end(), end()],
            ips: [None, None],
        });
        self.pipes.len() - 1
    }

    /// Closes side `me` of the wire; the peer gets `Closed` after the delay.
    fn close_side(&mut self, pipe: usize, me: usize) {
        let now = self.now;
        let p = &mut self.pipes[pipe];
        let other = 1 - me;
        if p.ends[me].gone {
            return;
        }
        p.ends[me].gone = true;
        p.ends[me].inbox.clear();
        if !p.cut && !p.ends[other].gone {
            let due = now.saturating_add(p.delay);
            p.ends[other]
                .inbox
                .push_back((due, Item::Closed(PEER_CLOSED.into())));
        }
    }

    /// A free ephemeral port for `ip` (`used`: whether the address is already occupied).
    fn ephemeral_port(&mut self, ip: IpAddr, used: impl Fn(&SocketAddr) -> bool) -> u16 {
        loop {
            self.last_port = match self.last_port {
                p if p < EPHEMERAL_FIRST || p == u16::MAX => EPHEMERAL_FIRST,
                p => p + 1,
            };
            if !used(&SocketAddr::new(ip, self.last_port)) {
                return self.last_port;
            }
        }
    }
}

impl MemNet {
    pub fn new() -> Self {
        Self::default()
    }

    /// New wire with a fixed delay per direction (`delay_ms`), created at the current played time.
    pub fn pair(&self, delay_ms: u64) -> (MemLink, MemLink) {
        let pipe = lock(&self.inner).new_pipe(delay_ms);
        let link = |side| MemLink {
            net: self.clone(),
            pipe,
            side,
        };
        (link(0), link(1))
    }

    /// Cuts every wire that ends at `ip` (like [`MemLink::cut`]): the machine drops out of the network (tests of the
    /// dropout).
    pub fn cut_host(&self, ip: IpAddr) {
        let mut inner = lock(&self.inner);
        let now = inner.now;
        for p in &mut inner.pipes {
            if p.ips.contains(&Some(ip)) {
                p.cut_now(now);
            }
        }
    }

    /// Cuts the wires between exactly these two machines (like [`MemLink::cut`]).
    pub fn cut_between(&self, a: IpAddr, b: IpAddr) {
        let mut inner = lock(&self.inner);
        let now = inner.now;
        for p in &mut inner.pipes {
            if p.ips == [Some(a), Some(b)] || p.ips == [Some(b), Some(a)] {
                p.cut_now(now);
            }
        }
    }

    /// Sets the played time (backwards has no effect) and generates the due heartbeats: one per side if `HEARTBEAT_MS`
    /// passed since its last send, unless the side is muted.
    pub fn advance_to(&self, now_ms: u64) {
        let mut inner = lock(&self.inner);
        if now_ms <= inner.now {
            return;
        }
        inner.now = now_ms;
        for p in &mut inner.pipes {
            if p.cut {
                continue;
            }
            for from in 0..2 {
                let to = 1 - from;
                if p.ends[from].gone || p.ends[to].gone {
                    continue;
                }
                let last = p.ends[from].last_sent;
                let due = now_ms.saturating_sub(last) / HEARTBEAT_MS;
                if due == 0 {
                    continue;
                }
                if !p.ends[from].muted {
                    // Heartbeat `k` = 1 ... `due` at time `last + k·HEARTBEAT_MS`. One that has already arrived and is
                    // followed by another that has also already arrived changes nothing any more (it is never
                    // delivered): left out, so that a large time jump does not create thousands of entries. The send
                    // times lie after everything already on its way; the sequence stays sorted by arrival.
                    let first =
                        (now_ms.saturating_sub(last.saturating_add(p.delay)) / HEARTBEAT_MS).max(1);
                    for k in first..=due {
                        let at = last + k * HEARTBEAT_MS;
                        p.ends[to]
                            .inbox
                            .push_back((at.saturating_add(p.delay), Item::Msg(NetMessage::Ping)));
                    }
                }
                p.ends[from].last_sent = last + due * HEARTBEAT_MS;
            }
        }
    }

    /// Current played time.
    pub fn now_ms(&self) -> u64 {
        lock(&self.inner).now
    }

    /// New machine address `10.0.x.y` (one per `MemTransport`).
    pub(crate) fn new_host_ip(&self) -> IpAddr {
        let mut inner = lock(&self.inner);
        inner.hosts += 1;
        let n = inner.hosts;
        IpAddr::V4(Ipv4Addr::new(10, 0, (n >> 8) as u8, n as u8))
    }

    /// Registers a listener; port 0 assigns a free port. Occupied -> `AddrInUse`.
    pub(crate) fn bind_listener(&self, addr: SocketAddr) -> std::io::Result<SocketAddr> {
        let mut inner = lock(&self.inner);
        let mut addr = addr;
        if addr.port() == 0 {
            let used: Vec<SocketAddr> = inner.listeners.keys().copied().collect();
            addr.set_port(inner.ephemeral_port(addr.ip(), |a| used.contains(a)));
        } else if inner.listeners.contains_key(&addr) {
            return Err(in_use(addr));
        }
        inner.listeners.insert(addr, VecDeque::new());
        Ok(addr)
    }

    /// Unregisters a listener; wires not collected are closed (the peer gets `Closed`).
    pub(crate) fn unbind_listener(&self, addr: SocketAddr) {
        let mut inner = lock(&self.inner);
        if let Some(pending) = inner.listeners.remove(&addr) {
            for (_, pipe) in pending {
                inner.close_side(pipe, 0);
            }
        }
    }

    /// Arrived connections of a listener (host side), in arrival order.
    pub(crate) fn accept(&self, addr: SocketAddr) -> Vec<MemLink> {
        let mut inner = lock(&self.inner);
        let now = inner.now;
        let Some(pending) = inner.listeners.get_mut(&addr) else {
            return Vec::new();
        };
        let mut pipes = Vec::new();
        while pending.front().is_some_and(|(due, _)| *due <= now) {
            if let Some((_, pipe)) = pending.pop_front() {
                pipes.push(pipe);
            }
        }
        drop(inner);
        pipes
            .into_iter()
            .map(|pipe| MemLink {
                net: self.clone(),
                pipe,
                side: 0,
            })
            .collect()
    }

    /// Connects to the listener at `addr` if there is one: new wire, the host's side arrives at the listener after
    /// `delay_ms`, the client's side is returned. `from` is the caller's address; afterwards both sides know the
    /// address of the peer ([`Link::peer_ip`]).
    pub(crate) fn dial(&self, addr: SocketAddr, delay_ms: u64, from: IpAddr) -> Option<MemLink> {
        let mut inner = lock(&self.inner);
        if !inner.listeners.contains_key(&addr) {
            return None;
        }
        let pipe = inner.new_pipe(delay_ms);
        inner.pipes[pipe].ips = [Some(addr.ip()), Some(from)];
        let due = inner.now.saturating_add(delay_ms);
        inner.listeners.get_mut(&addr)?.push_back((due, pipe));
        Some(MemLink {
            net: self.clone(),
            pipe,
            side: 1,
        })
    }

    /// Registers a datagram endpoint; port 0 assigns a free port. Occupied -> `AddrInUse`.
    pub(crate) fn bind_socket(&self, addr: SocketAddr) -> std::io::Result<SocketAddr> {
        let mut inner = lock(&self.inner);
        let mut addr = addr;
        if addr.port() == 0 {
            let used: Vec<SocketAddr> = inner.sockets.keys().copied().collect();
            addr.set_port(inner.ephemeral_port(addr.ip(), |a| used.contains(a)));
        } else if inner.sockets.contains_key(&addr) {
            return Err(in_use(addr));
        }
        inner.sockets.insert(addr, Vec::new());
        Ok(addr)
    }

    /// Unregisters a datagram endpoint; anything waiting is lost.
    pub(crate) fn unbind_socket(&self, addr: SocketAddr) {
        lock(&self.inner).sockets.remove(&addr);
    }

    /// Sends a datagram (arrival after `delay_ms`): to `255.255.255.255:<port>` to all other endpoints of this port (in
    /// address order), otherwise to the exact address; lost without a receiver.
    pub(crate) fn send_datagram(
        &self,
        from: SocketAddr,
        to: SocketAddr,
        bytes: &[u8],
        delay_ms: u64,
    ) {
        let mut inner = lock(&self.inner);
        let due = inner.now.saturating_add(delay_ms);
        let broadcast = to.ip() == IpAddr::V4(Ipv4Addr::BROADCAST);
        for (addr, queue) in inner.sockets.iter_mut() {
            let hit = if broadcast {
                addr.port() == to.port() && *addr != from
            } else {
                *addr == to
            };
            if hit {
                queue.push((due, from, bytes.to_vec()));
            }
        }
    }

    /// Arrived datagrams of an endpoint (sender, bytes), in send order.
    pub(crate) fn recv_datagrams(&self, at: SocketAddr) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut inner = lock(&self.inner);
        let now = inner.now;
        let Some(queue) = inner.sockets.get_mut(&at) else {
            return Vec::new();
        };
        let (due, later): (Vec<QueuedDatagram>, Vec<QueuedDatagram>) = std::mem::take(queue)
            .into_iter()
            .partition(|(t, _, _)| *t <= now);
        *queue = later;
        due.into_iter()
            .map(|(_, from, bytes)| (from, bytes))
            .collect()
    }
}

/// Error "address in use" in the process.
fn in_use(addr: SocketAddr) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        format!("address {addr} is already in use (in-process)"),
    )
}

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MemLink {
    /// Cuts the wire: what is on its way is lost, both sides report `Closed` (`CUT_REASON`) exactly once at the next
    /// `poll`, after that nothing passes any more.
    pub fn cut(&self) {
        let mut inner = lock(&self.net.inner);
        let now = inner.now;
        inner.pipes[self.pipe].cut_now(now);
    }

    /// Mutes this side (`true`): it sends nothing any more, not even heartbeats (for timeout tests). A `close` still
    /// reaches the peer.
    pub fn mute(&self, on: bool) {
        lock(&self.net.inner).pipes[self.pipe].ends[self.side].muted = on;
    }
}

impl Link for MemLink {
    fn send(&mut self, msg: NetMessage) {
        lock(&self.net.inner).transmit(self.pipe, self.side, Item::Msg(msg));
    }

    fn send_large(&mut self, id: u32, bytes: Vec<u8>) {
        let mut inner = lock(&self.net.inner);
        for chunk in split(id, &bytes) {
            inner.transmit(self.pipe, self.side, Item::Msg(chunk));
        }
    }

    fn poll(&mut self) -> Vec<LinkEvent> {
        let mut inner = lock(&self.net.inner);
        let now = inner.now;
        let end = &mut inner.pipes[self.pipe].ends[self.side];
        let mut out = Vec::new();
        if end.gone {
            return out;
        }
        while end.inbox.front().is_some_and(|(due, _)| *due <= now) {
            let Some((due, item)) = end.inbox.pop_front() else {
                break;
            };
            let closed = match item {
                Item::Closed(reason) => Some(reason),
                Item::Msg(msg) => {
                    end.last_rx = end.last_rx.max(due);
                    match deliver(Some(&mut end.assembler), msg) {
                        Ok(Some(ev)) => {
                            out.push(ev);
                            None
                        }
                        Ok(None) => None,
                        Err(e) => {
                            let reason = format!("Connection lost: {e}");
                            log::warn!("Network (in-process): {reason}");
                            Some(reason)
                        }
                    }
                }
            };
            if let Some(reason) = closed {
                out.push(LinkEvent::Closed(reason));
                end.gone = true;
                end.inbox.clear();
                break;
            }
        }
        out
    }

    fn last_received_ms(&self) -> u64 {
        let inner = lock(&self.net.inner);
        let p = &inner.pipes[self.pipe];
        let end = &p.ends[self.side];
        // Arrived counts even without polling (like the read thread with TCP).
        let arrived = end
            .inbox
            .iter()
            .take_while(|(due, _)| *due <= inner.now)
            .filter(|(_, item)| matches!(item, Item::Msg(_)))
            .map(|(due, _)| *due)
            .max()
            .unwrap_or(0);
        end.last_rx.max(arrived) - p.created
    }

    fn age_ms(&self) -> u64 {
        let inner = lock(&self.net.inner);
        inner.now - inner.pipes[self.pipe].created
    }

    fn close(&mut self, reason: &str) {
        let mut inner = lock(&self.net.inner);
        if inner.pipes[self.pipe].ends[self.side].gone {
            return;
        }
        log::debug!("Network (in-process): connection closed: {reason}");
        inner.close_side(self.pipe, self.side);
    }

    fn peer_ip(&self) -> Option<IpAddr> {
        lock(&self.net.inner).pipes[self.pipe].ips[1 - self.side]
    }

    fn mute(&mut self, on: bool) {
        // Call the inherent method explicitly (otherwise recursion).
        MemLink::mute(self, on);
    }
}

impl Drop for MemLink {
    fn drop(&mut self) {
        self.close("Connection released");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::CHUNK_SIZE;

    fn msg(slot: u8) -> NetMessage {
        NetMessage::Welcome { slot }
    }

    fn closed_count(evs: &[LinkEvent]) -> usize {
        evs.iter()
            .filter(|e| matches!(e, LinkEvent::Closed(_)))
            .count()
    }

    // QUIRK E-271: deviation
    #[test]
    fn mem_link_delays_and_orders() {
        let net = MemNet::new();
        let (mut a, mut b) = net.pair(120);
        a.send(msg(1));
        a.send(msg(2));
        b.send(msg(9));
        net.advance_to(119);
        assert!(b.poll().is_empty());
        assert!(a.poll().is_empty());
        net.advance_to(120);
        assert_eq!(
            b.poll(),
            vec![LinkEvent::Message(msg(1)), LinkEvent::Message(msg(2))]
        );
        assert_eq!(a.poll(), vec![LinkEvent::Message(msg(9))]);
        assert!(b.poll().is_empty());
        assert_eq!(b.last_received_ms(), 120);
        assert_eq!(b.age_ms(), 120);
    }

    #[test]
    fn mem_cut_reports_closed() {
        let net = MemNet::new();
        let (mut a, mut b) = net.pair(50);
        a.send(msg(1));
        net.advance_to(10);
        a.cut();
        // What is on its way is lost; both report Closed exactly once at once.
        assert_eq!(a.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        assert_eq!(b.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        a.send(msg(2));
        b.send(msg(3));
        net.advance_to(10_000);
        assert!(a.poll().is_empty());
        assert!(b.poll().is_empty());
        // A drop afterwards reports nothing further.
        drop(a);
        assert!(b.poll().is_empty());
    }

    #[test]
    fn mem_close_and_drop_reach_the_peer_after_the_delay() {
        let net = MemNet::new();
        let (mut a, mut b) = net.pair(30);
        a.send(NetMessage::Bye {
            reason: "gone".into(),
        });
        a.close("Test");
        assert!(a.poll().is_empty());
        net.advance_to(29);
        assert!(b.poll().is_empty());
        net.advance_to(30);
        let evs = b.poll();
        assert_eq!(
            evs,
            vec![
                // Bye is an ordinary message; Closed follows.
                LinkEvent::Message(NetMessage::Bye {
                    reason: "gone".into()
                }),
                LinkEvent::Closed(PEER_CLOSED.into()),
            ]
        );
        net.advance_to(5_000);
        assert!(b.poll().is_empty());

        let (c, mut d) = net.pair(0);
        drop(c);
        let evs = d.poll();
        assert_eq!(closed_count(&evs), 1, "{evs:?}");
        assert!(d.poll().is_empty());
    }

    #[test]
    fn mem_heartbeats_and_mute() {
        let net = MemNet::new();
        let (a, mut b) = net.pair(20);
        // Heartbeats without polling: one per HEARTBEAT_MS, there after 20 ms.
        net.advance_to(5 * HEARTBEAT_MS + 19);
        assert_eq!(b.last_received_ms(), 4 * HEARTBEAT_MS + 20);
        net.advance_to(5 * HEARTBEAT_MS + 20);
        assert_eq!(b.last_received_ms(), 5 * HEARTBEAT_MS + 20);
        assert!(b.poll().is_empty(), "Ping is not delivered");
        // Muted: nothing more, no heartbeat either.
        let mut a = a;
        a.mute(true);
        a.send(msg(1));
        net.advance_to(40 * HEARTBEAT_MS);
        assert!(b.poll().is_empty());
        assert_eq!(b.last_received_ms(), 5 * HEARTBEAT_MS + 20);
        // Unmuted again: the next heartbeat on the grid, no catching up.
        a.mute(false);
        net.advance_to(41 * HEARTBEAT_MS + 20);
        assert_eq!(b.last_received_ms(), 41 * HEARTBEAT_MS + 20);
        // Sending resets the counter.
        a.send(msg(2));
        net.advance_to(41 * HEARTBEAT_MS + 500);
        assert_eq!(b.poll(), vec![LinkEvent::Message(msg(2))]);
        assert_eq!(b.last_received_ms(), 41 * HEARTBEAT_MS + 40);
        net.advance_to(42 * HEARTBEAT_MS + 39);
        assert_eq!(b.last_received_ms(), 41 * HEARTBEAT_MS + 40);
        net.advance_to(42 * HEARTBEAT_MS + 40);
        assert_eq!(b.last_received_ms(), 42 * HEARTBEAT_MS + 40);
    }

    #[test]
    fn mem_large_is_assembled() {
        let net = MemNet::new();
        let (mut a, mut b) = net.pair(10);
        let bytes: Vec<u8> = (0..(2 * CHUNK_SIZE + 3)).map(|i| (i % 253) as u8).collect();
        a.send_large(4, bytes.clone());
        a.send(msg(7));
        net.advance_to(10);
        assert_eq!(
            b.poll(),
            vec![
                LinkEvent::Large { id: 4, bytes },
                LinkEvent::Message(msg(7))
            ]
        );
    }

    #[test]
    fn mem_pairs_keep_their_own_clock() {
        let net = MemNet::new();
        net.advance_to(1_000);
        let (mut a, b) = net.pair(0);
        assert_eq!(b.age_ms(), 0);
        assert_eq!(b.last_received_ms(), 0);
        net.advance_to(1_100);
        a.send(msg(1));
        net.advance_to(1_250);
        assert_eq!(b.age_ms(), 250);
        assert_eq!(b.last_received_ms(), 100);
        // Backwards has no effect.
        net.advance_to(10);
        assert_eq!(net.now_ms(), 1_250);
    }

    #[test]
    fn pair_links_have_no_peer_ip() {
        let net = MemNet::new();
        let (a, b) = net.pair(10);
        assert_eq!((a.peer_ip(), b.peer_ip()), (None, None));
    }

    /// Three machines A, B, C: A listens, B and C call. Returns the wires A-B, A-C (A's side) and B-A, C-A.
    fn three_hosts() -> (MemNet, [IpAddr; 3], [MemLink; 4]) {
        let net = MemNet::new();
        let ips = [net.new_host_ip(), net.new_host_ip(), net.new_host_ip()];
        let at = net.bind_listener(SocketAddr::new(ips[0], 0)).unwrap();
        let b = net.dial(at, 0, ips[1]).unwrap();
        let c = net.dial(at, 0, ips[2]).unwrap();
        let mut accepted = net.accept(at);
        assert_eq!(accepted.len(), 2);
        let ac = accepted.pop().unwrap();
        let ab = accepted.pop().unwrap();
        (net, ips, [ab, ac, b, c])
    }

    #[test]
    fn cut_host_cuts_every_pipe_of_that_host() {
        let (net, ips, [mut ab, mut ac, mut b, mut c]) = three_hosts();
        assert_eq!(ab.peer_ip(), Some(ips[1]));
        assert_eq!(b.peer_ip(), Some(ips[0]));
        net.cut_host(ips[1]);
        assert_eq!(ab.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        assert_eq!(b.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        // The wire between A and C stays.
        ac.send(msg(1));
        net.advance_to(10);
        assert_eq!(c.poll(), vec![LinkEvent::Message(msg(1))]);
        assert!(ac.poll().is_empty());
        // Machine A itself cuts everything.
        net.cut_host(ips[0]);
        assert_eq!(ac.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        assert_eq!(c.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
    }

    #[test]
    fn cut_between_cuts_only_those_two() {
        let (net, ips, [mut ab, mut ac, mut b, mut c]) = three_hosts();
        // B and C have no common wire: nothing happens.
        net.cut_between(ips[1], ips[2]);
        assert!(ab.poll().is_empty() && ac.poll().is_empty());
        assert!(b.poll().is_empty() && c.poll().is_empty());
        // A and B, in either order of the arguments.
        net.cut_between(ips[1], ips[0]);
        assert_eq!(ab.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        assert_eq!(b.poll(), vec![LinkEvent::Closed(CUT_REASON.into())]);
        assert!(ac.poll().is_empty() && c.poll().is_empty());
    }

    #[test]
    fn link_trait_mute_silences_one_side() {
        let net = MemNet::new();
        let (mut a, mut b) = net.pair(10);
        Link::mute(&mut a, true);
        a.send(msg(1));
        b.send(msg(2));
        net.advance_to(10);
        assert!(b.poll().is_empty());
        assert_eq!(a.poll(), vec![LinkEvent::Message(msg(2))]);
        Link::mute(&mut a, false);
        a.send(msg(3));
        net.advance_to(20);
        assert_eq!(b.poll(), vec![LinkEvent::Message(msg(3))]);
    }
}
