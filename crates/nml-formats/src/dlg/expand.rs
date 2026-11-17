//! Evaluation of a `.dlg` file according to the original's reading model (`docs/formats/dlg.md`, section
//! "Structure").
//!
//! The evaluation does **not** read the tree from [`super::parse_dlg`] but its words in file order
//! ([`DlgDocument::words`]) with the original's state (`MenuSystem`): next line of the file and rest of the
//! current line (line buffer `+0x14`), name prefix `+0x1C`, variables `+0x90`, loop stack `+0xAC`.
//! Only this way do the loop quirks come out right: `LOOP n` remembers the file position after the line of `n`
//! (`FUN_00574A90`), `/LOOP` resets the file to that position (`FUN_00574A70`), the line buffer stays as it is
//! (quirk 9). Empty lines are skipped by the tokenizer `FUN_004DD400`, so they do not count.
//!
//! Deviation from the original (quirks O-3, O-2): Where the original reads or computes forever (missing end word,
//! foreign `/...` in the element, `+` at the start of a number part), an English [`Diagnostic`] results and the
//! file aborts as on an error; whatever was built up to then stays in [`Expanded::widgets`]. In addition
//! [`MAX_WORDS`] and [`MAX_DEPTH`] limit foreign files.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

use super::desc::{WidgetDesc, WidgetKind, flag_bit, frame_flags, label_flags};
use super::{Diagnostic, DlgDocument, ElementKind};

/// Maximum number of words read per file (loops included). The shipped data reads far fewer; the limit
/// catches huge `LOOP` counts of foreign files before memory fills up.
pub const MAX_WORDS: usize = 1_000_000;

/// Deepest nesting of elements (shipped data: two levels); protects against stack overflow.
pub const MAX_DEPTH: usize = 64;

/// Settings of the evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExpandOptions {
    /// `Engine.ini` `IsMenuLevel` (`g_pEngineConfig+0x06`): controls `LEVELBACKGROUND` and `BACKGROUND` (O-7).
    pub is_menu_level: bool,
}

/// Application command that the original sends immediately to `[MenuSystem+0x10]` when reading a `FRAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    /// Command `0x20`: 3D background off.
    HideLevelBackground,
    /// Command `0x21`: load `.lvl` as 3D background (path in uppercase, as read).
    LoadLevelBackground(String),
}

/// Result of the evaluation of a file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Expanded {
    pub widgets: Vec<WidgetDesc>,
    pub commands: Vec<AppCommand>,
    pub preload: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Evaluates a file like `FUN_004E23E0` (child area with the root widget as parent). Always terminates; on an
/// error (in the original: `false` without a message) the widgets already built stay, plus exactly one
/// diagnostic. The top-level widgets hang from the root in the original (a modal `FRAME` from a blocker, which
/// `nml-ui` reproduces).
pub fn expand(doc: &DlgDocument, opts: ExpandOptions) -> Expanded {
    let mut ex = Expander::new(doc, opts);
    let mut widgets = Vec::new();
    if ex.children(&mut widgets, 0).is_ok() {
        // The child area returns only at end of file or on a pushed-back `/...` word; this word ends reading of
        // the file (`FUN_004E23E0` does not read on).
        if let Ok(Some(word)) = ex.word() {
            let line = ex.last_line;
            ex.diag(
                line,
                format!(
                    "\"{word}\" at top level ends reading of the file in the original; the rest is not read"
                ),
            );
        }
    }
    Expanded {
        widgets,
        commands: ex.commands,
        preload: ex.preload,
        diagnostics: ex.diagnostics,
    }
}

/// Abort of the file; the diagnostic is already recorded.
struct Abort;

type Step<T = ()> = Result<T, Abort>;

/// Entry of the loop stack `MenuSystem+0xAC` (`{i, n, file position}`, `FUN_0048FBA0`).
struct LoopState {
    index: i32,
    count: i32,
    /// Index of the line after the line of `n` in [`Expander::lines`].
    resume: usize,
}

struct Expander {
    is_menu_level: bool,
    /// Non-empty lines of the file: line number and uppercased words (`FUN_00574EA0`).
    lines: Vec<(u32, Vec<String>)>,
    /// Next line to read (file position).
    next_line: usize,
    /// Rest of the current line (line buffer `+0x14`).
    buffer: VecDeque<String>,
    buffer_line: u32,
    /// Line of the word read last.
    last_line: u32,
    words_read: usize,
    prefix: String,
    vars: BTreeMap<String, i32>,
    loops: Vec<LoopState>,
    commands: Vec<AppCommand>,
    preload: Vec<String>,
    diagnostics: Vec<Diagnostic>,
}

