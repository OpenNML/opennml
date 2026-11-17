//! Regions of a layer (`RegionLayer`, re/path-grid 4): the cells of the layer grid carry the region ID directly
//! (0 unlabeled, `0xFFFF` blocked). Built in 16 x 16 sectors followed by merging at the sector borders (the larger ID
//! wins); blocking refills only in certain corner and diagonal patterns, releasing rewrites the neighbor IDs across
//! the whole map. Separations are therefore detected only sometimes (weakness S5).

use serde::{Deserialize, Serialize};

use super::BLOCKED;

/// Sector size (`RegionLayer+0xA0C/+0xA10`).
pub const SECTOR: i32 = 16;

/// A border pair of the merging (`RegionLayer+0x08 + 0x14·k`).
#[derive(Debug, Clone, Copy)]
struct BorderPair {
    a: u16,
    b: u16,
    cell_a: (i32, i32),
    cell_b: (i32, i32),
}

/// Rings of a Chebyshev ring search around `g` on a `w × h` grid that can contain grid cells: from the distance of
/// the map (0 inside) to the distance of its farthest corner, in `i64` without overflow. Rings before that lie
/// entirely outside, rings after that check no cell at all (every row and column fails its condition `≥ 0` or
/// `< W/H`). For the ring searches of [`RegionLayer::find_free_cell_near`] and [`super::reach`].
pub(crate) fn ring_span(w: i32, h: i32, g: (i32, i32)) -> (i64, i64) {
    let axis = |v: i32, n: i32| {
        let (v, last) = (i64::from(v), i64::from(n) - 1);
        let near = if v < 0 {
            -v
        } else if v > last {
            v - last
        } else {
            0
        };
        (near, v.max(last - v))
    };
    let ((nx, fx), (nz, fz)) = (axis(g.0, w), axis(g.1, h));
    (nx.max(nz), fx.max(fz))
}

/// A layer grid with its regions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionLayer {
    pub width: i32,
    pub height: i32,
    /// Cells row by row (z outer, x inner): 0 free/unlabeled, `0xFFFF` blocked, otherwise ID.
    #[serde(with = "super::cell_bytes")]
    pub cells: Vec<u16>,
    /// ID counter (`+0x15328`) = next new ID.
    pub counter: i32,
}

