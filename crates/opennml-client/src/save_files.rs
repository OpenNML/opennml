//! Savegames in the file system: the folder `savegame/` next to the settings file (`settings::savegame_dir`; deviation
//! from the original), files `<name>.<GUID %03X>` as in the original (`"%s/%s.%03X"`, `FUN_004C8DE0`). The list and the
//! local time for the menus are delivered by [`FsSaveFiles`] through `opennml_ui::saves::SaveFiles`; reading, writing and
//! deleting are done by `game_flow`.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local, Timelike};
use opennml_ui::saves::{LocalStamp, SaveDir, SaveEntry, SaveFiles, list_name};

/// Suffix of a savegame (`".%03X"`): an `int` GUID, negative values as unsigned 32 bits.
fn suffix(guid: i32) -> String {
    format!(".{:03X}", guid as u32)
}

/// Path of a savegame: `dir/<name>.<guid %03X>` (`FUN_004C8DE0`, `FUN_004C7D50`, `FUN_004BF9A0`).
pub fn save_path(dir: &Path, name: &str, guid: i32) -> PathBuf {
    dir.join(format!("{name}{}", suffix(guid)))
}

/// Local time of a point in time (`localtime`).
fn local_stamp(t: DateTime<Local>) -> LocalStamp {
    LocalStamp {
        year: t.year(),
        month: t.month(),
        day: t.day(),
        hour: t.hour(),
        minute: t.minute(),
    }
}

/// The folder `savegame/` of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsSaveFiles {
    pub dir: PathBuf,
}

impl FsSaveFiles {
    /// Path of the savegame `name` in the folder.
    pub fn path(&self, name: &str, guid: i32) -> PathBuf {
        save_path(&self.dir, name, guid)
    }

    /// Folder of the recordings `savegame/autoplay` (path `0x0061F0A0`, E-329). The recording creates it at the start
    /// (deviating from E-335).
    pub fn autoplay_dir(&self) -> PathBuf {
        self.dir.join("autoplay")
    }

    /// Path in the directory `dir` (`FUN_004BF9A0`, O-150): [`SaveDir::Main`] like [`Self::path`] with suffix,
    /// [`SaveDir::Multiplayer`] `<dir>/multiplayer/<name>` without suffix.
    pub fn path_in(&self, dir: SaveDir, name: &str, guid: i32) -> PathBuf {
        match dir {
            SaveDir::Main => self.path(name, guid),
            SaveDir::Multiplayer => self.dir.join("multiplayer").join(name),
            SaveDir::Autoplay => self.autoplay_dir().join(name),
        }
    }
}

