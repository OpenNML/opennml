//! Parses all .cfg/.lvl/.pef/.wld/.prj files and default.dat of the original installation.
//! Skipped if it is missing.

use opennml_formats::gifcon::{HEROES_PATH, parse_heroes};
use opennml_formats::lightdata::{LIGHTDATA_PATH, find_preset, parse_lightdata};
use opennml_formats::prj::parse_project;
use opennml_formats::property::{
    PROPERTY_PATH, TRANSLATE_PATH, expand_guid, parse_property_ini, parse_translate,
};
use opennml_formats::text::{decode_cp1252, parse_blocks};
use opennml_rda::testutil::corpus_game_dir;
use opennml_rda::{Vfs, normalize_path};

#[test]
fn all_brace_text_files_parse() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut parsed = 0;
    let mut failures = Vec::new();
    for (archive, entry) in vfs.files() {
        let lower = normalize_path(&entry.path);
        let is_brace_text = [".cfg", ".lvl", ".pef", ".wld", ".prj"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
            || lower == "data/projects/default.dat";
        if !is_brace_text {
            continue;
        }
        let text = decode_cp1252(&archive.read(entry).unwrap());
        match parse_blocks(&text) {
            Ok(_) => parsed += 1,
            Err(e) => failures.push(format!("{}: {e}", entry.path)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} errors, e.g.:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
    assert!(parsed > 1860, "only {parsed} files parsed");
}

#[test]
fn lightdata_has_all_32_presets() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let presets = parse_lightdata(&decode_cp1252(&vfs.read(LIGHTDATA_PATH).unwrap()));
    assert_eq!(presets.len(), 32);
    assert_eq!(presets[0].name, "Sommer Tag");
    assert_eq!(presets[31].name, "Spa 5 Morgen");
    // Names used by the scripts (docs/formats/lightdata.md).
    for name in [
        "Sommer Tag",
        "Sommer Nacht",
        "2",
        "5",
        "6",
        "9",
        "10",
        "18",
        "20",
        "22",
        "23",
        "wid 2 Schnee Nacht",
        "PAt 4 Regen",
        "Spa 5 Nacht",
        "Spa 5 Morgen",
    ] {
        assert!(find_preset(&presets, name).is_some(), "{name} is missing");
    }
    let nacht = find_preset(&presets, "sommer nacht").unwrap();
    assert_eq!(
        (nacht.terrain.color, nacht.terrain.intensity),
        ([82, 90, 93], 15)
    );
    assert_eq!(
        (nacht.object.color, nacht.object.intensity),
        ([32, 88, 145], 65)
    );
}

#[test]
fn object_types_translations_and_project_shadow_files() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let read = |path: &str| decode_cp1252(&vfs.read(path).unwrap());
    // re/object-classes: 487 types, 47 different module indices.
    let types = parse_property_ini(&read(PROPERTY_PATH));
    assert_eq!(types.len(), 487);
    let mut indices: Vec<u32> = types
        .values()
        .flat_map(|t| t.modules.iter().map(|m| m.index))
        .collect();
    indices.sort_unstable();
    indices.dedup();
    assert_eq!(indices.len(), 47);
    let hq = &types[&1000];
    assert_eq!(
        (hq.name.as_str(), hq.category()),
        ("SPA_Hauptquartier", 0x200_0000)
    );
    assert_eq!(
        types[&3500].module(6).unwrap().value_int(70002),
        Some(0x500)
    );
    // 18 riders, all "X on horse" (two parts, both types present).
    let translations = parse_translate(&read(TRANSLATE_PATH));
    assert_eq!(translations.len(), 18);
    for &guid in translations.keys() {
        let parts = expand_guid(&translations, guid);
        assert_eq!(parts.len(), 2, "{guid}");
        assert!(parts.iter().all(|p| types.contains_key(p)), "{guid}");
    }
    assert_eq!(expand_guid(&translations, 1101), [1100, 3409]);
    // re/shadows: 462 types with shadow model, 75 with ground plane (always together).
    let project = parse_project(&read("data/projects/nml-wiese.prj")).unwrap();
    assert_eq!(project.objects.len(), 512);
    let shadow = project
        .objects
        .values()
        .filter(|o| o.shadow_model.is_some());
    assert_eq!(shadow.count(), 462);
    let ground: Vec<_> = project
        .objects
        .values()
        .filter(|o| o.groundplane_model.is_some())
        .collect();
    assert_eq!(ground.len(), 75);
    assert!(ground.iter().all(|o| o.shadow_model.is_some()));
    for o in &ground {
        let path = normalize_path(o.groundplane_model.as_deref().unwrap());
        assert!(path.ends_with(".rdo") && vfs.exists(&path), "{path}");
    }
}

