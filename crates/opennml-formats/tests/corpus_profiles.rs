//! `campaign.ini`, `profiles.ini`, multiplayer map list, `randommaptypes.ini` and `.ldg` files of the
//! original installation. Skipped if it is missing.
//! The installation is only read.

use opennml_formats::campaign::{CampaignEntry, CampaignTable, EntryKind, parse_campaign_ini};
use opennml_formats::gametype_rules::{
    GAMETYPE_DEFEAT_PATH, GAMETYPE_VICTORY_PATH, GameTypeRules, RuleLeaf, RuleNode,
    parse_gametype_rules,
};
use opennml_formats::ldg::parse_ldg;
use opennml_formats::mp_levels::{load_map_list, parse_random_map_types};
use opennml_formats::profiles::{parse_profiles, write_profiles};
use opennml_formats::text::decode_cp1252;
use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};

/// Short form of an entry for the comparison with the table in re/campaign-ui 1: `H` for a
/// headline, otherwise the file name without extension (`SPA-1`, `kamp2-6`), with `!` for `STOP`.
fn label(entry: &CampaignEntry) -> String {
    if entry.kind == EntryKind::Headline {
        return "H".to_string();
    }
    let file = entry.path.rsplit('/').next().unwrap();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    format!("{stem}{}", if entry.stop { "!" } else { "" })
}

fn labels(entries: &[CampaignEntry]) -> Vec<String> {
    entries.iter().map(label).collect()
}

/// What must hold in every edition: headlines without a path, videos and levels with a path in the VFS.
fn check_entries_are_consistent(table: &CampaignTable, vfs: &Vfs) {
    for (c, entries) in table.campaigns.iter().enumerate() {
        for (i, entry) in entries.iter().enumerate() {
            match entry.kind {
                EntryKind::Headline => assert!(entry.path.is_empty(), "{c}/{i}"),
                EntryKind::Video => {
                    assert!(entry.path.starts_with("data/videos/"), "{c}/{i}");
                    assert!(vfs.exists(&entry.path), "{c}/{i}: {}", entry.path);
                }
                EntryKind::Level => {
                    assert!(entry.path.starts_with("data/levels/"), "{c}/{i}");
                    assert!(vfs.exists(&entry.path), "{c}/{i}: {}", entry.path);
                }
            }
            assert_eq!(entry.stop, label(entry).ends_with('!'));
        }
    }
}

// QUIRK O-184: kept
#[test]
fn campaign_ini_matches_campaign_ui() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let text = decode_cp1252(&vfs.read("data/gifcon/campaign.ini").unwrap());
    let table = parse_campaign_ini(&text);
    check_entries_are_consistent(&table, &vfs);

    match game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) {
        GameVersion::German => {}
        GameVersion::English => {
            // The sequences and IDs below are documented only for the German edition (reference)
            // (re/campaign-ui 1, text IDs in `dialogtexte.deu`). For the English one only the
            // edition-independent form holds: four non-empty campaigns, each starting with a headline.
            for (c, entries) in table.campaigns.iter().enumerate() {
                assert!(!entries.is_empty(), "campaign {c} empty");
                assert_eq!(entries[0].kind, EntryKind::Headline, "campaign {c}");
            }
            eprintln!("English edition: sequences of the table not checked");
            return;
        }
    }

    let expected: [&[&str]; 4] = [
        &["H", "kamp0-1", "kamp0-2"],
        &[
            "H", "SPA-1!", "kamp1-1", "kamp1-2", "kamp1-3", "kamp1-4", "kamp1-5", "kamp1-6",
            "SPA-2",
        ],
        &[
            "H", "IND-1!", "kamp2-1", "kamp2-2", "kamp2-3", "kamp2-4", "kamp2-5", "H", "IND-2",
            "kamp2-6!", "kamp2-7", "kamp2-8", "kamp2-9", "kamp2-10", "IND-3",
        ],
        &[
            "H",
            "ENG!",
            "kamp3-1",
            "kamp3-2",
            "kamp3-3",
            "kamp3-4",
            "kamp3-5",
            "H",
            "PAT-1!",
            "kamp3-6",
            "kamp3-7",
            "kamp3-8",
            "kamp3-9",
            "kamp3-10",
            "PAT-2",
            "H",
            "kamp3-11!",
            "kamp3-12",
            "kamp3-13",
            "kamp3-14",
            "SID",
        ],
    ];
    for (c, want) in expected.iter().enumerate() {
        assert_eq!(labels(&table.campaigns[c]), *want, "campaign {c}");
    }

    // Name IDs (re/campaign-ui 1): headlines 27039 ... 27045, first levels 27030/27031 and 27000, respectively,
    // videos 28000 ...; `AUTOSAVENAME` ends up in the text field (27600 onward, introduction 27630/27631).
    let c0 = &table.campaigns[0];
    assert_eq!((c0[0].name_id, c0[0].text_id), (27039, 27039));
    assert_eq!((c0[1].name_id, c0[1].text_id), (27030, 27630));
    assert_eq!((c0[2].name_id, c0[2].text_id), (27031, 27631));
    let c1 = &table.campaigns[1];
    assert_eq!((c1[1].name_id, c1[1].text_id), (28000, 28050));
    assert_eq!((c1[2].name_id, c1[2].text_id), (27000, 27600));
    // Videos without `VIDEOTEXT` keep the ID as text (`IND-3`).
    let c2 = &table.campaigns[2];
    assert_eq!((c2[14].name_id, c2[14].text_id), (28004, 28004));
}

