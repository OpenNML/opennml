//! Tests of the `LAN2` lobby, the `CHATFRAME` lobby chat and the network lines in `LEVELINFO`: for each element the
//! rules from re/multiplayer-menus L.3 (status rule L.3.1, reading/writing L.3.2, ready L.3.3, start L.3.4) and L.4
//! (chat), plus the network lines of `[CURLEVELINFOS]`. The logic alone runs through [`Logic`], the input and the
//! buffer of the texts through `MenuApp` (the corpus needs `NML_GAME_DIR`).

use super::*;
use crate::assets::Texts;
use crate::draft::{
    DraftSettings, LevelInfoData, LevelNetInfo, LobbyView, MatchDraft, NetMenuView, SlotDraft,
    level_infos,
};
use crate::msg::LobbyAction;
use crate::screens::chat::INPUT_SIZE;
use crate::screens::chatframe::{chat_line, chat_text};

const LAN2: &str = "lan2";
const CHAT: &str = "chatframe";
const HIDE: i32 = 0x1000_0000;
const DISABLE: i32 = 0x2000_0000;
const HUMAN: i32 = 999;

/// Draft with one row per kind in `kinds` (key = row - 1): team = color = row, nation 0, handicap 0; humans are named
/// after their row "Anna", "Bert", ..., the others "EMPTY".
fn lobby_draft(kinds: &[i32]) -> MatchDraft {
    const NAMES: [&str; 8] = [
        "Anna", "Bert", "Cleo", "Dora", "Emil", "Finn", "Gina", "Hugo",
    ];
    MatchDraft {
        map: Some(0),
        info: LevelInfoData {
            name: "Map".into(),
            path: "data/levels/multiplayer/[4Players]-Map.lvl".into(),
            size_class: 1,
            players: i32::try_from(kinds.len()).unwrap(),
            level_type: 0,
            ..LevelInfoData::default()
        },
        slots: kinds
            .iter()
            .zip(0..)
            .map(|(&kind, row)| SlotDraft {
                key: row - 1,
                name: if kind == HUMAN {
                    NAMES[row as usize]
                } else {
                    "EMPTY"
                }
                .into(),
                kind,
                nation: 0,
                team: row,
                handicap: 0,
                color: row,
            })
            .collect(),
        settings: DraftSettings::default(),
    }
}

impl Logic {
    /// Lobby seen from row `own_row` (`is_host` = row 0) over the rows `kinds`; nobody is ready.
    pub(super) fn lobby(kinds: &[i32], own_row: u8) -> Logic {
        let mut l = Logic::new();
        l.data.draft = lobby_draft(kinds);
        let mut peers = [false; 8];
        for (row, &kind) in kinds.iter().enumerate() {
            peers[row] = kind == HUMAN;
        }
        l.data.net = Some(NetMenuView {
            games: Vec::new(),
            lobby: Some(LobbyView {
                is_host: own_row == 0,
                own_row,
                peers,
                ..LobbyView::default()
            }),
        });
        l
    }

    fn view(&mut self) -> &mut LobbyView {
        self.data
            .net
            .as_mut()
            .and_then(|n| n.lobby.as_mut())
            .expect("lobby")
    }

    fn lobby_status(&mut self, element: &str) -> Option<i32> {
        int_of(&self.send(LAN2, element, code::QUERY_STATUS).1, "status")
    }

    fn row(&self, row: i32) -> SlotDraft {
        self.data.draft.slot(row - 1).cloned().expect("row")
    }