/// re/upgrades 1.1, W-222: 232 `[UPGRADE]`, types per upgrade from `OBJ_UPGRADES`.
// QUIRK W-222: kept
#[test]
fn nml_wiese_lists_upgrades_and_their_types() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let project = parse_project(&decode_cp1252(
        &vfs.read("data/projects/nml-wiese.prj").unwrap(),
    ))
    .unwrap();
    assert_eq!(project.upgrades.len(), 232);
    let types_with = |guid: u32| -> Vec<u32> {
        project
            .objects
            .values()
            .filter(|o| o.upgrades.contains(&guid))
            .map(|o| o.guid)
            .collect()
    };
    assert_eq!(types_with(2800), [1100, 1102]);
    assert_eq!(types_with(3376).len(), 96);
    // W-222: 2918 (WID) is carried by the SPA tower 1010.
    assert!(types_with(2918).contains(&1010));
    // W-222: 2820 is not researchable, 2840 undefined - neither appears in any `[UPGRADE]`.
    assert!(!project.upgrades.contains_key(&2820));
    assert!(!project.upgrades.contains_key(&2840));
    assert!(project.upgrades.contains_key(&2800));
}

/// `docs/formats/gifcon-upgrades.md`, re/temp-upgrades 1.4: 13 records 4450 ... 4462 with the flags of the
/// shipped data.
#[test]
fn tempupgrades_ini_has_thirteen_records() {
    use opennml_formats::temp_upgrades::{TEMP_UPGRADES_PATH, parse_temp_upgrades};
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let records =
        parse_temp_upgrades(&decode_cp1252(&vfs.read(TEMP_UPGRADES_PATH).unwrap())).unwrap();
    let guids: Vec<u32> = records.iter().map(|r| r.guid).collect();
    assert_eq!(guids, (4450..=4462).collect::<Vec<u32>>());
    let flags = |guid: u32| records.iter().find(|r| r.guid == guid).unwrap().flags;
    assert_eq!(flags(4450), 0x4E1);
    assert_eq!(flags(4451), 0xE7);
    assert_eq!(flags(4452), 0xD9);
    assert_eq!(flags(4456), 0x2D9);
    assert_eq!(flags(4460), 0x49);
    // MSGTICK 57F {0, 1000} (taxes), MSGSTART/MSGEND 601 {300}/{100} (construction speed).
    let steuern = &records[0];
    assert_eq!(
        (steuern.msg_end.code, steuern.msg_end.args.as_slice()),
        (0x57F, &[0, 1000][..])
    );
    let bau = &records[1];
    assert_eq!(
        (bau.msg_start.code, bau.msg_start.args.as_slice()),
        (0x601, &[300][..])
    );
    assert_eq!(
        (bau.msg_end.code, bau.msg_end.args.as_slice()),
        (0x601, &[100][..])
    );
}

/// re/heroes-resources 1.2: 38 entries in file order.
// QUIRK W-273: kept
#[test]
fn heroes_ini_lists_38_entries() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let heroes = parse_heroes(&decode_cp1252(&vfs.read(HEROES_PATH).unwrap()));
    assert_eq!(heroes.len(), 38);
    for (i, guid) in [
        (0, 1200),
        (1, 1201),
        (12, 2402),
        (14, 2720),
        (19, 2725),
        (20, 1112),
        (37, 2616),
    ] {
        assert_eq!(heroes[i], guid, "index {i}");
    }
}
