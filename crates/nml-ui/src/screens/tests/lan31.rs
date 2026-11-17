//! Tests of `LAN31` "Change game" (re/multiplayer-menus L.2 and L.16): opening on the session, `LEVELTYPE` without
//! savegame, slot check with 15402, `CHANGEGAME` to the network session, with random map, and `EXITCHANGE`. The logic
//! alone runs through [`Logic`] (lobby of the host from `tests/lan2.rs`), opening additionally through `MenuApp` with
//! the corpus dialogs (needs `NML_GAME_DIR`).

use super::create_game::live_game;
use super::*;
use crate::draft::tests::catalog;
use crate::draft::{DraftSettings, LevelInfoData, LobbyView, NetMenuView};
use crate::msg::LobbyAction;
use crate::screens::{CHANGE_GAME_REFUSED, close_change_game};

const LAN31: &str = "lan31";
const MAP: &str = "loadmultiplayermap";
const HUMAN: i32 = 999;

/// Session settings in the tests: game type 2 (game mode 2 "NORMAL" of the test catalog), limits, switches, name.
fn session_settings() -> DraftSettings {
    DraftSettings {
        game_type: 2,
        unit_limit: 3,
        resources: 2,
        time_limit: 45,
        lock_teams: true,
        map_explored: true,
        save_replay: false,
        session: "Evening round".into(),
    }
}

impl Logic {
    /// Lobby of the host over the rows `kinds` (key = row - 1); session on "Second" (row 1 of the `NORMAL` list,
    /// 4 slots) with [`session_settings`].
    fn change_lobby(kinds: &[i32]) -> Logic {
        let mut l = Logic::lobby(kinds, 0);
        l.data.maps = catalog(&[
            ("First", "NORMAL", 2, 0, true),
            ("Second", "NORMAL", 4, 2, true),
            ("Third", "NORMAL", 3, 1, true),
            ("Track", "RAILROAD1", 3, 1, true),
        ]);
        l.data.draft.map = None;
        l.data.draft.info = LevelInfoData {
            name: "Second".into(),
            path: l.data.maps.maps[1].entry.path.clone(),
            size_class: 2,
            players: 4,
            level_type: 0,
            ..LevelInfoData::default()
        };
        l.data.draft.settings = session_settings();
        l
    }

    /// Open `LAN31` (code 5 of the frame), empty the queue and the requeries.
    fn open_change(&mut self) {
        assert_eq!(self.send(LAN31, LAN31, code::QUERY_STATUS).0, 1);
        self.take_queue();
        self.requery.clear();
    }

    /// Select row `row` of the map list (code 4).
    fn pick_row(&mut self, row: i32) {
        assert_eq!(self.send_int(MAP, "levels", code::ITEM_SELECTED, row).0, 1);
        self.requery.clear();
    }
}

#[test]
fn lan31_opens_on_the_session_map() {
    // (Deviation from E-281) code 5 of the frame sets map kind, game mode (list index for the game type), limits,
    // switches and `NetSession` from the session and opens the side frame afterwards; `RandomMapSeed` stays.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN, 0, 0]);
    l.config.level_type = 1;
    l.config.game_type = 0;
    l.config.unit_limit = 0;
    l.config.random_map_seed = name_field("Seed");
    assert_eq!(l.send(LAN31, LAN31, code::QUERY_STATUS).0, 1);
    assert_eq!(l.take_queue(), [open(MAP, false)]);
    let c = &l.config;
    assert_eq!((c.level_type, c.game_type), (0, 2));
    assert_eq!((c.unit_limit, c.resourcen, c.time_limit), (3, 2, 45));
    assert_eq!((c.lock_teams, c.map_explored, c.save_replay), (1, 1, 0));
    assert_eq!(name_text(&c.net_session), "Evening round");
    assert_eq!(name_text(&c.random_map_seed), "Seed");
    assert_eq!(l.data.level.map, Some(1));
    // The map list preselects the session map (row 1), not entry 0 (O-128).
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(1)));
    assert_eq!(l.data.level.info.name, "Second");
    // Other game mode (list without the session map): entry 0.
    l.config.game_type = 4;
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(0)));
    assert_eq!(l.data.level.info.name, "Track");
    // In `LAN3` (no lobby yet) it stays at entry 0.
    l.config.game_type = 2;
    l.data.net = None;
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(0)));

    // Random map of the session: values to `RANDOMMAP`, `RandomPlayers` = players - 2, side frame `randommap`.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN]);
    l.config.random_map_seed = name_field("Seed");
    l.data.draft.info = LevelInfoData {
        name: "12345".into(),
        path: String::new(),
        size_class: 1,
        players: 4,
        level_type: 1,
        random_terrain: 2,
        random_landscape: 1,
        random_daytime: 3,
    };
    assert_eq!(l.send(LAN31, LAN31, code::QUERY_STATUS).0, 1);
    assert_eq!(l.take_queue(), [open("randommap", false)]);
    let c = &l.config;
    assert_eq!(c.level_type, 1);
    assert_eq!(
        (
            c.random_map_size,
            c.random_players,
            c.random_terrain,
            c.random_landscape,
            c.random_daytime
        ),
        (1, 2, 2, 1, 3)
    );
    assert_eq!(name_text(&c.random_map_seed), "Seed", "E-313");

    // `SESSIONNAME` writes in `LAN31` through the mirror to `NetSession`.
    let (r, msg) = l.send(LAN31, "sessionname", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.net_session.clone()))
    );
    l.state.net_session.set("Late round");
    l.send(LAN31, "sessionname", code::EDIT_CHANGED);
    assert_eq!(name_text(&l.config.net_session), "Late round");

    // In operation: `LAN31` opens with `LOADMULTIPLAYERMAP`, the list is on the session map.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    let maps = &d.app.data().maps;
    let list = maps.filtered(0);
    assert!(list.len() > 1, "corpus: at least two maps of game mode 0");
    let m = maps.maps[list[1]].clone();
    let kind = maps.game_kind(0);
    let info = LevelInfoData {
        name: m.name.clone(),
        path: m.entry.path.clone(),
        size_class: m.size_class,
        players: m.players(),
        level_type: 0,
        ..LevelInfoData::default()
    };
    let settings = DraftSettings {
        game_type: kind,
        ..session_settings()
    };
    d.app.set_net(Some(NetMenuView {
        games: Vec::new(),
        lobby: Some(LobbyView {
            is_host: true,
            ..LobbyView::default()
        }),
    }));
    d.app.set_lobby_draft(m.slots.clone(), info, settings);
    show(&mut d, LAN31);
    assert_eq!(d.app.menu().open_names(), [LAN31, MAP]);
    assert_eq!(element(&d, MAP, "levels").selection, Some(1));
    assert_eq!(d.app.data().level.map, Some(list[1]));
    assert_eq!(name_text(&d.app.config().net_session), "Evening round");
    assert_eq!(element(&d, LAN31, "leveltype").items.len(), 2);
}

