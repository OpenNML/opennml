//! Tests of the role change in the turn driver: three machines, each with its own [`MemTransport`] of the same
//! [`MemNet`] ([`Lan`]). The clients listen on a free port, the host probes them before `Go`, and every connection
//! comes about through `connect` as in the program. A crash is [`MemNet::cut_host`] plus the end of the machine (it no
//! longer computes, its listener is closed).
//!
//! Every test compares the checksum sequences of the others with the replay of their command log without a network
//! ([`assert_replays`], pattern `net_match_equals_its_replay`): the migration touches the state only through bundles.

use std::net::IpAddr;

use super::*;
use nml_net::{
    HEARTBEAT_MS, HOST_LOST, Listener, MemTransport, REJOIN_WINDOW_MS, SUCCESSOR_CONNECT_MS,
    TIMEOUT_MS, Transport,
};
use nml_sim::game_end::DEFEAT_MESSAGE;

/// Game time after `Go` after which the tests let the host drop out.
const PLAY_MS: u64 = 1_500;
/// Port on which the host listens.
const HOST_PORT: u16 = 2302;
/// Delay per machine: the three from [`DELAYS`], plus a fourth machine (only [`Lan::running_n`]).
const LAN_DELAYS: [u64; 4] = [DELAYS[0], DELAYS[1], DELAYS[2], 80];

/// Three (or, with [`Lan::running_n`], four) machines over their own transports.
struct Lan {
    net: MemNet,
    machines: Vec<Machine>,
    /// Address per machine.
    ips: Vec<IpAddr>,
    /// The machine is still running (a crashed one no longer computes).
    alive: Vec<bool>,
    now: u64,
    go_at: u64,
    _dir: tempfile::TempDir,
    /// Listener of the host for the sign-on (stays open as in the program).
    _host_listener: Box<dyn Listener>,
    /// This machine corrupts its state at the ticks from [`Self::corrupt_at`] (like [`Table`]).
    corrupter: usize,
    corrupt_at: Vec<u64>,
    corrupted: Vec<u64>,
}

impl Lan {
    /// Sign-on, probe, start, loading and `Go`. `listen[i]`: client `i` listens (index 0 = host, no effect).
    fn running(listen: [bool; 3]) -> Self {
        Self::running_n(&listen)
    }

    /// Like [`Self::running`] with `listen.len()` machines (at most [`LAN_DELAYS`]); the match has as many slots
    /// ([`testworld::net_rules_with`]).
    fn running_n(listen: &[bool]) -> Self {
        let n = u8::try_from(listen.len()).expect("machine count");
        let cfg = TurnConfig::default();
        let net = MemNet::new();
        let dir = tempfile::tempdir().expect("folder");
        let mut host_t = MemTransport::new(&net, LAN_DELAYS[0]);
        let mut host_listener = host_t.listen(HOST_PORT).expect("listener of the host");
        let host_addr = SocketAddr::new(host_t.ip(), HOST_PORT);
        let mut host = NetDriver::host(
            crate::testutil::net_host_core(hello("Host"), cfg, 8),
            paths(dir.path(), 0),
        );
        host.set_transport(host_t.fork());
        let mut ips = vec![host_t.ip()];
        let mut drivers = vec![host];
        let mut now = 0;
        for slot in 1..n {
            let i = usize::from(slot);
            let mut t = MemTransport::new(&net, LAN_DELAYS[i]);
            ips.push(t.ip());
            let mut hello = hello(&format!("Client {slot}"));
            let listener = listen[i].then(|| t.listen(0).expect("listener of the client"));
            hello.listen_port = listener.as_ref().map(|l| l.port());
            let mut connecting = t.connect(host_addr, 1_000);
            let link = loop {
                now += FRAME_MS;
                net.advance_to(now);
                if let Some(result) = connecting.poll() {
                    break result.expect("connection to the host");
                }
            };
            let mut client = NetDriver::client(ClientCore::new(hello), link, paths(dir.path(), i));
            client.set_transport(t.fork());
            if let Some(l) = listener {
                client.set_listener(l);
            }
            let accepted = loop {
                let mut links = host_listener.accept_pending();
                if let Some(link) = links.pop() {
                    break link;
                }
                now += FRAME_MS;
                net.advance_to(now);
            };
            drivers[0].add_link(slot, accepted);
            drivers.push(client);
        }
        let machines: Vec<Machine> = drivers
            .into_iter()
            .enumerate()
            .map(|(i, mut d)| {
                d.set_fixed_clock(Some(CLOCK));
                Machine::with_slots(d, i as u8, n)
            })
            .collect();
        let mut lan = Self {
            net,
            machines,
            ips,
            alive: vec![true; listen.len()],
            now,
            go_at: 0,
            _dir: dir,
            _host_listener: host_listener,
            corrupter: usize::MAX,
            corrupt_at: Vec::new(),
            corrupted: Vec::new(),
        };
        lan.frames_until(|l| {
            l.machines[1..]
                .iter()
                .all(|m| m.driver.client_core().is_some_and(|c| c.peer().is_some()))
        });
        let package = {
            let sim = &lan.machines[0].sim;
            start_package(&sim.rules, &sim.setup)
        };
        let info = |peer: u8| StartInfo {
            package: package.clone(),
            seats: std::array::from_fn(|slot| Seat {
                peer: (slot < usize::from(n)).then_some(slot as u8),
                name: format!("P{slot}"),
            }),
            your_slot: peer,
            spacing: cfg.raw().0,
            lead: cfg.raw().1,
            game_speed: DEFAULT_GAME_SPEED,
            session: "Testpartie".into(),
            saved: None,
        };
        lan.machines[0]
            .driver
            .host_core_mut()
            .expect("Host")
            .start(info);
        lan.frames_until(|l| l.machines.iter().all(|m| m.phase() == Phase::Loading));
        for m in &mut lan.machines {
            m.driver.loaded(None);
        }
        lan.frames_until(|l| l.machines.iter().all(|m| m.phase() == Phase::Running));
        lan.go_at = lan.now;
        lan
    }

