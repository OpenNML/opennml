//! Tests of the map catalog and the map selection. The corpus test needs `NML_GAME_DIR` and silently skips itself
//! otherwise.

use nml_formats::mp_levels::{GameTypeDesc, NationDesc};
use nml_formats::wld::parse_world;
use nml_rda::testutil::{GameVersion, game_version};

use super::*;
use crate::app::tests::corpus;
use crate::dialog::tests::fixture;

#[test]
fn size_class_reads_the_first_terrain_dim_x() {
    // `FUN_004BDDA0`: line after `TrimLeft` with "TERRAIN_DIM_X ", `atol`; 64/128/256 -> 0/1/2, otherwise 0.
    let wld = |dim: &str| {
        format!("WORLDFILE V1.08e\r\n\r\n[TERRAIN]\r\n{{\r\n\tTERRAIN_DIM_X {dim}\r\n}}\r\n")
    };
    assert_eq!(size_class(wld("64").as_bytes()), 0);
    assert_eq!(size_class(wld("128").as_bytes()), 1);
    assert_eq!(size_class(wld("256").as_bytes()), 2);
    assert_eq!(size_class(wld("  256x").as_bytes()), 2, "atol");
    assert_eq!(size_class(wld("512").as_bytes()), 0);
    assert_eq!(size_class(wld("abc").as_bytes()), 0);
    assert_eq!(size_class(b"[TERRAIN]\n{\n}\n"), 0, "without an entry");
    // Without a space after the key no match (prefix comparison with "TERRAIN_DIM_X ").
    assert_eq!(size_class(b"TERRAIN_DIM_X\t128\n"), 0);
}

#[test]
fn preview_and_world_paths_replace_the_extension() {
    // `FUN_004C6FB0`: path - 3 + "tga"; `FUN_004BDDA0`: path - 4 + ".wld".
    let path = "data/levels/multiplayer/[2Players]-BattleBeach.lvl";
    assert_eq!(
        preview_path(path),
        "data/levels/multiplayer/[2Players]-BattleBeach.tga"
    );
    assert_eq!(
        world_path(path),
        "data/levels/multiplayer/[2Players]-BattleBeach.wld"
    );
    assert_eq!(preview_path(""), "tga");
}

fn types() -> RandomMapTypes {
    let game = |name_id, gtype: &str| GameTypeDesc {
        name_id,
        gtype: gtype.into(),
    };
    RandomMapTypes {
        game_types: vec![
            game(15540, "NORMAL"),
            game(15541, "NORMAL"),
            game(15542, "NORMAL"),
            game(15543, "KING_OF_THE_HILL"),
            game(15544, "RAILROAD1"),
            game(15545, "RAILROAD2"),
        ],
        nations: ["WID", "PRI", "SPA", "SID", "ENG", "PAT"]
            .iter()
            .zip(15120..)
            .map(|(n, id)| NationDesc {
                name_id: id,
                ntype: (*n).into(),
            })
            .collect(),
        ..RandomMapTypes::default()
    }
}

/// Catalog from entries (name, `TYPE`, slots, size, readable).
pub(crate) fn catalog(maps: &[(&str, &str, usize, i32, bool)]) -> MapCatalog {
    MapCatalog {
        maps: maps
            .iter()
            .zip(0..)
            .map(|(&(name, kind, players, size, loadable), i)| {
                let path = format!("data/levels/multiplayer/[{players}Players]-{name}.lvl");
                MapCatalogEntry {
                    entry: MapListEntry {
                        name_id: 15800 + i,
                        kind: kind.into(),
                        path: path.clone(),
                    },
                    name: name.into(),
                    size_class: size,
                    slots: (0..players)
                        .zip(0i32..)
                        .map(|(_, b)| SlotDraft {
                            key: b - 1,
                            name: EMPTY_SLOT_NAME.into(),
                            kind: 0,
                            nation: b % 6,
                            team: b,
                            handicap: 0,
                            color: 7 - b,
                        })
                        .collect(),
                    preview: preview_path(&path),
                    level_read: loadable,
                    world_read: loadable,
                }
            })
            .collect(),
        types: types(),
    }
}

