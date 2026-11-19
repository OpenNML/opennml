//! Virtual file system over all archives of an installation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{Archive, Entry, RdaError};

/// Normalizes a game path: surrounding whitespace removed, `\` -> `/`, empty and `.` segments
/// removed (including a leading `./` and `/`), lowercase. `..` is kept (see
/// [`Vfs::loose_file`]).
pub fn normalize_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase()
}

/// All archives of an installation plus loose files in the game directory.
///
/// Rules as in the original (`docs/formats/rda.md`): archives `nml0.rda`, `nml1.rda`, ... are
/// loaded in ascending order up to the first missing number, a later archive overrides
/// earlier entries with the same path, and a loose file on disk takes precedence over all
/// archives.
#[derive(Debug, Clone)]
pub struct Vfs {
    archives: Vec<Archive>,
    index: BTreeMap<String, (usize, usize)>,
    /// Loose files in the game directory (normalized path -> file), read once when opening;
    /// empty for [`Vfs::from_archives`].
    loose: BTreeMap<String, PathBuf>,
    /// Set in clones made by [`Vfs::recording`]: every [`Vfs::read`] is noted here.
    log: Option<ReadLog>,
}

/// One file read through a recording [`Vfs`] (see [`Vfs::recording`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadEntry {
    /// Normalized game path ([`normalize_path`]), as the lookup in [`Vfs::read`] uses it.
    pub path: String,
    /// FNV-1a 64 of the bytes read; `None` if reading failed (missing file, I/O error). If a path
    /// was read several times, the last result counts.
    pub hash: Option<u64>,
}

/// Shared record of the reads of a recording [`Vfs`] and its clones.
#[derive(Debug, Clone, Default)]
pub struct ReadLog {
    entries: Arc<Mutex<BTreeMap<String, Option<u64>>>>,
}

impl ReadLog {
    /// The files read so far, ascending by path, one entry per path.
    pub fn take(&self) -> Vec<ReadEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .map(|(path, hash)| ReadEntry {
                path: path.clone(),
                hash: *hash,
            })
            .collect()
    }

    fn note(&self, path: String, hash: Option<u64>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(path, hash);
    }
}

/// FNV-1a 64 (offset `0xcbf29ce484222325`, prime `0x100000001b3`).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl Vfs {
    /// Opens `nml0.rda`, `nml1.rda`, ... (case-insensitive) up to the first missing number
    /// and includes the loose files of the directory.
    pub fn open_game_dir(dir: impl AsRef<Path>) -> Result<Self, RdaError> {
        let dir = dir.as_ref();
        let io_err = |e: std::io::Error| RdaError::Io {
            path: dir.to_path_buf(),
            source: e,
        };
        let mut items = Vec::new();
        for item in std::fs::read_dir(dir).map_err(io_err)? {
            let item = item.map_err(io_err)?;
            items.push((
                item.file_name().to_string_lossy().to_lowercase(),
                item.path(),
            ));
        }
        // For names that differ only in case (possible only on case-sensitive file systems), the
        // smallest path wins.
        items.sort();
        let mut by_name = BTreeMap::new();
        for (name, path) in items {
            by_name.entry(name).or_insert(path);
        }
        let mut archives = Vec::new();
        while let Some(path) = by_name.remove(&format!("nml{}.rda", archives.len())) {
            archives.push(Archive::open(path)?);
        }
        if archives.is_empty() {
            return Err(RdaError::NoArchives(dir.to_path_buf()));
        }
        let mut vfs = Self::from_archives(archives);
        vfs.loose = index_loose_files(dir)?;
        Ok(vfs)
    }

    /// Builds the VFS from archives in load order (later ones override earlier ones), without loose
    /// files.
    pub fn from_archives(archives: Vec<Archive>) -> Self {
        let mut index = BTreeMap::new();
        for (ai, archive) in archives.iter().enumerate() {
            for (ei, entry) in archive.entries().iter().enumerate() {
                index.insert(normalize_path(&entry.path), (ai, ei));
            }
        }
        Self {
            archives,
            index,
            loose: BTreeMap::new(),
            log: None,
        }
    }

    pub fn archives(&self) -> &[Archive] {
        &self.archives
    }

    /// Number of effective archive files (without loose files).
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Looks up a file in the archives (for loose files see [`Vfs::loose_file`]); case,
    /// separators, and a leading `./` do not matter.
    pub fn get(&self, path: &str) -> Option<(&Archive, &Entry)> {
        let &(ai, ei) = self.index.get(&normalize_path(path))?;
        let archive = &self.archives[ai];
        Some((archive, &archive.entries()[ei]))
    }

    /// Loose file in the game directory for a game path (case-insensitive). The index is built
    /// when opening; the VFS does not see files created later. Paths with `..` never find
    /// anything.
    pub fn loose_file(&self, path: &str) -> Option<PathBuf> {
        self.loose.get(&normalize_path(path)).cloned()
    }

    /// Does the file exist, loose or in an archive?
    pub fn exists(&self, path: &str) -> bool {
        let key = normalize_path(path);
        self.loose.contains_key(&key) || self.index.contains_key(&key)
    }

    /// Reads a file: first loose from disk, otherwise from the effective archive.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, RdaError> {
        let result = self.read_unrecorded(path);
        if let Some(log) = &self.log {
            log.note(
                normalize_path(path),
                result.as_ref().ok().map(|bytes| fnv1a64(bytes)),
            );
        }
        result
    }

    /// A clone that notes every [`Vfs::read`] in the returned [`ReadLog`]; the original stays
    /// unchanged and notes nothing. Clones of the clone share the log.
    pub fn recording(&self) -> (Vfs, ReadLog) {
        let log = ReadLog::default();
        let mut vfs = self.clone();
        vfs.log = Some(log.clone());
        (vfs, log)
    }

    /// Normalized paths of the loose files that shadow an archive entry of the same name,
    /// ascending.
    pub fn overriding_loose_files(&self) -> Vec<String> {
        self.loose
            .keys()
            .filter(|key| self.index.contains_key(*key))
            .cloned()
            .collect()
    }

    /// Clone with the loose file `game_path` pointing to `disk` (added or replaced); for tests
    /// that need a second installation without copying one.
    pub(crate) fn with_loose(&self, game_path: &str, disk: &Path) -> Vfs {
        let mut vfs = self.clone();
        vfs.loose
            .insert(normalize_path(game_path), disk.to_path_buf());
        vfs
    }

    fn read_unrecorded(&self, path: &str) -> Result<Vec<u8>, RdaError> {
        if let Some(file) = self.loose_file(path) {
            return std::fs::read(&file).map_err(|source| RdaError::Io { path: file, source });
        }
        let (archive, entry) = self
            .get(path)
            .ok_or_else(|| RdaError::NotFound(path.to_string()))?;
        archive.read(entry)
    }

    /// All effective archive files (without loose files), sorted by normalized path.
    pub fn files(&self) -> impl Iterator<Item = (&Archive, &Entry)> + '_ {
        self.index.values().map(|&(ai, ei)| {
            let archive = &self.archives[ai];
            (archive, &archive.entries()[ei])
        })
    }
}

