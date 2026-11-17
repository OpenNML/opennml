//! `tempupgrades.ini` (`GameWorld_LoadTempUpgradesIni` `0x0048AB60`, `docs/formats/gifcon-upgrades.md`,
//! re/temp-upgrades 1.1): records of the temporary upgrades (4450 ... 4462). Line-oriented
//! (no brace grammar); it is read with prefix comparisons `Left(n)` and whole lines, as in the original.

use crate::FormatError;
use crate::config::{game_lines, left, mid};
use crate::error::syntax;

pub const TEMP_UPGRADES_PATH: &str = "data/gifcon/tempupgrades.ini";

/// Flags of a record (word 0, re/temp-upgrades 1.1).
pub mod flags {
    /// `EFFECT` (together with `HEIGHT`).
    pub const EFFECT: u32 = 0x01;
    /// `MSGSTART`.
    pub const MSGSTART: u32 = 0x02;
    /// `MSGEND`.
    pub const MSGEND: u32 = 0x04;
    /// `TAKEUPGRADEVALUES`.
    pub const TAKEUPGRADEVALUES: u32 = 0x08;
    /// `TEMPRADIUSUPGRADE` (together with `TEMPORARY`).
    pub const RADIUS: u32 = 0x10;
    /// `TEMPHANDLELISTUPGRADE` (together with `TEMPORARY`).
    pub const HANDLELIST: u32 = 0x20;
    /// `EFFECT` (together with `EFFECT`).
    pub const HEIGHT: u32 = 0x40;
    /// `UPGRADETEMPORARY`, `TEMPRADIUSUPGRADE`, `TEMPHANDLELISTUPGRADE`: temporary.
    pub const TEMPORARY: u32 = 0x80;
    /// `UPGRADETIME`.
    pub const TIME: u32 = 0x100;
    /// `ENEMYUPGRADE`.
    pub const ENEMY: u32 = 0x200;
    /// `MSGTICK`.
    pub const MSGTICK: u32 = 0x400;
}

/// Message of a `MSGSTART`/`MSGEND`/`MSGTICK` line: code (hex) and `argc` arguments.
/// Without a line, `code = 0` and `args` is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordMessage {
    pub code: u32,
    /// Exactly `argc` entries; missing numbers of the line are 0.
    pub args: Vec<i32>,
}

/// A record (`TEMPUPGRADE n ... END`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TempUpgradeRecord {
    pub guid: u32,
    /// Flags, see [`flags`].
    pub flags: u32,
    /// `UPGRADETIME n` raw in ms (only with flag `TIME`).
    pub time_ms: i32,
    /// `EFFECT h path`: path (only with flag `EFFECT`).
    pub effect_path: String,
    pub effect_height: i32,
    /// `MSGSTART` (only with flag `MSGSTART`).
    pub msg_start: RecordMessage,
    /// `MSGEND` **or** `MSGTICK`: the same slot in the original, the later line wins (W-18).
    pub msg_end: RecordMessage,
}

/// More numbers than `argc` + 8 arguments overwrite the stack in the original (W-18); here a load error (exception A7).
const MAX_NUMBERS: usize = 9;
const MAX_ARGS: usize = 8;

