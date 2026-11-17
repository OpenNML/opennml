//! Menu dialogs `.dlg`: lossless syntax tree (words with whitespace, macros unevaluated).
//!
//! Based on `docs/formats/dlg.md`. The tree reproduces the original's structure - child area
//! `FUN_004E1E50`, element parser `FUN_004DD9C0` (`FRAME`) and its siblings, tokenizer `FUN_004DD400` -
//! but evaluates nothing: loops, variables, numbers and flags stay words. The evaluation according to the
//! original's reading model ([`expand()`]) works on [`DlgDocument::words`] and returns a tree of
//! [`WidgetDesc`] with the descriptors from [`desc`].
//!
//! Writing ([`write_dlg`]) returns the input bytes exactly: every word carries the whitespace in front of it,
//! the rest after the last word is in [`DlgDocument::trailing`]. All bytes are read as Latin-1
//! (one `char` U+0000...U+00FF per byte) so that `0xFC` (`ü`) in paths survives the round trip.
//!
//! Deviation from the original (quirk O-3): Where the original reads forever on a missing end word or a foreign
//! `/...` word (`docs/formats/dlg.md`, section "Error behavior"), the element here ends with `end: None` and a
//! [`Diagnostic`]. Nothing is lost: every word of the file is in the tree.

pub mod desc;
pub mod expand;

pub use desc::{
    ButtonDesc, EditDesc, FrameDesc, LabelDesc, ListDesc, RadioDesc, SliderDesc, WidgetDesc,
    WidgetKind,
};
pub use expand::{AppCommand, ExpandOptions, Expanded, expand};

/// Deepest nesting (elements and `LOOP`) that the reader builds; keywords nested deeper remain [`Item::Stray`].
/// The shipped data does not go beyond four levels; the limit protects against stack overflow with foreign
/// files.
const MAX_DEPTH: usize = 64;

/// A word of the file with the whitespace in front of it (original spelling, Latin-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// Whitespace between the previous word (or the start of the file) and this word: space, tab, `\r`, `\n`.
    pub leading: String,
    /// The word itself, not uppercased (Latin-1: one `char` per byte).
    pub text: String,
    /// Line from 1 in which the word begins (`\n` are counted).
    pub line: u32,
}

/// The eight element kinds (`docs/formats/dlg.md`, section "Element kinds").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Frame,
    Button,
    ComboBox,
    ListBox,
    EditField,
    Slider,
    RadioButton,
    Label,
}

impl ElementKind {
    /// Element kind for an **uppercase** keyword (child area `FUN_004E1E50`,
    /// `0x004E2181`...`0x004E22AE`).
    pub fn from_keyword(upper: &str) -> Option<Self> {
        Some(match upper {
            "FRAME" => Self::Frame,
            "BUTTON" => Self::Button,
            "COMBOBOX" => Self::ComboBox,
            "LISTBOX" => Self::ListBox,
            "EDITFIELD" => Self::EditField,
            "SLIDER" => Self::Slider,
            "RADIOBUTTON" => Self::RadioButton,
            "LABEL" => Self::Label,
            _ => return None,
        })
    }

    /// End word of the kind (`/FRAME`, `/BUTTON` ...).
    pub fn end_keyword(self) -> &'static str {
        match self {
            Self::Frame => "/FRAME",
            Self::Button => "/BUTTON",
            Self::ComboBox => "/COMBOBOX",
            Self::ListBox => "/LISTBOX",
            Self::EditField => "/EDITFIELD",
            Self::Slider => "/SLIDER",
            Self::RadioButton => "/RADIOBUTTON",
            Self::Label => "/LABEL",
        }
    }
}

/// An entry of a child area (`FUN_004E1E50`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Element(Element),
    /// `VAR name expression`
    Var {
        keyword: Word,
        name: Word,
        value: Word,
    },
    /// `ADD name expression`
    Add {
        keyword: Word,
        name: Word,
        value: Word,
    },
    /// `LOOP n ... /LOOP`; `end` is missing at end of file or on a foreign `/...` word.
    Loop {
        keyword: Word,
        count: Word,
        body: Vec<Item>,
        end: Option<Word>,
    },
    /// Word that no original parser accepts at this point (unknown word, `/...` at top level, rest of a truncated
    /// `VAR`/`ADD`/`LOOP`); the original aborts the file here.
    Stray(Word),
}

