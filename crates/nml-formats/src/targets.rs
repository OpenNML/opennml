//! Match filters ("MatchStructs") from `targets.ini`, `flowcontroltragets.ini` and `priorities.ini`
//! with the names from `targetdefines.ini`, read like `GameWorld_LoadTargetsIni` (`0x00484190`)
//! and `MatchFilter_ParseBlock` (`0x005550B0`), see `docs/formats/targets.md`.

use crate::config::{game_lines, left, mid, scan_int};

/// Paths in the game.
pub const TARGETDEFINES_PATH: &str = "data/gifcon/targetdefines.ini";
pub const TARGETS_PATH: &str = "data/gifcon/targets.ini";
pub const FLOWCONTROL_TARGETS_PATH: &str = "data/gifcon/flowcontroltragets.ini";
pub const PRIORITIES_PATH: &str = "data/gifcon/priorities.ini";

/// Names from `targetdefines.ini` (`TargetDefines_LoadIni` `0x005563D0`): name up to the first
/// space, value `%d` after it; exact search, first match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetDefines {
    pub entries: Vec<(String, i32)>,
}

impl TargetDefines {
    pub fn lookup(&self, name: &str) -> Option<i32> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, v)| v)
    }
}

pub fn parse_target_defines(text: &str) -> TargetDefines {
    let mut defines = TargetDefines::default();
    for line in game_lines(text) {
        let Some(p) = line.find(' ') else { continue };
        if let Some(value) = scan_int(&line[p + 1..]) {
            defines.entries.push((line[..p].to_string(), value));
        }
    }
    defines
}

/// A filter (`0x54` bytes, re/match-filter 3 and 5). The run-time words (player for the
/// fog, reference object, layer/region, distance range) are set not by the parser but by the code that uses
/// the filter. Default (`Default`) = parser (`TargetEntry_Construct`, `MM_NEXT`): all
/// words 0; built-in filters of the code begin with [`MatchFilter::constructed`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchFilter {
    /// `MM_MODE` bits (word 0).
    pub mode: u32,
    /// `MM_MODE2` bits (word 1).
    pub mode2: u32,
    /// `MM_STATUS` (word 2).
    pub status: u32,
    /// `MM_MATCHBITS` (word 3; low byte = module or `0xFF` = category).
    pub match_bits: u32,
    /// `MM_PROPID`/`MM_PROPERTY` (word 4).
    pub prop_id: u32,
    /// `MM_GUID` (word 5).
    pub guid: i32,
    /// `MM_METHODINDEX` (word 7).
    pub method: i32,
    /// `MM_IS_ATTACKING_WHO` (word 9): `ME` 1, `MY_FRIEND` 2, `MY_FORMATION` 4.
    pub who: u32,
    /// Word 6: player of the fog check (`MM_CHECKFOG`, set only on the head filter).
    pub fog_player: i32,
    /// Word 8: reference object (ID raw; `MM2_IS_ATTACKING`, `MM2_IS_IN_WEAPON_RANGE`,
    /// `MM_METHODINDEX 3`, built-in distance check `mode & 0x20`).
    pub ref_object: i32,
    /// Word 0xA: path layer of the built-in check `mode & 0x20`.
    pub layer: i32,
    /// Word 0xB: region of the built-in check `mode & 0x20` (-1 or `0xFFFF` = none).
    pub region: i32,
    /// Word 0xC: largest distance of the built-in check (fixed point).
    pub range_max: i32,
    /// Word 0xD: smallest distance of the built-in check (fixed point).
    pub range_min: i32,
    /// Words 0xF/0x10: home of the attacker (position object `[0xE..0x10]`), -1 = none.
    pub home: (i32, i32),
    /// Word 0x11: `m_TetherLength` of the attacker (default -1); != 0 = cell search instead of
    /// center distance (re/combat-targeting 3.2).
    pub tether: i32,
    /// Word 0x12: facing tolerance in degrees (`mode2 & 8`).
    pub angle_tolerance: i32,
    /// Byte 0x13: additionally line of sight via the building map (`GameWorld_HasLineOfSight`).
    pub line_of_sight: bool,
    /// Next filter (`MM_NEXT`).
    pub next: Option<Box<MatchFilter>>,
}

impl MatchFilter {
    /// `MatchFilter_Construct` (`0x00500780`) with zero values for the ten arguments (words 0-9): the
    /// words 0xA-0xD and 0x11 as well as the home (0xF/0x10) are -1, word 0x12 and byte 0x13 are 0,
    /// no next filter.
    pub fn constructed() -> Self {
        Self {
            layer: -1,
            region: -1,
            range_max: -1,
            range_min: -1,
            home: (-1, -1),
            tether: -1,
            ..Self::default()
        }
    }
}

