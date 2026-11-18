//! `data/txt/localisation.<language>` (reader `FUN_00560420`) and text tables (`FUN_005601B0`).
//!
//! Basis: re/ui-rendering 3.1 and 3.8.6.

use std::collections::BTreeMap;

use crate::bkg::Scan;
use crate::text::decode_cp1252;

/// Font size: fixed in pixels or relative (`B * n / 1280`, B = width of the backbuffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSize {
    Fixed(i32),
    Relative(i32),
}

/// One font (closed by `FontName`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontDef {
    pub name: String,
    pub size: FontSize,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike_out: bool,
    pub texture_font: bool,
    pub face: String,
}

/// One icon set (closed by `IconsSize`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSetDef {
    pub name: String,
    pub path: String,
    pub size: [i32; 2],
}

/// Content of a `localisation.*`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Localisation {
    pub text_files: Vec<String>,
    pub fonts: Vec<FontDef>,
    pub icons: Vec<IconSetDef>,
    pub blood_mode: bool,
}

/// Bits of `Font+0x20` (re/ui-rendering 3.1).
const RELATIVE: u32 = 0x01;
const ITALIC: u32 = 0x02;
const UNDERLINE: u32 = 0x04;
const STRIKE_OUT: u32 = 0x08;
const WEIGHT: u32 = 0x10;
const TEXTURE_FONT: u32 = 0x20;

/// Lines like `GameFile_ReadLine` (`0x00574C00`): `\n` separates, `\r` is dropped everywhere, an empty line at the
/// end of the file does not count.
fn game_lines(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut pieces = text.split('\n').peekable();
    std::iter::from_fn(move || {
        let piece = pieces.next()?;
        let line: String = piece.chars().filter(|&c| c != '\r').collect();
        if pieces.peek().is_none() && line.is_empty() {
            return None;
        }
        Some(line)
    })
}

/// `CString::Mid(start)` in characters.
fn mid(line: &str, start: usize) -> &str {
    match line.char_indices().nth(start) {
        Some((i, _)) => &line[i..],
        None => "",
    }
}

/// Adds an item or replaces the one of the same name in its old place (`CMap`: the last entry per name wins).
fn put<T>(list: &mut Vec<T>, item: T, same: impl Fn(&T) -> bool) {
    match list.iter_mut().find(|x| same(x)) {
        Some(slot) => *slot = item,
        None => list.push(item),
    }
}

