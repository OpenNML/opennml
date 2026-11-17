//! Tests of the game menu: `INGAMEOPTIONS`/`INGAMEOPTIONS3` (`FUN_004D7110`) per row of the table re/ingame-menus 2
//! with the return command 2.1, opening and closing (`FUN_0042E150`, `FUN_0042E290`, `FUN_0042F6B0`,
//! re/ingame-menus 1) and the helpers for the game host (`set_game`, `reopen_menu`, `take_untaken_keys`). The logic
//! alone runs through [`Logic`], live through `MenuApp` with the corpus dialogs (these need `NML_GAME_DIR`).

use super::*;
use crate::assets::Texts;
use crate::dialog::DialogLibrary;

/// Free play (game type 0), local player in slot 2.
const FREE: GameInfo = GameInfo {
    game_type: 0,
    local_slot: 2,
    ui_locked: false,
    net: None,
    playback: false,
};
/// Campaign (game type -1).
const CAMPAIGN: GameInfo = GameInfo {
    game_type: -1,
    local_slot: 0,
    ui_locked: false,
    net: None,
    playback: false,
};
const MENU3: &str = "ingameoptions3";
const DISABLE: i32 = 0x2000_0000;

/// `MenuApp` without dialogs (only queue and state).
fn bare(opts: MenuOptions, config: EngineConfig) -> MenuApp {
    MenuApp::from_library(DialogLibrary::default(), config, opts)
}

fn queued(app: &MenuApp) -> Vec<AppCmd> {
    app.queue().iter().cloned().collect()
}

/// Corpus `MenuApp` with a running match `game` and an open game menu.
fn in_game(config: EngineConfig, game: GameInfo) -> Option<Driver<Corpus>> {
    let mut d = live_with(config, MenuOptions::default(), Box::new(dispatch))?;
    d.app.set_game(Some(game));
    assert!(d.app.open_game_menu());
    d.idle();
    Some(d)
}

fn element<'a>(
    report: &'a crate::app::UiReport,
    dialog: &str,
    name: &str,
) -> &'a crate::app::ReportElement {
    report
        .elements
        .iter()
        .find(|e| e.dialog == dialog && e.name == name)
        .unwrap_or_else(|| panic!("{dialog}/{name} is missing"))
}

fn resume(speed: Option<u8>) -> UiEffect {
    UiEffect::ResumeGame { game_speed: speed }
}

// QUIRK O-143: kept
#[test]
fn open_game_menu_needs_a_game_and_state_0() {
    // `FUN_0042E150`: nothing without an engine or with state != 0; otherwise pointer shown, state 1,
    // `{6,"INGAMEOPTIONS3"}`.
    let opts = MenuOptions {
        start_menu: true,
        ..MenuOptions::default()
    };
    let mut app = bare(opts, EngineConfig::default());
    assert!(
        app.pointer_hidden(),
        "start sequence with intro: pointer off"
    );
    let before = queued(&app);
    assert!(!app.open_game_menu(), "no match running");
    assert_eq!(app.game_menu_state(), 0);
    assert_eq!(queued(&app), before);
    assert!(app.pointer_hidden());

    app.set_game(Some(FREE));
    assert!(app.open_game_menu());
    assert_eq!(app.game_menu_state(), 1);
    assert!(!app.pointer_hidden(), "FUN_004CD6A0(1)");
    let mut expected = before.clone();
    expected.push(open(MENU3, false));
    assert_eq!(queued(&app), expected);
    // State 1: a second opening does nothing.
    assert!(!app.open_game_menu());
    assert_eq!(queued(&app), expected);
}

#[test]
fn campaign_opens_ingameoptions_free_game_ingameoptions3() {
    // `0x0042E1BE`...: game type -1 -> `INGAMEOPTIONS`, every other -> `INGAMEOPTIONS3`.
    for (game, name) in [
        (CAMPAIGN, "ingameoptions"),
        (FREE, MENU3),
        (
            GameInfo {
                game_type: 4,
                local_slot: 0,
                ui_locked: false,
                net: None,
                playback: false,
            },
            MENU3,
        ),
    ] {
        let mut app = bare(MenuOptions::default(), EngineConfig::default());
        app.set_game(Some(game));
        assert!(app.open_game_menu());
        assert_eq!(queued(&app), [open(name, false)], "{game:?}");
    }

    // Live: the buttons of the two versions (re/ingame-menus 2).
    let Some(d) = in_game(EngineConfig::default(), CAMPAIGN) else {
        return;
    };
    assert_eq!(d.app.menu().open_names(), ["ingameoptions"]);
    let report = d.app.report(S1280, d.now);
    for name in ["savegame", "load", "options", "quitgame", "quit", "return"] {
        element(&report, "ingameoptions", name);
    }
    let Some(d) = in_game(EngineConfig::default(), FREE) else {
        return;
    };
    assert_eq!(d.app.menu().open_names(), [MENU3]);
    let report = d.app.report(S1280, d.now);
    for name in ["savegame3", "load", "options", "quitgame", "quit", "return"] {
        element(&report, MENU3, name);
    }
    assert!(
        report.dialogs.iter().any(|r| r.name == MENU3 && r.modal),
        "MENUFRAME_MODAL"
    );
}

