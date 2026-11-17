//! Tests of the diplomacy dialog: `DIPLOMATIE` (`FUN_004D0750`) per row of the table re/game-ui D5.7, opening and
//! closing (`FUN_0042E060`) via `MenuApp::toggle_diplomacy` and "Back"/ESC. The logic alone runs through [`Logic`],
//! live through `MenuApp` with the corpus dialogs (these need `NML_GAME_DIR`).

use super::*;
use crate::dialog::DialogLibrary;
use crate::screens::diplomacy::{COLOR_TEXT, NATION_TEXT};

const D: &str = "diplomatie";
const HIDE: i32 = 0x1000_0000;
const DISABLE: i32 = 0x2000_0000;
/// Free play, local player in slot 0.
const FREE: GameInfo = GameInfo {
    game_type: 0,
    local_slot: 0,
    ui_locked: false,
    net: None,
    playback: false,
};

/// State with local slot 0: slots 0-3 occupied (3 enemy, 1 friend, 2 neutral), 4 eliminated (1), 5-7
/// unoccupied (3).
fn view() -> DiplomacyView {
    let rows = std::array::from_fn(|slot| {
        let s = slot as i32;
        DiplomacyRow {
            slot: slot as u8,
            name: format!("Player {slot}"),
            color_text: COLOR_TEXT + s,
            nation_text: NATION_TEXT + s % 6,
            result: match slot {
                0..=3 => 0,
                4 => 1,
                _ => 3,
            },
            status: match slot {
                0 | 1 => 0,
                3 => 1,
                _ => 2,
            },
            view: slot == 0,
            chat: slot == 0,
            tribute: slot == 0 || slot == 2,
        }
    });
    DiplomacyView {
        local: 0,
        rows,
        locked_teams: false,
    }
}

fn logic(view: DiplomacyView) -> Logic {
    let mut l = Logic::new();
    l.data.game = Some(FREE);
    l.data.diplomacy = Some(view);
    l
}

fn status(l: &mut Logic, element: &str) -> i32 {
    let (r, msg) = l.send(D, element, code::QUERY_STATUS);
    assert_eq!(r, 1, "{element}");
    int_of(&msg, "status").unwrap_or_else(|| panic!("{element}: no status"))
}

fn effects(l: &mut Logic) -> Vec<UiEffect> {
    std::mem::take(&mut l.effects)
}

fn act(a: DiplomacyAction) -> UiEffect {
    UiEffect::Diplomacy(a)
}

