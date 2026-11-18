//! Recording after `SAVEREPLAY` (E-329, E-334, E-335, E-337): if the switch of the match is set, the flow writes every
//! single-player and network match continuously to `savegame/autoplay/<YYMMDD-hhmm>` ([`ReplayRecorder`]), also after
//! loading a savegame (then with the whole log from tick 0). Host and flow as in `tests.rs` (fake with real maps,
//! savegames in a throwaway folder, fixed clock [`TEST_CLOCK`]); silently skipped without `NML_GAME_DIR`.

use std::path::PathBuf;

use opennml_sim::command::Command;
use opennml_sim::save::{MAX_REPLAY_STEPS, checksum};
use opennml_sim::setup::start_package;
use opennml_ui::app::MenuOptions;
use opennml_ui::saves::{LocalStamp, SaveDir, SaveFiles};

use super::tests::{KAMP1_1, Run, TEST_CLOCK, request};
use super::*;
use crate::game::sim_host::plain_testkit;
use crate::replay_file::{ReplayFile, ReplayHead, ReplayMeta, ReplayRecorder, replay_name};
use crate::testutil::{capture_warnings, take_warnings};

/// Name of the recording for the test clock.
pub(super) const NAME: &str = "261108-1435";
/// A later clock so that a new recording does not overwrite the old one.
pub(super) const LATER: LocalStamp = LocalStamp {
    year: 2026,
    month: 11,
    day: 8,
    hour: 15,
    minute: 2,
};
pub(super) const LATER_NAME: &str = "261108-1502";

/// Free match like [`request`] with `SAVEREPLAY` set (like `CREATESINGLEPLAYERGAME` with the switch `savereplay`,
/// `copy_settings` takes `SaveReplay` of the settings into the draft).
fn recorded_request() -> MatchRequest {
    let mut req = request(0);
    req.settings.save_replay = true;
    req
}

/// Starts `req` on the real map (like `tests::start_real`).
fn start_with(r: &mut Run, req: MatchRequest) {
    r.view.real_levels = true;
    r.apply(UiEffect::StartMatch(req));
    assert!(r.flow.running(), "match running");
    r.flow.begin_round();
    r.flow.begin_round();
}

fn start_recorded(r: &mut Run) {
    start_with(r, recorded_request());
}

/// Names in the folder `savegame/autoplay`, sorted; empty without the folder.
pub(super) fn recordings(run: &Run) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(run.files.autoplay_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn path_of(run: &Run, name: &str) -> PathBuf {
    run.files.autoplay_dir().join(name)
}

/// Reads the recording `name` of the run.
pub(super) fn read_recording(run: &Run, name: &str) -> ReplayFile {
    let bytes = std::fs::read(path_of(run, name)).expect("recording readable");
    ReplayFile::from_bytes(&bytes).expect("recording valid")
}

/// Checksum of the replay from the recording at tick `steps`.
pub(super) fn replayed_hash(file: &ReplayFile, rules: &Rules, steps: u64) -> u64 {
    checksum(&file.replay().play(rules, steps).expect("replay"))
}

/// Checksum of the running match at tick `steps` (from the history of the last ticks).
pub(super) fn live_hash(run: &Run, steps: u64) -> u64 {
    run.view
        .sim()
        .unwrap()
        .hash_at(steps)
        .unwrap_or_else(|| panic!("checksum at tick {steps}"))
}

/// An own unit with a walk module goes `d` units diagonally away (command via `SimHost::move_units`, on the network
/// into the next contribution).
pub(super) fn walk(run: &mut Run, d: f32) {
    let sim = run.view.sim_mut().unwrap();
    let local = sim.active_player;
    let unit = sim
        .state
        .objects()
        .find(|o| o.owner() == local && o.has_module(0, false))
        .expect("own unit with walk module")
        .id;
    let pos = sim.state.object(unit, false).unwrap().world_pos;
    let target = glam::Vec3::new(pos[0] + d, pos[1], pos[2] + d);
    let order = crate::game::formation_order::FormationOrder::default();
    assert!(sim.move_units(&[unit], target, None, order), "walk command");
}

fn run_to(r: &mut Run, steps: u64) {
    let until = r.now + 60_000;
    while r.view.steps() < steps {
        assert!(r.now < until, "tick {steps} not reached");
        r.frame();
    }
}

/// Several game effects in one frame (`apply_all` like `HostRequests::game`).
fn apply_in_one_frame(r: &mut Run, effects: Vec<UiEffect>) {
    let env = FlowEnv {
        vfs: &r.vfs,
        texts: &r.host.layer.texts,
        files: &r.files,
    };
    let shown = &mut r.shown;
    r.flow.apply_all(
        effects,
        &mut r.host.app,
        &mut r.view,
        &env,
        &mut |ldg, phase| shown.push((ldg.to_string(), phase.percent())),
    );
}

/// `QUITGAME` in the game menu -> confirmation "OK" -> defeat -> `END_DEFEAT` -> accounting -> "Main menu".
fn quit_to_main_menu(r: &mut Run) {
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1, "game menu open");
    r.click("ingameoptions3", "quitgame");
    r.idle();
    r.key("RETURN");
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    r.click("gameend_defeat", "end_defeat");
    r.idle();
    r.click("accounting", "mainmenu");
    r.idle();
    assert!(!r.flow.running(), "match ended");
    assert!(r.view.game.is_none());
}

/// Diplomacy command of the local slot to another active slot.
fn diplomacy_command(run: &Run) -> Command {
    let sim = run.view.sim().unwrap();
    let local = sim.active_player;
    let other = (0u8..)
        .zip(&sim.state.players)
        .find(|(slot, p)| *slot != local && p.slot_active)
        .map(|(slot, _)| slot)
        .expect("second slot");
    Command {
        player: local,
        kind: opennml_sim::command::CommandKind::Diplomacy {
            p: i32::from(local),
            q: i32::from(other),
            mode: 0,
        },
    }
}

