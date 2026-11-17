//! `keys.ini`: keys that trigger a console command, with context (`data/gifcon/keys.ini`).
//!
//! Reader of the original: `FUN_0049CAA0`. Lines `KEY <action> <type> <nation> <selection> <command>`;
//! everything else (comments `#`, empty lines, `KEYS ...`) is skipped by the reader. The evaluation (which line matches
//! which key and which context) belongs to the host, not to the format reader.

use crate::config::{game_lines, left, mid};
use crate::property::atol;

/// Path in the game directory (VFS).
pub const KEYS_INI_PATH: &str = "data/gifcon/keys.ini";

/// A `KEY` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysIniLine {
    /// Action number; the binding for it is in `Engine.ini` (`m_KeyMap`, default `defaultkeys.ini`).
    pub action: i32,
    /// Type of the last selected object (header comment of the file); in the shipped data 0, 2 and 3.
    pub kind: i32,
    /// Nation (0 = any).
    pub nation: i32,
    /// Selection (0 = regardless of whether something is selected).
    pub selection: i32,
    /// The rest of the line, unchanged (no trimming).
    pub command: String,
}

/// Reads `keys.ini` in file order like `FUN_0049CAA0` (`0x0049CC91`...`0x0049CE31`): only lines whose first four
/// characters are exactly `KEY `. Then four numbers, each up to the next single space (`_atol`: leading digits,
/// otherwise 0); the rest is the command.
///
/// Quirk: the original takes a field only if the space is at a position `> 0`; otherwise (double space, line
/// ended) it keeps the stack value of the previous line and the text does not advance. The reimplementation then
/// takes 0 and does not advance either. This does not occur in the shipped data.
pub fn parse_keys_ini(text: &str) -> Vec<KeysIniLine> {
    let mut out = Vec::new();
    for line in game_lines(text) {
        if left(&line, 4) != "KEY " {
            continue;
        }
        let mut rest = mid(&line, 4);
        let mut fields = [0i32; 4];
        for field in &mut fields {
            if let Some(pos) = rest.find(' ')
                && pos > 0
            {
                *field = atol(&rest[..pos]) as i32;
                rest = &rest[pos + 1..];
            }
        }
        out.push(KeysIniLine {
            action: fields[0],
            kind: fields[1],
            nation: fields[2],
            selection: fields[3],
            command: rest.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ini_reads_only_key_lines_and_keeps_the_rest_as_command() {
        let text = "# Header\r\n\r\nKEY 113 0 0 0 /game/changegamespeed(+1)\r\n#KEY 5 0 0 0 x\r\nKEYS 6 0 0 0 y\r\nKEY 530 3 2 1 /game/toolbar/Xbutton(530)\r\n key 7 0 0 0 z\r\nKEY 9 2 0 0 a  b \r\nKEY";
        assert_eq!(
            parse_keys_ini(text),
            vec![
                KeysIniLine {
                    action: 113,
                    kind: 0,
                    nation: 0,
                    selection: 0,
                    command: "/game/changegamespeed(+1)".into()
                },
                KeysIniLine {
                    action: 530,
                    kind: 3,
                    nation: 2,
                    selection: 1,
                    command: "/game/toolbar/Xbutton(530)".into()
                },
                // Double spaces and trailing spaces in the command are preserved.
                KeysIniLine {
                    action: 9,
                    kind: 2,
                    nation: 0,
                    selection: 0,
                    command: "a  b ".into()
                },
            ]
        );
    }

    #[test]
    fn numbers_read_like_atol_and_double_spaces_do_not_advance() {
        // `_atol`: leading digits, otherwise 0.
        let lines = parse_keys_ini("KEY 12x -3 +4 5q cmd\n");
        assert_eq!(
            (lines[0].action, lines[0].kind, lines[0].nation),
            (12, -3, 4)
        );
        assert_eq!((lines[0].selection, lines[0].command.as_str()), (5, "cmd"));
        // Double space: all fields stay 0, the text does not advance.
        let lines = parse_keys_ini("KEY  5 0 0 0 cmd\n");
        assert_eq!(lines[0].action, 0);
        assert_eq!(lines[0].command, " 5 0 0 0 cmd");
    }
}
