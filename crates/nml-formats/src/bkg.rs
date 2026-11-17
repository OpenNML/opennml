//! Frame description `.bkg` (reader `FUN_0045BB00`, `docs/formats/dlg.md`, section ".bkg").
//!
//! Line-oriented and unlike `.dlg`: the keyword is in column 0, the prefix is compared case-sensitively
//! (`CString_Left` + `_mbscmp`), the arguments are read by `sscanf`. Unknown input silently falls through. The
//! tree ([`BkgFile`]) keeps every line including its line ending so that [`write_bkg`] returns the input
//! byte-identical; [`BkgFile::desc`] evaluates it like the original.

/// A line of the file: text in Latin-1 (one `char` per byte) without line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BkgLine {
    pub text: String,
    /// `"\r\n"`, `"\n"` or `""` (last line without ending).
    pub ending: String,
}

/// Lossless line list of a `.bkg`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BkgFile {
    pub lines: Vec<BkgLine>,
}

/// Texture of the `FILE` line (`"FILE %s %d %d"`). The index in [`BkgDesc::files`] is the order of the
/// `FILE` lines; the path stays as written (not uppercased).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BkgTexture {
    pub path: String,
    pub tile_w: i32,
    pub tile_h: i32,
}

/// Tile `k` from file `f` (`TILE f k`; in the original `(k << 16) | f`). The indices are not checked against the
/// file and grid counts until drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRef {
    pub file: i32,
    pub tile: i32,
}

/// Slots of the 9-part table; the value is the index in [`BkgDesc::slots`] (line by line from the top left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    TopLeft,
    TopMid,
    TopRight,
    LeftMid,
    Fill,
    RightMid,
    BottomLeft,
    BottomMid,
    BottomRight,
}

impl Slot {
    /// All slots in index order.
    pub const ALL: [Slot; 9] = [
        Slot::TopLeft,
        Slot::TopMid,
        Slot::TopRight,
        Slot::LeftMid,
        Slot::Fill,
        Slot::RightMid,
        Slot::BottomLeft,
        Slot::BottomMid,
        Slot::BottomRight,
    ];

    /// Index in [`BkgDesc::slots`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Evaluated description (state of the original after `FUN_0045BB00`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BkgDesc {
    pub files: Vec<BkgTexture>,
    /// Tile sequence: a new line per `NEWLINE`.
    pub sequence: Vec<Vec<TileRef>>,
    pub slots: [Option<TileRef>; 9],
    /// `CLIENTAREA l t r b` (default 0).
    pub client_area: [i32; 4],
    pub fit_size: bool,
    pub top_left: Option<[i32; 2]>,
    pub bottom_right: Option<[i32; 2]>,
    pub size: Option<[i32; 2]>,
    /// `STRECHTOBG` (spelled like this).
    pub stretch: bool,
    pub alpha_test: bool,
}

/// Splits the bytes into lines. The line ending is `\n` (a preceding `\r` belongs to the ending); the last line may
/// have no ending, and no empty line is created after the last `\n`.
pub fn parse_bkg(bytes: &[u8]) -> BkgFile {
    let latin1 = |b: &[u8]| -> String { b.iter().map(|&c| c as char).collect() };
    let mut lines = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        match rest.iter().position(|&b| b == b'\n') {
            Some(i) => {
                let (body, ending) = if i > 0 && rest[i - 1] == b'\r' {
                    (&rest[..i - 1], "\r\n")
                } else {
                    (&rest[..i], "\n")
                };
                lines.push(BkgLine {
                    text: latin1(body),
                    ending: ending.to_string(),
                });
                rest = &rest[i + 1..];
            }
            None => {
                lines.push(BkgLine {
                    text: latin1(rest),
                    ending: String::new(),
                });
                break;
            }
        }
    }
    BkgFile { lines }
}

/// Writes the lines back; byte-identical to the input for unchanged files. Characters above U+00FF (only in
/// hand-built lines) become `?`.
pub fn write_bkg(file: &BkgFile) -> Vec<u8> {
    let mut out = Vec::new();
    for line in &file.lines {
        for part in [&line.text, &line.ending] {
            out.extend(part.chars().map(|c| u8::try_from(c).unwrap_or(b'?')));
        }
    }
    out
}