impl RegionLayer {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            cells: vec![0; (width.max(0) * height.max(0)) as usize],
            counter: 0,
        }
    }

    pub fn inside(&self, x: i32, z: i32) -> bool {
        (0..self.width).contains(&x) && (0..self.height).contains(&z)
    }

    fn idx(&self, x: i32, z: i32) -> usize {
        (self.width * z + x) as usize
    }

    pub fn cell(&self, x: i32, z: i32) -> u16 {
        self.cells[self.idx(x, z)]
    }

    /// `RegionLayer_GetRegionId` (`0x004F7840`): `0xFFFF` outside.
    pub fn region_id(&self, x: i32, z: i32) -> u16 {
        if self.inside(x, z) {
            self.cell(x, z)
        } else {
            BLOCKED
        }
    }

    /// `RegionLayer_FindFreeCellNear` (`0x004F9380`, re/walk 16.7): Chebyshev rings around `(gx, gz)` up to the end
    /// of the ring with the first hit, within the ring the smallest Manhattan distance (tie: the one checked first).
    /// The ring limit comes only from x, and the side columns do not begin until z = 1 (quirks).
    ///
    /// The computation uses `i64`, and only the rings from `ring_span` run: the others contain no map cell, so they
    /// check only cells outside (always blocked) - the result stays the same, and centers far outside neither
    /// overflow nor run endlessly. Deviation (A7, K-110): the original computes in 32 bits; a center with `gx` near
    /// `i32::MIN` overflows there (no search, stack garbage), the reimplementation searches.
    pub fn find_free_cell_near(&self, gx: i32, gz: i32) -> Option<(i32, i32)> {
        let (near, far) = ring_span(self.width, self.height, (gx, gz));
        let (w, h) = (i64::from(self.width), i64::from(self.height));
        let (gx, gz) = (i64::from(gx), i64::from(gz));
        let max_r = gx.max(w - gx - 1).min(far);
        let mut best = i64::MAX;
        let mut found = None;
        // Checked cells lie in x or z between 0 and the center or inside the map, so they fit into
        // `i32`.
        let mut test = |x: i64, z: i64, found: &mut Option<(i32, i32)>| {
            let (xi, zi) = (x as i32, z as i32);
            if self.region_id(xi, zi) != BLOCKED {
                let d = (gx - x).abs() + (gz - z).abs();
                if d < best {
                    best = d;
                    *found = Some((xi, zi));
                }
            }
        };
        let mut r = near;
        while r <= max_r && found.is_none() {
            let (x0, x1, z0, z1) = (gx - r, gx + r, gz - r, gz + r);
            for x in x0.max(0)..=x1.min(w - 1) {
                if z0 >= 0 {
                    test(x, z0, &mut found);
                }
                if z1 < h {
                    test(x, z1, &mut found);
                }
            }
            for z in (z0 + 1).max(1)..z1.min(h) {
                if x0 >= 0 {
                    test(x0, z, &mut found);
                }
                if x1 < w {
                    test(x1, z, &mut found);
                }
            }
            r += 1;
        }
        found
    }

    /// `RegionLayer_FloodFillSector` (`0x004F73C0`): the 4-connected part of the cells with the value of the start
    /// cell inside the sector gets `label` (as `u16`). In the original the game terminates silently if `label` equals
    /// the old value (possible only after a counter overflow, weakness S6); here the sector stays unchanged
    /// (exception A7).
    fn fill_sector(&mut self, sx: i32, sz: i32, x: i32, z: i32, label: i32) {
        let label = label as u16;
        let old = self.cell(x, z);
        if old == label {
            log::warn!(
                "Regions: filling at ({x}, {z}) with the old ID {label} (counter overflow, original terminates)"
            );
            return;
        }
        let start = self.idx(x, z);
        self.cells[start] = label;
        let mut stack = vec![(x, z)];
        while let Some((cx, cz)) = stack.pop() {
            for (nx, nz) in [(cx - 1, cz), (cx + 1, cz), (cx, cz - 1), (cx, cz + 1)] {
                if (sx..sx + SECTOR).contains(&nx) && (sz..sz + SECTOR).contains(&nz) {
                    let i = self.idx(nx, nz);
                    if self.cells[i] == old {
                        self.cells[i] = label;
                        stack.push((nx, nz));
                    }
                }
            }
        }
    }

    /// `RegionLayer_RelabelInSector` (`0x004F7550`): all cells `old` -> `new` in the sector.
    /// `old` is a `u32` argument against `u16` cells in the original: values above `0xFFFF` never hit.
    fn relabel_in_sector(&mut self, sx: i32, sz: i32, old: i32, new: i32) {
        for z in sz..sz + SECTOR {
            for x in sx..sx + SECTOR {
                let i = self.idx(x, z);
                if i32::from(self.cells[i]) == old {
                    self.cells[i] = new as u16;
                }
            }
        }
    }

    /// `RegionLayer_RelabelAll` (`0x004F75D0`): `old` -> `new` across the whole map.
    fn relabel_all(&mut self, old: u16, new: u16) {
        for c in &mut self.cells {
            if *c == old {
                *c = new;
            }
        }
    }

    /// `RegionLayer_LabelAllSectors` (`0x004F78D0`): sectors row by row, within them the cells row by row; every
    /// still unlabeled cell (0) starts a new ID from 1. Width and height are multiples of 16 (16, 64, 128, 256 in the
    /// shipped data).
    pub fn label_all_sectors(&mut self) {
        let mut label = 1;
        for sz in (0..self.height).step_by(SECTOR as usize) {
            for sx in (0..self.width).step_by(SECTOR as usize) {
                for dz in 0..SECTOR {
                    for dx in 0..SECTOR {
                        if self.cell(sx + dx, sz + dz) == 0 {
                            self.fill_sector(sx, sz, sx + dx, sz + dz, label);
                            label += 1;
                        }
                    }
                }
            }
        }
        self.counter = label;
    }

    /// `RegionLayer_CollectBorderPairs` (`0x004F7620`): 16 cell pairs along a sector border; `true` if two different
    /// free IDs meet. If a blocked cell faces a free cell, the sector switches to fill mode.
    fn collect_pairs(
        &self,
        first_a: (i32, i32),
        along_x: bool,
        pairs: &mut Vec<BorderPair>,
        flood: &mut bool,
    ) -> bool {
        let (step, across) = if along_x {
            ((1, 0), (0, 1))
        } else {
            ((0, 1), (1, 0))
        };
        let mut result = false;
        for k in 0..SECTOR {
            let cell_a = (first_a.0 + step.0 * k, first_a.1 + step.1 * k);
            let cell_b = (cell_a.0 + across.0, cell_a.1 + across.1);
            let (a, b) = (self.cell(cell_a.0, cell_a.1), self.cell(cell_b.0, cell_b.1));
            if a == BLOCKED || b == BLOCKED {
                if a != b {
                    *flood = true;
                }
                continue;
            }
            if a == b {
                continue;
            }
            match pairs.iter().position(|p| p.a == a || p.b == a) {
                None => pairs.push(BorderPair {
                    a,
                    b,
                    cell_a,
                    cell_b,
                }),
                Some(i) => {
                    if !pairs[i..].iter().any(|p| p.a == b || p.b == b) {
                        *flood = true;
                        pairs.push(BorderPair {
                            a,
                            b,
                            cell_a,
                            cell_b,
                        });
                    }
                }
            }
            result = true;
        }
        result
    }

    /// `RegionLayer_MergeSectorBorders` (`0x004F8510`): per sector collect the pairs at the top, bottom, left and
    /// right borders and replace the smaller ID with the larger - in the sector of the cell with the smaller ID, in
    /// fill mode only its connected part, otherwise all cells of this ID. Repeated until nothing changes.
    pub fn merge_sector_borders(&mut self) {
        let (w, h) = (self.width, self.height);
        loop {
            let mut changed = false;
            for sz in (0..h).step_by(SECTOR as usize) {
                for sx in (0..w).step_by(SECTOR as usize) {
                    let mut pairs = Vec::new();
                    let mut flood = false;
                    if sz > 0 {
                        changed |= self.collect_pairs((sx, sz - 1), true, &mut pairs, &mut flood);
                    }
                    if sz < h - SECTOR {
                        changed |=
                            self.collect_pairs((sx, sz + SECTOR - 1), true, &mut pairs, &mut flood);
                    }
                    if sx > 0 {
                        changed |= self.collect_pairs((sx - 1, sz), false, &mut pairs, &mut flood);
                    }
                    if sx < w - SECTOR {
                        changed |= self.collect_pairs(
                            (sx + SECTOR - 1, sz),
                            false,
                            &mut pairs,
                            &mut flood,
                        );
                    }
                    for p in pairs {
                        let (cell, small, big) = if p.a > p.b {
                            (p.cell_b, p.b, p.a)
                        } else {
                            (p.cell_a, p.a, p.b)
                        };
                        let (cx, cz) = cell;
                        if flood {
                            self.fill_sector(cx & !15, cz & !15, cx, cz, i32::from(big));
                        } else {
                            self.relabel_in_sector(
                                cx & !15,
                                cz & !15,
                                i32::from(small),
                                i32::from(big),
                            );
                        }
                    }
                }
            }
            if !changed {
                return;
            }
        }
    }

    /// Fills the neighbor with a new ID from the counter.
    fn fill_new(&mut self, (x, z): (i32, i32)) {
        let label = self.counter;
        self.fill_sector(x & !15, z & !15, x, z, label);
    }

    /// `RegionLayer_BlockCell` (`0x004F7B90`): block the cell and - only in certain corner and diagonal patterns of
    /// the neighbors, never at the map edge - fill the neighbors with new IDs. After exactly one fill the old ID is
    /// written back in the sector of the blocked cell; the counter stays incremented.
    pub fn block_cell(&mut self, x: i32, z: i32) {
        if !self.inside(x, z) {
            return;
        }
        let i = self.idx(x, z);
        if self.cells[i] == BLOCKED {
            return;
        }
        self.cells[i] = BLOCKED;
        if x < 1 || x >= self.width - 1 || z < 1 || z >= self.height - 1 {
            return;
        }
        let g = |dx: i32, dz: i32| self.cell(x + dx, z + dz);
        let (u, d, l, r) = (g(0, -1), g(0, 1), g(-1, 0), g(1, 0));
        let (ul, ur, dl, dr) = (g(-1, -1), g(1, -1), g(-1, 1), g(1, 1));
        let b = |v: u16| v == BLOCKED;
        if [u, d, l, r].iter().filter(|&&v| b(v)).count() > 2 {
            return;
        }
        let (up, down, left, right) = ((x, z - 1), (x, z + 1), (x - 1, z), (x + 1, z));
        let mut fills = Fills::default();
        // Decision tree with the jump targets of the original (re/path-grid 4.6).
        let p95 = |s: &mut Self, f: &mut Fills| {
            if b(d) && (b(ul) || b(ur)) {
                s.fill_three(f, [up, left, right], u, true, false);
            } else if b(r) && (b(ul) || b(dl)) {
                s.fill_three(f, [left, up, down], l, true, true);
            } else if !b(u) {
                s.fill_all_four(f, [up, down, left, right], (u, d, l, r), [ul, ur, dl, dr]);
            } else if b(dl) || b(dr) {
                s.fill_three(f, [down, left, right], d, true, true);
            }
        };
        let p62 = |s: &mut Self, f: &mut Fills| {
            if b(ur) || b(dr) {
                s.fill_three(f, [right, up, down], r, false, false);
            } else {
                p95(s, f);
            }
        };
        let p57 = |s: &mut Self, f: &mut Fills| {
            if !b(l) { p95(s, f) } else { p62(s, f) }
        };
        if b(u) {
            if !b(d) {
                p57(self, &mut fills);
            } else if b(l) {
                p62(self, &mut fills);
            } else if b(r) {
                p57(self, &mut fills);
            } else {
                self.fill_two(&mut fills, [left, right], l);
            }
        } else if b(d) {
            p57(self, &mut fills);
        } else if !b(l) {
            p95(self, &mut fills);
        } else if !b(r) {
            p62(self, &mut fills);
        } else {
            self.fill_two(&mut fills, [up, down], u);
        }
        if fills.second <= 0 && fills.third <= 0 && fills.fourth <= 0 {
            self.relabel_in_sector(x & !15, z & !15, fills.first, i32::from(fills.origin));
        }
    }

    /// "LR"/"UD" (`0x004F7D0B`, `0x004F7DBA`): two opposite neighbors.
    fn fill_two(&mut self, f: &mut Fills, [one, two]: [(i32, i32); 2], origin: u16) {
        f.origin = origin;
        self.fill_new(one);
        f.first = self.counter;
        self.counter = self.counter.wrapping_add(1);
        if i32::from(self.cell(two.0, two.1)) == f.first {
            return;
        }
        f.second = self.counter;
        self.fill_new(two);
        self.counter = self.counter.wrapping_add(1);
    }

    /// "R/U/L/D first" (`0x004F7E78`, `0x004F7FB8`, `0x004F80C1`, `0x004F8210`).
    fn fill_three(
        &mut self,
        f: &mut Fills,
        [first, second, third]: [(i32, i32); 3],
        origin: u16,
        zero_check_2: bool,
        zero_check_3: bool,
    ) {
        f.origin = origin;
        self.fill_new(first);
        f.first = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let v = i32::from(self.cell(second.0, second.1));
        if (!zero_check_2 || v != 0) && v != i32::from(BLOCKED) && v != f.first {
            f.second = self.counter;
            self.fill_new(second);
            self.counter = self.counter.wrapping_add(1);
        }
        let v = i32::from(self.cell(third.0, third.1));
        if (zero_check_3 && v == 0) || v == i32::from(BLOCKED) || v == f.first || v == f.second {
            return;
        }
        f.third = self.counter;
        self.fill_new(third);
        self.counter = self.counter.wrapping_add(1);
    }

    /// "all four" (`0x004F8306`/`0x004F8359`): only without blocked orthogonal neighbors and with at least two
    /// blocked diagonals.
    fn fill_all_four(
        &mut self,
        f: &mut Fills,
        [up, down, left, right]: [(i32, i32); 4],
        (_, d, l, r): (u16, u16, u16, u16),
        diagonals: [u16; 4],
    ) {
        if d == BLOCKED || l == BLOCKED || r == BLOCKED {
            return;
        }
        if diagonals.iter().filter(|&&v| v == BLOCKED).count() <= 1 {
            return;
        }
        f.origin = self.cell(up.0, up.1);
        self.fill_new(up);
        f.first = self.counter;
        self.counter = self.counter.wrapping_add(1);
        if i32::from(self.cell(down.0, down.1)) != f.first {
            f.second = self.counter;
            self.fill_new(down);
            self.counter = self.counter.wrapping_add(1);
        }
        let v = i32::from(self.cell(left.0, left.1));
        if v != f.first && v != f.second {
            f.third = self.counter;
            self.fill_new(left);
            self.counter = self.counter.wrapping_add(1);
        }
        let v = i32::from(self.cell(right.0, right.1));
        if v == f.first || v == f.second || v == f.third {
            return;
        }
        f.fourth = self.counter;
        self.fill_new(right);
        self.counter = self.counter.wrapping_add(1);
    }

    /// `RegionLayer_UnblockCell` (`0x004F7A20`): ID of the first free neighbor (order x+1, x-1, z+1, z-1), rewrite
    /// the remaining neighbor IDs across the whole map to it; without a free neighbor a new ID.
    pub fn unblock_cell(&mut self, x: i32, z: i32) {
        if !self.inside(x, z) || self.cell(x, z) != BLOCKED {
            return;
        }
        let neighbours = [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)].map(|(nx, nz)| {
            if self.inside(nx, nz) {
                self.cell(nx, nz)
            } else {
                BLOCKED
            }
        });
        let i = self.idx(x, z);
        match neighbours.iter().copied().find(|&v| v != BLOCKED) {
            None => {
                self.cells[i] = self.counter as u16;
                self.counter = self.counter.wrapping_add(1);
            }
            Some(first) => {
                self.cells[i] = first;
                for v in neighbours {
                    if v != BLOCKED && v != first {
                        self.relabel_all(v, first);
                    }
                }
            }
        }
    }

    /// `RegionLayer_SetCellBlocked` (`0x004F95C0`).
    pub fn set_cell_blocked(&mut self, x: i32, z: i32, blocked: bool) {
        if blocked {
            self.block_cell(x, z);
        } else {
            self.unblock_cell(x, z);
        }
    }
}

