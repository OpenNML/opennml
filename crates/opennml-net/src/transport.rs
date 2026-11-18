//! Transport: listening, non-blocking connecting and LAN discovery behind one interface.
//!
//! The client's network session (`opennml_client::net_session`) gets a `Box<dyn Transport>` from the program and calls
//! everything once per frame without blocking: [`SystemTransport`] over `std::net` (`opennml-game`), [`MemTransport`]
//! in-process over [`MemNet`] with played time (tests, `UI_GOLDEN`; the tests call `MemNet::advance_to` themselves).
//!
//! The original connects over DirectPlay synchronously in the main thread (E-254); here [`SystemTransport`] connects in
//! a thread with retries as `--join` did before (`apps/opennml-game/src/net_start.rs::join`), and the window loop only
//! asks [`Connecting::poll`].

use crate::discovery::mem::{MemBeacon, MemSeeker};
use crate::discovery::udp::{UdpBeacon, UdpSeeker};
use crate::discovery::{Announcement, DISCOVERY_PORT};
use crate::link::Link;
use crate::link::mem::{MemLink, MemNet};
use crate::link::tcp::{TcpHost, TcpLink};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Pause between two connection attempts of [`SystemTransport::connect`].
pub const CONNECT_RETRY_MS: u64 = 100;
/// Longest single connection attempt of [`SystemTransport::connect`].
pub const CONNECT_ATTEMPT_MS: u64 = 1_000;

/// Listens for incoming connections.
pub trait Listener: Send {
    /// All connections accepted since the last call; does not block.
    fn accept_pending(&mut self) -> Vec<Box<dyn Link>>;
    /// Actual port (also for `listen(0)`).
    fn port(&self) -> u16;
}

/// A running connection setup.
pub trait Connecting: Send {
    /// `None` while it runs; then exactly once `Some(Ok(connection))` or `Some(Err(...))` (after the deadline),
    /// afterwards `None` again. Does not block.
    fn poll(&mut self) -> Option<io::Result<Box<dyn Link>>>;
}

/// The host's beacon: collect and answer discovery queries. `Drop` releases the discovery port.
pub trait Beacon: Send {
    /// All waiting, valid discovery queries (sender, `nonce`); does not block.
    fn poll_queries(&mut self) -> Vec<(SocketAddr, u32)>;
    /// Sends an announcement to `to` (too large -> nothing, with a warning in the log).
    fn answer(&mut self, to: SocketAddr, a: &Announcement);
}

/// The client's seeker: send queries, collect announcements.
pub trait Seeker: Send {
    /// Sends a discovery query by broadcast.
    fn query(&mut self, nonce: u32);
    /// All waiting, valid and sanitized announcements (sender, announcement); does not block.
    fn poll(&mut self) -> Vec<(SocketAddr, Announcement)>;
}

/// Everything the network session needs from the network.
pub trait Transport: Send {
    /// Listens on `port` (0: free port).
    fn listen(&mut self, port: u16) -> io::Result<Box<dyn Listener>>;
    /// Begins connecting to `addr`; does not give up before `timeout_ms`.
    fn connect(&mut self, addr: SocketAddr, timeout_ms: u64) -> Box<dyn Connecting>;
    /// Occupies the discovery port for the host's announcements; occupied -> `Err` (the lobby opens anyway).
    fn beacon(&mut self) -> io::Result<Box<dyn Beacon>>;
    /// Opens a seeker.
    fn seeker(&mut self) -> io::Result<Box<dyn Seeker>>;
    /// A second transport of the same machine (same address, same network), which the `NetDriver` gets for the host
    /// migration.
    fn fork(&self) -> Box<dyn Transport>;
}

/// Transport over `std::net`.
///
/// `loopback`: the host listens (TCP and beacon) only on 127.0.0.1, the seeker also queries 127.0.0.1 (two instances on
/// one machine, `--net-loopback`). `discovery_port`: UDP port of the discovery, default [`DISCOVERY_PORT`]; tests use
/// their own free port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemTransport {
    pub loopback: bool,
    pub discovery_port: u16,
}

impl Default for SystemTransport {
    fn default() -> Self {
        Self::new(false)
    }
}

impl SystemTransport {
    /// Transport with the discovery port [`DISCOVERY_PORT`].
    pub fn new(loopback: bool) -> Self {
        Self {
            loopback,
            discovery_port: DISCOVERY_PORT,
        }
    }

    /// Address the host listens on: 127.0.0.1 with `loopback`, otherwise all interfaces.
    fn host_ip(&self) -> IpAddr {
        if self.loopback {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        }
    }
}

impl Transport for SystemTransport {
    fn listen(&mut self, port: u16) -> io::Result<Box<dyn Listener>> {
        Ok(Box::new(TcpHost::bind(SocketAddr::new(
            self.host_ip(),
            port,
        ))?))
    }

