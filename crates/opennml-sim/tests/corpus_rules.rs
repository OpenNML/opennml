//! Loads the rules of all multiplayer maps and one campaign map from the shipped data.

use std::collections::BTreeSet;

use opennml_formats::gifcon::button_kind;
use opennml_rda::Vfs;
use opennml_rda::testutil::{TestFile, build_rda, corpus_game_dir};
use opennml_sim::rules::Rules;
use opennml_world::level::{level_paths, load_level};

#[test]
fn rules_load_for_all_multiplayer_maps() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut levels: Vec<String> = level_paths(&vfs)
        .into_iter()
        .filter(|p| p.contains("/multiplayer/"))
        .collect();
    assert_eq!(levels.len(), 20);
    levels.push("data/levels/kampagne1/kamp1-1/kamp1-1.lvl".into());
    let mut checksums = BTreeSet::new();
    for lvl in &levels {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene, "DEU").unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
        checksums.insert(rules.checksum);
        assert_eq!(rules.targets.len(), 77, "{lvl}");
        // Economy files: menus, GUID groups, three starting resources.
        assert_eq!(rules.toolbar.menus.len(), 743, "{lvl}");
        assert_eq!(rules.guid_groups.len(), 2, "{lvl}");
        assert_eq!(rules.resource_config.start_records().count(), 3, "{lvl}");
        assert_eq!(rules.map.cell, [128; 3], "{lvl}");
        assert!(!rules.world_objects.is_empty(), "{lvl}");
        // All headquarters of the start objects have a hull box.
        for hq in [1000, 1300, 1600, 1900, 2200, 2500] {
            assert!(rules.model_boxes.contains_key(&hq), "{lvl}: {hq}");
        }
        // Colonist (stand module): stand animation with time marker 1.
        let stand = rules.types[&1100]
            .module(2)
            .unwrap()
            .value_int(30001)
            .unwrap() as i32;
        assert!(rules.anim_label_time(1100, stand) > 0, "{lvl}");
        // Every counter-upgrade button starts an upgrade of the rules: the former load check "no
        // upgrade of the rules" (removed) hit no reachable state.
        let counter_buttons: Vec<_> = rules
            .toolbar
            .menus
            .values()
            .flatten()
            .filter(|b| b.kind() == button_kind::COUNTERUPGRADE)
            .collect();
        assert!(!counter_buttons.is_empty(), "{lvl}");
        for b in counter_buttons {
            assert!(
                u32::try_from(b.msg).is_ok_and(|g| rules.is_upgrade(g)),
                "{lvl}: counter-upgrade button {} is not an upgrade",
                b.msg
            );
        }
    }
    // Every map has its own rules.
    assert_eq!(checksums.len(), levels.len());
}

/// The rules load the time markers of the animations for **all** types with a loadable model, not only for types
/// with a stand, combat or dying module. Reason (E-212): the animation lock 157 with 0 seconds takes marker 1 of the
/// animation of arbitrary figures.
// QUIRK E-212: kept
#[test]
fn rules_load_labels_for_all_animated_types() {
    use opennml_rda::normalize_path;
    use opennml_world::assets::ModelCache;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    let mut cache = ModelCache::new();
    let (mut animated, mut outside_old_modules) = (0, 0);
    for (&guid, model_path) in &rules.models {
        // Models without a type (e.g. 1101) cannot be carried by any unit.
        if !rules.types.contains_key(&guid) {
            continue;
        }
        let Some(model) = cache.get(&vfs, &normalize_path(model_path)) else {
            continue;
        };
        if model.animations.is_empty() {
            continue;
        }
        animated += 1;
        assert!(
            rules.anim_labels.contains_key(&guid),
            "type {guid} has animations but no time markers"
        );
        let old_modules = [2, 10, 16, 17, 18, 19, 20];
        if !old_modules
            .iter()
            .any(|&m| rules.types[&guid].has_module(m))
        {
            outside_old_modules += 1;
        }
    }
    eprintln!(
        "{animated} types with animations, of them {outside_old_modules} without the old modules"
    );
    assert!(animated > 100, "{animated}");
    assert!(outside_old_modules > 0);
    // Alarm post 1610 (no stand/combat module): both animations with marker 1 at 33 ms.
    assert_eq!(rules.anim_label_time(1610, 0), 33);
    // A human type from the uses of 157 in the shipped scripts (scripts name figures with the animations 3, 4, 6,
    // 10, 11, 13, 16): the colonist has a stand animation with marker, and every human type with
    // animations has a marker list.
    assert!(rules.anim_labels.contains_key(&1100));
}

