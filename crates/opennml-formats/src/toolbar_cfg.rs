//! Position of the command bar: `data/gifcon/toolbar.cfg` (reader `0x00455F30`, called by the constructor
//! of the command bar `0x00456630`), see `docs/formats/gifcon-misc.md` and re/game-ui 2.1.

use std::collections::BTreeMap;

use crate::config::{game_lines, left, mid};
use crate::cscan::{atol, scan_hex, scan_ints};

/// Path of the bar description (string `0x00619FE8`).
pub const TOOLBAR_CFG_PATH: &str = "data/gifcon/toolbar.cfg";

/// `ICON x y sheet tile`: one image of the bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolbarIcon {
    /// Position relative to the bar (pixels).
    pub pos: [i32; 2],
    /// Number of the sheet (order of the `ICONS` lines, without range check, as in the original).
    pub sheet: i32,
    /// Tile in the sheet.
    pub tile: i32,
}

/// A `TAG name ... /TAG` block. The original stores `ICON`, `POS` and `COLOR` in **one** list of
/// four-word entries (`ICON`: x y sheet tile, `POS`: x y 0 0, `COLOR`: 0 0 color 0); the
/// reimplementation separates them by kind, keeping the order within each kind. In the shipped data each
/// block has only one kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolbarTag {
    pub icons: Vec<ToolbarIcon>,
    /// `POS x y`.
    pub positions: Vec<[i32; 2]>,
    /// `COLOR AARRGGBB` (`%X`).
    pub colors: Vec<u32>,
}

/// Content of `toolbar.cfg`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolbarConfig {
    /// `ICONS w h file` in file order: file and tile size (list `+0x178` of the bar).
    pub sheets: Vec<(String, [i32; 2])>,
    /// Blocks by name (map `+0x15C`; a later block of the same name replaces the earlier one).
    pub tags: BTreeMap<String, ToolbarTag>,
}

/// Remainder after the first space; without a space the whole text (`Mid(Find(' ') + 1)` with
/// `Find` = -1).
fn after_space(text: &str) -> &str {
    text.find(' ').map_or(text, |i| &text[i + 1..])
}

/// `toolbar.cfg` like the reader `0x00455F30`: prefixes via `CString_Left` and `__mbscmp`
/// (case counts, no trimming), in the order `ICONS `, `TAG `, `/TAG`
/// (whole line), `ICON `, `POS `, `COLOR `.
///
/// - `ICONS w h file`: two `atol` with a space search in between, the remainder is the file.
/// - `TAG name` starts a block; only `/TAG` enters it under the name (a block without
///   `/TAG` is lost). Lines after `/TAG` continue to go to the most recently started block.
/// - `ICON`/`POS`/`COLOR` before the first `TAG` write to an invalid pointer in the original;
///   the reimplementation skips them.
///
/// Deviation: the original leaves missing numbers undefined (return value of
/// `sscanf` unchecked), the reimplementation takes 0.
pub fn parse_toolbar_cfg(text: &str) -> ToolbarConfig {
    let mut out = ToolbarConfig::default();
    // All started blocks (the objects of the original) and the name of the last `TAG` line.
    let mut arena: Vec<ToolbarTag> = Vec::new();
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    let mut name = String::new();
    for line in game_lines(text) {
        let line = line.as_str();
        if left(line, 6) == "ICONS " {
            let rest = mid(line, 6);
            let width = atol(rest);
            let rest = after_space(rest);
            let height = atol(rest);
            out.sheets
                .push((after_space(rest).to_string(), [width, height]));
        } else if left(line, 4) == "TAG " {
            name = mid(line, 4).to_string();
            arena.push(ToolbarTag::default());
        } else if line == "/TAG" {
            if let Some(last) = arena.len().checked_sub(1) {
                by_name.insert(name.clone(), last);
            }
        } else if left(line, 5) == "ICON " {
            let [x, y, sheet, tile] = scan_ints::<4>(mid(line, 5));
            if let Some(tag) = arena.last_mut() {
                tag.icons.push(ToolbarIcon {
                    pos: [x, y],
                    sheet,
                    tile,
                });
            }
        } else if left(line, 4) == "POS " {
            let [x, y] = scan_ints::<2>(mid(line, 4));
            if let Some(tag) = arena.last_mut() {
                tag.positions.push([x, y]);
            }
        } else if left(line, 6) == "COLOR " {
            let color = scan_hex(mid(line, 6)).unwrap_or(0);
            if let Some(tag) = arena.last_mut() {
                tag.colors.push(color);
            }
        }
    }
    out.tags = by_name
        .into_iter()
        .map(|(name, index)| (name, arena[index].clone()))
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#Header\r\nICONS 128 128 data/icons/status_a_00.tga\r\nICONS 64 64 data/icons/status_b_01.tga\r\n\r\nTAG TOOLBAR\r\nICON -2 -128 0 0\r\nICON -2 126 1 3\r\n/TAG\r\n\r\nTAG NORMALBUTTONS\r\nPOS 245 17\r\nPOS 298\t17\r\n/TAG\r\nTAG BUTTONNORMALCOLORS\r\nCOLOR FF0000FF\r\nCOLOR ff00ff00\r\n/TAG\r\n";

    #[test]
    fn toolbar_tags_collect_icons_positions_colors() {
        let c = parse_toolbar_cfg(SAMPLE);
        assert_eq!(
            c.sheets,
            [
                ("data/icons/status_a_00.tga".to_string(), [128, 128]),
                ("data/icons/status_b_01.tga".to_string(), [64, 64])
            ]
        );
        assert_eq!(c.tags.len(), 3);
        assert_eq!(
            c.tags["TOOLBAR"].icons,
            [
                ToolbarIcon {
                    pos: [-2, -128],
                    sheet: 0,
                    tile: 0
                },
                ToolbarIcon {
                    pos: [-2, 126],
                    sheet: 1,
                    tile: 3
                }
            ]
        );
        assert_eq!(c.tags["NORMALBUTTONS"].positions, [[245, 17], [298, 17]]);
        assert_eq!(
            c.tags["BUTTONNORMALCOLORS"].colors,
            [0xFF00_00FF, 0xFF00_FF00]
        );
    }

    #[test]
    fn a_block_needs_its_end_line_and_a_later_block_of_the_same_name_wins() {
        let c = parse_toolbar_cfg(
            "TAG A\r\nPOS 1 2\r\n/TAG\r\nTAG A\r\nPOS 3 4\r\n/TAG\r\nTAG LOST\r\nPOS 5 6\r\nPOS 9 9\r\n",
        );
        assert_eq!(c.tags.len(), 1);
        assert_eq!(c.tags["A"].positions, [[3, 4]]);
    }

    #[test]
    fn lines_outside_a_block_and_case_are_ignored() {
        let c = parse_toolbar_cfg("POS 1 2\r\ntag A\r\nPOS 1 2\r\n/TAG\r\n pos 1 2\r\n");
        assert!(c.tags.is_empty());
    }

    #[test]
    fn sheet_without_a_second_space_keeps_the_whole_rest() {
        // `Find(' ')` = -1 -> `Mid(0)`: the remainder is the file.
        let c = parse_toolbar_cfg("ICONS 16 file.tga\r\nICONS 7\r\n");
        assert_eq!(c.sheets[0], ("file.tga".to_string(), [16, 0]));
        // Without any further space the original reads the same remainder three times.
        assert_eq!(c.sheets[1], ("7".to_string(), [7, 7]));
    }
}
