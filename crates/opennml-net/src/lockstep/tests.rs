//! Tests of the turn core: host and clients over [`MemNet`] with simulated time.
//!
//! [`Rig`] connects a [`HostCore`] with any number of [`ClientCore`]s (delay per connection) and simulates machines:
//! each one "computes" ticks at its own speed (`Machine::tick_ms`), closes its contribution before every turn tick
//! (checksum = tick count as a placeholder) and waits without a bundle before an execution tick. If the bundle carries
//! a halt, the machine stands after its execution tick until the core reports `Running` again (like the driver). The
//! rig reports the silence per connection like the driver from `age_ms() - last_received_ms()`. Time runs in frames of
//! [`FRAME_MS`].
//!
//! Resync like the driver: if the host reports `SendState`, the rig sends its "state" ([`state_bytes`]) as soon as the
//! host stands after the execution tick; a client adopts a received `ResyncState` only when it stands there itself,
//! and then calls `synced`. On `Stop` everyone stands after the execution tick forever.

use super::*;
use crate::link::Link;
use crate::link::mem::{MemLink, MemNet};
use crate::lobby::{
    AcceptAll, DataInput, DataReport, DataState, HostEdit, KIND_HUMAN, LobbyBook, LobbyWish,
    SessionInfo,
};
use crate::msg::{
    Hello, MAX_GAME_SPEED, NetMessage, Seat, SeatClaim, StartInfo, Successor, TurnContribution,
    valid_save_name,
};
use crate::wire::{CHUNK_SIZE, NET_VERSION};
use opennml_sim::command::{Command, CommandKind};
use opennml_sim::setup::{MatchSetup, StartPackage};

/// Length of a frame of simulated time.
const FRAME_MS: u64 = 25;
/// Duration of a tick at game speed 1 (20 ticks per second).
const TICK_MS: u64 = 50;
/// Maximum number of frames before a wait counts as a hang.
const MAX_FRAMES: u32 = 20_000;

fn hello(name: &str) -> Hello {
    Hello {
        listen_port: None,
        net_version: NET_VERSION,
        protocol: 42,
        format: 29,
        edition: "de".into(),
        name: name.into(),
        build: "test".into(),
        seat: None,
    }
}

/// Lobby state with open rows for `players` players (rig and single tests; the start is unchecked).
fn book(cfg: TurnConfig, players: u8) -> LobbyBook {
    LobbyBook::open(
        SessionInfo::plain("Test match", "multiplayer/test.lvl", players, cfg),
        "Host",
    )
    .expect("valid session")
}

/// A distinguishable command of slot `player`.
fn cmd(player: u8, tag: i32) -> Command {
    Command {
        player,
        kind: CommandKind::Diplomacy {
            p: i32::from(player),
            q: tag,
            mode: 0,
        },
    }
}

fn package() -> StartPackage {
    StartPackage {
        format: 29,
        level: "multiplayer/test.lvl".into(),
        rules: opennml_sim::rules::RuleChecksums {
            logic: 0x1234,
            ..Default::default()
        },
        setup: MatchSetup::default(),
        random_map: None,
    }
}

/// A simulated machine.
#[derive(Debug, Default)]
struct Machine {
    /// Ticks computed (the machine stands before tick `steps`).
    steps: u64,
    /// Duration of a tick in ms.
    tick_ms: u64,
    /// Accumulated game time; discarded while waiting.
    budget: u64,
    /// Next turn to close.
    next_close: u32,
    /// Simulated time at which turn `k` was closed (index `k`).
    closed_at: Vec<u64>,
    /// Execution ticks with the commands of their bundle.
    executed: Vec<(u64, Vec<Command>)>,
    /// Stands after the execution tick of a bundle with a halt until the core reports `Running` again.
    halted: bool,
    /// Ticks at which the machine halted (state after the execution tick).
    halted_at: Vec<u64>,
    /// Turns whose contribution carries a wrong checksum (desync simulated).
    wrong_rounds: Vec<u32>,
    /// Client: does not yet adopt a received state (slow adoption).
    hold_sync: bool,
    /// Client: adopted states with the tick it stood at.
    adopted: Vec<(u64, Vec<u8>)>,
}

/// "State" of the host for the resync: larger than a piece so that it travels in pieces; carries the tick.
fn state_bytes(steps: u64) -> Vec<u8> {
    let mut bytes = format!("State before tick {steps};").into_bytes();
    bytes.resize(CHUNK_SIZE + 100, (steps % 251) as u8);
    bytes
}

struct Rig {
    net: MemNet,
    host: HostCore,
    clients: Vec<ClientCore>,
    /// Connections of the host, one per client (peer = index + 1).
    host_links: Vec<(Peer, MemLink)>,
    /// Connection of each client to the host.
    client_links: Vec<MemLink>,
    /// Index 0 = host, then the clients.
    machines: Vec<Machine>,
    /// Collected events per machine.
    events: Vec<Vec<CoreEvent>>,
    /// Slot per machine (default: index).
    slots: Vec<u8>,
    now: u64,
    /// Largest observed distance host - slowest client in ticks.
    max_lead: u64,
    /// States already sent by the host (one per `SendState`).
    states_sent: usize,
    /// Like the driver: after a sent `Reject`/`Bye` the host closes the connection (lobby tests).
    close_after_bye: bool,
    /// Connections closed by the host after `Reject`/`Bye`.
    closed: Vec<Peer>,
}

/// Core of machine `i` (0 = host).
fn core<'a>(
    host: &'a mut HostCore,
    clients: &'a mut [ClientCore],
    i: usize,
) -> &'a mut dyn Lockstep {
    if i == 0 { host } else { &mut clients[i - 1] }
}

impl Rig {
    /// Host and, per entry of `delays`, a client with this delay per direction; nothing pumped yet.
    fn new(delays: &[u64]) -> Self {
        Self::build(TurnConfig::default(), delays)
    }

    fn build(cfg: TurnConfig, delays: &[u64]) -> Self {
        let net = MemNet::new();
        let mut host_links = Vec::new();
        let mut client_links = Vec::new();
        let mut clients = Vec::new();
        for (i, &delay) in delays.iter().enumerate() {
            let (h, c) = net.pair(delay);
            let peer = (i + 1) as Peer;
            host_links.push((peer, h));
            client_links.push(c);
            clients.push(ClientCore::new(hello(&format!("Client {peer}"))));
        }
        let n = delays.len() + 1;
        Self {
            net,
            host: HostCore::new(hello("Host"), book(cfg, 8), Box::new(AcceptAll)),
            clients,
            host_links,
            client_links,
            machines: (0..n)
                .map(|_| Machine {
                    tick_ms: TICK_MS,
                    ..Machine::default()
                })
                .collect(),
            events: vec![Vec::new(); n],
            slots: (0..n as u8).collect(),
            now: 0,
            max_lead: 0,
            states_sent: 0,
            close_after_bye: false,
            closed: Vec::new(),
        }
    }

    /// Like [`Rig::new`], then sign-on, start, loading and `Go`; all stand at tick 0.
    fn running(delays: &[u64]) -> Self {
        Self::running_with(TurnConfig::default(), delays)
    }

    fn running_with(cfg: TurnConfig, delays: &[u64]) -> Self {
        let mut rig = Self::build(cfg, delays);
        rig.go();
        rig
    }

    /// Sign-on, start according to `slots`, loading and `Go`, without computing a tick.
    fn go(&mut self) {
        self.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
        self.start();
        self.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
        for c in &mut self.clients {
            c.loaded(None);
        }
        self.host.loaded_self();
        self.frames_until(|r| r.all_in(&Phase::Running));
    }

    fn all_in(&self, phase: &Phase) -> bool {
        self.host.phase() == phase && self.clients.iter().all(|c| c.phase() == phase)
    }

    /// Start information for peer `peer` according to `slots`.
    fn info(slots: &[u8], cfg: TurnConfig, peer: Peer) -> StartInfo {
        let mut seats: [Seat; 8] = std::array::from_fn(|_| Seat {
            peer: None,
            name: String::new(),
        });
        for (i, &slot) in slots.iter().enumerate() {
            seats[usize::from(slot)] = Seat {
                peer: Some(i as Peer),
                name: if i == 0 {
                    "Host".into()
                } else {
                    format!("Client {i}")
                },
            };
        }
        StartInfo {
            package: package(),
            seats,
            your_slot: slots[usize::from(peer)],
            spacing: cfg.raw().0,
            lead: cfg.raw().1,
            game_speed: 5,
            session: "Test match".into(),
            saved: None,
        }
    }

    fn start(&mut self) {
        let slots = self.slots.clone();
        let cfg = self.host.config();
        self.host.start(|peer| Self::info(&slots, cfg, peer));
    }

    /// Delivers all events and the silence per connection, passes messages on and collects events.
    fn pump(&mut self) {
        let now = self.now;
        for (peer, link) in &mut self.host_links {
            for ev in link.poll() {
                self.host.handle(now, *peer, ev);
            }
            let silent = link.age_ms().saturating_sub(link.last_received_ms());
            self.host.silence(now, *peer, silent);
        }
        self.host.tick(now);
        for (i, link) in self.client_links.iter_mut().enumerate() {
            for ev in link.poll() {
                match ev {
                    // Like the driver: the `Migrated` of the new host goes to `rehome`.
                    LinkEvent::Message(NetMessage::Migrated(m)) => {
                        if let Err(e) = self.clients[i].rehome(*m) {
                            log::warn!("Test: rehome rejected: {e}");
                        }
                    }
                    ev => self.clients[i].handle(now, HOST_PEER, ev),
                }
            }
            let silent = link.age_ms().saturating_sub(link.last_received_ms());
            self.clients[i].silence(now, HOST_PEER, silent);
            self.clients[i].tick(now);
        }
        for (to, msg) in self.host.outbox() {
            let (_, link) = self
                .host_links
                .iter_mut()
                .find(|(p, _)| *p == to)
                .expect("receiver unknown");
            let closing = match &msg {
                NetMessage::Bye { reason } | NetMessage::Reject { reason } => Some(reason.clone()),
                _ => None,
            };
            link.send(msg);
            if let Some(reason) = closing.filter(|_| self.close_after_bye) {
                link.close(&reason);
                self.closed.push(to);
            }
        }
        for (to, id, bytes) in self.host.large_outbox() {
            let (_, link) = self
                .host_links
                .iter_mut()
                .find(|(p, _)| *p == to)
                .expect("receiver unknown");
            link.send_large(id, bytes);
        }
        for (i, c) in self.clients.iter_mut().enumerate() {
            for (to, msg) in c.outbox() {
                assert_eq!(to, HOST_PEER, "client sends only to the host");
                self.client_links[i].send(msg);
            }
            for (to, id, bytes) in c.large_outbox() {
                assert_eq!(to, HOST_PEER);
                self.client_links[i].send_large(id, bytes);
            }
        }
        self.events[0].extend(self.host.events());
        for (i, c) in self.clients.iter_mut().enumerate() {
            self.events[i + 1].extend(c.events());
        }
    }

    /// Sets the simulated time exactly to `now` and pumps, without computing.
    fn pump_at(&mut self, now: u64) {
        assert!(now >= self.now);
        self.now = now;
        self.net.advance_to(now);
        self.pump();
    }

    /// One frame: time forward, collect, compute (at most up to tick `cap`), send.
    fn frame(&mut self, cap: u64) {
        self.now += FRAME_MS;
        self.net.advance_to(self.now);
        self.pump();
        for i in 0..self.machines.len() {
            self.step_machine(i, cap);
        }
        self.serve_resync();
        self.pump();
    }

    /// Resync like the driver: the host sends its state as soon as it stands after the execution tick; a client adopts
    /// a received state only then (except `hold_sync`) and reports `synced`.
    fn serve_resync(&mut self) {
        let wanted = self.count_events(0, |e| matches!(e, CoreEvent::SendState { .. }));
        if wanted > self.states_sent
            && self.machines[0].halted
            && self.host.phase() == &Phase::Halted(HaltReason::Resync)
        {
            self.host.send_state(state_bytes(self.machines[0].steps));
            self.states_sent += 1;
        }
        for i in 1..self.machines.len() {
            let got: Vec<Vec<u8>> = self.events[i]
                .iter()
                .filter_map(|e| match e {
                    CoreEvent::ResyncState { bytes } => Some(bytes.clone()),
                    _ => None,
                })
                .collect();
            let m = &mut self.machines[i];
            if got.len() > m.adopted.len() && m.halted && !m.hold_sync {
                m.adopted.push((m.steps, got[m.adopted.len()].clone()));
                self.clients[i - 1].synced();
            }
        }
    }

    /// Machine `i` computes as long as game time is available, `cap` is not reached and no bundle is missing.
    fn step_machine(&mut self, i: usize, cap: u64) {
        let now = self.now;
        self.machines[i].budget += FRAME_MS;
        loop {
            let core = core(&mut self.host, &mut self.clients, i);
            let m = &mut self.machines[i];
            // After `Stop` and `End` it still computes up to the execution tick of the bundle and then stands.
            let ending =
                core.phase() == &Phase::Ended && !m.halted && ended_turn(&self.events[i]).is_some();
            let live = core.phase().in_match() || core.phase() == &Phase::Stopped || ending;
            if !live || m.steps >= cap {
                m.budget = 0;
                break;
            }
            // Halt after an execution tick: continue only when the core runs again (Resume).
            if m.halted {
                if core.phase() != &Phase::Running {
                    m.budget = 0;
                    break;
                }
                m.halted = false;
            }
            let cfg = core.config();
            // Before the turn tick b(k): close the contribution.
            if let Some(k) = cfg.round_at(m.steps)
                && m.next_close == k
            {
                let wrong = u64::from(m.wrong_rounds.contains(&k));
                core.close_round(k, m.steps + wrong);
                m.next_close += 1;
                m.closed_at.push(now);
            }
            if m.budget < m.tick_ms {
                break;
            }
            let mut halt = false;
            if let Some(k) = cfg.exec_at(m.steps) {
                let Some(b) = core.bundle(k) else {
                    // Waiting: discard game time.
                    m.budget = 0;
                    break;
                };
                assert_eq!(b.turn, k);
                m.executed.push((m.steps, b.commands.clone()));
                halt = b.control.iter().any(|c| {
                    matches!(
                        c,
                        NetControl::Halt { .. }
                            | NetControl::Resync { .. }
                            | NetControl::Stop { .. }
                            | NetControl::End { .. }
                    )
                });
            }
            m.steps += 1;
            m.budget -= m.tick_ms;
            // Control entries take effect after the execution tick; if the Resume has already come, nobody halts.
            if halt
                && matches!(
                    core.phase(),
                    Phase::Halted(_) | Phase::Stopped | Phase::Ended
                )
            {
                m.halted = true;
                m.halted_at.push(m.steps);
            }
            if let Some(slowest) = self.machines[1..].iter().map(|m| m.steps).min() {
                let lead = self.machines[0].steps.saturating_sub(slowest);
                self.max_lead = self.max_lead.max(lead);
            }
        }
    }

    /// Frames without computing until `cond` holds.
    fn frames_until(&mut self, cond: impl Fn(&Self) -> bool) {
        for _ in 0..MAX_FRAMES {
            if cond(self) {
                return;
            }
            self.frame(0);
        }
        panic!(
            "condition not reached after {MAX_FRAMES} frames (time {})",
            self.now
        );
    }

    /// Computes until every machine stands at tick `step` (none beyond).
    fn run_until(&mut self, step: u64) {
        for _ in 0..MAX_FRAMES {
            if self.machines.iter().all(|m| m.steps >= step) {
                return;
            }
            self.frame(step);
        }
        let steps: Vec<u64> = self.machines.iter().map(|m| m.steps).collect();
        panic!("tick {step} not reached, state {steps:?}");
    }

    /// Computes `ms` of simulated time without an upper bound.
    fn run_for(&mut self, ms: u64) {
        for _ in 0..ms / FRAME_MS {
            self.frame(u64::MAX);
        }
    }

    fn steps(&self) -> Vec<u64> {
        self.machines.iter().map(|m| m.steps).collect()
    }

    /// Every machine has computed every execution tick before its state with a bundle, and all with the same commands.
    fn assert_same_execution(&self) {
        let cfg = self.host.config();
        for (i, m) in self.machines.iter().enumerate() {
            let expected: Vec<u64> = (0..m.steps).filter(|&s| cfg.exec_at(s).is_some()).collect();
            let got: Vec<u64> = m.executed.iter().map(|(s, _)| *s).collect();
            assert_eq!(got, expected, "machine {i}: execution ticks");
        }
        let reference = &self.machines[0].executed;
        for (i, m) in self.machines.iter().enumerate().skip(1) {
            let n = m.executed.len().min(reference.len());
            assert_eq!(m.executed[..n], reference[..n], "machine {i}: commands");
        }
    }

    /// Execution ticks with commands on machine `i`.
    fn nonempty(&self, i: usize) -> Vec<(u64, Vec<Command>)> {
        self.machines[i]
            .executed
            .iter()
            .filter(|(_, c)| !c.is_empty())
            .cloned()
            .collect()
    }

    fn has_event(&self, i: usize, pred: impl Fn(&CoreEvent) -> bool) -> bool {
        self.events[i].iter().any(pred)
    }