/// Without `property.ini` or `translate.ini` there would be no types: the rules fail with the
/// file name instead of succeeding with an empty type table (the map would then appear without
/// simulation).
#[test]
fn rules_fail_without_property_or_translate_ini() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let lvl = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
    let scene = load_level(&vfs, lvl).unwrap();
    // All files that `Rules::load` reads before `property.ini`, from the shipped data.
    let paths = vec![
        opennml_formats::config::DROPDOWN_PATH,
        opennml_formats::config::DEFAULT_DAT_PATH,
        lvl,
        opennml_formats::gifcon::DEFAULT_OBJECTS_PATH,
        opennml_formats::gifcon::UPGRADES_PATH,
        opennml_formats::targets::TARGETDEFINES_PATH,
        opennml_formats::targets::TARGETS_PATH,
        opennml_formats::bonus::BONUS_PATH,
        opennml_formats::targets::PRIORITIES_PATH,
        opennml_formats::targets::FLOWCONTROL_TARGETS_PATH,
        opennml_formats::gametype_rules::GAMETYPE_VICTORY_PATH,
        opennml_formats::gametype_rules::GAMETYPE_DEFEAT_PATH,
    ];
    let translate = opennml_formats::property::TRANSLATE_PATH;
    let property = opennml_formats::property::PROPERTY_PATH;
    let load_without = |extra: &[&str]| {
        let mut all = paths.clone();
        all.extend_from_slice(extra);
        let data: Vec<Vec<u8>> = all.iter().map(|p| vfs.read(p).unwrap()).collect();
        let files: Vec<TestFile> = all
            .iter()
            .zip(&data)
            .map(|(path, data)| TestFile {
                path,
                data,
                compress: false,
            })
            .collect();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("nml0.rda"), build_rda(&files)).unwrap();
        let reduced = Vfs::open_game_dir(tmp.path()).unwrap();
        Rules::load(&reduced, &scene, "DEU")
            .map(|_| ())
            .unwrap_err()
    };
    let err = format!("{:#}", load_without(&[]));
    assert!(err.contains(property), "{err}");
    let err = format!("{:#}", load_without(&[property]));
    assert!(err.contains(translate), "{err}");
}

/// The wall/corner pairs of the fort commands (`0x18`) from the building flags are exactly the
/// list of the build cursor (`FUN_00453530`: wall 1011, 1311, 1912, 2212, 2512; corner = wall + 1
/// from `FUN_00451260`).
#[test]
fn wall_corner_pairs_from_the_flags_match_the_fort_cursor() {
    use opennml_sim::placement::wall_corner_pair;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    let pairs: BTreeSet<(u32, u32)> = rules
        .types
        .keys()
        .filter_map(|&g| wall_corner_pair(&rules, g))
        .collect();
    let fort_cursor: BTreeSet<(u32, u32)> = [1011, 1311, 1912, 2212, 2512]
        .into_iter()
        .map(|w| (w, w + 1))
        .collect();
    assert_eq!(pairs, fort_cursor);
}

/// re/upgrades 1.1, 10.1, 10.4 F-U2; re/temp-upgrades 1.4: upgrade catalog, type lists, temporary
/// upgrades, lock via level data and time of day.
// QUIRK W-222: kept
#[test]
fn rules_know_upgrades_and_disabled_guids() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules_of = |lvl: &str| {
        let scene = load_level(&vfs, lvl).unwrap();
        Rules::load(&vfs, &scene, "DEU").unwrap_or_else(|e| panic!("{lvl}: {e:#}"))
    };
    let exile = rules_of("data/levels/multiplayer/[4players]-exile.lvl");
    assert!(exile.is_upgrade(2800));
    // 2840 (module 30) and the time-of-day upgrades 4500 and following are in no catalog (W-222).
    assert!(!exile.is_upgrade(2840));
    assert!(!exile.is_upgrade(4500));
    assert_eq!(exile.upgrades.len(), 232);
    assert_eq!(exile.types_with_upgrade(2800), [1100, 1102]);
    assert_eq!(exile.types_with_upgrade(2810), [1103, 1105, 1106, 1107]);
    assert_eq!(exile.types_with_upgrade(3376).len(), 96);
    assert!(exile.types_with_upgrade(2840).is_empty());
    assert!(exile.temp_upgrade(4450).is_some());
    assert_eq!(exile.temp_upgrades.len(), 13);
    assert_eq!(exile.temp_upgrade(4451).unwrap().msg_start.code, 0x601);
    assert!(exile.temp_upgrade(4449).is_none());
    // Multiplayer maps lock nothing via level data.
    assert!(!exile.is_guid_disabled(0, 1100));
    assert_eq!(exile.level_daytime(), 0);
    // Campaign 1-2: slot 6 has upgrade 2801 with config 1 = 0 ("deactivated") in the value set.
    let kamp = rules_of("data/levels/kampagne1/kamp1-2/kamp1-2.lvl");
    assert_eq!(
        kamp.type_value_set(6, 2801).and_then(|s| s.get(&1)),
        Some(&0)
    );
    assert!(kamp.is_guid_disabled(6, 2801));
    // Slot 0 does not lock 2801, and a type without config 1 counts as free.
    assert!(!kamp.is_guid_disabled(0, 2801));
    assert!(!kamp.is_guid_disabled(0, 1100));
    // Time of day of the level (`level_resources[40]`): campaign 2-1 has 2.
    assert_eq!(
        rules_of("data/levels/kampagne2/kamp2-1/kamp2-1.lvl").level_daytime(),
        2
    );
}

