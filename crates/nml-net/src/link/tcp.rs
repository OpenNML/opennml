//! Connection over TCP.
//!
//! Per connection one read thread (blocking read, finished events go through a channel to the game host) and one
//! write thread (orders from a channel through the send scheduling [`Outbox`], blocking write). The window loop never
//! blocks: sending is queueing, receiving is a `try_recv`, and `poll` collects all waiting events. The original sends
//! synchronously in the main thread over DirectPlay, accesses tables from the callback without a lock and processes
//! one message per frame (E-254, E-270, E-271 deviation; re/multiplayer M.1.1, M.1.2).
//!
//! The write thread sends heartbeats itself when nothing went out for `HEARTBEAT_MS` - even during a loading screen or
//! a hanging window loop. The turn core evaluates the timeout.
//!
//! End of a connection: read error, end of the data stream, broken frame (`WireError`) or write error are reported as
//! `LinkEvent::Closed` with a reason text (exactly once) and written to the log, never as a panic. Both threads end
//! by themselves afterwards; `close` and `Drop` do not block the caller.

use super::{Link, LinkEvent, Outbox, PEER_CLOSED, deliver};
use crate::msg::NetMessage;
use crate::wire::{Assembler, WireError, encode, read_frame};
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Game port for TCP (default of the DirectPlay 8 games, re/multiplayer M.1.1). `NetPort` 0 means this port; any other
/// value is used as given (E-254; the command line sets it).
pub const DEFAULT_PORT: u16 = 2302;
/// Heartbeat: the write thread sends `Ping` when nothing else went out for this long.
pub const HEARTBEAT_MS: u64 = 1_000;

/// Deadline for a single write; after it the connection counts as aborted (the peer no longer reads).
const WRITE_TIMEOUT_MS: u64 = 30_000;
/// After closing locally, the write thread waits this long for the peer to end before it closes hard.
const LINGER_MS: u64 = 500;
/// Poll interval of the accept thread.
const ACCEPT_POLL_MS: u64 = 20;
/// Read deadline after which the read thread checks whether it should stop. On Windows `shutdown` does not wake a
/// blocked read in another thread; without a deadline the read thread would hang forever on a silent peer.
const READ_POLL_MS: u64 = 100;

/// Order for the write thread.
enum Outgoing {
    Small(NetMessage),
    Large(u32, Vec<u8>),
    /// Still send what is queued, then close.
    Close,
    /// Stop at once (the read thread has seen the end and closed the stream).
    Abort,
}

/// Shared between game host, read thread and write thread.
struct Shared {
    start: Instant,
    /// ms since `start` at the last reception.
    last_rx: AtomicU64,
    /// The game host has closed itself.
    local_close: AtomicBool,
    /// The read thread has ended.
    reader_done: AtomicBool,
    /// Muted (`Link::mute`): the write thread discards frames and heartbeats.
    muted: AtomicBool,
    /// The write thread has ended: the read thread stops at the next frame or after `READ_POLL_MS`.
    writer_done: AtomicBool,
    /// Running threads (for tests: nothing hangs).
    threads: AtomicUsize,
}