    fn count_events(&self, i: usize, pred: impl Fn(&CoreEvent) -> bool) -> usize {
        self.events[i].iter().filter(|e| pred(e)).count()
    }

    /// Phase of machine `i` (0 = host).
    fn phase(&self, i: usize) -> &Phase {
        if i == 0 {
            self.host.phase()
        } else {
            self.clients[i - 1].phase()
        }
    }

    /// Reported control entries of machine `i` with their turn.
    fn controls(&self, i: usize) -> Vec<(u32, NetControl)> {
        self.events[i]
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Control { turn, control } => Some((*turn, control.clone())),
                _ => None,
            })
            .collect()
    }

    /// Reported dropouts of machine `i`: (peer, slot, reason).
    fn losses(&self, i: usize) -> Vec<(Peer, Option<u8>, String)> {
        self.events[i]
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Lost { peer, slot, reason } => Some((*peer, *slot, reason.clone())),
                _ => None,
            })
            .collect()
    }

    /// All executed commands of machine `i` that equal `cmd`.
    fn executed_count(&self, i: usize, cmd: &Command) -> usize {
        self.machines[i]
            .executed
            .iter()
            .flat_map(|(_, c)| c)
            .filter(|c| *c == cmd)
            .count()
    }
}

/// Dropout command of slot `slot`.
fn loss(slot: u8) -> Command {
    Command {
        player: slot,
        kind: CommandKind::SetPlayerResult { slot, result: 1 },
    }
}

/// Turn of the bundle with `End` among the events of a machine.
fn ended_turn(events: &[CoreEvent]) -> Option<u32> {
    events.iter().find_map(|e| match e {
        CoreEvent::Control {
            turn,
            control: NetControl::End { .. },
        } => Some(*turn),
        _ => None,
    })
}

fn is_lost(e: &CoreEvent) -> bool {
    matches!(e, CoreEvent::Lost { .. })
}

fn is_resumed(e: &CoreEvent) -> bool {
    e == &CoreEvent::Resumed
}

// QUIRK E-293: deviation
#[test]
fn start_waits_for_every_loaded() {
    let mut rig = Rig::new(&[10, 30]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    assert_eq!(rig.clients[0].peer(), Some(1));
    assert_eq!(rig.clients[1].peer(), Some(2));
    assert_eq!(
        rig.host.peers(),
        vec![
            (0, "Host".to_string()),
            (1, "Client 1".to_string()),
            (2, "Client 2".to_string())
        ]
    );
    for peer in [1, 2] {
        assert!(rig.has_event(
            0,
            |e| matches!(e, CoreEvent::Joined { peer: p, .. } if *p == peer)
        ));
    }
    assert_eq!(rig.host.phase(), &Phase::Gathering);

    rig.start();
    assert_eq!(rig.host.phase(), &Phase::Loading);
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    assert!(rig.has_event(0, |e| matches!(e, CoreEvent::Start(i) if i.your_slot == 0)));
    for i in 1..=2 {
        assert!(rig.has_event(
            i,
            |e| matches!(e, CoreEvent::Start(s) if usize::from(s.your_slot) == i)
        ));
        assert_eq!(rig.host.slot_of(i as Peer), Some(i as u8));
        assert_eq!(rig.clients[i - 1].slot(), Some(i as u8));
    }
    assert_eq!(rig.host.slot_of(HOST_PEER), Some(0));

    // Host and client 1 done, client 2 still loading: no Go, no tick.
    rig.host.loaded_self();
    rig.clients[0].loaded(None);
    rig.run_for(5_000);
    for i in 0..3 {
        assert!(!rig.has_event(i, |e| e == &CoreEvent::Go), "machine {i}");
    }
    assert!(rig.all_in(&Phase::Loading));
    assert_eq!(rig.steps(), vec![0, 0, 0]);

    rig.clients[1].loaded(None);
    rig.frames_until(|r| r.all_in(&Phase::Running));
    for i in 0..3 {
        assert_eq!(
            rig.events[i]
                .iter()
                .filter(|e| *e == &CoreEvent::Go)
                .count(),
            1,
            "machine {i}"
        );
    }
}

/// Names from the `LoadFailed` events of machine `i`.
fn load_failures(rig: &Rig, i: usize) -> Vec<String> {
    rig.events[i]
        .iter()
        .filter_map(|e| match e {
            CoreEvent::LoadFailed { name } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

// QUIRK E-293: deviation
#[test]
fn load_error_aborts_for_everyone() {
    let mut rig = Rig::new(&[10, 20]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    rig.host.loaded_self();
    rig.clients[0].loaded(None);
    rig.clients[1].loaded(Some("Different rules checksum".into()));
    rig.frames_until(|r| r.all_in(&Phase::Ended));
    for i in 0..3 {
        assert_eq!(
            load_failures(&rig, i),
            vec!["Client 2".to_string()],
            "machine {i}: {:?}",
            rig.events[i]
        );
        assert!(!rig.has_event(i, is_failed), "machine {i}");
        assert!(!rig.has_event(i, |e| e == &CoreEvent::Go));
    }
}

/// A client's loading error: the host sends `LoadFailed` with the player's name before the `Bye` to every client
/// (also the failing one); every machine reports `CoreEvent::LoadFailed` with the name instead of `Failed`, the host
/// closes the connections after the `Bye`.
// QUIRK E-293: deviation
#[test]
fn load_error_sends_load_failed_with_the_name() {
    let mut rig = Rig::new(&[10, 20]);
    rig.clients[1] = ClientCore::new(hello("Bert"));
    rig.close_after_bye = true;
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    rig.clients[1].loaded(Some("Different rules checksum".into()));
    rig.frames_until(|r| r.all_in(&Phase::Ended));
    for i in 0..3 {
        assert_eq!(
            load_failures(&rig, i),
            vec!["Bert".to_string()],
            "machine {i}"
        );
        // A `Bye` before the `LoadFailed` would have been a `Failed` at the clients.
        assert!(
            !rig.has_event(i, is_failed),
            "machine {i}: {:?}",
            rig.events[i]
        );
    }
    let mut closed = rig.closed.clone();
    closed.sort_unstable();
    assert_eq!(closed, vec![1, 2], "Bye to every client");
}

/// The host's own loading error: `LoadFailed` with the host's name before the `Bye` to every client, the host itself
/// reports `CoreEvent::LoadFailed` too; a second report has no effect.
// QUIRK E-293: deviation
#[test]
fn host_load_error_sends_load_failed_with_its_name() {
    let mut rig = Rig::new(&[10, 20]);
    rig.close_after_bye = true;
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    rig.clients[0].loaded(None);
    rig.host.load_failed("Map is missing".into());
    assert_eq!(rig.host.phase(), &Phase::Ended);
    rig.frames_until(|r| r.all_in(&Phase::Ended));
    for i in 0..3 {
        assert_eq!(
            load_failures(&rig, i),
            vec!["Host".to_string()],
            "machine {i}"
        );
        assert!(
            !rig.has_event(i, is_failed),
            "machine {i}: {:?}",
            rig.events[i]
        );
    }
    let mut closed = rig.closed.clone();
    closed.sort_unstable();
    assert_eq!(closed, vec![1, 2]);
    rig.host.load_failed("once more".into());
    assert!(rig.host.outbox().is_empty());
    assert!(rig.host.events().is_empty());
}

// QUIRK E-263: deviation
#[test]
fn wrong_versions_are_rejected() {
    let own = hello("Host");
    let cases: Vec<(&str, Hello)> = vec![
        (
            "network version",
            Hello {
                net_version: NET_VERSION + 1,
                ..own.clone()
            },
        ),
        (
            "protocol version",
            Hello {
                protocol: 41,
                ..own.clone()
            },
        ),
        (
            "format version",
            Hello {
                format: 28,
                ..own.clone()
            },
        ),
        (
            "game edition",
            Hello {
                edition: "en".into(),
                ..own.clone()
            },
        ),
    ];
    for (what, bad) in cases {
        let mut host = HostCore::new(
            own.clone(),
            book(TurnConfig::default(), 8),
            Box::new(AcceptAll),
        );
        host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(bad)));
        let out = host.outbox();
        assert_eq!(out.len(), 1, "{what}: {out:?}");
        match &out[0] {
            (1, NetMessage::Reject { reason }) => {
                assert!(reason.contains(what), "{what}: {reason}")
            }
            other => panic!("{what}: expected Reject, got {other:?}"),
        }
        assert_eq!(host.peers().len(), 1, "{what}: not accepted");
        assert!(host.events().is_empty(), "{what}");
    }
    // The name does not count.
    let mut host = HostCore::new(
        own.clone(),
        book(TurnConfig::default(), 8),
        Box::new(AcceptAll),
    );
    host.handle(
        0,
        3,
        LinkEvent::Message(NetMessage::Hello(Hello {
            name: "Somebody else".into(),
            ..own.clone()
        })),
    );
    // `Welcome` with the connection number, then the new lobby state (row 1).
    let out = host.outbox();
    assert_eq!(out[0], (3, NetMessage::Welcome { slot: 3 }));
    assert!(
        matches!(&out[1..], [(3, NetMessage::Lobby(state))] if state.rows[1].peer == Some(3)),
        "{out:?}"
    );
    assert_eq!(
        host.events(),
        vec![
            CoreEvent::Joined {
                peer: 3,
                name: "Somebody else".into()
            },
            CoreEvent::LobbyChanged
        ]
    );
}

// QUIRK E-287: kept
#[test]
fn full_or_started_session_rejects() {
    let own = hello("Host");
    let mut host = HostCore::new(
        own.clone(),
        book(TurnConfig::default(), 2),
        Box::new(AcceptAll),
    );
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello("A"))));
    host.handle(0, 2, LinkEvent::Message(NetMessage::Hello(hello("B"))));
    let out = host.outbox();
    assert_eq!(out[0], (1, NetMessage::Welcome { slot: 1 }));
    assert!(matches!(&out[1], (1, NetMessage::Lobby(_))), "{out:?}");
    assert!(
        matches!(&out[2], (2, NetMessage::Reject { reason }) if reason.contains("full")),
        "{out:?}"
    );
    // After the start: "match is already running".
    let mut host = HostCore::new(own, book(TurnConfig::default(), 8), Box::new(AcceptAll));
    host.start(|peer| Rig::info(&[0], TurnConfig::default(), peer));
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello("Late"))));
    let out = host.outbox();
    assert!(
        matches!(&out[..], [(1, NetMessage::Reject { reason })] if reason.contains("running")),
        "{out:?}"
    );
}

#[test]
fn invalid_raster_fails_without_panic() {
    let mut client = ClientCore::new(hello("C"));
    assert_eq!(
        client.outbox(),
        vec![(HOST_PEER, NetMessage::Hello(hello("C")))]
    );
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
    );
    let mut info = Rig::info(&[0, 1], TurnConfig::default(), 1);
    info.spacing = 2;
    info.lead = 2;
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Start(Box::new(info))),
    );
    assert_eq!(client.phase(), &Phase::Ended);
    let events = client.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::Failed(t) if t.contains("turn raster"))),
        "{events:?}"
    );
    assert!(
        client
            .outbox()
            .iter()
            .any(|(_, m)| matches!(m, NetMessage::Bye { .. }))
    );
}

/// A game speed above `MAX_GAME_SPEED` in `Start` is, like a wrong turn length, an invalid start: `Bye` to the host
/// and `Failed` with the reason as text.
#[test]
fn invalid_game_speed_fails_the_start() {
    for (speed, ok) in [
        (MAX_GAME_SPEED, true),
        (MAX_GAME_SPEED + 1, false),
        (u8::MAX, false),
    ] {
        let mut client = ClientCore::new(hello("C"));
        client.outbox();
        client.handle(
            0,
            HOST_PEER,
            LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
        );
        let mut info = Rig::info(&[0, 1], TurnConfig::default(), 1);
        info.game_speed = speed;
        client.handle(
            0,
            HOST_PEER,
            LinkEvent::Message(NetMessage::Start(Box::new(info))),
        );
        let events = client.events();
        let bye = client
            .outbox()
            .iter()
            .any(|(_, m)| matches!(m, NetMessage::Bye { .. }));
        if ok {
            assert_eq!(client.phase(), &Phase::Loading, "Game speed {speed}");
            assert!(!bye, "Game speed {speed}");
        } else {
            assert_eq!(client.phase(), &Phase::Ended, "Game speed {speed}");
            assert!(bye, "Game speed {speed}");
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, CoreEvent::Failed(t) if t.contains("Game speed"))),
                "Game speed {speed}: {events:?}"
            );
        }
    }
}

// QUIRK E-255: deviation
#[test]
fn start_carries_the_raster() {
    let cfg = TurnConfig::new(4, 2).unwrap();
    let mut rig = Rig::running_with(cfg, &[20, 60]);
    for c in &rig.clients {
        assert_eq!(c.config(), cfg, "client adopts 4/2 from StartInfo");
    }
    assert_eq!(rig.host.config(), cfg);
    rig.run_until(cfg.exec_step(2) + 1);
    rig.assert_same_execution();
}

// QUIRK E-255, E-256: deviation
#[test]
fn commands_execute_at_the_raster() {
    for l in [2, 1, 3] {
        let cfg = TurnConfig::from_turn_steps(l).unwrap();
        let mut rig = Rig::running_with(cfg, &[20, 60]);
        assert_eq!(rig.clients[0].config(), cfg, "grid from Start");
        rig.run_until(1);
        let c = cmd(0, 100);
        let d = cmd(1, 101);
        rig.host.submit(c.clone());
        rig.clients[0].submit(d.clone());
        rig.run_until(cfg.exec_step(3) + 1);
        // Input at tick 1 (after b(0), before b(1)) takes effect at e(1) - the same for host and client.
        for i in 0..3 {
            assert_eq!(
                rig.nonempty(i),
                vec![(cfg.exec_step(1), vec![c.clone(), d.clone()])],
                "L = {l}, machine {i}"
            );
        }
        if l == 2 {
            assert_eq!(cfg.exec_step(1), 12);
        }
        rig.assert_same_execution();

        rig.host.forget_before(2);
        assert!(rig.host.bundle(1).is_none());
        assert!(rig.host.bundle(2).is_some());
        rig.clients[1].forget_before(2);
        assert!(rig.clients[1].bundle(1).is_none());
        assert!(rig.clients[1].bundle(2).is_some());
    }
}

// QUIRK E-257: deviation
#[test]
fn bundle_orders_by_slot_then_input() {
    // (slot host, slot client) -> expected order. With the host on slot 0 the slot order contradicts the arrival order,
    // with the host on slot 2 it contradicts "host first".
    let a = cmd(1, 1);
    let b = cmd(1, 2);
    let c = cmd(0, 3);
    let cases = [
        (vec![0, 1], vec![c.clone(), a.clone(), b.clone()]),
        (vec![2, 1], vec![a.clone(), b.clone(), c.clone()]),
    ];
    for (slots, expected) in cases {
        let mut rig = Rig::new(&[10]);
        rig.slots = slots.clone();
        rig.go();
        // The host computes more slowly: the contribution of the client arrives before that of the host.
        rig.machines[0].tick_ms = 200;
        rig.run_until(1);
        rig.clients[0].submit(a.clone());
        rig.clients[0].submit(b.clone());
        rig.host.submit(c.clone());
        rig.run_until(13);
        let client_closed = rig.machines[1].closed_at[1];
        let host_closed = rig.machines[0].closed_at[1];
        assert!(
            client_closed + 10 < host_closed,
            "client first: {client_closed} / {host_closed}"
        );
        for i in 0..2 {
            assert_eq!(
                rig.nonempty(i),
                vec![(12, expected.clone())],
                "slots {slots:?}, machine {i}"
            );
        }
        rig.assert_same_execution();
    }
}

// QUIRK E-256: deviation
#[test]
fn missing_contribution_blocks_the_exec_step() {
    let mut rig = Rig::running(&[0, 2_000]);
    // All stand at tick 0. The contribution 0 of client 2 needs 2 s to the host.
    rig.run_for(1_900);
    assert_eq!(
        rig.steps(),
        vec![4, 4, 4],
        "nobody computes e(0) without a bundle"
    );
    assert!(rig.host.bundle(0).is_none());
    assert!(rig.clients[0].bundle(0).is_none());
    // Bundle 0 at the host from about 2,025 ms: host and client 1 compute on up to e(1) and wait there for contribution
    // 1 of client 2; client 2 still waits for its bundle 0 (on its way until about 4,025 ms).
    rig.run_for(800);
    assert!(rig.host.bundle(0).is_some());
    assert_eq!(rig.steps(), vec![12, 12, 4]);
    assert!(rig.clients[1].bundle(0).is_none());
    // It continues at the pace of the latency: a round trip of 4 s per turn.
    rig.run_for(10_000);
    rig.assert_same_execution();
    let steps = rig.steps();
    assert!(steps.iter().all(|&s| s > 12), "{steps:?}");
}