/// The empty form the original creates as long as no profile exists (re/singleplayer-menus 2.2).
const EMPTY_FORM: &str = "<m_CurProfileID>-1</><m_NextProfileGUID>1</><m_Profiles><arraysize>0</></>\
                          <m_CurLevelID>1</><m_CurCampaignID>0</>";

#[test]
fn installation_profiles_ini_reads_cleanly() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let Ok(bytes) = std::fs::read(dir.join("profiles.ini")) else {
        eprintln!("profiles.ini not present - skipped");
        return;
    };
    let text = decode_cp1252(&bytes);
    // Profiles the user created themselves must not break the test: all that is required is that the
    // file is read without errors.
    let file = parse_profiles(&text).unwrap_or_else(|e| panic!("{e}"));
    // Only the empty form is known byte for byte; it writes back unchanged.
    if text.trim_end() == EMPTY_FORM {
        assert_eq!(write_profiles(&file), EMPTY_FORM);
    }
}

/// All `.ldg` files in the VFS in archive order, sorted by path.
fn ldg_paths(vfs: &Vfs) -> Vec<String> {
    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, entry)| entry.path.clone())
        .filter(|path| path.to_ascii_lowercase().ends_with(".ldg"))
        .collect();
    paths.sort();
    paths
}

// QUIRK O-136, O-185: kept
#[test]
fn multiplayer_maps_and_ldg_read() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));

    // Map list: `levels0000.ini` up to the first missing file.
    let maps = load_map_list(|path| vfs.read(path).ok());
    let mut seen_paths = std::collections::HashSet::new();
    for (i, map) in maps.iter().enumerate() {
        assert!(
            ["NORMAL", "KING_OF_THE_HILL", "RAILROAD1", "RAILROAD2"].contains(&map.kind.as_str()),
            "map {i}: TYPE {:?}",
            map.kind
        );
        assert!(
            map.path.starts_with("data/levels/multiplayer/") && map.path.ends_with(".lvl"),
            "map {i}: {}",
            map.path
        );
        assert!(
            vfs.exists(&map.path),
            "map {i}: {} missing from the VFS",
            map.path
        );
        assert!(
            (15600..=15999).contains(&map.name_id),
            "map {i}: NAME {} outside the reserved range",
            map.name_id
        );
        assert!(
            seen_paths.insert(map.path.to_lowercase()),
            "map {i}: duplicate"
        );
    }
    // 20 maps in the German reference (the loose `NAME 15800` at the end of the file does not count, O-185);
    // the installation has only `levels0000.ini`.
    match version {
        GameVersion::German => {
            assert_eq!(maps.len(), 20);
            let ids: Vec<i32> = maps.iter().map(|m| m.name_id).collect();
            assert_eq!(ids, (15800..15820).collect::<Vec<i32>>());
            let count = |kind: &str| maps.iter().filter(|m| m.kind == kind).count();
            assert_eq!(
                [
                    count("NORMAL"),
                    count("KING_OF_THE_HILL"),
                    count("RAILROAD1"),
                    count("RAILROAD2")
                ],
                [8, 3, 4, 5],
            );
        }
        GameVersion::English => {
            assert!(!maps.is_empty(), "English edition: empty map list");
            eprintln!(
                "English edition: {} maps, count per kind not checked",
                maps.len()
            );
        }
    }

    // `randommaptypes.ini`: six game modes (texts 15540 ... 15545), six nations, map, landscape and
    // daytime lists.
    let text = decode_cp1252(&vfs.read("data/gifcon/randommaptypes.ini").unwrap());
    let types = parse_random_map_types(&text);
    let game: Vec<(i32, &str)> = types
        .game_types
        .iter()
        .map(|g| (g.name_id, g.gtype.as_str()))
        .collect();
    assert_eq!(
        game,
        [
            (15540, "NORMAL"),
            (15541, "NORMAL"),
            (15542, "NORMAL"),
            (15543, "KING_OF_THE_HILL"),
            (15544, "RAILROAD1"),
            (15545, "RAILROAD2"),
        ]
    );
    let nations: Vec<(i32, &str)> = types
        .nations
        .iter()
        .map(|n| (n.name_id, n.ntype.as_str()))
        .collect();
    assert_eq!(
        nations,
        [
            (15120, "WID"),
            (15121, "PRI"),
            (15122, "SPA"),
            (15123, "SID"),
            (15124, "ENG"),
            (15125, "PAT"),
        ]
    );
    assert_eq!(types.maps, (15470..=15475).collect::<Vec<i32>>());
    assert_eq!(types.landscapes, (15490..=15494).collect::<Vec<i32>>());
    assert_eq!(types.daytimes, vec![15500, 15501]);
    // Script paths and type words of the random map (rest of the line raw).
    assert_eq!(types.map_scripts.len(), 6);
    assert_eq!(types.map_scripts[3], "data/randommapscripts/Islands.rnd");
    assert!(
        types
            .map_scripts
            .iter()
            .all(|path| path.starts_with("data/randommapscripts/") && vfs.exists(path)),
        "{:?}",
        types.map_scripts
    );
    assert_eq!(
        types.landscape_types,
        ["CARIBBEAN", "PRAIRIE", "DESERT", "WINTER", "MEADOW"]
    );
    assert_eq!(types.daytime_types, ["DAY", "NIGHT"]);
    // Every map has a game mode that is a `GTYPE` of the list (this is how `FUN_004BDC50` filters).
    for map in &maps {
        assert!(
            types.game_types.iter().any(|g| g.gtype == map.kind),
            "{}: no game mode for TYPE {}",
            map.path,
            map.kind
        );
    }

    // Loading screens: 33 files under `data/levels/**` (four text blocks each), plus five under
    // `data/gifcon/`: `multiplayer.ldg` and `randommap.ldg` with three, `default*.ldg` without a text block.
    let paths = ldg_paths(&vfs);
    let mut level_files = 0;
    for path in &paths {
        let ldg = parse_ldg(&decode_cp1252(&vfs.read(path).unwrap()));
        let image = ldg
            .image
            .as_deref()
            .unwrap_or_else(|| panic!("{path}: no image"));
        let timber = ldg
            .timber
            .as_deref()
            .unwrap_or_else(|| panic!("{path}: no timber image"));
        assert!(
            vfs.exists(image),
            "{path}: image {image} missing from the VFS"
        );
        assert!(
            vfs.exists(timber),
            "{path}: timber image {timber} missing from the VFS"
        );
        assert!(
            ldg.rect[2] > ldg.rect[0] && ldg.rect[3] > ldg.rect[1],
            "{path}: {:?}",
            ldg.rect
        );
        let file = path.rsplit('/').next().unwrap().to_ascii_lowercase();
        let expected = if path.to_ascii_lowercase().starts_with("data/levels/") {
            level_files += 1;
            4
        } else if file == "multiplayer.ldg" || file == "randommap.ldg" {
            3
        } else {
            assert!(file.starts_with("default"), "{path}");
            0
        };
        assert_eq!(ldg.blocks.len(), expected, "{path}");
        for block in &ldg.blocks {
            assert!(
                block.extent[2] > block.extent[0],
                "{path}: {:?}",
                block.extent
            );
            assert!(!block.font.is_empty(), "{path}");
        }
    }
    assert_eq!(level_files, 33, "level loading screens");
    assert_eq!(paths.len(), 38, "all .ldg files");
}