/// An element (`FRAME`, `BUTTON` ...) with head and children in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub kind: ElementKind,
    pub keyword: Word,
    /// Missing only if the file ends right after the keyword.
    pub name: Option<Word>,
    pub parts: Vec<ElementPart>,
    /// End word; `None` at end of file or on a foreign `/...` word (O-3).
    pub end: Option<Word>,
}

/// Component of an element in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementPart {
    /// `FLAGS word... /FLAGS`; `close` is missing if `/FLAGS` is missing.
    Flags {
        open: Word,
        words: Vec<Word>,
        close: Option<Word>,
    },
    /// Head key with its arguments ([`prop_arity`]). A `Prop` after the first `Child` stays, but gets a
    /// diagnostic: the original reads it there as an unknown word and aborts the file.
    Prop {
        key: Word,
        args: Vec<Word>,
    },
    Child(Item),
}

/// The whole tree of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlgDocument {
    pub items: Vec<Item>,
    /// Whitespace after the last word.
    pub trailing: String,
}

/// Diagnostic while reading (English message, line from 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: u32,
    pub message: String,
}

impl DlgDocument {
    /// All words in file order.
    pub fn words(&self) -> Vec<&Word> {
        let mut out = Vec::new();
        for item in &self.items {
            collect_item(item, &mut out);
        }
        out
    }
}

fn collect_item<'a>(item: &'a Item, out: &mut Vec<&'a Word>) {
    match item {
        Item::Element(e) => {
            out.push(&e.keyword);
            out.extend(e.name.iter());
            for part in &e.parts {
                match part {
                    ElementPart::Flags { open, words, close } => {
                        out.push(open);
                        out.extend(words.iter());
                        out.extend(close.iter());
                    }
                    ElementPart::Prop { key, args } => {
                        out.push(key);
                        out.extend(args.iter());
                    }
                    ElementPart::Child(child) => collect_item(child, out),
                }
            }
            out.extend(e.end.iter());
        }
        Item::Var {
            keyword,
            name,
            value,
        }
        | Item::Add {
            keyword,
            name,
            value,
        } => out.extend([keyword, name, value]),
        Item::Loop {
            keyword,
            count,
            body,
            end,
        } => {
            out.extend([keyword, count]);
            for child in body {
                collect_item(child, out);
            }
            out.extend(end.iter());
        }
        Item::Stray(word) => out.push(word),
    }
}

/// Uppercasing like `FUN_00574EA0` (`0x004DD52F`): `a`-`z` and `ä ö ü` become `A`-`Z` and `Ä Ö Ü`, everything
/// else stays (also `ß`). All comparisons of the original run on these words (`docs/formats/dlg.md`,
/// "Lexical rules").
pub fn upper_word(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'a'..='z' => c.to_ascii_uppercase(),
            'ä' => 'Ä',
            'ö' => 'Ö',
            'ü' => 'Ü',
            other => other,
        })
        .collect()
}

