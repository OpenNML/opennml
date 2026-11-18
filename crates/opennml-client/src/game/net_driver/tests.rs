//! Tests of the turn driver: three machines, each with its own [`SimHost`], from the same network match
//! ([`testworld::net_rules`], [`testworld::net_setup`]), connected through [`MemNet`] with played time in frames of
//! [`FRAME_MS`]. For each machine the table collects the whole sequence `(steps, checksum)` from tick 0; if a machine
//! adopts a state during the sync, the new entry replaces the old one of the same tick. One client
//! ([`Table::corrupter`], default 2) corrupts its state with [`SimHost::debug_corrupt`] at the ticks from
//! [`Table::corrupt_at`]; every machine writes to [`NetPaths`] in its own folder of the table, the local time is fixed
//! ([`CLOCK`]).

use std::sync::{Arc, Mutex};

use super::*;
use crate::game::formation_order::FormationOrder;
use crate::game::options::DEFAULT_GAME_SPEED;
use crate::game::testworld::{self, NET_SLOTS, SPA_SOLDIER};
use glam::Vec3;
use opennml_net::{
    ClientCore, HaltReason, Hello, Link, LinkEvent, LocalTime, MemLink, MemNet, NET_VERSION,
    NetMessage, RasterStage, Seat, StartInfo, TurnBundle, TurnConfig,
};
use opennml_sim::command::{Command, CommandKind};
use opennml_sim::id::ObjectId;
use opennml_sim::save::{RecordedCommand, SaveGame, checksum, part_checksums};
use opennml_sim::setup::{new_match, start_package};
use opennml_sim::tick::step;

mod migration;

/// Length of a frame of played time.
const FRAME_MS: u64 = 25;
/// The tests compare this many ticks (sequence 0 ... 400).
const STEPS: u64 = 400;
/// Maximum number of frames before a run counts as a hang.
const MAX_FRAMES: u32 = 20_000;
/// Delay per direction: host (no line), client 1, client 2.
const DELAYS: [u64; 3] = [0, 40, 130];
/// Time after `Go` at which client 2 drops out in [`client_vanishes_mid_match`].
const CUT_MS: u64 = 2_000;
/// Fixed local time of all drivers in the table (name of the report and of the stop).
const CLOCK: LocalTime = LocalTime {
    year: 2026,
    month: 11,
    day: 6,
    hour: 14,
    minute: 15,
    second: 16,
};
/// The five files of a desync report, sorted.
const REPORT_FILES: [&str; 5] = [
    "host.nmllog",
    "host.nmlsave",
    "local.nmllog",
    "local.nmlsave",
    "report.txt",
];

/// FNV-1a 64 over the sequence `(steps, checksum)` 0 ... [`STEPS`] of the host in the scene [`scene`]. Measured:
/// three machines of the network match, delays [`DELAYS`], slot 0 moves its soldiers at 300 and 1,700 ms, slot 1
/// makes friends with slot 0 at 900 ms, slot 2 chats at 1,200 ms.
const NET_GOLDEN: u64 = 0x6f1f_1d84_bbcd_7c8b;

/// Connection whose [`MemLink`] the test keeps (for [`MemLink::cut`]).
struct SharedLink(Arc<Mutex<MemLink>>);