    pub(super) fn lobby_effects(&mut self) -> Vec<LobbyAction> {
        std::mem::take(&mut self.effects)
            .into_iter()
            .filter_map(|e| match e {
                UiEffect::Lobby(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    fn chat_key(&mut self, key: i32) -> i32 {
        let mut msg = ElementMsg::new(CHAT, "input", code::EDIT_KEY);
        msg.params.insert("key".into(), Param::Int(key));
        self.deliver(msg).0
    }
}

/// The columns of the status rule `READY`, `RACE`, `TEAM`, `HANDICAP`, `COLOR` and `NAME`.
const COLUMNS: [&str; 6] = ["ready", "race", "team", "handicap", "color", "name"];

// QUIRK E-302: kept
#[test]
fn status_rule_like_the_original() {
    // L.3.1: rows human (host), human (client), computer Normal, open; rows 4 ... 7 are missing. For each view and row,
    // the responses in the order of `COLUMNS` (0 = free).
    let kinds = [HUMAN, HUMAN, 2, 0];
    let host: [[i32; 6]; 5] = [
        // own row; `NAME0` is locked for everyone
        [0, 0, 0, 0, 0, DISABLE],
        // foreign human: locked; `NAME` is free for the host (kick)
        [DISABLE, DISABLE, DISABLE, DISABLE, DISABLE, 0],
        // computer Normal: free, handicap locked
        [0, 0, 0, DISABLE, 0, 0],
        // open: free
        [0, 0, 0, 0, 0, 0],
        // missing: hidden
        [HIDE; 6],
    ];
    let client: [[i32; 6]; 5] = [
        [DISABLE; 6],
        // own row; the client never changes its own name and kind
        [0, 0, 0, 0, 0, DISABLE],
        [DISABLE; 6],
        [DISABLE; 6],
        [HIDE; 6],
    ];
    for (own_row, want) in [(0u8, host), (1, client)] {
        let mut l = Logic::lobby(&kinds, own_row);
        for (row, cols) in want.iter().enumerate() {
            for (col, &status) in COLUMNS.iter().zip(cols) {
                let element = format!("{col}{row}");
                assert_eq!(
                    l.lobby_status(&element),
                    Some(status),
                    "View of row {own_row}, {element}"
                );
                // `timeout 1000` for all row elements.
                let (r, msg) = l.send(LAN2, &element, code::QUERY_TIMEOUT);
                assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)), "{element}");
            }
        }
        // Rows 5 ... 7 hidden.
        assert_eq!(l.lobby_status("color7"), Some(HIDE));
        assert_eq!(l.lobby_status("name7"), Some(HIDE));
    }
    // Without a lobby state of the host, no row responds.
    let mut l = Logic::lobby(&kinds, 0);
    l.data.net = None;
    assert_eq!(l.send(LAN2, "race0", code::QUERY_STATUS).0, -1);
}

#[test]
fn handicap_locked_for_computer_1_to_3() {
    // `HANDICAP` is additionally locked for computer Easy/Normal/Hard; "Manual" (4) is free; a click on a locked
    // handicap changes nothing.
    let mut l = Logic::lobby(&[HUMAN, 1, 2, 3, 4], 0);
    for row in 1..=3 {
        assert_eq!(
            l.lobby_status(&format!("handicap{row}")),
            Some(DISABLE),
            "kind {row}"
        );
    }
    assert_eq!(l.lobby_status("handicap4"), Some(0));
    assert_eq!(l.lobby_status("race1"), Some(0));
    assert_eq!(l.send_int(LAN2, "handicap1", code::ITEM_SELECTED, 3).0, 0);
    assert_eq!(l.row(1).handicap, 0);
    assert!(l.lobby_effects().is_empty());
    assert_eq!(l.send_int(LAN2, "handicap4", code::ITEM_SELECTED, 3).0, 1);
    assert_eq!(l.row(4).handicap, 3);
    assert_eq!(
        l.lobby_effects(),
        [LobbyAction::Row {
            row: 4,
            nation: 0,
            team: 4,
            handicap: 3,
            color: 4
        }]
    );
    // List 15140 ... 15145 (0 % ... 50 %), nations 15120 ... 15125.
    let want = l.texts(&[15140, 15141, 15142, 15143, 15144, 15145]);
    assert_eq!(l.items(LAN2, "handicap4"), want);
    let want = l.texts(&[15120, 15121, 15122, 15123, 15124, 15125]);
    assert_eq!(l.items(LAN2, "race0"), want);
}

#[test]
fn color_list_excludes_all_other_rows() {
    // `FUN_004BD0A0`: colors 0 ... 8 without the colors of all other rows (including open ones); `indexarray` = color.
    let mut l = Logic::lobby(&[HUMAN, HUMAN, 2, 0], 1);
    let (_, msg) = l.send(LAN2, "color1", code::QUERY_ITEMS);
    let free = vec![1, 4, 5, 6, 7, 8];
    assert_eq!(
        msg.params.get("indexarray"),
        Some(&Param::IndexArray(free.clone()))
    );
    let want = l.texts(&free.iter().map(|c| 15150 + c).collect::<Vec<_>>());
    assert_eq!(msg.params.get("array"), Some(&Param::Array(want)));
    // The color writes without `FUN_004BA710` (L.3.2): even a foreign color from an old list stays locally (the host
    // decides the race).
    assert_eq!(l.send_int(LAN2, "color1", code::ITEM_SELECTED, 2).0, 1);
    assert_eq!(l.row(1).color, 2);
    assert_eq!(
        l.lobby_effects(),
        [LobbyAction::Row {
            row: 1,
            nation: 0,
            team: 1,
            handicap: 0,
            color: 2
        }]
    );
}

#[test]
fn client_writes_only_his_row() {
    // The client changes only its own row; a click on a foreign row (host, computer, open) has no effect.
    let mut l = Logic::lobby(&[HUMAN, HUMAN, 2, 0], 1);
    let before = l.data.draft.clone();
    for element in ["race0", "race2", "team3", "color2", "handicap3"] {
        assert_eq!(
            l.send_int(LAN2, element, code::ITEM_SELECTED, 1).0,
            0,
            "{element}"
        );
    }
    for element in ["ready0", "ready2", "ready3"] {
        assert_eq!(
            l.send_int(LAN2, element, code::VALUE_CHANGED, 1).0,
            0,
            "{element}"
        );
    }
    for element in ["name1", "name2", "name3"] {
        assert_eq!(
            l.send_int(LAN2, element, code::ITEM_SELECTED, 0).0,
            0,
            "{element}"
        );
    }
    assert_eq!(l.data.draft, before);
    assert_eq!(l.view().ready, [false; 8]);
    assert!(l.lobby_effects().is_empty());

    // The own row: nation and ready set the local copy and go to the host.
    assert_eq!(l.send_int(LAN2, "race1", code::ITEM_SELECTED, 4).0, 1);
    assert_eq!(l.row(1).nation, 4);
    assert_eq!(l.query(LAN2, "race1", code::QUERY_VALUE), (1, Some(4)));
    assert_eq!(l.send_int(LAN2, "ready1", code::VALUE_CHANGED, 1).0, 1);
    assert!(l.view().ready[1]);
    assert_eq!(l.query(LAN2, "ready1", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(
        l.lobby_effects(),
        [
            LobbyAction::Row {
                row: 1,
                nation: 4,
                team: 1,
                handicap: 0,
                color: 1
            },
            LobbyAction::Ready {
                row: 1,
                ready: true
            }
        ]
    );
    // Team with `FUN_004BA710`: all occupied rows in the same team -> rejected, the row stays; the request is sent
    // anyway (path W), the host rejects it as well.
    l.data.draft.slots[2].team = 0;
    assert_eq!(l.send_int(LAN2, "team1", code::ITEM_SELECTED, 0).0, 1);
    assert_eq!(l.row(1).team, 1);
    assert!(matches!(
        l.lobby_effects().as_slice(),
        [LobbyAction::Row {
            row: 1,
            team: 1,
            ..
        }]
    ));
    // Ready of a missing row -1 (`FUN_004E8DC0`).
    assert_eq!(l.query(LAN2, "ready6", code::QUERY_VALUE), (1, Some(-1)));
}

// QUIRK E-302: kept
#[test]
fn host_sets_computer_ready_and_kicks() {
    let mut l = Logic::lobby(&[HUMAN, HUMAN, 2, 0], 0);
    // Clear the ready checkmark of the computer (blocks the start, L.3.3); not that of a foreign human.
    l.view().ready = [true, true, true, false, false, false, false, false];
    assert_eq!(l.send_int(LAN2, "ready2", code::VALUE_CHANGED, 0).0, 1);
    assert!(!l.view().ready[2]);
    assert_eq!(l.send_int(LAN2, "ready1", code::VALUE_CHANGED, 0).0, 0);
    assert!(l.view().ready[1]);
    assert_eq!(
        l.lobby_effects(),
        [LobbyAction::Ready {
            row: 2,
            ready: false
        }]
    );
    // `NAME` of a human: "Open" and its name (999); "Open" = kick -> kind 0, handicap 0, ready 0,
    // team = row, the name stays.
    assert_eq!(
        l.items(LAN2, "name1"),
        [l.texts(&[15190])[0].clone(), "Bert".into()]
    );
    l.data.draft.slots[1].team = 3;
    l.data.draft.slots[1].handicap = 2;
    assert_eq!(l.send_int(LAN2, "name1", code::ITEM_SELECTED, 0).0, 1);
    let kicked = l.row(1);
    assert_eq!(
        (
            kicked.kind,
            kicked.handicap,
            kicked.team,
            kicked.name.as_str()
        ),
        (0, 0, 1, "Bert")
    );
    assert!(!l.view().ready[1]);
    assert_eq!(l.lobby_effects(), [LobbyAction::Kind { row: 1, kind: 0 }]);
    // The human stays human when its name is chosen (999): no effect.
    let mut l2 = Logic::lobby(&[HUMAN, HUMAN], 0);
    assert_eq!(l2.send_int(LAN2, "name1", code::ITEM_SELECTED, HUMAN).0, 1);
    assert_eq!(l2.row(1).kind, HUMAN);
    assert!(l2.lobby_effects().is_empty());
    // Open row -> computer Easy: handicap 5, ready 1, team of the first other computer (`FUN_004BA980`).
    let want: Vec<String> = l.texts(&[15190, 15191, 15192, 15193, 15194]);
    assert_eq!(l.items(LAN2, "name3"), want);
    assert_eq!(l.send_int(LAN2, "name3", code::ITEM_SELECTED, 1).0, 1);
    let cpu = l.row(3);
    assert_eq!((cpu.kind, cpu.handicap, cpu.team), (1, 5, 2));
    assert!(l.view().ready[3]);
    assert_eq!(l.lobby_effects(), [LobbyAction::Kind { row: 3, kind: 1 }]);
    assert_eq!(l.query(LAN2, "name3", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(l.query(LAN2, "name6", code::QUERY_VALUE), (1, Some(-1)));
    // `NAME0` is locked for the host as well and accepts nothing.
    assert_eq!(l.send_int(LAN2, "name0", code::ITEM_SELECTED, 0).0, 0);
    assert_eq!(l.row(0).kind, HUMAN);
    // The host sets nation and team of the computer (path W), team with `FUN_004BA710`.
    assert_eq!(l.send_int(LAN2, "team2", code::ITEM_SELECTED, 0).0, 1);
    assert_eq!(l.row(2).team, 0);
    assert!(matches!(
        l.lobby_effects().as_slice(),
        [LobbyAction::Row {
            row: 2,
            team: 0,
            ..
        }]
    ));
}

// QUIRK E-302: kept
#[test]
fn start_visible_only_when_ready() {
    // L.3.4: host and every row with kind != 0 ready, at least two such rows, and the map name not empty.
    let mut l = Logic::lobby(&[HUMAN, HUMAN, 2, 0], 0);
    l.view().ready = [false, true, true, false, false, false, false, false];
    assert_eq!(
        l.lobby_status("start"),
        Some(HIDE),
        "host checkmark missing"
    );
    l.send_int(LAN2, "ready0", code::VALUE_CHANGED, 1);
    assert_eq!(
        l.lobby_effects(),
        [LobbyAction::Ready {
            row: 0,
            ready: true
        }]
    );
    assert_eq!(l.lobby_status("start"), Some(0), "all ready");
    l.view().ready[2] = false;
    assert_eq!(l.lobby_status("start"), Some(HIDE), "computer unchecked");
    l.view().ready[2] = true;
    l.data.draft.info.name.clear();
    assert_eq!(l.lobby_status("start"), Some(HIDE), "without map name");
    l.data.draft.info.name = "Map".into();
    // Only one occupied row.
    l.data.draft.slots[1].kind = 0;
    l.data.draft.slots[2].kind = 0;
    assert_eq!(l.lobby_status("start"), Some(HIDE), "alone");
    // `timeout 1000`; click -> request to the network session, no commands.
    let (r, msg) = l.send(LAN2, "start", code::QUERY_TIMEOUT);
    assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)));
    assert_eq!(l.click(LAN2, "start"), 1);
    assert_eq!(l.lobby_effects(), [LobbyAction::Start]);
    assert!(l.take_queue().is_empty());
    // Always hidden for the client, even when everyone is ready; a click sends nothing.
    let mut c = Logic::lobby(&[HUMAN, HUMAN], 1);
    c.view().ready = [true; 8];
    assert_eq!(c.lobby_status("start"), Some(HIDE));
    assert_eq!(c.click(LAN2, "start"), 1);
    assert!(c.lobby_effects().is_empty());
    // Hidden without a lobby.
    c.data.net = None;
    assert_eq!(c.lobby_status("start"), Some(HIDE));
}

/// Deviation (re/multiplayer-menus L.18): the data check of the network host locks `READY` of a row without the
/// verdict `Equal` (the click sets no local checkmark and sends nothing) and hides `START` while it holds the start
/// back, even with every checkmark set.
// QUIRK E-354: deviation
#[test]
fn data_check_locks_ready_and_start() {
    let mut c = Logic::lobby(&[HUMAN, HUMAN], 1);
    c.view().data_locked[1] = true;
    assert_eq!(c.lobby_status("ready1"), Some(DISABLE));
    assert_eq!(c.send_int(LAN2, "ready1", code::VALUE_CHANGED, 1).0, 0);
    assert!(!c.view().ready[1], "no local checkmark");
    assert!(c.lobby_effects().is_empty());
    assert_eq!(
        c.lobby_status("race1"),
        Some(0),
        "the rest of the row stays free"
    );
    c.view().data_locked[1] = false;
    assert_eq!(c.lobby_status("ready1"), Some(0));
    assert_eq!(c.send_int(LAN2, "ready1", code::VALUE_CHANGED, 1).0, 1);
    assert!(c.view().ready[1]);

    let mut h = Logic::lobby(&[HUMAN, HUMAN, 2], 0);
    h.view().ready = [true; 8];
    assert_eq!(h.lobby_status("start"), Some(0));
    h.view().start_locked = true;
    assert_eq!(
        h.lobby_status("start"),
        Some(HIDE),
        "held back by the data check"
    );
}

// QUIRK E-285: kept
#[test]
fn exit_goes_to_the_main_menu() {
    // E-285 (`0x004D49EE`...`0x004D4A44`): lobby closed, `{9}`, `{5,"start"}` - always to the main menu, never to
    // `LAN1`; for the host as for the client.
    let lobby_closed = [
        close(LAN2, true),
        close("levelinfo", false),
        close(CHAT, false),
    ];
    assert_eq!(crate::screens::close_lobby(), lobby_closed);
    assert_eq!(
        crate::screens::open_lobby(),
        [
            open(LAN2, true),
            open("levelinfo", false),
            open(CHAT, false)
        ]
    );
    for own_row in [0, 1] {
        let mut l = Logic::lobby(&[HUMAN, HUMAN], own_row);
        assert_eq!(l.click(LAN2, "exit"), 1);
        let mut want = lobby_closed.to_vec();
        want.push(AppCmd::CloseNet);
        want.push(open("start", true));
        assert_eq!(l.take_queue(), want, "row {own_row}");
        assert!(l.lobby_effects().is_empty());
    }
}

#[test]
fn changegame_visible_only_for_the_host() {
    // re/multiplayer-menus L.16 no. 1 (`0x004D4818`...`0x004D4892`): code 5 free for the host (`FUN_004E85F0`),
    // otherwise `HIDE`; code 9 every second.
    let mut host = Logic::lobby(&[HUMAN, HUMAN], 0);
    assert_eq!(host.lobby_status("changegame"), Some(0));
    let (r, msg) = host.send(LAN2, "changegame", code::QUERY_TIMEOUT);
    assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)));
    let mut client = Logic::lobby(&[HUMAN, HUMAN], 1);
    assert_eq!(client.lobby_status("changegame"), Some(HIDE));
    // A click by the client (the button is hidden) stays without effect.
    assert_eq!(client.click(LAN2, "changegame"), 1);
    assert!(client.take_queue().is_empty());
    assert!(client.effects.is_empty());
    // Hidden without a lobby.
    host.data.net = None;
    assert_eq!(host.lobby_status("changegame"), Some(HIDE));
}

