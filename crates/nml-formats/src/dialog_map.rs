//! `dialogs.map` (reader `FUN_004DE8E0`, `docs/formats/dlg.md`, section "`dialogs.map`").
//!
//! Word pairs `KEY path` via the same tokenizer as `.dlg` ([`crate::dlg::lex_words`]), all words
//! uppercased, lines and tabs do not matter; a leftover last word is discarded.

use std::collections::BTreeMap;

use crate::dlg::{Word, lex_words, upper_word};

/// Lossless word list of the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogMap {
    pub words: Vec<Word>,
    /// Whitespace after the last word.
    pub trailing: String,
}

/// Reads the words of the file.
pub fn parse_dialog_map(bytes: &[u8]) -> DialogMap {
    let (words, trailing) = lex_words(bytes);
    DialogMap { words, trailing }
}

/// Writes `leading + text` of all words, then `trailing`; byte-identical to the input for unchanged maps.
/// Characters above U+00FF (only in hand-built maps) become `?`.
pub fn write_dialog_map(map: &DialogMap) -> Vec<u8> {
    let mut out = Vec::new();
    let mut push = |s: &str| out.extend(s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')));
    for word in &map.words {
        push(&word.leading);
        push(&word.text);
    }
    push(&map.trailing);
    out
}

impl DialogMap {
    /// Pairs (key, path), both uppercased as in the original (`CMapStringToString`, `FUN_004DE8E0`).
    /// Duplicate keys: the last path applies, the order is that of the first occurrence; a
    /// leftover last word without a partner is dropped.
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        for pair in self.words.chunks_exact(2) {
            let key = upper_word(&pair[0].text);
            let path = upper_word(&pair[1].text);
            match index.get(&key) {
                Some(&i) => entries[i].1 = path,
                None => {
                    index.insert(key.clone(), entries.len());
                    entries.push((key, path));
                }
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_map_last_key_wins_and_drops_an_odd_word() {
        let bytes = b"start data\\men\xfc\\start.dlg\r\n  Options\to.dlg\nstart\ts2.dlg  \r\nrest";
        let map = parse_dialog_map(bytes);
        assert_eq!(
            map.entries(),
            [
                ("START".to_string(), "S2.DLG".to_string()),
                ("OPTIONS".to_string(), "O.DLG".to_string()),
            ]
        );
        assert_eq!(write_dialog_map(&map), bytes);
    }

    #[test]
    fn upper_case_follows_the_dlg_reader() {
        let map = parse_dialog_map(b"abc data\\men\xfc\\\xe4.dlg");
        assert_eq!(
            map.entries(),
            [("ABC".to_string(), "DATA\\MEN\u{dc}\\\u{c4}.DLG".to_string())]
        );
        assert!(parse_dialog_map(b"").entries().is_empty());
        assert!(parse_dialog_map(b"only").entries().is_empty());
    }
}