#[test]
fn filter_compares_gtype_and_type_exactly() {
    // `FUN_004BDC50`: `GTYPE` of the game mode against `TYPE` (`_mbscmp`, exact); invalid game mode -> empty word.
    let maps = catalog(&[
        ("A", "NORMAL", 2, 0, true),
        ("B", "RAILROAD1", 2, 0, true),
        ("C", "NORMAL", 4, 1, true),
        ("D", "normal", 2, 0, true),
        ("E", "", 2, 0, true),
    ]);
    assert_eq!(maps.filtered(0), [0, 2]);
    assert_eq!(maps.filtered(2), [0, 2]);
    assert_eq!(maps.filtered(4), [1]);
    assert!(maps.filtered(5).is_empty());
    assert_eq!(maps.filtered(6), [4], "invalid: empty word");
    assert_eq!(maps.filtered(-1), [4]);
    // `FUN_004CBB10`: game type = text - 15540, invalid 0.
    assert_eq!(maps.game_kind(3), 3);
    assert_eq!(maps.game_kind(9), 0);
    assert_eq!(nation_index(&maps.types, "SID"), 3);
    assert_eq!(nation_index(&maps.types, "sid"), -1);
}

#[test]
fn select_sets_info_and_texts_or_keeps_them_on_failure() {
    // `FUN_004BFAE0`: index in the filtered list; name text before reading, size/players only afterwards.
    let f = fixture();
    let maps = catalog(&[
        ("A", "NORMAL", 2, 0, true),
        ("B", "RAILROAD1", 4, 2, true),
        ("C", "NORMAL", 6, 1, false),
    ]);
    let mut level = SelectedLevel::default();
    assert!(!level.select(&maps, 0, 2, &f.texts), "outside the list");
    assert_eq!(level, SelectedLevel::default());
    assert!(level.select(&maps, 4, 0, &f.texts));
    assert_eq!(level.map, Some(1));
    assert_eq!(
        level.info,
        LevelInfoData {
            name: "B".into(),
            path: "data/levels/multiplayer/[4Players]-B.lvl".into(),
            size_class: 2,
            players: 4,
            level_type: 0,
            ..LevelInfoData::default()
        }
    );
    let text = level.text.clone().unwrap();
    assert_eq!(text.name, "B");
    assert_eq!(text.size, text_of(&f.texts, 15452));
    assert_eq!(text.players, text_of(&f.texts, 15462));
    // Map C is not readable: name, path and index new, size and players (and their texts) stay.
    assert!(!level.select(&maps, 0, 1, &f.texts));
    assert_eq!(level.map, Some(2));
    assert_eq!(level.info.path, "data/levels/multiplayer/[6Players]-C.lvl");
    assert_eq!((level.info.name.as_str(), level.info.players), ("B", 4));
    let after = level.text.clone().unwrap();
    assert_eq!(after.name, "C");
    assert_eq!((&after.size, &after.players), (&text.size, &text.players));
    // Only the `.wld` is missing: `FUN_004BDDA0` has already set the player count from the `.lvl`, not the size.
    let mut maps = maps;
    maps.maps[2].level_read = true;
    assert!(!level.select(&maps, 0, 1, &f.texts));
    assert_eq!(level.info.players, 6);
    assert_eq!((level.info.name.as_str(), level.info.size_class), ("B", 2));
    assert_eq!(
        level.text.unwrap().players,
        text.players,
        "text only after success"
    );
}

