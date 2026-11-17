//! `Engine.ini` and `defaultkeys.ini` of the original installation. Skipped if it is missing.
//! The installation is only read.

use std::collections::BTreeSet;

use nml_formats::engine_ini::{
    EngineConfig, parse_engine_ini_bytes, parse_engine_ini_bytes_onto, write_engine_ini_bytes,
};
use nml_formats::keys::{DEFAULT_KEYS_PATH, parse_default_keys};
use nml_formats::text::decode_cp1252;
use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;

#[test]
fn installation_engine_ini_round_trips() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let bytes = std::fs::read(dir.join("Engine.ini")).unwrap();
    let (cfg, warnings) = parse_engine_ini_bytes(&bytes);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(cfg.key_map.len(), 460);
    assert_eq!(cfg.sound.volumes, [349, 599, 499]);
    assert_eq!(cfg.language_tag, "DEU");
    // Writing yields the same bytes as the original (one line, no line breaks).
    let written = write_engine_ini_bytes(&cfg);
    assert_eq!(
        written.len(),
        bytes.len(),
        "length: written {} instead of {}",
        written.len(),
        bytes.len()
    );
    assert!(
        written == bytes,
        "Engine.ini is not written byte-identically"
    );
}

#[test]
fn default_keys_parse() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let text = decode_cp1252(&vfs.read(DEFAULT_KEYS_PATH).unwrap());
    let keys = parse_default_keys(&text);
    assert!(!keys.is_empty());
    let ids: BTreeSet<i32> = keys.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids.len(), keys.len(), "IDs not unique");
    // The file begins with a heading (ID < 10, empty value: `0=`).
    assert_eq!(keys[0], (0, String::new()));
    assert!(keys[0].0 < 10);

    // The same 460 IDs are in the key layout of the installation (there in the order of the `CMap` of the
    // original).
    let bytes = std::fs::read(dir.join("Engine.ini")).unwrap();
    let (cfg, _) = parse_engine_ini_bytes(&bytes);
    let installed: BTreeSet<i32> = cfg.key_map.iter().map(|(id, _)| *id).collect();
    assert_eq!(installed, ids);
}

#[test]
fn installation_engine_ini_onto_the_default_keys_keeps_the_mapping() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let defaults = parse_default_keys(&decode_cp1252(&vfs.read(DEFAULT_KEYS_PATH).unwrap()));
    let bytes = std::fs::read(dir.join("Engine.ini")).unwrap();
    let (plain, _) = parse_engine_ini_bytes(&bytes);
    let base = EngineConfig::default().with_default_keys(&defaults);
    let (cfg, warnings) = parse_engine_ini_bytes_onto(base, &bytes);
    assert!(warnings.is_empty(), "{warnings:?}");
    // The base has the same 460 IDs: things are overwritten, nothing is added; the order stays that of the
    // defaults, the assignment ID -> key is that of the installation.
    assert_eq!(cfg.key_map.len(), 460);
    let ids = |c: &EngineConfig| c.key_map.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    assert_eq!(
        ids(&cfg),
        defaults.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    );
    let sorted = |c: &EngineConfig| {
        let mut v = c.key_map.clone();
        v.sort();
        v
    };
    assert_eq!(sorted(&cfg), sorted(&plain));
}
