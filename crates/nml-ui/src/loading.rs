//! Loading screen from the `.ldg` as draw commands.
//!
//! Basis: re/singleplayer-menus part 2, 6.1-6.3 (file choice, drawing, progress), quirks E10 to E13 there;
//! `docs/formats/ldg.md`. The drawing is `FUN_004C08E0` (`0x004C08E0`) with the bar `FUN_004026A0` (`0x004026A0`), read
//! in Ghidra:
//!
//! - **Background** (`GameApp+0xD6F4`, texture A8R8G8B8): state block `Map` (`0x0061C418`), quad
//!   `(0, 0)...(W, H)` with UV `(0, 0)...(1, 1)` - the whole image stretched over the whole screen, neither
//!   cropped nor tiled -, filter LINEAR (no mip), no alpha blending, diffuse white.
//! - **Bar** (`GameApp+0xD6F0`, `FUN_00445F40(path, 256, 256, 1)`): state block `billboard`, quad
//!   `(x0, y0)...(right edge, y1)` with UV `(0, 0)...(1, 1)` over the **whole texture**; `FUN_004026A0` does not
//!   evaluate the 256 x 256 grid of the image object (`FUN_00401DF0`). So the image is squeezed to the respective
//!   width, not uncovered. Filter LINEAR, addressing CLAMP, diffuse = `LoadingColor`, blending
//!   `SRCALPHA/ONE` ([`Blend::Additive`], O-194).
//! - **Text** per block of the list in file order (`CObList::AddTail`, head `GameApp+0xD70C`): `FUN_00562770`
//!   lays the text out to the width `x1 - x0` with the flags `0x46`, `FUN_0055FD60(origin (x0, y0), no
//!   shadow)` draws it.

use nml_formats::ldg::Ldg;

use crate::coords::Screen;
use crate::dialog::UiContext;
use crate::draw::{Blend, DrawCmd, Filter, TextureId};
use crate::text::{LayoutParams, TextLayout, draw_runs, layout_flags, layout_text};

/// Replacement loading screen for levels without their own `.ldg` (`0x004C875C`, `0x004C79A0`).
pub const DEFAULT_LDG: &str = "data/gifcon/default.ldg";
/// Loading screen for multiplayer maps without their own `.ldg` (`0x004C8749`).
pub const MULTIPLAYER_LDG: &str = "data/gifcon/multiplayer.ldg";
/// Loading screen when restarting a random map (`FUN_00547560` from the generator `FUN_00540C30` with
/// `RM+0x58 = 0`, O-431); when loading a savegame [`DEFAULT_LDG`] (a deviation from O-431).
pub const RANDOM_MAP_LDG: &str = "data/gifcon/randommap.ldg";

/// Flags of the text layout in the loading screen (`uStack_2c8 = 0x46`, `0x004C1263`): start font, start color,
/// wrap to the width (needed for `[CENTER]`).
const TEXT_FLAGS: u32 = 0x46;
/// Diffuse of the background (`0xFFFFFFFF`, `0x004C0942`).
const WHITE: u32 = 0xFFFF_FFFF;

/// Path of the `.ldg` for a level (`Game_StartLevel` `0x004C870A`-`0x004C875C`, `Game_LoadLevelFromSavegame`
/// `0x004C7984`/`0x004C79A0`; re/singleplayer-menus 6.1, 6.4).
///
/// First `<level path without the last 4 characters>.ldg` (`0x0061ED90`, also with an extension other than `.lvl`).
/// If the file is missing (`exists` receives the path exactly like this): on a restart (`from_save == false`) and
/// game type > -1 `data/gifcon/multiplayer.ldg`, otherwise `data/gifcon/default.ldg`; when loading a savegame always
/// `default.ldg` (re/singleplayer-menus part 2, E13, O-139).
pub fn ldg_path_for(
    level: &str,
    game_type: i32,
    from_save: bool,
    exists: impl Fn(&str) -> bool,
) -> String {
    let keep = level.chars().count().saturating_sub(4);
    let own: String = level.chars().take(keep).chain(".ldg".chars()).collect();
    if exists(&own) {
        own
    } else if !from_save && game_type > -1 {
        MULTIPLAYER_LDG.to_string()
    } else {
        DEFAULT_LDG.to_string()
    }
}

/// `v * w / 1280` in integer arithmetic, truncating (`IMUL` and `0x66666667`, `0x004C0B20`...).
fn scale_x(v: i32, b: i32) -> i32 {
    (i64::from(v) * i64::from(b) / 1280) as i32
}

