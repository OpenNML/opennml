//! Tests of game end and accounting: `MenuApp::show_game_end` (`FUN_0042F9A0` and step 4 of
//! `GameEngine_SetPlayerResult` `0x00430CE0`, re/ingame-menus 6.2), the end screens `GAMEEND_VICTORY`/`GAMEEND_DEFEAT`
//! (`FUN_004D9030`, re/ingame-menus 6.3), surrendering from the game menu (`QUITGAME`, `0x27`), the command `0x0E`,
//! the frame `ACCOUNTING` (`FUN_004D93F0`, re/accounting 5) and its tabs `ACCOUNT0...2` (`FUN_004CF8C0`,
//! re/accounting 4.3). The logic alone runs through [`Logic`], live through `MenuApp` with the corpus dialogs
//! (these need `NML_GAME_DIR`).

use super::*;
use crate::dialog::DialogLibrary;
use crate::screens::AccountingView;

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
const VICTORY: &str = "gameend_victory";
const DEFEAT: &str = "gameend_defeat";
const DISABLE: i32 = 0x2000_0000;
const HIDE: i32 = 0x1000_0000;

/// `MenuApp` without dialogs (only queue and state).
fn bare() -> MenuApp {
    MenuApp::from_library(
        DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    )
}

fn queued(app: &MenuApp) -> Vec<AppCmd> {
    app.queue().iter().cloned().collect()
}

/// Corpus `MenuApp` with a running match `game` (game menu closed, state 0).
fn running(game: GameInfo) -> Option<Driver<Corpus>> {
    let mut d = live_with(
        EngineConfig::default(),
        MenuOptions::default(),
        Box::new(dispatch),
    )?;
    d.app.set_game(Some(game));
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

/// Accounting with `n` rows: name "P<z>", color index `z`, value `100 * z + s` in column `s`.
fn view(n: usize) -> AccountingView {
    AccountingView {
        rows: (0..n)
            .map(|z| {
                let values = std::array::from_fn(|s| (100 * z + s).to_string());
                (format!("P{z}"), z as i32, values)
            })
            .collect(),
        time_text: "[CENTER]Game time: 1:05".into(),
    }
}

fn text_of(msg: &ElementMsg) -> Option<String> {
    match msg.params.get("text") {
        Some(Param::Text(t)) => Some(t.clone()),
        Some(Param::Shared(t)) => Some(t.get()),
        _ => None,
    }
}

#[test]
fn game_end_closes_open_menus() {
    // `FUN_0042F9A0`: state 1 -> `FUN_0042E290` (`{4, game menu}`, speed from the settings), `{0x34}`, state 0, pointer
    // shown; then state 7 (defeat) or 6 (victory) and `{6,"GAMEEND_..."}` (re/ingame-menus 6.2 steps 3-4).
    let mut app = bare();
    app.set_game(Some(FREE));
    app.show_game_end(false);
    assert_eq!(app.game_menu_state(), 7);
    assert_eq!(
        queued(&app),
        [AppCmd::CloseAll, open(DEFEAT, false)],
        "state 0: without closing the game menu"
    );
    assert_eq!(app.take_effects(), [UiEffect::GameEnd { victory: false }]);

    let mut app = bare();
    app.set_game(Some(FREE));
    assert!(app.open_game_menu());
    app.take_effects();
    let before = queued(&app);
    app.show_game_end(true);
    assert_eq!(app.game_menu_state(), 6);
    assert!(!app.pointer_hidden());
    let mut expected = before;
    expected.extend([close(MENU3, false), AppCmd::CloseAll, open(VICTORY, false)]);
    assert_eq!(queued(&app), expected);
    assert_eq!(
        app.take_effects(),
        [
            UiEffect::ResumeGame {
                game_speed: Some(14)
            },
            UiEffect::GameEnd { victory: true }
        ]
    );

    // Live: options or a confirmation open - afterwards only the end screen, the game is stopped (6/7).
    let Some(mut d) = running(FREE) else {
        return;
    };
    assert!(d.app.open_game_menu());
    d.idle();
    d.click(MENU3, "options");
    d.idle();
    assert_eq!(d.app.menu().open_names().len(), 2, "options open");
    d.app.show_game_end(true);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [VICTORY]);
    assert_eq!(d.app.game_menu_state(), 6);
    // The effect arrives in the report with the next frame (`show_game_end` runs outside a frame).
    let report = d.app.report(S1280, d.now);
    assert!(
        report
            .effects
            .iter()
            .any(|(_, e)| *e == UiEffect::GameEnd { victory: true })
    );

    let Some(mut d) = running(FREE) else {
        return;
    };
    assert!(d.app.open_game_menu());
    d.idle();
    d.click(MENU3, "quit");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    d.app.show_game_end(false);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [DEFEAT]);
    assert_eq!(d.app.game_menu_state(), 7);
    // ESC has no effect in the end screen (no `CHECKESCAPE`), the game menu does not open.
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [DEFEAT]);
    assert!(!d.app.open_game_menu());
}