// QUIRK O-153: deviation
#[test]
fn return_closes_and_resumes_with_config_speed() {
    // O-153: `RETURN` queues `{0x0F}` (no closing of its own); `FUN_0042E290` in state 1: state 0, `{4, frame}`,
    // `Engine+0x2D30 = EngineConfig.GameSpeed` directly.
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    assert_eq!(l.click(MENU3, "return"), 1);
    assert_eq!(l.take_queue(), [AppCmd::CloseGameMenu]);
    assert!(l.effects.is_empty());

    let config = EngineConfig {
        game_speed: 7,
        ..EngineConfig::default()
    };
    let Some(mut d) = in_game(config, FREE) else {
        return;
    };
    d.app.take_effects();
    d.click(MENU3, "return");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
    assert_eq!(d.app.take_effects(), [click_sfx(), resume(Some(7))]);
    // ESC hits `RETURN` (`CHECKESCAPE`) and acts the same (without click sound); the key counts as taken.
    assert!(d.app.open_game_menu());
    d.idle();
    d.app.take_effects();
    d.key(ESCAPE);
    d.frame();
    assert!(d.app.take_untaken_keys().is_empty());
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(
        d.app.take_effects(),
        [resume(Some(7))],
        "broadcast without click sound"
    );
    // In state 0 `{0x0F}` does nothing.
    d.push(AppCmd::CloseGameMenu);
    d.idle();
    assert!(d.app.take_effects().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
}

// QUIRK O-195: kept
#[test]
fn close_game_menu_without_game_closes_ingameoptions() {
    // O-195: `{0x0F}` without an engine always closes `ingameoptions` (`0x004CA2A8`:
    // `FUN_004E2E80("ingameoptions", 0)`), never `ingameoptions3`; without a match no `ResumeGame`.
    let Some(mut d) = live(false, false) else {
        return;
    };
    for name in ["ingameoptions", MENU3] {
        d.push(open(name, true));
        d.idle();
    }
    d.app.take_effects();
    d.push(AppCmd::CloseGameMenu);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU3]);
    d.push(AppCmd::CloseGameMenu);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU3]);
    assert!(d.app.take_effects().is_empty());
}

// QUIRK O-195: kept
// QUIRK O-153: deviation
#[test]
fn resume_after_load_dialog_needs_state_1() {
    // `FUN_0042F6B0` (`0x004C99CD`: only with an engine): state 1 -> 0 and resume, without speed; otherwise nothing.
    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(AppCmd::ResumeAfterLoadDialog);
    d.idle();
    assert!(d.app.take_effects().is_empty(), "no match running");
    d.app.set_game(Some(FREE));
    d.push(AppCmd::ResumeAfterLoadDialog);
    d.idle();
    assert!(d.app.take_effects().is_empty(), "state 0");
    assert!(d.app.open_game_menu());
    d.idle();
    d.app.take_effects();
    d.push(AppCmd::ResumeAfterLoadDialog);
    d.idle();
    assert_eq!(d.app.game_menu_state(), 0);
    assert_eq!(d.app.take_effects(), [resume(None)]);
    // The frame stays open (in the original `INGAMELOADGAME CANCEL` closes itself beforehand).
    assert_eq!(d.app.menu().open_names(), [MENU3]);
}

#[test]
fn save_and_load_remember_the_return_command() {
    // re/ingame-menus 2, 2.1 (prepares O-146): `savegame*` remembers `{5, frame}`, `{3, frame}`, `{5,"savegame"}`;
    // `load` `{3, frame}`, `{5,"ingameloadgame"}` and remembers `{5, frame}`. The dialogs build the way back
    // (`save_load.rs`).
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    for (dialog, button) in [(MENU3, "savegame3"), ("ingameoptions", "savegame")] {
        assert_eq!(l.click(dialog, button), 1);
        assert_eq!(
            l.take_queue(),
            [close(dialog, true), open("savegame", true)],
            "{dialog}"
        );
        assert_eq!(l.state.return_cmd, Some(open(dialog, true)));
        l.state.return_cmd = None;
    }
    for dialog in [MENU3, "ingameoptions"] {
        assert_eq!(l.click(dialog, "load"), 1);
        assert_eq!(
            l.take_queue(),
            [close(dialog, true), open("ingameloadgame", true)]
        );
        assert_eq!(l.state.return_cmd, Some(open(dialog, true)));
    }
    assert!(l.effects.is_empty());
}

#[test]
fn options_open_gameplay_and_sound_2_and_return_to_menu() {
    // `options`: `{3, frame}`, `{6,"gameplayoptions2"}`, `{6,"soundoptions2"}`, return command `{5, frame}`
    // (`FUN_004CDCC0`); `SOUNDOPTIONS2 RETURN` queues it (`FUN_004CDD20`).
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    l.state.return_cmd = Some(AppCmd::Quit);
    assert_eq!(l.click(MENU3, "options"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(MENU3, true),
            open("gameplayoptions2", false),
            open("soundoptions2", false)
        ]
    );
    assert_eq!(
        l.state.return_cmd,
        Some(open(MENU3, true)),
        "old command free"
    );
    assert_eq!(l.click("soundoptions2", "return"), 0);
    assert_eq!(
        l.take_queue(),
        [
            close("soundoptions2", true),
            close("gameplayoptions2", false),
            open(MENU3, true)
        ]
    );
    assert_eq!(l.state.return_cmd, None);

    let Some(mut d) = in_game(EngineConfig::default(), FREE) else {
        return;
    };
    d.click(MENU3, "options");
    d.idle();
    let mut names = d.app.menu().open_names().to_vec();
    names.sort();
    assert_eq!(names, ["gameplayoptions2", "soundoptions2"]);
    d.click("soundoptions2", "return");
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU3]);
    assert_eq!(d.app.game_menu_state(), 1);
}

#[test]
fn quit_asks_15783_and_reopens_on_cancel() {
    // `quit` (`0x004D71AB`...`0x004D7243`): `{3, frame}`, confirmation 15783 with OK `{0x25}` and cancel `{5, frame}`.
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    assert_eq!(l.click(MENU3, "quit"), 1);
    assert_eq!(
        l.take_queue(),
        [close(MENU3, true), open("infoframe", true)]
    );
    assert_eq!(l.state.info_text, Some(15783));
    assert_eq!(l.state.info_ok, Some(AppCmd::Quit));
    assert_eq!(l.state.info_cancel, Some(open(MENU3, true)));

    let Some(mut d) = in_game(EngineConfig::default(), FREE) else {
        return;
    };
    d.click(MENU3, "quit");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU3]);
    assert_eq!(d.app.game_menu_state(), 1);
    d.click(MENU3, "quit");
    d.idle();
    d.app.take_effects();
    d.key(RETURN);
    d.idle();
    assert_eq!(d.app.take_effects(), [UiEffect::Quit]);
}

