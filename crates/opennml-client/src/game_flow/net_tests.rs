//! A match in the network in session and flow: three machines of the network match (`testworld::net_host`, connected
//! with [`connect_net`]) as fakes [`FakeSession`] with their own round driver. The tests of the session (pause, game
//! speed, game menu, resync) drive the fakes directly; the tests of the flow (saving, surrendering, loading/restart)
//! additionally a menu host per machine on the corpus dialogs ([`Run`], silently skipped without `NML_GAME_DIR`).

use opennml_net::{MemNet, TurnConfig};
use opennml_sim::command::CommandKind;
use opennml_ui::draft::LevelInfoData;

use super::tests::Run;
use super::*;
use crate::game::hud::{SPEED_LOWERED_MESSAGE, SPEED_RAISED_MESSAGE};
use crate::game::testworld::{self, NET_SLOTS};
use crate::testutil::{FakeSession, NET_CLOCK, NET_FRAME_MS, NET_SESSION, connect_net, net_paths};

/// Maximum number of frames before a run counts as hung.
const MAX_FRAMES: u32 = 20_000;

/// Track the checksum sequence `(steps, checksum)` of the host (per frame at most two ticks, the history holds 64); a
/// new value for the last tick (adoption at the resync, corrupting) replaces the old one.
fn collect(seq: &mut Vec<(u64, u64)>, sim: &SimHost) {
    for &(s, h) in &sim.hashes {
        match seq.last_mut() {
            Some((last, _)) if s < *last => {}
            Some((last, old)) if s == *last => *old = h,
            _ => seq.push((s, h)),
        }
    }
}

/// The three machines as fakes, [`NET_FRAME_MS`] of played time per frame.
struct Peers {
    net: MemNet,
    views: Vec<FakeSession>,
    now: u64,
    /// `running` per machine (game menu open = `false`, as `GameFlow::frame` passes it).
    running: [bool; 3],
    seqs: Vec<Vec<(u64, u64)>>,
    _dir: tempfile::TempDir,
}

impl Peers {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("folder");
        let mut now = 0;
        let (net, drivers) = connect_net(dir.path(), &mut now);
        let mut views = Vec::new();
        let mut seqs = Vec::new();
        for (slot, driver) in drivers.into_iter().enumerate() {
            let mut view = FakeSession::default();
            view.start_net(testworld::net_host(slot as u8, slot == 0), driver);
            let mut seq = Vec::new();
            collect(&mut seq, view.sim().unwrap());
            seqs.push(seq);
            views.push(view);
        }
        Self {
            net,
            views,
            now,
            running: [true; 3],
            seqs,
            _dir: dir,
        }
    }

    fn frame(&mut self) {
        self.now += NET_FRAME_MS;
        self.net.advance_to(self.now);
        for (i, view) in self.views.iter_mut().enumerate() {
            view.advance(self.now, self.running[i]);
            collect(&mut self.seqs[i], view.sim().unwrap());
        }
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame();
        }
    }

    fn frames_until(&mut self, cond: impl Fn(&Self) -> bool) {
        for _ in 0..MAX_FRAMES {
            if cond(self) {
                return;
            }
            self.frame();
        }
        panic!(
            "condition not reached after {MAX_FRAMES} frames, ticks {:?}",
            self.all_steps()
        );
    }

    fn sim(&self, i: usize) -> &SimHost {
        self.views[i].sim().unwrap()
    }

    fn steps(&self, i: usize) -> u64 {
        self.sim(i).state.steps
    }

    fn all_steps(&self) -> Vec<u64> {
        (0..self.views.len()).map(|i| self.steps(i)).collect()
    }

    fn all_shown(&self) -> bool {
        self.views.iter().all(|v| v.pause().shown)
    }

    fn speed_messages(&self, i: usize) -> Vec<(u64, i32)> {
        self.views[i].game.as_ref().unwrap().speed_messages.clone()
    }

    /// Sequence up to tick `n` per machine.
    fn prefix(&self, i: usize, n: u64) -> Vec<(u64, u64)> {
        self.seqs[i]
            .iter()
            .copied()
            .take_while(|&(s, _)| s <= n)
            .collect()
    }
}

/// O-143: Client 1 has the game menu open the whole time (`running = false`). In the network that does not stop it:
/// all three keep computing (the host cannot go on without the contributions of client 1, so they go out empty), its
/// presentation clock keeps running, and the sequences equal those of a run without a menu.
// QUIRK O-143: kept
#[test]
fn menu_open_keeps_the_rounds_running() {
    const UNTIL: u64 = 200;
    let mut reference = Peers::new();
    reference.frames_until(|p| p.all_steps().iter().all(|&s| s >= UNTIL));
    let mut p = Peers::new();
    p.running[1] = false;
    p.frames_until(|p| p.all_steps().iter().all(|&s| s >= UNTIL));
    let host = reference.prefix(0, UNTIL);
    assert_eq!(host.len() as u64, UNTIL + 1, "sequence gapless");
    for i in 0..3 {
        assert_eq!(p.prefix(i, UNTIL), host, "machine {i}");
    }
    assert!(p.sim(0).log.is_empty(), "no commands: empty contributions");
    let g = p.views[1].game.as_ref().unwrap();
    assert!(!g.running, "client 1 with an open menu");
    assert!(
        g.clock.now_ms > 0.0,
        "the presentation clock runs with the ticks"
    );
    assert!(!p.views[1].pause().shown, "no halt through the menu");
}

/// E-264, O-316: Client 1 pauses. All machines stop after the same execution tick and show the pause (also those that
/// did not pause); before that nobody shows it. Host and client 2 cannot lift it, only client 1.
// QUIRK O-316: kept
// QUIRK E-264: deviation
#[test]
fn net_pause_shows_on_every_peer_and_only_the_pauser_lifts_it() {
    let mut p = Peers::new();
    p.frames_until(|p| p.steps(1) >= 20);
    assert!(p.views[1].toggle_pause(), "pause wish of client 1");
    assert!(
        !p.views[1].pause().shown,
        "not until after the execution tick"
    );
    let mut stand = None;
    p.frames_until(|p| p.all_shown());
    for i in 0..3 {
        let s = p.steps(i);
        assert_eq!(*stand.get_or_insert(s), s, "all stop at the same tick");
        assert!(p.views[i].net().unwrap().halted());
    }
    let stand = stand.unwrap();
    assert!(
        TurnConfig::default().exec_at(stand - 1).is_some(),
        "stop tick {stand} = e(k) + 1"
    );
    p.frames(40);
    assert_eq!(p.all_steps(), vec![stand; 3], "stands");
    assert!(
        !p.views[0].toggle_pause(),
        "host does not lift a foreign pause"
    );
    assert!(!p.views[2].toggle_pause(), "client 2 likewise");
    p.frames(40);
    assert!(p.all_shown(), "pause stays");
    assert_eq!(p.all_steps(), vec![stand; 3]);
    assert!(p.views[1].toggle_pause(), "client 1 lifts it");
    p.frames_until(|p| {
        p.views.iter().all(|v| !v.pause().shown) && p.all_steps().iter().all(|&s| s > stand + 20)
    });
    let end = *p.all_steps().iter().min().unwrap();
    for i in 1..3 {
        assert_eq!(p.prefix(i, end), p.prefix(0, end), "machine {i}");
    }
}

/// O-154: Losing focus becomes a pause wish in the network like the key; another loss of focus does not lift the
/// pause, the key of the pauser does.
// QUIRK E-264: deviation
#[test]
fn focus_loss_sends_a_pause_wish() {
    let mut p = Peers::new();
    p.frames_until(|p| p.steps(0) >= 20);
    assert!(p.views[0].focus_lost(), "pause wish of the host");
    p.frames_until(|p| p.all_shown());
    let stand = p.steps(0);
    assert_eq!(p.all_steps(), vec![stand; 3]);
    assert!(!p.views[0].focus_lost(), "no lifting on focus loss");
    assert!(!p.views[2].focus_lost());
    p.frames(40);
    assert!(p.all_shown());
    assert_eq!(p.all_steps(), vec![stand; 3]);
    assert!(p.views[0].toggle_pause(), "the key of the pauser lifts it");
    p.frames_until(|p| p.all_steps().iter().all(|&s| s > stand + 10));
    assert!(p.views.iter().all(|v| !v.pause().shown));
}

/// E-265 (`FUN_0042F050`): On clients the speed keys have no effect, also no message and no value for the setting.
// QUIRK E-265: deviation
#[test]
fn speed_key_on_client_does_nothing() {
    let mut p = Peers::new();
    p.frames_until(|p| p.steps(1) >= 10);
    let speed = p.sim(1).game_speed;
    assert_eq!(p.views[1].change_game_speed(1), None);
    assert_eq!(p.views[2].change_game_speed(-1), None);
    p.frames_until(|p| p.all_steps().iter().all(|&s| s >= 100));
    for i in 0..3 {
        assert_eq!(p.sim(i).game_speed, speed, "machine {i}");
        assert!(p.speed_messages(i).is_empty(), "machine {i}");
    }
}

/// E-294 (`FUN_0042F050`, `GameEngine_CmdSetGameSpeed` `0x0042EF30`): The speed key of the host returns the target
/// value from the running game speed for the setting and wishes it; the game speed does not change until after the
/// execution tick, on all machines at the same tick, with the message 9 on each. Two presses in the same round give +1,
/// not +2. A later lowering reports 10.
// QUIRK E-294: kept
#[test]
fn speed_key_on_host_changes_all_at_the_same_step() {
    let mut p = Peers::new();
    p.frames_until(|p| p.steps(0) >= 10);
    let speed = p.sim(0).game_speed;
    assert_eq!(
        p.views[0].change_game_speed(1),
        Some(speed + 1),
        "setting of the host"
    );
    assert_eq!(
        p.views[0].change_game_speed(1),
        Some(speed + 1),
        "second press: again from the running game speed"
    );
    assert_eq!(p.sim(0).game_speed, speed, "not until the bundle");
    p.frames_until(|p| (0..3).all(|i| p.sim(i).game_speed == speed + 1));
    p.frames(80);
    let mut at = None;
    for i in 0..3 {
        assert_eq!(p.sim(i).game_speed, speed + 1, "machine {i}: +1 once");
        let messages = p.speed_messages(i);
        assert_eq!(messages.len(), 1, "machine {i}: {messages:?}");
        assert_eq!(messages[0].1, SPEED_RAISED_MESSAGE);
        assert_eq!(*at.get_or_insert(messages[0].0), messages[0].0, "same tick");
    }
    let at = at.unwrap();
    assert!(
        TurnConfig::default().exec_at(at - 1).is_some(),
        "after e(k): {at}"
    );
    assert_eq!(p.views[0].change_game_speed(-1), Some(speed));
    p.frames_until(|p| (0..3).all(|i| p.sim(i).game_speed == speed));
    for i in 0..3 {
        let messages = p.speed_messages(i);
        assert_eq!(messages.len(), 2, "machine {i}");
        assert_eq!(messages[1].1, SPEED_LOWERED_MESSAGE);
    }
}

/// If client 2 adopts the state of the host at the resync, its session rebuilds the presentation as after loading,
/// exactly once; the others do not. Afterwards equal sequences.
#[test]
fn net_adopt_rebuilds_the_presentation() {
    let mut p = Peers::new();
    p.frames_until(|p| p.steps(2) >= 45);
    assert_eq!(p.views[2].sim_mut().unwrap().debug_corrupt(), "daytime");
    p.frames_until(|p| p.all_steps().iter().all(|&s| s >= 200));
    let rebuilds: Vec<u32> = p
        .views
        .iter()
        .map(|v| v.game.as_ref().unwrap().rebuilds)
        .collect();
    assert_eq!(rebuilds, vec![0, 0, 1]);
    let tail = |seq: &[(u64, u64)]| -> Vec<(u64, u64)> {
        seq.iter()
            .copied()
            .filter(|&(s, _)| (100..=200).contains(&s))
            .collect()
    };
    assert_eq!(tail(&p.seqs[2]), tail(&p.seqs[0]));
}

// --- Flow: menu host per machine on the corpus dialogs ---------------------------------------------------------------

/// The three machines with flow and menu host (one [`Run`] each); common time like [`Run::frame`] (16 ms per frame).
struct NetRuns {
    net: MemNet,
    runs: Vec<Run>,
    dir: tempfile::TempDir,
}

impl NetRuns {
    fn corpus() -> Option<Self> {
        let dir = tempfile::tempdir().expect("folder");
        let mut now = 0;
        let (net, drivers) = connect_net(dir.path(), &mut now);
        let mut runs = Vec::new();
        for (slot, driver) in drivers.into_iter().enumerate() {
            let mut run = Run::corpus()?;
            run.now = now;
            run.view
                .start_net(testworld::net_host(slot as u8, slot == 0), driver);
            let meta = MatchMeta {
                level_info: LevelInfoData::default(),
                game_type: 0,
                lock_teams: false,
                save_replay: false,
                playback: None,
            };
            run.flow.started(meta, &mut run.host.app, &mut run.view);
            runs.push(run);
        }
        Some(Self { net, runs, dir })
    }

    fn frame(&mut self) {
        self.net.advance_to(self.runs[0].now);
        for run in &mut self.runs {
            run.frame();
        }
    }

    fn frames_until(&mut self, cond: impl Fn(&Self) -> bool) {
        for _ in 0..MAX_FRAMES {
            if cond(self) {
                return;
            }
            self.frame();
        }
        panic!("condition not reached after {MAX_FRAMES} frames");
    }

    fn sim(&self, i: usize) -> &SimHost {
        self.runs[i].view.sim().unwrap()
    }

    fn steps(&self, i: usize) -> u64 {
        self.sim(i).state.steps
    }

    /// Network savegame `name` of machine `i` (`savegame/multiplayer/<name>`, without extension).
    fn save_path(&self, i: usize, name: &str) -> std::path::PathBuf {
        net_paths(self.dir.path(), i).net_save(name)
    }
}

/// E-274: Saving in the network is a wish; each machine writes its own file `savegame/multiplayer/<name>` without
/// extension after the execution tick, all with the same simulation part and the own slot in the network part. The
/// single-player folder stays empty, an invalid name is not wished at all, and a second saving under the same name
/// overwrites without asking.
// QUIRK E-274: kept
#[test]
fn net_save_writes_one_file_per_peer() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(2) >= 10);
    n.runs[2].apply(UiEffect::SaveGame {
        name: "../evil".into(),
    });
    n.runs[2].apply(UiEffect::SaveGame {
        name: "netz1".into(),
    });
    assert!(
        !n.save_path(2, "netz1").exists(),
        "not until after the execution tick"
    );
    n.frames_until(|n| (0..3).all(|i| n.save_path(i, "netz1").exists()));
    let read = |i: usize| {
        GameSaveFile::from_bytes(&std::fs::read(n.save_path(i, "netz1")).unwrap()).unwrap()
    };
    let files: Vec<GameSaveFile> = (0..3).map(read).collect();
    for (i, file) in files.iter().enumerate() {
        file.check(&n.sim(i).rules).unwrap();
        assert_eq!(
            file.sim.to_bytes(),
            files[0].sim.to_bytes(),
            "machine {i}: same simulation part"
        );
        assert!(file.sim.state.network, "network match");
        let net = file.host.net.as_ref().expect("network part");
        assert_eq!(net.local_slot, i as u8);
        assert_eq!(net.session, NET_SESSION);
        assert_eq!((net.spacing, net.lead), (8, 4));
        assert_eq!(net.seats.len(), 8);
        assert_eq!(net.seats[2], (Some(2), "P2".to_string()));
        assert_eq!(net.seats[usize::from(NET_SLOTS)].0, None);
    }
    let at = files[0].sim.state.steps;
    assert!(
        TurnConfig::default().exec_at(at - 1).is_some(),
        "after e(k): {at}"
    );
    for i in 0..3 {
        let dir = net_paths(n.dir.path(), i).saves.join("multiplayer");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["netz1".to_string()],
            "machine {i}: only the valid name"
        );
        assert!(
            !n.runs[i].files.dir.exists(),
            "single-player folder untouched"
        );
    }
    // Once more under the same name (from the host): overwrites on all.
    n.frames_until(|n| n.steps(0) >= at + 20);
    n.runs[0].apply(UiEffect::SaveGame {
        name: "netz1".into(),
    });
    n.frames_until(|n| (0..3).all(|i| read_steps(&n.save_path(i, "netz1")) > at));
    let later = read_steps(&n.save_path(0, "netz1"));
    for i in 1..3 {
        assert_eq!(read_steps(&n.save_path(i, "netz1")), later);
    }
}

fn read_steps(path: &std::path::Path) -> u64 {
    std::fs::read(path)
        .ok()
        .and_then(|b| GameSaveFile::from_bytes(&b).ok())
        .map_or(0, |f| f.sim.state.steps)
}

/// E-267 (event `0x27`): Surrendering in the network goes through the round as its own command, not immediately;
/// surrendering lifts an own pause beforehand. The defeat comes at the execution tick on all machines, afterwards the
/// surrendering one shows the end screen (game music off).
// QUIRK E-267: kept
#[test]
fn surrender_goes_through_the_round() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(1) >= 10);
    assert!(n.runs[1].view.toggle_pause());
    n.frames_until(|n| n.runs.iter().all(|r| r.view.pause().shown));
    n.runs[1].apply(UiEffect::Surrender { slot: 1 });
    assert_eq!(
        n.sim(1).state.players[1].result,
        0,
        "no immediate execution"
    );
    assert!(
        !n.runs[1].view.music.contains(&"stop-game"),
        "no end screen yet"
    );
    n.frames_until(|n| (0..3).all(|i| n.sim(i).state.players[1].result == 1));
    let step_of = |sim: &SimHost| -> Vec<u64> {
        sim.log
            .iter()
            .filter(|c| {
                c.command.player == 1
                    && c.command.kind == CommandKind::SetPlayerResult { slot: 1, result: 1 }
            })
            .map(|c| c.step)
            .collect()
    };
    let at = step_of(n.sim(0));
    assert_eq!(at.len(), 1, "one result command");
    assert!(
        TurnConfig::default().exec_at(at[0]).is_some(),
        "at the execution tick"
    );
    for i in 1..3 {
        assert_eq!(step_of(n.sim(i)), at, "machine {i}");
    }
    for r in &n.runs {
        assert!(!r.view.pause().shown, "the own pause is lifted");
    }
    n.frame();
    assert!(
        n.runs[1].view.music.contains(&"stop-game"),
        "end screen on the surrendering player"
    );
    assert!(!n.runs[0].view.music.contains(&"stop-game"));
}

/// "Load game" and "Restart" of the game menu have no effect in the network (log), the game speed from the options
/// menu when closing the menu likewise (E-265); the match keeps running as before.
// QUIRK O-143: kept
// QUIRK E-265: deviation
#[test]
fn load_and_restart_are_inert_in_the_net() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(0) >= 10);
    let speed = n.sim(0).game_speed;
    let level = n.runs[0].view.level().to_string();
    n.runs[0].apply(UiEffect::LoadGame {
        name: "quicksave1".into(),
    });
    n.runs[0].apply(UiEffect::ReloadLevel);
    n.runs[1].apply(UiEffect::ResumeGame {
        game_speed: Some(3),
    });
    assert!(n.runs[0].flow.running());
    assert!(n.runs[0].view.music.is_empty(), "no loading started");
    assert_eq!(n.runs[0].view.level(), level);
    assert_eq!(
        n.sim(1).game_speed,
        speed,
        "no game speed from the options menu"
    );
    let before = n.steps(0);
    n.frames_until(|n| (0..3).all(|i| n.steps(i) >= before + 60));
    let at = (0..3).map(|i| n.steps(i)).min().unwrap();
    let hash_at = |i: usize| n.sim(i).hash_at(at);
    assert!(hash_at(0).is_some());
    assert_eq!(hash_at(1), hash_at(0));
    assert_eq!(hash_at(2), hash_at(0));
}

