//! Map scripts of the random map (`data/randommapscripts/*.rnd`) of the original installation: dispatcher
//! and records for all combinations of script, landscape, size, game mode and time of day.
//! Skipped if the installation is missing. The installation is only read; only counters and
//! checksums are recorded, no script contents.

use opennml_formats::mp_levels::{RandomMapTypes, parse_random_map_types};
use opennml_formats::rnd::{RandomMapScript, parse_random_map_script};
use opennml_formats::text::decode_cp1252;
use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};

const SIZES: [&str; 3] = ["MAP_SIZE_SMALL", "MAP_SIZE_MEDIUM", "MAP_SIZE_LARGE"];
const GAME_TYPES: [&str; 4] = ["NORMAL", "KING_OF_THE_HILL", "RAILROAD1", "RAILROAD2"];

fn open() -> Option<(Vfs, RandomMapTypes)> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let types = parse_random_map_types(&decode_cp1252(
        &vfs.read("data/gifcon/randommaptypes.ini").unwrap(),
    ));
    Some((vfs, types))
}

/// One run of the dispatcher with the fixed hook `|| 0`; returns the script and the number of draws.
fn parse(vfs: &Vfs, script: &str, defines: &[&str]) -> (RandomMapScript, usize) {
    let mut read = |path: &str| vfs.read(path).ok();
    let mut draws = 0;
    let mut random = || {
        draws += 1;
        0
    };
    let result = parse_random_map_script(
        script,
        defines.iter().map(|d| d.to_string()).collect(),
        &mut read,
        &mut random,
    )
    .unwrap_or_else(|e| panic!("{script} {defines:?}: {e}"));
    (result, draws)
}

