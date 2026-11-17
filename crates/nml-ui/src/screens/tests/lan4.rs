//! Tests of the waiting room `LAN4` of a loaded network match (re/multiplayer-saves S.4.1, handler `FUN_004CF290`):
//! per element the answers of the handler via [`Logic`], the exit with ESC via `MenuApp` with a test frame and with
//! the corpus frame `lan4.dlg` (needs `NML_GAME_DIR`).

use super::*;
use crate::dialog::DialogLibrary;
use crate::draft::{LobbyView, NetMenuView, SlotDraft};
use crate::screens::lan4::{close_lan4, open_lan4};

const LAN4: &str = "lan4";
const HIDE: i32 = 0x1000_0000;
const HUMAN: i32 = 999;
/// "Open" and "Occupied" (re/multiplayer-saves S.4.1, `0x3C3C`/`0x3C3D`).
const OPEN: i32 = 15420;
const TAKEN: i32 = 15421;
/// Color texts 15150 + color (`iStack_14 + 0x3B2E`).
const COLOR: i32 = 15150;

/// Loaded lobby with one row per kind in `kinds` (key = row - 1): color = 2 * row + 1, humans are named after their
/// row "Anna", "Bert", ..., open rows without names; `is_host` says whether this machine hosts.
fn loaded(kinds: &[i32], own_row: u8, is_host: bool) -> Logic {
    const NAMES: [&str; 8] = [
        "Anna", "Bert", "Cleo", "Dora", "Emil", "Finn", "Gina", "Hugo",
    ];
    let mut l = Logic::new();
    l.data.draft.slots = kinds
        .iter()
        .zip(0..)
        .map(|(&kind, row)| SlotDraft {
            key: row - 1,
            name: if kind == HUMAN {
                NAMES[row as usize].into()
            } else {
                String::new()
            },
            kind,
            nation: 0,
            team: row,
            handicap: 0,
            color: 2 * row + 1,
        })
        .collect();
    let mut peers = [false; 8];
    for (row, &kind) in kinds.iter().enumerate() {
        peers[row] = kind == HUMAN;
    }
    l.data.net = Some(NetMenuView {
        games: Vec::new(),
        lobby: Some(LobbyView {
            is_host,
            own_row,
            peers,
            ..LobbyView::default()
        }),
    });
    l
}

impl Logic {
    /// Code 7 to `lan4/element`: return value and text (`None` = no text).
    fn lan4_text(&mut self, element: &str) -> (i32, Option<String>) {
        let (r, msg) = self.send(LAN4, element, code::QUERY_TEXT);
        let text = match msg.params.get("text") {
            Some(Param::Text(t)) => Some(t.clone()),
            None => None,
            other => panic!("{element}: {other:?}"),
        };
        (r, text)
    }

    fn lan4_status(&mut self, element: &str) -> (i32, Option<i32>) {
        let (r, msg) = self.send(LAN4, element, code::QUERY_STATUS);
        (r, int_of(&msg, "status"))
    }

    fn text_of(&self, id: i32) -> String {
        self.texts(&[id]).remove(0)
    }
}

#[test]
fn lan4_rows_show_name_color_and_status() {
    // S.4.1: `name<n>` kind 999 -> name of the slot, otherwise 15420; `color<n>` 15150 + color; `status<n>` 0 -> 15420,
    // 999 -> 15421; code 5 free, code 9 `timeout 1000`. Element `<n>` = row `n` = key `n - 1`.
    let mut l = loaded(&[HUMAN, 0], 0, true);
    assert_eq!(l.lan4_text("name0"), (1, Some("Anna".into())));
    assert_eq!(l.lan4_text("name1"), (1, Some(l.text_of(OPEN))));
    assert_eq!(l.lan4_text("color0"), (1, Some(l.text_of(COLOR + 1))));
    assert_eq!(l.lan4_text("color1"), (1, Some(l.text_of(COLOR + 3))));
    assert_eq!(l.lan4_text("status0"), (1, Some(l.text_of(TAKEN))));
    assert_eq!(l.lan4_text("status1"), (1, Some(l.text_of(OPEN))));
    for element in ["name0", "color0", "status0", "name1", "color1", "status1"] {
        assert_eq!(l.lan4_status(element), (1, Some(0)), "{element}");
        let (r, msg) = l.send(LAN4, element, code::QUERY_TIMEOUT);
        assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)), "{element}");
    }
    // Other kinds (do not occur in the loaded lobby): name 15420, status without text.
    l.data.draft.slots[1].kind = 2;
    assert_eq!(l.lan4_text("name1"), (1, Some(l.text_of(OPEN))));
    assert_eq!(l.lan4_text("status1"), (1, None));
    // The rows change nothing and send nothing.
    assert!(l.take_queue().is_empty());
    assert!(l.effects.is_empty());
}

