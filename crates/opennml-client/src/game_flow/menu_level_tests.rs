//! Tests of the menu level (re/menu-level ML.2 - ML.4, ML.15): start while building `START`, run, stopping in
//! submenus with their own `BACKGROUND`, resuming without catching up. Host and flow as in `tests.rs` with the fake
//! on real maps (`real_levels`) and `IsMenuLevel 1`; silently skipped without `NML_GAME_DIR`.

use opennml_control::Command;
use opennml_ui::app::{LevelWorld, MenuOptions};
use opennml_ui::loading::DEFAULT_LDG;

use super::tests::{Run, S1024};
use super::*;
use crate::ui_control::state_data;

/// Path of the menu level as the flow leads it (`LEVELBACKGROUND` from `start.dlg`, normalized).
const LEVEL: &str = "data/levels/menülevel/menü2.lvl";

/// Host with main menu and `IsMenuLevel 1`; the fake loads real maps.
fn menu_level_run() -> Option<Run> {
    let mut r = Run::corpus_configured(
        MenuOptions {
            start_menu: true,
            ..MenuOptions::default()
        },
        |config| config.is_menu_level = true,
    )?;
    r.view.real_levels = true;
    Some(r)
}

/// Host with a running, visible menu level behind `START`.
fn started() -> Option<Run> {
    let mut r = menu_level_run()?;
    r.idle();
    assert_eq!(
        r.flow.menu_level(),
        Some((LEVEL, true)),
        "menu level running"
    );
    Some(r)
}

/// Names of the visible background images of the desktop.
fn desktop_image_names(r: &Run) -> Vec<String> {
    state_data(&r.host.app, S1024, r.now)
        .images
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// `n` frames, sum of the ticks.
fn frames(r: &mut Run, n: usize) -> u32 {
    (0..n).map(|_| r.frame()).sum()
}

/// `START` -> `SINGLEPLAYERMENU` (without profile: `SELECTPROFILE`); both dialogs carry `BACKGROUND` and send `0x20`.
fn hide_via_singleplayer(r: &mut Run) {
    r.click("start", "singleplayermenu");
    r.idle();
    assert_eq!(r.dialogs(), ["selectprofile"]);
}

/// Back to `START` (`CANCEL` in `SELECTPROFILE`): building `START` sends `0x21`.
fn back_to_start(r: &mut Run) {
    r.click("selectprofile", "cancel");
    r.idle();
    assert!(
        r.dialogs().contains(&"start".to_string()),
        "{:?}",
        r.dialogs()
    );
}

// QUIRK O-439: kept
#[test]
fn start_build_loads_the_menu_level() {
    // re/menu-level ML.2, O-439: Building `START` sends `0x21`; without an engine, `FUN_004C9140` loads `menü2.lvl`
    // through `Game_StartLevel` with game type -2 and the loading screen `default.ldg` (`menü2.ldg` is missing).
    // Step 2 of `Game_StartLevel` is skipped at -2: menus, desktop images and title music stay.
    let Some(mut r) = menu_level_run() else {
        return;
    };
    r.frame();
    assert_eq!(
        r.flow.menu_level(),
        None,
        "START built, 0x21 runs in the next frame"
    );
    let images = desktop_image_names(&r);
    r.idle();
    let phases: Vec<(String, u8)> = [0, 40, 70, 100]
        .into_iter()
        .map(|p| (DEFAULT_LDG.to_string(), p))
        .collect();
    assert_eq!(r.shown, phases, "loading screen default.ldg in phases");
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)));
    assert_eq!(r.view.level(), LEVEL);
    assert_eq!(r.view.last_setup.as_ref().map(|s| s.mode), Some(-2));
    assert_eq!(r.view.game_type, -2, "game type passed to the session");
    assert!(!r.flow.running(), "no match");
    assert!(r.host.app.game().is_none(), "MenuApp sees no match");
    assert!(!r.flow.take_menus_closed(), "no Game_StartLevel step 2");
    assert_eq!(desktop_image_names(&r), images, "desktop images stay");
    assert!(r.dialogs().contains(&"start".to_string()));
    assert_eq!(r.view.music, ["menu-level"], "menu music not stopped");
    assert!(r.flow.world_shown());
    assert_eq!(
        r.host.app.level_world(),
        LevelWorld {
            present: true,
            shown: true
        }
    );
}

// QUIRK O-439: kept
#[test]
fn load_time_counts_for_the_menus_only_after_the_menu_level_load() {
    // re/menu-level ML.15 no. 1: In the original, `START` stands at its destination right after the loading screen;
    // the frame of `START` ticks on `GetTickCount` (`FUN_004615A0`), so the synchronous loading in the command runs
    // into its fly-in. The flow tells the host that it counts the load time toward the menus this time (until the next
    // frame); the first frame of the session has frame time 0.
    let Some(mut r) = menu_level_run() else {
        return;
    };
    r.frame();
    r.apply(UiEffect::MenuLevel(
        opennml_ui::msg::MenuLevelEffect::Load {
            path: "DATA/LEVELS/MENÜLEVEL/MENÜ2.LVL".into(),
        },
    ));
    assert!(r.flow.just_loaded());
    assert!(r.flow.load_time_counts_for_menus());
    assert_eq!(r.view.steps(), 0);
    // First frame after loading (in `opennml-game` already at the clock after loading): frame time 0, no ticks.
    r.now += 2_000;
    let now = r.now;
    assert_eq!(
        r.flow
            .frame(now, &mut r.host.app, &mut r.view, &r.host.layer.texts),
        0
    );
    assert_eq!(r.view.game.as_ref().unwrap().clock.program_ms, now as f64);
    assert!(!r.flow.load_time_counts_for_menus());
    // After that it runs at the frame rate.
    r.now += 100;
    let now = r.now;
    assert!(
        r.flow
            .frame(now, &mut r.host.app, &mut r.view, &r.host.layer.texts)
            > 0
    );
}