/// Leaves of a rule table per leaf kind (names like the keywords of the files).
fn count_leaves(node: &RuleNode, counts: &mut std::collections::BTreeMap<&'static str, usize>) {
    match node {
        RuleNode::Or(children) | RuleNode::And(children) => {
            children.iter().for_each(|c| count_leaves(c, counts));
        }
        RuleNode::Not(inner) => count_leaves(inner, counts),
        RuleNode::Leaf(leaf) => {
            let name = match leaf {
                RuleLeaf::MatchLowerThan(..) => "MATCHLOWERTHAN",
                RuleLeaf::MatchGreaterThan(..) => "MATCHGREATERTHAN",
                RuleLeaf::ResourceLowerThan(..) => "RESOURCELOWERTHAN",
                RuleLeaf::ResourceGreaterThan(..) => "RESOURCEGREATERTHAN",
                RuleLeaf::CheckGuidTrigger(..) => "CHECKGUIDTRIGGER",
                RuleLeaf::CheckPlayerId(..) => "CHECKPLAYERID",
                RuleLeaf::CheckTimer(..) => "CHECKTIMER",
                RuleLeaf::CheckEnemy => "CHECKENEMY",
                RuleLeaf::CheckFlagOwner => "CHECKFLAGOWNER",
            };
            *counts.entry(name).or_default() += 1;
        }
    }
}