/// Reads a `localisation.*` like `FUN_00560420` (`0x00560420`, re/ui-rendering 3.1).
///
/// Line by line; no trimming, no comment character (`#...` matches no key). `BloodModeEnabled` counts only as a
/// whole line (`_mbscmp`, string `0x006322B0`); all other keys count as a prefix, case-sensitive
/// (`CString_Left(n)` + `_mbscmp`/`CString_EqualsMbs`), in this order, length in parentheses, strings from
/// `0x0063221C`: `TextDatei`(9) `0x006322A4`, `FontName`(8) `0x00632298`,
/// `FontSize`(8) `0x00632284`, `RelativeFontSize`(16) `0x00632270`, `"Weight "`(7, the space is part of the
/// key) `0x00632268`, `Italic`(6) `0x00632260`, `Underline`(9) `0x00632254`, `StrikeOut`(9)
/// `0x00632248`, `TextureFont`(11) `0x0063223C`, `Font`(4) `0x006185BC`, `IconsName`(9) `0x00632230`,
/// `IconsSize`(9) `0x00632224`, `Icons`(5) `0x0063221C`. The argument is the rest of the line from the position
/// key length + 1 (`CString::Mid`); numbers are read by `sscanf("%d")`.
///
/// The state of the reader persists between lines (local variables of the original):
/// - `FontSize` and `RelativeFontSize` write the same value field; only `RelativeFontSize` sets the
///   relative bit (O-85). `Weight` writes the value and bit `0x10`.
/// - `Font <Name>` sets the name and clears **only the bits**; size and weight as numbers stay.
/// - `FontName <Face>` closes the font: length >= 32 yields `Arial`; the font enters the table under the
///   current `Font` name (`FUN_00445CC0`), an identical name replaces the earlier one. The
///   weight counts only with bit `0x10` (`FUN_00428860`), otherwise 0.
/// - `Icons <Name>` and `IconsName <Path>` remember name and path, `IconsSize w h` creates the set
///   (`FUN_00445F40`).
///
/// Deviation: default values that the original never sets (stack garbage): size 10 (like the fallback font),
/// weight 0, icon size 0 x 0; if `%d` fails, the previous value stays.
pub fn parse_localisation(text: &str) -> Localisation {
    let mut out = Localisation::default();
    // Reader state.
    let mut font_name = String::new();
    let mut flags = 0u32;
    let mut size_value = 10;
    let mut weight_value = 0;
    let mut icons_name = String::new();
    let mut icons_path = String::new();
    let mut icons_size = [0, 0];

    let read_int = |arg: &str, value: &mut i32| {
        if let Some(v) = (Scan { rest: arg }).int() {
            *value = v;
        }
    };

    for line in game_lines(text) {
        let line = line.as_str();
        if line == "BloodModeEnabled" {
            out.blood_mode = true;
        } else if line.starts_with("TextDatei") {
            out.text_files.push(mid(line, 10).to_string());
        } else if line.starts_with("FontName") {
            let arg = mid(line, 9);
            let face = if arg.chars().count() >= 32 {
                "Arial".to_string()
            } else {
                arg.to_string()
            };
            let def = FontDef {
                name: font_name.clone(),
                size: if flags & RELATIVE != 0 {
                    FontSize::Relative(size_value)
                } else {
                    FontSize::Fixed(size_value)
                },
                weight: if flags & WEIGHT != 0 { weight_value } else { 0 },
                italic: flags & ITALIC != 0,
                underline: flags & UNDERLINE != 0,
                strike_out: flags & STRIKE_OUT != 0,
                texture_font: flags & TEXTURE_FONT != 0,
                face,
            };
            put(&mut out.fonts, def, |f| f.name == font_name);
        } else if line.starts_with("FontSize") {
            read_int(mid(line, 9), &mut size_value);
        } else if line.starts_with("RelativeFontSize") {
            read_int(mid(line, 17), &mut size_value);
            flags |= RELATIVE;
        } else if line.starts_with("Weight ") {
            read_int(mid(line, 7), &mut weight_value);
            flags |= WEIGHT;
        } else if line.starts_with("Italic") {
            flags |= ITALIC;
        } else if line.starts_with("Underline") {
            flags |= UNDERLINE;
        } else if line.starts_with("StrikeOut") {
            flags |= STRIKE_OUT;
        } else if line.starts_with("TextureFont") {
            flags |= TEXTURE_FONT;
        } else if line.starts_with("Font") {
            font_name = mid(line, 5).to_string();
            flags = 0;
        } else if line.starts_with("IconsName") {
            icons_path = mid(line, 10).to_string();
        } else if line.starts_with("IconsSize") {
            let mut scan = Scan {
                rest: mid(line, 10),
            };
            if let Some(w) = scan.int() {
                icons_size[0] = w;
                if let Some(h) = scan.int() {
                    icons_size[1] = h;
                }
            }
            let set = IconSetDef {
                name: icons_name.clone(),
                path: icons_path.clone(),
                size: icons_size,
            };
            put(&mut out.icons, set, |s| s.name == icons_name);
        } else if line.starts_with("Icons") {
            icons_name = mid(line, 6).to_string();
        }
    }
    out
}

/// Lines of the text table are limited to 4,096 characters (`FUN_005601B0`: loop `iVar6 < 0x1000`).
const MAX_TEXT_LINE: usize = 0x1000;