// QUIRK E-329: deviation
#[test]
fn replay_name_has_the_original_pattern() {
    // `strftime("%y%m%d-%H%M")` (format `0x0061F0B8`, `FUN_0042B810`), local time to the minute.
    assert_eq!(replay_name(TEST_CLOCK), NAME);
    assert_eq!(replay_name(LATER), LATER_NAME);
    let stamp = LocalStamp {
        year: 2003,
        month: 1,
        day: 2,
        hour: 3,
        minute: 4,
    };
    assert_eq!(replay_name(stamp), "030102-0304");
    let stamp = LocalStamp {
        year: 2100,
        month: 12,
        day: 31,
        hour: 23,
        minute: 59,
    };
    assert_eq!(replay_name(stamp), "001231-2359");
}

/// E-329, E-337: Single-player match with `SAVEREPLAY`, 150 ticks with walk commands, `QUITGAME` -> accounting ->
/// main menu: exactly one file, header with start package and host data, all commands up to the surrender, and the
/// replay has the checksum of the match at tick 150.
// QUIRK E-337: kept
// QUIRK E-329: deviation
#[test]
fn save_replay_records_a_single_player_match() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    let speed = r.view.sim().unwrap().game_speed;
    let package = {
        let sim = r.view.sim().unwrap();
        start_package(&sim.rules, &sim.setup)
    };
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 60);
    walk(&mut r, -300.0);
    run_to(&mut r, 150);
    let hash = live_hash(&r, 150);
    let rules = r.view.sim().unwrap().rules.clone();
    let log = r.view.sim().unwrap().log.clone();
    assert!(log.len() >= 2, "walk commands in the log");
    let slot = r.view.sim().unwrap().active_player;
    quit_to_main_menu(&mut r);
    assert!(r.flow.recorder().is_none(), "recording ends with the match");
    assert_eq!(recordings(&r), [NAME]);
    let file = read_recording(&r, NAME);
    assert!(!file.torn);
    assert_eq!(
        file.head,
        ReplayHead {
            start: package,
            meta: ReplayMeta {
                level_info: recorded_request().info,
                game_type: 0,
                lock_teams: false,
                game_speed: speed,
            },
        }
    );
    assert!(file.commands.starts_with(&log), "{:?}", file.commands);
    assert_eq!(
        file.commands.last().unwrap().command.kind,
        opennml_sim::command::CommandKind::SetPlayerResult {
            slot,
            result: opennml_sim::game_end::DEFEAT
        },
        "the surrender is in the file"
    );
    assert_eq!(replayed_hash(&file, &rules, 150), hash, "replay equal");
}

/// The file grows with every frame; read in the middle of the match it reproduces the beginning completely.
// QUIRK E-329: deviation
#[test]
fn the_file_grows_while_playing() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 50);
    let len50 = std::fs::metadata(path_of(&r, NAME)).unwrap().len();
    walk(&mut r, -300.0);
    run_to(&mut r, 100);
    let len100 = std::fs::metadata(path_of(&r, NAME)).unwrap().len();
    assert!(len50 < len100, "{len50} < {len100}");
    let file = read_recording(&r, NAME);
    assert!(!file.torn);
    let sim = r.view.sim().unwrap();
    assert_eq!(
        file.commands, sim.log,
        "the file has every command of the frame"
    );
    let rules = sim.rules.clone();
    let hash = live_hash(&r, 90);
    assert_eq!(replayed_hash(&file, &rules, 90), hash);
    let recorder = r.flow.recorder().expect("recording running");
    assert_eq!(recorder.last_step(), file.commands.last().map(|c| c.step));
    assert!(!recorder.ended());
}

/// Commands in the same frame before the end of the match are in the file - the surrender (executed at once) and the
/// game end `0x0E` in one round of the game effects.
// QUIRK E-337: kept
#[test]
fn last_commands_before_quit_are_in_the_file() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    run_to(&mut r, 20);
    let slot = r.view.sim().unwrap().active_player;
    let steps = r.view.steps();
    apply_in_one_frame(
        &mut r,
        vec![UiEffect::Surrender { slot }, UiEffect::EndMatch],
    );
    assert!(!r.flow.running());
    let file = read_recording(&r, NAME);
    let last = file.commands.last().expect("command in the file");
    assert_eq!(last.step, steps);
    assert_eq!(
        last.command.kind,
        opennml_sim::command::CommandKind::SetPlayerResult {
            slot,
            result: opennml_sim::game_end::DEFEAT
        }
    );
}

/// Deviates from E-335: `savegame/autoplay` is missing and is created when recording begins; the file has the header
/// already before the first command.
// QUIRK E-335: deviation
#[test]
fn autoplay_folder_is_created_when_recording_begins() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    assert!(!r.files.autoplay_dir().exists());
    assert_eq!(
        r.files.autoplay_dir(),
        r.files.dir.join("autoplay"),
        "folder `savegame/autoplay`"
    );
    start_recorded(&mut r);
    assert!(r.files.autoplay_dir().is_dir());
    assert_eq!(recordings(&r), [NAME]);
    let file = read_recording(&r, NAME);
    let sim = r.view.sim().unwrap();
    assert_eq!(file.head.start, start_package(&sim.rules, &sim.setup));
    assert!(file.commands.is_empty());
    let recorder = r.flow.recorder().expect("recording running");
    assert_eq!(recorder.path(), path_of(&r, NAME));
    assert_eq!(recorder.last_step(), None);
}

/// `UiEffect::DeleteSave { dir: Autoplay }` deletes the full name in the recording folder and nothing else.
// QUIRK E-342: deviation
#[test]
fn deleting_an_autoplay_entry_removes_the_file() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    let other = path_of(&r, "x.rec");
    std::fs::write(&other, b"x").unwrap();
    assert_eq!(recordings(&r), [NAME, "x.rec"]);
    apply_in_one_frame(
        &mut r,
        vec![UiEffect::DeleteSave {
            name: "x.rec".into(),
            dir: SaveDir::Autoplay,
        }],
    );
    assert!(!other.exists());
    assert_eq!(recordings(&r), [NAME], "the running recording stays");
    assert!(r.view.sim().is_some(), "the match continues");
}

/// Without `SAVEREPLAY` nothing is created.
// QUIRK E-329: deviation
#[test]
fn without_save_replay_nothing_is_recorded() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_with(&mut r, request(0));
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 30);
    assert!(r.flow.recorder().is_none());
    quit_to_main_menu(&mut r);
    assert!(!r.files.autoplay_dir().exists());
}

