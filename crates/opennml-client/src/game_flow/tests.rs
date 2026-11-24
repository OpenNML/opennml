//! Tests of the match flow: `GameFlow` with the fake `FakeSession` (`SimHost` without a map) over a menu host on the
//! corpus dialogs (`NML_GAME_DIR`, otherwise silently skipped), as the shell in `opennml-game` drives a frame;
//! `save_then_load_restores_checksum` with a real `GameSession` without a window.

use std::rc::Rc;

use opennml_control::{Command, hex_checksum};
use opennml_ui::app::MenuOptions;
use opennml_ui::draft::{DraftSettings, SlotDraft};
use opennml_ui::loading::{DEFAULT_LDG, MULTIPLAYER_LDG};
use opennml_ui::saves::{SaveDir, SaveFiles};

use super::*;
use crate::game::hotkeys::{HotkeyCommand, Hotkeys, KeyContext};
use crate::keys::MouseButton;
use crate::menu_host::MenuHost;
use crate::testutil::test_config;
use crate::testutil::{FakeAudio, FakeSession, capture_warnings, take_warnings};
use crate::ui_control::{execute_ui, state_data};
use crate::ui_layer::UiLayer;

pub(super) const S1024: Screen = Screen {
    width: 1024,
    height: 768,
};

/// Small multiplayer map with two slots (without its own `.ldg`).
pub(super) const TWO_PLAYERS: &str = "data/levels/multiplayer/[2players]-tintebariver.lvl";

pub(super) fn slot(key: i32, kind: i32, team: i32, color: i32) -> SlotDraft {
    SlotDraft {
        key,
        name: format!("S{key}"),
        kind,
        nation: 1,
        team,
        handicap: 0,
        color,
    }
}

/// Start request: free match of game type `game_type` on [`TWO_PLAYERS`], human and one computer.
pub(super) fn request(game_type: i32) -> MatchRequest {
    MatchRequest {
        level: TWO_PLAYERS.into(),
        game_type,
        slots: vec![slot(-1, 999, 0, 0), slot(0, 2, 1, 1)],
        settings: DraftSettings {
            game_type,
            unit_limit: 2,
            // Default from `Engine.ini`.
            time_limit: 20,
            ..DraftSettings::default()
        },
        info: LevelInfoData {
            name: "Ink".into(),
            path: TWO_PLAYERS.into(),
            size_class: 0,
            players: 2,
            level_type: 0,
            ..LevelInfoData::default()
        },
    }
}

/// Local time of the tests for the name of a recording ([`GameFlow::set_fixed_clock`]): `261108-1435`.
pub(super) const TEST_CLOCK: opennml_ui::saves::LocalStamp = opennml_ui::saves::LocalStamp {
    year: 2026,
    month: 11,
    day: 8,
    hour: 14,
    minute: 35,
};

/// Host with flow and fake: menu without main menu (no dialog open), savegames in a throwaway folder.
pub(super) struct Run {
    pub(super) vfs: Vfs,
    pub(super) host: MenuHost,
    pub(super) flow: GameFlow,
    pub(super) view: FakeSession,
    pub(super) files: FsSaveFiles,
    pub(super) audio: FakeAudio,
    pub(super) now: u64,
    /// Drawn loading screens (`.ldg`, percent).
    pub(super) shown: Vec<(String, u8)>,
    _tmp: tempfile::TempDir,
}

impl Run {
    pub(super) fn corpus() -> Option<Run> {
        Run::corpus_with(MenuOptions::default())
    }

    /// Like [`Run::corpus`] with own options of the menu (e.g. profiles); the savegames always lie in the throwaway
    /// folder.
    pub(super) fn corpus_with(base: MenuOptions) -> Option<Run> {
        Run::corpus_configured(base, |_| {})
    }

    /// Like [`Run::corpus_with`], the settings changed by `adjust` before the start of the menu.
    pub(super) fn corpus_configured(
        base: MenuOptions,
        adjust: impl FnOnce(&mut EngineConfig),
    ) -> Option<Run> {
        Run::corpus_in(base, None, adjust)
    }

    /// Like [`Run::corpus_configured`] with the savegame folder `saves` of the menu storage (otherwise `savegame` in
    /// the throwaway folder): a network machine gives the folder of its network session here (`NetPaths::saves`) so
    /// that the list of `LOADSAVEDNETWORKGAME` sees the network savegames it writes - like `opennml-game`.
    pub(super) fn corpus_in(
        base: MenuOptions,
        saves: Option<std::path::PathBuf>,
        adjust: impl FnOnce(&mut EngineConfig),
    ) -> Option<Run> {
        let dir = opennml_rda::testutil::corpus_game_dir()?;
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let files = FsSaveFiles {
            dir: saves.unwrap_or_else(|| tmp.path().join("savegame")),
        };
        let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut config = EngineConfig {
            is_intro: false,
            ..test_config(&vfs)
        };
        adjust(&mut config);
        let opts = MenuOptions {
            saves: Some(Rc::new(files.clone())),
            ..base
        };
        let audio = FakeAudio::default();
        let host = MenuHost::new(&vfs, layer, config, opts, &audio, None).unwrap();
        let mut flow = GameFlow::new(&host.layer.texts);
        flow.set_hotkeys(Hotkeys::load(&vfs));
        // Fixed local time for the names of the recordings.
        flow.set_fixed_clock(Some(TEST_CLOCK));
        Some(Run {
            vfs,
            host,
            flow,
            view: FakeSession::default(),
            files,
            audio,
            now: 0,
            shown: Vec::new(),
            _tmp: tmp,
        })
    }

    /// A game effect like from `HostRequests::game`.
    pub(super) fn apply(&mut self, effect: UiEffect) {
        let env = FlowEnv {
            vfs: &self.vfs,
            texts: &self.host.layer.texts,
            files: &self.files,
        };
        let shown = &mut self.shown;
        self.flow.apply_all(
            vec![effect],
            &mut self.host.app,
            &mut self.view,
            &env,
            &mut |ldg, phase| shown.push((ldg.to_string(), phase.percent())),
        );
    }

    /// A frame like `opennml-game`: menu frame, keys, effects, game effects, steps; then on by 16 ms.
    pub(super) fn frame(&mut self) -> u32 {
        self.host.frame_cmds(&self.vfs, S1024, self.now);
        self.flow.keys(&mut self.host.app, &mut self.view, self.now);
        let req = self.host.apply_effects(&mut self.audio, &self.vfs);
        let env = FlowEnv {
            vfs: &self.vfs,
            texts: &self.host.layer.texts,
            files: &self.files,
        };
        let shown = &mut self.shown;
        self.flow.apply_all(
            req.game,
            &mut self.host.app,
            &mut self.view,
            &env,
            &mut |ldg, phase| shown.push((ldg.to_string(), phase.percent())),
        );
        // Network session; has no effect without a session.
        self.flow.net_step(
            self.now,
            &mut self.host.app,
            &mut self.view,
            &env,
            &mut |ldg, phase| shown.push((ldg.to_string(), phase.percent())),
        );
        let steps = self.flow.frame(
            self.now,
            &mut self.host.app,
            &mut self.view,
            &self.host.layer.texts,
        );
        self.now += 16;
        steps
    }

    /// Frames until the UI rests (at most 3 s), and one more; sum of the steps.
    pub(super) fn idle(&mut self) -> u32 {
        let until = self.now + 3000;
        let mut steps = self.frame();
        let busy = |r: &Run| {
            let app = &r.host.app;
            app.menu().busy() || !app.queue().is_empty() || app.queue().waiting()
        };
        while busy(self) {
            assert!(self.now < until, "UI does not come to rest");
            steps += self.frame();
        }
        steps + self.frame()
    }

    /// Frames until one had at least one step.
    pub(super) fn until_step(&mut self) {
        let until = self.now + 1000;
        while self.frame() == 0 {
            assert!(self.now < until, "no step");
        }
    }

    pub(super) fn ui(&mut self, command: Command) {
        execute_ui(&mut self.host.app, &command, S1024, self.now).unwrap();
    }

    pub(super) fn key(&mut self, chord: &str) {
        self.ui(Command::UiKey {
            chord: chord.into(),
            char: None,
        });
    }

    pub(super) fn click(&mut self, dialog: &str, element: &str) {
        self.ui(Command::UiClick {
            dialog: dialog.into(),
            element: element.into(),
            button: None,
        });
    }

    pub(super) fn dialogs(&self) -> Vec<String> {
        state_data(&self.host.app, S1024, self.now)
            .dialogs
            .into_iter()
            .map(|d| d.name)
            .collect()
    }

    pub(super) fn start(&mut self) {
        self.apply(UiEffect::StartMatch(request(0)));
        assert!(self.flow.running(), "match running");
        self.flow.begin_round();
        self.flow.begin_round();
    }

    pub(super) fn state(&self) -> u8 {
        self.host.app.game_menu_state()
    }
}

// QUIRK O-139: kept
#[test]
fn start_match_shows_loading_phases_then_runs() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    assert!(!r.flow.running());
    assert_eq!(r.frame(), 0, "no steps without a match");
    r.apply(UiEffect::StartMatch(request(0)));
    // Multiplayer map without its own `.ldg`, restart, game type 0 -> `multiplayer.ldg`; phases 0/40/70/100.
    let phases: Vec<(String, u8)> = [0, 40, 70, 100]
        .into_iter()
        .map(|p| (MULTIPLAYER_LDG.to_string(), p))
        .collect();
    assert_eq!(r.shown, phases);
    assert_eq!(r.flow.loading(), None, "loading over");
    assert_eq!(r.view.level(), TWO_PLAYERS);
    // Setup from `to_match_setup`, speed `GameSpeed` (default 14).
    let setup = r.view.last_setup.clone().unwrap();
    assert_eq!(setup.lobby.len(), 2);
    assert_eq!(r.view.sim().unwrap().game_speed, 14);
    let game = *r.host.app.game().unwrap();
    assert_eq!(game.game_type, 0);
    assert_eq!(game.local_slot, r.view.sim().unwrap().active_player);
    assert_eq!(r.state(), 0);
    // The match runs: steps within one second (speed 14: 1.2 ticks per 50 ms).
    let mut steps = 0;
    for _ in 0..63 {
        steps += r.frame();
    }
    assert!((22..=26).contains(&steps), "{steps}");
    assert_eq!(r.view.steps(), u64::from(steps));
}

/// Visible background images of the desktop (`ui_state.images`) at the clock of the run.
fn desktop_images(r: &Run) -> Vec<(String, u8)> {
    state_data(&r.host.app, S1024, r.now).images
}

// QUIRK D-255: kept
#[test]
fn starting_a_level_drops_the_desktop_images() {
    // `Game_StartLevel` step 2 (`MenuManager_CloseAllAndStopMusic` `0x0045D0F0`): the image table of the desktop is
    // emptied before loading (`RemoveAll` on `+0x94`). So the background image of the menu does not fade out over the
    // world - for `0x0D` (free play) as for `0x0C` (campaign level).
    for campaign in [false, true] {
        let Some(mut r) = Run::corpus() else {
            return;
        };
        r.ui(Command::UiOpen {
            dialog: "singleplayermenu".into(),
        });
        r.idle();
        assert!(!desktop_images(&r).is_empty(), "menu with background image");
        if campaign {
            r.view.real_levels = true;
            r.apply(UiEffect::StartCampaignLevel {
                level: KAMP1_1.into(),
                difficulty: 0,
            });
        } else {
            r.apply(UiEffect::StartMatch(request(0)));
        }
        assert!(r.flow.running(), "match running");
        assert_eq!(
            desktop_images(&r),
            [],
            "campaign {campaign}: right after loading"
        );
        // The host discards the menu commands from before loading (exactly once).
        assert!(r.flow.take_menus_closed());
        assert!(!r.flow.take_menus_closed());
        r.frame();
        assert_eq!(
            desktop_images(&r),
            [],
            "campaign {campaign}: in the first frame"
        );
        // `0x1F` also starts `Game_StartLevel` via `FUN_004C8EC0`.
        r.apply(UiEffect::ReloadLevel);
        assert!(r.flow.take_menus_closed(), "campaign {campaign}: restart");
    }
}

#[test]
fn failed_start_reopens_the_singleplayer_menu() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    capture_warnings();
    // Empty map path (draft without a map, O-188): no loading, message, `singleplayermenu` open again.
    let mut empty = request(0);
    empty.level.clear();
    r.apply(UiEffect::StartMatch(empty));
    assert!(r.shown.is_empty(), "no loading screen");
    // Map not loadable: loading screen at 0 %, then message.
    r.view.fail_level = Some(TWO_PLAYERS.into());
    r.apply(UiEffect::StartMatch(request(0)));
    assert_eq!(r.shown, [(MULTIPLAYER_LDG.to_string(), 0)]);
    r.view.fail_level = None;
    let warnings = take_warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("no map chosen"), "{warnings:?}");
    assert!(warnings[1].contains("not readable"), "{warnings:?}");
    assert!(!r.flow.running());
    assert!(r.host.app.game().is_none());
    assert!(r.view.game.is_none());
    // Menu music: stopped before loading, on again after the failure without a match (the empty path does not load).
    assert_eq!(r.view.music, ["stop", "restart"]);
    r.idle();
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
}

// QUIRK O-143, O-152: kept
#[test]
fn escape_opens_the_menu_one_step_later_and_pauses() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    // ESC right after a frame with a tick: the following frames without a tick open nothing, the first with a tick
    // opens the game menu (flag `Engine+0x4F1`, O-152).
    r.key("ESCAPE");
    let mut without = 0;
    loop {
        let steps = r.frame();
        if steps == 0 {
            assert_eq!(r.state(), 0, "no menu without a tick");
            without += 1;
            continue;
        }
        assert_eq!(r.state(), 1, "the menu opens in the frame with a tick");
        break;
    }
    assert!(without >= 1, "the tick does not come until one frame later");
    let steps = r.view.steps();
    assert_eq!(r.idle(), 0, "the game menu halts");
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    for _ in 0..120 {
        assert_eq!(r.frame(), 0);
    }
    assert_eq!(r.view.steps(), steps);
    // `RETURN` -> `{0x0F}`: on without catching up.
    r.click("ingameoptions3", "return");
    r.idle();
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
    let mut later = 0;
    for _ in 0..30 {
        later += r.frame();
    }
    assert!((9..=13).contains(&later), "{later}");
    // ESC with the menu open sets no flag: after closing nothing opens by itself.
    r.key("F4");
    r.idle();
    assert_eq!(r.dialogs(), ["ingameloadgame"]);
    // An ESC that no widget took, in state 1: no flag (`GameEngine_Update` clears it in the halt branch).
    r.flow.key("ESCAPE", &mut r.host.app, &mut r.view, r.now);
    r.click("ingameloadgame", "cancel");
    r.idle();
    assert_eq!(r.state(), 0);
    for _ in 0..30 {
        r.frame();
    }
    assert_eq!(r.state(), 0, "no old ESC flag");
}

#[test]
fn flow_tells_the_session_game_type_and_menu_state() {
    // `GameFlow` tells the match at the start and before every frame the game type (`GE+0x108`, only -1 shows
    // cinemascope bars) and whether the game menu is open.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    assert_eq!(r.view.match_info, None);
    r.start();
    let game_type = r.host.app.game().unwrap().game_type;
    assert_eq!(r.view.match_info, Some((game_type, false)), "at the start");
    r.until_step();
    r.key("ESCAPE");
    while r.state() == 0 {
        r.frame();
    }
    r.frame();
    assert_eq!(r.view.match_info, Some((game_type, true)), "game menu open");
    // Campaign: -1.
    let Some(mut c) = campaign_run() else {
        return;
    };
    start_campaign(&mut c);
    assert_eq!(c.view.match_info.map(|(t, _)| t), Some(-1));
}