/// `v * h / 1024`, truncating (`(v + ((v >> 31) & 0x3FF)) >> 10`, `0x004C0B2D`...).
fn scale_y(v: i32, h: i32) -> i32 {
    (i64::from(v) * i64::from(h) / 1024) as i32
}

/// Screen rectangle of the bar at `percent` percent (`0x004C0AF3`...`0x004C0BBB`): `x0 = X0*W/1280`,
/// `y0 = Y0*H/1024`, `x1 = X1*W/1280`, `y1 = Y1*H/1024` (all truncating), right edge
/// `x0 + (x1 - x0)*p/100`. The height does not depend on `p` (re/singleplayer-menus part 2, E12: the bar grows only in
/// x). Subtraction and addition overflow in 32 bits as in the original (`wrapping_*` like the width of the text).
pub fn bar_rect(rect: [i32; 4], screen: Screen, percent: u8) -> [i32; 4] {
    let x0 = scale_x(rect[0], screen.width);
    let y0 = scale_y(rect[1], screen.height);
    let x1 = scale_x(rect[2], screen.width);
    let y1 = scale_y(rect[3], screen.height);
    let grown = (i64::from(percent) * i64::from(x1.wrapping_sub(x0)) / 100) as i32;
    [x0, y0, x0.wrapping_add(grown), y1]
}

/// Screen rectangle of a text block (`0x004C0BEB`...`0x004C0C6B`): `x0 = E0*W/1280`, **`y0 = E1*H/1280`**,
/// `x1 = E2*W/1280`, `y1 = E3*H/1024` (`y0` and `y1` with different divisors, re/singleplayer-menus part 2, E11).
pub fn text_rect(extent: [i32; 4], screen: Screen) -> [i32; 4] {
    [
        scale_x(extent[0], screen.width),
        (i64::from(extent[1]) * i64::from(screen.height) / 1280) as i32,
        scale_x(extent[2], screen.width),
        scale_y(extent[3], screen.height),
    ]
}

/// A loaded loading screen: the `.ldg` and its two images.
#[derive(Debug, Clone)]
pub struct LoadingScreen {
    ldg: Ldg,
    background: Option<TextureId>,
    timber: Option<TextureId>,
}

impl LoadingScreen {
    /// Loads the background (`LoadingScreen`) and the bar image (`LoadingTimber`) through the image collection. A
    /// missing image (or a missing line) omits that part: without a texture the original does not draw it
    /// (`0x004C0907`, `FUN_00401DF0` returns 0).
    pub fn new(ldg: Ldg, ctx: &mut UiContext) -> LoadingScreen {
        let background = ldg
            .image
            .as_deref()
            .and_then(|path| ctx.assets.texture(ctx.vfs, path));
        let timber = ldg
            .timber
            .as_deref()
            .and_then(|path| ctx.assets.texture(ctx.vfs, path));
        LoadingScreen {
            ldg,
            background,
            timber,
        }
    }