#[test]
fn missing_rows_are_hidden() {
    // S.4.1 code 5: slot missing (`FUN_004E97E0`) -> `HIDE`; code 7 without a slot -> no text.
    let mut l = loaded(&[HUMAN, HUMAN], 1, false);
    for row in 2..8 {
        for column in ["name", "color", "status"] {
            let element = format!("{column}{row}");
            assert_eq!(l.lan4_status(&element), (1, Some(HIDE)), "{element}");
            assert_eq!(l.lan4_text(&element), (1, None), "{element}");
        }
    }
    // Without a lobby state of the host (no network session) the handler answers no row (like `LAN2`).
    l.data.net = None;
    assert_eq!(l.lan4_status("name0").0, -1);
    assert_eq!(l.lan4_text("status0").0, -1);
}

#[test]
fn start_only_for_the_host_with_every_row_taken() {
    // S.4.1 `start` code 5: only the host (`FUN_004E85F0`) and every row occupied; code 9 `timeout 1000`
    // (`FUN_004CF290`), so that `START` is queried again after every new lobby state.
    let mut l = loaded(&[0, HUMAN, 0], 1, true);
    assert_eq!(l.lan4_status("start"), (1, Some(HIDE)), "two rows open");
    l.data.draft.slots[0].kind = HUMAN;
    assert_eq!(l.lan4_status("start"), (1, Some(HIDE)), "one row open");
    l.data.draft.slots[2].kind = HUMAN;
    assert_eq!(l.lan4_status("start"), (1, Some(0)), "all occupied");
    let (r, msg) = l.send(LAN4, "start", code::QUERY_TIMEOUT);
    assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)));
    // A client never sees `START`, even if every row is occupied.
    let mut c = loaded(&[HUMAN, HUMAN, HUMAN], 2, false);
    assert_eq!(c.lan4_status("start"), (1, Some(HIDE)));
    // Hidden without rows and without a lobby.
    l.data.draft.slots.clear();
    assert_eq!(l.lan4_status("start"), (1, Some(HIDE)));
    c.data.net = None;
    assert_eq!(c.lan4_status("start"), (1, Some(HIDE)));
}

#[test]
fn start_sends_0x14() {
    // S.4.1 `start` code 8 -> `{0x14}` (`FUN_004BCB40`); checking and starting is done by the network session of the
    // host, the handler checks nothing, as in the original.
    assert_eq!(AppCmd::StartSavedGame.code(), 0x14);
    for is_host in [true, false] {
        let mut l = loaded(&[HUMAN, HUMAN], u8::from(!is_host), is_host);
        assert_eq!(l.click(LAN4, "start"), 1);
        assert_eq!(l.take_queue(), [AppCmd::StartSavedGame], "host {is_host}");
        assert!(l.effects.is_empty());
    }
}

#[test]
fn exit_branch_is_dead() {
    // `0x004CF7C0`: `exit` code 5 and 8 return 1 and do nothing (`lan4.dlg` has no such button, E-326).
    let mut l = loaded(&[HUMAN, HUMAN], 0, true);
    assert_eq!(l.lan4_status("exit"), (1, None));
    assert_eq!(l.click(LAN4, "exit"), 1);
    assert!(l.take_queue().is_empty());
    assert!(l.effects.is_empty());
    assert_eq!(l.send(LAN4, "exit", code::QUERY_TIMEOUT).0, -1);
    assert_eq!(l.click(LAN4, "nowhere"), -1);
}

/// Test frames like `lan4.dlg` (without `CHECKESCAPE`), main menu and message for the ESC tests.
const LAN4_DLG: &str = "FRAME LAN4\n FLAGS  /FLAGS\n EXTENT 100 100 600 500\n\
     LABEL NAME0 FLAGS /FLAGS EXTENT 30 60 245 99 /LABEL\n\
     BUTTON START FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 160 340 380 389 /BUTTON\n/FRAME\n";