// QUIRK O-143: kept
#[test]
fn paused_game_takes_no_commands() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    let log = r.view.sim().unwrap().log.len();
    let steps = r.view.steps();
    r.view.inputs.clear();
    let press = |button, pressed| InputEvent::Button { button, pressed };
    let events = [
        InputEvent::CursorMoved { x: 500.0, y: 400.0 },
        press(MouseButton::Right, true),
        press(MouseButton::Right, false),
        press(MouseButton::Left, true),
        press(MouseButton::Left, false),
        InputEvent::Wheel { up: true },
        InputEvent::Key {
            code: winit::keyboard::KeyCode::ArrowUp,
            pressed: true,
            repeat: false,
        },
        InputEvent::Key {
            code: winit::keyboard::KeyCode::ArrowUp,
            pressed: false,
            repeat: false,
        },
    ];
    for ev in &events {
        r.flow.input(ev, S1024, r.now, &mut r.host.app, &mut r.view);
    }
    // Only pointer and released keys reach the map, no click.
    assert_eq!(r.view.inputs, [events[0], events[7]]);
    // F1 with the menu open does not save (`FUN_0042F490` only in state 0).
    r.key("F1");
    r.idle();
    assert!(r.files.list(SaveDir::Main, 0).is_empty());
    assert_eq!(r.view.sim().unwrap().log.len(), log);
    assert_eq!(r.view.steps(), steps);
    // On: input goes through again.
    r.click("ingameoptions3", "return");
    r.idle();
    r.view.inputs.clear();
    r.flow
        .input(&events[1], S1024, r.now, &mut r.host.app, &mut r.view);
    assert_eq!(r.view.inputs, [events[1]]);
}

// QUIRK O-152: kept
#[test]
fn keys_during_loading_are_ignored() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.apply(UiEffect::StartMatch(request(0)));
    assert!(r.flow.just_loaded());
    // The next round of the event loop brings the events piled up during loading: discarded.
    r.flow.begin_round();
    assert!(!r.flow.just_loaded());
    assert!(!r.flow.accepts_input());
    assert!(!r.flow.key("ESCAPE", &mut r.host.app, &mut r.view, r.now));
    assert!(!r.flow.key("F4", &mut r.host.app, &mut r.view, r.now));
    let click = InputEvent::Button {
        button: MouseButton::Right,
        pressed: true,
    };
    r.flow
        .input(&click, S1024, r.now, &mut r.host.app, &mut r.view);
    assert!(r.view.inputs.is_empty());
    for _ in 0..30 {
        r.frame();
    }
    assert_eq!(r.state(), 0, "piled-up ESC opens no menu");
    // After that input applies again.
    r.flow.begin_round();
    assert!(r.flow.accepts_input());
    r.flow
        .input(&click, S1024, r.now, &mut r.host.app, &mut r.view);
    assert_eq!(r.view.inputs, [click]);
    assert!(r.flow.key("ESCAPE", &mut r.host.app, &mut r.view, r.now));
}

// QUIRK O-31, O-151: kept
#[test]
fn quicksave_alternates_names_and_overwrites() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    let mut saved = Vec::new();
    for _ in 0..3 {
        r.until_step();
        r.key("F1");
        r.idle();
        assert_eq!(r.state(), 0);
        assert!(
            r.dialogs().is_empty(),
            "no confirmation prompt, SAVINGMESSAGE closed"
        );
        saved.push(r.view.steps());
    }
    // `quicksave1`, `quicksave2`, `quicksave1` (overwritten, O-31).
    let mut names: Vec<String> = r
        .files
        .list(SaveDir::Main, 0)
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, ["quicksave1", "quicksave2"]);
    let files = r.files.clone();
    let steps_of = |name: &str| {
        let bytes = std::fs::read(files.path(name, 0)).unwrap();
        GameSaveFile::from_bytes(&bytes).unwrap().sim.state.steps
    };
    assert!(steps_of("quicksave1") > steps_of("quicksave2"));
    assert!(steps_of("quicksave2") >= saved[0]);
    assert!(steps_of("quicksave1") <= saved[2]);
    // A new match begins the counter anew.
    r.start();
    r.key("F1");
    r.idle();
    assert!(steps_of("quicksave1") < saved[0], "quicksave1 again");
}

#[test]
fn no_steps_between_submit_and_write() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.key("F1");
    // The frame takes F1 (`{0x26}` queued); after that the game stands until writing (`0x0B`).
    r.frame();
    let at_submit = r.view.steps();
    let until = r.now + 1000;
    while !r.files.exists(SaveDir::Main, 0, "quicksave1") {
        assert!(r.now < until, "not written");
        let saving_before = r.host.app.saving();
        let steps = r.frame();
        if saving_before || r.host.app.saving() {
            assert_eq!(steps, 0, "no tick between submitting and writing");
        }
    }
    let bytes = std::fs::read(r.files.path("quicksave1", 0)).unwrap();
    let file = GameSaveFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.sim.state.steps, at_submit);
    assert!(!r.host.app.saving());
    // Host part: camera, map, game type, speed.
    assert_eq!(file.host.camera, crate::testutil::FAKE_CAMERA);
    assert_eq!(file.host.level_info.name, "Ink");
    assert_eq!((file.host.game_type, file.host.game_speed), (0, 14));
}

#[test]
fn quickload_opens_ingameloadgame_and_cancel_resumes() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.key("F4");
    r.frame();
    assert_eq!(r.state(), 1, "quickload halts the game");
    let steps = r.view.steps();
    assert_eq!(r.idle(), 0);
    assert_eq!(r.dialogs(), ["ingameloadgame"]);
    assert_eq!(r.view.steps(), steps);
    // `CANCEL` without a return command -> `{0x37}`: on in the game, no game menu.
    r.click("ingameloadgame", "cancel");
    r.idle();
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
    let mut later = 0;
    for _ in 0..30 {
        later += r.frame();
    }
    assert!(later > 0);
}

#[test]
fn broken_save_shows_message_and_stays_in_menu() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    capture_warnings();
    std::fs::create_dir_all(&r.files.dir).unwrap();
    std::fs::write(r.files.path("broken", 0), b"NMLGAME\0broken").unwrap();
    // Without a match (main menu `LOADSAVEDGAME2`): nothing started, `singleplayermenu` open again.
    r.apply(UiEffect::LoadGame {
        name: "broken".into(),
    });
    assert!(r.shown.is_empty(), "no loading screen for a broken file");
    assert!(!r.flow.running());
    assert!(
        r.view.music.is_empty(),
        "nothing loaded, the menu music continues"
    );
    r.idle();
    // The identifier is right, the number is not (`brok`): from another version or format.
    assert_eq!(msgbox_id(&r), Some(SAVE_OTHER_FORMAT));
    assert_eq!(r.dialogs(), ["singleplayermenu", "msgbox"]);
    r.click("msgbox", "ok");
    r.idle();
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    // With a match in the game menu: missing file, broken file, empty name, savegame with a foreign rules checksum -
    // the match stays as it was, the game menu opens again.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    std::fs::create_dir_all(&r.files.dir).unwrap();
    std::fs::write(r.files.path("broken", 0), b"NMLGAME\0broken").unwrap();
    r.start();
    r.until_step();
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    let before = (r.view.steps(), r.view.checksum());
    let mut bad = GameSaveFile::new(
        r.view.sim().unwrap().save(),
        r.flow.host_part(&r.host.app, &r.view).unwrap(),
        None,
    );
    bad.sim.start.rules.logic ^= 1;
    std::fs::write(r.files.path("rules", 0), bad.to_bytes()).unwrap();
    take_warnings();
    for name in ["missing", "broken", "", "rules"] {
        r.apply(UiEffect::LoadGame { name: name.into() });
    }
    let warnings = take_warnings();
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    assert!(
        warnings.iter().all(|w| w.contains("not loaded")),
        "{warnings:?}"
    );
    assert!(warnings[3].contains("rules differ"), "{warnings:?}");
    // Only the savegame with a foreign rules checksum got as far as loading; the match continues, no menu music.
    assert_eq!(r.view.music, ["stop", "stop"]);
    assert_eq!((r.view.steps(), r.view.checksum()), before);
    assert!(r.flow.running());
    assert_eq!(r.state(), 1, "the game still stands");
    r.idle();
    // The last load ("rules") passed `check`, which rejected the foreign rules checksum (`Mismatch`, a savegame of the
    // wrong game data): end to end the message "does not match the game data" stands over the game menu.
    assert_eq!(msgbox_id(&r), Some(SAVE_MISMATCH));
    r.click("msgbox", "ok");
    r.idle();
    assert!(r.dialogs().contains(&"ingameoptions3".to_string()));
    // Deleting without a name changes nothing.
    r.apply(UiEffect::DeleteSave {
        name: String::new(),
        dir: SaveDir::Main,
    });
    assert!(r.files.exists(SaveDir::Main, 0, "broken"));
    r.apply(UiEffect::DeleteSave {
        name: "broken".into(),
        dir: SaveDir::Main,
    });
    assert!(!r.files.exists(SaveDir::Main, 0, "broken"));
}

// QUIRK O-149: deviation
#[test]
fn save_failure_resumes_the_game() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    capture_warnings();
    // The folder `savegame` is a file: writing fails.
    std::fs::write(&r.files.dir, b"no storage").unwrap();
    r.start();
    r.until_step();
    r.key("F1");
    r.idle();
    let warnings = take_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("not saved")),
        "{warnings:?}"
    );
    // O-149: `SAVINGMESSAGE` closed, no saving open any more, the game continues.
    assert!(!r.host.app.saving());
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
    let mut later = 0;
    for _ in 0..30 {
        later += r.frame();
    }
    assert!(later > 0);
}

/// O-153: `GameSpeed` of the settings applies from 0 to 255; negative gives 0 (K-325).
// QUIRK K-325, O-153: deviation
#[test]
fn config_speed_keeps_0_to_255() {
    let speeds = [-10, -1, 0, 20, 30, 255, 256, i32::MAX, i32::MIN];
    let got: Vec<u8> = speeds
        .iter()
        .map(|&v| {
            let cfg = EngineConfig {
                game_speed: v,
                ..EngineConfig::default()
            };
            config_speed(&cfg)
        })
        .collect();
    assert_eq!(got, [0, 0, 0, 20, 30, 255, 255, 255, 0]);
}

// QUIRK O-153: deviation
#[test]
fn resume_and_reload_keep_the_match() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    // Without a match: restart and speed do nothing.
    r.apply(UiEffect::ReloadLevel);
    r.apply(UiEffect::ResumeGame {
        game_speed: Some(5),
    });
    assert!(!r.flow.running() && r.shown.is_empty());
    r.start();
    // `{0x0F}`: speed from the settings (without the limit 20, O-153).
    r.apply(UiEffect::ResumeGame {
        game_speed: Some(30),
    });
    assert_eq!(r.view.sim().unwrap().game_speed, 30);
    r.apply(UiEffect::ResumeGame { game_speed: None });
    assert_eq!(r.view.sim().unwrap().game_speed, 30);
    for _ in 0..30 {
        r.frame();
    }
    assert!(r.view.steps() > 0);
    // `0x1F`: the same map with the same start package from the beginning (loading screen of the restart).
    let setup = r.view.sim().unwrap().setup.clone();
    r.shown.clear();
    r.apply(UiEffect::ReloadLevel);
    assert_eq!(r.shown.len(), 4);
    assert_eq!(r.shown[0].0, MULTIPLAYER_LDG);
    assert_eq!(r.view.steps(), 0);
    assert_eq!(r.view.last_setup.as_ref(), Some(&setup));
    assert_eq!(r.view.level(), TWO_PLAYERS);
}

/// The settings of the option menus reach the match as in the original, which reads them from `g_pEngineConfig` when
/// needed - camera switches (`GameEngine_UpdateCamera` `0x0043C7C0`) and `UnitResponse` (`Unit_PlayResponseSound`
/// `0x0047BBC0`) in the next frame, also with the menu open.
#[test]
fn option_changes_reach_the_running_match_in_the_next_frame() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    let cfg = |r: &Run| r.host.app.config().clone();
    let game = |r: &Run| {
        let g = r.view.game.as_ref().unwrap();
        (g.controls.clone(), g.view.unit_response)
    };
    assert_eq!(game(&r), (controls(&cfg(&r)), cfg(&r).unit_response != 0));
    // Switches in `GAMEPLAYOPTIONS2` and `SOUNDOPTIONS` (options in the game), one click each.
    let before = cfg(&r);
    r.ui(Command::UiOpen {
        dialog: "gameplayoptions2".into(),
    });
    r.idle();
    for element in ["lockcamera", "zoom", "enablemousescroll"] {
        r.click("gameplayoptions2", element);
    }
    r.ui(Command::UiOpen {
        dialog: "soundoptions".into(),
    });
    r.idle();
    r.click("soundoptions", "unitresponse");
    r.frame();
    let after = cfg(&r);
    assert_ne!(after.lock_camera, before.lock_camera, "click takes effect");
    assert_ne!(after.enable_zoom, before.enable_zoom);
    assert_ne!(after.enable_mouse_scroll, before.enable_mouse_scroll);
    assert_ne!(after.unit_response, before.unit_response);
    let (controls_now, unit_response) = game(&r);
    assert_eq!(controls_now, controls(&after));
    assert_eq!(unit_response, after.unit_response != 0);
}

/// A start, a load and `0x1F` take over the settings of just now - also a change in the same frame (`VIDEOOPTIONS
/// ACCEPT` queues `{0x1F}` behind the qualities), the qualities only this way (`Game_StartLevel`).
// QUIRK O-98: kept
#[test]
fn start_and_reload_take_the_current_settings() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    let game = r.view.game.as_ref().unwrap();
    let (view, width) = (game.view.clone(), game.view.text_width);
    let cfg = r.host.app.config().clone();
    assert_eq!(view, view_options(&cfg, width));
    assert_eq!(game.controls, controls(&cfg));
    // Flip a switch and restart without a `GameFlow::frame` in between.
    r.ui(Command::UiOpen {
        dialog: "gameplayoptions2".into(),
    });
    r.idle();
    r.click("gameplayoptions2", "lockcamera");
    r.host.frame_cmds(&r.vfs, S1024, r.now);
    let changed = r.host.app.config().clone();
    assert_ne!(changed.lock_camera, cfg.lock_camera, "click takes effect");
    r.apply(UiEffect::ReloadLevel);
    let restarted = r.view.game.as_ref().unwrap();
    assert_eq!(restarted.controls, controls(&changed));
    assert_eq!(restarted.view, view_options(&changed, width));
}

