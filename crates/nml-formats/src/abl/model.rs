//! Lossless data model of an `.abl` file: each line as text with its line ending and a classification
//! for the editor. The classification does not affect the run-time view (see `runtime`).

use crate::text::{decode_cp1252, encode_cp1252};

/// Line ending of a line; `None` only for the last line without a terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnd {
    CrLf,
    Lf,
    Cr,
    None,
}

impl LineEnd {
    fn bytes(self) -> &'static [u8] {
        match self {
            LineEnd::CrLf => b"\r\n",
            LineEnd::Lf => b"\n",
            LineEnd::Cr => b"\r",
            LineEnd::None => b"",
        }
    }
}

/// Condition or action line in raw form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    /// Head between the first `=` and the first `:` (prefix, number, parameter); `None` without `:`.
    pub head: Option<String>,
    /// Plain text after the first `:` without a trailing `]`; `None` without `:`.
    pub comment: Option<String>,
    /// The line contains `§` and is dropped when the original reads it (E-122).
    pub disabled: bool,
}

/// Classification of a line for the editor; the first matching rule applies: `{`, `}`, `[DEACTIVATE]`,
/// `[BEDINGUNG=...`, `[AKTION=...`, `[X=...]`, `[X]`, `K=V`, blank line, otherwise `Other`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Open,
    Close,
    Deactivate,
    /// `[INI]`, `[OBJECTS]`, `[TIMER]`, `[SWITCHES]` ...
    Section(String),
    /// `[TITLE=...]`, `[POSITION=...]`, `[BESCHREIBUNG=...]`, `[DIALOG=...]`
    Tagged {
        tag: String,
        value: String,
    },
    KeyValue {
        key: String,
        value: String,
    },
    Condition(RawEntry),
    Action(RawEntry),
    Other,
}

/// A line: text without line ending (cp1252 to Unicode), the line ending and the classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AblLine {
    pub text: String,
    pub end: LineEnd,
    pub kind: LineKind,
}

/// An `.abl` file as a sequence of lines; [`AblFile::to_bytes`] returns the input byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AblFile {
    pub lines: Vec<AblLine>,
}

/// Splits the bytes into lines (CRLF, LF, lone CR, last line without ending). Never fails.
pub fn parse(bytes: &[u8]) -> AblFile {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let (end, len) = match bytes[i] {
            b'\n' => (LineEnd::Lf, 1),
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => (LineEnd::CrLf, 2),
            b'\r' => (LineEnd::Cr, 1),
            _ => {
                i += 1;
                continue;
            }
        };
        lines.push(make_line(&bytes[start..i], end));
        i += len;
        start = i;
    }
    if start < bytes.len() {
        lines.push(make_line(&bytes[start..], LineEnd::None));
    }
    AblFile { lines }
}

fn make_line(bytes: &[u8], end: LineEnd) -> AblLine {
    let text = decode_cp1252(bytes);
    let kind = classify(&text);
    AblLine { text, end, kind }
}

impl AblFile {
    /// Writes the file back; byte-identical to the input of [`parse`].
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for line in &self.lines {
            out.extend_from_slice(&encode_cp1252(&line.text));
            out.extend_from_slice(line.end.bytes());
        }
        out
    }
}

fn classify(text: &str) -> LineKind {
    let t = text.trim();
    if t == "{" {
        return LineKind::Open;
    }
    if t == "}" {
        return LineKind::Close;
    }
    if t == "[DEACTIVATE]" {
        return LineKind::Deactivate;
    }
    if t.starts_with("[BEDINGUNG=") {
        return LineKind::Condition(raw_entry(text));
    }
    if t.starts_with("[AKTION=") {
        return LineKind::Action(raw_entry(text));
    }
    if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return match inner.split_once('=') {
            Some((tag, value)) => LineKind::Tagged {
                tag: tag.to_string(),
                value: value.to_string(),
            },
            None => LineKind::Section(inner.to_string()),
        };
    }
    if let Some((key, value)) = t.split_once('=') {
        return LineKind::KeyValue {
            key: key.trim_end().to_string(),
            value: value.to_string(),
        };
    }
    if t.is_empty() {
        return LineKind::Blank;
    }
    LineKind::Other
}

fn raw_entry(text: &str) -> RawEntry {
    let disabled = text.contains('§');
    let (head, comment) = match text.split_once(':') {
        Some((before, after)) => {
            let head = before.split_once('=').map_or(before, |(_, h)| h);
            let comment = after.trim_end();
            let comment = comment.strip_suffix(']').unwrap_or(comment);
            (Some(head.to_string()), Some(comment.to_string()))
        }
        None => (None, None),
    };
    RawEntry {
        head,
        comment,
        disabled,
    }
}