    /// One frame for every running machine: time advances, inputs of the scene, corrupting.
    fn frame(&mut self, scene: Scene) {
        self.now += FRAME_MS;
        self.net.advance_to(self.now);
        let t = self.now - self.go_at;
        for (i, m) in self.machines.iter_mut().enumerate() {
            if !self.alive[i] {
                continue;
            }
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
            self.frame(scene);
        }
        let steps: Vec<u64> = self.machines.iter().map(|m| m.sim.state.steps).collect();
        panic!("condition not reached after {MAX_FRAMES} frames, ticks {steps:?}");
    }

    /// Plays the scene until `ms` after `Go`.
    fn play_for(&mut self, ms: u64) {
        let until = self.go_at + ms;
        while self.now < until {
            self.frame(scene);
        }
    }

    /// Machine `i` crashes: all its lines break off, it no longer computes, its listener is closed (whoever dials it is
    /// rejected and waits out the deadline).
    fn crash(&mut self, i: usize) {
        self.net.cut_host(self.ips[i]);
        self.alive[i] = false;
        self.machines[i].driver.listener = None;
    }

    /// Until every machine in `who` has reached tick `steps`.
    fn run_to(&mut self, who: &[usize], steps: u64) {
        let who = who.to_vec();
        self.frames_until(move |l| who.iter().all(|&i| l.machines[i].sim.state.steps >= steps));
    }
}

/// Highest bundle that the core of the driver holds.
fn top_bundle(d: &NetDriver) -> Option<u32> {
    let core = d.lockstep_ref();
    (0..=d.next_close + 4)
        .rev()
        .find(|&k| core.bundle(k).is_some())
}

/// The sequence of the machine from tick `from` on is the replay of its command log without a network (pattern
/// `net_match_equals_its_replay`).
fn assert_replays(m: &Machine, from: u64, what: &str) {
    let replay = m.sim.replay().expect("replay");
    let rules = &m.sim.rules;
    let mut state = new_match(rules, &replay.start.setup).expect("start package");
    let end = m.seq.last().expect("sequence").0;
    let mut seq = vec![(0, checksum(&state))];
    let mut next = 0;
    while state.steps < end {
        let now = state.steps;
        let mut commands = Vec::new();
        while next < replay.commands.len() && replay.commands[next].step == now {
            commands.push(replay.commands[next].command.clone());
            next += 1;
        }
        step(&mut state, rules, &commands);
        seq.push((state.steps, checksum(&state)));
    }
    let from_on = |s: &[(u64, u64)]| -> Vec<(u64, u64)> {
        s.iter().copied().filter(|&(st, _)| st >= from).collect()
    };
    assert_full_from(&m.seq, from, what);
    assert_eq!(
        from_on(&m.seq),
        from_on(&seq),
        "{what}: equal to the replay from tick {from}"
    );
}

