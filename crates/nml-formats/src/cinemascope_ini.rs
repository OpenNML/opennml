//! Cinemascope area of the game window: `data/gifcon/cinemascope.ini` (reader `0x004479D0`, called by the
//! constructor `0x004483D0`), see re/campaign-ui D6.1.

use std::collections::BTreeMap;

use crate::config::{game_lines, left, mid, trim_left};
use crate::cscan::{atol, scan_hex, scan_int_rest};

/// Path of the cinemascope description (constructor `0x004483D0`).
pub const CINEMASCOPE_INI_PATH: &str = "data/gifcon/cinemascope.ini";

/// `BARTEXTURETOP` / `BARTEXTUREDOWN w h file`: edge tile (icon set `FUN_00445F40`, `+0x44` / `+0x48`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconStrip {
    /// Width and height of a tile in pixels.
    pub tile: [i32; 2],
    /// Image file, as written.
    pub path: String,
}

/// A block `DIALOGAREA n` ... `/DIALOGAREA` (entry of the table `+0x164`, 0x28 bytes).
///
/// Position and sizes are virtual coordinates (1280 x 1024).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogArea {
    /// `PORTRAITPOS x y`.
    pub portrait_pos: [i32; 2],
    /// `PORTRAITSIZE b h`.
    pub portrait_size: [i32; 2],
    /// `TEXTPOS x y`.
    pub text_pos: [i32; 2],
    /// `TEXTSIZE b h`; only the width has an effect (O-352).
    pub text_size: [i32; 2],
    /// `TEXTFONT`: rest of the line, not trimmed on the right. Without the line (and without an inherited value) empty:
    /// drawing takes the name unchanged, an empty name results in the fallback font (O-22).
    pub text_font: String,
    /// `TEXTCOLOR` (`%X`); read but never used (O-352).
    pub text_color: u32,
}

/// Contents of `cinemascope.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CinemascopeIni {
    /// `BARSIZE` (`+0x3C`): virtual bar height; 0 without the line, and then there are never any bars.
    pub bar_size: i32,
    /// `BARTEXTURETOP`: edge tile below the upper bar.
    pub bar_top: Option<IconStrip>,
    /// `BARTEXTUREDOWN`: edge tile on the lower bar.
    pub bar_down: Option<IconStrip>,
    /// Areas by index (CMap `+0x164`); with the same index the last block wins.
    pub areas: BTreeMap<i32, DialogArea>,
}

impl CinemascopeIni {
    /// Area 0: the original always takes `DIALOGAREA 0` (`0x00446F00`, `0x00446FC0`). If it does not exist, a pixel
    /// branch applies in the original (`FUN_004477E0` for the portrait, `0x00448D44...0x00448DFE` for the text):
    /// portrait (0, 0, `bar`, `bar`), text slot 0 at (`bar`, 0), slot 1 at (`bar`, H - `bar`), width W - 2*`bar`,
    /// font `DialogText`, with `bar` = raw value [`Self::bar_size`] as pixels. It is unreachable with the
    /// installation and belongs in the area, not in this reader.
    pub fn area0(&self) -> Option<DialogArea> {
        self.areas.get(&0).cloned()
    }
}

/// `w h file` (`0x00447BE4...0x00447D13`): `w = atol`, first space (must be at position > 0), `h = atol` after it,
/// second space (same condition), the path is everything after that. Otherwise the line has no effect.
fn parse_strip(rest: &str) -> Option<IconStrip> {
    let w = atol(rest);
    let p1 = rest.find(' ').filter(|&p| p > 0)?;
    let rest = mid(rest, p1 + 1);
    let h = atol(rest);
    let p2 = rest.find(' ').filter(|&p| p > 0)?;
    Some(IconStrip {
        tile: [w, h],
        path: mid(rest, p2 + 1).to_string(),
    })
}

/// `sscanf("%d %d")` into the buffer: a missing number leaves the old value in place (stack of the original).
fn scan_pair(text: &str, slot: &mut [i32; 2]) {
    let Some((x, tail)) = scan_int_rest(text) else {
        return;
    };
    slot[0] = x;
    if let Some((y, _)) = scan_int_rest(tail) {
        slot[1] = y;
    }
}