const START_DLG: &str = "FRAME START\n FLAGS  /FLAGS\n EXTENT 100 100 600 500\n/FRAME\n";
const MSGBOX_DLG: &str = "FRAME MSGBOX\n FLAGS  /FLAGS\n EXTENT 200 200 500 400\n\
     BUTTON OK FLAGS MENUBUTTON_TEXT MENUBUTTON_CHECKENTER /FLAGS EXTENT 10 10 100 50 /BUTTON\n/FRAME\n";

/// `MenuApp` on the fixture with the test frames, without logic.
fn escape_driver() -> Driver<Fixture> {
    let library = DialogLibrary::from_sources(&[
        ("lan4", LAN4_DLG),
        ("start", START_DLG),
        ("msgbox", MSGBOX_DLG),
    ]);
    let app = MenuApp::from_library(library, EngineConfig::default(), MenuOptions::default());
    Driver::with_app(fixture(), app, crate::app::tests::no_answer())
}

/// ESC in the next frame; the effects of this frame.
fn escape(d: &mut Driver<impl crate::app::tests::World>) -> Vec<UiEffect> {
    d.key(ESCAPE);
    d.frame();
    d.app.take_effects()
}

fn leave_lobby() -> UiEffect {
    UiEffect::Net(AppCmd::LeaveLobby)
}

#[test]
fn escape_in_lan4_queues_leave_lobby() {
    // (Deviation from E-326) ESC that no element takes queues `{0x2D}` while `LAN4` is open (like `EXIT` of the lobby)
    // and counts as taken; `MenuApp` passes the command on to the host as `UiEffect::Net`.
    let mut d = escape_driver();
    d.push(open_lan4());
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["lan4"]);
    assert_eq!(escape(&mut d), [leave_lobby()]);
    assert!(d.app.take_untaken_keys().is_empty(), "ESC taken");
    // The host closes `LAN4` (`{4,"lan4"}`, `{9}`, `{5,"start"}`); a second ESC while the closing is pending queues
    // nothing more and stays taken.
    d.push(close_lan4());
    d.push(AppCmd::CloseNet);
    d.push(AppCmd::Open {
        name: "start".into(),
        wait: true,
    });
    let fx = escape(&mut d);
    assert!(!fx.contains(&leave_lobby()), "{fx:?}");
    assert!(d.app.take_untaken_keys().is_empty());
    d.idle();
    assert!(!d.app.menu().open_names().iter().any(|n| n == "lan4"));
    // Without `LAN4`, ESC goes to the host again.
    let fx = escape(&mut d);
    assert!(!fx.contains(&leave_lobby()), "{fx:?}");
    assert_eq!(d.app.take_untaken_keys(), ["ESCAPE"]);

    // If a message lies over `LAN4` (15212 after the host has left; `MSGBOX` does not take ESC, O-103), ESC has no
    // effect: the way out then goes only through "OK".
    let mut d = escape_driver();
    d.push(open_lan4());
    d.idle();
    d.tell(15212, Some(AppCmd::LeaveLobby));
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["lan4", "msgbox"]);
    let fx = escape(&mut d);
    assert!(!fx.contains(&leave_lobby()), "{fx:?}");
    assert_eq!(d.app.take_untaken_keys(), ["ESCAPE"]);

    // Corpus: no element of `lan4.dlg` takes ESC (no `CHECKESCAPE`, no `EXIT`).
    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(open_lan4());
    d.idle();
    for key in ["LAN4NAME0", "LAN4COLOR7", "LAN4STATUS3", "LAN4START"] {
        assert!(d.find(key).is_some(), "{key}");
    }
    assert!(d.find("LAN4EXIT").is_none());
    d.app.take_effects();
    assert_eq!(escape(&mut d), [leave_lobby()]);
    assert!(d.app.take_untaken_keys().is_empty());
}

#[test]
fn escape_elsewhere_is_unchanged() {
    // Without `LAN4`, an untaken ESC stays for the host, also over the main menu.
    let mut d = escape_driver();
    assert_eq!(escape(&mut d), []);
    assert_eq!(d.app.take_untaken_keys(), ["ESCAPE"]);
    d.push(AppCmd::Open {
        name: "start".into(),
        wait: true,
    });
    d.idle();
    d.app.take_effects();
    assert!(escape(&mut d).is_empty());
    assert_eq!(d.app.take_untaken_keys(), ["ESCAPE"]);
}