/// Real session without a window (corpus and graphics card, otherwise silently skipped): start match, 40 steps, save,
/// 40 steps, load -> steps and checksum as at saving, loading screen `default.ldg` (O-139).
// QUIRK O-139: kept
#[test]
fn save_then_load_restores_checksum() {
    let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
        return;
    };
    let Ok((device, queue, _)) =
        opennml_render::headless_device(opennml_render::BackendChoice::Auto)
    else {
        eprintln!("no GPU, test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let files = FsSaveFiles {
        dir: tmp.path().join("savegame"),
    };
    let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
    let config = EngineConfig {
        is_intro: false,
        ..test_config(&vfs)
    };
    let audio = FakeAudio::default();
    let mut host =
        MenuHost::new(&vfs, layer, config, MenuOptions::default(), &audio, None).unwrap();
    let mut flow = GameFlow::new(&host.layer.texts);
    let mut view_options = ViewOptions::default();
    let mut controls = Controls::default();
    let mut session: Option<GameSession> = None;
    fn live<'a>(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &'a Vfs,
        session: &'a mut Option<GameSession>,
        view: &'a mut ViewOptions,
        controls: &'a mut Controls,
    ) -> LiveSession<'a> {
        LiveSession {
            device: device.clone(),
            queue: queue.clone(),
            format: opennml_render::CAPTURE_FORMAT,
            vfs,
            session,
            view,
            controls,
            size: (320, 200),
            audio: None,
        }
    }
    let mut shown = Vec::new();
    let apply = |effect: UiEffect,
                 flow: &mut GameFlow,
                 host: &mut MenuHost,
                 session: &mut Option<GameSession>,
                 shown: &mut Vec<(String, u8)>| {
        let env = FlowEnv {
            vfs: &vfs,
            texts: &host.layer.texts,
            files: &files,
        };
        let (mut v, mut c) = (view_options.clone(), controls.clone());
        let mut view = live(&device, &queue, &vfs, session, &mut v, &mut c);
        assert!(flow.apply(
            &effect,
            &mut host.app,
            &mut view,
            &env,
            &mut |ldg, phase| shown.push((ldg.to_string(), phase.percent())),
        ));
    };
    apply(
        UiEffect::StartMatch(request(0)),
        &mut flow,
        &mut host,
        &mut session,
        &mut shown,
    );
    assert_eq!(shown.len(), 4);
    let mut now = 0;
    let run = |n: u64,
               now: &mut u64,
               flow: &mut GameFlow,
               host: &mut MenuHost,
               session: &mut Option<GameSession>| {
        let target = session.as_ref().unwrap().steps() + n;
        while session.as_ref().unwrap().steps() < target {
            *now += 50;
            let (mut v, mut c) = (view_options.clone(), controls.clone());
            let mut view = live(&device, &queue, &vfs, session, &mut v, &mut c);
            flow.frame(*now, &mut host.app, &mut view, &host.layer.texts);
        }
    };
    run(40, &mut now, &mut flow, &mut host, &mut session);
    let at_save = {
        let s = session.as_ref().unwrap();
        (s.steps(), s.checksum())
    };
    assert!(at_save.0 >= 40);
    apply(
        UiEffect::SaveGame {
            name: "probe".into(),
        },
        &mut flow,
        &mut host,
        &mut session,
        &mut shown,
    );
    assert!(files.exists(SaveDir::Main, 0, "probe"));
    run(40, &mut now, &mut flow, &mut host, &mut session);
    assert_ne!(session.as_ref().unwrap().steps(), at_save.0);
    shown.clear();
    apply(
        UiEffect::LoadGame {
            name: "probe".into(),
        },
        &mut flow,
        &mut host,
        &mut session,
        &mut shown,
    );
    let s = session.as_ref().unwrap();
    assert_eq!((s.steps(), s.checksum()), at_save);
    assert_eq!(
        shown
            .iter()
            .map(|(l, p)| (l.as_str(), *p))
            .collect::<Vec<_>>(),
        [
            (DEFAULT_LDG, 0),
            (DEFAULT_LDG, 40),
            (DEFAULT_LDG, 70),
            (DEFAULT_LDG, 100)
        ]
    );
    assert_eq!(host.app.game().unwrap().game_type, 0);
    // `apply_settings` rewrites view and controls of the host; camera switches and `UnitResponse` apply at once to the
    // running session, the qualities not until the next start (the session keeps its own).
    let cfg = EngineConfig {
        lock_camera: 0,
        enable_zoom: 0,
        mouse_scroll: 3,
        unit_response: 0,
        particles_quality: 0,
        shadow_quality: 1,
        ..host.app.config().clone()
    };
    let mut view = live(
        &device,
        &queue,
        &vfs,
        &mut session,
        &mut view_options,
        &mut controls,
    );
    view.apply_settings(&cfg);
    let s = session.as_ref().unwrap();
    assert_eq!(controls, super::controls(&cfg));
    assert_eq!(s.controls, controls);
    assert!(!s.voices.unit_response && !s.view.unit_response);
    assert_eq!(view_options, super::view_options(&cfg, 1280));
    assert_eq!(
        (s.view.particles_quality, s.view.shadow_quality),
        (2, 2),
        "qualities of the running session stay"
    );
}

/// Report of the match for the control channel: `ui_state` in `opennml-game` sets `game` from [`GameFlow::game_data`];
/// without a match it is missing, with a match it shows map, game type, menu state, steps, checksum and the course of
/// loading.
#[test]
fn ui_state_reports_the_running_game() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let report = |r: &Run| {
        r.flow.game_data(
            &r.host.app,
            r.view.sim(),
            r.view.level(),
            None,
            None,
            None,
            None,
            Vec::new(),
        )
    };
    assert_eq!(report(&r), None, "no report without a match");
    r.apply(UiEffect::StartMatch(request(0)));
    let data = report(&r).unwrap();
    assert_eq!(data.level, TWO_PLAYERS);
    assert_eq!(data.game_type, 0);
    assert_eq!(data.menu_state, 0);
    assert_eq!(data.steps, 0);
    assert_eq!(data.checksum, hex_checksum(r.view.checksum()));
    // Loading blocks: afterwards it is over, the course stays.
    assert_eq!(data.loading, None);
    assert_eq!(data.load_phases, [0, 40, 70, 100]);

    // The match runs: steps and checksum follow the simulation.
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    for _ in 0..40 {
        r.frame();
    }
    let data = report(&r).unwrap();
    assert!(data.steps > 0);
    assert_eq!(data.steps, r.view.steps());
    assert_eq!(data.checksum, hex_checksum(r.view.checksum()));
    assert_eq!(data.hud, None, "without UI of the host");
    // The game UI of the host (`GameSession::hud_state`) stands unchanged in `hud`.
    let hud = opennml_control::UiHudData {
        dialog: opennml_control::GAME_UI_DIALOG.into(),
        windows: Vec::new(),
        messages: Vec::new(),
        action_stack: vec![[1, 2]],
        status: None,
        toolbar: None,
    };
    let with = r
        .flow
        .game_data(
            &r.host.app,
            r.view.sim(),
            r.view.level(),
            Some(hud.clone()),
            None,
            None,
            None,
            Vec::new(),
        )
        .unwrap();
    assert_eq!(with.hud.as_ref(), Some(&hud));
    assert_eq!(UiGameData { hud: None, ..with }, data);

    // Game menu open: state 1, the steps stand still.
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    let held = report(&r).unwrap();
    assert_eq!(held.menu_state, 1);
    assert_eq!(held.steps, r.view.steps());

    // In the response of `ui_state`: field `game` next to the UI state (protocol form).
    let mut state = state_data(&r.host.app, S1024, r.now);
    assert_eq!(state.game, None, "state_data does not know the match");
    state.game = report(&r);
    let value = serde_json::to_value(&state).unwrap();
    assert_eq!(value["game"]["level"], TWO_PLAYERS);
    assert_eq!(value["game"]["menu_state"], 1);
    assert_eq!(
        value["game"]["checksum"],
        serde_json::json!(hex_checksum(r.view.checksum())),
        "hex text like sim_hash"
    );
    assert_eq!(value["game"]["checksum"].as_str().unwrap().len(), 16);
    assert_eq!(
        value["game"]["load_phases"],
        serde_json::json!([0, 40, 70, 100])
    );
    assert!(value["game"].get("loading").is_none());

    // A new load (restart level) replaces the course; the steps begin anew.
    r.click("ingameoptions3", "return");
    r.idle();
    r.apply(UiEffect::ReloadLevel);
    let data = report(&r).unwrap();
    assert_eq!(data.load_phases, [0, 40, 70, 100]);
    assert_eq!(data.steps, 0, "restarted");

    // Failed start: the course ends after phase 0, the old match stays (deviation) and shows it.
    capture_warnings();
    r.view.fail_level = Some(TWO_PLAYERS.into());
    r.apply(UiEffect::ReloadLevel);
    take_warnings();
    assert_eq!(report(&r).unwrap().load_phases, [0]);
}

/// Like [`Run::start`] with a real map and simulation (`FakeSession::real_levels`): `req` on the map of the
/// installation with its rules.
fn start_real(r: &mut Run, req: MatchRequest) {
    r.view.real_levels = true;
    r.apply(UiEffect::StartMatch(req));
    assert!(r.flow.running(), "match running");
    r.flow.begin_round();
    r.flow.begin_round();
}

/// Slot of the local player (`GameInfo::local_slot`).
fn local(r: &Run) -> u8 {
    r.host.app.game().expect("match").local_slot
}

/// Command `0x56` of the local player for the next tick boundary.
fn submit_result(r: &mut Run, slot: u8, result: i32) {
    let player = local(r);
    r.view
        .sim_mut()
        .unwrap()
        .submit(opennml_sim::command::Command {
            player,
            kind: opennml_sim::command::CommandKind::SetPlayerResult { slot, result },
        });
}

/// The reported `game_end` effects of the UI.
fn game_end_reports(r: &Run) -> usize {
    state_data(&r.host.app, S1024, r.now)
        .effects
        .iter()
        .filter(|e| matches!(e.effect, opennml_control::UiEffectKind::GameEnd { .. }))
        .count()
}

#[test]
fn local_defeat_opens_gameend_defeat_and_pauses() {
    // re/ingame-menus 6.2 steps 3-4 (query of the result after every frame): in the frame of the step with the result
    // the game music stops, state 7, `GAMEEND_DEFEAT`; after that the game stands, ESC and F4 open nothing.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    assert_eq!(
        r.view.sim().unwrap().state.players[usize::from(slot)].result,
        1
    );
    assert_eq!(r.state(), 7, "in the frame of the step");
    assert_eq!(r.view.music.last(), Some(&"stop-game"));
    let steps = r.view.steps();
    assert_eq!(r.idle(), 0, "the game stands");
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    for _ in 0..120 {
        assert_eq!(r.frame(), 0);
    }
    r.key("ESCAPE");
    r.key("F4");
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    assert_eq!(r.state(), 7);
    assert_eq!(r.view.steps(), steps);
}

// QUIRK D-302: kept
#[test]
fn timer_stays_after_the_game_end() {
    // After the end window (state 6/7) the single-player match makes no steps any more (`running` demands state 0), so
    // the timer of the list is no longer ticked and stays with its remainder - the display keeps showing the same
    // number.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    let slot = local(&r);
    r.view
        .sim_mut()
        .unwrap()
        .state
        .countdowns
        .insert(opennml_sim::flags::LEADER_COUNTDOWN, 700);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    assert_eq!(r.state(), 7, "in the frame of the step");
    let left = r.view.sim().unwrap().state.countdowns[&opennml_sim::flags::LEADER_COUNTDOWN];
    assert!((0..700).contains(&left), "ticked up to the result: {left}");
    let steps = r.view.steps();
    for _ in 0..120 {
        assert_eq!(r.frame(), 0);
    }
    assert_eq!(r.view.steps(), steps);
    assert_eq!(
        r.view.sim().unwrap().state.countdowns[&opennml_sim::flags::LEADER_COUNTDOWN],
        left,
        "the timer stands"
    );
}

#[test]
fn only_one_game_end_screen() {
    // The victory of the computer lets the local player lose in the same step (O-181), its own victory in the same tick
    // is without effect afterwards (result != 0) - exactly one end screen. A result changed later does not open a
    // second one either (flag in `GameFlow`).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    let slot = local(&r);
    let other = r
        .view
        .sim()
        .unwrap()
        .state
        .players
        .iter()
        .position(|p| p.control == opennml_sim::state::Control::Ai)
        .expect("computer") as u8;
    submit_result(&mut r, other, opennml_sim::game_end::VICTORY);
    submit_result(&mut r, slot, opennml_sim::game_end::VICTORY);
    r.until_step();
    let players = &r.view.sim().unwrap().state.players;
    assert_eq!(
        (
            players[usize::from(other)].result,
            players[usize::from(slot)].result
        ),
        (2, 1)
    );
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    assert_eq!(r.state(), 7);
    r.view.sim_mut().unwrap().state.players[usize::from(slot)].result = 2;
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    assert_eq!(r.state(), 7);
    assert_eq!(game_end_reports(&r), 1);
    assert_eq!(
        r.view.music.iter().filter(|m| **m == "stop-game").count(),
        1
    );
}

#[test]
fn quitgame_surrenders_and_shows_defeat() {
    // `QUITGAME` -> prompt 15780 -> OK `{0x27}` -> defeat at once, although the game menu (state 1) halts every tick;
    // the command is in the log with the current step. After that `GAMEEND_DEFEAT` (state 7).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    let steps = r.view.steps();
    r.click("ingameoptions3", "quitgame");
    r.idle();
    assert_eq!(r.dialogs(), ["infoframe"]);
    r.key("RETURN");
    r.idle();
    let slot = local(&r);
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.state.players[usize::from(slot)].result, 1);
    assert_eq!(sim.state.steps, steps, "no tick");
    let last = sim.log.last().unwrap();
    assert_eq!(last.step, steps);
    assert_eq!(
        last.command.kind,
        opennml_sim::command::CommandKind::SetPlayerResult { slot, result: 1 }
    );
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    assert_eq!(r.state(), 7);
    assert_eq!(r.view.music.last(), Some(&"stop-game"));
}

// QUIRK O-165: deviation
#[test]
fn end_match_builds_accounting_and_ends_the_match() {
    // `END_DEFEAT` -> `{4}`, `{0x15}`, `{0x0E}` (re/accounting 6): table from the simulation, game time, match closed,
    // `accounting` with the tab `account1`.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    r.idle();
    let steps = r.view.steps();
    r.click("gameend_defeat", "end_defeat");
    r.idle();
    let mut dialogs = r.dialogs();
    dialogs.sort();
    assert_eq!(dialogs, ["account1", "accounting"]);
    assert!(!r.flow.running());
    assert!(r.view.game.is_none(), "match ended");
    assert!(r.host.app.game().is_none());
    assert_eq!(r.state(), 0);
    let view = r.host.app.data().accounting.clone().unwrap();
    let names: Vec<&str> = view.rows.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(names[0], "S-1", "lobby name of the human");
    let texts = &r.host.layer.texts;
    assert_eq!(view.time_text, play_time(&texts.display(15349), steps));
    // More frames without a match: no steps, no second game end.
    assert_eq!(r.idle(), 0);
}

#[test]
fn ui_state_reports_result_and_accounting() {
    // `sim_result` sets the result through a command of the simulation (next tick, in the command log);
    // `ui_state.game.result` shows the result of the local player, `accounting` and `play_time` the open accounting
    // (from `MenuData::accounting`, after `0x0E` without a match).
    use crate::game::sim_control::execute_sim;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    // Without a match: an invalid command is reported first, otherwise "no match".
    let result = |player, result| Command::SimResult { player, result };
    assert_eq!(
        execute_sim(None, &result(8, 2)).unwrap_err(),
        "player = 8 invalid: allowed 0 to 7"
    );
    assert_eq!(
        execute_sim(None, &result(0, 3)).unwrap_err(),
        "result = 3 invalid: allowed 1 (lost) or 2 (won)"
    );
    assert_eq!(execute_sim(None, &result(0, 2)).unwrap_err(), "no match");

    start_real(&mut r, request(0));
    r.until_step();
    let report = |r: &Run| {
        r.flow.game_data(
            &r.host.app,
            r.view.sim(),
            r.view.level(),
            None,
            None,
            None,
            None,
            Vec::new(),
        )
    };
    assert_eq!(report(&r).unwrap().result, 0, "playing");
    let state = state_data(&r.host.app, S1024, r.now);
    assert_eq!((state.accounting, state.play_time), (None, None));

    let slot = local(&r);
    let steps = r.view.steps();
    let answer = execute_sim(r.view.sim_mut(), &result(slot, 2)).unwrap();
    assert_eq!(answer, serde_json::json!({ "step": steps }));
    assert_eq!(report(&r).unwrap().result, 0, "not until the next tick");
    r.until_step();
    let sim = r.view.sim().unwrap();
    let logged = sim.log.last().unwrap();
    assert_eq!(logged.step, steps);
    assert_eq!(
        logged.command,
        opennml_sim::command::Command {
            player: sim.active_player,
            kind: opennml_sim::command::CommandKind::SetPlayerResult { slot, result: 2 },
        }
    );
    assert_eq!(report(&r).unwrap().result, 2);
    assert_eq!(r.state(), 6);
    let value = serde_json::to_value(r.flow.game_data(
        &r.host.app,
        r.view.sim(),
        "",
        None,
        None,
        None,
        None,
        Vec::new(),
    ))
    .unwrap();
    assert_eq!(value["result"], 2);

    // Accounting: not reported until the frame `accounting` is open, rows and game time like `MenuData::accounting`.
    r.idle();
    r.click("gameend_victory", "end_victory");
    r.idle();
    let state = state_data(&r.host.app, S1024, r.now);
    let view = r.host.app.data().accounting.clone().unwrap();
    let rows = state.accounting.expect("accounting open");
    assert_eq!(rows.len(), 2);
    for (row, (name, color, values)) in rows.iter().zip(&view.rows) {
        assert_eq!((&row.name, row.color, &row.values), (name, *color, values));
    }
    assert_eq!(state.play_time.as_deref(), Some(view.time_text.as_str()));
    let value = serde_json::to_value(state_data(&r.host.app, S1024, r.now)).unwrap();
    assert_eq!(value["accounting"][0]["name"], "S-1");
    assert_eq!(
        value["accounting"][0]["values"].as_array().unwrap().len(),
        12
    );
    assert_eq!(value["play_time"], serde_json::json!(view.time_text));
    assert!(!r.flow.running(), "match ended");
    // Main menu: frame closed, nothing reported any more (the table stays in `MenuData`).
    r.click("accounting", "mainmenu");
    r.idle();
    let state = state_data(&r.host.app, S1024, r.now);
    assert_eq!((state.accounting, state.play_time), (None, None));
    assert!(r.host.app.data().accounting.is_some());
}