/// Simple replacement for `sscanf`: skip whitespace, then word or `%d`.
pub(crate) struct Scan<'a> {
    pub(crate) rest: &'a str,
}

impl<'a> Scan<'a> {
    pub(crate) fn skip_space(&mut self) {
        self.rest = self.rest.trim_start_matches(is_scan_space);
    }

    /// `%d`: whitespace, optional sign, at least one digit (32-bit with overflow like `atol`).
    pub(crate) fn int(&mut self) -> Option<i32> {
        self.skip_space();
        let mut chars = self.rest.char_indices().peekable();
        let negative = match chars.peek() {
            Some((_, '-')) => {
                chars.next();
                true
            }
            Some((_, '+')) => {
                chars.next();
                false
            }
            _ => false,
        };
        let mut value = 0i32;
        let mut digits = 0;
        let mut end = 0;
        while let Some(&(i, c)) = chars.peek() {
            let Some(d) = c.to_digit(10) else {
                break;
            };
            value = value.wrapping_mul(10).wrapping_add(d as i32);
            digits += 1;
            end = i + 1;
            chars.next();
        }
        if digits == 0 {
            return None;
        }
        self.rest = &self.rest[end..];
        Some(if negative {
            value.wrapping_neg()
        } else {
            value
        })
    }

    /// `%s`: skip whitespace, then up to the next whitespace.
    pub(crate) fn word(&mut self) -> Option<&'a str> {
        self.skip_space();
        let end = self.rest.find(is_scan_space).unwrap_or(self.rest.len());
        if end == 0 {
            return None;
        }
        let (word, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(word)
    }
}

/// Whitespace of the C runtime (`isspace`).
fn is_scan_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
}

/// `CString::Mid(start)` in characters.
fn mid(line: &str, start: usize) -> &str {
    match line.char_indices().nth(start) {
        Some((i, _)) => &line[i..],
        None => "",
    }
}

fn ints<const N: usize>(text: &str) -> Option<[i32; N]> {
    let mut scan = Scan { rest: text };
    let mut out = [0; N];
    for value in &mut out {
        *value = scan.int()?;
    }
    Some(out)
}

impl BkgFile {
    /// Evaluates the lines like `FUN_0045BB00` (`0x0045BB00`...`0x0045C892`, `docs/formats/dlg.md`, section ".bkg").
    ///
    /// Every line is read like `GameFile_ReadLine` (`0x00574C00`) does (`\r` is dropped everywhere). The original
    /// checks the keywords in the order `ALPHATEST`, `FILE`, `TILE`, `NEWLINE`, `TOPLEFT`, `TOPRIGHT`, `BOTTOMLEFT`,
    /// `BOTTOMRIGHT`, `LEFTMID`, `RIGHTMID`, `TOPMID`, `BOTTOMMID`, `FILLTILE`, `CLIENTAREA`, `FITSIZETOBACKGROUND`,
    /// `SETTOPLEFTPOS`, `SETBOTTOMRIGHTPOS`, `SETSIZE`, `STRECHTOBG` (strings `0x0061A144`...`0x0061A230`); no word is
    /// a prefix of another, so the order does not change the result. `FILE` and `TILE` read the whole line with the
    /// format `"FILE %s %d %d"` and `"TILE %d %d"` respectively, all others the characters from position word
    /// length + 1.
    ///
    /// Deviation: if `sscanf` fails (too few numbers), the original would keep the stack values of the previous
    /// line; here the line is skipped.
    pub fn desc(&self) -> BkgDesc {
        let mut d = BkgDesc::default();
        // The sequence consists of lines; `open` is true as long as the last line accepts tiles.
        let mut open = false;
        for line in &self.lines {
            let text: String = line.text.chars().filter(|&c| c != '\r').collect();
            let text = text.split('\0').next().unwrap_or("");
            if text.starts_with("ALPHATEST") {
                d.alpha_test = true;
            } else if let Some(rest) = text.strip_prefix("FILE") {
                let mut scan = Scan { rest };
                if let (Some(path), Some(tile_w), Some(tile_h)) =
                    (scan.word(), scan.int(), scan.int())
                {
                    d.files.push(BkgTexture {
                        path: path.to_string(),
                        tile_w,
                        tile_h,
                    });
                }
            } else if let Some(rest) = text.strip_prefix("TILE") {
                if let Some([file, tile]) = ints(rest) {
                    if !open {
                        d.sequence.push(Vec::new());
                        open = true;
                    }
                    if let Some(row) = d.sequence.last_mut() {
                        row.push(TileRef { file, tile });
                    }
                }
            } else if text.starts_with("NEWLINE") {
                if !open {
                    d.sequence.push(Vec::new());
                }
                open = false;
            } else if let Some((slot, key)) = SLOT_KEYS.iter().find(|(_, k)| text.starts_with(k)) {
                if let Some([file, tile]) = ints(mid(text, key.len() + 1)) {
                    d.slots[slot.index()] = Some(TileRef { file, tile });
                }
            } else if text.starts_with("CLIENTAREA") {
                if let Some(v) = ints(mid(text, "CLIENTAREA".len() + 1)) {
                    d.client_area = v;
                }
            } else if text.starts_with("FITSIZETOBACKGROUND") {
                d.fit_size = true;
            } else if text.starts_with("SETTOPLEFTPOS") {
                if let Some(v) = ints(mid(text, "SETTOPLEFTPOS".len() + 1)) {
                    d.top_left = Some(v);
                }
            } else if text.starts_with("SETBOTTOMRIGHTPOS") {
                if let Some(v) = ints(mid(text, "SETBOTTOMRIGHTPOS".len() + 1)) {
                    d.bottom_right = Some(v);
                }
            } else if text.starts_with("SETSIZE") {
                if let Some(v) = ints(mid(text, "SETSIZE".len() + 1)) {
                    d.size = Some(v);
                }
            } else if text.starts_with("STRECHTOBG") {
                d.stretch = true;
            }
        }
        d
    }
}

