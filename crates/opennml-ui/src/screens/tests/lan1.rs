//! Tests of the network menus `LAN1` and `LAN3`: per element the row of the tables in re/multiplayer-menus L.1.1 and
//! L.2, plus the fallback for the selection field `TURNRASTER`, the own texts and the network commands of
//! `MenuApp`. The logic alone runs through [`Logic`], live through `MenuApp` with the corpus dialogs (these need
//! `NML_GAME_DIR`).

use opennml_rda::GameVersion;

use super::create_game::live_game;
use super::*;
use crate::assets::Texts;
use crate::draft::tests::catalog;
use crate::draft::{FoundGameView, LevelInfoData, LobbyView, NetMenuView, SlotDraft};
use crate::own_texts;

const LAN1: &str = "lan1";
const LAN3: &str = "lan3";
const MAP: &str = "loadmultiplayermap";
const HIDE: i32 = 0x1000_0000;

fn game(instance: u64, name: &str) -> FoundGameView {
    FoundGameView {
        instance,
        name: name.into(),
    }
}

impl Logic {
    /// Logic with the player name "Anna" and the found games `games`.
    fn lan(games: Vec<FoundGameView>) -> Logic {
        let mut l = Logic::new();
        l.config.net_name = name_field("Anna");
        l.set_games(games);
        l
    }

    fn set_games(&mut self, games: Vec<FoundGameView>) {
        self.data.net = Some(NetMenuView { games, lobby: None });
    }

    fn lan_status(&mut self, dialog: &str, element: &str) -> Option<i32> {
        int_of(&self.send(dialog, element, code::QUERY_STATUS).1, "status")
    }

    /// The per-second query of the list like the ListBox (`FUN_004638F0`): code 2, then code 0; returns entries and
    /// selection (-1 without `int`).
    fn refresh(&mut self) -> (Vec<String>, i32) {
        let items = self.items(LAN1, "opengames");
        let (r, v) = self.query(LAN1, "opengames", code::QUERY_VALUE);
        let sel = if r == -1 { -1 } else { v.unwrap_or(-1) };
        (items, sel)
    }
}

#[test]
fn lan1_starts_the_search() {
    // `0x004D3058`...`0x004D3076`: code 5 of the frame starts the search, selection -1, flag "join running" 0.
    let mut l = Logic::lan(vec![game(7, "Round")]);
    l.data.lan_choice = Some(7);
    l.state.lan_joining = true;
    let (r, msg) = l.send(LAN1, LAN1, code::QUERY_STATUS);
    assert_eq!(r, 1);
    assert!(!msg.params.contains_key("status"));
    assert_eq!(l.effects, [UiEffect::Search(true)]);
    assert_eq!(l.data.lan_choice, None);
    assert!(!l.state.lan_joining);
    assert!(l.take_queue().is_empty());
    // `timeout 1000` of the list; unknown elements go to no branch.
    let (r, msg) = l.send(LAN1, "opengames", code::QUERY_TIMEOUT);
    assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)));
    assert_eq!(l.click(LAN1, "options"), -1);
}

#[test]
fn join_and_create_hidden_without_name() {
    // `join` code 5: hidden without a selection or with an empty name; `create` code 5: hidden with an empty name.
    let mut l = Logic::lan(vec![game(1, "A")]);
    assert_eq!(l.lan_status(LAN1, "join"), Some(HIDE), "without selection");
    assert_eq!(l.lan_status(LAN1, "create"), Some(0));
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 0);
    assert_eq!(l.data.lan_choice, Some(1));
    assert_eq!(l.requery, [Requery::new(LAN1, "join", code::QUERY_STATUS)]);
    l.requery.clear();
    assert_eq!(l.lan_status(LAN1, "join"), Some(0));
    l.config.net_name = [0; 40];
    assert_eq!(l.lan_status(LAN1, "join"), Some(HIDE), "empty name");
    assert_eq!(l.lan_status(LAN1, "create"), Some(HIDE));

    // `playername`: code 7 shows `NetName` (pointer, `size 30`, `inputbuffer 1`); code `0xC` writes `NetName` via the
    // mirror and requeries `join` and `create` (`0x004D3155`).
    l.config.net_name = name_field("Bert");
    let (r, msg) = l.send(LAN1, "playername", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.net_name.clone()))
    );
    assert_eq!(l.state.net_name.get(), "Bert");
    assert_eq!(int_of(&msg, "size"), Some(30));
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    l.state.net_name.set("");
    l.send(LAN1, "playername", code::EDIT_CHANGED);
    assert_eq!(name_text(&l.config.net_name), "");
    assert_eq!(
        l.requery,
        [
            Requery::new(LAN1, "join", code::QUERY_STATUS),
            Requery::new(LAN1, "create", code::QUERY_STATUS)
        ]
    );
    assert_eq!(l.lan_status(LAN1, "join"), Some(HIDE));
    l.state.net_name.set("Cleo");
    l.send(LAN1, "playername", code::EDIT_CHANGED);
    assert_eq!(name_text(&l.config.net_name), "Cleo");
    assert_eq!(l.lan_status(LAN1, "join"), Some(0));
    assert_eq!(l.lan_status(LAN1, "create"), Some(0));
}