// QUIRK O-435: kept
#[test]
fn menu_level_steps_like_a_match() {
    let Some(mut r) = started() else {
        return;
    };
    let before = r.view.steps();
    let steps = frames(&mut r, 200);
    assert!(steps > 0, "ticks run");
    assert_eq!(r.view.steps(), before + u64::from(steps));
    let sim = r.view.sim().unwrap();
    assert!(
        sim.state.script.mission.cinemascope,
        "menü2.abl switches cinemascope on in the first tick (O-435)"
    );
    assert_eq!(sim.game_speed, 10, "game speed 10 in cinemascope");
    assert_eq!(r.view.match_info, Some((-2, false)));
}

// QUIRK O-437: kept
#[test]
fn submenu_hides_and_halts() {
    // re/menu-level ML.4, O-437: `SINGLEPLAYERMENU`/`SELECTPROFILE` carry `BACKGROUND` -> `0x20`: invisible and
    // stopped, the session stays.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 50);
    hide_via_singleplayer(&mut r);
    let steps = r.view.steps();
    assert!(!r.flow.world_shown());
    assert_eq!(r.flow.menu_level(), Some((LEVEL, false)));
    assert_eq!(
        r.host.app.level_world(),
        LevelWorld {
            present: true,
            shown: false
        }
    );
    assert_eq!(frames(&mut r, 100), 0, "no ticks");
    assert_eq!(r.view.steps(), steps);
    assert!(r.view.sim().is_some(), "the session stays");
    // Back to `START`: visible again, the same session keeps running.
    back_to_start(&mut r);
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)));
    assert!(frames(&mut r, 100) > 0);
    assert_eq!(r.shown.len(), 4, "no second load");
}

#[test]
fn end_menu_level_closes_the_session() {
    // `FUN_004BAF40`: engine gone, `D5E0 = 6`; a second call does nothing (it is called before every start).
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 10);
    r.flow.end_menu_level(&mut r.host.app, &mut r.view);
    assert_eq!(r.flow.menu_level(), None);
    assert!(r.view.game.is_none(), "session closed");
    assert!(!r.flow.world_shown());
    assert_eq!(r.host.app.level_world(), LevelWorld::default());
    r.flow.end_menu_level(&mut r.host.app, &mut r.view);
    assert_eq!(r.flow.menu_level(), None);
    assert_eq!(frames(&mut r, 10), 0);
}

// QUIRK O-437: kept
#[test]
fn resume_does_not_catch_up() {
    // re/menu-level ML.4 (`GameEngine_Update` `0x00445650`): at most 2 ticks in the first frame after resuming, the
    // simulation does not catch up the pause; afterwards the same tick rate, and the state is that of a run without
    // interruption.
    let Some(mut r) = started() else {
        return;
    };
    let before = frames(&mut r, 100);
    hide_via_singleplayer(&mut r);
    frames(&mut r, 5);
    r.now += 60_000;
    frames(&mut r, 5);
    let held = r.view.steps();
    r.click("selectprofile", "cancel");
    let mut first = None;
    for _ in 0..100 {
        let steps = r.frame();
        if r.flow.world_shown() {
            first = Some(steps);
            break;
        }
    }
    let first = first.expect("visible again");
    assert!(first <= 2, "first frame {first} ticks");
    assert_eq!(r.view.steps(), held + u64::from(first));
    r.idle();
    let after = frames(&mut r, 100);
    assert!(
        after.abs_diff(before) <= 2,
        "ticks before {before}, after {after}"
    );
    // Run without interruption up to the same tick count.
    let Some(mut b) = started() else {
        return;
    };
    for _ in 0..10_000 {
        let (sa, sb) = (r.view.steps(), b.view.steps());
        if sa == sb {
            break;
        }
        if sa < sb {
            r.frame();
        } else {
            b.frame();
        }
    }
    assert_eq!(r.view.steps(), b.view.steps());
    assert_eq!(
        r.view.checksum(),
        b.view.checksum(),
        "checksum as without pause"
    );
}

// QUIRK O-447: kept
#[test]
fn script_camera_catches_up_after_resume() {
    // O-447 (re/menu-level ML.15 no. 4): `GameEngine_Update` gives the script camera (`GameEngine_UpdateCameraFollow`
    // `0x00432590`) the real time since the last `Update` without an upper limit; `GameApp_Tick` does not call
    // `Update` while the menu level is stopped, `FUN_004C9140` does not reset the time base. So in the first frame
    // after resuming the camera gets the whole pause time, the simulation does not.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 20);
    // Last running frame: `0x20` does not come until after the fly-out of `START`, when `SELECTPROFILE` is built.
    let mut last_running = r.now - 16;
    r.click("start", "singleplayermenu");
    loop {
        let now = r.now;
        r.frame();
        if !r.flow.world_shown() {
            break;
        }
        last_running = now;
    }
    r.idle();
    assert_eq!(r.dialogs(), ["selectprofile"]);
    r.now += 30_000;
    frames(&mut r, 3);
    r.click("selectprofile", "cancel");
    let mut resumed = None;
    for _ in 0..100 {
        let now = r.now;
        r.frame();
        if r.flow.world_shown() {
            resumed = Some(now);
            break;
        }
    }
    let resumed = resumed.expect("visible again");
    let game = r.view.game.as_ref().unwrap();
    assert_eq!(game.camera_catch_up.len(), 1, "caught up once");
    let held = game.camera_catch_up[0];
    // Camera dt of the frame = stopped time + frame time = time since the last running frame.
    let expected = (resumed - 16 - last_running) as f32;
    assert_eq!(held, expected, "caught up");
    assert!(held >= 30_000.0);
    frames(&mut r, 20);
    assert_eq!(
        r.view.game.as_ref().unwrap().camera_catch_up.len(),
        1,
        "nothing more afterwards"
    );
}