// QUIRK E-256: deviation
#[test]
fn slow_peer_bounds_the_lead() {
    for l in [1, 2, 3] {
        let cfg = TurnConfig::from_turn_steps(l).unwrap();
        let mut rig = Rig::running_with(cfg, &[30, 80]);
        // Client 2 computes three times more slowly (low frame rate).
        rig.machines[2].tick_ms = 150;
        rig.run_for(20_000);
        let bound = cfg.spacing() + cfg.lead();
        assert!(
            rig.max_lead <= bound,
            "L = {l}: distance {} > {bound}",
            rig.max_lead
        );
        assert!(
            rig.max_lead > cfg.lead(),
            "L = {l}: the slow one visibly holds back ({})",
            rig.max_lead
        );
        // The host runs at the pace of the slowest.
        let steps = rig.steps();
        assert!(steps[0] <= steps[2] + bound, "{steps:?}");
        assert!(steps[2] > 100, "{steps:?}");
        rig.assert_same_execution();
    }
}

// QUIRK E-293: deviation
#[test]
fn turn_before_go_is_ignored() {
    let mut rig = Rig::new(&[10]);
    rig.frames_until(|r| r.clients[0].peer().is_some());
    let bogus = |turn| {
        NetMessage::Turn(TurnContribution {
            turn,
            hash: 0,
            commands: vec![cmd(1, 99)],
            wishes: Vec::new(),
        })
    };
    // In `Gathering` and `Loading` the host discards a contribution without a dropout.
    rig.client_links[0].send(bogus(0));
    rig.frames_until(|r| r.now >= 100);
    rig.start();
    rig.frames_until(|r| r.clients[0].phase() == &Phase::Loading);
    rig.client_links[0].send(bogus(0));
    rig.client_links[0].send(bogus(1));
    // `close_round` before `Go` has no effect on the client.
    rig.clients[0].submit(cmd(1, 98));
    rig.clients[0].close_round(0, 0);
    assert!(rig.clients[0].outbox().is_empty());
    rig.host.close_round(0, 0);
    rig.frames_until(|r| r.now >= 300);
    assert!(
        !rig.has_event(0, |e| matches!(
            e,
            CoreEvent::Lost { .. } | CoreEvent::Failed(_)
        )),
        "{:?}",
        rig.events[0]
    );
    assert_eq!(rig.host.phase(), &Phase::Loading);

    rig.host.loaded_self();
    rig.clients[0].loaded(None);
    rig.frames_until(|r| r.all_in(&Phase::Running));
    rig.run_until(30);
    // The real contribution 0 was accepted, nothing of the discarded one got into the bundle.
    assert!(rig.nonempty(0).is_empty(), "{:?}", rig.nonempty(0));
    assert!(rig.nonempty(1).is_empty());
    rig.assert_same_execution();
}

// ---------------------------------------------------------------------------------------------------------------
// Wishes, pause, sender check, dropout
// ---------------------------------------------------------------------------------------------------------------

/// Pause from slot 1 at tick 1 -> halt in bundle 1; everyone stands after `e(1)`.
fn paused_by_client_1(delays: &[u64]) -> Rig {
    let mut rig = Rig::running(delays);
    rig.run_until(1);
    rig.clients[0].wish(Wish::Pause);
    rig.run_for(3_000);
    rig
}

// QUIRK E-264: deviation
#[test]
fn pause_halts_everyone_after_the_same_exec_step() {
    let mut rig = paused_by_client_1(&[20, 70]);
    let cfg = rig.host.config();
    let halt = NetControl::Halt { by: 1 };
    assert_eq!(
        rig.host.bundle(1).map(|b| b.control.clone()),
        Some(vec![halt.clone()])
    );
    for i in 0..3 {
        assert_eq!(rig.controls(i), vec![(1, halt.clone())], "machine {i}");
        assert_eq!(
            rig.phase(i),
            &Phase::Halted(HaltReason::Pause { by: 1 }),
            "machine {i}"
        );
        assert_eq!(
            rig.machines[i].halted_at,
            vec![cfg.exec_step(1) + 1],
            "machine {i}"
        );
    }
    // The pause holds: even long afterwards nobody computes on.
    rig.run_for(10_000);
    assert_eq!(rig.steps(), vec![cfg.exec_step(1) + 1; 3]);
    rig.assert_same_execution();
    // A pause wish during the pause has no effect.
    rig.clients[1].wish(Wish::Pause);
    rig.host.wish(Wish::Pause);
    rig.clients[0].resume();
    rig.run_for(3_000);
    assert_eq!(rig.controls(0), vec![(1, halt.clone())]);
    assert!(rig.host.bundle(2).is_some_and(|b| b.control.is_empty()));
    assert!(rig.steps().iter().all(|&s| s > cfg.exec_step(2)));
}