/// Entry of a filter file (`TargetEntry`, `0xBC` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetEntry {
    /// `BUTTONMSG` (`+0xB0`).
    pub button_msg: i32,
    /// `MAUSICONID` (`+0xAC`).
    pub mouse_icon: i32,
    /// Byte "active" (`+0xB8`); `DISABLERBUTTON` takes effect only inside a filter block and is
    /// always outside in the shipped data - all entries stay active.
    pub enabled: bool,
    pub my: MatchFilter,
    pub target: MatchFilter,
}

impl TargetEntry {
    fn new() -> Self {
        Self {
            button_msg: 0,
            mouse_icon: 0,
            enabled: true,
            my: MatchFilter::default(),
            target: MatchFilter::default(),
        }
    }
}

/// Result of the loader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetFile {
    /// Entries closed with `END` in file order.
    pub entries: Vec<TargetEntry>,
    /// The loader aborted (unknown mode value or name, end of file in the block); the rest of the
    /// file is missing as in the game. Text: line and reason.
    pub aborted: Option<String>,
}

/// `MM_MODE` values in check order.
/// Not settable by file are 0x20, 0x80 and 0x80000 (built-in filters).
pub const MODE_NAMES: [(&str, u32); 28] = [
    ("MM_ANDNEXTMATCH", 0x1000_0000),
    ("MM_ORNEXTMATCH", 0x2000_0000),
    ("MM_ANDNOTNEXTMATCH", 0x4000_0000),
    ("MM_ORNOTNEXTMATCH", 0x8000_0000),
    ("MM_CHECKHANDLETYPE", 0x2),
    ("MM_CHECKHANDLETYPE_ANDMODE", 0x40),
    ("MM_CHECKGUID", 0x8),
    ("MM_CHECKPROPERTY", 0x4),
    ("MM_CHECKSTATUS", 0x1),
    ("MM_ANDALL", 0x10),
    ("MM_HL_MYLIST", 0x100),
    ("MM_HL_NOMANSLIST", 0x200),
    ("MM_HL_ENEMYLIST", 0x400),
    ("MM_HL_FRIENDLIST", 0x800),
    ("MM_HL_TREELIST", 0x1000),
    ("MM_HL_PLAYERLIST", 0x40000),
    ("MM_CHECKNOTSTATUS", 0x2000),
    ("MM_CHECKNOTHANDLETYPE", 0x4000),
    ("MM_CHECKNOTPROPERTY", 0x8000),
    ("MM_CHECKNOTGUID", 0x10000),
    ("MM_CHECKNOTLOAD", 0x20000),
    ("MM_CHECKLOADGOLD", 0x10_0000),
    ("MM_CHECKLOADWOOD", 0x20_0000),
    ("MM_CHECKLOADFOOD", 0x40_0000),
    ("MM_RETURNFALSE", 0x800_0000),
    ("MM_ENERGYLOST", 0x80_0000),
    ("MM_RETURN_DIED", 0x400_0000),
    ("MM_CHECKFOG", 0x200_0000),
];

/// `MM_MODE2` values in check order.
pub const MODE2_NAMES: [(&str, u32); 12] = [
    ("MM2_MORETHAN70", 0x800),
    ("MM2_EXTRAENERGYLOST", 0x1),
    ("MM2_CALLMETHOD", 0x2),
    ("MM2_NOHITPOINTS", 0x10),
    ("MM2_SHOOTABLE", 0x20),
    ("MM2_NOT_TOO_MANY_OPPONENTS", 0x40),
    ("MM2_RETURNTRUE", 0x80),
    ("MM2_DONOTIGNOREQUARTEREDUNITS", 0x4),
    ("MM2_CAN_FIGHT_FAR", 0x100),
    ("MM2_IS_IN_WEAPON_RANGE", 0x200),
    ("MM2_IS_ATTACKING", 0x400),
    ("MM2_POSITIONALLOWED", 0x1000),
];

/// `MM_STATUS` values in check order.
pub const STATUS_NAMES: [(&str, u32); 22] = [
    ("CONSTRUCT", 0x20),
    ("MINING", 0x40),
    ("LUMBERING", 0x80),
    ("HARVESTING", 0x400),
    ("EVOKING", 0x20_0000),
    ("ISMOUNTED", 0x2000),
    ("CARRIING", 0x4000),
    ("ISMOUNTEDONBOAT", 0x8000),
    ("DIVING", 0x20000),
    ("SWIMMING", 0x10000),
    ("MASKED", 0x10_0000),
    ("WAITING4KILLING", 0x40_0000),
    ("MOVE", 0x1),
    ("HEAL", 0x8_0000),
    ("NOTWAITING", 0xF8EC_0FFF),
    ("NOTWAITINGORLUMBERING", 0xFFEC_0F7F),
    ("NOTWAITINGORMINING", 0xFFEC_0FBF),
    ("NOTWAITINGORHARVESTING", 0xFFEC_0BFF),
    ("FIGHTING", 0x2),
    ("QUARTERED", 0x100),
    ("ONWALL", 0x200_0000),
    ("DIED", 0x4_0000),
];