/// E-334: The campaign never records, even with `SaveReplay 1` in the settings.
// QUIRK E-334: kept
#[test]
fn campaign_does_not_record() {
    let Some(mut r) = Run::corpus_configured(MenuOptions::default(), |c| c.save_replay = 1) else {
        return;
    };
    r.apply(UiEffect::StartCampaignLevel {
        level: KAMP1_1.into(),
        difficulty: 0,
    });
    assert!(r.flow.running(), "campaign level running");
    r.flow.begin_round();
    r.flow.begin_round();
    for _ in 0..30 {
        r.frame();
    }
    assert!(r.flow.recorder().is_none());
    assert!(!r.files.autoplay_dir().exists());
}

/// E-334, O-444: The menu level never records, even with `SaveReplay 1`.
// QUIRK E-334: kept
// QUIRK O-444: deviation
#[test]
fn menu_level_does_not_record() {
    let Some(mut r) = Run::corpus_configured(
        MenuOptions {
            start_menu: true,
            ..MenuOptions::default()
        },
        |c| {
            c.is_menu_level = true;
            c.save_replay = 1;
        },
    ) else {
        return;
    };
    r.view.real_levels = true;
    r.idle();
    assert!(r.flow.menu_level().is_some(), "menu level running");
    let mut steps = 0;
    for _ in 0..60 {
        steps += r.frame();
    }
    assert!(steps > 0, "the menu level computes");
    assert!(r.flow.recorder().is_none());
    assert!(!r.files.autoplay_dir().exists());
}

/// E-337: The restart ends the old recording (with the command executed at once before the restart) and begins a new
/// one with tick 0 of the new match; its replay equals the new match.
// QUIRK E-337: kept
// QUIRK E-329: deviation
#[test]
fn restart_begins_a_new_recording() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 80);
    let command = diplomacy_command(&r);
    r.view.sim_mut().unwrap().execute_now(command);
    let old_log = r.view.sim().unwrap().log.clone();
    r.flow.set_fixed_clock(Some(LATER));
    r.apply(UiEffect::ReloadLevel);
    assert_eq!(r.view.steps(), 0, "restarted");
    assert_eq!(recordings(&r), [NAME, LATER_NAME]);
    let old = read_recording(&r, NAME);
    assert_eq!(
        old.commands, old_log,
        "the old recording has everything up to the restart"
    );
    let new = read_recording(&r, LATER_NAME);
    assert!(new.commands.is_empty(), "the new one begins at tick 0");
    assert_eq!(new.head.start, old.head.start, "same start package");
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    walk(&mut r, -250.0);
    run_to(&mut r, 60);
    let hash = live_hash(&r, 60);
    let sim = r.view.sim().unwrap();
    let rules = sim.rules.clone();
    let new = read_recording(&r, LATER_NAME);
    assert_eq!(new.commands, sim.log);
    assert!(!new.commands.is_empty());
    assert_eq!(replayed_hash(&new, &rules, 60), hash);
}

/// Deviates from E-329: Quicksave at tick 100, on to 150, load the savegame: the old recording ends with everything
/// up to 150, the new one begins with the whole log of the savegame from tick 0; its replay equals the match up to the
/// load point + 50.
// QUIRK E-337: kept
// QUIRK E-329: deviation
#[test]
fn loaded_save_records_the_whole_match_again() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 100);
    r.key("F1");
    let until = r.now + 2000;
    while !r.files.exists(SaveDir::Main, 0, "quicksave1") {
        assert!(r.now < until, "not written");
        r.frame();
    }
    let bytes = std::fs::read(r.files.path("quicksave1", 0)).unwrap();
    let save = GameSaveFile::from_bytes(&bytes).unwrap();
    assert!(save.host.save_replay, "SAVEREPLAY in the host part");
    let saved_log = save.host.log.clone().expect("log in the savegame");
    let at = save.sim.state.steps;
    walk(&mut r, -300.0);
    run_to(&mut r, 150);
    let before_load = r.view.sim().unwrap().log.clone();
    r.flow.set_fixed_clock(Some(LATER));
    r.apply(UiEffect::LoadGame {
        name: "quicksave1".into(),
    });
    assert_eq!(r.view.steps(), at, "loaded");
    assert_eq!(recordings(&r), [NAME, LATER_NAME]);
    let old = read_recording(&r, NAME);
    assert_eq!(
        old.commands, before_load,
        "the old recording ends with the load"
    );
    let new = read_recording(&r, LATER_NAME);
    assert_eq!(new.commands, saved_log, "the whole log from tick 0");
    assert_eq!(new.head.start, old.head.start);
    r.flow.begin_round();
    r.flow.begin_round();
    walk(&mut r, 200.0);
    run_to(&mut r, at + 50);
    let hash = live_hash(&r, at + 50);
    let rules = r.view.sim().unwrap().rules.clone();
    let new = read_recording(&r, LATER_NAME);
    assert!(new.commands.starts_with(&saved_log));
    assert!(new.commands.len() > saved_log.len(), "the new walk command");
    assert_eq!(
        replayed_hash(&new, &rules, at + 50),
        hash,
        "beyond the load point"
    );
}

/// A savegame without a command log (e.g. after eight hours) gives no recording, only a log warning; the match runs.
// QUIRK E-329: deviation
#[test]
fn save_without_log_is_not_recorded() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_with(&mut r, request(0));
    run_to(&mut r, 20);
    r.key("F1");
    let until = r.now + 2000;
    while !r.files.exists(SaveDir::Main, 0, "quicksave1") {
        assert!(r.now < until, "not written");
        r.frame();
    }
    let bytes = std::fs::read(r.files.path("quicksave1", 0)).unwrap();
    let mut save = GameSaveFile::from_bytes(&bytes).unwrap();
    save.host.log = None;
    save.host.save_replay = true;
    std::fs::write(r.files.path("without", 0), save.to_bytes()).unwrap();
    capture_warnings();
    r.apply(UiEffect::LoadGame {
        name: "without".into(),
    });
    let warnings = take_warnings();
    assert!(r.flow.running(), "loaded");
    assert_eq!(r.view.steps(), save.sim.state.steps);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("No recording") && w.contains("without a command log")),
        "{warnings:?}"
    );
    assert!(r.flow.recorder().is_none());
    assert!(!r.files.autoplay_dir().exists());
}