impl Shared {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// Counts a running thread; also releases it on a panic.
struct ThreadGuard(Arc<Shared>);

impl ThreadGuard {
    fn new(shared: &Arc<Shared>) -> Self {
        shared.threads.fetch_add(1, Ordering::SeqCst);
        Self(shared.clone())
    }
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.0.threads.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Sets the flag when leaving the scope.
struct SetOnDrop<'a>(&'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A connection over TCP.
pub struct TcpLink {
    out: mpsc::Sender<Outgoing>,
    events: mpsc::Receiver<LinkEvent>,
    shared: Arc<Shared>,
    /// `Closed` reported or closed itself: `poll` returns nothing more.
    done: bool,
    peer: Option<SocketAddr>,
}

fn addr_text(addr: Option<SocketAddr>) -> String {
    addr.map_or_else(|| "?".to_string(), |a| a.to_string())
}

impl TcpLink {
    /// Builds a connection to `addr` (deadline `timeout_ms`, at least 1 ms).
    pub fn connect(addr: SocketAddr, timeout_ms: u64) -> std::io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms.max(1)))?;
        Self::from_stream(stream)
    }

    /// Takes over a connected stream and starts the read and write thread (with chunking: the client's side).
    pub fn from_stream(stream: TcpStream) -> std::io::Result<Self> {
        Self::open(stream, true)
    }

    /// Like [`Self::from_stream`], but for a connection accepted by the host: without chunking. Clients never send
    /// large content to the host; a `Chunk` ends the connection (`Closed` with a reason text), so no peer occupies up
    /// to [`crate::wire::MAX_ASSEMBLED`] bytes before the `Hello`. [`TcpHost`] accepts this way.
    pub fn from_accepted_stream(stream: TcpStream) -> std::io::Result<Self> {
        Self::open(stream, false)
    }

    /// Starts the read and write thread; `accept_large`: reassemble received `Chunk` sequences.
    fn open(stream: TcpStream, accept_large: bool) -> std::io::Result<Self> {
        // An accepted stream can inherit the non-blocking mode of the listener (Windows).
        stream.set_nonblocking(false)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(Duration::from_millis(WRITE_TIMEOUT_MS)))?;
        stream.set_read_timeout(Some(Duration::from_millis(READ_POLL_MS)))?;
        let peer = stream.peer_addr().ok();
        let read_half = stream.try_clone()?;
        let shared = Arc::new(Shared {
            start: Instant::now(),
            last_rx: AtomicU64::new(0),
            local_close: AtomicBool::new(false),
            reader_done: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            writer_done: AtomicBool::new(false),
            threads: AtomicUsize::new(0),
        });
        let (out_tx, out_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let name = addr_text(peer);

        {
            let guard = ThreadGuard::new(&shared);
            let shared = shared.clone();
            let ev_tx = ev_tx.clone();
            let name = name.clone();
            std::thread::Builder::new()
                .name("nml-net-write".into())
                .spawn(move || {
                    let _guard = guard;
                    // Set even on a panic, so that the read thread does not hang.
                    let _done = SetOnDrop(&shared.writer_done);
                    write_loop(stream, out_rx, ev_tx, &shared, &name);
                })?;
        }
        let reader = {
            let guard = ThreadGuard::new(&shared);
            let shared = shared.clone();
            let out_tx = out_tx.clone();
            let name = name.clone();
            std::thread::Builder::new()
                .name("nml-net-read".into())
                .spawn(move || {
                    let _guard = guard;
                    read_loop(read_half, ev_tx, out_tx, &shared, &name, accept_large);
                })
        };
        if let Err(e) = reader {
            // The write thread is already running: end it (it closes the stream).
            shared.local_close.store(true, Ordering::SeqCst);
            let _ = out_tx.send(Outgoing::Close);
            return Err(e);
        }
        log::info!("Network: connection to {name} established");
        Ok(Self {
            out: out_tx,
            events: ev_rx,
            shared,
            done: false,
            peer,
        })
    }

    /// Address of the peer, if known.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }
}

impl Link for TcpLink {
    fn send(&mut self, msg: NetMessage) {
        if self.done {
            log::debug!(
                "Network: sending to {} discarded after the end",
                addr_text(self.peer)
            );
            return;
        }
        let _ = self.out.send(Outgoing::Small(msg));
    }

    fn send_large(&mut self, id: u32, bytes: Vec<u8>) {
        if self.done {
            log::debug!(
                "Network: sending to {} discarded after the end",
                addr_text(self.peer)
            );
            return;
        }
        let _ = self.out.send(Outgoing::Large(id, bytes));
    }

    fn poll(&mut self) -> Vec<LinkEvent> {
        let mut out = Vec::new();
        if self.done {
            // Only drain, so that nothing is left over.
            while self.events.try_recv().is_ok() {}
            return out;
        }
        loop {
            match self.events.try_recv() {
                Ok(LinkEvent::Closed(reason)) => {
                    self.done = true;
                    out.push(LinkEvent::Closed(reason));
                    break;
                }
                Ok(ev) => out.push(ev),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Both threads are gone without reporting an end (should not happen).
                    self.done = true;
                    out.push(LinkEvent::Closed(
                        "Connection threads ended unexpectedly".into(),
                    ));
                    break;
                }
            }
        }
        out
    }

    fn last_received_ms(&self) -> u64 {
        self.shared.last_rx.load(Ordering::SeqCst)
    }

    fn age_ms(&self) -> u64 {
        self.shared.now_ms()
    }

    fn close(&mut self, reason: &str) {
        if !self.done {
            log::info!(
                "Network: connection to {} closed: {reason}",
                addr_text(self.peer)
            );
        }
        self.done = true;
        if !self.shared.local_close.swap(true, Ordering::SeqCst) {
            let _ = self.out.send(Outgoing::Close);
        }
    }

    fn peer_ip(&self) -> Option<IpAddr> {
        self.peer.map(|a| a.ip())
    }

    fn mute(&mut self, on: bool) {
        self.shared.muted.store(on, Ordering::SeqCst);
    }
}

