//! Occupancy grid (`GameWorld+0x1FC0`, re/occupancy 4): one bit per terrain tile, read by the building site check and
//! the flag search (not by the pathfinding).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccupancyGrid {
    pub width: i32,
    pub height: i32,
    /// 0/1 per tile, row by row.
    #[serde(with = "serde_bytes")]
    pub cells: Vec<u8>,
}

impl OccupancyGrid {
    /// `BitGrid_Init` (`0x00407710`): everything free.
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            cells: vec![0; (width.max(0) * height.max(0)) as usize],
        }
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        ((0..self.width).contains(&x) && (0..self.height).contains(&z))
            .then(|| (self.width * z + x) as usize)
    }

    /// `BitGrid_SetBit` (`0x00465E70`): no effect outside.
    pub fn set(&mut self, x: i32, z: i32, value: bool) {
        if let Some(i) = self.index(x, z) {
            self.cells[i] = u8::from(value);
        }
    }

    /// `BitGrid_TestBit` (`0x00465F00`): free outside.
    pub fn test(&self, x: i32, z: i32) -> bool {
        self.index(x, z).is_some_and(|i| self.cells[i] != 0)
    }

    /// `GameWorld_SetOccupancyRect` (`0x00475EB0`): rectangle `[x0, z0, x1, z1]` inclusive, z outer loop.
    pub fn set_rect(&mut self, rect: [i32; 4], value: bool) {
        for z in rect[1]..=rect[3] {
            for x in rect[0]..=rect[2] {
                self.set(x, z, value);
            }
        }
    }

    /// Occupied tiles (checks, display).
    pub fn count(&self) -> usize {
        self.cells.iter().filter(|&&c| c != 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_ignore_cells_outside_and_rects_are_inclusive() {
        let mut g = OccupancyGrid::new(100, 4);
        g.set(37, 2, true);
        g.set(100, 2, true);
        g.set(-1, 0, true);
        assert!(g.test(37, 2) && !g.test(100, 2) && !g.test(-1, 0));
        g.set_rect([98, 0, 101, 1], true);
        assert_eq!(g.count(), 5);
        g.set(37, 2, false);
        assert_eq!(g.count(), 4);
    }
}