/// Reads the file like the original: per line (trimmed on both sides) the prefixes `Left(12)` for
/// `TEMPUPGRADE ` and `UPGRADETIME `, `Left(9)` for `MSGSTART `, `Left(7)` for `MSGEND `, `MSGTICK` and
/// `EFFECT `, otherwise whole lines; everything else (comments, blank lines) is skipped. The current
/// record lives on past `END`: `TEMPUPGRADE` only resets flags and GUID, the path, duration and messages
/// of the predecessor remain (ineffective without flags, W-18). `END` stores a copy of the current
/// record; lines after the last `END` are dropped. `TEMPUPGRADETIME` from the file header is not
/// recognized because of the prefix comparison.
pub fn parse_temp_upgrades(text: &str) -> Result<Vec<TempUpgradeRecord>, FormatError> {
    let mut records = Vec::new();
    let mut current = TempUpgradeRecord::default();
    for (index, raw) in game_lines(text).iter().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if left(line, 12) == "TEMPUPGRADE " {
            current.flags = 0;
            current.guid = number(line_no, mid(line, 12))? as u32;
        } else if line == "TAKEUPGRADEVALUES" {
            current.flags |= flags::TAKEUPGRADEVALUES;
        } else if line == "TEMPRADIUSUPGRADE" {
            current.flags |= flags::RADIUS | flags::TEMPORARY;
        } else if line == "TEMPHANDLELISTUPGRADE" {
            current.flags |= flags::HANDLELIST | flags::TEMPORARY;
        } else if line == "UPGRADETEMPORARY" {
            current.flags |= flags::TEMPORARY;
        } else if line == "ENEMYUPGRADE" {
            current.flags |= flags::ENEMY;
        } else if left(line, 12) == "UPGRADETIME " {
            current.flags |= flags::TIME;
            current.time_ms = number(line_no, mid(line, 12))?;
        } else if left(line, 9) == "MSGSTART " {
            current.flags |= flags::MSGSTART;
            current.msg_start = message(line_no, mid(line, 9))?;
        } else if left(line, 7) == "MSGEND " {
            current.flags |= flags::MSGEND;
            current.msg_end = message(line_no, mid(line, 7))?;
        } else if left(line, 7) == "MSGTICK" {
            current.flags |= flags::MSGTICK;
            current.msg_end = message(line_no, mid(line, 8))?;
        } else if left(line, 7) == "EFFECT " {
            current.flags |= flags::EFFECT | flags::HEIGHT;
            let rest = mid(line, 7).trim_start();
            let (height, path) = rest.split_once([' ', '\t']).unwrap_or((rest, ""));
            current.effect_height = number(line_no, height)?;
            current.effect_path = path.trim().to_string();
        } else if line == "END" {
            records.push(current.clone());
        }
    }
    Ok(records)
}

fn number(line: usize, token: &str) -> Result<i32, FormatError> {
    let token = token.trim();
    token
        .parse::<i32>()
        .map_err(|_| syntax(line, &format!("number expected, found \"{token}\"")))
}

