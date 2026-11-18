//! Planning a tile path for a unit (`GameWorld_PlanHighPath` `0x004753B0`, `PathPlanner_FindTilePath` `0x004F6600`,
//! re/tile-paths 2-3): substitute start out of the obstacle, make the goal cell temporarily reachable, fallback
//! target via ring search, tile A* from the goal to the start, smoothing and waypoints in fixed point. The result is
//! the content of the message `PMSG_PTTOPT_PATH` (`0x500`), which the original sends to the unit itself.

use serde::{Deserialize, Serialize};

use super::BLOCKED;
use super::astar::find_path;
use super::grid::{COST_FREE, LayerView, PathGrid};
use super::smooth::{smooth, waypoints};
use crate::fx::fixed_hypot;
use crate::message::Message;

/// `PMSG_PTTOPT_PATH`.
pub const MSG_PATH: u32 = 0x500;

/// Largest cell count of the path buffer (8000 bytes in the original, unchecked; weakness S4).
pub const MAX_TILE_PATH: usize = 1000;

/// "Invalid" look direction (`0x00671FEC` = -200 * 4096).
const NO_DIRECTION: i32 = -200 << 12;

/// Result of `GameWorld_PlanHighPath`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighPath {
    /// Target point in fixed point (tile center of the fallback target, otherwise the requested goal).
    pub goal: (i32, i32),
    /// FINDTO flags, passed through unchanged.
    pub flags: i32,
    /// Waypoints in fixed point (without the target point).
    pub points: Vec<(i32, i32)>,
    /// Start tile of the search (after a substitute start).
    pub start_tile: (i32, i32),
    /// Tile path from `PathPlanner_FindTilePath` (start tile to the cell before the goal).
    pub tile_path: Vec<(i32, i32)>,
    /// Tiles after smoothing (basis of the waypoints).
    pub smoothed: Vec<(i32, i32)>,
    /// Expansions of the A* (0 without a search).
    pub expansions: u32,
}

impl HighPath {
    /// `PathMsg_Construct` (`0x004E5310`) with `PathMsg_AppendPoint`: `[goal x, goal z, 1, flags,
    /// −1, −200·4096, −1, points…]`.
    pub fn message(&self) -> Message {
        let mut args = vec![
            self.goal.0,
            self.goal.1,
            1,
            self.flags,
            -1,
            NO_DIRECTION,
            -1,
        ];
        for &(x, z) in &self.points {
            args.push(x);
            args.push(z);
        }
        Message {
            code: MSG_PATH,
            args,
        }
    }
}

/// Why no planning came about (the original then sends no message).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanFailure {
    /// Start tile blocked, no free orthogonal neighbor.
    StartBlocked,
    /// Override requested, but no orthogonal neighbor of the goal in the start region.
    GoalNotAdjacent,
    /// Start or goal outside the map.
    OutsideMap,
    /// No cell of the start region found (ring search).
    Unreachable,
    /// The A* did not reach the goal.
    NoPath,
    /// More than [`MAX_TILE_PATH`] cells (exception A7: a buffer overflow in the original).
    TooLong,
    /// Layer outside 0...8 (reachable only via viewer and control channel; exception A7).
    InvalidLayer,
}

/// Result of `PathPlanner_FindTilePath`.
struct TilePath {
    cells: Vec<(i32, i32)>,
    goal: (i32, i32),
    expansions: u32,
}