/// Reads all files under `dir` recursively (key: normalized path relative to `dir`).
/// For paths that differ only in case (possible only on case-sensitive file systems), the
/// smallest path wins.
fn index_loose_files(dir: &Path) -> Result<BTreeMap<String, PathBuf>, RdaError> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = std::fs::read_dir(&current).map_err(|source| RdaError::Io {
            path: current.clone(),
            source,
        })?;
        for item in entries.flatten() {
            let path = item.path();
            match item.file_type() {
                Ok(t) if t.is_dir() => pending.push(path),
                Ok(t) if t.is_file() => {
                    if let Ok(relative) = path.strip_prefix(dir) {
                        let key = normalize_path(&relative.to_string_lossy());
                        found.push((key, path));
                    }
                }
                _ => {}
            }
        }
    }
    found.sort();
    let mut index = BTreeMap::new();
    for (key, path) in found {
        index.entry(key).or_insert(path);
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TestFile, build_rda};

    fn game_dir(archives: &[(&str, &[TestFile<'_>])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, files) in archives {
            std::fs::write(dir.path().join(name), build_rda(files)).unwrap();
        }
        dir
    }

    #[test]
    fn normalize_path_unifies_game_spellings() {
        assert_eq!(
            normalize_path(r".\Data\R3D\Wid\x.RDO"),
            "data/r3d/wid/x.rdo"
        );
        assert_eq!(normalize_path("/data/Menü/A.dlg"), "data/menü/a.dlg");
        assert_eq!(
            normalize_path("data/config/gebäude.cfg"),
            "data/config/gebäude.cfg"
        );
    }

    #[test]
    fn normalize_path_collapses_and_trims() {
        assert_eq!(normalize_path(" data//r3d/./x.rdo "), "data/r3d/x.rdo");
        assert_eq!(normalize_path(r".\.\Data\\Levels\"), "data/levels");
        assert_eq!(normalize_path("a/../b"), "a/../b");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn exists_sees_loose_and_archive_files() {
        let dir = game_dir(&[(
            "NML0.RDA",
            &[TestFile {
                path: "data/a.txt",
                data: b"x",
                compress: false,
            }],
        )]);
        std::fs::create_dir(dir.path().join("Data")).unwrap();
        std::fs::write(dir.path().join("Data").join("Loose.TXT"), b"l").unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert!(vfs.exists(r".\DATA\A.txt"));
        assert!(vfs.exists("data/loose.txt"));
        assert!(!vfs.exists("data/missing.txt"));
        assert!(!vfs.exists("data"));
    }

    /// Empty archives `NML<n>.RDA` for all numbers in `numbers`.
    fn add_empty_archives(dir: &Path, numbers: impl IntoIterator<Item = u32>) {
        for n in numbers {
            std::fs::write(dir.join(format!("NML{n}.RDA")), build_rda(&[])).unwrap();
        }
    }

    #[test]
    fn later_archive_number_overrides_earlier() {
        // As in the original (RdaVfs_LoadArchives @ 0x0057E830): nml0, nml1, ... in ascending
        // order, a later archive overrides the map entry of an earlier one.
        let dir = game_dir(&[
            (
                "nml10.rda",
                &[TestFile {
                    path: "data/a.txt",
                    data: b"ten",
                    compress: true,
                }],
            ),
            (
                "NML2.RDA",
                &[
                    TestFile {
                        path: "data/a.txt",
                        data: b"two",
                        compress: false,
                    },
                    TestFile {
                        path: "data/b.txt",
                        data: b"only two",
                        compress: false,
                    },
                ],
            ),
        ]);
        add_empty_archives(dir.path(), (0..10).filter(|&n| n != 2));
        std::fs::write(dir.path().join("NML.exe"), b"no archive").unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(vfs.archives().len(), 11);
        assert_eq!(vfs.read("data/a.txt").unwrap(), b"ten");
        assert_eq!(vfs.read("data/b.txt").unwrap(), b"only two");
        assert_eq!(vfs.len(), 2);
    }

    #[test]
    fn archive_numbers_stop_at_first_gap() {
        // The original stops at the first missing `nml<n>.rda`; later numbers are not
        // loaded, nor are spellings with a leading zero.
        let dir = game_dir(&[
            (
                "NML3.RDA",
                &[TestFile {
                    path: "data/three.txt",
                    data: b"3",
                    compress: false,
                }],
            ),
            (
                "nml01.rda",
                &[TestFile {
                    path: "data/zero-one.txt",
                    data: b"01",
                    compress: false,
                }],
            ),
        ]);
        add_empty_archives(dir.path(), [0, 1]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(vfs.archives().len(), 2);
        assert!(vfs.get("data/three.txt").is_none());
        assert!(vfs.get("data/zero-one.txt").is_none());

        let without_zero = game_dir(&[("NML1.RDA", &[])]);
        assert!(matches!(
            Vfs::open_game_dir(without_zero.path()),
            Err(RdaError::NoArchives(_))
        ));
    }

    #[test]
    fn loose_file_overrides_archive() {
        // RdaVfs_OpenFile @ 0x0057EBC0 first tries CFile::Open on disk.
        let dir = game_dir(&[(
            "NML0.RDA",
            &[TestFile {
                path: "data/a.txt",
                data: b"archive",
                compress: true,
            }],
        )]);
        std::fs::create_dir(dir.path().join("DATA")).unwrap();
        std::fs::write(dir.path().join("DATA").join("a.txt"), b"loose").unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(vfs.read("data/a.txt").unwrap(), b"loose");
        assert_eq!(vfs.read(r".\Data\A.TXT").unwrap(), b"loose");
    }

    #[test]
    fn loose_files_without_archive_entry_are_readable() {
        let dir = game_dir(&[("NML0.RDA", &[])]);
        let music = dir.path().join("DATA").join("MUSIC");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join("TITLE.OGG"), b"ogg").unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(vfs.read("data/music/title.ogg").unwrap(), b"ogg");
        assert_eq!(
            vfs.loose_file("Data/Music/Title.ogg").unwrap(),
            music.join("TITLE.OGG")
        );
        // Directories and paths outside the game directory do not count.
        assert!(vfs.loose_file("data/music").is_none());
        assert!(vfs.loose_file("../x").is_none());
        assert!(matches!(vfs.read("data/music"), Err(RdaError::NotFound(_))));
    }

    #[test]
    fn from_archives_has_no_loose_files() {
        let dir = game_dir(&[("NML0.RDA", &[])]);
        std::fs::write(dir.path().join("a.txt"), b"loose").unwrap();
        let archive = Archive::open(dir.path().join("NML0.RDA")).unwrap();
        let vfs = Vfs::from_archives(vec![archive]);
        assert!(vfs.loose_file("a.txt").is_none());
        assert!(matches!(vfs.read("a.txt"), Err(RdaError::NotFound(_))));
    }

    #[test]
    fn lookup_ignores_case_separators_and_dot_prefix() {
        let dir = game_dir(&[(
            "NML0.RDA",
            &[TestFile {
                path: "data/config/gebäude.cfg",
                data: b"x",
                compress: true,
            }],
        )]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(vfs.read(r".\DATA\Config\GEBÄUDE.cfg").unwrap(), b"x");
        let (_, entry) = vfs.get("data/config/gebäude.cfg").unwrap();
        assert_eq!(entry.path, "data/config/gebäude.cfg");
    }

    #[test]
    fn files_are_sorted_and_deduplicated() {
        let dir = game_dir(&[
            (
                "NML0.RDA",
                &[
                    TestFile {
                        path: "b.txt",
                        data: b"1",
                        compress: false,
                    },
                    TestFile {
                        path: "a.txt",
                        data: b"1",
                        compress: false,
                    },
                ],
            ),
            (
                "NML1.RDA",
                &[TestFile {
                    path: "B.TXT",
                    data: b"2",
                    compress: false,
                }],
            ),
        ]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let paths: Vec<_> = vfs.files().map(|(_, e)| e.path.clone()).collect();
        assert_eq!(paths, ["a.txt", "B.TXT"]);
    }

    #[test]
    fn missing_file_and_empty_dir_are_errors() {
        let dir = game_dir(&[("NML0.RDA", &[])]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert!(matches!(vfs.read("absent.txt"), Err(RdaError::NotFound(p)) if p == "absent.txt"));

        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(
            Vfs::open_game_dir(empty.path()),
            Err(RdaError::NoArchives(_))
        ));
    }

    /// Installation with `data/a.txt` and `data/b.txt` in the archive.
    fn two_files() -> (tempfile::TempDir, Vfs) {
        let dir = game_dir(&[(
            "NML0.RDA",
            &[
                TestFile {
                    path: "data/a.txt",
                    data: b"alpha",
                    compress: true,
                },
                TestFile {
                    path: "data/b.txt",
                    data: b"beta",
                    compress: false,
                },
            ],
        )]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    #[test]
    fn fnv1a64_matches_reference_values() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn recording_lists_every_read_once() {
        let (_dir, vfs) = two_files();
        let (rec, log) = vfs.recording();
        assert_eq!(rec.read("data/a.txt").unwrap(), b"alpha");
        assert_eq!(rec.read(r".\DATA\A.TXT").unwrap(), b"alpha");
        assert!(rec.read("data/missing.txt").is_err());
        assert_eq!(
            log.take(),
            [
                ReadEntry {
                    path: "data/a.txt".into(),
                    hash: Some(fnv1a64(b"alpha")),
                },
                ReadEntry {
                    path: "data/missing.txt".into(),
                    hash: None,
                },
            ]
        );
    }

    #[test]
    fn recording_does_not_touch_the_original() {
        let (_dir, vfs) = two_files();
        let (rec, log) = vfs.recording();
        vfs.read("data/b.txt").unwrap();
        assert!(log.take().is_empty());
        rec.clone().read("data/b.txt").unwrap();
        assert_eq!(log.take().len(), 1, "clones of the recorder share the log");
        let (_other, other_log) = vfs.recording();
        assert!(other_log.take().is_empty());
    }

    #[test]
    fn overriding_loose_files_lists_only_shadowed_entries() {
        let (dir, _) = two_files();
        std::fs::create_dir(dir.path().join("Data")).unwrap();
        std::fs::write(dir.path().join("Data").join("B.TXT"), b"loose b").unwrap();
        std::fs::write(dir.path().join("Data").join("A.TXT"), b"loose a").unwrap();
        std::fs::write(dir.path().join("Data").join("new.txt"), b"new").unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(vfs.overriding_loose_files(), ["data/a.txt", "data/b.txt"]);
        assert_eq!(vfs.read("data/b.txt").unwrap(), b"loose b");
    }

    #[test]
    fn clone_reads_the_same_bytes() {
        let (_dir, vfs) = two_files();
        let copy = vfs.clone();
        for path in ["data/a.txt", "data/b.txt"] {
            assert_eq!(copy.read(path).unwrap(), vfs.read(path).unwrap());
        }
        assert_eq!(copy.len(), vfs.len());
    }

    #[test]
    fn with_loose_file_adds_and_replaces() {
        let (dir, vfs) = two_files();
        let disk = dir.path().join("x.bin");
        std::fs::write(&disk, b"changed").unwrap();
        let other = crate::testutil::with_loose_file(&vfs, r"Data\A.txt", &disk);
        assert_eq!(other.read("data/a.txt").unwrap(), b"changed");
        assert_eq!(vfs.read("data/a.txt").unwrap(), b"alpha");
        assert_eq!(other.overriding_loose_files(), ["data/a.txt"]);
    }
}
