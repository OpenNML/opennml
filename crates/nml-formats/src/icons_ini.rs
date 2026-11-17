//! Portraits and icons of the command bar: `data/gifcon/icons.ini` (reader `0x004553B0`, called
//! by the constructor of the command bar `0x00456630`), see `docs/formats/gifcon-misc.md` and
//! re/game-ui 2.1.

use std::collections::BTreeMap;

use crate::config::{game_lines, left, mid};
use crate::cscan::scan_ints;

/// Path of the icon mapping (string `0x00619FD0`).
pub const ICONS_INI_PATH: &str = "data/gifcon/icons.ini";
/// Edge length of a portrait tile (`PORTRAIT` sheets, `0x00455546`).
pub const PORTRAIT_TILE: i32 = 80;
/// Edge length of an icon tile (`ICONS` sheets, `0x00455616`).
pub const ICON_TILE: i32 = 50;

/// `GUID g pb p ib i`: sheet and tile of portrait and icon of a GUID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuidIcons {
    /// Number of the portrait sheet (order of the `PORTRAIT` lines).
    pub portrait_sheet: i32,
    /// Tile in the portrait sheet.
    pub portrait: i32,
    /// Number of the icon sheet (order of the `ICONS` lines).
    pub icon_sheet: i32,
    /// Tile in the icon sheet.
    pub icon: i32,
}

/// Contents of `icons.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IconsIni {
    /// `PORTRAIT file`: sheets of 80 x 80 ([`PORTRAIT_TILE`]) in file order.
    pub portrait_sheets: Vec<String>,
    /// `ICONS file`: sheets of 50 x 50 ([`ICON_TILE`]) in file order.
    pub icon_sheets: Vec<String>,
    /// Entry per GUID (map `+0x530`; a later line of the same GUID replaces the values).
    pub guids: BTreeMap<i32, GuidIcons>,
}

/// `icons.ini` like the reader `0x004553B0`: prefixes via `CString_Left` and `__mbscmp`
/// (case matters, no trimming) in the order `PORTRAIT` (8), `ICONS` (5),
/// `GUID` (4) - without a space in the key; `Mid` cuts off the separator as well (9, 6, 5).
/// `GUID` reads `sscanf("%d %d %d %d %d")`: GUID, portrait sheet, portrait, icon sheet, icon.
///
/// Deviation: the original leaves missing numbers undetermined (return value of
/// `sscanf` unchecked), the reimplementation takes 0.
pub fn parse_icons_ini(text: &str) -> IconsIni {
    let mut out = IconsIni::default();
    for line in game_lines(text) {
        let line = line.as_str();
        if left(line, 8) == "PORTRAIT" {
            out.portrait_sheets.push(mid(line, 9).to_string());
        } else if left(line, 5) == "ICONS" {
            out.icon_sheets.push(mid(line, 6).to_string());
        } else if left(line, 4) == "GUID" {
            let [guid, portrait_sheet, portrait, icon_sheet, icon] = scan_ints::<5>(mid(line, 5));
            out.guids.insert(
                guid,
                GuidIcons {
                    portrait_sheet,
                    portrait,
                    icon_sheet,
                    icon,
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#List\r\nPORTRAIT data/icons/a_80x80.tga\r\nPORTRAIT data/icons/b_80x80.tga\r\nICONS data/icons/s.tga\r\n[GUID] [POTRAITFILE#]\r\n#Hitpoints\r\nGUID 20 4 0 0 3\r\nGUID 500 2 30 0 3\r\nGUID 500 1 2 3 4\r\n";

    #[test]
    fn icons_ini_maps_guids_to_sheets() {
        let c = parse_icons_ini(SAMPLE);
        assert_eq!(
            c.portrait_sheets,
            ["data/icons/a_80x80.tga", "data/icons/b_80x80.tga"]
        );
        assert_eq!(c.icon_sheets, ["data/icons/s.tga"]);
        assert_eq!(
            c.guids[&20],
            GuidIcons {
                portrait_sheet: 4,
                portrait: 0,
                icon_sheet: 0,
                icon: 3
            }
        );
        // A later line of the same GUID replaces the values.
        assert_eq!(
            c.guids[&500],
            GuidIcons {
                portrait_sheet: 1,
                portrait: 2,
                icon_sheet: 3,
                icon: 4
            }
        );
        assert_eq!(c.guids.len(), 2);
    }

    #[test]
    fn missing_numbers_default_to_zero_and_case_counts() {
        let c = parse_icons_ini("GUID 7 1 2\r\nguid 8 1 1 1 1\r\nportrait x.tga\r\n");
        assert_eq!(
            c.guids[&7],
            GuidIcons {
                portrait_sheet: 1,
                portrait: 2,
                icon_sheet: 0,
                icon: 0
            }
        );
        assert_eq!(c.guids.len(), 1);
        assert!(c.portrait_sheets.is_empty());
    }
}