// QUIRK O-30, O-147: kept
#[test]
fn campaign_menu_restart_asks_and_reloads() {
    // O-147 (`0x004D753A`...`0x004D75CE`): `restart` queues `{3, frame}`, builds the cancel command `{5, frame}`,
    // queues **again** `{3, frame}` and asks with 15780 (OK `{0x1F}`, cancel `{5, frame}`); no `{0x15}`. The original
    // does not answer code 5 for `restart`.
    let mut l = Logic::new();
    l.data.game = Some(CAMPAIGN);
    let (r, msg) = l.send("ingameoptions", "restart", code::QUERY_STATUS);
    assert_eq!((r, int_of(&msg, "status")), (-1, None));
    assert_eq!(l.click("ingameoptions", "restart"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("ingameoptions", true),
            close("ingameoptions", true),
            open("infoframe", true)
        ]
    );
    assert_eq!(l.state.info_text, Some(15780));
    assert_eq!(l.state.info_ok, Some(AppCmd::ReloadLevel));
    assert_eq!(l.state.info_cancel, Some(open("ingameoptions", true)));
    assert!(l.effects.is_empty());

    let Some(mut d) = in_game(EngineConfig::default(), CAMPAIGN) else {
        return;
    };
    // Cancel (ESC hits the confirmation) -> game menu open again, state 1, nothing restarted.
    d.click("ingameoptions", "restart");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    d.app.take_effects();
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["ingameoptions"]);
    assert_eq!(d.app.game_menu_state(), 1);
    assert!(d.app.take_effects().is_empty());
    // OK -> `0x1F` with a match: effect `ReloadLevel`; the game menu is closed, only the host changes the state.
    d.click("ingameoptions", "restart");
    d.idle();
    d.app.take_effects();
    d.key(RETURN);
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.take_effects(), [UiEffect::ReloadLevel]);
    assert_eq!(d.app.game_menu_state(), 1);
}

#[test]
fn quest_and_tiplog_return_to_the_menu() {
    // `questmenu`/`tiplogmenu` (`0x004D749E`...`0x004D7535`): `{3, frame}`, `{5,"questmenu2"}` or
    // `{5,"tiplogmenu"}`; `QUESTMENU2 OK2` (`FUN_004D1FF0`) and `TIPLOGMENU OK` (`FUN_004DB6F0`) close themselves and
    // always open `ingameoptions`. Without an engine both handlers answer 0.
    let mut l = Logic::new();
    assert_eq!(l.click("questmenu2", "ok2"), 0, "no match running");
    assert_eq!(l.click("tiplogmenu", "ok"), 0, "no match running");
    assert!(l.take_queue().is_empty());
    l.data.game = Some(CAMPAIGN);
    for (button, target) in [("questmenu", "questmenu2"), ("tiplogmenu", "tiplogmenu")] {
        assert_eq!(l.click("ingameoptions", button), 1);
        assert_eq!(
            l.take_queue(),
            [close("ingameoptions", true), open(target, true)]
        );
    }
    for (dialog, button) in [("questmenu2", "ok2"), ("tiplogmenu", "ok")] {
        assert_eq!(l.click(dialog, button), 1, "{dialog}");
        assert_eq!(
            l.take_queue(),
            [close(dialog, true), open("ingameoptions", true)]
        );
    }
    // Content empty: the lists answer with an empty `array`, code 9 with 1, other elements with -1.
    for (dialog, list) in [("questmenu2", "questtext"), ("tiplogmenu", "tiplog")] {
        assert!(l.items(dialog, list).is_empty(), "{dialog}");
        assert_eq!(l.send(dialog, list, code::QUERY_TIMEOUT).0, 1);
        assert_eq!(l.send(dialog, "other", code::TRIGGERED).0, -1);
    }
    assert_eq!(l.send("questmenu2", "questtext", code::QUERY_TEXT).0, 0);
    assert_eq!(l.send("questmenu2", "questmenu2", code::QUERY_STATUS).0, 1);

    // Live: game menu -> mission objectives -> OK2 -> game menu (state stays 1), likewise the message list.
    let Some(mut d) = in_game(EngineConfig::default(), CAMPAIGN) else {
        return;
    };
    for (button, dialog, ok) in [
        ("questmenu", "questmenu2", "ok2"),
        ("tiplogmenu", "tiplogmenu", "ok"),
    ] {
        d.click("ingameoptions", button);
        d.idle();
        assert_eq!(d.app.menu().open_names(), [dialog]);
        assert_eq!(d.app.game_menu_state(), 1);
        d.click(dialog, ok);
        d.idle();
        assert_eq!(d.app.menu().open_names(), ["ingameoptions"]);
        assert_eq!(d.app.game_menu_state(), 1);
    }
}

// QUIRK O-143: kept
#[test]
fn menu_state_stays_1_across_sub_dialogs() {
    // O-143: state `Engine+0x2CCC` stays 1 across options, loading, saving and confirmations.
    let Some(mut d) = in_game(EngineConfig::default(), FREE) else {
        return;
    };
    let mut states = Vec::new();
    d.click(MENU3, "options");
    d.idle();
    states.push(d.app.game_menu_state());
    d.click("soundoptions2", "return");
    d.idle();
    d.click(MENU3, "quit");
    d.idle();
    states.push(d.app.game_menu_state());
    d.key(ESCAPE);
    d.idle();
    states.push(d.app.game_menu_state());
    for (button, dialog) in [("load", "ingameloadgame"), ("savegame3", "savegame")] {
        d.click(MENU3, button);
        d.idle();
        assert_eq!(d.app.menu().open_names(), [dialog]);
        states.push(d.app.game_menu_state());
        let back = d.app.screen_state().return_cmd.clone();
        d.push(close(dialog, true));
        d.push(back.expect("return command"));
        d.idle();
        assert_eq!(d.app.menu().open_names(), [MENU3]);
    }
    assert_eq!(states, [1; 5]);
}