fn find(table: &[(&str, u32)], name: &str) -> Option<u32> {
    table.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
}

/// Path to the current filter of the chain (`MM_NEXT` appends to the last one).
fn last(filter: &mut MatchFilter) -> &mut MatchFilter {
    let mut f = filter;
    while f.next.is_some() {
        f = f.next.as_mut().unwrap();
    }
    f
}

/// `MatchFilter_ParseBlock`: lines up to `MM_END`; `Err` = abort (the loader then discards the
/// rest of the file). Prefixes of fixed length, value from `Mid(n + 1)` without trim.
fn parse_block(
    lines: &mut std::slice::Iter<'_, String>,
    filter: &mut MatchFilter,
    enabled: &mut bool,
    defines: &TargetDefines,
) -> Result<(), String> {
    for line in lines.by_ref() {
        let line = line.as_str();
        let f = last(filter);
        if line == "DISABLERBUTTON" {
            *enabled = false;
        } else if line == "MM_END" {
            return Ok(());
        } else if line == "MM_NEXT" {
            f.next = Some(Box::default());
        } else if left(line, 8) == "MM_MODE2" {
            let value = mid(line, 9);
            f.mode2 |= find(&MODE2_NAMES, value)
                .ok_or_else(|| format!("MM_MODE2 value \"{value}\" unknown"))?;
        } else if left(line, 7) == "MM_MODE" {
            let value = mid(line, 8);
            f.mode |= find(&MODE_NAMES, value)
                .ok_or_else(|| format!("MM_MODE value \"{value}\" unknown"))?;
        } else if left(line, 9) == "MM_PROPID" || left(line, 11) == "MM_PROPERTY" {
            let n = if left(line, 9) == "MM_PROPID" { 10 } else { 12 };
            let name = mid(line, n);
            f.prop_id = defines
                .lookup(name)
                .ok_or_else(|| format!("name \"{name}\" missing in targetdefines.ini"))?
                as u32;
        } else if left(line, 12) == "MM_MATCHBITS" {
            let name = mid(line, 13);
            f.match_bits |= defines
                .lookup(name)
                .ok_or_else(|| format!("name \"{name}\" missing in targetdefines.ini"))?
                as u32;
        } else if left(line, 7) == "MM_GUID" {
            if let Some(v) = scan_int(mid(line, 8)) {
                f.guid = v;
            }
        } else if left(line, 14) == "MM_METHODINDEX" {
            if let Some(v) = scan_int(mid(line, 15)) {
                f.method = v;
            }
        } else if left(line, 19) == "MM_IS_ATTACKING_WHO" {
            f.who |= match mid(line, 20) {
                "ME" => 1,
                "MY_FRIEND" => 2,
                "MY_FORMATION" => 4,
                _ => 0,
            };
        } else if left(line, 9) == "MM_STATUS" {
            f.status |= find(&STATUS_NAMES, mid(line, 10)).unwrap_or(0);
        }
    }
    Err("end of file in the filter block".into())
}