/// New IDs of the fills in `BlockCell` (`+0xA14…+0xA24`; 0 = no fill).
#[derive(Default)]
struct Fills {
    first: i32,
    second: i32,
    third: i32,
    fourth: i32,
    /// ID of the neighbor filled first, before filling (`+0xA24`).
    origin: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_cells_are_found_in_rings_and_row_zero_is_skipped_on_the_sides() {
        // re/walk 16.7: (width, height, blocked, start) -> result.
        let layer = |w: i32, h: i32, blocked: &[(i32, i32)]| {
            let mut l = RegionLayer::new(w, h);
            l.cells.fill(1);
            for &(x, z) in blocked {
                l.cells[(z * w + x) as usize] = BLOCKED;
            }
            l
        };
        for (w, h, blocked, start, want) in [
            (5, 5, vec![], (2, 2), Some((2, 2))),
            (5, 5, vec![(2, 2)], (2, 2), Some((2, 1))),
            (5, 5, vec![(2, 2), (2, 1)], (2, 2), Some((2, 3))),
            (5, 5, vec![(2, 2), (2, 1), (2, 3)], (2, 2), Some((1, 2))),
            (5, 5, vec![(0, 0)], (0, 0), Some((0, 1))),
            (5, 5, vec![(0, 0), (0, 1), (1, 1)], (0, 0), Some((0, 2))),
            (
                5,
                5,
                vec![(0, 0), (0, 1), (1, 1), (0, 2), (1, 2), (2, 2)],
                (0, 0),
                Some((2, 1)),
            ),
            (
                3,
                9,
                (0..3).flat_map(|x| (3..6).map(move |z| (x, z))).collect(),
                (1, 4),
                None,
            ),
            (9, 1, vec![(4, 0)], (4, 0), None),
            (9, 2, vec![(4, 0)], (4, 0), Some((4, 1))),
            (9, 3, vec![(4, 1)], (4, 1), Some((4, 0))),
        ] {
            let l = layer(w, h, &blocked);
            assert_eq!(
                l.find_free_cell_near(start.0, start.1),
                want,
                "{w}x{h} {blocked:?}"
            );
        }
    }