    fn connect(&mut self, addr: SocketAddr, timeout_ms: u64) -> Box<dyn Connecting> {
        Box::new(SystemConnecting::start(addr, timeout_ms))
    }

    fn beacon(&mut self) -> io::Result<Box<dyn Beacon>> {
        Ok(Box::new(UdpBeacon::bind(SocketAddr::new(
            self.host_ip(),
            self.discovery_port,
        ))?))
    }

    fn seeker(&mut self) -> io::Result<Box<dyn Seeker>> {
        Ok(Box::new(UdpSeeker::bind(
            self.discovery_port,
            self.loopback,
        )?))
    }

    fn fork(&self) -> Box<dyn Transport> {
        Box::new(*self)
    }
}

/// Connection setup over TCP in its own thread: attempts of at most [`CONNECT_ATTEMPT_MS`], with a pause of
/// [`CONNECT_RETRY_MS`] in between, until the deadline passes (like `net_start::join`). `Drop` aborts; a connection
/// that still comes about afterwards is closed again at once.
struct SystemConnecting {
    result: mpsc::Receiver<io::Result<TcpLink>>,
    cancel: Arc<AtomicBool>,
    done: bool,
}

impl SystemConnecting {
    fn start(addr: SocketAddr, timeout_ms: u64) -> Self {
        let (tx, result) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("opennml-net-connect".into())
            .spawn(move || {
                if let Some(r) = connect_with_retry(addr, timeout_ms, &stop) {
                    // If the receiver is already gone, the connection is closed with the failure of the send.
                    let _ = tx.send(r);
                }
            });
        if let Err(e) = spawned {
            // Without a thread the setup fails at once; the game host sees the error at the next `poll`.
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Err(e));
            return Self {
                result: rx,
                cancel,
                done: false,
            };
        }
        Self {
            result,
            cancel,
            done: false,
        }
    }
}

/// Tries to connect until `timeout_ms`; `None` after an abort.
fn connect_with_retry(
    addr: SocketAddr,
    timeout_ms: u64,
    cancel: &AtomicBool,
) -> Option<io::Result<TcpLink>> {
    let started = Instant::now();
    let elapsed = || started.elapsed().as_millis() as u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        let left = timeout_ms.saturating_sub(elapsed());
        match TcpLink::connect(addr, left.clamp(1, CONNECT_ATTEMPT_MS)) {
            Ok(link) => return Some(Ok(link)),
            Err(e) => {
                let left = timeout_ms.saturating_sub(elapsed());
                if left == 0 {
                    return Some(Err(io::Error::new(
                        e.kind(),
                        format!(
                            "No host reachable at {addr}, given up after {} s ({e})",
                            timeout_ms.div_ceil(1000)
                        ),
                    )));
                }
                log::debug!("Network: {addr} not reachable yet ({e}), trying again");
                std::thread::sleep(Duration::from_millis(left.min(CONNECT_RETRY_MS)));
            }
        }
    }
}

impl Connecting for SystemConnecting {
    fn poll(&mut self) -> Option<io::Result<Box<dyn Link>>> {
        if self.done {
            return None;
        }
        let r = match self.result.try_recv() {
            Ok(r) => r.map(|link| Box::new(link) as Box<dyn Link>),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(io::Error::other(
                "Connection setup aborted without a result",
            )),
        };
        self.done = true;
        Some(r)
    }
}

impl Drop for SystemConnecting {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Transport in-process over [`MemNet`]: every `MemTransport` is a machine with its own address `10.0.x.y`; wires and
/// datagrams arrive after `delay_ms` of played time (like [`MemNet::pair`]). The seeker's broadcast reaches all beacons
/// of the same `MemNet`.
#[derive(Clone)]
pub struct MemTransport {
    net: MemNet,
    delay_ms: u64,
    ip: IpAddr,
}

impl MemTransport {
    pub fn new(net: &MemNet, delay_ms: u64) -> Self {
        Self {
            net: net.clone(),
            delay_ms,
            ip: net.new_host_ip(),
        }
    }

    /// Address of this machine in the process.
    pub fn ip(&self) -> IpAddr {
        self.ip
    }
}

impl Transport for MemTransport {
    fn listen(&mut self, port: u16) -> io::Result<Box<dyn Listener>> {
        let addr = self.net.bind_listener(SocketAddr::new(self.ip, port))?;
        Ok(Box::new(MemListener {
            net: self.net.clone(),
            addr,
        }))
    }

    fn connect(&mut self, addr: SocketAddr, timeout_ms: u64) -> Box<dyn Connecting> {
        let now = self.net.now_ms();
        let mut c = MemConnecting {
            net: self.net.clone(),
            addr,
            from: self.ip,
            delay_ms: self.delay_ms,
            deadline: now.saturating_add(timeout_ms),
            ready_at: 0,
            link: None,
            done: false,
        };
        c.dial(now);
        Box::new(c)
    }