impl Drop for TcpLink {
    fn drop(&mut self) {
        self.close("Connection released");
    }
}

/// Write thread: orders into the send scheduling, frames out, heartbeats on silence.
fn write_loop(
    mut stream: TcpStream,
    jobs: mpsc::Receiver<Outgoing>,
    events: mpsc::Sender<LinkEvent>,
    shared: &Shared,
    name: &str,
) {
    let heartbeat = Duration::from_millis(HEARTBEAT_MS);
    let mut outbox = Outbox::new();
    let mut closing = false;
    let mut last_write = Instant::now();
    loop {
        // Take over everything queued before the next frame goes out: this way waiting small messages go before the
        // next piece of a large content.
        loop {
            match jobs.try_recv() {
                Ok(Outgoing::Abort) => return,
                Ok(job) => closing |= take(&mut outbox, job),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    closing = true;
                    break;
                }
            }
        }
        let frame = match outbox.next_frame() {
            Some(frame) => frame,
            None if closing => break,
            None => {
                let wait = heartbeat.saturating_sub(last_write.elapsed());
                if wait.is_zero() {
                    if shared.muted.load(Ordering::SeqCst) {
                        // Muted: no heartbeat; the clock keeps running so that the thread does not spin.
                        last_write = Instant::now();
                        continue;
                    }
                    NetMessage::Ping
                } else {
                    match jobs.recv_timeout(wait) {
                        Ok(Outgoing::Abort) => return,
                        Ok(job) => closing |= take(&mut outbox, job),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => closing = true,
                    }
                    continue;
                }
            }
        };
        if shared.muted.load(Ordering::SeqCst) {
            // Muted: the frame is lost (check aid).
            last_write = Instant::now();
            continue;
        }
        let result = encode(&frame)
            .map_err(|e| format!("message cannot be sent: {e}"))
            .and_then(|bytes| {
                stream
                    .write_all(&bytes)
                    .map_err(|e| format!("sending failed: {e}"))
            });
        if let Err(reason) = result {
            if shared.local_close.load(Ordering::SeqCst) {
                log::debug!("Network: {name}: {reason} (after closing locally)");
            } else {
                log::warn!("Network: connection to {name} lost: {reason}");
            }
            let _ = events.send(LinkEvent::Closed(reason));
            // Also wakes the read thread.
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        last_write = Instant::now();
    }
    // Close properly: send the end of the data stream and wait briefly for the peer to end (unread data on a hard
    // close could trigger a reset that discards what we sent last); then close hard, which also wakes a still blocked
    // read thread.
    let _ = stream.shutdown(Shutdown::Write);
    let linger = Instant::now();
    while !shared.reader_done.load(Ordering::SeqCst)
        && linger.elapsed() < Duration::from_millis(LINGER_MS)
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Takes an order into the send scheduling; `true` for `Close`.
fn take(outbox: &mut Outbox, job: Outgoing) -> bool {
    match job {
        Outgoing::Small(msg) => outbox.push_small(msg),
        Outgoing::Large(id, bytes) => outbox.push_large(id, bytes),
        Outgoing::Close => return true,
        Outgoing::Abort => {}
    }
    false
}

/// Read thread: read frames, note the reception time, swallow heartbeats, reassemble pieces (only with
/// `accept_large`, otherwise a piece ends the connection).
fn read_loop(
    stream: TcpStream,
    events: mpsc::Sender<LinkEvent>,
    writer: mpsc::Sender<Outgoing>,
    shared: &Shared,
    name: &str,
    accept_large: bool,
) {
    let mut reader = BufReader::with_capacity(64 * 1024, StopRead { stream, shared });
    let mut assembler = accept_large.then(Assembler::new);
    let reason = loop {
        if shared.writer_done.load(Ordering::SeqCst) {
            break STOPPED.to_string();
        }
        let msg = match read_frame(&mut reader) {
            Ok(msg) => msg,
            Err(e) => break describe(&e),
        };
        shared.last_rx.store(shared.now_ms(), Ordering::SeqCst);
        match deliver(assembler.as_mut(), msg) {
            Ok(Some(ev)) => {
                if events.send(ev).is_err() {
                    break "Connection released".to_string();
                }
            }
            Ok(None) => {}
            Err(e) => break format!("Connection lost: {e}"),
        }
    };
    shared.reader_done.store(true, Ordering::SeqCst);
    if shared.local_close.load(Ordering::SeqCst) {
        log::debug!("Network: reading from {name} ended: {reason} (after closing locally)");
    } else {
        log::warn!("Network: connection to {name} lost: {reason}");
    }
    let _ = events.send(LinkEvent::Closed(reason));
    let _ = writer.send(Outgoing::Abort);
    let _ = reader.get_ref().stream.shutdown(Shutdown::Both);
}

/// Reason when the read thread stops after the end of the write thread.
const STOPPED: &str = "Connection closed";

/// Reading with a deadline: a deadline expiry is repeated until the write thread has ended. The deadline stays
/// invisible to `read_frame` (and its `read_exact`); a half frame is not lost.
struct StopRead<'a> {
    stream: TcpStream,
    shared: &'a Shared,
}