// QUIRK E-278: kept
// QUIRK E-300: deviation
#[test]
fn selection_follows_the_session() {
    // E-300, deviation: the selection is tied to the session ID. B selected at index 1, A disappears -> the list shows
    // B at index 0, the selection stays B (the original silently showed the next game at the old index).
    let mut l = Logic::lan(vec![game(10, "A"), game(20, "B"), game(30, "C")]);
    assert_eq!(l.refresh(), (vec!["A".into(), "B".into(), "C".into()], -1));
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 1);
    assert_eq!(l.data.lan_choice, Some(20));
    assert_eq!(l.refresh().1, 1);
    l.set_games(vec![game(20, "B"), game(30, "C")]);
    assert_eq!(l.refresh(), (vec!["B".into(), "C".into()], 0));
    assert_eq!(l.data.lan_choice, Some(20));
    assert_eq!(l.lan_status(LAN1, "join"), Some(0));
    // A new game before it does not move the selection either.
    l.set_games(vec![game(40, "D"), game(20, "B")]);
    assert_eq!(l.refresh().1, 1);
    assert_eq!(l.data.lan_choice, Some(20));
}

// QUIRK E-278: kept
// QUIRK E-300: deviation
#[test]
fn vanished_selection_resets() {
    // If the selected session disappears, the selection is empty (-1) and `JOIN` hidden; a click does nothing.
    let mut l = Logic::lan(vec![game(10, "A"), game(20, "B")]);
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 1);
    l.set_games(vec![game(10, "A")]);
    assert_eq!(l.refresh(), (vec!["A".into()], -1));
    assert_eq!(l.data.lan_choice, None);
    assert_eq!(l.lan_status(LAN1, "join"), Some(HIDE));
    assert_eq!(l.click(LAN1, "join"), 1);
    assert!(l.take_queue().is_empty());
    // Without a network session the list is empty.
    l.data.net = None;
    assert_eq!(l.refresh(), (Vec::<String>::new(), -1));
    // Code 0 alone (without a previous code 2) also clears the selection if the session is missing.
    l.set_games(vec![game(10, "A")]);
    l.data.lan_choice = Some(99);
    assert_eq!(l.query(LAN1, "opengames", code::QUERY_VALUE), (-1, None));
    assert_eq!(l.data.lan_choice, None);
    // A selection outside the list selects nothing.
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 5);
    assert_eq!(l.data.lan_choice, None);
}

#[test]
fn double_click_joins_once() {
    // `0x004D3478`...`0x004D3505`: selection, name not empty, flag 0 -> abort the search, `{3,"lan1"}`, `{7}`, flag 1;
    // a second double click (or `JOIN`) stays without effect until the next opening of `LAN1`.
    let mut l = Logic::lan(vec![game(10, "A")]);
    l.send(LAN1, "opengames", code::DOUBLE_CLICK);
    assert!(l.take_queue().is_empty(), "without selection nothing");
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 0);
    assert_eq!(l.send(LAN1, "opengames", code::DOUBLE_CLICK).0, 1);
    assert_eq!(l.take_queue(), [close(LAN1, true), AppCmd::Join]);
    assert_eq!(l.effects, [UiEffect::Search(false)]);
    assert!(l.state.lan_joining);
    l.send(LAN1, "opengames", code::DOUBLE_CLICK);
    l.click(LAN1, "join");
    assert!(l.take_queue().is_empty());
    // A new opening resets the flag (and the selection); `JOIN` acts like the double click.
    l.send(LAN1, LAN1, code::QUERY_STATUS);
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 0);
    l.effects.clear();
    assert_eq!(l.click(LAN1, "join"), 1);
    assert_eq!(l.take_queue(), [close(LAN1, true), AppCmd::Join]);
    assert_eq!(l.effects, [UiEffect::Search(false)]);
    // Double click with an empty name: nothing (`wcslen(NetName) > 0`).
    l.send(LAN1, LAN1, code::QUERY_STATUS);
    l.send_int(LAN1, "opengames", code::ITEM_SELECTED, 0);
    l.config.net_name = [0; 40];
    l.send(LAN1, "opengames", code::DOUBLE_CLICK);
    assert!(l.take_queue().is_empty());
}

