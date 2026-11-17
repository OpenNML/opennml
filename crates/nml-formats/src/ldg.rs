//! Loading screen `.ldg` (`docs/formats/ldg.md`, re/singleplayer-menus part 2, 6.2).
//!
//! Read like in `LoadingScreen_ParseLdg` (`0x004BE690`): line by line, every line trimmed on the left,
//! key at the beginning of the line.

use crate::cscan::{C_SPACE, game_file_lines, scan_hex, scan_int, scan_int_rest};

/// Initial values of the text block template per call (`0x004BE690`): `TextGuid 20012`, color white,
/// `TextExtent 20 20 800 600`, font `LoadingFont` (`0x0061ED84`).
const TEMPLATE_TEXT_ID: i32 = 20012;
const TEMPLATE_EXTENT: [i32; 4] = [20, 20, 800, 600];
const TEMPLATE_FONT: &str = "LoadingFont";
const OPAQUE_WHITE: u32 = 0xFFFF_FFFF;

/// A text block of the loading screen (0x118 bytes in the original).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdgTextBlock {
    /// `TextExtent x0 y0 x1 y1` (coordinates of the 1280x1024 template).
    pub extent: [i32; 4],
    /// `TextGuid`: text ID of the text table.
    pub text_id: i32,
    /// `TextColor` (`AARRGGBB`).
    pub color: u32,
    /// `TextFont`: font name (`LoadingFont`, `LoadingFontKapitel`, `InfoFont`).
    pub font: String,
}

/// The contents of an `.ldg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ldg {
    /// `LoadingScreen`: background image.
    pub image: Option<String>,
    /// `LoadingTimber`: image of the progress bar.
    pub timber: Option<String>,
    /// `LoadingRect x0 y0 x1 y1` of the bar; 0 without the line.
    pub rect: [i32; 4],
    /// `LoadingColor` of the bar (`AARRGGBB`), default `0xFFFFFFFF`.
    pub color: u32,
    /// The text blocks in file order.
    pub blocks: Vec<LdgTextBlock>,
}