#[test]
fn lan31_offers_no_saved_game() {
    // (Deviation from E-308) `LEVELTYPE` in `LAN31` only "Map" (0) and "Random map" (1); a `LevelType` 2 becomes 0.
    // `LAN3` still offers "Load game".
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN]);
    l.config.level_type = 2;
    let (r, msg) = l.send(LAN31, "leveltype", code::QUERY_ITEMS);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("indexarray"),
        Some(&Param::IndexArray(vec![0, 1]))
    );
    assert_eq!(l.items(LAN31, "leveltype").len(), 2);
    assert_eq!(l.config.level_type, 0);
    let (_, msg) = l.send("lan3", "leveltype", code::QUERY_ITEMS);
    assert_eq!(
        msg.params.get("indexarray"),
        Some(&Param::IndexArray(vec![0, 1, 2]))
    );
    // Code 4 and the game mode change do not requery `create2` in `LAN31`.
    l.send_int(LAN31, "leveltype", code::ITEM_SELECTED, 1);
    l.send_int(LAN31, "gametype", code::ITEM_SELECTED, 0);
    assert!(l.requery.iter().all(|q| q.element != "create2"));
    // `LAN31` does not know the buttons of `LAN3`, and `LAN3` not those of `LAN31`.
    assert_eq!(l.click(LAN31, "create"), -1);
    assert_eq!(l.click(LAN31, "exit"), -1);
    assert_eq!(l.click("lan3", "changegame"), -1);
    assert_eq!(l.click("lan3", "exitchange"), -1);
}

#[test]
fn changegame_with_occupied_dropped_rows_shows_15402() {
    // (Deviation from E-307) computer on row 3, new map with 3 players -> message 15402 without follow-up command,
    // `LAN31` stays open, no change to the network session, the draft stays.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN, 0, 2]);
    l.open_change();
    let before = l.data.draft.clone();
    l.pick_row(2);
    assert_eq!(l.click(LAN31, "changegame"), 1);
    assert_eq!(l.take_queue(), [open("msgbox", true)]);
    assert_eq!(l.state.info_text, Some(CHANGE_GAME_REFUSED));
    assert_eq!(l.state.info_ok, None);
    assert!(l.lobby_effects().is_empty());
    assert_eq!(l.data.draft, before);
    // A human on row 3 also blocks the change.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN, 0, HUMAN]);
    l.open_change();
    l.pick_row(2);
    l.click(LAN31, "changegame");
    assert_eq!(l.take_queue(), [open("msgbox", true)]);
    assert!(l.lobby_effects().is_empty());
}

