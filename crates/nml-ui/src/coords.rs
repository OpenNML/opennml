//! Virtual space 1280 x 1024 and conversion to the screen (re/ui-rendering 2.1,
//! re/menu-system 8.4.1).

/// Size of the back buffer in pixels (`B × H`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub width: i32,
    pub height: i32,
}

/// Width of the virtual space (`0x500`, constructor `FUN_0045A270`).
pub const VIRTUAL_W: i32 = 1280;
/// Height of the virtual space (`0x400`).
pub const VIRTUAL_H: i32 = 1024;

/// Result of a 64-bit calculation as `i32` (clamped instead of overflowing; the original calculates with 32 bits,
/// only values far below the limits occur in the shipped data).
fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Virtual -> screen, x: `(B·x + 640) / 1280`, C division (rounding towards 0), `MenuWidget_VirtualToScreenRect`
/// `0x004594E0` / point `FUN_00459490` (re/ui-rendering 2.1, re/menu-system 8.4.1).
pub fn to_screen_x(s: Screen, x: i32) -> i32 {
    clamp_i32((i64::from(s.width) * i64::from(x) + i64::from(VIRTUAL_W) / 2) / i64::from(VIRTUAL_W))
}

/// Virtual -> screen, y: `(H·y + 512) / 1024`, likewise divided **towards 0** (`(v + ((v>>31)&0x3FF)) >> 10`,
/// not rounded down: y = -60 at 768 gives -44, not -45).
pub fn to_screen_y(s: Screen, y: i32) -> i32 {
    clamp_i32(
        (i64::from(s.height) * i64::from(y) + i64::from(VIRTUAL_H) / 2) / i64::from(VIRTUAL_H),
    )
}

/// Converts a rectangle `[x0, y0, x1, y1]` as a whole (`FUN_004594E0`): each corner individually with the same
/// formula, shared edges fall onto the same pixels.
pub fn to_screen_rect(s: Screen, r: [i32; 4]) -> [i32; 4] {
    [
        to_screen_x(s, r[0]),
        to_screen_y(s, r[1]),
        to_screen_x(s, r[2]),
        to_screen_y(s, r[3]),
    ]
}

/// Game window, x: `B · x / 1280` in **32 bit**, division **towards zero** (truncating, not rounding like
/// [`to_screen_x`]); `0x00452800` (re/game-ui N.13: factor `0x66666667`, `SAR 9`, sign correction). The product wraps
/// around in 32 bit like `imul`; with screen sizes that does not occur.
pub fn game_x(s: Screen, x: i32) -> i32 {
    s.width.wrapping_mul(x) / VIRTUAL_W
}

/// Game window, y or length: `(H · y + (H · y < 0 ? 0x3FF : 0)) >> 10`, i.e. division by 1024 towards zero
/// (`0x00452830`, re/game-ui N.13).
pub fn game_y(s: Screen, y: i32) -> i32 {
    let p = s.height.wrapping_mul(y);
    p.wrapping_add(if p < 0 { 0x3FF } else { 0 }) >> 10
}

/// Rectangle `[left, top, right, bottom]` of the game windows (`0x00452880`): each corner with [`game_x`]/[`game_y`].
pub fn game_rect(s: Screen, r: [i32; 4]) -> [i32; 4] {
    [
        game_x(s, r[0]),
        game_y(s, r[1]),
        game_x(s, r[2]),
        game_y(s, r[3]),
    ]
}

/// Height of the command bar and side length of the minimap in screen pixels (`0x00452BE0`, `0x00449B90`).
pub const TOOLBAR_SIZE: i32 = 176;
/// Width of the command bar in screen pixels (`0x00452BE0`).
pub const TOOLBAR_WIDTH: i32 = 640;

/// Command bar in fixed screen pixels: `(0, H − 176, 640, H)` (`0x00452BE0`, re/game-ui 1.3, O-34).
pub fn toolbar_rect(s: Screen) -> [i32; 4] {
    [0, s.height - TOOLBAR_SIZE, TOOLBAR_WIDTH, s.height]
}