/// `x argc a1 ... a8` (`x` hex `%X`, the rest `%d`); at most 9 numbers, `argc` at most 8.
fn message(line: usize, rest: &str) -> Result<RecordMessage, FormatError> {
    let mut tokens = rest.split_whitespace();
    let code_token = tokens.next().unwrap_or("");
    let code = u32::from_str_radix(code_token, 16).map_err(|_| {
        syntax(
            line,
            &format!("message code (hex) expected, found \"{code_token}\""),
        )
    })?;
    let numbers: Vec<i32> = tokens.map(|t| number(line, t)).collect::<Result<_, _>>()?;
    if numbers.len() > MAX_NUMBERS {
        return Err(syntax(
            line,
            &format!(
                "message with {} numbers (at most {MAX_NUMBERS}: count and {MAX_ARGS} arguments)",
                numbers.len()
            ),
        ));
    }
    let argc = numbers.first().copied().unwrap_or(0);
    let argc = usize::try_from(argc)
        .ok()
        .filter(|&n| n <= MAX_ARGS)
        .ok_or_else(|| {
            syntax(
                line,
                &format!("argument count {argc} outside 0 to {MAX_ARGS}"),
            )
        })?;
    let args = (0..argc)
        .map(|i| numbers.get(1 + i).copied().unwrap_or(0))
        .collect();
    Ok(RecordMessage { code, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    // QUIRK K-286: kept
    #[test]
    fn reads_records_like_the_game() {
        let t = "TEMPUPGRADE 4450\nTEMPHANDLELISTUPGRADE\nEFFECT 0 data/effects/x.pef\nMSGTICK 57F 2 0 1000\nEND\n\
                 TEMPUPGRADE 4451\nTEMPHANDLELISTUPGRADE\nMSGSTART 601 1 300\nMSGEND 601 1 100\nUPGRADETIME 5000\nEND\n";
        let r = parse_temp_upgrades(t).unwrap();
        assert_eq!((r[0].guid, r[0].flags), (4450, 0xE1 | 0x400));
        assert_eq!(
            (r[0].msg_end.code, r[0].msg_end.args.clone()),
            (0x57F, vec![0, 1000])
        );
        assert_eq!(
            (r[1].flags, r[1].time_ms),
            (0xA0 | 0x02 | 0x04 | 0x100, 5000)
        );
        assert_eq!(r[1].msg_end.args, vec![100]);
        // W-18: TEMPUPGRADE only resets flags and GUID - the path and messages of the predecessor
        // remain.
        assert_eq!(r[1].effect_path, "data/effects/x.pef");
        assert_eq!(
            r[1].msg_start,
            RecordMessage {
                code: 0x601,
                args: vec![300]
            }
        );
    }

    // QUIRK W-18: kept
    #[test]
    fn more_than_nine_numbers_in_a_message_line_are_a_load_error() {
        // A7: "MSGSTART 601 9 1 2 3 4 5 6 7 8 9" (10 numbers) -> Err with line number; argc > 8 likewise.
        let ten = "TEMPUPGRADE 1\nMSGSTART 601 9 1 2 3 4 5 6 7 8 9\nEND\n";
        let err = parse_temp_upgrades(ten).unwrap_err();
        assert!(matches!(err, FormatError::Syntax { line: 2, .. }), "{err}");
        // Nine numbers (argc 8 and eight arguments) are still allowed.
        let nine = "TEMPUPGRADE 1\nMSGEND 601 8 1 2 3 4 5 6 7 8\nEND\n";
        assert_eq!(
            parse_temp_upgrades(nine).unwrap()[0].msg_end.args,
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        let argc = "TEMPUPGRADE 1\nEND\nMSGTICK 601 9\nEND\n";
        let err = parse_temp_upgrades(argc).unwrap_err();
        assert!(matches!(err, FormatError::Syntax { line: 3, .. }), "{err}");
        let negative = "MSGSTART 601 -1\n";
        assert!(parse_temp_upgrades(negative).is_err());
    }

    // QUIRK W-18: kept
    #[test]
    fn temp_upgrade_time_from_the_header_is_not_recognised() {
        // W-18: the line "TEMPUPGRADETIME 60" changes neither flags nor time_ms.
        let t = "TEMPUPGRADE 7\nTEMPUPGRADETIME 60\nEND\nTEMPUPGRADE 8\nUPGRADETIME 60\nEND\n";
        let r = parse_temp_upgrades(t).unwrap();
        assert_eq!((r[0].flags, r[0].time_ms), (0, 0));
        assert_eq!((r[1].flags, r[1].time_ms), (flags::TIME, 60));
    }

    #[test]
    fn missing_numbers_are_zero_and_messages_without_a_line_are_empty() {
        // "MSGSTART 601 3 7": argc 3, missing numbers 0; without a line code 0 and no arguments.
        let t = "TEMPUPGRADE 5\nMSGSTART 601 3 7\nEND\nTEMPUPGRADE 6\nEND\n";
        let r = parse_temp_upgrades(t).unwrap();
        assert_eq!(r[0].msg_start.args, [7, 0, 0]);
        assert_eq!(r[0].msg_end, RecordMessage::default());
        // The second record inherits the message of the first, but not the flags.
        assert_eq!((r[1].flags, r[1].msg_start.code), (0, 0x601));
    }

    // QUIRK W-18: kept
    #[test]
    fn the_later_of_msgtick_and_msgend_wins_the_shared_slot() {
        let t = "TEMPUPGRADE 5\nMSGTICK 57F 1 9\nMSGEND 601 1 100\nEND\n";
        let r = parse_temp_upgrades(t).unwrap();
        assert_eq!(r[0].flags, flags::MSGTICK | flags::MSGEND);
        assert_eq!(
            r[0].msg_end,
            RecordMessage {
                code: 0x601,
                args: vec![100]
            }
        );
    }

    #[test]
    fn comments_blank_lines_and_crlf_are_ignored() {
        let t = "# Header\r\n\r\n#TEMPUPGRADETIME - if\r\nTEMPUPGRADE 9\r\nTAKEUPGRADEVALUES\r\nENEMYUPGRADE\r\nTEMPRADIUSUPGRADE\r\nEND\r\n";
        let r = parse_temp_upgrades(t).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].flags, flags::TAKEUPGRADEVALUES | flags::ENEMY | 0x90);
    }
}