impl SharedLink {
    fn with<R>(&self, f: impl FnOnce(&mut MemLink) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// This many frames [`LateLarge`] holds back a large content.
const LATE_POLLS: u32 = 8;

/// Connection of the host that holds large contents back for [`LATE_POLLS`] polls (frames) before sending them:
/// like a slow transfer of a large state over TCP (the state arrives after the bundle).
struct LateLarge {
    inner: MemLink,
    held: Vec<(u32, u32, Vec<u8>)>,
}

impl Link for LateLarge {
    fn send(&mut self, msg: NetMessage) {
        self.inner.send(msg);
    }
    fn send_large(&mut self, id: u32, bytes: Vec<u8>) {
        self.held.push((LATE_POLLS, id, bytes));
    }
    fn poll(&mut self) -> Vec<LinkEvent> {
        for (left, id, bytes) in std::mem::take(&mut self.held) {
            if left == 0 {
                self.inner.send_large(id, bytes);
            } else {
                self.held.push((left - 1, id, bytes));
            }
        }
        self.inner.poll()
    }
    fn last_received_ms(&self) -> u64 {
        self.inner.last_received_ms()
    }
    fn age_ms(&self) -> u64 {
        self.inner.age_ms()
    }
    fn close(&mut self, reason: &str) {
        self.inner.close(reason);
    }
    fn peer_ip(&self) -> Option<std::net::IpAddr> {
        self.inner.peer_ip()
    }
    fn mute(&mut self, on: bool) {
        self.inner.mute(on);
    }
}

impl Link for SharedLink {
    fn send(&mut self, msg: NetMessage) {
        self.with(|l| l.send(msg));
    }
    fn send_large(&mut self, id: u32, bytes: Vec<u8>) {
        self.with(|l| l.send_large(id, bytes));
    }
    fn poll(&mut self) -> Vec<LinkEvent> {
        self.with(|l| l.poll())
    }
    fn last_received_ms(&self) -> u64 {
        self.with(|l| l.last_received_ms())
    }
    fn age_ms(&self) -> u64 {
        self.with(|l| l.age_ms())
    }
    fn close(&mut self, reason: &str) {
        self.with(|l| l.close(reason));
    }
    fn peer_ip(&self) -> Option<std::net::IpAddr> {
        self.with(|l| l.peer_ip())
    }
    fn mute(&mut self, on: bool) {
        self.with(|l| MemLink::mute(l, on));
    }
}

fn hello(name: &str) -> Hello {
    Hello {
        listen_port: None,
        net_version: NET_VERSION,
        protocol: 1,
        format: 1,
        edition: "test".into(),
        name: name.into(),
        build: "test".into(),
        seat: None,
    }
}

/// Storage locations of machine `i` under `dir`.
fn paths(dir: &std::path::Path, i: usize) -> NetPaths {
    let dir = dir.join(format!("machine{i}"));
    NetPaths {
        saves: dir.join("savegame"),
        netlog: dir.join("netlog"),
    }
}

/// One machine: driver, game host and what it has seen.
struct Machine {
    driver: NetDriver,
    sim: SimHost,
    /// `(steps, checksum)` from tick 0, one entry per computed tick.
    seq: Vec<(u64, u64)>,
    lost: Vec<(u8, String)>,
    failed: Option<String>,
    controls: Vec<NetControl>,
    /// Per frame with [`NetFrame::adopted`] the number of its events (those of the discarded state lapse).
    adopted: Vec<usize>,
    /// Messages of the simulation to the own slot: `(tick, message)` (200 + p on a dropout).
    messages: Vec<(u64, i32)>,
    /// Speed changes from [`NetFrame::speed`].
    speed: Vec<SpeedChange>,
    /// Network savegames from [`NetFrame::saves`].
    saves: Vec<NetSave>,
    /// For each entry in [`Self::saves`] the tick at the end of its frame.
    save_frame_steps: Vec<u64>,
    /// Game time per frame in ms (default [`FRAME_MS`]; with a larger value a fast machine computes up to two ticks
    /// per frame).
    dt_ms: f64,
    /// End of the network match from [`NetFrame::finish`].
    finish: Vec<NetFinish>,
    /// Ticks at the end of the frames with [`NetFrame::became_host`].
    became_host: Vec<u64>,
}

impl Machine {
    fn new(driver: NetDriver, slot: u8) -> Self {
        Self::with_slots(driver, slot, NET_SLOTS)
    }

    /// Machine on slot `slot` of a network match with `slots` slots ([`testworld::net_host_with`]).
    fn with_slots(driver: NetDriver, slot: u8, slots: u8) -> Self {
        let mut m = Self {
            driver,
            sim: testworld::net_host_with(slot, slot == 0, slots),
            seq: Vec::new(),
            lost: Vec::new(),
            failed: None,
            controls: Vec::new(),
            adopted: Vec::new(),
            messages: Vec::new(),
            speed: Vec::new(),
            saves: Vec::new(),
            save_frame_steps: Vec::new(),
            dt_ms: FRAME_MS as f64,
            finish: Vec::new(),
            became_host: Vec::new(),
        };
        m.collect();
        m
    }

    fn frame(&mut self, now: u64) {
        let f = self.driver.advance(&mut self.sim, self.dt_ms, now);
        self.lost.extend(f.lost);
        if self.failed.is_none() {
            self.failed = f.failed;
        }
        self.controls.extend(f.controls);
        if f.adopted {
            self.adopted.push(f.events.len());
        }
        let me = self.sim.active_player;
        for e in &f.events {
            if let SimEvent::PlayerMessage { to, id, .. } = e
                && *to == Some(me)
            {
                self.messages.push((self.sim.state.steps, *id));
            }
        }
        self.speed.extend(f.speed);
        for _ in &f.saves {
            self.save_frame_steps.push(self.sim.state.steps);
        }
        self.saves.extend(f.saves);
        self.finish.extend(f.finish);
        if f.became_host {
            self.became_host.push(self.sim.state.steps);
        }
        self.collect();
    }

    /// Take over new entries of the checksum history (at most two ticks per frame, the history holds 64). A different
    /// value for the last tick (corrupting, adoption during the sync) replaces the old one.
    fn collect(&mut self) {
        for &(s, h) in &self.sim.hashes {
            match self.seq.last_mut() {
                Some((last, _)) if s < *last => {}
                Some((last, old)) if s == *last => *old = h,
                _ => self.seq.push((s, h)),
            }
        }
    }

    /// The sequence 0 ... [`STEPS`].
    fn prefix(&self) -> Vec<(u64, u64)> {
        self.seq
            .iter()
            .copied()
            .take_while(|&(s, _)| s <= STEPS)
            .collect()
    }

    fn phase(&self) -> Phase {
        self.driver.status().phase
    }
}

/// Inputs of a machine: `(index, time after Go, game host)`.
type Scene = fn(usize, u64, &mut SimHost);

/// Own soldiers of the game host.
fn own_soldiers(sim: &SimHost) -> Vec<ObjectId> {
    let me = sim.active_player;
    sim.state
        .objects()
        .filter(|o| o.guid == SPA_SOLDIER && o.owner() == me)
        .map(|o| o.id)
        .collect()
}

fn move_soldiers(sim: &mut SimHost, tile: (i32, i32)) {
    let units = own_soldiers(sim);
    let target = Vec3::new(
        tile.0 as f32 * 128.0 + 64.0,
        0.0,
        tile.1 as f32 * 128.0 + 64.0,
    );
    assert!(sim.move_units(&units, target, None, FormationOrder::default()));
}

/// Scene of the test.
fn scene(i: usize, t: u64, sim: &mut SimHost) {
    match (i, t) {
        (0, 300) => move_soldiers(sim, (30, 12)),
        (0, 1_700) => move_soldiers(sim, (20, 30)),
        (1, 900) => sim.submit(Command {
            player: 1,
            kind: CommandKind::Diplomacy {
                p: 1,
                q: 0,
                mode: 0,
            },
        }),
        (2, 1_200) => sim.submit(Command {
            player: 2,
            kind: CommandKind::Chat {
                player: 2,
                mask: 0xFF,
                text: "Hallo".encode_utf16().collect(),
            },
        }),
        _ => {}
    }
}

/// Host (index 0) and two clients with the delays [`DELAYS`].
struct Table {
    net: MemNet,
    machines: Vec<Machine>,
    /// Client side of the line for each client (index 0 = client 1).
    client_links: Vec<Arc<Mutex<MemLink>>>,
    now: u64,
    /// Time of the first frame with `Go` at all of them.
    go_at: u64,
    /// Tick of the host when [`Table::run`] cut the line.
    cut_step: Option<u64>,
    /// Storage locations of the machines (one subfolder per machine, [`paths`]).
    dir: tempfile::TempDir,
    /// This machine (default client 2) corrupts its state as soon as it (not standing) reaches the ticks from
    /// [`Self::corrupt_at`].
    corrupter: usize,
    /// Ticks of the corruption, once per entry.
    corrupt_at: Vec<u64>,
    /// Ticks at which the machine corrupted (state before this tick).
    corrupted: Vec<u64>,
}

impl Table {
    /// Sign-on, start, loading and `Go`; all stand at `e(0)` at most.
    fn running() -> Self {
        Self::running_with(false)
    }

    /// Like [`Self::running`]; `late_large`: the host sends large contents (the state during the sync) only
    /// [`LATE_POLLS`] frames later ([`LateLarge`]).
    fn running_with(late_large: bool) -> Self {
        Self::running_cfg(late_large, TurnConfig::default())
    }

    /// Like [`Self::running_with`] with fixed raster `cfg`.
    fn running_cfg(late_large: bool, cfg: TurnConfig) -> Self {
        let mut t = Self::loading_cfg(late_large, cfg);
        for m in &mut t.machines {
            m.driver.loaded(None);
        }
        t.frames_until(|t| t.machines.iter().all(|m| m.phase() == Phase::Running));
        // Everyone computes from its own `Go` (the host first), but not beyond `e(0)` without bundle 0.
        let e0 = cfg.exec_step(0);
        for m in &t.machines {
            assert!(m.sim.state.steps <= e0, "tick {}", m.sim.state.steps);
        }
        t.go_at = t.now;
        t
    }

    /// Sign-on and start; all stand in `Loading` and have checked the start package.
    fn loading() -> Self {
        Self::loading_with(false)
    }

    fn loading_with(late_large: bool) -> Self {
        Self::loading_cfg(late_large, TurnConfig::default())
    }

    fn loading_cfg(late_large: bool, cfg: TurnConfig) -> Self {
        let net = MemNet::new();
        let dir = tempfile::tempdir().expect("folder");
        let mut host = NetDriver::host(
            crate::testutil::net_host_core(hello("Host"), cfg, 8),
            paths(dir.path(), 0),
        );
        let mut clients = Vec::new();
        let mut client_links = Vec::new();
        for slot in 1..NET_SLOTS {
            let (h, c) = net.pair(DELAYS[usize::from(slot)]);
            if late_large {
                host.add_link(
                    slot,
                    Box::new(LateLarge {
                        inner: h,
                        held: Vec::new(),
                    }),
                );
            } else {
                host.add_link(slot, Box::new(h));
            }
            let c = Arc::new(Mutex::new(c));
            client_links.push(c.clone());
            let core = ClientCore::new(hello(&format!("Client {slot}")));
            clients.push(NetDriver::client(
                core,
                Box::new(SharedLink(c)),
                paths(dir.path(), usize::from(slot)),
            ));
        }
        let mut machines = vec![Machine::new(host, 0)];
        for (i, driver) in clients.into_iter().enumerate() {
            machines.push(Machine::new(driver, i as u8 + 1));
        }
        for m in &mut machines {
            m.driver.set_fixed_clock(Some(CLOCK));
        }
        let mut t = Self {
            net,
            machines,
            client_links,
            now: 0,
            go_at: 0,
            cut_step: None,
            dir,
            corrupter: 2,
            corrupt_at: Vec::new(),
            corrupted: Vec::new(),
        };
        t.frames_until(|t| {
            t.machines[1..]
                .iter()
                .all(|m| m.driver.client_core().is_some_and(|c| c.peer().is_some()))
        });
        let package = {
            let sim = &t.machines[0].sim;
            start_package(&sim.rules, &sim.setup)
        };
        let info = |peer: u8| {
            let seats: [Seat; 8] = std::array::from_fn(|slot| Seat {
                peer: (slot < usize::from(NET_SLOTS)).then_some(slot as u8),
                name: format!("P{slot}"),
            });
            StartInfo {
                package: package.clone(),
                seats,
                your_slot: peer,
                spacing: cfg.raw().0,
                lead: cfg.raw().1,
                game_speed: DEFAULT_GAME_SPEED,
                session: "Testpartie".into(),
                saved: None,
            }
        };
        t.machines[0]
            .driver
            .host_core_mut()
            .expect("Host")
            .start(info);
        t.frames_until(|t| t.machines.iter().all(|m| m.phase() == Phase::Loading));
        for m in &mut t.machines {
            let start = m.driver.start_info().expect("Start");
            assert_eq!(start.package, start_package(&m.sim.rules, &m.sim.setup));
            assert_eq!(start.your_slot, m.sim.active_player);
        }
        t
    }

    /// One frame: time advances, inputs of the scene, corrupting ([`Self::corrupter`], [`Self::corrupt_at`]), every
    /// machine once.
    fn frame(&mut self, scene: Scene) {
        self.now += FRAME_MS;
        self.net.advance_to(self.now);
        let t = self.now - self.go_at;
        for (i, m) in self.machines.iter_mut().enumerate() {
            if self.go_at > 0 {
                scene(i, t, &mut m.sim);
            }
            if i == self.corrupter
                && let Some(&at) = self.corrupt_at.get(self.corrupted.len())
                && !m.driver.halted()
                && m.sim.state.steps >= at
            {
                assert_eq!(m.sim.debug_corrupt(), "daytime");
                self.corrupted.push(m.sim.state.steps);
            }
            m.frame(self.now);
        }
    }

    fn frames_until(&mut self, cond: impl Fn(&Self) -> bool) {
        for _ in 0..MAX_FRAMES {
            if cond(self) {
                return;
            }
            self.frame(|_, _, _| {});
        }
        panic!("condition not reached after {MAX_FRAMES} frames");
    }

    /// Plays `scene` until every machine still in the match has computed tick [`STEPS`]. `cut`: client `n` (1 ...)
    /// drops out at this time after `Go`.
    fn run(&mut self, scene: Scene, cut: Option<(usize, u64)>) {
        self.run_observed(scene, cut, &mut |_| {});
    }

    /// Like [`Self::run`]; `observe` sees the table after every frame.
    fn run_observed(
        &mut self,
        scene: Scene,
        cut: Option<(usize, u64)>,
        observe: &mut dyn FnMut(&Self),
    ) {
        for _ in 0..MAX_FRAMES {
            if self
                .machines
                .iter()
                .filter(|m| m.failed.is_none())
                .all(|m| m.sim.state.steps >= STEPS)
            {
                return;
            }
            if let Some((n, at)) = cut
                && self.now - self.go_at == at
            {
                self.client_links[n - 1]
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .cut();
                self.cut_step = Some(self.machines[0].sim.state.steps);
            }
            self.frame(scene);
            observe(self);
        }
        let steps: Vec<u64> = self.machines.iter().map(|m| m.sim.state.steps).collect();
        panic!("tick {STEPS} not reached, state {steps:?}");
    }
}

/// FNV-1a 64 over the sequence (per entry `steps` and checksum, little endian).
fn sequence_hash(seq: &[(u64, u64)]) -> u64 {
    let bytes: Vec<u8> = seq
        .iter()
        .flat_map(|&(s, h)| s.to_le_bytes().into_iter().chain(h.to_le_bytes()))
        .collect();
    crate::snapshot::fnv1a64(&bytes)
}

fn assert_full(seq: &[(u64, u64)]) {
    assert_eq!(
        seq.len() as u64,
        STEPS + 1,
        "one entry per tick 0 ... {STEPS}"
    );
    for (i, &(s, _)) in seq.iter().enumerate() {
        assert_eq!(s, i as u64, "sequence without gaps");
    }
}

/// E-255/E-256/E-257/E-277/E-290: Three machines with 0/40/130 ms compute the same checksum sequence over 400
/// ticks; every machine records all commands, at the execution ticks of the raster.
// QUIRK E-276, E-277, E-290: kept
// QUIRK E-255, E-256, E-257: deviation
#[test]
fn three_peers_share_the_hash_sequence() {
    let mut t = Table::running();
    t.run(scene, None);
    let host = t.machines[0].prefix();
    assert_full(&host);
    for m in &t.machines[1..] {
        assert_eq!(m.prefix(), host, "same sequence as the host");
    }
    let cfg = TurnConfig::default();
    let log = &t.machines[0].sim.log;
    let kinds: Vec<&CommandKind> = log.iter().map(|c| &c.command.kind).collect();
    let count = |f: fn(&CommandKind) -> bool| kinds.iter().filter(|k| f(k)).count();
    assert_eq!(
        count(|k| matches!(k, CommandKind::Move { .. })),
        2,
        "{kinds:?}"
    );
    assert_eq!(
        count(|k| matches!(k, CommandKind::Diplomacy { .. } | CommandKind::Chat { .. })),
        2,
        "diplomacy and chat as turn commands (E-277): {kinds:?}"
    );
    assert_eq!(log.len(), 4, "two moves, diplomacy, chat: {kinds:?}");
    for c in log {
        assert!(
            cfg.exec_at(c.step).is_some(),
            "command at tick {}, not an execution tick",
            c.step
        );
    }
    for m in &t.machines[1..] {
        assert_eq!(
            &m.sim.log, log,
            "every machine records all commands (E-290)"
        );
    }
    // The commands take effect: friendship 1 -> 0, the soldiers of slot 0 no longer stand at the start.
    for m in &t.machines {
        assert!(m.sim.state.diplomacy.is_friend(1, 0));
        let start = testworld::net_unit_tile(0, 0);
        let moved = own_soldiers(&t.machines[0].sim).iter().any(|&id| {
            let o = m.sim.state.object(id, false).expect("soldier");
            o.tile != start && o.tile != testworld::net_unit_tile(0, 1)
        });
        assert!(moved, "movement executed");
    }
    // The log is ordered by tick (bundles in turn order).
    let steps: Vec<u64> = log.iter().map(|c| c.step).collect();
    assert!(steps.windows(2).all(|w| w[0] <= w[1]), "{steps:?}");
}

/// The network match equals the replay of its command log without a network: the same checksum tick by tick.
// QUIRK E-290: kept
// QUIRK E-255: deviation
#[test]
fn net_match_equals_its_replay() {
    let mut t = Table::running();
    t.run(scene, None);
    let host = &t.machines[0];
    let replay = host.sim.replay().expect("replay");
    let cfg = TurnConfig::default();
    assert!(!replay.commands.is_empty());
    for c in &replay.commands {
        assert!(cfg.exec_at(c.step).is_some(), "tick {}", c.step);
    }
    let rules = &host.sim.rules;
    let mut state = new_match(rules, &replay.start.setup).expect("start package");
    assert!(state.network, "network flag in the start package");
    let mut seq = vec![(0, checksum(&state))];
    let mut next = 0;
    while state.steps < STEPS {
        let now = state.steps;
        let mut commands = Vec::new();
        while next < replay.commands.len() && replay.commands[next].step == now {
            commands.push(replay.commands[next].command.clone());
            next += 1;
        }
        step(&mut state, rules, &commands);
        seq.push((state.steps, checksum(&state)));
    }
    assert_eq!(seq, host.prefix());
    let played = replay.play(rules, STEPS).expect("replaying");
    assert_eq!(checksum(&played), host.prefix()[STEPS as usize].1);
}

/// `NET_GOLDEN` in debug and release (`cargo test -p opennml-client --release net_driver`).
// QUIRK E-257: deviation
#[test]
fn net_golden_is_fixed_in_debug_and_release() {
    let mut t = Table::running();
    t.run(scene, None);
    let host = t.machines[0].prefix();
    assert_full(&host);
    let h = sequence_hash(&host);
    assert_eq!(h, NET_GOLDEN, "checksum sequence {h:#018x}");
}

/// `NET_GOLDEN_FINE` in debug and release. Like `NET_GOLDEN`, but with the stage `LanFast` (2/1): all three machines
/// compute the same sequence. The value is measured; `NET_GOLDEN` itself stays at the raster of the original.
const NET_GOLDEN_FINE: u64 = 0xc802_0248_f8a7_5a72;

// QUIRK E-255: deviation
#[test]
fn net_golden_fine_is_fixed() {
    let mut t = Table::running_cfg(false, RasterStage::LanFast.config());
    t.run(scene, None);
    let host = t.machines[0].prefix();
    assert_full(&host);
    for m in &t.machines[1..] {
        assert_eq!(m.prefix(), host, "same sequence on all machines");
    }
    let h = sequence_hash(&host);
    assert_eq!(h, NET_GOLDEN_FINE, "checksum sequence {h:#018x}");
}

/// All three machines continue the same state at tick 37, origin 37, raster 8/4. Turn `k` begins at `37 + 8k`
/// (contributions before 37, 45, ...), its commands take effect at `41 + 8k`; `next_exec_step` counts absolutely. All
/// compute the same sequence from tick 37 without desync, and every command is in the log at an execution tick of the
/// shifted raster.
// QUIRK E-328: deviation
#[test]
fn origin_shifts_the_round_grid() {
    const ORIGIN: u64 = 37;
    let mut base = testworld::net_host(0, true);
    for _ in 0..ORIGIN {
        base.step_with(Vec::new());
    }
    let save = base.save();
    let mut t = Table::loading();
    for (i, m) in t.machines.iter_mut().enumerate() {
        m.sim.load(save.clone(), None);
        m.sim.enter_rounds(i as u8, i == 0).expect("human slot");
        m.seq.clear();
        m.collect();
        m.driver.set_origin(ORIGIN);
        assert_eq!(m.driver.origin(), ORIGIN);
        assert_eq!(
            m.driver.status().next_exec_step,
            ORIGIN + 4,
            "e(0) absolute"
        );
        m.driver.loaded(None);
    }
    // The raster of the driver: turn start and execution from the origin on, nothing before it.
    let d = &t.machines[0].driver;
    for (s, round, exec) in [
        (0, None, None),
        (4, None, None),
        (32, None, None),
        (36, None, None),
        (37, Some(0), None),
        (41, None, Some(0)),
        (45, Some(1), None),
        (49, None, Some(1)),
        (53, Some(2), None),
    ] {
        assert_eq!((d.round_at(s), d.exec_at(s)), (round, exec), "tick {s}");
    }
    t.frames_until(|t| t.machines.iter().all(|m| m.phase() == Phase::Running));
    for m in &t.machines {
        assert!(
            m.sim.state.steps <= ORIGIN + 4,
            "tick {}",
            m.sim.state.steps
        );
    }
    t.go_at = t.now;
    t.run(scene, None);
    let host = t.machines[0].prefix();
    assert_eq!(
        host.len() as u64,
        STEPS - ORIGIN + 1,
        "sequence from tick 37"
    );
    for (i, &(s, _)) in host.iter().enumerate() {
        assert_eq!(s, ORIGIN + i as u64, "without gaps from the origin");
    }
    for m in &t.machines[1..] {
        assert_eq!(m.prefix(), host, "same sequence as the host");
    }
    assert_eq!(t.machines[0].driver.status().desyncs, Some(0));
    let log = &t.machines[0].sim.log;
    assert_eq!(log.len(), 4, "two moves, diplomacy, chat: {log:?}");
    for c in log {
        assert_eq!(
            (c.step - ORIGIN) % 8,
            4,
            "command at tick {}, no execution tick from 41",
            c.step
        );
    }
    let status = t.machines[0].driver.status();
    assert_eq!(
        status.next_exec_step,
        ORIGIN + 8 * u64::from(status.round) + 4
    );
}

/// Client 2 drops out at 2,000 ms without `Bye` (`cut`). Its slot loses at host and client 1 at the same tick, the
/// host no longer waits for it afterwards, and the sequences stay equal.
// QUIRK E-267, E-272: kept
// QUIRK E-256: deviation
#[test]
fn client_vanishes_mid_match() {
    let mut t = Table::running();
    t.run(scene, Some((2, CUT_MS)));
    assert!(
        t.machines[2].failed.is_some(),
        "client 2 notices the dropout"
    );
    assert_eq!(
        t.machines[0]
            .lost
            .iter()
            .map(|(slot, _)| *slot)
            .collect::<Vec<_>>(),
        vec![2],
        "host reports the dropout once"
    );
    let host = t.machines[0].prefix();
    assert_full(&host);
    assert_eq!(t.machines[1].prefix(), host, "client 1 stays equal");
    let gone = &t.machines[2].seq;
    assert!(!gone.is_empty() && gone.len() < host.len());
    assert_eq!(&host[..gone.len()], &gone[..], "equal until the dropout");
    let losses = |m: &Machine| -> Vec<u64> {
        m.sim
            .log
            .iter()
            .filter(|c| {
                c.command.player == 2
                    && c.command.kind == CommandKind::SetPlayerResult { slot: 2, result: 1 }
            })
            .map(|c| c.step)
            .collect()
    };
    let at = losses(&t.machines[0]);
    assert_eq!(at.len(), 1, "one dropout command");
    // At once, not after 30 s of silence: at the latest at the execution tick two turns after the turn of the host
    // at the cut.
    let cfg = TurnConfig::default();
    let cut_step = t.cut_step.expect("cut");
    let latest = cfg.exec_step((cut_step / cfg.spacing()) as u32 + 2);
    assert!(
        at[0] <= latest,
        "dropout at tick {} (cut at {cut_step}, latest {latest})",
        at[0]
    );
    assert_eq!(losses(&t.machines[1]), at, "at the same tick at client 1");
    assert!(TurnConfig::default().exec_at(at[0]).is_some());
    for m in &t.machines[..2] {
        assert_eq!(m.sim.state.players[2].result, 1, "slot 2 has lost");
        assert_eq!(m.sim.state.players[0].result, 0);
        assert_eq!(m.sim.state.players[1].result, 0);
        // E-272: The message 17200 + p "... was eliminated!" (message 200 + p) is created by the simulation itself
        // when it executes the dropout command (`game_end::set_player_result`, free play), once per machine at the tick
        // after the execution tick; the flow shows nothing additional for the dropout.
        let shown: Vec<u64> = m
            .messages
            .iter()
            .filter(|&&(_, id)| id == opennml_sim::game_end::DEFEAT_MESSAGE + 2)
            .map(|&(s, _)| s)
            .collect();
        assert_eq!(shown.len(), 1, "message 202 once: {shown:?}");
        assert!(
            (at[0] + 1..=at[0] + 2).contains(&shown[0]),
            "in the frame of the defeat (tick {}): {shown:?}",
            at[0]
        );
    }
}

/// Client 1 pauses. All machines stand after the execution tick `e(k)` of the bundle with the halt at `e(k) + 1`, show
/// the pause there and do not compute on until client 1 lifts it. Before that nobody shows the pause, and the distance
/// between the machines stays at most `T + V` in every frame.
// QUIRK O-316: kept
// QUIRK E-264: deviation
#[test]
fn net_pause_holds_every_peer_until_resume() {
    let mut t = Table::running();
    let cfg = TurnConfig::default();
    let bound = cfg.spacing() + cfg.exec_step(0);
    let mut stand: Option<u64> = None;
    let mut paused_frames = 0;
    let mut resumed = false;
    for _ in 0..MAX_FRAMES {
        if t.machines.iter().all(|m| m.sim.state.steps >= STEPS) {
            break;
        }
        let since = t.now - t.go_at;
        if since == 1_000 {
            assert!(t.machines[1].driver.toggle_pause(), "pause request");
        }
        t.frame(scene);
        let steps: Vec<u64> = t.machines.iter().map(|m| m.sim.state.steps).collect();
        let (lo, hi) = (*steps.iter().min().unwrap(), *steps.iter().max().unwrap());
        assert!(hi - lo <= bound, "distance {steps:?} above T + V = {bound}");
        let halt = t.machines[0]
            .controls
            .iter()
            .any(|c| matches!(c, NetControl::Halt { by: 1 }));
        if stand.is_none() && halt {
            // The host has executed the bundle with the halt: it stands at `e(k) + 1`.
            stand = Some(t.machines[0].sim.state.steps);
        }
        let shown: Vec<bool> = t.machines.iter().map(|m| m.driver.pause_shown()).collect();
        for (i, m) in t.machines.iter().enumerate() {
            if shown[i] {
                assert_eq!(
                    Some(m.sim.state.steps),
                    stand,
                    "machine {i} shows the pause only at the standstill"
                );
                assert!(m.driver.halted());
            }
        }
        let Some(stand) = stand else {
            continue;
        };
        assert!(
            cfg.exec_at(stand - 1).is_some(),
            "standstill {stand} after an execution tick"
        );
        if !resumed {
            for (i, &s) in steps.iter().enumerate() {
                assert!(
                    s <= stand,
                    "machine {i} at tick {s} before lifting (standstill {stand})"
                );
            }
        }
        if !resumed && shown.iter().all(|&s| s) {
            paused_frames += 1;
            if paused_frames == 40 {
                // Only the pausing player lifts it (O-316).
                assert!(
                    !t.machines[0].driver.toggle_pause(),
                    "host does not lift a foreign pause"
                );
                assert!(!t.machines[2].driver.toggle_pause(), "client 2 likewise");
                assert!(t.machines[1].driver.toggle_pause(), "client 1 lifts it");
                resumed = true;
            }
        }
    }
    assert!(resumed, "all stood together in the pause for 40 frames");
    for (i, m) in t.machines.iter().enumerate() {
        assert!(
            m.sim.state.steps >= STEPS,
            "machine {i} computes on after lifting"
        );
        assert!(!m.driver.pause_shown());
        assert_eq!(
            m.controls
                .iter()
                .filter(|c| matches!(c, NetControl::Halt { .. }))
                .count(),
            1,
            "machine {i}: one halt"
        );
    }
    let host = t.machines[0].prefix();
    assert_full(&host);
    for m in &t.machines[1..] {
        assert_eq!(m.prefix(), host, "same sequence despite the pause");
    }
}

/// E-274: The simulation part of a network savegame comes from the execution tick, not from the end of the frame.
/// Client 1 computes with four times the frame time (two ticks per frame, so in the frame of `e(k)` it often already
/// computes `e(k) + 1`); nevertheless all three record the same bytes at `e(k) + 1` for `Save` and for the `Stop` after
/// the third desync.
#[test]
fn net_saves_are_taken_at_the_exec_step() {
    let mut t = Table::running();
    t.machines[1].dt_ms = 4.0 * FRAME_MS as f64;
    t.corrupt_at = vec![45, 101, 149];
    let cfg = TurnConfig::default();
    for _ in 0..MAX_FRAMES {
        let stopped = t
            .machines
            .iter()
            .all(|m| m.saves.iter().any(|s| s.name.starts_with("checksumerror_")));
        if stopped {
            break;
        }
        let since = t.now - t.go_at;
        if [500, 1_500, 2_500].contains(&since) {
            let name = format!("netz{}", since / 1_000 + 1);
            assert!(t.machines[2].driver.wish_save(&name));
        }
        t.frame(scene);
    }
    let names: Vec<&str> = t.machines[0]
        .saves
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["netz1", "netz2", "netz3", "checksumerror_2026_11_06_1415"],
        "saving and stop"
    );
    for (i, m) in t.machines.iter().enumerate() {
        assert_eq!(m.saves.len(), 4, "machine {i}");
        for (save, host) in m.saves.iter().zip(&t.machines[0].saves) {
            assert_eq!(save.name, host.name);
            assert_eq!(
                save.sim.state.steps, host.sim.state.steps,
                "machine {i}, \"{}\"",
                save.name
            );
            // Client 2 corrupts (desync until the sync, forever at the stop): its states after the first corruption
            // rightly deviate; it is compared only before that.
            if i == 2 && save.sim.state.steps > t.corrupted[0] {
                continue;
            }
            assert_eq!(
                save.sim.to_bytes(),
                host.sim.to_bytes(),
                "machine {i}, \"{}\": same bytes",
                save.name
            );
            assert_eq!(save.game_speed, host.game_speed);
            let at = save.sim.state.steps;
            assert!(
                cfg.exec_at(at - 1).is_some(),
                "\"{}\" at e(k) + 1, not {at}",
                save.name
            );
        }
    }
    // The fast machine was at the end of the frame already beyond `e(k) + 1` at least once: a state from the end of the
    // frame would deviate there, otherwise the test would check nothing.
    let fast = &t.machines[1];
    let ahead = fast
        .saves
        .iter()
        .zip(&fast.save_frame_steps)
        .filter(|(s, end)| **end > s.sim.state.steps)
        .count();
    assert!(
        ahead > 0,
        "ticks at the end of the frame {:?}",
        fast.save_frame_steps
    );
}

/// If a client adopts the state of the host during the sync, the driver reports it exactly once
/// ([`NetFrame::adopted`]) and without the events of the discarded state; the others never.
#[test]
fn adopt_is_reported_once_without_events() {
    let mut t = Table::running();
    t.corrupt_at = vec![45];
    t.run(scene, None);
    assert_eq!(t.machines[2].adopted, vec![0], "once, without events");
    assert!(t.machines[0].adopted.is_empty());
    assert!(t.machines[1].adopted.is_empty());
}

/// Turn mode of the game host: own commands go through `outgoing` (also `execute_now`, `flush_pending` and with the
/// match halted), foreign senders and the slot switch are locked, and `step_with` books the commands with the tick
/// before.
#[test]
fn round_mode_routes_own_commands_to_outgoing() {
    let mut sim = testworld::net_host(1, false);
    assert_eq!(sim.active_player, 1);
    let own = |tag: i32| Command {
        player: 1,
        kind: CommandKind::Diplomacy {
            p: 1,
            q: tag,
            mode: 0,
        },
    };
    let before = checksum(&sim.state);
    sim.halted = true;
    sim.submit(own(0));
    sim.halted = false;
    sim.submit(own(2));
    sim.execute_now(own(3));
    sim.flush_pending();
    sim.submit(Command {
        player: 0,
        kind: CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 0,
        },
    });
    assert_eq!(checksum(&sim.state), before, "nothing executed at once");
    assert!(sim.log.is_empty());
    assert!(sim.pending_commands().is_empty());
    assert_eq!(sim.take_outgoing(), vec![own(0), own(2), own(3)]);
    assert!(sim.take_outgoing().is_empty());
    sim.switch_player(1);
    assert_eq!(sim.active_player, 1, "slot switch locked");
    assert!(sim.set_active_player(0).is_err());
    assert!(sim.set_active_player(1).is_ok());
    assert_eq!(sim.hash_at(0), Some(before));
    assert_eq!(sim.hash_at(1), None);
    sim.step_with(vec![own(4)]);
    assert_eq!(sim.state.steps, 1);
    assert_eq!(sim.log.len(), 1);
    assert_eq!(sim.log[0].step, 0, "booked with the tick before");
    assert_eq!(sim.hash_at(1), Some(sim.checksum()));
    assert_eq!(
        sim.results(),
        [false, false, false, true, true, true, true, true]
    );
}