/// All combinations of a script in fixed order: landscape, size, game mode, time of day.
fn combinations(types: &RandomMapTypes) -> Vec<[&str; 4]> {
    let mut out = Vec::new();
    for landscape in &types.landscape_types {
        for size in SIZES {
            for game in GAME_TYPES {
                for daytime in &types.daytime_types {
                    out.push([landscape.as_str(), size, game, daytime.as_str()]);
                }
            }
        }
    }
    out
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

const FNV_START: u64 = 0xCBF2_9CE4_8422_2325;

#[test]
fn all_scripts_parse_for_every_combination() {
    let Some((vfs, types)) = open() else { return };
    assert_eq!(types.map_scripts.len(), 6);
    let mut runs = 0;
    for script in &types.map_scripts {
        for combo in combinations(&types) {
            let (result, _) = parse(&vfs, script, &combo);
            assert!(
                result.missing_includes.is_empty(),
                "{script} {combo:?}: {:?}",
                result.missing_includes
            );
            assert_eq!(
                result.project_file.as_deref(),
                Some("Data\\Projects\\nml-wiese.prj"),
                "{script} {combo:?}"
            );
            assert!(vfs.exists("data/projects/nml-wiese.prj"));
            assert!(!result.textures.is_empty(), "{script} {combo:?}");
            runs += 1;
        }
    }
    assert_eq!(runs, 6 * 5 * 3 * 4 * 2);
}

// QUIRK D-322: kept
#[test]
fn preprocessor_draws_per_script() {
    let Some((vfs, types)) = open() else { return };
    let combo = ["MEADOW", "MAP_SIZE_MEDIUM", "NORMAL", "DAY"];
    let draws: Vec<(String, usize)> = types
        .map_scripts
        .iter()
        .map(|script| {
            let name = script.rsplit('/').next().unwrap().to_lowercase();
            (name, parse(&vfs, script, &combo).1)
        })
        .collect();
    // Only `#random` outside `#ifdef` draws randomness - independent of the defines.
    let expected = [
        ("continental.rnd", 3),
        ("flatland.rnd", 1),
        ("highland.rnd", 0),
        ("islands.rnd", 0),
        ("lakes.rnd", 1),
        ("rivers.rnd", 1),
    ];
    let draws: Vec<(&str, usize)> = draws.iter().map(|(n, d)| (n.as_str(), *d)).collect();
    assert_eq!(draws, expected);
    for script in &types.map_scripts {
        let reference = parse(&vfs, script, &combo).1;
        for combo in combinations(&types) {
            assert_eq!(
                parse(&vfs, script, &combo).1,
                reference,
                "{script} {combo:?}"
            );
        }
    }
}

#[test]
fn flag_land_area_exists_without_koth() {
    let Some((vfs, types)) = open() else { return };
    for script in &types.map_scripts {
        for combo in combinations(&types) {
            let (result, _) = parse(&vfs, script, &combo);
            let flags: Vec<_> = result
                .land_areas
                .iter()
                .filter(|a| a.texture == "FLAG")
                .collect();
            if combo[2] == "KING_OF_THE_HILL" {
                assert_eq!(result.land_areas.len(), 2, "{script} {combo:?}");
            } else {
                // D-320: the inner `#endif` switches to active, the 1-tile area FLAG remains.
                assert_eq!(result.land_areas.len(), 1, "{script} {combo:?}");
                assert_eq!(flags.len(), 1, "{script} {combo:?}");
                let flag = flags[0];
                assert_eq!(flag.num_tiles, 1, "{script} {combo:?}");
                assert_eq!(
                    [flag.left, flag.right, flag.top, flag.bottom],
                    [49; 4],
                    "{script} {combo:?}"
                );
            }
        }
    }
}

// QUIRK D-323: kept
#[test]
fn continental_tree_block_follows_the_preprocessor() {
    let Some((vfs, types)) = open() else { return };
    let continental = types
        .map_scripts
        .iter()
        .find(|s| s.to_lowercase().ends_with("continental.rnd"))
        .unwrap();
    // Names of the `place_Object` blocks left behind by the tree block (`number 5`, `numGroups 20`,
    // `ForAllPlayers`, `minDistToPlayers 8`).
    let tree_block = |landscape: &str| -> Vec<String> {
        let (result, _) = parse(
            &vfs,
            continental,
            &[landscape, "MAP_SIZE_MEDIUM", "NORMAL", "DAY"],
        );
        result
            .placements
            .iter()
            .filter(|p| {
                p.number == 5
                    && p.num_groups == 20
                    && p.for_all_players
                    && p.min_dist_to_players == 8
                    && p.max_dist_to_players == 10
            })
            .map(|p| p.name.clone())
            .collect()
    };
    // `#elsif` is not a directive (D-323): the block depends only on `#ifdef WINTER`, `#elseif DESERT`
    // and `#else`. Winter gets `CONIFER_WINTER`, desert gets no header (the block lines fall through the
    // dispatcher), all others `TREE_SMALL`; `place_object PALM2` (small o) stays
    // without effect everywhere.
    assert_eq!(tree_block("WINTER"), ["CONIFER_WINTER"]);
    assert!(tree_block("DESERT").is_empty());
    for landscape in ["CARIBBEAN", "PRAIRIE", "MEADOW"] {
        assert_eq!(tree_block(landscape), ["TREE_SMALL"], "{landscape}");
    }
    for landscape in ["CARIBBEAN", "PRAIRIE", "DESERT", "WINTER", "MEADOW"] {
        let (result, _) = parse(
            &vfs,
            continental,
            &[landscape, "MAP_SIZE_MEDIUM", "NORMAL", "DAY"],
        );
        // `PALM2` only comes from the forest block with `#ifdef CARIBBEAN` (capital O), otherwise never.
        let palm2 = result
            .placements
            .iter()
            .filter(|p| p.name == "PALM2")
            .count();
        assert_eq!(palm2, usize::from(landscape == "CARIBBEAN"), "{landscape}");
    }
}

#[test]
fn script_snapshot_per_script() {
    let Some((vfs, types)) = open() else { return };
    let digests_of = || {
        let mut digests = Vec::new();
        for script in &types.map_scripts {
            // A checksum over the `Debug` output of all 120 combinations of the script.
            let mut hash = FNV_START;
            for combo in combinations(&types) {
                let (result, _) = parse(&vfs, script, &combo);
                fnv1a(&mut hash, format!("{result:?}").as_bytes());
            }
            digests.push(hash);
        }
        digests
    };
    let digests = digests_of();
    match game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) {
        // Measured with the fixed hook `|| 0` on the German edition (with patch).
        GameVersion::German => assert_eq!(digests, SNAPSHOT, "{digests:#x?}"),
        // No measurements exist for the English edition (only the German installation is
        // available); there only structurally: everything parses (`parse` aborts otherwise), and a
        // repetition yields the same sums. Not measured.
        GameVersion::English => {
            assert_eq!(digests.len(), SNAPSHOT.len());
            assert_eq!(digests_of(), digests);
        }
    }
}

/// Checksums per script (`continental`, `flatland`, `highland`, `islands`, `lakes`, `rivers`).
const SNAPSHOT: [u64; 6] = [
    0x7913e75f1e098ea7,
    0x4437f49633a27f73,
    0x00f6663db0f48ea7,
    0xd1cd9ec7ed4d26eb,
    0xe1ce89bbbaa03a0b,
    0xf1974cb73b0f8df3,
];

#[test]
fn random_map_types_from_the_corpus() {
    let Some((_, types)) = open() else { return };
    assert_eq!(types.map_scripts[3], "data/randommapscripts/Islands.rnd");
    assert_eq!(
        types.landscape_types,
        ["CARIBBEAN", "PRAIRIE", "DESERT", "WINTER", "MEADOW"]
    );
    assert_eq!(types.daytime_types, ["DAY", "NIGHT"]);
}