/// `GameWorld_LoadTargetsIni`: `BEGIN` creates an entry, `END` appends it, `BUTTONMSG` and
/// `MAUSICONID` via `%d`, `MY_MM`/`TARGET_MM` read a filter block (everything exact as a whole
/// line or prefix). An abort in the block ends reading; closed entries stay.
pub fn parse_targets(text: &str, defines: &TargetDefines) -> TargetFile {
    let lines = game_lines(text);
    let mut out = TargetFile::default();
    let mut current = TargetEntry::new();
    let mut iter = lines.iter();
    while let Some(line) = iter.next() {
        let line_no = lines.len() - iter.len();
        let line = line.as_str();
        if line == "BEGIN" {
            current = TargetEntry::new();
        } else if line == "END" {
            out.entries.push(current.clone());
        } else if left(line, 9) == "BUTTONMSG" {
            if let Some(v) = scan_int(mid(line, 10)) {
                current.button_msg = v;
            }
        } else if left(line, 10) == "MAUSICONID" {
            if let Some(v) = scan_int(mid(line, 11)) {
                current.mouse_icon = v;
            }
        } else if line == "MY_MM" || line == "TARGET_MM" {
            let filter = if line == "MY_MM" {
                &mut current.my
            } else {
                &mut current.target
            };
            if let Err(reason) = parse_block(&mut iter, filter, &mut current.enabled, defines) {
                out.aborted = Some(format!("line {line_no}: {reason}"));
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINES: &str =
        "PROP_LUMBERING 8\r\nMID_PROP_ARMING_ISARMED 295\r\nMID_PROP_BUILDING_FARM 515\r\n";

    #[test]
    fn entries_parse_with_names_chains_and_the_ineffective_disabler() {
        let d = parse_target_defines(DEFINES);
        assert_eq!(d.lookup("PROP_LUMBERING"), Some(8));
        assert_eq!(d.lookup("prop_lumbering"), None);
        let text = "#Comment\r\nBEGIN\r\nDISABLERBUTTON\r\nBUTTONMSG 485\r\nMY_MM\r\nMM_MODE MM_CHECKNOTHANDLETYPE\r\nMM_MATCHBITS MID_PROP_ARMING_ISARMED\r\nMM_STATUS DOESNOTEXIST\r\nMM_STATUS ISMOUNTED\r\nMM_MDOE MM_CHECKPROPERTY\r\nMM_MODE_MM_ANDALL\r\nMM_END\r\nTARGET_MM\r\nMM_MODE MM_ORNEXTMATCH\r\nMM_NEXT\r\nMM_MODE MM_CHECKHANDLETYPE\r\nMM_MATCHBITS MID_PROP_BUILDING_FARM\r\nMM_MODE2 MM2_CALLMETHOD\r\nMM_METHODINDEX 4x\r\nMM_IS_ATTACKING_WHO ME\r\nMM_END\r\nMAUSICONID 7\r\nEND\r\n";
        let file = parse_targets(text, &d);
        assert_eq!(file.aborted, None);
        let e = &file.entries[0];
        // DISABLERBUTTON outside a block: no effect.
        assert!(e.enabled);
        assert_eq!((e.button_msg, e.mouse_icon), (485, 7));
        assert_eq!(e.my.mode, 0x4000 | 0x10);
        assert_eq!((e.my.match_bits, e.my.status), (295, 0x2000));
        assert_eq!(e.target.mode, 0x2000_0000);
        let next = e.target.next.as_ref().unwrap();
        assert_eq!((next.mode, next.match_bits, next.mode2), (0x2, 515, 0x2));
        assert_eq!((next.method, next.who), (4, 1));
        // In the block DISABLERBUTTON takes effect.
        let off = parse_targets("BEGIN\r\nMY_MM\r\nDISABLERBUTTON\r\nMM_END\r\nEND\r\n", &d);
        assert!(!off.entries[0].enabled);
    }

    #[test]
    fn runtime_words_start_at_zero_from_the_parser_and_minus_one_when_constructed() {
        let d = parse_target_defines(DEFINES);
        let file = parse_targets("BEGIN\r\nMY_MM\r\nMM_NEXT\r\nMM_END\r\nEND\r\n", &d);
        let f = &file.entries[0].my;
        for g in [f, f.next.as_deref().unwrap()] {
            assert_eq!(
                (g.fog_player, g.ref_object, g.layer, g.region),
                (0, 0, 0, 0)
            );
            assert_eq!((g.range_max, g.range_min, g.tether), (0, 0, 0));
            assert_eq!((g.home, g.angle_tolerance), ((0, 0), 0));
            assert!(!g.line_of_sight);
        }
        let c = MatchFilter::constructed();
        assert_eq!(
            (c.layer, c.region, c.range_max, c.range_min),
            (-1, -1, -1, -1)
        );
        assert_eq!((c.tether, c.fog_player, c.ref_object), (-1, 0, 0));
        assert_eq!((c.home, c.angle_tolerance), ((-1, -1), 0));
        assert!(!c.line_of_sight && c.next.is_none());
    }

    #[test]
    fn unknown_mode_values_or_names_abort_the_rest_of_the_file() {
        let d = parse_target_defines(DEFINES);
        let text = "BEGIN\r\nBUTTONMSG 1\r\nEND\r\nBEGIN\r\nBUTTONMSG 2\r\nMY_MM\r\nMM_MODE MM_CHECKHANDLETYP\r\nMM_END\r\nEND\r\nBEGIN\r\nBUTTONMSG 3\r\nEND\r\n";
        let file = parse_targets(text, &d);
        assert_eq!(file.entries.len(), 1);
        assert!(
            file.aborted
                .as_deref()
                .unwrap()
                .contains("MM_CHECKHANDLETYP")
        );
        let unknown = parse_targets("BEGIN\r\nMY_MM\r\nMM_PROPID PROP_X\r\n", &d);
        assert!(unknown.aborted.unwrap().contains("PROP_X"));
        let open = parse_targets("BEGIN\r\nMY_MM\r\nMM_MODE MM_ANDALL\r\n", &d);
        assert!(open.aborted.unwrap().contains("end of file"));
    }
}
