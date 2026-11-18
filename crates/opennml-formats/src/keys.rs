//! `defaultkeys.ini`: key layout defaults (`data/gifcon/defaultkeys.ini`).
//!
//! The original loads it in `FUN_00494FD0` into the key layout of `Engine.ini` (`m_KeyMap`,
//! see `docs/formats/engine-ini.md`). Lines: `id=KEY` (key name or combination like
//! `ALT N`, also empty), `#` begins a comment.

/// Path in the game directory (VFS).
pub const DEFAULT_KEYS_PATH: &str = "data/gifcon/defaultkeys.ini";

/// Reads `defaultkeys.ini` in file order. Empty lines, comments, lines without `=` and
/// lines without a number before the `=` are skipped; an empty value (`0=`) is a valid
/// entry.
pub fn parse_default_keys(text: &str) -> Vec<(i32, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (id, key) = line.split_once('=')?;
            Some((id.trim().parse().ok()?, key.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keys_keep_the_file_order() {
        let text =
            "#\r\n# General commands\r\n#\r\n\r\n0=\r\n\r\n113=+\r\n578=F1\r\n573=ALT N\r\n9=\r\n";
        assert_eq!(
            parse_default_keys(text),
            vec![
                (0, String::new()),
                (113, "+".to_string()),
                (578, "F1".to_string()),
                (573, "ALT N".to_string()),
                (9, String::new()),
            ]
        );
    }

    #[test]
    fn lines_without_equals_or_number_are_skipped() {
        let text = "\u{feff}5=B\nHeading\nabc=X\n=Y\n12\n 7 = SHIFT D \n#8=A\n";
        assert_eq!(
            parse_default_keys(text),
            vec![(5, "B".to_string()), (7, "SHIFT D".to_string())]
        );
    }

    #[test]
    fn only_the_first_equals_splits_a_line() {
        assert_eq!(parse_default_keys("113==\n"), vec![(113, "=".to_string())]);
        assert!(parse_default_keys("").is_empty());
    }
}