// QUIRK O-437: kept
#[test]
fn options_keep_it_running() {
    // `OPTIONS` carries `LEVELBACKGROUND` (again `0x21`), the subpages only flag `0x80`: the world keeps running.
    let Some(mut r) = started() else {
        return;
    };
    r.click("start", "options");
    r.idle();
    assert!(
        r.dialogs().contains(&"options".to_string()),
        "{:?}",
        r.dialogs()
    );
    assert!(r.flow.world_shown());
    assert!(frames(&mut r, 100) > 0, "OPTIONS: ticks run");
    for page in [
        "gameplayoptions",
        "videooptions",
        "soundoptions",
        "controloptions",
    ] {
        r.click("options", page);
        r.idle();
        assert!(r.dialogs().contains(&page.to_string()), "{:?}", r.dialogs());
        assert!(r.flow.world_shown(), "{page}");
        assert!(frames(&mut r, 100) > 0, "{page}: ticks run");
    }
}

// QUIRK O-437: kept
#[test]
fn second_start_build_does_not_reload() {
    // re/menu-level ML.2 step 5: if an engine is running, `0x21` only sets bit `0x1`.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 30);
    let steps = r.view.steps();
    r.click("start", "options");
    r.idle();
    r.click("options", "ok");
    r.idle();
    assert!(
        r.dialogs().contains(&"start".to_string()),
        "{:?}",
        r.dialogs()
    );
    assert_eq!(r.shown.len(), 4, "no second loading screen");
    assert_eq!(r.view.music, ["menu-level"], "one start");
    assert!(r.view.steps() > steps, "the same session keeps running");
}

// QUIRK O-439: kept
#[test]
fn failed_menu_level_load_keeps_the_menu() {
    // If loading fails: warning, no menu level, the menu stays; the next build tries again.
    let Some(mut r) = menu_level_run() else {
        return;
    };
    r.view.fail_level = Some(LEVEL.into());
    r.idle();
    assert_eq!(r.flow.menu_level(), None);
    assert!(!r.flow.world_shown());
    assert!(!r.flow.running());
    assert!(
        r.dialogs().contains(&"start".to_string()),
        "{:?}",
        r.dialogs()
    );
    assert_eq!(r.view.music, ["menu-level"], "menu music not restarted");
    assert_eq!(r.host.app.level_world(), LevelWorld::default());
    assert_eq!(
        r.shown,
        [(DEFAULT_LDG.to_string(), 0)],
        "loading screen up to the error"
    );
    r.view.fail_level = None;
    r.ui(Command::UiClick {
        dialog: "start".into(),
        element: "options".into(),
        button: None,
    });
    r.idle();
    assert_eq!(
        r.flow.menu_level(),
        Some((LEVEL, true)),
        "OPTIONS tries again"
    );
    assert_eq!(r.shown.len(), 5, "second load with loading screen");
}

/// Names of the loading screens of the last `n` loading screen calls (`.ldg`).
fn last_ldgs(r: &Run, n: usize) -> Vec<&str> {
    r.shown[r.shown.len() - n..]
        .iter()
        .map(|(ldg, _)| ldg.as_str())
        .collect()
}

// QUIRK O-439: kept
#[test]
fn campaign_start_ends_the_paused_menu_level() {
    // Starting a campaign map (`FUN_004BAF40` in `Game_StartLevel`) ends the (stopped) menu level before loading:
    // `view.end`, state gone, game type -1, loading screen of the map.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 20);
    hide_via_singleplayer(&mut r);
    assert_eq!(r.flow.menu_level(), Some((LEVEL, false)));
    assert_eq!(r.view.ends, 0);
    r.apply(UiEffect::StartCampaignLevel {
        level: super::tests::KAMP1_1.into(),
        difficulty: 0,
    });
    assert_eq!(r.flow.menu_level(), None);
    assert_eq!(r.view.ends, 1, "view.end before loading");
    assert!(r.flow.running(), "match running");
    assert_eq!(r.view.game_type, -1);
    assert_eq!(r.view.level(), super::tests::KAMP1_1);
    assert_eq!(
        last_ldgs(&r, 4),
        ["data/levels/kampagne1/kamp1-1/kamp1-1.ldg"; 4]
    );
    assert_eq!(
        r.host.app.level_world(),
        LevelWorld::default(),
        "match instead of menu level"
    );
}

// QUIRK O-439: kept
#[test]
fn match_end_restarts_it_on_start() {
    // Match -> game end `0x0E` -> accounting -> `MAINMENU` -> `START`: loading screen `default.ldg` again, new session
    // (tick 0), the menu level runs again in the initial view (re/menu-level ML.15 no. 1).
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 20);
    // Like the path through the menus: the commands of the menu close all dialogs before `{0x0D}`.
    r.host.app.close_all_menus();
    r.apply(UiEffect::StartMatch(super::tests::request(0)));
    assert_eq!(
        r.flow.menu_level(),
        None,
        "starting the match ends the menu level"
    );
    assert!(r.flow.running());
    assert_eq!(r.view.ends, 1);
    frames(&mut r, 20);
    r.apply(UiEffect::EndMatch);
    r.idle();
    assert!(!r.flow.running());
    assert_eq!(
        r.flow.menu_level(),
        None,
        "the accounting has no menu level"
    );
    let loaded = r.shown.len();
    r.click("accounting", "mainmenu");
    let mut steps_at_start = None;
    for _ in 0..200 {
        r.frame();
        if r.flow.menu_level().is_some() {
            steps_at_start = Some(r.view.steps());
            break;
        }
    }
    assert_eq!(steps_at_start, Some(0), "new session, tick 0");
    assert_eq!(r.shown.len(), loaded + 4);
    assert_eq!(last_ldgs(&r, 4), [DEFAULT_LDG; 4], "loading screen again");
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)));
    r.idle();
    assert!(r.dialogs().contains(&"start".to_string()));
    assert!(frames(&mut r, 50) > 0);
}

