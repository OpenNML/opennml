//! `bindings2.ini`: fixed key bindings `bind("KEY","/command")` (`data/gifcon/bindings2.ini`).
//!
//! The bindings apply before `keys.ini`; evaluation and comparison of the key set are done in the host.

use crate::config::trim_left;

/// Path in the game directory (VFS).
pub const BINDINGS_INI_PATH: &str = "data/gifcon/bindings2.ini";

/// A binding: chord (key name or combination, e.g. `CONTROL P`) and console command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The chord, as in the file between the first quotation marks.
    pub chord: String,
    /// The command, as in the file between the second quotation marks (`/game/togglepause`).
    pub command: String,
}

/// Reads `bindings2.ini` in file order: per line `bind("KEY","/command")`. Spaces after the parenthesis and
/// before and after the comma are tolerated (three bindings in the shipped data have `, `); comments and all
/// other lines are skipped.
pub fn parse_bindings(text: &str) -> Vec<Binding> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Binding> {
    let rest = trim_left(line).strip_prefix("bind(")?;
    let rest = trim_left(rest).strip_prefix('"')?;
    let (chord, rest) = rest.split_once('"')?;
    let rest = trim_left(rest).strip_prefix(',')?;
    let rest = trim_left(rest).strip_prefix('"')?;
    let (command, _) = rest.split_once('"')?;
    Some(Binding {
        chord: chord.to_string(),
        command: command.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_tolerate_spaces_after_the_comma_and_skip_other_lines() {
        let text = "# Header\r\n\r\nbind(\"CONTROL 0\",\"/game/storegroup(0)\")\r\nbind(\"HOME\", \"/setcamera\")\r\nbind(\"PAUSE\" ,  \"/game/togglepause\")\r\nbind(\"X\")\r\nbind(\"Y\",\"/open\r\nanother line\r\n";
        assert_eq!(
            parse_bindings(text),
            vec![
                Binding {
                    chord: "CONTROL 0".into(),
                    command: "/game/storegroup(0)".into()
                },
                Binding {
                    chord: "HOME".into(),
                    command: "/setcamera".into()
                },
                Binding {
                    chord: "PAUSE".into(),
                    command: "/game/togglepause".into()
                },
            ]
        );
    }
}