// QUIRK O-164: kept
#[test]
fn play_time_shows_hours_and_minutes() {
    // O-164 (`0x004C9FAD`...`0x004CA001`): `t = steps * 50` ms, `%s %d:%02d` with hours and minutes (seconds cut off);
    // deviation: simulation steps instead of real time (O-165).
    assert_eq!(play_time("Game time:", 3_900_000 / 50), "Game time: 1:05");
    assert_eq!(play_time("Game time:", 0), "Game time: 0:00");
    assert_eq!(play_time("Game time:", 3_599_999 / 50), "Game time: 0:59");
    assert_eq!(play_time("T", 36_000_000 / 50), "T 10:00");
}

#[test]
fn table_wraps_on_overflow_and_fits_twelve_chars() {
    // `FUN_00403FA0`: sums wrap in 32 bit, `"%d"` fits with terminator into `wchar[12]`; empty name -> "Computer"
    // (15195). In the campaign the display resolves `.lvl` names with a text ID like `PlayerSetup_ParseLvl`
    // (`0x00486640`: `_atol`, text of the number >= 1 except 29320).
    let Some(r) = Run::corpus() else {
        return;
    };
    let texts = &r.host.layer.texts;
    let mut host = crate::game::sim_host::plain_testkit::host();
    let records = &mut host.state.accounting.records;
    records[0].active = true;
    records[0].name = "29081 RED".into();
    records[0].entries = [(8, i32::MAX), (8, i32::MAX), (9, i32::MIN), (10, -1)]
        .into_iter()
        .map(|(kind, parm)| opennml_sim::accounting::AccountEntry {
            kind,
            time: 0,
            parm,
        })
        .collect();
    records[2].active = true;
    records[2].name = String::new();
    records[2].color = 5;
    records[4].active = true;
    records[4].name = "29320 Placeholder".into();
    let view = accounting_view(&host, texts);
    assert_eq!(view.rows.len(), 3);
    let (name, color, values) = &view.rows[0];
    assert_eq!(*name, texts.display(29081));
    assert_eq!(*color, 0);
    assert_eq!(values[8], "-2");
    assert_eq!(values[9], "-2147483648");
    assert_eq!(values[10], "-1");
    assert_eq!(values[0], "0");
    assert!(values.iter().all(|v| v.chars().count() <= 11));
    assert_eq!(view.rows[1].0, texts.display(15195));
    assert_eq!(view.rows[1].1, 5);
    assert_eq!(view.rows[2].0, "29320 Placeholder", "29320 is not resolved");
    // Free play: names stay as the lobby sets them.
    host.state.mode = 0;
    assert_eq!(accounting_view(&host, texts).rows[0].0, "29081 RED");
}

// QUIRK K-5: kept
#[test]
fn no_rule_check_while_paused() {
    // re/ingame-menus 0 point 1: in single player every open game menu halts the whole engine, also
    // `GameEngine_CheckVictoryDefeat`. Match of game type 0 without an opponent: the victory rule (`CHECKENEMY`, no
    // enemy) is fulfilled from the first step, checking starts from the 20th (K-5). With the game menu open for 5 s: no
    // step, no result; after `RETURN` the player wins in the 20th step.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let alone = MatchRequest {
        slots: vec![slot(-1, 999, 0, 0)],
        ..request(0)
    };
    start_real(&mut r, alone);
    assert!(r.host.app.open_game_menu());
    let until = r.now + 5000;
    while r.now < until {
        assert_eq!(r.frame(), 0);
    }
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.state.mode, 0);
    assert_eq!(sim.state.steps, 0);
    assert!(
        sim.state
            .players
            .iter()
            .all(|p| p.result != 1 && p.result != 2)
    );
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    r.click("ingameoptions3", "return");
    r.idle();
    let until = r.now + 3000;
    while r.state() == 0 {
        assert!(r.now < until, "no game end");
        r.frame();
    }
    let sim = r.view.sim().unwrap();
    // First check in the 20th step; a frame has at most two ticks (the second changes nothing any more).
    assert!((20..=21).contains(&sim.state.steps), "{}", sim.state.steps);
    assert_eq!(sim.state.players[usize::from(local(&r))].result, 2);
    assert_eq!(r.state(), 6);
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_victory"]);
}

/// Campaign level of the installation with its own `.ldg`.
pub(super) const KAMP1_1: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";

#[test]
fn campaign_level_starts_in_campaign_mode() {
    // `UiEffect::StartCampaignLevel` (command `0x0C`, re/singleplayer-menus part 1, 6.2) loads the level with its own
    // `.ldg`, setup `campaign_setup` (game type -1, difficulty, script and AI on with `TimePerChar`, profile name
    // `NetName`); the UI runs in campaign mode (game menu `INGAMEOPTIONS`).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.view.real_levels = true;
    r.apply(UiEffect::StartCampaignLevel {
        level: KAMP1_1.into(),
        difficulty: 0,
    });
    assert!(r.flow.running(), "match running");
    let ldg = "data/levels/kampagne1/kamp1-1/kamp1-1.ldg";
    let phases: Vec<(String, u8)> = [0, 40, 70, 100]
        .into_iter()
        .map(|p| (ldg.to_string(), p))
        .collect();
    assert_eq!(r.shown, phases);
    assert_eq!(r.view.level(), KAMP1_1);
    let setup = r.view.last_setup.clone().unwrap();
    let config = r.host.app.config().clone();
    assert_eq!(setup.mode, -1);
    assert!(setup.lobby.is_empty());
    assert_eq!(setup.ki.campaign_difficulty, 0);
    assert!(setup.ki.enabled && setup.script.enabled);
    assert_eq!(setup.script.time_per_char, config.time_per_char);
    assert_eq!(
        setup.local_name.as_deref(),
        Some(name_text(&config.net_name).as_str())
    );
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.state.mode, -1);
    assert!(sim.state.script.level.is_some(), "level script running");
    assert_eq!(r.host.app.game().map(|g| g.game_type), Some(-1));
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.dialogs(), ["ingameoptions"]);
}

#[test]
fn unreadable_campaign_level_reopens_the_menu() {
    // The deviation (menu reopens after a failed start) applies to `0x0C` as well: if loading fails, no game and the
    // menu opens again - the campaign selection (the original does not evaluate the result of `FUN_004C8EC0` and would
    // stay without a menu), with menu music.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.view.real_levels = true;
    r.view.fail_level = Some(KAMP1_1.into());
    capture_warnings();
    r.apply(UiEffect::StartCampaignLevel {
        level: KAMP1_1.into(),
        difficulty: 1,
    });
    assert!(!r.flow.running());
    let warnings = take_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("kamp1-1")),
        "{warnings:?}"
    );
    r.idle();
    let dialogs = r.dialogs();
    assert!(dialogs.iter().any(|d| d == "selectcampaign"), "{dialogs:?}");
    assert!(
        !dialogs.iter().any(|d| d == "singleplayermenu"),
        "{dialogs:?}"
    );
    assert_eq!(r.view.music.last(), Some(&"restart"));
}

/// Profile "Anna" with the records 0 ... 2 of campaign 1, current entry `kamp1-1` (campaign 1, entry 2).
fn campaign_run() -> Option<Run> {
    use opennml_formats::profiles::{LevelRecord, ProfileRecord, ProfilesFile};
    let levels = (0..=2)
        .map(|level| LevelRecord {
            campaign: 1,
            level,
            trials: 0,
            complete: false,
        })
        .collect();
    let profiles = ProfilesFile {
        current: 0,
        next_guid: 1,
        profiles: vec![ProfileRecord {
            name: opennml_ui::profiles::name_field("Anna"),
            guid: 0,
            levels,
            vars: Vec::new(),
        }],
        current_level: 2,
        current_campaign: 1,
    };
    Run::corpus_with(MenuOptions {
        profiles,
        ..MenuOptions::default()
    })
}

/// Start `kamp1-1` as a campaign level (`0x0C`) and let it run.
fn start_campaign(r: &mut Run) {
    r.view.real_levels = true;
    r.apply(UiEffect::StartCampaignLevel {
        level: KAMP1_1.into(),
        difficulty: 1,
    });
    assert!(r.flow.running(), "match running");
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
}

/// Campaign/entry, flag "newly free" and the records (trials, completed) of (1, 2) and (1, 3).
type CampaignState = ((i32, i32), bool, Vec<(i32, bool)>, Vec<(i32, bool)>);

fn campaign_state(r: &Run) -> CampaignState {
    let p = r.host.app.profiles();
    let records = |e: i32| -> Vec<(i32, bool)> {
        p.file.profiles[0]
            .levels
            .iter()
            .filter(|l| l.campaign == 1 && l.level == e)
            .map(|l| (l.trials, l.complete))
            .collect()
    };
    (
        (p.file.current_campaign, p.file.current_level),
        p.new_flag,
        records(2),
        records(3),
    )
}

/// The reported effects from `game_end` on (end screen, booking, writing the profiles); what arose outside a frame
/// appears in the history after the next frame.
fn end_reports(r: &Run) -> Vec<opennml_control::UiEffectKind> {
    use opennml_control::UiEffectKind as K;
    state_data(&r.host.app, S1024, r.now)
        .effects
        .into_iter()
        .map(|e| e.effect)
        .filter(|e| {
            matches!(
                e,
                K::GameEnd { .. } | K::BookCampaign { .. } | K::WriteProfiles
            )
        })
        .collect()
}

#[test]
fn campaign_victory_books_and_unlocks() {
    // `GameEngine_SetPlayerResult` (`0x00430CE0`, re/ingame-menus 6.2 step 4b) for the local player at
    // `Engine+0x108 == -1` (`0x00430EE1`): after the end screen `FUN_004C6B10(1)` - the current entry `kamp1-1` is
    // completed, `kamp1-2` newly free and current (flag 1), `profiles.ini` is written.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    assert_eq!(
        campaign_state(&r),
        ((1, 2), false, vec![(0, false)], vec![])
    );
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::VICTORY);
    r.until_step();
    assert_eq!(r.state(), 6);
    assert_eq!(
        campaign_state(&r),
        ((1, 3), true, vec![(0, true)], vec![(0, false)])
    );
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_victory"]);
    use opennml_control::UiEffectKind as K;
    assert_eq!(
        end_reports(&r),
        [
            K::GameEnd { victory: true },
            K::BookCampaign { won: true },
            K::WriteProfiles
        ]
    );
    // Exactly one booking per match.
    for _ in 0..30 {
        r.frame();
    }
    assert_eq!(
        campaign_state(&r),
        ((1, 3), true, vec![(0, true)], vec![(0, false)])
    );
}

#[test]
fn campaign_defeat_counts_trials() {
    // Step 4a: defeat in the campaign -> `FUN_004C6B10(0)`: trials of the current entry + 1, flag 0, no advancing;
    // likewise after giving up (`QUITGAME` -> `0x27`, re/ingame-menus 6.2).
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    assert_eq!(r.state(), 7);
    assert_eq!(
        campaign_state(&r),
        ((1, 2), false, vec![(1, false)], vec![])
    );
    r.idle();
    use opennml_control::UiEffectKind as K;
    assert_eq!(
        end_reports(&r),
        [
            K::GameEnd { victory: false },
            K::BookCampaign { won: false },
            K::WriteProfiles
        ]
    );
    // Giving up in a new match counts the next trial.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let slot = local(&r);
    r.apply(UiEffect::Surrender { slot });
    assert_eq!(r.state(), 7);
    assert_eq!(
        campaign_state(&r),
        ((1, 2), false, vec![(1, false)], vec![])
    );
}

#[test]
fn free_game_end_books_nothing() {
    // Outside the campaign (`Engine+0x108 != -1`) the game end books nothing, also with a profile.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::VICTORY);
    r.until_step();
    assert_eq!(r.state(), 6);
    assert_eq!(
        campaign_state(&r),
        ((1, 2), false, vec![(0, false)], vec![])
    );
    r.idle();
    use opennml_control::UiEffectKind as K;
    assert_eq!(end_reports(&r), [K::GameEnd { victory: true }]);
}

// QUIRK O-30, O-147: kept
#[test]
fn defeat_restart_reloads_without_booking() {
    // `GAMEEND_DEFEAT RESTART` in the campaign (`{4}`, `{0x15}`, `{0x1F}`, `FUN_004D9030`) starts the level from the
    // beginning (state 0, new loading phases, result 0) and books nothing: the game end has booked (`FUN_004C6B10(0)`,
    // exactly once per match); the new match does not book again until its own end.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 7);
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    let one_trial = ((1, 2), false, vec![(1, false)], vec![]);
    assert_eq!(campaign_state(&r), one_trial);
    let booked = end_reports(&r);
    assert_eq!(booked.len(), 3);
    let loads = r.shown.len();

    r.click("gameend_defeat", "restart");
    r.idle();
    assert_eq!(r.shown.len(), loads + 4, "loading screen of the restart");
    assert_eq!(
        r.shown[loads].0,
        "data/levels/kampagne1/kamp1-1/kamp1-1.ldg"
    );
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
    assert!(r.flow.running());
    assert_eq!(
        r.host.app.game().map(|g| (g.game_type, g.local_slot)),
        Some((-1, slot))
    );
    assert_eq!(
        r.view.sim().unwrap().state.players[usize::from(slot)].result,
        0
    );
    assert_eq!(campaign_state(&r), one_trial, "no second booking");
    assert_eq!(end_reports(&r), booked);
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    for _ in 0..30 {
        r.frame();
    }
    assert_eq!(r.state(), 0, "the game runs again");
    assert_eq!(campaign_state(&r), one_trial);
    assert_eq!(end_reports(&r), booked);

    // The new match ends on its own: next defeat = second trial.
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    assert_eq!(r.state(), 7);
    assert_eq!(
        campaign_state(&r),
        ((1, 2), false, vec![(2, false)], vec![])
    );
}

// QUIRK O-208: deviation
#[test]
fn failed_load_from_the_defeat_screen_reopens_it() {
    // From `GAMEEND_DEFEAT`, `LOAD` leads to `INGAMELOADGAME`; if loading fails (file missing), the match stays stopped
    // and the defeat screen comes back (state 7), not the game menu. `CANCEL` leads back as well.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 7);

    r.click("gameend_defeat", "load");
    r.idle();
    assert_eq!(r.dialogs(), ["ingameloadgame"]);
    assert_eq!(r.state(), 7);
    r.click("ingameloadgame", "cancel");
    r.idle();
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    assert_eq!(r.state(), 7);

    r.click("gameend_defeat", "load");
    r.idle();
    capture_warnings();
    r.apply(UiEffect::LoadGame {
        name: "missing".into(),
    });
    let warnings = take_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("not loaded")),
        "{warnings:?}"
    );
    r.idle();
    let dialogs = r.dialogs();
    assert!(dialogs.iter().any(|d| d == "gameend_defeat"), "{dialogs:?}");
    assert!(!dialogs.iter().any(|d| d.starts_with("ingameoptions")));
    assert_eq!(r.state(), 7);
    assert!(r.flow.running());
    assert_eq!(game_end_reports(&r), 1, "no second end screen");
}