#[test]
fn end_with_message_closes_menus_and_ends_the_match_on_ok() {
    // End of a network match with a message instead of an end screen. Like `FUN_0042F9A0` first close the game menu
    // (`{4, game menu}`, speed), `{0x34}`, state 0, pointer shown; then `MSGBOX` with the free text, whose "OK" queues
    // `{0x0E}` - the game end that the host leads to the main menu via the accounting on the network (directly before
    // `Go`).
    let text =
        "The connection to the host was aborted. The match was saved as \"netz_2026_11_06_1807\".";
    let mut app = bare();
    app.set_game(Some(FREE));
    assert!(app.open_game_menu());
    app.take_effects();
    let before = queued(&app);
    app.end_with_message(text.into());
    assert_eq!(app.game_menu_state(), 0);
    assert!(!app.pointer_hidden());
    let mut expected = before;
    expected.extend([close(MENU3, false), AppCmd::CloseAll, open("msgbox", true)]);
    assert_eq!(queued(&app), expected);
    assert_eq!(app.screen_state().info_free.as_deref(), Some(text));
    assert_eq!(app.screen_state().info_ok, Some(AppCmd::EndMatch));
    assert_eq!(
        app.take_effects(),
        [UiEffect::ResumeGame {
            game_speed: Some(14)
        }],
        "no end screen"
    );
    // Main menu after an end before `Go` (without accounting): `{5,"start"}`.
    let before = queued(&app);
    app.open_main_menu();
    let mut expected = before;
    expected.push(open("start", true));
    assert_eq!(queued(&app), expected);

    // Live: the message shows the text, "OK" closes it and queues `{0x0E}`.
    let Some(mut d) = running(FREE) else {
        return;
    };
    assert!(d.app.open_game_menu());
    d.idle();
    d.app.end_with_message(text.into());
    d.idle();
    assert!(d.find("MSGBOX").is_some());
    assert!(d.find("INGAMEOPTIONS3").is_none());
    let report = d.app.report(S1280, d.now);
    assert_eq!(
        element(&report, "msgbox", "textinfo").text.as_deref(),
        Some(text)
    );
    d.app.take_effects();
    d.click("msgbox", "ok");
    d.idle();
    assert!(d.find("MSGBOX").is_none());
    assert!(
        d.app.take_effects().contains(&UiEffect::EndMatch),
        "OK ends the match"
    );
}

