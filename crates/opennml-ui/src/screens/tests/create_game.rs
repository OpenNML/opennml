//! Tests of the own game (`CREATESINGLEPLAYERGAME`, `LOADMULTIPLAYERMAP`, `RANDOMMAP`) - per element the row of
//! the tables in re/singleplayer-menus part 2, 1 to 3. The logic alone runs through [`Logic`], live through `MenuApp`
//! with the corpus dialogs (these need `NML_GAME_DIR`).

use super::*;
use crate::app::tests::World;
use crate::coords::to_screen_rect;
use crate::dialog::FONT_BUTTON;
use crate::draft::tests::catalog;
use crate::draft::{LevelInfoData, SeedRand, SlotDraft, text_of};
use crate::input::MouseButton;
use crate::paint::paint_ui_with;
use crate::text::{CurLevelText, TextContext, text_height};

const DISABLE: i32 = 0x2000_0000;
pub(super) const CREATE: &str = "createsingleplayergame";
const MAP: &str = "loadmultiplayermap";

fn rqc(element: &str, code: u32) -> Requery {
    Requery::new(CREATE, element, code)
}

fn open_now(name: &str) -> AppCmd {
    open(name, false)
}

impl Logic {
    /// Logic with a small catalog: two `NORMAL` maps (2 and 4 slots), one `RAILROAD1` map.
    fn with_maps() -> Logic {
        let mut l = Logic::new();
        l.data.maps = catalog(&[
            ("First", "NORMAL", 2, 0, true),
            ("Second", "NORMAL", 4, 2, true),
            ("Track", "RAILROAD1", 3, 1, true),
        ]);
        l
    }

    fn status_of(&mut self, dialog: &str, element: &str) -> Option<i32> {
        self.status(dialog, element).1
    }
}

/// `MenuApp` with profile "Anna" and these settings, without a start sequence.
pub(super) fn live_game(config: EngineConfig) -> Option<Driver<Corpus>> {
    let config = EngineConfig {
        net_name: name_field("Anna"),
        ..config
    };
    let opts = MenuOptions {
        profiles: profiles_with(&["Anna"], 0),
        ..MenuOptions::default()
    };
    live_with(config, opts, Box::new(dispatch))
}

/// Frames until a new click no longer counts as a double click (`DoubleClickTimeout`).
pub(super) fn settle(d: &mut Driver<Corpus>) {
    let timeout = u64::try_from(d.app.config().double_click_timeout).unwrap_or(0);
    d.run_until(d.now + timeout);
}

/// Mouse click at a screen point in the next frame (after the double-click time has elapsed).
fn click_at(d: &mut Driver<Corpus>, (x, y): (i32, i32)) {
    settle(d);
    let now = d.now;
    d.app.input(UiInput::Move { x, y }, S1280, now);
    d.app.input(UiInput::Down(MouseButton::Left), S1280, now);
    d.app.input(UiInput::Up(MouseButton::Left), S1280, now);
}

/// Center of row `row` of a ListBox (row height `h(L"X")` of the font `MenuButton`, from the client top edge).
fn row_center(d: &Driver<Corpus>, list: crate::tree::NodeId, row: i32) -> (i32, i32) {
    let c = to_screen_rect(S1280, d.app.tree().client_rect(list));
    let fonts = &d.world.fonts;
    let lh = text_height("X", fonts.by_name(FONT_BUTTON), fonts);
    ((c[0] + c[2]) / 2, c[1] + lh * row + lh / 2)
}

/// Picks row `row` of the ComboBox `dialog/combo`: opens it, clicks the row, waits until idle.
pub(super) fn pick(d: &mut Driver<Corpus>, dialog: &str, combo: &str, row: i32) {
    settle(d);
    d.click(dialog, combo);
    d.frame();
    let key = format!("{}{}", dialog.to_uppercase(), combo.to_uppercase());
    let id = d.find(&key).unwrap_or_else(|| panic!("{key} is missing"));
    let NodeKind::Button(b) = &d.app.tree().node(id).kind else {
        panic!("{key} is not a ComboBox");
    };
    let popup = b.popup.expect("list open");
    let NodeKind::ComboPopup(p) = &d.app.tree().node(popup).kind else {
        panic!("no popup");
    };
    let at = row_center(d, p.list_box.expect("list"), row);
    click_at(d, at);
    d.idle();
}

/// Clicks row `row` of the map list.
pub(super) fn pick_map(d: &mut Driver<Corpus>, row: i32) {
    let list = d.find("LOADMULTIPLAYERMAPLEVELS").expect("map list");
    let at = row_center(d, list, row);
    click_at(d, at);
    d.idle();
}

/// Names of the maps for the game mode in the catalog of the live app.
fn map_names(d: &Driver<Corpus>, game_type: i32) -> Vec<String> {
    let maps = &d.app.data().maps;
    maps.filtered(game_type)
        .into_iter()
        .map(|i| maps.maps[i].name.clone())
        .collect()
}