// --- End and abort of the host, leaving ------------------------------------------------------------------------------

/// Text of the open message `MSGBOX` of the run (free text), otherwise `None`.
pub(super) fn message(run: &Run) -> Option<String> {
    if !run.dialogs().iter().any(|d| d == "msgbox") {
        return None;
    }
    run.host.app.screen_state().info_free.clone()
}

/// The run shows the main menu without match and without accounting.
pub(super) fn in_main_menu(run: &Run) -> bool {
    let dialogs = run.dialogs();
    dialogs.iter().any(|d| d == "start")
        && !dialogs.iter().any(|d| d == "accounting" || d == "msgbox")
        && !run.flow.running()
        && run.view.game.is_none()
        && run.host.app.game().is_none()
}

/// The host ends the match for all ([`NetDriver::end`]). In the game menu it surrenders like a client; the end for all
/// comes from the end screen or from the program end without a reachable client ([`NetDriver::host_leaves`]). The tests
/// of the end therefore call it directly.
pub(super) fn end_for_all(run: &mut Run) {
    assert!(
        run.view.net_mut().expect("driver in the match").end(),
        "end accepted"
    );
}

/// The run shows the accounting without match, without message and without main menu yet.
pub(super) fn in_accounting(run: &Run) -> bool {
    let dialogs = run.dialogs();
    dialogs.iter().any(|d| d == "accounting")
        && !dialogs.iter().any(|d| d == "start" || d == "msgbox")
        && !run.flow.running()
        && run.view.game.is_none()
        && run.host.app.game().is_none()
        && run.host.app.data().accounting.is_some()
}

/// "Main menu" of the accounting (`MAINMENU`, `0x004D94A7`: `{3,"accounting"}`, `{4, tab}`, `{5,"start"}`); the
/// accounting is in state 0.
pub(super) fn leave_accounting(run: &mut Run) {
    assert_eq!(run.state(), 0, "state 0 in the accounting");
    run.click("accounting", "mainmenu");
}

impl NetRuns {
    /// The runs `runs` show the accounting; their "Main menu" leads to the main menu, state 0.
    fn accounting_then_main_menu(&mut self, runs: std::ops::Range<usize>) {
        let r = runs.clone();
        self.frames_until(move |n| n.runs[r.clone()].iter().all(in_accounting));
        self.settle();
        for run in &mut self.runs[runs.clone()] {
            leave_accounting(run);
        }
        let r = runs.clone();
        self.frames_until(move |n| n.runs[r.clone()].iter().all(in_main_menu));
        for run in &self.runs[runs] {
            assert_eq!(run.state(), 0, "state 0 in the main menu");
        }
    }

    /// Frames until the UI of every run rests (dialogs opened and drawn), at most 3 s.
    fn settle(&mut self) {
        let busy = |r: &Run| {
            let app = &r.host.app;
            app.menu().busy() || !app.queue().is_empty() || app.queue().waiting()
        };
        for _ in 0..200 {
            self.frame();
            if !self.runs.iter().any(busy) {
                break;
            }
        }
        self.frame();
    }

    /// "OK" of the message on the runs `runs` as soon as it is drawn.
    fn confirm(&mut self, runs: std::ops::Range<usize>) {
        self.settle();
        for run in &mut self.runs[runs] {
            run.click("msgbox", "ok");
        }
    }
}

fn read_save(path: &std::path::Path) -> GameSaveFile {
    GameSaveFile::from_bytes(&std::fs::read(path).expect("network savegame")).expect("readable")
}

/// The host ends the network match for all ([`end_for_all`]; without a reachable client from the end
/// screen or at the program end). Each machine writes its network savegame
/// `netz_YYYY_MM_DD_hhmm` (local time of the host) with the same simulation part, stops the game music and shows "The
/// host has ended the match. It was saved as "<name>"." as `MSGBOX` over the stopped match; "OK" is the game end
/// `0x0E`: accounting from the own statistics, whose "Main menu" leads to the main menu.
// QUIRK E-273: kept
// QUIRK E-268: deviation
#[test]
fn host_end_shows_the_message_then_the_accounting() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(2) >= 10);
    end_for_all(&mut n.runs[0]);
    n.frames_until(|n| n.runs.iter().all(|r| message(r).is_some()));
    let name = NET_CLOCK.net_end_name();
    let expected = format!("The host has ended the match. It was saved as \"{name}\".");
    let files: Vec<GameSaveFile> = (0..3).map(|i| read_save(&n.save_path(i, &name))).collect();
    for (i, file) in files.iter().enumerate() {
        let run = &n.runs[i];
        assert_eq!(
            message(run).as_deref(),
            Some(expected.as_str()),
            "machine {i}"
        );
        assert!(run.view.music.contains(&"stop-game"), "machine {i}");
        assert!(
            run.flow.running(),
            "machine {i}: the match stands behind the message"
        );
        file.check(&n.sim(i).rules).unwrap();
        assert_eq!(
            file.sim.to_bytes(),
            files[0].sim.to_bytes(),
            "machine {i}: same simulation part"
        );
        assert_eq!(
            file.sim.state.steps,
            n.steps(i),
            "machine {i} stands at the state"
        );
        assert_eq!(file.host.net.as_ref().unwrap().local_slot, i as u8);
        assert_eq!(
            n.sim(i).state.players[0].result,
            0,
            "no surrender of the host"
        );
    }
    n.confirm(0..3);
    n.frames_until(|n| n.runs.iter().all(in_accounting));
    for (i, run) in n.runs.iter().enumerate() {
        let view = run.host.app.data().accounting.as_ref().unwrap();
        assert_eq!(view.rows.len(), usize::from(NET_SLOTS), "machine {i}");
    }
    n.accounting_then_main_menu(0..3);
}

/// Abort: The host crashes (its session disappears without `Bye`). Each client writes its state after the last tick it
/// could compute as `netz_YYYY_MM_DD_hhmm` (own local time), shows "The connection to the host was aborted. The match
/// was saved as "<name>"."; "OK" leads through the accounting to the main menu.
// QUIRK E-273: kept
// QUIRK E-258: deviation
#[test]
fn host_crash_shows_the_abort_message_then_the_accounting() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(2) >= 10);
    n.runs[0].view.end();
    n.frames_until(|n| n.runs[1..].iter().all(|r| message(r).is_some()));
    let name = NET_CLOCK.net_end_name();
    let expected =
        format!("The connection to the host was aborted. The match was saved as \"{name}\".");
    for i in 1..3 {
        assert_eq!(
            message(&n.runs[i]).as_deref(),
            Some(expected.as_str()),
            "machine {i}"
        );
        let file = read_save(&n.save_path(i, &name));
        file.check(&n.sim(i).rules).unwrap();
        assert_eq!(file.sim.state.steps, n.steps(i), "machine {i}");
        assert!(n.runs[i].view.music.contains(&"stop-game"));
    }
    n.confirm(1..3);
    n.accounting_then_main_menu(1..3);
}

/// A client surrenders and closes the end screen ("End current game", `0x0E`): in the network `Bye`, accounting and
/// its "Main menu". The host treats the `Bye` as a dropout and no longer waits for it; host and client 2 see no
/// message.
// QUIRK E-273: kept
#[test]
fn client_leaves_with_bye_on_the_way_to_the_main_menu() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(1) >= 10);
    n.runs[1].apply(UiEffect::Surrender { slot: 1 });
    n.frames_until(|n| n.runs[1].dialogs().iter().any(|d| d == "gameend_defeat"));
    n.settle();
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.accounting_then_main_menu(1..2);
    let left_at = n.steps(0);
    n.frames_until(|n| n.steps(0) > left_at + 100 && n.steps(2) > left_at + 100);
    for i in [0, 2] {
        assert_eq!(message(&n.runs[i]), None, "machine {i}");
        assert!(n.runs[i].flow.running(), "machine {i}");
        assert_eq!(n.sim(i).state.players[1].result, 1, "machine {i}");
    }
}

/// After the third desync all show "The match is no longer in sync (checksum error). It was stopped and saved as
/// "<name>"."; "OK" leads through the accounting to the main menu (the path via the game menu is dropped: its
/// "Surrender" would go through rounds that no longer run).
// QUIRK E-273: kept
#[test]
fn third_desync_shows_the_stop_message() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    for at in [45, 101, 149] {
        n.frames_until(|n| {
            n.steps(2) >= at && !n.runs[2].view.net().is_some_and(NetDriver::halted)
        });
        assert_eq!(n.runs[2].view.sim_mut().unwrap().debug_corrupt(), "daytime");
    }
    n.frames_until(|n| n.runs.iter().all(|r| message(r).is_some()));
    let name = NET_CLOCK.checksum_error_name();
    let expected = format!(
        "The match is no longer in sync (checksum error). It was stopped and saved as \"{name}\"."
    );
    for i in 0..3 {
        assert_eq!(
            message(&n.runs[i]).as_deref(),
            Some(expected.as_str()),
            "machine {i}"
        );
        assert!(n.save_path(i, &name).exists(), "machine {i}");
    }
    n.confirm(0..3);
    n.accounting_then_main_menu(0..3);
}