/// Minimap in fixed screen pixels: `(B − 176, H − 176, B, H)` (`0x00449B90`, re/game-ui 1.3, O-34).
pub fn minimap_rect(s: Screen) -> [i32; 4] {
    [
        s.width - TOOLBAR_SIZE,
        s.height - TOOLBAR_SIZE,
        s.width,
        s.height,
    ]
}

/// Status bar: `(0, 0, B, STATUSSIZE)` (`0x00451F80`, `statusbar.ini`; re/game-ui 1.3).
pub fn status_rect(s: Screen, size: i32) -> [i32; 4] {
    [0, 0, s.width, size]
}

/// Screen -> virtual, x: `(B/2 + x·1280) / B`, C division (`FUN_00459570`, re/menu-system 3.2). Width 0 gives 0
/// (the original divided by 0; the screen size is never 0 there).
pub fn to_virtual_x(s: Screen, x: i32) -> i32 {
    if s.width == 0 {
        return 0;
    }
    let b = i64::from(s.width);
    clamp_i32((b / 2 + i64::from(x) * i64::from(VIRTUAL_W)) / b)
}

/// Screen -> virtual, y: `(H/2 + y·1024) / H` (`FUN_00459610`).
pub fn to_virtual_y(s: Screen, y: i32) -> i32 {
    if s.height == 0 {
        return 0;
    }
    let h = i64::from(s.height);
    clamp_i32((h / 2 + i64::from(y) * i64::from(VIRTUAL_H)) / h)
}

