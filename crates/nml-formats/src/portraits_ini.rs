//! Portrait lists of the cinemascope area: `data/gifcon/portraits.ini` and `data/gifcon/portraitframes.ini` (both via
//! `FUN_00445E10` into the reader `FUN_004F9D60`, called by the constructor `0x004483D0`), see re/campaign-ui
//! D6.1 and D6.17.

use std::collections::BTreeMap;

use crate::config::game_lines;
use crate::cscan::scan_int;

/// Path of the portrait list (constructor `0x004483D0`, record `+0x4C`).
pub const PORTRAITS_INI_PATH: &str = "data/gifcon/portraits.ini";
/// Path of the frame list (record `+0x50`).
pub const PORTRAIT_FRAMES_INI_PATH: &str = "data/gifcon/portraitframes.ini";

/// `ID=NAME=path` like the reader `FUN_004F9D60` (`0x004F9E70...0x004F9FE2`): no trimming, the line needs a `=`; the ID
/// is read by `sscanf("%d")` from the part before it (leading spaces are skipped, text after the number is ignored). If
/// the ID already exists, the line is skipped (the **first** entry wins); then a second `=` must follow (otherwise the
/// line is skipped and does not occupy the ID), the path is everything after it, untrimmed.
///
/// If the `sscanf` fails (no number), the ID of the previous line stays - the original never resets it. Deviation:
/// in the first line it is stack content there; the reimplementation skips the line.
pub fn parse_portraits_ini(text: &str) -> BTreeMap<i32, String> {
    let mut out = BTreeMap::new();
    let mut last_id: Option<i32> = None;
    for line in game_lines(text) {
        let Some(first) = line.find('=') else {
            continue;
        };
        if let Some(id) = scan_int(&line[..first]) {
            last_id = Some(id);
        }
        let Some(id) = last_id else {
            continue;
        };
        if out.contains_key(&id) {
            continue;
        }
        let rest = &line[first + 1..];
        let Some(second) = rest.find('=') else {
            continue;
        };
        out.insert(id, rest[second + 1..].to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portraits_ini_takes_path_after_second_equals_and_first_id_wins() {
        let m = parse_portraits_ini(
            "#List\r\n1000=A=x.tga\r\n1000=B=y.tga\r\n5=A=b=c.tga\r\n 6 =N= p q.tga \r\n",
        );
        assert_eq!(m[&1000], "x.tga");
        assert_eq!(m[&5], "b=c.tga");
        // Leading spaces of the number are skipped, the path stays untrimmed.
        assert_eq!(m[&6], " p q.tga ");
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn portraits_ini_skips_lines_without_number_or_second_equals() {
        // First line without a number: stack content in the original, the reimplementation skips it.
        let m = parse_portraits_ini("=A=first.tga\r\nname=A=first2.tga\r\n");
        assert!(m.is_empty());
        // Only one `=`: does not occupy the ID, a later valid line with the same ID wins.
        let m = parse_portraits_ini("3=only\r\n3=A=p.tga\r\nwithout equals\r\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[&3], "p.tga");
    }

    #[test]
    fn portraits_ini_line_without_number_reuses_the_previous_id() {
        // `sscanf` fails, the ID of the previous line stays: here it has no path yet.
        let m = parse_portraits_ini("7=only\r\nx=A=p.tga\r\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[&7], "p.tga");
        // If the ID is already entered, the line without a number is skipped.
        let m = parse_portraits_ini("7=A=a.tga\r\nx=A=p.tga\r\n8=A=b.tga\r\n");
        assert_eq!(m[&7], "a.tga");
        assert_eq!(m[&8], "b.tga");
        assert_eq!(m.len(), 2);
    }
}