/// The host drops out and leaves the match via "End current game" of its end screen (`0x0E`). As long as the rounds
/// are running, that is the orderly end, no abort: all show "The host has ended the match ..." with the same network
/// savegame; accounting, main menu and `Bye` do not follow on the host until after this end, with its "OK". The
/// clients of [`NetRuns`] have no address (connections via `MemNet::pair`), so there is nobody to hand over to; with
/// reachable clients the host hands over (`host_leaving_the_end_screen_hands_over`).
// QUIRK E-273: kept
#[test]
fn host_leaving_from_the_end_screen_ends_the_match_for_all() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(2) >= 10);
    n.runs[0].view.sim_mut().unwrap().submit(Command {
        player: 0,
        kind: CommandKind::SetPlayerResult { slot: 0, result: 1 },
    });
    n.frames_until(|n| n.runs[0].dialogs().iter().any(|d| d == "gameend_defeat"));
    n.settle();
    n.runs[0].click("gameend_defeat", "end_defeat");
    n.frames_until(|n| n.runs.iter().all(|r| message(r).is_some()));
    let name = NET_CLOCK.net_end_name();
    let expected = format!("The host has ended the match. It was saved as \"{name}\".");
    let files: Vec<GameSaveFile> = (0..3).map(|i| read_save(&n.save_path(i, &name))).collect();
    for (i, file) in files.iter().enumerate() {
        assert_eq!(
            message(&n.runs[i]).as_deref(),
            Some(expected.as_str()),
            "machine {i}"
        );
        assert_eq!(file.sim.to_bytes(), files[0].sim.to_bytes(), "machine {i}");
    }
    n.confirm(0..3);
    n.accounting_then_main_menu(0..3);
    for run in &n.runs {
        assert_eq!(
            run.host.app.screen_state().info_free,
            None,
            "message forgotten"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Network start: `GameFlow::start_net_match` loads the map of the start package like `start_match`, checks the rules
// checksum and reports `Loaded` through the driver. A client driver with a raw remote side as host (Welcome, Start),
// corpus map [`super::tests::TWO_PLAYERS`] over the fake with real maps.

/// Client driver with a raw remote side: after `Welcome` and `Start(info)` in `Loading`.
struct NetStart {
    net: MemNet,
    host_side: opennml_net::MemLink,
    driver: Option<NetDriver>,
    now: u64,
    _dir: tempfile::TempDir,
}

impl NetStart {
    fn new(info: opennml_net::StartInfo) -> Self {
        use opennml_net::{ClientCore, Link, NetMessage};
        let net = MemNet::new();
        let (to_host, mut host_side) = net.pair(5);
        let dir = tempfile::tempdir().expect("folder");
        let hello = opennml_net::Hello {
            listen_port: None,
            net_version: opennml_net::NET_VERSION,
            protocol: 1,
            format: 1,
            edition: "test".into(),
            name: "Guest".into(),
            build: "test".into(),
            seat: None,
        };
        let driver = NetDriver::client(
            ClientCore::new(hello),
            Box::new(to_host),
            net_paths(dir.path(), 1),
        );
        host_side.send(NetMessage::Welcome { slot: 1 });
        host_side.send(NetMessage::Start(Box::new(info)));
        let mut s = Self {
            net,
            host_side,
            driver: Some(driver),
            now: 0,
            _dir: dir,
        };
        for _ in 0..40 {
            s.pump();
        }
        let d = s.driver.as_ref().unwrap();
        assert_eq!(d.status().phase, opennml_net::Phase::Loading);
        assert!(d.start_info().is_some());
        s
    }

    fn pump(&mut self) {
        self.now += NET_FRAME_MS;
        self.net.advance_to(self.now);
        if let Some(d) = self.driver.as_mut() {
            d.pump(self.now);
        }
    }

    /// `Loaded` on the raw remote side (after a few frames of the driver `driver`).
    fn loaded_reply(&mut self, driver: &mut NetDriver) -> Option<Option<String>> {
        use opennml_net::{Link, LinkEvent, NetMessage};
        for _ in 0..40 {
            self.now += NET_FRAME_MS;
            self.net.advance_to(self.now);
            driver.pump(self.now);
        }
        self.host_side.poll().into_iter().find_map(|e| match e {
            LinkEvent::Message(NetMessage::Loaded { error }) => Some(error),
            _ => None,
        })
    }
}

/// Rules of the corpus map [`super::tests::TWO_PLAYERS`] (as on the host before the start) and its active slots.
fn two_player_rules(vfs: &Vfs) -> (Rules, Vec<u8>) {
    let load = crate::game::map::MapLoad::read(vfs, super::tests::TWO_PLAYERS).expect("map");
    let rules = Rules::load(vfs, &load.scene).expect("rules");
    let mut active: Vec<u8> = rules.level.players.iter().map(|p| p.slot).collect();
    active.sort_unstable();
    active.dedup();
    (rules, active)
}

fn net_start_info(
    rules: &Rules,
    checksum: u64,
    setup: MatchSetup,
    your_slot: u8,
) -> opennml_net::StartInfo {
    opennml_net::StartInfo {
        package: opennml_sim::setup::StartPackage {
            rules_checksum: checksum,
            ..opennml_sim::setup::start_package(rules, &setup)
        },
        seats: std::array::from_fn(|slot| opennml_net::Seat {
            peer: (slot < 2).then_some(slot as u8),
            name: format!("P{slot}"),
        }),
        your_slot,
        spacing: 8,
        lead: 4,
        game_speed: 3,
        session: NET_SESSION.into(),
        saved: None,
    }
}

fn run_net_start(run: &mut Run, driver: NetDriver) -> Result<(), Box<NetStartFailure>> {
    let env = FlowEnv {
        vfs: &run.vfs,
        texts: &run.host.layer.texts,
        files: &run.files,
    };
    let shown = &mut run.shown;
    run.flow.start_net_match(
        driver,
        &mut run.host.app,
        &mut run.view,
        &env,
        &mut |ldg, phase| shown.push((ldg.to_string(), phase.percent())),
    )
}

/// E-262, E-263: If the rules checksum of the start package differs from that of the map just loaded (different game
/// data or map), the match does not start; the client reports `Loaded { error }` with the reason, the driver comes back
/// (the game host waits for the abort by the network host), and no match remains.
// QUIRK E-263: deviation
#[test]
fn start_net_match_rejects_foreign_rules() {
    let Some(mut run) = Run::corpus() else {
        return;
    };
    run.view.real_levels = true;
    let (rules, active) = two_player_rules(&run.vfs);
    let setup = MatchSetup {
        network: true,
        seed: 5,
        ..MatchSetup::default_for(&rules)
    };
    let mut start = NetStart::new(net_start_info(&rules, rules.checksum ^ 1, setup, active[1]));
    let driver = start.driver.take().unwrap();
    let failure = run_net_start(&mut run, driver).expect_err("foreign rules");
    assert!(
        failure.error.contains("rules checksum"),
        "{}",
        failure.error
    );
    assert!(!run.flow.running(), "no match");
    assert!(run.view.game.is_none());
    assert!(run.host.app.game().is_none());
    let mut driver = failure.net;
    assert_eq!(
        start.loaded_reply(&mut driver),
        Some(Some(failure.error.clone())),
        "Loaded with reason"
    );
    assert!(!driver.started());
}

/// A slot without a human (here closed) cannot be the own one: `Loaded { error }`, no match.
#[test]
fn start_net_match_rejects_a_slot_without_human() {
    let Some(mut run) = Run::corpus() else {
        return;
    };
    run.view.real_levels = true;
    let (rules, active) = two_player_rules(&run.vfs);
    let mut setup = MatchSetup {
        network: true,
        ..MatchSetup::default_for(&rules)
    };
    setup.lobby[1].kind = opennml_sim::setup::SlotKind::Closed;
    let mut start = NetStart::new(net_start_info(&rules, rules.checksum, setup, active[1]));
    let driver = start.driver.take().unwrap();
    let failure = run_net_start(&mut run, driver).expect_err("no human slot");
    assert!(failure.error.contains("human slot"), "{}", failure.error);
    assert!(!run.flow.running());
    assert!(run.view.game.is_none());
    let mut driver = failure.net;
    assert_eq!(
        start.loaded_reply(&mut driver),
        Some(Some(failure.error.clone()))
    );
}

/// Same rules: the match runs with the setup of the start package (seed, network flag), the game speed of the host and
/// the own slot as local player in turn mode; the driver is in the session, `Loaded` without error goes out. Game type
/// 0, loading screen up to 100 %.
// QUIRK E-262: deviation
#[test]
fn start_net_match_enters_rounds_on_the_own_slot() {
    let Some(mut run) = Run::corpus() else {
        return;
    };
    run.view.real_levels = true;
    let (rules, active) = two_player_rules(&run.vfs);
    let setup = MatchSetup {
        network: true,
        seed: 5,
        ..MatchSetup::default_for(&rules)
    };
    let slot = active[1];
    let mut start = NetStart::new(net_start_info(&rules, rules.checksum, setup.clone(), slot));
    let driver = start.driver.take().unwrap();
    run_net_start(&mut run, driver).expect("Start");
    assert!(run.flow.running());
    assert_eq!(run.view.last_setup.as_ref(), Some(&setup));
    let sim = run.view.sim().unwrap();
    assert!(sim.net && !sim.net_host);
    assert_eq!(sim.active_player, slot);
    assert!(sim.state.network);
    assert_eq!(sim.game_speed, 3);
    let game = run.host.app.game().copied().unwrap();
    assert_eq!((game.game_type, game.local_slot), (0, slot));
    assert!(
        run.shown.iter().any(|(_, p)| *p == 100),
        "loading screen: {:?}",
        run.shown
    );
    let mut driver = run
        .view
        .game
        .as_mut()
        .unwrap()
        .net
        .take()
        .expect("driver in the session");
    assert_eq!(start.loaded_reply(&mut driver), Some(None));
}

/// The nation music is that of the own slot. The original sets the local player from the lobby
/// (`GameEngine_SetupPlayersFromLobby` `0x00432C00`) before `Game_FinishLevelSetup` (`0x004C7828`) starts the music
/// with its nation; an earlier version of the reimplementation started it at loading, before `enter_rounds`
/// set the own slot, thus with the nation of the first human of the map (the host). Real session (corpus and GPU,
/// otherwise silently skipped) with sound management without output, client on the second slot.
// QUIRK E-297: kept
#[test]
fn net_start_plays_the_nation_music_of_the_own_slot() {
    let Some(mut run) = Run::corpus() else {
        return;
    };
    let Ok((device, queue, _)) =
        opennml_render::headless_device(opennml_render::BackendChoice::Auto)
    else {
        eprintln!("no GPU, test skipped");
        return;
    };
    let (rules, active) = two_player_rules(&run.vfs);
    let setup = MatchSetup {
        network: true,
        seed: 5,
        ..MatchSetup::default_for(&rules)
    };
    let slot = active[1];
    let mut start = NetStart::new(net_start_info(&rules, rules.checksum, setup, slot));
    let driver = start.driver.take().unwrap();
    let mut audio = AudioEngine::new(opennml_audio::db::SoundDb::load(&run.vfs), None);
    let mut session = None;
    let mut view_options = ViewOptions::default();
    let mut controls = Controls::default();
    let env = FlowEnv {
        vfs: &run.vfs,
        texts: &run.host.layer.texts,
        files: &run.files,
    };
    let mut view = LiveSession {
        device,
        queue,
        format: opennml_render::CAPTURE_FORMAT,
        vfs: &run.vfs,
        session: &mut session,
        view: &mut view_options,
        controls: &mut controls,
        size: (320, 200),
        audio: Some(&mut audio),
    };
    run.flow
        .start_net_match(driver, &mut run.host.app, &mut view, &env, &mut |_, _| {})
        .expect("Start");
    let sim = view.sim().unwrap();
    assert_eq!(sim.active_player, slot);
    let nation = |s: u8| usize::try_from(sim.state.players[usize::from(s)].nation).ok();
    let (own, first) = (nation(slot), nation(active[0]));
    assert_ne!(own, first, "the map must give the slots different nations");
    drop(view);
    assert_eq!(
        audio.music.nation(),
        own,
        "nation music of the own slot {slot}"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Network session from the menu: one menu host ([`Run`]) per machine with [`NetSession`] over a shared [`MemNet`]
// (`MemTransport`, played time 16 ms per frame); `LAN1` -> `LAN3` -> `LAN2`, join, lobby, start. Corpus dialogs and
// real maps (`real_levels`), silently skipped without `NML_GAME_DIR`.

use crate::game::net_driver::NetPaths;
use crate::net_session::{InlineWorker, KICKED_TEXT, NetIdentity, NetPhase, NetSession};
use opennml_net::{DataState, LobbyState, MemTransport, RasterStage};
use opennml_ui::input::{MouseButton as UiButton, UiInput};
use opennml_ui::msg::{AppCmd, LobbyAction};

/// Game port of the hosts in the process.
const LAN_PORT: u16 = 2302;
/// Fixed seed of the network matches from the menu (tests and `UI_GOLDEN` fixed).
pub(super) const LAN_SEED: u32 = 4711;
/// Maximum number of frames of 16 ms until a step of the network menus counts as hung.
const MAX_LAN_FRAMES: u32 = 3_000;
/// The frames of the lobby (`open_lobby`), sorted.
const LOBBY: [&str; 3] = ["chatframe", "lan2", "levelinfo"];

/// Sign-on of a machine of the tests (build ID "test").
fn lan_hello(name: &str) -> opennml_net::Hello {
    opennml_net::Hello {
        listen_port: None,
        net_version: opennml_net::NET_VERSION,
        protocol: 1,
        format: 1,
        edition: "test".into(),
        name: name.into(),
        build: "test".into(),
        seat: None,
    }
}

/// Machine with menu host and network session over a shared `MemNet`.
pub(super) struct Lan {
    pub(super) net: MemNet,
    pub(super) runs: Vec<Run>,
    /// Address per machine in the `MemNet` (for [`MemNet::cut_host`]).
    pub(super) ips: Vec<std::net::IpAddr>,
    dir: tempfile::TempDir,
}

impl Lan {
    /// One machine per name (machine 0 without, the others with 10 ms delay per machine); `NetName` = name.
    pub(super) fn corpus(names: &[&str]) -> Option<Self> {
        Self::corpus_with(names, |_, _| {})
    }

    /// Like [`Self::corpus`], the settings of machine `i` changed by `adjust`. Each machine has its own savegame folder
    /// ([`Self::paths`]); menu storage and network session share it as in `opennml-game`.
    pub(super) fn corpus_with(
        names: &[&str],
        adjust: impl Fn(usize, &mut EngineConfig),
    ) -> Option<Self> {
        Self::corpus_listening(names, |_| true, adjust)
    }

    /// Like [`Self::corpus_with`]; machine `i` listens as a client only if `listens(i)` holds.
    pub(super) fn corpus_listening(
        names: &[&str],
        listens: impl Fn(usize) -> bool,
        adjust: impl Fn(usize, &mut EngineConfig),
    ) -> Option<Self> {
        let net = MemNet::new();
        let dir = tempfile::tempdir().expect("folder");
        let mut runs = Vec::new();
        let mut ips = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let mut run = Run::corpus_in(
                opennml_ui::app::MenuOptions::default(),
                Some(net_paths(dir.path(), i).saves),
                |cfg| adjust(i, cfg),
            )?;
            run.view.real_levels = true;
            let units: Vec<u16> = name.encode_utf16().collect();
            run.host.app.set_net_name(&units);
            let identity = NetIdentity {
                hello: lan_hello(name),
                port: LAN_PORT,
                seed: Box::new(|| LAN_SEED),
                listen: listens(i),
            };
            let transport = MemTransport::new(&net, 10 * i as u64);
            ips.push(transport.ip());
            let mut session =
                NetSession::new(Box::new(transport), identity, net_paths(dir.path(), i));
            session.set_fixed_clock(Some(NET_CLOCK));
            session.set_rules_worker(Box::new(InlineWorker::default()));
            run.flow.set_net_session(session);
            runs.push(run);
        }
        Some(Self {
            net,
            runs,
            ips,
            dir,
        })
    }

    /// Storage locations of machine `i` (savegames, network logs).
    pub(super) fn paths(&self, i: usize) -> NetPaths {
        net_paths(self.dir.path(), i)
    }

    /// Installation of machine `i`: the game file `game_path` changed by `edit` as a loose file of its own (shadows
    /// the archive entry, [`opennml_rda::testutil::with_loose_file`]); the other machines keep theirs. Call before
    /// the rules of a session are computed.
    pub(super) fn change_file(
        &mut self,
        i: usize,
        game_path: &str,
        edit: impl FnOnce(&mut Vec<u8>),
    ) {
        let run = &mut self.runs[i];
        let mut bytes = run.vfs.read(game_path).expect("game file");
        edit(&mut bytes);
        let disk = self.dir.path().join(format!("install-{i}"));
        std::fs::create_dir_all(&disk).expect("folder");
        let name = game_path.rsplit('/').next().expect("file name");
        let disk = disk.join(name);
        std::fs::write(&disk, &bytes).expect("loose file");
        run.vfs = opennml_rda::testutil::with_loose_file(&run.vfs, game_path, &disk);
    }

    /// Installation of machine `i` without the game file `game_path`: a loose file of its own that does not exist on
    /// disk shadows the archive entry, so reading it fails as for a file deleted after the program start (the `Vfs`
    /// keeps its index). The other machines keep theirs; the map catalog of the menu stays as loaded.
    pub(super) fn hide_file(&mut self, i: usize, game_path: &str) {
        let name = game_path.rsplit('/').next().expect("file name");
        let disk = self
            .dir
            .path()
            .join(format!("install-{i}"))
            .join("hidden")
            .join(name);
        assert!(!disk.exists(), "{disk:?} must not exist");
        let run = &mut self.runs[i];
        run.vfs = opennml_rda::testutil::with_loose_file(&run.vfs, game_path, &disk);
        assert!(run.vfs.read(game_path).is_err(), "{game_path} hidden");
    }

    pub(super) fn frame(&mut self) {
        self.net.advance_to(self.runs[0].now);
        for run in &mut self.runs {
            run.frame();
        }
    }

    pub(super) fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame();
        }
    }

    pub(super) fn frames_until(&mut self, what: &str, cond: impl Fn(&Self) -> bool) {
        for _ in 0..MAX_LAN_FRAMES {
            if cond(self) {
                return;
            }
            self.frame();
        }
        panic!("{what}: not reached after {MAX_LAN_FRAMES} frames");
    }

    /// Frames until the UI of every machine rests, and one more.
    pub(super) fn settle(&mut self) {
        let busy = |r: &Run| {
            let app = &r.host.app;
            app.menu().busy() || !app.queue().is_empty() || app.queue().waiting()
        };
        for _ in 0..300 {
            self.frame();
            if !self.runs.iter().any(busy) {
                break;
            }
        }
        self.frame();
    }

    pub(super) fn session(&self, i: usize) -> &NetSession {
        self.runs[i].flow.net_session().expect("network session")
    }

    pub(super) fn phase(&self, i: usize) -> NetPhase {
        self.session(i).phase()
    }

    pub(super) fn lobby(&self, i: usize) -> LobbyState {
        self.session(i).status().lobby.expect("lobby state")
    }

    /// Open dialogs of machine `i`, sorted.
    pub(super) fn dialogs(&self, i: usize) -> Vec<String> {
        let mut d = self.runs[i].dialogs();
        d.sort();
        d
    }

    pub(super) fn in_lobby(&self, i: usize) -> bool {
        let d = self.dialogs(i);
        LOBBY.iter().all(|l| d.iter().any(|x| x == l))
    }

    pub(super) fn click(&mut self, i: usize, dialog: &str, element: &str) {
        self.runs[i].click(dialog, element);
        self.settle();
    }

    /// Select row `row` of the list `list` (row height 22 px as in `UI_GOLDEN`).
    pub(super) fn list_row(&mut self, i: usize, dialog: &str, list: &str, row: usize) {
        let element = |run: &Run| {
            crate::ui_control::state_data(&run.host.app, super::tests::S1024, run.now)
                .elements
                .into_iter()
                .find(|e| e.dialog == dialog && e.name == list)
                .unwrap_or_else(|| panic!("{dialog}/{list} is missing"))
        };
        let e = element(&self.runs[i]);
        let [x0, y0, x1, _] = e.rect;
        let (x, y) = ((x0 + x1) / 2, y0 + 11 + 22 * row as i32);
        let run = &mut self.runs[i];
        let app = &mut run.host.app;
        app.input(UiInput::Move { x, y }, super::tests::S1024, run.now);
        app.input(UiInput::Down(UiButton::Left), super::tests::S1024, run.now);
        app.input(UiInput::Up(UiButton::Left), super::tests::S1024, run.now);
        self.settle();
        assert_eq!(
            element(&self.runs[i]).selection,
            Some(row as i32),
            "{dialog}/{list}: row {row} of {:?}",
            e.items
        );
    }

    /// Open `LAN1` (like "Multiplayer" in the main menu; the frame starts the search).
    pub(super) fn open_lan1(&mut self, i: usize) {
        self.runs[i].host.app.push_command(AppCmd::Open {
            name: "lan1".into(),
            wait: true,
        });
        self.settle();
        assert!(
            self.dialogs(i).contains(&"lan1".to_string()),
            "{:?}",
            self.dialogs(i)
        );
    }

    /// Machine `i` creates a match via `LAN1` -> `LAN3` (map with `map` in the name, otherwise the first of the list)
    /// and stands in the lobby.
    pub(super) fn host(&mut self, i: usize, map: Option<&str>) {
        self.host_by(i, |m| {
            map.is_none_or(|map| m.name.to_lowercase().contains(map))
        });
    }

    /// Like [`Self::host`] with the first map of the list for which `pick` holds.
    pub(super) fn host_by(
        &mut self,
        i: usize,
        pick: impl Fn(&opennml_ui::draft::MapCatalogEntry) -> bool,
    ) {
        self.open_lan1(i);
        self.click(i, "lan1", "create");
        assert!(
            self.dialogs(i).contains(&"lan3".to_string()),
            "{:?}",
            self.dialogs(i)
        );
        // The list of `LOADMULTIPLAYERMAP` shows the maps of the game mode in catalog order (`FUN_004BDD40`).
        let app = &self.runs[i].host.app;
        let maps = &app.data().maps;
        let row = maps
            .filtered(app.config().game_type)
            .into_iter()
            .position(|k| pick(&maps.maps[k]))
            .expect("matching map");
        if row != 0 {
            self.list_row(i, "loadmultiplayermap", "levels", row);
        }
        self.click(i, "lan3", "create");
        assert_eq!(self.phase(i), NetPhase::Lobby);
        assert!(self.in_lobby(i), "{:?}", self.dialogs(i));
    }

    /// Machine `i` searches in `LAN1`, selects the first game and clicks `JOIN`.
    pub(super) fn click_join(&mut self, i: usize) {
        self.open_lan1(i);
        // The list queries again every second (`timeout 1000`).
        self.frames_until("game in the list", |l| {
            let run = &l.runs[i];
            crate::ui_control::state_data(&run.host.app, super::tests::S1024, run.now)
                .elements
                .iter()
                .any(|e| e.dialog == "lan1" && e.name == "opengames" && !e.items.is_empty())
        });
        self.list_row(i, "lan1", "opengames", 0);
        self.click(i, "lan1", "join");
    }

    /// Like [`Self::click_join`], then up to the lobby and the verdict of the game data check on the own row (only
    /// with `Equal` does the core take `READY`).
    pub(super) fn join(&mut self, i: usize) {
        self.click_join(i);
        self.frames_until("lobby of the client", |l| {
            l.phase(i) == NetPhase::Lobby && l.in_lobby(i)
        });
        self.frames_until("verdict of the game data check", |l| {
            l.own_data(i).is_some_and(|d| d != DataState::Pending)
        });
        self.settle();
    }

    /// Verdict of the game data check on the own row of machine `i` in its last lobby state.
    pub(super) fn own_data(&self, i: usize) -> Option<DataState> {
        let status = self.session(i).status();
        let row = usize::from(status.own_row?);
        status.lobby?.rows.get(row).map(|r| r.data.clone())
    }

    /// Lobby request of the UI as from `LAN2`/`CHATFRAME` (`UiEffect::Lobby`).
    pub(super) fn wish(&mut self, i: usize, action: LobbyAction) {
        self.runs[i].apply(UiEffect::Lobby(action));
    }

    /// Row `row` ready (`READYn`).
    pub(super) fn ready(&mut self, i: usize, row: u8) {
        self.wish(i, LobbyAction::Ready { row, ready: true });
    }

    /// All ready, until `START` holds on the host.
    pub(super) fn all_ready(&mut self, rows: &[(usize, u8)]) {
        for &(i, row) in rows {
            self.ready(i, row);
        }
        self.frames_until("all ready", |l| {
            l.runs[0]
                .flow
                .net_session()
                .and_then(NetSession::driver)
                .and_then(NetDriver::host_core)
                .is_some_and(opennml_net::HostCore::can_start)
        });
    }
}

/// Number of the text of the open message `MSGBOX`, otherwise `None`.
fn message_id(run: &Run) -> Option<i32> {
    if !run.dialogs().iter().any(|d| d == "msgbox") {
        return None;
    }
    run.host.app.screen_state().info_text
}

/// The run shows the main menu `START` without lobby and without message.
fn at_start(n: &Lan, i: usize) -> bool {
    let d = n.dialogs(i);
    d.iter().any(|x| x == "start") && !d.iter().any(|x| x == "msgbox" || x == "lan2")
}

/// `{8}` (`0x004CA1B5`) on the host, `{7}` (`0x004CA0FA`) on the client: both have `LAN2`, `LEVELINFO`, `CHATFRAME`
/// open, the client stands in row 1 (E-282) and sees the state of the host; the session name is "Session - <name>".
// QUIRK E-300: deviation
#[test]
fn menu_host_and_join_reach_the_lobby() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    n.host(0, Some("tinte"));
    let session = n.lobby(0).session;
    assert_eq!(session.name, "Session - Anna", "session name");
    assert_eq!(
        name_text(&n.runs[0].host.app.config().net_session),
        "Session - Anna",
        "back to NetSession"
    );
    n.join(1);
    n.frames(10);
    let (host, client) = (n.lobby(0), n.lobby(1));
    assert_eq!(host, client, "state of the host on the client");
    assert_eq!(n.session(1).status().own_row, Some(1));
    assert_eq!(host.rows[1].name, "Bert");
    assert_eq!(host.rows[1].peer, Some(1));
    let view = n.runs[1]
        .host
        .app
        .data()
        .net
        .clone()
        .unwrap()
        .lobby
        .unwrap();
    assert!(!view.is_host);
    assert_eq!(view.own_row, 1);
    assert!(view.peers[0] && view.peers[1]);
    // Draft of the client from the state and the own catalog.
    let draft = &n.runs[1].host.app.data().draft;
    assert!(draft.info.path.to_lowercase().contains("tinte"));
    assert!(!draft.info.name.is_empty());
    assert_eq!(draft.slots[1].name, "Bert");
}

/// Element `dialog/name` of machine `i` in the last frame.
fn element(n: &Lan, i: usize, dialog: &str, name: &str) -> Option<opennml_control::UiElementData> {
    let run = &n.runs[i];
    crate::ui_control::state_data(&run.host.app, super::tests::S1024, run.now)
        .elements
        .into_iter()
        .find(|e| e.dialog == dialog && e.name == name)
}

/// `START` of the host is visible (code 5 again every second, re/multiplayer-menus L.3.4).
pub(super) fn start_shown(n: &Lan) -> bool {
    element(n, 0, "lan2", "start").is_some_and(|e| e.visible && !e.hidden)
}

/// No driver and no lobby any more: the session is closed (deviates from E-312).
fn session_closed(n: &Lan, i: usize) -> bool {
    let s = n.session(i);
    s.driver().is_none() && s.status().lobby.is_none()
}

/// Host Anna and client Bert in the lobby on the small map "Tintebariver".
fn lobby_pair() -> Option<Lan> {
    lobby_pair_with(|_, _| {})
}

fn lobby_pair_with(adjust: impl Fn(usize, &mut EngineConfig)) -> Option<Lan> {
    let mut n = Lan::corpus_with(&["Anna", "Bert"], adjust)?;
    n.host(0, Some("tinte"));
    n.join(1);
    Some(n)
}

/// E-279; deviates from E-312: The client has chosen the game, the host closes the lobby (`EXIT`), then `JOIN`: the
/// connection setup fails at the deadline, the client silently stands in `LAN1` again (no message), the session is
/// closed.
// QUIRK E-279: kept
// QUIRK E-312: deviation
#[test]
fn join_to_a_vanished_host_returns_to_lan1() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    n.host(0, Some("tinte"));
    n.open_lan1(1);
    n.frames_until("game in the list", |l| {
        element(l, 1, "lan1", "opengames").is_some_and(|e| !e.items.is_empty())
    });
    n.list_row(1, "lan1", "opengames", 0);
    n.click(0, "lan2", "exit");
    assert!(at_start(&n, 0), "{:?}", n.dialogs(0));
    assert_eq!(n.phase(0), NetPhase::Idle);
    n.click(1, "lan1", "join");
    assert_eq!(n.phase(1), NetPhase::Connecting);
    n.frames_until("join failed", |l| l.phase(1) != NetPhase::Connecting);
    n.settle();
    let d = n.dialogs(1);
    assert!(d.contains(&"lan1".to_string()), "{d:?}");
    assert!(!d.iter().any(|x| x == "msgbox" || x == "lan2"), "{d:?}");
    assert!(session_closed(&n, 1));
}