/// Mirror commands of the UI are sent in the network only by the host, through `outgoing` and once per selection, also
/// while the command is still on its way.
#[test]
fn mirror_commands_go_out_only_from_the_host() {
    use crate::game::testworld::abl;
    let with_35 = abl(&[(&["&35;x"], &["n10"])]);
    let rules = testworld::script_rules(&with_35);
    let mut setup = opennml_sim::setup::MatchSetup::default_for(&rules);
    setup.script.enabled = true;
    let (a, b) = (ObjectId(0x800), ObjectId(0x801));
    for host in [true, false] {
        let mut sim = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        sim.net = true;
        sim.net_host = host;
        let me = sim.active_player;
        sim.sync_selection(&[a, b]);
        let first = sim.take_outgoing();
        sim.sync_selection(&[b, a]);
        sim.sync_selection(&[a, b]);
        sim.press_button(7);
        let rest = sim.take_outgoing();
        assert!(sim.pending_commands().is_empty());
        if host {
            assert_eq!(
                first,
                vec![Command {
                    player: me,
                    kind: CommandKind::ScriptSelection { ids: vec![a, b] },
                }]
            );
            assert_eq!(
                rest,
                vec![Command {
                    player: me,
                    kind: CommandKind::ScriptButton { word: 7 },
                }],
                "same selection on its way: no second mirror command"
            );
        } else {
            assert!(
                first.is_empty() && rest.is_empty(),
                "client: no mirror commands"
            );
        }
    }
}