#[test]
fn cancel_closes_the_net() {
    // `0x004D30A1`...`0x004D311F`: abort the search, `{3,"lan1"}`, `{5,"start"}`, `{9}`; `CREATE` (`0x004D363B`...):
    // abort the search, `{3,"lan1"}`, `{5,"lan3"}`.
    let mut l = Logic::lan(Vec::new());
    assert_eq!(l.click(LAN1, "cancel"), 1);
    assert_eq!(
        l.take_queue(),
        [close(LAN1, true), open("start", true), AppCmd::CloseNet]
    );
    assert_eq!(l.effects, [UiEffect::Search(false)]);
    l.effects.clear();
    assert_eq!(l.click(LAN1, "create"), 1);
    assert_eq!(l.take_queue(), [close(LAN1, true), open(LAN3, true)]);
    assert_eq!(l.effects, [UiEffect::Search(false)]);
}

/// Logic with a map catalog, player name "Anna" and the second map (4 slots) selected.
fn lan3_with_map() -> Logic {
    let mut l = Logic::new();
    l.data.maps = catalog(&[
        ("First", "NORMAL", 2, 0, true),
        ("Second", "NORMAL", 4, 2, true),
    ]);
    l.config.net_name = name_field("Anna");
    let texts = &l.fixture.texts;
    let data = &mut l.parts.data;
    assert!(data.level.select(&data.maps, 0, 1, texts));
    l
}

#[test]
fn lan3_create_hosts_a_fixed_map() {
    // `0x004D4F05`...`0x004D4FBC`: `{3}`, `{4, side frame}`, `FUN_004BF1F0(0)` (slots from the map, own slot type 999
    // with `NetName`), `FUN_004B9700`, `{8}`.
    let mut l = lan3_with_map();
    l.config.game_type = 3;
    l.config.unit_limit = 1;
    l.config.net_session = name_field("Round");
    l.config.handicap = 2;
    assert_eq!(l.click(LAN3, "create"), 1);
    assert_eq!(
        l.take_queue(),
        [close(LAN3, true), close(MAP, false), AppCmd::Host]
    );
    let draft = &l.data.draft;
    assert_eq!(draft.map, Some(1));
    assert_eq!(draft.info.name, "Second");
    assert_eq!(draft.info.players, 4);
    assert_eq!(draft.info.level_type, 0);
    assert_eq!(draft.slots.len(), 4);
    assert_eq!(
        draft.slots[0],
        SlotDraft {
            key: -1,
            name: "Anna".into(),
            kind: 999,
            handicap: 2,
            ..l.data.maps.maps[1].slots[0].clone()
        }
    );
    assert!(draft.slots[1..].iter().all(|s| s.kind == 0));
    assert_eq!(draft.settings.session, "Round");
    assert_eq!(
        (draft.settings.game_type, draft.settings.unit_limit),
        (3, 1)
    );
    // `LAN3` does not know `CREATE2` and `EXIT2` of the single player.
    assert_eq!(l.click(LAN3, "create2"), -1);
    assert_eq!(l.click(LAN3, "exit2"), -1);
    assert_eq!(l.send(LAN3, "create2", code::QUERY_STATUS).0, -1);
    assert!(l.take_queue().is_empty());
}