// QUIRK O-316: kept
#[test]
fn only_the_pauser_resumes() {
    let mut rig = paused_by_client_1(&[20, 70]);
    let halted = Phase::Halted(HaltReason::Pause { by: 1 });
    // Slot 2 and the host (slot 0) do not lift it, not even with a Resume past the check.
    rig.clients[1].resume();
    assert!(rig.clients[1].outbox().is_empty());
    rig.client_links[1].send(NetMessage::Resume);
    rig.host.resume();
    rig.run_for(2_000);
    for i in 0..3 {
        assert!(!rig.has_event(i, is_resumed), "machine {i}");
        assert_eq!(rig.phase(i), &halted, "machine {i}");
    }
    assert_eq!(rig.steps(), vec![13; 3]);
    // Slot 1 lifts it: exactly one `Resumed` everywhere, all compute on.
    rig.clients[0].resume();
    rig.run_for(3_000);
    for i in 0..3 {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert!(rig.steps().iter().all(|&s| s > 20), "{:?}", rig.steps());
    rig.assert_same_execution();

    // The pause of the host (slot 0) is lifted only by the host.
    let mut rig = Rig::running(&[20, 70]);
    rig.run_until(1);
    rig.host.wish(Wish::Pause);
    rig.run_for(3_000);
    for i in 0..3 {
        assert_eq!(rig.controls(i), vec![(1, NetControl::Halt { by: 0 })]);
    }
    rig.clients[0].resume();
    rig.run_for(1_000);
    assert!(!rig.has_event(0, is_resumed));
    rig.host.resume();
    rig.run_for(3_000);
    for i in 0..3 {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
    }
    rig.assert_same_execution();
}

// QUIRK E-294: kept
// QUIRK E-265: deviation
#[test]
fn speed_only_from_host() {
    let mut rig = Rig::running(&[20, 50]);
    let cfg = rig.host.config();
    rig.run_until(1);
    rig.clients[0].wish(Wish::Speed(9));
    // Turn 1 is closed everywhere (b(1) = 8); the next wish travels in turn 2.
    rig.run_until(9);
    rig.host.wish(Wish::Speed(7));
    rig.run_until(17);
    // Above the maximum game speed of the game host (slider 0 ... 20): discarded.
    rig.host.wish(Wish::Speed(MAX_GAME_SPEED + 1));
    rig.run_until(cfg.exec_step(3) + 1);
    assert_eq!(rig.host.bundle(1).map(|b| b.control.clone()), Some(vec![]));
    assert_eq!(rig.host.bundle(3).map(|b| b.control.clone()), Some(vec![]));
    assert_eq!(
        rig.host.bundle(2).map(|b| b.control.clone()),
        Some(vec![NetControl::Speed(7)])
    );
    for i in 0..3 {
        assert_eq!(
            rig.controls(i),
            vec![(2, NetControl::Speed(7))],
            "machine {i}"
        );
    }
    rig.assert_same_execution();
}

// QUIRK E-294: kept
#[test]
fn one_wish_per_kind_and_round() {
    let mut rig = Rig::running(&[10, 30, 60]);
    rig.run_until(1);
    rig.clients[1].wish(Wish::Pause);
    rig.clients[1].wish(Wish::Save("two".into()));
    rig.clients[0].wish(Wish::Pause);
    rig.clients[0].wish(Wish::Pause);
    rig.clients[0].wish(Wish::Save("one".into()));
    rig.clients[0].wish(Wish::Save("one b".into()));
    rig.clients[2].wish(Wish::Save("three".into()));
    rig.host.wish(Wish::Speed(7));
    rig.host.wish(Wish::Speed(9));
    rig.run_for(3_000);
    // One per kind, the smallest slot; order: game speed, save, halt.
    let expected = vec![
        NetControl::Speed(7),
        NetControl::Save { name: "one".into() },
        NetControl::Halt { by: 1 },
    ];
    assert_eq!(
        rig.host.bundle(1).map(|b| b.control.clone()),
        Some(expected.clone())
    );
    for i in 0..4 {
        let got: Vec<NetControl> = rig.controls(i).into_iter().map(|(_, c)| c).collect();
        assert_eq!(got, expected, "machine {i}");
        assert!(rig.controls(i).iter().all(|(t, _)| *t == 1));
    }
    rig.assert_same_execution();
}

#[test]
fn save_wish_from_any_player() {
    let mut rig = Rig::running(&[20, 40]);
    let cfg = rig.host.config();
    rig.run_until(1);
    rig.clients[1].wish(Wish::Save("from two".into()));
    rig.run_until(9);
    rig.host.wish(Wish::Save("from the host".into()));
    rig.run_until(17);
    rig.clients[0].wish(Wish::Save("from one".into()));
    rig.run_until(cfg.exec_step(4) + 1);
    let save = |n: &str| NetControl::Save { name: n.into() };
    let expected = vec![
        (1, save("from two")),
        (2, save("from the host")),
        (3, save("from one")),
    ];
    for i in 0..3 {
        assert_eq!(rig.controls(i), expected, "machine {i}");
        assert!(rig.machines[i].halted_at.is_empty(), "saving does not halt");
    }
    rig.assert_same_execution();
}

/// A name with a dot or space at the end is not a network savegame. Windows strips both when opening (`CON.` and
/// `NUL ` are then the devices, `name.` would overwrite `name`), and network savegames have no extension that could
/// hide this.
// QUIRK E-274: kept
#[test]
fn save_name_with_trailing_dot_or_space_is_rejected() {
    for bad in [
        "CON.",
        "NUL ",
        "nul. ",
        "Savegame.",
        "Savegame ",
        "a. .",
        " ",
    ] {
        assert!(!valid_save_name(bad), "{bad:?}");
    }
    for good in ["Savegame 1", "a.b", " Start", "x.y z"] {
        assert!(valid_save_name(good), "{good:?}");
    }
}

#[test]
fn foreign_save_name_is_rejected() {
    let long_ok = "x".repeat(64);
    let long_bad = "x".repeat(65);
    for good in [
        "Savegame 1",
        "netz_2026_11_05_1430",
        "Übung",
        "a.b",
        long_ok.as_str(),
        "CONSOLE",
        "Nullable",
        "COM10",
        "LPT0",
        "my.CON",
        "CONIN",
        "CONOUT",
        "COM\u{2074}",
        "LPT\u{B4}",
    ] {
        assert!(valid_save_name(good), "{good:?}");
    }
    for bad in [
        "",
        ".",
        "..",
        "../evil",
        "a..b",
        "a/b",
        "a\\b",
        "C:evil",
        "a\nb",
        "a\u{7}b",
        "a\u{0}b",
        long_bad.as_str(),
        "CON",
        "nul",
        "Aux.sav",
        "prn.tar.gz",
        "COM1",
        "com9.x",
        "LPT1",
        "lpt9",
        "CONIN$",
        "conout$.txt",
        "COM\u{B9}",
        "com\u{B2}.sav",
        "COM\u{B3}",
        "LPT\u{B9}",
        "lpt\u{B2}",
        "LPT\u{B3}.x",
        "a<b",
        "a>b",
        "a\"b",
        "a|b",
        "a?b",
        "a*b",
    ] {
        assert!(!valid_save_name(bad), "{bad:?}");
    }
    // An invalid name drops out, the next valid one (larger slot) applies.
    let mut rig = Rig::running(&[20, 40]);
    let cfg = rig.host.config();
    rig.run_until(1);
    rig.clients[0].wish(Wish::Save("../../evil".into()));
    rig.clients[1].wish(Wish::Save("good".into()));
    rig.run_until(9);
    rig.host.wish(Wish::Save("C:\\Windows\\evil".into()));
    rig.run_until(cfg.exec_step(3) + 1);
    for i in 0..3 {
        assert_eq!(
            rig.controls(i),
            vec![(
                1,
                NetControl::Save {
                    name: "good".into()
                }
            )],
            "machine {i}"
        );
    }
    rig.assert_same_execution();
}

#[test]
fn foreign_sender_is_dropped() {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(1);
    rig.clients[0].submit(cmd(2, 5));
    rig.clients[0].submit(cmd(1, 6));
    rig.clients[0].submit(cmd(0, 7));
    rig.run_until(13);
    for i in 0..3 {
        assert_eq!(rig.nonempty(i), vec![(12, vec![cmd(1, 6)])], "machine {i}");
        assert!(!rig.has_event(i, is_lost), "no dropout");
    }
    rig.assert_same_execution();
}

#[test]
fn mirror_commands_from_clients_are_dropped() {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(1);
    let mirror = |player, kind| Command { player, kind };
    rig.clients[0].submit(mirror(1, CommandKind::ScriptSelection { ids: Vec::new() }));
    rig.clients[0].submit(mirror(1, CommandKind::ScriptButton { word: 3 }));
    rig.clients[0].submit(cmd(1, 6));
    let host_button = mirror(0, CommandKind::ScriptButton { word: 4 });
    rig.host.submit(host_button.clone());
    rig.run_until(13);
    for i in 0..3 {
        assert_eq!(
            rig.nonempty(i),
            vec![(12, vec![host_button.clone(), cmd(1, 6)])],
            "machine {i}"
        );
    }
    rig.assert_same_execution();
}

// QUIRK E-267, E-272: kept
// QUIRK E-258: deviation
#[test]
fn closed_link_defeats_the_slot_once() {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(5);
    rig.client_links[0].cut();
    rig.frames_until(|r| r.has_event(0, is_lost));
    let lost = rig.losses(0);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!((lost[0].0, lost[0].1), (1, Some(1)));
    // A second `Closed` and a `Bye` from the same peer change nothing.
    let now = rig.now;
    rig.host
        .handle(now, 1, LinkEvent::Closed("closed again".into()));
    rig.host.handle(
        now,
        1,
        LinkEvent::Message(NetMessage::Bye {
            reason: "bye".into(),
        }),
    );
    rig.run_for(5_000);
    assert_eq!(rig.count_events(0, is_lost), 1);
    // Exactly one dropout command, in the next bundle (1, at e(1) = 12), on host and client 2.
    for i in [0, 2] {
        assert_eq!(rig.nonempty(i), vec![(12, vec![loss(1)])], "machine {i}");
        assert_eq!(rig.executed_count(i, &loss(1)), 1);
    }
    // Afterwards nobody waits for slot 1.
    assert!(
        rig.machines[0].steps > 60 && rig.machines[2].steps > 60,
        "{:?}",
        rig.steps()
    );
    assert_eq!(rig.clients[0].phase(), &Phase::Ended);
    rig.assert_same_execution();

    // If the slot already has a result, the command is omitted; the dropout is still reported.
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(5);
    let mut defeated = [false; 8];
    defeated[1] = true;
    rig.host.set_results(defeated);
    rig.client_links[0].cut();
    rig.run_for(5_000);
    assert_eq!(rig.count_events(0, is_lost), 1);
    for i in [0, 2] {
        assert!(
            rig.nonempty(i).is_empty(),
            "machine {i}: {:?}",
            rig.nonempty(i)
        );
    }
    assert!(rig.machines[0].steps > 60, "{:?}", rig.steps());
    rig.assert_same_execution();

    // If the slot gives up itself in the same bundle (contribution came before the `Bye`), the dropout command is
    // omitted too.
    let rig = leaves_after_round_1(|c| c.submit(loss(1)));
    for i in [0, 2] {
        assert_eq!(rig.nonempty(i), vec![(12, vec![loss(1)])], "machine {i}");
    }

    // A victory command (`result: 2`, rejected by the simulation in the network) is not giving up: the dropout command
    // follows anyway.
    let win = Command {
        player: 1,
        kind: CommandKind::SetPlayerResult { slot: 1, result: 2 },
    };
    let rig = leaves_after_round_1(|c| c.submit(win.clone()));
    for i in [0, 2] {
        assert_eq!(
            rig.nonempty(i),
            vec![(12, vec![win.clone(), loss(1)])],
            "machine {i}"
        );
    }
}

/// Client 1 puts something into its contribution 1 (`input`) at tick 1 and leaves with `Bye` after the contribution is
/// at the host but before the host (slowly) can form bundle 1; then 3 s on.
fn leaves_after_round_1(input: impl FnOnce(&mut ClientCore)) -> Rig {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(1);
    input(&mut rig.clients[0]);
    rig.machines[0].tick_ms = 400;
    while rig.machines[1].next_close < 2 {
        rig.frame(u64::MAX);
    }
    rig.client_links[0].send(NetMessage::Bye {
        reason: "left".into(),
    });
    rig.frames_until(|r| r.has_event(0, is_lost));
    assert!(
        rig.host.bundle(1).is_none(),
        "bundle 1 only after the dropout"
    );
    assert_eq!(rig.host.buffered_rounds(), vec![(1, vec![1])]);
    rig.machines[0].tick_ms = TICK_MS;
    rig.run_for(3_000);
    rig
}

// QUIRK E-264: deviation
#[test]
fn pauser_lost_before_the_halt_bundle_does_not_halt() {
    // Pause in contribution 1, then `Bye` before bundle 1 is formed: the contribution counts, the pause does not.
    let rig = leaves_after_round_1(|c| c.wish(Wish::Pause));
    assert_eq!(rig.host.bundle(1).map(|b| b.control.clone()), Some(vec![]));
    for i in [0, 2] {
        assert!(
            rig.controls(i).is_empty(),
            "machine {i}: {:?}",
            rig.controls(i)
        );
        assert!(rig.machines[i].halted_at.is_empty(), "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
        assert_eq!(rig.nonempty(i), vec![(12, vec![loss(1)])], "machine {i}");
    }
    assert!(
        rig.machines[0].steps > 30 && rig.machines[2].steps > 30,
        "{:?}",
        rig.steps()
    );
}

// QUIRK E-267, E-272: kept
#[test]
fn losses_follow_commands_in_slot_order() {
    let mut rig = Rig::running(&[10, 20, 30]);
    rig.run_until(5);
    rig.host.submit(cmd(0, 1));
    rig.clients[2].submit(cmd(3, 2));
    // Slot 2 aborts, then slot 1 leaves with `Bye` (and closes; the `Closed` afterwards does not count twice).
    rig.client_links[1].cut();
    rig.frames_until(|r| r.has_event(0, is_lost));
    rig.client_links[0].send(NetMessage::Bye {
        reason: "Player has left".into(),
    });
    rig.client_links[0].close("left");
    rig.frames_until(|r| r.count_events(0, is_lost) == 2 && r.now > 200);
    let order: Vec<Peer> = rig.losses(0).iter().map(|l| l.0).collect();
    assert_eq!(order, vec![2, 1], "dropout reported in arrival order");
    rig.run_for(3_000);
    assert_eq!(rig.count_events(0, is_lost), 2);
    let expected = vec![(12, vec![cmd(0, 1), cmd(3, 2), loss(1), loss(2)])];
    for i in [0, 3] {
        assert_eq!(rig.nonempty(i), expected, "machine {i}");
    }
}

// QUIRK E-267: kept
// QUIRK E-258: deviation
#[test]
fn silence_for_30s_is_a_loss() {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(5);
    // Client 1 sends nothing any more, not even signs of life.
    rig.client_links[0].mute(true);
    let silent = |r: &Rig| {
        let link = &r.host_links[0].1;
        link.age_ms() - link.last_received_ms()
    };
    while silent(&rig) + FRAME_MS < TIMEOUT_MS {
        rig.frame(5);
    }
    let t = rig.now + (TIMEOUT_MS - 1 - silent(&rig));
    rig.pump_at(t);
    assert_eq!(silent(&rig), TIMEOUT_MS - 1);
    assert!(!rig.has_event(0, is_lost), "29,999 ms: no dropout");
    rig.pump_at(t + 1);
    assert_eq!(silent(&rig), TIMEOUT_MS);
    let lost = rig.losses(0);
    assert_eq!(lost.len(), 1, "30,000 ms: dropout");
    assert_eq!((lost[0].0, lost[0].1), (1, Some(1)));
    assert!(lost[0].2.contains("30 s"), "{}", lost[0].2);
    // The muted client gets a `Bye` and ends; the others play on with the dropout command.
    rig.run_for(3_000);
    assert_eq!(rig.clients[0].phase(), &Phase::Ended);
    for i in [0, 2] {
        assert_eq!(rig.nonempty(i), vec![(12, vec![loss(1)])], "machine {i}");
    }
    assert!(rig.machines[0].steps > 40, "{:?}", rig.steps());
    rig.assert_same_execution();

    // The client evaluates the silence toward the host the same way.
    let mut client = ClientCore::new(hello("C"));
    client.outbox();
    client.silence(0, HOST_PEER, TIMEOUT_MS - 1);
    assert_eq!(client.phase(), &Phase::Gathering);
    assert!(client.events().is_empty());
    client.silence(0, HOST_PEER, TIMEOUT_MS);
    assert_eq!(client.phase(), &Phase::Ended);
    // The text is `HOST_LOST` in every phase, the reason ("30 s") is in the `Bye` and in the log.
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);
    assert!(matches!(
        &client.outbox()[..],
        [(HOST_PEER, NetMessage::Bye { reason })] if reason.contains("30 s")
    ));
}

// QUIRK O-316: kept
// QUIRK E-264: deviation
#[test]
fn pauser_lost_lifts_the_pause() {
    let mut rig = paused_by_client_1(&[20, 40]);
    assert_eq!(rig.steps(), vec![13; 3]);
    rig.client_links[0].cut();
    rig.run_for(3_000);
    assert_eq!(rig.losses(0).len(), 1);
    for i in [0, 2] {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
        // The dropout command is in the next bundle (2, at e(2) = 20).
        assert_eq!(rig.nonempty(i), vec![(20, vec![loss(1)])], "machine {i}");
    }
    assert!(
        rig.machines[0].steps > 30 && rig.machines[2].steps > 30,
        "{:?}",
        rig.steps()
    );
    rig.assert_same_execution();
}

// QUIRK E-267: kept
#[test]
fn loss_before_go_aborts_the_start() {
    let mut rig = Rig::new(&[10, 20, 30]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    // In `Gathering` the host merely removes the machine from the list.
    rig.client_links[1].cut();
    rig.frames_until(|r| r.has_event(0, is_lost));
    assert_eq!(rig.losses(0)[0].0, 2);
    assert_eq!(rig.losses(0)[0].1, None);
    assert_eq!(rig.host.phase(), &Phase::Gathering);
    let peers: Vec<Peer> = rig.host.peers().iter().map(|p| p.0).collect();
    assert_eq!(peers, vec![0, 1, 3]);
    // In `Loading` a dropout aborts the start for everyone, like a load error.
    rig.start();
    rig.frames_until(|r| r.clients[2].phase() == &Phase::Loading);
    rig.host.loaded_self();
    rig.clients[2].loaded(None);
    rig.client_links[0].cut();
    rig.frames_until(|r| r.host.phase() == &Phase::Ended && r.clients[2].phase() == &Phase::Ended);
    for i in [0, 3] {
        let failed: Vec<&String> = rig.events[i]
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Failed(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(failed.len(), 1, "machine {i}: {:?}", rig.events[i]);
        assert!(failed[0].contains("Client 1"), "{}", failed[0]);
        assert!(!rig.has_event(i, |e| e == &CoreEvent::Go));
    }
    assert_eq!(
        rig.count_events(0, is_lost),
        1,
        "only the dropout before the start"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Checksum comparison, resync, stop
// ---------------------------------------------------------------------------------------------------------------

fn is_send_state(e: &CoreEvent) -> bool {
    matches!(e, CoreEvent::SendState { .. })
}

fn is_resync_state(e: &CoreEvent) -> bool {
    matches!(e, CoreEvent::ResyncState { .. })
}

fn resync(slots: &[u8]) -> NetControl {
    NetControl::Resync {
        slots: slots.to_vec(),
    }
}

const RESYNC_HALT: Phase = Phase::Halted(HaltReason::Resync);

// QUIRK E-260: deviation
#[test]
fn equal_hashes_never_resync() {
    let mut rig = Rig::running(&[20, 60]);
    rig.run_until(400);
    for i in 0..3 {
        assert!(
            rig.controls(i).is_empty(),
            "machine {i}: {:?}",
            rig.controls(i)
        );
        assert!(rig.machines[i].halted_at.is_empty(), "machine {i}");
        assert!(!rig.has_event(i, |e| is_send_state(e) || is_resync_state(e)));
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert_eq!(rig.host.desyncs(), 0);
    rig.assert_same_execution();
}

// QUIRK E-260: deviation
#[test]
fn first_desync_resyncs_the_deviant_only() {
    let mut rig = Rig::running(&[20, 60]);
    let cfg = rig.host.config();
    let stand = cfg.exec_step(2) + 1;
    // Client 2 (slot 2) reports a wrong checksum for turn 2; it adopts only when told.
    rig.machines[2].wrong_rounds = vec![2];
    rig.machines[2].hold_sync = true;
    rig.run_for(3_000);
    assert_eq!(
        rig.host.bundle(2).map(|b| b.control.clone()),
        Some(vec![resync(&[2])])
    );
    for i in 0..3 {
        assert_eq!(rig.controls(i), vec![(2, resync(&[2]))], "machine {i}");
        assert_eq!(rig.phase(i), &RESYNC_HALT, "machine {i}");
        assert_eq!(rig.machines[i].halted_at, vec![stand], "machine {i}");
    }
    assert_eq!(rig.steps(), vec![stand; 3]);
    assert_eq!(rig.host.desyncs(), 1);
    assert_eq!(
        rig.events[0]
            .iter()
            .filter(|e| is_send_state(e))
            .collect::<Vec<_>>(),
        vec![&CoreEvent::SendState { slots: vec![2] }]
    );
    // Only the deviant gets the state of the host (in pieces, unaltered).
    assert_eq!(
        rig.events[2]
            .iter()
            .filter(|e| is_resync_state(e))
            .collect::<Vec<_>>(),
        vec![&CoreEvent::ResyncState {
            bytes: state_bytes(stand)
        }]
    );
    assert!(!rig.has_event(1, is_resync_state));
    // A non-target reports nothing.
    rig.clients[0].synced();
    assert!(rig.clients[0].outbox().is_empty());

    rig.machines[2].hold_sync = false;
    rig.run_for(3_000);
    assert_eq!(rig.machines[2].adopted, vec![(stand, state_bytes(stand))]);
    for i in 0..3 {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert!(rig.steps().iter().all(|&s| s > 40), "{:?}", rig.steps());
    assert_eq!(rig.host.desyncs(), 1);
    assert_eq!(rig.controls(0), vec![(2, resync(&[2]))]);
    rig.assert_same_execution();
}

// QUIRK E-261: deviation
#[test]
fn two_deviants_in_one_round_count_once() {
    let mut rig = Rig::new(&[20, 40, 60]);
    // Slots against the peer order: peer 1 sits on slot 3, peer 3 on slot 1.
    rig.slots = vec![0, 3, 2, 1];
    rig.go();
    rig.machines[1].wrong_rounds = vec![2];
    rig.machines[3].wrong_rounds = vec![2];
    rig.run_for(6_000);
    for i in 0..4 {
        assert_eq!(rig.controls(i), vec![(2, resync(&[1, 3]))], "machine {i}");
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert_eq!(rig.host.desyncs(), 1);
    assert_eq!(rig.count_events(0, is_send_state), 1);
    assert!(rig.has_event(0, |e| e == &CoreEvent::SendState { slots: vec![1, 3] }));
    for i in [1, 3] {
        assert_eq!(rig.count_events(i, is_resync_state), 1, "machine {i}");
    }
    assert!(!rig.has_event(2, is_resync_state));
    assert!(rig.steps().iter().all(|&s| s > 40), "{:?}", rig.steps());
    rig.assert_same_execution();
}

// QUIRK E-261: deviation
#[test]
fn third_desync_stops() {
    let mut rig = Rig::running(&[20, 60]);
    let cfg = rig.host.config();
    rig.host.set_clock(LocalTime {
        year: 2026,
        month: 11,
        day: 5,
        hour: 14,
        minute: 30,
        second: 12,
    });
    rig.machines[1].wrong_rounds = vec![2, 4, 6];
    // In turn 6 also game speed, saving and a pause: order as usual, the halt is omitted on the stop.
    rig.run_until(cfg.round_step(5) + 1);
    rig.host.wish(Wish::Speed(7));
    rig.host.wish(Wish::Save("before".into()));
    rig.clients[0].wish(Wish::Pause);
    rig.run_for(10_000);
    let name = "checksumerror_2026_11_05_1430".to_string();
    let stop = NetControl::Stop { name: name.clone() };
    let expected = vec![
        (2, resync(&[1])),
        (4, resync(&[1])),
        (6, NetControl::Speed(7)),
        (
            6,
            NetControl::Save {
                name: "before".into(),
            },
        ),
        (6, stop.clone()),
    ];
    let stand = cfg.exec_step(6) + 1;
    for i in 0..3 {
        assert_eq!(rig.controls(i), expected, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Stopped, "machine {i}");
        assert_eq!(
            rig.machines[i].halted_at,
            vec![cfg.exec_step(2) + 1, cfg.exec_step(4) + 1, stand],
            "machine {i}"
        );
        assert_eq!(rig.count_events(i, is_resumed), 2, "machine {i}");
    }
    assert_eq!(rig.host.desyncs(), 3);
    // Two resyncs, the third desync does not resync.
    assert_eq!(rig.count_events(0, is_send_state), 2);
    assert_eq!(rig.count_events(1, is_resync_state), 2);
    // Stopped stays stopped: no lifting, no tick any more.
    rig.clients[0].resume();
    rig.host.resume();
    rig.client_links[0].send(NetMessage::Resume);
    rig.run_for(10_000);
    for i in 0..3 {
        assert_eq!(rig.phase(i), &Phase::Stopped, "machine {i}");
        assert_eq!(rig.count_events(i, is_resumed), 2, "machine {i}");
    }
    assert_eq!(rig.steps(), vec![stand; 3]);
    assert!(rig.host.bundle(7).is_none());
    rig.assert_same_execution();

    // Without a set clock the fallback 1970-01-01 00:00 applies.
    assert_eq!(
        LocalTime::FALLBACK.checksum_error_name(),
        "checksumerror_1970_01_01_0000"
    );
    let mut rig = Rig::running(&[10]);
    rig.machines[1].wrong_rounds = vec![0, 1, 2];
    rig.run_for(10_000);
    let stop = NetControl::Stop {
        name: "checksumerror_1970_01_01_0000".into(),
    };
    for i in 0..2 {
        assert_eq!(
            rig.controls(i),
            vec![(0, resync(&[1])), (1, resync(&[1])), (2, stop.clone())],
            "machine {i}"
        );
        assert_eq!(rig.phase(i), &Phase::Stopped, "machine {i}");
    }
}

// QUIRK E-260: deviation
#[test]
fn resync_waits_for_all_targets() {
    let mut rig = Rig::running(&[20, 40, 60]);
    let stand = rig.host.config().exec_step(2) + 1;
    rig.machines[1].wrong_rounds = vec![2];
    rig.machines[2].wrong_rounds = vec![2];
    rig.machines[2].hold_sync = true;
    rig.run_for(4_000);
    // Slot 1 has adopted, slot 2 not yet: all keep standing.
    assert_eq!(rig.machines[1].adopted.len(), 1);
    assert!(rig.machines[2].adopted.is_empty());
    assert!(rig.has_event(2, is_resync_state));
    let check_halted = |rig: &Rig| {
        for i in 0..4 {
            assert_eq!(rig.phase(i), &RESYNC_HALT, "machine {i}");
            assert!(!rig.has_event(i, is_resumed), "machine {i}");
        }
        assert_eq!(rig.steps(), vec![stand; 4]);
    };
    check_halted(&rig);
    // Foreign things change nothing: `synced` and `Synced` from a non-target (slot 3), a second `Synced` from slot 1, a
    // `Resume` of a client (a resync is not a pause), a second `send_state`.
    rig.clients[2].synced();
    assert!(rig.clients[2].outbox().is_empty());
    rig.client_links[2].send(NetMessage::Synced);
    rig.client_links[0].send(NetMessage::Synced);
    rig.client_links[2].send(NetMessage::Resume);
    rig.clients[0].resume();
    rig.host.resume();
    rig.host.send_state(vec![1, 2, 3]);
    assert!(rig.host.large_outbox().is_empty());
    // Large content to the non-target and a second one to a target that has already received drop out.
    rig.host_links[2].1.send_large(90, state_bytes(1));
    rig.host_links[0].1.send_large(91, state_bytes(2));
    rig.run_for(2_000);
    check_halted(&rig);
    for i in [1, 2] {
        assert_eq!(rig.count_events(i, is_resync_state), 1, "machine {i}");
    }
    assert!(!rig.has_event(3, is_resync_state));

    rig.machines[2].hold_sync = false;
    rig.run_for(3_000);
    for i in 0..4 {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert!(rig.steps().iter().all(|&s| s > 40), "{:?}", rig.steps());
    rig.assert_same_execution();

    // If a target drops out during the resync, the host no longer waits for it.
    let mut rig = Rig::running(&[20, 40, 60]);
    rig.machines[1].wrong_rounds = vec![2];
    rig.machines[2].wrong_rounds = vec![2];
    rig.machines[2].hold_sync = true;
    rig.run_for(4_000);
    assert_eq!(rig.steps(), vec![stand; 4]);
    rig.client_links[1].cut();
    rig.run_for(3_000);
    assert_eq!(rig.losses(0).len(), 1);
    for i in [0, 1, 3] {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
        // The dropout command is in the next bundle (3, at e(3) = 28).
        assert_eq!(rig.nonempty(i), vec![(28, vec![loss(2)])], "machine {i}");
    }
    assert_eq!(rig.host.desyncs(), 1);
    rig.assert_same_execution();
}

// QUIRK E-260: deviation
#[test]
fn deviant_lost_before_the_bundle_is_no_desync() {
    // Client 1 contributes turn 1 with a wrong checksum and leaves before the slow host forms bundle 1: its
    // contribution counts, its checksum does not.
    let mut rig = Rig::running(&[20, 40]);
    rig.machines[1].wrong_rounds = vec![1];
    rig.machines[0].tick_ms = 400;
    while rig.machines[1].next_close < 2 {
        rig.frame(u64::MAX);
    }
    rig.client_links[0].send(NetMessage::Bye {
        reason: "left".into(),
    });
    rig.frames_until(|r| r.has_event(0, is_lost));
    assert!(
        rig.host.bundle(1).is_none(),
        "bundle 1 only after the dropout"
    );
    assert_eq!(rig.host.buffered_rounds(), vec![(1, vec![1])]);
    rig.machines[0].tick_ms = TICK_MS;
    rig.run_for(3_000);
    assert_eq!(rig.host.desyncs(), 0);
    for i in [0, 2] {
        assert!(
            rig.controls(i).is_empty(),
            "machine {i}: {:?}",
            rig.controls(i)
        );
        assert!(rig.machines[i].halted_at.is_empty(), "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
        assert_eq!(rig.nonempty(i), vec![(12, vec![loss(1)])], "machine {i}");
    }
}

/// Pause from slot 1 and deviation from slot 2 in turn 1; slot 2 adopts only when told.
fn pause_and_resync_in_round_1() -> Rig {
    let mut rig = Rig::running(&[20, 60]);
    rig.run_until(1);
    rig.clients[0].wish(Wish::Pause);
    rig.machines[2].wrong_rounds = vec![1];
    rig.machines[2].hold_sync = true;
    rig.run_for(3_000);
    let both = vec![(1, resync(&[2])), (1, NetControl::Halt { by: 1 })];
    for i in 0..3 {
        assert_eq!(rig.controls(i), both, "machine {i}");
        assert_eq!(rig.phase(i), &RESYNC_HALT, "machine {i}");
        assert_eq!(rig.machines[i].halted_at, vec![13], "machine {i}");
    }
    rig
}

// QUIRK E-261: deviation
#[test]
fn resync_runs_before_a_pause_in_the_same_bundle() {
    let mut rig = pause_and_resync_in_round_1();
    // During the resync the pauser lifts nothing either.
    rig.clients[0].resume();
    assert!(rig.clients[0].outbox().is_empty());
    rig.machines[2].hold_sync = false;
    rig.run_for(3_000);
    // Resync done, afterwards the match stands in the pause.
    assert_eq!(rig.machines[2].adopted.len(), 1);
    for i in 0..3 {
        assert_eq!(
            rig.phase(i),
            &Phase::Halted(HaltReason::Pause { by: 1 }),
            "machine {i}"
        );
        assert!(!rig.has_event(i, is_resumed), "machine {i}");
    }
    rig.run_for(5_000);
    assert_eq!(rig.steps(), vec![13; 3]);
    // A pause wish in this pause has no effect; only the pauser lifts it.
    rig.clients[1].wish(Wish::Pause);
    rig.host.resume();
    rig.run_for(1_000);
    assert_eq!(rig.steps(), vec![13; 3]);
    rig.clients[0].resume();
    rig.run_for(3_000);
    for i in 0..3 {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert!(rig.steps().iter().all(|&s| s > 20), "{:?}", rig.steps());
    assert!(rig.host.bundle(2).is_some_and(|b| b.control.is_empty()));
    rig.assert_same_execution();
}

// QUIRK E-261: deviation
#[test]
fn pauser_lost_during_resync_lifts_the_pause() {
    let mut rig = pause_and_resync_in_round_1();
    rig.client_links[0].cut();
    rig.run_for(1_000);
    assert_eq!(rig.losses(0).len(), 1);
    rig.machines[2].hold_sync = false;
    rig.run_for(3_000);
    for i in [0, 2] {
        assert_eq!(rig.count_events(i, is_resumed), 1, "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
        assert_eq!(rig.nonempty(i), vec![(20, vec![loss(1)])], "machine {i}");
    }
    assert!(
        rig.machines[0].steps > 30 && rig.machines[2].steps > 30,
        "{:?}",
        rig.steps()
    );
    rig.assert_same_execution();
}

// QUIRK E-261: deviation
#[test]
fn stray_resync_messages_are_dropped() {
    let mut rig = Rig::running(&[20]);
    rig.run_until(5);
    // Large content, `synced`, `Synced` and `send_state` without a running resync.
    rig.host_links[0].1.send_large(77, state_bytes(5));
    rig.clients[0].synced();
    assert!(rig.clients[0].outbox().is_empty());
    rig.client_links[0].send(NetMessage::Synced);
    rig.host.send_state(vec![9]);
    assert!(rig.host.large_outbox().is_empty());
    rig.run_until(60);
    for i in 0..2 {
        assert!(!rig.has_event(i, is_resync_state), "machine {i}");
        assert!(!rig.has_event(i, is_resumed), "machine {i}");
        assert!(rig.controls(i).is_empty(), "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Running, "machine {i}");
    }
    assert_eq!(rig.host.desyncs(), 0);
    rig.assert_same_execution();
}

/// Status line: `missing` names the slots without a contribution to the turn that the host bundles next; dropped-out
/// ones do not count, outside the match the list is empty.
#[test]
fn missing_lists_slots_without_contribution() {
    let rig = Rig::new(&[0, 0]);
    assert!(rig.host.missing().is_empty(), "empty before the start");
    let mut rig = Rig::running(&[0, 0]);
    assert_eq!(rig.host.missing(), vec![0, 1, 2]);
    rig.host.close_round(0, 0);
    assert_eq!(rig.host.missing(), vec![1, 2]);
    rig.clients[0].close_round(0, 0);
    let now = rig.now;
    rig.pump_at(now);
    rig.pump_at(now);
    assert_eq!(rig.host.missing(), vec![2]);
    rig.clients[1].close_round(0, 0);
    rig.pump_at(now);
    rig.pump_at(now);
    assert!(rig.host.bundle(0).is_some(), "bundle 0 formed");
    assert_eq!(rig.host.missing(), vec![0, 1, 2], "next turn");
    rig.host_links[1].1.cut();
    rig.pump_at(now);
    assert_eq!(
        rig.host.missing(),
        vec![0, 1],
        "dropped-out ones do not count"
    );
}

/// `HostCore::abort` sends `Bye` to all clients and ends the session (E-293); every client reports `Failed` with the
/// reason. After the end a second abort has no effect.
#[test]
fn host_abort_ends_the_session_for_all() {
    let mut rig = Rig::new(&[0, 30]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    rig.host.abort("Map is missing".into());
    assert_eq!(rig.host.phase(), &Phase::Ended);
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Ended));
    for i in 0..3 {
        let failed: Vec<&String> = rig.events[i]
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Failed(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(failed.len(), 1, "machine {i}: {failed:?}");
        assert!(
            failed[0].contains("Map is missing"),
            "machine {i}: {failed:?}"
        );
    }
    rig.host.abort("once more".into());
    assert!(rig.host.outbox().is_empty());
    assert!(rig.host.events().is_empty());
}

// ---------------------------------------------------------------------------------------------------------------
// End and abort of the host, protocol errors, leaving
// ---------------------------------------------------------------------------------------------------------------

/// Local time of the host in these tests.
const END_CLOCK: LocalTime = LocalTime {
    year: 2026,
    month: 11,
    day: 6,
    hour: 18,
    minute: 7,
    second: 9,
};

fn is_failed(e: &CoreEvent) -> bool {
    matches!(e, CoreEvent::Failed(_))
}

/// Texts of the `Failed` events of machine `i`.
fn failures(rig: &Rig, i: usize) -> Vec<String> {
    rig.events[i]
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Failed(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Orderly end (E-268): `HostCore::end` appends `End { name }` (`net_JJJJ_MM_TT_hhmm`, local time of the host) to the
/// next bundle; everyone halts after its execution tick at the same tick in `Phase::Ended`, then `Bye` to all (without
/// `Failed` at the clients). A running pause is lifted by the end beforehand so that the bundle can be formed.
// QUIRK E-268: deviation
#[test]
fn host_end_halts_everyone_after_the_end_bundle() {
    for paused in [false, true] {
        let mut rig = if paused {
            paused_by_client_1(&[20, 70])
        } else {
            let mut rig = Rig::running(&[20, 70]);
            rig.run_until(5);
            rig
        };
        rig.host.set_clock(END_CLOCK);
        assert!(rig.host.end(), "accepted");
        assert!(!rig.host.end(), "a second end has no effect");
        rig.run_for(3_000);
        let name = END_CLOCK.net_end_name(&rig.host.words().net_save_prefix);
        assert_eq!(name, "net_2026_11_06_1807");
        let turn = ended_turn(&rig.events[0]).expect("end in the bundle");
        let stand = rig.host.config().exec_step(turn) + 1;
        for i in 0..3 {
            let ends: Vec<(u32, NetControl)> = rig
                .controls(i)
                .into_iter()
                .filter(|(_, c)| matches!(c, NetControl::End { .. }))
                .collect();
            assert_eq!(
                ends,
                vec![(turn, NetControl::End { name: name.clone() })],
                "machine {i}, pause {paused}"
            );
            assert_eq!(rig.phase(i), &Phase::Ended, "machine {i}");
            assert_eq!(
                rig.machines[i].steps, stand,
                "machine {i} stands after e(k)"
            );
            assert!(rig.machines[i].halted, "machine {i}");
            assert!(
                !rig.has_event(i, is_failed),
                "machine {i}: {:?}",
                rig.events[i]
            );
        }
        assert!(!rig.has_event(0, is_lost), "{:?}", rig.events[0]);
        rig.assert_same_execution();
        if paused {
            assert_eq!(
                rig.count_events(0, is_resumed),
                1,
                "the end lifts the pause"
            );
        }
        // After the end the host accepts nothing more.
        assert!(!rig.host.end());
        assert!(rig.host.outbox().is_empty());
    }
}

/// After the end, `Bye` goes to all: the host queues the `Bye` to each client right behind the end bundle.
#[test]
fn host_end_sends_the_bundle_then_bye() {
    let mut rig = Rig::running(&[20]);
    rig.run_until(5);
    rig.host.end();
    let mut tail = Vec::new();
    for _ in 0..MAX_FRAMES {
        rig.now += FRAME_MS;
        rig.net.advance_to(rig.now);
        // Like `Rig::pump`, but the outbox of the host is recorded too.
        for (peer, link) in &mut rig.host_links {
            for ev in link.poll() {
                rig.host.handle(rig.now, *peer, ev);
            }
        }
        for (i, link) in rig.client_links.iter_mut().enumerate() {
            for ev in link.poll() {
                rig.clients[i].handle(rig.now, HOST_PEER, ev);
            }
        }
        for i in 0..rig.machines.len() {
            rig.step_machine(i, u64::MAX);
        }
        let out = rig.host.outbox();
        for (to, msg) in &out {
            let (_, link) = rig.host_links.iter_mut().find(|(p, _)| p == to).unwrap();
            link.send(msg.clone());
        }
        for (to, msg) in rig.clients[0].outbox() {
            assert_eq!(to, HOST_PEER);
            rig.client_links[0].send(msg);
        }
        rig.events[0].extend(rig.host.events());
        rig.events[1].extend(rig.clients[0].events());
        if rig.host.phase() == &Phase::Ended {
            tail = out;
            break;
        }
    }
    assert_eq!(rig.host.phase(), &Phase::Ended);
    let tail: Vec<&NetMessage> = tail.iter().map(|(_, m)| m).collect();
    assert!(
        matches!(
            &tail[..],
            [NetMessage::Bundle(b), NetMessage::Bye { .. }]
                if b.control.iter().any(|c| matches!(c, NetControl::End { .. }))
        ),
        "{tail:?}"
    );
}

/// Abort: If the host drops out in the match (connection closed, 30 s of silence, `Bye`), every client reports
/// `Failed("The connection to the host was aborted.")` and ends; before `Go` the same on a closed connection and
/// silence, only a `Bye` carries its cleaned reason ([`silence_for_30s_is_a_loss`],
/// [`host_loss_before_go_shows_a_clean_text`]).
// QUIRK E-258: deviation
#[test]
fn host_loss_fails_clients_with_the_abort_text() {
    let mut rig = Rig::running(&[20, 70]);
    rig.run_until(7);
    for link in &rig.client_links {
        link.cut();
    }
    rig.run_for(500);
    for i in 1..3 {
        assert_eq!(
            failures(&rig, i),
            vec![HOST_LOST.to_string()],
            "machine {i}"
        );
        assert_eq!(rig.phase(i), &Phase::Ended);
    }
    // 30 s of silence in the match: the same text, plus a `Bye` to the host.
    let mut rig = Rig::running(&[20]);
    rig.run_until(3);
    rig.clients[0].outbox();
    rig.clients[0].silence(rig.now, HOST_PEER, TIMEOUT_MS);
    assert!(matches!(
        &rig.clients[0].outbox()[..],
        [(HOST_PEER, NetMessage::Bye { .. })]
    ));
    assert_eq!(
        rig.clients[0].events().last(),
        Some(&CoreEvent::Failed(HOST_LOST.into()))
    );
    // `Bye` of the host in the match (here after `leave` at program exit): the same text.
    let mut rig = Rig::running(&[20]);
    rig.run_until(3);
    rig.host.leave("Program exit".into());
    assert_eq!(rig.host.phase(), &Phase::Ended);
    assert!(
        rig.host.events().is_empty(),
        "the host leaves without an event"
    );
    rig.run_for(500);
    assert_eq!(failures(&rig, 1), vec![HOST_LOST.to_string()]);
}

/// Protocol error like a dropout: A bundle with a wrong turn from the host makes the client end with the abort
/// message (`Bye` to the host, which makes the slot lost); a contribution with a wrong turn from a client (from the
/// future, duplicate or already bundled) makes its slot lost (`Bye` to it, dropout command in the next bundle). An
/// honest machine never sends either: contributions and bundles travel without gaps in turn order over an ordered
/// wire, and a client closes turn `k` only after bundle `k − 1`.
// QUIRK E-259: deviation
#[test]
fn protocol_error_counts_as_loss() {
    // Bundle with a wrong turn from the host.
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(5);
    let wrong = TurnBundle {
        turn: 9,
        commands: Vec::new(),
        control: Vec::new(),
    };
    rig.host_links[0].1.send(NetMessage::Bundle(wrong));
    rig.frames_until(|r| r.has_event(0, is_lost));
    assert_eq!(failures(&rig, 1), vec![HOST_LOST.to_string()]);
    let lost = rig.losses(0);
    assert_eq!((lost[0].0, lost[0].1), (1, Some(1)));
    assert!(lost[0].2.contains("Protocol error"), "{lost:?}");
    rig.run_for(3_000);
    assert_eq!(rig.executed_count(0, &loss(1)), 1);
    assert_eq!(rig.executed_count(2, &loss(1)), 1);
    assert!(!rig.has_event(2, is_failed));
    rig.assert_same_execution();

    // Contribution with a wrong turn from the client: future (without a gap, as an attacker sends it), duplicate,
    // bundled.
    for (case, turn) in [("future", 2), ("duplicate", 1), ("bundled", 0)] {
        let mut rig = Rig::running(&[10, 30]);
        // The host computes more slowly: client 1 has already contributed turn 1, the host not yet.
        rig.machines[0].tick_ms = 200;
        while rig.machines[1].next_close < 2 {
            rig.frame(u64::MAX);
        }
        let arrived = rig.now + 50;
        rig.frames_until(|r| r.now >= arrived);
        let buffered = rig.host.buffered_rounds();
        assert!(
            matches!(&buffered[..], [(1, peers)] if peers.contains(&1)),
            "{case}: {buffered:?}"
        );
        rig.client_links[0].send(NetMessage::Turn(TurnContribution {
            turn,
            hash: 0,
            commands: vec![cmd(1, 77)],
            wishes: Vec::new(),
        }));
        rig.frames_until(|r| r.has_event(0, is_lost));
        let lost = rig.losses(0);
        assert_eq!(lost.len(), 1, "{case}");
        assert_eq!((lost[0].0, lost[0].1), (1, Some(1)), "{case}");
        assert!(lost[0].2.contains("Protocol error"), "{case}: {lost:?}");
        rig.machines[0].tick_ms = TICK_MS;
        rig.run_for(3_000);
        // The client learned of it by `Bye`; the others play on with the dropout command, the wrong contribution
        // appears nowhere.
        assert_eq!(rig.phase(1), &Phase::Ended, "{case}");
        assert_eq!(failures(&rig, 1), vec![HOST_LOST.to_string()], "{case}");
        for i in [0, 2] {
            assert_eq!(rig.executed_count(i, &loss(1)), 1, "{case}, machine {i}");
            assert_eq!(rig.executed_count(i, &cmd(1, 77)), 0, "{case}, machine {i}");
        }
        assert!(rig.machines[2].steps > 40, "{case}: {:?}", rig.steps());
        rig.assert_same_execution();
    }
}

/// A client leaves the match with `leave`: `Bye` to the host, phase `Ended` without `Failed`; the host treats that
/// like a dropout (one `Lost`, dropout command).
#[test]
fn client_leave_is_a_loss_for_the_host() {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(5);
    let reason = "The player has left the match.";
    rig.clients[0].leave(reason.into());
    assert_eq!(rig.clients[0].phase(), &Phase::Ended);
    rig.frames_until(|r| r.has_event(0, is_lost));
    assert_eq!(rig.losses(0), vec![(1, Some(1), reason.to_string())]);
    rig.run_for(3_000);
    assert!(!rig.has_event(1, is_failed), "{:?}", rig.events[1]);
    for i in [0, 2] {
        assert_eq!(rig.executed_count(i, &loss(1)), 1, "machine {i}");
    }
    // A second `leave` has no effect.
    rig.clients[0].leave("once more".into());
    assert!(rig.clients[0].outbox().is_empty());
}

/// Names from the network end up in `Seat.name`, in the desync report and in savegames. The host cleans the name of a
/// `Hello` on acceptance: control characters become spaces, whitespace at the edges is dropped, at most
/// [`MAX_PLAYER_NAME`] characters, an empty name becomes the fallback.
#[test]
fn hello_names_are_cleaned() {
    use crate::msg::{MAX_PLAYER_NAME, clean_name};
    assert_eq!(clean_name("Anna", "Player"), "Anna");
    assert_eq!(
        clean_name("  Ev\nil\tr\u{7}Na\u{1b}me  ", "Player"),
        "Ev il r Na me"
    );
    assert_eq!(clean_name("\r\n\t ", "Player"), "Player");
    assert_eq!(clean_name("", "Player"), "Player");
    let long: String = "ä".repeat(100);
    assert_eq!(clean_name(&long, "Player").chars().count(), MAX_PLAYER_NAME);
    assert_eq!(
        clean_name("a\u{202e}b", "Player"),
        "a b",
        "bidi control characters"
    );

    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 3),
        Box::new(AcceptAll),
    );
    let dirty = format!("Eve\n{}", "x".repeat(60));
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello(&dirty))));
    let clean = clean_name(&dirty, "Player");
    assert!(clean.starts_with("Eve x"), "{clean}");
    assert_eq!(host.peers()[1], (1, clean.clone()));
    assert!(host.events().iter().any(|e| e
        == &CoreEvent::Joined {
            peer: 1,
            name: clean.clone()
        }));
}

/// A contribution must not drive the bundle beyond `MAX_FRAME` (otherwise `encode` would fail in the write thread and
/// all connections would close). The host keeps at most `MAX_COMMANDS_PER_TURN` commands and `MAX_TURN_COMMAND_BYTES`
/// encoded bytes per contribution (the beginning), its own too; eight full contributions fit into one frame. Without
/// the limit the contributions here would carry a good 1.6 MB.
#[test]
fn oversized_contributions_are_cut() {
    use crate::wire::{MAX_FRAME, encode};
    use opennml_sim::id::ObjectId;
    // A command with 10,000 units of 5 bytes each (IDs from 2^28): a good 50,000 bytes, two fit, three do not.
    let big = |player: u8, tag: u32| Command {
        player,
        kind: CommandKind::Halt {
            units: (0..10_000)
                .map(|i| ObjectId(0x1000_0000 + tag * 10_000 + i))
                .collect(),
        },
    };
    let size = postcard::to_allocvec(&big(1, 0)).unwrap().len();
    assert!(
        2 * size <= MAX_TURN_COMMAND_BYTES && 3 * size > MAX_TURN_COMMAND_BYTES,
        "{size}"
    );

    let mut rig = Rig::running(&[20; 7]);
    rig.run_until(1);
    for t in 0..(MAX_COMMANDS_PER_TURN as i32 + 50) {
        rig.clients[0].submit(cmd(1, t));
    }
    for (i, client) in rig.clients.iter_mut().enumerate().skip(1) {
        let player = i as u8 + 1;
        for t in 0..4 {
            client.submit(big(player, t));
        }
    }
    for t in 0..4 {
        rig.host.submit(big(0, t));
    }
    rig.run_until(13);

    let mut expected: Vec<Command> = (0..2).map(|t| big(0, t)).collect();
    expected.extend((0..MAX_COMMANDS_PER_TURN as i32).map(|t| cmd(1, t)));
    for player in 2..8u8 {
        expected.extend((0..2).map(|t| big(player, t)));
    }
    for i in 0..8 {
        assert_eq!(rig.nonempty(i).len(), 1, "machine {i}");
        assert!(rig.nonempty(i)[0].1 == expected, "machine {i}: commands");
        assert!(!rig.has_event(i, is_lost), "machine {i}: no dropout");
    }
    rig.assert_same_execution();
    // The bundle fits into one frame.
    let bundle = TurnBundle {
        turn: 1,
        commands: expected,
        control: Vec::new(),
    };
    let frame = encode(&NetMessage::Bundle(bundle)).expect("bundle fits into a frame");
    assert!(frame.len() <= MAX_FRAME, "{}", frame.len());
}

/// More than `MAX_WISHES_PER_TURN` wishes from a client drop out; the accepted ones stay.
#[test]
fn too_many_wishes_are_cut() {
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(1);
    rig.clients[0].wish(Wish::Save("first".into()));
    for _ in 0..(MAX_WISHES_PER_TURN * 10) {
        rig.clients[0].wish(Wish::Save("further".into()));
    }
    rig.run_until(13);
    for i in 0..3 {
        assert_eq!(
            rig.controls(i),
            vec![(
                1,
                NetControl::Save {
                    name: "first".into()
                }
            )],
            "machine {i}"
        );
    }
}

/// A foreign game edition goes into the log and the rejection shortened and cleaned.
#[test]
fn foreign_edition_is_cut_in_the_reject() {
    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 8),
        Box::new(AcceptAll),
    );
    let edition = format!("en\n{}", "y".repeat(10_000));
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(Hello {
            edition,
            ..hello("C")
        })),
    );
    match &host.outbox()[..] {
        [(1, NetMessage::Reject { reason })] => {
            assert!(reason.contains("\"en yyyy"), "{reason}");
            assert!(!reason.contains('\n'), "{reason}");
            assert!(reason.chars().count() < 80, "{reason}");
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Lobby in the turn core (broadcast, chat, kick, latency, checked start)
// ---------------------------------------------------------------------------------------------------------------

/// Host and clients with the delays `delays`, the driver contract "closed after `Bye`/`Reject`" applies; waits until
/// all are accepted and have the same lobby state as the host.
fn lobby_rig(delays: &[u64]) -> Rig {
    let mut rig = Rig::new(delays);
    rig.close_after_bye = true;
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.frames_until(Rig::lobbies_agree);
    rig
}

impl Rig {
    /// Every client has exactly the lobby state of the host (same `revision`).
    fn lobbies_agree(&self) -> bool {
        self.clients
            .iter()
            .filter(|c| c.phase() == &Phase::Gathering)
            .all(|c| c.lobby() == Some(self.host.lobby()))
    }

    /// Chat lines of machine `i`: (row, name, text).
    fn chats(&self, i: usize) -> Vec<(u8, String, String)> {
        self.events[i]
            .iter()
            .filter_map(|e| match e {
                CoreEvent::Chat { row, name, text } => Some((*row, name.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }
}

/// (deviates from E-303, E-304): After each join the whole state goes to each client; after the third join all three
/// clients have the same state as the host, same `revision`, rows in join order.
// QUIRK E-283, E-284, E-303, E-306: deviation
#[test]
fn join_broadcasts_the_lobby() {
    let rig = lobby_rig(&[20, 30, 40]);
    let state = rig.host.lobby();
    for (i, c) in rig.clients.iter().enumerate() {
        assert_eq!(c.lobby(), Some(state), "Client {}", i + 1);
        assert_eq!(c.own_row(), Some(i as u8 + 1), "Client {}", i + 1);
        assert!(rig.has_event(i + 1, |e| e == &CoreEvent::LobbyChanged));
    }
    for row in 1..4u8 {
        let r = &state.rows[usize::from(row)];
        assert_eq!(r.kind, KIND_HUMAN);
        assert_eq!(r.peer, Some(row));
        assert_eq!(r.name, format!("Client {row}"));
        assert!(!r.ready);
    }
    assert_eq!(state.rows[0].peer, Some(HOST_PEER));
    assert!(
        state.rows[4..]
            .iter()
            .all(|r| r.peer.is_none() && r.kind == 0)
    );
    assert!(rig.has_event(0, |e| e == &CoreEvent::LobbyChanged));
}

/// If a client leaves in the lobby, its row becomes open, and everyone sees that in the next state.
// QUIRK E-304: deviation
#[test]
fn leave_broadcasts_the_open_row() {
    let mut rig = lobby_rig(&[20, 30, 40]);
    let before = rig.host.lobby().revision;
    rig.clients[1].leave("Window closed".into());
    rig.frames_until(|r| r.host.lobby().rows[2].peer.is_none());
    rig.frames_until(Rig::lobbies_agree);
    let state = rig.host.lobby().clone();
    assert!(state.revision != before);
    assert_eq!(state.rows[2].kind, 0);
    assert!(!state.rows[2].ready);
    assert_eq!(state.rows[2].team, 2);
    for i in [0, 2] {
        assert_eq!(rig.clients[i].lobby(), Some(&state), "Client {}", i + 1);
    }
    assert_eq!(rig.host.peers().len(), 3);
    assert_eq!(rig.losses(0), vec![(2, None, "Window closed".to_string())]);
}

/// Kick = `Bye { KICKED }`, connection closed, the client reports `Kicked` exactly once (no `Failed`), the others see
/// row 2 open.
// QUIRK E-280, E-305: deviation
#[test]
fn kick_closes_and_opens_the_row() {
    let mut rig = lobby_rig(&[20, 30, 40]);
    assert_eq!(
        rig.host.host_edit(HostEdit::Kind { row: 2, kind: 0 }),
        Ok(())
    );
    rig.frames_until(|r| r.clients[1].phase() == &Phase::Ended);
    rig.frames_until(Rig::lobbies_agree);
    rig.run_for(500);
    assert_eq!(rig.count_events(2, |e| e == &CoreEvent::Kicked), 1);
    assert!(!rig.has_event(2, is_failed), "{:?}", rig.events[2]);
    assert_eq!(rig.closed, vec![2], "connection closed");
    let peers: Vec<Peer> = rig.host.peers().iter().map(|(p, _)| *p).collect();
    assert_eq!(peers, vec![0, 1, 3]);
    let state = rig.host.lobby().clone();
    assert_eq!((state.rows[2].kind, state.rows[2].peer), (0, None));
    for i in [0, 2] {
        assert_eq!(rig.clients[i].lobby(), Some(&state), "Client {}", i + 1);
    }
    // A human to anything other than "Open", and row 0, stay forbidden.
    assert!(
        rig.host
            .host_edit(HostEdit::Kind { row: 1, kind: 2 })
            .is_err()
    );
    assert!(
        rig.host
            .host_edit(HostEdit::Kind { row: 0, kind: 0 })
            .is_err()
    );
}

/// Between the kick and sending the `Bye` (the driver reads before sending) a second `Hello` with `Ready` can still
/// arrive from the kicked machine. Afterwards the core discards everything from this connection: no new row, no
/// `Welcome`, no start with the ghost.
#[test]
fn kicked_peer_cannot_rejoin_before_the_bye() {
    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 2),
        Box::new(AcceptAll),
    );
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello("Eve"))));
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::LobbyWish(LobbyWish::Ready(true))),
    );
    host.lobby_wish(LobbyWish::Ready(true));
    assert!(host.can_start(), "starting point: both ready");
    assert_eq!(host.host_edit(HostEdit::Kind { row: 1, kind: 0 }), Ok(()));
    host.outbox();
    host.events();
    // Before the `Bye` is sent: the same connection reports again and wants to be ready.
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello("Eve"))));
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::LobbyWish(LobbyWish::Ready(true))),
    );
    let row = &host.lobby().rows[1];
    assert_eq!((row.kind, row.peer), (0, None), "row stays open");
    assert!(!host.can_start());
    assert_eq!(host.peers().len(), 1, "{:?}", host.peers());
    let out = host.outbox();
    assert!(
        !out.iter()
            .any(|(_, m)| matches!(m, NetMessage::Welcome { .. })),
        "{out:?}"
    );
    assert!(
        !host
            .events()
            .iter()
            .any(|e| matches!(e, CoreEvent::Joined { .. })),
        "no second join"
    );
    assert!(host.start_from_lobby(|_, _| unreachable!()).is_err());
}

/// If the host leaves the lobby, every client gets exactly one `HostLeft` (no `Failed`).
#[test]
fn host_leave_tells_every_client_once() {
    let mut rig = lobby_rig(&[20, 30, 40]);
    rig.host.leave("Program exit".into());
    assert_eq!(rig.host.phase(), &Phase::Ended);
    rig.run_for(1_000);
    assert_eq!(rig.closed, vec![1, 2, 3]);
    for i in 1..4 {
        assert_eq!(
            rig.count_events(i, |e| e == &CoreEvent::HostLeft),
            1,
            "machine {i}"
        );
        assert!(!rig.has_event(i, is_failed), "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Ended);
    }
}

/// E-286, deviating from E-310: the sender of a chat line is the row of the connection; `[`/`]` are dropped, at most
/// `CHAT_INPUT` characters; everyone gets the line exactly once, also the sender and the host.
// QUIRK E-286: kept
// QUIRK E-280, E-306, E-310: deviation
#[test]
fn chat_sender_comes_from_the_link() {
    use crate::msg::CHAT_INPUT;
    let mut rig = lobby_rig(&[20, 30, 40]);
    rig.clients[0].lobby_wish(LobbyWish::Chat("[COLOR FF00FF00]Hello".into()));
    rig.run_for(500);
    let line = (1, "Client 1".to_string(), "COLOR FF00FF00Hello".to_string());
    for i in 0..4 {
        assert_eq!(rig.chats(i), vec![line.clone()], "machine {i}");
    }
    // Long line (from the host itself): cut to 80, sender row 0; chat does not change the state.
    let revision = rig.host.lobby().revision;
    rig.host.lobby_wish(LobbyWish::Chat("x".repeat(100)));
    assert_eq!(rig.host.lobby().revision, revision);
    rig.run_for(500);
    for i in 0..4 {
        let chats = rig.chats(i);
        assert_eq!(chats.len(), 2, "machine {i}");
        assert_eq!(chats[1].0, 0);
        assert_eq!(chats[1].1, "Host");
        assert_eq!(chats[1].2.chars().count(), CHAT_INPUT);
    }
}

/// The host measures the round trip time per machine with `Probe`/`ProbeAck` (30 ms per direction -> 60 ms, at most one
/// frame more) and enters the largest into the state that the clients get too.
#[test]
fn rtt_is_measured() {
    let mut rig = Rig::new(&[30, 10]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    // Fine steps so that the measurement does not depend on the frame grid.
    for _ in 0..1_000 {
        if rig.host.rtt_ms(1).is_some() && rig.host.rtt_ms(2).is_some() {
            break;
        }
        let at = rig.now + 5;
        rig.pump_at(at);
    }
    let slow = rig.host.rtt_ms(1).expect("round trip time to client 1");
    let fast = rig.host.rtt_ms(2).expect("round trip time to client 2");
    assert!((60..=60 + FRAME_MS as u32).contains(&slow), "{slow}");
    assert!((20..=20 + FRAME_MS as u32).contains(&fast), "{fast}");
    assert_eq!(rig.host.lobby().max_rtt_ms, Some(slow));
    rig.frames_until(Rig::lobbies_agree);
    for c in &rig.clients {
        assert_eq!(c.lobby().and_then(|l| l.max_rtt_ms), Some(slow));
    }
    // Further probes at intervals of `PROBE_INTERVAL_MS` do not change the state as long as the round trip time
    // stays the same.
    let revision = rig.host.lobby().revision;
    for _ in 0..(3 * PROBE_INTERVAL_MS / 5) {
        let at = rig.now + 5;
        rig.pump_at(at);
    }
    assert_eq!(rig.host.lobby().revision, revision);
    assert_eq!(rig.host.rtt_ms(1), Some(slow));
}

/// If a client leaves between `can_start` and `start_from_lobby`, the start itself checks again: `Err` with a reason,
/// no `Start` message, the phase stays `Gathering`.
#[test]
fn start_rechecks_the_rows() {
    let mut rig = lobby_rig(&[20]);
    assert!(!rig.host.can_start(), "nobody ready yet");
    rig.host.lobby_wish(LobbyWish::Ready(true));
    rig.clients[0].lobby_wish(LobbyWish::Ready(true));
    rig.frames_until(|r| r.host.can_start());
    rig.clients[0].leave("Window closed".into());
    rig.frames_until(|r| r.host.lobby().rows[1].peer.is_none());
    rig.host.outbox();
    let result = rig
        .host
        .start_from_lobby(|peer, _| Rig::info(&[0, 1], TurnConfig::default(), peer));
    let reason = result.expect_err("start without a fellow player");
    assert!(reason.contains("ready"), "{reason}");
    assert!(
        !rig.host
            .outbox()
            .iter()
            .any(|(_, m)| matches!(m, NetMessage::Start(_)))
    );
    assert_eq!(rig.host.phase(), &Phase::Gathering);
    assert!(!rig.has_event(0, |e| matches!(e, CoreEvent::Start(_))));
}

/// `start_from_lobby` gives `info_for` the row of each machine and takes slot and occupancy entirely from `info_for`
/// (no assumption "slot = row"); the grid comes from the session.
#[test]
fn start_assigns_slots_from_rows() {
    use std::cell::RefCell;
    let cfg = TurnConfig::new(4, 2).unwrap();
    let mut rig = Rig::build(cfg, &[20, 30]);
    rig.close_after_bye = true;
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.host.lobby_wish(LobbyWish::Ready(true));
    for c in &mut rig.clients {
        c.lobby_wish(LobbyWish::Ready(true));
    }
    rig.frames_until(|r| r.host.can_start());
    // Row -> slot by a computation of the game host: row 1 on slot 5, row 2 on slot 2.
    let slot_of_row = [0u8, 5, 2];
    let calls = RefCell::new(Vec::new());
    rig.host
        .start_from_lobby(|peer, row| {
            calls.borrow_mut().push((peer, row));
            let mut info = Rig::info(&[0, 5, 2], TurnConfig::default(), peer);
            info.your_slot = slot_of_row[usize::from(row)];
            info
        })
        .expect("start from the lobby");
    let mut calls = calls.into_inner();
    calls.sort_unstable();
    assert_eq!(calls, vec![(0, 0), (1, 1), (2, 2)]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    assert_eq!(rig.host.slot_of(HOST_PEER), Some(0));
    assert_eq!(rig.host.slot_of(1), Some(5));
    assert_eq!(rig.host.slot_of(2), Some(2));
    assert_eq!(rig.clients[0].slot(), Some(5));
    assert_eq!(rig.clients[1].slot(), Some(2));
    for i in 0..3 {
        let info = rig.events[i]
            .iter()
            .find_map(|e| match e {
                CoreEvent::Start(info) => Some(info.clone()),
                _ => None,
            })
            .expect("Start");
        assert_eq!(
            (info.spacing, info.lead),
            (4, 2),
            "machine {i}: grid of the session"
        );
        assert_eq!(rig.phase(i), &Phase::Loading);
    }
    assert_eq!(rig.clients[1].config().raw(), (4, 2));
}

/// After the start the host rejects every `Hello` (E-287); the late client reports `Rejected` (no `Failed`).
// QUIRK E-287: kept
#[test]
fn late_join_is_rejected() {
    let mut rig = lobby_rig(&[20]);
    rig.host.lobby_wish(LobbyWish::Ready(true));
    rig.clients[0].lobby_wish(LobbyWish::Ready(true));
    rig.frames_until(|r| r.host.can_start());
    rig.host
        .start_from_lobby(|peer, _| Rig::info(&[0, 1], TurnConfig::default(), peer))
        .expect("Start");
    rig.host.outbox();
    let mut late = ClientCore::new(hello("Late"));
    for (_, msg) in late.outbox() {
        rig.host.handle(rig.now, 5, LinkEvent::Message(msg));
    }
    let out = rig.host.outbox();
    let reason = match &out[..] {
        [(5, NetMessage::Reject { reason })] => reason.clone(),
        other => panic!("expected Reject, got {other:?}"),
    };
    assert!(reason.contains("running"), "{reason}");
    late.handle(
        rig.now,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Reject {
            reason: reason.clone(),
        }),
    );
    assert_eq!(late.events(), vec![CoreEvent::Rejected { reason }]);
    assert_eq!(late.phase(), &Phase::Ended);
    // A `Reject` after the `Welcome` stays a failure.
    let mut client = ClientCore::new(hello("C"));
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
    );
    client.events();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Reject {
            reason: "broken".into(),
        }),
    );
    assert!(matches!(&client.events()[..], [CoreEvent::Failed(t)] if t.contains("broken")));
}