/// `cinemascope.ini` like the reader `0x004479D0`: lines trimmed on the left only, keys via `Left(n)` including the
/// space (case matters); unknown lines and `#` fall through.
///
/// The area buffer (stack `[ESP+0x88...0xAC]`) and the index apply across the whole file and are never reset: value
/// lines also take effect outside a block, a later block inherits every value it does not set itself (O-352);
/// `/DIALOGAREA` inserts a copy at the last read index.
///
/// Deviation: the numbers of the first block are stack contents in the original (the index without a
/// `DIALOGAREA` line likewise), the reimplementation starts at 0; the font starts empty as in the original
/// (`0x00447A63`).
pub fn parse_cinemascope_ini(text: &str) -> CinemascopeIni {
    let mut out = CinemascopeIni::default();
    let mut area = DialogArea::default();
    let mut index = 0;
    for line in game_lines(text) {
        let line = trim_left(&line);
        if left(line, 8) == "BARSIZE " {
            out.bar_size = atol(mid(line, 8));
        } else if left(line, 14) == "BARTEXTURETOP " {
            if let Some(strip) = parse_strip(mid(line, 14)) {
                out.bar_top = Some(strip);
            }
        } else if left(line, 15) == "BARTEXTUREDOWN " {
            if let Some(strip) = parse_strip(mid(line, 15)) {
                out.bar_down = Some(strip);
            }
        } else if left(line, 11) == "DIALOGAREA " {
            index = atol(mid(line, 11));
        } else if left(line, 12) == "PORTRAITPOS " {
            scan_pair(mid(line, 12), &mut area.portrait_pos);
        } else if left(line, 13) == "PORTRAITSIZE " {
            scan_pair(mid(line, 13), &mut area.portrait_size);
        } else if left(line, 8) == "TEXTPOS " {
            scan_pair(mid(line, 8), &mut area.text_pos);
        } else if left(line, 9) == "TEXTSIZE " {
            scan_pair(mid(line, 9), &mut area.text_size);
        } else if left(line, 9) == "TEXTFONT " {
            area.text_font = mid(line, 9).to_string();
        } else if left(line, 10) == "TEXTCOLOR " {
            if let Some(color) = scan_hex(mid(line, 10)) {
                area.text_color = color;
            }
        } else if line == "/DIALOGAREA" {
            out.areas.insert(index, area.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structure of the file of the installation (values from re/campaign-ui D6.1).
    const SAMPLE: &str = "#Cinemascope\r\nBARSIZE 170\r\nBARTEXTURETOP 32 32 data/icons/cinemascope_icon_oben.tga\r\nBARTEXTUREDOWN 32 32 data/icons/cinemascope_icon_unten.tga\r\nDIALOGAREA 0\r\n  PORTRAITPOS 24 876\r\n  PORTRAITSIZE 128 128\r\n  TEXTPOS 170 880\r\n  TEXTSIZE 933 124\r\n  TEXTFONT DialogText\r\n  TEXTCOLOR FFFFFFFF\r\n/DIALOGAREA\r\n";

    #[test]
    fn cinemascope_ini_reads_bars_tiles_and_area_0() {
        let c = parse_cinemascope_ini(SAMPLE);
        assert_eq!(c.bar_size, 170);
        assert_eq!(
            c.bar_top,
            Some(IconStrip {
                tile: [32, 32],
                path: "data/icons/cinemascope_icon_oben.tga".into()
            })
        );
        assert_eq!(
            c.bar_down,
            Some(IconStrip {
                tile: [32, 32],
                path: "data/icons/cinemascope_icon_unten.tga".into()
            })
        );
        assert_eq!(c.areas.len(), 1);
        assert_eq!(
            c.area0(),
            Some(DialogArea {
                portrait_pos: [24, 876],
                portrait_size: [128, 128],
                text_pos: [170, 880],
                text_size: [933, 124],
                text_font: "DialogText".into(),
                text_color: 0xFFFF_FFFF,
            })
        );
    }

    #[test]
    fn later_area_inherits_missing_values() {
        // The buffer is never reset (O-352): area 1 inherits everything it does not set itself, including the font.
        let text = format!("{SAMPLE}DIALOGAREA 1\r\nTEXTPOS 1 2\r\n/DIALOGAREA\r\n");
        let c = parse_cinemascope_ini(&text);
        let a1 = &c.areas[&1];
        assert_eq!(a1.text_pos, [1, 2]);
        assert_eq!(a1.portrait_pos, [24, 876]);
        assert_eq!(a1.portrait_size, [128, 128]);
        assert_eq!(a1.text_size, [933, 124]);
        assert_eq!(a1.text_font, "DialogText");
        assert_eq!(a1.text_color, 0xFFFF_FFFF);
        // Area 0 stays unchanged; the same index later overrides.
        assert_eq!(c.areas[&0].text_pos, [170, 880]);
        let text = format!("{SAMPLE}DIALOGAREA 0\r\nTEXTPOS 7 8\r\n/DIALOGAREA\r\n");
        let c = parse_cinemascope_ini(&text);
        assert_eq!(c.areas.len(), 1);
        assert_eq!(c.areas[&0].text_pos, [7, 8]);
    }

    #[test]
    fn area_without_textfont_has_empty_font_name() {
        // Without `TEXTFONT` the area carries "" (no `DialogText`, `0x00447A63`); missing numbers stay 0.
        let c = parse_cinemascope_ini("DIALOGAREA 0\r\nTEXTPOS 5 6\r\n/DIALOGAREA\r\n");
        let a = c.area0().unwrap();
        assert_eq!(a.text_font, "");
        assert_eq!(a.text_pos, [5, 6]);
        assert_eq!(a.text_size, [0, 0]);
        assert_eq!(a.text_color, 0);
    }

    #[test]
    fn key_needs_exact_case_and_trailing_space() {
        for text in ["barsize 170\r\n", "BARSIZE170\r\n", "BARSIZE\r\n"] {
            assert_eq!(parse_cinemascope_ini(text).bar_size, 0, "{text:?}");
        }
        // Lines are trimmed on the left; the font keeps the rest of the line including trailing spaces.
        let c = parse_cinemascope_ini(
            "  BARSIZE 170\r\n\tDIALOGAREA 0\r\nTEXTFONT Abc \r\n/DIALOGAREA\r\n",
        );
        assert_eq!(c.bar_size, 170);
        assert_eq!(c.area0().unwrap().text_font, "Abc ");
        // `/DIALOGAREA` counts only as a whole line.
        let c = parse_cinemascope_ini("DIALOGAREA 0\r\n/DIALOGAREA \r\n");
        assert!(c.areas.is_empty());
    }

    #[test]
    fn missing_barsize_means_no_bars() {
        let c = parse_cinemascope_ini("#nothing\r\nBARTEXTURETOP 32 32 a.tga\r\n");
        assert_eq!(c.bar_size, 0);
        assert_eq!(c.bar_down, None);
        // Two spaces in a row (position 0) and a missing path make the tile line have no effect.
        let c = parse_cinemascope_ini("BARTEXTURETOP  32 a.tga\r\nBARTEXTUREDOWN 32 32\r\n");
        assert_eq!((c.bar_top, c.bar_down), (None, None));
    }

    #[test]
    fn area0_is_none_without_dialogarea() {
        // Without area 0 there is nothing; the area builds the pixel branch of the original (FUN_004477E0).
        assert_eq!(parse_cinemascope_ini("BARSIZE 170\r\n").area0(), None);
        assert_eq!(
            parse_cinemascope_ini("DIALOGAREA 3\r\n/DIALOGAREA\r\n").area0(),
            None
        );
        // Value lines without `/DIALOGAREA` do not create an area.
        assert_eq!(parse_cinemascope_ini("TEXTPOS 1 2\r\n").area0(), None);
    }

    #[test]
    fn value_lines_work_outside_blocks_and_keep_old_value_on_missing_number() {
        // No block state: the values before `DIALOGAREA` belong to the buffer that `/DIALOGAREA` inserts (index 0
        // beforehand).
        let c = parse_cinemascope_ini("TEXTPOS 3 4\r\nTEXTPOS 9\r\n/DIALOGAREA\r\n");
        assert_eq!(c.area0().unwrap().text_pos, [9, 4]);
    }
}