/// Number of arguments of a head key (`docs/formats/dlg.md`, section "Element kinds", tables per kind); `key_upper` is
/// uppercase. `None`: not a head key of this kind (`FLAGS` does not count).
pub fn prop_arity(kind: ElementKind, key_upper: &str) -> Option<usize> {
    use ElementKind::*;
    let one = Some(1);
    let four = Some(4);
    match (kind, key_upper) {
        // FRAME, FUN_004DD9C0
        (Frame, "EXTENT" | "BAREXTENT" | "TITLEEXTENT") => four,
        (Frame, "ZIELPOSITION") => Some(2),
        (
            Frame,
            "DURATION" | "TITLECOLOR" | "TITLE" | "BACKGROUND" | "LEVELBACKGROUND" | "MUSIC"
            | "BKGFILE" | "PRELOAD",
        ) => one,
        // BUTTON, FUN_004E0300
        (Button, "EXTENT" | "BORDERSPACE") => four,
        (
            Button,
            "TEXTCOLOR"
            | "DISABLEDTEXTCOLOR"
            | "MOUSEOVERTEXTCOLOR"
            | "PRESSEDBUTTONTEXTCOLOR"
            | "TEXT",
        ) => one,
        // COMBOBOX, FUN_004DEDE0
        (ComboBox, "BUTTONEXTENT" | "LISTBOXEXTENT") => four,
        (
            ComboBox,
            "TEXTCOLOR"
            | "DISABLEDTEXTCOLOR"
            | "SELECTIONBARCOLOR"
            | "MOUSEOVERCOLOR"
            | "MOUSEOVERTEXTCOLOR"
            | "PRESSEDBUTTONTEXTCOLOR"
            | "SELECTEDTEXTCOLOR"
            | "TEXT"
            | "CURSELECTION",
        ) => one,
        // LISTBOX, FUN_004DF790
        (ListBox, "EXTENT") => four,
        (
            ListBox,
            "TEXTCOLOR" | "DISABLEDTEXTCOLOR" | "SELECTIONBARCOLOR" | "MOUSEOVERCOLOR"
            | "SELECTEDTEXTCOLOR" | "CURSELECTION",
        ) => one,
        // EDITFIELD, FUN_004DFD90
        (EditField, "EXTENT") => four,
        (EditField, "TEXTCOLOR" | "DISABLEDTEXTCOLOR" | "TEXT") => one,
        // SLIDER, FUN_004E0B20
        (Slider, "EXTENT") => four,
        (
            Slider,
            "CURVALUE" | "RESOLUTION" | "TEXTCOLOR" | "DISABLEDTEXTCOLOR" | "THUMBCOLOR"
            | "SCALECOLOR" | "TEXT",
        ) => one,
        // RADIOBUTTON, FUN_004E11E0
        (RadioButton, "EXTENT") => four,
        (RadioButton, "ICONOFFSET" | "TEXT" | "TEXTCOLOR" | "DISABLEDTEXTCOLOR") => one,
        // LABEL, FUN_004E17B0
        (Label, "EXTENT") => four,
        (Label, "CREDITS" | "TEXTCOLOR" | "BACKGROUNDCOLOR" | "DISABLEDTEXTCOLOR" | "TEXT") => one,
        _ => None,
    }
}

fn is_separator(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// Splits bytes into words (tokenizer `FUN_004DD400`, `docs/formats/dlg.md`, section "Lexical rules"): a word
/// ends at space, tab, `\r` or `\n`; line ends are whitespace. Returns the words and the whitespace after the
/// last word. Also for `dialogs.map` (same tokenizer, `FUN_004DE8E0`).
pub fn lex_words(bytes: &[u8]) -> (Vec<Word>, String) {
    let mut words = Vec::new();
    let mut line = 1u32;
    let mut pos = 0;
    while pos < bytes.len() {
        let lead_start = pos;
        while pos < bytes.len() && is_separator(bytes[pos]) {
            if bytes[pos] == b'\n' {
                line = line.saturating_add(1);
            }
            pos += 1;
        }
        let leading: String = bytes[lead_start..pos].iter().map(|&b| b as char).collect();
        if pos >= bytes.len() {
            return (words, leading);
        }
        let text_start = pos;
        while pos < bytes.len() && !is_separator(bytes[pos]) {
            pos += 1;
        }
        let text: String = bytes[text_start..pos].iter().map(|&b| b as char).collect();
        words.push(Word {
            leading,
            text,
            line,
        });
    }
    (words, String::new())
}

/// Reads a `.dlg`. Always returns a tree that contains all words; anomalies are in the diagnostics (line from 1).
/// Terminates in every case, also for faulty files (O-3).
pub fn parse_dlg(bytes: &[u8]) -> (DlgDocument, Vec<Diagnostic>) {
    let (words, trailing) = lex_words(bytes);
    let mut parser = Parser {
        words: words.into_iter().peekable(),
        last_line: 1,
        diags: Vec::new(),
    };
    let items = parser.parse_top();
    (DlgDocument { items, trailing }, parser.diags)
}

/// Writes the tree back: `leading + text` of all words in file order, then `trailing`. Byte-identical to the input
/// for unchanged trees. Characters above U+00FF (only in hand-built trees) become `?`.
pub fn write_dlg(doc: &DlgDocument) -> Vec<u8> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        out.extend(s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')));
    };
    for word in doc.words() {
        push(&word.leading);
        push(&word.text);
    }
    push(&doc.trailing);
    out
}

struct Parser {
    words: std::iter::Peekable<std::vec::IntoIter<Word>>,
    /// Line of the word read last (for messages at end of file).
    last_line: u32,
    diags: Vec<Diagnostic>,
}

impl Parser {
    fn next_word(&mut self) -> Option<Word> {
        let word = self.words.next()?;
        self.last_line = word.line;
        Some(word)
    }