/// E-293: If loading fails at the host, the session aborts for all. Every client gets `Bye`, reports `failed` and
/// stands in `Phase::Ended`; nobody computes a tick.
#[test]
fn host_load_failure_ends_the_session_for_all() {
    let mut t = Table::loading();
    for m in &mut t.machines[1..] {
        m.driver.loaded(None);
    }
    t.machines[0].driver.loaded(Some("Map missing".into()));
    t.frames_until(|t| t.machines.iter().all(|m| m.failed.is_some()));
    for (i, m) in t.machines.iter().enumerate() {
        assert_eq!(m.phase(), Phase::Ended, "machine {i}");
        let text = m.failed.as_deref().unwrap_or_default();
        assert!(text.contains("Map missing"), "machine {i}: {text}");
        assert_eq!(m.sim.state.steps, 0, "nobody computes");
        // Whoever is already in the loaded match gets an end with a message.
        assert!(
            matches!(m.finish.as_slice(), [NetFinish::Failed(f)] if f.contains("Map missing")),
            "machine {i}: {:?}",
            m.finish
        );
    }
}

/// The host leaves the session after the clients have loaded, but before `Go`. Every client is already in the match
/// (only it calls `advance`) and reports exactly once `NetFinish::Failed` with the reason - otherwise the loaded map
/// would stand without a message and without a way to the main menu. No tick runs, no state.
#[test]
fn host_leaving_before_go_finishes_the_loaded_match() {
    let mut t = Table::loading();
    for m in &mut t.machines[1..] {
        m.driver.loaded(None);
    }
    t.frames_until(|t| {
        t.machines[0]
            .driver
            .host_core()
            .is_some_and(|h| h.phase() == &Phase::Loading)
    });
    t.machines[0].driver.leave("Check run: host aborts");
    t.frames_until(|t| t.machines[1..].iter().all(|m| !m.finish.is_empty()));
    for _ in 0..20 {
        t.frame(|_, _, _| {});
    }
    for (i, m) in t.machines.iter().enumerate().skip(1) {
        assert!(!m.driver.started(), "machine {i}: no Go");
        assert!(m.driver.finished(), "machine {i}");
        assert!(
            matches!(m.finish.as_slice(), [NetFinish::Failed(f)] if f.contains("The host has ended the connection")),
            "machine {i}: {:?}",
            m.finish
        );
        assert!(m.saves.is_empty(), "machine {i}: no state before Go");
        assert_eq!(m.sim.state.steps, 0, "machine {i}: no tick");
    }
}

/// In turn mode `advance` and `step_once` do not compute (only `advance_rounds` with bundles); the single-player path
/// computes as before.
#[test]
fn round_mode_blocks_advance_and_step_once() {
    let mut sim = testworld::net_host(0, true);
    let before = checksum(&sim.state);
    assert!(sim.advance(1_000.0).is_empty());
    assert!(sim.step_once().is_empty());
    assert_eq!(sim.state.steps, 0);
    assert_eq!(checksum(&sim.state), before);
    let rules = testworld::net_rules();
    let setup = testworld::net_setup(&rules);
    let mut solo = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
    solo.step_once();
    solo.advance(100.0);
    assert_eq!(
        solo.state.steps, 3,
        "single player: one tick plus two per frame"
    );
}

// --- State sync and desync report -----------------------------------------------------------------------------------

/// Turn whose contribution carries the corruption before tick `at` first: checksum before `b(k)` with `b(k) >= at`.
fn desync_round(at: u64) -> u32 {
    at.div_ceil(TurnConfig::default().spacing()) as u32
}

/// Tick at which all stand after the execution tick of turn `k` (`e(k) + 1`).
fn stand_step(k: u32) -> u64 {
    TurnConfig::default().exec_step(k) + 1
}

