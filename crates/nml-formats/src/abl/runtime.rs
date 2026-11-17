//! Run-time view of an `.abl`: the blocks, conditions and actions, as `Script_ParseAblFile`
//! (`0x00566680`) delivers them to the original (re/scripts 3, E-119 ... E-123).

use std::fmt;

use super::model::{AblFile, LineEnd};
use crate::config::lower_german;
use crate::property::{atol, is_c_space};

/// A script that has been read: only active blocks in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFile {
    pub blocks: Vec<RtBlock>,
}

/// A block ("trigger") with conditions and actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtBlock {
    /// `Mid(15)` of the `[BESCHREIBUNG` line (from the 16th character, untrimmed, with `]`); shorter lines
    /// yield `""`. The name has no effect on the flow, only in the savegame (`m_TriggerName`).
    pub name: String,
    /// Source line (1-based).
    pub line: u32,
    pub conditions: Vec<RtCondition>,
    pub actions: Vec<RtAction>,
}

/// Condition: `or` is `prefix != '&'`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtCondition {
    pub or: bool,
    pub number: i32,
    pub params: Vec<String>,
    pub line: u32,
}

/// Action: `wait` is `prefix == 'w'`; the number is `i32` as in the original (truncated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtAction {
    pub wait: bool,
    pub number: i32,
    pub params: Vec<String>,
    pub line: u32,
}

/// Parser abort (E-120) with line; the caller adds the file path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AblError {
    pub line: u32,
    pub message: String,
}

impl fmt::Display for AblError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for AblError {}

/// Parameter rule of the original (E-123): trim, lowercase German-style, remove the prefixes `dialog `,
/// `player `, `camera ` in this order at most once each at the front, `false`/`true`
/// to `0`/`1`. `area ` stays.
pub fn normalize_param(raw: &str) -> String {
    let lowered = lower_german(raw.trim_matches(is_c_space));
    let mut p = lowered.as_str();
    for prefix in ["dialog ", "player ", "camera "] {
        p = p.strip_prefix(prefix).unwrap_or(p);
    }
    match p {
        "false" => "0".to_string(),
        "true" => "1".to_string(),
        _ => p.to_string(),
    }
}

/// `CString::Mid(5)` (`FUN_00563FA0`): drop the first five characters (the area parameter
/// `area <name>`), shorter texts yield `""`.
pub fn mid5(param: &str) -> &str {
    param.char_indices().nth(5).map_or("", |(i, _)| &param[i..])
}

impl AblFile {
    /// Applies the parser rules of the original to the line texts (not to the classification).
    pub fn runtime(&self) -> Result<ScriptFile, AblError> {
        let mut blocks: Vec<RtBlock> = Vec::new();
        let mut deactivated = false;
        // Index of the open block; `None` before the first and after a deactivated block.
        let mut current: Option<usize> = None;
        for (index, text) in self.logical_lines().iter().enumerate() {
            let text = text.as_str();
            let number = index as u32 + 1;
            if text.contains('{') {
                deactivated = false;
                continue;
            }
            if text.contains("[DEACTIVATE]") {
                deactivated = true;
                continue;
            }
            if text.contains("[BESCHREIBUNG") {
                if deactivated {
                    current = None;
                } else {
                    blocks.push(RtBlock {
                        name: text.chars().skip(15).collect(),
                        line: number,
                        conditions: Vec::new(),
                        actions: Vec::new(),
                    });
                    current = Some(blocks.len() - 1);
                }
                continue;
            }
            let is_condition = text.contains("[BEDINGUNG=");
            if !is_condition && !text.contains("[AKTION=") {
                continue;
            }
            let Some(block) = current.map(|i| &mut blocks[i]) else {
                continue;
            };
            if text.contains('§') {
                continue;
            }
            let Some((before_colon, _)) = text.split_once(':') else {
                return Err(AblError {
                    line: number,
                    message: "action or condition line without ':' (parser abort in the original)"
                        .to_string(),
                });
            };
            let head = before_colon
                .split_once('=')
                .map_or(before_colon, |(_, h)| h);
            let mut chars = head.chars();
            let Some(prefix) = chars.next() else {
                return Err(AblError {
                    line: number,
                    message:
                        "action or condition line with empty head (parser abort in the original)"
                            .to_string(),
                });
            };
            let head = chars.as_str();
            let (number_text, params) = match head.split_once(';') {
                Some((n, r)) => {
                    let rest = r.trim_start_matches(is_c_space);
                    // Empty remainder after `;`: the original inserts `Fehlerhafte_ParameterListe`.
                    let rest = if rest.is_empty() {
                        "Fehlerhafte_ParameterListe"
                    } else {
                        rest
                    };
                    (n, split_params(rest))
                }
                None => (head, Vec::new()),
            };
            let value = atol(number_text) as i32;
            if is_condition {
                block.conditions.push(RtCondition {
                    or: prefix != '&',
                    number: value,
                    params,
                    line: number,
                });
            } else {
                block.actions.push(RtAction {
                    wait: prefix == 'w',
                    number: value,
                    params,
                    line: number,
                });
            }
        }
        Ok(ScriptFile { blocks })
    }