/// Keywords of the 9-part table (`0x0061A144`...).
const SLOT_KEYS: [(Slot, &str); 9] = [
    (Slot::TopLeft, "TOPLEFT"),
    (Slot::TopRight, "TOPRIGHT"),
    (Slot::BottomLeft, "BOTTOMLEFT"),
    (Slot::BottomRight, "BOTTOMRIGHT"),
    (Slot::LeftMid, "LEFTMID"),
    (Slot::RightMid, "RIGHTMID"),
    (Slot::TopMid, "TOPMID"),
    (Slot::BottomMid, "BOTTOMMID"),
    (Slot::Fill, "FILLTILE"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(file: i32, tile: i32) -> TileRef {
        TileRef { file, tile }
    }

    // QUIRK O-11, O-12: kept
    #[test]
    fn bkg_reads_slots_sequence_and_client_area() {
        let text = "FILE x.tga 21 21\r\nTILE 0 1\r\nNEWLINE\r\nTILE 0 2\nTOPLEFT 0 0\r\n\
                    FILE y.tga 8 4\r\nFILLTILE 1 4\r\nCLIENTAREA 15 30 15 30\r\nALPHATEST\r\nFITSIZETOBACKGROUND\r\n\
                    SETTOPLEFTPOS 1 2\r\nSETBOTTOMRIGHTPOS 3 4\r\nSETSIZE 5 6\r\nSTRECHTOBG";
        let file = parse_bkg(text.as_bytes());
        let d = file.desc();
        assert_eq!(
            d.files,
            [
                BkgTexture {
                    path: "x.tga".into(),
                    tile_w: 21,
                    tile_h: 21
                },
                BkgTexture {
                    path: "y.tga".into(),
                    tile_w: 8,
                    tile_h: 4
                }
            ]
        );
        assert_eq!(d.sequence, [vec![tile(0, 1)], vec![tile(0, 2)]]);
        assert_eq!(d.slots[Slot::TopLeft.index()], Some(tile(0, 0)));
        assert_eq!(d.slots[Slot::Fill.index()], Some(tile(1, 4)));
        assert_eq!(d.slots.iter().flatten().count(), 2);
        assert_eq!(d.client_area, [15, 30, 15, 30]);
        assert!(d.alpha_test && d.fit_size && d.stretch);
        assert_eq!(d.top_left, Some([1, 2]));
        assert_eq!(d.bottom_right, Some([3, 4]));
        assert_eq!(d.size, Some([5, 6]));
        // Round trip byte-identical: CRLF, LF and last line without ending.
        assert_eq!(write_bkg(&file), text.as_bytes());
    }

    #[test]
    fn all_nine_slot_keywords_fill_their_place() {
        let names = [
            "TOPLEFT",
            "TOPMID",
            "TOPRIGHT",
            "LEFTMID",
            "FILLTILE",
            "RIGHTMID",
            "BOTTOMLEFT",
            "BOTTOMMID",
            "BOTTOMRIGHT",
        ];
        let text: String = names
            .iter()
            .enumerate()
            .map(|(i, n)| format!("{n} 0 {i}\n"))
            .collect();
        let d = parse_bkg(text.as_bytes()).desc();
        for (i, slot) in d.slots.iter().enumerate() {
            assert_eq!(*slot, Some(tile(0, i as i32)), "slot {i}");
        }
        for (i, slot) in Slot::ALL.iter().enumerate() {
            assert_eq!(slot.index(), i);
        }
    }

    #[test]
    fn round_trip_keeps_odd_line_endings() {
        for raw in [
            &b""[..],
            b"\n",
            b"\r\n\r\n",
            b"a\r",
            b"a\r\nb\nc",
            b"a\r\r\nb\n\n",
            b"FILE \xfc.tga 1 1\r\n",
        ] {
            assert_eq!(write_bkg(&parse_bkg(raw)), raw, "{raw:?}");
        }
        assert_eq!(parse_bkg(b"a\r\nb").lines.len(), 2);
        assert!(parse_bkg(b"").lines.is_empty());
        assert_eq!(parse_bkg(b"a\n").lines.len(), 1);
    }

    // QUIRK O-12: kept
    #[test]
    fn bkg_ignores_bad_lines_and_indices() {
        let text = " TOPLEFT 0 0\ntopleft 0 0\nTOPLEFTX 1 1\nUNKNOWN 1 2\n# Comment\n\nTILEX 3 4\n\
                    FILE only.tga\nTILE 7 99\nTILE a b\nCLIENTAREA 1 2 3\nSETSIZE 4\n";
        let d = parse_bkg(text.as_bytes()).desc();
        // Prefix comparison (`FUN_0045BB00`): `TOPLEFTX` acts like `TOPLEFT`; the arguments begin one
        // character after the word.
        assert_eq!(d.slots[Slot::TopLeft.index()], Some(tile(1, 1)));
        assert_eq!(d.slots.iter().flatten().count(), 1);
        // `TILEX 3 4` is not read by `sscanf("TILE %d %d")`; faulty arguments are skipped
        // (deviation: the original would keep stack values).
        assert!(d.files.is_empty());
        // A tile index outside the files/the grid stays; it is not checked until drawing.
        assert_eq!(d.sequence, [vec![tile(7, 99)]]);
        assert_eq!(d.client_area, [0, 0, 0, 0]);
        assert_eq!(d.size, None);
    }

    // QUIRK O-12: kept
    #[test]
    fn keywords_are_prefixes_and_stray_cr_is_dropped() {
        // `CString_Left(n) == word`: ALPHATESTX counts as ALPHATEST; a `\r` in the middle of the line is dropped
        // as in `GameFile_ReadLine` (`0x00574C00`).
        let d = parse_bkg(b"ALPHATESTX\nTILE 0\r 1\n").desc();
        assert!(d.alpha_test);
        assert_eq!(d.sequence, [vec![tile(0, 1)]]);
        let d = parse_bkg(b"NEW\rLINE\nTILE 0 1\n").desc();
        assert_eq!(d.sequence, [vec![], vec![tile(0, 1)]]);
        // NEWLINE as prefix of NEWLINEX; two NEWLINEs give an empty line, a trailing one gives none.
        let d = parse_bkg(b"TILE 0 1\nNEWLINEX\nNEWLINE\nTILE 0 2\nNEWLINE\n").desc();
        assert_eq!(d.sequence, [vec![tile(0, 1)], vec![], vec![tile(0, 2)]]);
    }

    #[test]
    fn later_lines_override_and_signs_are_read() {
        let d = parse_bkg(b"CLIENTAREA 1 1 1 1\nCLIENTAREA -4 5 +6 0\nTOPLEFT -1 2\n").desc();
        assert_eq!(d.client_area, [-4, 5, 6, 0]);
        assert_eq!(d.slots[0], Some(tile(-1, 2)));
    }
}