/// The sequence has no gaps from tick 0.
fn assert_full_from(seq: &[(u64, u64)], from: u64, what: &str) {
    for (i, &(s, _)) in seq.iter().enumerate() {
        assert_eq!(s, i as u64, "{what}: sequence without gaps");
    }
    assert!(seq.last().is_some_and(|&(s, _)| s >= from), "{what}");
}

/// The common beginning of two sequences is equal.
fn assert_same_prefix(a: &Machine, b: &Machine, what: &str) {
    let n = a.seq.len().min(b.seq.len());
    assert!(n > 0, "{what}");
    assert_eq!(&a.seq[..n], &b.seq[..n], "{what}: same sequence");
}

/// How often the message about the defeat of slot `slot` (17200 + p via message 200 + p) reached the machine.
fn defeat_shown(m: &Machine, slot: u8) -> usize {
    m.messages
        .iter()
        .filter(|&&(_, id)| id == DEFEAT_MESSAGE + i32::from(slot))
        .count()
}

/// Peers of the slot assignment of the machine.
fn seat_peers(m: &Machine) -> Vec<Option<u8>> {
    m.driver.start_info().expect("Start").seats[..3]
        .iter()
        .map(|s| s.peer)
        .collect()
}

/// The host crashes. Client 1 (first candidate) becomes host, client 2 comes back, the slot of the old host loses once,
/// and both then compute 400 ticks of the same sequence as without a network.
// QUIRK E-268, E-347: deviation
#[test]
fn host_cut_migrates_and_keeps_hashes_equal() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[1].sim.state.steps;
    assert!(!lan.machines[1].sim.net_host);
    lan.crash(0);
    // State of the migration as `net_status` reports it.
    let mut seen: Vec<(usize, MigrationStatus)> = Vec::new();
    for _ in 0..MAX_FRAMES {
        lan.frame(scene);
        let states: Vec<Option<MigrationStatus>> = [1, 2]
            .iter()
            .map(|&i| lan.machines[i].driver.status().migration)
            .collect();
        for (i, s) in [1, 2].into_iter().zip(&states) {
            if let Some(s) = s
                && !seen.contains(&(i, s.clone()))
            {
                seen.push((i, s.clone()));
            }
        }
        if !seen.is_empty() && states.iter().all(Option::is_none) {
            break;
        }
    }
    for expected in [
        (
            1,
            MigrationStatus::Gathering {
                expected: vec![2],
                arrived: Vec::new(),
            },
        ),
        (2, MigrationStatus::Dialing { peer: 1 }),
        (2, MigrationStatus::Rejoining { peer: 1 }),
    ] {
        assert!(seen.contains(&expected), "{expected:?} not seen: {seen:?}");
    }
    lan.run_to(&[1, 2], cut + STEPS);
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    assert!(m1.driver.is_host(), "client 1 is the new host");
    assert!(!m2.driver.is_host());
    for (i, m) in [(1, m1), (2, m2)] {
        let what = format!("machine {i}");
        assert!(m.failed.is_none() && m.finish.is_empty(), "{what}");
        assert_eq!(m.sim.state.players[0].result, 1, "{what}: old host loses");
        assert_eq!(m.sim.state.players[1].result, 0, "{what}");
        assert_eq!(m.sim.state.players[2].result, 0, "{what}");
        assert_eq!(defeat_shown(m, 0), 1, "{what}: message 200 once");
        assert_eq!(seat_peers(m), vec![None, Some(0), Some(2)], "{what}");
        assert_eq!(m.driver.status().migration, None, "{what}");
        assert_eq!(m.phase(), Phase::Running, "{what}");
        assert_replays(m, 0, &what);
    }
    assert_same_prefix(m1, m2, "client 1 and 2");
    assert_same_prefix(&lan.machines[0], m1, "until the crash like the old host");
    let lost: Vec<u8> = m1.lost.iter().map(|(s, _)| *s).collect();
    assert_eq!(lost, vec![0], "new host reports the old one once");
    assert!(m2.lost.is_empty());
    assert_eq!(m1.became_host.len(), 1);
    assert!(m1.sim.net_host && !m2.sim.net_host);
    // Successor list for a second migration: only client 2 left.
    let s1 = m1.driver.status().successors;
    let s2 = m2.driver.status().successors;
    assert_eq!(s1.iter().map(|s| s.peer).collect::<Vec<_>>(), vec![2]);
    assert_eq!(s1, s2);
    assert_eq!(m1.driver.status().role, NetRole::Host);
    assert!(m1.driver.listener.is_none(), "new host no longer listens");
    assert!(m2.driver.listener.is_some(), "client keeps listening");
}