// QUIRK E-310: deviation
#[test]
fn changegame_closes_the_lobby_and_opens_lan31() {
    // re/multiplayer-menus L.16 no. 1: code 8 checks nothing (even without clients and with all ready checkmarks):
    // `FUN_004BCBE0(0)`, then `{5,"lan31"}`; no lobby request, the core stays in the lobby.
    for kinds in [&[HUMAN][..], &[HUMAN, HUMAN, 2, 0]] {
        let mut l = Logic::lobby(kinds, 0);
        l.view().ready = [true; 8];
        assert_eq!(l.click(LAN2, "changegame"), 1);
        assert_eq!(
            l.take_queue(),
            [
                close(LAN2, true),
                close("levelinfo", false),
                close(CHAT, false),
                open("lan31", true)
            ],
            "{kinds:?}"
        );
        assert!(l.effects.is_empty());
    }
}

// QUIRK E-286: kept
#[test]
fn chat_input_drops_brackets_and_caps_79() {
    // re/multiplayer-menus L.4: `INPUT` shares the buffer `App+0x1418` (`size 0x50` = 79 characters, `inputbuffer 1`);
    // `[` and `]` are discarded (code `0xB` -> 1), the field accepts other characters (-1); ENTER and `SEND` send,
    // empty -> nothing; the input is empty afterwards.
    let mut l = Logic::lobby(&[HUMAN, HUMAN], 1);
    let (r, msg) = l.send(CHAT, "input", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(int_of(&msg, "size"), Some(INPUT_SIZE));
    assert_eq!(INPUT_SIZE, 0x50);
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.chat_text.clone()))
    );
    assert_eq!(l.chat_key(i32::from(b'[')), 1);
    assert_eq!(l.chat_key(i32::from(b']')), 1);
    assert_eq!(l.chat_key(i32::from(b'a')), -1);
    assert_eq!(l.chat_key(27), -1, "ESC does not close the chat");
    assert_eq!(l.send(CHAT, "input", code::EDIT_KEY).0, -1, "without `key`");
    assert!(l.effects.is_empty());
    // Empty: nothing.
    assert_eq!(l.chat_key(13), 1);
    assert_eq!(l.click(CHAT, "send"), 1);
    assert!(l.effects.is_empty());
    l.state.chat_text.set("Hello");
    assert_eq!(l.chat_key(13), 1);
    assert_eq!(l.lobby_effects(), [LobbyAction::Chat("Hello".into())]);
    assert_eq!(l.state.chat_text.get(), "");
    l.state.chat_text.set("Ready?");
    assert_eq!(l.click(CHAT, "send"), 1);
    assert_eq!(l.lobby_effects(), [LobbyAction::Chat("Ready?".into())]);
    assert_eq!(l.state.chat_text.get(), "");
    // No window closes (unlike the chat windows of the match).
    assert!(l.take_queue().is_empty());
    assert_eq!(l.send(CHAT, "send", code::QUERY_STATUS).0, -1);

    // In operation (corpus): typing accepts at most 79 characters and discards the brackets (narrow characters so that
    // the\n    // width of the field does not limit earlier).
    let Some(mut d) = live_with(
        EngineConfig::default(),
        MenuOptions::default(),
        Box::new(dispatch),
    ) else {
        return;
    };
    show(&mut d, CHAT);
    d.click(CHAT, "input");
    d.frame();
    for c in "a[b]"
        .encode_utf16()
        .chain(std::iter::repeat_n(u16::from(b'i'), 85))
    {
        let now = d.now;
        d.app.input(
            UiInput::Key {
                held: vec![0x41],
                ch: Some(c),
            },
            S1280,
            now,
        );
    }
    d.frame();
    let typed = d.app.screen_state().chat_text.get();
    assert_eq!(typed.chars().count(), 79);
    assert!(typed.starts_with("abii"), "{typed}");
    assert!(!typed.contains('[') && !typed.contains(']'));
    d.key(RETURN);
    d.idle();
    let sent = d
        .app
        .take_effects()
        .into_iter()
        .filter(|e| matches!(e, UiEffect::Lobby(_)))
        .collect::<Vec<_>>();
    assert_eq!(sent, [UiEffect::Lobby(LobbyAction::Chat(typed))]);
    assert_eq!(d.app.screen_state().chat_text.get(), "");
    assert_eq!(d.app.menu().open_names(), [CHAT], "the frame stays open");
}