// QUIRK O-439: kept
#[test]
fn load_game_ends_the_menu_level() {
    // Loading (`0x0A`) ends the menu level before loading, like every level start.
    let Some(mut r) = menu_level_run() else {
        return;
    };
    r.view.real_levels = true;
    r.host.app.close_all_menus();
    r.apply(UiEffect::StartMatch(super::tests::request(0)));
    r.apply(UiEffect::SaveGame { name: "ml".into() });
    r.apply(UiEffect::EndMatch);
    r.idle();
    r.click("accounting", "mainmenu");
    r.idle();
    assert_eq!(
        r.flow.menu_level(),
        Some((LEVEL, true)),
        "START restarts it"
    );
    frames(&mut r, 20);
    let ends = r.view.ends;
    r.apply(UiEffect::LoadGame { name: "ml".into() });
    assert_eq!(r.flow.menu_level(), None);
    assert_eq!(r.view.ends, ends + 1, "view.end before loading");
    assert!(r.flow.running(), "savegame loaded");
    assert_eq!(r.view.game_type, 0);
    assert_eq!(last_ldgs(&r, 4), [DEFAULT_LDG; 4]);
}

// QUIRK O-439: kept
#[test]
fn failed_start_restarts_the_menu_level_on_start() {
    // If the start fails after the end (empty map path), there is no menu level any more and no match; the way back
    // through `START` restarts it, with loading screen.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 20);
    let loaded = r.shown.len();
    let mut req = super::tests::request(0);
    req.level.clear();
    r.host.app.close_all_menus();
    r.apply(UiEffect::StartMatch(req));
    assert_eq!(r.flow.menu_level(), None);
    assert_eq!(r.view.ends, 1);
    assert!(!r.flow.running());
    assert!(r.host.app.game().is_none(), "no set_game");
    assert_eq!(
        r.shown.len(),
        loaded,
        "no loading screen for the empty path"
    );
    assert!(!r.flow.world_shown());
    assert_eq!(r.host.app.level_world(), LevelWorld::default());
    // `reopen_menu` opens the single-player menu; back to `START` builds it again.
    r.idle();
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    assert_eq!(r.flow.menu_level(), None);
    r.click("singleplayermenu", "cancel");
    r.idle();
    assert!(r.dialogs().contains(&"start".to_string()));
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)), "START restarts");
    assert_eq!(r.shown.len(), loaded + 4);
    assert_eq!(last_ldgs(&r, 4), [DEFAULT_LDG; 4]);
}

/// `START` -> `OPTIONS` -> `GAMEPLAYOPTIONS` -> toggle the checkbox `menulevel` -> back up to `START`.
fn toggle_menulevel_and_back(r: &mut Run) {
    r.click("start", "options");
    r.idle();
    r.click("options", "gameplayoptions");
    r.idle();
    r.click("gameplayoptions", "menulevel");
    r.idle();
    r.click("options", "ok");
    r.idle();
    assert!(
        r.dialogs().contains(&"start".to_string()),
        "{:?}",
        r.dialogs()
    );
}

// QUIRK O-438: kept
#[test]
fn menulevel_switch_takes_effect_on_the_next_start_build() {
    // re/menu-level ML.15 no. 6, O-438: `IsMenuLevel` does not take effect until the next build (`start.dlg`
    // read again).
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 30);
    r.click("start", "options");
    r.idle();
    r.click("options", "gameplayoptions");
    r.idle();
    r.click("gameplayoptions", "menulevel");
    r.idle();
    assert!(!r.host.app.config().is_menu_level);
    assert!(r.flow.world_shown(), "checkbox off: nothing happened yet");
    assert!(frames(&mut r, 50) > 0);
    r.click("options", "ok");
    r.idle();
    // Back to `START`: the variant without menu level sends `0x20` - stopped, invisible, session present.
    assert!(r.dialogs().contains(&"start".to_string()));
    assert_eq!(r.flow.menu_level(), Some((LEVEL, false)));
    assert!(!r.flow.world_shown());
    let steps = r.view.steps();
    assert_eq!(frames(&mut r, 50), 0);
    assert_eq!(r.view.steps(), steps);
    assert!(r.view.sim().is_some(), "session stays");
    // On again (2D-`OPTIONS` -> `GAMEPLAYOPTIONS`), back to `START`: continues, without loading screen.
    toggle_menulevel_and_back(&mut r);
    assert!(r.host.app.config().is_menu_level);
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)));
    assert!(r.flow.world_shown());
    assert_eq!(r.shown.len(), 4, "no second loading screen");
    assert!(frames(&mut r, 50) > 0);
}