impl Read for StopRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.stream.read(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if self.shared.writer_done.load(Ordering::SeqCst) {
                        return Err(std::io::Error::other(STOPPED));
                    }
                }
                other => return other,
            }
        }
    }
}

/// Reason text for the end of reading.
fn describe(e: &WireError) -> String {
    match e {
        WireError::Io(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
            PEER_CLOSED.to_string()
        }
        WireError::Io(io) => format!("Connection aborted: {io}"),
        other => format!("Connection lost: {other}"),
    }
}

/// Accepts connections (accept thread); the game host collects them without blocking. Accepted connections have no
/// chunking ([`TcpLink::from_accepted_stream`]).
///
/// The accept thread starts a read and a write thread for every new connection at once, so their heartbeats run even
/// when the game host is not collecting at the moment. `Drop` stops the accept thread (after `ACCEPT_POLL_MS` at the
/// latest).
pub struct TcpHost {
    addr: SocketAddr,
    incoming: mpsc::Receiver<TcpLink>,
    stop: Arc<AtomicBool>,
}

impl TcpHost {
    /// Listens on `addr` (tests: `127.0.0.1:0`; the game: all interfaces).
    pub fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, incoming) = mpsc::channel();
        {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("nml-net-accept".into())
                .spawn(move || accept_loop(listener, tx, &stop))?;
        }
        log::info!("Network: listening on {addr}");
        Ok(Self {
            addr,
            incoming,
            stop,
        })
    }

    /// Actual address (with the assigned port for port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// All connections accepted since the last call; does not block.
    pub fn accept_pending(&mut self) -> Vec<TcpLink> {
        self.incoming.try_iter().collect()
    }
}