    fn peek_upper(&mut self) -> Option<String> {
        self.words.peek().map(|w| upper_word(&w.text))
    }

    /// Line and text of the next word (for messages).
    fn peek_info(&mut self) -> (u32, String) {
        match self.words.peek() {
            Some(w) => (w.line, w.text.clone()),
            None => (self.last_line, String::new()),
        }
    }

    fn report(&mut self, line: u32, message: String) {
        self.diags.push(Diagnostic { line, message });
    }

    /// Top level: like a child area, but a `/...` word ends reading of the file in the original
    /// (`docs/formats/dlg.md`, section "Error behavior"); here it stays as `Stray` and reading continues.
    fn parse_top(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while self.words.peek().is_some() {
            if !self.parse_item(0, &mut items)
                && let Some(word) = self.next_word()
            {
                self.report(
                    word.line,
                    format!(
                        "\"{}\" is outside an element; the original ends reading of the file here",
                        word.text
                    ),
                );
                items.push(Item::Stray(word));
            }
        }
        items
    }

    /// An entry of the child area (`FUN_004E1E50`), appended to `out`. `false` (without reading anything) at end of
    /// file and on a word starting with `/`: the caller decides both. Every other unknown word becomes `Stray`. A
    /// `VAR`/`ADD`/`LOOP` truncated at end of file becomes individual `Stray` entries (the tree has no
    /// placeholders).
    fn parse_item(&mut self, depth: usize, out: &mut Vec<Item>) -> bool {
        let Some(upper) = self.peek_upper() else {
            return false;
        };
        if upper.starts_with('/') {
            return false;
        }
        let Some(keyword) = self.next_word() else {
            return false;
        };
        let element_kind = ElementKind::from_keyword(&upper);
        if depth >= MAX_DEPTH && (element_kind.is_some() || upper == "LOOP") {
            self.report(
                keyword.line,
                format!(
                    "Nesting deeper than {MAX_DEPTH} levels: \"{}\" is not read any more",
                    keyword.text
                ),
            );
            out.push(Item::Stray(keyword));
            return true;
        }
        if let Some(kind) = element_kind {
            out.push(Item::Element(self.parse_element(kind, keyword, depth)));
            return true;
        }
        match upper.as_str() {
            "VAR" | "ADD" => {
                let Some(name) = self.next_word() else {
                    self.report_truncated(&keyword);
                    out.push(Item::Stray(keyword));
                    return true;
                };
                let Some(value) = self.next_word() else {
                    self.report_truncated(&keyword);
                    out.push(Item::Stray(keyword));
                    out.push(Item::Stray(name));
                    return true;
                };
                out.push(if upper == "VAR" {
                    Item::Var {
                        keyword,
                        name,
                        value,
                    }
                } else {
                    Item::Add {
                        keyword,
                        name,
                        value,
                    }
                });
            }
            "LOOP" => {
                let Some(count) = self.next_word() else {
                    self.report_truncated(&keyword);
                    out.push(Item::Stray(keyword));
                    return true;
                };
                out.push(self.parse_loop(keyword, count, depth));
            }
            _ => {
                self.report(
                    keyword.line,
                    format!(
                        "Unknown word \"{}\" in the child area; the original aborts the file here",
                        keyword.text
                    ),
                );
                out.push(Item::Stray(keyword));
            }
        }
        true
    }

    fn report_truncated(&mut self, keyword: &Word) {
        self.report(
            keyword.line,
            format!("\"{}\" is truncated at end of file", keyword.text),
        );
    }

    /// `LOOP n ... /LOOP`. Another `/...` word or end of file ends the loop without `/LOOP`.
    fn parse_loop(&mut self, keyword: Word, count: Word, depth: usize) -> Item {
        let mut body = Vec::new();
        while self.parse_item(depth + 1, &mut body) {}
        let end = if self.peek_upper().as_deref() == Some("/LOOP") {
            self.next_word()
        } else {
            let (line, found) = self.peek_info();
            let message = if found.is_empty() {
                format!(
                    "LOOP (line {}) without /LOOP until end of file",
                    keyword.line
                )
            } else {
                format!(
                    "LOOP (line {}) without /LOOP: \"{found}\" instead; the original reads forever here",
                    keyword.line
                )
            };
            self.report(line, message);
            None
        };
        Item::Loop {
            keyword,
            count,
            body,
            end,
        }
    }