/// The host stays silent (`set_muted`) but keeps computing. After 30 s of silence the clients see a dropout and
/// migrate; afterwards they compute the same sequence.
// QUIRK E-268: deviation
#[test]
fn host_silent_migrates_after_the_timeout() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[1].sim.state.steps;
    lan.machines[0].driver.set_muted(true);
    let muted_at = lan.now;
    lan.frames_until(|l| l.machines[1].driver.is_host());
    // The silence counts from the last reception, so up to one keepalive before the muting.
    let after = lan.now - muted_at;
    assert!(
        (TIMEOUT_MS - HEARTBEAT_MS..TIMEOUT_MS + 2_000).contains(&after),
        "change after {after} ms of silence"
    );
    lan.run_to(&[1, 2], cut + STEPS);
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    for (i, m) in [(1, m1), (2, m2)] {
        let what = format!("machine {i}");
        assert!(m.failed.is_none(), "{what}");
        assert_eq!(m.sim.state.players[0].result, 1, "{what}");
        assert_eq!(defeat_shown(m, 0), 1, "{what}");
        assert_replays(m, 0, &what);
    }
    assert_same_prefix(m1, m2, "client 1 and 2");
}

/// The line host-client 1 breaks, client 1 becomes host; the old host distributes the next bundle `K` only to client 2
/// and then crashes. The new host takes `K` from the `Rejoin` of client 2, and both compute the same sequence.
// QUIRK E-348, E-350: deviation
#[test]
fn crash_mid_distribution_keeps_everyone_equal() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[2].sim.state.steps;
    let (h, c1) = (lan.ips[0], lan.ips[1]);
    lan.net.cut_between(h, c1);
    lan.frames_until(|l| top_bundle(&l.machines[2].driver) > top_bundle(&l.machines[1].driver));
    let k = top_bundle(&lan.machines[2].driver).expect("bundle K");
    assert!(
        lan.machines[1].driver.is_host(),
        "client 1 saw the dropout at once"
    );
    lan.crash(0);
    lan.run_to(&[1, 2], cut + STEPS);
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    assert!(m1.driver.next_exec > k && m2.driver.next_exec > k);
    for (i, m) in [(1, m1), (2, m2)] {
        let what = format!("machine {i}");
        assert!(m.failed.is_none(), "{what}");
        assert_eq!(m.sim.state.players[0].result, 1, "{what}");
        assert_replays(m, 0, &what);
    }
    assert_same_prefix(m1, m2, "client 1 and 2");
    assert_eq!(m1.sim.log, m2.sim.log, "same command log");
}