impl Drop for TcpHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Listener of the transport over `std::net`.
impl crate::transport::Listener for TcpHost {
    fn accept_pending(&mut self) -> Vec<Box<dyn Link>> {
        TcpHost::accept_pending(self)
            .into_iter()
            .map(|link| Box::new(link) as Box<dyn Link>)
            .collect()
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

fn accept_loop(listener: TcpListener, tx: mpsc::Sender<TcpLink>, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => match TcpLink::from_accepted_stream(stream) {
                Ok(link) => {
                    if tx.send(link).is_err() {
                        break;
                    }
                }
                Err(e) => log::warn!("Network: connection from {peer} not taken over: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(ACCEPT_POLL_MS));
            }
            Err(e) => {
                log::warn!("Network: accepting failed: {e}");
                std::thread::sleep(Duration::from_millis(ACCEPT_POLL_MS));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// Host side and client side of a fresh connection.
    fn pair() -> (TcpLink, TcpLink) {
        let mut host = TcpHost::bind(local()).unwrap();
        let client = TcpLink::connect(host.local_addr(), 2_000).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(link) = host.accept_pending().pop() {
                return (link, client);
            }
            assert!(Instant::now() < deadline, "no connection accepted");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Polls until `done` holds or the deadline passes; returns everything collected.
    fn collect(
        link: &mut TcpLink,
        timeout_ms: u64,
        done: impl Fn(&[LinkEvent]) -> bool,
    ) -> Vec<LinkEvent> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut all = Vec::new();
        loop {
            all.extend(link.poll());
            if done(&all) || Instant::now() >= deadline {
                return all;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn closed_count(evs: &[LinkEvent]) -> usize {
        evs.iter()
            .filter(|e| matches!(e, LinkEvent::Closed(_)))
            .count()
    }

    fn wait_threads_gone(shared: &Arc<Shared>, timeout_ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while shared.threads.load(Ordering::SeqCst) > 0 {
            assert!(
                Instant::now() < deadline,
                "threads still running: {}",
                shared.threads.load(Ordering::SeqCst)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn tcp_round_trip_both_ways() {
        let (mut h, mut c) = pair();
        h.send(NetMessage::Welcome { slot: 3 });
        h.send(NetMessage::Bye {
            reason: "end".into(),
        });
        c.send(NetMessage::Loaded { error: None });
        c.send(NetMessage::Synced);
        let at_c = collect(&mut c, 3_000, |e| e.len() >= 2);
        assert_eq!(
            at_c,
            vec![
                LinkEvent::Message(NetMessage::Welcome { slot: 3 }),
                // Bye is an ordinary message, not a Closed.
                LinkEvent::Message(NetMessage::Bye {
                    reason: "end".into()
                }),
            ]
        );
        let at_h = collect(&mut h, 3_000, |e| e.len() >= 2);
        assert_eq!(
            at_h,
            vec![
                LinkEvent::Message(NetMessage::Loaded { error: None }),
                LinkEvent::Message(NetMessage::Synced),
            ]
        );
        assert!(h.last_received_ms() <= h.age_ms());
        assert!(h.peer_addr().is_some());
    }

    #[test]
    fn tcp_close_is_reported() {
        let (mut h, mut c) = pair();
        let shared = h.shared.clone();
        c.send(NetMessage::Resume);
        h.close("Test");
        // Whoever closes itself gets nothing more.
        assert!(h.poll().is_empty());
        let evs = collect(&mut c, 3_000, |e| closed_count(e) > 0);
        assert_eq!(closed_count(&evs), 1, "{evs:?}");
        // Exactly once, not again later either.
        let later = collect(&mut c, 300, |_| false);
        assert!(later.is_empty(), "{later:?}");
        c.send(NetMessage::Resume);
        drop(c);
        wait_threads_gone(&shared, 3_000);
    }

    #[test]
    fn tcp_drop_without_bye_is_reported() {
        let (h, mut c) = pair();
        let shared = h.shared.clone();
        drop(h);
        let evs = collect(&mut c, 3_000, |e| closed_count(e) > 0);
        assert_eq!(closed_count(&evs), 1, "{evs:?}");
        assert!(collect(&mut c, 300, |_| false).is_empty());
        let c_shared = c.shared.clone();
        drop(c);
        wait_threads_gone(&shared, 3_000);
        wait_threads_gone(&c_shared, 3_000);
    }

    #[test]
    fn heartbeats_continue_without_polling() {
        let (h, c) = pair();
        // Neither side polls; the threads send and read anyway.
        std::thread::sleep(Duration::from_millis(2_500));
        for link in [&h, &c] {
            let age = link.age_ms();
            let last = link.last_received_ms();
            assert!(age >= 2_500, "age {age}");
            assert!(age - last <= 1_500, "age {age}, last reception {last}");
        }
        // Heartbeats are not delivered.
        let (mut h, mut c) = (h, c);
        assert!(h.poll().is_empty());
        assert!(c.poll().is_empty());
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect()
    }

    #[test]
    fn chunked_state_interleaves_pings() {
        let (mut h, mut c) = pair();
        let state = pseudo_random(3 << 20);
        h.send_large(11, state.clone());
        for slot in 0..5 {
            h.send(NetMessage::Welcome { slot });
        }
        let evs = collect(&mut c, 10_000, |e| e.len() >= 6);
        assert_eq!(evs.len(), 6, "{} events", evs.len());
        // Over TCP only completeness counts; the time order is checked by `outbox_sends_small_before_next_chunk`.
        let smalls: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                LinkEvent::Message(NetMessage::Welcome { slot }) => Some(*slot),
                _ => None,
            })
            .collect();
        assert_eq!(smalls, vec![0, 1, 2, 3, 4]);
        let large: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                LinkEvent::Large { id, bytes } => Some((*id, bytes)),
                _ => None,
            })
            .collect();
        assert_eq!(large.len(), 1);
        assert_eq!(large[0].0, 11);
        assert!(*large[0].1 == state, "large content corrupted");
    }

    /// A connection accepted by the host does not reassemble pieces; even the first `Chunk` ends it with a reason text
    /// (the opposite direction is checked by `chunked_state_interleaves_pings`).
    #[test]
    fn host_side_rejects_chunks() {
        let (mut h, mut c) = pair();
        let shared = h.shared.clone();
        c.send(NetMessage::Resume);
        c.send_large(7, vec![9u8; 1000]);
        let evs = collect(&mut h, 5_000, |e| closed_count(e) > 0);
        assert_eq!(evs[0], LinkEvent::Message(NetMessage::Resume), "{evs:?}");
        assert!(
            !evs.iter().any(|e| matches!(e, LinkEvent::Large { .. })),
            "{evs:?}"
        );
        let Some(LinkEvent::Closed(reason)) = evs.last() else {
            panic!("expected Closed: {evs:?}")
        };
        assert!(reason.contains("large content"), "{reason}");
        drop(h);
        wait_threads_gone(&shared, 3_000);
    }

    #[test]
    fn broken_frame_closes_with_reason() {
        let listener = std::net::TcpListener::bind(local()).unwrap();
        let mut raw = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let mut link = TcpLink::from_stream(accepted).unwrap();
        let shared = link.shared.clone();
        raw.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let evs = collect(&mut link, 3_000, |e| closed_count(e) > 0);
        assert_eq!(evs.len(), 1, "{evs:?}");
        let LinkEvent::Closed(reason) = &evs[0] else {
            panic!("expected Closed: {evs:?}")
        };
        assert!(reason.contains("protocol"), "{reason}");
        drop(link);
        wait_threads_gone(&shared, 3_000);
    }

    #[test]
    fn drop_stops_threads_with_silent_peer() {
        // Peer that never sends anything and never closes.
        let listener = std::net::TcpListener::bind(local()).unwrap();
        let raw = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let mut link = TcpLink::from_stream(accepted).unwrap();
        let shared = link.shared.clone();
        link.send(NetMessage::Resume);
        let begin = Instant::now();
        drop(link);
        assert!(begin.elapsed() < Duration::from_millis(100), "drop blocks");
        wait_threads_gone(&shared, 3_000);
        drop(raw);
    }

    #[test]
    fn host_accepts_several_and_stops() {
        let mut host = TcpHost::bind(local()).unwrap();
        assert_ne!(host.local_addr().port(), 0);
        let a = TcpLink::connect(host.local_addr(), 2_000).unwrap();
        let b = TcpLink::connect(host.local_addr(), 2_000).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < 2 && Instant::now() < deadline {
            got.extend(host.accept_pending());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), 2);
        assert!(host.accept_pending().is_empty());
        drop(host);
        drop((a, b, got));
    }

    #[test]
    fn tcp_link_knows_the_peer_ip() {
        let (h, c) = pair();
        assert_eq!(h.peer_ip(), Some(local().ip()));
        assert_eq!(c.peer_ip(), Some(local().ip()));
    }

    /// `mute(true)`: the write thread discards everything including heartbeats, the other side hears nothing more; with
    /// `mute(false)` the heartbeats come back.
    #[test]
    fn muted_tcp_link_goes_silent() {
        let (mut h, mut c) = pair();
        let silence = |l: &TcpLink| l.age_ms().saturating_sub(l.last_received_ms());
        std::thread::sleep(Duration::from_millis(1_200));
        assert!(silence(&c) <= 1_500, "before muting {}", silence(&c));
        h.mute(true);
        h.send(NetMessage::Resume);
        std::thread::sleep(Duration::from_millis(2_600));
        assert!(silence(&c) > 1_500, "after muting {}", silence(&c));
        assert!(c.poll().is_empty(), "muted: messages are lost too");
        // The opposite direction is not affected.
        assert!(silence(&h) <= 1_500, "opposite direction {}", silence(&h));
        h.mute(false);
        std::thread::sleep(Duration::from_millis(1_400));
        assert!(silence(&c) <= 1_500, "after unmuting {}", silence(&c));
        // A `close` also goes out when muted.
        h.mute(true);
        h.close("Test");
        let evs = collect(&mut c, 3_000, |e| closed_count(e) > 0);
        assert_eq!(closed_count(&evs), 1, "{evs:?}");
    }
}