/// Report folders under `netlog`, sorted by name.
fn report_dirs(netlog: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(netlog) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries.map(|e| e.expect("entry").path()).collect();
    dirs.sort();
    dirs
}

/// File names of a folder, sorted.
fn file_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("report folder")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Lines of the report with checksums per state part (`Part <name>: ...`, deviating ones preceded by `≠`).
fn part_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.contains("Part ")).collect()
}

/// Names of the parts marked as deviating.
fn marked_parts(text: &str) -> Vec<String> {
    part_lines(text)
        .iter()
        .filter(|l| l.starts_with('≠'))
        .map(|l| {
            l.split("Part ")
                .nth(1)
                .and_then(|r| r.split(':').next())
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Client 2 corrupts its state (`daytime`) before tick 50. The host detects the deviation at the next contribution,
/// all halt after `e(k)`, client 2 writes the report (five files, `report.txt` marks exactly `daytime`), adopts state
/// and command log of the host, and afterwards all three compute the same sequence. Its game host part (own slot,
/// speed) stays.
// QUIRK E-260: deviation
#[test]
fn resync_restores_equal_hashes() {
    let mut t = Table::running();
    t.corrupt_at = vec![45];
    let speed = t.machines[2].sim.game_speed;
    t.run(scene, None);
    let at = t.corrupted[0];
    assert!(at < 50, "corrupted before tick 50, not at {at}");
    let stand = stand_step(desync_round(at));
    let host = t.machines[0].prefix();
    assert_full(&host);
    let after = |seq: &[(u64, u64)]| -> Vec<(u64, u64)> {
        seq.iter().copied().filter(|&(s, _)| s >= stand).collect()
    };
    for (i, m) in t.machines.iter().enumerate() {
        assert_full(&m.prefix());
        assert_eq!(
            after(&m.prefix()),
            after(&host),
            "machine {i} from tick {stand}"
        );
    }
    let differing: Vec<u64> = t.machines[2]
        .prefix()
        .iter()
        .zip(&host)
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a.0)
        .collect();
    assert!(
        !differing.is_empty(),
        "the corruption is visible in the sequence"
    );
    assert!(
        differing.iter().all(|&s| s >= at && s < stand),
        "only between corruption {at} and sync {stand}: {differing:?}"
    );
    assert_eq!(
        t.machines[1].prefix(),
        host,
        "client 1 never deviates and only waits"
    );
    assert_eq!(
        t.machines[2].sim.log, t.machines[0].sim.log,
        "log of the host"
    );
    assert!(t.machines[2].sim.replay().is_ok(), "replay complete");
    assert_eq!(t.machines[2].sim.active_player, 2, "own slot stays");
    assert_eq!(t.machines[2].sim.game_speed, speed, "speed stays");
    assert_eq!(t.machines[0].driver.status().desyncs, Some(1));
    for m in &t.machines {
        assert_eq!(m.phase(), Phase::Running);
        assert!(m.failed.is_none());
        assert!(
            m.controls
                .iter()
                .any(|c| matches!(c, NetControl::Resync { slots } if slots == &vec![2]))
        );
    }
    // Report only at the deviating client.
    for i in [0, 1] {
        assert!(
            report_dirs(&paths(t.dir.path(), i).netlog).is_empty(),
            "machine {i}"
        );
    }
    let dirs = report_dirs(&paths(t.dir.path(), 2).netlog);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert_eq!(
        dirs[0].file_name().unwrap().to_string_lossy(),
        "desync_2026_11_06_141516_1"
    );
    assert_eq!(file_names(&dirs[0]), REPORT_FILES);
    let text = std::fs::read_to_string(dirs[0].join("report.txt")).expect("report");
    assert_eq!(marked_parts(&text), vec!["daytime".to_string()], "{text}");
    assert!(text.contains(&format!("Tick: {stand}")), "{text}");
    let host_state = NetState::from_bytes(&std::fs::read(dirs[0].join("host.nmllog")).unwrap())
        .expect("host.nmllog");
    assert_eq!(host_state.save.state.steps, stand);
    let host_hash = host.iter().find(|&&(s, _)| s == stand).unwrap().1;
    assert_eq!(checksum(&host_state.save.state), host_hash);
    let local = NetState::from_bytes(&std::fs::read(dirs[0].join("local.nmllog")).unwrap())
        .expect("local.nmllog");
    assert_ne!(checksum(&local.save.state), host_hash);
    for name in ["host.nmlsave", "local.nmlsave"] {
        let save = SaveGame::from_bytes(&std::fs::read(dirs[0].join(name)).unwrap()).expect(name);
        assert!(save.commands.is_empty());
        assert_eq!(save.state.steps, stand);
    }
}

/// During the sync all machines stand after `e(k)` and do not compute on until the `Resume`. As long as the host is in
/// the sync, no machine is beyond `e(k) + 1`; likewise every client as long as its core is in the sync. The host does
/// not send its state until it stands there itself (otherwise client 2 would reject the tick and report `failed`).
// QUIRK E-260: deviation
#[test]
fn resync_holds_every_peer_until_resume() {
    let mut t = Table::running();
    t.corrupt_at = vec![45];
    let mut all_stood = 0;
    let mut halted_frames = 0;
    t.run_observed(scene, None, &mut |t| {
        let Some(&at) = t.corrupted.first() else {
            return;
        };
        let stand = stand_step(desync_round(at));
        let resync = Phase::Halted(HaltReason::Resync);
        let host_halted = t.machines[0].phase() == resync;
        for (i, m) in t.machines.iter().enumerate() {
            if host_halted || m.phase() == resync {
                assert!(
                    m.sim.state.steps <= stand,
                    "machine {i} at tick {} before the end of the sync (standstill {stand})",
                    m.sim.state.steps
                );
            }
        }
        if host_halted {
            halted_frames += 1;
            if t.machines.iter().all(|m| m.sim.state.steps == stand) {
                all_stood += 1;
            }
        }
    });
    assert!(halted_frames > 0, "the sync takes frames");
    assert!(all_stood > 0, "all stood together at e(k) + 1");
    for (i, m) in t.machines.iter().enumerate() {
        assert!(m.failed.is_none(), "machine {i}: {:?}", m.failed);
        assert!(
            m.sim.state.steps >= STEPS,
            "all keep running after the sync"
        );
    }
}

/// The third desync does not sync any more. All stand in `Stopped` after `e(k)`, every machine reports the control
/// entry `Stop` with the name from the local time of the host, and nobody computes on.
// QUIRK E-261: deviation
#[test]
fn third_desync_stops_all_peers() {
    let mut t = Table::running();
    t.corrupt_at = vec![45, 101, 149];
    t.frames_until(|t| {
        t.machines
            .iter()
            .all(|m| m.driver.halted() && m.phase() == Phase::Stopped)
    });
    assert_eq!(t.corrupted.len(), 3, "{:?}", t.corrupted);
    let stand = stand_step(desync_round(t.corrupted[2]));
    let name = CLOCK.checksum_error_name();
    assert_eq!(name, "checksumerror_2026_11_06_1415");
    for (i, m) in t.machines.iter().enumerate() {
        assert_eq!(m.sim.state.steps, stand, "machine {i} stands after e(k)");
        assert_eq!(m.driver.status().waiting, Some(WaitReason::Stopped));
        let stops: Vec<&NetControl> = m
            .controls
            .iter()
            .filter(|c| matches!(c, NetControl::Stop { .. }))
            .collect();
        assert_eq!(
            stops,
            vec![&NetControl::Stop { name: name.clone() }],
            "machine {i}"
        );
        let resyncs = m
            .controls
            .iter()
            .filter(|c| matches!(c, NetControl::Resync { .. }))
            .count();
        assert_eq!(resyncs, 2, "machine {i}: two syncs before the stop");
        // The end of the match reported once.
        assert_eq!(
            m.finish,
            vec![NetFinish::Stopped { name: name.clone() }],
            "machine {i}"
        );
    }
    assert_eq!(t.machines[0].driver.status().desyncs, Some(3));
    let dirs = report_dirs(&paths(t.dir.path(), 2).netlog);
    let names: Vec<String> = dirs
        .iter()
        .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["desync_2026_11_06_141516_1", "desync_2026_11_06_141516_2"],
        "one report per sync, none at the stop"
    );
    for _ in 0..40 {
        t.frame(|_, _, _| {});
    }
    for m in &t.machines {
        assert_eq!(m.sim.state.steps, stand, "the stop is final");
    }
}

/// Slots and bundles for the report.
fn report_inputs() -> ([Seat; 8], Vec<TurnBundle>) {
    let seats: [Seat; 8] = std::array::from_fn(|slot| Seat {
        peer: (slot < usize::from(NET_SLOTS)).then_some(slot as u8),
        name: format!("Player {slot}"),
    });
    let bundle = |turn: u32| TurnBundle {
        turn,
        commands: vec![Command {
            player: 1,
            kind: CommandKind::Diplomacy {
                p: 1,
                q: 0,
                mode: turn as i32,
            },
        }],
        control: Vec::new(),
    };
    let mut last = bundle(6);
    last.control.push(NetControl::Resync { slots: vec![1] });
    (seats, vec![bundle(4), bundle(5), last])
}

/// `report.txt` has one line per part of `SimState` with both values (number of lines = number of parts, in
/// declaration order), marks exactly the deviating ones, names turn, tick, both checksums, slots and names and the
/// commands of the last three bundles; the folder has the five files.
// QUIRK E-260: deviation
#[test]
fn report_lists_every_part() {
    let host_sim = testworld::net_host(0, true);
    let mut local_sim = testworld::net_host(1, false);
    assert_eq!(local_sim.debug_corrupt(), "daytime");
    let (host, local) = (host_sim.net_state(), local_sim.net_state());
    let (seats, bundles) = report_inputs();
    let dir = tempfile::tempdir().unwrap();
    let out = report::write_desync(dir.path(), 3, 6, 29, &local, &host, &seats, &bundles, CLOCK)
        .expect("report");
    assert_eq!(out, dir.path().join("desync_2026_11_06_141516_3"));
    assert_eq!(file_names(&out), REPORT_FILES);
    let text = std::fs::read_to_string(out.join("report.txt")).unwrap();
    let parts = part_checksums(&host.save.state);
    let lines = part_lines(&text);
    assert_eq!(lines.len(), parts.len(), "{text}");
    for (line, (name, value)) in lines.iter().zip(&parts) {
        assert!(line.contains(&format!("Part {name}:")), "{line}");
        assert!(line.contains(&format!("{value:#018x}")), "{line}");
    }
    assert_eq!(marked_parts(&text), vec!["daytime".to_string()]);
    for needle in [
        "Turn: 6".to_string(),
        "Tick: 29".to_string(),
        format!("Checksum host: {:#018x}", checksum(&host.save.state)),
        format!("Checksum local: {:#018x}", checksum(&local.save.state)),
        "Slot 0: Machine 0 (host), \"Player 0\"".to_string(),
        "Slot 2: Machine 2, \"Player 2\"".to_string(),
        "Slot 3: no machine, \"Player 3\"".to_string(),
        "Bundle 4".to_string(),
        "Bundle 5".to_string(),
        "Bundle 6".to_string(),
        "Resync".to_string(),
        "mode: 6".to_string(),
    ] {
        assert!(text.contains(&needle), "\"{needle}\" missing:\n{text}");
    }
    assert_eq!(
        SaveGame::from_bytes(&std::fs::read(out.join("host.nmlsave")).unwrap()).unwrap(),
        host.save
    );
    assert_eq!(
        NetState::from_bytes(&std::fs::read(out.join("local.nmllog")).unwrap()).unwrap(),
        local
    );
}

/// The network state (`NMLNETST`) survives writing and reading and rejects a foreign identifier and a wrong checksum
/// with an error message.
// QUIRK E-260: deviation
#[test]
fn net_state_round_trips_and_rejects_damage() {
    let mut sim = testworld::net_host(0, true);
    for _ in 0..6 {
        sim.step_with(Vec::new());
    }
    let own = |step: u64, tag: i32| RecordedCommand {
        step,
        command: Command {
            player: 0,
            kind: CommandKind::Diplomacy {
                p: 0,
                q: tag,
                mode: 0,
            },
        },
    };
    sim.log = vec![own(0, 1), own(4, 2), own(4, 3)];
    let state = sim.net_state();
    assert!(state.save.commands.is_empty());
    assert_eq!(state.log, sim.log);
    let bytes = state.to_bytes();
    assert_eq!(&bytes[..8], b"NMLNETST");
    assert_eq!(NetState::from_bytes(&bytes).unwrap(), state);
    state.check(&sim.rules).expect("matches the rules");
    let err = |bytes: &[u8]| format!("{:#}", NetState::from_bytes(bytes).unwrap_err());
    let mut foreign = bytes.clone();
    foreign[..8].copy_from_slice(b"NMLSAVE\0");
    assert!(err(&foreign).contains("identifier"), "{}", err(&foreign));
    let mut flipped = bytes.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 1;
    assert!(err(&flipped).contains("checksum"), "{}", err(&flipped));
}

/// `adopt` takes state and log of the host and rebuilds history, fog and fog memories; own slot, speed, fog switch and
/// turn mode stay. A state of a different match is rejected.
// QUIRK E-260: deviation
#[test]
fn adopt_takes_state_and_log_and_keeps_the_host_part() {
    let mut host = testworld::net_host(0, true);
    for _ in 0..9 {
        host.step_with(Vec::new());
    }
    host.log = vec![RecordedCommand {
        step: 4,
        command: Command {
            player: 1,
            kind: CommandKind::Diplomacy {
                p: 1,
                q: 0,
                mode: 0,
            },
        },
    }];
    let mut client = testworld::net_host(2, false);
    client.game_speed = 3;
    client.set_fog_shown(false);
    client.debug_corrupt();
    for _ in 0..9 {
        client.step_with(Vec::new());
    }
    assert_ne!(client.checksum(), host.checksum());
    client.adopt(host.net_state()).expect("adopted");
    assert_eq!(client.state, host.state);
    assert_eq!(client.log, host.log);
    assert_eq!(client.checksum(), host.checksum());
    assert_eq!(client.hashes.len(), 1, "history rebuilt");
    assert_eq!(client.hash_at(9), Some(host.checksum()));
    assert_eq!(client.active_player, 2);
    assert_eq!(client.game_speed, 3);
    assert_eq!(client.fog.show_mode, crate::game::fog_view::FOG_OFF);
    assert!(client.net && !client.net_host);
    // Other match: other start package.
    let rules = testworld::net_rules();
    let mut setup = testworld::net_setup(&rules);
    setup.seed = 8;
    let other = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
    let before = client.checksum();
    let e = client.adopt(other.net_state()).unwrap_err();
    assert!(format!("{e:#}").contains("different match"), "{e:#}");
    assert_eq!(client.checksum(), before, "unchanged on rejection");
}

/// Late arrival: client 1 (40 ms) deviates, the host sends its state delayed ([`LateLarge`]). Client 1 stands after
/// `e(k)` before the state of the host is there, adopts it while pumping at the standstill and reports `Synced`;
/// afterwards all compute the same sequence.
// QUIRK E-260: deviation
#[test]
fn resync_of_a_near_client_adopts_while_standing() {
    let mut t = Table::running_with(true);
    t.corrupter = 1;
    t.corrupt_at = vec![45];
    let mut stood_without_state = 0;
    t.run_observed(scene, None, &mut |t| {
        let Some(&at) = t.corrupted.first() else {
            return;
        };
        let m = &t.machines[1];
        if m.driver.halted() && m.sim.state.steps == stand_step(desync_round(at)) {
            stood_without_state += usize::from(m.driver.adopt_due.is_some());
        }
    });
    assert!(stood_without_state > 0, "client 1 stood before the arrival");
    let stand = stand_step(desync_round(t.corrupted[0]));
    let host = t.machines[0].prefix();
    for (i, m) in t.machines.iter().enumerate() {
        assert!(m.failed.is_none(), "machine {i}");
        let after: Vec<_> = m
            .prefix()
            .into_iter()
            .filter(|&(s, _)| s >= stand)
            .collect();
        let want: Vec<_> = host.iter().copied().filter(|&(s, _)| s >= stand).collect();
        assert_eq!(after, want, "machine {i}");
    }
    assert_eq!(report_dirs(&paths(t.dir.path(), 1).netlog).len(), 1);
    assert!(report_dirs(&paths(t.dir.path(), 2).netlog).is_empty());
}

// --- End and abort of the host, leaving ------------------------------------------------------------------------------

/// Plays `scene` until `ms` of played time have passed after `Go`.
fn play_for(t: &mut Table, ms: u64) {
    let until = t.go_at + ms;
    while t.now < until {
        t.frame(scene);
    }
}

/// E-268: The host ends the match ([`NetDriver::end`]). All machines compute up to the execution tick of the bundle
/// with `End`, record the network savegame `netz_YYYY_MM_DD_hhmm` (local time of the host) there and then stand
/// forever; the simulation parts are bit-identical, and everyone reports the end exactly once ([`NetFrame::finish`]),
/// without failure at the clients (the `Bye` of the host after the bundle has no effect).
// QUIRK E-268: deviation
#[test]
fn host_end_saves_equal_states_everywhere() {
    let mut t = Table::running();
    play_for(&mut t, 1_000);
    assert!(t.machines[0].driver.end(), "the host ends it");
    assert!(!t.machines[1].driver.end(), "a client cannot end it");
    t.frames_until(|t| t.machines.iter().all(|m| !m.finish.is_empty()));
    let name = CLOCK.net_end_name();
    assert_eq!(name, "netz_2026_11_06_1415");
    let host = t.machines[0]
        .saves
        .last()
        .expect("state of the host")
        .clone();
    let at = host.sim.state.steps;
    assert!(
        TurnConfig::default().exec_at(at - 1).is_some(),
        "after e(k): {at}"
    );
    for (i, m) in t.machines.iter().enumerate() {
        assert_eq!(
            m.finish,
            vec![NetFinish::HostEnded { name: name.clone() }],
            "machine {i}"
        );
        assert_eq!(m.saves.len(), 1, "machine {i}: one network savegame");
        let save = &m.saves[0];
        assert_eq!(save.name, name);
        assert_eq!(
            save.sim.to_bytes(),
            host.sim.to_bytes(),
            "machine {i}: same simulation part"
        );
        assert_eq!(m.sim.state.steps, at, "machine {i} stands after e(k)");
        assert!(m.driver.halted(), "machine {i}");
        assert_eq!(m.phase(), Phase::Ended, "machine {i}");
        assert_eq!(m.failed, None, "machine {i}");
        assert!(m.lost.is_empty(), "machine {i}: {:?}", m.lost);
    }
    for _ in 0..40 {
        t.frame(scene);
    }
    for (i, m) in t.machines.iter().enumerate() {
        assert_eq!(m.sim.state.steps, at, "machine {i}: the end is final");
        assert_eq!(m.finish.len(), 1, "machine {i}");
        assert_eq!(m.saves.len(), 1, "machine {i}");
    }
}

/// The host crashes (all its connections cut, it no longer computes). Every client reports [`opennml_net::HOST_LOST`],
/// computes with the bundles it has up to the execution tick of the first missing bundle, records exactly one network
/// savegame `netz_YYYY_MM_DD_hhmm` (own local time) there and then stands forever. The state equals the state of the
/// host at the same tick.
// QUIRK E-258: deviation
#[test]
fn host_crash_lets_clients_save_and_leave() {
    let mut t = Table::running();
    play_for(&mut t, 1_500);
    for link in &t.client_links {
        link.lock().unwrap_or_else(|e| e.into_inner()).cut();
    }
    let host_seq = t.machines[0].seq.clone();
    let cut_at: Vec<u64> = t.machines[1..].iter().map(|m| m.sim.state.steps).collect();
    // The host no longer computes (crashed): only the clients.
    let host = t.machines.remove(0);
    t.corrupter = usize::MAX;
    t.frames_until(|t| t.machines.iter().all(|m| !m.finish.is_empty()));
    let name = CLOCK.net_end_name();
    for (i, m) in t.machines.iter().enumerate() {
        let i = i + 1;
        assert_eq!(
            m.failed.as_deref(),
            Some(opennml_net::HOST_LOST),
            "machine {i}"
        );
        assert_eq!(
            m.finish,
            vec![NetFinish::HostLost { name: name.clone() }],
            "machine {i}"
        );
        assert_eq!(m.saves.len(), 1, "machine {i}: one network savegame");
        let save = &m.saves[0];
        assert_eq!(save.name, name);
        let at = save.sim.state.steps;
        assert_eq!(
            at, m.sim.state.steps,
            "machine {i}: state at the waiting point"
        );
        let status = m.driver.status();
        assert_eq!(
            at, status.next_exec_step,
            "machine {i}: before the first missing bundle"
        );
        assert_eq!(status.waiting, Some(WaitReason::Ended), "machine {i}");
        assert!(m.driver.halted(), "machine {i}");
        assert!(at >= cut_at[i - 1], "machine {i}: no tick back");
        let host_hash = host_seq.iter().find(|(s, _)| *s == at).map(|(_, h)| *h);
        assert_eq!(
            Some(checksum(&save.sim.state)),
            host_hash,
            "machine {i}: same as the host at tick {at}"
        );
    }
    // At least one client still had a bundle ahead of it at the abort and computed it at once (without game time).
    assert!(
        t.machines
            .iter()
            .zip(&cut_at)
            .any(|(m, &cut)| m.saves[0].sim.state.steps > cut),
        "states {:?}, cut at {cut_at:?}",
        t.machines
            .iter()
            .map(|m| m.saves[0].sim.state.steps)
            .collect::<Vec<_>>()
    );
    for _ in 0..40 {
        t.frame(|_, _, _| {});
    }
    for (i, m) in t.machines.iter().enumerate() {
        assert_eq!(m.saves.len(), 1, "machine {}: no second state", i + 1);
        assert_eq!(m.finish.len(), 1);
    }
    drop(host);
}

/// If the core reports `HostGone` and the only candidate without transport cannot be dialed, the driver gives up at
/// once, and the `Failed` from `abandon` applies in the same pass as today.
#[test]
fn host_gone_without_dialable_candidate_aborts_in_the_same_pump() {
    let mut t = Table::loading();
    let now = t.now;
    let m = &mut t.machines[1];
    m.driver.loaded(None);
    // `Go` with a candidate (client 2, reachable) directly to the core; the table has no transport.
    let Core::Client(core) = &mut m.driver.core else {
        panic!("client expected");
    };
    core.handle(
        now,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Go {
            successors: vec![opennml_net::Successor {
                peer: 2,
                slot: 2,
                addr: Some(SocketAddr::from(([10, 0, 0, 3], 4000))),
            }],
        }),
    );
    m.driver.pump(now);
    assert!(m.driver.in_play);
    assert_eq!(m.phase(), Phase::Running);
    t.client_links[0]
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cut();
    let m = &mut t.machines[1];
    m.driver.pump(now);
    assert_eq!(m.phase(), Phase::Ended);
    assert_eq!(m.driver.failure.as_deref(), Some(HOST_LOST));
    assert!(m.driver.abort_due, "abort savegame in the next advance");
}