// QUIRK E-313: kept
#[test]
fn lan3_create_random_hosts_a_random_map() {
    // `LevelType` 1 like `CREATE2` (as in E-313) - `{3}`, `{4,"randommap"}`, `FUN_004BF1F0(0)` over the random map
    // (slots `i = 0 ... n-1` open, team and color `i`, own slot type 999 with `NetName`), `FUN_004B9700`, then `{8}`.
    // `Info.Path` stays that of the last selected fixed map.
    let mut l = lan3_with_map();
    l.config.level_type = 1;
    l.config.game_type = 1;
    l.config.random_map_seed = name_field("Seed");
    l.config.random_map_size = 1;
    l.config.random_players = 1;
    l.config.random_terrain = 2;
    l.config.random_landscape = 3;
    l.config.random_daytime = 1;
    l.config.net_session = name_field("Round");
    l.config.handicap = 2;
    let old_path = l.data.level.info.path.clone();
    assert_eq!(l.click(LAN3, "create"), 1);
    assert_eq!(
        l.take_queue(),
        [close(LAN3, true), close("randommap", false), AppCmd::Host]
    );
    let draft = &l.data.draft;
    assert_eq!(
        draft.info,
        LevelInfoData {
            name: "Seed".into(),
            path: old_path,
            size_class: 1,
            players: 3,
            level_type: 1,
            random_terrain: 2,
            random_landscape: 3,
            random_daytime: 1,
        }
    );
    let open_slot = |i: i32| SlotDraft {
        key: i - 1,
        name: String::new(),
        kind: 0,
        nation: 0,
        team: i,
        handicap: 0,
        color: i,
    };
    assert_eq!(
        draft.slots,
        [
            SlotDraft {
                name: "Anna".into(),
                kind: 999,
                handicap: 2,
                ..open_slot(0)
            },
            open_slot(1),
            open_slot(2)
        ]
    );
    assert_eq!(draft.settings.session, "Round");
    assert_eq!(draft.settings.game_type, l.data.maps.game_kind(1));
    // Empty seed: drawn anew at every `CREATE` from the menu state (O-140), only in `Info`, `SEED` stays empty.
    l.config.random_map_seed = name_field("");
    l.click(LAN3, "create");
    assert_eq!(l.data.draft.info.name, "41");
    l.click(LAN3, "create");
    assert_eq!(l.data.draft.info.name, "18467");
    assert_eq!(name_text(&l.config.random_map_seed), "");
    assert_eq!(l.data.draft.slots.len(), 3, "rebuilt, not duplicated");
}

#[test]
fn lan3_create_saved_sends_0x12() {
    // re/multiplayer-saves S.2.1 (`0x004D4F05`...`0x004D4FBC`): `LevelType` 2 (savegame) -> `{3,"lan3"}`,
    // `{4,"loadsavednetworkgame"}`, `{0x12}` instead of `{8}`; no slots and no settings from the menu (they come from
    // the savegame), no effect in the handler.
    let mut l = lan3_with_map();
    l.config.level_type = 2;
    l.config.net_session = name_field("Round");
    assert_eq!(l.click(LAN3, "create"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(LAN3, true),
            close("loadsavednetworkgame", false),
            AppCmd::HostSavedGame
        ]
    );
    assert!(l.data.draft.slots.is_empty());
    assert_eq!(l.data.draft.settings.session, "");
    assert!(l.effects.is_empty());
}

#[test]
fn lan3_exit_goes_to_lan1() {
    // `0x004D521D`...`0x004D5295`: `{3,"lan3"}`, `{4, side frame}`, `{5,"lan1"}`.
    let mut l = Logic::new();
    for (level_type, frame) in [
        (0, Some(MAP)),
        (1, Some("randommap")),
        (2, Some("loadsavednetworkgame")),
        (5, None),
    ] {
        l.config.level_type = level_type;
        assert_eq!(l.click(LAN3, "exit"), 1);
        let mut want = vec![close(LAN3, true)];
        want.extend(frame.map(|f| close(f, false)));
        want.push(open(LAN1, true));
        assert_eq!(l.take_queue(), want, "{level_type}");
    }
    // The frame of `LAN3` does not clamp `LevelType` 2 (re/multiplayer-menus L.2): savegame opens
    // `LOADSAVEDNETWORKGAME`.
    l.config.level_type = 2;
    assert_eq!(l.send(LAN3, LAN3, code::QUERY_STATUS).0, 1);
    assert_eq!(l.config.level_type, 2);
    assert_eq!(l.take_queue(), [open("loadsavednetworkgame", false)]);
    l.config.level_type = 3;
    assert_eq!(l.send(LAN3, LAN3, code::QUERY_STATUS).0, 0);
    assert!(l.take_queue().is_empty());
    // `LEVELTYPE` offers "Load game" on the network (value 2); code 4 does not requery `create2`.
    l.config.level_type = 0;
    let (_, msg) = l.send(LAN3, "leveltype", code::QUERY_ITEMS);
    assert_eq!(
        msg.params.get("indexarray"),
        Some(&Param::IndexArray(vec![0, 1, 2]))
    );
    l.send_int(LAN3, "leveltype", code::ITEM_SELECTED, 1);
    assert!(l.requery.iter().all(|q| q.element != "create2"));
    l.requery.clear();
    l.send_int(LAN3, "gametype", code::ITEM_SELECTED, 0);
    assert!(l.requery.iter().all(|q| q.element != "create2"));
}