// QUIRK E-286: kept
#[test]
fn chat_shows_five_lines_newest_last() {
    // `FUN_004B9A80`: five lines, the oldest on top, the newest at the bottom; missing ones on top as " [CR]".
    let line = |i: usize| chat_line("Anna", &format!("Line {i}"));
    let mut l = Logic::lobby(&[HUMAN, HUMAN], 0);
    l.view().chat = vec![line(1), line(2)];
    let (r, msg) = l.send(CHAT, "chat", code::QUERY_TEXT);
    assert_eq!(r, 1);
    let want = format!(" [CR] [CR] [CR]{}{}", line(1), line(2));
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.lobby_chat.clone()))
    );
    assert_eq!(l.state.lobby_chat.get(), want);
    l.view().chat = (1..=7).map(line).collect();
    l.send(CHAT, "chat", code::QUERY_TEXT);
    assert_eq!(
        l.state.lobby_chat.get(),
        (3..=7).map(line).collect::<String>()
    );
    assert_eq!(chat_text(None), "");

    // `MenuApp::set_net` writes the history immediately into the buffer that the label shows.
    let mut app = MenuApp::from_library(
        crate::dialog::DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    );
    let shared = app.screen_state().lobby_chat.clone();
    app.set_net(Some(NetMenuView {
        games: Vec::new(),
        lobby: Some(LobbyView {
            chat: vec![line(9)],
            ..LobbyView::default()
        }),
    }));
    assert_eq!(shared.get(), format!("{}{}", " [CR]".repeat(4), line(9)));
}