/// E-287 (`Reject` "The match is full"), E-279: Bert and Cara click `JOIN` at the same time on a match with two slots;
/// Bert (shorter delay) gets row 1, Cara is rejected and silently stands in `LAN1` again.
// QUIRK E-279, E-287: kept
// QUIRK E-312: deviation
#[test]
fn rejected_join_returns_silently() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert", "Cara"]) else {
        return;
    };
    n.host(0, Some("tinte"));
    for i in [1, 2] {
        n.open_lan1(i);
        n.frames_until("game in the list", |l| {
            element(l, i, "lan1", "opengames").is_some_and(|e| !e.items.is_empty())
        });
        n.list_row(i, "lan1", "opengames", 0);
    }
    n.runs[1].click("lan1", "join");
    n.runs[2].click("lan1", "join");
    n.frames_until("both joins decided", |l| {
        l.phase(1) == NetPhase::Lobby && l.phase(2) != NetPhase::Connecting
    });
    n.settle();
    assert!(n.in_lobby(1));
    let d = n.dialogs(2);
    assert!(d.contains(&"lan1".to_string()), "{d:?}");
    assert!(!d.iter().any(|x| x == "msgbox" || x == "lan2"), "{d:?}");
    assert!(session_closed(&n, 2));
    let rows = n.lobby(0).rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].name, "Bert");
}

/// Deviates from E-305: The host sets row 1 to "Open" (kick); the client shows 15212 exactly once, "OK" (`{0x2D}`)
/// leads to the main menu, the session is closed; on the host the row is open.
// QUIRK E-279: kept
// QUIRK E-305: deviation
#[test]
fn kicked_client_sees_15212_once() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    n.wish(0, LobbyAction::Kind { row: 1, kind: 0 });
    n.frames_until("message", |l| message_id(&l.runs[1]).is_some());
    n.settle();
    n.frames(120);
    assert_eq!(message_id(&n.runs[1]), Some(KICKED_TEXT));
    let opened = |run: &Run| run.dialogs().iter().filter(|d| *d == "msgbox").count();
    assert_eq!(opened(&n.runs[1]), 1);
    assert!(n.runs[1].host.app.queue().is_empty(), "no second message");
    let row = &n.lobby(0).rows[1];
    assert_eq!((row.kind, row.peer), (0, None));
    n.click(1, "msgbox", "ok");
    n.settle();
    assert!(at_start(&n, 1), "{:?}", n.dialogs(1));
    assert!(session_closed(&n, 1));
    assert_eq!(n.phase(1), NetPhase::Idle);
}

/// Deviates from E-311: The host leaves the lobby (`EXIT`); both clients show 15212 exactly once (`{0x2F}`: `{9}`,
/// message with `{0x2D}`), "OK" leads to the main menu.
// QUIRK E-279: kept
// QUIRK E-311: deviation
#[test]
fn host_exit_shows_15212_once_on_clients() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert", "Cara"]) else {
        return;
    };
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.join(2);
    n.click(0, "lan2", "exit");
    assert!(at_start(&n, 0), "{:?}", n.dialogs(0));
    n.frames_until("message on both", |l| {
        message_id(&l.runs[1]).is_some() && message_id(&l.runs[2]).is_some()
    });
    n.settle();
    n.frames(120);
    for i in [1, 2] {
        assert_eq!(message_id(&n.runs[i]), Some(KICKED_TEXT), "Client {i}");
        assert_eq!(
            n.runs[i]
                .dialogs()
                .iter()
                .filter(|d| *d == "msgbox")
                .count(),
            1
        );
        assert!(n.runs[i].host.app.queue().is_empty(), "Client {i}");
        assert!(session_closed(&n, i), "Client {i}");
        n.click(i, "msgbox", "ok");
        assert!(at_start(&n, i), "Client {i}: {:?}", n.dialogs(i));
    }
}

/// Both are ready (`START` visible); the client leaves the lobby (`EXIT`, E-285): it stands in the main menu without a
/// session, the host sees the row open, and `START` is hidden again.
#[test]
fn client_leaves_in_the_lobby() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    n.click(1, "lan2", "exit");
    assert!(at_start(&n, 1), "{:?}", n.dialogs(1));
    assert!(session_closed(&n, 1));
    n.frames_until("row open", |l| l.lobby(0).rows[1].peer.is_none());
    let row = &n.lobby(0).rows[1];
    assert_eq!((row.kind, row.ready), (0, false));
    n.frames_until("START hidden", |l| !start_shown(l));
    assert!(n.in_lobby(0));
    assert_eq!(n.phase(0), NetPhase::Lobby);
}

/// Track the simulation checksums `(tick, checksum)` of machine `i` up to tick `until`.
fn collect_lan(seq: &mut Vec<(u64, u64)>, run: &Run) {
    if let Some(sim) = run.view.sim() {
        collect(seq, sim);
    }
}

/// Both ready, click on `START` of the host (`{3,"lan2"}` ..., `{0x15}`, `{0x0D}` follow in the next frames).
fn start_from_lobby(n: &mut Lan) {
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
}

/// E-321: `START` from the menu; both load the map with `multiplayer.ldg`, compute 400 ticks with equal checksums, in
/// the raster `LanFast` (2/1) of the setting `NetTurnRaster` of the host.
// QUIRK E-321: kept
#[test]
fn start_from_the_menu_runs_equal_hashes() {
    let Some(mut n) = lobby_pair_with(|i, cfg| {
        if i == 0 {
            cfg.net_turn_raster = 2;
        }
    }) else {
        return;
    };
    assert_eq!(n.lobby(1).session.spacing, 2);
    start_from_lobby(&mut n);
    let mut seqs = [Vec::new(), Vec::new()];
    for _ in 0..MAX_LAN_FRAMES {
        if n.runs.iter().all(|r| r.view.steps() >= 400) {
            break;
        }
        n.frame();
        for (i, run) in n.runs.iter().enumerate() {
            collect_lan(&mut seqs[i], run);
        }
    }
    let prefix = |seq: &[(u64, u64)]| -> Vec<(u64, u64)> {
        seq.iter().copied().take_while(|&(s, _)| s <= 400).collect()
    };
    let host = prefix(&seqs[0]);
    assert_eq!(
        host.len(),
        401,
        "sequence of the host gapless up to tick 400"
    );
    assert_eq!(prefix(&seqs[1]), host, "equal checksums up to tick 400");
    for (i, run) in n.runs.iter().enumerate() {
        assert!(run.flow.running(), "machine {i}");
        assert!(
            !run.shown.is_empty() && run.shown.iter().all(|(ldg, _)| ldg == MULTIPLAYER_LDG),
            "machine {i}: {:?}",
            run.shown
        );
        let net = run.view.net().expect("driver in the match");
        assert_eq!(
            net.raster().raw(),
            RasterStage::LanFast.config().raw(),
            "machine {i}"
        );
        assert_eq!(n.phase(i), NetPhase::Match);
    }
    // The seed comes from the identity.
    assert_eq!(
        n.runs[1].view.last_setup.as_ref().map(|s| s.seed),
        Some(LAN_SEED)
    );
}

/// Successor list that `Go` brought to the clients `peers` (empty before `Go`).
fn go_lists(n: &Lan, peers: std::ops::RangeInclusive<usize>) -> Vec<Vec<opennml_net::Successor>> {
    peers
        .map(|i| {
            n.runs[i]
                .view
                .net()
                .and_then(NetDriver::client_core)
                .map(|c| c.successors().to_vec())
                .unwrap_or_default()
        })
        .collect()
}

/// Host and two clients from the menu; each client listens from the join on, the host probes them in the lobby, and
/// `Go` carries the successor list with both addresses (peers 1 and 2 with the IP of their machine).
#[test]
fn lobby_client_listens_and_is_reachable_in_go() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert", "Cara"]) else {
        return;
    };
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.join(2);
    n.all_ready(&[(0, 0), (1, 1), (2, 2)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.frames_until("Go on both clients", |l| {
        go_lists(l, 1..=2).iter().all(|g| !g.is_empty())
    });
    let lists: Vec<Vec<opennml_net::Successor>> = (1..=2)
        .map(|i| {
            n.runs[i]
                .view
                .net()
                .and_then(NetDriver::client_core)
                .expect("Client")
                .successors()
                .to_vec()
        })
        .collect();
    assert_eq!(lists[0], lists[1], "Go distributes the same list");
    assert_eq!(lists[0].len(), 2);
    for (s, peer) in lists[0].iter().zip([1u8, 2]) {
        assert_eq!(s.peer, peer);
        let addr = s.addr.expect("reachable");
        assert_ne!(addr.port(), 0);
    }
    let host = n.runs[0].view.net().and_then(NetDriver::host_core).unwrap();
    assert_eq!(host.successors(), lists[0]);
}

/// A client without a listener (`NetIdentity::listen` off, like `--net-no-listen`) reports no port; it stands without
/// an address in the list, and the start comes anyway (the host does not wait for a probe that does not exist).
#[test]
fn client_without_listener_is_unreachable() {
    let Some(mut n) = Lan::corpus_listening(&["Anna", "Bert", "Cara"], |i| i != 2, |_, _| {})
    else {
        return;
    };
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.join(2);
    n.all_ready(&[(0, 0), (1, 1), (2, 2)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.frames_until("Go on both clients", |l| {
        go_lists(l, 1..=2).iter().all(|g| !g.is_empty())
    });
    let list = n.runs[1]
        .view
        .net()
        .and_then(NetDriver::client_core)
        .expect("Client")
        .successors()
        .to_vec();
    assert_eq!(list.len(), 2);
    assert!(list[0].addr.is_some(), "Bert listens");
    assert_eq!(
        (list[1].peer, list[1].addr),
        (2, None),
        "Cara does not listen"
    );
}

/// `LockTeams` of the session: `LOCKTEAMS` in `LAN3` of the host applies in the match of both machines.
// QUIRK E-320: kept
#[test]
fn lock_teams_comes_from_the_session() {
    let Some(mut n) = lobby_pair_with(|i, cfg| {
        if i == 0 {
            cfg.lock_teams = 1;
        }
    }) else {
        return;
    };
    assert!(n.lobby(1).session.settings.lock_teams);
    start_from_lobby(&mut n);
    n.frames_until("both running", |l| l.runs.iter().all(|r| r.flow.running()));
    for (i, run) in n.runs.iter().enumerate() {
        assert!(run.flow.meta.as_ref().unwrap().lock_teams, "machine {i}");
    }
}

/// Inputs are free in search and lobby and only locked while the network session loads (from the start until `Go`).
#[test]
fn menu_input_locked_only_while_loading() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    n.open_lan1(1);
    assert_eq!(n.phase(1), NetPhase::Searching);
    assert!(n.runs[1].flow.accepts_input());
    n.host(0, Some("tinte"));
    assert!(n.runs[0].flow.accepts_input(), "lobby of the host");
    n.join(1);
    assert!(n.runs[1].flow.accepts_input(), "lobby of the client");
    start_from_lobby(&mut n);
    let mut locked = [false; 2];
    for _ in 0..MAX_LAN_FRAMES {
        for (i, flag) in locked.iter_mut().enumerate() {
            if n.phase(i) == NetPhase::Loading {
                assert!(!n.runs[i].flow.accepts_input(), "machine {i} loads");
                *flag = true;
            }
        }
        if (0..2).all(|i| n.phase(i) == NetPhase::Match) {
            break;
        }
        n.frame();
    }
    assert!(locked[0], "the host waits for Go after loading");
    for i in 0..2 {
        assert_eq!(n.phase(i), NetPhase::Match);
        assert!(n.runs[i].flow.accepts_input(), "machine {i} after Go");
    }
}

/// Both are ready, the host clicks `START`, the client leaves before `{0x0D}` is executed. `start_from_lobby` refuses,
/// the lobby opens again, and the draft shows all rows again - also the open ones that `{0x0D}`
/// (`remove_open_slots`) had removed - without the lobby state changing once more.
#[test]
fn failed_start_reopens_the_lobby_with_open_rows() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.runs[1].apply(UiEffect::Net(AppCmd::CloseNet));
    n.settle();
    n.frames(70);
    let rows = n.lobby(0).rows;
    assert_eq!(
        (rows[1].kind, rows[1].peer),
        (0, None),
        "row of the client open"
    );
    assert_eq!(n.phase(0), NetPhase::Lobby);
    assert!(n.in_lobby(0), "{:?}", n.dialogs(0));
    assert!(!n.runs[0].flow.running(), "no match");
    assert!(
        n.runs[0].shown.is_empty(),
        "no loading: {:?}",
        n.runs[0].shown
    );
    let draft = &n.runs[0].host.app.data().draft;
    assert_eq!(draft.slots.len(), rows.len(), "all rows in the draft");
    for (row, _) in rows.iter().enumerate().skip(1) {
        let key = row as i32 - 1;
        assert_eq!(draft.kind_of(key), 0, "row {row} open in the draft");
        let name = element(&n, 0, "lan2", &format!("name{row}")).expect("NAME");
        assert!(name.visible && !name.hidden, "row {row} visible: {name:?}");
    }
}

/// The host aborts after the client has loaded the map but before `Go`. The client does not stand still on the map:
/// it shows the message with the reason, the input is free for its "OK", and "OK" leads to the main menu; the network
/// session is closed afterwards.
// QUIRK E-273: kept
#[test]
fn host_abort_after_loading_before_go_returns_to_the_main_menu() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    start_from_lobby(&mut n);
    n.frames_until("host has loaded", |l| l.runs[0].flow.running());
    // From now on only the client computes: the host never hears its `Loaded`, `Go` does not come.
    let client_frame = |n: &mut Lan| {
        n.runs[1].frame();
        n.net.advance_to(n.runs[1].now);
    };
    for _ in 0..MAX_LAN_FRAMES {
        if n.runs[1].flow.running() {
            break;
        }
        client_frame(&mut n);
    }
    assert!(n.runs[1].flow.running(), "client has loaded");
    for _ in 0..10 {
        client_frame(&mut n);
    }
    assert_eq!(n.phase(1), NetPhase::Loading);
    assert!(!n.runs[1].flow.accepts_input(), "locked while loading");
    assert!(!n.runs[1].view.net().expect("driver in the match").started());
    n.runs[0]
        .view
        .net_mut()
        .expect("driver of the host")
        .leave("Check run: host aborts");
    for _ in 0..MAX_LAN_FRAMES {
        if message(&n.runs[1]).is_some() {
            break;
        }
        client_frame(&mut n);
    }
    let text = message(&n.runs[1]).expect("message on the client");
    assert!(text.contains("The host has ended the connection"), "{text}");
    assert!(n.runs[1].flow.accepts_input(), "input free for the OK");
    for _ in 0..200 {
        client_frame(&mut n);
    }
    n.runs[1].click("msgbox", "ok");
    for _ in 0..MAX_LAN_FRAMES {
        if in_main_menu(&n.runs[1]) {
            break;
        }
        client_frame(&mut n);
    }
    assert!(in_main_menu(&n.runs[1]), "{:?}", n.dialogs(1));
    assert_eq!(n.phase(1), NetPhase::Idle);
}

// ---------------------------------------------------------------------------------------------------------------
// Map change in the network session (host: `LobbyAction::ChangeGame`; client: game data check per session revision).
// The draft of `LAN31` is re-created here via `set_lobby_draft`, a missing or differing map via
// `HostCore::set_session`; differing game data via an installation of its own per machine ([`Lan::change_file`]).

use std::sync::{Arc, Mutex};

use crate::net_session::{map_available, map_checksum};
use opennml_net::{
    Beacon, Connecting, Link, LinkEvent, Listener, MapId, NetMessage, Seeker, Transport,
};
use opennml_ui::draft::{DraftSettings, MapCatalogEntry};

/// `Bye` reasons that a machine has sent.
type Byes = Arc<Mutex<Vec<String>>>;

/// `MemTransport` whose connections record the `Bye` reasons.
struct ByeTransport {
    inner: MemTransport,
    byes: Byes,
}

struct ByeConnecting {
    inner: Box<dyn Connecting>,
    byes: Byes,
}

struct ByeLink {
    inner: Box<dyn Link>,
    byes: Byes,
}

impl Transport for ByeTransport {
    fn listen(&mut self, port: u16) -> std::io::Result<Box<dyn Listener>> {
        self.inner.listen(port)
    }
    fn connect(&mut self, addr: std::net::SocketAddr, timeout_ms: u64) -> Box<dyn Connecting> {
        Box::new(ByeConnecting {
            inner: self.inner.connect(addr, timeout_ms),
            byes: Arc::clone(&self.byes),
        })
    }
    fn beacon(&mut self) -> std::io::Result<Box<dyn Beacon>> {
        self.inner.beacon()
    }
    fn seeker(&mut self) -> std::io::Result<Box<dyn Seeker>> {
        self.inner.seeker()
    }
    fn fork(&self) -> Box<dyn Transport> {
        Box::new(ByeTransport {
            inner: self.inner.clone(),
            byes: Arc::clone(&self.byes),
        })
    }
}

impl Connecting for ByeConnecting {
    fn poll(&mut self) -> Option<std::io::Result<Box<dyn Link>>> {
        let byes = Arc::clone(&self.byes);
        self.inner
            .poll()
            .map(|r| r.map(|inner| Box::new(ByeLink { inner, byes }) as Box<dyn Link>))
    }
}

impl Link for ByeLink {
    fn send(&mut self, msg: NetMessage) {
        if let NetMessage::Bye { reason } = &msg {
            self.byes.lock().expect("record").push(reason.clone());
        }
        self.inner.send(msg);
    }
    fn send_large(&mut self, id: u32, bytes: Vec<u8>) {
        self.inner.send_large(id, bytes);
    }
    fn poll(&mut self) -> Vec<LinkEvent> {
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

impl Lan {
    /// Machine `i` (name `name`) gets a new network session whose connections record the `Bye` reasons (call before
    /// the join).
    fn count_byes(&mut self, i: usize, name: &str) -> Byes {
        let byes = Byes::default();
        let identity = NetIdentity {
            hello: lan_hello(name),
            port: LAN_PORT,
            seed: Box::new(|| LAN_SEED),
            listen: true,
        };
        let transport = ByeTransport {
            inner: MemTransport::new(&self.net, 10 * i as u64),
            byes: Arc::clone(&byes),
        };
        let mut session = NetSession::new(Box::new(transport), identity, self.paths(i));
        session.set_fixed_clock(Some(NET_CLOCK));
        session.set_rules_worker(Box::new(InlineWorker::default()));
        self.runs[i].flow.set_net_session(session);
        byes
    }

    /// Core of the host (machine 0) in the lobby.
    fn host_core_mut(&mut self) -> &mut opennml_net::HostCore {
        self.runs[0]
            .flow
            .net_session_mut()
            .and_then(NetSession::driver_mut)
            .and_then(NetDriver::host_core_mut)
            .expect("core of the host")
    }

    /// First map of the catalog of machine `i` (with `.lvl` and `.wld`) for which `pick` holds.
    pub(super) fn catalog_map(
        &self,
        i: usize,
        pick: impl Fn(&MapCatalogEntry) -> bool,
    ) -> MapCatalogEntry {
        self.runs[i]
            .host
            .app
            .data()
            .maps
            .maps
            .iter()
            .find(|m| m.level_read && m.world_read && pick(m))
            .cloned()
            .expect("matching map in the catalog")
    }
}

/// Map description of a fixed map of the catalog (like `build_slots`/`copy_settings` in `LAN3`).
fn info_of(m: &MapCatalogEntry) -> LevelInfoData {
    LevelInfoData {
        name: m.name.clone(),
        path: m.entry.path.clone(),
        size_class: m.size_class,
        players: m.players(),
        level_type: 0,
        ..LevelInfoData::default()
    }
}

/// Deviates from E-281: `CHANGEGAME` of the host makes map, player count and session name of the draft the new session
/// (raster stays); the client in the lobby receives it, and the announcement that a searcher sees in `LAN1` carries it
/// afterwards (session name and map filter current).
// QUIRK E-281, E-322: deviation
#[test]
fn change_game_updates_the_announcement() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert", "Cara"]) else {
        return;
    };
    // Three slots: with Bert one is still free, otherwise the host does not answer the search.
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.open_lan1(2);
    n.frames_until("game in the list", |l| {
        !l.session(2).status().games.is_empty()
    });
    let old = n.lobby(0).session;
    assert_eq!(n.session(2).status().games[0].announcement.session, old);
    let MapId::Fixed { path: old_path, .. } = &old.map else {
        panic!("fixed map expected");
    };
    // The new map also with a free slot next to Bert (otherwise no answer to Cara).
    let next = n.catalog_map(0, |m| {
        !m.entry.path.eq_ignore_ascii_case(old_path) && (3..=8).contains(&m.players())
    });
    let app = &mut n.runs[0].host.app;
    let draft = app.data().draft.clone();
    app.set_lobby_draft(draft.slots, info_of(&next), draft.settings);
    let units: Vec<u16> = "New round".encode_utf16().collect();
    app.set_net_session(&units);
    n.wish(0, LobbyAction::ChangeGame);
    n.settle();
    let session = n.lobby(0).session;
    assert_eq!(session.name, "New round");
    assert_eq!(session.map_name, next.name);
    assert_eq!(usize::from(session.players), next.slots.len());
    assert_eq!((session.spacing, session.lead), (old.spacing, old.lead));
    let MapId::Fixed { path, checksum } = &session.map else {
        panic!("fixed map expected");
    };
    assert_eq!(path, &next.entry.path);
    assert_eq!(Some(*checksum), map_checksum(&n.runs[0].vfs, path));
    n.frames_until("new announcement", |l| {
        l.session(2)
            .status()
            .games
            .first()
            .is_some_and(|g| g.announcement.session == session)
    });
    assert_eq!(n.session(2).status().games[0].name, "New round");
    n.frames_until("state on the client", |l| l.lobby(1).session == session);
    assert!(n.in_lobby(1), "{:?}", n.dialogs(1));
    assert_eq!(n.phase(1), NetPhase::Lobby);
    assert_eq!(message_id(&n.runs[1]), None);
}