/// After the end of the match (here `HostLost`) the driver no longer attempts an adoption: `finish` clears the pending
/// sync, and `try_adopt` aborts after the end (otherwise it would write a misleading error line and report a failure).
#[test]
fn no_adopt_after_the_end() {
    let mut t = Table::running();
    play_for(&mut t, 500);
    let m = &mut t.machines[1];
    m.driver.adopt_due = Some(3);
    m.driver.resync_bytes = Some(vec![1, 2, 3]);
    m.driver.finish(NetFinish::HostLost {
        name: CLOCK.net_end_name(),
    });
    assert_eq!(m.driver.adopt_due, None);
    assert_eq!(m.driver.resync_bytes, None);
    // Also a state arriving afterwards is not adopted.
    m.driver.adopt_due = Some(3);
    m.driver.resync_bytes = Some(vec![1, 2, 3]);
    m.driver.try_adopt(&mut m.sim);
    assert!(!m.driver.gave_up, "no failure after the end");
    assert!(m.driver.frame.failed.is_none());
}

/// A client leaves the match ([`NetDriver::leave`]): `Bye` to the host and connection closed, without failure and
/// without network savegame. The host reports the dropout once, the slot loses at host and client 2 at the same
/// execution tick, and nobody waits for it any more.
#[test]
fn client_leave_says_bye_and_loses_the_slot() {
    let mut t = Table::running();
    play_for(&mut t, 1_000);
    t.machines[1].driver.leave("The player has left the match.");
    let left_at = t.machines[1].sim.state.steps;
    assert!(t.machines[1].driver.halted());
    assert_eq!(t.machines[1].phase(), Phase::Ended);
    t.frames_until(|t| !t.machines[0].lost.is_empty());
    assert_eq!(
        t.machines[0].lost,
        vec![(1, "The player has left the match.".to_string())]
    );
    // `net_status.links` lists only open connections, not the closed one of the dropped player (none at the one who
    // left).
    let open: Vec<Peer> = t.machines[0]
        .driver
        .status()
        .links
        .iter()
        .map(|l| l.peer)
        .collect();
    assert_eq!(open, [2], "host: only client 2 left");
    assert!(t.machines[1].driver.status().links.is_empty());
    t.frames_until(|t| t.machines[0].sim.state.steps > left_at + 100);
    let m = &t.machines[1];
    assert_eq!(m.failed, None);
    assert!(m.finish.is_empty() && m.saves.is_empty());
    assert_eq!(
        m.sim.state.steps, left_at,
        "the one who left no longer computes"
    );
    for i in [0, 2] {
        assert_eq!(t.machines[i].sim.state.players[1].result, 1, "machine {i}");
    }
    assert!(t.machines[2].sim.state.steps > left_at + 60);
}