/// re/game-ui D5.7: the own row shows only name, color and nation; slots with result != 0 (eliminated, unoccupied)
/// are completely hidden; every row element requeries every 500 ms.
#[test]
fn rows_hide_self_and_finished_slots() {
    let mut l = logic(view());
    for col in ["name", "color", "nation"] {
        assert_eq!(status(&mut l, &format!("{col}0")), 0, "{col}0");
        assert_eq!(status(&mut l, &format!("{col}3")), 0, "{col}3");
        assert_eq!(status(&mut l, &format!("{col}4")), HIDE, "{col}4");
        assert_eq!(status(&mut l, &format!("{col}7")), HIDE, "{col}7");
    }
    for col in ["chat", "status", "view", "tribut"] {
        assert_eq!(status(&mut l, &format!("{col}0")), HIDE, "{col}0 own row");
        assert_eq!(status(&mut l, &format!("{col}2")), 0, "{col}2");
        assert_eq!(
            status(&mut l, &format!("{col}4")),
            HIDE,
            "{col}4 eliminated"
        );
        assert_eq!(
            status(&mut l, &format!("{col}6")),
            HIDE,
            "{col}6 unoccupied"
        );
    }
    for col in [
        "chat", "name", "color", "nation", "status", "view", "tribut",
    ] {
        let (r, msg) = l.send(D, &format!("{col}5"), code::QUERY_TIMEOUT);
        assert_eq!((r, int_of(&msg, "timeout")), (1, Some(500)), "{col}");
    }
    // Texts: name, color 470 + color, nation 15120 + nation.
    let text = |l: &mut Logic, e: &str| match l.send(D, e, code::QUERY_TEXT).1.params.get("text") {
        Some(Param::Text(t)) => t.clone(),
        other => panic!("{e}: {other:?}"),
    };
    assert_eq!(text(&mut l, "name3"), "Player 3");
    let expected = l.texts(&[COLOR_TEXT + 3, NATION_TEXT + 3]);
    assert_eq!(text(&mut l, "color3"), expected[0]);
    assert_eq!(text(&mut l, "nation3"), expected[1]);
    // Values: sight, masks, status.
    assert_eq!(l.query(D, "tribut2", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(l.query(D, "tribut3", code::QUERY_VALUE), (1, Some(0)));
    assert_eq!(l.query(D, "status3", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(l.query(D, "status2", code::QUERY_VALUE), (1, Some(2)));
    // Slots outside 0-7 do not exist (`GameEngine_GetPlayer` null): -1; the frame itself likewise.
    assert_eq!(l.send(D, "chat8", code::QUERY_STATUS).0, -1);
    assert_eq!(l.send(D, D, code::QUERY_STATUS).0, -1);
    // Without a match or without the host's state the handler does not answer (`FUN_004B9140() == 0`).
    l.data.diplomacy = None;
    assert_eq!(l.send(D, "name0", code::QUERY_STATUS).0, -1);
    let mut none = logic(view());
    none.data.game = None;
    assert_eq!(none.send(D, "name0", code::QUERY_STATUS).0, -1);
}

/// O-323: With fixed teams (`Engine+0x20`) the status selection is locked, the other columns are not.
#[test]
fn status_locked_with_fixed_teams() {
    let mut l = logic(DiplomacyView {
        locked_teams: true,
        ..view()
    });
    assert_eq!(status(&mut l, "status1"), DISABLE);
    assert_eq!(status(&mut l, "status0"), HIDE, "own row stays hidden");
    assert_eq!(status(&mut l, "status4"), HIDE);
    assert_eq!(status(&mut l, "view1"), 0);
    assert_eq!(status(&mut l, "chat1"), 0);
}

/// Not selectable through the control channel (list entries without names), so proven here: list 15720-15722,
/// choosing "Allied" (0) in row 3 gives `Status { 3, 0 }`; the copy shows the new status immediately (the ComboBox
/// requeries right after the choice).
#[test]
fn status_choice_d2_sends_the_relation() {
    let mut l = logic(view());
    let expected = l.texts(&[15720, 15721, 15722]);
    assert_eq!(l.items(D, "status3"), expected);
    assert_eq!(l.send_int(D, "status3", code::ITEM_SELECTED, 0).0, 1);
    assert_eq!(
        effects(&mut l),
        [act(DiplomacyAction::Status { slot: 3, mode: 0 })]
    );
    assert_eq!(l.query(D, "status3", code::QUERY_VALUE), (1, Some(0)));
    // Without `int` 0 and nothing.
    assert_eq!(l.send(D, "status3", code::ITEM_SELECTED).0, 0);
    assert!(effects(&mut l).is_empty());
}

/// Check marks and buttons: sight, chat and tribute bits locally, chat by relation, tribute 100, "Back".
#[test]
fn hooks_and_buttons_give_their_actions() {
    let mut l = logic(view());
    assert_eq!(l.send_int(D, "view3", code::VALUE_CHANGED, 1).0, 1);
    assert_eq!(l.send_int(D, "chat2", code::VALUE_CHANGED, 1).0, 1);
    assert_eq!(l.send_int(D, "tribut2", code::VALUE_CHANGED, 0).0, 1);
    assert_eq!(
        effects(&mut l),
        [
            act(DiplomacyAction::View { slot: 3, on: true }),
            act(DiplomacyAction::Chat { slot: 2, on: true }),
            act(DiplomacyAction::Tribute { slot: 2, on: false }),
        ]
    );
    assert_eq!(l.query(D, "view3", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(l.query(D, "chat2", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(l.query(D, "tribut2", code::QUERY_VALUE), (1, Some(0)));
    // Buttons only with code 8; the chat buttons answer 1 for every code, the tribute buttons -1 (`0x004D1159`).
    for (button, relation) in [
        ("chatall", -1),
        ("chatfriend", 0),
        ("chatenemy", 1),
        ("chatneutral", 2),
    ] {
        assert_eq!(l.send(D, button, code::QUERY_STATUS).0, 1);
        assert!(effects(&mut l).is_empty());
        assert_eq!(l.click(D, button), 1);
        assert_eq!(effects(&mut l), [act(DiplomacyAction::ChatBy { relation })]);
    }
    for (button, resource) in [("tributgold", 0), ("tributwood", 1), ("tributfood", 2)] {
        assert_eq!(l.send(D, button, code::QUERY_STATUS).0, -1);
        assert_eq!(l.click(D, button), -1);
        assert_eq!(
            effects(&mut l),
            [act(DiplomacyAction::SendTribute { resource })]
        );
    }
    // "Back" in state 10: state 0, `{4,"DIPLOMATIE"}`, report `Close`.
    l.game_menu = 10;
    assert_eq!(l.click(D, "back"), 1);
    assert_eq!(l.game_menu, 0);
    assert_eq!(l.take_queue(), [close(D, false)]);
    assert_eq!(effects(&mut l), [act(DiplomacyAction::Close)]);
    // In state != 10 it does nothing.
    assert_eq!(l.click(D, "back"), 1);
    assert!(l.take_queue().is_empty());
    assert!(effects(&mut l).is_empty());
}

/// `FUN_0042E060` (UI part): without a match nothing; 0 -> 10 with `{6,"DIPLOMATIE"}`, 10 -> 0 with
/// `{4,"DIPLOMATIE"}`; in state 1 (game menu) nothing; under the lock it does not open and does not close by toggling
/// either, "Back" (`open = false`) closes.
#[test]
fn toggle_diplomacy_follows_0042e060() {
    let mut app = MenuApp::from_library(
        DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    );
    let queued = |app: &MenuApp| -> Vec<AppCmd> { app.queue().iter().cloned().collect() };
    assert!(!app.toggle_diplomacy(true), "no match running");
    assert!(queued(&app).is_empty());
    app.set_game(Some(FREE));
    assert!(app.toggle_diplomacy(true));
    assert_eq!(app.game_menu_state(), 10);
    assert_eq!(queued(&app), [open(D, false)]);
    assert!(!app.open_game_menu(), "state 10: no game menu");
    assert!(app.toggle_diplomacy(true), "toggle");
    assert_eq!(app.game_menu_state(), 0);
    assert_eq!(queued(&app), [open(D, false), close(D, false)]);
    assert!(!app.toggle_diplomacy(false), "closed stays closed");
    // Game menu open (state 1): nothing.
    assert!(app.open_game_menu());
    assert!(!app.toggle_diplomacy(true));
    assert_eq!(app.game_menu_state(), 1);
    // Lock: do not open; do not toggle when open, but "Back" closes.
    app.set_game(Some(FREE));
    app.set_ui_locked(true);
    assert!(!app.toggle_diplomacy(true));
    assert_eq!(app.game_menu_state(), 0);
    app.set_ui_locked(false);
    assert!(app.toggle_diplomacy(true));
    app.set_ui_locked(true);
    assert!(!app.toggle_diplomacy(true));
    assert_eq!(app.game_menu_state(), 10);
    assert!(app.toggle_diplomacy(false));
    assert_eq!(app.game_menu_state(), 0);
    // Game end with an open dialog: close first, then the end screen.
    app.set_ui_locked(false);
    assert!(app.toggle_diplomacy(true));
    let before = queued(&app).len();
    app.show_game_end(true);
    assert_eq!(app.game_menu_state(), 6);
    assert_eq!(
        queued(&app)[before..],
        [
            close(D, false),
            AppCmd::CloseAll,
            open("gameend_victory", false)
        ]
    );
}

/// Live (corpus): the dialog opens, "Back" by click and ESC (`MENUBUTTON_CHECKESCAPE`) close via
/// `FUN_0042E060`.
#[test]
fn back_and_escape_close_the_dialog() {
    let Some(mut d) = live_with(
        EngineConfig::default(),
        MenuOptions::default(),
        Box::new(dispatch),
    ) else {
        return;
    };
    d.app.set_game(Some(FREE));
    d.app.set_diplomacy(Some(view()));
    for close_by_escape in [false, true] {
        assert!(d.app.toggle_diplomacy(true));
        d.idle();
        assert_eq!(d.app.menu().open_names(), [D]);
        assert_eq!(d.app.game_menu_state(), 10);
        // The own row and the slots without a player show no check marks.
        let report = d.app.report(S1280, d.now);
        let shown = |name: &str| {
            report
                .elements
                .iter()
                .any(|e| e.dialog == D && e.name == name && !e.hidden)
        };
        assert!(shown("name0") && shown("chat1") && !shown("chat0") && !shown("chat5"));
        if close_by_escape {
            d.key(ESCAPE);
        } else {
            d.click(D, "back");
        }
        d.idle();
        assert_eq!(d.app.game_menu_state(), 0, "ESC {close_by_escape}");
        assert!(d.app.menu().open_names().is_empty());
        assert!(
            d.app
                .take_effects()
                .contains(&UiEffect::Diplomacy(DiplomacyAction::Close))
        );
    }
}