/// Text table like `FUN_005601B0` (`0x005601B0`, re/ui-rendering 3.8.6): UTF-16LE with BOM `FF FE`, otherwise
/// `None` (the original aborts). Line separators are `\r` and `\n` in any order; a line is limited to
/// 4,096 characters, the rest is read as a separate line (O-83). Without `=` the line is
/// ignored; the number is read by `swscanf("%d")` at the start of the line, **if that fails, the number of the
/// previous line applies** (default before the first: 0, undetermined in the original). The text is everything
/// after the first `=` without trimming; a null character ends the line (`wcschr`, `wcscpy`). Later lines with the
/// same number replace earlier ones.
///
/// Deviation: after the file, the original still reads as many characters of an unwritten
/// buffer half (`2 * file length` bytes allocated, `file length` characters traversed); here the
/// text ends with the file. An odd last byte is dropped.
pub fn parse_text_file(bytes: &[u8]) -> Option<BTreeMap<i32, String>> {
    let body = bytes.strip_prefix(&[0xFF, 0xFE])?;
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let is_separator = |u: u16| u == 0x0D || u == 0x0A;
    let mut table = BTreeMap::new();
    let mut number = 0i32;
    let mut pos = 0;
    while pos < units.len() {
        let start = pos;
        while pos < units.len() && pos - start < MAX_TEXT_LINE && !is_separator(units[pos]) {
            pos += 1;
        }
        let mut line = &units[start..pos];
        while pos < units.len() && is_separator(units[pos]) {
            pos += 1;
        }
        if let Some(nul) = line.iter().position(|&u| u == 0) {
            line = &line[..nul];
        }
        let Some(eq) = line.iter().position(|&u| u == u16::from(b'=')) else {
            continue;
        };
        let head = String::from_utf16_lossy(&line[..eq]);
        if let Some(v) = (Scan { rest: &head }).int() {
            number = v;
        }
        table.insert(number, String::from_utf16_lossy(&line[eq + 1..]));
    }
    Some(table)
}

/// Path of the language selection (`0x0061F1C8`, re/ui-rendering 3.1).
const SELECT_LANGUAGE: &str = "data/txt/selectlanguage.ini";
/// Default without language selection (German installation).
const DEFAULT_LOCALISATION: &str = "data/txt/localisation.deu";

