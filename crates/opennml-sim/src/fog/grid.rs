//! Fog grids (`BitGrid`, one byte per tile, bit p = player p; re/vision 1.1), circle mask
//! (`GameWorld_BuildFogCircleMask` `0x00476660`) and stamp (`BitGrid_StampScaledMask`
//! `0x0040AC10`, re/vision 2).

use serde::{Deserialize, Serialize};

/// Side of the circle mask (61 x 61 bits).
pub const MASK_SIDE: i32 = 61;

/// Byte grid in map size, row by row (`width · z + x`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteGrid {
    pub width: i32,
    pub height: i32,
    #[serde(with = "serde_bytes")]
    pub cells: Vec<u8>,
}

impl ByteGrid {
    /// Empty grid (`BitGrid_Init`); negative sides count as 0.
    pub fn new(width: i32, height: i32) -> Self {
        let (w, h) = (width.max(0), height.max(0));
        Self {
            width: w,
            height: h,
            cells: vec![0; w as usize * h as usize],
        }
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        ((0..self.width).contains(&x) && (0..self.height).contains(&z))
            .then(|| self.width as usize * z as usize + x as usize)
    }

    /// Byte of the tile; outside `None`.
    pub fn get(&self, x: i32, z: i32) -> Option<u8> {
        self.index(x, z).map(|i| self.cells[i])
    }

    /// ORs bits in; no effect outside.
    pub fn or(&mut self, x: i32, z: i32, bits: u8) {
        if let Some(i) = self.index(x, z) {
            self.cells[i] |= bits;
        }
    }

    /// Clears bits (`&= !bits`); no effect outside.
    pub fn clear_bits(&mut self, x: i32, z: i32, bits: u8) {
        if let Some(i) = self.index(x, z) {
            self.cells[i] &= !bits;
        }
    }

    /// `BitGrid_Clear`.
    pub fn clear(&mut self) {
        self.cells.fill(0);
    }

    /// `BitGrid_Fill` (`0x0040AE20`).
    pub fn fill(&mut self, value: u8) {
        self.cells.fill(value);
    }

    /// `BitGrid_CopyFrom` (`0x0040ADE0`).
    pub fn copy_from(&mut self, other: &ByteGrid) {
        self.clone_from(other);
    }

    /// `BitGrid_AnyBitInRect` (`0x00474F20`): rectangle `[x0, z0, x1, z1]` inclusive, clamped to the grid;
    /// any tile with one of the `bits`.
    pub fn any_in_rect(&self, rect: [i32; 4], bits: u8) -> bool {
        let (x0, z0) = (rect[0].max(0), rect[1].max(0));
        let (x1, z1) = (rect[2].min(self.width - 1), rect[3].min(self.height - 1));
        (z0..=z1).any(|z| {
            let row = self.width as usize * z as usize;
            (x0..=x1).any(|x| self.cells[row + x as usize] & bits != 0)
        })
    }

    /// Tiles with one of the `bits` (tests, display, control channel).
    pub fn count(&self, bits: u8) -> usize {
        self.cells.iter().filter(|&&c| c & bits != 0).count()
    }
}

/// Bit of the circle mask for `x, y` in `0..61`: the floating-point loop of the original
/// (`⌊√((j − 30.5)² + (i − 30.5)²)⌋ ≤ 30`, mirrored four times) as an equivalent integer rule.
pub fn circle_bit(x: i32, y: i32) -> bool {
    let (i, j) = (x.min(60 - x), y.min(60 - y));
    (2 * i - 61).pow(2) + (2 * j - 61).pow(2) < 3844
}

/// `BitGrid_StampScaledMask(x0, z0, mask, w, h, bits, 0)`: the circle mask sampled with "nearest neighbor"
/// onto `w x h` tiles (sides clamped to 2...256, quirk) and ORed into the grid; whatever lies outside is
/// dropped. Only rows and columns inside the grid are iterated - equivalent to skipping per tile, without
/// overflow at extreme corners.
pub fn stamp(grid: &mut ByteGrid, x0: i32, z0: i32, w: i32, h: i32, bits: u8) {
    let (w, h) = (w.clamp(2, 256), h.clamp(2, 256));
    let (j0, j1) = (
        z0.saturating_neg().clamp(0, h),
        grid.height.saturating_sub(z0).clamp(0, h),
    );
    let (i0, i1) = (
        x0.saturating_neg().clamp(0, w),
        grid.width.saturating_sub(x0).clamp(0, w),
    );
    for j in j0..j1 {
        let my = (h / 2 + j * MASK_SIDE) / h;
        let row = (z0 + j) as usize * grid.width as usize;
        for i in i0..i1 {
            let mx = (w / 2 + i * MASK_SIDE) / w;
            if circle_bit(mx, my) {
                grid.cells[row + (x0 + i) as usize] |= bits;
            }
        }
    }
}