// QUIRK O-179: kept
#[test]
fn free_game_disables_load_and_restart() {
    // O-179 (`0x004D931B`...`0x004D935C`): code 5 for `load`/`restart` -> `DISABLE` if `Engine+0x108 != -1`; in the
    // campaign no `status`. Other elements at code 5: 1 without `status`.
    let mut l = Logic::new();
    for (game, status) in [(FREE, Some(DISABLE)), (CAMPAIGN, None)] {
        l.data.game = Some(game);
        for dialog in [VICTORY, DEFEAT] {
            for button in ["load", "restart"] {
                let (r, msg) = l.send(dialog, button, code::QUERY_STATUS);
                assert_eq!(
                    (r, int_of(&msg, "status")),
                    (1, status),
                    "{game:?} {dialog}/{button}"
                );
            }
            let (r, msg) = l.send(dialog, "end_defeat", code::QUERY_STATUS);
            assert_eq!((r, int_of(&msg, "status")), (1, None));
            assert_eq!(l.send(dialog, "end_defeat", code::QUERY_TEXT).0, -1);
        }
    }

    let Some(mut d) = running(FREE) else {
        return;
    };
    d.app.show_game_end(false);
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(!element(&report, DEFEAT, "load").enabled);
    assert!(!element(&report, DEFEAT, "restart").enabled);
    assert!(element(&report, DEFEAT, "end_defeat").enabled);
    let Some(mut d) = running(FREE) else {
        return;
    };
    d.app.show_game_end(true);
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(!element(&report, VICTORY, "restart").enabled);
    assert!(element(&report, VICTORY, "end_victory").enabled);
}

// QUIRK E-319: kept
#[test]
fn net_end_screen_greys_load_and_restart() {
    // E-319 (`FUN_004D9030` `0x004D919F`/`0x004D91B9`): On the network (host as well as client) `LOAD`/`RESTART` are
    // visible but locked (game type != -1); there is no "Continue" (`CONTINUE_*` are missing in both `.dlg`); "End
    // current game" stays free.
    let net = |role| GameInfo {
        game_type: 0,
        local_slot: 1,
        ui_locked: false,
        net: Some(role),
        playback: false,
    };
    let mut l = Logic::new();
    for role in [NetRoleView::Host, NetRoleView::Client] {
        l.data.game = Some(net(role));
        for dialog in [VICTORY, DEFEAT] {
            for button in ["load", "restart"] {
                let (r, msg) = l.send(dialog, button, code::QUERY_STATUS);
                assert_eq!(
                    (r, int_of(&msg, "status")),
                    (1, Some(DISABLE)),
                    "{role:?} {dialog}/{button}"
                );
            }
        }
    }

    for (role, victory) in [(NetRoleView::Client, false), (NetRoleView::Host, true)] {
        let Some(mut d) = running(net(role)) else {
            return;
        };
        d.app.show_game_end(victory);
        d.idle();
        let (dialog, end, buttons): (_, _, &[&str]) = if victory {
            (VICTORY, "end_victory", &["restart"])
        } else {
            (DEFEAT, "end_defeat", &["load", "restart"])
        };
        let report = d.app.report(S1280, d.now);
        for button in buttons {
            let e = element(&report, dialog, button);
            assert!(e.visible && !e.hidden, "{role:?} {dialog}/{button} visible");
            assert!(!e.enabled, "{role:?} {dialog}/{button} locked");
        }
        assert!(element(&report, dialog, end).enabled);
        assert!(
            !report
                .elements
                .iter()
                .any(|e| e.dialog == dialog && e.name.starts_with("continue")),
            "no continue"
        );
    }
}