#[test]
fn changegame_sends_the_change_and_reopens_the_lobby() {
    // An open row 3 drops out with 3 players -> `Info` from the chosen map, map and settings into the draft, the rows
    // stay, and only the change to the network session; it closes `LAN31` on success with [`close_change_game`] and
    // opens the lobby.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN, 0, 0]);
    l.open_change();
    let slots = l.data.draft.slots.clone();
    l.pick_row(2);
    l.config.unit_limit = 1;
    l.config.net_session = name_field("New round");
    assert_eq!(l.click(LAN31, "changegame"), 1);
    assert!(
        l.take_queue().is_empty(),
        "LAN31 closes the network session"
    );
    assert_eq!(l.lobby_effects(), [LobbyAction::ChangeGame]);
    let draft = &l.data.draft;
    assert_eq!(draft.map, Some(2));
    assert_eq!(
        draft.info,
        LevelInfoData {
            name: "Third".into(),
            path: l.data.maps.maps[2].entry.path.clone(),
            size_class: 1,
            players: 3,
            level_type: 0,
            ..LevelInfoData::default()
        }
    );
    assert_eq!(draft.slots, slots, "the lobby rows stay");
    assert_eq!(draft.settings.unit_limit, 1);
    assert_eq!(draft.settings.session, "New round");
    assert_eq!(draft.settings.game_type, 2);
    // Sequence on success (and with `EXITCHANGE`): `{3,"lan31"}`, `{4, side frame}`, lobby open.
    let lobby = [
        open("lan2", true),
        open("levelinfo", false),
        open("chatframe", false),
    ];
    for (level_type, frame) in [(0, Some(MAP)), (1, Some("randommap")), (5, None)] {
        let mut want = vec![close(LAN31, true)];
        want.extend(frame.map(|f| close(f, false)));
        want.extend(lobby.clone());
        assert_eq!(close_change_game(level_type), want, "{level_type}");
    }
    // Random map: likewise only the change, the rows stay ([`empty_seed_draws_anew_on_each_changegame`]).
    l.config.level_type = 1;
    assert_eq!(l.click(LAN31, "changegame"), 1);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.lobby_effects(), [LobbyAction::ChangeGame]);
    assert_eq!(l.data.draft.info.level_type, 1);
    assert_eq!(l.data.draft.slots, slots);
}

#[test]
fn empty_seed_draws_anew_on_each_changegame() {
    // E-313 (reproduced): `CHANGEGAME` with a random map fills `Info` like `random_slots` (seed, size, players
    // `RandomPlayers + 2`, map type, landscape, time of day), but does not replace the lobby rows. An empty `SEED` is
    // drawn anew at each `CHANGEGAME` (`0x004D50D2` -> `FUN_004BF1F0(1)` -> `rand()`), only in `Info`; `RandomMapSeed`
    // stays empty.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN, 0, 0]);
    l.open_change();
    let slots = l.data.draft.slots.clone();
    assert_eq!(l.send_int(LAN31, "leveltype", code::ITEM_SELECTED, 1).0, 1);
    l.take_queue();
    l.requery.clear();
    l.config.random_map_seed = name_field("");
    l.config.random_map_size = 1;
    l.config.random_players = 0;
    l.config.random_terrain = 2;
    l.config.random_landscape = 1;
    l.config.random_daytime = 3;
    let mut seeds = Vec::new();
    for _ in 0..2 {
        assert_eq!(l.click(LAN31, "changegame"), 1);
        assert!(
            l.take_queue().is_empty(),
            "LAN31 closes the network session"
        );
        assert_eq!(l.lobby_effects(), [LobbyAction::ChangeGame]);
        let draft = &l.data.draft;
        assert_eq!(
            draft.info,
            LevelInfoData {
                name: draft.info.name.clone(),
                path: draft.info.path.clone(),
                size_class: 1,
                players: 2,
                level_type: 1,
                random_terrain: 2,
                random_landscape: 1,
                random_daytime: 3,
            }
        );
        assert_eq!(draft.slots, slots, "the lobby rows stay");
        assert_eq!(draft.settings.session, "Evening round");
        seeds.push(draft.info.name.clone());
    }
    assert_eq!(seeds, ["41", "18467"]);
    assert_eq!(name_text(&l.config.random_map_seed), "");
    // A set seed is used unchanged.
    l.config.random_map_seed = name_field("Net");
    l.click(LAN31, "changegame");
    assert_eq!(l.data.draft.info.name, "Net");

    // An occupied row that drops out also with a random map: 15402, no change, the draft stays.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN, 2, 0]);
    l.open_change();
    let before = l.data.draft.clone();
    l.send_int(LAN31, "leveltype", code::ITEM_SELECTED, 1);
    l.take_queue();
    l.config.random_map_size = 0;
    l.config.random_players = 0;
    l.click(LAN31, "changegame");
    assert_eq!(l.take_queue(), [open("msgbox", true)]);
    assert_eq!(l.state.info_text, Some(CHANGE_GAME_REFUSED));
    assert!(l.lobby_effects().is_empty());
    assert_eq!(l.data.draft, before);
}

#[test]
fn exitchange_reopens_the_lobby_without_change() {
    // `0x004D51A7`...`0x004D5202`: frame closed, lobby open - nothing else; the draft (= session) stays, the settings
    // keep the last choice as in the original.
    let mut l = Logic::change_lobby(&[HUMAN, HUMAN]);
    l.open_change();
    let before = l.data.draft.clone();
    l.pick_row(0);
    l.config.unit_limit = 0;
    assert_eq!(l.click(LAN31, "exitchange"), 1);
    assert_eq!(l.take_queue(), close_change_game(0));
    assert!(l.effects.is_empty());
    assert_eq!(l.data.draft, before);
    assert_eq!(l.config.unit_limit, 0);
}