    /// An element (`FUN_004DD9C0` and siblings, `docs/formats/dlg.md`, section "Elements"). Head keys, `FLAGS`
    /// and children are in `parts` in file order; the first foreign word starts the children. A foreign `/...`
    /// word or end of file ends the element without an end word (deviation, O-3).
    fn parse_element(&mut self, kind: ElementKind, keyword: Word, depth: usize) -> Element {
        let name = self.next_word();
        let mut parts = Vec::new();
        let mut seen_child = false;
        let mut end = None;
        let open_line = keyword.line;
        let end_keyword = kind.end_keyword();
        let kind_word = upper_word(&keyword.text);
        loop {
            let Some(upper) = self.peek_upper() else {
                self.report(
                    self.last_line,
                    format!(
                        "{kind_word} (line {open_line}) without {end_keyword} until end of file"
                    ),
                );
                break;
            };
            if upper == end_keyword {
                end = self.next_word();
                break;
            }
            if upper.starts_with('/') {
                let (line, found) = self.peek_info();
                self.report(
                    line,
                    format!(
                        "{kind_word} (line {open_line}) without {end_keyword}: \"{found}\" instead; \
                         the original reads forever here"
                    ),
                );
                break;
            }
            if upper == "FLAGS" {
                if let Some(open) = self.next_word() {
                    if seen_child {
                        self.report_late_head(&open);
                    }
                    let flags = self.parse_flags(open);
                    parts.push(flags);
                }
                continue;
            }
            if let Some(arity) = prop_arity(kind, &upper) {
                let Some(key) = self.next_word() else {
                    break;
                };
                if seen_child {
                    self.report_late_head(&key);
                }
                let mut args = Vec::new();
                while args.len() < arity {
                    let Some(arg) = self.next_word() else {
                        self.report(
                            key.line,
                            format!(
                                "{upper} has {} instead of {arity} arguments (end of file)",
                                args.len()
                            ),
                        );
                        break;
                    };
                    args.push(arg);
                }
                parts.push(ElementPart::Prop { key, args });
                continue;
            }
            let mut children = Vec::new();
            if self.parse_item(depth + 1, &mut children) {
                seen_child = true;
                parts.extend(children.into_iter().map(ElementPart::Child));
            }
        }
        Element {
            kind,
            keyword,
            name,
            parts,
            end,
        }
    }

    fn report_late_head(&mut self, word: &Word) {
        self.report(
            word.line,
            format!(
                "\"{}\" appears after the first child; the original reads it there as an unknown word \
                 and aborts the file",
                word.text
            ),
        );
    }

    /// `FLAGS word... /FLAGS`. Another `/...` word or end of file ends the flags without `/FLAGS`.
    fn parse_flags(&mut self, open: Word) -> ElementPart {
        let mut words = Vec::new();
        let mut close = None;
        loop {
            match self.peek_upper() {
                None => {
                    self.report(
                        self.last_line,
                        format!(
                            "FLAGS (line {}) without /FLAGS until end of file",
                            open.line
                        ),
                    );
                    break;
                }
                Some(upper) if upper == "/FLAGS" => {
                    close = self.next_word();
                    break;
                }
                Some(upper) if upper.starts_with('/') => {
                    let (line, found) = self.peek_info();
                    self.report(
                        line,
                        format!(
                            "FLAGS (line {}) without /FLAGS: \"{found}\" instead",
                            open.line
                        ),
                    );
                    break;
                }
                Some(_) => words.extend(self.next_word()),
            }
        }
        ElementPart::Flags { open, words, close }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"FRAME A\r\n\tFLAGS X\r\n Y /FLAGS\r\n/FRAME";

    fn texts(doc: &DlgDocument) -> Vec<&str> {
        doc.words().iter().map(|w| w.text.as_str()).collect()
    }

    fn only_element(doc: &DlgDocument) -> &Element {
        match doc.items.as_slice() {
            [Item::Element(e)] => e,
            other => panic!("expected exactly one element, found {other:?}"),
        }
    }

    #[test]
    fn words_split_on_space_and_tab_and_ignore_line_ends() {
        let (doc, diags) = parse_dlg(SAMPLE);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            texts(&doc),
            ["FRAME", "A", "FLAGS", "X", "Y", "/FLAGS", "/FRAME"]
        );
        let lines: Vec<u32> = doc.words().iter().map(|w| w.line).collect();
        assert_eq!(lines, [1, 1, 2, 2, 3, 3, 4]);
    }