// QUIRK E-310: deviation
#[test]
fn chat_line_format() {
    // `FUN_004B9A80`: name red, text in the base color; text cut to 79 characters at the receiver (`FUN_004B9BD0`).
    assert_eq!(
        chat_line("Anna", "Hello"),
        "[COLOR FFFF0000]Anna: [DEFAULTCOLOR]Hello [CR]"
    );
    let long = "x".repeat(100);
    assert_eq!(
        chat_line("Bert", &long),
        format!(
            "[COLOR FFFF0000]Bert: [DEFAULTCOLOR]{} [CR]",
            "x".repeat(79)
        )
    );
    assert_eq!(
        chat_line("Cleo", &"ä".repeat(80)),
        format!(
            "[COLOR FFFF0000]Cleo: [DEFAULTCOLOR]{} [CR]",
            "ä".repeat(79)
        )
    );
}

#[test]
fn team_color_check_reports_the_swap() {
    // `FUN_004BA710` as the verdict for the lobby rule of the host. Game type 4: same team as another occupied slot ->
    // swap (the other one gets the old team); same color -> reject; team >= number of occupied slots -> reject.
    // Otherwise all in the same team -> reject.
    use crate::screens::{TeamColor, team_color_check};
    let slots = lobby_draft(&[HUMAN, HUMAN, 2, 0]).slots;
    let mut changed = slots[1].clone();
    changed.team = 2;
    assert_eq!(
        team_color_check(&slots, &changed, 4),
        TeamColor::Swap { key: 1, team: 1 }
    );
    assert_eq!(team_color_check(&slots, &changed, 0), TeamColor::Ok);
    changed.team = 3;
    assert_eq!(team_color_check(&slots, &changed, 4), TeamColor::Reject);
    changed.team = 1;
    changed.color = 2;
    assert_eq!(team_color_check(&slots, &changed, 0), TeamColor::Reject);
    let mut same = slots.clone();
    same[2].team = 0;
    let mut changed = same[1].clone();
    changed.team = 0;
    assert_eq!(team_color_check(&same, &changed, 0), TeamColor::Reject);
}