/// Host Anna and client Bert (recorded `Bye`s) in the lobby, both ready.
fn ready_pair() -> Option<(Lan, Byes)> {
    let mut n = Lan::corpus(&["Anna", "Bert"])?;
    let byes = n.count_byes(1, "Bert");
    n.host(0, Some("tinte"));
    n.join(1);
    n.all_ready(&[(0, 0), (1, 1)]);
    Some((n, byes))
}

/// The host switches the session in its core to the map `map` ("Other map"); one frame later the client's row is
/// `Pending` in the host's state.
fn switch_session(n: &mut Lan, map: MapId) {
    let mut s = n.lobby(0).session;
    s.map = map;
    s.map_name = "Other map".into();
    n.host_core_mut()
        .set_session(s)
        .expect("switch in the core");
    n.frame();
    assert_eq!(n.lobby(0).rows[1].data, DataState::Pending);
}

/// The client's row gets the verdict `data`, and the client stays in the lobby: no `Bye`, lobby open on both, the row
/// keeps its machine but loses its Ready flag, `READY` of the client is discarded and `START` stays locked.
fn assert_client_blocked(n: &mut Lan, byes: &Byes, data: &DataState) {
    n.frames_until("verdict on the host", |l| l.lobby(0).rows[1].data == *data);
    n.settle();
    n.frames(120);
    assert_eq!(
        n.own_data(1).as_ref(),
        Some(data),
        "the client sees its verdict"
    );
    assert!(n.in_lobby(1), "{:?}", n.dialogs(1));
    assert_eq!(n.phase(1), NetPhase::Lobby);
    assert!(byes.lock().unwrap().is_empty(), "no Bye");
    n.ready(1, 1);
    n.frames(30);
    let row = &n.lobby(0).rows[1];
    assert_eq!((row.peer, row.ready), (Some(1), false));
    assert_eq!(&row.data, data);
    assert!(!n.host_core_mut().can_start());
    assert!(n.in_lobby(0));
}

/// Deviates from E-322 (new form): the new fixed map is missing on the client. It reports `Missing` and stays in the
/// lobby; `START` stays locked until the host chooses another map or removes the client.
// QUIRK E-322: deviation
#[test]
fn client_without_the_new_map_stays_and_blocks_start() {
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    let map = MapId::Fixed {
        path: "multiplayer/gibtesnicht.lvl".into(),
        checksum: 1,
    };
    assert!(!map_available(&n.runs[1].vfs, &map));
    switch_session(&mut n, map);
    assert_client_blocked(&mut n, &byes, &DataState::Missing);
}

/// The same path, but a different map checksum on the host (the map differs): `Missing` like a missing map.
// QUIRK E-322: deviation
#[test]
fn changed_map_checksum_blocks_start() {
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    let MapId::Fixed { path, checksum } = n.lobby(0).session.map else {
        panic!("fixed map expected");
    };
    let map = MapId::Fixed {
        path,
        checksum: checksum ^ 1,
    };
    switch_session(&mut n, map);
    assert_client_blocked(&mut n, &byes, &DataState::Missing);
}

/// A random map is checked through its rules like every map: the client computes them, the row is `Equal` again, no
/// `Bye`, and the ready flags stay (E-302), so `START` holds.
// QUIRK E-322: deviation
#[test]
fn random_map_change_is_checked_by_rules() {
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    let map = MapId::Random {
        seed_text: "12345".into(),
        size: 0,
        players: 2,
        terrain: 0,
        landscape: 0,
        daytime: 0,
    };
    switch_session(&mut n, map.clone());
    n.frames_until("verdict on the host", |l| {
        l.lobby(0).rows[1].data == DataState::Equal
    });
    n.frames(120);
    assert_eq!(n.lobby(1).session.map, map);
    assert!(n.lobby(0).rules_checksum.is_some());
    assert!(n.in_lobby(1), "{:?}", n.dialogs(1));
    assert_eq!(n.phase(1), NetPhase::Lobby);
    assert!(byes.lock().unwrap().is_empty());
    assert_eq!(n.lobby(0), n.lobby(1));
    assert!(n.host_core_mut().can_start(), "ready flags stay (E-302)");
}

/// Deviates from E-322: the host changes from a map the client lacks to one it has: the row becomes `Equal` again, the
/// client can become ready, and `START` holds.
// QUIRK E-322: deviation
#[test]
fn host_change_to_another_map_clears_missing() {
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    let old = n.lobby(0).session.map;
    let missing = MapId::Fixed {
        path: "multiplayer/gibtesnicht.lvl".into(),
        checksum: 1,
    };
    switch_session(&mut n, missing);
    assert_client_blocked(&mut n, &byes, &DataState::Missing);
    switch_session(&mut n, old);
    n.frames_until("verdict on the host", |l| {
        l.lobby(0).rows[1].data == DataState::Equal
    });
    n.all_ready(&[(1, 1)]);
    assert!(byes.lock().unwrap().is_empty());
}

/// Game file of the rules of every map (bonus values of the objects).
pub(super) const BONUS_INI: &str = "data/gifcon/bonus.ini";

/// Map types, landscapes and game modes of the random maps (input of the generator).
const RANDOM_MAP_TYPES: &str = "data/gifcon/randommaptypes.ini";

/// Changes the last digit of the file (a digit of a value, so that the file still parses).
pub(super) fn bump_last_digit(bytes: &mut [u8]) {
    let at = bytes.iter().rposition(u8::is_ascii_digit).expect("a digit");
    bytes[at] = if bytes[at] == b'9' {
        b'8'
    } else {
        bytes[at] + 1
    };
}

/// Verdict of a client whose `bonus.ini` differs.
fn bonus_differs() -> DataState {
    DataState::Differs {
        files: vec![BONUS_INI.to_string()],
        more: 0,
    }
}

/// Host Anna and client Bert (recorded `Bye`s) on "Tintebariver", Bert with a changed `bonus.ini`; the host is ready.
fn bonus_pair() -> Option<(Lan, Byes)> {
    let mut n = Lan::corpus(&["Anna", "Bert"])?;
    let byes = n.count_byes(1, "Bert");
    n.change_file(1, BONUS_INI, |b| bump_last_digit(b));
    n.host(0, Some("tinte"));
    n.join(1);
    n.ready(0, 0);
    Some((n, byes))
}

/// Two installations of their own with the same game data (both with the same changed `bonus.ini`): the row becomes
/// `Equal`, `START` holds, and both compute the same up to tick 200.
#[test]
fn equal_installations_start_as_before() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    for i in 0..2 {
        n.change_file(i, BONUS_INI, |b| bump_last_digit(b));
    }
    n.host(0, Some("tinte"));
    n.join(1);
    assert_eq!(n.lobby(0).rows[1].data, DataState::Equal);
    let path = fixed_path(&n.lobby(0).session);
    start_from_lobby(&mut n);
    run_and_compare(&mut n, 200, &path);
}

/// Deviates from E-354: the client's `bonus.ini` differs from the host's. Its row names the file, the client stays in
/// the lobby and cannot become ready, `START` stays locked.
// QUIRK E-354: deviation
#[test]
fn changed_bonus_ini_blocks_start_and_names_the_file() {
    let Some((mut n, byes)) = bonus_pair() else {
        return;
    };
    assert_client_blocked(&mut n, &byes, &bonus_differs());
    assert!(n.lobby(0).rows[0].ready);
}

/// A map change to another map whose own files are the same on both still differs in the shared `bonus.ini`.
// QUIRK E-354: deviation
#[test]
fn map_change_to_a_clean_map_still_differs_for_shared_files() {
    let Some((mut n, byes)) = bonus_pair() else {
        return;
    };
    assert_client_blocked(&mut n, &byes, &bonus_differs());
    let old = fixed_path(&n.lobby(0).session);
    let next = n.catalog_map(0, |m| {
        !m.entry.path.eq_ignore_ascii_case(&old) && m.players() == 2
    });
    let checksum = map_checksum(&n.runs[0].vfs, &next.entry.path).expect("map checksum");
    assert_eq!(
        map_checksum(&n.runs[1].vfs, &next.entry.path),
        Some(checksum)
    );
    switch_session(
        &mut n,
        MapId::Fixed {
            path: next.entry.path.clone(),
            checksum,
        },
    );
    let revision = n.lobby(0).session_revision;
    assert_client_blocked(&mut n, &byes, &bonus_differs());
    assert_eq!(n.lobby(0).session_revision, revision);
    assert_eq!(fixed_path(&n.lobby(0).session), next.entry.path);
}

/// A random map whose generator data differ on the client (`randommaptypes.ini`: the map type of the session runs
/// another script there): the rules differ, the row names the file.
// QUIRK E-354: deviation
#[test]
fn random_map_with_other_generator_data_differs() {
    let Some(mut n) = Lan::corpus_with(&["Anna", "Bert"], |i, cfg| {
        if i == 0 {
            random_values(cfg, "Netz");
            cfg.level_type = 1;
        }
    }) else {
        return;
    };
    // The first map type (`terrain` 0 of the session) gets the script of the second.
    n.change_file(1, RANDOM_MAP_TYPES, |bytes| {
        // Byte by byte (the file is not UTF-8 throughout).
        let text: String = bytes.iter().map(|&b| char::from(b)).collect();
        let scripts: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("MSCRIPT "))
            .collect();
        assert!(scripts.len() >= 2, "{scripts:?}");
        let changed = text.replacen(
            &format!("MSCRIPT {}", scripts[0]),
            &format!("MSCRIPT {}", scripts[1]),
            1,
        );
        *bytes = changed.chars().map(|c| c as u8).collect();
    });
    n.host_random(0);
    n.join(1);
    let DataState::Differs { files, more } = n.lobby(0).rows[1].data.clone() else {
        panic!("Differs expected: {:?}", n.lobby(0).rows[1].data);
    };
    assert!(
        files.iter().any(|f| f == RANDOM_MAP_TYPES),
        "{files:?} and {more} more"
    );
    assert!(!n.lobby(0).rows[1].ready);
    n.ready(0, 0);
    n.ready(1, 1);
    n.frames(30);
    assert!(!n.host_core_mut().can_start());
    assert!(n.in_lobby(1), "{:?}", n.dialogs(1));
}

// --- Messages of the game data check and of a failed load (re/multiplayer-menus L.18) --------------------------------

use opennml_ui::own_texts::{DATA_DIFFERS, LOAD_FAILED, ROW_DIFFERS, fill};

/// Own text `id` of machine `i`, filled with `args`.
pub(super) fn own_text(n: &Lan, i: usize, id: i32, args: &[&str]) -> String {
    fill(&n.runs[i].host.layer.texts.display(id), args)
}

/// Host Anna and clients Bert and Carl on a map with three slots; the machines `changed` with a changed `bonus.ini`.
fn bonus_trio(changed: &[usize]) -> Option<Lan> {
    let mut n = Lan::corpus(&["Anna", "Bert", "Carl"])?;
    for &i in changed {
        n.change_file(i, BONUS_INI, |b| bump_last_digit(b));
    }
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.join(2);
    n.frames(30);
    n.settle();
    Some(n)
}

/// Bert's game data differ, Carl's do not: Bert and the host see one message each ("OK" without a follow-up command,
/// the lobby stays), Carl none.
#[test]
fn only_the_differing_client_and_the_host_see_a_message() {
    let Some(mut n) = bonus_trio(&[1]) else {
        return;
    };
    assert_eq!(n.lobby(0).rows[2].data, DataState::Equal);
    assert_eq!(message(&n.runs[2]), None, "Carl sees nothing");
    let host = own_text(&n, 0, ROW_DIFFERS, &["Bert", BONUS_INI]);
    assert_eq!(message(&n.runs[0]), Some(host));
    let bert = own_text(&n, 1, DATA_DIFFERS, &[BONUS_INI]);
    assert_eq!(message(&n.runs[1]), Some(bert));
    for i in 0..2 {
        n.click(i, "msgbox", "ok");
    }
    n.frames(120);
    for i in 0..3 {
        assert_eq!(message(&n.runs[i]), None, "machine {i}");
        assert!(n.in_lobby(i), "machine {i}: {:?}", n.dialogs(i));
        assert_eq!(n.phase(i), NetPhase::Lobby, "machine {i}");
    }
}

/// Bert's and Carl's game data differ: each sees its own message once; the host sees Bert's, after its "OK" Carl's,
/// then none; the lobby stays on all.
#[test]
fn host_sees_two_differing_clients_in_turn() {
    let Some(mut n) = bonus_trio(&[1, 2]) else {
        return;
    };
    for i in 1..3 {
        let own = own_text(&n, i, DATA_DIFFERS, &[BONUS_INI]);
        assert_eq!(message(&n.runs[i]), Some(own), "machine {i}");
    }
    for name in ["Bert", "Carl"] {
        let row = own_text(&n, 0, ROW_DIFFERS, &[name, BONUS_INI]);
        assert_eq!(message(&n.runs[0]), Some(row));
        n.click(0, "msgbox", "ok");
    }
    for i in 1..3 {
        n.click(i, "msgbox", "ok");
    }
    n.frames(120);
    for i in 0..3 {
        assert_eq!(message(&n.runs[i]), None, "machine {i}");
        assert!(n.in_lobby(i), "machine {i}: {:?}", n.dialogs(i));
    }
    assert!(!n.host_core_mut().can_start());
}