// QUIRK E-273: kept
#[test]
fn end_button_and_end_message_end_once() {
    // The end with a message carries its own `0x0E`. (a) If the message is pending (`info_ok` = `{0x0E}`), `END_*`
    // stays without effect - otherwise `{0x15}` would wait forever behind the open message. (b) If the chain of `END_*`
    // is already queued, `end_with_message` takes its `{0x0E}` out.
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    l.state.info_ok = Some(AppCmd::EndMatch);
    for (dialog, button) in [(VICTORY, "end_victory"), (DEFEAT, "end_defeat")] {
        assert_eq!(l.click(dialog, button), 1);
        assert_eq!(l.take_queue(), [], "{dialog}: no effect");
    }
    // Other remembered commands do not block.
    l.state.info_ok = Some(AppCmd::Quit);
    assert_eq!(l.click(DEFEAT, "end_defeat"), 1);
    assert_eq!(
        l.take_queue(),
        [close(DEFEAT, false), AppCmd::WaitDialogs, AppCmd::EndMatch]
    );

    let mut app = bare();
    app.set_game(Some(FREE));
    app.show_game_end(false);
    for cmd in [close(DEFEAT, false), AppCmd::WaitDialogs, AppCmd::EndMatch] {
        app.push_command(cmd);
    }
    app.end_with_message("End".into());
    assert_eq!(
        queued(&app),
        [
            AppCmd::CloseAll,
            open(DEFEAT, false),
            close(DEFEAT, false),
            AppCmd::WaitDialogs,
            AppCmd::CloseAll,
            open("msgbox", true)
        ]
    );
    assert_eq!(app.screen_state().info_ok, Some(AppCmd::EndMatch));
}

// QUIRK E-273: kept
#[test]
fn end_message_blocks_the_end_screen_and_ends_with_the_match() {
    // The message for the end of the network match is pending (`info_ok` = `{0x0E}`): `show_game_end` opens no end
    // screen - the message carries the end, its "OK" leads to the accounting. The flag does not outlive the match:
    // `set_game(None)` clears it, in the next match end screen and `END_*` work again.
    let mut app = bare();
    app.set_game(Some(FREE));
    app.end_with_message("End".into());
    let before = queued(&app);
    app.show_game_end(false);
    assert_eq!(queued(&app), before, "no effect");
    assert_eq!(app.game_menu_state(), 0);
    assert_eq!(app.screen_state().info_ok, Some(AppCmd::EndMatch));
    app.set_game(None);
    assert_eq!(app.screen_state().info_ok, None, "flag gone with the match");
    app.set_game(Some(FREE));
    app.show_game_end(false);
    assert_eq!(app.game_menu_state(), 7);
    assert!(
        queued(&app).ends_with(&[AppCmd::CloseAll, open(DEFEAT, false)]),
        "{:?}",
        queued(&app)
    );
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    l.state = app.screen_state().clone();
    assert_eq!(l.click(DEFEAT, "end_defeat"), 1);
    assert_eq!(
        l.take_queue(),
        [close(DEFEAT, false), AppCmd::WaitDialogs, AppCmd::EndMatch]
    );
}

// QUIRK O-208: deviation
#[test]
fn defeat_load_returns_to_the_defeat_screen_on_cancel() {
    // `load` (`0x004D91E5`...`0x004D9232`): `{3, frame}`, `{5,"ingameloadgame"}`, return command `{5, frame}`; the
    // state stays 7. `INGAMELOADGAME CANCEL` queues the return command: the defeat screen comes back. If loading
    // fails, `reopen_menu` opens the end screen in state 6/7 instead of the game menu.
    let mut l = Logic::new();
    l.data.game = Some(CAMPAIGN);
    assert_eq!(l.click(DEFEAT, "load"), 1);
    assert_eq!(
        l.take_queue(),
        [close(DEFEAT, true), open("ingameloadgame", true)]
    );
    assert_eq!(l.state.return_cmd, Some(open(DEFEAT, true)));
    assert!(l.effects.is_empty());

    let Some(mut d) = running(CAMPAIGN) else {
        return;
    };
    d.app.show_game_end(false);
    d.idle();
    d.click(DEFEAT, "load");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["ingameloadgame"]);
    assert_eq!(d.app.game_menu_state(), 7);
    d.click("ingameloadgame", "cancel");
    d.idle();
    assert_eq!(d.app.menu().open_names(), [DEFEAT]);
    assert_eq!(d.app.game_menu_state(), 7);
    assert!(d.app.queue().is_empty());

    // Failed loading (the match stays, the state is 6/7) -> the end screen, not the game menu.
    for (victory, name, state) in [(false, DEFEAT, 7), (true, VICTORY, 6)] {
        let mut app = bare();
        app.set_game(Some(CAMPAIGN));
        app.show_game_end(victory);
        let before = queued(&app);
        app.reopen_menu();
        assert_eq!(app.game_menu_state(), state);
        let mut expected = before;
        expected.push(open(name, false));
        assert_eq!(queued(&app), expected);
    }
    // State 0/1 stays with the game menu.
    let mut app = bare();
    app.set_game(Some(CAMPAIGN));
    assert!(app.open_game_menu());
    let before = queued(&app);
    app.reopen_menu();
    let mut expected = before;
    expected.push(open("ingameoptions", false));
    assert_eq!(queued(&app), expected);
}