#[test]
fn levelinfo_shows_stage_and_rtt_for_the_host() {
    // In the lobby, `[CURLEVELINFOS]` appends "Turn raster" (99_100) with the stage (everyone), for the host
    // additionally the largest round-trip time once measured; in single player nothing.
    let texts = Texts::from_entries(&[
        (2, "<d>"),
        (6, "|"),
        (11, "<g>"),
        (15673, "Spielart: "),
        (15540, "DM"),
        (99_100, "Rundenraster"),
        (99_101, "Original"),
        (99_102, "LAN"),
        (99_103, "LAN schnell"),
        (99_104, "Laufzeit: %d ms"),
    ]);
    let draft = lobby_draft(&[HUMAN, HUMAN]);
    let single = level_infos(&draft, None, &texts);
    assert!(single.ends_with("<g>Spielart: <d>DM|"), "{single}");
    let host = LobbyView {
        is_host: true,
        stage: 2,
        max_rtt_ms: Some(37),
        ..LobbyView::default()
    };
    let net = LevelNetInfo::from_lobby(&host);
    assert_eq!(
        level_infos(&draft, Some(&net), &texts),
        format!("{single}<g>Rundenraster: <d>LAN schnell|<d>Laufzeit: 37 ms|")
    );
    // Client: only the stage, even if the state carries a round-trip time; host without measurement: only the stage.
    let client = LobbyView {
        is_host: false,
        own_row: 1,
        ..host.clone()
    };
    let only_stage = format!("{single}<g>Rundenraster: <d>LAN schnell|");
    assert_eq!(
        level_infos(&draft, Some(&LevelNetInfo::from_lobby(&client)), &texts),
        only_stage
    );
    let unmeasured = LobbyView {
        max_rtt_ms: None,
        ..host.clone()
    };
    assert_eq!(
        level_infos(&draft, Some(&LevelNetInfo::from_lobby(&unmeasured)), &texts),
        only_stage
    );
    assert_eq!(LevelNetInfo::from_net(None), None);
    assert_eq!(
        LevelNetInfo::from_net(Some(&NetMenuView::default())),
        None,
        "without lobby"
    );

    // `MenuApp` recomputes `[CURLEVELINFOS]` when stage or round-trip time change, not only on the
    // draft.
    let app = MenuApp::from_library(
        crate::dialog::DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    );
    let mut d = Driver::with_app(fixture(), app, Box::new(dispatch));
    d.app.set_lobby_draft(
        draft.slots.clone(),
        draft.info.clone(),
        draft.settings.clone(),
    );
    d.idle();
    let infos = |d: &Driver<_>| {
        d.app
            .text_context()
            .and_then(|c| c.cur_level.as_ref())
            .map(|l| l.infos.clone())
            .unwrap_or_default()
    };
    assert!(!infos(&d).contains("Rundenraster"), "without lobby");
    d.app.set_net(Some(NetMenuView {
        games: Vec::new(),
        lobby: Some(LobbyView {
            max_rtt_ms: Some(12),
            ..host.clone()
        }),
    }));
    d.frame();
    assert!(infos(&d).contains("LAN schnell"), "{}", infos(&d));
    assert!(infos(&d).contains("Laufzeit: 12 ms"), "{}", infos(&d));
    d.app.set_net(Some(NetMenuView {
        games: Vec::new(),
        lobby: Some(LobbyView {
            stage: 0,
            max_rtt_ms: Some(80),
            ..host
        }),
    }));
    d.frame();
    assert!(infos(&d).contains("Original"), "{}", infos(&d));
    assert!(infos(&d).contains("Laufzeit: 80 ms"), "{}", infos(&d));
}