    /// `find_free_cell_near` before bounding: reference for the bounded ring run.
    fn old_free_cell_near(l: &RegionLayer, gx: i32, gz: i32) -> Option<(i32, i32)> {
        let (w, h) = (l.width, l.height);
        let max_r = gx.max(w - gx - 1);
        let mut best = i32::MAX;
        let mut found = None;
        let mut test = |x: i32, z: i32, found: &mut Option<(i32, i32)>| {
            if l.region_id(x, z) != BLOCKED {
                let d = (gx - x).abs() + (gz - z).abs();
                if d < best {
                    best = d;
                    *found = Some((x, z));
                }
            }
        };
        let mut r = 0;
        while r <= max_r && found.is_none() {
            let (x0, x1, z0, z1) = (gx - r, gx + r, gz - r, gz + r);
            for x in x0.max(0)..=x1.min(w - 1) {
                if z0 >= 0 {
                    test(x, z0, &mut found);
                }
                if z1 < h {
                    test(x, z1, &mut found);
                }
            }
            for z in (z0 + 1).max(1)..z1.min(h) {
                if x0 >= 0 {
                    test(x0, z, &mut found);
                }
                if x1 < w {
                    test(x1, z, &mut found);
                }
            }
            r += 1;
        }
        found
    }

    #[test]
    fn the_bounded_free_cell_search_finds_what_the_old_one_found() {
        // Rings without a map cell are skipped; same result for centers inside the map and up to 40 cells
        // before and behind it, also on narrow maps.
        for (w, h) in [(24, 24), (9, 30), (30, 9)] {
            let mut l = RegionLayer::new(w, h);
            l.cells.fill(1);
            for i in 0..w.min(h) {
                if i % 3 != 0 {
                    l.cells[(i * w + i) as usize] = BLOCKED;
                    l.cells[(i * w + (w - 1 - i)) as usize] = BLOCKED;
                }
            }
            for gx in -40..w + 40 {
                for gz in -40..h + 40 {
                    assert_eq!(
                        l.find_free_cell_near(gx, gz),
                        old_free_cell_near(&l, gx, gz),
                        "{w}x{h} around ({gx}, {gz})"
                    );
                }
            }
        }
    }