/// `RegionLayer_RingSearchReachable` (`0x004F87B0`) in the simple case of `FindTilePath`: the cell of region
/// `region` with the smallest table distance to the center of `goal`, first the goal itself, then Chebyshev rings up
/// to the end of the ring with the first hit. Tie: the one checked first. The ring limit comes only from the x
/// extent, and the side columns do not start until z = 1 (quirks).
pub fn ring_search(view: &LayerView, region: u16, goal: (i32, i32)) -> Option<(i32, i32)> {
    let (gx, gz) = goal;
    let (w, h) = (view.width(), view.height());
    let center = ((gx << 12) + 0x800, (gz << 12) + 0x800);
    let mut best = i32::MAX;
    let mut result = None;
    let mut eval = |x: i32, z: i32, result: &mut Option<(i32, i32)>| {
        let d = fixed_hypot(
            ((x << 12) + 0x800) - center.0,
            ((z << 12) + 0x800) - center.1,
        );
        if d < best && view.region_id(x, z) == region {
            best = d;
            *result = Some((x, z));
        }
    };
    eval(gx, gz, &mut result);
    if result.is_some() {
        return result;
    }
    let max_r = gx.max(w - gx - 1);
    let mut r = 1;
    while r <= max_r && result.is_none() {
        let (x0, x1, z0, z1) = (gx - r, gx + r, gz - r, gz + r);
        for x in x0.max(0)..=x1.min(w - 1) {
            if z0 >= 0 {
                eval(x, z0, &mut result);
            }
            if z1 < h {
                eval(x, z1, &mut result);
            }
        }
        for z in (z0 + 1).max(1)..z1.min(h) {
            if x0 >= 0 {
                eval(x0, z, &mut result);
            }
            if x1 < w {
                eval(x1, z, &mut result);
            }
        }
        r += 1;
    }
    result
}

/// `PathPlanner_FindTilePath`: if the goal lies in another region, the nearest cell of the start region takes its
/// place; the A* searches from the goal to the start.
fn find_tile_path(
    view: &LayerView,
    start: (i32, i32),
    goal: (i32, i32),
) -> Result<TilePath, PlanFailure> {
    if !view.inside(start.0, start.1) || !view.inside(goal.0, goal.1) {
        return Err(PlanFailure::OutsideMap);
    }
    let mut goal = goal;
    // `RegionLayer_AreCellsConnected`: same ID, also two blocked cells.
    let start_region = view.region_id(start.0, start.1);
    if start_region != view.region_id(goal.0, goal.1) {
        goal = ring_search(view, start_region, goal).ok_or(PlanFailure::Unreachable)?;
    }
    if goal == start {
        return Ok(TilePath {
            cells: Vec::new(),
            goal,
            expansions: 0,
        });
    }
    let path = find_path(*view, goal, start).ok_or(PlanFailure::NoPath)?;
    if path.cells.len() > MAX_TILE_PATH {
        log::warn!(
            "Tile path with {} cells from {start:?} to {goal:?} rejected (original: overflow above {MAX_TILE_PATH})",
            path.cells.len()
        );
        return Err(PlanFailure::TooLong);
    }
    Ok(TilePath {
        cells: path.cells,
        goal,
        expansions: path.expansions,
    })
}

/// Cell count of the tile path from `PathPlanner_FindTilePath` in layer `layer` from cell `start` to cell `goal`
/// (AI query `0x4FB0`, re/ki-queries 5.18.3) - without the substitute start and the smoothing of
/// [`plan_high_path`]. `None`: layer outside 0...8, start or goal outside the map, no cell of the start region, no
/// path or more than [`MAX_TILE_PATH`] cells (original: buffer overflow; exception A7). Start = (fallback) goal
/// yields 0.
pub(crate) fn tile_path_cells(
    grid: &PathGrid,
    layer: usize,
    start: (i32, i32),
    goal: (i32, i32),
) -> Option<usize> {
    if layer >= super::LAYER_COUNT {
        return None;
    }
    find_tile_path(&grid.view(layer), start, goal)
        .ok()
        .map(|p| p.cells.len())
}

