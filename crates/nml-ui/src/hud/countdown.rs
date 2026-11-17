//! Countdown displays of `FUN_00435EE0` (from `GameEngine_RenderWorld` `0x00438F74`), both as `m:ss` in the font
//! `CountDown`, without simulation: the host passes in the seconds and draws with the world, below the UI (O-367).
//!
//! - Part 1 (`0x00435EFD...0x00435FBE`): the remaining time of the registered container (`n15`) at the top right
//!   ([`countdown_cmds`]); basis re/campaign-ui D6.9.
//! - Part 2 (`0x00435FC0...0x004360B5`): the timer list `m_CountDownList`. Every entry with kind bit 1 (`TEST AL,1`
//!   at `0x00435FDE`) sits at its own position `{x, y}` in the 1280 x 1024 grid ([`timer_cmds`]; `FUN_0042E8B0` is the
//!   grid conversion of the game window, not a world projection). These are the leader timer of game types 2/3
//!   (`0x00445556`) and the time limit of game type 4 (`0x00445416`), both kind 1 and position [`TIMER_POS`].

use super::COUNTDOWN_FONT;
use crate::coords::{game_x, game_y};
use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::text::{atlas_codes, draw_string, measure};

/// Top edge of the countdown in screen pixels (`0x14`, without conversion from the 1280 x 1024 grid).
pub const COUNTDOWN_Y: i32 = 20;
/// Color from 10 seconds on (`0xC8FFFFFF`).
pub const COUNTDOWN_COLOR: u32 = 0xC8FF_FFFF;
/// Color below 10 seconds (`0xC8FF0000`).
pub const COUNTDOWN_RED: u32 = 0xC8FF_0000;

/// Position of the timers of the list in the 1280 x 1024 grid (`{0, 0x28}`, top left corner of the text); this is how
/// `GameEngine_EvaluateFlagLeader` (`0x00445556`) and `FUN_004453D0` (`0x00445416`) create them.
pub const TIMER_POS: [i32; 2] = [0, 0x28];