    const W: i32 = 32;

    fn fresh() -> RegionLayer {
        let mut l = RegionLayer::new(W, W);
        l.label_all_sectors();
        l.merge_sector_borders();
        l
    }

    fn ids(l: &RegionLayer) -> Vec<u16> {
        let mut v: Vec<u16> = l.cells.clone();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn block_merging(l: &mut RegionLayer, cells: impl IntoIterator<Item = (i32, i32)>) {
        for (x, z) in cells {
            l.block_cell(x, z);
            l.merge_sector_borders();
        }
    }

    fn ring(x0: i32, x1: i32, z0: i32, z1: i32) -> Vec<(i32, i32)> {
        let mut r: Vec<_> = (x0..=x1).map(|x| (x, z0)).collect();
        r.extend((z0 + 1..=z1).map(|z| (x1, z)));
        r.extend((x0..x1).rev().map(|x| (x, z1)));
        r.extend((z0 + 1..z1).rev().map(|z| (x0, z)));
        r
    }

    #[test]
    fn sectors_get_their_own_ids_and_the_larger_one_wins_the_merge() {
        // T1: four sectors 1-4, after merging everything 4.
        let mut l = RegionLayer::new(W, W);
        l.label_all_sectors();
        assert_eq!(
            [
                l.cell(0, 0),
                l.cell(16, 0),
                l.cell(0, 16),
                l.cell(16, 16),
                l.counter as u16
            ],
            [1, 2, 3, 4, 5]
        );
        l.merge_sector_borders();
        assert_eq!((ids(&l), l.counter), (vec![4], 5));
    }

    #[test]
    fn straight_walls_never_split_but_inner_islands_do() {
        // T2: wall from edge to edge - no fill, the separation stays undetected (S5).
        let mut l = fresh();
        block_merging(&mut l, (0..32).map(|z| (8, z)));
        assert_eq!((l.cell(3, 5), l.cell(20, 5), l.counter), (4, 4, 5));
        // T3 + releasing: the cell takes over the ID of its neighbor.
        let mut l = fresh();
        block_merging(&mut l, (0..16).map(|z| (8, z)));
        assert_eq!((ids(&l), l.counter), (vec![4, BLOCKED], 5));
        l.unblock_cell(8, 7);
        assert_eq!((l.cell(8, 7), l.counter), (4, 5));
        // T4: ring inside a sector - island detected.
        let mut l = fresh();
        block_merging(&mut l, ring(3, 7, 3, 7));
        assert_eq!((l.cell(5, 5), l.cell(1, 1), l.counter), (6, 5, 7));
        // T9: open the ring again - rewritten globally to the ID of the first free neighbor.
        l.unblock_cell(5, 3);
        assert_eq!((l.cell(5, 3), l.cell(1, 1), l.counter), (6, 6, 7));
        // T5: ring across a sector border - not detected.
        let mut l = fresh();
        block_merging(&mut l, ring(13, 18, 3, 7));
        assert_eq!(
            (l.cell(15, 5), l.cell(16, 5), l.cell(1, 1), l.counter),
            (6, 6, 6, 7)
        );
    }

    #[test]
    fn a_wall_closed_inside_a_sector_splits_the_map() {
        // T7: wall x = 8, the last cell (8, 10) lies inside the sector.
        let mut l = fresh();
        let mut cells: Vec<_> = (0..32).filter(|&z| z != 10).map(|z| (8, z)).collect();
        block_merging(&mut l, cells.drain(..));
        l.block_cell(8, 10);
        assert_eq!(
            (l.cell(7, 10), l.cell(9, 10), l.cell(3, 20), l.counter),
            (5, 6, 4, 7)
        );
        l.merge_sector_borders();
        assert_eq!((l.cell(3, 5), l.cell(20, 5), l.cell(3, 20)), (5, 6, 5));
        // T8: if the wall ends before the edge, everything stays one region.
        let mut l = fresh();
        block_merging(&mut l, (1..31).map(|z| (8, z)));
        assert_eq!((ids(&l), l.counter), (vec![4, BLOCKED], 5));
        // T10: a blocked diagonal triggers no fill.
        let mut l = fresh();
        block_merging(&mut l, [(5, 5)]);
        l.block_cell(6, 6);
        assert_eq!(l.counter, 5);
    }

    #[test]
    fn blocked_borders_and_labels_outside_the_map() {
        let mut l = fresh();
        assert_eq!(l.region_id(-1, 0), BLOCKED);
        assert_eq!(l.region_id(0, 32), BLOCKED);
        // Edge: only block, no fill.
        l.block_cell(0, 5);
        l.block_cell(0, 5);
        l.block_cell(40, 5);
        assert_eq!((l.cell(0, 5), l.counter), (BLOCKED, 5));
        // Releasing without free neighbors: new ID from the counter.
        let mut l = RegionLayer::new(16, 16);
        l.cells.fill(BLOCKED);
        l.unblock_cell(4, 4);
        assert_eq!((l.cell(4, 4), l.counter), (0, 1));
    }

    #[test]
    fn a_counter_overflow_onto_the_old_id_leaves_the_sector_alone() {
        // After a counter overflow the new ID (as u16) hits the old one: the original terminates silently, here the
        // sector stays unchanged (exception A7).
        let mut l = fresh();
        l.fill_sector(0, 0, 3, 3, 0x1_0004);
        assert_eq!((ids(&l), l.cell(3, 3)), (vec![4], 4));
        // IDs above 0xFFFF are written as u16: 0x1_0007 gives 7.
        l.fill_sector(0, 0, 3, 3, 0x1_0007);
        assert_eq!((l.cell(3, 3), l.cell(16, 0)), (7, 4));
    }
}