/// Stamp side from the sight in quarter tiles (re/vision 2.3): `sight / 2` (truncated), even sides +1;
/// `None` for side <= 0 (no stamp, does not count toward the budget).
pub fn stamp_side(sight: i32) -> Option<i32> {
    let s = sight / 2;
    (s > 0).then_some(if s % 2 == 0 { s + 1 } else { s })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiles with `bits` and their enclosing rectangle `[x0, z0, x1, z1]`.
    fn cover(g: &ByteGrid, bits: u8) -> (usize, [i32; 4]) {
        let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for z in 0..g.height {
            for x in 0..g.width {
                if g.get(x, z).unwrap() & bits != 0 {
                    b = [b[0].min(x), b[1].min(z), b[2].max(x), b[3].max(z)];
                }
            }
        }
        (g.count(bits), b)
    }

    #[test]
    fn the_circle_mask_has_2901_bits_and_is_mirrored() {
        let n = (0..61)
            .flat_map(|y| (0..61).map(move |x| (x, y)))
            .filter(|&(x, y)| circle_bit(x, y))
            .count();
        assert_eq!(n, 2901);
        assert!(circle_bit(0, 30) && circle_bit(60, 30) && circle_bit(30, 0));
        assert!(!circle_bit(0, 0) && !circle_bit(60, 60) && !circle_bit(0, 24));
    }

    #[test]
    fn stamps_cover_the_tiles_of_the_table_in_vision_md() {
        // (sight, seen, explored, rectangle seen, rectangle explored) at tile (100, 100).
        let table = [
            (2, 3, 16, [100, 100, 101, 101], [99, 99, 102, 102]),
            (3, 3, 16, [100, 100, 101, 101], [99, 99, 102, 102]),
            (4, 4, 37, [100, 100, 101, 101], [97, 97, 103, 103]),
            (8, 16, 64, [99, 99, 102, 102], [96, 96, 104, 104]),
            (16, 64, 129, [96, 96, 104, 104], [94, 94, 106, 106]),
            (32, 224, 345, [92, 92, 108, 108], [90, 90, 110, 110]),
            (56, 654, 853, [86, 86, 114, 114], [84, 84, 116, 116]),
        ];
        for (sight, seen, explored, seen_box, explored_box) in table {
            let s = stamp_side(sight).unwrap();
            let (x0, z0) = (100 - s / 2, 100 - s / 2);
            let mut a = ByteGrid::new(200, 200);
            let mut b = ByteGrid::new(200, 200);
            stamp(&mut a, x0, z0, s, s, 1);
            stamp(&mut b, x0 - 2, z0 - 2, s + 4, s + 4, 1);
            assert_eq!(cover(&a, 1), (seen, seen_box), "seen, sight {sight}");
            assert_eq!(
                cover(&b, 1),
                (explored, explored_box),
                "explored, sight {sight}"
            );
        }
        assert_eq!([stamp_side(1), stamp_side(0), stamp_side(-4)], [None; 3]);
        assert_eq!(
            [stamp_side(32), stamp_side(33), stamp_side(34)],
            [Some(17), Some(17), Some(17)]
        );
    }

    #[test]
    fn side_5_leaves_the_first_row_and_column_empty() {
        // Quirk: sampling with nearest neighbor: at side 5, row 0 and column 0 drop out.
        let mut g = ByteGrid::new(10, 10);
        stamp(&mut g, 0, 0, 5, 5, 4);
        assert_eq!(cover(&g, 4), (16, [1, 1, 4, 4]));
    }

    #[test]
    fn stamps_clip_at_the_border_and_sides_clamp_to_2_and_256() {
        let mut g = ByteGrid::new(64, 64);
        stamp(&mut g, -5, -5, 17, 17, 1);
        assert_eq!(g.count(1), 134);
        // Side 1 becomes 2 (not centered).
        let mut g = ByteGrid::new(10, 10);
        stamp(&mut g, 0, 0, 1, 1, 2);
        let tiles: Vec<_> = (0..4).filter(|&i| g.get(i % 2, i / 2) == Some(2)).collect();
        assert_eq!(tiles, [1, 2, 3]);
        // Clamped above 256.
        let (mut a, mut b) = (ByteGrid::new(300, 300), ByteGrid::new(300, 300));
        stamp(&mut a, 0, 0, 300, 300, 1);
        stamp(&mut b, 0, 0, 256, 256, 1);
        assert_eq!((a.count(1), a), (50855, b));
        // Entirely outside and with extreme values: nothing, no panic.
        let mut g = ByteGrid::new(8, 8);
        for (x0, z0) in [
            (i32::MAX, 0),
            (i32::MIN, i32::MIN),
            (0, i32::MAX - 3),
            (100, 100),
        ] {
            stamp(&mut g, x0, z0, i32::MAX, i32::MIN, 1);
        }
        assert_eq!(g.count(0xFF), 0);
    }

    #[test]
    fn grids_or_clear_fill_copy_and_find_bits_in_clamped_rects() {
        let mut g = ByteGrid::new(4, 3);
        g.or(1, 1, 0b01);
        g.or(1, 1, 0b10);
        g.or(4, 0, 0xFF);
        g.or(-1, 0, 0xFF);
        assert_eq!(
            (g.get(1, 1), g.get(4, 0), g.count(0xFF)),
            (Some(3), None, 1)
        );
        assert!(g.any_in_rect([-5, -5, 1, 1], 2) && !g.any_in_rect([2, 0, 9, 9], 0xFF));
        assert!(!g.any_in_rect([1, 1, 0, 1], 0xFF), "swapped bounds: empty");
        let mut h = ByteGrid::new(4, 3);
        h.copy_from(&g);
        assert_eq!(h, g);
        h.fill(0xFF);
        assert_eq!(h.count(0x80), 12);
        h.clear();
        assert_eq!(h.count(0xFF), 0);
        assert_eq!(ByteGrid::new(-3, 5).cells.len(), 0);
    }
}