/// Reads the `localisation.*` of the selected language (`FUN_004C9290`, re/ui-rendering 3.1): path = first
/// non-empty line of `data/txt/selectlanguage.ini` (trimmed), otherwise `data/txt/localisation.deu`; bytes as
/// Windows-1252. `read` returns the bytes of a game file (archive or disk) or `None`.
pub fn load_localisation(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Localisation, String> {
    let path = read(SELECT_LANGUAGE)
        .and_then(|bytes| {
            decode_cp1252(&bytes)
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_LOCALISATION.to_string());
    let bytes = read(&path).ok_or_else(|| format!("language file \"{path}\" unreadable"))?;
    Ok(parse_localisation(&decode_cp1252(&bytes)))
}

/// Merges the text tables from `loc.text_files` in this order; later numbers replace earlier ones. A missing
/// or BOM-less file is ignored with a warning (second return value, in English; the original then aborts
/// reading that file).
pub fn merge_text_tables(
    loc: &Localisation,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> (BTreeMap<i32, String>, Vec<String>) {
    let mut table = BTreeMap::new();
    let mut warnings = Vec::new();
    for path in &loc.text_files {
        match read(path) {
            Some(bytes) => match parse_text_file(&bytes) {
                Some(entries) => table.extend(entries),
                None => warnings.push(format!(
                    "text table \"{path}\" does not begin with the byte sequence FF FE"
                )),
            },
            None => warnings.push(format!("text table \"{path}\" unreadable")),
        }
    }
    (table, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE];
        out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        out
    }

    fn table(text: &str) -> BTreeMap<i32, String> {
        parse_text_file(&utf16(text)).unwrap()
    }

    // QUIRK O-83: kept
    #[test]
    fn text_file_follows_the_original() {
        // Without BOM: abort.
        assert_eq!(parse_text_file(b"1\0=\0a\0"), None);
        assert_eq!(parse_text_file(b""), None);
        assert_eq!(parse_text_file(&[0xFF]), None);
        assert_eq!(parse_text_file(&[0xFF, 0xFE]), Some(BTreeMap::new()));
        // Separators \r, \n, \r\n in any order; empty lines; a line without `=` is ignored.
        let t = table("1=a\r2=b\n3=c\r\n\r\n\n\r4=d\r\n#comment\r\n*x\r\n5");
        assert_eq!(
            t.into_iter().collect::<Vec<_>>(),
            [
                (1, "a".to_string()),
                (2, "b".to_string()),
                (3, "c".to_string()),
                (4, "d".to_string())
            ]
        );
        // Text after the first `=`, untrimmed; number with whitespace and sign.
        let t = table("20= a = b \r\n123=\r\n -5=x\r\n  7 =y");
        assert_eq!(t[&20], " a = b ");
        assert_eq!(t[&123], "");
        assert_eq!(t[&-5], "x");
        assert_eq!(t[&7], "y");
        // No escape sequences.
        assert_eq!(table("1=a\\nb")[&1], "a\\nb");
    }

    // QUIRK O-83: kept
    #[test]
    fn a_line_without_a_number_overwrites_the_previous_one() {
        // O-83: `swscanf` fails, the number of the previous line applies.
        let t = table("7=a\r\nx=y\r\n=z\r\n8=b\r\n-=w");
        assert_eq!(t[&7], "z");
        assert_eq!(t[&8], "w");
        assert_eq!(t.len(), 2);
        // Duplicate number: the last one wins.
        assert_eq!(table("1=a\r\n1=b")[&1], "b");
    }

    // QUIRK O-83: kept
    #[test]
    fn long_lines_are_split_after_4096_characters() {
        let long = "9=".to_string() + &"a".repeat(5000);
        let t = table(&format!("{long}\r\n3=x"));
        assert_eq!(t[&9].chars().count(), 4094);
        assert_eq!(t[&3], "x");
        // The rest (906 characters `a`) has no `=` and is dropped; with a `=` in it, it becomes a separate line.
        let long = "9=".to_string() + &"a".repeat(4094) + "5=rest";
        let t = table(&long);
        assert_eq!(t[&9].len(), 4094);
        assert_eq!(t[&5], "rest");
        // Exactly 4096 characters: nothing is lost.
        let exact = "9=".to_string() + &"a".repeat(4094);
        assert_eq!(table(&exact)[&9].len(), 4094);
    }

    #[test]
    fn nul_ends_a_line_text() {
        let mut bytes = utf16("1=ab");
        bytes.extend([0, 0]);
        bytes.extend("cd".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(parse_text_file(&bytes).unwrap()[&1], "ab");
    }

    // QUIRK O-64: kept
    #[test]
    fn localisation_follows_the_reader() {
        let text = "# Comment\r\nTextDatei data\\txt\\a.deu\r\nTextDatei data\\txt\\b.deu\r\n\
                    BloodModeEnabled\r\n\
                    Font MenuButton\r\nRelativeFontSize 27\r\nWeight 600\r\nTextureFont\r\nFontName Garamond\r\n\
                    Font Credits\r\nFontSize 15\r\nItalic\r\nUnderline\r\nStrikeOut\r\nFontName Times New Roman\r\n\
                    Icons INFOICONS\r\nIconsName data\\gifcon\\x.tga\r\nIconsSize 16 16\r\n";
        let l = parse_localisation(text);
        assert_eq!(l.text_files, ["data\\txt\\a.deu", "data\\txt\\b.deu"]);
        assert!(l.blood_mode);
        assert_eq!(
            l.fonts,
            [
                FontDef {
                    name: "MenuButton".into(),
                    size: FontSize::Relative(27),
                    weight: 600,
                    italic: false,
                    underline: false,
                    strike_out: false,
                    texture_font: true,
                    face: "Garamond".into(),
                },
                // `Font` clears only the switches: size 27 and weight 600 stay as numbers, the
                // relative and the weight bit do not; `FontSize` sets no switch.
                FontDef {
                    name: "Credits".into(),
                    size: FontSize::Fixed(15),
                    weight: 0,
                    italic: true,
                    underline: true,
                    strike_out: true,
                    texture_font: false,
                    face: "Times New Roman".into(),
                },
            ]
        );
        assert_eq!(
            l.icons,
            [IconSetDef {
                name: "INFOICONS".into(),
                path: "data\\gifcon\\x.tga".into(),
                size: [16, 16]
            }]
        );
    }

    // QUIRK O-85: kept
    #[test]
    fn relative_bit_stays_until_font_resets_it() {
        // O-85: `FontSize` and `RelativeFontSize` write the same field, only `RelativeFontSize` sets
        // the relative bit, only `Font` clears the bits.
        let l = parse_localisation(
            "Font A\nRelativeFontSize 27\nFontSize 15\nFontName X\n\
             Font B\nFontName Y\n\
             Font C\nFontSize 9\nWeight 700\nFontName Z\n\
             Font D\nFontName W\n",
        );
        let sizes: Vec<_> = l.fonts.iter().map(|f| (f.size, f.weight)).collect();
        assert_eq!(
            sizes,
            [
                (FontSize::Relative(15), 0),
                // `Font B` cleared the bits, the number 15 stays.
                (FontSize::Fixed(15), 0),
                (FontSize::Fixed(9), 700),
                // Weight 700 stays as a number, but does not count without the `Weight` bit.
                (FontSize::Fixed(9), 0),
            ]
        );
    }

    // QUIRK O-85: kept
    #[test]
    fn keys_are_prefixes_with_case_and_column_zero() {
        let l = parse_localisation(
            "Font F\nFontSize 12\n fontname No\nfontname small\nFontNameX Arial\nFontName  Two\n\
             Weight 5\nWeight5 6\nBloodModeEnabled \nTextDateiX data\\x\n",
        );
        // `FontNameX Arial` counts as `FontName` (prefix), argument from character 9: `Arial`.
        // `FontName  Two` with two spaces: argument ` Two`.
        assert_eq!(l.fonts.len(), 1);
        assert_eq!(l.fonts[0].name, "F");
        assert_eq!(l.fonts[0].face, " Two");
        // `BloodModeEnabled ` with a space is not equal; `TextDateiX` prefix: argument from character 10.
        assert!(!l.blood_mode);
        assert_eq!(l.text_files, [" data\\x"]);
    }

    // QUIRK O-85: kept
    #[test]
    fn weight_needs_its_space_and_long_face_names_fall_back_to_arial() {
        let l = parse_localisation("Font A\nWeight5 6\nWeight 7\nFontName X\n");
        assert_eq!(l.fonts[0].weight, 7);
        let l = parse_localisation("Font A\nWeight5 6\nFontName X\n");
        assert_eq!(l.fonts[0].weight, 0);
        let long31 = "A".repeat(31);
        let long32 = "A".repeat(32);
        let l = parse_localisation(&format!(
            "Font L\nFontName {long31}\nFont M\nFontName {long32}\n"
        ));
        assert_eq!(l.fonts[0].face, long31);
        assert_eq!(l.fonts[1].face, "Arial");
    }

    #[test]
    fn a_font_needs_fontname_and_an_icon_set_needs_iconssize() {
        let l = parse_localisation(
            "Font A\nFontSize 3\nIcons I\nIconsName p\nFont B\nFontName X\nIcons J\nIconsSize 8 4\n\
             IconsSize 2 2\n",
        );
        assert_eq!(l.fonts.len(), 1);
        assert_eq!(l.fonts[0].name, "B");
        // Every `IconsSize` line creates a set with the current name and path; an identical name
        // replaces the earlier one (`CMap`).
        assert_eq!(
            l.icons,
            [IconSetDef {
                name: "J".into(),
                path: "p".into(),
                size: [2, 2]
            }]
        );
    }

    #[test]
    fn same_name_replaces_the_earlier_definition() {
        let l = parse_localisation(
            "Font A\nFontSize 1\nFontName X\nFont B\nFontName Y\nFont A\nFontSize 2\nFontName Z\n",
        );
        assert_eq!(
            l.fonts
                .iter()
                .map(|f| (f.name.as_str(), f.face.as_str()))
                .collect::<Vec<_>>(),
            [("A", "Z"), ("B", "Y")]
        );
    }

    #[test]
    fn lines_split_like_game_file_read_line() {
        // `\r` is dropped everywhere, `\n` separates; the empty line at the end does not count.
        let l = parse_localisation("TextDatei a\r\r\nTextDatei b\nTextDatei c");
        assert_eq!(l.text_files, ["a", "b", "c"]);
    }

    /// Read function over a fixed file list (paths as in the game, `/` as separator).
    fn reader(files: &[(&str, Vec<u8>)]) -> impl Fn(&str) -> Option<Vec<u8>> + use<> {
        let files: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(p, b)| (p.to_string(), b.clone()))
            .collect();
        move |path| {
            let path = path.replace('\\', "/");
            files
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, b)| b.clone())
        }
    }

    #[test]
    fn localisation_path_comes_from_selectlanguage_ini() {
        let loc_text = "TextDatei data\\txt\\x.deu\nFontName Arial\n";
        let read = reader(&[
            ("data/txt/localisation.deu", loc_text.as_bytes().to_vec()),
            (
                "data/txt/localisation.eng",
                b"TextDatei data\\txt\\y.eng\n".to_vec(),
            ),
            (
                "data/txt/selectlanguage.ini",
                b"\r\n  data\\txt\\localisation.eng \r\nsecond\r\n".to_vec(),
            ),
        ]);
        let loc = load_localisation(&read).unwrap();
        assert_eq!(loc.text_files, ["data\\txt\\y.eng"]);

        // Without a selection file or with an empty file: German default.
        for files in [
            vec![("data/txt/localisation.deu", loc_text.as_bytes().to_vec())],
            vec![
                ("data/txt/localisation.deu", loc_text.as_bytes().to_vec()),
                ("data/txt/selectlanguage.ini", b"\r\n  \r\n".to_vec()),
            ],
        ] {
            let read = reader(&files);
            assert_eq!(
                load_localisation(&read).unwrap().text_files,
                ["data\\txt\\x.deu"]
            );
        }
    }

    #[test]
    fn localisation_errors_are_german() {
        let read = reader(&[(
            "data/txt/selectlanguage.ini",
            b"data\\txt\\doesnotexist.xyz\n".to_vec(),
        )]);
        let err = load_localisation(&read).unwrap_err();
        assert!(err.contains("doesnotexist.xyz"), "{err}");
        assert!(err.contains("unreadable"), "{err}");
        let read = reader(&[("data/other.txt", b"x".to_vec())]);
        let err = load_localisation(&read).unwrap_err();
        assert!(err.contains("data/txt/localisation.deu"), "{err}");
    }

    #[test]
    fn merge_text_tables_later_files_replace_earlier() {
        let read = reader(&[
            ("data/txt/one.deu", utf16("# Comment\r\n1=One\r\n2=Two\r\n")),
            ("data/txt/two.deu", utf16("2=Two new\r\n3=Three\r\n")),
            ("data/txt/broken.deu", b"1=without BOM".to_vec()),
        ]);
        let loc = parse_localisation(
            "TextDatei data\\txt\\one.deu\nTextDatei data\\txt\\missing.deu\nTextDatei data\\txt\\broken.deu\nTextDatei data\\txt\\two.deu\n",
        );
        let (table, warnings) = merge_text_tables(&loc, &read);
        assert_eq!(table.get(&1).map(String::as_str), Some("One"));
        assert_eq!(table.get(&2).map(String::as_str), Some("Two new"));
        assert_eq!(table.get(&3).map(String::as_str), Some("Three"));
        assert_eq!(table.get(&4), None);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("missing.deu"), "{warnings:?}");
        assert!(warnings[1].contains("broken.deu"), "{warnings:?}");
    }
}