// QUIRK O-135: kept
#[test]
fn slots_are_read_line_by_line_like_the_original() {
    // `FUN_004BDDA0`: block exactly "[PLAYER]" up to a line exactly "}"; `PLAYER_PLAYER` Mid(15) without the last
    // character, `PLAYER_COLOR` atol, `PLAYER_TYPE` discarded; nation and color inherit from the previous block.
    let text = [
        "LEVELFILE V1.08e",
        "[PLAYER]",
        "{",
        "\tPLAYER_NAME \"29081 RED\"",
        "\tPLAYER_PLAYER \"SPA\"",
        "\tPLAYER_TYPE 1",
        "\tPLAYER_COLOR 3",
        "\t[RESOURCES]",
        "\t{",
        "\t}",
        "\tPLAYER_COLOR 5",
        "}",
        "[PLAYER]",
        "{",
        "\tPLAYER_TYPE 2",
        "}",
        " [PLAYER]",
        "{",
        "}",
        "[PLAYER]",
        "{",
        "\tPLAYER_PLAYER \"spa\"",
        "\tPLAYER_COLOR x",
        "}",
        "",
    ]
    .join("\r\n");
    let slots = read_slots(&text, &types());
    let view: Vec<(i32, i32, i32, i32, i32)> = slots
        .iter()
        .map(|s| (s.key, s.team, s.kind, s.nation, s.color))
        .collect();
    assert_eq!(
        view,
        [(-1, 0, 0, 2, 5), (0, 1, 0, 2, 5), (1, 2, 0, -1, 0)],
        "indented }} does not end, \" [PLAYER]\" does not count, inherited from the previous block"
    );
    assert!(slots.iter().all(|s| s.name == "EMPTY" && s.handicap == 0));
    assert!(read_slots("", &types()).is_empty());
}

/// Player count from the file name `[nPlayers]-...`.
fn players_in_name(path: &str) -> usize {
    let name = path.rsplit('/').next().unwrap();
    name[1..2].parse().unwrap()
}

#[test]
fn map_catalog_reads_every_multiplayer_map() {
    let Some(c) = corpus() else {
        return;
    };
    let maps = MapCatalog::load(&c.vfs, &c.texts);
    let version = game_version(&c.vfs).unwrap_or_else(|e| panic!("{e}"));
    if version == GameVersion::German {
        assert_eq!(maps.maps.len(), 20);
        assert_eq!(maps.types.game_types.len(), 6);
    }
    assert!(!maps.maps.is_empty());
    let mut sizes = [0usize; 3];
    for m in &maps.maps {
        let path = &m.entry.path;
        assert!(m.level_read && m.world_read, "{path}");
        assert!(!m.name.is_empty(), "{path}");
        // Slots 2...8, as many as in the file name; key -1...n-2, team = block, nation from the `NTYPE` list.
        let n = m.slots.len();
        assert!((2..=8).contains(&n), "{path}: {n} slots");
        assert_eq!(n, players_in_name(path), "{path}");
        for (s, b) in m.slots.iter().zip(0i32..) {
            assert_eq!(
                (s.key, s.team, s.kind, s.handicap),
                (b - 1, b, 0, 0),
                "{path}"
            );
            assert_eq!(s.name, "EMPTY");
            assert!((0..6).contains(&s.nation), "{path}: nation {}", s.nation);
            assert!((0..8).contains(&s.color), "{path}: color {}", s.color);
        }
        // Line reader = block reader of the `.lvl` (nation, color, number of blocks).
        let level =
            nml_formats::lvl::parse_level(&decode_cp1252(&c.vfs.read(path).unwrap())).unwrap();
        let from_blocks: Vec<(i32, i32)> = level
            .players
            .iter()
            .map(|p| (nation_index(&maps.types, &p.nation), p.color))
            .collect();
        let from_lines: Vec<(i32, i32)> = m.slots.iter().map(|s| (s.nation, s.color)).collect();
        assert_eq!(from_lines, from_blocks, "{path}");
        // Preview in the VFS.
        assert!(c.vfs.exists(&m.preview), "{path}: {} is missing", m.preview);
        // Header reader = world reader; exactly one `TERRAIN_DIM_X` per file (first hit = last as in the original).
        let bytes = c.vfs.read(&world_path(path)).unwrap();
        let text = decode_cp1252(&bytes);
        let hits = text
            .lines()
            .filter(|l| l.trim_start().starts_with("TERRAIN_DIM_X "))
            .count();
        assert_eq!(hits, 1, "{path}");
        let dim = parse_world(&text).unwrap().terrain.dim_x;
        let expected = match dim {
            128 => 1,
            256 => 2,
            _ => 0,
        };
        assert!([64, 128, 256].contains(&dim), "{path}: {dim}");
        assert_eq!(m.size_class, expected, "{path}: {dim}");
        sizes[usize::try_from(m.size_class).unwrap()] += 1;
    }
    eprintln!("Size classes small/medium/large: {sizes:?}");
}