#[test]
fn savegame_disabled_without_game() {
    // Code 5 (`0x004D7643`...`0x004D771E`): `savegame*` `DISABLE` without an engine; `load` never locked without
    // replay; other elements -1.
    let mut l = Logic::new();
    for (game, status) in [(None, Some(DISABLE)), (Some(FREE), None)] {
        l.data.game = game;
        for (dialog, button) in [(MENU3, "savegame3"), ("ingameoptions", "savegame")] {
            let (r, msg) = l.send(dialog, button, code::QUERY_STATUS);
            assert_eq!(
                (r, int_of(&msg, "status")),
                (1, status),
                "{game:?} {button}"
            );
        }
        let (r, msg) = l.send(MENU3, "load", code::QUERY_STATUS);
        assert_eq!((r, int_of(&msg, "status")), (1, None));
        assert_eq!(l.send(MENU3, "options", code::QUERY_STATUS).0, -1);
        assert_eq!(l.send(MENU3, "return", code::QUERY_VALUE).0, -1);
    }

    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(open(MENU3, true));
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(!element(&report, MENU3, "savegame3").enabled);
    assert!(element(&report, MENU3, "load").enabled);
    let Some(d) = in_game(EngineConfig::default(), FREE) else {
        return;
    };
    let report = d.app.report(S1280, d.now);
    assert!(element(&report, MENU3, "savegame3").enabled);
}

#[test]
fn savegame_disabled_under_the_lock() {
    // re/campaign-ui D6.9 (`0x004D7643`...`0x004D771E`, `Engine+0x600`): under the UI lock of the scripts `savegame*`
    // answer with `DISABLE`; `load` stays free. `set_ui_locked` only writes the flag of the match (no command, state
    // stays).
    let mut l = Logic::new();
    for (game, status) in [
        (FREE, None),
        (
            GameInfo {
                ui_locked: true,
                ..FREE
            },
            Some(DISABLE),
        ),
    ] {
        l.data.game = Some(game);
        for (dialog, button) in [(MENU3, "savegame3"), ("ingameoptions", "savegame")] {
            let (r, msg) = l.send(dialog, button, code::QUERY_STATUS);
            assert_eq!(
                (r, int_of(&msg, "status")),
                (1, status),
                "{game:?} {button}"
            );
        }
        let (r, msg) = l.send(MENU3, "load", code::QUERY_STATUS);
        assert_eq!((r, int_of(&msg, "status")), (1, None));
    }

    let mut app = bare(MenuOptions::default(), EngineConfig::default());
    app.set_ui_locked(true);
    assert_eq!(app.game(), None, "no match: nothing");
    app.set_game(Some(FREE));
    assert!(app.open_game_menu());
    let before = queued(&app);
    app.set_ui_locked(true);
    assert!(app.game().unwrap().ui_locked);
    assert_eq!(app.game_menu_state(), 1, "state stays");
    assert_eq!(queued(&app), before, "no command");

    let Some(mut d) = in_game(EngineConfig::default(), FREE) else {
        return;
    };
    d.app.set_ui_locked(true);
    d.click(MENU3, "return");
    d.idle();
    assert!(d.app.open_game_menu());
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(!element(&report, MENU3, "savegame3").enabled);
    assert!(element(&report, MENU3, "load").enabled);
}

// QUIRK E-339: deviation
#[test]
fn ingame_menu_disables_save_and_load_during_playback() {
    // `0x004D7643`...`0x004D771E`, `GameApp+0x19F3`: during a replay `savegame*` and `load` answer with `DISABLE`; the
    // other buttons stay (options, surrender, quit, back).
    let playback = GameInfo {
        playback: true,
        ..FREE
    };
    let mut l = Logic::new();
    l.data.game = Some(playback);
    for (dialog, button) in [
        (MENU3, "savegame3"),
        ("ingameoptions", "savegame"),
        ("ingameoptions2", "savegame2"),
        (MENU3, "load"),
        ("ingameoptions", "load"),
    ] {
        let (r, msg) = l.send(dialog, button, code::QUERY_STATUS);
        assert_eq!(
            (r, int_of(&msg, "status")),
            (1, Some(DISABLE)),
            "{dialog}/{button}"
        );
    }
    assert_eq!(l.send(MENU3, "options", code::QUERY_STATUS).0, -1);
    assert_eq!(l.send(MENU3, "quitgame", code::QUERY_STATUS).0, -1);
    // Without a replay `load` stays free.
    l.data.game = Some(FREE);
    let (r, msg) = l.send(MENU3, "load", code::QUERY_STATUS);
    assert_eq!((r, int_of(&msg, "status")), (1, None));

    let Some(d) = in_game(EngineConfig::default(), playback) else {
        return;
    };
    let report = d.app.report(S1280, d.now);
    assert!(!element(&report, MENU3, "savegame3").enabled);
    assert!(!element(&report, MENU3, "load").enabled);
    assert!(element(&report, MENU3, "quitgame").enabled);
    assert!(element(&report, MENU3, "options").enabled);
}

