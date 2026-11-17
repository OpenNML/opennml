//! The path grid (`PathPlanner`, `GameEngine+0x298`, re/path-grid 3): nine layers with one cell per terrain tile each,
//! a dirty rectangle with a layer mask from which the derived layers 5-8 (2 x 2, 3 x 3) are recomputed on flushing,
//! and the regions. Before the region build, cells are written raw (0/`0xFFFF`); afterwards by blocking/releasing the
//! regions.

use serde::{Deserialize, Serialize};

use super::region::RegionLayer;
use super::{BLOCKED, LAYER_COUNT, LAYER_MASKS, base_layer};
use crate::fixed_vec::FixedVec;

/// Costs of a free cell, a blocked cell and a cell outside (`PathPlanner_GetCellCost`).
pub const COST_FREE: u32 = 0;
pub const COST_BLOCKED: u32 = 0x3FFF_FFFE;
pub const COST_OUTSIDE: u32 = 0x3FFF_FFFF;

/// Dirty rectangle (`PathPlanner+0xEC…+0xFC`): unclamped, with the layer mask of the open change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyRect {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub mask: u32,
}

impl Default for DirtyRect {
    fn default() -> Self {
        Self {
            min_x: 0x10000,
            min_z: 0x10000,
            max_x: -1,
            max_z: -1,
            mask: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathGrid {
    pub width: i32,
    pub height: i32,
    /// Layers 0-8 with their regions.
    pub layers: FixedVec<RegionLayer, LAYER_COUNT>,
    /// Regions built (`+0xB0`).
    pub built: bool,
    pub dirty: DirtyRect,
}

/// A layer for reading, optionally with one temporarily overwritten cell (`PathPlanner_TempOverrideCell`
/// `0x004F6C40`; the original writes it into the grid and resets it after planning).
#[derive(Debug, Clone, Copy)]
pub struct LayerView<'a> {
    pub layer: &'a RegionLayer,
    pub patch: Option<((i32, i32), u16)>,
}

impl LayerView<'_> {
    pub fn width(&self) -> i32 {
        self.layer.width
    }

    pub fn height(&self) -> i32 {
        self.layer.height
    }

    pub fn inside(&self, x: i32, z: i32) -> bool {
        self.layer.inside(x, z)
    }

    /// Region ID; `0xFFFF` outside.
    pub fn region_id(&self, x: i32, z: i32) -> u16 {
        match self.patch {
            Some((cell, value)) if cell == (x, z) && self.inside(x, z) => value,
            _ => self.layer.region_id(x, z),
        }
    }

    /// `PathPlanner_GetCellCost` (`0x004F6DA0`) for this layer.
    pub fn cost(&self, x: i32, z: i32) -> u32 {
        if !self.inside(x, z) {
            COST_OUTSIDE
        } else if self.region_id(x, z) == BLOCKED {
            COST_BLOCKED
        } else {
            COST_FREE
        }
    }
}

impl PathGrid {
    /// `PathPlanner_Init` (`0x004F67A0`): nine empty grids, regions not built.
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            layers: FixedVec::from_fn(|_| RegionLayer::new(width, height)),
            built: false,
            dirty: DirtyRect::default(),
        }
    }

    pub fn inside(&self, x: i32, z: i32) -> bool {
        (0..self.width).contains(&x) && (0..self.height).contains(&z)
    }

    pub fn view(&self, layer: usize) -> LayerView<'_> {
        LayerView {
            layer: &self.layers[layer],
            patch: None,
        }
    }

    /// `PathPlanner_GetCellCost` (`0x004F6DA0`): layer outside 0-8 -> 0.
    pub fn cell_cost(&self, x: i32, z: i32, layer: usize) -> u32 {
        if layer >= LAYER_COUNT {
            return COST_FREE;
        }
        self.view(layer).cost(x, z)
    }

    /// `PathPlanner_GetRegionId` (`0x004F6970`): outside or layer outside 0-8 -> `0xFFFF`.
    pub fn region_id(&self, layer: usize, x: i32, z: i32) -> u16 {
        if layer >= LAYER_COUNT {
            return BLOCKED;
        }
        self.layers[layer].region_id(x, z)
    }

    /// `PathPlanner_MarkCell` (`0x004F71D0`): extend the dirty rectangle (flush it first if the mask differs), then
    /// set the cell in every layer of the mask (bit i = layer i).
    pub fn mark_cell(&mut self, mask: u32, x: i32, z: i32, blocked: bool) {
        self.extend_dirty_rect(mask, x, z);
        self.set_cell_blocked(mask, x, z, blocked);
    }

    /// `PathPlanner_ExtendDirtyRect` (`0x004F7150`), without clamping to the map.
    fn extend_dirty_rect(&mut self, mask: u32, x: i32, z: i32) {
        if self.dirty.mask != mask {
            self.flush_dirty_rect();
        }
        let d = &mut self.dirty;
        d.mask = mask;
        d.min_x = d.min_x.min(x);
        d.max_x = d.max_x.max(x);
        d.min_z = d.min_z.min(z);
        d.max_z = d.max_z.max(z);
    }

    /// `PathPlanner_SetCellBlocked` (`0x004F6D00`): after the region build via the regions, before that raw
    /// (`PathPlanner_WriteCellRaw` `0x004F6B00`: 0 or `0xFFFF`, only inside the map).
    fn set_cell_blocked(&mut self, mask: u32, x: i32, z: i32, blocked: bool) {
        for (i, layer) in self.layers.iter_mut().enumerate() {
            if mask & (1 << i) == 0 {
                continue;
            }
            if self.built {
                layer.set_cell_blocked(x, z, blocked);
            } else if layer.inside(x, z) {
                let index = (layer.width * z + x) as usize;
                layer.cells[index] = if blocked { BLOCKED } else { 0 };
            }
        }
    }

    /// Write raw without a dirty rectangle (loading, before the region build).
    pub(crate) fn set_raw(&mut self, mask: u32, x: i32, z: i32, blocked: bool) {
        debug_assert!(!self.built);
        self.set_cell_blocked(mask, x, z, blocked);
    }

    /// Mark the whole map as changed (loading).
    pub(crate) fn mark_whole_map_dirty(&mut self) {
        self.dirty = DirtyRect {
            min_x: 0,
            min_z: 0,
            max_x: self.width - 1,
            max_z: self.height - 1,
            mask: 0x1F,
        };
    }

    /// `PathPlanner_FlushDirtyRect` (`0x004F70D0`): recompute all derived layers in the rectangle +-1 (independent
    /// of the mask), merge after the region build, clear the rectangle.
    pub fn flush_dirty_rect(&mut self) {
        if self.dirty.mask == 0 {
            return;
        }
        for (layer, &m) in LAYER_MASKS.iter().enumerate().skip(5) {
            if m & 2 != 0 {
                self.derive(layer, false);
            }
            if m & 4 != 0 {
                self.derive(layer, true);
            }
        }
        if self.built {
            for layer in &mut self.layers {
                layer.merge_sector_borders();
            }
        }
        self.dirty = DirtyRect::default();
    }

    /// `PathPlanner_DeriveLayer2x2` (`0x004F6E00`) or `…3x3` (`0x004F6F30`): cell blocked if a cell of the window in
    /// the base layer is blocked or outside; 2 x 2 = (x-1...x, z-1...z), 3 x 3 = (x-1...x+1, z-1...z+1). z outer,
    /// x inner.
    fn derive(&mut self, layer: usize, three: bool) {
        let base = base_layer(layer);
        let d = self.dirty;
        for z in d.min_z - 1..=d.max_z + 1 {
            for x in d.min_x - 1..=d.max_x + 1 {
                let (x0, z0) = (x - 1, z - 1);
                let (x1, z1) = if three { (x + 1, z + 1) } else { (x, z) };
                let view = self.view(base);
                let blocked =
                    (z0..=z1).any(|wz| (x0..=x1).any(|wx| view.cost(wx, wz) != COST_FREE));
                self.set_cell_blocked(1 << layer, x, z, blocked);
            }
        }
    }

    /// `PathPlanner_BuildRegions` (`0x004F68D0`): label and merge each layer of the mask; does not clear the dirty
    /// rectangle.
    pub fn build_regions(&mut self, mask: u32) {
        for (i, layer) in self.layers.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                layer.counter = 0;
                layer.label_all_sectors();
                layer.merge_sector_borders();
            }
        }
        self.built = true;
    }

    /// Blocked cells of a layer (checks, display).
    pub fn blocked_count(&self, layer: usize) -> usize {
        self.layers[layer]
            .cells
            .iter()
            .filter(|&&c| c == BLOCKED)
            .count()
    }

    /// Distinct regions of a layer (without the blocked mark).
    pub fn region_count(&self, layer: usize) -> usize {
        let mut ids: Vec<u16> = self.layers[layer]
            .cells
            .iter()
            .copied()
            .filter(|&c| c != BLOCKED)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_layers_block_the_border_and_follow_their_base() {
        // T6: 32 x 32 free; the derived layers block the edge (outside = blocked).
        let mut g = PathGrid::new(32, 32);
        for z in 0..32 {
            for x in 0..32 {
                g.mark_cell(0x1F, x, z, false);
            }
        }
        g.flush_dirty_rect();
        g.build_regions(u32::MAX);
        assert_eq!((g.blocked_count(5), g.blocked_count(8)), (63, 124));
        assert_eq!(
            g.layers.iter().map(|l| l.counter).collect::<Vec<_>>(),
            [5; 9]
        );
        assert_eq!(g.region_count(0), 1);
        assert_eq!(g.layers[0].cell(3, 3), 4);
        g.mark_cell(0x1, 10, 10, true);
        g.flush_dirty_rect();
        assert_eq!((g.blocked_count(5), g.region_count(5)), (67, 1));
        g.mark_cell(0x1, 10, 10, false);
        g.flush_dirty_rect();
        assert_eq!(g.blocked_count(5), 63);
        assert_eq!(
            g.layers.iter().map(|l| l.counter).collect::<Vec<_>>(),
            [5; 9]
        );
    }

    #[test]
    fn costs_regions_and_mask_changes() {
        let mut g = PathGrid::new(16, 16);
        g.mark_cell(0x1, 2, 3, true);
        // Other mask: flush first, then the new rectangle.
        g.mark_cell(0x10, 5, 5, true);
        assert_eq!(g.dirty.mask, 0x10);
        assert_eq!((g.dirty.min_x, g.dirty.max_z), (5, 5));
        assert_eq!(g.cell_cost(2, 3, 0), COST_BLOCKED);
        assert_eq!(g.cell_cost(2, 4, 0), COST_FREE);
        assert_eq!(g.cell_cost(-1, 4, 0), COST_OUTSIDE);
        assert_eq!(g.cell_cost(2, 3, 9), COST_FREE);
        assert_eq!(g.region_id(9, 2, 3), BLOCKED);
        // Layer 5 (2 x 2 from layer 0) caught up at the first flush.
        assert_eq!(g.region_id(5, 3, 4), BLOCKED);
        // Outside, only the rectangle is extended.
        g.mark_cell(0x10, 40, 40, true);
        assert_eq!(g.dirty.max_x, 40);
        let v = LayerView {
            layer: &g.layers[0],
            patch: Some(((2, 3), 7)),
        };
        assert_eq!((v.region_id(2, 3), v.cost(2, 3)), (7, COST_FREE));
    }
}