// QUIRK O-30, O-147: kept
#[test]
fn campaign_restart_waits_then_reloads() {
    // `restart` (`0x004D9255`...`0x004D9294`): `{4, frame}`, `{0x15}`, `{0x1F}` - in the campaign, without a
    // confirmation; applies to both end screens.
    let mut l = Logic::new();
    l.data.game = Some(CAMPAIGN);
    for dialog in [VICTORY, DEFEAT] {
        assert_eq!(l.click(dialog, "restart"), 1);
        assert_eq!(
            l.take_queue(),
            [
                close(dialog, false),
                AppCmd::WaitDialogs,
                AppCmd::ReloadLevel
            ],
            "{dialog}"
        );
    }
    assert!(l.effects.is_empty());

    let Some(mut d) = running(CAMPAIGN) else {
        return;
    };
    d.app.show_game_end(false);
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(element(&report, DEFEAT, "load").enabled);
    assert!(element(&report, DEFEAT, "restart").enabled);
    d.app.take_effects();
    d.click(DEFEAT, "restart");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.take_effects(), [click_sfx(), UiEffect::ReloadLevel]);
}

#[test]
fn end_button_queues_close_wait_and_0x0e() {
    // `end_victory`/`end_defeat` (`0x004D9091`...`0x004D9174`): `{4, frame}`, `{0x15}`, `{0x0E}`.
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    for (dialog, button) in [(VICTORY, "end_victory"), (DEFEAT, "end_defeat")] {
        assert_eq!(l.click(dialog, button), 1);
        assert_eq!(
            l.take_queue(),
            [close(dialog, false), AppCmd::WaitDialogs, AppCmd::EndMatch]
        );
    }
    assert!(l.effects.is_empty());
    // The dead buttons `continue_*` (O-25) and other codes: not responsible.
    assert_eq!(l.click(VICTORY, "continue_victory"), -1);
    assert_eq!(l.send(VICTORY, "end_victory", code::QUERY_VALUE).0, -1);

    // `0x0E` goes to the host as an effect.
    let mut d = Driver::with_app(
        crate::dialog::tests::fixture(),
        bare(),
        crate::app::tests::no_answer(),
    );
    d.push(AppCmd::EndMatch);
    d.frame();
    assert_eq!(d.app.take_effects(), [UiEffect::EndMatch]);

    let Some(mut d) = running(FREE) else {
        return;
    };
    d.app.show_game_end(true);
    d.idle();
    d.app.take_effects();
    d.click(VICTORY, "end_victory");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.take_effects(), [click_sfx(), UiEffect::EndMatch]);
}