/// At the limit of eight hours the log ends and with it the recording; the commands before it are in the file.
// QUIRK E-332: deviation
#[test]
fn recording_ends_at_eight_hours() {
    let tmp = tempfile::tempdir().unwrap();
    let mut host = plain_testkit::host();
    host.state.steps = MAX_REPLAY_STEPS - 2;
    let head = ReplayHead {
        start: start_package(&host.rules, &host.setup),
        meta: ReplayMeta {
            level_info: LevelInfoData::default(),
            game_type: 0,
            lock_teams: false,
            game_speed: host.game_speed,
        },
    };
    let mut recorder = ReplayRecorder::begin(tmp.path(), NAME, &head, &host).unwrap();
    let cmd = || Command {
        player: 0,
        kind: opennml_sim::command::CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 0,
        },
    };
    host.step_with(vec![cmd()]);
    host.step_with(vec![cmd()]);
    recorder.sync(&host);
    assert!(!recorder.ended(), "tick 575,999 still belongs");
    assert_eq!(recorder.last_step(), Some(MAX_REPLAY_STEPS - 1));
    host.step_with(vec![cmd()]);
    assert!(!host.log_complete());
    recorder.sync(&host);
    assert!(recorder.ended(), "the recording ends with the log");
    host.step_with(vec![cmd()]);
    recorder.sync(&host);
    let file = ReplayFile::from_bytes(&std::fs::read(recorder.path()).unwrap()).unwrap();
    let steps: Vec<u64> = file.commands.iter().map(|c| c.step).collect();
    assert_eq!(steps, [MAX_REPLAY_STEPS - 2, MAX_REPLAY_STEPS - 1]);
    assert!(!file.torn);
}

/// `savegame/autoplay` is a file - the recording does not begin (log warning), the match runs without a crash to the
/// main menu.
// QUIRK E-335: deviation
#[test]
fn write_error_ends_the_recording_but_not_the_match() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    std::fs::create_dir_all(&r.files.dir).unwrap();
    std::fs::write(r.files.autoplay_dir(), b"not a folder").unwrap();
    capture_warnings();
    start_recorded(&mut r);
    let warnings = take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("Recording") && w.contains("not started")),
        "{warnings:?}"
    );
    assert!(r.flow.recorder().is_none());
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 40);
    quit_to_main_menu(&mut r);
    assert!(r.files.autoplay_dir().is_file(), "the file stays");
}

// Replay in the match.

/// Checksums of a match per tick, collected from the history of the last ticks.
pub(super) type Hashes = std::collections::BTreeMap<u64, u64>;

/// Take over the history of the checksums of the running match into `hashes` (a command executed at once replaces the
/// entry of its tick).
pub(super) fn collect_hashes(run: &Run, hashes: &mut Hashes) {
    if let Some(sim) = run.view.sim() {
        hashes.extend(sim.hashes.iter().copied());
    }
}

/// Frames up to tick `steps` (at most one tick per frame at speed 14), the checksums collected after each frame.
fn run_collecting(r: &mut Run, steps: u64, hashes: &mut Hashes) {
    let until = r.now + 120_000;
    collect_hashes(r, hashes);
    while r.view.steps() < steps {
        assert!(r.now < until, "tick {steps} not reached");
        r.frame();
        collect_hashes(r, hashes);
    }
}

/// Records a match with `SAVEREPLAY`: walk commands, diplomacy through the input (`submit` in the running game, waits
/// for the tick boundary), the last command at tick `last`, then without commands up to `until`. The recording ends
/// without a command (`end_recording` as at program end), then via `QUITGAME` and the accounting to the main menu.
/// Checksums of the match per tick up to `until`.
fn record_match(r: &mut Run, last: u64, until: u64) -> Hashes {
    let mut hashes = Hashes::new();
    run_collecting(r, 0, &mut hashes);
    r.until_step();
    walk(r, 400.0);
    run_collecting(r, last / 3, &mut hashes);
    let diplomacy = diplomacy_command(r);
    r.view.sim_mut().unwrap().submit(diplomacy);
    run_collecting(r, 2 * last / 3, &mut hashes);
    walk(r, -300.0);
    run_collecting(r, last, &mut hashes);
    walk(r, 200.0);
    run_collecting(r, until, &mut hashes);
    r.flow.end_recording(r.view.sim());
    assert!(r.flow.recorder().is_none());
    quit_to_main_menu(r);
    hashes
}

/// Click on row `row` of the list `list` (middle of the row, row height 22 like `Lan::list_row`).
fn list_row(r: &mut Run, dialog: &str, list: &str, row: usize) {
    use opennml_ui::input::{MouseButton as UiButton, UiInput};
    let element = |run: &Run| {
        crate::ui_control::state_data(&run.host.app, super::tests::S1024, run.now)
            .elements
            .into_iter()
            .find(|e| e.dialog == dialog && e.name == list)
            .unwrap_or_else(|| panic!("{dialog}/{list} missing"))
    };
    let e = element(r);
    let [x0, y0, x1, _] = e.rect;
    let (x, y) = ((x0 + x1) / 2, y0 + 11 + 22 * row as i32);
    let app = &mut r.host.app;
    app.input(UiInput::Move { x, y }, super::tests::S1024, r.now);
    app.input(UiInput::Down(UiButton::Left), super::tests::S1024, r.now);
    app.input(UiInput::Up(UiButton::Left), super::tests::S1024, r.now);
    r.idle();
    assert_eq!(
        element(r).selection,
        Some(row as i32),
        "{dialog}/{list}: row {row} of {:?}",
        e.items
    );
}