/// Only the line host-client 2 breaks. Client 1 still hears the host and holds the `Rejoin` of client 2 until `W`, then
/// rejects it with `MIGRATION_REFUSED`; client 2 gives up (abort savegame) and does not become host itself. Host and
/// client 1 play on.
// QUIRK E-292: kept
#[test]
fn candidate_still_hearing_the_host_refuses_rejoin() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[0].sim.state.steps;
    let (h, c2) = (lan.ips[0], lan.ips[2]);
    lan.net.cut_between(h, c2);
    let cut_at = lan.now;
    lan.frames_until(|l| !l.machines[2].finish.is_empty());
    let after = lan.now - cut_at;
    assert!(
        after >= REJOIN_WINDOW_MS,
        "not rejected until after holding ({after} ms)"
    );
    lan.run_to(&[0, 1], cut + STEPS);
    let (m0, m1, m2) = (&lan.machines[0], &lan.machines[1], &lan.machines[2]);
    let name = CLOCK.net_end_name();
    assert_eq!(m2.finish, vec![NetFinish::HostLost { name }]);
    assert_eq!(m2.failed.as_deref(), Some(HOST_LOST));
    assert!(
        !m2.driver.is_host() && m2.became_host.is_empty(),
        "no second host"
    );
    assert!(!m1.driver.is_host() && m1.became_host.is_empty());
    assert!(m0.failed.is_none() && m1.failed.is_none());
    assert_eq!(m0.lost.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![2]);
    assert_same_prefix(m0, m1, "host and client 1");
    assert_replays(m0, 0, "host");
    assert_replays(m1, 0, "client 1");
    // The abort savegame of client 2 equals the host at this tick.
    let save = &m2.saves[0];
    let at = save.sim.state.steps;
    let host = m0.seq.iter().find(|(s, _)| *s == at).map(|(_, h)| *h);
    assert_eq!(Some(checksum(&save.sim.state)), host);
}

/// Four machines. Only the line host-client 2 breaks; client 1 still hears the host and holds the `Rejoin` of client
/// 2. About 5 s later the host crashes, and with it client 3. Client 1 becomes host, accepts the held `Rejoin` and
/// gathers until `W` from its switch (client 3 does not come). The deadline of client 2 covers holding and gathering:
/// it waits for the answer of the new host and does not become host itself (no second host). Because it has been 5 s
/// behind since the cut, the new host can no longer supplement it (`MIGRATION_FAILED`); client 2 writes the abort
/// savegame as today, equal to the state of the new host at this tick. The new host computes the sequence of its
/// replay.
// QUIRK E-292: kept
#[test]
fn held_rejoin_then_gathering_makes_no_second_host() {
    let mut lan = Lan::running_n(&[true; 4]);
    lan.play_for(PLAY_MS);
    let (h, c2) = (lan.ips[0], lan.ips[2]);
    lan.net.cut_between(h, c2);
    let cut_at = lan.now;
    lan.frames_until(|l| {
        l.machines[2].driver.status().migration == Some(MigrationStatus::Rejoining { peer: 1 })
    });
    while lan.now < cut_at + 5_000 {
        lan.frame(scene);
    }
    assert!(
        !lan.machines[1].driver.is_host(),
        "client 1 still hears the host and holds the Rejoin"
    );
    let cut = lan.machines[1].sim.state.steps;
    lan.crash(0);
    lan.crash(3);
    let crash_at = lan.now;
    lan.frames_until(|l| !l.machines[2].finish.is_empty());
    let after = lan.now - crash_at;
    assert!(
        after >= REJOIN_WINDOW_MS,
        "no answer until the new host has gathered ({after} ms)"
    );
    lan.run_to(&[1], cut + STEPS);
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    assert!(m1.driver.is_host(), "client 1 is the new host");
    assert!(
        !m2.driver.is_host() && m2.became_host.is_empty(),
        "no second host"
    );
    assert_eq!(
        m2.finish,
        vec![NetFinish::HostLost {
            name: CLOCK.net_end_name()
        }]
    );
    assert_eq!(m2.failed.as_deref(), Some(HOST_LOST));
    assert!(m1.failed.is_none() && m1.finish.is_empty());
    assert_eq!(m1.driver.status().migration, None);
    assert_eq!(m1.sim.state.players[0].result, 1, "old host loses");
    assert_eq!(
        m1.sim.state.players[2].result, 1,
        "client 2 dropped out at the old host"
    );
    assert_eq!(m1.sim.state.players[3].result, 1, "client 3 did not come");
    assert_replays(m1, 0, "new host");
    assert_same_prefix(m1, m2, "client 1 and 2");
    let save = &m2.saves[0];
    let at = save.sim.state.steps;
    let host = m1.seq.iter().find(|(s, _)| *s == at).map(|(_, h)| *h);
    assert_eq!(Some(checksum(&save.sim.state)), host, "abort savegame");
}