/// Host with four active slots for the name tests: slot 0 computer, 1 and 2 human, 3 computer; all with the map name
/// `29081 RED` (text ID) or the placeholder `29320 ...` at slot 3.
fn named_host(mode: i32) -> crate::game::sim_host::SimHost {
    use opennml_sim::state::Control;
    let mut host = crate::game::sim_host::plain_testkit::host();
    host.state.mode = mode;
    for (slot, control) in [
        (0, Control::Ai),
        (1, Control::Human),
        (2, Control::Human),
        (3, Control::Ai),
    ] {
        let player = &mut host.state.players[slot];
        player.slot_active = true;
        player.control = control;
        player.name = "29081 RED".into();
    }
    host.state.players[3].name = "29320 Placeholder".into();
    host
}

// QUIRK O-250: kept
#[test]
fn display_name_prefers_the_profile_name_for_the_local_slot() {
    // O-250 (`Game_SelectLocalPlayer` `0x004BB446`): the first active human slot is named like the profile, even if the
    // map gives it a text ID; the second human keeps the resolved map name.
    let Some(r) = Run::corpus() else {
        return;
    };
    let texts = &r.host.layer.texts;
    let mut host = named_host(-1);
    host.setup.local_name = Some("Anna".into());
    assert_eq!(player_display_name(&host, texts, 1), "Anna");
    assert_eq!(player_display_name(&host, texts, 2), texts.display(29081));
    // Without a profile name (free play, never set) the map rule applies to the first human as well.
    host.setup.local_name = None;
    assert_eq!(player_display_name(&host, texts, 1), texts.display(29081));
}

// QUIRK O-250: kept
#[test]
fn campaign_names_resolve_their_text_id() {
    // `PlayerSetup_ParseLvl`: number >= 1 except 29320 -> text of the number, truncated to 39 characters; outside 0...7
    // empty (`GameEngine_GetPlayer`).
    let Some(r) = Run::corpus() else {
        return;
    };
    let texts = &r.host.layer.texts;
    let mut host = named_host(-1);
    assert_eq!(player_display_name(&host, texts, 0), texts.display(29081));
    assert_eq!(
        player_display_name(&host, texts, 3),
        "29320 Placeholder",
        "29320 is not resolved"
    );
    for slot in [-1, 8, 100] {
        assert_eq!(player_display_name(&host, texts, slot), "", "slot {slot}");
    }
    // A text over 39 characters is truncated (number without text: the last formatted number, `Texts::display`).
    host.state.players[0].name = "12345678".into();
    assert!(player_display_name(&host, texts, 0).chars().count() <= 39);
}

// QUIRK O-250: kept
#[test]
fn lobby_names_stay_raw() {
    // Free play (game type >= 0): the names come raw from the lobby, also with a leading number.
    let Some(r) = Run::corpus() else {
        return;
    };
    let texts = &r.host.layer.texts;
    let mut host = named_host(0);
    assert_eq!(player_display_name(&host, texts, 0), "29081 RED");
    assert_eq!(player_display_name(&host, texts, 1), "29081 RED");
    // With a profile name (network match without campaign) the profile slot applies all the same (O-250 applies to
    // every match).
    host.setup.local_name = Some("Anna".into());
    assert_eq!(player_display_name(&host, texts, 1), "Anna");
    assert_eq!(player_display_name(&host, texts, 0), "29081 RED");
}

// QUIRK O-250: kept
#[test]
fn accounting_view_is_unchanged() {
    // The accounting reads the names from the statistics log: the profile record stays raw, the other records resolve
    // the text ID of the campaign (existing test `table_wraps_on_overflow_and_fits_twelve_chars`).
    let Some(r) = Run::corpus() else {
        return;
    };
    let texts = &r.host.layer.texts;
    let mut host = named_host(-1);
    host.setup.local_name = Some("29081 Profile".into());
    let records = &mut host.state.accounting.records;
    for (slot, name) in [(1, "29081 Profile"), (2, "29081 RED")] {
        records[slot].active = true;
        records[slot].name = name.into();
    }
    let view = accounting_view(&host, texts);
    assert_eq!(view.rows.len(), 2);
    assert_eq!(view.rows[0].0, "29081 Profile", "profile record raw");
    assert_eq!(view.rows[1].0, texts.display(29081));
}

// --- Presentation of the scripts in the flow -----------------------------------------------------------------------

/// A presentation event like from a step to the fake (GPU-free part of `GameSession`).
fn present(r: &mut Run, event: opennml_sim::script::Presentation) {
    let g = r.view.game.as_mut().expect("match");
    g.presentation
        .apply_host(&event, &mut g.host, &mut g.selection);
}

fn speed(r: &Run) -> u8 {
    r.view.sim().unwrap().game_speed
}

// QUIRK E-169, E-238: kept
#[test]
fn cinemascope_tempo_follows_the_original() {
    // E-238 (`FUN_00430920`): 147 remembers the speed and sets 10, 148 restores the memory; a double 147 overwrites the
    // memory with 10; 148 without 147 sets 0 (half speed); closing the game menu sets the speed only without
    // cinemascope; after loading in the middle of cinemascope speed 10 applies with the saved memory, and 148 does not
    // get stuck at 10.
    use crate::game::presentation::{CINEMASCOPE_SPEED, ScriptPresentation};
    use crate::game::selection::Selection;
    use opennml_sim::script::Presentation::Cinemascope;
    let on = Cinemascope { on: true };
    let off = Cinemascope { on: false };
    let mut host = crate::game::sim_host::plain_testkit::host();
    let mut selection = Selection::default();
    let mut p = ScriptPresentation::new(&ViewOptions::default());
    host.game_speed = 14;
    p.apply_host(&on, &mut host, &mut selection);
    assert_eq!(
        (host.game_speed, p.tempo_memory),
        (CINEMASCOPE_SPEED, Some(14))
    );
    assert!(p.cinemascope);
    p.apply_host(&off, &mut host, &mut selection);
    assert_eq!(host.game_speed, 14);
    assert!(!p.cinemascope);
    // Double 147: the memory becomes 10, 148 stays at 10.
    p.apply_host(&on, &mut host, &mut selection);
    p.apply_host(&on, &mut host, &mut selection);
    assert_eq!(p.tempo_memory, Some(CINEMASCOPE_SPEED));
    p.apply_host(&off, &mut host, &mut selection);
    assert_eq!(host.game_speed, CINEMASCOPE_SPEED);
    // 148 without 147 in a new match: memory 0.
    let mut fresh = crate::game::sim_host::plain_testkit::host();
    fresh.game_speed = 14;
    let mut q = ScriptPresentation::new(&ViewOptions::default());
    q.apply_host(&off, &mut fresh, &mut selection);
    assert_eq!(fresh.game_speed, 0, "148 without 147 -> 0");
    // Loading in the middle of cinemascope: speed 10 and the memory from the host part; 148 restores it.
    let mut loaded = crate::game::sim_host::plain_testkit::host();
    loaded.game_speed = 3;
    let mut mission = loaded.state.script.mission.clone();
    mission.cinemascope = true;
    let mut l = ScriptPresentation::new(&ViewOptions::default());
    l.restore_host(&mission, Some(16), &mut loaded);
    assert_eq!(
        (loaded.game_speed, l.tempo_memory),
        (CINEMASCOPE_SPEED, Some(16))
    );
    assert!(l.cinemascope);
    l.apply_host(&off, &mut loaded, &mut selection);
    assert_eq!(loaded.game_speed, 16, "no sticking at 10");
    // Without a memory in the savegame (viewer): the speed before loading.
    let mut viewer = crate::game::sim_host::plain_testkit::host();
    viewer.game_speed = 12;
    let mut v = ScriptPresentation::new(&ViewOptions::default());
    v.restore_host(&mission, None, &mut viewer);
    assert_eq!(
        (viewer.game_speed, v.tempo_memory),
        (CINEMASCOPE_SPEED, Some(12))
    );

    // Close the game menu (`ResumeGame`, `FUN_0042E290`) in the flow: in cinemascope 10 stays, afterwards the speed
    // applies.
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.view.sim_mut().unwrap().game_speed = 14;
    present(&mut r, on.clone());
    assert_eq!(speed(&r), CINEMASCOPE_SPEED);
    // The fake runs without a script: the test sets the switch of the simulation like 147.
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = true;
    r.apply(UiEffect::ResumeGame {
        game_speed: Some(18),
    });
    assert_eq!(
        speed(&r),
        CINEMASCOPE_SPEED,
        "closing the game menu leaves 10"
    );
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = false;
    present(&mut r, off);
    assert_eq!(speed(&r), 14);
    r.apply(UiEffect::ResumeGame {
        game_speed: Some(18),
    });
    assert_eq!(speed(&r), 18);
    // The memory goes into the host part of the savegame.
    present(&mut r, on);
    let part = r.flow.host_part(&r.host.app, &r.view).unwrap();
    assert_eq!(part.cinemascope_speed, Some(18));
}

// QUIRK E-246, O-56, O-305: kept
#[test]
fn f1_and_f4_blocked_under_the_lock_but_escape_opens_the_menu() {
    // E-246, O-56, re/campaign-ui D6.10 (`0x0043C7DC`, `FUN_004C6050` `0x004C61FD`): the lock 18 keeps left click and
    // wheel away from the map via `InputGate` and switches off the keyboard and mouse camera; the right-click command
    // stays (O-233), likewise the `bindings2.ini` path (O-369). The shortcuts of `keys.ini` (F1/F4) have no effect, ESC
    // and the game menu stay open.
    use crate::game::hotkeys::Hotkeys;
    use crate::game::input::CameraKeys;
    use crate::game::input_gate::InputGate;
    use crate::game_session::{Pointer, frame_input};
    use winit::keyboard::KeyCode;
    let gate = InputGate {
        ui_locked: true,
        ..InputGate::default()
    };
    assert!(!gate.world_click() && !gate.player_camera() && !gate.keys_ini());
    assert!(gate.right_click_command() && gate.bindings());
    let mut pointer = Pointer {
        cursor: Some((0.0, 0.0)),
        ..Pointer::default()
    };
    pointer.keys.insert(KeyCode::ArrowUp);
    pointer.keys.insert(KeyCode::KeyW);
    let frame = |locked: bool| {
        frame_input(
            &pointer,
            &[],
            (800, 600),
            locked,
            false,
            &CameraKeys::Viewer,
            &Hotkeys::default(),
        )
    };
    let free = frame(!InputGate::default().player_camera());
    assert_ne!(free.camera, Default::default(), "arrow and edge move");
    let locked = frame(!gate.player_camera());
    assert_eq!(locked.camera, Default::default(), "game camera stands");
    assert_eq!(locked.free, free.free, "not the free camera of the viewer");

    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = true;
    r.until_step();
    assert_eq!(r.flow.gate, gate, "query of the fake from the simulation");
    // F1 does not save, F4 opens nothing; F10 (quest menu, O-364) does not exist.
    r.key("F1");
    r.idle();
    assert!(
        r.files.list(SaveDir::Main, 0).is_empty(),
        "no quicksave under the lock"
    );
    r.key("F4");
    r.idle();
    r.key("F10");
    r.idle();
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
    // ESC opens the game menu as usual.
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    // The flow passes input to the map on; the lock takes effect in `GameSession::input_with`.
    r.click("ingameoptions3", "return");
    r.idle();
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = false;
    r.until_step();
    r.key("F1");
    r.idle();
    assert_eq!(
        r.files.list(SaveDir::Main, 0).len(),
        1,
        "again after the lock"
    );
}

// QUIRK O-151: kept
#[test]
fn quick_save_uses_the_autosave_name() {
    // re/script-actions C.13 (`FUN_0042FA00`): 508 in the campaign saves after the step through the save path of the UI
    // under the text `AUTOSAVENAME` of the current campaign entry; the counter of F1 stays untouched.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let entry =
        opennml_ui::campaign::current_entry(r.host.app.profiles(), &r.host.app.data().campaigns)
            .expect("entry kamp1-1")
            .clone();
    assert!(
        (27600..=27631).contains(&entry.text_id),
        "AUTOSAVENAME {}",
        entry.text_id
    );
    let name = r.host.layer.texts.display(entry.text_id);
    assert!(!name.is_empty());
    present(&mut r, opennml_sim::script::Presentation::QuickSave);
    let steps = r.view.steps();
    r.frame();
    r.idle();
    let names: Vec<String> = r
        .files
        .list(SaveDir::Main, 0)
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, std::slice::from_ref(&name));
    let bytes = std::fs::read(r.files.path(&name, 0)).unwrap();
    let saved = GameSaveFile::from_bytes(&bytes).unwrap();
    assert_eq!(saved.host.game_type, -1);
    assert!(saved.sim.state.steps >= steps, "savegame after the step");
    // The counter of F1 stays untouched (`quicksave1` comes next; F1 itself is locked here by the introduction of
    // `kamp1-1`, 18).
    assert_eq!(r.flow.quicksave_counter, 0);
}

// QUIRK E-166: deviation
#[test]
fn profile_var_updates_the_current_profile() {
    // 382 (`FUN_004BD900` -> `FUN_0040DAA0`, E-166): the variable goes into the active profile (name lowercased, an
    // existing one is replaced); the next campaign start passes it to the simulation via `campaign_setup` (383).
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    for (name, value) in [("gold", 5), ("bonus", 1), ("gold", 7)] {
        present(
            &mut r,
            opennml_sim::script::Presentation::ProfileVar {
                name: name.into(),
                value,
            },
        );
    }
    r.frame();
    assert_eq!(
        r.host.app.profiles().current_vars(),
        [("gold".to_string(), 7), ("bonus".to_string(), 1)]
    );
    // Next start: the setup carries them.
    start_campaign(&mut r);
    let vars = r.view.last_setup.clone().unwrap().script.profile_vars;
    assert_eq!(vars.get("gold"), Some(&7));
    assert_eq!(vars.get("bonus"), Some(&1));
    let mission = &r.view.sim().unwrap().state.script.mission;
    assert_eq!(mission.profile_vars, vars, "mirror of the match");
}

// ---- Key path of the original (`FUN_004C6050`, re/game-ui D5.3) -----------------------------------------------------

impl Run {
    /// Key via the path of the window to the menu host (`MenuHost::on_key`): press, also as auto-repeat.
    fn press(&mut self, code: winit::keyboard::KeyCode) {
        self.host.on_key(code, true, None, S1024, self.now);
    }

    /// Key released (the table follows at the beginning of the next round).
    fn release(&mut self, code: winit::keyboard::KeyCode) {
        self.host.on_key(code, false, None, S1024, self.now);
    }

    /// Left mouse button via the path of the window (`MenuHost::on_input`).
    fn left_button(&mut self, pressed: bool) {
        let ev = InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        };
        self.host.on_input(&ev, S1024, self.now);
    }

    /// The commands that the key path has given to the match since the last call.
    fn hotkeys(&mut self) -> Vec<HotkeyCommand> {
        std::mem::take(&mut self.view.hotkeys)
    }
}