    #[test]
    fn lex_words_returns_leading_space_and_trailing() {
        let (words, trailing) = lex_words(b"  a\tb\r\n\r\nc \r\n");
        let got: Vec<(&str, &str, u32)> = words
            .iter()
            .map(|w| (w.leading.as_str(), w.text.as_str(), w.line))
            .collect();
        assert_eq!(got, [("  ", "a", 1), ("\t", "b", 1), ("\r\n\r\n", "c", 3)]);
        assert_eq!(trailing, " \r\n");
        let (none, only_space) = lex_words(b" \r\n");
        assert!(none.is_empty());
        assert_eq!(only_space, " \r\n");
    }

    #[test]
    fn round_trip_is_byte_identical() {
        let mut input = Vec::new();
        input.extend_from_slice(b"FRAME A\r\n\tFLAGS X\r\n Y /FLAGS\r\n\r\n\r\n");
        input.extend_from_slice(b"\tBACKGROUND data/men\xFC/stra\xE4e.tga\r\n");
        input.extend_from_slice(b"\t\tBUTTON  B\t/BUTTON\r\n/FRAME");
        let (doc, diags) = parse_dlg(&input);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(write_dlg(&doc), input);
        // Also with a line end at the end and with only whitespace.
        for extra in [&b"\r\n"[..], b" \t\r\n\r\n"] {
            let mut longer = input.clone();
            longer.extend_from_slice(extra);
            assert_eq!(write_dlg(&parse_dlg(&longer).0), longer);
        }
        assert_eq!(write_dlg(&parse_dlg(b"  \r\n").0), b"  \r\n");
    }