#[test]
fn domination_lists_the_eight_normal_maps() {
    // Game mode 2 "Domination" has `GTYPE NORMAL` and shows the same map list as Deathmatch and Heroes - eight maps
    // in the shipped data (re/domination F.1); King of the Hill (3) has three of its own.
    let Some(c) = corpus() else {
        return;
    };
    let maps = MapCatalog::load(&c.vfs, &c.texts);
    let list = maps.filtered(2);
    assert_eq!(maps.gtype(2), "NORMAL");
    assert_eq!(list, maps.filtered(0));
    assert_eq!(list, maps.filtered(1));
    assert!(!list.is_empty());
    assert!(
        list.iter().all(|&i| maps.maps[i].entry.kind == "NORMAL"),
        "only NORMAL maps"
    );
    let names: Vec<String> = list
        .iter()
        .map(|&i| {
            let path = &maps.maps[i].entry.path;
            path.rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".lvl")
                .to_lowercase()
        })
        .collect();
    if game_version(&c.vfs).unwrap_or_else(|e| panic!("{e}")) == GameVersion::German {
        assert_eq!(list.len(), 8, "{names:?}");
        assert_eq!(maps.filtered(3).len(), 3);
        for expected in [
            "[2players]-battlebeach",
            "[2players]-tintebariver",
            "[3players]-canonballcanyon",
            "[4players]-donut-isle",
            "[4players]-snowdream",
            "[5players]-viciouscircle",
            "[6players]-legends",
            "[8players]-tribes",
        ] {
            assert!(names.iter().any(|n| n == expected), "{expected}: {names:?}");
        }
    }
}