/// O-304: The UI lock blocks only `keys.ini` (also F1/F4 and `+`) and the screenshot; the bindings from
/// `bindings2.ini` and ESC continue to work.
// QUIRK O-14, O-152, O-304: kept
#[test]
fn lock_blocks_keys_ini_but_not_bindings_or_escape() {
    use HotkeyCommand as C;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = true;
    r.until_step();
    r.key("F1");
    r.idle();
    assert!(
        r.files.list(SaveDir::Main, 0).is_empty(),
        "F1 does not save"
    );
    assert!(r.hotkeys().is_empty());
    for chord in ["1", "CONTROL P", "HOME", "SHIFT 5", "CONTROL 0", "PAUSE"] {
        r.key(chord);
        r.frame();
    }
    assert_eq!(
        r.hotkeys(),
        [
            C::SelectGroup(1),
            C::TogglePause,
            C::SetCamera,
            C::AddToGroup(5),
            C::StoreGroup(0),
            C::TogglePause
        ]
    );
    r.key("SNAPSHOT");
    r.frame();
    assert!(!r.flow.take_hardcopy(), "no screenshot under the lock");
    // ESC sets the flag even under the lock; the game menu opens in the next frame with a tick.
    r.key("ESCAPE");
    r.host.frame_cmds(&r.vfs, S1024, r.now);
    r.flow.keys(&mut r.host.app, &mut r.view, r.now);
    assert!(r.flow.escape, "ESC flag set");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
}

/// O-301: With the left mouse button held the chord is `LBUTTON F1` or `LBUTTON 1` and hits neither the quicksave nor
/// a binding. The mouse button is not in the table until the next round (`HeldVk::begin_round` at the end of
/// `frame_cmds`).
// QUIRK O-116, O-301: kept
#[test]
fn mouse_button_in_the_chord_blocks_every_shortcut() {
    use winit::keyboard::KeyCode as K;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.left_button(true);
    r.frame();
    // Every key alone: press, menu frame (the name exactly `LBUTTON <key>`), key path; release and a frame, so that the
    // key drops out of the table at the beginning of the next round and the following one arrives alone with the mouse
    // button.
    for (code, name) in [
        (K::F1, "LBUTTON F1"),
        (K::Digit1, "LBUTTON 1"),
        (K::Home, "LBUTTON HOME"),
    ] {
        r.press(code);
        r.host.frame_cmds(&r.vfs, S1024, r.now);
        assert_eq!(r.host.app.take_untaken_keys(), [name]);
        assert!(
            !r.flow.key(name, &mut r.host.app, &mut r.view, r.now),
            "{name} triggers nothing"
        );
        assert!(r.hotkeys().is_empty(), "{name} hits no binding");
        r.release(code);
        r.frame();
    }
    r.idle();
    assert!(
        r.files.list(SaveDir::Main, 0).is_empty(),
        "LBUTTON F1 does not save"
    );
    assert!(r.hotkeys().is_empty());
    // Counter-check: mouse button released, after the start of the round the key takes effect.
    r.left_button(false);
    r.frame();
    r.press(K::Digit1);
    r.frame();
    r.release(K::Digit1);
    assert_eq!(r.hotkeys(), [HotkeyCommand::SelectGroup(1)]);
}

/// O-300: Every auto-repeat is an event of its own (`0x00414B70` does not read `lParam`), without a release event.
/// `+` (game speed, `keys.ini`) reaches the key path three times; `1` (binding) selects three times.
// QUIRK O-300: kept
#[test]
fn every_repeat_is_an_event() {
    use winit::keyboard::KeyCode as K;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    for _ in 0..3 {
        r.press(K::BracketRight);
    }
    r.host.frame_cmds(&r.vfs, S1024, r.now);
    assert_eq!(r.host.app.take_untaken_keys(), ["+", "+", "+"]);
    r.release(K::BracketRight);
    r.frame();
    for _ in 0..3 {
        r.press(K::Digit1);
    }
    r.frame();
    assert_eq!(r.hotkeys(), [HotkeyCommand::SelectGroup(1); 3]);
    r.release(K::Digit1);
    r.frame();
    // Three press events `+` without release trigger the game speed three times via `keys.ini` (the fake writes them
    // into `hotkeys`; the effect is checked by the tests in `speed_tests.rs`).
    for _ in 0..3 {
        r.press(K::BracketRight);
    }
    r.frame();
    assert_eq!(r.hotkeys(), [HotkeyCommand::ChangeGameSpeed(1); 3]);
}

/// A held key across a focus loss. `MenuHost::release_keys` empties the table (winit does not report the release);
/// after that the key belongs to no chord any more and repeats nothing.
// QUIRK O-300: kept
#[test]
fn held_key_stops_repeating_after_focus_loss() {
    use HotkeyCommand as C;
    use winit::keyboard::KeyCode as K;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.press(K::Digit1);
    r.frame();
    assert_eq!(r.hotkeys(), [C::SelectGroup(1)]);
    // Held: the next key is called `1 2` and hits no binding.
    r.press(K::Digit2);
    r.frame();
    r.release(K::Digit2);
    r.frame();
    assert!(r.hotkeys().is_empty(), "1 2 is no binding");
    // Focus loss: everything released, without a release event.
    r.host.release_keys();
    for _ in 0..5 {
        r.frame();
    }
    assert!(r.hotkeys().is_empty(), "no repeat after the focus loss");
    r.press(K::Digit2);
    r.frame();
    assert_eq!(
        r.hotkeys(),
        [C::SelectGroup(2)],
        "the old key is no longer in the chord"
    );
}

/// O-305: TAB (`KD_SPEEDUP_GAME`) and `KD_STOP_DIALOG` are in `keymap.ini`, the code never queries them: TAB does
/// nothing, ESC only opens the game menu.
// QUIRK O-152, O-305: kept
#[test]
fn tab_does_nothing_and_escape_only_opens_the_menu() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    assert!(!r.flow.key("TAB", &mut r.host.app, &mut r.view, r.now));
    r.key("TAB");
    r.idle();
    assert!(r.hotkeys().is_empty());
    assert_eq!(r.state(), 0);
    assert!(r.flow.key("ESCAPE", &mut r.host.app, &mut r.view, r.now));
    assert!(r.hotkeys().is_empty());
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
}

/// `KD_HARDCOPY` (`SNAPSHOT`) contained in the chord - also with a held mouse button - files the screenshot request
/// (`GameApp+0xD698`), except under the lock. The request is assigned per event (O-326): a further key in the same
/// frame clears it. Windows reports the print key only on release (`0x0042C357`): `MenuHost::on_key` makes an event of
/// it.
// QUIRK O-304, O-326: kept
#[test]
fn snapshot_in_the_chord_requests_a_hardcopy() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    r.key("SNAPSHOT");
    r.frame();
    assert!(r.flow.take_hardcopy());
    assert!(!r.flow.take_hardcopy(), "once per request");
    r.key("LBUTTON SNAPSHOT");
    r.frame();
    assert!(r.flow.take_hardcopy(), "subset: also with a mouse button");
    r.key("SNAPSHOT");
    r.key("A");
    r.frame();
    assert!(
        !r.flow.take_hardcopy(),
        "O-326: the second key clears the request"
    );
    r.release(winit::keyboard::KeyCode::PrintScreen);
    r.frame();
    assert!(r.flow.take_hardcopy(), "print key only released");
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = true;
    r.until_step();
    r.key("SNAPSHOT");
    r.frame();
    assert!(!r.flow.take_hardcopy(), "not under the lock");
}

/// O-308 (hypothesis, to be checked against the original): ESC opens the game menu also with an open build cursor,
/// the cursor stays (`0x00458510` has no ESC branch). Real `GameSession` without a window.
// QUIRK O-308: kept
#[test]
fn escape_with_an_open_build_cursor_opens_the_menu_and_keeps_the_cursor() {
    use opennml_ui::hud::toolbar::{BuildCursor, Cursor};
    let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
        return;
    };
    let Ok((device, queue, _)) =
        opennml_render::headless_device(opennml_render::BackendChoice::Auto)
    else {
        eprintln!("no GPU, test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let files = FsSaveFiles {
        dir: tmp.path().join("savegame"),
    };
    let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
    let config = EngineConfig {
        is_intro: false,
        ..test_config(&vfs)
    };
    let audio = FakeAudio::default();
    let mut host =
        MenuHost::new(&vfs, layer, config, MenuOptions::default(), &audio, None).unwrap();
    let mut flow = GameFlow::new(&host.layer.texts);
    flow.set_hotkeys(Hotkeys::load(&vfs));
    let (mut view_options, mut controls) = (ViewOptions::default(), Controls::default());
    let mut session: Option<GameSession> = None;
    fn live<'a>(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &'a Vfs,
        session: &'a mut Option<GameSession>,
        view: &'a mut ViewOptions,
        controls: &'a mut Controls,
    ) -> LiveSession<'a> {
        LiveSession {
            device: device.clone(),
            queue: queue.clone(),
            format: opennml_render::CAPTURE_FORMAT,
            vfs,
            session,
            view,
            controls,
            size: (1024, 768),
            audio: None,
        }
    }
    {
        let env = FlowEnv {
            vfs: &vfs,
            texts: &host.layer.texts,
            files: &files,
        };
        let mut view = live(
            &device,
            &queue,
            &vfs,
            &mut session,
            &mut view_options,
            &mut controls,
        );
        assert!(flow.apply(
            &UiEffect::StartMatch(request(0)),
            &mut host.app,
            &mut view,
            &env,
            &mut |_, _| {},
        ));
    }
    flow.begin_round();
    flow.begin_round();
    let mut now = 0;
    // A frame like `opennml-game`: menu frame, keys, match.
    let mut frame = |host: &mut MenuHost,
                     flow: &mut GameFlow,
                     session: &mut Option<GameSession>,
                     now: &mut u64| {
        let now_ms = *now;
        host.frame_cmds(&vfs, S1024, now_ms);
        let mut view = live(
            &device,
            &queue,
            &vfs,
            session,
            &mut view_options,
            &mut controls,
        );
        flow.keys(&mut host.app, &mut view, now_ms);
        flow.frame(now_ms, &mut host.app, &mut view, &host.layer.texts);
        *now += 16;
    };
    for _ in 0..3 {
        frame(&mut host, &mut flow, &mut session, &mut now);
    }
    let cursor = |session: &Option<GameSession>| {
        session
            .as_ref()
            .unwrap()
            .hud
            .toolbar
            .as_ref()
            .unwrap()
            .cursor
            .clone()
    };
    let guid = {
        let s = session.as_mut().unwrap();
        let guid = s.sim().unwrap().state.objects().next().unwrap().guid;
        s.hud.toolbar.as_mut().unwrap().cursor = Some(Cursor::Build(BuildCursor::new(guid)));
        guid
    };
    frame(&mut host, &mut flow, &mut session, &mut now);
    assert!(
        matches!(cursor(&session), Some(Cursor::Build(b)) if b.guid == guid),
        "build cursor open"
    );
    execute_ui(
        &mut host.app,
        &Command::UiKey {
            chord: "ESCAPE".into(),
            char: None,
        },
        S1024,
        now,
    )
    .unwrap();
    for _ in 0..60 {
        frame(&mut host, &mut flow, &mut session, &mut now);
        if host.app.game_menu_state() != 0 {
            break;
        }
    }
    assert_eq!(host.app.game_menu_state(), 1, "ESC opens the game menu");
    for _ in 0..10 {
        frame(&mut host, &mut flow, &mut session, &mut now);
    }
    assert!(
        matches!(cursor(&session), Some(Cursor::Build(b)) if b.guid == guid),
        "the cursor stays (O-308)"
    );
}

// ---- `keys.ini` with context (`FUN_0049D110`, re/game-ui D5.3) ------------------------------------------------------

/// O-307: The numpad (`ADD`, `SUBTRACT`) triggers nothing, only `+`/`-` of the main field (assignment 113/114).
// QUIRK O-307: kept
#[test]
fn numpad_plus_does_nothing() {
    use HotkeyCommand as C;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    for name in ["ADD", "SUBTRACT"] {
        assert!(
            !r.flow.key(name, &mut r.host.app, &mut r.view, r.now),
            "{name}"
        );
    }
    assert!(r.hotkeys().is_empty());
    r.key("+");
    r.key("-");
    r.frame();
    assert_eq!(r.hotkeys(), [C::ChangeGameSpeed(1), C::ChangeGameSpeed(-1)]);
}

/// Quicksave and quickload come from `keys.ini` (578/581, context 0/0/0, so in every context); `GameFlow` executes
/// them, the match does not see them. Without the two lines F1/F4 do nothing.
// QUIRK O-302: kept
#[test]
fn f1_and_f4_come_from_keys_ini() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let full = Hotkeys::load(&r.vfs);
    let mut without = full.clone();
    without.keys.retain(|l| l.action != 578 && l.action != 581);
    r.flow.set_hotkeys(without);
    r.start();
    r.until_step();
    assert!(!r.flow.key("F1", &mut r.host.app, &mut r.view, r.now));
    assert!(!r.flow.key("F4", &mut r.host.app, &mut r.view, r.now));
    r.idle();
    assert!(
        r.files.list(SaveDir::Main, 0).is_empty(),
        "without line 578 no saving"
    );
    assert!(r.dialogs().is_empty(), "without line 581 no loading");
    // With the lines, in a different context (building, nation 3, selection 1000): the 0 lines apply everywhere.
    r.flow.set_hotkeys(full);
    r.view.key_context = Some(KeyContext {
        kind: 3,
        nation: 3,
        selection: 1000,
    });
    r.key("F1");
    r.idle();
    let names: Vec<String> = r
        .files
        .list(SaveDir::Main, 0)
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["quicksave1"]);
    r.key("F4");
    r.idle();
    assert_eq!(r.dialogs(), ["ingameloadgame"]);
    assert!(r.hotkeys().is_empty(), "not to the match");
}

/// With the game menu open, untaken keys go to bindings and `keys.ini`; quicksave checks the menu state itself and
/// does not save.
#[test]
fn untaken_keys_reach_bindings_with_the_game_menu_open() {
    use HotkeyCommand as C;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    for chord in ["+", "1", "F1"] {
        r.key(chord);
        r.frame();
    }
    r.idle();
    assert_eq!(r.hotkeys(), [C::ChangeGameSpeed(1), C::SelectGroup(1)]);
    assert!(
        r.files.list(SaveDir::Main, 0).is_empty(),
        "F1 does not save with the menu open"
    );
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
}

/// First own unit with a walk module of the local slot.
fn own_walker(r: &Run) -> opennml_sim::id::ObjectId {
    let sim = r.view.sim().unwrap();
    let local = sim.active_player;
    sim.state
        .objects()
        .find(|o| o.owner() == local && o.has_module(0, false))
        .expect("own unit with walk module")
        .id
}

/// O-316, O-152: `CONTROL P` toggles the pause of the match; during the pause no steps run, but engine ticks without a
/// world tick - so ESC opens the game menu also during the pause, in the next tick.
// QUIRK O-317: kept
#[test]
fn escape_opens_the_menu_in_the_pause() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.key("CONTROL P");
    r.frame();
    assert_eq!(
        r.view.pause(),
        PauseState {
            sim: true,
            shown: true
        }
    );
    let steps = r.view.steps();
    let mut ticks = 0;
    for _ in 0..30 {
        ticks += r.frame();
    }
    assert_eq!(r.view.steps(), steps, "no world tick");
    assert!(ticks > 0, "engine ticks run");
    r.key("ESCAPE");
    r.until_step();
    assert_eq!(r.state(), 1, "game menu also during the pause");
    r.idle();
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    assert_eq!(r.view.steps(), steps);
    assert!(r.view.pause().sim, "the menu does not lift the pause");
}

/// re/ingame-menus 4 (`0x27`): giving up lifts the pause (`togglepause` at `+0x24E4`) and gives the defeat at once,
/// without a tick.
// QUIRK O-316: kept
#[test]
fn surrender_resumes() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.key("CONTROL P");
    r.frame();
    assert!(r.view.pause().sim);
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    let steps = r.view.steps();
    r.click("ingameoptions3", "quitgame");
    r.idle();
    assert_eq!(r.dialogs(), ["infoframe"]);
    r.key("RETURN");
    r.idle();
    assert_eq!(r.view.pause(), PauseState::default(), "pause lifted");
    let slot = local(&r);
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.state.players[usize::from(slot)].result, 1);
    assert_eq!(sim.state.steps, steps, "no tick");
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    // Without a pause, giving up does not switch one on (`togglepause` only with the bit set).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.apply(UiEffect::Surrender { slot: local(&r) });
    assert_eq!(r.view.pause(), PauseState::default());
}