/// A foreign build ID is rejected by the host, the reason names both IDs; two "unknown" ones match.
#[test]
fn foreign_build_is_rejected() {
    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 4),
        Box::new(AcceptAll),
    );
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(Hello {
            build: "0123456789ab-deadbeef".into(),
            ..hello("C")
        })),
    );
    match &host.outbox()[..] {
        [(1, NetMessage::Reject { reason })] => {
            assert!(reason.contains("program version"), "{reason}");
            assert!(reason.contains("0123456789ab-deadbeef"), "{reason}");
            assert!(reason.contains("test"), "{reason}");
        }
        other => panic!("expected Reject, got {other:?}"),
    }
    assert_eq!(host.peers().len(), 1);
    let unknown = |name: &str| Hello {
        build: "unbekannt".into(),
        ..hello(name)
    };
    let mut host = HostCore::new(
        unknown("Host"),
        book(TurnConfig::default(), 4),
        Box::new(AcceptAll),
    );
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(unknown("C"))));
    assert_eq!(host.outbox()[0], (1, NetMessage::Welcome { slot: 1 }));
}

/// Names lose `[` and `]` (deviates from E-309), also in the lobby state and in the host's peer list.
// QUIRK E-309: deviation
#[test]
fn names_lose_brackets() {
    use crate::msg::clean_name;
    assert_eq!(
        clean_name("[COLOR FF0000]Eve]", "Player"),
        "COLOR FF0000Eve"
    );
    assert_eq!(clean_name("[]", "Player"), "Player");
    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 4),
        Box::new(AcceptAll),
    );
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(hello("[b]Mallory[/b]"))),
    );
    assert_eq!(host.lobby().rows[1].name, "bMallory/b");
    assert_eq!(host.peers()[1], (1, "bMallory/b".to_string()));
}