    fn beacon(&mut self) -> io::Result<Box<dyn Beacon>> {
        Ok(Box::new(MemBeacon::bind(
            &self.net,
            self.ip,
            self.delay_ms,
        )?))
    }

    fn seeker(&mut self) -> io::Result<Box<dyn Seeker>> {
        Ok(Box::new(MemSeeker::bind(
            &self.net,
            self.ip,
            self.delay_ms,
        )?))
    }

    fn fork(&self) -> Box<dyn Transport> {
        Box::new(self.clone())
    }
}

/// In-process listener; `Drop` unregisters, connections not collected are closed.
struct MemListener {
    net: MemNet,
    addr: SocketAddr,
}

impl Listener for MemListener {
    fn accept_pending(&mut self) -> Vec<Box<dyn Link>> {
        self.net
            .accept(self.addr)
            .into_iter()
            .map(|link| Box::new(link) as Box<dyn Link>)
            .collect()
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

impl Drop for MemListener {
    fn drop(&mut self) {
        self.net.unbind_listener(self.addr);
    }
}

/// In-process connection setup: asks the listener again with every `poll` (like the retry over TCP); it is connected
/// after `delay_ms` of played time, without a listener it fails at the deadline.
struct MemConnecting {
    net: MemNet,
    addr: SocketAddr,
    /// Address of the caller.
    from: IpAddr,
    delay_ms: u64,
    deadline: u64,
    ready_at: u64,
    link: Option<MemLink>,
    done: bool,
}

impl MemConnecting {
    fn dial(&mut self, now: u64) {
        if self.link.is_none()
            && let Some(link) = self.net.dial(self.addr, self.delay_ms, self.from)
        {
            self.link = Some(link);
            self.ready_at = now.saturating_add(self.delay_ms);
        }
    }
}

impl Connecting for MemConnecting {
    fn poll(&mut self) -> Option<io::Result<Box<dyn Link>>> {
        if self.done {
            return None;
        }
        let now = self.net.now_ms();
        self.dial(now);
        if self.link.is_some() && now >= self.ready_at {
            self.done = true;
            return self
                .link
                .take()
                .map(|link| Ok(Box::new(link) as Box<dyn Link>));
        }
        if self.link.is_none() && now >= self.deadline {
            self.done = true;
            return Some(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("No host reachable at {} (in-process)", self.addr),
            )));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::LinkEvent;
    use crate::msg::NetMessage;

    /// Calls `listener` (address `at`) from `from`; returns (accepted side, calling side).
    fn dial(
        net: &MemNet,
        from: &mut dyn Transport,
        listener: &mut dyn Listener,
        at: SocketAddr,
    ) -> (Box<dyn Link>, Box<dyn Link>) {
        let mut connecting = from.connect(at, 1_000);
        net.advance_to(net.now_ms() + 10);
        let dialer = connecting.poll().unwrap().unwrap();
        let accepted = listener.accept_pending().pop().unwrap();
        (accepted, dialer)
    }

    #[test]
    fn mem_links_know_the_peer_ip() {
        let net = MemNet::new();
        let mut a = MemTransport::new(&net, 5);
        let mut b = MemTransport::new(&net, 5);
        let mut listener = a.listen(0).unwrap();
        let at = SocketAddr::new(a.ip(), listener.port());
        // B calls A's listener.
        let (accepted, dialer) = dial(&net, &mut b, listener.as_mut(), at);
        assert_eq!(accepted.peer_ip(), Some(b.ip()), "caller");
        assert_eq!(dialer.peer_ip(), Some(a.ip()), "listener");
    }

    #[test]
    fn forked_mem_transport_keeps_its_ip() {
        let net = MemNet::new();
        let mut a = MemTransport::new(&net, 5);
        let b = MemTransport::new(&net, 5);
        let mut b_fork = b.fork();
        // The split-off calls with B's address ...
        let mut listener = a.listen(0).unwrap();
        let at = SocketAddr::new(a.ip(), listener.port());
        let (accepted, dialer) = dial(&net, b_fork.as_mut(), listener.as_mut(), at);
        assert_eq!(accepted.peer_ip(), Some(b.ip()));
        assert_eq!(dialer.peer_ip(), Some(a.ip()));
        // ... and listens at the same address in the same network.
        let mut fork_listener = b_fork.listen(0).unwrap();
        let at = SocketAddr::new(b.ip(), fork_listener.port());
        let (mut at_fork, mut to_fork) = dial(&net, &mut a, fork_listener.as_mut(), at);
        assert_eq!(at_fork.peer_ip(), Some(a.ip()));
        to_fork.send(NetMessage::Resume);
        net.advance_to(net.now_ms() + 10);
        assert_eq!(at_fork.poll(), vec![LinkEvent::Message(NetMessage::Resume)]);
    }

    #[test]
    fn system_transport_fork_is_a_copy() {
        let t = SystemTransport::new(true);
        let mut f = t.fork();
        let listener = f.listen(0).unwrap();
        assert_ne!(listener.port(), 0);
    }
}