/// O-318: pause, movement command (executed at once), quicksave during the pause, resume; the replay gives the same
/// checksum as the match. The savegame has the state with the command, but no pause: after loading the match runs and
/// arrives at the same state as the replay.
// QUIRK O-318: kept
#[test]
fn paused_commands_survive_save_load_and_replay() {
    use opennml_sim::save::{Replay, checksum};
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.key("CONTROL P");
    r.frame();
    assert!(r.view.pause().sim);
    let unit = own_walker(&r);
    let sim = r.view.sim_mut().unwrap();
    let pos = sim.state.object(unit, false).unwrap().world_pos;
    let steps = sim.state.steps;
    let target = glam::Vec3::new(pos[0] + 400.0, pos[1], pos[2] + 400.0);
    let order = crate::game::formation_order::FormationOrder::default();
    assert!(sim.move_units(&[unit], target, None, order));
    assert!(sim.pending_commands().is_empty(), "executed at once");
    assert_eq!(sim.log.last().unwrap().step, steps);
    let paused_sum = checksum(&sim.state);
    // Quicksave during the pause (state 0): the savegame has the command in the state.
    r.key("F1");
    r.idle();
    assert_eq!(r.view.steps(), steps, "no tick during the pause");
    let bytes = std::fs::read(r.files.path("quicksave1", 0)).unwrap();
    let file = GameSaveFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.sim.state.steps, steps);
    assert_eq!(checksum(&file.sim.state), paused_sum);
    // Resume: the unit walks, the replay arrives at the same state.
    r.key("CONTROL P");
    r.frame();
    assert!(!r.view.pause().sim);
    while r.view.steps() < steps + 30 {
        r.frame();
    }
    let sim = r.view.sim().unwrap();
    assert_ne!(sim.state.object(unit, false).unwrap().world_pos, pos);
    let replay = Replay::from_bytes(&sim.replay().unwrap().to_bytes()).unwrap();
    let rules = sim.rules.clone();
    let played = replay.play(&rules, sim.state.steps).unwrap();
    assert_eq!(checksum(&played), checksum(&sim.state), "replay equal");
    // Load: without a pause, the same state; the progress meets the replay.
    r.apply(UiEffect::LoadGame {
        name: "quicksave1".into(),
    });
    assert_eq!(
        r.view.pause(),
        PauseState::default(),
        "pause not in the savegame"
    );
    assert_eq!(r.view.steps(), steps);
    assert_eq!(checksum(&r.view.sim().unwrap().state), paused_sum);
    r.flow.begin_round();
    r.flow.begin_round();
    while r.view.steps() < steps + 30 {
        r.frame();
    }
    let sim = r.view.sim().unwrap();
    let played = replay.play(&rules, sim.state.steps).unwrap();
    assert_eq!(checksum(&played), checksum(&sim.state), "loaded like live");
}

/// F1 at tick 100, play on, F4/load: the log is in the savegame, the replay stays complete and leads to the state of
/// the match.
// QUIRK E-329: deviation
#[test]
fn quick_save_and_load_keep_the_replay() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    let unit = own_walker(&r);
    let sim = r.view.sim_mut().unwrap();
    let pos = sim.state.object(unit, false).unwrap().world_pos;
    let target = glam::Vec3::new(pos[0] + 400.0, pos[1], pos[2] + 400.0);
    let order = crate::game::formation_order::FormationOrder::default();
    assert!(sim.move_units(&[unit], target, None, order));
    while r.view.steps() < 100 {
        r.frame();
    }
    r.key("F1");
    let until = r.now + 2000;
    while !r.files.exists(SaveDir::Main, 0, "quicksave1") {
        assert!(r.now < until, "not written");
        r.frame();
    }
    let bytes = std::fs::read(r.files.path("quicksave1", 0)).unwrap();
    let file = GameSaveFile::from_bytes(&bytes).unwrap();
    let saved = file.host.log.expect("log in the savegame");
    assert!(!saved.is_empty(), "the command is in the log");
    let at = file.sim.state.steps;
    while r.view.steps() < at + 30 {
        r.frame();
    }
    r.apply(UiEffect::LoadGame {
        name: "quicksave1".into(),
    });
    assert_eq!(r.view.steps(), at);
    r.flow.begin_round();
    r.flow.begin_round();
    while r.view.steps() < at + 30 {
        r.frame();
    }
    let sim = r.view.sim().unwrap();
    assert!(sim.log_complete());
    let replay = sim.replay().expect("replay complete");
    assert!(replay.commands.starts_with(&saved));
    let rules = sim.rules.clone();
    let played = replay.play(&rules, sim.state.steps).unwrap();
    assert_eq!(
        opennml_sim::save::checksum(&played),
        opennml_sim::save::checksum(&sim.state),
        "replay equal"
    );
}

// ---- Game speed per key (`FUN_0042F050`) ----------------------------------------------------------------------------

impl Run {
    /// Tap and release a key, one frame each (the flow evaluates it in the frame).
    fn tap(&mut self, code: winit::keyboard::KeyCode) {
        self.press(code);
        self.frame();
        self.release(code);
        self.frame();
    }

    /// Game speed of the match and of the setting `GameSpeed`.
    fn speeds(&self) -> (u8, i32) {
        (speed(self), self.host.app.config().game_speed)
    }

    fn set_speeds(&mut self, v: u8) {
        self.view.sim_mut().unwrap().game_speed = v;
        self.host.app.set_game_speed(i32::from(v));
    }
}

/// O-320: In cinemascope the keys act on the speed 10 and the setting, the memory (`cinemascope_speed`) stays 14; the
/// end restores the memory (14), the setting keeps the key value (11), and the next closing of the game menu takes it
/// over (O-153).
// QUIRK O-320: kept
// QUIRK O-153: deviation
#[test]
fn speed_keys_in_cinemascope_keep_the_memory() {
    use opennml_sim::script::Presentation::Cinemascope;
    use winit::keyboard::KeyCode as K;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.set_speeds(14);
    present(&mut r, Cinemascope { on: true });
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = true;
    assert_eq!(r.speeds(), (10, 14));
    assert_eq!(r.view.cinemascope_speed(), Some(14));
    r.tap(K::BracketRight);
    assert_eq!(
        r.speeds(),
        (11, 11),
        "key in cinemascope: game speed and setting"
    );
    assert_eq!(r.view.cinemascope_speed(), Some(14), "memory untouched");
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = false;
    present(&mut r, Cinemascope { on: false });
    assert_eq!(r.speeds(), (14, 11), "end: memory 14, setting 11");
    // Game menu open and closed: `RETURN` takes over the setting.
    assert!(r.host.app.open_game_menu());
    r.idle();
    let dialog = r.dialogs()[0].clone();
    r.click(&dialog, "return");
    r.idle();
    assert_eq!(r.speeds(), (11, 11));
}

/// The speed keys check neither pause nor game menu; during the pause and with the game menu open they change game
/// speed and setting, the world stays still.
// QUIRK O-319: kept
#[test]
fn speed_key_works_in_pause_and_with_the_game_menu_open() {
    use winit::keyboard::KeyCode as K;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.set_speeds(10);
    r.key("CONTROL P");
    r.frame();
    assert!(r.view.pause().sim);
    r.tap(K::BracketRight);
    assert_eq!(r.speeds(), (11, 11), "during the pause");
    r.tap(K::Slash);
    assert_eq!(r.speeds(), (10, 10), "`-` during the pause");
    r.key("CONTROL P");
    r.frame();
    assert!(!r.view.pause().sim);
    // Game menu open: the world stands, the key takes effect all the same.
    r.until_step();
    assert!(r.host.app.open_game_menu());
    r.idle();
    let steps_open = r.view.steps();
    r.tap(K::BracketRight);
    r.tap(K::BracketRight);
    assert_eq!(r.speeds(), (12, 12), "with the game menu open");
    assert_eq!(r.view.steps(), steps_open, "the menu halts the world");
}

// --- Pointer ---------------------------------------------------------------------------------------------------------

// QUIRK O-299: kept
#[test]
fn pointer_hidden_only_in_campaign_cinemascope() {
    // re/game-ui D5.2 (`0x004309FF`): cinemascope hides the pointer only in the campaign (game type -1), its end shows
    // it; in free play it stays. It is set at the edges: the game menu opened in cinemascope shows it (`FUN_0042E150`),
    // and it stays visible. With the menu open the flow chooses 7/0.
    use crate::game::pointer::PointerChoice;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    assert_eq!(
        r.flow.pointer_choice(&r.host.app, &r.view),
        PointerChoice::NORMAL,
        "without a match"
    );
    r.start();
    r.frame();
    assert!(!r.host.app.pointer_hidden());
    let cinemascope = |r: &mut Run, on: bool| {
        r.view.sim_mut().unwrap().state.script.mission.cinemascope = on;
    };
    // Free play (game type 0): cinemascope leaves the pointer.
    assert_eq!(r.view.game_type, 0);
    cinemascope(&mut r, true);
    r.frame();
    assert!(!r.host.app.pointer_hidden(), "cinemascope in free play");
    cinemascope(&mut r, false);
    r.frame();
    assert!(!r.host.app.pointer_hidden());
    // Campaign: off while the cinemascope runs. The game type comes from the flow (`set_match_info` per frame); it is
    // switched at the edge of the cinemascope (`GameFlow::refresh_lock`).
    r.flow.meta.as_mut().unwrap().game_type = -1;
    cinemascope(&mut r, true);
    r.frame();
    assert_eq!(r.view.game_type, -1);
    assert!(r.host.app.pointer_hidden(), "cinemascope of the campaign");
    r.frame();
    assert!(r.host.app.pointer_hidden());
    // ESC game menu shows it and keeps it (no edge); the flow chooses 7/0.
    assert!(r.host.app.open_game_menu());
    r.idle();
    assert!(!r.host.app.pointer_hidden(), "game menu in cinemascope");
    assert!(!r.host.app.menu().open_names().is_empty());
    assert_eq!(
        r.flow.pointer_choice(&r.host.app, &r.view),
        PointerChoice::NORMAL
    );
    // End of cinemascope: on; new cinemascope: off again.
    cinemascope(&mut r, false);
    r.frame();
    assert!(!r.host.app.pointer_hidden());
    cinemascope(&mut r, true);
    r.frame();
    assert!(r.host.app.pointer_hidden());
    cinemascope(&mut r, false);
    r.frame();
    assert!(!r.host.app.pointer_hidden());
}

// --- Input via `InputGate`, save lock, pointer -----------------------------------------------------------------------

/// ESC opens the game menu (one frame with a tick later), then `RETURN`.
fn open_and_close_menu(r: &mut Run, menu: &str) -> (bool, bool) {
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    assert_eq!(r.dialogs(), [menu]);
    let open = r.host.app.pointer_hidden();
    r.click(menu, "return");
    r.idle();
    assert_eq!(r.state(), 0);
    (open, r.host.app.pointer_hidden())
}

// QUIRK O-299, O-379: kept
#[test]
fn pointer_follows_the_menu_only_under_the_lock() {
    // O-371, O-379, re/campaign-ui D6.17: ESC switches the pointer on (`FUN_0042E150` `0x0042E183`), closing the game
    // menu switches it off only under the lock (`FUN_0042E290` `0x0042E2BE`) - also without cinemascope, although it
    // was visible before. The end of a cinemascope switches it on, the start off only in the campaign (`FUN_00430920`).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    let hidden = |r: &Run| r.host.app.pointer_hidden();
    assert!(!hidden(&r));
    assert_eq!(
        open_and_close_menu(&mut r, "ingameoptions3"),
        (false, false),
        "without the lock it stays on"
    );
    // Lock without cinemascope: ESC on, closing off (O-379).
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = true;
    r.until_step();
    assert!(r.host.app.game().unwrap().ui_locked, "lock per frame");
    assert_eq!(
        open_and_close_menu(&mut r, "ingameoptions3"),
        (false, true),
        "off under the lock"
    );
    // The end of the lock does not switch it on (hypothesis O-379: only the end of a cinemascope).
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = false;
    r.until_step();
    assert!(!r.host.app.game().unwrap().ui_locked);
    assert!(hidden(&r));
    // Free play (game type 0): the start of cinemascope leaves the pointer, the end switches it on.
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = true;
    r.until_step();
    assert!(hidden(&r), "start changes nothing");
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = false;
    r.until_step();
    assert!(!hidden(&r), "end -> on");
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = true;
    r.until_step();
    assert!(!hidden(&r), "never off outside the campaign");
}

// QUIRK O-56, O-371: kept
#[test]
fn escape_in_a_scene_opens_the_game_menu_and_holds_the_script() {
    // O-56, re/campaign-ui D6.10: scenes cannot be skipped; ESC opens the game menu also in cinemascope under the lock
    // (`0x0043E1A0`) and halts script and simulation in single player (`0x00445650`). There only "Save" is locked
    // (re/campaign-ui D6.9). The pointer: off in the cinemascope of the campaign, ESC on, closing under the lock off
    // again (O-371).
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let until = r.now + 10_000;
    while !(r.flow.gate.cinemascope && r.flow.gate.ui_locked) {
        assert!(
            r.now < until,
            "introduction of kamp1-1 without cinemascope and lock"
        );
        r.frame();
    }
    assert!(
        r.host.app.pointer_hidden(),
        "cinemascope of the campaign: off"
    );
    // ESC (not `KD_STOP_DIALOG`): the game menu, no skipping.
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    assert_eq!(r.dialogs(), ["ingameoptions"]);
    assert!(!r.host.app.pointer_hidden(), "ESC -> on");
    let report = r.host.app.report(S1024, r.now);
    let enabled = |name: &str| {
        report
            .elements
            .iter()
            .find(|e| e.dialog == "ingameoptions" && e.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .enabled
    };
    assert!(!enabled("savegame"), "saving disabled under the lock");
    assert!(enabled("load") && enabled("questmenu") && enabled("tiplogmenu"));
    // Script and simulation stand still while the menu is open.
    let (steps, checksum) = (r.view.steps(), r.view.checksum());
    for _ in 0..30 {
        assert_eq!(r.frame(), 0);
    }
    assert_eq!((r.view.steps(), r.view.checksum()), (steps, checksum));
    assert!(r.flow.gate.cinemascope && r.flow.gate.ui_locked);
    // Back: the scene continues, the pointer is off again.
    r.click("ingameoptions", "return");
    r.idle();
    assert_eq!(r.state(), 0);
    assert!(r.host.app.pointer_hidden(), "closing under the lock -> off");
    r.until_step();
    assert!(r.view.steps() > steps);
}

#[test]
fn loading_inside_the_campaign_cinemascope_hides_the_pointer() {
    // A savegame from the cinemascope of the campaign has bars and no pointer after loading - `started` resets the
    // edges, `refresh_lock` sees the start of the cinemascope (`FUN_00430920`, game type -1) and switches the pointer
    // off, even if the game menu had switched it on before.
    let Some(mut r) = campaign_run() else {
        return;
    };
    start_campaign(&mut r);
    let until = r.now + 10_000;
    while !(r.flow.gate.cinemascope && r.flow.gate.ui_locked) {
        assert!(
            r.now < until,
            "introduction of kamp1-1 without cinemascope and lock"
        );
        r.frame();
    }
    let save = GameSaveFile::new(
        r.view.sim().unwrap().save(),
        r.flow.host_part(&r.host.app, &r.view).unwrap(),
        None,
    );
    std::fs::create_dir_all(&r.files.dir).unwrap();
    std::fs::write(r.files.path("scene", 0), save.to_bytes()).unwrap();
    // ESC switches the pointer on (the game menu needs it), the savegame brings the scene back.
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    assert_eq!(r.state(), 1);
    assert!(!r.host.app.pointer_hidden(), "ESC -> on");
    r.apply(UiEffect::LoadGame {
        name: "scene".into(),
    });
    r.idle();
    assert_eq!(r.state(), 0, "the game runs after loading");
    assert!(r.flow.gate.cinemascope && r.flow.gate.ui_locked);
    assert!(
        r.host.app.pointer_hidden(),
        "loading in the cinemascope of the campaign: pointer off"
    );
}

// QUIRK O-53: kept
#[test]
fn mission_follows_the_simulation_each_frame_and_ends_with_the_match() {
    // `refresh_lock` gives main objectives and `m_InfoHistory` per frame to the UI (`MenuApp::set_mission`), `0x0E`
    // sets them back to `None`. Secondary objectives do not go along (O-53).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    assert_eq!(
        r.host.app.data().mission,
        Some(MissionInfo::default()),
        "empty mission state"
    );
    {
        let mission = &mut r.view.sim_mut().unwrap().state.script.mission;
        mission.main_quests[2] = opennml_sim::script::Quest {
            status: 3,
            text: 20100,
        };
        mission.sub_quests[0] = opennml_sim::script::Quest {
            status: 3,
            text: 20103,
        };
        mission.info_history = vec![20100, 20101];
    }
    r.until_step();
    let m = r.host.app.data().mission.clone().expect("mission");
    assert_eq!(m.main_quests[2], (3, 20100));
    assert_eq!(m.main_quests.iter().filter(|q| q.0 != 0).count(), 1);
    assert_eq!(m.info_history, [20100, 20101]);
    // The host does not touch menu state and queue.
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());

    let slot = local(&r);
    submit_result(&mut r, slot, opennml_sim::game_end::DEFEAT);
    r.until_step();
    r.idle();
    r.click("gameend_defeat", "end_defeat");
    r.idle();
    assert!(r.host.app.game().is_none());
    assert_eq!(
        r.host.app.data().mission,
        None,
        "None at the end of the match"
    );
}

