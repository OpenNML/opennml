//! PAUSE lettering of the visible pause (re/game-ui N.4 and D5.5): at the end of `GameEngine_RenderWorld`
//! (`0x004390F9...0x0043916D`), when `Engine+0x24E5` is set, text 15788 in the font `CountDown` (`Engine+0x2710`),
//! white, centered without shadow and without tags; also in cinemascope and with the game menu open. Whether and when
//! to draw is decided by the host.

use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::text::{atlas_codes, draw_string, measure};

/// Font of the lettering (`Engine+0x2710`).
pub const PAUSE_FONT: &str = "CountDown";
/// Text "PAUSE" (`0x3DAC`).
pub const PAUSE_TEXT: i32 = 15788;
/// Color `0xFFFFFFFF`.
const PAUSE_COLOR: u32 = 0xFFFF_FFFF;

/// Draws `text` (the host fetches text [`PAUSE_TEXT`]) in [`PAUSE_FONT`] at `(W/2 - b/2, H/2 - h/2)`: `W`, `H` the
/// screen size (`Renderer+0xA4/+0xA8`), `b`, `h` the dimensions of the text (`0x00428450`), each part halved with C
/// division (direction 0). Without text layout and shadow (`0x00428390`); tags appear literally.
pub fn draw(ctx: &mut UiContext<'_>, text: &str, out: &mut Vec<DrawCmd>) {
    let font = ctx.fonts.by_name(PAUSE_FONT);
    let codes = atlas_codes(text);
    let [w, h] = measure(&codes, font, ctx.fonts);
    // `i32 /` rounds toward 0 like C.
    let x = ctx.screen.width / 2 - w / 2;
    let y = ctx.screen.height / 2 - h / 2;
    draw_string(&codes, [x, y], PAUSE_COLOR, font, ctx.fonts, out);
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nml_formats::localisation::{Localisation, parse_localisation};

    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::coords::Screen;
    use crate::fonts::FontBook;
    use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};

    /// Font `CountDown`: every character 25 px wide, line height 31 (both odd so that the halving counts).
    fn fonts() -> &'static FontBook {
        static FONTS: OnceLock<FontBook> = OnceLock::new();
        FONTS.get_or_init(|| {
            let vfs = test_vfs(&[
                (
                    "data/fonts/Test_15_0_0_0_0.fnt",
                    fnt_text(|_| 25, |_| 31).into_bytes(),
                ),
                (
                    "data/fonts/Test_15_0_0_0_0.tga",
                    tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]),
                ),
            ]);
            let mut assets = UiAssets::new();
            let loc =
                parse_localisation("Font CountDown\nFontSize 15\nTextureFont\nFontName Test\n");
            FontBook::load(&vfs, &loc, 1024, &mut assets, None)
        })
    }

    fn draw_at(text: &str, width: i32, height: i32) -> Vec<DrawCmd> {
        let vfs = test_vfs(&[]);
        let mut assets = UiAssets::new();
        let texts = Texts::load(&vfs, &Localisation::default());
        let mut ctx = UiContext {
            vfs: &vfs,
            assets: &mut assets,
            fonts: fonts(),
            texts: &texts,
            screen: Screen { width, height },
        };
        let mut out = Vec::new();
        draw(&mut ctx, text, &mut out);
        out
    }

    /// Top-left corner and color of the character quads.
    fn quads(cmds: &[DrawCmd]) -> Vec<([i32; 2], u32)> {
        cmds.iter()
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

    /// N.4: PAUSE centered at `(W/2 - b/2, H/2 - h/2)` with C division, white, one quad per character, without
    /// shadow; tags stay literal.
    #[test]
    fn pause_text_is_centered_with_c_division() {
        // b = 5 * 25 = 125 -> 62, h = 31 -> 15.
        let at_1024 = quads(&draw_at("PAUSE", 1024, 768));
        assert_eq!(at_1024.len(), 5, "one quad per character, no shadow");
        assert_eq!(at_1024[0], ([512 - 62, 384 - 15], 0xFFFF_FFFF));
        assert_eq!(at_1024[4].0, [450 + 4 * 25, 369]);
        let at_800 = quads(&draw_at("PAUSE", 800, 600));
        assert_eq!(at_800[0], ([400 - 62, 300 - 15], 0xFFFF_FFFF));
        // Odd screen size: 801/2 = 400 (toward 0), not rounded.
        assert_eq!(quads(&draw_at("PAUSE", 801, 601))[0].0, [338, 285]);
        // Wider than the screen: x negative, halved toward 0 (-62 from 125/2, 50 from 101/2).
        assert_eq!(quads(&draw_at("PAUSE", 101, 31))[0].0, [50 - 62, 0]);
        // Without text layout: `[COLOR ...]` appears literally.
        assert_eq!(quads(&draw_at("[B]X", 1024, 768)).len(), 4);
    }
}
