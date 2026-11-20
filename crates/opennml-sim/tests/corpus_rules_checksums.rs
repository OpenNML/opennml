//! The parts of the rules checksum (`logic`, `derived`, `speech`) against the shipped data: which input changes which
//! part, and the German edition with patch against a second installation (`NML_GAME_DIR_SECOND`, e.g. the Steam
//! edition), which differs only in the speech files.

use std::path::{Path, PathBuf};

use opennml_rda::Vfs;
use opennml_rda::testutil::{corpus_game_dir, with_loose_file};
use opennml_sim::rules::{RuleChecksums, Rules};
use opennml_world::level::{LevelScene, level_paths, load_level};

/// Campaign map with a level script and speech.
const LEVEL: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
/// Type with time markers from its model (alarm post).
const MODEL_TYPE: u32 = 1610;

/// `vfs` with the game file `game_path` replaced by `bytes` (written to `dir`).
fn replaced(vfs: &Vfs, dir: &Path, game_path: &str, bytes: &[u8]) -> Vfs {
    let disk = dir.join(game_path.replace('/', "_"));
    std::fs::write(&disk, bytes).unwrap();
    with_loose_file(vfs, game_path, &disk)
}

fn checksums(vfs: &Vfs, scene: &LevelScene) -> RuleChecksums {
    let rules = Rules::load(vfs, scene).unwrap();
    assert_eq!(rules.checksum, rules.checksums.full());
    rules.checksums
}

#[test]
fn checksums_split_by_input() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let base = rules.checksums;
    assert_eq!(checksums(&vfs, &scene), base, "equal on repetition");
    let tmp = tempfile::tempdir().unwrap();

    // A speech file changes only `speech`.
    let (&id, _) = rules
        .scripts
        .speech
        .iter()
        .find(|(_, len)| len.ogg_ms > 0)
        .expect("speech line with a speech file");
    let ogg = format!("data/sprache/{id}.ogg");
    let other = checksums(&replaced(&vfs, tmp.path(), &ogg, b"no ogg"), &scene);
    assert_eq!((other.logic, other.derived), (base.logic, base.derived));
    assert_ne!(other.speech, base.speech);

    // A file under `data/txt/` leaves `logic` alone; the text lengths change `speech`.
    let select = "data/txt/selectlanguage.ini";
    let other = checksums(
        &replaced(&vfs, tmp.path(), select, b"data/txt/missing.deu"),
        &scene,
    );
    assert_eq!((other.logic, other.derived), (base.logic, base.derived));
    assert_ne!(other.speech, base.speech, "the text table was read");

    // A model file from which a derived table comes changes `logic`.
    assert!(rules.anim_labels.contains_key(&MODEL_TYPE));
    let model = &rules.models[&MODEL_TYPE];
    let mut bytes = vfs.read(model).unwrap();
    bytes.push(0);
    let other = checksums(&replaced(&vfs, tmp.path(), model, &bytes), &scene);
    assert_ne!(other.logic, base.logic);
    assert_eq!(other.speech, base.speech);

    // `property.ini` changes `logic` (the types come from the scene, nothing else changes).
    let property = opennml_formats::property::PROPERTY_PATH;
    let mut bytes = vfs.read(property).unwrap();
    bytes.extend_from_slice(b"\r\n");
    let other = checksums(&replaced(&vfs, tmp.path(), property, &bytes), &scene);
    assert_ne!(other.logic, base.logic);
    assert_eq!((other.derived, other.speech), (base.derived, base.speech));
}

/// Second installation for the comparison of editions: `NML_GAME_DIR_SECOND`.
fn second_game_dir() -> Option<PathBuf> {
    std::env::var_os("NML_GAME_DIR_SECOND")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
}

/// The German edition with patch and the Steam edition differ only in `data/sprache`: `logic` and `derived` are equal
/// on every level, `speech` differs on the 32 campaign and tutorial missions and is equal on the other 23 maps.
#[test]
fn german_and_steam_have_equal_logic() {
    let (Some(first), Some(second)) = (corpus_game_dir(), second_game_dir()) else {
        eprintln!("Two installations not found (NML_GAME_DIR_SECOND) - corpus test skipped");
        return;
    };
    let a = Vfs::open_game_dir(&first).unwrap();
    let b = Vfs::open_game_dir(&second).unwrap();
    let levels = level_paths(&a);
    assert_eq!(levels, level_paths(&b));
    let (mut differ, mut equal) = (Vec::new(), Vec::new());
    for lvl in &levels {
        let of = |vfs: &Vfs| {
            let scene = load_level(vfs, lvl).unwrap();
            Rules::load(vfs, &scene).unwrap_or_else(|e| panic!("{lvl}: {e:#}"))
        };
        let (ra, rb) = (of(&a), of(&b));
        let (ca, cb) = (ra.checksums, rb.checksums);
        assert_eq!((ca.logic, ca.derived), (cb.logic, cb.derived), "{lvl}");
        if ca.speech == cb.speech {
            assert_eq!(ra.checksum, rb.checksum, "{lvl}");
            equal.push(lvl);
        } else {
            assert!(!ra.scripts.speech.is_empty(), "{lvl}");
            differ.push(lvl);
        }
    }
    eprintln!("speech differs: {differ:?}");
    assert_eq!((differ.len(), equal.len()), (32, 23), "equal: {equal:?}");
}