// QUIRK O-436: deviation
#[test]
fn net_start_ends_the_menu_level() {
    // Deviates from O-436: `START` -> `LAN1` (stopped) -> `LAN3` -> `LAN2` -> start: starting the network match ends
    // the menu level; the menu level writes nothing into the network session or the lobby.
    use super::net_tests::{Lan, start_shown};
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    n.runs[0].apply(UiEffect::MenuLevel(
        opennml_ui::msg::MenuLevelEffect::Load {
            path: "DATA/LEVELS/MENÜLEVEL/MENÜ2.LVL".into(),
        },
    ));
    assert_eq!(n.runs[0].flow.menu_level(), Some((LEVEL, true)));
    n.host(0, Some("tinte"));
    n.join(1);
    assert_eq!(
        n.runs[0].flow.menu_level(),
        Some((LEVEL, false)),
        "LAN1 stops it, the session stays"
    );
    assert!(n.runs[1].flow.menu_level().is_none());
    let rows = n.lobby(0).rows;
    assert_eq!(rows.len(), 2, "lobby state of the host");
    assert_eq!(rows[0].name, "Anna");
    assert_eq!(rows[1].name, "Bert");
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.frames_until("both running", |l| {
        (0..2).all(|i| l.runs[i].view.steps() > 0)
    });
    assert_eq!(n.runs[0].flow.menu_level(), None, "menu level gone");
    assert_eq!(n.runs[0].view.ends, 1);
    assert!(n.runs[0].flow.running());
    assert_eq!(n.runs[0].view.game_type, 0);
}

/// Start a menu video through the real path: fake opener, `play_video_by_name`, frames until it runs.
fn start_video(r: &mut Run) {
    let log = std::rc::Rc::new(std::cell::RefCell::new(
        crate::testutil::FakeVideoLog::default(),
    ));
    r.host
        .set_video_opener(crate::testutil::fake_opener(&log, false, None));
    r.host.app.play_video_by_name("t").unwrap();
    for _ in 0..50 {
        r.frame();
        if r.host.app.video().is_some() {
            return;
        }
    }
    panic!("video does not start");
}

/// Frames until the video is over.
fn finish_video(r: &mut Run) {
    for _ in 0..200 {
        r.frame();
        if r.host.app.video().is_none() {
            r.frame();
            return;
        }
    }
    panic!("video does not end");
}

// QUIRK O-437: kept
#[test]
fn video_keeps_the_old_state() {
    // re/menu-level ML.4: Every menu video fades out the menu level and stops it, afterwards the old state is back.
    // The flow never calls `restart_menu_music` here (the `RestartMusic` of the menu).
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 30);
    let steps = r.view.steps();
    start_video(&mut r);
    r.frame();
    assert!(!r.flow.world_shown());
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)), "old state saved");
    let under_video = r.flow.menu_level_data(r.view.sim()).unwrap();
    assert!(!under_video.shown, "the world is invisible under the video");
    assert_eq!(under_video.steps, r.view.steps());
    assert_eq!(
        r.host.app.level_world(),
        LevelWorld {
            present: true,
            shown: false
        }
    );
    let held = r.view.steps();
    assert!(held >= steps);
    assert_eq!(r.frame(), 0, "no ticks");
    assert_eq!(r.view.steps(), held);
    finish_video(&mut r);
    assert!(r.flow.world_shown());
    assert!(frames(&mut r, 50) > 0, "runs again");
    // The `RestartMusic` after the video comes from the menu host (`MenuHost`, continuing with the menu level as game
    // music), not from the flow.
    assert_eq!(
        r.view.music,
        ["menu-level"],
        "the flow does not touch the music"
    );
    // An invisible menu level stays invisible after the video.
    hide_via_singleplayer(&mut r);
    start_video(&mut r);
    finish_video(&mut r);
    assert_eq!(r.flow.menu_level(), Some((LEVEL, false)));
    assert!(!r.flow.world_shown());
    assert_eq!(frames(&mut r, 20), 0);
}

#[test]
fn video_end_with_menu_level_resumes_game_music() {
    // re/menu-level ML.4 B: With a menu level, the end of the video calls `MusicManager_ResumeMusic` instead of
    // `MenuMusic_Restart`; this also holds for an invisible menu level (session present).
    let Some(mut r) = started() else {
        return;
    };
    for hidden in [false, true] {
        if hidden {
            frames(&mut r, 50);
            hide_via_singleplayer(&mut r);
        }
        r.audio.calls.clear();
        start_video(&mut r);
        finish_video(&mut r);
        // The `AfterVideo` comes two frames after the end of the video, so still inside the window.
        let mut resumed = false;
        for _ in 0..5 {
            frames(&mut r, 1);
            resumed = r.audio.calls.iter().any(|c| c == "resume");
            if resumed {
                break;
            }
        }
        assert!(resumed, "hidden={hidden}: {:?}", r.audio.calls);
        // The first `RestartMusic` uses up the window: a second one (e.g. from the sound options) calls
        // `MenuMusic_Restart` again.
        r.audio.calls.clear();
        r.host
            .apply(vec![UiEffect::RestartMusic], &mut r.audio, &r.vfs);
        assert_eq!(
            r.audio.calls,
            ["restart"],
            "hidden={hidden}: {:?}",
            r.audio.calls
        );
    }
}

#[test]
fn video_end_without_menu_level_restarts_the_menu_music() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    r.idle();
    assert_eq!(r.flow.menu_level(), None);
    r.audio.calls.clear();
    start_video(&mut r);
    finish_video(&mut r);
    frames(&mut r, 5);
    let music: Vec<&str> = r
        .audio
        .calls
        .iter()
        .map(String::as_str)
        .filter(|c| matches!(*c, "resume" | "restart"))
        .collect();
    assert_eq!(music, ["restart"], "{:?}", r.audio.calls);
}