#[test]
fn levelinfos_lines_follow_singleplayer_menus() {
    // `FUN_004B9C90`: per line symbol 11 + label + symbol 2 + value + symbol 6 (`extrasymbols.txt`); switch lines
    // only if set, as symbol 2 + text + symbol 6; time limit only with game type 2...4.
    let texts = Texts::from_entries(&[
        (2, "<d>"),
        (6, "|"),
        (11, "<g>"),
        (15451, "Medium"),
        (15462, "4 players"),
        (15532, "100"),
        (15540, "DM"),
        (15543, "KotH"),
        (15552, "normal"),
        (15650, "Name: "),
        (15651, "Size: "),
        (15652, "Players: "),
        (15653, "Limit: "),
        (15654, "Resources: "),
        (15655, "Time: "),
        (15656, "Recording"),
        (15657, "Fixed teams"),
        (15658, "Explored"),
        (15673, "Game mode: "),
        (15674, "minutes"),
    ]);
    let mut draft = MatchDraft {
        info: LevelInfoData {
            name: "Island".into(),
            path: "x.lvl".into(),
            size_class: 1,
            players: 4,
            level_type: 0,
            ..LevelInfoData::default()
        },
        settings: DraftSettings {
            game_type: 0,
            unit_limit: 1,
            resources: 2,
            time_limit: 30,
            ..DraftSettings::default()
        },
        ..MatchDraft::default()
    };
    let base = |middle: &str, game: &str| {
        format!(
            "<g>Name: <d>Island|<g>Size: <d>Medium|<g>Players: <d>4 players|<g>Limit: <d>100|{middle}\
             <g>Resources: <d>normal|<g>Game mode: <d>{game}|"
        )
    };
    assert_eq!(level_infos(&draft, None, &texts), base("", "DM"));
    // Game type 3 (King of the Hill): time limit; switches in the order SaveReplay, LockTeams, MapExplored.
    draft.settings.game_type = 3;
    draft.settings.save_replay = true;
    draft.settings.map_explored = true;
    assert_eq!(
        level_infos(&draft, None, &texts),
        base("<d>Recording|<d>Explored|<g>Time: <d>30 minutes|", "KotH")
    );
    draft.settings.lock_teams = true;
    draft.settings.save_replay = false;
    draft.settings.map_explored = false;
    draft.settings.game_type = 5;
    assert!(level_infos(&draft, None, &texts).contains("<d>Fixed teams|<g>Resources"));
    assert!(
        !level_infos(&draft, None, &texts).contains("Time"),
        "game type 5"
    );
    draft.settings.game_type = 2;
    assert!(level_infos(&draft, None, &texts).contains("Time"));
    // Invalid session (`LevelType -1`): only the labels, without values, switches and time limit.
    draft.info.level_type = -1;
    assert_eq!(
        level_infos(&draft, None, &texts),
        "<g>Name: <d>|<g>Size: <d>|<g>Players: <d>|<g>Limit: <d>|<g>Resources: <d>|<g>Game mode: <d>|"
    );
}

#[test]
fn levelinfos_show_the_random_map_values() {
    // `FUN_004B9C90` with a random map: line 1 with 15659 "Random value: " and the seed, then map type (15671,
    // `15470 + terrain`), landscape (15670, `15490 + landscape`) and time of day (15672, `15500 + time of day`) from
    // the set (`+0x168/+0x16C/+0x170`).
    let texts = Texts::from_entries(&[
        (2, "<d>"),
        (6, "|"),
        (11, "<g>"),
        (15450, "Small"),
        (15460, "2 players"),
        (15472, "Islands"),
        (15491, "Snow"),
        (15500, "Day"),
        (15531, "75"),
        (15540, "DM"),
        (15550, "Map"),
        (15651, "Size: "),
        (15652, "Players: "),
        (15653, "Limit: "),
        (15654, "Resources: "),
        (15659, "Random: "),
        (15670, "Landscape: "),
        (15671, "Type: "),
        (15672, "Time of day: "),
        (15673, "Game mode: "),
    ]);
    let draft = MatchDraft {
        info: LevelInfoData {
            name: "41".into(),
            size_class: 0,
            players: 2,
            level_type: 1,
            random_terrain: 2,
            random_landscape: 1,
            random_daytime: 0,
            ..LevelInfoData::default()
        },
        ..MatchDraft::default()
    };
    assert_eq!(
        level_infos(&draft, None, &texts),
        "<g>Random: <d>41|<g>Size: <d>Small|<g>Players: <d>2 players|<g>Type: <d>Islands|\
         <g>Landscape: <d>Snow|<g>Time of day: <d>Day|<g>Limit: <d>75|<g>Resources: <d>Map|<g>Game mode: <d>DM|"
    );
}

// QUIRK O-140: deviation
#[test]
fn seed_rand_is_the_msvc_sequence() {
    // Own `rand()` state of the menu, start 1 (`state = state * 214013 + 2531011`, result
    // `(state >> 16) & 0x7FFF`).
    let mut rand = SeedRand::default();
    assert_eq!(rand, SeedRand(1));
    let values: Vec<i32> = (0..5).map(|_| rand.next()).collect();
    assert_eq!(values, [41, 18467, 6334, 26500, 19169]);
}