#[test]
fn lan3_sessionname_mirrors_netsession() {
    // `SESSIONNAME` shows `NetSession` (pointer, `size 30`); code `0xC` writes back via the mirror.
    let mut l = Logic::new();
    l.config.net_session = name_field("Round");
    let (r, msg) = l.send(LAN3, "sessionname", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.net_session.clone()))
    );
    assert_eq!(int_of(&msg, "size"), Some(30));
    l.state.net_session.set("Evening round");
    l.send(LAN3, "sessionname", code::EDIT_CHANGED);
    assert_eq!(name_text(&l.config.net_session), "Evening round");

    // Live: typing writes `NetSession`.
    let Some(mut d) = live_game(EngineConfig {
        net_session: name_field("Round"),
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, LAN3);
    d.click(LAN3, "sessionname");
    d.frame();
    type_text(&mut d, "X");
    assert_eq!(name_text(&d.app.config().net_session), "RoundX");
}

#[test]
fn turn_raster_has_no_field_in_lan3() {
    // Fallback: `TURNRASTER` covered the right halves of the switches `SAVEREPLAY`, `LOCKTEAMS`, `MAPEXPLORED` and took
    // clicks from them; `LAN3` has no free space. The field is dropped, the stage only comes from `NetTurnRaster` and
    // the command line; `LAN3` again has exactly the elements of its file.
    let mut l = Logic::new();
    assert_eq!(l.send(LAN3, "turnraster", code::QUERY_ITEMS).0, -1);
    assert_eq!(l.send(LAN3, "turnraster", code::QUERY_VALUE).0, -1);
    l.send_int(LAN3, "turnraster", code::ITEM_SELECTED, 2);
    assert_eq!(
        l.config.net_turn_raster, 0,
        "no handler writes NetTurnRaster"
    );

    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, LAN3);
    let names: Vec<String> = d
        .app
        .report(S1280, d.now)
        .elements
        .into_iter()
        .filter(|e| e.dialog == LAN3)
        .map(|e| e.name)
        .collect();
    assert!(names.iter().any(|n| n == "lockteams"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("turnraster")),
        "{names:?}"
    );
}