fn load_rules(vfs: &Vfs, path: &str) -> GameTypeRules {
    parse_gametype_rules(&decode_cp1252(&vfs.read(path).unwrap()))
}

/// Both rule files: for each game type 0 ... 5 all six nations occupied, game type 6 empty (re/ingame-menus 6.1),
/// leaf counts of the reference (German edition with patch); the English edition was not available.
#[test]
fn installation_rules_have_twelve_gametypes() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let victory = load_rules(&vfs, GAMETYPE_VICTORY_PATH);
    let defeat = load_rules(&vfs, GAMETYPE_DEFEAT_PATH);

    let mut counts = std::collections::BTreeMap::new();
    for (name, rules) in [("victory", &victory), ("defeat", &defeat)] {
        for (gametype, row) in rules.rules.iter().enumerate() {
            for (nation, root) in row.iter().enumerate() {
                match (gametype, root) {
                    (0..=5, Some(root)) => count_leaves(root, &mut counts),
                    (0..=5, None) => {
                        panic!("{name}: game type {gametype} nation {nation} without rule")
                    }
                    (_, root) => assert!(root.is_none(), "{name}: game type {gametype} occupied"),
                }
            }
        }
    }
    let want = [
        ("MATCHLOWERTHAN", 84),
        ("CHECKENEMY", 36),
        ("CHECKTIMER", 18),
        ("CHECKFLAGOWNER", 12),
        ("CHECKGUIDTRIGGER", 12),
        ("CHECKPLAYERID", 12),
    ];
    let got: Vec<_> = counts.iter().map(|(k, v)| (*k, *v)).collect();
    let mut want_sorted = want.to_vec();
    want_sorted.sort();
    assert_eq!(got, want_sorted, "leaf counts of both files combined");

    // Samples from re/ingame-menus 6.1: game type 0 loses without buildings and wins via `CHECKENEMY`.
    for nation in 0..6 {
        assert_eq!(
            defeat.rules[0][nation],
            Some(RuleNode::And(vec![RuleNode::Leaf(
                RuleLeaf::MatchLowerThan(8, 1)
            )]))
        );
        assert_eq!(
            victory.rules[0][nation],
            Some(RuleNode::Or(vec![RuleNode::Leaf(RuleLeaf::CheckEnemy)]))
        );
    }
}