/// The messages about the end of the network match with the name of the savegame, or without it if it could not be
/// written.
// QUIRK E-296: kept
#[test]
fn finish_messages_name_the_save() {
    let name = "netz_2026_11_06_1415".to_string();
    assert_eq!(
        NetFinish::HostEnded { name: name.clone() }.message(true),
        "The host has ended the match. It was saved as \"netz_2026_11_06_1415\"."
    );
    assert_eq!(
        NetFinish::HostLost { name: name.clone() }.message(true),
        "The connection to the host was aborted. The match was saved as \"netz_2026_11_06_1415\"."
    );
    assert_eq!(
        NetFinish::Stopped {
            name: "checksumerror_2026_11_06_1415".into()
        }
        .message(true),
        "The match is no longer in sync (checksum error). It was stopped and saved as \
         \"checksumerror_2026_11_06_1415\"."
    );
    assert_eq!(
        NetFinish::HostLost { name }.message(false),
        "The connection to the host was aborted. The match could not be saved."
    );
    assert_eq!(
        NetFinish::Failed("The resync with the host failed.".into()).message(false),
        "The resync with the host failed."
    );
}

/// The network messages appear in the `MSGBOX` through the text path of the original, which takes only the lower byte
/// of each UTF-16 unit as atlas code (E-296, O-61). Every message may therefore contain only characters up to U+00FF
/// (umlauts yes, typographic quotation marks no); foreign text in [`NetFinish::Failed`] (e.g. a load error with
/// typographic quotation marks) is converted for this.
// QUIRK E-296: kept
#[test]
fn net_messages_stay_within_latin1() {
    let name = "netz_2026_11_06_1415".to_string();
    let mut texts = Vec::new();
    for saved in [true, false] {
        for finish in [
            NetFinish::Stopped {
                name: "checksumerror_2026_11_06_1415".into(),
            },
            NetFinish::HostEnded { name: name.clone() },
            NetFinish::HostLost { name: name.clone() },
            NetFinish::Failed(HOST_LOST.into()),
            NetFinish::Failed("Savegame „x“ – damaged (café) … ≠ ‚y‘".into()),
            NetFinish::HandedOver,
        ] {
            texts.push(finish.message(saved));
        }
    }
    texts.extend([HOST_LOST, LEAVE_REASON].map(String::from));
    for text in &texts {
        let wide: Vec<char> = text.chars().filter(|&c| u32::from(c) > 0xFF).collect();
        assert!(
            wide.is_empty(),
            "characters above U+00FF {wide:?} in \"{text}\""
        );
    }
    assert_eq!(
        NetFinish::Failed("Savegame „x“ – damaged (café) … ≠ ‚y‘".into()).message(false),
        "Savegame \"x\" - damaged (café) ... != 'y'"
    );
}

/// The save point at the abort does not depend on the race between halt and `Resume`. The host pauses and lifts it
/// again at once. Client 2 (far, fast) stands after `e(k)` before its `Resume` arrives; then its line breaks off.
/// Client 1 (near, slow) gets the `Resume` before its `e(k)` and keeps running; its line breaks off afterwards, before
/// another bundle arrives. Both have the same last bundle `k` and write their state at the same tick before `e(k + 1)`
/// with the same checksum as the host there.
#[test]
fn abort_saves_at_the_same_step_despite_the_pause_race() {
    let mut t = Table::running();
    t.machines[1].dt_ms = 0.2 * FRAME_MS as f64;
    t.machines[2].dt_ms = 4.0 * FRAME_MS as f64;
    t.frames_until(|t| t.machines.iter().all(|m| m.sim.state.steps >= 10));
    assert!(t.machines[0].driver.toggle_pause(), "pause requested");
    t.frames_until(|t| t.machines[0].driver.pause_shown());
    let stand = t.machines[0].sim.state.steps;
    assert!(t.machines[0].driver.toggle_pause(), "pause lifted");
    t.frames_until(|t| t.machines[2].driver.pause_shown());
    assert_eq!(
        t.machines[2].sim.state.steps, stand,
        "client 2 stands after e(k)"
    );
    t.client_links[1]
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cut();
    t.frames_until(|t| t.machines[1].sim.state.steps > stand);
    assert!(
        !t.machines[1].driver.halted(),
        "client 1 kept running without a halt"
    );
    t.client_links[0]
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cut();
    // The host computes on alone (both dropped out), so also beyond the tick of the states.
    t.frames_until(|t| t.machines[1..].iter().all(|m| !m.finish.is_empty()));
    let cfg = TurnConfig::default();
    let k = cfg.exec_at(stand - 1).expect("halt after e(k)");
    let at = cfg.exec_step(k + 1);
    t.frames_until(|t| t.machines[0].sim.state.steps > at);
    let host_seq = &t.machines[0].seq;
    for (i, m) in t.machines.iter().enumerate().skip(1) {
        assert_eq!(
            m.driver.status().round,
            k + 1,
            "machine {i}: last bundle {k}"
        );
        assert_eq!(m.saves.len(), 1, "machine {i}");
        let save = &m.saves[0];
        assert_eq!(save.sim.state.steps, at, "machine {i}: before e(k + 1)");
        let host_hash = host_seq.iter().find(|(s, _)| *s == at).map(|(_, h)| *h);
        assert_eq!(Some(checksum(&save.sim.state)), host_hash, "machine {i}");
    }
    assert_eq!(
        t.machines[1].saves[0].sim.to_bytes(),
        t.machines[2].saves[0].sim.to_bytes(),
        "same state"
    );
}

/// An accepted connection that sends no `Hello` within [`HELLO_TIMEOUT_MS`] is closed by the host itself; one with
/// `Hello` stays.
#[test]
fn connection_without_hello_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let mut host = NetDriver::host(
        crate::testutil::net_host_core(hello("Host"), TurnConfig::default(), 3),
        paths(dir.path(), 0),
    );
    let (silent_host, mut silent) = net.pair(10);
    let (talking_host, mut talking) = net.pair(10);
    host.add_link(1, Box::new(silent_host));
    host.add_link(2, Box::new(talking_host));
    talking.send(NetMessage::Hello(hello("Guest")));
    let mut now = 0;
    let frame = |host: &mut NetDriver, now: &mut u64| {
        *now += FRAME_MS;
        net.advance_to(*now);
        host.pump(*now);
    };
    while now < HELLO_TIMEOUT_MS - 500 {
        frame(&mut host, &mut now);
    }
    assert!(
        !silent
            .poll()
            .iter()
            .any(|e| matches!(e, LinkEvent::Closed(_)))
    );
    assert_eq!(
        host.status().links.len(),
        2,
        "both there before the deadline"
    );
    while now < HELLO_TIMEOUT_MS + 500 {
        frame(&mut host, &mut now);
    }
    let events = silent.poll();
    assert!(
        events.iter().any(|e| matches!(e, LinkEvent::Closed(_))),
        "{events:?}"
    );
    let events = talking.poll();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LinkEvent::Message(NetMessage::Welcome { slot: 2 }))),
        "{events:?}"
    );
    assert!(!events.iter().any(|e| matches!(e, LinkEvent::Closed(_))));
    let links: Vec<Peer> = host.status().links.iter().map(|l| l.peer).collect();
    assert_eq!(links, vec![2], "the silent connection is gone");
    assert_eq!(host.failure(), None);
    assert!(!host.started());
}

