//! Status bar of the game window: `data/gifcon/statusbar.ini` (reader `0x00451F80`, called by the
//! constructor `0x00452720`), see `docs/formats/gifcon-misc.md` and re/game-ui 2.2, N.4,
//! N.13.

use crate::config::{game_lines, left, mid};
use crate::cscan::{atol, scan_ints};

/// Path of the status bar description (string `0x00619D5C`).
pub const STATUSBAR_PATH: &str = "data/gifcon/statusbar.ini";

/// `RESOURCE i ix iy tx ty` or `PLAYERNAME ix iy tx ty` (index -1): position of icon and text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusEntry {
    /// Resource number; `-1` for the player name.
    pub index: i32,
    /// Position of the icon in the bar (pixels).
    pub icon: [i32; 2],
    /// Position of the text in the bar (pixels).
    pub text: [i32; 2],
}

/// Content of `statusbar.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusBarConfig {
    /// `STATUSSIZE`: height of the bar (`+0x78`).
    pub size: i32,
    /// `ICONSIZE w h`: size of an icon (`+0x70/+0x74`).
    pub icon_size: [i32; 2],
    /// `ICONFILE`: icon sheet, as written (`+0x48`).
    pub icon_file: String,
    /// `RESOURCE` and `PLAYERNAME` lines in file order (list `+0x54`).
    pub entries: Vec<StatusEntry>,
    /// `STATUSCOLOR`: background `0x80RRGGBB` (`+0x40`).
    pub color: u32,
    /// `STATUSFONTCOLOR`: font `0xFFRRGGBB` (`+0x3C`).
    pub font_color: u32,
}

/// Color word like the reader: The file is in **b g r** (comment "Aussehen b,g,r"); the **third**
/// value becomes the red byte, the first the blue byte. Each value enters only with `& 0xFF`, the
/// alpha byte is fixed (`0x00451F80`, composition at `0x004525BD` and `0x00452691`).
fn pack_color(alpha: u32, values: [i32; 3]) -> u32 {
    let [b, g, r] = values.map(|v| (v & 0xFF) as u32);
    alpha << 24 | r << 16 | g << 8 | b
}

/// `statusbar.ini` like the reader `0x00451F80`: per line a pure prefix comparison with
/// `CString_Left(n)` and `__mbscmp` (case matters, no trimming, comments match
/// nothing), in this order `STATUSSIZE`, `ICONSIZE`, `ICONFILE`, `RESOURCE`, `PLAYERNAME`,
/// `STATUSCOLOR`, `STATUSFONTCOLOR`; the rest after the key and one separator character
/// (`Mid(n + 1)`) is read with `atol` or `sscanf`. Later lines of the same kind
/// overwrite earlier ones, `RESOURCE`/`PLAYERNAME` append.
///
/// Deviation: The original has no defaults and never checks the return value of
/// `sscanf` - missing lines or numbers stayed heap or stack content. The reimplementation takes 0
/// or empty; the shipped data has all lines.
pub fn parse_statusbar(text: &str) -> StatusBarConfig {
    let mut out = StatusBarConfig::default();
    for line in game_lines(text) {
        let line = line.as_str();
        if left(line, 10) == "STATUSSIZE" {
            out.size = atol(mid(line, 11));
        } else if left(line, 8) == "ICONSIZE" {
            out.icon_size = scan_ints::<2>(mid(line, 9));
        } else if left(line, 8) == "ICONFILE" {
            out.icon_file = mid(line, 9).to_string();
        } else if left(line, 8) == "RESOURCE" {
            let [index, ix, iy, tx, ty] = scan_ints::<5>(mid(line, 9));
            out.entries.push(StatusEntry {
                index,
                icon: [ix, iy],
                text: [tx, ty],
            });
        } else if left(line, 10) == "PLAYERNAME" {
            let [ix, iy, tx, ty] = scan_ints::<4>(mid(line, 11));
            out.entries.push(StatusEntry {
                index: -1,
                icon: [ix, iy],
                text: [tx, ty],
            });
        } else if left(line, 11) == "STATUSCOLOR" {
            out.color = pack_color(0x80, scan_ints::<3>(mid(line, 12)));
        } else if left(line, 15) == "STATUSFONTCOLOR" {
            out.font_color = pack_color(0xFF, scan_ints::<3>(mid(line, 16)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#Header\r\nSTATUSSIZE 26\r\nSTATUSCOLOR 2 22 35\r\nSTATUSFONTCOLOR 255 255 255\r\nICONSIZE 32 32\r\nICONFILE data/icons/resourcen_icons.tga\r\nRESOURCE 0 1 1 33 5\r\nRESOURCE 1 120 1 153 5\r\nPLAYERNAME 480 1 480 5\r\n";

    // QUIRK O-38: kept
    #[test]
    fn statusbar_reads_colors_like_the_original() {
        // Third value = red byte (file in b g r), per value only `& 0xFF`, alpha fixed.
        let c = parse_statusbar("STATUSCOLOR 10 20 30\r\nSTATUSFONTCOLOR 300 1 2\r\n");
        assert_eq!(c.color, 0x801E_140A);
        assert_eq!(c.font_color, 0xFF02_012C);
        let c = parse_statusbar("STATUSCOLOR -1 256 511\r\n");
        assert_eq!(c.color, 0x80FF_00FF);
        assert_eq!(parse_statusbar(SAMPLE).color, 0x8023_1602);
        assert_eq!(parse_statusbar(SAMPLE).font_color, 0xFFFF_FFFF);
    }

    #[test]
    fn statusbar_reads_size_icons_and_entries() {
        let c = parse_statusbar(SAMPLE);
        assert_eq!(
            (c.size, c.icon_size, c.icon_file.as_str()),
            (26, [32, 32], "data/icons/resourcen_icons.tga")
        );
        assert_eq!(c.entries.len(), 3);
        assert_eq!(
            c.entries[1],
            StatusEntry {
                index: 1,
                icon: [120, 1],
                text: [153, 5]
            }
        );
    }

    #[test]
    fn playername_entry_has_index_minus_one() {
        let c = parse_statusbar(SAMPLE);
        assert_eq!(
            c.entries[2],
            StatusEntry {
                index: -1,
                icon: [480, 1],
                text: [480, 5]
            }
        );
    }

    #[test]
    fn keys_are_case_sensitive_untrimmed_prefixes() {
        // Case matters, indentation matches nothing, a prefix suffices
        // (`Mid` cuts off one character as the separator).
        let c = parse_statusbar("statussize 5\r\n STATUSSIZE 6\r\nSTATUSSIZEX 7\r\n");
        assert_eq!(c.size, 7);
        let c = parse_statusbar("STATUSSIZE 5\r\nSTATUSSIZE 9\r\n");
        assert_eq!(c.size, 9, "the last line wins");
    }

    // QUIRK O-251: deviation
    #[test]
    fn missing_lines_and_numbers_default_to_zero() {
        assert_eq!(parse_statusbar(""), StatusBarConfig::default());
        let c = parse_statusbar("ICONSIZE 16\r\nRESOURCE 2 7\r\nSTATUSCOLOR 1 2\r\n");
        assert_eq!(c.icon_size, [16, 0]);
        assert_eq!(
            c.entries,
            [StatusEntry {
                index: 2,
                icon: [7, 0],
                text: [0, 0]
            }]
        );
        assert_eq!(c.color, 0x8000_0201);
    }
}