/// Open `LOADAUTOPLAY` (as from `SINGLEPLAYERMENU`, which closes then: `{0x15}` waits for no open dialog), choose row
/// `row` and `START` (`{3}`, `{0x15}`, `{0x19}`).
fn play_from_menu(r: &mut Run, row: usize) {
    r.host.app.push_command(opennml_ui::msg::AppCmd::CloseAll);
    r.host.app.push_command(opennml_ui::msg::AppCmd::Open {
        name: "loadautoplay".into(),
        wait: true,
    });
    r.idle();
    assert!(
        r.dialogs().iter().any(|d| d == "loadautoplay"),
        "{:?}",
        r.dialogs()
    );
    list_row(r, "loadautoplay", "dir", row);
    r.click("loadautoplay", "start");
    r.idle();
}

/// The message `MSGBOX` over the menu: its text ID, `None` without a message.
fn message(r: &Run) -> Option<i32> {
    if !r.dialogs().iter().any(|d| d == "msgbox") {
        return None;
    }
    r.host.app.screen_state().info_text
}

/// E-331, E-333: Recording with walk commands and diplomacy, last command at tick 100, match up to 300;
/// `LOADAUTOPLAY` -> row -> `START` plays it back. The checksum equals the match at every tick 1 ... 300, also after
/// the end of the file at tick 100: AI and scripts recompute and continue in the replay.
// QUIRK E-331: kept
// QUIRK E-333: deviation
#[test]
fn playback_repeats_the_recorded_hashes() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    let live = record_match(&mut r, 100, 300);
    let file = read_recording(&r, NAME);
    assert_eq!(
        file.commands.last().unwrap().step,
        100,
        "end of file at tick 100"
    );
    assert!(
        file.commands.iter().any(|c| matches!(
            c.command.kind,
            opennml_sim::command::CommandKind::Diplomacy { .. }
        )),
        "diplomacy in the recording"
    );
    assert!(!r.flow.running());
    play_from_menu(&mut r, 0);
    assert!(r.flow.running(), "replay running");
    assert_eq!(r.view.playback_begun, Some(file.commands.len()));
    assert!(r.view.net().is_none(), "no network session");
    let mut played = Hashes::new();
    run_collecting(&mut r, 300, &mut played);
    for step in 1..=300 {
        assert!(live.contains_key(&step), "match: tick {step} missing");
        assert_eq!(played.get(&step), live.get(&step), "tick {step}");
    }
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.log, file.commands, "exactly the commands of the file");
    assert_eq!(
        sim.playback(),
        Some(crate::game::sim_host::PlaybackStatus {
            next_step: None,
            ended: true
        })
    );
}

/// Deviates from E-339 (E-341 as in the original): In the replay the selection is locked and empty; commands of the
/// input and of the control channel (`sim_command`) have no effect - the checksum stays that of the recording.
/// `QUITGAME` gives up the view slot: defeat, accounting, main menu.
// QUIRK E-341: kept
// QUIRK E-339: deviation
#[test]
fn playback_blocks_selection_and_commands_but_not_surrender() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    let live = record_match(&mut r, 30, 60);
    play_from_menu(&mut r, 0);
    r.flow.begin_round();
    r.flow.begin_round();
    let game = r.view.game.as_ref().unwrap();
    assert!(game.selection.locked, "selection locked");
    assert!(game.selection.ids.is_empty());
    r.until_step();
    // Walk command (right-click path `move_units`), diplomacy through the control channel and at once: none of it goes
    // through.
    walk(&mut r, 250.0);
    let diplomacy = diplomacy_command(&r);
    let sim = r.view.sim_mut().unwrap();
    sim.control_submit(diplomacy.clone());
    sim.execute_now(diplomacy);
    sim.press_button(7);
    assert!(sim.pending_commands().is_empty(), "nothing accepted");
    let mut played = Hashes::new();
    run_collecting(&mut r, 50, &mut played);
    for (step, hash) in played.iter().filter(|(s, _)| **s >= 1) {
        assert_eq!(Some(hash), live.get(step), "tick {step}");
    }
    assert_eq!(r.view.sim().unwrap().log, read_recording(&r, NAME).commands);
    // Giving up goes through (at once, as in the game).
    quit_to_main_menu(&mut r);
}

/// E-341: A recorded defeat of the view slot (`QUITGAME` of the match, executed at once and in the file with tick S)
/// comes in the replay from the file at the transition S -> S+1 and shows the end screen as in the game; "Defeat"
/// leads via the accounting to the main menu.
// QUIRK E-341: kept
#[test]
fn recorded_defeat_ends_the_playback() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 40);
    let slot = r.view.sim().unwrap().active_player;
    quit_to_main_menu(&mut r);
    let file = read_recording(&r, NAME);
    let last = file.commands.last().expect("command in the file");
    assert_eq!(
        last.command.kind,
        opennml_sim::command::CommandKind::SetPlayerResult {
            slot,
            result: opennml_sim::game_end::DEFEAT
        },
        "the surrender is in the file"
    );
    let surrender_step = last.step;
    play_from_menu(&mut r, 0);
    assert!(r.flow.running(), "replay running");
    r.flow.begin_round();
    r.flow.begin_round();
    assert_eq!(r.host.app.game().unwrap().local_slot, slot);
    let until = r.now + 60_000;
    while !r.dialogs().iter().any(|d| d == "gameend_defeat") {
        assert!(r.now < until, "no end screen");
        assert!(
            r.view.steps() <= surrender_step + 1,
            "tick {} without end screen",
            r.view.steps()
        );
        r.frame();
    }
    assert_eq!(
        r.view.steps(),
        surrender_step + 1,
        "the defeat comes from the file"
    );
    assert_eq!(
        r.view.sim().unwrap().state.players[usize::from(slot)].result,
        opennml_sim::game_end::DEFEAT
    );
    assert_eq!(
        r.view.sim().unwrap().log,
        file.commands,
        "no command from the input"
    );
    r.idle();
    r.click("gameend_defeat", "end_defeat");
    r.idle();
    assert!(
        r.dialogs().iter().any(|d| d == "accounting"),
        "{:?}",
        r.dialogs()
    );
    r.click("accounting", "mainmenu");
    r.idle();
    assert!(!r.flow.running(), "replay ended");
    assert!(r.view.game.is_none());
}