impl SaveFiles for FsSaveFiles {
    /// `FUN_004C63A0` steps 2-4 with `FUN_004156A0`: `FindFirstFile("savegame/*.%03X")` (case-insensitive, files
    /// only), found order like `read_dir` (the same search on Windows), then sorted by `time_write` in seconds in
    /// descending order - before the first older entry, otherwise at the end, so stable; names through [`list_name`]
    /// (`.` files are dropped, O-196). If the folder is missing, the list is empty.
    ///
    /// In the network folder (`savegame/multiplayer`, pattern `*.*`, O-150) all files count without a suffix check;
    /// [`list_name`] cuts off a three-digit suffix. In the recording folder (`savegame/autoplay`) all files count as
    /// well except `.` names, with the full name and ascending by name.
    fn list(&self, dir: SaveDir, guid: i32) -> Vec<SaveEntry> {
        let suffix = suffix(guid);
        let folder = match dir {
            SaveDir::Main => self.dir.clone(),
            SaveDir::Multiplayer => self.dir.join("multiplayer"),
            SaveDir::Autoplay => self.autoplay_dir(),
        };
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                log::warn!("Savegames \"{}\" not readable: {e}", folder.display());
                return Vec::new();
            }
        };
        let mut found: Vec<(i64, SaveEntry)> = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let matches = dir != SaveDir::Main
                || file_name.len() >= suffix.len()
                    && file_name.is_char_boundary(file_name.len() - suffix.len())
                    && file_name[file_name.len() - suffix.len()..].eq_ignore_ascii_case(&suffix);
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !matches || !meta.is_file() {
                continue;
            }
            // Recordings keep their full name (no suffix cut off); `.` names are dropped there as well.
            let name = if dir == SaveDir::Autoplay {
                (!file_name.starts_with('.')).then_some(file_name)
            } else {
                list_name(&file_name)
            };
            let Some(name) = name else {
                continue;
            };
            let modified: DateTime<Local> =
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH).into();
            found.push((
                modified.timestamp(),
                SaveEntry {
                    name,
                    modified: local_stamp(modified),
                },
            ));
        }
        if dir == SaveDir::Autoplay {
            // Recordings ascending by name (deviating from E-342: the original orders by time).
            found.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
        } else {
            found.sort_by_key(|(secs, _)| Reverse(*secs));
        }
        found.into_iter().map(|(_, e)| e).collect()
    }

    /// `FUN_004C6B30`: the file can be opened.
    fn exists(&self, dir: SaveDir, guid: i32, name: &str) -> bool {
        std::fs::File::open(self.path_in(dir, name, guid)).is_ok()
    }

    /// `localtime(time())`.
    fn now_local(&self) -> LocalStamp {
        local_stamp(Local::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Creates `name` with the modification time `secs` (+ `ms`) after 1970.
    fn touch(dir: &Path, name: &str, secs: u64, ms: u64) {
        let path = dir.join(name);
        std::fs::write(&path, name).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(ms);
        file.set_modified(t).unwrap();
    }

    #[test]
    fn fs_save_files_list_and_sort() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("savegame");
        let files = FsSaveFiles { dir: dir.clone() };
        // Folder missing: empty, nothing present.
        assert!(files.list(SaveDir::Main, 10).is_empty());
        assert!(!files.exists(SaveDir::Main, 10, "a"));
        std::fs::create_dir(&dir).unwrap();
        let day = 1_700_000_000;
        // GUID 10 -> suffix `.00A` (case-insensitive); `c` lies in the same second as `a`, found later.
        touch(&dir, "a.00A", day, 100);
        touch(&dir, "b.00A", day + 3600, 0);
        touch(&dir, "c.00a", day, 900);
        touch(&dir, "d.00A", day - 60, 0);
        // Other GUID, file without suffix, `.` file, folder with matching suffix, four-digit similar.
        touch(&dir, "foreign.00B", day + 7200, 0);
        touch(&dir, "plain", day + 7200, 0);
        touch(&dir, ".hidden.00A", day + 7200, 0);
        std::fs::create_dir(dir.join("folder.00A")).unwrap();
        touch(&dir, "e.000A", day + 7200, 0);
        let list = files.list(SaveDir::Main, 10);
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        // Newest first; same second in found order (a before c).
        assert_eq!(names, ["b", "a", "c", "d"]);
        let local: DateTime<Local> =
            (SystemTime::UNIX_EPOCH + Duration::from_secs(day + 3600)).into();
        assert_eq!(list[0].modified, local_stamp(local));
        assert!(files.exists(SaveDir::Main, 10, "a"));
        assert!(
            files.exists(SaveDir::Main, 10, "c"),
            "case of the suffix does not count on Windows"
        );
        assert!(!files.exists(SaveDir::Main, 10, "foreign"));
        assert!(files.exists(SaveDir::Main, 11, "foreign"));
        // GUID from 0x1000: four-digit suffix, the list does not cut it off (O-196).
        touch(&dir, "x.1000", day, 0);
        let names: Vec<String> = files
            .list(SaveDir::Main, 0x1000)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["x.1000"]);
        assert_eq!(
            save_path(&dir, "x", 0x1000),
            dir.join("x.1000"),
            "path like `%s/%s.%03X`"
        );
        assert_eq!(save_path(&dir, "q", -1), dir.join("q.FFFFFFFF"));
        // Local time of the host: plausible (not the fixed clock of `NoSaveFiles`).
        assert!(files.now_local().year >= 2024);
    }

    #[test]
    fn main_list_is_unchanged() {
        // The main folder ignores `multiplayer/` (a folder, not a savegame) and its files.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("savegame");
        std::fs::create_dir_all(dir.join("multiplayer")).unwrap();
        let files = FsSaveFiles { dir: dir.clone() };
        touch(&dir, "a.00A", 1_700_000_000, 0);
        touch(&dir.join("multiplayer"), "net.00A", 1_700_000_100, 0);
        let names: Vec<String> = files
            .list(SaveDir::Main, 10)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["a"]);
        assert!(!files.exists(SaveDir::Main, 10, "net"));
    }

    #[test]
    fn multiplayer_list_takes_files_without_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("savegame");
        let net = dir.join("multiplayer");
        let files = FsSaveFiles { dir };
        // Folder missing: empty.
        assert!(files.list(SaveDir::Multiplayer, 10).is_empty());
        std::fs::create_dir_all(&net).unwrap();
        let day = 1_700_000_000;
        touch(&net, "a", day, 0);
        touch(&net, "b.0A3", day + 60, 0);
        touch(&net, ".x", day + 120, 0);
        std::fs::create_dir(net.join("c")).unwrap();
        // The GUID does not matter in the network folder.
        for guid in [10, 99] {
            let names: Vec<String> = files
                .list(SaveDir::Multiplayer, guid)
                .into_iter()
                .map(|e| e.name)
                .collect();
            assert_eq!(names, ["b", "a"], "{guid}");
        }
        assert!(files.exists(SaveDir::Multiplayer, 10, "a"));
        assert!(files.exists(SaveDir::Multiplayer, 10, "b.0A3"));
        assert!(!files.exists(SaveDir::Multiplayer, 10, "b"));
        assert!(!files.exists(SaveDir::Main, 10, "a"));
    }

    #[test]
    fn multiplayer_path_has_no_suffix() {
        let files = FsSaveFiles {
            dir: PathBuf::from("sg"),
        };
        assert_eq!(
            files.path_in(SaveDir::Multiplayer, "Burg", 10),
            Path::new("sg").join("multiplayer").join("Burg")
        );
        assert_eq!(
            files.path_in(SaveDir::Main, "Burg", 10),
            Path::new("sg").join("Burg.00A")
        );
    }

    // QUIRK E-342: deviation
    #[test]
    fn autoplay_list_is_sorted_by_name_with_full_names() {
        // Deviating from E-342: all files except `.` names, full name (a three-digit suffix stays as well),
        // ascending by name - independent of the modification time.
        let tmp = tempfile::tempdir().unwrap();
        let files = FsSaveFiles {
            dir: tmp.path().join("savegame"),
        };
        assert!(
            files.list(SaveDir::Autoplay, 10).is_empty(),
            "folder missing"
        );
        let folder = files.autoplay_dir();
        std::fs::create_dir_all(&folder).unwrap();
        touch(&folder, "261108-1435", 1_700_000_000, 0);
        touch(&folder, "x.rec", 1_700_000_900, 0);
        touch(&folder, "261108-0900", 1_700_000_500, 0);
        touch(&folder, ".hidden", 1_700_000_100, 0);
        touch(&folder, "a.000", 1_700_000_200, 0);
        std::fs::create_dir(folder.join("subfolder")).unwrap();
        let names: Vec<String> = files
            .list(SaveDir::Autoplay, 10)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["261108-0900", "261108-1435", "a.000", "x.rec"]);
        assert!(files.exists(SaveDir::Autoplay, 10, "a.000"));
        assert!(!files.exists(SaveDir::Autoplay, 10, "a"));
        // The savegame folder stays untouched by this.
        assert!(files.list(SaveDir::Main, 10).is_empty());
    }

    // QUIRK E-342: deviation
    #[test]
    fn autoplay_path_keeps_the_full_name() {
        let files = FsSaveFiles {
            dir: PathBuf::from("sg"),
        };
        assert_eq!(
            files.path_in(SaveDir::Autoplay, "261108-1435", 10),
            Path::new("sg").join("autoplay").join("261108-1435")
        );
        assert_eq!(
            files.path_in(SaveDir::Autoplay, "x.rec", 10),
            Path::new("sg").join("autoplay").join("x.rec")
        );
    }

    #[test]
    fn autoplay_dir_lies_in_the_savegame_folder() {
        let files = FsSaveFiles {
            dir: PathBuf::from("sg"),
        };
        assert_eq!(files.autoplay_dir(), Path::new("sg").join("autoplay"));
    }
}