/// Deviates from E-352: loading fails despite the check (Bert's `bonus.ini` changes after the verdict `Equal`). The
/// host names the machine to everyone: host, Carl and Bert himself show 99 113 with "Bert" (the reason stays in the
/// log); "OK" leads each to the main menu, the sessions are closed, nobody panics.
// QUIRK E-352: deviation
#[test]
fn load_failure_names_the_machine_for_everyone() {
    let Some(mut n) = bonus_trio(&[]) else {
        return;
    };
    for i in 1..3 {
        assert_eq!(n.lobby(0).rows[i].data, DataState::Equal, "row {i}");
    }
    n.change_file(1, BONUS_INI, |b| bump_last_digit(b));
    n.all_ready(&[(0, 0), (1, 1), (2, 2)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.frames_until("message on all", |l| {
        l.runs.iter().all(|r| message(r).is_some())
    });
    n.settle();
    for i in 0..3 {
        let expected = own_text(&n, i, LOAD_FAILED, &["Bert"]);
        assert_eq!(message(&n.runs[i]), Some(expected), "machine {i}");
    }
    assert!(!n.runs[1].flow.running(), "Bert has not loaded");
    for run in &mut n.runs {
        run.click("msgbox", "ok");
    }
    n.frames_until("main menu on all", |l| l.runs.iter().all(in_main_menu));
    for i in 0..3 {
        assert_eq!(n.phase(i), NetPhase::Idle, "machine {i}");
        assert!(session_closed(&n, i), "machine {i}");
        assert!(!n.runs[i].flow.running(), "machine {i}: no match");
        assert_eq!(n.runs[i].host.app.data().accounting, None, "machine {i}");
    }
}

/// The host cleans the computer names of the rows when creating them (`[`/`]` are dropped); host and client show the
/// same name.
#[test]
fn computer_names_are_cleaned_in_the_lobby() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    let m = n.catalog_map(0, |m| m.players() >= 3);
    let mut slots = m.slots.clone();
    for s in &mut slots {
        match s.key {
            -1 => {
                s.kind = 999;
                s.name = "Anna".into();
            }
            1 => {
                s.kind = 2;
                s.handicap = 2;
                s.name = "[KI] Hans".into();
            }
            _ => {}
        }
    }
    n.runs[0]
        .host
        .app
        .set_lobby_draft(slots, info_of(&m), DraftSettings::default());
    n.runs[0].apply(UiEffect::Net(AppCmd::Host));
    n.settle();
    assert_eq!(n.phase(0), NetPhase::Lobby);
    assert!(n.in_lobby(0), "{:?}", n.dialogs(0));
    let host = n.lobby(0);
    assert_eq!(
        (host.rows[2].kind, host.rows[2].name.as_str()),
        (2, "KI Hans")
    );
    n.join(1);
    n.frames(10);
    let (host, client) = (n.lobby(0), n.lobby(1));
    assert_eq!(host, client);
    assert_eq!(client.rows[2].name, "KI Hans");
    assert_eq!(client.rows[1].name, "Bert");
    let draft = &n.runs[1].host.app.data().draft;
    assert_eq!(draft.slot(1).map(|s| s.name.as_str()), Some("KI Hans"));
}

// ---------------------------------------------------------------------------------------------------------------
// "Change game" from the menu: `LAN2 CHANGEGAME` -> `LAN31` -> `CHANGEGAME` or `EXITCHANGE`, start of the host from the
// session.

use opennml_net::{HostEdit, SessionInfo};
use opennml_ui::draft::level_infos;

/// "Change game" (`lan31.dlg`) and its map list.
pub(super) const CHANGE: &str = "lan31";
const MAP_LIST: &str = "loadmultiplayermap";

impl Lan {
    /// The host (machine 0) clicks `CHANGEGAME` in `LAN2`: its lobby is closed, `LAN31` with the map list open.
    pub(super) fn open_change(&mut self) {
        self.frames_until("CHANGEGAME visible", |l| {
            element(l, 0, "lan2", "changegame").is_some_and(|e| e.visible && !e.hidden)
        });
        self.click(0, "lan2", "changegame");
        let d = self.dialogs(0);
        assert!(
            d.iter().any(|x| x == CHANGE) && d.iter().any(|x| x == MAP_LIST),
            "{d:?}"
        );
        assert!(!LOBBY.iter().any(|l| d.iter().any(|x| x == l)), "{d:?}");
    }

    /// The host clicks `CHANGEGAME` in `LAN31` and the switch succeeds: `LAN31`, map list and `RANDOMMAP` are closed,
    /// the lobby of the host is open again, no message.
    pub(super) fn change_game(&mut self) {
        self.click(0, CHANGE, "changegame");
        let d = self.dialogs(0);
        assert!(self.in_lobby(0), "{d:?}");
        assert!(
            !d.iter()
                .any(|x| x == CHANGE || x == MAP_LIST || x == "randommap"),
            "{d:?}"
        );
        assert_eq!(message_id(&self.runs[0]), None);
    }

    /// Row of the map list of machine `i` (filtered by its game mode) whose map satisfies `pick`.
    pub(super) fn map_row(&self, i: usize, pick: impl Fn(&MapCatalogEntry) -> bool) -> usize {
        let app = &self.runs[i].host.app;
        let maps = &app.data().maps;
        maps.filtered(app.config().game_type)
            .into_iter()
            .position(|k| pick(&maps.maps[k]))
            .expect("matching map in the list")
    }

    /// Select entry `row` of the combo box `dialog/combo` of machine `i`: open it, then the row of the list in the
    /// popup (row height 21 px as in `UI_GOLDEN`).
    pub(super) fn combo_row(&mut self, i: usize, dialog: &str, combo: &str, row: usize) {
        use opennml_ui::tree::NodeKind;
        // Wait until the double-click time has passed, otherwise opening counts as the second click of the previous
        // one.
        let timeout =
            u32::try_from(self.runs[i].host.app.config().double_click_timeout).unwrap_or(0);
        self.frames(timeout / 16 + 1);
        self.click(i, dialog, combo);
        let run = &mut self.runs[i];
        let tree = run.host.app.tree();
        let key = format!("{}{}", dialog.to_uppercase(), combo.to_uppercase());
        let id = tree
            .find(&key)
            .unwrap_or_else(|| panic!("{key} is missing"));
        let NodeKind::Button(b) = &tree.node(id).kind else {
            panic!("{key} is not a combo box");
        };
        let popup = b.popup.expect("list open");
        let NodeKind::ComboPopup(p) = &tree.node(popup).kind else {
            panic!("no popup");
        };
        let list = p.list_box.expect("list in the popup");
        let [x0, y0, x1, _] =
            opennml_ui::coords::to_screen_rect(super::tests::S1024, tree.client_rect(list));
        let (x, y) = ((x0 + x1) / 2, y0 + 10 + 21 * row as i32);
        let app = &mut run.host.app;
        app.input(UiInput::Move { x, y }, super::tests::S1024, run.now);
        app.input(UiInput::Down(UiButton::Left), super::tests::S1024, run.now);
        app.input(UiInput::Up(UiButton::Left), super::tests::S1024, run.now);
        self.settle();
        assert_eq!(
            element(self, i, dialog, combo).and_then(|e| e.selection),
            Some(row as i32),
            "{dialog}/{combo}: row {row}"
        );
    }
}

/// Path of the fixed map of a session.
fn fixed_path(session: &SessionInfo) -> String {
    match &session.map {
        MapId::Fixed { path, .. } => path.clone(),
        MapId::Random { .. } | MapId::Saved { .. } => panic!("fixed map expected"),
    }
}

/// After `START`: frames up to tick `steps` on both machines; the checksum sequences are equal and gapless, and both
/// have loaded the map `path`.
fn run_and_compare(n: &mut Lan, steps: u64, path: &str) {
    let mut seqs = [Vec::new(), Vec::new()];
    for _ in 0..MAX_LAN_FRAMES {
        if n.runs.iter().all(|r| r.view.steps() >= steps) {
            break;
        }
        n.frame();
        for (i, run) in n.runs.iter().enumerate() {
            collect_lan(&mut seqs[i], run);
        }
    }
    let prefix = |seq: &[(u64, u64)]| -> Vec<(u64, u64)> {
        seq.iter()
            .copied()
            .take_while(|&(s, _)| s <= steps)
            .collect()
    };
    let host = prefix(&seqs[0]);
    assert_eq!(host.len() as u64, steps + 1, "sequence of the host gapless");
    assert_eq!(prefix(&seqs[1]), host, "equal checksums up to tick {steps}");
    for (i, run) in n.runs.iter().enumerate() {
        let level = &run.view.sim().expect("match").rules.level_path;
        assert!(
            level.eq_ignore_ascii_case(path),
            "machine {i}: {level} instead of {path}"
        );
    }
}

/// Deviates from E-281: The host opens `LAN31` (the list stands on the session map), selects another map, switches to
/// random map and closes with `EXITCHANGE`: the session stays, both load the session map at `START` and compute
/// equally up to tick 200 (the host starts from the session).
// QUIRK E-281: deviation
#[test]
fn exitchange_keeps_the_session_map() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    let session = n.lobby(0).session;
    let path = fixed_path(&session);
    n.open_change();
    let at_session = n.map_row(0, |m| m.entry.path.eq_ignore_ascii_case(&path));
    assert_eq!(
        element(&n, 0, MAP_LIST, "levels").and_then(|e| e.selection),
        Some(at_session as i32),
        "preselection of the session map"
    );
    let other = n.map_row(0, |m| {
        !m.entry.path.eq_ignore_ascii_case(&path) && m.level_read && m.world_read
    });
    n.list_row(0, MAP_LIST, "levels", other);
    assert!(
        !n.runs[0]
            .host
            .app
            .data()
            .level
            .info
            .path
            .eq_ignore_ascii_case(&path),
        "other map selected"
    );
    n.combo_row(0, CHANGE, "leveltype", 1);
    assert_eq!(n.runs[0].host.app.config().level_type, 1);
    assert!(
        n.dialogs(0).iter().any(|d| d == "randommap"),
        "{:?}",
        n.dialogs(0)
    );
    n.click(0, CHANGE, "exitchange");
    let d = n.dialogs(0);
    assert!(n.in_lobby(0), "{d:?}");
    assert!(
        !d.iter()
            .any(|x| x == CHANGE || x == MAP_LIST || x == "randommap"),
        "{d:?}"
    );
    assert_eq!(n.lobby(0).session, session, "the session stays");
    n.frames(10);
    assert_eq!(n.lobby(1).session, session);
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    // Also a draft of the host with another map (like `Info` after browsing in the original) does not start - the
    // request comes from the session.
    let stale = n.catalog_map(0, |m| !m.entry.path.eq_ignore_ascii_case(&path));
    let app = &mut n.runs[0].host.app;
    let draft = app.data().draft.clone();
    app.set_lobby_draft(draft.slots, info_of(&stale), draft.settings);
    n.runs[0].click("lan2", "start");
    run_and_compare(&mut n, 200, &path);
}

/// `CHANGEGAME` from the menu distributes the new map; `LAN31` is closed, the lobby of the host open again; the client
/// shows the new map in `LEVELINFO`, the ready flags stay (E-302), and `START` loads the new map on both machines with
/// equal checksums.
// QUIRK E-281: deviation
#[test]
fn change_game_from_the_menu_reaches_the_client() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    n.all_ready(&[(0, 0), (1, 1)]);
    let old = n.lobby(0).session;
    let old_path = fixed_path(&old);
    n.open_change();
    let row = n.map_row(0, |m| {
        !m.entry.path.eq_ignore_ascii_case(&old_path) && m.level_read && m.world_read
    });
    n.list_row(0, MAP_LIST, "levels", row);
    let next = n.runs[0].host.app.data().level.info.clone();
    n.click(0, CHANGE, "changegame");
    let d = n.dialogs(0);
    assert!(n.in_lobby(0), "{d:?}");
    assert!(!d.iter().any(|x| x == CHANGE || x == MAP_LIST), "{d:?}");
    assert_eq!(message_id(&n.runs[0]), None);
    let session = n.lobby(0).session;
    assert!(fixed_path(&session).eq_ignore_ascii_case(&next.path));
    assert_eq!(session.map_name, next.name);
    assert_eq!(i32::from(session.players), next.players);
    assert_eq!((session.spacing, session.lead), (old.spacing, old.lead));
    n.frames_until("state on the client", |l| l.lobby(1).session == session);
    n.settle();
    let client = &n.runs[1];
    let draft = &client.host.app.data().draft;
    assert_eq!(draft.info.name, next.name);
    assert!(draft.info.path.eq_ignore_ascii_case(&next.path));
    // `[CURLEVELINFOS]` computes `LEVELINFO` from the draft.
    let text = level_infos(draft, None, &client.host.layer.texts);
    assert!(text.contains(&next.name), "LEVELINFO of the client: {text}");
    let rows = n.lobby(0).rows;
    assert!(rows[0].ready && rows[1].ready, "ready flags stay");
    assert!(n.host_core_mut().can_start());
    start_from_lobby(&mut n);
    run_and_compare(&mut n, 200, &next.path);
}

/// Adopts the `LAN31` part of E-310: As long as `LAN31` is open, the chat of the host is closed; the core keeps
/// collecting the lines of the clients, after `EXITCHANGE` `CHATFRAME` of the host shows them.
// QUIRK E-310: deviation
#[test]
fn host_chat_is_closed_while_lan31_is_open() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    n.open_change();
    assert!(!n.dialogs(0).iter().any(|d| d == "chatframe"));
    n.wish(1, LobbyAction::Chat("Hello Anna".into()));
    n.frames_until("chat on the host", |l| {
        l.session(0)
            .status()
            .chat
            .iter()
            .any(|c| c.contains("Hello Anna"))
    });
    n.settle();
    assert!(
        n.dialogs(0).iter().any(|d| d == CHANGE),
        "{:?}",
        n.dialogs(0)
    );
    assert!(!n.dialogs(0).iter().any(|d| d == "chatframe"));
    n.click(0, CHANGE, "exitchange");
    assert!(n.in_lobby(0), "{:?}", n.dialogs(0));
    let chat = n.runs[0].host.app.screen_state().lobby_chat.get();
    assert!(chat.contains("Hello Anna"), "{chat}");
}

/// If the core occupies a row that falls away between the check in `LAN31` and the switch (here a computer that the
/// host sets in the same frame, like with a join), it refuses: message 15402, `LAN31` stays open, lobby state and
/// session stay; "OK" leaves `LAN31` standing.
// QUIRK E-307: deviation
#[test]
fn refused_change_keeps_lan31_open() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    let old = n.lobby(0).session;
    let old_path = fixed_path(&old);
    n.open_change();
    let row = n.map_row(0, |m| {
        m.players() == 2
            && !m.entry.path.eq_ignore_ascii_case(&old_path)
            && m.level_read
            && m.world_read
    });
    n.list_row(0, MAP_LIST, "levels", row);
    n.host_core_mut()
        .host_edit(HostEdit::Kind { row: 2, kind: 2 })
        .expect("computer on row 2");
    n.click(0, CHANGE, "changegame");
    assert_eq!(
        message_id(&n.runs[0]),
        Some(opennml_ui::screens::CHANGE_GAME_REFUSED)
    );
    assert!(
        n.dialogs(0).iter().any(|d| d == CHANGE),
        "{:?}",
        n.dialogs(0)
    );
    assert!(!n.in_lobby(0));
    assert_eq!(n.lobby(0).session, old);
    assert_eq!(n.lobby(0).rows[2].kind, 2);
    n.click(0, "msgbox", "ok");
    let d = n.dialogs(0);
    assert!(
        d.iter().any(|x| x == CHANGE) && !d.iter().any(|x| x == "msgbox"),
        "{d:?}"
    );
    n.frames(10);
    assert_eq!(n.lobby(1).session, old);
}

// ---------------------------------------------------------------------------------------------------------------
// Random map in the network: `LAN3 CREATE` and `LAN31 CHANGEGAME` with `LevelType` 1, the host reads the rules of the
// random map at the start ([`crate::net_session::net_rules`]), both load them from the start package (`random_map`)
// with `randommap.ldg`.

use crate::net_session::net_rules;
use opennml_ui::profiles::name_field;
use opennml_world::environment::RANDOM_MAP_PREFIX;

/// Values of the random map in the settings: small, two players, map type/landscape/time of day 0, seed `seed` (empty:
/// `CREATE`/`CHANGEGAME` draw it), game mode 0 (random maps only for game modes 0 to 3).
pub(super) fn random_values(cfg: &mut EngineConfig, seed: &str) {
    cfg.game_type = 0;
    cfg.random_map_seed = name_field(seed);
    cfg.random_map_size = 0;
    cfg.random_players = 0;
    cfg.random_terrain = 0;
    cfg.random_landscape = 0;
    cfg.random_daytime = 0;
}

impl Lan {
    /// Machine `i` creates a match on a random map via `LAN1` -> `LAN3` with `LevelType` 1 (from its settings; the
    /// frame opens `RANDOMMAP`) and stands in the lobby.
    pub(super) fn host_random(&mut self, i: usize) {
        assert_eq!(self.runs[i].host.app.config().level_type, 1);
        self.open_lan1(i);
        self.click(i, "lan1", "create");
        let d = self.dialogs(i);
        assert!(
            d.iter().any(|x| x == "lan3") && d.iter().any(|x| x == "randommap"),
            "{d:?}"
        );
        self.click(i, "lan3", "create");
        assert_eq!(self.phase(i), NetPhase::Lobby);
        assert!(self.in_lobby(i), "{:?}", self.dialogs(i));
    }
}

/// Both machines have loaded their match with the loading screen `randommap.ldg` (never `multiplayer.ldg`).
fn assert_random_ldg(n: &Lan) {
    for (i, run) in n.runs.iter().enumerate() {
        assert!(
            !run.shown.is_empty() && run.shown.iter().all(|(ldg, _)| ldg == RANDOM_MAP_LDG),
            "machine {i}: {:?}",
            run.shown
        );
    }
}

/// Adopts E-313: The host creates a random map in `LAN3` (session `MapId::Random`, map name = seed text, player count
/// `RandomPlayers + 2`), the client sees it in the list, joins and shows it in `LEVELINFO`; `START` loads the random
/// map on both with `randommap.ldg`, the checksums are equal up to tick 200.
// QUIRK E-313: kept
// QUIRK E-314: deviation
#[test]
fn random_map_from_the_menu_runs_equal_hashes() {
    let Some(mut n) = Lan::corpus_with(&["Anna", "Bert"], |i, cfg| {
        if i == 0 {
            random_values(cfg, "Netz");
            cfg.level_type = 1;
        }
    }) else {
        return;
    };
    n.host_random(0);
    let session = n.lobby(0).session;
    assert_eq!(
        session.map,
        MapId::Random {
            seed_text: "Netz".into(),
            size: 0,
            players: 2,
            terrain: 0,
            landscape: 0,
            daytime: 0,
        }
    );
    assert_eq!((session.map_name.as_str(), session.players), ("Netz", 2));
    assert_eq!(n.lobby(0).rows.len(), 2);
    n.join(1);
    n.frames(10);
    assert_eq!(n.lobby(1).session, session);
    let client = &n.runs[1];
    let draft = &client.host.app.data().draft;
    assert_eq!(
        (draft.info.level_type, draft.info.name.as_str()),
        (1, "Netz")
    );
    let text = level_infos(draft, None, &client.host.layer.texts);
    let label = client.host.layer.texts.display(15659);
    assert!(
        text.contains(&label) && text.contains("Netz"),
        "LEVELINFO of the client: {text}"
    );
    start_from_lobby(&mut n);
    run_and_compare(&mut n, 200, &format!("{RANDOM_MAP_PREFIX}Netz"));
    assert_random_ldg(&n);
    for run in &n.runs {
        assert!(run.view.sim().unwrap().rules.random_map.is_some());
    }
}

/// Adopts E-313: `CHANGEGAME` to a random map with an empty seed - the host draws the seed text (first draw "41", not
/// into `RandomMapSeed`), the session becomes `MapId::Random`, the rows and ready flags stay; the client receives it,
/// and `START` loads the random map on both with equal checksums.
// QUIRK E-313: kept
// QUIRK E-314: deviation
#[test]
fn change_to_a_random_map_runs_equal_hashes() {
    let Some(mut n) = lobby_pair_with(|i, cfg| {
        if i == 0 {
            random_values(cfg, "");
        }
    }) else {
        return;
    };
    n.all_ready(&[(0, 0), (1, 1)]);
    let old = n.lobby(0).session;
    let rows = n.lobby(0).rows;
    n.open_change();
    n.combo_row(0, CHANGE, "leveltype", 1);
    assert!(n.dialogs(0).iter().any(|d| d == "randommap"));
    n.change_game();
    let session = n.lobby(0).session;
    assert_eq!(
        session.map,
        MapId::Random {
            seed_text: "41".into(),
            size: 0,
            players: 2,
            terrain: 0,
            landscape: 0,
            daytime: 0,
        }
    );
    assert_eq!(session.map_name, "41");
    assert_eq!((session.spacing, session.lead), (old.spacing, old.lead));
    assert_eq!(n.lobby(0).rows, rows, "rows and ready flags stay");
    assert_eq!(
        name_text(&n.runs[0].host.app.config().random_map_seed),
        "",
        "RandomMapSeed stays empty"
    );
    n.frames_until("state on the client", |l| l.lobby(1).session == session);
    n.settle();
    assert!(n.in_lobby(1), "{:?}", n.dialogs(1));
    let draft = &n.runs[1].host.app.data().draft;
    assert_eq!((draft.info.level_type, draft.info.name.as_str()), (1, "41"));
    start_from_lobby(&mut n);
    run_and_compare(&mut n, 200, &format!("{RANDOM_MAP_PREFIX}41"));
    assert_random_ldg(&n);
}

/// Deviates from E-314: If the rules checksum of the start package of a random map differs from that of the map
/// generated on the client (different generator data), the match does not start - like
/// [`start_net_match_rejects_foreign_rules`]: `Loaded { error }`, no match. The host path [`net_rules`] builds the
/// rules from the request.
// QUIRK E-314: deviation
#[test]
fn random_map_with_other_rules_is_rejected() {
    let Some(mut run) = Run::corpus() else {
        return;
    };
    run.view.real_levels = true;
    let req = MatchRequest {
        level: String::new(),
        game_type: 0,
        slots: Vec::new(),
        settings: DraftSettings::default(),
        info: LevelInfoData {
            name: "77".into(),
            players: 2,
            level_type: 1,
            ..LevelInfoData::default()
        },
    };
    let rules = net_rules(&run.vfs, &req).expect("rules of the random map");
    assert!(rules.random_map.is_some());
    let active = crate::match_request::active_slots(&rules);
    let setup = MatchSetup {
        network: true,
        seed: 5,
        ..MatchSetup::default_for(&rules)
    };
    let info = net_start_info(&rules, rules.checksum ^ 1, setup, active[1]);
    assert!(
        info.package.random_map.is_some(),
        "start package with random_map"
    );
    let mut start = NetStart::new(info);
    let driver = start.driver.take().unwrap();
    let failure = run_net_start(&mut run, driver).expect_err("foreign generator data");
    assert!(
        failure.error.contains("rules checksum"),
        "{}",
        failure.error
    );
    assert!(
        run.shown.iter().all(|(ldg, _)| ldg == RANDOM_MAP_LDG),
        "{:?}",
        run.shown
    );
    assert!(!run.flow.running(), "no match");
    assert!(run.view.game.is_none());
    let mut driver = failure.net;
    assert_eq!(
        start.loaded_reply(&mut driver),
        Some(Some(failure.error.clone())),
        "Loaded with reason"
    );
}

// --- Network game menu `INGAMEOPTIONS2` and diplomacy in the network (re/multiplayer-menus L.7) ---------------