// QUIRK O-371: kept
#[test]
fn closing_menu_or_load_dialog_hides_the_pointer_only_under_the_lock() {
    // O-371, O-379, re/campaign-ui D6.17: `FUN_0042E290` (`0x0042E2BE`, close game menu) and `FUN_0042F6B0`
    // (`0x0042F6CD`, close load dialog) turn the pointer off only under the lock `Engine+0x600`; without the lock it
    // stays as ESC set it. `set_pointer_hidden` sets it per frame for the host.
    for lock in [false, true] {
        for cmd in [AppCmd::CloseGameMenu, AppCmd::ResumeAfterLoadDialog] {
            let Some(mut d) = in_game(EngineConfig::default(), FREE) else {
                return;
            };
            d.app.set_ui_locked(lock);
            assert!(!d.app.pointer_hidden(), "ESC -> on");
            d.push(cmd.clone());
            d.idle();
            assert_eq!(d.app.game_menu_state(), 0);
            assert_eq!(d.app.pointer_hidden(), lock, "{cmd:?}, lock {lock}");
        }
    }
    let mut app = bare(MenuOptions::default(), EngineConfig::default());
    app.set_pointer_hidden(true);
    assert!(app.pointer_hidden());
    app.set_pointer_hidden(false);
    assert!(!app.pointer_hidden());
}

// QUIRK O-143: kept
#[test]
fn set_game_resets_the_state_and_reopen_menu_follows_the_game() {
    // `set_game` sets the state to 0; `reopen_menu` queues `{6, game menu}` (with a match) or
    // `{5,"singleplayermenu"}` (without), without changing the state.
    let mut app = bare(MenuOptions::default(), EngineConfig::default());
    app.reopen_menu();
    assert_eq!(queued(&app), [open("singleplayermenu", true)]);
    app.set_game(Some(CAMPAIGN));
    assert!(app.open_game_menu());
    app.reopen_menu();
    assert_eq!(app.game_menu_state(), 1);
    app.set_game(Some(FREE));
    assert_eq!(app.game_menu_state(), 0);
    app.reopen_menu();
    assert_eq!(
        queued(&app),
        [
            open("singleplayermenu", true),
            open("ingameoptions", false),
            open("ingameoptions", false),
            open(MENU3, false)
        ]
    );
    assert!(app.open_game_menu());
    app.set_game(None);
    assert_eq!(app.game_menu_state(), 0);
    assert!(!app.open_game_menu());
}

#[test]
fn reload_level_needs_a_game() {
    // `0x1F` only with a loaded level (`GameApp+0xD648`, `0x004C9940`), otherwise without effect.
    let mut d = Driver::with_app(
        crate::dialog::tests::fixture(),
        bare(MenuOptions::default(), EngineConfig::default()),
        crate::app::tests::no_answer(),
    );
    d.push(AppCmd::ReloadLevel);
    d.frame();
    assert!(d.app.take_effects().is_empty());
    d.app.set_game(Some(FREE));
    d.push(AppCmd::ReloadLevel);
    d.frame();
    assert_eq!(d.app.take_effects(), [UiEffect::ReloadLevel]);
}

#[test]
fn untaken_keys_name_the_keys_no_widget_took() {
    // Keys that neither the broadcast nor a widget takes are returned by `take_untaken_keys` with their name
    // (`FUN_0049C340`); the list applies to the last frame.
    let mut d = Driver::with_app(
        crate::dialog::tests::fixture(),
        bare(MenuOptions::default(), EngineConfig::default()),
        crate::app::tests::no_answer(),
    );
    d.key(ESCAPE);
    d.key(0x70);
    d.frame();
    assert_eq!(d.app.take_untaken_keys(), ["ESCAPE", "F1"]);
    d.key(0x73);
    d.frame();
    d.frame();
    assert!(d.app.take_untaken_keys().is_empty(), "only the last frame");
}

/// Symbol texts and objective texts of the tests (`extrasymbols.txt` 2, 11, 12; objective texts 20100...20102).
fn mission_texts() -> Texts {
    Texts::from_entries(&[
        (2, "[DEFAULTCOLOR]"),
        (11, "[COLOR FF88FF88]"),
        (12, "[CR;CENTERXPOS]*[CR]"),
        (20100, "T20100"),
        (20101, "T20101"),
    ])
}

fn quests(list: &[(i32, i32)]) -> MissionInfo {
    let mut m = MissionInfo::default();
    m.main_quests[..list.len()].copy_from_slice(list);
    m
}

/// `QUERY_ITEMS` to `dialog`/`list` with the texts `texts` and the mission `mission`.
fn mission_items(
    texts: &Texts,
    mission: Option<MissionInfo>,
    dialog: &str,
    list: &str,
) -> Vec<String> {
    let mut parts = EnvParts::default();
    parts.data.game = Some(CAMPAIGN);
    parts.data.mission = mission;
    let mut msg = ElementMsg::new(dialog, list, code::QUERY_ITEMS);
    assert_eq!(parts.deliver(texts, &mut msg), 1);
    match msg.params.get("array") {
        Some(Param::Array(items)) => items.clone(),
        other => panic!("{dialog}/{list}: {other:?}"),
    }
}

// QUIRK O-53: kept
#[test]
fn quest_text_follows_d6_9() {
    // `FUN_004334E0` (re/campaign-ui D6.9): bit 4 (6) -> green, bit 1 (3) -> base color, four entries per objective
    // with a separator also after the last one; status 10 (failed) is missing, secondary objectives never (O-53).
    let texts = mission_texts();
    let m = quests(&[(3, 20100), (6, 20101), (10, 20102)]);
    let expected = [
        "[DEFAULTCOLOR]",
        "T20100",
        "[DEFAULTCOLOR]",
        "[CR;CENTERXPOS]*[CR]",
        "[COLOR FF88FF88]",
        "T20101",
        "[DEFAULTCOLOR]",
        "[CR;CENTERXPOS]*[CR]",
    ];
    assert_eq!(quest_items(&m, &texts), expected);
    assert_eq!(
        mission_items(&texts, Some(m), "questmenu2", "questtext"),
        expected,
        "via the handler"
    );
    // Status 1 (without text) is there with an empty text, 0 (empty) is not; slot order, not order of the objectives.
    let m = quests(&[(0, 20100), (1, 0), (0, 0), (6, 20101)]);
    assert_eq!(
        quest_items(&m, &texts),
        [
            "[DEFAULTCOLOR]",
            "",
            "[DEFAULTCOLOR]",
            "[CR;CENTERXPOS]*[CR]",
            "[COLOR FF88FF88]",
            "T20101",
            "[DEFAULTCOLOR]",
            "[CR;CENTERXPOS]*[CR]",
        ]
    );
}