/// `failure` keeps the reason of the failure, also after the frame is collected; `drop_links` cuts the connections
/// without `Bye` (debug aid `net_debug drop`).
#[test]
fn failure_is_kept_and_drop_cuts_without_bye() {
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let (to_host, mut host_side) = net.pair(10);
    let mut client = NetDriver::client(
        ClientCore::new(hello("Guest")),
        Box::new(to_host),
        paths(dir.path(), 1),
    );
    let mut now = 0;
    for _ in 0..4 {
        now += FRAME_MS;
        net.advance_to(now);
        client.pump(now);
    }
    let events = host_side.poll();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LinkEvent::Message(NetMessage::Hello(h)) if h.name == "Guest")),
        "{events:?}"
    );
    client.drop_links();
    for _ in 0..4 {
        now += FRAME_MS;
        net.advance_to(now);
        client.pump(now);
    }
    let events = host_side.poll();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LinkEvent::Message(NetMessage::Bye { .. }))),
        "no Bye: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, LinkEvent::Closed(_))),
        "{events:?}"
    );

    // Rejected: the reason stays available.
    let (to_host, mut host_side) = net.pair(10);
    let mut client = NetDriver::client(
        ClientCore::new(hello("Guest")),
        Box::new(to_host),
        paths(dir.path(), 2),
    );
    host_side.send(NetMessage::Reject {
        reason: "The match is full".into(),
    });
    for _ in 0..4 {
        now += FRAME_MS;
        net.advance_to(now);
        client.pump(now);
    }
    let text = client.failure().map(str::to_string).unwrap_or_default();
    assert!(text.contains("The match is full"), "{text}");
    assert_eq!(client.status().phase, Phase::Ended);
    assert!(!client.started());
}

/// The game host fetches the events of the lobby before the start with `take_lobby_events` (state, chat); a kick
/// arrives at the client as `Kicked`, sets `failure` to the reason and closes the connection.
#[test]
fn lobby_events_are_taken_and_a_kick_closes() {
    use opennml_net::{CoreEvent, HostEdit, KICKED, LobbyWish};
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let mut host = NetDriver::host(
        crate::testutil::net_host_core(hello("Host"), TurnConfig::default(), 3),
        paths(dir.path(), 0),
    );
    let (h, c) = net.pair(10);
    host.add_link(1, Box::new(h));
    let mut client = NetDriver::client(
        ClientCore::new(hello("Guest")),
        Box::new(c),
        paths(dir.path(), 1),
    );
    let mut now = 0;
    let mut frames = |host: &mut NetDriver, client: &mut NetDriver, n: usize| {
        for _ in 0..n {
            now += FRAME_MS;
            net.advance_to(now);
            host.pump(now);
            client.pump(now);
        }
    };
    frames(&mut host, &mut client, 20);
    assert!(host.take_lobby_events().contains(&CoreEvent::LobbyChanged));
    assert!(
        client
            .take_lobby_events()
            .contains(&CoreEvent::LobbyChanged)
    );
    assert_eq!(client.client_core().and_then(ClientCore::own_row), Some(1));

    client
        .client_core_mut()
        .unwrap()
        .lobby_wish(LobbyWish::Chat("Hello".into()));
    frames(&mut host, &mut client, 10);
    let chat = CoreEvent::Chat {
        row: 1,
        name: "Guest".into(),
        text: "Hello".into(),
    };
    assert!(host.take_lobby_events().contains(&chat));
    assert!(client.take_lobby_events().contains(&chat));

    host.host_core_mut()
        .unwrap()
        .host_edit(HostEdit::Kind { row: 1, kind: 0 })
        .unwrap();
    frames(&mut host, &mut client, 10);
    assert_eq!(client.take_lobby_events(), vec![CoreEvent::Kicked]);
    assert_eq!(client.failure(), Some(KICKED));
    assert!(client.status().links.is_empty(), "connection closed");
    assert_eq!(client.status().phase, Phase::Ended);
    assert_eq!(host.host_core().unwrap().peers().len(), 1);
    assert!(host.take_lobby_events().contains(&CoreEvent::LobbyChanged));
}

/// Listener of the client in [`probe_pair`].
#[derive(Clone, Copy, PartialEq)]
enum Hear {
    /// Listens and reports the port.
    Yes,
    /// Reports no port.
    No,
    /// Reports a port on which nobody listens (listener closed again at once).
    Dead,
}

/// Host and client on two `MemTransport` machines: the client listens on a free port and reports it in the `Hello`,
/// the host has a second transport for the probe.
fn probe_pair(hear: Hear) -> (MemNet, NetDriver, NetDriver, Option<SocketAddr>) {
    use opennml_net::{Listener, MemTransport, Transport};
    let net = MemNet::new();
    let mut host_t = MemTransport::new(&net, 0);
    let mut client_t = MemTransport::new(&net, 0);
    let host_listener = host_t.listen(2302).unwrap();
    let host_addr = SocketAddr::new(host_t.ip(), 2302);
    let mut host = NetDriver::host(
        crate::testutil::net_host_core(hello("Host"), TurnConfig::default(), 3),
        paths(tempfile::tempdir().unwrap().keep().as_path(), 0),
    );
    host.set_transport(host_t.fork());
    let mut hello = hello("Guest");
    let mut client_listener: Option<Box<dyn Listener>> = None;
    if hear != Hear::No {
        let l = client_t.listen(0).unwrap();
        hello.listen_port = Some(l.port());
        if hear == Hear::Yes {
            client_listener = Some(l);
        }
    }
    let mut connecting = client_t.connect(host_addr, 1_000);
    let mut host_listener = host_listener;
    let mut client = None;
    for t in 0..200u64 {
        net.advance_to(t * 5);
        if let Some(Ok(link)) = connecting.poll() {
            client = Some(NetDriver::client(
                ClientCore::new(hello.clone()),
                link,
                paths(tempfile::tempdir().unwrap().keep().as_path(), 1),
            ));
            break;
        }
    }
    let mut client = client.expect("connection");
    client.set_transport(client_t.fork());
    if let Some(l) = client_listener {
        client.set_listener(l);
    }
    for t in 0..200u64 {
        net.advance_to(1_000 + t * 5);
        for link in host_listener.accept_pending() {
            host.add_link(1, link);
        }
        if !host.links.is_empty() {
            break;
        }
    }
    let addr = hello.listen_port.map(|p| SocketAddr::new(client_t.ip(), p));
    (net, host, client, addr)
}

/// Pumps both drivers for `frames` frames of 25 ms from `from_ms`; returns the time afterwards.
fn pump_pair(
    net: &MemNet,
    host: &mut NetDriver,
    client: &mut NetDriver,
    from_ms: u64,
    frames: u64,
) -> u64 {
    let mut now = from_ms;
    for _ in 0..frames {
        now += 25;
        net.advance_to(now);
        host.pump(now);
        client.pump(now);
    }
    now
}

/// The host probes the listener port of the client and closes at once; the client has accepted the connection, and
/// after the `Closed` no entry remains in its waiting list (and no probe at the host).
#[test]
fn probe_connection_is_closed_by_the_client() {
    let (net, mut host, mut client, _addr) = probe_pair(Hear::Yes);
    pump_pair(&net, &mut host, &mut client, 2_000, 40);
    assert_eq!(client.accepted_total, 1, "probe accepted");
    assert!(
        client.accepted.is_empty(),
        "waiting list empty after closing"
    );
    assert!(host.probes.is_empty(), "probe ended at the host");
    assert!(client.failure().is_none() && host.failure().is_none());
}

/// Without a listener (`listen_port: None`) there is no probe: nothing is accepted, nothing stays open.
#[test]
fn client_without_listen_port_gets_no_probe() {
    let (net, mut host, mut client, _addr) = probe_pair(Hear::No);
    pump_pair(&net, &mut host, &mut client, 2_000, 40);
    assert_eq!(client.accepted_total, 0);
    assert!(host.probes.is_empty());
}

/// An accepted connection that stays silent is closed by the driver after `REJOIN_WINDOW_MS`; one with a first
/// message other than `Rejoin` at once.
#[test]
fn accepted_connection_ends_on_first_message_or_window() {
    use opennml_net::{MemTransport, Transport};
    let (net, mut host, mut client, addr) = probe_pair(Hear::Yes);
    let addr = addr.unwrap();
    let mut other = MemTransport::new(&net, 0);
    let mut c1 = other.connect(addr, 1_000);
    let mut c2 = other.connect(addr, 1_000);
    let mut now = pump_pair(&net, &mut host, &mut client, 2_000, 20);
    let mut l1 = c1.poll().and_then(Result::ok).expect("first");
    let mut l2 = c2.poll().and_then(Result::ok).expect("second");
    now = pump_pair(&net, &mut host, &mut client, now, 4);
    assert_eq!(client.accepted.len(), 2);
    l2.send(NetMessage::Chat {
        row: 0,
        text: "x".into(),
    });
    now = pump_pair(&net, &mut host, &mut client, now, 8);
    assert_eq!(client.accepted.len(), 1, "first message closes");
    let events = l2.poll();
    assert!(
        events.iter().any(|e| matches!(e, LinkEvent::Closed(_))),
        "{events:?}"
    );
    pump_pair(&net, &mut host, &mut client, now, REJOIN_WINDOW_MS / 25 + 4);
    assert!(client.accepted.is_empty(), "deadline expired");
    let events = l1.poll();
    assert!(events.iter().any(|e| matches!(e, LinkEvent::Closed(_))));
}

/// If the client reports a port on which nobody listens, the probe stays open until the deadline
/// `SUCCESSOR_CONNECT_MS` (the host waits, `Go` too) and then ends without an address; nothing aborts and no client
/// accepts anything.
#[test]
fn probe_of_a_dead_port_ends_after_the_connect_deadline() {
    let (net, mut host, mut client, _addr) = probe_pair(Hear::Dead);
    let t0 = 2_000;
    let now = pump_pair(&net, &mut host, &mut client, t0, 40);
    assert_eq!(host.probes.len(), 1, "still open after 1 s");
    let now = pump_pair(&net, &mut host, &mut client, now, 40);
    assert_eq!(host.probes.len(), 1, "still open after 2 s");
    let now = pump_pair(&net, &mut host, &mut client, now, 40 + 8);
    assert!(
        now - t0 > SUCCESSOR_CONNECT_MS,
        "played time {} ms",
        now - t0
    );
    assert!(host.probes.is_empty(), "ended after the deadline");
    assert_eq!(client.accepted_total, 0);
    assert!(client.failure().is_none() && host.failure().is_none());
}