/// First the host drops out (client 1 becomes host), then the new host. Client 2 takes the next entry of the list from
/// `Migrated` - itself - and plays on alone; both former hosts lose.
// QUIRK E-269, E-349: deviation
#[test]
fn second_migration_takes_the_next_successor() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    lan.crash(0);
    lan.frames_until(|l| {
        l.machines[1].driver.is_host()
            && l.machines[1].driver.status().migration.is_none()
            && l.machines[2].driver.status().migration.is_none()
    });
    lan.play_for(PLAY_MS + 2 * REJOIN_WINDOW_MS);
    let cut = lan.machines[2].sim.state.steps;
    lan.crash(1);
    lan.run_to(&[2], cut + STEPS);
    let m2 = &lan.machines[2];
    assert!(m2.driver.is_host(), "client 2 is the second new host");
    assert!(m2.failed.is_none());
    assert_eq!(m2.became_host.len(), 1);
    assert_eq!(seat_peers(m2), vec![None, None, Some(0)]);
    assert_eq!(m2.lost.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![1]);
    assert_eq!(m2.sim.state.players[0].result, 1);
    assert_eq!(m2.sim.state.players[1].result, 1);
    assert_eq!(defeat_shown(m2, 0), 1);
    assert_eq!(defeat_shown(m2, 1), 1);
    assert_same_prefix(&lan.machines[1], m2, "until the second crash");
    assert_replays(m2, 0, "client 2");
}