impl Expander {
    fn new(doc: &DlgDocument, opts: ExpandOptions) -> Self {
        let mut lines: Vec<(u32, Vec<String>)> = Vec::new();
        for word in doc.words() {
            let upper = super::upper_word(&word.text);
            match lines.last_mut() {
                Some((line, words)) if *line == word.line => words.push(upper),
                _ => lines.push((word.line, vec![upper])),
            }
        }
        Self {
            is_menu_level: opts.is_menu_level,
            lines,
            next_line: 0,
            buffer: VecDeque::new(),
            buffer_line: 1,
            last_line: 1,
            words_read: 0,
            prefix: String::new(),
            vars: BTreeMap::new(),
            loops: Vec::new(),
            commands: Vec::new(),
            preload: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn diag(&mut self, line: u32, message: String) {
        self.diagnostics.push(Diagnostic { line, message });
    }

    fn fail<T>(&mut self, line: u32, message: String) -> Step<T> {
        self.diag(line, message);
        Err(Abort)
    }

    /// Next word (tokenizer `FUN_004DD400`): first the line buffer, then the next non-empty line.
    /// `None` at end of file.
    fn word(&mut self) -> Step<Option<String>> {
        loop {
            if let Some(word) = self.buffer.pop_front() {
                self.words_read += 1;
                self.last_line = self.buffer_line;
                if self.words_read > MAX_WORDS {
                    let line = self.last_line;
                    return self.fail(
                        line,
                        format!(
                            "Evaluation aborted after {MAX_WORDS} words (loops too large); the original \
                             would keep reading"
                        ),
                    );
                }
                return Ok(Some(word));
            }
            let Some((line, words)) = self.lines.get(self.next_line) else {
                return Ok(None);
            };
            self.buffer_line = *line;
            self.buffer = words.iter().cloned().collect();
            self.next_line += 1;
        }
    }

    /// Pushes a word back (`FUN_004DD550`: line buffer = word + `" "` + rest).
    fn push_back(&mut self, word: String) {
        self.buffer.push_front(word);
        self.buffer_line = self.last_line;
    }

    /// Child area `FUN_004E1E50`. `Ok` at end of file and on a `/...` word (pushed back).
    fn children(&mut self, parent: &mut Vec<WidgetDesc>, depth: usize) -> Step {
        loop {
            let Some(word) = self.word()? else {
                return Ok(());
            };
            let line = self.last_line;
            match word.as_str() {
                // 0x004E1EC0...0x004E1F76: {0, n, position after the line of n}
                "LOOP" => {
                    let Some(count) = self.word()? else {
                        return Ok(());
                    };
                    let count = self.eval(&count)?;
                    self.loops.push(LoopState {
                        index: 0,
                        count,
                        resume: self.next_line,
                    });
                }
                // 0x004E1F84...0x004E1FF0
                "/LOOP" => {
                    let Some(top) = self.loops.pop() else {
                        return self.fail(
                            line,
                            "/LOOP without open LOOP; the original aborts the file".to_string(),
                        );
                    };
                    if top.index + 1 < top.count {
                        self.next_line = top.resume;
                        self.loops.push(LoopState {
                            index: top.index + 1,
                            ..top
                        });
                    }
                }
                // 0x004E1FFE...0x004E207B: evaluate first, then set.
                "VAR" => {
                    let Some(name) = self.word()? else {
                        return Ok(());
                    };
                    let Some(value) = self.word()? else {
                        return Ok(());
                    };
                    let value = self.eval(&value)?;
                    self.vars.insert(name, value);
                }
                // 0x004E20AD...0x004E214F: unknown variable -> error (the expression is not read any further).
                "ADD" => {
                    let Some(name) = self.word()? else {
                        return self
                            .fail(line, "ADD at end of file without a variable".to_string());
                    };
                    let Some(&old) = self.vars.get(&name) else {
                        let line = self.last_line;
                        return self.fail(
                            line,
                            format!(
                                "ADD on the unknown variable \"{name}\"; the original aborts the file"
                            ),
                        );
                    };
                    let Some(value) = self.word()? else {
                        return Ok(());
                    };
                    let value = self.eval(&value)?;
                    self.vars.insert(name, old.wrapping_add(value));
                }
                _ => {
                    if let Some(kind) = ElementKind::from_keyword(&word) {
                        self.element(kind, parent, depth + 1)?;
                    } else if word.starts_with('/') {
                        // 0x004E2374...0x004E238E
                        self.push_back(word);
                        return Ok(());
                    } else {
                        return self.fail(
                            line,
                            format!(
                                "Unknown word \"{word}\" in the child area; the original aborts the file here"
                            ),
                        );
                    }
                }
            }
        }
    }

    /// Element parser (`FUN_004DD9C0` and siblings, `docs/formats/dlg.md`, section "Elements"): descriptor with
    /// defaults, name, then flags and head keys; the first foreign word builds the widget and starts the child
    /// area, the end word builds it at the latest.
    fn element(&mut self, kind: ElementKind, parent: &mut Vec<WidgetDesc>, depth: usize) -> Step {
        let open_line = self.last_line;
        let end = kind.end_keyword();
        let keyword = &end[1..];
        if depth > MAX_DEPTH {
            return self.fail(
                open_line,
                format!("Nesting deeper than {MAX_DEPTH} levels at {keyword}; evaluation aborted"),
            );
        }
        if kind == ElementKind::Frame {
            // 0x004DD9ED / 0x004DD9FF: clear loop stack and variables, before the name.
            self.loops.clear();
            self.vars.clear();
        }
        let Some(raw_name) = self.word()? else {
            return self.fail(
                open_line,
                format!("{keyword} without a name at end of file"),
            );
        };
        let (name, element) = if kind == ElementKind::Frame {
            // 0x004DDA19 / 0x004DDA2E: name without prefix, also the new prefix; element name equals frame name.
            self.prefix = raw_name.clone();
            (raw_name.clone(), raw_name)
        } else {
            // FUN_004DEAE0: counter of each active loop from outside to inside (element name `+0x9C`), then the
            // prefix in front of it for the table key (`FUN_00410030`, `0x004E0746`).
            let mut element = raw_name;
            for state in &self.loops {
                let _ = write!(element, "{}", state.index);
            }
            (format!("{}{element}", self.prefix), element)
        };
        let mut desc = WidgetKind::with_defaults(kind);
        let mut in_flags = false;
        let mut built: Option<usize> = None;
        loop {
            let Some(word) = self.word()? else {
                let line = self.last_line;
                return self.fail(
                    line,
                    format!(
                        "{keyword} {name} (line {open_line}) without {end} until end of file; the original \
                         aborts the file"
                    ),
                );
            };
            let line = self.last_line;
            if word == "FLAGS" {
                in_flags = true;
                continue;
            }
            if in_flags {
                if let Some(bit) = flag_bit(kind, &word) {
                    *desc.flags_mut() |= bit;
                } else if word == "/FLAGS" {
                    in_flags = false;
                } else {
                    return self.fail(
                        line,
                        format!(
                            "Unknown flag \"{word}\" in {keyword} {name}; the original aborts the file"
                        ),
                    );
                }
                continue;
            }
            if self.head_key(&word, &mut desc)? {
                continue;
            }
            if word == end {
                if built.is_none() {
                    parent.push(WidgetDesc {
                        name,
                        element,
                        kind: desc,
                        children: Vec::new(),
                    });
                }
                return Ok(());
            }
            // First foreign word: build the widget from the descriptor (copy; later head keys have no effect).
            let index = match built {
                Some(index) => index,
                None => {
                    parent.push(WidgetDesc {
                        name: name.clone(),
                        element: element.clone(),
                        kind: desc.clone(),
                        children: Vec::new(),
                    });
                    built = Some(parent.len() - 1);
                    parent.len() - 1
                }
            };
            if word.starts_with('/') {
                // The original pushes the word back, the child area returns it immediately: endless loop
                // (frame 0x004DE1A4...0x004DE405, BUTTON 0x004E0783...0x004E0390).
                return self.fail(
                    line,
                    format!(
                        "{keyword} {name} (line {open_line}) without {end}: \"{word}\" instead; the original \
                         reads forever here, the reimplementation aborts the file"
                    ),
                );
            }
            self.push_back(word);
            self.children(&mut parent[index].children, depth)?;
        }
    }

    /// Head key of the kind; `false` if `key` is none. Reads the arguments and fills the descriptor like the
    /// element parsers (tables in `docs/formats/dlg.md`, section "Element kinds").
    fn head_key(&mut self, key: &str, desc: &mut WidgetKind) -> Step<bool> {
        match desc {
            WidgetKind::Frame(f) => match key {
                "EXTENT" => f.extent = self.rect(key)?,
                "ZIELPOSITION" => f.target = [self.number(key)?, self.number(key)?],
                "DURATION" => f.duration_ms = self.number(key)?,
                "BAREXTENT" => f.bar_extent = self.rect(key)?,
                "TITLECOLOR" => self.color(key, &mut f.title_color)?,
                "TITLEEXTENT" => f.title_extent = self.rect(key)?,
                "TITLE" => f.title = Some(self.number(key)?),
                // 0x004DDEA8 (O-7)
                "BACKGROUND" => {
                    let path = self.arg(key)?;
                    let level = f.flags & frame_flags::LEVELBACKGROUND != 0;
                    if (!level || self.is_menu_level) && f.flags & frame_flags::BACKGROUND == 0 {
                        if !level {
                            // Path discarded, 3D background off anyway.
                            self.commands.push(AppCommand::HideLevelBackground);
                        }
                    } else {
                        f.flags |= frame_flags::BACKGROUND;
                        f.background = Some(path);
                        f.flags &= !frame_flags::LEVELBACKGROUND;
                        self.commands.push(AppCommand::HideLevelBackground);
                    }
                }
                // 0x004DDF4A: only with IsMenuLevel; command 0x21 only if the flag is already set.
                "LEVELBACKGROUND" => {
                    let path = self.arg(key)?;
                    if self.is_menu_level {
                        if f.flags & frame_flags::LEVELBACKGROUND != 0 {
                            self.commands
                                .push(AppCommand::LoadLevelBackground(path.clone()));
                        }
                        f.background = Some(path);
                    }
                }
                "MUSIC" => f.music = Some(self.arg(key)?),
                "BKGFILE" => f.bkg_file = Some(self.arg(key)?),
                // 0x004DE071...0x004DE0B3: load texture immediately.
                "PRELOAD" => {
                    let path = self.arg(key)?;
                    self.preload.push(path);
                }
                _ => return Ok(false),
            },
            WidgetKind::Button(b) => match key {
                "EXTENT" => b.extent = self.rect(key)?,
                "BORDERSPACE" => b.border_space = self.rect(key)?,
                "TEXTCOLOR" => self.color(key, &mut b.text_color)?,
                "DISABLEDTEXTCOLOR" => self.color(key, &mut b.disabled_text_color)?,
                "MOUSEOVERTEXTCOLOR" => self.color_opt(key, &mut b.mouse_over_text_color)?,
                "PRESSEDBUTTONTEXTCOLOR" => self.color_opt(key, &mut b.pressed_text_color)?,
                "TEXT" => b.text = Some(self.number(key)?),
                _ => return Ok(false),
            },
            // FUN_004DEDE0 (O-10): TEXTCOLOR into the button, then copied into the list; MOUSEOVERCOLOR and
            // MOUSEOVERTEXTCOLOR into the list, then copied into the button.
            WidgetKind::ComboBox(b, l) => match key {
                "BUTTONEXTENT" => b.extent = self.rect(key)?,
                "LISTBOXEXTENT" => l.extent = self.rect(key)?,
                "TEXTCOLOR" => {
                    self.color(key, &mut b.text_color)?;
                    l.text_color = b.text_color;
                }
                "DISABLEDTEXTCOLOR" => self.color(key, &mut b.disabled_text_color)?,
                "SELECTIONBARCOLOR" => self.color(key, &mut l.selection_bar_color)?,
                "MOUSEOVERCOLOR" | "MOUSEOVERTEXTCOLOR" => {
                    self.color(key, &mut l.mouse_over_color)?;
                    b.mouse_over_text_color = Some(l.mouse_over_color);
                }
                "PRESSEDBUTTONTEXTCOLOR" => self.color_opt(key, &mut b.pressed_text_color)?,
                "SELECTEDTEXTCOLOR" => self.color(key, &mut l.selected_text_color)?,
                "TEXT" => b.text = Some(self.number(key)?),
                "CURSELECTION" => l.cur_selection = self.number(key)?,
                _ => return Ok(false),
            },
            WidgetKind::ListBox(l) => match key {
                "EXTENT" => l.extent = self.rect(key)?,
                "TEXTCOLOR" => self.color(key, &mut l.text_color)?,
                "DISABLEDTEXTCOLOR" => self.color(key, &mut l.disabled_text_color)?,
                "SELECTIONBARCOLOR" => self.color(key, &mut l.selection_bar_color)?,
                "MOUSEOVERCOLOR" => self.color(key, &mut l.mouse_over_color)?,
                "SELECTEDTEXTCOLOR" => self.color(key, &mut l.selected_text_color)?,
                "CURSELECTION" => l.cur_selection = self.number(key)?,
                _ => return Ok(false),
            },
            WidgetKind::EditField(e) => match key {
                "EXTENT" => e.extent = self.rect(key)?,
                "TEXTCOLOR" => self.color(key, &mut e.text_color)?,
                "DISABLEDTEXTCOLOR" => self.color(key, &mut e.disabled_text_color)?,
                "TEXT" => e.text = Some(self.number(key)?),
                _ => return Ok(false),
            },
            WidgetKind::Slider(s) => match key {
                "EXTENT" => s.extent = self.rect(key)?,
                "CURVALUE" => s.cur_value = self.number(key)?,
                "RESOLUTION" => s.resolution = self.number(key)?,
                "TEXTCOLOR" => self.color(key, &mut s.text_color)?,
                "DISABLEDTEXTCOLOR" => self.color(key, &mut s.disabled_text_color)?,
                "THUMBCOLOR" => self.color(key, &mut s.thumb_color)?,
                "SCALECOLOR" => self.color(key, &mut s.scale_color)?,
                "TEXT" => s.text = Some(self.number(key)?),
                _ => return Ok(false),
            },
            WidgetKind::RadioButton(r) => match key {
                "EXTENT" => r.extent = self.rect(key)?,
                "ICONOFFSET" => r.icon_offset = self.number(key)?,
                "TEXT" => r.text = Some(self.number(key)?),
                "TEXTCOLOR" => self.color(key, &mut r.text_color)?,
                "DISABLEDTEXTCOLOR" => self.color(key, &mut r.disabled_text_color)?,
                _ => return Ok(false),
            },
            WidgetKind::Label(l) => match key {
                // 0x004E1882 / 0x004E188B
                "CREDITS" => {
                    l.credits = Some(self.arg(key)?);
                    l.flags |= label_flags::CREDITS;
                }
                "EXTENT" => l.extent = self.rect(key)?,
                "TEXTCOLOR" => self.color(key, &mut l.text_color)?,
                "BACKGROUNDCOLOR" => self.color_opt(key, &mut l.background_color)?,
                "DISABLEDTEXTCOLOR" => self.color(key, &mut l.disabled_text_color)?,
                "TEXT" => l.text = Some(self.number(key)?),
                _ => return Ok(false),
            },
        }
        Ok(true)
    }

    /// One argument; end of file in the arguments is an error (the element parser reads nothing after that).
    fn arg(&mut self, key: &str) -> Step<String> {
        match self.word()? {
            Some(word) => Ok(word),
            None => {
                let line = self.last_line;
                self.fail(line, format!("{key}: end of file in the arguments"))
            }
        }
    }

    fn number(&mut self, key: &str) -> Step<i32> {
        let word = self.arg(key)?;
        self.eval(&word)
    }

    fn rect(&mut self, key: &str) -> Step<[i32; 4]> {
        Ok([
            self.number(key)?,
            self.number(key)?,
            self.number(key)?,
            self.number(key)?,
        ])
    }

    /// Color via `sscanf("%x")` (`0x00616DE0`): if reading fails, the field stays unchanged.
    fn color(&mut self, key: &str, target: &mut u32) -> Step {
        let word = self.arg(key)?;
        if let Some(value) = scan_hex(&word) {
            *target = value;
        }
        Ok(())
    }

    /// Like [`Self::color`] for fields without a default (O-9).
    fn color_opt(&mut self, key: &str, target: &mut Option<u32>) -> Step {
        let word = self.arg(key)?;
        if let Some(value) = scan_hex(&word) {
            *target = Some(value);
        }
        Ok(())
    }

    /// Number expression like `FUN_004DD730` (`MenuDlg_EvalNumberExpr`): parts split at `+`, each part first as a
    /// variable, otherwise `atol`; sum with 32-bit overflow. A `+` at the start of the rest makes the original
    /// compute forever (`CString_Left` with length 0 does not shorten) - here diagnostic and abort (deviation, O-2).
    fn eval(&mut self, expr: &str) -> Step<i32> {
        let line = self.last_line;
        let mut total = 0i32;
        let mut rest = expr;
        loop {
            let (part, tail) = match rest.find('+') {
                None => (rest, None),
                Some(0) => {
                    return self.fail(
                        line,
                        format!(
                            "Number expression \"{expr}\": \"+\" at the start of a part; the original computes \
                             forever here, the reimplementation aborts the file"
                        ),
                    );
                }
                Some(pos) => (&rest[..pos], Some(&rest[pos + 1..])),
            };
            let value = match self.vars.get(part) {
                Some(&value) => value,
                None => atol(part),
            };
            total = total.wrapping_add(value);
            match tail {
                None => return Ok(total),
                Some(tail) => rest = tail,
            }
        }
    }
}

/// Whitespace for `atol`/`sscanf` (C locale: `\t`...`\r` and space).
fn is_c_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ')
}

/// `atol` of the VS 2003 runtime (`0x005D2E7D`): whitespace, one sign, digits; overflow without limit.
fn atol(text: &str) -> i32 {
    let mut chars = text.chars().skip_while(|&c| is_c_space(c)).peekable();
    let negative = match chars.peek() {
        Some('-') => {
            chars.next();
            true
        }
        Some('+') => {
            chars.next();
            false
        }
        _ => false,
    };
    let mut value = 0i32;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
        value = value.wrapping_mul(10).wrapping_add(digit as i32);
        chars.next();
    }
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// `sscanf(word, "%x")` of the VS 2003 runtime: whitespace, one sign, `0x`/`0X`, hex digits (the lower
/// 32 bits remain). `None` if no digit was read - then `sscanf` writes nothing.
fn scan_hex(word: &str) -> Option<u32> {
    let mut chars = word.chars().skip_while(|&c| is_c_space(c)).peekable();
    let negative = match chars.peek() {
        Some('-') => {
            chars.next();
            true
        }
        Some('+') => {
            chars.next();
            false
        }
        _ => false,
    };
    let mut started = false;
    if chars.peek() == Some(&'0') {
        chars.next();
        if matches!(chars.peek(), Some('x' | 'X')) {
            chars.next();
        } else {
            // The zero counts as a digit.
            started = true;
        }
    }
    let mut value = 0u32;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(16)) {
        value = (value << 4).wrapping_add(digit);
        started = true;
        chars.next();
    }
    started.then(|| {
        if negative {
            value.wrapping_neg()
        } else {
            value
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::desc::*;
    use super::super::parse_dlg;
    use super::*;

    const MENU: ExpandOptions = ExpandOptions {
        is_menu_level: true,
    };
    const NO_MENU: ExpandOptions = ExpandOptions {
        is_menu_level: false,
    };

    /// Evaluates `src`; the characters are read as Latin-1 (one byte per character), like the files.
    fn run(src: &str, opts: ExpandOptions) -> Expanded {
        let bytes: Vec<u8> = src
            .chars()
            .map(|c| u8::try_from(c).expect("test text must be Latin-1"))
            .collect();
        let (doc, _) = parse_dlg(&bytes);
        expand(&doc, opts)
    }

    /// All names in tree order (depth first).
    fn names(widgets: &[WidgetDesc]) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(w: &[WidgetDesc], out: &mut Vec<String>) {
            for d in w {
                out.push(d.name.clone());
                walk(&d.children, out);
            }
        }
        walk(widgets, &mut out);
        out
    }

    /// Tree as text: `F(FA,FB(..))`.
    fn shape(widgets: &[WidgetDesc]) -> String {
        widgets
            .iter()
            .map(|w| {
                if w.children.is_empty() {
                    w.name.clone()
                } else {
                    format!("{}({})", w.name, shape(&w.children))
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    fn only_child(out: &Expanded) -> &WidgetDesc {
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        match out.widgets.as_slice() {
            [frame] => match frame.children.as_slice() {
                [child] => child,
                other => panic!("expected exactly one child, found {}", other.len()),
            },
            other => panic!("expected exactly one FRAME, found {}", other.len()),
        }
    }

    fn frame_desc(out: &Expanded) -> &FrameDesc {
        match &out.widgets[0].kind {
            WidgetKind::Frame(f) => f,
            other => panic!("not a FRAME: {other:?}"),
        }
    }

    // QUIRK O-2: kept
    #[test]
    fn numbers_add_variables_and_ignore_minus() {
        let out = run(
            "FRAME F\r\nVAR A 10\r\nLABEL L EXTENT A+5 A+-5 A-5 7 /LABEL\r\n/FRAME\r\n",
            NO_MENU,
        );
        let WidgetKind::Label(label) = &only_child(&out).kind else {
            panic!("not a LABEL");
        };
        assert_eq!(label.extent, [15, 5, 0, 7]);
        // atol reads a leading sign and digits; the rest does not count. An empty part after `+` is 0.
        let out = run(
            "FRAME F LABEL L EXTENT 12AB -440 A+ 2147483647+1 /LABEL /FRAME",
            NO_MENU,
        );
        let WidgetKind::Label(label) = &only_child(&out).kind else {
            panic!("not a LABEL");
        };
        assert_eq!(label.extent, [12, -440, 0, i32::MIN]);
        // Variable names are uppercased like all words.
        let out = run(
            "FRAME F var xpos 3 LABEL L EXTENT XPOS+xpos 0 0 0 /LABEL /FRAME",
            NO_MENU,
        );
        let WidgetKind::Label(label) = &only_child(&out).kind else {
            panic!("not a LABEL");
        };
        assert_eq!(label.extent, [6, 0, 0, 0]);
    }

    #[test]
    fn hex_colors_read_like_sscanf() {
        let out = run(
            "FRAME F BUTTON B TEXTCOLOR ddFFf5bf DISABLEDTEXTCOLOR 0x00ff00ff \
             MOUSEOVERTEXTCOLOR -1 PRESSEDBUTTONTEXTCOLOR 123456789AB /BUTTON /FRAME",
            NO_MENU,
        );
        let WidgetKind::Button(b) = &only_child(&out).kind else {
            panic!("not a BUTTON");
        };
        assert_eq!(b.text_color, 0xDDFF_F5BF);
        assert_eq!(b.disabled_text_color, 0x00FF_00FF);
        assert_eq!(b.mouse_over_text_color, Some(0xFFFF_FFFF));
        // More than eight digits: only the lower 32 bits remain.
        assert_eq!(b.pressed_text_color, Some(0x4567_89AB));
        // Not a hex value: sscanf fails, the field keeps its value.
        let out = run(
            "FRAME F BUTTON B TEXTCOLOR XYZ PRESSEDBUTTONTEXTCOLOR 0x /BUTTON /FRAME",
            NO_MENU,
        );
        let WidgetKind::Button(b) = &only_child(&out).kind else {
            panic!("not a BUTTON");
        };
        assert_eq!(b.text_color, 0xFFFF_FFFF);
        assert_eq!(b.pressed_text_color, None);
    }

    // QUIRK O-6: kept
    #[test]
    fn loop_repeats_and_appends_counters() {
        let src = "FRAME F\r\n\
                   LOOP 0\r\n\tLABEL Z /LABEL\r\n/LOOP\r\n\
                   LOOP -3\r\n\tLABEL N /LABEL\r\n/LOOP\r\n\
                   LOOP 2\r\n\tLABEL V /LABEL\r\n/LOOP\r\n\
                   LOOP 2\r\n\tLOOP 2\r\n\t\tLABEL W /LABEL\r\n\t/LOOP\r\n/LOOP\r\n\
                   LABEL E /LABEL\r\n\
                   /FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(
            names(&out.widgets),
            [
                "F", "FZ0", "FN0", "FV0", "FV1", "FW00", "FW01", "FW10", "FW11", "FE"
            ]
        );
        // Multi-digit counters without separator (O-6).
        let out = run(
            "FRAME F\r\nLOOP 12\r\nLABEL V /LABEL\r\n/LOOP\r\n/FRAME",
            NO_MENU,
        );
        let all = names(&out.widgets);
        assert_eq!(all.len(), 13);
        assert_eq!(all[12], "FV11");
    }

    #[test]
    fn element_names_keep_counters_without_prefix() {
        // `+0x9C` = element name in uppercase with counters, without the frame prefix; for the FRAME its name.
        let out = run(
            "FRAME options\r\nbutton ok /BUTTON\r\nLOOP 2\r\nLOOP 2\r\nLABEL x /LABEL\r\n/LOOP\r\n/LOOP\r\n\
             FRAME inner LABEL y /LABEL /FRAME\r\n/FRAME\r\n",
            NO_MENU,
        );
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        let mut all = Vec::new();
        fn walk(w: &[WidgetDesc], out: &mut Vec<(String, String)>) {
            for d in w {
                out.push((d.name.clone(), d.element.clone()));
                walk(&d.children, out);
            }
        }
        walk(&out.widgets, &mut all);
        let pairs: Vec<(&str, &str)> = all.iter().map(|(n, e)| (n.as_str(), e.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("OPTIONS", "OPTIONS"),
                ("OPTIONSOK", "OK"),
                ("OPTIONSX00", "X00"),
                ("OPTIONSX01", "X01"),
                ("OPTIONSX10", "X10"),
                ("OPTIONSX11", "X11"),
                ("INNER", "INNER"),
                ("INNERY", "Y"),
            ]
        );
    }

    // QUIRK O-6: kept
    #[test]
    fn loop_jumps_back_by_line() {
        // The rest of the /LOOP line is still in the line buffer and is read first after every jump back
        // (`docs/formats/dlg.md`, quirk 9); after the last pass no loop is active any more.
        let src = "FRAME F\r\nLOOP 3\r\nLABEL A /LABEL\r\n/LOOP LABEL X /LABEL\r\n/FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(
            names(&out.widgets),
            ["F", "FA0", "FX1", "FA1", "FX2", "FA2", "FX"]
        );
        // The rest of the LOOP line is not repeated: the jump back goes to the start of the following line.
        let src = "FRAME F\r\nLOOP 2 LABEL Y /LABEL\r\nLABEL B /LABEL\r\n/LOOP\r\n/FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(names(&out.widgets), ["F", "FY0", "FB0", "FB1"]);
        // If the number is on the next line, the body begins after its line.
        let src = "FRAME F\r\nLOOP\r\n2 LABEL Y /LABEL\r\nLABEL B /LABEL /LOOP\r\n/FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(names(&out.widgets), ["F", "FY0", "FB0", "FB1"]);
    }

    // QUIRK O-6: kept
    #[test]
    fn frame_clears_variables_and_loops() {
        let src = "FRAME F VAR A 5 LABEL L EXTENT A 0 0 0 /LABEL /FRAME\r\n\
                   FRAME G LABEL M EXTENT A 0 0 0 /LABEL /FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(shape(&out.widgets), "F(FL),G(GM)");
        let extent = |w: &WidgetDesc| match &w.kind {
            WidgetKind::Label(l) => l.extent,
            _ => unreachable!(),
        };
        assert_eq!(extent(&out.widgets[0].children[0]), [5, 0, 0, 0]);
        // In the second FRAME, A is unknown: atol("A") = 0.
        assert_eq!(extent(&out.widgets[1].children[0]), [0, 0, 0, 0]);

        // A FRAME inside a loop clears the loop stack: the /LOOP after it is an error.
        let out = run(
            "LOOP 2\r\nFRAME H /FRAME\r\n/LOOP\r\nFRAME I /FRAME\r\n",
            NO_MENU,
        );
        assert_eq!(shape(&out.widgets), "H");
        assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
        assert_eq!(out.diagnostics[0].line, 3);

        // After a nested FRAME, the prefix stays that FRAME's name.
        let out = run(
            "FRAME A LABEL X /LABEL FRAME B /FRAME LABEL L /LABEL /FRAME",
            NO_MENU,
        );
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(shape(&out.widgets), "A(AX,B,BL)");
    }

    // QUIRK O-5: kept
    #[test]
    fn head_keys_after_the_first_child_abort_the_file() {
        // After the first child, the child area (FUN_004E1E50) keeps reading; a head key there is an unknown word
        // and aborts the file - also PRELOAD, BACKGROUND and LEVELBACKGROUND.
        for (key, line) in [
            ("PRELOAD DATA/X.TGA", 4),
            ("BACKGROUND DATA/X.TGA", 4),
            ("LEVELBACKGROUND DATA/X.LVL", 4),
            ("TITLE 5", 4),
        ] {
            let src = format!(
                "FRAME F\r\nFLAGS MENUFRAME_LEVELBACKGROUND /FLAGS\r\nLABEL A /LABEL\r\n{key}\r\n\
                 LABEL B /LABEL\r\n/FRAME\r\n"
            );
            for opts in [MENU, NO_MENU] {
                let out = run(&src, opts);
                assert_eq!(shape(&out.widgets), "F(FA)", "{key}");
                assert!(out.preload.is_empty(), "{key}");
                assert!(out.commands.is_empty(), "{key}: {:?}", out.commands);
                assert_eq!(frame_desc(&out).background, None, "{key}");
                assert_eq!(frame_desc(&out).title, None, "{key}");
                assert_eq!(out.diagnostics.len(), 1, "{key}: {:?}", out.diagnostics);
                assert_eq!(out.diagnostics[0].line, line, "{key}");
            }
        }
        // Likewise in other elements: the TEXT after the child has no effect, the button stays built.
        let out = run(
            "FRAME F BUTTON B LABEL L /LABEL TEXT 5 /BUTTON /FRAME",
            NO_MENU,
        );
        assert_eq!(shape(&out.widgets), "F(FB(FL))");
        let WidgetKind::Button(b) = &out.widgets[0].children[0].kind else {
            panic!("not a BUTTON");
        };
        assert_eq!(b.text, None);
        assert_eq!(out.diagnostics.len(), 1);
        // VAR in the element starts the child area likewise.
        let out = run("FRAME F VAR A 1 TITLE 5 /FRAME", NO_MENU);
        assert_eq!(shape(&out.widgets), "F");
        assert_eq!(frame_desc(&out).title, None);
        assert_eq!(out.diagnostics.len(), 1);
    }

    // QUIRK O-7: kept
    #[test]
    fn background_keys_depend_on_order_and_menu_level() {
        const LVL: &str = "DATA/LEVELS/MENÜLEVEL/MENÜ2.LVL";
        const PAT: &str = "DATA/MENÜ/BACKGROUND_1024X1024_PAT.TGA";
        let check = |src: &str,
                     opts: ExpandOptions,
                     flags: u32,
                     background: Option<&str>,
                     commands: &[AppCommand]| {
            let out = run(src, opts);
            assert!(out.diagnostics.is_empty(), "{src}: {:?}", out.diagnostics);
            let frame = frame_desc(&out);
            assert_eq!(frame.flags, flags, "{src} {opts:?}");
            assert_eq!(frame.background.as_deref(), background, "{src} {opts:?}");
            assert_eq!(out.commands, commands, "{src} {opts:?}");
        };
        use AppCommand::*;
        let load = LoadLevelBackground(LVL.to_string());
        // Case 1 (start.dlg): flag 0x80, then LEVELBACKGROUND and BACKGROUND.
        let src = "FRAME F FLAGS MENUFRAME_LEVELBACKGROUND /FLAGS\r\n\
                   LEVELBACKGROUND data/levels/menülevel/menü2.lvl\r\n\
                   BACKGROUND data/menü/background_1024x1024_pat.tga\r\n/FRAME";
        check(src, MENU, 0x80, Some(LVL), std::slice::from_ref(&load));
        check(src, NO_MENU, 0x1, Some(PAT), &[HideLevelBackground]);
        // Case 2: MENUFRAME_BACKGROUND with BACKGROUND - the image in both modes.
        let src = "FRAME F FLAGS MENUFRAME_BACKGROUND /FLAGS BACKGROUND data/menü/background_1024x1024_pat.tga /FRAME";
        check(src, MENU, 0x1, Some(PAT), &[HideLevelBackground]);
        check(src, NO_MENU, 0x1, Some(PAT), &[HideLevelBackground]);
        // Case 3: BACKGROUND without a matching flag (credits.dlg): path discarded, 3D background off anyway.
        let src = "FRAME F BACKGROUND data/menü/background_1024x1024_pat.tga /FRAME";
        check(src, MENU, 0, None, &[HideLevelBackground]);
        check(src, NO_MENU, 0, None, &[HideLevelBackground]);
        // Case 4: order matters - BACKGROUND before FLAGS does not see any flags yet, LEVELBACKGROUND before FLAGS
        // sends no command.
        let src = "FRAME F BACKGROUND data/menü/background_1024x1024_pat.tga \
                   FLAGS MENUFRAME_LEVELBACKGROUND /FLAGS /FRAME";
        check(src, MENU, 0x80, None, &[HideLevelBackground]);
        check(src, NO_MENU, 0x80, None, &[HideLevelBackground]);
        let src = "FRAME F LEVELBACKGROUND data/levels/menülevel/menü2.lvl \
                   FLAGS MENUFRAME_LEVELBACKGROUND /FLAGS /FRAME";
        check(src, MENU, 0x80, Some(LVL), &[]);
        check(src, NO_MENU, 0x80, None, &[]);
        // LEVELBACKGROUND without flag and without IsMenuLevel: the word is skipped.
        let src = "FRAME F LEVELBACKGROUND data/levels/menülevel/menü2.lvl /FRAME";
        check(src, MENU, 0, Some(LVL), &[]);
        check(src, NO_MENU, 0, None, &[]);
    }

    #[test]
    fn frame_keys_fill_the_descriptor() {
        let out = run(
            "FRAME F FLAGS MENUFRAME_MUSIC MENUFRAME_MOVE HIDE /FLAGS EXTENT -440 212 0 912 ZIELPOSITION 40 212 \
             DURATION 300 BAREXTENT 0 140 550 225 TITLECOLOR ddFFf5bf TITLEEXTENT -490 50 -50 185 TITLE 15099 \
             MUSIC data/music/title.ogg BKGFILE data/x.bkg PRELOAD data/a.tga PRELOAD data/b.tga /FRAME",
            NO_MENU,
        );
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(
            *frame_desc(&out),
            FrameDesc {
                flags: 0x40 | 0x1000 | 0x1000_0000,
                extent: [-440, 212, 0, 912],
                target: [40, 212],
                duration_ms: 300,
                bar_extent: [0, 140, 550, 225],
                title_color: 0xDDFF_F5BF,
                title_extent: [-490, 50, -50, 185],
                title: Some(15099),
                background: None,
                music: Some("DATA/MUSIC/TITLE.OGG".to_string()),
                bkg_file: Some("DATA/X.BKG".to_string()),
            }
        );
        assert_eq!(out.preload, ["DATA/A.TGA", "DATA/B.TGA"]);
        // A key can appear several times, the last one counts.
        let out = run("FRAME F DURATION 5 DURATION 7 /FRAME", NO_MENU);
        assert_eq!(frame_desc(&out).duration_ms, 7);
    }

    // QUIRK O-10: kept
    // QUIRK O-9: deviation
    #[test]
    fn defaults_follow_the_original() {
        let src = "FRAME F\r\nBUTTON B /BUTTON\r\nCOMBOBOX C /COMBOBOX\r\nLISTBOX L /LISTBOX\r\n\
                   SLIDER S /SLIDER\r\nEDITFIELD E /EDITFIELD\r\nRADIOBUTTON R /RADIOBUTTON\r\n\
                   LABEL T /LABEL\r\n/FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        let frame = &out.widgets[0];
        assert_eq!(frame.name, "F");
        assert_eq!(
            frame.kind,
            WidgetKind::Frame(FrameDesc {
                flags: 0,
                extent: [0; 4],
                target: [0; 2],
                duration_ms: 1000,
                bar_extent: [0; 4],
                title_color: 0xFFFF_FFFF,
                title_extent: [0; 4],
                title: None,
                background: None,
                music: None,
                bkg_file: None,
            })
        );
        let kinds: Vec<&WidgetKind> = frame.children.iter().map(|c| &c.kind).collect();
        let button = ButtonDesc {
            flags: 0,
            extent: [0; 4],
            border_space: [0; 4],
            text: None,
            text_color: 0xFFFF_FFFF,
            disabled_text_color: 0xFFB0_B0B0,
            mouse_over_text_color: None,
            pressed_text_color: None,
        };
        let list = ListDesc {
            flags: 0,
            extent: [0; 4],
            text_color: 0xFFB0_B0B0,
            disabled_text_color: 0xFFA0_A0A0,
            selection_bar_color: 0xFF00_A000,
            mouse_over_color: 0xFFFF_FFFF,
            selected_text_color: 0xFF00_FF00,
            cur_selection: -1,
        };
        assert_eq!(*kinds[0], WidgetKind::Button(button.clone()));
        assert_eq!(
            *kinds[1],
            WidgetKind::ComboBox(
                ButtonDesc {
                    flags: 0x10,
                    ..button.clone()
                },
                list.clone()
            )
        );
        assert_eq!(*kinds[2], WidgetKind::ListBox(list.clone()));
        assert_eq!(
            *kinds[3],
            WidgetKind::Slider(SliderDesc {
                flags: 0,
                extent: [0; 4],
                cur_value: 0,
                resolution: 10,
                text_color: 0xFFFF_FFFF,
                disabled_text_color: 0xFFB0_B0B0,
                thumb_color: 0xFFFF_FFFF,
                scale_color: 0x8080_8080,
                text: None,
            })
        );
        assert_eq!(
            *kinds[4],
            WidgetKind::EditField(EditDesc {
                flags: 0,
                extent: [0; 4],
                text_color: 0xFFFF_FFFF,
                disabled_text_color: 0xFFB0_B0B0,
                text: None,
            })
        );
        assert_eq!(
            *kinds[5],
            WidgetKind::RadioButton(RadioDesc {
                flags: 0,
                extent: [0; 4],
                icon_offset: 0,
                text: None,
                text_color: 0xFFFF_FFFF,
                disabled_text_color: 0xFFB0_B0B0,
            })
        );
        assert_eq!(
            *kinds[6],
            WidgetKind::Label(LabelDesc {
                flags: 0,
                extent: [0; 4],
                text_color: 0xFFFF_FFFF,
                background_color: None,
                disabled_text_color: 0xFFB0_B0B0,
                text: None,
                credits: None,
            })
        );

        // O-10: TEXTCOLOR writes button and list, DISABLEDTEXTCOLOR only the button, MOUSEOVERCOLOR and
        // MOUSEOVERTEXTCOLOR both (list +0x2C, button +0x6C); combobox flags are in the button.
        let out = run(
            "FRAME F COMBOBOX C FLAGS MENUCOMBOBOX_TEXT /FLAGS BUTTONEXTENT 1 2 3 4 LISTBOXEXTENT 5 6 7 8 \
             TEXTCOLOR 11223344 DISABLEDTEXTCOLOR 55667788 MOUSEOVERCOLOR 99AABBCC SELECTEDTEXTCOLOR 01020304 \
             SELECTIONBARCOLOR 0 PRESSEDBUTTONTEXTCOLOR 0A0B0C0D TEXT 7 CURSELECTION 2 /COMBOBOX /FRAME",
            NO_MENU,
        );
        assert_eq!(
            only_child(&out).kind,
            WidgetKind::ComboBox(
                ButtonDesc {
                    flags: 0x11,
                    extent: [1, 2, 3, 4],
                    border_space: [0; 4],
                    text: Some(7),
                    text_color: 0x1122_3344,
                    disabled_text_color: 0x5566_7788,
                    mouse_over_text_color: Some(0x99AA_BBCC),
                    pressed_text_color: Some(0x0A0B_0C0D),
                },
                ListDesc {
                    flags: 0,
                    extent: [5, 6, 7, 8],
                    text_color: 0x1122_3344,
                    disabled_text_color: 0xFFA0_A0A0,
                    selection_bar_color: 0,
                    mouse_over_color: 0x99AA_BBCC,
                    selected_text_color: 0x0102_0304,
                    cur_selection: 2,
                }
            )
        );
        let out = run(
            "FRAME F COMBOBOX C MOUSEOVERTEXTCOLOR 12345678 /COMBOBOX /FRAME",
            NO_MENU,
        );
        let WidgetKind::ComboBox(b, l) = &only_child(&out).kind else {
            panic!("not a COMBOBOX");
        };
        assert_eq!(
            (b.mouse_over_text_color, l.mouse_over_color),
            (Some(0x1234_5678), 0x1234_5678)
        );
        // There are no list flags in the combobox: an unknown flag aborts.
        let out = run(
            "FRAME F COMBOBOX C FLAGS LISTBOX_AUTOYSIZE /FLAGS /COMBOBOX /FRAME",
            NO_MENU,
        );
        assert_eq!(shape(&out.widgets), "F");
        assert_eq!(out.diagnostics.len(), 1);
    }

    // QUIRK O-11: kept
    #[test]
    fn element_keys_fill_the_descriptors() {
        let src = "FRAME F\r\n\
            BUTTON B FLAGS MENUBUTTON_TEXT MENUBUTTON_BORDER MENUBUTTON_CHECKESCAPE MENUBUTTON_CHECKENTER \
            MENUBUTTON_SELECTABLE MENUBUTTON_SELECTED DISABLE /FLAGS EXTENT 55 20 345 100 BORDERSPACE 16 15 14 13 \
            TEXT 15110 /BUTTON\r\n\
            LISTBOX L FLAGS LISTBOX_AUTOYSIZE /FLAGS EXTENT 1 2 3 4 TEXTCOLOR 1 DISABLEDTEXTCOLOR 2 \
            SELECTIONBARCOLOR 3 MOUSEOVERCOLOR 4 SELECTEDTEXTCOLOR 5 CURSELECTION 6 /LISTBOX\r\n\
            EDITFIELD E FLAGS KEYNAME /FLAGS EXTENT 1 2 3 4 TEXTCOLOR 1 DISABLEDTEXTCOLOR 2 TEXT 3 /EDITFIELD\r\n\
            SLIDER S FLAGS MENUSLIDER_TEXT MENUSLIDER_SCALE MENUSLIDER_VERTICAL MENUSLIDER_CENTERTEXT /FLAGS \
            EXTENT 1 2 3 4 CURVALUE 12 RESOLUTION 5 TEXTCOLOR 1 DISABLEDTEXTCOLOR 2 THUMBCOLOR 3 SCALECOLOR 4 \
            TEXT 9 /SLIDER\r\n\
            RADIOBUTTON R FLAGS RADIOBUTTON_TEXT RADIOBUTTON_CHECKED /FLAGS EXTENT 1 2 3 4 ICONOFFSET 6 TEXT 7 \
            TEXTCOLOR 1 DISABLEDTEXTCOLOR 2 /RADIOBUTTON\r\n\
            LABEL T FLAGS MENULABEL_BACKGROUND MENULABEL_TEXTARRAY MENULABEL_AUTOHSCROLL MENULABEL_NOLINEBREAK \
            MENULABEL_TGA /FLAGS CREDITS data/txt/credits.txt EXTENT 1 2 3 4 TEXTCOLOR 1 BACKGROUNDCOLOR 2 \
            DISABLEDTEXTCOLOR 3 TEXT 4 /LABEL\r\n\
            /FRAME\r\n";
        let out = run(src, NO_MENU);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        let kinds: Vec<&WidgetKind> = out.widgets[0].children.iter().map(|c| &c.kind).collect();
        assert_eq!(
            *kinds[0],
            WidgetKind::Button(ButtonDesc {
                flags: 0x1 | 0x2 | 0x4 | 0x8 | 0x1_0000 | 0x2_0000 | 0x2000_0000,
                extent: [55, 20, 345, 100],
                border_space: [16, 15, 14, 13],
                text: Some(15110),
                text_color: 0xFFFF_FFFF,
                disabled_text_color: 0xFFB0_B0B0,
                mouse_over_text_color: None,
                pressed_text_color: None,
            })
        );
        assert_eq!(
            *kinds[1],
            WidgetKind::ListBox(ListDesc {
                flags: 0x100,
                extent: [1, 2, 3, 4],
                text_color: 1,
                disabled_text_color: 2,
                selection_bar_color: 3,
                mouse_over_color: 4,
                selected_text_color: 5,
                cur_selection: 6,
            })
        );
        assert_eq!(
            *kinds[2],
            WidgetKind::EditField(EditDesc {
                flags: 0x1,
                extent: [1, 2, 3, 4],
                text_color: 1,
                disabled_text_color: 2,
                text: Some(3),
            })
        );
        assert_eq!(
            *kinds[3],
            WidgetKind::Slider(SliderDesc {
                flags: 0x1E,
                extent: [1, 2, 3, 4],
                cur_value: 12,
                resolution: 5,
                text_color: 1,
                disabled_text_color: 2,
                thumb_color: 3,
                scale_color: 4,
                text: Some(9),
            })
        );
        assert_eq!(
            *kinds[4],
            WidgetKind::RadioButton(RadioDesc {
                flags: 0x5,
                extent: [1, 2, 3, 4],
                icon_offset: 6,
                text: Some(7),
                text_color: 1,
                disabled_text_color: 2,
            })
        );
        assert_eq!(
            *kinds[5],
            WidgetKind::Label(LabelDesc {
                flags: 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20,
                extent: [1, 2, 3, 4],
                text_color: 1,
                background_color: Some(2),
                disabled_text_color: 3,
                text: Some(4),
                credits: Some("DATA/TXT/CREDITS.TXT".to_string()),
            })
        );
    }

    // QUIRK O-2, O-3: deviation
    #[test]
    fn expand_stops_like_the_original() {
        // (case, source text, expected tree, line of the diagnostic)
        let cases: [(&str, &str, &str, u32); 10] = [
            (
                "unknown word in the child area",
                "FRAME F\r\nLABEL A /LABEL\r\nFOO\r\nLABEL B /LABEL\r\n/FRAME",
                "F(FA)",
                3,
            ),
            (
                "unknown flag",
                "FRAME F\r\nLABEL A /LABEL\r\nBUTTON B FLAGS NOPE /FLAGS /BUTTON\r\n/FRAME",
                "F(FA)",
                3,
            ),
            (
                "end word in flag mode",
                "FRAME F\r\nLABEL A /LABEL\r\nBUTTON B FLAGS\r\n/BUTTON\r\n/FRAME",
                "F(FA)",
                4,
            ),
            (
                "ADD on an unknown variable",
                "FRAME F\r\nLABEL A /LABEL\r\nADD X 1\r\nLABEL B /LABEL\r\n/FRAME",
                "F(FA)",
                3,
            ),
            (
                "/LOOP without LOOP",
                "FRAME F\r\nLABEL A /LABEL\r\n/LOOP\r\nLABEL B /LABEL\r\n/FRAME",
                "F(FA)",
                3,
            ),
            (
                "+5 as expression (deviation)",
                "FRAME F\r\nLABEL A\r\nEXTENT +5 0 0 0\r\n/LABEL\r\n/FRAME",
                "F",
                3,
            ),
            (
                "expression with ++ (deviation)",
                "FRAME F VAR A 1\r\nLABEL A EXTENT A++5 0 0 0 /LABEL /FRAME",
                "F",
                2,
            ),
            (
                "missing end word (deviation)",
                "FRAME F\r\nBUTTON B\r\n/FRAME\r\n",
                "F(FB)",
                3,
            ),
            (
                "/FLAGS without FLAGS (deviation)",
                "FRAME F\r\n/FLAGS\r\n/FRAME\r\n",
                "F",
                2,
            ),
            (
                "end of file in the element",
                "FRAME F\r\nLABEL A /LABEL\r\n",
                "F(FA)",
                2,
            ),
        ];
        for (name, src, tree, line) in cases {
            for opts in [MENU, NO_MENU] {
                let out = run(src, opts);
                assert_eq!(shape(&out.widgets), tree, "{name}");
                assert_eq!(out.diagnostics.len(), 1, "{name}: {:?}", out.diagnostics);
                assert_eq!(
                    out.diagnostics[0].line, line,
                    "{name}: {:?}",
                    out.diagnostics
                );
                assert!(!out.diagnostics[0].message.is_empty());
            }
        }
        // A /... word at top level ends reading; the rest stays unread.
        let out = run("FRAME F /FRAME\r\n/X\r\nFRAME G /FRAME", NO_MENU);
        assert_eq!(shape(&out.widgets), "F");
        assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
        assert_eq!(out.diagnostics[0].line, 2);
        // Empty file, file without FRAME: nothing built, no diagnostic.
        for src in ["", " \r\n", "VAR A 1"] {
            let out = run(src, NO_MENU);
            assert!(
                out.widgets.is_empty() && out.diagnostics.is_empty(),
                "{src:?}"
            );
        }
    }

    // QUIRK O-3: deviation
    #[test]
    fn huge_or_deep_files_terminate() {
        // A huge loop ends with a diagnostic at the word limit instead of filling up memory.
        let out = run(
            "FRAME F\r\nLOOP 2000000000\r\nLABEL A /LABEL\r\n/LOOP\r\n/FRAME\r\n",
            NO_MENU,
        );
        assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
        assert!(out.widgets[0].children.len() < 1_000_000);
        // Deep nesting: diagnostic instead of stack overflow.
        let src = "FRAME F ".repeat(10_000);
        let out = run(&src, NO_MENU);
        assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
        let src = "LABEL L ".repeat(10_000);
        let out = run(&src, NO_MENU);
        assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
    }
}