// QUIRK O-365: kept
#[test]
fn quest_text_is_empty_without_quests() {
    // O-365: with 0 objectives the field stays empty, the default text 15034 does not appear - also without mission
    // state.
    let texts = Texts::from_entries(&[(15034, "No mission objective selected!"), (12, "-")]);
    let empty = quests(&[]);
    assert!(quest_items(&empty, &texts).is_empty());
    assert!(mission_items(&texts, Some(empty), "questmenu2", "questtext").is_empty());
    assert!(mission_items(&texts, None, "questmenu2", "questtext").is_empty());
    // Only status 0 and 10: likewise empty.
    assert!(quest_items(&quests(&[(0, 1), (10, 2)]), &texts).is_empty());
}

// QUIRK O-366: kept
#[test]
fn tiplog_lists_old_to_new_with_separators_between() {
    // `FUN_004DB6F0` (re/campaign-ui D6.8, O-366): old -> new, duplicates stay, separators only between the entries, no
    // colors; entries without text drop out.
    let texts = mission_texts();
    let m = MissionInfo {
        info_history: vec![20100, 20101, 20100],
        ..MissionInfo::default()
    };
    let expected = [
        "T20100",
        "[CR;CENTERXPOS]*[CR]",
        "T20101",
        "[CR;CENTERXPOS]*[CR]",
        "T20100",
    ];
    assert_eq!(tiplog_items(&m, &texts), expected);
    assert_eq!(
        mission_items(&texts, Some(m), "tiplogmenu", "tiplog"),
        expected,
        "via the handler"
    );
    let m = MissionInfo {
        info_history: vec![99999, 20101, 99998],
        ..MissionInfo::default()
    };
    assert_eq!(tiplog_items(&m, &texts), ["T20101"]);
    assert!(tiplog_items(&MissionInfo::default(), &texts).is_empty());
}

// QUIRK O-365: kept
#[test]
fn quest_text_asks_for_no_requery() {
    // O-365: code 9 answers 1 without `timeout` - `query_head` reads that as "never requery".
    let mut l = Logic::new();
    l.data.game = Some(CAMPAIGN);
    for (dialog, list) in [("questmenu2", "questtext"), ("tiplogmenu", "tiplog")] {
        let (r, msg) = l.send(dialog, list, code::QUERY_TIMEOUT);
        assert_eq!(r, 1);
        assert!(!msg.params.contains_key("timeout"), "{dialog}");
    }
}

// QUIRK O-365: kept
#[test]
fn quest_text_is_filled_once_at_open() {
    // O-365: The content is created on opening (code 2); if an objective changes while the menu is open, it stays. Only
    // reopening reads the new state. `set_mission` touches neither queue nor menu state.
    let Some(mut d) = in_game(EngineConfig::default(), CAMPAIGN) else {
        return;
    };
    let first = quests(&[(3, 15034)]);
    let second = quests(&[(6, 15034), (3, 15783)]);
    let (a, b) = (
        quest_items(&first, &d.world.texts),
        quest_items(&second, &d.world.texts),
    );
    assert_ne!(a, b);
    d.app.set_mission(Some(first));
    d.click("ingameoptions", "questmenu");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["questmenu2"]);
    // `QUESTTEXT` is a label with `TEXTARRAY`: the text field is in the widget's state (the report does not show it).
    let items = |d: &Driver<Corpus>| {
        let id = d.find("QUESTMENU2QUESTTEXT").expect("questtext");
        match &d.app.tree().node(id).kind {
            NodeKind::Label(s) => s.items.clone(),
            _ => panic!("questtext is not a label"),
        }
    };
    assert_eq!(items(&d), a);
    let state = d.app.game_menu_state();
    d.app.set_mission(Some(second));
    assert!(d.app.queue().is_empty(), "set_mission queues nothing");
    assert_eq!(d.app.game_menu_state(), state);
    for _ in 0..30 {
        d.frame();
    }
    assert_eq!(items(&d), a, "stays while the menu is open");
    d.click("questmenu2", "ok2");
    d.idle();
    d.click("ingameoptions", "questmenu");
    d.idle();
    assert_eq!(items(&d), b, "reopening reads the new state");
}

// QUIRK O-365: kept
#[test]
fn set_mission_keeps_the_game_menu() {
    // `set_mission` touches neither state nor queue (unlike `set_game`, which sets the state to 0).
    let mut app = bare(MenuOptions::default(), EngineConfig::default());
    app.set_game(Some(CAMPAIGN));
    assert!(app.open_game_menu());
    let before = queued(&app);
    app.set_mission(Some(quests(&[(3, 1)])));
    assert_eq!(app.game_menu_state(), 1);
    assert_eq!(queued(&app), before);
    app.set_mission(None);
    assert_eq!(app.game_menu_state(), 1);
    assert_eq!(queued(&app), before);
}

// --- Network game menu `INGAMEOPTIONS2` (re/multiplayer-menus L.7.1) ----------------------------------------------

/// Network match at the client (game type 0), local player in slot 1.
const NET_CLIENT: GameInfo = GameInfo {
    game_type: 0,
    local_slot: 1,
    ui_locked: false,
    net: Some(NetRoleView::Client),
    playback: false,
};
/// Network match at the host.
const NET_HOST: GameInfo = GameInfo {
    net: Some(NetRoleView::Host),
    local_slot: 0,
    ..NET_CLIENT
};
const MENU2: &str = "ingameoptions2";

fn no_resume(effects: &[UiEffect]) -> bool {
    !effects
        .iter()
        .any(|e| matches!(e, UiEffect::ResumeGame { .. }))
}