#[test]
fn own_texts_resolve_per_edition() {
    // One text per edition; the table only gets missing numbers.
    use GameVersion::{English, German};
    assert_eq!(own_texts::text(99_100, German), Some("Rundenraster"));
    assert_eq!(own_texts::text(99_100, English), Some("Turn raster"));
    assert_eq!(own_texts::text(99_103, German), Some("LAN schnell"));
    assert_eq!(own_texts::text(99_103, English), Some("LAN fast"));
    assert_eq!(own_texts::text(99_104, German), Some("Laufzeit: %d ms"));
    assert_eq!(own_texts::text(99_104, English), Some("Round trip: %d ms"));
    // 99 105 = `MAP_MISSING`.
    assert_eq!(own_texts::MAP_MISSING, 99_105);
    assert_eq!(
        own_texts::text(99_105, German),
        Some(
            "[CENTER]Die Karte der Partie fehlt auf diesem Rechner. Sie können erst mitspielen, wenn der Host \
             eine andere Karte wählt."
        )
    );
    assert_eq!(
        own_texts::text(99_105, English),
        Some(
            "[CENTER]The game's map is missing on this computer. You can only take part once the host chooses \
             another map."
        )
    );
    // 99 106 ... 99 108 (messages when hosting from a network savegame).
    let saved = [
        (
            own_texts::SAVE_NOT_LOADED,
            99_106,
            "[CENTER]Der Netz-Spielstand lässt sich nicht laden.",
            "[CENTER]The network saved game cannot be loaded.",
        ),
        (
            own_texts::SAVE_ONE_PLAYER,
            99_107,
            "[CENTER]In diesem Spielstand ist nur noch ein Spieler im Spiel.",
            "[CENTER]Only one player is still in this saved game.",
        ),
        (
            own_texts::SAVE_SLOT_OUT,
            99_108,
            "[CENTER]Ihr Platz in diesem Spielstand ist schon ausgeschieden.",
            "[CENTER]Your place in this saved game is already out.",
        ),
    ];
    for (id, number, de, en) in saved {
        assert_eq!(id, number);
        assert_eq!(own_texts::text(id, German), Some(de));
        assert_eq!(own_texts::text(id, English), Some(en));
    }
    // 99 109 = `REPLAY_NOT_PLAYABLE`.
    assert_eq!(own_texts::REPLAY_NOT_PLAYABLE, 99_109);
    assert_eq!(
        own_texts::text(99_109, German),
        Some("[CENTER]Die Aufnahme lässt sich nicht abspielen.")
    );
    assert_eq!(
        own_texts::text(99_109, English),
        Some("[CENTER]The recording cannot be played back.")
    );
    // 99 110 ... 99 115 (game data check in the lobby, failed network start).
    let data = [
        (
            own_texts::DATA_DIFFERS,
            99_110,
            "[CENTER]Ihre Spieldaten für diese Karte weichen vom Host ab: %s. Sie können erst mitspielen, wenn \
             die Dateien übereinstimmen.",
            "[CENTER]Your game data for this map differs from the host's: %s. You can only take part once the \
             files match.",
        ),
        (
            own_texts::ROW_DIFFERS,
            99_111,
            "[CENTER]Die Spieldaten von %s weichen ab: %s. Die Partie kann so nicht beginnen.",
            "[CENTER]%s's game data differs: %s. The game cannot start like this.",
        ),
        (
            own_texts::ROW_MAP_MISSING,
            99_112,
            "[CENTER]%s fehlt die Karte der Partie. Die Partie kann so nicht beginnen.",
            "[CENTER]%s does not have the game's map. The game cannot start like this.",
        ),
        (
            own_texts::LOAD_FAILED,
            99_113,
            "[CENTER]%s konnte die Partie nicht laden. Der Start wurde abgebrochen.",
            "[CENTER]%s could not load the game. The start was cancelled.",
        ),
        (
            own_texts::FILES_MORE,
            99_114,
            " und %d weitere",
            " and %d more",
        ),
        (
            own_texts::FILE_UNKNOWN,
            99_115,
            "Datei unbekannt",
            "file unknown",
        ),
        (
            own_texts::NET_STOPPED_SAVED,
            99_116,
            "Die Partie läuft nicht mehr synchron (Prüfsummenfehler). Sie wurde angehalten und als \"%s\" gespeichert.",
            "The match is no longer in sync (checksum error). It was stopped and saved as \"%s\".",
        ),
        (
            own_texts::NET_STOPPED_UNSAVED,
            99_117,
            "Die Partie läuft nicht mehr synchron (Prüfsummenfehler). Sie wurde angehalten und konnte nicht gespeichert werden.",
            "The match is no longer in sync (checksum error). It was stopped and could not be saved.",
        ),
        (
            own_texts::NET_HOST_ENDED_SAVED,
            99_118,
            "Der Host hat die Partie beendet. Sie wurde als \"%s\" gespeichert.",
            "The host has ended the match. It was saved as \"%s\".",
        ),
        (
            own_texts::NET_HOST_ENDED_UNSAVED,
            99_119,
            "Der Host hat die Partie beendet. Sie konnte nicht gespeichert werden.",
            "The host has ended the match. It could not be saved.",
        ),
        (
            own_texts::NET_HOST_LOST_SAVED,
            99_120,
            "Die Verbindung zum Host ist abgebrochen. Die Partie wurde als \"%s\" gespeichert.",
            "The connection to the host was aborted. The match was saved as \"%s\".",
        ),
        (
            own_texts::NET_HOST_LOST_UNSAVED,
            99_121,
            "Die Verbindung zum Host ist abgebrochen. Die Partie konnte nicht gespeichert werden.",
            "The connection to the host was aborted. The match could not be saved.",
        ),
        (
            own_texts::NET_HOST_LOST,
            99_122,
            "Die Verbindung zum Host ist abgebrochen.",
            "The connection to the host was aborted.",
        ),
        (
            own_texts::NET_FAILED,
            99_123,
            "Die Netzpartie wurde abgebrochen: %s",
            "The network match was aborted: %s",
        ),
        (own_texts::PLAYER, 99_124, "Spieler", "Player"),
        (own_texts::PLAYER_N, 99_125, "Spieler %d", "Player %d"),
        (own_texts::NET_SAVE_PREFIX, 99_126, "netz_", "net_"),
    ];
    for (id, number, de, en) in data {
        assert_eq!(id, number);
        assert_eq!(own_texts::text(id, German), Some(de));
        assert_eq!(own_texts::text(id, English), Some(en));
    }
    assert_eq!(own_texts::text(99_127, German), None);
    assert_eq!(own_texts::all(English).count(), 27);

    let mut texts = Texts::from_entries(&[(99_101, "Own")]);
    texts.add_own_texts(English);
    assert_eq!(texts.resolve(99_100), Some("Turn raster"));
    assert_eq!(texts.resolve(99_101), Some("Own"), "occupied number stays");
    assert_eq!(texts.resolve(99_102), Some("LAN"));
    // Without a detected edition (test VFS without the 11 archives) German.
    let l = Logic::new();
    assert_eq!(l.fixture.texts.resolve(99_100), Some("Rundenraster"));
    assert_eq!(l.texts(&[99_103]), ["LAN schnell"]);
}