/// `GameWorld_PlanHighPath` for a unit at `unit` (fixed point) in layer `layer` toward `goal` (fixed point).
/// `allow_override` = FINDTO message != `0x200` (also FINDTOTREE): the goal cell temporarily counts toward the start
/// region provided an orthogonal neighbor belongs to it (otherwise failure). Only reads the grid (the original
/// overwrites the cell and resets it).
pub fn plan_high_path(
    grid: &PathGrid,
    layer: usize,
    unit: (i32, i32),
    goal: (i32, i32),
    allow_override: bool,
    flags: i32,
) -> Result<HighPath, PlanFailure> {
    if layer >= super::LAYER_COUNT {
        return Err(PlanFailure::InvalidLayer);
    }
    let unit_tile = (unit.0 >> 12, unit.1 >> 12);
    let mut path = HighPath {
        goal,
        flags,
        points: Vec::new(),
        start_tile: unit_tile,
        tile_path: Vec::new(),
        smoothed: Vec::new(),
        expansions: 0,
    };
    if goal == unit {
        return Ok(path);
    }
    let base = grid.view(layer);
    let (ux, uz) = unit_tile;
    let goal_tile = (goal.0 >> 12, goal.1 >> 12);
    let (gx, gz) = goal_tile;
    let goal_region = base.region_id(gx, gz);
    let orthogonal = |(x, z): (i32, i32)| [(x, z - 1), (x, z + 1), (x - 1, z), (x + 1, z)];
    let mut start = unit_tile;
    if base.cost(ux, uz) != COST_FREE {
        // The unit stands inside the obstacle: substitute start.
        let adjacent = (gx == ux && (gz - uz).abs() == 1) || (gz == uz && (gx - ux).abs() == 1);
        let in_goal_region = orthogonal(unit_tile)
            .into_iter()
            .find(|&(x, z)| base.region_id(x, z) == goal_region);
        let free = orthogonal(unit_tile)
            .into_iter()
            .find(|&(x, z)| base.cost(x, z) == COST_FREE);
        start = match (goal_region != BLOCKED, adjacent, in_goal_region) {
            (true, true, _) => goal_tile,
            (true, false, Some(cell)) => cell,
            _ => free.ok_or(PlanFailure::StartBlocked)?,
        };
    }
    let mut view = base;
    if allow_override {
        let start_region = base.region_id(start.0, start.1);
        if !orthogonal(goal_tile)
            .into_iter()
            .any(|(x, z)| base.region_id(x, z) == start_region)
        {
            return Err(PlanFailure::GoalNotAdjacent);
        }
        view.patch = Some((goal_tile, start_region));
    }
    let found = find_tile_path(&view, start, goal_tile)?;
    if found.goal != goal_tile {
        path.goal = ((found.goal.0 << 12) + 0x800, (found.goal.1 << 12) + 0x800);
    }
    let fx = |v: i32| v as f32 * (1.0 / 4096.0);
    let from = if start == unit_tile {
        (fx(unit.0), fx(unit.1))
    } else {
        (start.0 as f32 + 0.5, start.1 as f32 + 0.5)
    };
    let to = (fx(path.goal.0), fx(path.goal.1));
    let region_at = |x: i32, z: i32| view.region_id(x, z);
    path.smoothed = smooth(&region_at, &found.cells, from, to);
    path.points = waypoints(&path.smoothed, unit_tile);
    path.start_tile = start;
    path.tile_path = found.cells;
    path.expansions = found.expansions;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::region::RegionLayer;

    /// 16 x 16 grid with all layers equal: `region(x, z)` per cell.
    fn grid(region: impl Fn(i32, i32) -> u16) -> PathGrid {
        let mut g = PathGrid::new(16, 16);
        for l in &mut g.layers {
            for z in 0..16 {
                for x in 0..16 {
                    l.cells[(z * 16 + x) as usize] = region(x, z);
                }
            }
        }
        g.built = true;
        g
    }

    fn fx(tile: (i32, i32)) -> (i32, i32) {
        ((tile.0 << 12) + 0x800, (tile.1 << 12) + 0x800)
    }

    #[test]
    fn ring_search_skips_row_zero_on_the_side_columns() {
        // R1 to R3: region A (ID 1) x <= 5; region B (ID 2) = (12...15, 0) and (12...15, 10...15).
        let g = grid(|x, z| {
            if x <= 5 {
                1
            } else if x >= 12 && (z == 0 || z >= 10) {
                2
            } else {
                BLOCKED
            }
        });
        let v = g.view(0);
        assert_eq!(ring_search(&v, 2, (5, 0)), Some((12, 10)));
        assert_eq!(ring_search(&v, 2, (5, 5)), Some((12, 10)));
        assert_eq!(ring_search(&v, 2, (5, 15)), Some((12, 15)));
        assert_eq!(ring_search(&v, 1, (3, 3)), Some((3, 3)));
        assert_eq!(ring_search(&v, 7, (3, 3)), None);
    }

    #[test]
    fn unreachable_goals_are_replaced_by_the_nearest_cell_centre() {
        // Wall x = 8 across the whole height: goal on the right, unit on the left.
        let g = grid(|x, _| match x {
            8 => BLOCKED,
            0..8 => 1,
            _ => 2,
        });
        let p = plan_high_path(&g, 0, fx((2, 5)), fx((12, 5)), false, 0x201).unwrap();
        assert_eq!(p.goal, fx((7, 5)));
        assert_eq!(p.tile_path, vec![(2, 5), (3, 5), (4, 5), (5, 5), (6, 5)]);
        assert_eq!(p.points, vec![]);
        let m = p.message();
        assert_eq!(m.code, 0x500);
        assert_eq!(
            m.args,
            [fx((7, 5)).0, fx((7, 5)).1, 1, 0x201, -1, -819_200, -1]
        );
        // With override: no neighbor of the goal in the start region -> failure.
        assert_eq!(
            plan_high_path(&g, 0, fx((2, 5)), fx((12, 5)), true, 0),
            Err(PlanFailure::GoalNotAdjacent)
        );
        // Goal inside the wall, neighbor (7, 5) in the start region: the cell counts as reachable.
        let p = plan_high_path(&g, 0, fx((2, 5)), fx((8, 5)), true, 0).unwrap();
        assert_eq!(p.goal, fx((8, 5)));
        assert_eq!(p.tile_path.len(), 6);
        // Without override there: fallback target (7, 5).
        let p = plan_high_path(&g, 0, fx((2, 5)), fx((8, 5)), false, 0).unwrap();
        assert_eq!(p.goal, fx((7, 5)));
    }

    #[test]
    fn units_inside_obstacles_start_from_a_neighbour() {
        let mut g = grid(|_, _| 1);
        for l in &mut g.layers {
            l.cells[5 * 16 + 5] = BLOCKED;
        }
        // Goal is an orthogonal neighbor: "start" there -> empty path.
        let p = plan_high_path(&g, 0, fx((5, 5)), fx((5, 6)), false, 0).unwrap();
        assert_eq!((p.start_tile, p.tile_path.len()), ((5, 6), 0));
        // Otherwise the first neighbor of the goal region in the order z-1, z+1, x-1, x+1.
        let p = plan_high_path(&g, 0, fx((5, 5)), fx((10, 10)), false, 0).unwrap();
        assert_eq!(p.start_tile, (5, 4));
        // Fully enclosed: failure.
        let mut g = grid(|_, _| 1);
        for (x, z) in [(5, 5), (5, 4), (5, 6), (4, 5), (6, 5)] {
            g.layers[0].cells[z * 16 + x] = BLOCKED;
        }
        assert_eq!(
            plan_high_path(&g, 0, fx((5, 5)), fx((10, 10)), false, 0),
            Err(PlanFailure::StartBlocked)
        );
        assert_eq!(
            plan_high_path(&g, 9, fx((1, 1)), fx((3, 1)), false, 0),
            Err(PlanFailure::InvalidLayer)
        );
        // Goal = position: path without points, also outside.
        let p = plan_high_path(&g, 0, (-5, -5), (-5, -5), true, 3).unwrap();
        assert!(p.points.is_empty() && p.tile_path.is_empty());
        assert_eq!(
            plan_high_path(&g, 0, fx((1, 1)), fx((16, 1)), false, 0),
            Err(PlanFailure::OutsideMap)
        );
    }

    #[test]
    fn paths_longer_than_the_original_buffer_are_rejected() {
        // Snake line on 64 x 64: walls in every second column, alternately open at the top/bottom.
        let mut l = RegionLayer::new(64, 64);
        l.cells.fill(1);
        for x in (1..64).step_by(2) {
            let open = if x % 4 == 1 { 63 } else { 0 };
            for z in 0..64 {
                if z != open {
                    l.cells[(z * 64 + x) as usize] = BLOCKED;
                }
            }
        }
        let mut g = PathGrid::new(64, 64);
        g.layers[0] = l;
        g.built = true;
        assert_eq!(
            plan_high_path(&g, 0, fx((0, 0)), fx((62, 0)), false, 0),
            Err(PlanFailure::TooLong)
        );
        assert!(plan_high_path(&g, 0, fx((0, 0)), fx((4, 0)), false, 0).is_ok());
    }
}