/// Reads an `.ldg` (`LoadingScreen_ParseLdg`).
///
/// The keys need the space and the exact spelling (`_mbscmp`); `StartTextBlock` and `EndTextBlock` apply only as a
/// whole line. Numbers are read by `sscanf` (`%d`, hex `%X` case-insensitive); if reading fails, the old value
/// stays. `StartTextBlock` does nothing; `EndTextBlock` appends the **template** that `TextExtent`, `TextGuid`,
/// `TextColor` and `TextFont` change - fields not mentioned inherit from the previous block (O-136).
pub fn parse_ldg(text: &str) -> Ldg {
    let mut ldg = Ldg {
        image: None,
        timber: None,
        rect: [0; 4],
        color: OPAQUE_WHITE,
        blocks: Vec::new(),
    };
    let mut template = LdgTextBlock {
        extent: TEMPLATE_EXTENT,
        text_id: TEMPLATE_TEXT_ID,
        color: OPAQUE_WHITE,
        font: TEMPLATE_FONT.to_string(),
    };
    for line in game_file_lines(text) {
        let line = line.trim_start_matches(C_SPACE);
        if let Some(rest) = line.strip_prefix("LoadingScreen ") {
            ldg.image = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("LoadingTimber ") {
            ldg.timber = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("LoadingRect ") {
            scan_four_ints(rest, &mut ldg.rect);
        } else if let Some(rest) = line.strip_prefix("LoadingColor ") {
            set_hex(rest, &mut ldg.color);
        } else if line == "StartTextBlock" {
            // Does nothing (O-136).
        } else if line == "EndTextBlock" {
            ldg.blocks.push(template.clone());
        } else if let Some(rest) = line.strip_prefix("TextExtent ") {
            scan_four_ints(rest, &mut template.extent);
        } else if let Some(rest) = line.strip_prefix("TextGuid ") {
            if let Some(id) = scan_int(rest) {
                template.text_id = id;
            }
        } else if let Some(rest) = line.strip_prefix("TextColor ") {
            set_hex(rest, &mut template.color);
        } else if let Some(rest) = line.strip_prefix("TextFont ") {
            template.font = rest.to_string();
        }
    }
    ldg
}

/// `sscanf("%d %d %d %d")`: as many values as can be read, the rest stays.
fn scan_four_ints(text: &str, target: &mut [i32; 4]) {
    let mut rest = text;
    for slot in target {
        let Some((value, after)) = scan_int_rest(rest) else {
            return;
        };
        *slot = value;
        rest = after;
    }
}

/// `sscanf("%X")`: on failure the value stays.
fn set_hex(text: &str, target: &mut u32) {
    if let Some(value) = scan_hex(text) {
        *target = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(extent: [i32; 4], text_id: i32, color: u32, font: &str) -> LdgTextBlock {
        LdgTextBlock {
            extent,
            text_id,
            color,
            font: font.to_string(),
        }
    }

    #[test]
    fn ldg_blocks_inherit_fields() {
        // `StartTextBlock` resets nothing (O-136): the second block inherits `TextFont` and `TextColor`
        // of the first, it changes `TextGuid` and `TextExtent`.
        let text = "\
LoadingScreen data/gfx/a.tga\r\nLoadingTimber data/gfx/b.tga\r\nLoadingRect 834 690 1072 1120\r\nLoadingColor ffff00ff\r\n\
\r\nStartTextBlock\r\nTextExtent 705 847 1200 1011\r\nTextGuid 27060\r\nTextColor aa463910\r\nTextFont InfoFont\r\nEndTextBlock\r\n\
\r\nStartTextBlock\r\nTextExtent 1 2 3 4\r\nTextGuid 27061\r\nEndTextBlock\r\n\
StartTextBlock\r\nEndTextBlock";
        let ldg = parse_ldg(text);
        assert_eq!(ldg.image.as_deref(), Some("data/gfx/a.tga"));
        assert_eq!(ldg.timber.as_deref(), Some("data/gfx/b.tga"));
        assert_eq!(ldg.rect, [834, 690, 1072, 1120]);
        assert_eq!(ldg.color, 0xFFFF_00FF);
        assert_eq!(
            ldg.blocks,
            vec![
                block([705, 847, 1200, 1011], 27060, 0xAA46_3910, "InfoFont"),
                block([1, 2, 3, 4], 27061, 0xAA46_3910, "InfoFont"),
                // End of file without a line ending: the last block counts anyway; it inherits everything.
                block([1, 2, 3, 4], 27061, 0xAA46_3910, "InfoFont"),
            ]
        );
    }

    #[test]
    fn ldg_defaults_without_keys() {
        let ldg = parse_ldg("");
        assert_eq!(ldg.image, None);
        assert_eq!(ldg.timber, None);
        assert_eq!(ldg.color, 0xFFFF_FFFF);
        assert_eq!(ldg.rect, [0, 0, 0, 0]);
        assert!(ldg.blocks.is_empty());
        // A block without lines carries the template of the original.
        let ldg = parse_ldg("StartTextBlock\nEndTextBlock\n");
        assert_eq!(
            ldg.blocks,
            vec![block([20, 20, 800, 600], 20012, 0xFFFF_FFFF, "LoadingFont")]
        );
    }

    #[test]
    fn ldg_keys_follow_the_original_matching() {
        // The line start is trimmed on the left; keys need the space and the exact spelling;
        // `StartTextBlock`/`EndTextBlock` apply only to the whole line; hex case-insensitive;
        // a number that cannot be read leaves the value in place.
        let text = "\
  \tLoadingScreen data/gfx/x.tga\nloadingtimber data/gfx/y.tga\nLoadingRect 1 2\nLoadingColor 80FFFFFF\n\
EndTextBlock \nTextGuid zz\nTextColor ZZ\nTextExtent 5 6 7 8\nEndTextBlock\n";
        let ldg = parse_ldg(text);
        assert_eq!(ldg.image.as_deref(), Some("data/gfx/x.tga"));
        assert_eq!(ldg.timber, None);
        // `%d %d %d %d` with two numbers: the two read ones apply, the rest stays.
        assert_eq!(ldg.rect, [1, 2, 0, 0]);
        assert_eq!(ldg.color, 0x80FF_FFFF);
        assert_eq!(
            ldg.blocks,
            vec![block([5, 6, 7, 8], 20012, 0xFFFF_FFFF, "LoadingFont")]
        );
    }

    #[test]
    fn every_parse_starts_from_the_template() {
        // The template applies per call: a second call sees nothing of the first.
        let first = parse_ldg("TextGuid 5\nStartTextBlock\nEndTextBlock\n");
        assert_eq!(first.blocks[0].text_id, 5);
        let second = parse_ldg("StartTextBlock\nEndTextBlock\n");
        assert_eq!(second.blocks[0].text_id, 20012);
    }
}