/// Text of the countdown (`"%d:%02d"`, `0x00618C98`): minutes without a limit, seconds two-digit; division and
/// remainder truncate toward 0 as in C (negative values yield e.g. `"0:-5"`).
pub fn countdown_text(seconds: i32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Draws the countdown (`FUN_00435EE0`, first part): font `CountDown`, right-aligned at `x = W - text width`,
/// `y = 20`, without text layout and without a shadow (`FUN_00428600` measures, `FUN_004287C0` draws); color
/// [`COUNTDOWN_COLOR`], with `seconds < 10` [`COUNTDOWN_RED`]. The host checks the condition "container registered"
/// (`GE+0x608 != 0`).
pub fn countdown_cmds(seconds: i32, ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>) {
    let codes = atlas_codes(&countdown_text(seconds));
    let font = ctx.fonts.by_name(COUNTDOWN_FONT);
    let width = measure(&codes, font, ctx.fonts)[0];
    draw_string(
        &codes,
        [ctx.screen.width.wrapping_sub(width), COUNTDOWN_Y],
        color_for(seconds),
        font,
        ctx.fonts,
        out,
    );
}

/// Color of both displays: [`COUNTDOWN_COLOR`], with `seconds < 10` [`COUNTDOWN_RED`].
fn color_for(seconds: i32) -> u32 {
    if seconds < 10 {
        COUNTDOWN_RED
    } else {
        COUNTDOWN_COLOR
    }
}

/// Draws one timer of the list (`FUN_00435EE0`, second part): text like [`countdown_text`], top left corner at `pos`
/// (grid 1280 x 1024, converted to the screen with `game_x`/`game_y`, `FUN_0042E8B0`), font `CountDown`, without text
/// layout and without a shadow; color as in part 1. `seconds` is `remaining_ms / 1000` with C division (Rust `/` on
/// `i32` truncates toward 0 the same way). The host checks the condition "kind bit 1" (all timers of the
/// reimplementation have kind 1).
pub fn timer_cmds(seconds: i32, pos: [i32; 2], ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>) {
    let codes = atlas_codes(&countdown_text(seconds));
    let font = ctx.fonts.by_name(COUNTDOWN_FONT);
    draw_string(
        &codes,
        [game_x(ctx.screen, pos[0]), game_y(ctx.screen, pos[1])],
        color_for(seconds),
        font,
        ctx.fonts,
        out,
    );
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

    /// Font `CountDown`: every character 10 px wide, line height 15 (cell 16).
    fn fonts() -> &'static FontBook {
        static FONTS: OnceLock<FontBook> = OnceLock::new();
        FONTS.get_or_init(|| {
            let vfs = test_vfs(&[
                (
                    "data/fonts/Test_15_0_0_0_0.fnt",
                    fnt_text(|_| 10, |_| 15).into_bytes(),
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

    /// The quads of the countdown (part 1) as `(x, y, color)`.
    fn drawn(seconds: i32, screen: Screen) -> Vec<(i32, i32, u32)> {
        drawn_with(screen, |ctx, out| countdown_cmds(seconds, ctx, out))
    }

    /// The quads of a timer (part 2) as `(x, y, color)`.
    fn drawn_timer(seconds: i32, screen: Screen) -> Vec<(i32, i32, u32)> {
        drawn_with(screen, |ctx, out| timer_cmds(seconds, TIMER_POS, ctx, out))
    }

    fn drawn_with(
        screen: Screen,
        draw: impl FnOnce(&mut UiContext<'_>, &mut Vec<DrawCmd>),
    ) -> Vec<(i32, i32, u32)> {
        let vfs = test_vfs(&[]);
        let mut assets = UiAssets::new();
        let texts = Texts::load(&vfs, &Localisation::default());
        let mut ctx = UiContext {
            vfs: &vfs,
            assets: &mut assets,
            fonts: fonts(),
            texts: &texts,
            screen,
        };
        let mut out = Vec::new();
        draw(&mut ctx, &mut out);
        out.iter()
            .map(|c| match *c {
                DrawCmd::Quad {
                    texture: Some(_),
                    dst,
                    color,
                    ..
                } => (dst[0], dst[1], color),
                ref other => panic!("only font cells expected: {other:?}"),
            })
            .collect()
    }

    const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    /// D6.9: 65 s -> `1:05` in white `0xC8FFFFFF`; minutes without a limit (`61:01`).
    // QUIRK O-367: kept
    #[test]
    fn countdown_65_is_1_05_white() {
        assert_eq!(countdown_text(65), "1:05");
        assert_eq!(countdown_text(3_661), "61:01");
        assert_eq!(countdown_text(10), "0:10");
        let quads = drawn(65, SCREEN);
        assert_eq!(quads.len(), 4, "one quad per character, without a shadow");
        assert!(quads.iter().all(|&(.., c)| c == COUNTDOWN_COLOR));
        assert!(
            drawn(10, SCREEN)
                .iter()
                .all(|&(.., c)| c == COUNTDOWN_COLOR),
            "10 s still white"
        );
    }

    /// D6.9: below 10 s red `0xC8FF0000`, two-digit seconds; negative values as in C (`%d:%02d`).
    // QUIRK O-367: kept
    #[test]
    fn countdown_9_is_0_09_red() {
        assert_eq!(countdown_text(9), "0:09");
        assert_eq!(countdown_text(0), "0:00");
        assert_eq!(countdown_text(-5), "0:-5");
        let quads = drawn(9, SCREEN);
        assert_eq!(quads.len(), 4);
        assert!(quads.iter().all(|&(.., c)| c == COUNTDOWN_RED));
    }

    /// D6.9: `x = W - text width` (sum of the advances), `y = 20` pixels without conversion, also at another screen
    /// size.
    // QUIRK O-367: kept
    #[test]
    fn countdown_right_aligned_at_y_20() {
        let xs = |seconds, screen| -> Vec<(i32, i32)> {
            drawn(seconds, screen)
                .into_iter()
                .map(|(x, y, _)| (x, y))
                .collect()
        };
        // "1:05": four characters of 10 px -> 40 px, right-aligned.
        assert_eq!(
            xs(65, SCREEN),
            [(984, 20), (994, 20), (1004, 20), (1014, 20)]
        );
        let wide = Screen {
            width: 1600,
            height: 1200,
        };
        // "12:34": five characters -> 50 px.
        assert_eq!(xs(754, wide)[0], (1550, 20));
    }

    /// Part 2: position `{0, 0x28}` in the grid, top left corner: at 1024 x 768 `(0, 30)`, at 1280 x 1024 `(0, 40)`;
    /// the text runs to the right with the advances, without a shadow.
    #[test]
    fn timer_at_top_left_in_grid() {
        assert_eq!(TIMER_POS, [0, 0x28]);
        let xs = |seconds, screen| -> Vec<(i32, i32)> {
            drawn_timer(seconds, screen)
                .into_iter()
                .map(|(x, y, _)| (x, y))
                .collect()
        };
        // "0:59": four characters of 10 px.
        assert_eq!(xs(59, SCREEN), [(0, 30), (10, 30), (20, 30), (30, 30)]);
        let big = Screen {
            width: 1280,
            height: 1024,
        };
        assert_eq!(xs(59, big)[0], (0, 40));
        // Other screen size: `x = W * 0 / 1280 = 0`, `y = H * 40 / 1024` truncating (900 -> 35).
        let other = Screen {
            width: 1600,
            height: 900,
        };
        assert_eq!(xs(59, other)[0], (0, 35));
    }

    /// Part 2: white at 10 s, red at 9 s; minutes without a limit (3,600 s -> `60:00`); one quad per character.
    #[test]
    fn timer_color_and_text() {
        assert_eq!(countdown_text(3_600), "60:00");
        let quads = drawn_timer(3_600, SCREEN);
        assert_eq!(quads.len(), 5, "\"60:00\" without a shadow: {quads:?}");
        assert!(quads.iter().all(|&(.., c)| c == COUNTDOWN_COLOR));
        assert!(
            drawn_timer(10, SCREEN)
                .iter()
                .all(|&(.., c)| c == COUNTDOWN_COLOR),
            "10 s still white"
        );
        let red = drawn_timer(9, SCREEN);
        assert_eq!(red.len(), 4);
        assert!(red.iter().all(|&(.., c)| c == COUNTDOWN_RED));
        // "0:00" (remainder 0...999 ms) is red as well.
        assert!(
            drawn_timer(0, SCREEN)
                .iter()
                .all(|&(.., c)| c == COUNTDOWN_RED)
        );
    }
}