// QUIRK E-315: kept
#[test]
fn net_menu_is_ingameoptions2_with_five_buttons() {
    // `FUN_0042E150` with a network session: state 2 and `{6,"INGAMEOPTIONS2"}` on every machine (host and client, also
    // with game type -1 not the campaign version); `ingameoptions2.dlg` has exactly five buttons, no load, no restart,
    // no quests/tips.
    for game in [
        NET_CLIENT,
        NET_HOST,
        GameInfo {
            game_type: -1,
            ..NET_CLIENT
        },
    ] {
        let mut app = bare(MenuOptions::default(), EngineConfig::default());
        app.set_game(Some(game));
        assert!(app.open_game_menu());
        assert_eq!(app.game_menu_state(), 2, "{game:?}");
        assert_eq!(queued(&app), [open(MENU2, false)], "{game:?}");
        assert!(!app.open_game_menu(), "state 2: no second opening");
        assert_eq!(queued(&app), [open(MENU2, false)]);
    }

    let Some(d) = in_game(EngineConfig::default(), NET_CLIENT) else {
        return;
    };
    assert_eq!(d.app.menu().open_names(), [MENU2]);
    assert_eq!(d.app.game_menu_state(), 2);
    let report = d.app.report(S1280, d.now);
    let mut buttons: Vec<&str> = report
        .elements
        .iter()
        .filter(|e| e.dialog == MENU2 && e.kind == "button")
        .map(|e| e.name.as_str())
        .collect();
    buttons.sort_unstable();
    assert_eq!(
        buttons,
        ["options2", "quit", "quitgame", "return2", "savegame2"],
        "exactly five buttons"
    );
    for name in ["load", "restart", "questmenu", "tiplogmenu"] {
        assert!(
            !report
                .elements
                .iter()
                .any(|e| e.dialog == MENU2 && e.name == name),
            "no {name}"
        );
    }
    assert!(element(&report, MENU2, "savegame2").enabled);
    assert!(
        report.dialogs.iter().any(|r| r.name == MENU2 && r.modal),
        "MENUFRAME_MODAL"
    );
}

#[test]
fn set_net_role_keeps_the_game_menu() {
    // After the host migration the host only writes the network role (`GameInfo::net`); the open game menu stays
    // (state 2, no command), unlike `set_game`. Without a match nothing.
    let mut app = bare(MenuOptions::default(), EngineConfig::default());
    app.set_net_role(Some(NetRoleView::Host));
    assert_eq!(app.game(), None, "no match: nothing");
    app.set_game(Some(NET_CLIENT));
    assert!(app.open_game_menu());
    let before = queued(&app);
    app.set_net_role(Some(NetRoleView::Host));
    assert_eq!(
        app.game(),
        Some(&GameInfo {
            net: Some(NetRoleView::Host),
            ..NET_CLIENT
        })
    );
    assert_eq!(app.game_menu_state(), 2, "state stays");
    assert_eq!(queued(&app), before, "no command");
}

// QUIRK E-315: kept
#[test]
fn return2_sends_0x13() {
    // `RETURN2` (`0x004D73CD`): `{0x13}` without a closing of its own; `0x13` with an engine `FUN_0042E5D0`: only in
    // state 2 state 0 and `{4,"INGAMEOPTIONS2"}` - **no** speed (E-265, speed only from the host).
    let mut l = Logic::new();
    l.data.game = Some(NET_CLIENT);
    assert_eq!(l.click(MENU2, "return2"), 1);
    assert_eq!(l.take_queue(), [AppCmd::CloseNetGameMenu]);
    assert!(l.effects.is_empty());

    let config = EngineConfig {
        game_speed: 7,
        ..EngineConfig::default()
    };
    let Some(mut d) = in_game(config, NET_CLIENT) else {
        return;
    };
    d.app.take_effects();
    d.click(MENU2, "return2");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
    assert_eq!(d.app.take_effects(), [click_sfx()], "no ResumeGame");
    // In state 0 `{0x13}` does nothing; `{0x0F}` never closes the network menu (only state 1).
    d.push(AppCmd::CloseNetGameMenu);
    d.idle();
    assert!(d.app.take_effects().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
    assert!(d.app.open_game_menu());
    d.idle();
    d.push(AppCmd::CloseGameMenu);
    d.idle();
    assert_eq!(d.app.game_menu_state(), 2);
    assert_eq!(d.app.menu().open_names(), [MENU2]);
    assert!(d.app.take_effects().is_empty());

    // Without an engine (`0x004CA33C`): `FUN_004E2E80("ingameoptions2", 0)` directly, no lobby command.
    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(open(MENU2, true));
    d.idle();
    d.app.take_effects();
    d.push(AppCmd::CloseNetGameMenu);
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert!(d.app.take_effects().is_empty());
}

// QUIRK E-315: kept
#[test]
fn escape_closes_the_net_menu() {
    // `RETURN2` carries `CHECKESCAPE`: ESC acts like the button (without click sound) and counts as taken.
    let Some(mut d) = in_game(EngineConfig::default(), NET_HOST) else {
        return;
    };
    d.app.take_effects();
    d.key(ESCAPE);
    d.frame();
    assert!(d.app.take_untaken_keys().is_empty());
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
    assert!(
        d.app.take_effects().is_empty(),
        "without click sound, without speed"
    );
}

// QUIRK E-315: kept
// QUIRK E-316: deviation
#[test]
fn quitgame_asks_15780_then_0x27() {
    // `QUITGAME` (`0x004D7248`): `{3, frame}`, confirmation 15780, OK `{0x27, 2, slot, 1}`, cancel `{5, frame}`. The
    // state stays 2 until the defeat (E-316): no menu visible, ESC, chat and diplomacy open nothing.
    let mut l = Logic::new();
    l.data.game = Some(NET_CLIENT);
    assert_eq!(l.click(MENU2, "quitgame"), 1);
    assert_eq!(
        l.take_queue(),
        [close(MENU2, true), open("infoframe", true)]
    );
    assert_eq!(l.state.info_text, Some(15780));
    assert_eq!(l.state.info_ok, Some(AppCmd::Surrender { slot: 1 }));
    assert_eq!(l.state.info_cancel, Some(open(MENU2, true)));

    let Some(mut d) = in_game(EngineConfig::default(), NET_CLIENT) else {
        return;
    };
    d.click(MENU2, "quitgame");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU2], "cancel: menu again");
    assert_eq!(d.app.game_menu_state(), 2);
    d.click(MENU2, "quitgame");
    d.idle();
    d.app.take_effects();
    d.key(RETURN);
    d.idle();
    assert_eq!(d.app.take_effects(), [UiEffect::Surrender { slot: 1 }]);
    assert!(d.app.menu().open_names().is_empty(), "no menu visible");
    assert_eq!(d.app.game_menu_state(), 2, "state stays 2");
    assert!(!d.app.open_game_menu(), "ESC opens nothing");
    assert!(!d.app.toggle_diplomacy(true), "no diplomacy");
    assert!(!d.app.show_chat_window(1), "no chat");
    // The own defeat ends the wait: state 7 via the end screen, no speed (E-316).
    d.app.show_game_end(false);
    assert_eq!(d.app.game_menu_state(), 7);
    let effects = d.app.take_effects();
    assert!(no_resume(&effects), "{effects:?}");
}

