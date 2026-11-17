//! Virtual file system over all archives of an installation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
#[derive(Debug)]
pub struct Vfs {
    archives: Vec<Archive>,
    index: BTreeMap<String, (usize, usize)>,
    /// Loose files in the game directory (normalized path -> file), read once when opening;
    /// empty for [`Vfs::from_archives`].
    loose: BTreeMap<String, PathBuf>,
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
}