/// Deviates from E-339: F1, F4 and the script autosave 508 do nothing in the replay; in the game menu `savegame3` and
/// `load` are locked, giving up and options are not.
// QUIRK E-339: deviation
#[test]
fn playback_blocks_saving_and_loading() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    record_match(&mut r, 20, 30);
    play_from_menu(&mut r, 0);
    r.flow.begin_round();
    r.flow.begin_round();
    assert!(r.host.app.game().unwrap().playback, "GameInfo::playback");
    r.until_step();
    let saves = |r: &Run| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&r.files.dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    r.key("F1");
    for _ in 0..30 {
        r.frame();
    }
    assert_eq!(saves(&r), ["autoplay"], "F1 does not save");
    r.key("F4");
    r.idle();
    assert_eq!(r.state(), 0, "F4 opens nothing");
    assert!(!r.dialogs().iter().any(|d| d == "ingameloadgame"));
    // Script autosave 508 (action `QuickSave` of the presentation, as from the level script).
    capture_warnings();
    {
        let g = r.view.game.as_mut().unwrap();
        g.presentation.apply_host(
            &opennml_sim::script::present::Presentation::QuickSave,
            &mut g.host,
            &mut g.selection,
        );
    }
    for _ in 0..10 {
        r.frame();
    }
    let warnings = take_warnings();
    assert!(
        !warnings.iter().any(|w| w.contains("campaign entry")),
        "{warnings:?}"
    );
    assert_eq!(saves(&r), ["autoplay"], "508 does not save");
    // Game menu: saving and loading locked.
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1, "game menu open");
    let data = crate::ui_control::state_data(&r.host.app, super::tests::S1024, r.now);
    let enabled = |name: &str| {
        data.elements
            .iter()
            .find(|e| e.dialog == "ingameoptions3" && e.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .enabled
    };
    assert!(!enabled("savegame3"));
    assert!(!enabled("load"));
    assert!(enabled("quitgame"));
    assert!(enabled("options"));
}

/// A `LoadGame` effect that does not come from the locked menu (control channel) loads nothing in the replay either;
/// the replay continues.
#[test]
fn playback_ignores_load_game_effect() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    run_to(&mut r, 20);
    r.key("F1");
    let until = r.now + 2000;
    while !r.files.exists(SaveDir::Main, 0, "quicksave1") {
        assert!(r.now < until, "not written");
        r.frame();
    }
    record_match(&mut r, 40, 60);
    play_from_menu(&mut r, 0);
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    assert!(r.flow.playing_back(), "replay running");
    let steps = r.view.steps();
    r.apply(UiEffect::LoadGame {
        name: "quicksave1".into(),
    });
    assert!(r.flow.playing_back(), "replay continues");
    assert!(r.view.sim().unwrap().playback().is_some());
    assert!(r.view.steps() >= steps, "nothing loaded");
}

/// E-344: The replay shows everything (fog off, `FOG_OFF`) and hides the command bar (`{0x11, 1}`); the selection is
/// empty and locked. Flow via the fake, bar and fog of the real session via `GameSession::begin_playback` (graphics
/// card, otherwise silently skipped).
// QUIRK E-331, E-344: kept
#[test]
fn playback_shows_everything_and_hides_the_command_bar() {
    use crate::game::fog_view::FOG_OFF;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    record_match(&mut r, 20, 30);
    play_from_menu(&mut r, 0);
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.fog.show_mode, FOG_OFF, "fog off");
    assert!(sim.playback().is_some());
    assert!(r.view.game.as_ref().unwrap().selection.locked);

    let Some(mut m) = super::gameui_tests::Match::start() else {
        return;
    };
    let session = m.session.as_mut().unwrap();
    let toolbar = session.hud.toolbar_window().expect("command bar");
    assert!(session.hud.windows.visible(toolbar), "visible before");
    session.begin_playback(Vec::new());
    assert!(session.toolbar_host.hidden(), "{{0x11, 1}}");
    assert!(session.selection.locked);
    assert!(session.selection.ids.is_empty());
    assert_eq!(session.sim().unwrap().fog.show_mode, FOG_OFF);
    for _ in 0..5 {
        m.frame();
    }
    let session = m.session();
    assert!(
        !session.hud.windows.visible(toolbar),
        "the bar stays hidden"
    );
    assert_eq!(session.sim().unwrap().fog.show_mode, FOG_OFF);
}

/// Deviates from E-338: In the replay the view slot keeps the recorded name, not the `NetName` of the machine at
/// replay time.
// QUIRK E-338: deviation
#[test]
fn playback_keeps_the_recorded_name_of_the_local_player() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let mut req = recorded_request();
    req.slots[0].name = "Anna".into();
    start_with(&mut r, req);
    record_match(&mut r, 20, 30);
    let units: Vec<u16> = "Bert".encode_utf16().collect();
    r.host.app.set_net_name(&units);
    play_from_menu(&mut r, 0);
    let sim = r.view.sim().unwrap();
    let slot = sim.active_player;
    assert_eq!(sim.state.players[usize::from(slot)].name, "Anna");
    assert_eq!(
        player_display_name(sim, &r.host.layer.texts, i32::from(slot)),
        "Anna"
    );
    assert_eq!(r.host.app.game().unwrap().local_slot, slot);
}

/// E-340: The replay begins with the speed from the header, not with `GameSpeed` of the settings; after closing the
/// game menu the own `GameSpeed` applies again (as in the original).
// QUIRK E-340: kept
#[test]
fn playback_starts_with_the_recorded_speed() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.host.app.set_game_speed(7);
    start_recorded(&mut r);
    assert_eq!(r.view.sim().unwrap().game_speed, 7);
    record_match(&mut r, 20, 30);
    assert_eq!(read_recording(&r, NAME).head.meta.game_speed, 7);
    r.host.app.set_game_speed(18);
    play_from_menu(&mut r, 0);
    assert_eq!(
        r.view.sim().unwrap().game_speed,
        7,
        "speed of the recording"
    );
    r.flow.begin_round();
    r.flow.begin_round();
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    r.click("ingameoptions3", "return");
    r.idle();
    assert_eq!(r.view.sim().unwrap().game_speed, 18, "own GameSpeed");
}

