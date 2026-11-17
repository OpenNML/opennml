//! Multiplayer map list `data/levels/multiplayer/levels%04d.ini` and `data/gifcon/randommaptypes.ini`
//! (basis: re/singleplayer-menus part 2, 2.2 and 3; `docs/formats/gifcon-misc.md`).

use crate::cscan::{atol, game_file_lines};
use crate::text::decode_cp1252;

/// Directory of the map lists (`0x0062079C`) and file pattern `"%s/levels%04d.ini"` (`0x00620788`).
const MAP_LIST_DIR: &str = "data/levels/multiplayer";

/// Highest file number the original still tries (`FUN_004E5EF0`: loop up to counter 10000).
const MAX_LIST_FILE: u32 = 9999;

/// One entry of the map list (`FUN_004E5E50`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapListEntry {
    /// Text ID of the display name (`NAME`).
    pub name_id: i32,
    /// Game mode word (`TYPE`): `NORMAL`, `KING_OF_THE_HILL`, `RAILROAD1`, `RAILROAD2`.
    pub kind: String,
    /// Path of the `.lvl` file (`PATH`).
    pub path: String,
}

/// Reads a map list file (loop body of `FUN_004E5EF0`, `0x004E5EF0`).
///
/// The line beginnings `NAME `, `TYPE ` and `PATH ` (with space, exact, without trimming) each set an
/// intermediate variable; **only the `PATH` line appends an entry** (`FUN_004E5E50`) with the currently
/// valid name and type. Consequences (O-185): a stray `NAME` at the end of the file - the installation has
/// one (`NAME 15800`) - creates no entry; `NAME` and `TYPE` apply to all further `PATH` lines
/// until they are set again. The original never resets the name (not even between files);
/// the reimplementation begins every file with name 0 and an empty type - only a file that begins with `PATH`
/// differs.
pub fn parse_level_list(text: &str) -> Vec<MapListEntry> {
    let mut name_id = 0;
    let mut kind = String::new();
    let mut entries = Vec::new();
    for line in game_file_lines(text) {
        if let Some(rest) = line.strip_prefix("NAME ") {
            name_id = atol(rest);
        } else if let Some(rest) = line.strip_prefix("TYPE ") {
            kind = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("PATH ") {
            entries.push(MapListEntry {
                name_id,
                kind: kind.clone(),
                path: rest.to_string(),
            });
        }
    }
    entries
}

/// Builds the map list (`FUN_004E5EF0`): `data/levels/multiplayer/levels%04d.ini` ascending from 0000 up to
/// the first missing file, at most up to 9999; the entries of all files in file order, without
/// sorting and without a directory scan. `read` returns the bytes of a path in the file system of the game.
pub fn load_map_list(read: impl Fn(&str) -> Option<Vec<u8>>) -> Vec<MapListEntry> {
    let mut entries = Vec::new();
    for index in 0..=MAX_LIST_FILE {
        let Some(bytes) = read(&format!("{MAP_LIST_DIR}/levels{index:04}.ini")) else {
            break;
        };
        entries.extend(parse_level_list(&decode_cp1252(&bytes)));
    }
    entries
}

/// A game mode (`GNAME`/`GTYPE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameTypeDesc {
    pub name_id: i32,
    pub gtype: String,
}

/// A nation (`NNAME`/`NTYPE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NationDesc {
    pub name_id: i32,
    pub ntype: String,
}

/// The content of `randommaptypes.ini` (lists in file order).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RandomMapTypes {
    /// Game modes (texts 15540 ... 15545 with `NORMAL`, `KING_OF_THE_HILL`, `RAILROAD1`, `RAILROAD2`).
    pub game_types: Vec<GameTypeDesc>,
    /// Nations; the index of `PLAYER_PLAYER` in the list is the nation number (re/singleplayer-menus).
    pub nations: Vec<NationDesc>,
    /// Text IDs of the map types (`MNAME`).
    pub maps: Vec<i32>,
    /// Text IDs of the landscapes (`LNAME`).
    pub landscapes: Vec<i32>,
    /// Text IDs of the times of day (`DNAME`).
    pub daytimes: Vec<i32>,
    /// Script paths of the map types (rest of the `MSCRIPT` line raw, same length as `maps`).
    pub map_scripts: Vec<String>,
    /// Landscape words (rest of the `LTYPE` line raw, same length as `landscapes`), e.g. `WINTER`.
    pub landscape_types: Vec<String>,
    /// Time-of-day words (rest of the `DTYPE` line raw, same length as `daytimes`), e.g. `NIGHT`.
    pub daytime_types: Vec<String>,
}