    /// The commands for `percent` percent progress (`FUN_004C08E0`): background, bar, texts in
    /// file order. The version number at the bottom right (`FUN_004BFCC0`) is omitted (a deviation from the original).
    pub fn draw(&self, percent: u8, ctx: &UiContext) -> Vec<DrawCmd> {
        let screen = ctx.screen;
        let mut out = Vec::new();
        if let Some(texture) = self.background {
            out.push(DrawCmd::Quad {
                texture: Some(texture),
                dst: [0, 0, screen.width, screen.height],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: WHITE,
                filter: Filter::Linear,
                blend: Blend::Alpha,
            });
        }
        if let Some(texture) = self.timber {
            out.push(DrawCmd::Quad {
                texture: Some(texture),
                dst: bar_rect(self.ldg.rect, screen, percent),
                uv: [0.0, 0.0, 1.0, 1.0],
                color: self.ldg.color,
                filter: Filter::Linear,
                // State block `billboard`: additive (O-194).
                blend: Blend::Additive,
            });
        }
        for block in &self.ldg.blocks {
            let [x0, y0, x1, _] = text_rect(block.extent, screen);
            let text = ctx.texts.display(block.text_id);
            let params = LayoutParams {
                flags: TEXT_FLAGS,
                width: x1.wrapping_sub(x0),
                font: ctx.fonts.by_name(&block.font),
                color: block.color,
            };
            debug_assert!(params.flags & layout_flags::WIDTH != 0);
            let mut layout = TextLayout::default();
            layout_text(&text, &params, ctx.fonts, &mut layout);
            draw_runs(&layout, [x0, y0], false, None, ctx.fonts, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::fonts::FontBook;
    use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};
    use nml_formats::ldg::parse_ldg;
    use nml_formats::localisation::parse_localisation;
    use nml_rda::Vfs;

    const LOC: &str = "Font LoadingFont\nFontSize 15\nTextureFont\nFontName Test\n\
Font InfoFont\nFontSize 30\nTextureFont\nFontName Big\n";

    /// Images, fonts (advance 10 and 20) and texts for the tests.
    struct Fixture {
        vfs: TestVfs,
        assets: UiAssets,
        fonts: FontBook,
        texts: Texts,
    }

    fn fixture() -> Fixture {
        let white = || tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]);
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            ("data/fonts/Test_15_0_0_0_0.tga", white()),
            (
                "data/fonts/Big_30_0_0_0_0.fnt",
                fnt_text(|_| 20, |_| 30).into_bytes(),
            ),
            ("data/fonts/Big_30_0_0_0_0.tga", white()),
            (
                "data/gfx/back.tga",
                tga_bottom_up(8, 4, |_, _| [10, 20, 30, 255]),
            ),
            (
                "data/gfx/timber.tga",
                tga_bottom_up(4, 4, |_, _| [200, 100, 50, 255]),
            ),
        ]);
        let mut assets = UiAssets::new();
        let fonts = FontBook::load(&vfs, &parse_localisation(LOC), 1024, &mut assets, None);
        let texts = Texts::from_entries(&[(20012, "Text"), (27060, "[CENTER]AB")]);
        Fixture {
            vfs,
            assets,
            fonts,
            texts,
        }
    }

    impl Fixture {
        fn ctx(&mut self, width: i32, height: i32) -> UiContext<'_> {
            UiContext {
                vfs: &self.vfs as &Vfs,
                assets: &mut self.assets,
                fonts: &self.fonts,
                texts: &self.texts,
                screen: Screen { width, height },
            }
        }
    }

    const KAMPAGNE: &str = "LoadingScreen data/gfx/back.tga\r\nLoadingTimber data/gfx/timber.tga\r\n\
LoadingRect 834 690 1072 1120\r\nLoadingColor ffff00ff\r\n";

    /// Rectangle, UV, color, filter and blend mode of a quad.
    type QuadFields = ([i32; 4], [f32; 4], u32, Filter, Blend);

    fn quads(cmds: &[DrawCmd]) -> Vec<QuadFields> {
        cmds.iter()
            .map(|c| match *c {
                DrawCmd::Quad {
                    dst,
                    uv,
                    color,
                    filter,
                    blend,
                    ..
                } => (dst, uv, color, filter, blend),
                DrawCmd::Quad4 { .. } => unreachable!(),
            })
            .collect()
    }

    /// O-137, O-138: bar `x = X*W/1280`, `y = Y*H/1024`, right edge `x0 + (x1 - x0)*p/100`, all truncating.
    // QUIRK O-137, O-138: kept
    #[test]
    fn loading_rects_follow_the_original_formulas() {
        let s = |width, height| Screen { width, height };
        let rect = [834, 690, 1072, 1120];
        // 1024 x 768: x0 = 667 (667.2), y0 = 517 (517.5), x1 = 857 (857.6), y1 = 840.
        assert_eq!(bar_rect(rect, s(1024, 768), 0), [667, 517, 667, 840]);
        assert_eq!(bar_rect(rect, s(1024, 768), 40), [667, 517, 743, 840]);
        assert_eq!(bar_rect(rect, s(1024, 768), 100), [667, 517, 857, 840]);
        // 1600 x 900: x0 = 1042 (1042.5), y0 = 606 (606.45), x1 = 1340, y1 = 984 (984.375).
        assert_eq!(bar_rect(rect, s(1600, 900), 70), [1042, 606, 1250, 984]);
        assert_eq!(bar_rect(rect, s(1600, 900), 100), [1042, 606, 1340, 984]);
        // Text: `y0` divided by 1280, `y1` by 1024 (re/singleplayer-menus part 2, E11).
        let extent = [100, 200, 900, 400];
        assert_eq!(text_rect(extent, s(1024, 768)), [80, 120, 720, 300]);
        assert_eq!(text_rect(extent, s(1600, 900)), [125, 140, 1125, 351]);
        // Truncation toward 0 also for negative values (`(v + ((v >> 31) & 0x3FF)) >> 10`).
        assert_eq!(bar_rect([-1, -1, 1, 1], s(1024, 768), 100)[..2], [0, 0]);
        // Screen of size 0: nothing to divide, no panic.
        assert_eq!(bar_rect(rect, s(0, 0), 50), [0, 0, 0, 0]);
    }

    /// Extreme values of the `.ldg` (`Rect` arbitrary `int`): 32-bit overflow as in the original (`SUB`/`ADD`), no
    /// panic in the debug build.
    #[test]
    fn bar_rect_wraps_like_the_original_on_extreme_values() {
        let s = Screen {
            width: 1280,
            height: 1024,
        };
        // x0 = i32::MIN, x1 = i32::MAX: `x1 - x0` overflows (-1), `x0 + (-1)` likewise (i32::MAX).
        let r = bar_rect([i32::MIN, 0, i32::MAX, 0], s, 100);
        assert_eq!(r, [i32::MIN, 0, i32::MAX, 0]);
        assert_eq!(
            bar_rect([i32::MAX, i32::MIN, i32::MIN, i32::MAX], s, 50)[0],
            i32::MAX
        );
        for p in [0u8, 1, 50, 100, 255] {
            bar_rect([i32::MIN, i32::MIN, i32::MAX, i32::MAX], s, p);
            bar_rect([i32::MAX, i32::MAX, i32::MIN, i32::MIN], s, p);
        }
    }

    /// The command list contains the background (whole screen, stretched) and the bar with the formulas.
    // QUIRK O-138, O-193, O-194: kept
    #[test]
    fn draw_emits_background_and_bar() {
        let mut f = fixture();
        for (w, h, bar) in [
            (1024, 768, [667, 517, 857, 840]),
            (1600, 900, [1042, 606, 1340, 984]),
        ] {
            let mut ctx = f.ctx(w, h);
            let screen = LoadingScreen::new(parse_ldg(KAMPAGNE), &mut ctx);
            let cmds = screen.draw(100, &ctx);
            assert_eq!(
                quads(&cmds),
                vec![
                    (
                        [0, 0, w, h],
                        [0.0, 0.0, 1.0, 1.0],
                        WHITE,
                        Filter::Linear,
                        Blend::Alpha
                    ),
                    (
                        bar,
                        [0.0, 0.0, 1.0, 1.0],
                        0xFFFF_00FF,
                        Filter::Linear,
                        Blend::Additive
                    ),
                ]
            );
        }
    }

    // QUIRK O-138: kept
    #[test]
    fn bar_grows_only_in_x() {
        let mut f = fixture();
        let mut ctx = f.ctx(1024, 768);
        let screen = LoadingScreen::new(parse_ldg(KAMPAGNE), &mut ctx);
        let bars: Vec<_> = [0u8, 40, 70, 100]
            .iter()
            .map(|&p| quads(&screen.draw(p, &ctx))[1])
            .collect();
        for bar in &bars {
            // Top edge, bottom edge, left edge, UV, color and filter stay.
            assert_eq!((bar.0[0], bar.0[1], bar.0[3]), (667, 517, 840));
            assert_eq!((bar.1, bar.2, bar.3), (bars[0].1, bars[0].2, bars[0].3));
        }
        let right: Vec<i32> = bars.iter().map(|b| b.0[2]).collect();
        assert_eq!(right, vec![667, 743, 800, 857]);
    }

    /// `Rect` without bar image, without background: none of it is drawn (no crash).
    #[test]
    fn missing_images_skip_their_parts() {
        let mut f = fixture();
        let mut ctx = f.ctx(1024, 768);
        let ldg = parse_ldg("LoadingScreen data/gfx/missing.tga\r\nLoadingRect 0 0 100 100\r\n");
        let screen = LoadingScreen::new(ldg, &mut ctx);
        assert!(screen.draw(50, &ctx).is_empty());
        // Bar image missing: only the background.
        let ldg = parse_ldg(
            "LoadingScreen data/gfx/back.tga\r\nLoadingTimber data/gfx/missing.tga\r\nLoadingRect 0 0 100 100\r\n",
        );
        let screen = LoadingScreen::new(ldg, &mut ctx);
        assert_eq!(screen.draw(50, &ctx).len(), 1);
    }

    /// Text: format `0x46`, width `x1 - x0`, origin `(x0, y0)`, no shadow, `[CENTER]` takes effect, blocks in
    /// file order after the bar, font by name and text color of the block.
    // QUIRK O-137: kept
    #[test]
    fn text_blocks_are_laid_out_in_their_rectangles() {
        let mut f = fixture();
        let mut ctx = f.ctx(1024, 768);
        let ldg = parse_ldg(&format!(
            "{KAMPAGNE}StartTextBlock\r\nTextExtent 100 200 900 400\r\nTextGuid 27060\r\n\
TextColor 80FF0000\r\nTextFont InfoFont\r\nEndTextBlock\r\n\
StartTextBlock\r\nTextExtent 0 0 1280 100\r\nTextGuid 20012\r\nTextColor ff00ff00\r\n\
TextFont LoadingFont\r\nEndTextBlock\r\n"
        ));
        let screen = LoadingScreen::new(ldg, &mut ctx);
        let cmds = screen.draw(10, &ctx);
        let all = quads(&cmds);
        // 2 (background, bar) + "AB" (2 characters, no shadow) + "Text" (4 characters).
        assert_eq!(all.len(), 2 + 2 + 4);
        // Block 1: rectangle (80, 120)...(720, 300), font InfoFont (advance 20, cell 16 x 16), width 640:
        // centered x = 80 + (640 - 40)/2 = 380, y = 120.
        let (dst, _, color, filter, _) = all[2];
        assert_eq!((dst[0], dst[1]), (380, 120));
        assert_eq!((color, filter), (0x80FF_0000, Filter::Point));
        assert_eq!(all[3].0[0], 380 + 20);
        // Block 2: rectangle (0, 0)...(1024, 76), LoadingFont (advance 10), left-aligned at (0, 0).
        let (dst, _, color, _, _) = all[4];
        assert_eq!((dst[0], dst[1], color), (0, 0, 0xFF00_FF00));
        assert_eq!(all[5].0[0], 10);
    }

    /// The text of a missing ID shows the number as everywhere (O-4); the block inherits defaults (O-136).
    #[test]
    fn template_block_uses_the_default_text() {
        let mut f = fixture();
        let mut ctx = f.ctx(1024, 768);
        let ldg = parse_ldg("StartTextBlock\r\nEndTextBlock\r\n");
        let screen = LoadingScreen::new(ldg, &mut ctx);
        // `TextGuid 20012` = "Text" (4 characters), font `LoadingFont`, white; rectangle (20, 20)...(800, 600).
        let cmds = screen.draw(0, &ctx);
        let all = quads(&cmds);
        assert_eq!(all.len(), 4);
        // x0 = 20*1024/1280 = 16, y0 = 20*768/1280 = 12.
        assert_eq!((all[0].0[0], all[0].0[1], all[0].2), (16, 12, WHITE));
    }

    /// O-139: missing `.ldg` - restart of a multiplayer map `multiplayer.ldg`, otherwise and when loading
    /// `default.ldg`.
    // QUIRK O-139: kept
    #[test]
    fn missing_ldg_falls_back_to_default() {
        let none = |_: &str| false;
        let level = "data/levels/multiplayer/4-karte/4-karte.lvl";
        assert_eq!(ldg_path_for(level, 0, false, none), MULTIPLAYER_LDG);
        assert_eq!(ldg_path_for(level, 4, false, none), MULTIPLAYER_LDG);
        // Campaign (game type -1) and menu level (-2): `default.ldg`.
        assert_eq!(ldg_path_for(level, -1, false, none), DEFAULT_LDG);
        assert_eq!(ldg_path_for(level, -2, false, none), DEFAULT_LDG);
        // Loading: never `multiplayer.ldg`, even with game type > -1 (re/singleplayer-menus part 2, E13).
        assert_eq!(ldg_path_for(level, 0, true, none), DEFAULT_LDG);
        assert_eq!(ldg_path_for(level, -1, true, none), DEFAULT_LDG);
    }

    // QUIRK O-139: kept
    #[test]
    fn existing_ldg_is_taken_without_the_last_four_characters() {
        let level = "data/levels/kampagne1/kamp1-1.lvl";
        let own = "data/levels/kampagne1/kamp1-1.ldg";
        let only_own = |p: &str| p == own;
        assert_eq!(ldg_path_for(level, -1, false, only_own), own);
        assert_eq!(ldg_path_for(level, 0, true, only_own), own);
        // The last four characters are dropped, also with another extension or a name that is too short.
        let weird = |p: &str| p == "data/x.ldg" || p == ".ldg";
        assert_eq!(ldg_path_for("data/x.abc", 0, false, weird), "data/x.ldg");
        assert_eq!(ldg_path_for("abc", 0, false, weird), ".ldg");
    }
}