pub(super) const NET_MENU: &str = "ingameoptions2";

impl NetRuns {
    /// ESC on machine `i`, until `INGAMEOPTIONS2` is open and drawn (the menu opens in a frame with a tick).
    fn open_net_menu(&mut self, i: usize) {
        self.runs[i].key("ESCAPE");
        self.frames_until(|n| n.runs[i].dialogs().iter().any(|d| d == NET_MENU));
        self.settle();
        assert_eq!(
            self.runs[i].state(),
            2,
            "state 2 (FUN_0042E150 with network)"
        );
    }

    /// Frames until all machines have computed `steps` ticks; then equal checksums at the smallest common tick.
    fn run_to_and_compare(&mut self, steps: u64) {
        self.frames_until(|n| (0..3).all(|i| n.steps(i) >= steps));
        let at = (0..3).map(|i| self.steps(i)).min().unwrap();
        let hash = self.sim(0).hash_at(at);
        assert!(hash.is_some(), "checksum at {at}");
        for i in 1..3 {
            assert_eq!(self.sim(i).hash_at(at), hash, "machine {i} at {at}");
        }
    }
}

impl Lan {
    /// ESC on machine `i`, until `INGAMEOPTIONS2` is open and drawn (state 2, like [`NetRuns::open_net_menu`]).
    pub(super) fn escape(&mut self, i: usize) {
        self.runs[i].key("ESCAPE");
        self.frames_until("INGAMEOPTIONS2 open", |l| {
            l.runs[i].dialogs().iter().any(|d| d == NET_MENU)
        });
        self.settle();
        assert_eq!(
            self.runs[i].state(),
            2,
            "state 2 (FUN_0042E150 with network)"
        );
    }

    /// Diplomacy key on machine `i` (`UiRequest::ToggleDiplomacy`), then until the UI rests.
    pub(super) fn toggle_diplomacy(&mut self, i: usize) {
        let run = &mut self.runs[i];
        run.view.ui_requests.push(UiRequest::ToggleDiplomacy);
        poke_net(run);
        self.settle();
    }
}

/// Input to the match of the run without a left click: fetches the requests of the match like every input
/// (`UiRequest::ToggleDiplomacy` among others).
fn poke_net(run: &mut Run) {
    let ev = InputEvent::CursorMoved { x: 5.0, y: 5.0 };
    run.flow.input(
        &ev,
        super::tests::S1024,
        run.now,
        &mut run.host.app,
        &mut run.view,
    );
}

/// E-315, O-143, re/multiplayer-menus L.7.1: ESC on the client opens `INGAMEOPTIONS2` (state 2), the ticks keep running
/// on all machines, on the host also with its own open menu, and all compute equally. The network role comes from the
/// session (`GameFlow::started`).
// QUIRK E-315: kept
#[test]
fn net_menu_keeps_the_rounds_running() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    let roles: Vec<_> = n
        .runs
        .iter()
        .map(|r| r.host.app.game().unwrap().net)
        .collect();
    assert_eq!(
        roles,
        [
            Some(NetRoleView::Host),
            Some(NetRoleView::Client),
            Some(NetRoleView::Client)
        ]
    );
    n.frames_until(|n| n.steps(1) >= 10);
    n.open_net_menu(1);
    let before: Vec<u64> = (0..3).map(|i| n.steps(i)).collect();
    n.run_to_and_compare(before[1] + 60);
    assert_eq!(n.runs[1].state(), 2, "menu stays open");
    assert!(n.runs[1].dialogs().iter().any(|d| d == NET_MENU));
    for (i, &b) in before.iter().enumerate() {
        assert!(n.steps(i) >= b + 60, "machine {i} keeps computing");
    }
    // The menu of the host does not stop anything either.
    n.open_net_menu(0);
    let host = n.steps(0);
    n.run_to_and_compare(host + 60);
    assert_eq!((n.runs[0].state(), n.runs[1].state()), (2, 2));
}

/// E-265: `RETURN2` and ESC close `INGAMEOPTIONS2` with `{0x13}` without a speed command - even if the own setting
/// `GameSpeed` differs from the running game speed, on the host as on the client. The game speed stays on all machines,
/// no speed message, state 0.
// QUIRK E-265, E-316: deviation
#[test]
fn closing_the_net_menu_sends_no_speed_command() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(0) >= 10);
    let speed = n.sim(0).game_speed;
    let other = i32::from(speed) + 3;
    for i in [0, 1] {
        n.runs[i].host.app.set_game_speed(other);
        n.open_net_menu(i);
    }
    n.runs[0].click(NET_MENU, "return2");
    n.runs[1].key("ESCAPE");
    n.settle();
    for i in [0, 1] {
        assert_eq!(n.runs[i].state(), 0, "machine {i}: state 0");
        assert!(!n.runs[i].dialogs().iter().any(|d| d == NET_MENU));
        assert_eq!(
            n.runs[i].host.app.config().game_speed,
            other,
            "setting stays"
        );
    }
    let now = n.steps(0);
    n.run_to_and_compare(now + 100);
    for i in 0..3 {
        assert_eq!(n.sim(i).game_speed, speed, "machine {i}: game speed stays");
        let g = n.runs[i].view.game.as_ref().unwrap();
        assert!(g.speed_messages.is_empty(), "machine {i}: no speed message");
    }
}

/// E-316 (re/multiplayer-menus L.7.1): After "End current game" (confirmation prompt 15780, OK `{0x27}`) the client
/// stays in state 2 until its defeat is executed: no menu visible, ESC, chat and diplomacy without effect. Then the
/// end screen (state 7), without a speed command; all compute equally.
// QUIRK E-320: kept
// QUIRK E-316: deviation
#[test]
fn after_quitgame_escape_chat_and_diplomacy_are_inert() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(1) >= 10);
    let speed = n.sim(0).game_speed;
    n.open_net_menu(1);
    n.runs[1].click(NET_MENU, "quitgame");
    n.settle();
    assert!(n.runs[1].dialogs().iter().any(|d| d == "infoframe"));
    assert_eq!(n.runs[1].host.app.screen_state().info_text, Some(15780));
    n.runs[1].click("infoframe", "ok");
    n.frame();
    // Until the defeat: every frame try ESC, chat and diplomacy.
    let mut waited = 0;
    while n.sim(1).state.players[1].result == 0 {
        let run = &mut n.runs[1];
        assert_eq!(run.state(), 2, "waiting time: state 2");
        assert!(
            !run.dialogs()
                .iter()
                .any(|d| d == NET_MENU || d == "diplomatie"),
            "no menu visible"
        );
        run.key("ESCAPE");
        assert!(!run.host.app.show_chat_window(1), "no chat");
        run.view.ui_requests.push(UiRequest::ToggleDiplomacy);
        poke_net(run);
        assert_eq!(run.state(), 2, "no diplomacy");
        n.frame();
        waited += 1;
        assert!(waited < MAX_FRAMES, "defeat never comes");
    }
    assert!(waited >= 1, "the defeat goes through the round");
    n.frames_until(|n| n.runs[1].state() == 7);
    n.settle();
    assert!(n.runs[1].dialogs().iter().any(|d| d == "gameend_defeat"));
    let now = n.steps(0);
    n.run_to_and_compare(now + 40);
    for i in 0..3 {
        assert_eq!(n.sim(i).state.players[1].result, 1, "machine {i}");
        assert_eq!(n.sim(i).game_speed, speed, "machine {i}: no speed command");
    }
}

/// E-320 (re/multiplayer-menus L.7.2): The diplomacy dialog opens in the network at state 0 (state 10), does not stop
/// the match, and a status change goes as a command through the round: not immediately, but at the execution tick on
/// all machines at the same tick; all compute equally.
// QUIRK E-320: kept
#[test]
fn diplomacy_in_the_net_keeps_running_and_goes_through_the_round() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(1) >= 10);
    let run = &mut n.runs[1];
    run.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke_net(run);
    assert_eq!(run.state(), DIPLOMACY_MENU_STATE);
    n.settle();
    assert!(n.runs[1].dialogs().iter().any(|d| d == "diplomatie"));
    let before = n.steps(1);
    n.frames_until(|n| n.steps(1) >= before + 20);
    assert_eq!(n.runs[1].state(), DIPLOMACY_MENU_STATE, "dialog open");
    let friend = |sim: &SimHost| sim.state.diplomacy.is_friend(1, 2);
    assert!(!friend(n.sim(1)));
    n.runs[1].apply(UiEffect::Diplomacy(DiplomacyAction::Status {
        slot: 2,
        mode: 0,
    }));
    assert!(!friend(n.sim(1)), "no immediate execution");
    n.frames_until(|n| (0..3).all(|i| friend(n.sim(i))));
    let step_of = |sim: &SimHost| -> Vec<u64> {
        sim.log
            .iter()
            .filter(|c| {
                c.command.player == 1
                    && c.command.kind
                        == CommandKind::Diplomacy {
                            p: 1,
                            q: 2,
                            mode: 0,
                        }
            })
            .map(|c| c.step)
            .collect()
    };
    let at = step_of(n.sim(0));
    assert_eq!(at.len(), 1, "one command");
    assert!(
        TurnConfig::default().exec_at(at[0]).is_some(),
        "at the execution tick"
    );
    for i in 1..3 {
        assert_eq!(step_of(n.sim(i)), at, "machine {i}");
    }
    assert_eq!(n.runs[1].state(), DIPLOMACY_MENU_STATE, "dialog still open");
    let now = n.steps(0);
    n.run_to_and_compare(now + 40);
    n.runs[1].click("diplomatie", "back");
    n.settle();
    assert_eq!(n.runs[1].state(), 0);
}

// --- End of the match in the network with accounting (E-273, E-299, E-319) ------------------------------------

use opennml_sim::game_end::DEFEAT_MESSAGE;

/// Both machines of the lobby are ready; `START` of the host, then frames until after `Go` (phase `Match` on both)
/// and at least `steps` ticks on both.
pub(super) fn lan_match(n: &mut Lan, steps: u64) {
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.frames_until("both after Go", |l| {
        (0..2).all(|i| l.phase(i) == NetPhase::Match && l.runs[i].view.steps() >= steps)
    });
    for run in &n.runs {
        assert!(run.view.net().is_some_and(NetDriver::started));
    }
}

impl Lan {
    /// Machine `i` surrenders (`0x27`, through the round) and then stands in front of its defeat screen (state 7).
    pub(super) fn surrender_to_the_end_screen(&mut self, i: usize) -> u8 {
        let slot = self.runs[i].host.app.game().expect("match").local_slot;
        self.runs[i].apply(UiEffect::Surrender { slot });
        self.frames_until("end screen", |l| {
            l.runs[i].dialogs().iter().any(|d| d == "gameend_defeat")
        });
        self.settle();
        assert_eq!(self.runs[i].state(), 7);
        slot
    }
}

/// E-273: The client surrenders; its end screen shows the defeat, `END_DEFEAT` closes the network (exactly one `Bye`),
/// ends the match and opens the accounting with both slots from the own statistics. The host computes on and has
/// received the message 200 + p (text 17200 + p "... was eliminated!").
// QUIRK E-273: kept
#[test]
fn own_defeat_ends_with_the_accounting() {
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    lan_match(&mut n, 10);
    let slot = n.surrender_to_the_end_screen(1);
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.frames_until("accounting", |l| in_accounting(&l.runs[1]));
    n.settle();
    let run = &n.runs[1];
    let view = run.host.app.data().accounting.clone().unwrap();
    assert_eq!(view.rows.len(), 2, "both slots: {:?}", view.rows);
    assert_eq!(run.state(), 0);
    assert!(session_closed(&n, 1), "network closed");
    assert_eq!(n.phase(1), NetPhase::Idle);
    assert_eq!(*byes.lock().unwrap(), vec![LEAVE_REASON.to_string()]);
    let at = n.runs[0].view.steps();
    n.frames_until("host computes on", |l| l.runs[0].view.steps() >= at + 40);
    let host = &n.runs[0];
    assert!(host.flow.running(), "host in the match");
    assert_eq!(
        host.view.sim().unwrap().state.players[usize::from(slot)].result,
        1
    );
    let shown = &host.view.game.as_ref().unwrap().player_messages;
    assert!(
        shown
            .iter()
            .any(|&(_, id)| id == DEFEAT_MESSAGE + i32::from(slot)),
        "message 17200 + {slot} on the host: {shown:?}"
    );
    assert_eq!(message(host), None, "no message on the host");
}

/// The host drops out (result 1 through the round; a victory command would be rejected in the network), the client
/// wins with that by the victory rules of the game mode (no opponents left, `GameEngine_CheckVictoryDefeat`).
/// `END_VICTORY` leads through the accounting like the defeat, the network is closed. Afterwards the host closes its
/// end screen while the rounds are running, thus in an orderly way: message "The host has ended the match ...", "OK" ->
/// accounting -> main menu.
// QUIRK E-273: kept
#[test]
fn victory_ends_with_the_accounting() {
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    lan_match(&mut n, 10);
    let host_slot = n.runs[0].host.app.game().unwrap().local_slot;
    n.runs[0].view.sim_mut().unwrap().submit(Command {
        player: host_slot,
        kind: CommandKind::SetPlayerResult {
            slot: host_slot,
            result: 1,
        },
    });
    n.frames_until("victory screen", |l| {
        l.runs[1].dialogs().iter().any(|d| d == "gameend_victory")
    });
    n.settle();
    assert_eq!(n.runs[1].state(), 6);
    n.runs[1].click("gameend_victory", "end_victory");
    n.frames_until("accounting of the winner", |l| in_accounting(&l.runs[1]));
    let view = n.runs[1].host.app.data().accounting.clone().unwrap();
    assert_eq!(view.rows.len(), 2, "{:?}", view.rows);
    assert!(session_closed(&n, 1));
    assert_eq!(*byes.lock().unwrap(), vec![LEAVE_REASON.to_string()]);
    n.frames_until("defeat on the host", |l| {
        l.runs[0].dialogs().iter().any(|d| d == "gameend_defeat")
    });
    n.settle();
    n.runs[0].click("gameend_defeat", "end_defeat");
    n.frames_until("message on the host", |l| message(&l.runs[0]).is_some());
    n.settle();
    assert!(
        !n.runs[0].dialogs().iter().any(|d| d == "accounting"),
        "not until after the OK"
    );
    n.runs[0].click("msgbox", "ok");
    n.frames_until("accounting of the host", |l| in_accounting(&l.runs[0]));
    n.settle();
    for i in [0, 1] {
        leave_accounting(&mut n.runs[i]);
    }
    n.settle();
    for i in [0, 1] {
        assert!(in_main_menu(&n.runs[i]), "machine {i}: {:?}", n.dialogs(i));
        assert_eq!(n.phase(i), NetPhase::Idle, "machine {i}");
    }
}

/// After the main menu, no frame is open in any run except `start` (and `logo`).
fn assert_only_the_main_menu(n: &NetRuns) {
    for (i, run) in n.runs.iter().enumerate() {
        let rest: Vec<String> = run
            .dialogs()
            .into_iter()
            .filter(|d| d != "start" && d != "logo")
            .collect();
        assert!(rest.is_empty(), "machine {i}: remainder {rest:?}");
    }
}

/// The host ends the match while client 1 has `INGAMEOPTIONS2` and client 2 the diplomacy dialog open; then once more
/// while client 1 waits for its defeat after `QUITGAME` (state 2, no menu). Each time: menus closed, state 0 under the
/// message, "OK" -> accounting -> main menu, no hanging frames, state 0 in the main menu.
// QUIRK E-273: kept
#[test]
fn host_end_while_the_client_menu_is_open_shows_the_accounting() {
    let leftovers = |run: &Run| -> Vec<String> {
        run.dialogs()
            .into_iter()
            .filter(|d| {
                [NET_MENU, "diplomatie", "msgbox", "accounting", "infoframe"].contains(&d.as_str())
                    || d.starts_with("gameend")
            })
            .collect()
    };
    // Game menu and diplomacy.
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(2) >= 10);
    n.open_net_menu(1);
    let run = &mut n.runs[2];
    run.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke_net(run);
    assert_eq!(run.state(), DIPLOMACY_MENU_STATE);
    n.settle();
    assert!(n.runs[2].dialogs().iter().any(|d| d == "diplomatie"));
    end_for_all(&mut n.runs[0]);
    n.frames_until(|n| n.runs.iter().all(|r| message(r).is_some()));
    n.settle();
    for i in 1..3 {
        let run = &n.runs[i];
        assert_eq!(run.state(), 0, "machine {i}: state 0 under the message");
        assert_eq!(leftovers(run), ["msgbox"], "machine {i}");
    }
    n.confirm(0..3);
    n.accounting_then_main_menu(0..3);
    n.settle();
    assert_only_the_main_menu(&n);

    // Waiting time after `QUITGAME`: confirmation prompt open, the host ends, in the same frame the client confirms.
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(1) >= 10);
    n.open_net_menu(1);
    n.runs[1].click(NET_MENU, "quitgame");
    n.settle();
    assert_eq!(n.runs[1].host.app.screen_state().info_text, Some(15780));
    end_for_all(&mut n.runs[0]);
    n.runs[1].click("infoframe", "ok");
    n.frame();
    assert_eq!(n.runs[1].state(), 2, "waiting time");
    assert!(
        !n.runs[1].dialogs().iter().any(|d| d == NET_MENU),
        "{:?}",
        n.runs[1].dialogs()
    );
    n.frames_until(|n| n.runs.iter().all(|r| message(r).is_some()));
    n.settle();
    assert_eq!(
        n.sim(1).state.players[1].result,
        0,
        "the end came before the defeat"
    );
    assert_eq!(n.runs[1].state(), 0, "state 0 under the message");
    assert_eq!(leftovers(&n.runs[1]), ["msgbox"]);
    n.confirm(0..3);
    n.accounting_then_main_menu(0..3);
    n.settle();
    assert_only_the_main_menu(&n);
}

/// Double end of the match. (a) Client: `END_DEFEAT` twice in the same frame (two `0x0E` through the queue), then two
/// more `0x0E` without a match - exactly one accounting (unchanged), exactly one `Bye`, no panic. (b) Host with running
/// rounds: `END_DEFEAT` twice and another `0x0E` during the end finish in an orderly way (no accounting yet); "OK" of
/// the message opens exactly one accounting, a `0x0E` afterwards has no effect.
// QUIRK E-273: kept
#[test]
fn second_end_in_the_net_is_inert() {
    let count = |run: &Run, name: &str| run.dialogs().iter().filter(|d| *d == name).count();
    let Some((mut n, byes)) = ready_pair() else {
        return;
    };
    lan_match(&mut n, 10);
    n.surrender_to_the_end_screen(1);
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.frames_until("accounting", |l| in_accounting(&l.runs[1]));
    n.settle();
    let view = n.runs[1].host.app.data().accounting.clone().unwrap();
    n.runs[1].apply(UiEffect::EndMatch);
    n.runs[1].apply(UiEffect::EndMatch);
    n.settle();
    let run = &n.runs[1];
    assert_eq!(count(run, "accounting"), 1);
    assert!(in_accounting(run), "{:?}", run.dialogs());
    assert_eq!(run.host.app.data().accounting.as_ref(), Some(&view));
    assert_eq!(*byes.lock().unwrap(), vec![LEAVE_REASON.to_string()]);
    assert!(session_closed(&n, 1));
    leave_accounting(&mut n.runs[1]);
    n.settle();
    assert!(in_main_menu(&n.runs[1]), "{:?}", n.dialogs(1));

    // (b) Host.
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(2) >= 10);
    n.runs[0].view.sim_mut().unwrap().submit(Command {
        player: 0,
        kind: CommandKind::SetPlayerResult { slot: 0, result: 1 },
    });
    n.frames_until(|n| n.runs[0].dialogs().iter().any(|d| d == "gameend_defeat"));
    n.settle();
    n.runs[0].click("gameend_defeat", "end_defeat");
    n.runs[0].click("gameend_defeat", "end_defeat");
    n.frames_until(|n| n.runs[0].view.net().is_some_and(NetDriver::ending));
    n.runs[0].apply(UiEffect::EndMatch);
    assert!(n.runs[0].flow.running(), "the match stays until the end");
    n.frames_until(|n| n.runs.iter().all(|r| message(r).is_some()));
    n.settle();
    assert_eq!(count(&n.runs[0], "accounting"), 0);
    n.runs[0].click("msgbox", "ok");
    n.frames_until(|n| in_accounting(&n.runs[0]));
    n.settle();
    n.runs[0].apply(UiEffect::EndMatch);
    n.settle();
    assert_eq!(count(&n.runs[0], "accounting"), 1);
    assert!(in_accounting(&n.runs[0]), "{:?}", n.runs[0].dialogs());
    n.confirm(1..3);
    n.accounting_then_main_menu(0..3);
}