#[test]
fn failed_load_game_keeps_the_paused_menu_level() {
    // A savegame that cannot be loaded (missing, broken) is no `Game_StartLevel`: the stopped menu level stays (same
    // session, no `view.end`), the menu is open again.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 20);
    hide_via_singleplayer(&mut r);
    let steps = r.view.steps();
    let shown = r.shown.len();
    std::fs::create_dir_all(&r.files.dir).unwrap();
    std::fs::write(r.files.path("broken", 0), b"NMLGAME\0broken").unwrap();
    for name in ["missing", "broken"] {
        r.apply(UiEffect::LoadGame { name: name.into() });
        assert_eq!(r.flow.menu_level(), Some((LEVEL, false)), "{name}");
        assert_eq!(r.view.ends, 0, "{name}: no view.end");
        assert!(r.view.sim().is_some(), "{name}: same session");
        assert_eq!(r.shown.len(), shown, "{name}: no loading screen");
        assert!(!r.flow.running());
    }
    r.idle();
    assert!(!r.dialogs().is_empty(), "menu open again");
    assert_eq!(r.view.steps(), steps);
    assert_eq!(r.flow.menu_level(), Some((LEVEL, false)));
}

// ---- Input and locks (re/menu-level ML.6, ML.8, ML.15 no. 5; O-441 ... O-443) ---------------------------------------
//
// Many of these locks take effect structurally: `GameFlow::key` and `GameFlow::input` return anyway without a running
// match. The tests still pin the behavior down, as a regression net for later changes.

/// An input through the path of the window like `opennml-game` (`MenuHost::on_input`, then `GameFlow::input`).
fn window(r: &mut Run, ev: InputEvent) {
    r.host.on_input(&ev, S1024, r.now);
    r.flow
        .input(&ev, S1024, r.now, &mut r.host.app, &mut r.view);
}

/// A physical key through the path of the window (`MenuHost::on_key`, then `GameFlow::input`), without a frame.
fn window_key(r: &mut Run, code: winit::keyboard::KeyCode, pressed: bool) {
    r.host.on_key(code, pressed, None, S1024, r.now);
    let ev = InputEvent::Key {
        code,
        pressed,
        repeat: false,
    };
    r.flow
        .input(&ev, S1024, r.now, &mut r.host.app, &mut r.view);
}

/// Pointer to `(x, y)` (window pixels at 1024 x 768).
fn move_to(r: &mut Run, x: f64, y: f64) {
    window(r, InputEvent::CursorMoved { x, y });
}

/// Press or release a mouse button through the path of the window.
fn button(r: &mut Run, button: MouseButton, pressed: bool) {
    window(r, InputEvent::Button { button, pressed });
}

/// Steps and checksum after each of `n` frames; `input(r, i)` runs before frame `i`.
fn trace(r: &mut Run, n: usize, mut input: impl FnMut(&mut Run, usize)) -> Vec<(u64, u64)> {
    (0..n)
        .map(|i| {
            input(r, i);
            r.frame();
            (r.view.steps(), r.view.checksum())
        })
        .collect()
}

/// What the inputs must not change: no commands and no inputs to the session, no pause, no game menu, no requests of
/// the match.
fn assert_untouched(r: &Run, what: &str) {
    assert!(r.view.hotkeys.is_empty(), "{what}: {:?}", r.view.hotkeys);
    assert!(r.view.inputs.is_empty(), "{what}: {:?}", r.view.inputs);
    assert!(r.view.ui_requests.is_empty(), "{what}");
    assert_eq!(r.view.pause(), PauseState::default(), "{what}: no pause");
    assert_eq!(r.host.app.game_menu_state(), 0, "{what}: no game menu");
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)), "{what}");
}

// QUIRK O-441, O-442: kept
#[test]
fn menu_level_blocks_game_keys() {
    // O-441 (re/menu-level ML.6, ML.8): In the menu level, ESC (game menu `FUN_0042E150` not until -1), Ctrl+P and
    // Pause (`GameEngine_Pause` `< −1`), F1/F4 (`0x0042F4A0`/`0x0042F5D4`), Home (`FUN_00420230`), the camera keys,
    // the wheel, the middle button (`GameEngine_UpdateCamera` `0x0043C80F`) and the chat keys (`0x0042EA42`) have no
    // effect. The run is the same frame by frame as without input.
    use winit::keyboard::KeyCode as K;
    let (Some(mut a), Some(mut b)) = (started(), started()) else {
        return;
    };
    // Counter-check: in a match, the same keys would be bindings from `bindings2.ini`.
    for (chord, cmd) in [
        ("CONTROL P", HotkeyCommand::TogglePause),
        ("PAUSE", HotkeyCommand::TogglePause),
        ("HOME", HotkeyCommand::SetCamera),
        ("SHIFT RETURN", HotkeyCommand::ShowChatWindow(2)),
        ("CONTROL RETURN", HotkeyCommand::ShowChatWindow(3)),
    ] {
        assert_eq!(
            a.flow.hotkeys.binding(&chord_codes(chord)),
            Some(&cmd),
            "{chord}"
        );
    }
    let dialogs = a.dialogs();
    let with_input = trace(&mut a, 160, |r, i| match i {
        10 => r.key("ESCAPE"),
        20 => r.key("CONTROL P"),
        30 => r.key("PAUSE"),
        40 => r.key("F1"),
        50 => r.key("F4"),
        60 => r.key("HOME"),
        70 => {
            window_key(r, K::ArrowLeft, true);
            window_key(r, K::ArrowUp, true);
        }
        90 => {
            window_key(r, K::ArrowLeft, false);
            window_key(r, K::ArrowUp, false);
        }
        100 => {
            move_to(r, 800.0, 400.0);
            window(r, InputEvent::Wheel { up: true });
            window(r, InputEvent::Wheel { up: false });
        }
        110 => button(r, MouseButton::Middle, true),
        111 => move_to(r, 700.0, 300.0),
        120 => button(r, MouseButton::Middle, false),
        130 => r.key("SHIFT RETURN"),
        140 => r.key("CONTROL RETURN"),
        _ => {}
    });
    let without = trace(&mut b, 160, |_, _| {});
    assert_eq!(with_input, without, "checksum sequence as without input");
    assert!(with_input.last().unwrap().0 > with_input[0].0, "ticks run");
    assert_untouched(&a, "game keys");
    assert_eq!(a.dialogs(), dialogs, "no game menu, no chat window");
    assert!(!a.files.dir.exists(), "no savegame");
    assert!(!a.flow.take_hardcopy());
    let gate = a.view.input_gate();
    assert!(gate.menu_level && !gate.player_camera(), "{gate:?}");
}