/// (a) Client 1 does not listen (not reachable at the probe): it is not a candidate but is expected, and connects to
/// client 2, who becomes host. (b) The first candidate has crashed with the host: client 2 waits out the deadline of
/// dialing and then becomes host itself.
#[test]
fn unreachable_candidate_is_skipped() {
    let mut lan = Lan::running([true, false, true]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[2].sim.state.steps;
    lan.crash(0);
    lan.run_to(&[1, 2], cut + STEPS);
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    assert!(m2.driver.is_host() && !m1.driver.is_host());
    assert_eq!(seat_peers(m1), vec![None, Some(1), Some(0)]);
    for (i, m) in [(1, m1), (2, m2)] {
        let what = format!("case a, machine {i}");
        assert!(m.failed.is_none(), "{what}");
        assert_eq!(m.sim.state.players[0].result, 1, "{what}");
        assert_replays(m, 0, &what);
    }
    assert_same_prefix(m1, m2, "case a");

    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[2].sim.state.steps;
    lan.crash(0);
    lan.crash(1);
    let crash_at = lan.now;
    lan.frames_until(|l| l.machines[2].driver.is_host());
    let after = lan.now - crash_at;
    assert!(
        after >= SUCCESSOR_CONNECT_MS,
        "deadline waited out ({after} ms)"
    );
    lan.run_to(&[2], cut + STEPS);
    let m2 = &lan.machines[2];
    assert!(m2.failed.is_none());
    assert_eq!(
        m2.lost.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        vec![0, 1],
        "old host and the one who did not come back"
    );
    assert_eq!(m2.sim.state.players[1].result, 1);
    assert_replays(m2, 0, "case b");
}

/// (a) The host pauses and crashes during the pause: the new host lifts it, both play on. (b) The host crashes in the
/// middle of the sync (its state for client 2 is lost): the sync lapses, the new host recognizes the remaining
/// deviation at the next checksum and syncs itself.
#[test]
fn migration_during_pause_and_resync_runs_on() {
    // (a) Pause of the old host.
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    assert!(lan.machines[0].driver.toggle_pause());
    lan.frames_until(|l| l.machines.iter().all(|m| m.driver.pause_shown()));
    let cut = lan.machines[1].sim.state.steps;
    lan.crash(0);
    lan.run_to(&[1, 2], cut + STEPS);
    for i in [1, 2] {
        let m = &lan.machines[i];
        let what = format!("pause, machine {i}");
        assert!(m.failed.is_none(), "{what}");
        assert!(!m.driver.pause_shown(), "{what}");
        assert_replays(m, 0, &what);
    }
    assert_same_prefix(&lan.machines[1], &lan.machines[2], "pause");

    // (b) Sync.
    let mut lan = Lan::running([true; 3]);
    lan.corrupter = 2;
    lan.corrupt_at = vec![45];
    // Client 1 (closer to the host) has the bundle with `Resync`; the state of the host is not yet at client 2 and is
    // lost with the crash.
    lan.frames_until(|l| {
        l.machines[1].phase() == Phase::Halted(HaltReason::Resync)
            && l.machines[2].driver.resync_bytes.is_none()
            && l.machines[2].adopted.is_empty()
    });
    let cut = lan.machines[1].sim.state.steps;
    lan.crash(0);
    lan.run_to(&[1, 2], cut + STEPS);
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    assert!(m1.driver.is_host());
    assert_eq!(
        m1.driver.status().desyncs,
        Some(2),
        "second sync at the new host"
    );
    assert!(m1.failed.is_none() && m2.failed.is_none());
    assert_eq!(
        m2.adopted.len(),
        1,
        "client 2 adopts once, from the new host"
    );
    assert_replays(m1, 0, "sync, new host");
    let adopted_at = lan.machines[2]
        .seq
        .iter()
        .zip(&m1.seq)
        .rposition(|(a, b)| a != b)
        .map_or(0, |i| i as u64 + 1);
    assert!(adopted_at > cut, "deviation until after the migration");
    assert_replays(m2, adopted_at, "sync, client 2 after the adoption");
    assert_eq!(m2.sim.log, m1.sim.log);
}

/// Client 2 does not listen (no candidate), and the only candidate, client 1, crashes with the host. Client 2 waits out
/// the deadline of dialing and then writes the abort savegame as today (`HostLost`), equal to the state of the host at
/// this tick.
// QUIRK E-268: deviation
#[test]
fn failed_migration_falls_back_to_the_abort_save() {
    let mut lan = Lan::running([true, true, false]);
    lan.play_for(PLAY_MS);
    lan.crash(0);
    lan.crash(1);
    let crash_at = lan.now;
    lan.frames_until(|l| !l.machines[2].finish.is_empty());
    let after = lan.now - crash_at;
    assert!(
        after >= SUCCESSOR_CONNECT_MS,
        "deadline waited out ({after} ms)"
    );
    let m2 = &lan.machines[2];
    assert_eq!(
        m2.finish,
        vec![NetFinish::HostLost {
            name: CLOCK.net_end_name()
        }]
    );
    assert_eq!(m2.failed.as_deref(), Some(HOST_LOST));
    assert!(!m2.driver.is_host());
    assert_eq!(m2.saves.len(), 1);
    let save = &m2.saves[0];
    let at = save.sim.state.steps;
    assert_eq!(
        at,
        m2.driver.status().next_exec_step,
        "at the waiting point"
    );
    let host = lan.machines[0]
        .seq
        .iter()
        .find(|(s, _)| *s == at)
        .map(|(_, h)| *h);
    assert_eq!(Some(checksum(&save.sim.state)), host, "equal to the host");
    assert_replays(m2, 0, "client 2");
    assert!(
        m2.driver.listener.is_none(),
        "finished driver no longer listens"
    );
}

/// Client 2 does not listen; after the accepted handover the only reachable client 1 crashes before the bundle is
/// formed. The bundle carries `End` instead of `Handover`: the host ends with `HostEnded` and network savegame as
/// at the end for all (the driver no longer counts as handing over), without its own defeat; client 2 ends
/// likewise with the same state.
// QUIRK E-268: deviation
#[test]
fn handover_falls_back_to_end_when_the_last_candidate_crashes() {
    let mut lan = Lan::running([true, true, false]);
    lan.play_for(PLAY_MS);
    assert!(lan.machines[0].driver.hand_over(), "handover accepted");
    assert!(lan.machines[0].driver.handing_over);
    lan.crash(1);
    lan.frames_until(|l| !l.machines[0].finish.is_empty() && !l.machines[2].finish.is_empty());
    let name = CLOCK.net_end_name();
    let (m0, m2) = (&lan.machines[0], &lan.machines[2]);
    for (i, m) in [(0, m0), (2, m2)] {
        let what = format!("machine {i}");
        assert_eq!(
            m.finish,
            vec![NetFinish::HostEnded { name: name.clone() }],
            "{what}"
        );
        assert_eq!(m.saves.len(), 1, "{what}: network savegame");
        assert_eq!(
            m.sim.state.players[0].result, 0,
            "{what}: no defeat of the host"
        );
        assert!(
            !m.controls
                .iter()
                .any(|c| matches!(c, NetControl::Handover { .. })),
            "{what}"
        );
        assert_replays(m, 0, &what);
    }
    assert!(!m0.driver.handing_over, "fallback to the end");
    assert!(m0.driver.finished() && !m0.driver.ending());
    assert!(!m2.driver.is_host() && m2.became_host.is_empty());
    assert_eq!(
        checksum(&m0.saves[0].sim.state),
        checksum(&m2.saves[0].sim.state),
        "same state"
    );
}

/// The host hands over. It computes up to the execution tick of the handover bundle and ends with `HandedOver`
/// without a savegame; client 1 becomes host without waiting time, client 2 comes back, and the old host loses exactly
/// once.
#[test]
fn handover_hands_the_match_to_client_1() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    let cut = lan.machines[1].sim.state.steps;
    assert!(lan.machines[0].driver.hand_over(), "handover accepted");
    assert!(lan.machines[0].driver.ending());
    assert!(
        !lan.machines[0].driver.end(),
        "end refused after the handover"
    );
    let asked = lan.now;
    lan.frames_until(|l| {
        l.machines[1].driver.is_host() && l.machines[2].driver.status().migration.is_none()
    });
    let after = lan.now - asked;
    assert!(after < 2_000, "without waiting time ({after} ms)");
    lan.run_to(&[1, 2], cut + STEPS);
    let m0 = &lan.machines[0];
    assert_eq!(m0.finish, vec![NetFinish::HandedOver]);
    assert!(m0.saves.is_empty(), "no file at the old host");
    assert!(m0.failed.is_none());
    assert!(!m0.driver.ending() && m0.driver.finished());
    assert_eq!(NetFinish::HandedOver.save_name(), None);
    let handover_step = m0.sim.state.steps;
    let (m1, m2) = (&lan.machines[1], &lan.machines[2]);
    for (i, m) in [(1, m1), (2, m2)] {
        let what = format!("machine {i}");
        assert!(m.failed.is_none() && m.finish.is_empty(), "{what}");
        assert_eq!(m.sim.state.players[0].result, 1, "{what}");
        assert_eq!(defeat_shown(m, 0), 1, "{what}: message 200 once");
        assert!(
            m.controls
                .iter()
                .any(|c| matches!(c, NetControl::Handover { .. }))
        );
        assert_replays(m, 0, &what);
    }
    assert_eq!(
        m1.lost.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        vec![0],
        "dropout of the old host once"
    );
    assert_same_prefix(m0, m1, "until the handover");
    assert!(m1.seq.len() as u64 > handover_step);
}