/// Reads `data/gifcon/randommaptypes.ini` (`FUN_004CC0D0`, `0x004CC0D0`; built by `FUN_004CCA20`).
///
/// Line beginnings with a space, exact and without trimming: `GNAME `, `MNAME `, `LNAME `, `DNAME `, `NNAME `
/// set the **same** intermediate variable (text ID); the entry is created only in the following `GTYPE `,
/// `MSCRIPT `, `LTYPE `, `DTYPE ` or `NTYPE ` line with the currently valid name. `MSCRIPT`, `LTYPE`
/// and `DTYPE` store the rest of the line raw besides the name ID (`Mid(n)`, like `GTYPE`): script path,
/// landscape word and time-of-day word of the random map. Without a preceding `*NAME` the value is 0 (an
/// undetermined value in the original).
pub fn parse_random_map_types(text: &str) -> RandomMapTypes {
    let mut types = RandomMapTypes::default();
    let mut name_id = 0;
    for line in game_file_lines(text) {
        if let Some(rest) = ["GNAME ", "MNAME ", "LNAME ", "DNAME ", "NNAME "]
            .iter()
            .find_map(|key| line.strip_prefix(key))
        {
            name_id = atol(rest);
        } else if let Some(rest) = line.strip_prefix("GTYPE ") {
            types.game_types.push(GameTypeDesc {
                name_id,
                gtype: rest.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("MSCRIPT ") {
            types.maps.push(name_id);
            types.map_scripts.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("LTYPE ") {
            types.landscapes.push(name_id);
            types.landscape_types.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("DTYPE ") {
            types.daytimes.push(name_id);
            types.daytime_types.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("NTYPE ") {
            types.nations.push(NationDesc {
                name_id,
                ntype: rest.to_string(),
            });
        }
    }
    types
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn entry(name_id: i32, kind: &str, path: &str) -> MapListEntry {
        MapListEntry {
            name_id,
            kind: kind.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn level_list_stops_at_first_gap() {
        // 0000 (2 entries) + 0001 (1) + 0003 (behind the gap, ignored) -> 3 entries.
        let files: HashMap<String, Vec<u8>> = [
            (
                "data/levels/multiplayer/levels0000.ini",
                "NAME 1\r\nTYPE NORMAL\r\nPATH a.lvl\r\nNAME 2\r\nTYPE RAILROAD1\r\nPATH b.lvl\r\n",
            ),
            (
                "data/levels/multiplayer/levels0001.ini",
                "NAME 3\r\nTYPE NORMAL\r\nPATH c.lvl\r\n",
            ),
            (
                "data/levels/multiplayer/levels0003.ini",
                "NAME 4\r\nTYPE NORMAL\r\nPATH d.lvl\r\n",
            ),
        ]
        .into_iter()
        .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
        .collect();
        let list = load_map_list(|path| files.get(path).cloned());
        assert_eq!(
            list,
            vec![
                entry(1, "NORMAL", "a.lvl"),
                entry(2, "RAILROAD1", "b.lvl"),
                entry(3, "NORMAL", "c.lvl"),
            ]
        );
        assert!(load_map_list(|_| None).is_empty());
    }

    #[test]
    fn map_list_is_decoded_as_cp1252() {
        let read = |path: &str| {
            (path == "data/levels/multiplayer/levels0000.ini").then(|| {
                b"NAME 1\nTYPE NORMAL\nPATH data/levels/multiplayer/Gr\xfcn.lvl\n".to_vec()
            })
        };
        assert_eq!(
            load_map_list(read),
            vec![entry(1, "NORMAL", "data/levels/multiplayer/Grün.lvl")]
        );
    }

    // QUIRK O-185: kept
    #[test]
    fn entry_is_appended_at_the_path_line() {
        // The original appends only at `PATH` (FUN_004E5EF0): a stray `NAME` at the end of the file creates no
        // entry (O-185); `TYPE` and `NAME` continue to apply until someone sets them again; two
        // `PATH` lines yield two entries.
        let text = "# Comment\r\nNAME 7\r\nTYPE KING_OF_THE_HILL\r\nPATH a.lvl\r\nPATH b.lvl\r\nNAME 8\r\nPATH c.lvl\r\nNAME 15800\r\n";
        assert_eq!(
            parse_level_list(text),
            vec![
                entry(7, "KING_OF_THE_HILL", "a.lvl"),
                entry(7, "KING_OF_THE_HILL", "b.lvl"),
                entry(8, "KING_OF_THE_HILL", "c.lvl"),
            ]
        );
    }

    #[test]
    fn level_list_keys_need_the_space_and_exact_case() {
        // The keys are `NAME `, `TYPE `, `PATH ` (with space, exact case, without
        // trimming the line beginning); the path stays unchanged (spaces in the name).
        let text = "NAME\n name 5\nNAME  9\nTYPE NORMAL\nPATH data/levels/multiplayer/[4Players]-Four Points.lvl\nPATH\nPATHx y.lvl\n";
        assert_eq!(
            parse_level_list(text),
            vec![entry(
                9,
                "NORMAL",
                "data/levels/multiplayer/[4Players]-Four Points.lvl"
            )]
        );
    }

    #[test]
    fn random_map_types_keep_file_order() {
        let text = "\
#\r\n# MAP TYPES\r\n\r\n\
MNAME 15470\r\nMSCRIPT data/randommapscripts/continental.rnd\r\n\r\n\
MNAME 15471\r\nMSCRIPT data/randommapscripts/flatland.rnd\r\n\r\n\
LNAME 15490\r\nLTYPE CARIBBEAN\r\n\r\nLNAME 15491\r\nLTYPE PRAIRIE\r\n\r\n\
DNAME 15500\r\nDTYPE DAY\r\n\r\nDNAME 15501\r\nDTYPE NIGHT\r\n\r\n\
GNAME 15540\r\nGTYPE NORMAL\r\n\r\nGNAME 15543\r\nGTYPE KING_OF_THE_HILL\r\n\r\nGNAME 15544\r\nGTYPE RAILROAD1\r\n\r\n\
NNAME 15120\r\nNTYPE WID\r\n\r\nNNAME 15121\r\nNTYPE PRI\r\n\r\nNNAME 15122\r\nNTYPE SPA\r\n\r\n\
NNAME 15123\r\nNTYPE SID\r\n\r\nNNAME 15124\r\nNTYPE ENG\r\n\r\nNNAME 15125\r\nNTYPE PAT\r\n";
        let types = parse_random_map_types(text);
        assert_eq!(types.maps, vec![15470, 15471]);
        assert_eq!(types.landscapes, vec![15490, 15491]);
        assert_eq!(types.daytimes, vec![15500, 15501]);
        let game: Vec<(i32, &str)> = types
            .game_types
            .iter()
            .map(|g| (g.name_id, g.gtype.as_str()))
            .collect();
        assert_eq!(
            game,
            [
                (15540, "NORMAL"),
                (15543, "KING_OF_THE_HILL"),
                (15544, "RAILROAD1")
            ]
        );
        // Six nations in file order; the index of `PLAYER_PLAYER` in this list is the nation number.
        let nations: Vec<&str> = types.nations.iter().map(|n| n.ntype.as_str()).collect();
        assert_eq!(nations, ["WID", "PRI", "SPA", "SID", "ENG", "PAT"]);
        assert_eq!(types.nations[5].name_id, 15125);
    }

    #[test]
    fn random_map_types_keep_script_paths_and_type_words() {
        // `MSCRIPT`/`LTYPE`/`DTYPE` store the rest of the line raw (`Mid(n)`), same length as
        // the name lists.
        let text = "MNAME 1\nMSCRIPT data/randommapscripts/Continental.rnd\n\
                    MNAME 2\nMSCRIPT data/randommapscripts/Islands.rnd\n\
                    LNAME 3\nLTYPE WINTER\nDNAME 4\nDTYPE NIGHT\n";
        let types = parse_random_map_types(text);
        assert_eq!(
            types.map_scripts,
            [
                "data/randommapscripts/Continental.rnd",
                "data/randommapscripts/Islands.rnd"
            ]
        );
        assert_eq!(types.landscape_types, ["WINTER"]);
        assert_eq!(types.daytime_types, ["NIGHT"]);
        assert_eq!(types.map_scripts.len(), types.maps.len());
        assert_eq!(types.landscape_types.len(), types.landscapes.len());
        assert_eq!(types.daytime_types.len(), types.daytimes.len());
    }

    // QUIRK O-185: kept
    #[test]
    fn random_map_types_share_one_pending_name() {
        // FUN_004CC0D0: all `*NAME` lines write the same intermediate variable, the entry is created only
        // in the `*TYPE`/`MSCRIPT` line. An `MNAME` without `MSCRIPT` creates nothing, its name goes to
        // the next entry of any list.
        let text =
            "MNAME 1\nGTYPE NORMAL\nDNAME 2\nMSCRIPT x.rnd\nLTYPE WINTER\nNTYPE WID\nXNAME 3\n";
        let types = parse_random_map_types(text);
        assert_eq!(
            types.game_types,
            vec![GameTypeDesc {
                name_id: 1,
                gtype: "NORMAL".into()
            }]
        );
        assert_eq!(types.maps, vec![2]);
        assert_eq!(types.landscapes, vec![2]);
        assert_eq!(
            types.nations,
            vec![NationDesc {
                name_id: 2,
                ntype: "WID".into()
            }]
        );
        assert!(types.daytimes.is_empty());
    }
}