/// A failure before `Go` leads to the main menu without accounting: The host has loaded the map, the client cannot
/// load it (`Loaded { error }`); the host aborts the start for all. It stands in the loaded match without `Go`, shows
/// the message with the reason, and "OK" (`0x0E`) leads without accounting to the main menu (a different failure than
/// `host_abort_after_loading_before_go_returns_to_the_main_menu`).
// QUIRK E-273: kept
#[test]
fn failure_before_go_still_goes_to_the_main_menu() {
    let Some(mut n) = lobby_pair() else {
        return;
    };
    start_from_lobby(&mut n);
    n.frames_until("host has loaded", |l| l.runs[0].flow.running());
    assert!(!n.runs[1].flow.running(), "the client loads later");
    let level = n.runs[0].view.level().to_string();
    n.runs[1].view.fail_level = Some(level);
    n.frames_until("message on the host", |l| message(&l.runs[0]).is_some());
    let text = message(&n.runs[0]).unwrap();
    assert_eq!(text, own_text(&n, 0, LOAD_FAILED, &["Bert"]));
    assert!(
        !n.runs[0].view.net().expect("driver in the match").started(),
        "no Go"
    );
    n.settle();
    n.runs[0].click("msgbox", "ok");
    n.frames_until("main menu of the host", |l| in_main_menu(&l.runs[0]));
    assert_eq!(n.runs[0].host.app.data().accounting, None, "no accounting");
    assert_eq!(n.phase(0), NetPhase::Idle);
    assert_eq!(n.runs[0].state(), 0);
    assert!(!n.runs[1].dialogs().iter().any(|d| d == "accounting"));
    assert_eq!(n.runs[1].host.app.data().accounting, None);
}

/// Deviates from E-299: "Main menu" of the accounting leads to `START` after a network match - on the command-line
/// path (driver directly at the match, next to it the empty network session of the menu path as in `opennml-game`) as on
/// the menu path; no program end, state 0, the network session is free for a new search.
// QUIRK E-299: deviation
#[test]
fn accounting_main_menu_returns_to_start_after_the_net() {
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    let identity = NetIdentity {
        hello: lan_hello("Client 1"),
        port: LAN_PORT,
        seed: Box::new(|| LAN_SEED),
        listen: true,
    };
    let transport = MemTransport::new(&n.net, 0);
    let session = NetSession::new(Box::new(transport), identity, net_paths(n.dir.path(), 1));
    n.runs[1].flow.set_net_session(session);
    n.frames_until(|n| n.steps(1) >= 10);
    n.runs[1].apply(UiEffect::Surrender { slot: 1 });
    n.frames_until(|n| n.runs[1].dialogs().iter().any(|d| d == "gameend_defeat"));
    n.settle();
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.accounting_then_main_menu(1..2);
    let run = &n.runs[1];
    assert_eq!(
        run.flow.net_session().map(NetSession::phase),
        Some(NetPhase::Idle)
    );
    assert!(n.runs[0].flow.running(), "the host plays on");

    // Menu path: afterwards `LAN1` opens again and searches.
    let Some((mut n, _)) = ready_pair() else {
        return;
    };
    lan_match(&mut n, 10);
    n.surrender_to_the_end_screen(1);
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.frames_until("accounting", |l| in_accounting(&l.runs[1]));
    n.settle();
    leave_accounting(&mut n.runs[1]);
    n.settle();
    assert!(at_start(&n, 1), "{:?}", n.dialogs(1));
    assert!(in_main_menu(&n.runs[1]));
    assert_eq!(n.runs[1].state(), 0);
    assert_eq!(n.phase(1), NetPhase::Idle);
    n.open_lan1(1);
    assert_eq!(n.phase(1), NetPhase::Searching);
}

/// After the own elimination no chat and no diplomacy - an open chat window closes with the end screen, in the end
/// screen and in the accounting neither chat nor diplomacy open.
// QUIRK E-273: kept
#[test]
fn chat_is_closed_after_own_defeat() {
    let chat_open = |run: &Run| run.dialogs().iter().any(|d| d.starts_with("chatwindow"));
    let Some(mut n) = NetRuns::corpus() else {
        return;
    };
    n.frames_until(|n| n.steps(1) >= 10);
    assert!(n.runs[1].host.app.show_chat_window(1));
    n.settle();
    assert!(chat_open(&n.runs[1]), "{:?}", n.runs[1].dialogs());
    n.runs[1].apply(UiEffect::Surrender { slot: 1 });
    n.frames_until(|n| n.runs[1].state() == 7);
    n.settle();
    let try_both = |run: &mut Run| {
        let state = run.state();
        assert!(!run.host.app.show_chat_window(1), "no chat");
        run.view.ui_requests.push(UiRequest::ToggleDiplomacy);
        poke_net(run);
        assert_eq!(run.state(), state, "no diplomacy");
    };
    try_both(&mut n.runs[1]);
    n.settle();
    let run = &n.runs[1];
    assert!(
        !chat_open(run),
        "chat closed with the end screen: {:?}",
        run.dialogs()
    );
    assert!(!run.dialogs().iter().any(|d| d == "diplomatie"));
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.frames_until(|n| in_accounting(&n.runs[1]));
    n.settle();
    try_both(&mut n.runs[1]);
    n.settle();
    let run = &n.runs[1];
    assert!(!chat_open(run) && !run.dialogs().iter().any(|d| d == "diplomatie"));
    assert!(in_accounting(run));
}

/// The client stands on `GAMEEND_DEFEAT`, the host ends the match. Frames until the driver of the client reports
/// `finished()` (in this frame `net_frame` shows the message, `end_with_message`); with `click_before = Some(k)` the
/// client clicks `END_DEFEAT` before frame `k` (counted from the end of the host). Returns with the number of frames.
fn end_message_over_the_end_screen(click_before: Option<u32>) -> Option<(Lan, Byes, u32)> {
    let (mut n, byes) = ready_pair()?;
    lan_match(&mut n, 10);
    n.surrender_to_the_end_screen(1);
    end_for_all(&mut n.runs[0]);
    let finished = |l: &Lan| l.runs[1].view.net().is_some_and(NetDriver::finished);
    let mut frames = 0;
    while !finished(&n) {
        if click_before == Some(frames) {
            n.runs[1].click("gameend_defeat", "end_defeat");
        }
        n.frame();
        frames += 1;
        assert!(frames < MAX_LAN_FRAMES, "end on the client never comes");
    }
    Some((n, byes, frames))
}

/// After `END_DEFEAT` and the message about the end in the same frame: exactly one end through the message - no hang
/// of the queue, "OK" opens exactly one accounting without a message remainder, at most one `Bye`, main menu
/// reachable.
fn assert_one_end_through_the_message(mut n: Lan, byes: &Byes) {
    n.frames_until("message on the client", |l| message(&l.runs[1]).is_some());
    n.settle();
    let d = n.dialogs(1);
    assert!(
        !d.iter().any(|x| x == "accounting" || x == "gameend_defeat"),
        "{d:?}"
    );
    // No chain of `END_*` behind the message (`{0x15}` waited for the open message, `{0x0E}` behind it).
    let queue = n.runs[1].host.app.queue();
    assert!(
        queue.is_empty() && !queue.waiting(),
        "{:?}, waiting {}",
        queue.iter().collect::<Vec<_>>(),
        queue.waiting()
    );
    n.runs[1].click("msgbox", "ok");
    n.frames_until("accounting", |l| in_accounting(&l.runs[1]));
    n.settle();
    let d = n.dialogs(1);
    assert_eq!(d.iter().filter(|x| *x == "accounting").count(), 1, "{d:?}");
    assert!(
        !d.iter().any(|x| x == "msgbox"),
        "no message remainder: {d:?}"
    );
    assert!(n.runs[1].host.app.queue().is_empty(), "nothing hangs");
    // After the orderly end the host has already closed the connection: at most one `Bye`, never two.
    let sent = byes.lock().unwrap().clone();
    assert!(
        sent.is_empty() || sent == [LEAVE_REASON.to_string()],
        "{sent:?}"
    );
    leave_accounting(&mut n.runs[1]);
    n.settle();
    assert!(in_main_menu(&n.runs[1]), "{:?}", n.dialogs(1));
    assert!(!n.dialogs(1).iter().any(|x| x == "msgbox"));
}

/// `END_*` and the message about the end in the same frame. (a) The click comes in the frame after `net_frame` (the
/// message is queued but not yet open): `END_*` has no effect, the message carries its own `0x0E`. (b) The click
/// comes before `net_frame` of the same frame (the chain `{4}`, `{0x15}`, `{0x0E}` already stands):
/// `end_with_message` takes the queued `0x0E` out. Both times: one accounting, one `Bye`, no remainder.
// QUIRK E-273: kept
#[test]
fn end_button_and_end_message_in_the_same_frame_end_once() {
    let Some((mut n, byes, frames)) = end_message_over_the_end_screen(None) else {
        return;
    };
    n.runs[1].click("gameend_defeat", "end_defeat");
    assert_one_end_through_the_message(n, &byes);

    let (n, byes, again) = end_message_over_the_end_screen(Some(frames - 1)).expect("corpus");
    assert_eq!(again, frames, "same flow");
    assert_one_end_through_the_message(n, &byes);
}

// --- End of the network match and own result in the same frame -------------------------------------------------------

/// Frames from the surrender of client 1 up to the frame in which its defeat falls on it (through the round).
fn frames_to_defeat() -> Option<u32> {
    let mut n = NetRuns::corpus()?;
    n.frames_until(|n| n.steps(2) >= 10);
    n.runs[1].apply(UiEffect::Surrender { slot: 1 });
    let mut frames = 0;
    while n.sim(1).state.players[1].result == 0 {
        n.frame();
        frames += 1;
        assert!(frames < MAX_FRAMES, "defeat never comes");
    }
    Some(frames)
}

/// Client 1 surrenders; `lead` frames later the match ends on the host (`crash`: crash without `Bye`, otherwise
/// orderly for all, [`end_for_all`]). Frames up to the one in which the driver of the client reports the end
/// ([`NetDriver::finished`]); `Some` if the defeat of the client fell in exactly this frame (orderly: `End` and defeat
/// in the same bundle; crash: the driver recomputes the distributed bundles, at most `T + V` ticks).
fn end_in_the_defeat_frame(lead: u32, crash: bool) -> Option<NetRuns> {
    let mut n = NetRuns::corpus()?;
    n.frames_until(|n| n.steps(2) >= 10);
    n.runs[1].apply(UiEffect::Surrender { slot: 1 });
    for _ in 0..lead {
        n.frame();
    }
    if crash {
        n.runs[0].view.end();
    } else {
        end_for_all(&mut n.runs[0]);
    }
    let finished = |n: &NetRuns| n.runs[1].view.net().is_some_and(NetDriver::finished);
    let result = |n: &NetRuns| n.sim(1).state.players[1].result;
    for _ in 0..MAX_FRAMES {
        let before = result(&n);
        n.frame();
        if finished(&n) {
            return (before == 0 && result(&n) == 1).then_some(n);
        }
    }
    panic!("end on the client never comes");
}

/// The end of the network match (orderly or crash of the host) and the own defeat fall in the same frame on the
/// client. The message carries the end: exactly one `MSGBOX`, no `GAMEEND_*` (otherwise `{0x34}` would close the
/// message, `END_*` would stay without effect through the flag of the message, and the player would be stuck).
/// "OK" -> accounting -> `MAINMENU` -> main menu, afterwards no flag `{0x0E}` remains that would block the `END_*` of
/// a later match.
// QUIRK E-273: kept
#[test]
fn end_and_own_defeat_in_the_same_frame_show_one_message() {
    let Some(defeat) = frames_to_defeat() else {
        return;
    };
    for crash in [false, true] {
        let mut n = (0..defeat)
            .rev()
            .find_map(|lead| end_in_the_defeat_frame(lead, crash))
            .unwrap_or_else(|| panic!("end and defeat not in the same frame (crash {crash})"));
        n.settle();
        let run = &n.runs[1];
        let dialogs = run.dialogs();
        assert_eq!(
            dialogs.iter().filter(|d| *d == "msgbox").count(),
            1,
            "crash {crash}: {dialogs:?}"
        );
        assert!(
            !dialogs.iter().any(|d| d.starts_with("gameend")),
            "crash {crash}: no end screen under the message: {dialogs:?}"
        );
        assert_eq!(run.state(), 0, "crash {crash}");
        assert_eq!(n.sim(1).state.players[1].result, 1, "crash {crash}");
        let runs = if crash { 1..3 } else { 0..3 };
        let r = runs.clone();
        n.frames_until(move |n| n.runs[r.clone()].iter().all(|r| message(r).is_some()));
        n.confirm(runs.clone());
        n.accounting_then_main_menu(runs.clone());
        n.settle();
        for i in runs {
            let state = n.runs[i].host.app.screen_state();
            assert_eq!(
                state.info_ok, None,
                "crash {crash}, machine {i}: flag empty"
            );
        }
    }
}

mod migration;
mod saved;

// ---- Recording in the network (E-290) -------------------------------------------------------------------------------

/// E-290: `SAVEREPLAY` in the draft of the host (`LAN3`, session data): both machines record, also the client with
/// `SaveReplay 0` in its own settings, each in its own folder. Both files carry the same commands of all slots, and
/// their replay equals the match.
// QUIRK E-290: kept
// QUIRK E-329: deviation
#[test]
fn both_machines_record_the_net_match() {
    use super::replay_tests::{NAME, live_hash, read_recording, recordings, replayed_hash, walk};
    let Some(mut n) = Lan::corpus_with(&["Anna", "Bert"], |i, cfg| {
        if i == 0 {
            cfg.save_replay = 1;
        }
    }) else {
        return;
    };
    assert_eq!(n.runs[1].host.app.config().save_replay, 0);
    n.host(0, Some("tinte"));
    n.join(1);
    assert!(
        n.lobby(1).session.settings.save_replay,
        "switch in the session data"
    );
    n.all_ready(&[(0, 0), (1, 1)]);
    lan_match(&mut n, 10);
    walk(&mut n.runs[0], 400.0);
    walk(&mut n.runs[1], -400.0);
    n.frames_until("tick 150", |l| l.runs.iter().all(|r| r.view.steps() >= 150));
    let hash = live_hash(&n.runs[0], 150);
    assert_eq!(live_hash(&n.runs[1], 150), hash);
    let rules = n.runs[0].view.sim().unwrap().rules.clone();
    let slots = [0, 1].map(|i| n.runs[i].view.sim().unwrap().active_player);
    n.host_ends_to_the_main_menu();
    let files = [0, 1].map(|i| {
        assert_eq!(recordings(&n.runs[i]), [NAME], "machine {i}");
        read_recording(&n.runs[i], NAME)
    });
    assert_eq!(files[0].head, files[1].head);
    assert_eq!(files[0].commands, files[1].commands, "same commands");
    for slot in slots {
        assert!(
            files[0].commands.iter().any(|c| c.command.player == slot),
            "command of slot {slot}"
        );
    }
    for (i, file) in files.iter().enumerate() {
        assert!(!file.torn);
        assert_eq!(replayed_hash(file, &rules, 150), hash, "machine {i}");
    }
}

/// E-331: The recording of the client replays in single player - without network session, with the view of the host
/// slot (`first_player`, lobby key -1) and tick by tick the checksums of the network match.
// QUIRK E-331: kept
// QUIRK E-333: deviation
#[test]
fn client_recording_views_the_host() {
    use super::replay_tests::{Hashes, NAME, collect_hashes, read_recording, walk};
    let Some(mut n) = Lan::corpus_with(&["Anna", "Bert"], |i, cfg| {
        if i == 0 {
            cfg.save_replay = 1;
        }
    }) else {
        return;
    };
    n.host(0, Some("tinte"));
    n.join(1);
    n.all_ready(&[(0, 0), (1, 1)]);
    lan_match(&mut n, 10);
    let mut live = Hashes::new();
    collect_hashes(&n.runs[1], &mut live);
    walk(&mut n.runs[0], 400.0);
    walk(&mut n.runs[1], -400.0);
    while n.runs[1].view.steps() < 150 {
        n.frame();
        collect_hashes(&n.runs[1], &mut live);
    }
    let host_slot = n.runs[0].view.sim().unwrap().active_player;
    let client_slot = n.runs[1].view.sim().unwrap().active_player;
    assert_ne!(host_slot, client_slot);
    n.host_ends_to_the_main_menu();
    let file = read_recording(&n.runs[1], NAME);
    assert!(file.head.start.setup.network, "network recording");
    let client = &mut n.runs[1];
    client.apply(UiEffect::PlayReplay { name: NAME.into() });
    assert!(client.flow.running(), "replay running");
    assert!(client.view.net().is_none(), "no network session");
    let sim = client.view.sim().unwrap();
    assert!(!sim.net, "no turn mode");
    assert_eq!(sim.active_player, host_slot, "view of the host");
    assert_eq!(client.host.app.game().unwrap().local_slot, host_slot);
    assert!(client.host.app.game().unwrap().net.is_none());
    client.flow.begin_round();
    client.flow.begin_round();
    let mut played = Hashes::new();
    let until = client.now + 120_000;
    while client.view.steps() < 150 {
        assert!(client.now < until, "tick 150 not reached");
        client.frame();
        collect_hashes(client, &mut played);
    }
    for step in 1..=150 {
        assert!(
            live.contains_key(&step),
            "network match: tick {step} missing"
        );
        assert_eq!(played.get(&step), live.get(&step), "tick {step}");
    }
}

/// The command-line path (`--host`/`--join`, without network session of the menus) never records, even with
/// `SaveReplay 1` in the settings.
// QUIRK E-329: deviation
#[test]
fn command_line_net_match_does_not_record() {
    let Some(mut run) = Run::corpus_configured(opennml_ui::app::MenuOptions::default(), |c| {
        c.save_replay = 1
    }) else {
        return;
    };
    run.view.real_levels = true;
    let (rules, active) = two_player_rules(&run.vfs);
    let setup = MatchSetup {
        network: true,
        seed: 5,
        ..MatchSetup::default_for(&rules)
    };
    let mut start = NetStart::new(net_start_info(&rules, rules.checksum, setup, active[1]));
    let driver = start.driver.take().unwrap();
    run_net_start(&mut run, driver).expect("Start");
    assert!(run.flow.running());
    assert!(run.flow.recorder().is_none());
    for _ in 0..20 {
        run.frame();
    }
    assert!(!run.files.autoplay_dir().exists());
}