/// The new host is fully host - in the frame of the change the driver sets `SimHost::net_host`, which enables the
/// mirror commands of the UI; the other client stays without.
// QUIRK E-269: deviation
#[test]
fn new_host_mirrors_script_commands() {
    let mut lan = Lan::running([true; 3]);
    lan.play_for(PLAY_MS);
    for i in [1, 2] {
        assert!(!lan.machines[i].sim.net_host);
    }
    lan.crash(0);
    lan.frames_until(|l| !l.machines[1].became_host.is_empty());
    assert!(lan.machines[1].sim.net_host, "set in the same frame");
    assert!(lan.machines[1].driver.is_host());
    lan.play_for(PLAY_MS + REJOIN_WINDOW_MS);
    assert_eq!(lan.machines[1].became_host.len(), 1);
    assert!(lan.machines[2].became_host.is_empty());
    assert!(!lan.machines[2].sim.net_host);
}

/// `NET_GOLDEN` stays when the clients listen and the host probes them before `Go` (connections over the transports).
#[test]
fn net_golden_is_unchanged_with_listening_clients() {
    let mut lan = Lan::running([true; 3]);
    lan.run_to(&[0, 1, 2], STEPS);
    let host = lan.machines[0].prefix();
    assert_full(&host);
    for m in &lan.machines[1..] {
        assert_eq!(m.prefix(), host, "same sequence as the host");
    }
    let h = sequence_hash(&host);
    assert_eq!(h, NET_GOLDEN, "checksum sequence {h:#018x}");
    assert_replays(&lan.machines[0], 0, "host");
}