/// O-153: A speed above 20 (30) is recorded and replayed without clamping.
// QUIRK O-153: deviation
#[test]
fn playback_keeps_a_speed_above_20() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.host.app.set_game_speed(30);
    start_recorded(&mut r);
    assert_eq!(r.view.sim().unwrap().game_speed, 30);
    record_match(&mut r, 20, 30);
    assert_eq!(read_recording(&r, NAME).head.meta.game_speed, 30);
    play_from_menu(&mut r, 0);
    assert_eq!(
        r.view.sim().unwrap().game_speed,
        30,
        "speed of the recording"
    );
}

/// The replay never records, even with `SaveReplay 1`; the played file stays unchanged.
#[test]
fn playback_records_nothing() {
    let Some(mut r) = Run::corpus_configured(MenuOptions::default(), |c| c.save_replay = 1) else {
        return;
    };
    start_recorded(&mut r);
    record_match(&mut r, 20, 30);
    let bytes = std::fs::read(path_of(&r, NAME)).unwrap();
    r.flow.set_fixed_clock(Some(LATER));
    play_from_menu(&mut r, 0);
    assert!(r.flow.running());
    for _ in 0..60 {
        r.frame();
    }
    assert!(r.flow.recorder().is_none(), "no recording");
    quit_to_main_menu(&mut r);
    assert_eq!(recordings(&r), [NAME]);
    assert_eq!(std::fs::read(path_of(&r, NAME)).unwrap(), bytes);
}

/// On a map with a UI condition (35) the match mirrors the selection as `ScriptSelection`; in the replay the mirror
/// commands come only from the file - the live selection gives nothing into the simulation, the checksum equals the
/// recording at every tick.
// QUIRK E-333, E-339: deviation
#[test]
fn playback_does_not_mirror_the_live_selection() {
    use crate::game::options::DEFAULT_GAME_SPEED;
    use crate::game::testworld::{abl, script_rules};
    use opennml_sim::id::ObjectId;
    let level = abl(&[(&["&35;x"], &["n10"])]);
    let host = || {
        let rules = script_rules(&level);
        let mut setup = opennml_sim::setup::MatchSetup::default_for(&rules);
        setup.script.enabled = true;
        SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap()
    };
    let (a, b, c) = (ObjectId(0x800), ObjectId(0x801), ObjectId(0x802));
    let mut live = host();
    assert!(live.rules.scripts.uses_ui_conditions);
    let mut hashes = Vec::new();
    for step in 0..40u64 {
        match step {
            5 => live.sync_selection(&[a, b]),
            12 => live.sync_selection(&[a]),
            20 => live.sync_selection(&[]),
            _ => {}
        }
        live.step_once();
        hashes.push(live.checksum());
    }
    let mirrored = live
        .log
        .iter()
        .filter(|c| {
            matches!(
                c.command.kind,
                opennml_sim::command::CommandKind::ScriptSelection { .. }
            )
        })
        .count();
    assert_eq!(mirrored, 3, "three mirror commands recorded");
    let mut play = host();
    play.begin_playback(live.log.clone());
    for (step, hash) in hashes.iter().enumerate() {
        // The live selection of the replay deviates: it gives no command.
        play.sync_selection(&[c, b, a][..step % 3]);
        assert!(play.pending_commands().is_empty(), "tick {step}");
        play.step_once();
        assert_eq!(play.checksum(), *hash, "tick {}", step + 1);
    }
    assert_eq!(play.log, live.log);
    assert_eq!(
        play.state.script.mission.selection,
        live.state.script.mission.selection
    );
}

/// A torn recording (cut in the last section) plays up to the last whole section, with a log warning.
// QUIRK E-343: deviation
#[test]
fn torn_recording_plays_to_the_last_whole_chunk() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_recorded(&mut r);
    let live = record_match(&mut r, 60, 80);
    let bytes = std::fs::read(path_of(&r, NAME)).unwrap();
    let whole = ReplayFile::from_bytes(&bytes).unwrap();
    let cut = &bytes[..bytes.len() - 3];
    let torn = ReplayFile::from_bytes(cut).unwrap();
    assert!(torn.torn);
    assert!(torn.commands.len() < whole.commands.len());
    assert!(whole.commands.starts_with(&torn.commands));
    let missing = whole.commands[torn.commands.len()].step;
    std::fs::write(path_of(&r, NAME), cut).unwrap();
    capture_warnings();
    play_from_menu(&mut r, 0);
    let warnings = take_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("truncated")),
        "{warnings:?}"
    );
    assert!(r.flow.running());
    let mut played = Hashes::new();
    run_collecting(&mut r, 80, &mut played);
    assert_eq!(r.view.sim().unwrap().log, torn.commands);
    for step in 1..=missing {
        assert_eq!(played.get(&step), live.get(&step), "tick {step}");
    }
}