#[test]
fn quitgame_surrenders_and_shows_defeat() {
    // `quitgame` (`0x004D7248`...`0x004D72E8`): the original does not answer code 5; code 8 `{3, frame}`, confirmation
    // 15780 with OK `{0x27, 2, slot of the local player, 1}` and cancel `{5, frame}`.
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    for dialog in [MENU3, "ingameoptions"] {
        let (r, msg) = l.send(dialog, "quitgame", code::QUERY_STATUS);
        assert_eq!((r, int_of(&msg, "status")), (-1, None));
        assert_eq!(l.click(dialog, "quitgame"), 1);
        assert_eq!(
            l.take_queue(),
            [close(dialog, true), open("infoframe", true)]
        );
        assert_eq!(l.state.info_text, Some(15780));
        assert_eq!(l.state.info_ok, Some(AppCmd::Surrender { slot: 2 }));
        assert_eq!(l.state.info_cancel, Some(open(dialog, true)));
    }

    // `0x27` with a match: effect `Surrender` to the host (which carries out the defeat immediately); without a match
    // nothing.
    let mut d = Driver::with_app(
        crate::dialog::tests::fixture(),
        bare(),
        crate::app::tests::no_answer(),
    );
    d.push(AppCmd::Surrender { slot: 2 });
    d.frame();
    assert!(d.app.take_effects().is_empty(), "no match running");
    d.app.set_game(Some(FREE));
    d.push(AppCmd::Surrender { slot: 2 });
    d.frame();
    assert_eq!(d.app.take_effects(), [UiEffect::Surrender { slot: 2 }]);

    // Live: confirmation, OK queues `{0x27}`; the host reports the defeat with `show_game_end`.
    let Some(mut d) = running(FREE) else {
        return;
    };
    assert!(d.app.open_game_menu());
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(element(&report, MENU3, "quitgame").enabled);
    d.click(MENU3, "quitgame");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    assert_eq!(d.app.game_menu_state(), 1);
    d.app.take_effects();
    d.key(RETURN);
    d.idle();
    assert_eq!(d.app.take_effects(), [UiEffect::Surrender { slot: 2 }]);
    d.app.show_game_end(false);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [DEFEAT]);
    assert_eq!(d.app.game_menu_state(), 7);
}

// QUIRK O-166: kept
#[test]
fn accounting_opens_on_military_tab_each_time() {
    // O-166 (`0x004D9749`...`0x004D978B`): code 5 of the frame sets the flag to `account1` and opens it; another tab
    // button closes the remembered one (`{3}`) and opens the new one (`{5}`), the same one does nothing.
    let mut l = Logic::new();
    assert_eq!(l.send("accounting", "accounting", code::QUERY_STATUS).0, 1);
    assert_eq!(l.take_queue(), [open("account1", true)]);
    assert_eq!(l.state.accounting_tab, "account1");
    assert_eq!(l.click("accounting", "account2"), 1);
    assert_eq!(
        l.take_queue(),
        [close("account1", true), open("account2", true)]
    );
    assert_eq!(l.state.accounting_tab, "account2");
    assert_eq!(l.click("accounting", "account2"), 1);
    assert!(l.take_queue().is_empty(), "same tab");
    assert_eq!(l.send("accounting", "accounting", code::QUERY_STATUS).0, 1);
    assert_eq!(l.take_queue(), [open("account1", true)]);
    assert_eq!(l.state.accounting_tab, "account1");
    // `mainmenu` code 7 (text "start Gamespy" only without a read `Engine.ini`) is omitted (O-26).
    assert_eq!(l.send("accounting", "mainmenu", code::QUERY_TEXT).0, -1);

    // Live: `set_accounting` queues `{6,"accounting"}`; every opening begins with `account1`.
    let Some(mut d) = running(FREE) else {
        return;
    };
    d.app.set_game(None);
    d.app.set_accounting(view(2));
    assert_eq!(queued(&d.app), [open("accounting", false)]);
    d.idle();
    let mut names = d.app.menu().open_names().to_vec();
    names.sort();
    assert_eq!(names, ["account1", "accounting"]);
    d.click("accounting", "account0");
    d.idle();
    let mut names = d.app.menu().open_names().to_vec();
    names.sort();
    assert_eq!(names, ["account0", "accounting"]);
    // Back to the main menu (`start` opens its `logo` too) and into the accounting again: `account1` again.
    d.click("accounting", "mainmenu");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["logo", "start"]);
    d.push(close("start", true));
    d.app.set_accounting(view(1));
    d.idle();
    let mut names = d.app.menu().open_names().to_vec();
    names.sort();
    assert_eq!(names, ["account1", "accounting"]);
}