#[test]
fn opening_create_game_opens_the_map_side_frame() {
    // `0x004D4E02`...`0x004D4EA1`: `LevelType > 1` -> 1, `< 0` -> return value 0 without a frame, otherwise
    // `{6, side frame}`.
    let mut l = Logic::new();
    for (lt, after, frame) in [
        (0, 0, MAP),
        (1, 1, "randommap"),
        (2, 1, "randommap"),
        (7, 1, "randommap"),
    ] {
        l.config.level_type = lt;
        let (r, msg) = l.send(CREATE, CREATE, code::QUERY_STATUS);
        assert_eq!(r, 1, "{lt}");
        assert!(!msg.params.contains_key("status"), "no status");
        assert_eq!(l.config.level_type, after);
        assert_eq!(l.take_queue(), [open_now(frame)], "{lt}");
    }
    l.config.level_type = -1;
    assert_eq!(l.send(CREATE, CREATE, code::QUERY_STATUS).0, 0);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.config.level_type, -1);

    // Live via the single-player menu: with `LevelType` 2 `RANDOMMAP` opens (LevelType 1), with -1 nothing.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, "singleplayermenu");
    d.click("singleplayermenu", CREATE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), [CREATE, MAP]);
    let Some(mut d) = live_game(EngineConfig {
        level_type: 2,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(d.app.menu().open_names(), [CREATE, "randommap"]);
    assert_eq!(d.app.config().level_type, 1);
    let Some(mut d) = live_game(EngineConfig {
        level_type: -1,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(d.app.menu().open_names(), [CREATE]);
}

// QUIRK O-128, O-187: kept
#[test]
fn game_type_filters_the_map_list_and_reselects_entry_0() {
    // O-128: `gametype` code 4 reopens `LOADMULTIPLAYERMAP` (`{3}`, `{5}`), the list is filtered by `GTYPE`,
    // code 0 selects entry 0 again.
    let mut l = Logic::with_maps();
    assert_eq!(l.items(MAP, "levels"), ["First", "Second"]);
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(0)));
    assert_eq!(l.data.level.map, Some(0));
    assert_eq!(l.send_int(MAP, "levels", code::ITEM_SELECTED, 1).0, 1);
    assert_eq!(l.data.level.map, Some(1));
    assert_eq!(l.requery, [Requery::new(MAP, "mappreview", 5)]);
    let (r, msg) = l.send(MAP, "mappreview", code::QUERY_STATUS);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("tga"),
        Some(&Param::Text(
            "data/levels/multiplayer/[4Players]-Second.tga".into()
        ))
    );
    assert!(!msg.params.contains_key("status"));
    // Outside the list: 0, no requery.
    l.requery.clear();
    assert_eq!(l.send_int(MAP, "levels", code::ITEM_SELECTED, 2).0, 0);
    assert!(l.requery.is_empty());
    assert_eq!(
        l.send(MAP, "levels", code::ITEM_SELECTED).0,
        0,
        "without int"
    );
    // Game mode 4: `{3}`, `{5}` for `loadmultiplayermap`, requeries `leveltype` 2, `timelimit` 5 (+ `create2`).
    l.requery.clear();
    assert_eq!(l.send_int(CREATE, "gametype", code::ITEM_SELECTED, 4).0, 1);
    assert_eq!(l.config.game_type, 4);
    assert_eq!(l.take_queue(), [close(MAP, true), open(MAP, true)]);
    assert_eq!(
        l.requery,
        [rqc("leveltype", 2), rqc("timelimit", 5), rqc("create2", 5)]
    );
    assert_eq!(l.items(MAP, "levels"), ["Track"]);
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(0)));
    assert_eq!(l.data.level.map, Some(2));
    // Without a matching map: code 0 fails (return value 0), the choice stays.
    l.config.game_type = 5;
    assert!(l.items(MAP, "levels").is_empty());
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (0, Some(0)));
    assert_eq!(l.data.level.map, Some(2));

    // Live: pick the third map, then game mode "Destroy railroad" - list anew, entry 0 again.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    let normal = map_names(&d, 0);
    let levels = element(&d, MAP, "levels");
    assert_eq!((levels.items, levels.selection), (normal.clone(), Some(0)));
    pick_map(&mut d, 2);
    assert_eq!(element(&d, MAP, "levels").selection, Some(2));
    let third = d.app.data().maps.filtered(0)[2];
    assert_eq!(d.app.data().level.map, Some(third));
    // O-187: the ComboBox `LEVELTYPE` keeps its old list after the switch to railroad (requery code 2 without effect);
    // only opening it requeries.
    assert_eq!(element(&d, CREATE, "leveltype").items.len(), 2);
    pick(&mut d, CREATE, "gametype", 4);
    assert_eq!(d.app.config().game_type, 4);
    assert_eq!(d.app.menu().open_names(), [CREATE, MAP]);
    let levels = element(&d, MAP, "levels");
    assert_eq!(
        (levels.items, levels.selection),
        (map_names(&d, 4), Some(0))
    );
    assert_ne!(map_names(&d, 4), normal);
    assert_eq!(
        d.app.data().level.map,
        Some(d.app.data().maps.filtered(4)[0])
    );
    assert_eq!(element(&d, CREATE, "leveltype").items.len(), 2, "O-187");
}

