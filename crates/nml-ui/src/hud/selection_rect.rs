//! Selection box of the root of the game UI (re/game-ui, section "Selection box addendum"; quirk O-384): slot 1
//! of the root (`0x00449890`) draws, while a box is running (`+0x48`), via `0x004494E0` a semi-transparent area and
//! an opaque 1-px outline in screen pixels, between the start point of the press (`+0x40/+0x44`) and the pointer
//! position of the frame (`Cursor3D` `0x004CCB00`). Whether and when to draw is decided by the host.

use crate::bkg_paint::fill_rect;
use crate::draw::DrawCmd;
use crate::hud::tooltip::outline;

/// Area (`Renderer_DrawUpTriStripScreen` `0x00571B90`, state block `alpharect1`): blue `(0, 0, 128)` with alpha
/// 128, the same color as the tooltip box.
pub const FILL_COLOR: u32 = 0x8000_0080;
/// Outline (`Renderer_DrawIndexedUpLineListScreen` `0x00571CE0`, indices 0-1, 1-3, 3-2, 2-0, also `alpharect1`):
/// alpha 255, so opaque `(0, 0, 128)` like the outline of the tooltip.
pub const OUTLINE_COLOR: u32 = 0xFF00_0080;

/// Draws the box between `start` and `end` (UI pixels, any drag direction): first the area over `[min, max)`
/// (triangle strip of the four corners), then the four lines with the pixels measured on the tooltip outline of the
/// same line call ([`outline`]: outside right and bottom, bottom-right corner free). Empty parts are dropped: without
/// extent nothing remains, with width or height 0 only one line.
pub fn draw(start: [i32; 2], end: [i32; 2], out: &mut Vec<DrawCmd>) {
    let rect = [
        start[0].min(end[0]),
        start[1].min(end[1]),
        start[0].max(end[0]),
        start[1].max(end[1]),
    ];
    for (part, color) in std::iter::once(rect)
        .map(|r| (r, FILL_COLOR))
        .chain(outline(rect).into_iter().map(|e| (e, OUTLINE_COLOR)))
    {
        if part[2] > part[0] && part[3] > part[1] {
            fill_rect(part, color, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::{Blend, Filter};

    fn quad(dst: [i32; 4], color: u32) -> DrawCmd {
        DrawCmd::Quad {
            texture: None,
            dst,
            uv: [0.0; 4],
            color,
            filter: Filter::Point,
            blend: Blend::Alpha,
        }
    }

    /// Area over `[l, r) x [t, b)`, then top, right, bottom, left like the tooltip outline.
    #[test]
    fn fill_then_outline_like_the_tooltip() {
        let mut out = Vec::new();
        draw([10, 20], [50, 40], &mut out);
        assert_eq!(
            out,
            [
                quad([10, 20, 50, 40], FILL_COLOR),
                quad([10, 20, 50, 21], OUTLINE_COLOR),
                quad([50, 20, 51, 40], OUTLINE_COLOR),
                quad([10, 40, 50, 41], OUTLINE_COLOR),
                quad([10, 20, 11, 40], OUTLINE_COLOR),
            ]
        );
    }

    /// The drag direction changes nothing: dragged to the top left yields the same pixels.
    #[test]
    fn any_drag_direction_gives_the_same_pixels() {
        let mut forward = Vec::new();
        draw([10, 20], [50, 40], &mut forward);
        for (start, end) in [
            ([50, 40], [10, 20]),
            ([50, 20], [10, 40]),
            ([10, 40], [50, 20]),
        ] {
            let mut out = Vec::new();
            draw(start, end, &mut out);
            assert_eq!(out, forward, "{start:?} -> {end:?}");
        }
    }

    /// No minimum distance: without extent nothing, with height 0 only the horizontal line (top and bottom are the
    /// same row, which the line list of the original draws twice), with width 0 only the vertical one.
    #[test]
    fn degenerate_rectangles_draw_lines_only() {
        let mut out = Vec::new();
        draw([7, 7], [7, 7], &mut out);
        assert!(out.is_empty());
        draw([10, 20], [15, 20], &mut out);
        assert_eq!(
            out,
            [
                quad([10, 20, 15, 21], OUTLINE_COLOR),
                quad([10, 20, 15, 21], OUTLINE_COLOR),
            ]
        );
        out.clear();
        draw([10, 20], [10, 23], &mut out);
        assert_eq!(
            out,
            [
                quad([10, 20, 11, 23], OUTLINE_COLOR),
                quad([10, 20, 11, 23], OUTLINE_COLOR),
            ]
        );
    }
}
