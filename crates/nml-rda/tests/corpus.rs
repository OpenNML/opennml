//! Checks all archives of the original installation. Skipped if it is missing.

use nml_rda::Vfs;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};

fn open() -> Option<Vfs> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    Some(Vfs::open_game_dir(&dir).unwrap())
}

#[test]
fn every_file_in_every_archive_unpacks() {
    let Some(vfs) = open() else { return };
    assert_eq!(vfs.archives().len(), 11);
    let mut total = 0;
    for archive in vfs.archives() {
        for entry in archive.entries() {
            let data = archive.read(entry).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(data.len(), entry.size as usize, "{}", entry.path);
            total += 1;
        }
    }
    // The German patch replaces `NML4.RDA` (4 more entries); the other archives are the same.
    let (entries, effective) = match game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) {
        GameVersion::English => (8370, 8351),
        GameVersion::German => (8374, 8355),
    };
    assert_eq!(total, entries, "total number of entries");
    assert_eq!(
        vfs.len(),
        effective,
        "number of effective files (19 duplicates)"
    );
}

#[test]
fn umlaut_paths_and_patch_overrides_resolve() {
    let Some(vfs) = open() else { return };
    assert!(!vfs.read(r".\Data\Config\Gebäude.cfg").unwrap().is_empty());
    assert!(vfs.get("data/menü/account2.dlg").is_some());
    let (archive, _) = vfs
        .get("data/levels/kampagne1/kamp1-6/kamp1-6.lvl")
        .unwrap();
    assert_eq!(
        archive.path().file_name().unwrap().to_ascii_lowercase(),
        "nml10.rda"
    );
}

#[test]
fn loose_media_files_resolve_from_disk() {
    let Some(vfs) = open() else { return };
    // Music and videos exist only as loose files (`DATA/MUSIC`, `DATA/Videos`), not in archives.
    for path in ["data/music/title.ogg", "data/videos/intro.bik"] {
        assert!(vfs.get(path).is_none(), "{path} in an archive");
        let file = vfs
            .loose_file(path)
            .unwrap_or_else(|| panic!("{path} not found as a loose file"));
        assert!(file.is_file(), "{}", file.display());
    }
    assert!(vfs.loose_file("data/config/gebäude.cfg").is_none());
}
