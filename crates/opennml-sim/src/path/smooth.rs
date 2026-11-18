//! Smoothing of the tile path via lines of sight (`TilePath_SmoothWithLineOfSight` `0x004F9060`,
//! `RegionLayer_IsLineInRegion` `0x004F8F60`, re/tile-paths 4) and the conversion into fixed-point waypoints from
//! `GameWorld_PlanHighPath`. One of the three floating-point places of the simulation: each operation individually in
//! `f32` like the x87 of the original with 24-bit precision, without `mul_add` and without reordering.

use crate::fx::ftol;

/// `0.1` (`0x3DCCCCCD`, `0x00618FB4`).
const OFFSET: f32 = f32::from_bits(0x3DCC_CCCD);

/// `RegionLayer_IsLineInRegion`: samples the line in steps of 1/32 tile (start included, end point not) and
/// requires the region `region` at every point.
pub fn line_in_region(
    region_at: &impl Fn(i32, i32) -> u16,
    x0: f32,
    z0: f32,
    x1: f32,
    z1: f32,
    region: u16,
) -> bool {
    let dx = x1 - x0;
    let dz = z1 - z0;
    let len = (dz * dz + dx * dx).sqrt();
    if len == 0.0 {
        return true;
    }
    let n = len * 32.0;
    let inv = 1.0 / n;
    let step_x = dx * inv;
    let step_z = dz * inv;
    if n.is_nan() || n <= 0.0 {
        return true;
    }
    let (mut x, mut z, mut left) = (x0, z0, n);
    loop {
        if region_at(ftol(x), ftol(z)) != region {
            return false;
        }
        x += step_x;
        z += step_z;
        left -= 1.0;
        if left.is_nan() || left <= 0.0 {
            return true;
        }
    }
}

/// Snaps a coordinate to the tile center if it lies less than 0.1 before a tile boundary (truncation toward 0: for
/// `v ∈ [0; 0.1)` no snapping, quirk).
fn snap(v: f32) -> f32 {
    let iv = ftol(v);
    if ftol(v - OFFSET) != iv || ftol(v + OFFSET) != iv {
        iv as f32 + 0.5
    } else {
        v
    }
}

/// `TilePath_SmoothWithLineOfSight`: of `cells` (start cell of the unit to the cell before the goal) keeps only the
/// cells that would no longer be visible from the respective last one. Candidates in the order goal, `cells[n−1]`,
/// ..., `cells[0]`; the first visible one wins, `cells[0]` if need be even invisible. Visible = both lines offset by
/// (-0.1, -0.1) and (+0.1, +0.1) stay in the region of the start. A path of one cell yields no point without a
/// check.
pub fn smooth(
    region_at: &impl Fn(i32, i32) -> u16,
    cells: &[(i32, i32)],
    start: (f32, f32),
    goal: (f32, f32),
) -> Vec<(i32, i32)> {
    let (sx, sz, gx, gz) = (snap(start.0), snap(start.1), snap(goal.0), snap(goal.1));
    let count = cells.len();
    let center = |(x, z): (i32, i32)| (x as f32 + 0.5, z as f32 + 0.5);
    let region = region_at(ftol(sx), ftol(sz));
    let (mut cx, mut cz) = (sx, sz);
    let mut out = Vec::new();
    let mut i = 1;
    while count > 0 && i < count {
        let mut j = count;
        let (mut tx, mut tz) = (gx, gz);
        let (cxm, czm) = (cx - OFFSET, cz - OFFSET);
        loop {
            let visible = line_in_region(region_at, cxm, czm, tx - OFFSET, tz - OFFSET, region)
                && line_in_region(
                    region_at,
                    cx + OFFSET,
                    cz + OFFSET,
                    tx + OFFSET,
                    tz + OFFSET,
                    region,
                );
            if visible || j == 0 {
                break;
            }
            j -= 1;
            (tx, tz) = center(cells[j]);
        }
        if j < count {
            (cx, cz) = center(cells[j]);
            out.push((ftol(tx), ftol(tz)));
        }
        i = j;
        if out.len() >= count {
            break;
        }
    }
    out
}

