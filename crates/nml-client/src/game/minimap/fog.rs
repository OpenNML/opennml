//! Fog of the minimap in a double buffer (`Minimap_RenderFogRows` `0x004E34F0`, immediate rebuild in
//! `GameWorld_UpdateFogVertexAlpha(1)` `0x00479160`; O-287).
//!
//! The original does not write the fog level of the map at once but, per idle tick of the fog cycle
//! (re/vision 3.2), only `(dim_z / 10) * 2` tile rows into the back one of two textures; when the pass has
//! reached the last row, the textures are swapped and the next one starts at z = 0 again.
//! The front one is always drawn. The reimplementation keeps two [`RgbaImage`] and delivers the front one;
//! uploading it as a texture is done by the draw list.

use image::{Rgba, RgbaImage};

/// Rows per idle tick: `(dim_z / 10) * 2` (caller of `0x004E34F0`, integer: 128 -> 24, 100 -> 20, 256 -> 50).
pub fn rows_per_idle_tick(dim_z: u32) -> u32 {
    // For maps under 10 rows this would be 0 and the pass would never end; the original has the same calculation
    // (smallest real map: 64).
    dim_z / 10 * 2
}

/// Pixel of a tile from the color buffer (`0x00BBGGRR`) and the fog level `level` of the query
/// `GameWorld_QueryFogOfWarArea`: 0 black, 1 half brightness per channel (`>> 1`), otherwise full (`0x004E34F0`).
///
/// Alpha 255 also for black: the original draws the map without blending, the UI pass always blends.
fn fog_pixel(color: u32, level: u8) -> Rgba<u8> {
    let [r, g, b, _] = color.to_le_bytes();
    let shade = |c: u8| match level {
        0 => 0,
        1 => c >> 1,
        _ => c,
    };
    Rgba([shade(r), shade(g), shade(b), 255])
}

/// The two fog images of the minimap.
#[derive(Debug, Clone)]
pub struct MinimapFog {
    /// The drawn image (`tex[+0x08]`).
    front: RgbaImage,
    /// The image the running pass writes into.
    back: RgbaImage,
    /// Next tile row z to write of the running pass (`Minimap+0x04`).
    next_row: u32,
    /// Number of buffer swaps since creation (only for the control channel, `ui_state.game.minimap.fog_swaps`): every
    /// finished pass counts one, the immediate rebuild does not.
    swaps: u32,
}

impl MinimapFog {
    /// Initially both buffers show the unfogged terrain image (`image` of [`super::MinimapTerrain`]).
    pub fn new(terrain_image: &RgbaImage) -> Self {
        Self {
            front: terrain_image.clone(),
            back: terrain_image.clone(),
            next_row: 0,
            swaps: 0,
        }
    }

    /// The drawn image; row 0 = largest z.
    pub fn front(&self) -> &RgbaImage {
        &self.front
    }

    /// Next tile row of the running pass (for checking).
    pub fn next_row(&self) -> u32 {
        self.next_row
    }

    /// Number of buffer swaps (finished passes) since creation.
    pub fn swaps(&self) -> u32 {
        self.swaps
    }

    /// One idle tick (`0x004E34F0` with `texture = 0`): writes the next [`rows_per_idle_tick`] tile rows from
    /// z = `next_row` upward into the back texture (image row `dim_z - 1 - z`). If the pass reaches the last row
    /// (`end >= height`, unsigned), only the remaining rows are written, the buffers are swapped and `next_row` is
    /// set to 0.
    ///
    /// `colors`: color buffer of the terrain (`0x00BBGGRR`, index `z * dim_x + x`); `level(x, z)`: fog level of the
    /// tile (`FogView::area(x, z, player, 0)` with the view switch: 0 unexplored, 1 explored, otherwise seen).
    pub fn idle_tick(&mut self, colors: &[u32], level: impl Fn(u32, u32) -> u8) {
        let dim_z = self.back.height();
        let start = self.next_row;
        let end = start.saturating_add(rows_per_idle_tick(dim_z));
        let done = end >= dim_z;
        write_rows(&mut self.back, colors, &level, start, end.min(dim_z));
        if done {
            // Pass finished: swap (`+0x08 ^= 1`), the next one starts at z = 0.
            std::mem::swap(&mut self.front, &mut self.back);
            self.next_row = 0;
            self.swaps = self.swaps.wrapping_add(1);
        } else {
            self.next_row = end;
        }
    }

