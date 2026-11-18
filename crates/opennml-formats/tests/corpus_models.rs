//! Reads all .rdo/.rdw/.rdm and object .cfg files of the original installation and compares tracks
//! with bone and mesh names.

use std::collections::{BTreeMap, BTreeSet};

use opennml_formats::object_cfg::parse_object_cfg;
use opennml_formats::rdm::{RdmFile, RdmLayout, bone_track_key, parse_rdm};
use opennml_formats::rdo::{RdoFile, RdoLayout, parse_rdo};
use opennml_formats::text::decode_cp1252;
use opennml_rda::testutil::corpus_game_dir;
use opennml_rda::{Vfs, normalize_path};

// QUIRK D-264: kept
#[test]
fn all_models_animations_and_object_configs_parse() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut models: BTreeMap<String, RdoFile> = BTreeMap::new();
    let mut animations: BTreeMap<String, RdmFile> = BTreeMap::new();
    let mut configs = BTreeMap::new();
    let mut failures = Vec::new();
    for (archive, entry) in vfs.files() {
        let path = normalize_path(&entry.path);
        let is_model = path.ends_with(".rdo") || path.ends_with(".rdw");
        let is_object_cfg = path.starts_with("data/r3d/")
            && path.ends_with(".cfg")
            && !path.ends_with("/viewer.cfg");
        if !(is_model || path.ends_with(".rdm") || is_object_cfg) {
            continue;
        }
        let bytes = archive.read(entry).unwrap();
        let result = if is_model {
            parse_rdo(&bytes).map(|m| {
                models.insert(path.clone(), m);
            })
        } else if path.ends_with(".rdm") {
            parse_rdm(&bytes).map(|a| {
                animations.insert(path.clone(), a);
            })
        } else {
            parse_object_cfg(&decode_cp1252(&bytes)).map(|c| {
                configs.insert(path.clone(), c);
            })
        };
        if let Err(e) = result {
            failures.push(format!("{path}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} errors:\n{}",
        failures.len(),
        failures.join("\n")
    );
    let count = |suffix: &str| models.keys().filter(|p| p.ends_with(suffix)).count();
    assert_eq!((count(".rdo"), count(".rdw")), (994, 99));
    assert_eq!(animations.len(), 1581);
    assert_eq!(configs.len(), 547);

    let legacy_models: Vec<&str> = models
        .iter()
        .filter(|(_, m)| m.layout == RdoLayout::Legacy)
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(
        legacy_models,
        [
            "data/r3d/animals/bär/bear.rdw",
            "data/rdm/kanone_spa_stehen.rdo"
        ]
    );
    let legacy_animations: Vec<&str> = animations
        .iter()
        .filter(|(_, a)| a.layout == RdmLayout::Legacy)
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(legacy_animations, ["data/r3d/pri/units/kanu/ruder.rdm"]);

    // Cross references: model via OBJECT/FILE, bones as the set of track keys.
    let mut stats: BTreeMap<&str, usize> = BTreeMap::new();
    let mut missing_files = Vec::new();
    let mut missing_sub_objects = Vec::new();
    for (cfg_path, cfg) in &configs {
        // The game silently skips missing sub-objects (linked only if FILE is loaded);
        // they are only reported.
        for sub in cfg
            .sub_objects
            .iter()
            .chain(cfg.animations.iter().flat_map(|a| &a.sub_objects))
        {
            if !vfs.exists(&sub.file) {
                missing_sub_objects.push(format!("{cfg_path}: {}", sub.file));
            }
        }
        let Some(model) = cfg
            .model_file
            .as_deref()
            .and_then(|f| models.get(&normalize_path(f)))
        else {
            missing_files.push(format!("{cfg_path}: OBJECT FILE {:?}", cfg.model_file));
            continue;
        };
        let bones: BTreeSet<String> = model
            .bones
            .iter()
            .map(|b| bone_track_key(&b.name))
            .collect();
        for entry in &cfg.animations {
            let Some(file) = entry.file.as_deref() else {
                continue;
            };
            let Some(anim) = animations.get(&normalize_path(file)) else {
                missing_files.push(format!("{cfg_path}: ANIMATION {file}"));
                continue;
            };
            let tracks: BTreeSet<String> = anim
                .bone_tracks
                .iter()
                .map(|t| bone_track_key(&t.name))
                .collect();
            if !bones.is_empty() {
                *stats.entry("bones_with_track").or_default() +=
                    bones.intersection(&tracks).count();
                *stats.entry("bones_without_track").or_default() +=
                    bones.difference(&tracks).count();
            }
            for node in &anim.node_tracks {
                let found = model.meshes.iter().any(|m| m.name == node.name);
                *stats
                    .entry(if found {
                        "node_track_found"
                    } else {
                        "node_track_missing"
                    })
                    .or_default() += 1;
            }
            for morph in &anim.morph_tracks {
                let key = match model.meshes.iter().find(|m| m.name == morph.name) {
                    None => "morph_track_missing",
                    Some(m)
                        if morph
                            .frames
                            .iter()
                            .all(|f| f.positions.len() == m.positions.len()) =>
                    {
                        "morph_track_fits"
                    }
                    Some(_) => "morph_track_differs",
                };
                *stats.entry(key).or_default() += 1;
            }
        }
    }
    eprintln!(
        "{} missing sub-objects: {missing_sub_objects:#?}",
        missing_sub_objects.len()
    );
    assert!(missing_files.is_empty(), "{missing_files:#?}");
    let expected: BTreeMap<&str, usize> = [
        ("bones_with_track", 30006),
        ("bones_without_track", 99),
        ("node_track_found", 2210),
        ("node_track_missing", 166),
        ("morph_track_fits", 213),
        ("morph_track_missing", 6),
    ]
    .into();
    assert_eq!(stats, expected);
}