// QUIRK O-167, O-202: kept
#[test]
fn continue_shown_only_in_campaign() {
    // O-167 corrected (`0x004D97AD`...`0x004D97EC`, `FUN_004BA5C0` = start parameter `GameApp+0xD650 -> +0x1F4`, in
    // free play the game type 0...6, -1 only in the campaign): "Continue" (`quit`) is visible (`status` 0) only with
    // -1, otherwise `HIDE` - also in free single player.
    let mut l = Logic::new();
    for (game_type, status) in [(-1, 0), (0, HIDE), (4, HIDE)] {
        l.data.draft.settings.game_type = game_type;
        let (r, msg) = l.send("accounting", "quit", code::QUERY_STATUS);
        assert_eq!(
            (r, int_of(&msg, "status")),
            (1, Some(status)),
            "{game_type}"
        );
    }
    assert_eq!(l.send("accounting", "mainmenu", code::QUERY_STATUS).0, -1);

    // Code 8 (`0x004D9506`...): `{3,"accounting"}`, `{4, flag}`, then (without automatic continue) campaign ->
    // `selectcampaign`, free play without a profile -> `start`, with a profile -> `singleplayermenu` (O-202:
    // unreachable in free play, the button is hidden).
    l.state.accounting_tab = "account1".into();
    for (game_type, profiles, target) in [
        (-1, ProfilesFile::default(), "selectcampaign"),
        (0, ProfilesFile::default(), "start"),
        (0, profiles_with(&["Anna"], 0), "singleplayermenu"),
    ] {
        l.data.draft.settings.game_type = game_type;
        l.data.profiles = Profiles::new(profiles);
        assert_eq!(l.click("accounting", "quit"), 1);
        assert_eq!(
            l.take_queue(),
            [
                close("accounting", true),
                close("account1", false),
                open(target, true)
            ],
            "{game_type}"
        );
    }

    let Some(mut d) = running(FREE) else {
        return;
    };
    d.app.set_game(None);
    d.app.set_accounting(view(1));
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert!(element(&report, "accounting", "quit").hidden);
    assert!(!element(&report, "accounting", "mainmenu").hidden);
}

#[test]
fn mainmenu_returns_to_start() {
    // `mainmenu` code 8 (`0x004D94BD`...`0x004D94F7`): `EngineConfig+0x0C` is always 1 after the start (O-26) ->
    // `{3,"accounting"}`, `{4, flag}`, `{5,"start"}`.
    let mut l = Logic::new();
    l.state.accounting_tab = "account2".into();
    assert_eq!(l.click("accounting", "mainmenu"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("accounting", true),
            close("account2", false),
            open("start", true)
        ]
    );
    assert!(l.effects.is_empty());
}