/// re/heroes-resources 1.2, 9.1: hero list in the rules, hero index and the missing elite bit of
/// 2401 (W-274).
// QUIRK W-273, W-274: kept
#[test]
fn rules_know_the_heroes() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    assert_eq!(rules.heroes.len(), 38);
    for (guid, index) in [
        (1200, 0),
        (1201, 1),
        (2402, 12),
        (2720, 14),
        (2725, 19),
        (1112, 20),
        (2616, 37),
        (1100, -1),
        (1114, -1),
    ] {
        assert_eq!(rules.hero_index(guid), index, "{guid}");
    }
    // `ELITEUNIT` (`0x800000`): all entries except 2401 (Madeleine Carpenter, `0x2000000`).
    const ELITE_UNIT: u32 = 0x80_0000;
    let without: Vec<u32> = rules
        .heroes
        .iter()
        .copied()
        .filter(|g| rules.types[g].category() & ELITE_UNIT == 0)
        .collect();
    assert_eq!(without, [2401]);
    assert_eq!(rules.types[&2401].category(), 0x200_0000);
}

/// `flowcontroltragets.ini`: the MY filters of the AI filter IDs below 1000 (`FUN_0047E390`).
#[test]
fn flowcontrol_targets_are_loaded_from_the_game() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/kampagne1/kamp1-1/kamp1-1.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    let entries = &rules.flowcontrol_targets;
    assert_eq!(entries.len(), 73);
    let ids: BTreeSet<i32> = entries.iter().map(|e| e.button_msg).collect();
    assert_eq!(ids.len(), 73, "no duplicate IDs");
    assert_eq!(
        (ids.first(), ids.last()),
        (Some(&1), Some(&188)),
        "IDs 1 to 188"
    );
    assert!(
        entries
            .iter()
            .all(|e| e.my.mode & 0x200_0000 == 0 && e.my.next.is_none()),
        "no MM_CHECKFOG, no chain"
    );
    let with_mode2: Vec<i32> = entries
        .iter()
        .filter(|e| e.my.mode2 != 0)
        .map(|e| e.button_msg)
        .collect();
    assert_eq!(with_mode2, [183], "exactly one entry with mode2 != 0");
    assert!(rules.flowcontrol_target(183).is_some());
    assert!(rules.flowcontrol_target(999).is_none());
}

/// `water_tiles` counts the tiles whose highest corner height lies below the water level
/// (`GameEngine_ClassifyAllTiles` `0x00432140`, re/ki-queries 5.16, E-83); `tile_count` is
/// `DIM_X * DIM_Z`. Neither goes into the checksum of the rules (derived).
// QUIRK E-83: kept
#[test]
fn water_tiles_count_fully_submerged_tiles() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let water = |lvl: &str| {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
        assert_eq!(rules.tile_count, rules.map.dim_x * rules.map.dim_z, "{lvl}");
        (rules.water_tiles, rules.tile_count)
    };
    // Multiplayer map with water (Donut Isle): 0 < water < tiles.
    let (w, n) = water("data/levels/multiplayer/[4players]-donut-isle.lvl");
    assert_eq!((w, n), (6083, 16384));
    // Multiplayer map without water (Crossrail): 0.
    let (w, n) = water("data/levels/multiplayer/[2players]-crossrail.lvl");
    assert_eq!((w, n), (0, 16384));
    // For all multiplayer maps: 0 <= water <= tiles.
    for lvl in level_paths(&vfs)
        .into_iter()
        .filter(|p| p.contains("/multiplayer/"))
    {
        let (w, n) = water(&lvl);
        assert!((0..=n).contains(&w), "{lvl}");
    }
}

/// The rules checksum of fixed maps (`.lvl` path of `Rules::load`, setup shared with `Rules::load_random`)
/// matches the measured values: German edition with patch, one multiplayer and one campaign map (with level script).
/// Changed (intended): the rules checksum is the combination of `logic` (the files read, now including
/// the `.tlm` and the model and animation files of the derived tables), `derived` and `speech`;
/// before: `0xa615_c20f_4815_21f0` (Exile) and `0x6b27_db0d_7775_cd62` (campaign 1-1).
#[test]
fn fixed_map_rules_checksum_matches_the_measurement() {
    use opennml_rda::testutil::{GameVersion, game_version};
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let measured = [
        (
            "data/levels/multiplayer/[4players]-exile.lvl",
            0x507f_fa95_f50c_8c26_u64,
        ),
        (
            "data/levels/kampagne1/kamp1-1/kamp1-1.lvl",
            0xfa9c_6bae_1f3c_dd43,
        ),
    ];
    for (lvl, expected) in measured {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene, "DEU").unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
        assert!(rules.random_map.is_none(), "{lvl}");
        match game_version(&vfs).unwrap() {
            GameVersion::German => assert_eq!(rules.checksum, expected, "{lvl}"),
            // No measurement of the English edition: only equal on repetition.
            GameVersion::English => assert_eq!(
                Rules::load(&vfs, &scene, "DEU").unwrap().checksum,
                rules.checksum,
                "{lvl}"
            ),
        }
    }
}