    /// Immediate rebuild (`0x00479160(1)`: twice `0x004E34F0(0, dim_z)`): both buffers completely new,
    /// `next_row = 0`. The original leaves the back one new from the running row and old below; visibly that is
    /// equivalent, because the back one is written completely new before its swap anyway.
    pub fn rebuild_now(&mut self, colors: &[u32], level: impl Fn(u32, u32) -> u8) {
        let dim_z = self.back.height();
        write_rows(&mut self.back, colors, &level, 0, dim_z);
        self.front.clone_from(&self.back);
        self.next_row = 0;
    }
}

/// Writes the tile rows `z_start..z_end` from z = `z_start` upward (image row `dim_z - 1 - z`).
fn write_rows(
    image: &mut RgbaImage,
    colors: &[u32],
    level: &impl Fn(u32, u32) -> u8,
    z_start: u32,
    z_end: u32,
) {
    let (dim_x, dim_z) = image.dimensions();
    for z in z_start..z_end {
        let row = dim_z - 1 - z;
        for x in 0..dim_x {
            // A color buffer that is too short (cannot occur for a terrain of the same map) stays black.
            let color = colors.get((z * dim_x + x) as usize).copied().unwrap_or(0);
            image.put_pixel(x, row, fog_pixel(color, level(x, z)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Terrain without fog: every tile `0x00BBGGRR` from the position, image like `MinimapTerrain::image`.
    fn terrain(dim_x: u32, dim_z: u32) -> (Vec<u32>, RgbaImage) {
        let colors: Vec<u32> = (0..dim_x * dim_z)
            .map(|i| 0x0022_5971 + i * 0x100)
            .collect();
        let mut image = RgbaImage::new(dim_x, dim_z);
        for z in 0..dim_z {
            for x in 0..dim_x {
                image.put_pixel(
                    x,
                    dim_z - 1 - z,
                    fog_pixel(colors[(z * dim_x + x) as usize], 7),
                );
            }
        }
        (colors, image)
    }

    /// How many idle ticks until the buffers are swapped?
    fn ticks_until_swap(dim_z: u32) -> u32 {
        let (colors, image) = terrain(4, dim_z);
        let mut fog = MinimapFog::new(&image);
        for ticks in 1..=1000 {
            fog.idle_tick(&colors, |_, _| 7);
            if fog.next_row() == 0 {
                return ticks;
            }
        }
        panic!("pass never ends");
    }

    #[test]
    fn rows_per_idle_tick_24_20_50() {
        assert_eq!(rows_per_idle_tick(128), 24);
        assert_eq!(rows_per_idle_tick(100), 20);
        assert_eq!(rows_per_idle_tick(256), 50);
        // Swap after `ceil(dim_z / rows)` idle ticks: 6, 5, 6.
        assert_eq!(ticks_until_swap(128), 6);
        assert_eq!(ticks_until_swap(100), 5);
        assert_eq!(ticks_until_swap(256), 6);
    }

    #[test]
    fn explored_is_half_and_unexplored_black() {
        // `#715922` (tile (10, 10) in Tinteba River) is halved per channel to `#382C11`.
        assert_eq!(fog_pixel(0x0022_5971, 1).0, [0x38, 0x2C, 0x11, 255]);
        assert_eq!(fog_pixel(0x0022_5971, 0).0, [0, 0, 0, 255]);
        assert_eq!(fog_pixel(0x0022_5971, 3).0, [0x71, 0x59, 0x22, 255]);
        assert_eq!(fog_pixel(0x0022_5971, 7).0, [0x71, 0x59, 0x22, 255]);
        // The halving runs per channel, not over the whole word (no carry between the channels).
        assert_eq!(fog_pixel(0x00FF_01FF, 1).0, [0x7F, 0x00, 0x7F, 255]);
    }

    #[test]
    fn the_idle_ticks_write_rows_from_z0_upwards_into_the_back_buffer() {
        let (colors, image) = terrain(4, 20);
        let mut fog = MinimapFog::new(&image);
        let before = fog.front().clone();
        // 20 / 10 * 2 = 4 rows per idle tick; everything unexplored.
        fog.idle_tick(&colors, |_, _| 0);
        assert_eq!(fog.next_row(), 4);
        assert_eq!(
            fog.front(),
            &before,
            "the front one is drawn until the swap"
        );
        for _ in 1..5 {
            fog.idle_tick(&colors, |_, _| 0);
        }
        // After the swap the former back one is in front: completely black, with alpha 255.
        assert_eq!(fog.next_row(), 0);
        assert!(fog.front().pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    /// The counter of the control channel counts every finished pass (swap), the immediate rebuild does not.
    #[test]
    fn swaps_count_finished_passes_only() {
        let (colors, image) = terrain(2, 128);
        let mut fog = MinimapFog::new(&image);
        assert_eq!(fog.swaps(), 0);
        for _ in 0..5 {
            fog.idle_tick(&colors, |_, _| 7);
        }
        assert_eq!(fog.swaps(), 0, "five idle ticks: the pass is still running");
        fog.idle_tick(&colors, |_, _| 7);
        assert_eq!(fog.swaps(), 1);
        fog.rebuild_now(&colors, |_, _| 0);
        assert_eq!(fog.swaps(), 1, "the immediate rebuild does not swap");
    }

    #[test]
    fn a_pass_writes_the_rows_of_its_own_ticks() {
        // First pass: only z < 8 (two idle ticks of 4 rows) are explored; the image row is `dim_z - 1 - z`.
        let (colors, image) = terrain(4, 20);
        let mut fog = MinimapFog::new(&image);
        for _ in 0..5 {
            fog.idle_tick(&colors, |_, z| u8::from(z < 8));
        }
        for z in 0..20u32 {
            let expected = fog_pixel(colors[(z * 4 + 2) as usize], u8::from(z < 8));
            assert_eq!(fog.front().get_pixel(2, 19 - z), &expected, "z = {z}");
        }
    }

    #[test]
    fn the_last_tick_of_a_pass_writes_only_the_remaining_rows() {
        // 128 rows, 24 per idle tick: the 6th writes only 8 (120..127), then a new pass begins at 0.
        let (colors, image) = terrain(2, 128);
        let mut fog = MinimapFog::new(&image);
        for _ in 0..5 {
            fog.idle_tick(&colors, |_, _| 7);
        }
        assert_eq!(fog.next_row(), 120);
        fog.idle_tick(&colors, |_, _| 0);
        assert_eq!(fog.next_row(), 0);
        let front = fog.front();
        assert_eq!(
            front.get_pixel(0, 127 - 127).0,
            [0, 0, 0, 255],
            "z = 127 at the very top"
        );
        assert_eq!(front.get_pixel(0, 127 - 120).0, [0, 0, 0, 255]);
        assert_eq!(
            front.get_pixel(0, 127 - 119),
            &fog_pixel(colors[119 * 2], 7)
        );
        // The second pass begins at z = 0 again: after one idle tick only z < 24 is new.
        fog.idle_tick(&colors, |_, _| 7);
        assert_eq!(fog.next_row(), 24);
    }

    #[test]
    fn rebuild_now_writes_both_buffers_and_restarts_the_pass() {
        let (colors, image) = terrain(4, 20);
        let mut fog = MinimapFog::new(&image);
        fog.idle_tick(&colors, |_, _| 0);
        fog.idle_tick(&colors, |_, _| 0);
        assert_eq!(fog.next_row(), 8);
        fog.rebuild_now(&colors, |_, _| 1);
        assert_eq!(fog.next_row(), 0);
        let expected = |x: u32, row: u32| fog_pixel(colors[((19 - row) * 4 + x) as usize], 1);
        for row in 0..20 {
            for x in 0..4 {
                assert_eq!(fog.front().get_pixel(x, row), &expected(x, row));
            }
        }
        // The back one is completely new too: after a whole pass of idle ticks without a change of the level the
        // image stays; it is swapped after 5 idle ticks, before that it already shows the state.
        for _ in 0..4 {
            fog.idle_tick(&colors, |_, _| 1);
            assert_eq!(fog.front().get_pixel(1, 3), &expected(1, 3));
        }
    }

    #[test]
    fn new_shows_the_unfogged_terrain_in_both_buffers() {
        let (colors, image) = terrain(4, 20);
        let mut fog = MinimapFog::new(&image);
        assert_eq!(fog.front(), &image);
        assert_eq!(fog.next_row(), 0);
        // A swap without a change of the level (everything seen) leaves the image the same: the back one was the
        // terrain too.
        for _ in 0..5 {
            fog.idle_tick(&colors, |_, _| 7);
        }
        assert_eq!(fog.front(), &image);
    }
}