#[test]
fn quicksave_is_refused_in_the_menu_level() {
    // re/menu-level ML.6 (`0x0042F4A0`), O-443: The menu level cannot be saved. F1 does not reach it, the quicksave
    // path needs a match in `MenuApp`, and `{0x0B}` the match of the flow.
    let Some(mut r) = started() else {
        return;
    };
    for _ in 0..3 {
        r.key("F1");
        frames(&mut r, 5);
    }
    r.flow.quick_save(&mut r.host.app);
    frames(&mut r, 5);
    r.apply(UiEffect::SaveGame {
        name: "menu-level".into(),
    });
    frames(&mut r, 5);
    assert!(!r.files.dir.exists(), "no savegame");
    assert_eq!(r.flow.quicksave_counter, 0, "counter untouched");
    assert_untouched(&r, "F1");
}

#[test]
fn focus_loss_does_not_pause_the_menu_level() {
    // re/menu-level ML.6, O-316: `GameEngine_Pause` (`0x0042D5E0`) does not switch at game type -2, not even on losing
    // focus (`0x004C9A4B`); the world keeps running.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 5);
    assert!(!r.view.focus_lost(), "no pause");
    assert_eq!(r.view.pause(), PauseState::default());
    assert!(frames(&mut r, 20) > 0, "ticks keep running");
}

// QUIRK O-442: kept
#[test]
fn speed_keys_do_not_reach_the_menu_level() {
    // re/menu-level ML.15 no. 5 (B): `+`/`−` change neither the game speed nor `GameSpeed` (slider, `Engine.ini`) in
    // the original.
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 5);
    let speed = |r: &Run| r.view.sim().unwrap().game_speed;
    let speeds = (speed(&r), r.host.app.config().game_speed);
    for chord in ["+", "+", "+", "-", "-", "-"] {
        r.key(chord);
        frames(&mut r, 3);
    }
    assert_eq!((speed(&r), r.host.app.config().game_speed), speeds);
    assert_untouched(&r, "speed keys");
}

// QUIRK O-442: kept
#[test]
fn shift_d_does_not_reach_the_menu_level() {
    // re/menu-level ML.15 no. 5 (B): `SHIFT D` opens no `DIPLOMATIE`, the world keeps running.
    let Some(mut r) = started() else {
        return;
    };
    let dialogs = r.dialogs();
    r.key("SHIFT D");
    assert!(frames(&mut r, 20) > 0, "world runs");
    assert_eq!(r.dialogs(), dialogs, "no diplomacy");
    assert_untouched(&r, "SHIFT D");
}

// QUIRK O-442: kept
#[test]
fn group_keys_do_not_reach_the_menu_level() {
    // The bindings from `bindings2.ini` do not reach the world in the key path, like `keys.ini` (the speed keys have no
    // effect there, O-442); without clicks to the world the selection would stay empty anyway.
    let Some(mut r) = started() else {
        return;
    };
    for chord in ["CONTROL 1", "1", "SHIFT 2", "1"] {
        r.key(chord);
        frames(&mut r, 3);
    }
    assert_untouched(&r, "group keys");
}

// QUIRK O-442: kept
#[test]
fn snapshot_requests_a_hardcopy_in_the_menu_level() {
    // re/menu-level ML.15 no. 5 (B): The Print key works (`ScreenShots/screenshot0000.jpg` with menu and world); the
    // reimplementation files the request as in a match (`KD_HARDCOPY`), `opennml-game` writes the image.
    let Some(mut r) = started() else {
        return;
    };
    r.key("SNAPSHOT");
    r.frame();
    assert!(r.flow.take_hardcopy());
    assert!(!r.flow.take_hardcopy(), "once per request");
    r.host.on_key(
        winit::keyboard::KeyCode::PrintScreen,
        false,
        None,
        S1024,
        r.now,
    );
    r.frame();
    assert!(r.flow.take_hardcopy(), "Print key only released");
    assert_untouched(&r, "Print");
}

/// Left click, selection box and double-click to the right of the START frame, each with movement over the world.
fn left_clicks_beside_the_menu(r: &mut Run, i: usize) {
    match i {
        5 => move_to(r, 800.0, 400.0),
        6 => button(r, MouseButton::Left, true),
        7 => button(r, MouseButton::Left, false),
        8 => {
            button(r, MouseButton::Left, true);
            button(r, MouseButton::Left, false);
        }
        20 => button(r, MouseButton::Left, true),
        21 => move_to(r, 950.0, 600.0),
        30 => button(r, MouseButton::Left, false),
        _ => {}
    }
}