/// Client after `Welcome` and `Start` (`Loading`, before `Go`).
fn loading_client() -> ClientCore {
    let mut client = ClientCore::new(hello("Bert"));
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
    );
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Start(Box::new(Rig::info(
            &[0, 1],
            TurnConfig::default(),
            1,
        )))),
    );
    assert_eq!(client.phase(), &Phase::Loading);
    client.events();
    client
}

/// Between `Start` and `Go` the client reports a closed connection and silence as in the match with [`HOST_LOST`]
/// (the system text only in the log); a `Bye` of the host carries its reason, but without `[`/`]`, control characters
/// and excess length. Likewise a `Reject`.
#[test]
fn host_loss_before_go_shows_a_clean_text() {
    use crate::msg::MAX_REASON_SHOWN;
    let os = "Connection aborted: An existing connection was forcibly closed by the remote host. (os error 10054)";
    let mut client = loading_client();
    client.handle(0, HOST_PEER, LinkEvent::Closed(os.into()));
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);

    let mut client = loading_client();
    client.silence(0, HOST_PEER, TIMEOUT_MS);
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);

    let dirty = format!("[COLOR FF0000]Evil\nMessage{}", "x".repeat(500));
    let shown = |events: Vec<CoreEvent>| match &events[..] {
        [CoreEvent::Failed(text)] => text.clone(),
        other => panic!("expected Failed, got {other:?}"),
    };
    let mut client = loading_client();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Bye {
            reason: dirty.clone(),
        }),
    );
    let text = shown(client.events());
    assert!(
        text.starts_with("The host has ended the connection: COLOR FF0000Evil Message"),
        "{text}"
    );
    assert!(!text.contains(['[', ']', '\n']), "{text}");
    assert!(text.chars().count() <= 40 + MAX_REASON_SHOWN, "{text}");

    let mut client = loading_client();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Reject { reason: dirty }),
    );
    let text = shown(client.events());
    assert!(
        text.starts_with("Rejected by the host: COLOR FF0000Evil Message"),
        "{text}"
    );
    assert!(!text.contains(['[', ']', '\n']), "{text}");
    assert!(text.chars().count() <= 40 + MAX_REASON_SHOWN, "{text}");
}