/// Waypoints in fixed point (`GameWorld_PlanHighPath`, `0x0047582E`...`0x004758B5`): tile * 4096 + `0x400`; if the
/// point is not behind the previous one in x, **z** moves by `0x800`, if it (with the new z) is not behind the
/// previous one in z, **x** moves (axes swapped, quirk). The first comparison runs against the tile index of the
/// unit instead of its fixed-point position, so the first point always lies at `+0xC00/+0xC00`.
pub fn waypoints(cells: &[(i32, i32)], unit_tile: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut px, mut pz) = unit_tile;
    cells
        .iter()
        .map(|&(cx, cz)| {
            let mut x = cx.wrapping_shl(12).wrapping_add(0x400);
            let mut z = cz.wrapping_shl(12).wrapping_add(0x400);
            if x.wrapping_sub(px) >= 0 {
                z = z.wrapping_add(0x800);
            }
            if z.wrapping_sub(pz) >= 0 {
                x = x.wrapping_add(0x800);
            }
            (px, pz) = (x, z);
            (x, z)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::astar::find_path;
    use crate::path::astar::tests::layer;
    use crate::path::grid::LayerView;

    /// `PlanHighPath` without substitute start, fallback target and override: tile path, smoothed cells and
    /// waypoints (test vectors P1 to P9 from the model of the original code).
    fn plan(walls: &[(i32, i32)], unit: (i32, i32), goal: (i32, i32)) -> [Vec<(i32, i32)>; 3] {
        let l = layer(walls);
        let view = LayerView {
            layer: &l,
            patch: None,
        };
        let at = |x: i32, z: i32| view.region_id(x, z);
        let tile = (unit.0 >> 12, unit.1 >> 12);
        let cells = find_path(view, (goal.0 >> 12, goal.1 >> 12), tile)
            .expect("path")
            .cells;
        let fx = |v: i32| v as f32 * (1.0 / 4096.0);
        let smoothed = smooth(
            &at,
            &cells,
            (fx(unit.0), fx(unit.1)),
            (fx(goal.0), fx(goal.1)),
        );
        let points = waypoints(&smoothed, tile);
        [cells, smoothed, points]
    }

    #[test]
    fn straight_and_neighbour_paths_need_no_waypoints() {
        assert_eq!(plan(&[], (0x800, 0x800), (0x5800, 0x3800))[2], vec![]);
        // P6: one cell -> no check, no point.
        let [cells, smoothed, _] = plan(&[], (0x800, 0x800), (0x1800, 0x800));
        assert_eq!((cells, smoothed), (vec![(0, 0)], vec![]));
    }

    #[test]
    fn corners_become_waypoints_with_the_swapped_axis_rule() {
        let wall: Vec<_> = (0..5).map(|z| (1, z)).collect();
        let [_, smoothed, points] = plan(&wall, (0x800, 0x800), (0x2800, 0x800));
        assert_eq!(smoothed, vec![(0, 5), (2, 5)]);
        assert_eq!(points, vec![(0xC00, 0x5C00), (0x2C00, 0x5C00)]);
        let block: Vec<_> = (2..5).flat_map(|x| (2..5).map(move |z| (x, z))).collect();
        assert_eq!(
            plan(&block, (0x800, 0x800), (0x6800, 0x6800))[2],
            vec![(0x1C00, 0x5C00)]
        );
        // P7: movement in -x, the first point still lies at 3/4 / 3/4.
        assert_eq!(
            plan(&block, (0x6800, 0x6800), (0x800, 0x800))[1..],
            [vec![(1, 5)], vec![(0x1C00, 0x5C00)]]
        );
        assert_eq!(
            plan(&block, (0x800, 0x6800), (0x6800, 0x800))[2],
            vec![(0x1C00, 0x1C00)]
        );
        // P9: second point - x decreases (z stays at 1/4), z increases (x at 3/4).
        let wall: Vec<_> = (3..13).map(|x| (x, 5)).collect();
        let [cells, smoothed, points] = plan(&wall, (0x8800, 0x1800), (0x8800, 0x9800));
        assert_eq!(cells.len(), 12);
        assert_eq!(smoothed, vec![(13, 4), (13, 6)]);
        assert_eq!(points, vec![(0xDC00, 0x4C00), (0xDC00, 0x6400)]);
    }

    #[test]
    fn lines_sample_every_32nd_tile_without_the_end_point() {
        // End point 0.01 in cell 3: the cell is not checked.
        let l = layer(&[(3, 0)]);
        let at = |x: i32, z: i32| l.region_id(x, z);
        assert!(line_in_region(&at, 0.4, 0.4, 3.01, 0.4, 1));
        assert!(!line_in_region(&at, 0.4, 0.4, 3.2, 0.4, 1));
        assert!(line_in_region(&at, 2.0, 2.0, 2.0, 2.0, 7));
        // Snapping only near a boundary, not at [0; 0.1).
        assert_eq!(snap(2.95), 2.5);
        assert_eq!(snap(2.5), 2.5);
        assert_eq!(snap(0.05), 0.05);
    }
}