/// Deviates from E-343: A broken header, another `FORMAT_VERSION`, an `NMLREPL` replay, game type < 0 and a missing
/// file each give the message 99 109 over the reopened `LOADAUTOPLAY`, a log entry with the reason, and the menu
/// level continues. If the start package does not fit the game data, that does not show until after loading the map:
/// failed start, `LOADAUTOPLAY` with 99 109, the menu level is off and restarts with `START`.
// QUIRK E-343: deviation
#[test]
fn broken_recording_keeps_the_menu_alive() {
    use crate::replay_file::head_bytes;
    use opennml_ui::own_texts::REPLAY_NOT_PLAYABLE;
    // A valid recording from another match.
    let Some(mut rec) = Run::corpus() else {
        return;
    };
    start_recorded(&mut rec);
    record_match(&mut rec, 20, 30);
    let good = std::fs::read(path_of(&rec, NAME)).unwrap();
    let file = ReplayFile::from_bytes(&good).unwrap();
    let body_len = u32::from_le_bytes(good[16..20].try_into().unwrap()) as usize;
    let with_head = |head: &ReplayHead| {
        let mut bytes = head_bytes(head);
        bytes.extend_from_slice(&good[28 + body_len..]);
        bytes
    };
    let mut broken = good.clone();
    broken[30] ^= 0xFF;
    let mut version = good.clone();
    version[12..16].copy_from_slice(&(opennml_sim::setup::FORMAT_VERSION + 1).to_le_bytes());
    let mut campaign = file.head.clone();
    campaign.meta.game_type = -1;
    let mut rules = file.head.clone();
    rules.start.rules_checksum ^= 1;
    assert_eq!(
        ReplayFile::from_bytes(&with_head(&rules)).unwrap().commands,
        file.commands,
        "only the start package deviates"
    );

    let Some(mut r) = Run::corpus_configured(
        MenuOptions {
            start_menu: true,
            ..MenuOptions::default()
        },
        |c| c.is_menu_level = true,
    ) else {
        return;
    };
    r.view.real_levels = true;
    r.idle();
    assert!(r.flow.menu_level().is_some(), "menu level running");
    std::fs::create_dir_all(r.files.autoplay_dir()).unwrap();
    for (name, bytes) in [
        ("a-broken", broken),
        ("b-version", version),
        ("c-nmlrepl", file.replay().to_bytes()),
        ("d-campaign", with_head(&campaign)),
        ("e-gamedata", with_head(&rules)),
    ] {
        std::fs::write(path_of(&r, name), bytes).unwrap();
    }
    let ends = r.view.ends;
    // Checked before loading: the menu level continues. The first case through the list, the others as an effect.
    for (row, name, reason) in [
        (Some(0), "a-broken", "checksum"),
        (None, "b-version", "format version"),
        (None, "c-nmlrepl", "identifier"),
        (None, "d-campaign", "game type"),
        (None, "missing", "reading"),
    ] {
        capture_warnings();
        match row {
            Some(row) => play_from_menu(&mut r, row),
            None => {
                r.apply(UiEffect::PlayReplay { name: name.into() });
                r.idle();
            }
        }
        let warnings = take_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(name) && w.contains(reason)),
            "{name}: {warnings:?}"
        );
        assert!(!r.flow.running(), "{name}");
        assert!(r.flow.menu_level().is_some(), "{name}: menu level running");
        assert_eq!(r.view.ends, ends, "{name}: session not ended");
        assert_eq!(message(&r), Some(REPLAY_NOT_PLAYABLE), "{name}");
        assert!(r.dialogs().iter().any(|d| d == "loadautoplay"), "{name}");
        r.click("msgbox", "ok");
        r.idle();
    }
    // Wrong game data: not detected until loading.
    capture_warnings();
    r.apply(UiEffect::PlayReplay {
        name: "e-gamedata".into(),
    });
    r.idle();
    let warnings = take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("e-gamedata") && w.contains("game data")),
        "{warnings:?}"
    );
    assert!(!r.flow.running());
    assert!(r.flow.menu_level().is_none(), "menu level off");
    assert_eq!(message(&r), Some(REPLAY_NOT_PLAYABLE));
    assert!(r.dialogs().iter().any(|d| d == "loadautoplay"));
    // `START` restarts the menu level.
    r.click("msgbox", "ok");
    r.idle();
    r.click("loadautoplay", "cancel");
    r.idle();
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    r.click("singleplayermenu", "cancel");
    r.idle();
    assert!(
        r.dialogs().iter().any(|d| d == "start"),
        "{:?}",
        r.dialogs()
    );
    assert!(r.flow.menu_level().is_some(), "menu level back");
}

// Control channel.

/// `ui_state.game.replay` of the running match ([`GameFlow::game_data`]).
fn replay_data(r: &Run) -> Option<opennml_control::UiReplayData> {
    r.flow
        .game_data(
            &r.host.app,
            r.view.sim(),
            r.view.level(),
            None,
            None,
            None,
            None,
            Vec::new(),
        )
        .and_then(|d| d.replay)
}

/// `ui_state.game.replay` shows recording and replay. Without `SAVEREPLAY` the field is missing. While recording mode
/// `record`, file name (without folder) and the tick of the simulation at the last catch-up - the file reaches that
/// far, even if the last command lies earlier. In the replay mode `play` with the tick of the next command of the
/// file, after the end of the file `ended` and the current tick.
#[test]
fn ui_state_shows_recording_and_playback() {
    use opennml_control::{UiReplayData, UiReplayMode};
    let Some(mut r) = Run::corpus() else {
        return;
    };
    assert_eq!(replay_data(&r), None, "without a match");
    start_with(&mut r, request(0));
    run_to(&mut r, 10);
    assert_eq!(replay_data(&r), None, "without SAVEREPLAY");
    quit_to_main_menu(&mut r);

    start_recorded(&mut r);
    assert_eq!(
        replay_data(&r),
        Some(UiReplayData {
            mode: UiReplayMode::Record,
            file: NAME.into(),
            step: 0,
            ended: false,
        })
    );
    r.until_step();
    walk(&mut r, 400.0);
    run_to(&mut r, 20);
    walk(&mut r, -300.0);
    run_to(&mut r, 40);
    let data = replay_data(&r).expect("recording");
    assert_eq!(data.step, r.view.steps(), "caught up to the current tick");
    let last = r.flow.recorder().unwrap().last_step().expect("commands");
    assert!(last < data.step, "last command at {last}");
    assert!(!data.ended);
    r.flow.end_recording(r.view.sim());
    assert_eq!(replay_data(&r), None, "recording ended");
    quit_to_main_menu(&mut r);

    let steps: Vec<u64> = read_recording(&r, NAME)
        .commands
        .iter()
        .map(|c| c.step)
        .collect();
    play_from_menu(&mut r, 0);
    r.flow.begin_round();
    r.flow.begin_round();
    let next = |now: u64| steps.iter().copied().find(|&s| s >= now);
    let data = replay_data(&r).expect("replay");
    assert_eq!(
        data,
        UiReplayData {
            mode: UiReplayMode::Play,
            file: NAME.into(),
            step: next(r.view.steps()).unwrap(),
            ended: false,
        }
    );
    run_to(&mut r, steps[0] + 1);
    let data = replay_data(&r).unwrap();
    assert_eq!(Some(data.step), next(r.view.steps()), "next command");
    assert!(!data.ended);
    let end = *steps.last().unwrap();
    run_to(&mut r, end + 5);
    let data = replay_data(&r).unwrap();
    assert_eq!(
        (data.mode, data.step, data.ended),
        (UiReplayMode::Play, r.view.steps(), true),
        "end of file at tick {end}"
    );
    quit_to_main_menu(&mut r);
    assert_eq!(replay_data(&r), None);
}