/// Orders the corners so that `left <= right` and `top <= bottom` (base constructor `FUN_0045A270` swaps swapped
/// `EXTENT` corners; re/menu-system 2.1).
pub fn normalize_rect(r: [i32; 4]) -> [i32; 4] {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn screen(width: i32, height: i32) -> Screen {
        Screen { width, height }
    }

    /// The formulas of the original with 64-bit integers, C division (Rust `/` rounds towards 0).
    fn expect_x(b: i64, x: i64) -> i32 {
        ((b * x + 640) / 1280) as i32
    }
    fn expect_y(h: i64, y: i64) -> i32 {
        ((h * y + 512) / 1024) as i32
    }

    #[test]
    fn coordinates_follow_the_original() {
        // videooptions.dlg, ZIELPOSITION 710 212 at 1024 x 768.
        let s = screen(1024, 768);
        assert_eq!(to_screen_x(s, 710), 568);
        assert_eq!(to_screen_y(s, 212), 159);
        // Negative values are divided towards 0, not rounded down: (-450560 + 640) / 1280 = -351,
        // (-46080 + 512) / 1024 = -44 (rounding down would give -45).
        assert_eq!(to_screen_x(s, -440), -351);
        assert_eq!(to_screen_y(s, -60), -44);
        // 800 x 600
        let s = screen(800, 600);
        assert_eq!(to_screen_x(s, 710), 444);
        assert_eq!(to_screen_y(s, 212), 124);
        // 1280 x 1024: identity for values >= 0; for negative values the division towards 0 shifts one pixel
        // upwards (-500 -> -499, -1 -> 0), that is how the original calculates.
        let s = screen(1280, 1024);
        for v in [0, 1, 212, 710, 1280] {
            assert_eq!(to_screen_x(s, v), v);
            assert_eq!(to_screen_y(s, v), v);
        }
        assert_eq!(to_screen_x(s, -500), -499);
        assert_eq!(to_screen_y(s, -1), 0);
        // 1600 x 900
        let s = screen(1600, 900);
        assert_eq!(to_screen_x(s, 710), 888);
        assert_eq!(to_screen_y(s, 212), 186);
        // Rectangle: first in the grid, then converted as a whole.
        assert_eq!(
            to_screen_rect(screen(1024, 768), [1280, 212, 1820, 912]),
            [1024, 159, 1456, 684]
        );
    }

    #[test]
    fn coordinates_follow_the_original_at_extreme_sizes() {
        for (b, h) in [(640, 480), (3840, 2160)] {
            let s = screen(b, h);
            assert_eq!(to_screen_x(s, 1820), expect_x(b.into(), 1820));
            assert_eq!(to_screen_y(s, -500), expect_y(h.into(), -500));
            assert_eq!(
                to_screen_rect(s, [0, 0, 1280, 1024]),
                [0, 0, b, h],
                "the whole grid is the whole screen"
            );
        }
        assert_eq!(to_screen_x(screen(3840, 2160), 1820), 5460);
        assert_eq!(to_screen_y(screen(3840, 2160), -500), expect_y(2160, -500));
        // No overflow at i32 limits (intermediate values in 64 bit, result clamped).
        let s = screen(3840, 2160);
        assert_eq!(to_screen_x(s, i32::MAX), i32::MAX);
        assert_eq!(to_screen_x(s, i32::MIN), i32::MIN);
        assert_eq!(to_screen_y(s, i32::MIN), i32::MIN);
    }

    #[test]
    fn screen_to_virtual_is_the_c_division_inverse() {
        // FUN_00459570/FUN_00459610: x = (xs*1280 + B/2) / B, likewise y.
        let s = screen(1024, 768);
        assert_eq!(to_virtual_x(s, 568), (568 * 1280 + 512) / 1024);
        assert_eq!(to_virtual_y(s, 159), (159 * 1024 + 384) / 768);
        // The round trip is exact when scaling up, within one grid step when scaling down.
        for x in [0, 100, 710, 1280] {
            let back = to_virtual_x(s, to_screen_x(s, x));
            assert!((back - x).abs() <= 1, "x = {x}, back {back}");
        }
        // Negative: C division towards 0.
        assert_eq!(to_virtual_x(s, -351), (-351 * 1280 + 512) / 1024);
        // Width 0 must not crash.
        assert_eq!(to_virtual_x(screen(0, 0), 10), 0);
        assert_eq!(to_virtual_y(screen(0, 0), 10), 0);
    }

    #[test]
    fn normalize_rect_swaps_corners() {
        assert_eq!(normalize_rect([10, 20, 30, 40]), [10, 20, 30, 40]);
        assert_eq!(normalize_rect([30, 40, 10, 20]), [10, 20, 30, 40]);
        assert_eq!(normalize_rect([30, 20, 10, 40]), [10, 20, 30, 40]);
        assert_eq!(normalize_rect([10, 40, 30, 20]), [10, 20, 30, 40]);
        assert_eq!(normalize_rect([5, 5, 5, 5]), [5, 5, 5, 5]);
    }

    #[test]
    fn game_coords_truncate_toward_zero() {
        // 0x00452830: (H*y) >> 10 with sign correction, not the rounding widget conversion.
        let s = screen(1024, 768);
        assert_eq!([400, 500, 600].map(|y| game_y(s, y)), [300, 375, 450]);
        let s = screen(1280, 1024);
        assert_eq!([400, 500, 600].map(|y| game_y(s, y)), [400, 500, 600]);
        let s = screen(800, 600);
        assert_eq!([400, 500, 600].map(|y| game_y(s, y)), [234, 292, 351]);
        // Rounded down, -1 would give -1; divided towards zero it gives 0.
        assert_eq!(game_x(s, -1), 0);
        assert_eq!(game_y(s, -1), 0);
        assert_eq!(game_x(s, 1280), 800);
        // Negative: towards zero, not downward (800*(-100)/1280 = -62.5 -> -62).
        assert_eq!(game_x(s, -100), -62);
        // The rounding conversion stays unchanged beside it: 800*100 + 640 -> 63.
        assert_eq!(to_screen_x(s, 100), 63);
        assert_eq!(game_x(s, 100), 62);
        assert_eq!(
            game_rect(screen(1024, 768), [0, 400, 1280, 1024]),
            [0, 300, 1024, 768]
        );
    }

    #[test]
    fn game_rects_at_1024x768() {
        let s = screen(1024, 768);
        assert_eq!(toolbar_rect(s), [0, 592, 640, 768]);
        assert_eq!(minimap_rect(s), [848, 592, 1024, 768]);
        assert_eq!(status_rect(s, 26), [0, 0, 1024, 26]);
        // O-34: at 800 x 600 command bar and minimap overlap by 16 pixels (fixed pixels, no scaling).
        let s = screen(800, 600);
        assert_eq!(toolbar_rect(s)[2] - minimap_rect(s)[0], 16);
    }
}