// QUIRK E-315: kept
#[test]
fn quit_asks_15783_then_0x25() {
    // `QUIT`: `{3, frame}`, confirmation 15783, OK `{0x25}`, cancel `{5, frame}`.
    let mut l = Logic::new();
    l.data.game = Some(NET_HOST);
    assert_eq!(l.click(MENU2, "quit"), 1);
    assert_eq!(
        l.take_queue(),
        [close(MENU2, true), open("infoframe", true)]
    );
    assert_eq!(l.state.info_text, Some(15783));
    assert_eq!(l.state.info_ok, Some(AppCmd::Quit));
    assert_eq!(l.state.info_cancel, Some(open(MENU2, true)));

    let Some(mut d) = in_game(EngineConfig::default(), NET_HOST) else {
        return;
    };
    d.click(MENU2, "quit");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU2]);
    assert_eq!(d.app.game_menu_state(), 2);
    d.click(MENU2, "quit");
    d.idle();
    d.app.take_effects();
    d.key(RETURN);
    d.idle();
    assert_eq!(d.app.take_effects(), [UiEffect::Quit]);
}

// QUIRK E-315: kept
#[test]
fn net_menu_returns_from_options_to_ingameoptions2() {
    // `OPTIONS2`: `{3, frame}`, `{6,"gameplayoptions2"}`, `{6,"soundoptions2"}`, return `{5,"ingameoptions2"}`;
    // `SAVEGAME2`: return `{5,"ingameoptions2"}`, `{3, frame}`, `{5,"savegame"}`. The state stays 2.
    let mut l = Logic::new();
    l.data.game = Some(NET_CLIENT);
    assert_eq!(l.click(MENU2, "options2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(MENU2, true),
            open("gameplayoptions2", false),
            open("soundoptions2", false)
        ]
    );
    assert_eq!(l.state.return_cmd, Some(open(MENU2, true)));
    l.state.return_cmd = None;
    assert_eq!(l.click(MENU2, "savegame2"), 1);
    assert_eq!(l.take_queue(), [close(MENU2, true), open("savegame", true)]);
    assert_eq!(l.state.return_cmd, Some(open(MENU2, true)));
    // `SAVEGAME2` code 5 as in single player: locked under the UI lock.
    let (r, msg) = l.send(MENU2, "savegame2", code::QUERY_STATUS);
    assert_eq!((r, int_of(&msg, "status")), (1, None));
    l.data.game = Some(GameInfo {
        ui_locked: true,
        ..NET_CLIENT
    });
    let (r, msg) = l.send(MENU2, "savegame2", code::QUERY_STATUS);
    assert_eq!((r, int_of(&msg, "status")), (1, Some(DISABLE)));

    let Some(mut d) = in_game(EngineConfig::default(), NET_CLIENT) else {
        return;
    };
    d.click(MENU2, "options2");
    d.idle();
    let mut names = d.app.menu().open_names().to_vec();
    names.sort();
    assert_eq!(names, ["gameplayoptions2", "soundoptions2"]);
    assert_eq!(d.app.game_menu_state(), 2);
    d.click("soundoptions2", "return");
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU2]);
    assert_eq!(d.app.game_menu_state(), 2);
    d.click(MENU2, "savegame2");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["savegame"]);
    assert_eq!(d.app.game_menu_state(), 2);
    let back = d.app.screen_state().return_cmd.clone();
    assert_eq!(back, Some(open(MENU2, true)));
}

// QUIRK E-316: deviation
#[test]
fn net_menu_closes_before_the_end_screen_and_the_message() {
    // `FUN_0042F9A0` -> `FUN_0042E5D0`: state 2 like 1, but without speed - `{4,"ingameoptions2"}` (if open), state 0,
    // then as before (`{0x34}`, end screen or message).
    let Some(mut d) = in_game(EngineConfig::default(), NET_CLIENT) else {
        return;
    };
    d.app.take_effects();
    d.app.show_game_end(true);
    d.idle();
    assert_eq!(d.app.game_menu_state(), 6);
    assert_eq!(d.app.menu().open_names(), ["gameend_victory"]);
    let effects = d.app.take_effects();
    assert!(no_resume(&effects), "{effects:?}");

    let Some(mut d) = in_game(EngineConfig::default(), NET_HOST) else {
        return;
    };
    d.app.take_effects();
    d.app.end_with_message("End".into());
    d.idle();
    assert_eq!(d.app.game_menu_state(), 0, "state 0 during the message");
    assert_eq!(d.app.menu().open_names(), ["msgbox"]);
    let effects = d.app.take_effects();
    assert!(no_resume(&effects), "{effects:?}");
}