    /// Lines like `GameFile_ReadLine` (`0x00574C00`): only LF ends a line, every CR
    /// is dropped. Physical lines with [`LineEnd::Cr`] are joined with the following one; an
    /// empty tail without LF yields no line. The line numbers of the run-time view count
    /// these lines (1-based).
    fn logical_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut pending = String::new();
        for line in &self.lines {
            pending.push_str(&line.text);
            if line.end != LineEnd::Cr {
                lines.push(std::mem::take(&mut pending));
            }
        }
        if !pending.is_empty() {
            lines.push(pending);
        }
        lines
    }
}

/// Parameter list separated by `;`; an empty remainder yields no parameter, a trailing `;`
/// no empty last one (the placeholder for "nothing after the first `;`" comes from the caller).
fn split_params(rest: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut rest = rest;
    while !rest.is_empty() {
        let (part, tail) = rest.split_once(';').unwrap_or((rest, ""));
        params.push(normalize_param(part));
        rest = tail;
    }
    params
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn file(text: &str) -> AblFile {
        parse(&crate::text::encode_cp1252(text))
    }

    fn rt(text: &str) -> Result<ScriptFile, AblError> {
        file(text).runtime()
    }

    fn block(name: &str, body: &str) -> String {
        format!("{{\r\n\t[BESCHREIBUNG={name}]\r\n{body}}}\r\n")
    }

    #[test]
    fn prefix_is_always_cut_even_a_digit() {
        let file = rt(&block(
            "b",
            "\t[AKTION=151;x:Text]\r\n\t[BEDINGUNG=&5;50:Text]\r\n",
        ))
        .unwrap();
        let b = &file.blocks[0];
        assert_eq!(b.actions.len(), 1);
        assert_eq!(b.actions[0].number, 51);
        assert_eq!(b.actions[0].params, ["x"]);
        assert!(!b.actions[0].wait);
        assert_eq!(b.conditions[0].number, 5);
        assert_eq!(b.conditions[0].params, ["50"]);
    }

    #[test]
    fn brace_line_drops_its_action_and_ends_deactivation() {
        // A `{` line is dropped together with its action (E-121) and ends the deactivation.
        let text = "{\r\n\t[DEACTIVATE]\r\n\t[BESCHREIBUNG=dead]\r\n\t[AKTION=n1:x]\r\n{\r\n\t[BESCHREIBUNG=alive]\r\n\t[AKTION=n1;a:x]\r\n\t{ [AKTION=n2:y]\r\n\t[AKTION=n3:z]\r\n}\r\n";
        let file = rt(text).unwrap();
        assert_eq!(file.blocks.len(), 1);
        assert_eq!(file.blocks[0].name, "alive]");
        assert_eq!(file.blocks[0].line, 6);
        let numbers: Vec<_> = file.blocks[0].actions.iter().map(|a| a.number).collect();
        assert_eq!(numbers, [1, 3]);
    }

    #[test]
    fn section_sign_anywhere_drops_the_line() {
        // `§` anywhere (E-122), even without ':' - then no abort.
        let text = block(
            "b",
            "\t[AKTION=n1:§ old]\r\n\t[AKTION=n2:Text § middle]\r\n\t[BEDINGUNG=&3§without colon]\r\n\t[AKTION=n4;a:ok]\r\n",
        );
        let file = rt(&text).unwrap();
        assert!(file.blocks[0].conditions.is_empty());
        let numbers: Vec<_> = file.blocks[0].actions.iter().map(|a| a.number).collect();
        assert_eq!(numbers, [4]);
    }

    #[test]
    fn params_are_trimmed_lowered_and_stripped() {
        assert_eq!(normalize_param(" Player 6 "), "6");
        assert_eq!(normalize_param("DIALOG Dialog Intro"), "dialog intro");
        assert_eq!(normalize_param("Dialog Player 3"), "3");
        assert_eq!(normalize_param("Camera Player Dialog x"), "player dialog x");
        assert_eq!(normalize_param("Player Dialog x"), "dialog x");
        assert_eq!(normalize_param("TRUE"), "1");
        assert_eq!(normalize_param("False"), "0");
        assert_eq!(normalize_param("Area Gate"), "area gate");
        assert_eq!(normalize_param("ÄÖÜ"), "äöü");
        assert_eq!(normalize_param("Player"), "player");
        assert_eq!(normalize_param(""), "");
    }

    #[test]
    fn params_split_at_semicolons() {
        let file = rt(&block(
            "b",
            "\t[AKTION=n25; A Name ;15000;;Player 2:x]\r\n\t[AKTION=n9:x]\r\n\t[AKTION=n8;:x]\r\n",
        ))
        .unwrap();
        let a = &file.blocks[0].actions;
        assert_eq!(a[0].number, 25);
        assert_eq!(a[0].params, ["a name", "15000", "", "2"]);
        assert!(a[1].params.is_empty());
        // `;` without remainder: the original inserts the placeholder "Fehlerhafte_ParameterListe".
        assert_eq!(a[2].params, ["fehlerhafte_parameterliste"]);
    }

    #[test]
    fn empty_rest_after_semicolon_becomes_the_placeholder_parameter() {
        let file = rt(&block(
            "b",
            "\t[AKTION=n8; \t:x]\r\n\t[BEDINGUNG=&5;:x]\r\n\t[AKTION=n7;a;:x]\r\n\t[AKTION=n6:x]\r\n",
        ))
        .unwrap();
        let b = &file.blocks[0];
        assert_eq!(b.actions[0].params, ["fehlerhafte_parameterliste"]);
        assert_eq!(b.conditions[0].params, ["fehlerhafte_parameterliste"]);
        // A trailing `;` after a parameter yields no further one.
        assert_eq!(b.actions[1].params, ["a"]);
        assert!(b.actions[2].params.is_empty());
    }

    #[test]
    fn block_name_is_mid15_of_the_whole_line() {
        let text = "{\r\n\t[BESCHREIBUNG=Drake Final]\r\n{\r\n[BESCHREIBUNG=x]\r\n{\r\n[BESCHREIBUNG\r\n{\r\n  [BESCHREIBUNG= ä ]\r\n";
        let names: Vec<_> = rt(text)
            .unwrap()
            .blocks
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, ["Drake Final]", "]", "", "= ä ]"]);
    }

    #[test]
    fn lone_cr_is_dropped_and_joins_the_lines_like_read_line() {
        // Only '\n' ends a line, every '\r' is dropped (GameFile_ReadLine).
        let text = "{\n\t[BESCHREIBUNG=a]\r\n[AKT\rION=n1:x]\n[AKTION=n2:x]\r\r\n\r\r";
        let file = rt(text).unwrap();
        let b = &file.blocks[0];
        assert_eq!(b.line, 2);
        let actions: Vec<_> = b.actions.iter().map(|a| (a.number, a.line)).collect();
        assert_eq!(actions, [(1, 3), (2, 4)]);
        // Error line according to the original's counting (the Cr line does not count extra).
        let err = rt("{\n\t[BESCHREIBUNG=a]\r\n[AKT\rION=n1 x]\n").unwrap_err();
        assert_eq!(err.line, 3);
        // A `§` behind the separation is found.
        let file = rt("{\n[BESCHREIBUNG=a]\n[AKTION=n1:\r§\n[AKTION=n2:x]").unwrap();
        assert_eq!(file.blocks[0].actions.len(), 1);
        // The editor model keeps its physical lines.
        assert_eq!(parse(b"a\rb\n").lines.len(), 2);
    }

    #[test]
    fn only_ascii_white_space_is_trimmed_like_c_isspace() {
        assert_eq!(normalize_param("\u{a0}Player 6"), "\u{a0}player 6");
        assert_eq!(normalize_param("\t\u{b}x\u{c}\r "), "x");
        let file = rt(&block(
            "b",
            "\t[AKTION=n5;\u{a0}x:y]\r\n\t[AKTION=n8;\u{a0}:y]\r\n\t[AKTION=n\u{a0}7;a:y]\r\n",
        ))
        .unwrap();
        let a = &file.blocks[0].actions;
        assert_eq!(a[0].params, ["\u{a0}x"]);
        // Only NBSP after `;` is not an empty remainder.
        assert_eq!(a[1].params, ["\u{a0}"]);
        // `atol` does not skip NBSP: the number is 0.
        assert_eq!(a[2].number, 0);
    }

    #[test]
    fn mid5_cuts_five_characters() {
        assert_eq!(mid5("area gate"), "gate");
        assert_eq!(mid5("area "), "");
        assert_eq!(mid5("abc"), "");
        assert_eq!(mid5("äöüßx!"), "!");
    }

    #[test]
    fn missing_colon_aborts_the_whole_file() {
        let text = format!(
            "{}{}",
            block("a", "\t[AKTION=n1:x]\r\n"),
            block(
                "b",
                "\t[AKTION=n2:x]\r\n\t[AKTION=n3;y]\r\n\t[AKTION=n4:x]\r\n"
            ),
        );
        let err = rt(&text).unwrap_err();
        assert_eq!(err.line, 8);
        assert_eq!(err.to_string(), format!("line 8: {}", err.message));
    }

    #[test]
    fn empty_head_aborts_the_whole_file() {
        let err = rt(&block("a", "\t[AKTION=:x]\r\n")).unwrap_err();
        assert_eq!(err.line, 3);
    }

    #[test]
    fn deactivated_block_and_lines_without_block_are_ignored() {
        let text = "[AKTION=n1:before the first block]\r\n{\r\n\t[DEACTIVATE]\r\n\t[BESCHREIBUNG=off]\r\n\t[BEDINGUNG=&1:x]\r\n\t[AKTION=n2:x]\r\n}\r\n[AKTION=n3:after the block]\r\n";
        let file = rt(text).unwrap();
        assert!(file.blocks.is_empty());
        // Without a block not even ':' is checked.
        assert!(
            rt("[AKTION=n1 without colon]\r\n")
                .unwrap()
                .blocks
                .is_empty()
        );
    }

    #[test]
    fn condition_prefix_and_or_wait_flag() {
        let file = rt(&block(
            "b",
            "\t[BEDINGUNG=&1;a:x]\r\n\t[BEDINGUNG=|2;a:x]\r\n\t[BEDINGUNG=x3;a:x]\r\n\t[AKTION=n1:x]\r\n\t[AKTION=w40;1000:x]\r\n\t[AKTION=&2:x]\r\n",
        ))
        .unwrap();
        let b = &file.blocks[0];
        let or: Vec<_> = b.conditions.iter().map(|c| c.or).collect();
        assert_eq!(or, [false, true, true]);
        let wait: Vec<_> = b.actions.iter().map(|a| a.wait).collect();
        assert_eq!(wait, [false, true, false]);
        assert_eq!(b.conditions[1].line, 4);
        assert_eq!(b.actions[1].line, 7);
    }

    #[test]
    fn number_is_truncated_to_i32_like_the_original() {
        let file = rt(&block("b", "\t[AKTION=n4294967297;a:x]\r\n")).unwrap();
        assert_eq!(file.blocks[0].actions[0].number, 1);
    }

    #[test]
    fn round_trip_is_byte_exact() {
        let cases: [&[u8]; 8] = [
            b"[INI]\r\n{\r\n\tPANELNAME=x\r\n}\r\n",
            b"a\nb\nc\n",
            b"a\rb\r",
            b"a\r\nb\nc\rd",
            b"last line without ending",
            b"\r\n\r\n\n\r\r\n",
            b"",
            b"\t[AKTION=n1:\xa7 \x81\x9d\x8d\x8f\x90 \xe4\xf6\xfc\x80]\r\n",
        ];
        for bytes in cases {
            let file = parse(bytes);
            assert_eq!(
                file.to_bytes(),
                bytes,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
        let all: Vec<u8> = (0..=255u8).filter(|b| *b != b'\r' && *b != b'\n').collect();
        assert_eq!(parse(&all).to_bytes(), all);
    }

    #[test]
    fn line_ends_are_remembered() {
        let file = parse(b"a\r\nb\nc\rd");
        let ends: Vec<_> = file.lines.iter().map(|l| l.end).collect();
        assert_eq!(
            ends,
            [LineEnd::CrLf, LineEnd::Lf, LineEnd::Cr, LineEnd::None]
        );
        let texts: Vec<_> = file.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["a", "b", "c", "d"]);
    }

    #[test]
    fn lines_are_classified_for_the_editor() {
        let text = "[INI]\r\n{\r\n\tPANELNAME=x=y\r\n}\r\n\r\n  \r\n[TITLE=Victory conditions]\r\n\t[DEACTIVATE]\r\n\t[BESCHREIBUNG=Drake]\r\n\t[BEDINGUNG=&5;50:(and) If]\r\n\t[AKTION=w40;1000:Wait §]\r\n\t[AKTION=n1 broken]\r\nrest\r\n";
        let kinds: Vec<_> = file(text).lines.into_iter().map(|l| l.kind).collect();
        assert_eq!(kinds[0], LineKind::Section("INI".into()));
        assert_eq!(kinds[1], LineKind::Open);
        assert_eq!(
            kinds[2],
            LineKind::KeyValue {
                key: "PANELNAME".into(),
                value: "x=y".into()
            }
        );
        assert_eq!(kinds[3], LineKind::Close);
        assert_eq!(kinds[4], LineKind::Blank);
        assert_eq!(kinds[5], LineKind::Blank);
        assert_eq!(
            kinds[6],
            LineKind::Tagged {
                tag: "TITLE".into(),
                value: "Victory conditions".into()
            }
        );
        assert_eq!(kinds[7], LineKind::Deactivate);
        assert_eq!(
            kinds[8],
            LineKind::Tagged {
                tag: "BESCHREIBUNG".into(),
                value: "Drake".into()
            }
        );
        assert_eq!(
            kinds[9],
            LineKind::Condition(RawEntry {
                head: Some("&5;50".into()),
                comment: Some("(and) If".into()),
                disabled: false
            })
        );
        assert_eq!(
            kinds[10],
            LineKind::Action(RawEntry {
                head: Some("w40;1000".into()),
                comment: Some("Wait §".into()),
                disabled: true
            })
        );
        assert_eq!(
            kinds[11],
            LineKind::Action(RawEntry {
                head: None,
                comment: None,
                disabled: false
            })
        );
        assert_eq!(kinds[12], LineKind::Other);
    }
}
