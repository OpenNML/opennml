//! Working grid of the generator: tiles with texture number (`Tile+0x08` during generation,
//! re/random-map Z.6), ID grid `G+0x110` (`FUN_0053FA70`) and heights of the grid points.

/// Tiles `(x, z)` lie at `dim_x·z + x`, grid points `(vx, vz)` at `(dim_x + 1)·vz + vx`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Grid {
    pub(crate) dim_x: usize,
    pub(crate) dim_z: usize,
    /// Texture number per tile (1, 2, ... from `#Texture`), initially -1 (re/random-map Z.5.5).
    pub(crate) tiles: Vec<i32>,
    /// Area ID per tile, initially 0 (`FUN_0053FA70`).
    pub(crate) id_grid: Vec<i32>,
    /// Heights `(dim_x + 1)·(dim_z + 1)`, row by row, x innermost; initially 0.
    pub(crate) heights: Vec<f32>,
}

impl Grid {
    pub(crate) fn new(dim_x: usize, dim_z: usize) -> Self {
        Self {
            dim_x,
            dim_z,
            tiles: vec![-1; dim_x * dim_z],
            id_grid: vec![0; dim_x * dim_z],
            heights: vec![0.0; (dim_x + 1) * (dim_z + 1)],
        }
    }

    pub(crate) fn tile(&self, x: usize, z: usize) -> i32 {
        debug_assert!(x < self.dim_x && z < self.dim_z);
        self.tiles[self.dim_x * z + x]
    }

    pub(crate) fn set_tile(&mut self, x: usize, z: usize, number: i32) {
        debug_assert!(x < self.dim_x && z < self.dim_z);
        self.tiles[self.dim_x * z + x] = number;
    }

    /// Area ID of tile `(x, z)`.
    pub(crate) fn id(&self, x: usize, z: usize) -> i32 {
        debug_assert!(x < self.dim_x && z < self.dim_z);
        self.id_grid[self.dim_x * z + x]
    }

    pub(crate) fn height(&self, vx: usize, vz: usize) -> f32 {
        debug_assert!(vx <= self.dim_x && vz <= self.dim_z);
        self.heights[(self.dim_x + 1) * vz + vx]
    }

    pub(crate) fn set_height(&mut self, vx: usize, vz: usize, height: f32) {
        debug_assert!(vx <= self.dim_x && vz <= self.dim_z);
        self.heights[(self.dim_x + 1) * vz + vx] = height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_empty_and_indexes_rows() {
        let mut grid = Grid::new(4, 2);
        assert_eq!(grid.tiles, vec![-1; 8]);
        assert_eq!(grid.id_grid, vec![0; 8]);
        assert_eq!(grid.heights.len(), 15);
        grid.set_tile(3, 1, 7);
        assert_eq!(grid.tiles[7], 7);
        assert_eq!(grid.tile(3, 1), 7);
        grid.set_height(4, 2, 1.5);
        assert_eq!(grid.heights[14], 1.5);
        assert_eq!(grid.height(4, 2), 1.5);
    }
}
