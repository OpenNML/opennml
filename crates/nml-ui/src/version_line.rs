//! Version line in the bottom right corner (`FUN_004BFCC0`, re/ui-rendering 2.3, re/game-ui 1.2).
//!
//! The original formats `"%s"` with the hard-coded string `"V1.1"` (`0x0061EE00`/`0x0061EE04`, `FUN_004BFAC0`),
//! measures the text (`FUN_00428600`) and draws it at (W - width, H - height), flush in the bottom right corner
//! (`FUN_004287C0`), color `0xC8FFFFFF`, font `TextFont`, in fixed pixels. The host decides when it appears (O-440:
//! only without a visible world and without video; always on the loading screen).
//!
//! **Deviation:** The reimplementation writes its own identifier [`VERSION_TEXT`] instead of `V1.1`.

use crate::coords::Screen;
use crate::draw::DrawCmd;
use crate::fonts::FontBook;
use crate::text::{atlas_codes, draw_string, measure};

/// Text of the version line: `OpenNML v` and the version of the workspace (`[workspace.package]` in `Cargo.toml`).
pub const VERSION_TEXT: &str = concat!("OpenNML v", env!("CARGO_PKG_VERSION"));

/// Color of the version line (`0xC8FFFFFF`: white, alpha 200; `FUN_004BFCC0`).
pub const VERSION_COLOR: u32 = 0xC8FF_FFFF;

/// Font of the version line (`TextFont`, Garamond 16; `FUN_004C9290` -> `GameApp+0x1754`).
pub const VERSION_FONT: &str = "TextFont";

/// Appends the version line `text` to `out`: measured with [`measure`] (width = sum of the advances, height =
/// `max_height`), drawn from (W - width, H - height) with [`draw_string`].
pub fn draw_version_line(text: &str, screen: Screen, fonts: &FontBook, out: &mut Vec<DrawCmd>) {
    let font = fonts.by_name(VERSION_FONT);
    let codes = atlas_codes(text);
    let [w, h] = measure(&codes, font, fonts);
    let pos = [screen.width.wrapping_sub(w), screen.height.wrapping_sub(h)];
    draw_string(&codes, pos, VERSION_COLOR, font, fonts, out);
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nml_formats::localisation::parse_localisation;

    use super::*;
    use crate::assets::UiAssets;
    use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};

    /// Font `TextFont`: every character 9 px wide, line height 17.
    fn fonts() -> &'static FontBook {
        static FONTS: OnceLock<FontBook> = OnceLock::new();
        FONTS.get_or_init(|| {
            let vfs = test_vfs(&[
                (
                    "data/fonts/Test_16_0_0_0_0.fnt",
                    fnt_text(|_| 9, |_| 17).into_bytes(),
                ),
                (
                    "data/fonts/Test_16_0_0_0_0.tga",
                    tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]),
                ),
            ]);
            let mut assets = UiAssets::new();
            let loc =
                parse_localisation("Font TextFont\nFontSize 16\nTextureFont\nFontName Test\n");
            FontBook::load(&vfs, &loc, 1024, &mut assets, None)
        })
    }

    /// Top left corner and color of the character quads.
    fn quads(text: &str, width: i32, height: i32) -> Vec<([i32; 2], u32)> {
        let mut out = Vec::new();
        draw_version_line(text, Screen { width, height }, fonts(), &mut out);
        out.iter()
            .map(|c| match *c {
                DrawCmd::Quad {
                    texture: Some(_),
                    dst,
                    color,
                    ..
                } => ([dst[0], dst[1]], color),
                ref other => panic!("unexpected command {other:?}"),
            })
            .collect()
    }

    /// `FUN_004BFCC0`: flush bottom right at (W - width, H - height), color `0xC8FFFFFF`, one quad per character
    /// without a shadow, in fixed pixels (the position follows the screen size, the font does not).
    #[test]
    fn version_line_sits_flush_in_the_bottom_right_corner() {
        // Width 4 * 9 = 36, height 17.
        let at_1024 = quads("V1.1", 1024, 768);
        assert_eq!(at_1024.len(), 4, "one quad per character, no shadow");
        assert_eq!(at_1024[0], ([1024 - 36, 768 - 17], 0xC8FF_FFFF));
        assert_eq!(at_1024[3].0, [1024 - 9, 751]);
        let at_800 = quads("V1.1", 800, 600);
        assert_eq!(at_800[0].0, [800 - 36, 600 - 17]);
    }

    /// The reimplementation writes `OpenNML v` and the workspace version instead of `V1.1`.
    #[test]
    fn version_text_names_opennml_and_the_workspace_version() {
        assert_eq!(
            VERSION_TEXT,
            format!("OpenNML v{}", env!("CARGO_PKG_VERSION"))
        );
        let n = VERSION_TEXT.chars().count();
        let line = quads(VERSION_TEXT, 1024, 768);
        assert_eq!(line.len(), n);
        assert_eq!(line[0].0, [1024 - 9 * n as i32, 751]);
    }
}