// QUIRK O-129: kept
#[test]
fn game_type_change_closes_the_random_map() {
    // O-129 corrected: The requery `leveltype` code 2 does not reach a ComboBox (`FUN_0045E8A0` takes only code 5),
    // `LevelType` stays 1; the "without random map" branch sets 0, `{3,"randommap"}`,
    // `{5,"loadmultiplayermap"}` and requeries `leveltype` code 5 - no second side frame.
    let mut l = Logic::with_maps();
    l.config.level_type = 1;
    assert_eq!(l.send_int(CREATE, "gametype", code::ITEM_SELECTED, 3).0, 1);
    assert_eq!(
        l.config.level_type, 1,
        "King of the Hill allows random maps"
    );
    assert!(l.take_queue().is_empty());
    l.requery.clear();
    assert_eq!(l.send_int(CREATE, "gametype", code::ITEM_SELECTED, 5).0, 1);
    assert_eq!(l.config.level_type, 0);
    assert_eq!(l.take_queue(), [close("randommap", true), open(MAP, true)]);
    assert_eq!(
        l.requery,
        [
            rqc("leveltype", 2),
            rqc("leveltype", 5),
            rqc("timelimit", 5),
            rqc("create2", 5)
        ]
    );

    let Some(mut d) = live_game(EngineConfig {
        level_type: 1,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(d.app.menu().open_names(), [CREATE, "randommap"]);
    pick(&mut d, CREATE, "gametype", 5);
    assert_eq!(d.app.config().level_type, 0);
    assert_eq!(d.app.menu().open_names(), [CREATE, MAP]);
    let leveltype = element(&d, CREATE, "leveltype");
    assert_eq!((leveltype.items.len(), leveltype.selection), (1, Some(0)));
}

/// `Engine.ini` with random map and game mode "Destroy railroad" (cannot be set via the menu, O-187).
fn saved_random_railroad() -> EngineConfig {
    EngineConfig {
        level_type: 1,
        game_type: 4,
        ..EngineConfig::default()
    }
}

// QUIRK O-187, O-188: kept
#[test]
fn saved_random_map_with_railroad_opens_the_random_frame_as_level_type_0() {
    // O-188 (remaining case): code 5 of the frame opens `RANDOMMAP` (`LevelType 1`), then `leveltype` code 2 silently
    // sets `LevelType` to 0 (order frame before elements; a hypothesis for the original). The state cannot be
    // reached via the menu: after the switch to railroad, opening `LEVELTYPE` requeries the list (O-187), "Create
    // random map" is missing, a click on the second row selects nothing new.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    pick(&mut d, CREATE, "gametype", 4);
    d.log.borrow_mut().clear();
    pick(&mut d, CREATE, "leveltype", 1);
    assert!(
        d.log
            .borrow()
            .contains(&(CREATE.into(), "leveltype".into(), code::QUERY_ITEMS)),
        "opening requeries code 2"
    );
    assert_eq!(d.app.config().level_type, 0);
    assert_eq!(d.app.menu().open_names(), [CREATE, MAP]);

    // Opened with these settings: `RANDOMMAP` open, `LevelType` 0, `CREATE2` (last element of the `.dlg`, queried with
    // `LevelType 0`) visible.
    let Some(mut d) = live_game(saved_random_railroad()) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(d.app.menu().open_names(), [CREATE, "randommap"]);
    assert_eq!(d.app.config().level_type, 0);
    assert!(!element(&d, CREATE, "create2").hidden);

    // No map selected yet (`LOADMULTIPLAYERMAP` was never open): `CREATE2` only closes side frame[0], `RANDOMMAP` stays
    // open via `SINGLEPLAYERSTART`, the draft has only the own slot and an empty path.
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(
        d.app.menu().open_names(),
        ["randommap", "singleplayerstart", "levelinfo"]
    );
    let draft = &d.app.data().draft;
    assert_eq!(draft.map, None);
    assert_eq!(draft.info.path, "");
    assert_eq!(draft.slots.len(), 1);
    assert_eq!((draft.slots[0].key, draft.slots[0].kind), (-1, 999));

    // A game mode change opens `LOADMULTIPLAYERMAP` in the "LevelType 0" branch as well: two side frames.
    let Some(mut d) = live_game(saved_random_railroad()) else {
        return;
    };
    show(&mut d, CREATE);
    pick(&mut d, CREATE, "gametype", 5);
    assert_eq!(d.app.menu().open_names(), [CREATE, "randommap", MAP]);
}

#[test]
fn random_map_only_for_game_types_0_to_3() {
    // `leveltype` code 2: 15520 (0), 15521 (1) only for `FUN_004B98E0` (0 <= GameType < 4), otherwise `LevelType`
    // 1 -> 0; 15522 only on the network, in single player `LevelType` 2 -> 0.
    let mut l = Logic::new();
    let all = l.texts(&[15520, 15521]);
    for gt in -1..7 {
        l.config.game_type = gt;
        l.config.level_type = 1;
        let (r, msg) = l.send(CREATE, "leveltype", code::QUERY_ITEMS);
        assert_eq!(r, 1);
        let random = (0..4).contains(&gt);
        let n = if random { 2 } else { 1 };
        assert_eq!(
            msg.params.get("array"),
            Some(&Param::Array(all[..n].to_vec())),
            "{gt}"
        );
        let values: Vec<i32> = (0..i32::try_from(n).unwrap()).collect();
        assert_eq!(
            msg.params.get("indexarray"),
            Some(&Param::IndexArray(values))
        );
        assert_eq!(l.config.level_type, i32::from(random), "{gt}");
    }
    l.config.game_type = 0;
    l.config.level_type = 2;
    l.send(CREATE, "leveltype", code::QUERY_ITEMS);
    assert_eq!(l.config.level_type, 0);
    assert_eq!(
        l.query(CREATE, "leveltype", code::QUERY_VALUE),
        (1, Some(0))
    );
    // Code 5 without an answer; code 4: frame change and nine requeries.
    assert_eq!(l.send(CREATE, "leveltype", code::QUERY_STATUS).0, -1);
    assert_eq!(l.send_int(CREATE, "leveltype", code::ITEM_SELECTED, 1).0, 1);
    assert_eq!(l.config.level_type, 1);
    assert_eq!(l.take_queue(), [close(MAP, true), open("randommap", true)]);
    let names = [
        "savereplay",
        "lockteams",
        "mapexplored",
        "gametype",
        "unitlimit",
        "resourcen",
        "timelimit",
        "gametype",
        "create2",
    ];
    assert_eq!(l.requery, names.map(|n| rqc(n, 5)));
    l.requery.clear();
    assert_eq!(l.send_int(CREATE, "leveltype", code::ITEM_SELECTED, 1).0, 1);
    assert!(l.take_queue().is_empty(), "same value: no frame change");
    assert_eq!(l.requery.len(), 9);
    assert_eq!(l.send(CREATE, "leveltype", code::ITEM_SELECTED).0, 0);
}

/// The six elements of `RANDOMMAP` (`.dlg` order).
const RANDOM_ELEMENTS: [&str; 6] = [
    "seed",
    "mapsize",
    "players",
    "terrain",
    "landscape",
    "daytime",
];

// QUIRK O-140: deviation
#[test]
fn random_map_page_writes_the_settings() {
    // `FUN_004D7760`: no element has a code 5 branch (-1, no `status`, all free); code 0/2 show fields and lists,
    // code 4 writes them (`players` `0x004D7A39` without a check, `terrain` `0x004D7B72`, `landscape` `0x004D7C31`,
    // `daytime` `0x004D7CF8`).
    let mut l = Logic::new();
    for e in RANDOM_ELEMENTS {
        assert_eq!(l.status("randommap", e), (-1, None), "{e}");
    }
    assert_eq!(l.send("randommap", "randommap", code::QUERY_STATUS).0, -1);
    assert_eq!(
        l.items("randommap", "mapsize"),
        l.texts(&[15450, 15451, 15452])
    );
    for (size, n) in [(0, 1), (1, 3), (2, 7)] {
        l.config.random_map_size = size;
        let ids: Vec<i32> = (15460..15460 + n).collect();
        assert_eq!(l.items("randommap", "players"), l.texts(&ids));
    }
    for (e, v) in [
        ("players", 2),
        ("terrain", 3),
        ("landscape", 1),
        ("daytime", 2),
    ] {
        assert_eq!(
            l.send_int("randommap", e, code::ITEM_SELECTED, v).0,
            1,
            "{e}"
        );
        assert_eq!(
            l.query("randommap", e, code::QUERY_VALUE),
            (1, Some(v)),
            "{e}"
        );
    }
    let c = &l.config;
    assert_eq!(
        (
            c.random_players,
            c.random_terrain,
            c.random_landscape,
            c.random_daytime
        ),
        (2, 3, 1, 2)
    );
    // `players` writes without a check, also above the limit of the size; only `mapsize` requeries.
    l.config.random_map_size = 0;
    assert_eq!(
        l.send_int("randommap", "players", code::ITEM_SELECTED, 6).0,
        1
    );
    assert_eq!(l.config.random_players, 6);
    assert!(l.requery.is_empty(), "{:?}", l.requery);
    // Without `int` no change; unknown elements without an answer.
    assert_eq!(l.send("randommap", "terrain", code::ITEM_SELECTED).0, 0);
    assert_eq!(l.config.random_terrain, 3);
    assert_eq!(l.click("randommap", "cancel"), -1);
    // `CREATE2` is visible with a random map and triggers.
    l.config.level_type = 1;
    assert_eq!(l.status_of(CREATE, "create2"), Some(0));
    assert_eq!(l.click(CREATE, "create2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(CREATE, true),
            close("randommap", false),
            open("singleplayerstart", true),
            open("levelinfo", true)
        ]
    );

    // Live: "Create random map" - map list closed, `RANDOMMAP` open and free, `CREATE2` visible; the lists write the
    // settings.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    pick(&mut d, CREATE, "leveltype", 1);
    assert_eq!(d.app.config().level_type, 1);
    assert_eq!(d.app.menu().open_names(), [CREATE, "randommap"]);
    for e in RANDOM_ELEMENTS {
        assert!(element(&d, "randommap", e).enabled, "{e}");
    }
    assert!(!element(&d, CREATE, "create2").hidden);
    pick(&mut d, "randommap", "terrain", 1);
    pick(&mut d, "randommap", "landscape", 1);
    pick(&mut d, "randommap", "daytime", 1);
    let c = d.app.config();
    assert_eq!(
        (c.random_terrain, c.random_landscape, c.random_daytime),
        (1, 1, 1)
    );
    // Back to the predefined map: `CREATE2` stays visible.
    pick(&mut d, CREATE, "leveltype", 0);
    assert_eq!(d.app.menu().open_names(), [CREATE, MAP]);
    assert!(!element(&d, CREATE, "create2").hidden);
}

#[test]
fn mapsize_clamps_players() {
    // `mapsize` code 4 (`0x004D78B3`...`0x004D7950`): set the size, clamp `RandomPlayers` to at most {0, 2, 6}[size]
    // (never raise), then code 5 to `players` (the ComboBox requeries code 2 and 0).
    let mut l = Logic::new();
    l.config.random_players = 6;
    assert_eq!(
        l.send_int("randommap", "mapsize", code::ITEM_SELECTED, 1).0,
        1
    );
    assert_eq!((l.config.random_map_size, l.config.random_players), (1, 2));
    assert_eq!(
        l.requery,
        [Requery::new("randommap", "players", code::QUERY_STATUS)]
    );
    l.requery.clear();
    for (size, before, after) in [(0, 2, 0), (2, 0, 0), (2, 5, 5), (1, 1, 1), (0, 1, 0)] {
        l.config.random_players = before;
        l.send_int("randommap", "mapsize", code::ITEM_SELECTED, size);
        assert_eq!(l.config.random_map_size, size);
        assert_eq!(l.config.random_players, after, "{size}/{before}");
    }
    // Size outside 0...2 (only from a changed `Engine.ini`): in the original a stack value as the limit, the
    // reimplementation does not clamp.
    l.config.random_players = 5;
    l.send_int("randommap", "mapsize", code::ITEM_SELECTED, 3);
    assert_eq!((l.config.random_map_size, l.config.random_players), (3, 5));

    // Live: large with 8 players (row 6), then small - the player list has one entry, row 0 selected.
    let Some(mut d) = live_game(EngineConfig {
        level_type: 1,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(d.app.menu().open_names(), [CREATE, "randommap"]);
    pick(&mut d, "randommap", "mapsize", 2);
    assert_eq!(element(&d, "randommap", "players").items.len(), 7);
    pick(&mut d, "randommap", "players", 6);
    assert_eq!(d.app.config().random_players, 6);
    pick(&mut d, "randommap", "mapsize", 0);
    assert_eq!(
        (
            d.app.config().random_map_size,
            d.app.config().random_players
        ),
        (0, 0)
    );
    let players = element(&d, "randommap", "players");
    assert_eq!((players.items.len(), players.selection), (1, Some(0)));
}

#[test]
fn seed_field_keeps_the_typed_text() {
    // `seed` code 7 (`0x004D77D0`...`0x004D7836`): pointer to `RandomMapSeed`, `size 30`, `inputbuffer 1` - the field
    // writes the typed text into the setting (mirror like `SESSIONNAME`: code `0xC` transfers it before the handler,
    // which itself answers -1). A copy would lose the text.
    let mut l = Logic::new();
    l.config.random_map_seed = name_field("4711");
    let (r, msg) = l.send("randommap", "seed", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.random_map_seed.clone()))
    );
    assert_eq!(l.state.random_map_seed.get(), "4711");
    assert_eq!(int_of(&msg, "size"), Some(30));
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    l.state.random_map_seed.set("4711Z");
    assert_eq!(l.send("randommap", "seed", code::EDIT_CHANGED).0, -1);
    assert_eq!(name_text(&l.config.random_map_seed), "4711Z");

    // Live: type, then `CREATE2` - the seed is in the setting and in `Info`.
    let Some(mut d) = live_game(EngineConfig {
        level_type: 1,
        random_map_seed: name_field("Ab"),
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(element(&d, "randommap", "seed").text.as_deref(), Some("Ab"));
    d.click("randommap", "seed");
    d.frame();
    type_text(&mut d, "C");
    assert_eq!(name_text(&d.app.config().random_map_seed), "AbC");
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(
        d.app.menu().open_names(),
        ["singleplayerstart", "levelinfo"]
    );
    assert_eq!(d.app.data().draft.info.name, "AbC");
    assert_eq!(name_text(&d.app.config().random_map_seed), "AbC");
}

// QUIRK O-140: deviation
#[test]
fn create2_builds_random_map_slots() {
    // `FUN_004BF1F0(0)` with `LevelType 1` (`0x004BF241`...`0x004BF3C8`): `Info` = seed, size, players + 2,
    // map type, landscape, time of day, path unchanged; slots `i = 0 ... n-1` (name "", type 0, key i - 1,
    // nation 0, team i, color i, handicap 0), then the own one (key -1): `NetName`, type 999, `Handicap`.
    let mut l = Logic::with_maps();
    l.config.level_type = 1;
    l.config.game_type = 1;
    l.config.random_map_seed = name_field("Seed");
    l.config.random_map_size = 1;
    l.config.random_players = 1;
    l.config.random_terrain = 2;
    l.config.random_landscape = 3;
    l.config.random_daytime = 1;
    l.config.net_name = name_field("Bert");
    l.config.handicap = 2;
    // Last selected fixed map: its path stays in `Info` (`Info+0x50` untouched).
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(0)));
    let old_path = l.data.level.info.path.clone();
    assert!(!old_path.is_empty());
    assert_eq!(l.click(CREATE, "create2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(CREATE, true),
            close("randommap", false),
            open("singleplayerstart", true),
            open("levelinfo", true)
        ]
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
                name: "Bert".into(),
                kind: 999,
                handicap: 2,
                ..open_slot(0)
            },
            open_slot(1),
            open_slot(2)
        ]
    );
    assert_eq!(draft.settings.game_type, 1);
    // A second `CREATE2` rebuilds (no duplication); a seed of spaces counts as set (`wcslen`).
    l.config.random_map_seed = name_field(" ");
    l.config.random_players = 6;
    l.config.random_map_size = 2;
    l.click(CREATE, "create2");
    assert_eq!(l.data.draft.slots.len(), 8);
    assert_eq!(l.data.draft.info.name, " ");
    assert_eq!(l.data.draft.info.players, 8);
    assert_eq!(l.data.seed_rand, SeedRand::default(), "nothing drawn");

    // Live: random map with 3 players, create - `SINGLEPLAYERSTART` and `LEVELINFO` open, three slots.
    let Some(mut d) = live_game(EngineConfig {
        level_type: 1,
        random_map_size: 1,
        random_players: 1,
        random_map_seed: name_field("Seed"),
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(
        d.app.menu().open_names(),
        ["singleplayerstart", "levelinfo"]
    );
    let draft = &d.app.data().draft;
    assert_eq!(draft.slots.len(), 3);
    assert_eq!((draft.info.level_type, draft.info.players), (1, 3));
    let own = &draft.slots[0];
    assert_eq!((own.key, own.name.as_str(), own.kind), (-1, "Anna", 999));
}

// QUIRK O-140: deviation
#[test]
fn empty_seed_gives_41_then_18467() {
    // O-140: an empty seed is drawn at every `CREATE2` from the menu's own MSVC state (`swprintf(L"%d", rand())`,
    // `0x004BF261`; start 1 at program start, not saved): first "41", then "18467". The value is only in `Info`,
    // `SEED` stays empty.
    let mut l = Logic::with_maps();
    l.config.level_type = 1;
    assert_eq!(l.click(CREATE, "create2"), 1);
    assert_eq!(l.data.draft.info.name, "41");
    assert_eq!(name_text(&l.config.random_map_seed), "");
    l.click(CREATE, "create2");
    assert_eq!(l.data.draft.info.name, "18467");
    assert_eq!(name_text(&l.config.random_map_seed), "");

    // Live: the first empty seed after program start is "41".
    let Some(mut d) = live_game(EngineConfig {
        level_type: 1,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(d.app.data().draft.info.name, "41");
    assert_eq!(name_text(&d.app.config().random_map_seed), "");
}

#[test]
fn domination_shows_create2() {
    // Game mode 2 "Domination" is in the list and `CREATE2` is free (O-134), also with the random map
    // (`LevelType 1`).
    let mut l = Logic::with_maps();
    let names = l.texts(&[15540, 15541, 15542, 15543, 15544, 15545]);
    assert_eq!(l.items(CREATE, "gametype"), names);
    for gt in [0, 1, 2, 3, 4, 5] {
        l.config.game_type = gt;
        assert_eq!(l.status_of(CREATE, "create2"), Some(0), "{gt}");
    }
    // The random map with Domination triggers as well.
    l.config.game_type = 2;
    l.config.level_type = 1;
    assert_eq!(l.status_of(CREATE, "create2"), Some(0));
    assert_eq!(l.click(CREATE, "create2"), 1);
    assert!(l.take_queue().contains(&open("singleplayerstart", true)));

    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    pick(&mut d, CREATE, "gametype", 2);
    assert!(!element(&d, CREATE, "create2").hidden);
    // The random map (selectable with game mode 2) shows `CREATE2` as well.
    pick(&mut d, CREATE, "leveltype", 1);
    assert!(!element(&d, CREATE, "create2").hidden);
    pick(&mut d, CREATE, "leveltype", 0);
    assert!(!element(&d, CREATE, "create2").hidden);
    pick(&mut d, CREATE, "gametype", 3);
    assert!(!element(&d, CREATE, "create2").hidden);
}

#[test]
fn domination_starts_the_match_dialogs() {
    // With game mode 2 `CREATE2` triggers like with any other game mode: dialog closed, `singleplayerstart` and
    // `levelinfo` open.
    let mut l = Logic::with_maps();
    l.config.game_type = 2;
    l.config.level_type = 0;
    l.data.level.map = Some(0);
    assert_eq!(l.click(CREATE, "create2"), 1);
    let queue = l.take_queue();
    assert!(
        queue.contains(&open("singleplayerstart", true))
            && queue.contains(&open("levelinfo", true)),
        "{queue:?}"
    );
}

#[test]
fn domination_frees_the_time_limit_and_shows_info_line_11() {
    // re/domination V12: with game mode 2 `TIMELIMIT` is free (`1 < game type < 5` and != 5), the menu still clamps to
    // 1...999, and info line 11 (15655 "Time limit: ") shows "n minutes".
    let mut l = Logic::with_maps();
    l.config.game_type = 2;
    assert_eq!(l.status_of(CREATE, "timelimit"), Some(0));
    l.config.time_limit = 5000;
    l.send(CREATE, "timelimit", code::QUERY_TEXT);
    assert_eq!(l.config.time_limit, 999, "clamping stays 1...999");
    l.config.time_limit = 45;
    l.send(CREATE, "timelimit", code::QUERY_TEXT);
    assert_eq!(l.state.time_limit_text.get(), "45");

    // `CREATE2` copies game mode and time limit into the session; `[CURLEVELINFOS]` carries line 11 (15655).
    l.config.level_type = 0;
    l.data.level.map = Some(0);
    assert_eq!(l.click(CREATE, "create2"), 1);
    assert_eq!(l.data.draft.settings.game_type, 2);
    assert_eq!(l.data.draft.settings.time_limit, 45);
    let texts = &l.fixture.texts;
    let infos = crate::draft::level_infos(&l.data.draft, None, texts);
    let value = format!("{} {}", 45, text_of(texts, 15674));
    assert!(infos.contains(&value), "{infos}");
    assert!(infos.contains(&text_of(texts, 15655)), "{infos}");

    let Some(mut d) = live_game(EngineConfig {
        time_limit: 45,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert!(!element(&d, CREATE, "timelimit").enabled);
    pick(&mut d, CREATE, "gametype", 2);
    assert!(element(&d, CREATE, "timelimit").enabled);
    assert_eq!(element(&d, CREATE, "timelimit").text.as_deref(), Some("45"));
}

// QUIRK O-127: kept
#[test]
fn unit_limit_offers_75_to_150() {
    // O-127: four texts 15531...15534 (without 15530 "50"); index = value; `RESOURCEN` 15550...15553.
    let mut l = Logic::new();
    assert_eq!(
        l.items(CREATE, "unitlimit"),
        l.texts(&[15531, 15532, 15533, 15534])
    );
    assert_eq!(
        l.items(CREATE, "resourcen"),
        l.texts(&[15550, 15551, 15552, 15553])
    );
    l.config.unit_limit = 3;
    assert_eq!(
        l.query(CREATE, "unitlimit", code::QUERY_VALUE),
        (1, Some(3))
    );
    assert_eq!(l.send_int(CREATE, "unitlimit", code::ITEM_SELECTED, 0).0, 1);
    assert_eq!(l.config.unit_limit, 0);
    assert_eq!(l.send_int(CREATE, "resourcen", code::ITEM_SELECTED, 2).0, 1);
    assert_eq!(l.config.resourcen, 2);
    assert_eq!(
        l.send(CREATE, "resourcen", code::ITEM_SELECTED).0,
        1,
        "without int"
    );
    assert_eq!(l.config.resourcen, 2);
    // Switches: code 0/1 onto the settings, status `DISABLE` only with `LevelType 2`.
    for (e, set) in [("savereplay", 1), ("lockteams", 1), ("mapexplored", 1)] {
        assert_eq!(l.send_int(CREATE, e, code::VALUE_CHANGED, set).0, 1);
        assert_eq!(l.query(CREATE, e, code::QUERY_VALUE), (1, Some(1)), "{e}");
    }
    assert_eq!(
        (
            l.config.save_replay,
            l.config.lock_teams,
            l.config.map_explored
        ),
        (1, 1, 1)
    );
    for e in [
        "savereplay",
        "lockteams",
        "mapexplored",
        "gametype",
        "unitlimit",
        "resourcen",
    ] {
        l.config.level_type = 0;
        assert_eq!(l.status_of(CREATE, e), Some(0), "{e}");
        l.config.level_type = 2;
        assert_eq!(l.status_of(CREATE, e), Some(DISABLE), "{e}");
    }

    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    let unit = element(&d, CREATE, "unitlimit");
    assert_eq!(unit.items.len(), 4);
    assert_eq!(unit.selection, Some(2), "default UnitLimit 2");
    assert_eq!(unit.items[0], text_of(&d.world.texts, 15531));
}

#[test]
fn time_limit_free_only_for_types_2_to_4() {
    // `0x004D5977`...`0x004D59AC`: `DISABLE` with `LevelType 2`, `GameType < 2` or `GameType 5`.
    let mut l = Logic::new();
    for (gt, free) in [
        (-1, false),
        (0, false),
        (1, false),
        (2, true),
        (3, true),
        (4, true),
        (5, false),
        (6, true),
    ] {
        l.config.game_type = gt;
        let expected = if free { 0 } else { DISABLE };
        assert_eq!(l.status_of(CREATE, "timelimit"), Some(expected), "{gt}");
    }
    l.config.game_type = 3;
    l.config.level_type = 2;
    assert_eq!(l.status_of(CREATE, "timelimit"), Some(DISABLE));

    // Live: locked with Deathmatch, free after the switch to King of the Hill (requery of `gametype`).
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    assert!(!element(&d, CREATE, "timelimit").enabled);
    pick(&mut d, CREATE, "gametype", 3);
    assert!(element(&d, CREATE, "timelimit").enabled);
}

// QUIRK O-131: kept
#[test]
fn time_limit_filter_accepts_digits_only_and_swallows_escape() {
    // O-131: code `0xB` without `key` -1, digits -1 (insert), every other character 1 - also ESC (27) and RETURN.
    let mut l = Logic::new();
    assert_eq!(l.send(CREATE, "timelimit", code::EDIT_KEY).0, -1);
    let key = |l: &mut Logic, k: i32| {
        let mut msg = ElementMsg::new(CREATE, "timelimit", code::EDIT_KEY);
        msg.params.insert("key".into(), Param::Int(k));
        l.deliver(msg).0
    };
    for k in '0'..='9' {
        assert_eq!(key(&mut l, k as i32), -1, "{k}");
    }
    for k in ['a', '/', ':', ' ', '-'] {
        assert_eq!(key(&mut l, k as i32), 1, "{k}");
    }
    assert_eq!(key(&mut l, 27), 1);
    assert_eq!(key(&mut l, 13), 1);

    // Live (King of the Hill, field free): "a" discarded, digits up to the field size (three characters with
    // `size 4`), ESC swallowed without consequences.
    let Some(mut d) = live_game(EngineConfig {
        game_type: 3,
        time_limit: 5,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(element(&d, CREATE, "timelimit").text.as_deref(), Some("5"));
    d.click(CREATE, "timelimit");
    d.frame();
    type_text(&mut d, "a12x34");
    assert_eq!(
        element(&d, CREATE, "timelimit").text.as_deref(),
        Some("512")
    );
    assert_eq!(d.app.config().time_limit, 512);
    d.log.borrow_mut().clear();
    d.key(ESCAPE);
    d.idle();
    assert!(
        d.log
            .borrow()
            .contains(&(CREATE.into(), "timelimit".into(), code::EDIT_KEY)),
        "ESC broadcast at the field"
    );
    assert_eq!(d.app.menu().open_names(), [CREATE, MAP]);
    assert_eq!(
        element(&d, CREATE, "timelimit").text.as_deref(),
        Some("512")
    );
}

// QUIRK O-130: kept
#[test]
fn time_limit_clamps_on_query_only() {
    // O-130: code 7 clamps `TimeLimit` to 1...999 (written back) and shows the buffer; code `0xC` takes the text with
    // `swscanf` unclamped upward, `<= 0` -> 1; without a number the value stays (then <= 0 -> 1).
    let mut l = Logic::new();
    for (value, shown) in [(5000, 999), (0, 1), (-3, 1), (42, 42)] {
        l.config.time_limit = value;
        let (r, msg) = l.send(CREATE, "timelimit", code::QUERY_TEXT);
        assert_eq!(r, 1);
        assert_eq!(l.config.time_limit, shown);
        assert_eq!(
            msg.params.get("text"),
            Some(&Param::Shared(l.state.time_limit_text.clone()))
        );
        assert_eq!(l.state.time_limit_text.get(), shown.to_string());
        assert_eq!(int_of(&msg, "size"), Some(4));
        assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    }
    let change = |l: &mut Logic, text: &str| {
        let mut msg = ElementMsg::new(CREATE, "timelimit", code::EDIT_CHANGED);
        msg.params.insert("text".into(), Param::Text(text.into()));
        l.deliver(msg).0
    };
    for (text, before, after) in [
        ("1234", 20, 1234),
        ("0", 20, 1),
        ("", 50, 50),
        ("", 0, 1),
        (" 77x", 20, 77),
        ("-5", 20, 1),
    ] {
        l.config.time_limit = before;
        assert_eq!(change(&mut l, text), -1);
        assert_eq!(l.config.time_limit, after, "{text:?}");
    }
    l.config.time_limit = -4;
    assert_eq!(l.send(CREATE, "timelimit", code::EDIT_CHANGED).0, -1);
    assert_eq!(l.config.time_limit, -4, "without text no check");

    // Live: `TimeLimit` 5000 from the settings file shows 999; deleting until empty leaves the value standing.
    let Some(mut d) = live_game(EngineConfig {
        game_type: 3,
        time_limit: 5000,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    assert_eq!(
        element(&d, CREATE, "timelimit").text.as_deref(),
        Some("999")
    );
    assert_eq!(d.app.config().time_limit, 999);
    d.click(CREATE, "timelimit");
    d.frame();
    for _ in 0..3 {
        d.key(0x08);
        d.frame();
    }
    assert_eq!(element(&d, CREATE, "timelimit").text.as_deref(), Some(""));
    assert_eq!(
        d.app.config().time_limit,
        9,
        "\"9\" remained from the last character before"
    );
}

// QUIRK O-132: kept
#[test]
fn session_name_edits_net_session() {
    // O-132: `SESSIONNAME` also exists in single player and writes `NetSession` (pointer, `size 30`).
    let mut l = Logic::new();
    l.config.net_session = name_field("Round");
    let (r, msg) = l.send(CREATE, "sessionname", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.net_session.clone()))
    );
    assert_eq!(l.state.net_session.get(), "Round");
    assert_eq!(int_of(&msg, "size"), Some(30));
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));

    let Some(mut d) = live_game(EngineConfig {
        net_session: name_field("Round"),
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "sessionname");
    d.frame();
    type_text(&mut d, "X");
    assert_eq!(name_text(&d.app.config().net_session), "RoundX");
}

// QUIRK O-132: kept
#[test]
fn create2_copies_settings_into_the_draft() {
    // `create2` code 8: `{3, dialog}`, `{4, side frame}`, `FUN_004BF1F0(0)` (slots from the map, own slot type 999 with
    // `NetName` and `Handicap`), `FUN_004B9700` (settings), `{5,"singleplayerstart"}`, `{5,"levelinfo"}`.
    let mut l = Logic::with_maps();
    l.config.game_type = 3;
    l.config.unit_limit = 1;
    l.config.resourcen = 2;
    l.config.time_limit = 30;
    l.config.lock_teams = 1;
    l.config.save_replay = 1;
    l.config.net_session = name_field("Round");
    l.config.net_name = name_field("Bert");
    l.config.handicap = 3;
    l.data.maps.maps[1].entry.kind = "KING_OF_THE_HILL".into();
    assert_eq!(l.query(MAP, "levels", code::QUERY_VALUE), (1, Some(0)));
    assert_eq!(l.data.level.map, Some(1));
    assert_eq!(l.click(CREATE, "create2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(CREATE, true),
            close(MAP, false),
            open("singleplayerstart", true),
            open("levelinfo", true)
        ]
    );
    let draft = &l.data.draft;
    assert_eq!(draft.map, Some(1));
    assert_eq!(
        draft.info,
        LevelInfoData {
            name: "Second".into(),
            path: "data/levels/multiplayer/[4Players]-Second.lvl".into(),
            size_class: 2,
            players: 4,
            level_type: 0,
            ..LevelInfoData::default()
        }
    );
    let mut slots = l.data.maps.maps[1].slots.clone();
    slots[0] = SlotDraft {
        name: "Bert".into(),
        kind: 999,
        handicap: 3,
        ..slots[0].clone()
    };
    assert_eq!(draft.slots, slots);
    assert_eq!((slots[0].key, slots[0].team, slots[0].color), (-1, 0, 7));
    assert_eq!(slots[1].name, "EMPTY");
    let s = &draft.settings;
    assert_eq!(
        (s.game_type, s.unit_limit, s.resources, s.time_limit),
        (3, 1, 2, 30)
    );
    assert_eq!(
        (s.lock_teams, s.map_explored, s.save_replay),
        (true, false, true)
    );
    assert_eq!(s.session, "Round");
    // A second `CREATE2` rebuilds the slots (no duplication).
    l.click(CREATE, "create2");
    assert_eq!(l.data.draft.slots.len(), 4);
    // If only the `.wld` is missing, slots and player count are already there (`FUN_004BDDA0` creates them when
    // reading the `.lvl`), the size stays.
    l.data.maps.maps[1].world_read = false;
    l.data.level.info.size_class = 9;
    l.click(CREATE, "create2");
    assert_eq!(l.data.draft.slots.len(), 4);
    assert_eq!(
        (l.data.draft.info.players, l.data.draft.info.size_class),
        (4, 9)
    );
    l.data.maps.maps[1].level_read = false;
    l.click(CREATE, "create2");
    assert_eq!(
        l.data.draft.slots.len(),
        1,
        "without .lvl only the own slot"
    );
    // Without a selected map: only the own slot (hypothesis about the uninitialized buffer of the original).
    let mut l = Logic::new();
    l.config.net_name = name_field("Cleo");
    l.click(CREATE, "create2");
    assert_eq!(
        l.data.draft.slots,
        [SlotDraft {
            key: -1,
            name: "Cleo".into(),
            kind: 999,
            ..SlotDraft::default()
        }]
    );

    // Live: second map, create - `SINGLEPLAYERSTART` and `LEVELINFO` open, slots from the map.
    let Some(mut d) = live_game(EngineConfig {
        handicap: 1,
        ..EngineConfig::default()
    }) else {
        return;
    };
    show(&mut d, CREATE);
    pick_map(&mut d, 1);
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(
        d.app.menu().open_names(),
        ["singleplayerstart", "levelinfo"]
    );
    let data = d.app.data();
    let index = data.maps.filtered(0)[1];
    let map = &data.maps.maps[index];
    assert_eq!(data.draft.map, Some(index));
    assert_eq!(data.draft.info.name, map.name);
    assert_eq!(data.draft.slots.len(), map.slots.len());
    let own = &data.draft.slots[0];
    assert_eq!(
        (own.key, own.name.as_str(), own.kind, own.handicap),
        (-1, "Anna", 999, 1)
    );
    assert_eq!(data.draft.settings.game_type, 0);
    assert_eq!(data.draft.settings.unit_limit, 2);
}

#[test]
fn exit2_returns_to_singleplayer_menu() {
    // `0x004D529E`...`0x004D5316`: `{3, dialog}`, `{4, side frame}` (only 0...2), `{5,"singleplayermenu"}`.
    let mut l = Logic::new();
    assert_eq!(l.click(CREATE, "exit2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(CREATE, true),
            close(MAP, false),
            open("singleplayermenu", true)
        ]
    );
    l.config.level_type = 5;
    l.click(CREATE, "exit2");
    assert_eq!(
        l.take_queue(),
        [close(CREATE, true), open("singleplayermenu", true)]
    );
    assert_eq!(l.click(CREATE, "nowhere"), -1);

    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "exit2");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["singleplayermenu"]);
}

#[test]
fn levelinfo_placeholders_show_map_name_size_players() {
    // `FUN_004BFAE0` sets name, size (`15450 + class`) and players (`15458 + number`); `LEVELINFO` (text 15581) shows
    // them via `[CURLEVELNAME]`, `[CURLEVELSIZE]`, `[CURLEVELPLAYERS]`. A map choice shows in the same frame.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    let texts = &d.world.texts;
    let first = &d.app.data().maps.maps[d.app.data().maps.filtered(0)[0]];
    let players = i32::try_from(first.slots.len()).unwrap();
    let expected = CurLevelText {
        name: first.name.clone(),
        size: text_of(texts, 15450 + first.size_class),
        players: text_of(texts, 15458 + players),
        infos: String::new(),
    };
    assert_eq!(d.app.data().level.text, Some(expected.clone()));
    // `[CURLEVELINFOS]` comes from the session (`FUN_004B9C90`), not from the selected map.
    let expected = CurLevelText {
        infos: crate::draft::level_infos(&d.app.data().draft, None, texts),
        ..expected
    };
    assert_eq!(
        d.app.text_context().and_then(|c| c.cur_level.clone()),
        Some(expected.clone())
    );
    // The label's typesetting inserts the texts: without map data fewer glyphs.
    let now = d.now - crate::app::tests::DT;
    let with = d.app.text_context().cloned().unwrap();
    let without = TextContext {
        cur_level: None,
        ..with.clone()
    };
    let quads = |d: &mut Driver<Corpus>, t: &TextContext| {
        let mut ctx = d.world.ctx(S1280);
        paint_ui_with(d.app.tree(), &mut ctx, now, Some(t), None).len()
    };
    let (a, b) = (quads(&mut d, &with), quads(&mut d, &without));
    let glyphs = expected.name.chars().filter(|c| *c != ' ').count();
    assert!(a >= b + glyphs, "{a} commands with, {b} without map data");
    // Another map: the frame that handles the click already draws with the new name.
    let list = d.find("LOADMULTIPLAYERMAPLEVELS").unwrap();
    let at = row_center(&d, list, 3);
    click_at(&mut d, at);
    let shown = d.frame();
    let now = d.now - crate::app::tests::DT;
    let third = &d.app.data().maps.maps[d.app.data().maps.filtered(0)[3]];
    let new = d.app.text_context().cloned().unwrap();
    assert_eq!(
        new.cur_level.as_ref().map(|c| c.name.clone()),
        Some(third.name.clone())
    );
    assert_ne!(third.name, expected.name);
    // The output of exactly this frame equals the typesetting with the new data, not the one with the old data.
    let old = TextContext {
        cur_level: Some(expected),
        ..new.clone()
    };
    let paint = |d: &mut Driver<Corpus>, t: &TextContext| {
        let mut ctx = d.world.ctx(S1280);
        paint_ui_with(d.app.tree(), &mut ctx, now, Some(t), None)
    };
    assert!(shown == paint(&mut d, &new), "frame with the new name");
    assert!(shown != paint(&mut d, &old), "no longer the old name");
}