// QUIRK O-305: kept
#[test]
fn f10_opens_nothing() {
    // O-364: `KD_CHECK_QUESTMENU` is unassigned in the original; F10 opens neither `QUESTMENU` nor any other dialog,
    // also without the lock (the lock is checked by `f1_and_f4_blocked_under_the_lock_but_escape_opens_the_menu`).
    let Some(mut r) = Run::corpus() else {
        return;
    };
    start_real(&mut r, request(0));
    r.until_step();
    r.key("F10");
    r.idle();
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
    assert!(r.flow.running());
    // Also with the game menu open everything stays as it is.
    r.key("ESCAPE");
    r.until_step();
    r.idle();
    let dialogs = r.dialogs();
    assert_eq!(r.state(), 1);
    r.key("F10");
    r.idle();
    assert_eq!(r.state(), 1);
    assert_eq!(r.dialogs(), dialogs);
}

/// Button 579 asks for the game menu; the flow fetches the request directly after the input or the key command,
/// without a tick in between, and opens only at menu state 0.
#[test]
fn ui_request_opens_the_game_menu_at_once_only_in_state_0() {
    use crate::game_session::UiRequest;
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.start();
    r.until_step();
    let steps = r.view.steps();
    r.view.ui_requests.push(UiRequest::GameMenu);
    let click = InputEvent::Button {
        button: MouseButton::Left,
        pressed: true,
    };
    r.flow
        .input(&click, S1024, r.now, &mut r.host.app, &mut r.view);
    assert_eq!(r.state(), 1, "at once after the input");
    assert!(r.view.ui_requests.is_empty());
    assert_eq!(r.view.steps(), steps, "no tick in between");
    r.idle();
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
    // With the game menu open (state != 0) a further request stays without effect; it is fetched all the same.
    r.view.ui_requests.push(UiRequest::GameMenu);
    r.frame();
    assert!(r.view.ui_requests.is_empty());
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
}

// --- Random map: savegame and load path ----------------------------------------------

/// Seed `ZKB`, small map (64 x 64), 2 players, `continental`, meadow, day, game mode NORMAL (index 0).
fn random_spec(vfs: &Vfs) -> opennml_randommap::RandomMapSpec {
    let types = opennml_randommap::load_types(vfs).unwrap();
    opennml_randommap::RandomMapSpec {
        seed_text: "ZKB".into(),
        size: 0,
        players: 2,
        terrain: types
            .map_scripts
            .iter()
            .position(|s| s.to_lowercase().ends_with("/continental.rnd"))
            .unwrap(),
        landscape: types
            .landscape_types
            .iter()
            .position(|l| l == "MEADOW")
            .unwrap(),
        daytime: types.daytime_types.iter().position(|d| d == "DAY").unwrap(),
        game_type: 0,
    }
}

/// Starts a random-map match (human and computer) directly via [`SessionView::start`] (real simulation), makes 20
/// steps and writes it as savegame `name` (host part as after `0x0B`); after that no match. Returns the setup and the
/// checksum at the start.
fn write_random_save(
    r: &mut Run,
    spec: &opennml_randommap::RandomMapSpec,
    name: &str,
) -> (MatchSetup, u64) {
    use opennml_sim::setup::{LobbyEntry, SlotKind};
    r.view.real_levels = true;
    let setup = MatchSetup {
        mode: spec.game_type as i32,
        lobby: [(SlotKind::Human, "Mensch"), (SlotKind::Ai, "KI")]
            .into_iter()
            .zip(0..)
            .map(|((kind, name), team)| LobbyEntry {
                kind,
                name: name.into(),
                nation: 0,
                team,
                color: team,
                handicap: 0,
            })
            .collect(),
        ..MatchSetup::default()
    };
    let source = MapSource::Random(spec.clone());
    let chosen = setup.clone();
    SessionView::start(
        &mut r.view,
        &r.vfs,
        &source,
        &mut |_| Ok(chosen.clone()),
        10,
        &mut |_| {},
    )
    .unwrap();
    let at_start = r.view.checksum();
    for _ in 0..20 {
        r.view.sim_mut().unwrap().step_once();
    }
    let host = HostPart {
        camera: r.view.camera(),
        level_info: LevelInfoData {
            name: spec.seed_text.clone(),
            players: spec.players,
            level_type: 1,
            ..LevelInfoData::default()
        },
        game_type: spec.game_type as i32,
        local_name: "Mensch".into(),
        game_speed: 10,
        cinemascope_speed: None,
        script_camera: ScriptCamera::default(),
        control_groups: control_groups::empty_save(),
        next_guid_index: control_groups::empty_next_guid_index(),
        idle_worker_index: [0; 8],
        masks: Masks::initial(),
        lock_teams: false,
        fog_show_mode: 0,
        net: None,
        log: None,
        save_replay: false,
    };
    // Like `0x0B`: with the generated map of the match.
    let map = r.view.random_map().cloned().map(Box::new);
    assert!(map.is_some(), "generated map of the match");
    let file = GameSaveFile::new(r.view.sim().unwrap().save(), host, map);
    assert_eq!(file.sim.start.random_map.as_ref(), Some(spec));
    write_file(&r.files.path(name, 0), &file.to_bytes()).unwrap();
    r.view.end();
    (setup, at_start)
}

/// O-431: A savegame of a random map loads with `default.ldg`; the map comes from the generated map in the savegame -
/// `start.level` (`randommap:ZKB`) is not read as a file (the fake with a real simulation reads fixed
/// maps via `load_level`, which would fail here).
// QUIRK D-341, O-431: deviation
#[test]
fn load_of_a_random_map_save_shows_default_ldg() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let spec = random_spec(&r.vfs);
    write_random_save(&mut r, &spec, "random");
    assert!(!r.vfs.exists("randommap:ZKB"));
    r.shown.clear();
    r.apply(UiEffect::LoadGame {
        name: "random".into(),
    });
    assert!(r.flow.running(), "savegame loaded");
    assert_eq!(
        r.shown
            .iter()
            .map(|(l, p)| (l.as_str(), *p))
            .collect::<Vec<_>>(),
        [
            (DEFAULT_LDG, 0),
            (DEFAULT_LDG, 40),
            (DEFAULT_LDG, 70),
            (DEFAULT_LDG, 100)
        ]
    );
    assert_eq!(r.view.level(), "randommap:ZKB");
    assert_eq!(r.view.steps(), 20);
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.rules.random_map.as_ref().map(|m| &m.spec), Some(&spec));
    assert_eq!(sim.rules.level_path, "randommap:ZKB");
}

/// `0x1F` restarts a random map from its description (not via the map path, which is not a file) - with the same
/// setup, bit-identical to the first start, loading screen of the restart `randommap.ldg` (`FUN_00547560`, O-431).
// QUIRK O-431: deviation
#[test]
fn reload_level_restarts_a_random_map() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let spec = random_spec(&r.vfs);
    let (setup, at_start) = write_random_save(&mut r, &spec, "random");
    r.apply(UiEffect::LoadGame {
        name: "random".into(),
    });
    assert!(r.flow.running(), "savegame loaded");
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    assert!(r.view.steps() > 20);
    r.shown.clear();
    r.apply(UiEffect::ReloadLevel);
    assert_eq!(r.shown.len(), 4, "restarted");
    assert_eq!(r.shown[0].0, opennml_ui::loading::RANDOM_MAP_LDG);
    assert_eq!(r.view.steps(), 0);
    assert_eq!(r.view.last_setup.as_ref(), Some(&setup));
    assert_eq!(r.view.level(), "randommap:ZKB");
    assert_eq!(r.view.checksum(), at_start, "like the first start");
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.rules.random_map.as_ref().map(|m| &m.spec), Some(&spec));
}

/// `MapLoad::open` builds a random map from its description (scene `randommap:ZKB`, rules input, state for
/// `random_map`), the simulation loads the rules with it without `.lvl`; a fixed map has neither.
#[test]
fn map_load_opens_a_random_map_with_its_state() {
    use crate::game::map::MapLoad;
    let Some(r) = Run::corpus() else {
        return;
    };
    let spec = random_spec(&r.vfs);
    let source = MapSource::Random(spec.clone());
    assert_eq!(source.label(), "randommap:ZKB");
    let mut load = MapLoad::open(&r.vfs, &source).unwrap();
    assert_eq!(load.scene.lvl_path, "randommap:ZKB");
    assert_eq!(load.random.as_ref().map(|m| &m.spec), Some(&spec));
    let state = load.random_state.as_ref().unwrap();
    assert_eq!(state.seed_text, "ZKB");
    assert_eq!(state.dim, 64);
    load.start_sim(&r.vfs, "DEU", 10, |rules| {
        assert_eq!(rules.random_map.as_ref().map(|m| &m.spec), Some(&spec));
        Ok(MatchSetup::default())
    })
    .unwrap_err();
    // A setup with the game mode of the description starts.
    load.start_sim(&r.vfs, "DEU", 10, |_| {
        Ok(MatchSetup {
            mode: 0,
            ..MatchSetup::default()
        })
    })
    .unwrap();
    assert!(load.sim.is_some());
    let fixed = MapLoad::open(&r.vfs, &MapSource::Level(TWO_PLAYERS.into())).unwrap();
    assert!(fixed.random.is_none() && fixed.random_state.is_none());
}

// --- Random map: start from the menu -------------------------------------------------------

/// Start request of a random map as after `CREATE2` (`FUN_004BF1F0` with `LevelType 1`) with the values of
/// [`random_spec`]: human and one computer, seed in `Info.name`; the map path is that of the last chosen fixed map
/// (`Info+0x50` stays) and is not read.
fn random_request(spec: &opennml_randommap::RandomMapSpec) -> MatchRequest {
    let mut req = request(spec.game_type as i32);
    req.info = LevelInfoData {
        name: spec.seed_text.clone(),
        path: TWO_PLAYERS.into(),
        size_class: spec.size,
        players: spec.players,
        level_type: 1,
        random_terrain: spec.terrain as i32,
        random_landscape: spec.landscape as i32,
        random_daytime: spec.daytime as i32,
    };
    req
}

/// O-431 (restart): `0x0D` with `LevelType 1` starts the random map from the map description of the session (not via
/// the map path) with the loading screen `randommap.ldg`; the match is called `randommap:<seed>`, the simulation has
/// the rules without `.lvl`.
// QUIRK O-431, O-433: deviation
#[test]
fn random_map_start_uses_randommap_ldg() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.view.real_levels = true;
    let spec = random_spec(&r.vfs);
    r.apply(UiEffect::StartMatch(random_request(&spec)));
    assert!(r.flow.running(), "match running");
    let ldg = opennml_ui::loading::RANDOM_MAP_LDG;
    assert_eq!(
        r.shown
            .iter()
            .map(|(l, p)| (l.as_str(), *p))
            .collect::<Vec<_>>(),
        [(ldg, 0), (ldg, 40), (ldg, 70), (ldg, 100)]
    );
    assert_eq!(r.view.level(), "randommap:ZKB");
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.rules.random_map.as_ref().map(|m| &m.spec), Some(&spec));
    assert_eq!(sim.rules.level_path, "randommap:ZKB");
    let setup = r.view.last_setup.clone().unwrap();
    assert_eq!((setup.mode, setup.lobby.len()), (0, 2));
    assert_eq!(r.host.app.game().unwrap().game_type, 0);
    // Also without a map path (draft without a previously chosen fixed map): no abort "no map chosen".
    let mut req = random_request(&spec);
    req.level.clear();
    req.info.path.clear();
    r.shown.clear();
    r.apply(UiEffect::StartMatch(req));
    assert_eq!(r.shown.len(), 4);
    assert_eq!(r.view.level(), "randommap:ZKB");
}

/// D-327: if the generator fails (here a map type that `randommaptypes.ini` does not know), the start aborts with a
/// message - no match, menu music on again, `singleplayermenu` open.
// QUIRK D-327: deviation
#[test]
fn random_map_start_failure_reopens_the_menu() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.view.real_levels = true;
    capture_warnings();
    let spec = random_spec(&r.vfs);
    let mut req = random_request(&spec);
    req.info.random_terrain = 99;
    r.apply(UiEffect::StartMatch(req));
    assert_eq!(
        r.shown[0],
        (opennml_ui::loading::RANDOM_MAP_LDG.to_string(), 0)
    );
    let warnings = take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("randommap:ZKB"), "{warnings:?}");
    assert!(warnings[0].contains("map type 99 unknown"), "{warnings:?}");
    assert!(!r.flow.running());
    assert!(r.host.app.game().is_none());
    assert!(r.view.game.is_none());
    assert_eq!(r.view.music, ["stop", "restart"]);
    r.idle();
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
}

/// The message `MSGBOX` over the menu: its text ID, `None` without a message.
pub(super) fn msgbox_id(r: &Run) -> Option<i32> {
    if !r.dialogs().iter().any(|d| d == "msgbox") {
        return None;
    }
    r.host.app.screen_state().info_text
}

/// `save_failure_text` picks the own text by the kind of the error; without a `SaveError` the stage decides: not yet
/// read (file, name) is damaged, read but not loadable (map, rules, start) does not match the game data.
#[test]
fn save_failure_text_maps_every_kind() {
    use crate::game_save::SaveErrorKind::*;
    use opennml_ui::own_texts::{SAVE_DAMAGED, SAVE_MISMATCH, SAVE_OTHER_FORMAT};
    for (kind, decoded, id) in [
        (Some(Format), false, SAVE_OTHER_FORMAT),
        (Some(Newer), false, SAVE_OTHER_FORMAT),
        (Some(Corrupted), true, SAVE_DAMAGED),
        (Some(Mismatch), true, SAVE_MISMATCH),
        (None, false, SAVE_DAMAGED),
        (None, true, SAVE_MISMATCH),
    ] {
        assert_eq!(save_failure_text(kind, decoded), id, "{kind:?} {decoded}");
    }
}