// QUIRK O-442: kept
#[test]
fn click_beside_the_menu_does_not_reach_the_menu_level() {
    // re/menu-level ML.15 no. 5, O-442: A left click and a selection box beside the START frame show nothing in the
    // original. `GameWorld_RenderSelectionMarkers` `0x0047A490` draws selection rings and health bars even in
    // cinemascope; had the world taken the clicks, there would be rings in the photos. The world gets no click.
    let (Some(mut a), Some(mut b)) = (started(), started()) else {
        return;
    };
    let with_clicks = trace(&mut a, 40, left_clicks_beside_the_menu);
    assert_eq!(with_clicks, trace(&mut b, 40, |_, _| {}));
    assert_untouched(&a, "left click");
    assert!(!a.view.input_gate().world_click());
}

// QUIRK O-442: kept
#[test]
fn right_click_does_not_reach_the_menu_level() {
    // re/menu-level ML.15 no. 5, O-442: A right-click beside the START frame has no effect (like the left click).
    let Some(mut r) = started() else {
        return;
    };
    move_to(&mut r, 800.0, 400.0);
    button(&mut r, MouseButton::Right, true);
    r.frame();
    move_to(&mut r, 820.0, 420.0);
    button(&mut r, MouseButton::Right, false);
    frames(&mut r, 3);
    assert_untouched(&r, "right-click");
    assert!(!r.view.input_gate().right_click_command());
}

// QUIRK O-442: kept
#[test]
fn minimap_click_does_not_reach_the_menu_level() {
    // re/menu-level ML.15 no. 5 (B): A click at the bottom right on the hidden minimap (935/679, 870/620) does not move
    // the camera.
    let Some(mut r) = started() else {
        return;
    };
    for (x, y) in [(935.0, 679.0), (870.0, 620.0)] {
        move_to(&mut r, x, y);
        button(&mut r, MouseButton::Left, true);
        button(&mut r, MouseButton::Left, false);
        frames(&mut r, 3);
    }
    assert_untouched(&r, "minimap");
    assert!(!r.view.input_gate().minimap_input());
}

// QUIRK O-442: kept
#[test]
fn hover_keeps_the_normal_pointer_in_the_menu_level() {
    // re/menu-level ML.15 no. 5 (B): Mouse over units and buildings: the arrow stays (pointer 7/0), no target pointer.
    let Some(mut r) = started() else {
        return;
    };
    for (x, y) in [(600.0, 300.0), (800.0, 400.0), (950.0, 650.0)] {
        move_to(&mut r, x, y);
        r.frame();
        assert_eq!(
            r.flow.pointer_choice(&r.host.app, &r.view),
            PointerChoice::NORMAL
        );
    }
    assert_untouched(&r, "pointer");
}

// QUIRK O-443: deviation
#[test]
fn local_result_halts_the_menu_level() {
    // O-443: A result of the local slot (script `0x56`, no trigger in the shipped data) stops the menu level and goes
    // into the log; there is no `GAMEEND_*` (that would need a match in `MenuApp`).
    crate::testutil::capture_warnings();
    let Some(mut r) = started() else {
        return;
    };
    frames(&mut r, 10);
    let dialogs = r.dialogs();
    let local = usize::from(r.view.sim().unwrap().active_player);
    r.view.sim_mut().unwrap().state.players[local].result = DEFEAT;
    let steps = r.view.steps();
    assert_eq!(frames(&mut r, 50), 0, "no more ticks");
    assert_eq!(r.view.steps(), steps);
    assert_eq!(r.dialogs(), dialogs, "no end screen");
    assert!(r.host.app.game().is_none());
    assert_eq!(r.flow.menu_level(), Some((LEVEL, true)), "stays visible");
    let warnings = crate::testutil::take_warnings();
    assert_eq!(
        warnings
            .iter()
            .filter(|w| w.contains("Menu level") && w.contains("result"))
            .count(),
        1,
        "once into the log: {warnings:?}"
    );
}

#[test]
fn menu_level_data_follows_the_flow() {
    // `ui_state.menu_level` - `None` before the build, afterwards visible with ticks and checksum, invisible in
    // `SINGLEPLAYERMENU` with ticks standing; the query does not change the checksum.
    let Some(mut r) = menu_level_run() else {
        return;
    };
    assert_eq!(
        r.flow.menu_level_data(r.view.sim()),
        None,
        "before the build"
    );
    r.frame();
    assert_eq!(r.flow.menu_level_data(r.view.sim()), None, "START built");
    r.idle();
    frames(&mut r, 50);
    let data = r
        .flow
        .menu_level_data(r.view.sim())
        .expect("menu level running");
    assert_eq!(data.level, LEVEL);
    assert!(data.shown);
    assert!(data.steps > 0, "ticks run");
    assert_eq!(data.steps, r.view.steps());
    assert_eq!(data.checksum.len(), 16);
    assert_eq!(
        data.checksum,
        opennml_control::hex_checksum(r.view.sim().unwrap().checksum())
    );
    assert_eq!(
        r.flow.menu_level_data(r.view.sim()),
        Some(data.clone()),
        "query changes nothing"
    );
    assert_eq!(r.flow.menu_level_data(None), None, "without simulation");
    hide_via_singleplayer(&mut r);
    let hidden = r.flow.menu_level_data(r.view.sim()).unwrap();
    assert!(!hidden.shown);
    assert!(hidden.steps >= data.steps);
    frames(&mut r, 50);
    assert_eq!(
        r.flow.menu_level_data(r.view.sim()),
        Some(hidden),
        "stopped: ticks and checksum stand"
    );
    r.flow.end_menu_level(&mut r.host.app, &mut r.view);
    assert_eq!(r.flow.menu_level_data(r.view.sim()), None, "after the end");
}