/// The client accepts names from the lobby state of the host only cleaned - row names like `clean_name` (empty rows
/// stay empty), session and map name like `clean_session_name`; the map path stays (ID).
#[test]
fn client_cleans_names_from_the_lobby() {
    let mut client = ClientCore::new(hello("Bert"));
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
    );
    let path = "multiplayer/[2players]-test.lvl";
    let mut state = book(TurnConfig::default(), 3).state().clone();
    state.revision = 7;
    state.session = SessionInfo {
        name: format!("[COLOR FF0000]Round\n{}", "y".repeat(80)),
        map_name: "[b]Map[/b]".into(),
        ..SessionInfo::plain("x", path, 3, TurnConfig::default())
    };
    state.rows[0].name = "[COLOR 00FF00]Anna".into();
    state.rows[1].name = "Bert\u{7}".into();
    state.rows[1].peer = Some(1);
    state.rows[1].kind = KIND_HUMAN;
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Lobby(Box::new(state))),
    );
    let lobby = client.lobby().expect("lobby state").clone();
    assert_eq!(lobby.rows[0].name, "COLOR 00FF00Anna");
    assert_eq!(lobby.rows[1].name, "Bert");
    assert_eq!(lobby.rows[2].name, "", "empty row stays empty");
    assert!(lobby.session.name.starts_with("COLOR FF0000Round yyy"));
    assert_eq!(lobby.session.name.chars().count(), 39);
    assert_eq!(lobby.session.map_name, "bMap/b");
    assert_eq!(
        lobby.session.map,
        crate::lobby::MapId::Fixed {
            path: path.into(),
            checksum: 0
        },
        "map path unchanged"
    );
    assert_eq!(client.own_row(), Some(1));
}

/// After `Go` lobby wishes have no effect: the client sends none, the host discards foreign and own ones, the state
/// stays; probe, chat and host edit likewise.
// QUIRK E-280: deviation
#[test]
fn lobby_wish_after_go_is_ignored() {
    let mut rig = Rig::running(&[20]);
    rig.run_until(3);
    rig.clients[0].outbox();
    rig.clients[0].lobby_wish(LobbyWish::Ready(true));
    assert!(rig.clients[0].outbox().is_empty());
    let revision = rig.host.lobby().revision;
    rig.host.outbox();
    rig.host.handle(
        rig.now,
        1,
        LinkEvent::Message(NetMessage::LobbyWish(LobbyWish::Chat("late".into()))),
    );
    rig.host.handle(
        rig.now,
        1,
        LinkEvent::Message(NetMessage::LobbyWish(LobbyWish::Ready(true))),
    );
    rig.host.lobby_wish(LobbyWish::Ready(true));
    rig.host.tick(rig.now + 10 * PROBE_INTERVAL_MS);
    assert!(
        rig.host
            .host_edit(HostEdit::Kind { row: 1, kind: 0 })
            .is_err()
    );
    assert!(rig.host.outbox().is_empty());
    assert!(rig.host.events().is_empty());
    assert_eq!(rig.host.lobby().revision, revision);
    rig.run_until(20);
    rig.assert_same_execution();
}

/// With `auto_ready` the client reports itself ready after the first lobby state (command-line path), exactly
/// once.
#[test]
fn auto_ready_wishes_ready_after_the_first_lobby() {
    let mut rig = Rig::new(&[20]);
    rig.clients[0].set_auto_ready(true);
    rig.frames_until(|r| r.host.lobby().rows[1].ready);
    rig.host.lobby_wish(LobbyWish::Ready(true));
    rig.frames_until(Rig::lobbies_agree);
    assert!(rig.host.can_start());
    // No second wish: if the host takes the checkmark back (replayed here by the client's own wish), it stays
    // gone.
    rig.clients[0].lobby_wish(LobbyWish::Ready(false));
    rig.run_for(2_000);
    assert!(!rig.host.lobby().rows[1].ready);
}

// ---------------------------------------------------------------------------------------------------------------
// Map switch in the lobby state
// ---------------------------------------------------------------------------------------------------------------

/// Session "New match" on `path` for `players` players in the grid of the rigs.
fn changed_session(path: &str, players: u8) -> SessionInfo {
    SessionInfo::plain("New match", path, players, TurnConfig::default())
}

/// (deviates from E-281): `set_session` distributes the whole state with the new map to each client; `revision`
/// counts up by exactly one.
#[test]
fn change_game_reaches_every_client() {
    let mut rig = lobby_rig(&[20, 30]);
    let before = rig.host.lobby().revision;
    assert_eq!(
        rig.host
            .set_session(changed_session("multiplayer/new.lvl", 4)),
        Ok(())
    );
    rig.frames_until(Rig::lobbies_agree);
    let state = rig.host.lobby().clone();
    assert_eq!(state.revision, before + 1);
    assert_eq!(state.rows.len(), 4);
    assert_eq!(
        state.session.map,
        crate::lobby::MapId::Fixed {
            path: "multiplayer/new.lvl".into(),
            checksum: 0
        }
    );
    for (i, c) in rig.clients.iter().enumerate() {
        assert_eq!(c.lobby(), Some(&state), "Client {}", i + 1);
        assert_eq!(c.own_row(), Some(i as u8 + 1), "Client {}", i + 1);
    }
}

/// Data check on the host: the host's checksum and every verdict reach every client with the lobby state; a client
/// with other rules stays out of `Ready`, `START` waits for every row.
#[test]
fn data_state_reaches_every_client() {
    let mut rig = Rig::new(&[20, 30]);
    rig.close_after_bye = true;
    rig.host.enable_data_check();
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.frames_until(Rig::lobbies_agree);
    let pending: Vec<DataState> = rig.host.lobby().rows[..3]
        .iter()
        .map(|r| r.data.clone())
        .collect();
    assert_eq!(
        pending,
        vec![DataState::Equal, DataState::Pending, DataState::Pending]
    );
    let input = |path: &str, hash: u64| DataInput {
        path: path.into(),
        hash: Some(hash),
    };
    let host_inputs = vec![input("maps/a.ini", 1), input("maps/b.ini", 2)];
    let session = rig.host.lobby().session_revision;
    assert!(
        !rig.host
            .set_rules(session + 1, Some(7), host_inputs.clone())
    );
    assert_eq!(rig.host.lobby().rules_checksum, None, "other revision");
    assert!(rig.host.set_rules(session, Some(7), host_inputs));
    rig.frames_until(|r| {
        r.clients
            .iter()
            .all(|c| c.lobby().is_some_and(|l| l.rules_checksum == Some(7)))
    });
    rig.clients[0].lobby_wish(LobbyWish::Data {
        session_revision: session,
        report: DataReport::Rules {
            checksum: Some(7),
            inputs: Vec::new(),
        },
    });
    rig.clients[1].lobby_wish(LobbyWish::Data {
        session_revision: session,
        report: DataReport::Rules {
            checksum: Some(8),
            inputs: vec![input("maps/a.ini", 1), input("maps/b.ini", 20)],
        },
    });
    rig.frames_until(|r| r.host.lobby().rows[2].data != DataState::Pending);
    rig.frames_until(Rig::lobbies_agree);
    let state = rig.host.lobby().clone();
    assert_eq!(state.rows[1].data, DataState::Equal);
    assert_eq!(
        state.rows[2].data,
        DataState::Differs {
            files: vec!["maps/b.ini".into()],
            more: 0
        }
    );
    for (i, c) in rig.clients.iter().enumerate() {
        assert_eq!(c.lobby(), Some(&state), "Client {}", i + 1);
    }
    rig.host.lobby_wish(LobbyWish::Ready(true));
    for c in &mut rig.clients {
        c.lobby_wish(LobbyWish::Ready(true));
    }
    rig.frames_until(|r| r.host.lobby().rows[1].ready);
    rig.run_for(500);
    assert!(!rig.host.lobby().rows[2].ready, "Ready without Equal");
    assert!(!rig.host.can_start());
}

