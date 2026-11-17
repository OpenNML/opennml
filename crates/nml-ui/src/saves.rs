//! Savegames from the point of view of the menus (re/ingame-menus 3.1, 3.4): the list in the folder `savegame/` (in
//! the reimplementation next to the settings file, a deviation from the original), whether a savegame exists, and
//! the local time.
//!
//! `nml-ui` reads no files and no clock (determinism): the host provides both through [`SaveFiles`]. Without storage
//! (viewer, `--screenshot`, tests without savegames) [`NoSaveFiles`] applies.

use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

/// Local time like `localtime` (full year, month 1 to 12, day, hour, minute).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalStamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// An entry of the savegame list (`GameApp+0xD6DC`, 0x28C bytes: `+2` name, `+0x20A` display text): the name without
/// the extension `.<GUID %03X>` ([`list_name`]) and the modification time of the file (`time_write`) as local time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveEntry {
    pub name: String,
    pub modified: LocalStamp,
}

/// The current savegame directory (`GameApp+0x1984`, set by `FUN_004BFC40`): `Main` = `savegame/` (files
/// `<name>.<GUID %03X>`, value 1), `Multiplayer` = `savegame/multiplayer/` (files without extension, value 2),
/// `Autoplay` = `savegame/autoplay/` (the recordings, value 3). List, "present" and delete apply in the current
/// directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SaveDir {
    #[default]
    Main,
    Multiplayer,
    Autoplay,
}

/// The savegames of the host (re/ingame-menus 3.1, 3.2).
pub trait SaveFiles {
    /// The savegames `savegame/*.<guid %03X>` (pattern `"*.%03X"`, `FUN_004C63A0` step 2) like `FUN_004156A0`: files
    /// only, **by modification time descending** (newest first), equal time in the order found (insert before the
    /// first older entry, otherwise at the end); names that begin with `.` ignored; an extension of exactly three
    /// digits cut off ([`list_name`]). In the directory [`SaveDir::Multiplayer`] all files count (`*.*`, the GUID
    /// does not apply). In the directory [`SaveDir::Autoplay`] likewise all files, **by name ascending** and with
    /// the full file name (deviation from E-342).
    fn list(&self, dir: SaveDir, guid: i32) -> Vec<SaveEntry>;
    /// `FUN_004C6B30`: the file `savegame/<name>.<guid %03X>` (in the network folder `savegame/multiplayer/<name>`)
    /// can be opened.
    fn exists(&self, dir: SaveDir, guid: i32, name: &str) -> bool;
    /// Local time now (`localtime(time())`, for the default name [`default_save_name`]).
    fn now_local(&self) -> LocalStamp;
}

/// Storage without savegames: empty list, nothing present, fixed clock (Jan 1, 2000, 0:00).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSaveFiles;

impl SaveFiles for NoSaveFiles {
    fn list(&self, _dir: SaveDir, _guid: i32) -> Vec<SaveEntry> {
        Vec::new()
    }

    fn exists(&self, _dir: SaveDir, _guid: i32, _name: &str) -> bool {
        false
    }

    fn now_local(&self) -> LocalStamp {
        LocalStamp {
            year: 2000,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
        }
    }
}

/// The storage of the menus (`MenuOptions::saves`, otherwise [`NoSaveFiles`]); shared with the host.
#[derive(Clone)]
pub struct Saves(pub Rc<dyn SaveFiles>);

impl Default for Saves {
    fn default() -> Saves {
        Saves(Rc::new(NoSaveFiles))
    }
}

impl fmt::Debug for Saves {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Saves(..)")
    }
}

impl Deref for Saves {
    type Target = dyn SaveFiles;
    fn deref(&self) -> &(dyn SaveFiles + 'static) {
        &*self.0
    }
}

/// List name of a file (`FUN_004C63A0` step 4, `0x004C64D0`...`0x004C64F4`): if the name begins with `.`, the file
/// is dropped (`None`); if there is a `.` at length - 4 (UTF-16 units), it is cut off there - exactly one
/// three-digit extension is dropped (O-196: a GUID from `0x1000` yields a four-digit extension, which stays).
pub fn list_name(file_name: &str) -> Option<String> {
    let units: Vec<u16> = file_name.encode_utf16().collect();
    if units.first() == Some(&u16::from(b'.')) {
        return None;
    }
    let cut = units
        .len()
        .checked_sub(4)
        .filter(|&i| units[i] == u16::from(b'.'));
    Some(String::from_utf16_lossy(
        &units[..cut.unwrap_or(units.len())],
    ))
}

/// Display text of an entry `L"(%02d/%02d/%02d %d:%02d) %s"` (format `0x0061F05C`, `0x004C6506`...`0x004C65BC`):
/// year - 2000, month, day, hour without a leading zero, minute, name.
pub fn display_name(e: &SaveEntry) -> String {
    let t = e.modified;
    format!(
        "({:02}/{:02}/{:02} {}:{:02}) {}",
        t.year - 2000,
        t.month,
        t.day,
        t.hour,
        t.minute,
        e.name
    )
}

/// Default name for an empty `FILENAME` (`FUN_004BC710`, format `0x0061EC40`): `savegame_%04d_%02d_%02d_%02d%02d`.
pub fn default_save_name(now: LocalStamp) -> String {
    format!(
        "savegame_{:04}_{:02}_{:02}_{:02}{:02}",
        now.year, now.month, now.day, now.hour, now.minute
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> LocalStamp {
        LocalStamp {
            year,
            month,
            day,
            hour,
            minute,
        }
    }

    #[test]
    fn display_and_default_names_follow_the_formats() {
        let e = SaveEntry {
            name: "Castle".into(),
            modified: stamp(2026, 3, 7, 9, 5),
        };
        assert_eq!(display_name(&e), "(26/03/07 9:05) Castle");
        let e = SaveEntry {
            name: "x".into(),
            modified: stamp(2003, 11, 30, 23, 59),
        };
        assert_eq!(display_name(&e), "(03/11/30 23:59) x");
        assert_eq!(
            default_save_name(stamp(2026, 10, 27, 9, 5)),
            "savegame_2026_10_27_0905"
        );
    }

    // QUIRK O-196: kept
    #[test]
    fn list_name_cuts_exactly_three_digit_extensions() {
        assert_eq!(list_name("castle.00A").as_deref(), Some("castle"));
        assert_eq!(list_name("a.b.001").as_deref(), Some("a.b"));
        // Four-digit extension (GUID >= 0x1000) and names without an extension stay whole (O-196).
        assert_eq!(list_name("castle.1000").as_deref(), Some("castle.1000"));
        assert_eq!(list_name("abc").as_deref(), Some("abc"));
        assert_eq!(list_name(".001").as_deref(), None);
        assert_eq!(list_name(".hidden").as_deref(), None);
        // Exactly four characters with a leading dot: already ignored as a `.` file; one character before the
        // extension stays.
        assert_eq!(list_name("x.001").as_deref(), Some("x"));
        assert_eq!(list_name("ä.001").as_deref(), Some("ä"));
    }

    #[test]
    fn no_save_files_is_empty_with_a_fixed_clock() {
        let s = Saves::default();
        assert!(s.list(SaveDir::Main, 0).is_empty());
        assert!(!s.exists(SaveDir::Multiplayer, 0, "quicksave1"));
        assert_eq!(s.now_local(), stamp(2000, 1, 1, 0, 0));
    }
}