    // QUIRK O-5: kept
    #[test]
    fn elements_nest_and_keep_order() {
        let input = b"FRAME F\r\nFLAGS A B /FLAGS\r\nEXTENT 1 2 3 4\r\nVAR YPOS 10\r\n\
                      BUTTON B EXTENT 0 0 1 1 /BUTTON\r\nTITLE 5\r\n/FRAME\r\n";
        let (doc, diags) = parse_dlg(input);
        let frame = only_element(&doc);
        assert_eq!(frame.kind, ElementKind::Frame);
        assert_eq!(frame.name.as_ref().unwrap().text, "F");
        assert!(frame.end.is_some());
        let shape: Vec<&str> = frame
            .parts
            .iter()
            .map(|p| match p {
                ElementPart::Flags { .. } => "Flags",
                ElementPart::Prop { .. } => "Prop",
                ElementPart::Child(Item::Var { .. }) => "Var",
                ElementPart::Child(Item::Element(_)) => "Element",
                ElementPart::Child(_) => "other child",
            })
            .collect();
        assert_eq!(shape, ["Flags", "Prop", "Var", "Element", "Prop"]);
        let ElementPart::Prop { key, args } = &frame.parts[1] else {
            unreachable!()
        };
        assert_eq!((key.text.as_str(), args.len()), ("EXTENT", 4));
        // The TITLE after the child stays in the tree but produces exactly one diagnostic.
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].line, 6);
        assert_eq!(write_dlg(&doc), input);
    }

    #[test]
    fn loop_body_is_a_subtree() {
        let (doc, diags) = parse_dlg(b"LOOP 3 LABEL L /LABEL /LOOP");
        assert!(diags.is_empty(), "{diags:?}");
        let [
            Item::Loop {
                count, body, end, ..
            },
        ] = doc.items.as_slice()
        else {
            panic!("expected a loop: {:?}", doc.items);
        };
        assert_eq!(count.text, "3");
        assert!(matches!(body.as_slice(), [Item::Element(e)] if e.kind == ElementKind::Label));
        assert!(end.is_some());
    }

    // QUIRK O-3: deviation
    #[test]
    fn malformed_dlg_never_hangs() {
        let cases: [(&str, &[u8]); 8] = [
            ("missing /BUTTON", b"FRAME F\r\nBUTTON B\r\n/FRAME\r\n"),
            (
                "foreign /LABEL in BUTTON",
                b"FRAME F\r\nBUTTON B\r\n/LABEL\r\n/BUTTON\r\n/FRAME\r\n",
            ),
            ("/FLAGS without FLAGS", b"FRAME F\r\n/FLAGS\r\n/FRAME\r\n"),
            (
                "LOOP without /LOOP",
                b"FRAME F\r\nLOOP 2\r\nLABEL L /LABEL\r\n/FRAME\r\n",
            ),
            (
                "unknown word in the child area",
                b"FRAME F\r\nBUTTON B /BUTTON\r\nFOO\r\n/FRAME\r\n",
            ),
            (
                "FLAGS without /FLAGS",
                b"FRAME F\r\nFLAGS A B\r\n/FRAME\r\n",
            ),
            ("truncated VAR", b"VAR X"),
            ("arguments at end of file", b"FRAME F EXTENT 1 2"),
        ];
        for (name, input) in cases {
            let (doc, diags) = parse_dlg(input);
            assert!(!diags.is_empty(), "{name}: no diagnostic");
            assert!(
                diags.iter().all(|d| d.line >= 1 && !d.message.is_empty()),
                "{name}: {diags:?}"
            );
            assert_eq!(write_dlg(&doc), input, "{name}: not byte-identical");
        }
        // Empty file: no diagnostic, empty tree.
        let (doc, diags) = parse_dlg(b"");
        assert!(diags.is_empty() && doc.items.is_empty());
        assert_eq!(write_dlg(&doc), b"");
        // The lines of the diagnostics match the location of the problem.
        let (_, diags) = parse_dlg(b"FRAME F\r\nBUTTON B\r\n/LABEL\r\n/BUTTON\r\n/FRAME\r\n");
        assert_eq!(diags[0].line, 3, "{diags:?}");
    }

    // QUIRK O-3: deviation
    #[test]
    fn deep_nesting_is_bounded() {
        let input = "LOOP 1 ".repeat(100_000);
        let (doc, diags) = parse_dlg(input.as_bytes());
        assert!(!diags.is_empty());
        assert_eq!(write_dlg(&doc), input.as_bytes());
        let frames = "FRAME F ".repeat(100_000);
        let (doc, _) = parse_dlg(frames.as_bytes());
        assert_eq!(write_dlg(&doc), frames.as_bytes());
    }

    // QUIRK O-1: kept
    #[test]
    fn upper_word_follows_the_original() {
        assert_eq!(upper_word("daten/menü/straße"), "DATEN/MENÜ/STRAßE");
        assert_eq!(upper_word("äöü ÄÖÜ"), "ÄÖÜ ÄÖÜ");
        assert_eq!(upper_word("0x00FF"), "0X00FF");
    }

    #[test]
    fn prop_arity_follows_the_tables() {
        use ElementKind::*;
        assert_eq!(prop_arity(Frame, "EXTENT"), Some(4));
        assert_eq!(prop_arity(Frame, "ZIELPOSITION"), Some(2));
        assert_eq!(prop_arity(Frame, "TITLE"), Some(1));
        assert_eq!(prop_arity(Button, "BORDERSPACE"), Some(4));
        assert_eq!(prop_arity(ComboBox, "LISTBOXEXTENT"), Some(4));
        assert_eq!(prop_arity(ComboBox, "EXTENT"), None);
        assert_eq!(prop_arity(Slider, "RESOLUTION"), Some(1));
        assert_eq!(prop_arity(Label, "CREDITS"), Some(1));
        assert_eq!(prop_arity(Button, "FLAGS"), None);
        assert_eq!(prop_arity(Button, "LOOP"), None);
    }

    #[test]
    fn element_keywords_round_trip() {
        for kw in [
            "FRAME",
            "BUTTON",
            "COMBOBOX",
            "LISTBOX",
            "EDITFIELD",
            "SLIDER",
            "RADIOBUTTON",
            "LABEL",
        ] {
            let kind = ElementKind::from_keyword(kw).unwrap();
            assert_eq!(kind.end_keyword(), format!("/{kw}"));
        }
        assert_eq!(ElementKind::from_keyword("VAR"), None);
        assert_eq!(ElementKind::from_keyword("frame"), None);
    }

    // QUIRK O-1: kept
    #[test]
    fn keywords_are_matched_case_insensitively() {
        let (doc, diags) = parse_dlg(b"frame f extent 1 2 3 4 /frame");
        assert!(diags.is_empty(), "{diags:?}");
        let frame = only_element(&doc);
        assert_eq!(frame.parts.len(), 1);
        assert_eq!(write_dlg(&doc), b"frame f extent 1 2 3 4 /frame");
    }
}