#[test]
fn net_commands_go_to_the_host() {
    // `7`, `8`, `9`, `0x2D`, `0x2F` as `UiEffect::Net` to the host; plus the setters for the host.
    let mut app = MenuApp::from_library(
        crate::dialog::DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    );
    let cmds = [
        AppCmd::Join,
        AppCmd::Host,
        AppCmd::CloseNet,
        AppCmd::LeaveLobby,
        AppCmd::HostGone,
    ];
    for cmd in &cmds {
        app.push_command(cmd.clone());
    }
    let mut d = Driver::with_app(fixture(), app, Box::new(dispatch));
    d.idle();
    let effects = d.app.take_effects();
    let net: Vec<_> = effects
        .into_iter()
        .filter(|e| matches!(e, UiEffect::Net(_)))
        .collect();
    assert_eq!(net, cmds.map(UiEffect::Net));

    // Setters: network state, lobby draft, names with mirrors, message with follow-up command.
    let view = NetMenuView {
        games: vec![game(1, "A")],
        lobby: Some(LobbyView {
            is_host: true,
            ..LobbyView::default()
        }),
    };
    d.app.set_net(Some(view.clone()));
    assert_eq!(d.app.data().net, Some(view));
    let slots = vec![SlotDraft {
        key: -1,
        kind: 999,
        ..SlotDraft::default()
    }];
    let info = crate::draft::LevelInfoData {
        name: "Map".into(),
        ..Default::default()
    };
    let settings = crate::draft::DraftSettings {
        session: "Round".into(),
        ..Default::default()
    };
    d.app
        .set_lobby_draft(slots.clone(), info.clone(), settings.clone());
    assert_eq!(d.app.data().draft.slots, slots);
    assert_eq!(d.app.data().draft.info, info);
    assert_eq!(d.app.data().draft.settings, settings);
    let long: Vec<u16> = "Session - Anna with a very long name after it"
        .encode_utf16()
        .collect();
    d.app.set_net_session(&long);
    assert_eq!(
        name_text(&d.app.config().net_session),
        String::from_utf16_lossy(&long[..39])
    );
    assert_eq!(
        d.app.screen_state().net_session.get(),
        String::from_utf16_lossy(&long[..39])
    );
    d.app
        .set_net_name(&"Bert".encode_utf16().collect::<Vec<_>>());
    assert_eq!(name_text(&d.app.config().net_name), "Bert");
    assert_eq!(d.app.screen_state().net_name.get(), "Bert");
    d.app.tell(15212, Some(AppCmd::LeaveLobby));
    assert_eq!(d.app.screen_state().info_text, Some(15212));
    assert_eq!(d.app.screen_state().info_ok, Some(AppCmd::LeaveLobby));
    assert_eq!(d.app.queue().iter().last(), Some(&open("msgbox", true)));
}
