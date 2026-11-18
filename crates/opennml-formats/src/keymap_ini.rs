//! `keymap.ini`: keys that the game polls without a buffer (`KD_...`, `data/gifcon/keymap.ini`).
//!
//! Reader of the original: `FUN_00414E50`. Lines `NAME = CHORD`; the chord is a key name or a
//! combination (see `opennml_ui::keys`).

use crate::config::{game_lines, trim_left};

/// Path in the game directory (VFS).
pub const KEYMAP_INI_PATH: &str = "data/gifcon/keymap.ini";

const SPACE: [char; 6] = [' ', '\t', '\n', '\x0B', '\x0C', '\r'];

/// Reads `keymap.ini` in file order like `FUN_00414E50`: lines with an `=` at a position `> 0`; the name on the
/// left is trimmed at both ends (`TrimLeft`, `TrimRight`), the chord to the right of it is returned as text.
///
/// Deviation: the original passes the value on untrimmed (`" SNAPSHOT"`), the chord parser
/// (`FUN_0049C2B0` -> `FUN_0049C050`) trims itself. The reimplementation trims the chord here, which is
/// equivalent.
pub fn parse_keymap_ini(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in game_lines(text) {
        let Some(pos) = line.find('=') else {
            continue;
        };
        if pos == 0 {
            continue;
        }
        let name = trim_left(&line[..pos]).trim_end_matches(SPACE);
        let chord = trim_left(&line[pos + 1..]).trim_end_matches(SPACE);
        out.push((name.to_string(), chord.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_chords_are_trimmed_and_other_lines_skipped() {
        let text = "#Keys\r\n\r\nKD_HARDCOPY                = SNAPSHOT\r\n  KD_X\t=  SHIFT D  \r\n=A\r\nno equals\r\nKD_E =\r\nKD_F = a=b\r\n";
        assert_eq!(
            parse_keymap_ini(text),
            vec![
                ("KD_HARDCOPY".to_string(), "SNAPSHOT".to_string()),
                ("KD_X".to_string(), "SHIFT D".to_string()),
                ("KD_E".to_string(), String::new()),
                ("KD_F".to_string(), "a=b".to_string()),
            ]
        );
    }
}