/// The game host checks the rows (`rows_fit` holds), then a join takes row 3; the switch to three players is rejected
/// in the core, state and announcement stay with the old session.
// QUIRK E-307: deviation
#[test]
fn change_refused_when_a_join_takes_a_dropped_row() {
    let mut rig = Rig::new(&[20, 30, 400]);
    rig.close_after_bye = true;
    rig.frames_until(|r| r.clients[0].peer().is_some() && r.clients[1].peer().is_some());
    assert!(rig.clients[2].peer().is_none(), "client 3 still on its way");
    assert!(crate::lobby::rows_fit(&rig.host.lobby().rows, 3));
    rig.frames_until(|r| r.clients[2].peer().is_some());
    rig.frames_until(Rig::lobbies_agree);
    let before = rig.host.lobby().clone();
    assert_eq!(before.rows[3].peer, Some(3));
    assert!(!crate::lobby::rows_fit(&before.rows, 3));
    assert!(
        rig.host
            .set_session(changed_session("multiplayer/small.lvl", 3))
            .is_err()
    );
    assert_eq!(rig.host.lobby(), &before, "state unchanged");
    rig.run_for(500);
    assert_eq!(rig.host.lobby().session, before.session);
    for (i, c) in rig.clients.iter().enumerate() {
        assert_eq!(
            c.lobby().map(|l| &l.session),
            Some(&before.session),
            "Client {}",
            i + 1
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Loaded lobby in the turn core
// ---------------------------------------------------------------------------------------------------------------

/// Checksum of the simulation part in the savegame of the tests.
const SAVE_SUM: u64 = 0x5A7E_0000_0042;

/// Session of a network savegame with `players` rows.
fn saved_session(players: u8) -> SessionInfo {
    let mut s = SessionInfo::plain("Test match", "", players, TurnConfig::default());
    s.map = crate::lobby::MapId::Saved {
        name: "Netgame".into(),
        checksum: SAVE_SUM,
    };
    s.map_name = "Netgame".into();
    s
}

/// Loaded lobby state with the slots `seats`, the host on `host_seat` (team and color = slot).
fn loaded_book(seats: &[u8], host_seat: u8) -> LobbyBook {
    let rows = seats
        .iter()
        .map(|&s| crate::lobby::LobbyRow {
            kind: 0,
            name: String::new(),
            nation: 0,
            team: i32::from(s),
            handicap: 0,
            color: i32::from(s),
            ready: false,
            peer: None,
            data: DataState::Equal,
        })
        .collect();
    LobbyBook::loaded(
        saved_session(seats.len() as u8),
        rows,
        seats.to_vec(),
        host_seat,
        "Host",
    )
    .expect("valid state")
}

fn loaded_host(seats: &[u8], host_seat: u8) -> HostCore {
    HostCore::new(
        hello("Host"),
        loaded_book(seats, host_seat),
        Box::new(AcceptAll),
    )
}

/// `Hello` with a slot claim.
fn claim(name: &str, slot: u8, checksum: u64) -> Hello {
    Hello {
        seat: Some(SeatClaim { slot, checksum }),
        ..hello(name)
    }
}

/// The reason of the only `Reject` to `peer` in the outbox of the host.
fn rejected(host: &mut HostCore, peer: Peer) -> String {
    match &host.outbox()[..] {
        [(p, NetMessage::Reject { reason })] if *p == peer => reason.clone(),
        other => panic!("expected Reject to {peer}, got {other:?}"),
    }
}

/// A `Hello` with a matching slot claim goes into the row of its slot ("Occupied", broadcast); a second machine with
/// the same slot and a slot without a row are rejected with the reason from `join_seat`.
// QUIRK E-325: deviation
#[test]
fn hello_with_matching_claim_joins_its_seat() {
    let mut host = loaded_host(&[1, 3, 4], 3);
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 4, SAVE_SUM))),
    );
    let out = host.outbox();
    assert_eq!(out[0], (1, NetMessage::Welcome { slot: 1 }));
    assert!(
        matches!(&out[1..], [(1, NetMessage::Lobby(state))] if state.rows[2].peer == Some(1)),
        "{out:?}"
    );
    let r = &host.lobby().rows[2];
    assert_eq!(
        (r.kind, r.peer, r.name.as_str()),
        (KIND_HUMAN, Some(1), "Bert")
    );
    assert!(host.events().contains(&CoreEvent::Joined {
        peer: 1,
        name: "Bert".into()
    }));
    let before = host.lobby().clone();
    host.handle(
        0,
        2,
        LinkEvent::Message(NetMessage::Hello(claim("Cora", 4, SAVE_SUM))),
    );
    assert_eq!(rejected(&mut host, 2), "Slot 5 is already occupied.");
    host.handle(
        0,
        3,
        LinkEvent::Message(NetMessage::Hello(claim("Dora", 2, SAVE_SUM))),
    );
    assert_eq!(
        rejected(&mut host, 3),
        "Slot 3 has no player left in this savegame."
    );
    assert_eq!(host.lobby(), &before);
    assert_eq!(host.peers().len(), 2);
}

/// (deviates from E-275): Without a slot claim nobody enters a loaded lobby.
#[test]
fn hello_without_claim_is_rejected_in_a_loaded_lobby() {
    let mut host = loaded_host(&[1, 3, 4], 3);
    let before = host.lobby().clone();
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello("Bert"))));
    assert_eq!(
        rejected(&mut host, 1),
        "This match needs the savegame on this machine."
    );
    assert_eq!(host.lobby(), &before);
    assert_eq!(host.peers().len(), 1);
    assert!(host.events().is_empty());
}

/// A different checksum of the simulation part is rejected by the host, also for a free slot.
// QUIRK E-325: deviation
#[test]
fn hello_with_other_checksum_is_rejected() {
    let mut host = loaded_host(&[1, 3, 4], 3);
    let before = host.lobby().clone();
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 4, SAVE_SUM ^ 1))),
    );
    assert_eq!(
        rejected(&mut host, 1),
        "The savegame on this machine differs from the host's."
    );
    assert_eq!(host.lobby(), &before);
    assert_eq!(host.peers().len(), 1);
}

/// A new lobby accepts no `Hello` with a slot claim.
#[test]
fn hello_with_claim_is_rejected_in_a_new_lobby() {
    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 4),
        Box::new(AcceptAll),
    );
    let before = host.lobby().clone();
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 1, SAVE_SUM))),
    );
    assert_eq!(
        rejected(&mut host, 1),
        "This match is not a loaded savegame."
    );
    assert_eq!(host.lobby(), &before);
    assert_eq!(host.peers().len(), 1);
}

/// If the host plays the last slot (rows `[open, host]`), the match is not full: a client enters row 0.
#[test]
fn hello_joins_row_0_when_the_host_has_the_last_seat() {
    let mut host = loaded_host(&[2, 6], 6);
    assert_eq!(host.lobby().rows[1].peer, Some(HOST_PEER));
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 2, SAVE_SUM))),
    );
    assert_eq!(host.outbox()[0], (1, NetMessage::Welcome { slot: 1 }));
    assert_eq!(host.lobby().rows[0].peer, Some(1));
    assert!(host.can_start());
}

/// A client in row 0 can leave; the row becomes open again, `START` disappears.
#[test]
fn client_on_row_0_can_leave() {
    let mut host = loaded_host(&[2, 6], 6);
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 2, SAVE_SUM))),
    );
    assert!(host.can_start());
    host.handle(0, 1, LinkEvent::Closed("Window closed".into()));
    let r = &host.lobby().rows[0];
    assert_eq!((r.kind, r.peer, r.name.as_str()), (0, None, ""));
    assert_eq!((r.team, r.color), (2, 2), "values of the state stay");
    assert!(!host.can_start());
    assert_eq!(host.peers().len(), 1);
    // The slot is free again.
    host.handle(
        0,
        2,
        LinkEvent::Message(NetMessage::Hello(claim("Cora", 2, SAVE_SUM))),
    );
    assert_eq!(host.lobby().rows[0].peer, Some(2));
}

/// In the loaded lobby there is no chat and no wishes, neither from the host nor from clients.
#[test]
fn loaded_lobby_has_no_chat() {
    let mut host = loaded_host(&[2, 6], 6);
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 2, SAVE_SUM))),
    );
    host.outbox();
    host.events();
    let before = host.lobby().clone();
    host.lobby_wish(LobbyWish::Chat("Hello".into()));
    host.lobby_wish(LobbyWish::Ready(true));
    for w in [LobbyWish::Chat("Hello".into()), LobbyWish::Ready(true)] {
        host.handle(0, 1, LinkEvent::Message(NetMessage::LobbyWish(w)));
    }
    assert!(host.outbox().is_empty());
    assert!(host.events().is_empty());
    assert_eq!(host.lobby(), &before);
}

/// Start from a loaded lobby in which the host is not in row 0 (slots 1, 3, 4, host on 3 = row 1). `info_for` gets the
/// right pairs (machine, row), slot and `saved` arrive at everyone, and the match starts.
// QUIRK E-325: deviation
#[test]
fn start_from_a_loaded_lobby_with_the_host_not_in_row_0() {
    use crate::msg::SavedStart;
    use std::cell::RefCell;
    let seats = [1u8, 3, 4];
    let mut rig = Rig::new(&[20, 30]);
    rig.close_after_bye = true;
    rig.host = loaded_host(&seats, 3);
    // Client 1 plays slot 4 (row 2), client 2 slot 1 (row 0).
    rig.clients[0] = ClientCore::new(claim("Client 1", 4, SAVE_SUM));
    rig.clients[1] = ClientCore::new(claim("Client 2", 1, SAVE_SUM));
    rig.slots = vec![3, 4, 1];
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.frames_until(Rig::lobbies_agree);
    assert!(rig.host.can_start(), "all rows occupied, without Ready");
    for (i, c) in rig.clients.iter().enumerate() {
        assert_eq!(c.lobby().map(|l| l.seats.clone()), Some(seats.to_vec()));
        assert_eq!(c.own_row(), Some([2, 0][i]), "Client {}", i + 1);
    }
    let saved = SavedStart {
        name: "Netgame".into(),
        checksum: SAVE_SUM,
        steps: 1_234,
    };
    let calls = RefCell::new(Vec::new());
    rig.host
        .start_from_lobby(|peer, row| {
            calls.borrow_mut().push((peer, row));
            let mut info = Rig::info(&[3, 4, 1], TurnConfig::default(), peer);
            info.your_slot = seats[usize::from(row)];
            info.saved = Some(saved.clone());
            info
        })
        .expect("start from the loaded lobby");
    let mut calls = calls.into_inner();
    calls.sort_unstable();
    assert_eq!(calls, vec![(0, 1), (1, 2), (2, 0)]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    assert_eq!(rig.host.slot_of(HOST_PEER), Some(3));
    assert_eq!(rig.host.slot_of(1), Some(4));
    assert_eq!(rig.host.slot_of(2), Some(1));
    assert_eq!(rig.clients[0].slot(), Some(4));
    assert_eq!(rig.clients[1].slot(), Some(1));
    for i in 0..3 {
        let info = rig.events[i]
            .iter()
            .find_map(|e| match e {
                CoreEvent::Start(info) => Some(info.clone()),
                _ => None,
            })
            .expect("Start");
        assert_eq!(info.saved.as_ref(), Some(&saved), "machine {i}");
    }
    for c in &mut rig.clients {
        c.loaded(None);
    }
    rig.host.loaded_self();
    rig.frames_until(|r| r.all_in(&Phase::Running));
}

/// E-287 (deviates from E-328): After the start of a loaded match the host admits nobody any more, not even with a
/// matching slot claim; the reason names the running match (not the occupied slot).
// QUIRK E-328: deviation
#[test]
fn no_join_after_a_loaded_start() {
    let mut host = loaded_host(&[0, 2], 0);
    host.handle(
        0,
        1,
        LinkEvent::Message(NetMessage::Hello(claim("Bert", 2, SAVE_SUM))),
    );
    host.start_from_lobby(|peer, _| Rig::info(&[0, 2], TurnConfig::default(), peer))
        .expect("Start");
    host.outbox();
    host.handle(
        0,
        2,
        LinkEvent::Message(NetMessage::Hello(claim("Late", 2, SAVE_SUM))),
    );
    let reason = rejected(&mut host, 2);
    assert!(reason.contains("running"), "{reason}");
    assert_eq!(host.peers().len(), 2, "no new machine");
}

/// A client discards a lobby state whose slots do not fit the rows.
#[test]
fn client_drops_a_lobby_with_wrong_seats() {
    let mut client = ClientCore::new(claim("Bert", 2, SAVE_SUM));
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
    );
    let mut state = loaded_book(&[2, 6], 6).state().clone();
    state.seats = vec![2];
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Lobby(Box::new(state.clone()))),
    );
    assert_eq!(client.lobby(), None);
    state.seats = vec![2, 6];
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Lobby(Box::new(state.clone()))),
    );
    assert_eq!(client.lobby(), Some(&state));
}

#[test]
fn migration_deadlines_are_fixed() {
    assert_eq!(crate::SUCCESSOR_CONNECT_MS, 3_000);
    assert_eq!(crate::REJOIN_WINDOW_MS, 10_000);
}

/// Hello with a listening port.
fn hello_listening(name: &str, port: u16) -> Hello {
    Hello {
        listen_port: Some(port),
        ..hello(name)
    }
}

/// Address of a listener in the test.
fn addr(port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], port))
}

/// Rig with two clients that listen: sign-on, start and loading are done, `Go` is still waiting for the probe result.
fn rig_loaded_with_listeners() -> Rig {
    let mut rig = Rig::build(TurnConfig::default(), &[0, 0]);
    rig.clients[0] = ClientCore::new(hello_listening("Client 1", 5001));
    rig.clients[1] = ClientCore::new(hello_listening("Client 2", 5002));
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    for c in &mut rig.clients {
        c.loaded(None);
    }
    rig.host.loaded_self();
    for _ in 0..40 {
        rig.pump_at(rig.now + FRAME_MS);
    }
    rig
}

#[test]
fn go_waits_for_every_probe_verdict() {
    let mut rig = rig_loaded_with_listeners();
    assert_eq!(rig.host.listen_port(1), Some(5001));
    assert_eq!(rig.host.listen_port(2), Some(5002));
    assert_eq!(rig.host.phase(), &Phase::Loading, "no result, no Go");
    // One result is still missing.
    rig.host.set_reachable(1, Some(addr(5001)));
    for _ in 0..40 {
        rig.pump_at(rig.now + FRAME_MS);
    }
    assert_eq!(rig.host.phase(), &Phase::Loading);
    assert!(rig.clients.iter().all(|c| c.phase() == &Phase::Loading));
    // Only the first call per peer counts.
    rig.host.set_reachable(1, None);
    rig.host.set_reachable(2, None);
    rig.frames_until(|r| r.all_in(&Phase::Running));
    let expected = vec![
        Successor {
            peer: 1,
            slot: 1,
            addr: Some(addr(5001)),
        },
        Successor {
            peer: 2,
            slot: 2,
            addr: None,
        },
    ];
    assert_eq!(rig.host.successors(), expected);
    assert_eq!(rig.clients[0].successors(), expected.as_slice());
    assert_eq!(rig.clients[1].successors(), expected.as_slice());
}

#[test]
fn client_without_listen_port_is_unreachable() {
    // `hello()` reports no listening port: `Go` comes without a probe result, all entries without an address.
    let rig = Rig::running(&[0, 0]);
    assert_eq!(rig.host.listen_port(1), None);
    let expected = vec![
        Successor {
            peer: 1,
            slot: 1,
            addr: None,
        },
        Successor {
            peer: 2,
            slot: 2,
            addr: None,
        },
    ];
    assert_eq!(rig.host.successors(), expected);
}

#[test]
fn client_keeps_the_successor_list_from_go() {
    let mut rig = Rig::build(TurnConfig::default(), &[0]);
    rig.clients[0] = ClientCore::new(hello_listening("Client 1", 6001));
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    assert!(rig.clients[0].successors().is_empty(), "empty before Go");
    rig.clients[0].loaded(None);
    rig.host.loaded_self();
    rig.host.set_reachable(1, Some(addr(6001)));
    rig.frames_until(|r| r.all_in(&Phase::Running));
    assert_eq!(
        rig.clients[0].successors(),
        [Successor {
            peer: 1,
            slot: 1,
            addr: Some(addr(6001)),
        }]
    );
}

mod migration;

/// The name of the network savegame starts with the prefix of the edition.
#[test]
fn net_end_name_takes_the_prefix() {
    let time = LocalTime {
        year: 2026,
        month: 11,
        day: 20,
        hour: 9,
        minute: 5,
        second: 0,
    };
    assert_eq!(time.net_end_name("net_"), "net_2026_11_20_0905");
    assert_eq!(time.net_end_name("netz_"), "netz_2026_11_20_0905");
}

fn german_words() -> LocalWords {
    LocalWords {
        player: "Spieler".to_string(),
        net_save_prefix: "netz_".to_string(),
    }
}

/// Every cleanup of a core takes the words of the installation: the host for the name of a `Hello`, for the end name
/// of the match and for the lobby book, the client for the names in the lobby state and for a failed load. Without
/// `set_words` the English words apply.
#[test]
fn cores_clean_with_their_local_words() {
    assert_eq!(LocalWords::default().player, "Player");
    assert_eq!(LocalWords::default().net_save_prefix, "net_");

    let mut host = HostCore::new(
        hello("Host"),
        book(TurnConfig::default(), 3),
        Box::new(AcceptAll),
    );
    host.handle(0, 1, LinkEvent::Message(NetMessage::Hello(hello(" "))));
    assert_eq!(host.lobby().rows[1].name, "Player");
    host.set_words(german_words());
    assert_eq!(host.words(), &german_words());
    host.handle(0, 2, LinkEvent::Message(NetMessage::Hello(hello("\n"))));
    assert_eq!(host.lobby().rows[2].name, "Spieler");

    let mut client = ClientCore::new(hello("Bert"));
    assert_eq!(client.words(), &LocalWords::default());
    client.set_words(german_words());
    assert_eq!(client.words().net_save_prefix, "netz_");
    let mut state = host.lobby().clone();
    state.rows[1].name = "\u{200B}".to_string();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Welcome { slot: 1 }),
    );
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Lobby(Box::new(state))),
    );
    assert_eq!(client.lobby().unwrap().rows[1].name, "Spieler");
}

/// The host names the end of the match with the prefix of its edition.
#[test]
fn host_end_uses_the_edition_prefix() {
    for (words, expected) in [
        (LocalWords::default(), "net_2026_11_06_1807"),
        (german_words(), "netz_2026_11_06_1807"),
    ] {
        let mut rig = Rig::running(&[20, 70]);
        rig.run_until(5);
        rig.host.set_words(words);
        rig.host.set_clock(END_CLOCK);
        assert!(rig.host.end());
        rig.run_for(3_000);
        let ends: Vec<NetControl> = rig
            .controls(0)
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| matches!(c, NetControl::End { .. }))
            .collect();
        assert_eq!(
            ends,
            vec![NetControl::End {
                name: expected.to_string()
            }]
        );
    }
}