// QUIRK O-163: kept
#[test]
fn hidden_rows_beyond_active_count() {
    // `FUN_004CF8C0`: `name<z>`/`value<c><z>` code 5 -> `HIDE` from `z >= rows`, otherwise 1 without `status`; code 7
    // only for `z < rows`: name with the color from `0x0061F744[color index]`, value of the column `c + 4*k` (`k` =
    // digit of the tab); `spielzeit` code 7 = game time text. Other elements and codes: -1.
    let mut l = Logic::new();
    l.data.accounting = Some(view(3));
    for page in ["account0", "account1", "account2"] {
        for z in 0..8 {
            let (r, msg) = l.send(page, &format!("name{z}"), code::QUERY_STATUS);
            let hidden = (z >= 3).then_some(HIDE);
            assert_eq!((r, int_of(&msg, "status")), (1, hidden), "{page} name{z}");
            let (r, msg) = l.send(page, &format!("value1{z}"), code::QUERY_STATUS);
            assert_eq!((r, int_of(&msg, "status")), (1, hidden), "{page} value1{z}");
        }
        let (r, msg) = l.send(page, "name5", code::QUERY_TEXT);
        assert_eq!((r, text_of(&msg)), (-1, None));
        let (r, msg) = l.send(page, "spielzeit", code::QUERY_TEXT);
        assert_eq!(
            (r, text_of(&msg).as_deref()),
            (1, Some("[CENTER]Game time: 1:05"))
        );
        assert_eq!(l.send(page, "spielzeit", code::QUERY_STATUS).0, -1);
        assert_eq!(l.send(page, "labelname", code::QUERY_TEXT).0, -1);
    }
    // Name and color (table like `[SETPLAYERCOLOR n]`, gray from index 8).
    let (r, msg) = l.send("account1", "name2", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(text_of(&msg).as_deref(), Some("P2"));
    assert_eq!(int_of(&msg, "color"), Some(0xFF17_A710_u32 as i32));
    // Columns: page 0 -> 0...2, page 1 -> 4...7, page 2 -> 8...11.
    for (page, c, column) in [("account0", 2, 2), ("account1", 0, 4), ("account2", 3, 11)] {
        let (r, msg) = l.send(page, &format!("value{c}1"), code::QUERY_TEXT);
        assert_eq!(r, 1);
        assert_eq!(text_of(&msg), Some((100 + column).to_string()), "{page}");
    }
    // Color index 8 and more: gray.
    l.data.accounting.as_mut().unwrap().rows[0].1 = 8;
    let (_, msg) = l.send("account1", "name0", code::QUERY_TEXT);
    assert_eq!(int_of(&msg, "color"), Some(0xFFAC_ACAC_u32 as i32));
    // Without accounting: no rows, game time empty.
    l.data.accounting = None;
    let (_, msg) = l.send("account1", "name0", code::QUERY_STATUS);
    assert_eq!(int_of(&msg, "status"), Some(HIDE));
    let (_, msg) = l.send("account1", "spielzeit", code::QUERY_TEXT);
    assert_eq!(text_of(&msg).as_deref(), Some(""));

    // Live: three rows visible, the others hidden.
    let Some(mut d) = running(FREE) else {
        return;
    };
    d.app.set_game(None);
    d.app.set_accounting(view(3));
    d.idle();
    let report = d.app.report(S1280, d.now);
    assert_eq!(
        element(&report, "account1", "name1").text.as_deref(),
        Some("P1")
    );
    assert!(!element(&report, "account1", "name2").hidden);
    assert!(element(&report, "account1", "name3").hidden);
    assert!(element(&report, "account1", "value03").hidden);
    assert_eq!(
        element(&report, "account1", "value32").text.as_deref(),
        Some("207")
    );
    assert_eq!(
        element(&report, "account1", "spielzeit").text.as_deref(),
        Some("[CENTER]Game time: 1:05")
    );
}

#[test]
fn a_running_game_sets_the_session_game_type() {
    // Open point: after loading a savegame the original sets the game type of the session to that of the loaded engine
    // (`Game_LoadLevelFromSavegame` `0x004C7860`: `[GameApp+0xD650]+0x1F4 = Engine+0x108`); at start (`0x0D`, `0x0C`)
    // and restart both already agree. After a campaign (-1) and loading a free savegame, "Continue" of the accounting
    // is hidden again. The game end (`None`) changes nothing.
    let mut app = bare();
    app.data_mut().draft.settings.game_type = -1;
    app.set_game(Some(FREE));
    assert_eq!(app.data().draft.settings.game_type, 0);
    app.set_game(None);
    assert_eq!(app.data().draft.settings.game_type, 0);
    app.set_game(Some(CAMPAIGN));
    assert_eq!(app.data().draft.settings.game_type, -1);
    app.set_game(None);
    assert_eq!(app.data().draft.settings.game_type, -1);
}
