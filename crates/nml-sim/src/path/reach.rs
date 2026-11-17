//! Nearest reachable cell with a filter (`PathPlanner_FindNearestReachableCell` `0x004F6A40`,
//! `RegionLayer_RingSearchReachable` `0x004F87B0`, `RegionLayer_EvaluateRingCandidate` `0x004F8C60`; re/path-grid 4,
//! re/combat-targeting 3.2): the cell search of the reach filter and of the walk combat part. Combat never uses the
//! original's second layer, but boarding vehicles does ([`find_boarding_cells`], re/ships 3.2).

use crate::fx::fixed_hypot;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send_mut;
use crate::path::BLOCKED;
use crate::path::region::ring_span;
use crate::sim::Sim;

/// Filter words 0xC/0xD, 0xF/0x10, 0x11 and byte 0x13 of the reach filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReachFilter {
    pub range_min: i32,
    pub range_max: i32,
    /// Home (fixed point) or (-1, -1).
    pub home: (i32, i32),
    /// Tether; < 0 = none.
    pub tether: i32,
    /// Line of sight via the building map (`0x00491E90`).
    pub line_of_sight: bool,
}

fn cell_center(c: (i32, i32)) -> (i32, i32) {
    ((c.0 << 12) + 0x800, (c.1 << 12) + 0x800)
}

/// Nearest cell of the region of the start cell `start` around the goal cell `goal`; `max_r` rings (< 0 = up to the
/// edge; a building target extends it by `0x3E03`). The center of the distances is the target (building: the edge
/// point `0x3803` to the candidate cell), without a target the center of the goal cell. A layer outside 0...8
/// yields `None` (re/combat-targeting 3.2).
pub fn find_nearest_reachable_cell(
    sim: &mut Sim,
    layer: i32,
    start: (i32, i32),
    goal: (i32, i32),
    max_r: i32,
    filter: Option<&ReachFilter>,
    target: Option<ObjectId>,
) -> Option<(i32, i32)> {
    let layer = usize::try_from(layer)
        .ok()
        .filter(|&l| l < crate::path::LAYER_COUNT)?;
    let (w, h, region) = {
        let view = sim.state.path.view(layer);
        (
            view.width(),
            view.height(),
            view.region_id(start.0, start.1),
        )
    };
    let building = target.filter(|&t| sim.object(t).is_some_and(|o| o.has_module(3, false)));
    let mut lim = max_r;
    if let Some(b) = building {
        let v = send_mut(sim, b, &mut Message::new(0x3E03, &[0]));
        if v >= 0 {
            lim = v.wrapping_add(lim);
        }
    }
    let rmax = ring_limit(w, goal.0, lim);
    let base = target
        .and_then(|t| sim.object(t))
        .map_or(cell_center(goal), |o| (o.grid.x.0, o.grid.z.0));
    let mut best = i32::MAX;
    let mut eval = |sim: &mut Sim, x: i32, z: i32| -> bool {
        let mut cp = cell_center((x, z));
        let mut c = base;
        if let Some(b) = building {
            let mut m = Message::new(0x3803, &[cp.0, cp.1]);
            if send_mut(sim, b, &mut m) != 0 {
                c = (m.arg(0), m.arg(1));
            }
        }
        if let Some(f) = filter
            && (x, z) == (f.home.0 >> 12, f.home.1 >> 12)
        {
            cp = f.home;
        }
        let d = fixed_hypot(cp.0.wrapping_sub(c.0), cp.1.wrapping_sub(c.1));
        if d >= best || sim.state.path.region_id(layer, x, z) != region {
            return false;
        }
        if let Some(f) = filter {
            let home = fixed_hypot(cp.0.wrapping_sub(f.home.0), cp.1.wrapping_sub(f.home.1));
            if f.tether >= 0 && home > f.tether {
                return false;
            }
            if d > f.range_max || d < f.range_min {
                return false;
            }
        }
        best = d;
        // `0x00491E90` (C-45): cell at cell height + 0.75 tile to the target at map height + 0.75 tile, without the
        // water maximum and without the building exception; `best` is already lowered (C-40).
        if let (Some(f), Some(t)) = (filter, target)
            && f.line_of_sight
        {
            let Some((tx, tz)) = sim.object(t).map(|o| (o.grid.x.0, o.grid.z.0)) else {
                return false;
            };
            let from = [
                cp.0,
                cp.1,
                crate::tile_map::height(sim, x, z).wrapping_add(0xC00),
            ];
            let to = [
                tx,
                tz,
                crate::tile_map::height(sim, tx >> 12, tz >> 12).wrapping_add(0xC00),
            ];
            return crate::tile_map::has_line_of_sight_points(sim, from, to);
        }
        true
    };
    ring_scan(sim, (w, h), goal, rmax, region == BLOCKED, &mut eval)
}

/// Ring run of the ring search (`RegionLayer_RingSearchReachable` `0x004F87B0`): first the goal cell (hit -> result
/// immediately), then Chebyshev rings `1..=rmax` around it, until a ring has a hit. Order per ring: top row `z − r`,
/// bottom row `z + r`, each with x ascending (top before bottom), then the columns `x − r` and `x + r`, each with z
/// from `max(z − r + 1, 1)` up to `< min(z + r, H)` (left before right; quirk: if the ring lies outside at the top,
/// the columns do not begin until `z = 1`, re/path-grid 4).
/// The ring is checked to the end; the result is the **last** hit of the ring. What counts as a hit (best mark,
/// region, filter) is decided by `eval`.
///
/// The computation uses `i64` without overflow; for any input the result is that of the original. Rings beyond the
/// distance of the farthest map corner ([`ring_span`]) check no cell and are skipped. Rings in front of the map check
/// only cells outside (region `0xFFFF`): if such a cell cannot hit (`outside_hits = false`, searched region not
/// `0xFFFF`), the run does not begin until the distance of the map, otherwise at 1 as in the original. So goals far
/// outside never run endlessly; with `outside_hits`, `rmax.min(far)` bounds the run (a blocked start then usually
/// already hits the first cell outside).
fn ring_scan(
    sim: &mut Sim,
    (w, h): (i32, i32),
    goal: (i32, i32),
    rmax: i64,
    outside_hits: bool,
    eval: &mut impl FnMut(&mut Sim, i32, i32) -> bool,
) -> Option<(i32, i32)> {
    if eval(sim, goal.0, goal.1) {
        return Some(goal);
    }
    let (near, far) = ring_span(w, h, goal);
    let (w, h) = (i64::from(w), i64::from(h));
    let (gx, gz) = (i64::from(goal.0), i64::from(goal.1));
    // Checked cells lie in x or z between 0 and the goal or inside the map, so they fit into
    // `i32`.
    let mut hit = |sim: &mut Sim, x: i64, z: i64| eval(sim, x as i32, z as i32);
    let mut found = None;
    let mut r = if outside_hits { 1 } else { near.max(1) };
    while r <= rmax.min(far) && found.is_none() {
        let (x0, x1, z0, z1) = (gx - r, gx + r, gz - r, gz + r);
        for x in x0.max(0)..=x1.min(w - 1) {
            if z0 >= 0 && hit(sim, x, z0) {
                found = Some((x, z0));
            }
            if z1 < h && hit(sim, x, z1) {
                found = Some((x, z1));
            }
        }
        for z in (z0 + 1).max(1)..z1.min(h) {
            if x0 >= 0 && hit(sim, x0, z) {
                found = Some((x0, z));
            }
            if x1 < w && hit(sim, x1, z) {
                found = Some((x1, z));
            }
        }
        r += 1;
    }
    found.map(|(x, z)| (x as i32, z as i32))
}

/// Ring limit of the ring search without a building target: `u = max(x, W − x − 1)` (the x extent only),
/// bounded by `lim` (< 0 = up to the edge); in `i64` without overflow.
fn ring_limit(width: i32, goal_x: i32, lim: i32) -> i64 {
    let (w, x, lim) = (i64::from(width), i64::from(goal_x), i64::from(lim));
    let u = x.max(w - x - 1);
    if lim < 0 || u < lim { u } else { lim }
}

/// Inner search of [`find_boarding_cells`] (ring search in the second layer, simple case of
/// `RegionLayer_EvaluateRingCandidate` `0x004F8C60` without filter and target): the cell of region `region` in layer
/// `layer` with the smallest `Fixed_Hypot` distance of the cell centers to `centre`; the best mark is lowered, on a
/// tie the one checked first stays.
fn nearest_cell_in_region(
    sim: &mut Sim,
    layer: usize,
    centre: (i32, i32),
    region: u16,
    max_r: i32,
) -> Option<(i32, i32)> {
    let (w, h) = {
        let view = sim.state.path.view(layer);
        (view.width(), view.height())
    };
    let c = cell_center(centre);
    let mut best = i32::MAX;
    let mut eval = |sim: &mut Sim, x: i32, z: i32| -> bool {
        let p = cell_center((x, z));
        let d = fixed_hypot(p.0.wrapping_sub(c.0), p.1.wrapping_sub(c.1));
        if d >= best || sim.state.path.region_id(layer, x, z) != region {
            return false;
        }
        best = d;
        true
    };
    ring_scan(
        sim,
        (w, h),
        centre,
        ring_limit(w, centre.0, max_r),
        region == BLOCKED,
        &mut eval,
    )
}

/// Meeting point of a mobile container (`PathPlanner_FindNearestReachableCell` with a second layer,
/// `0x004F95F0` -> `0x004F87B0` -> `0x004F8C60`; re/ships 3.2): land cell `a` in the region of the unit cell
/// `unit_cell` (layer `land_layer`) and water cell `b` in the region of the ship cell `ship_cell` (layer
/// `water_layer`). Outer run around the ship cell in the land layer with a limit from the x extent only; for each
/// candidate in the land region the inner run ([`nearest_cell_in_region`]) searches the nearest water cell within
/// the radius `inner_max_r`; if it hits, the candidate is valid. **The outer best mark is never lowered (W-196):**
/// the ring is checked to the end, `a` is the last valid candidate of the first ring with hits (not the shore point
/// nearest the ship) and `b` the inner result of exactly this candidate (the original leaves `b` undetermined on
/// later failures, presumably as here). A layer outside 0...8 yields `None`.
pub fn find_boarding_cells(
    sim: &mut Sim,
    land_layer: i32,
    unit_cell: (i32, i32),
    ship_cell: (i32, i32),
    water_layer: i32,
    inner_max_r: i32,
) -> Option<((i32, i32), (i32, i32))> {
    let layer_of = |l: i32| {
        usize::try_from(l)
            .ok()
            .filter(|&l| l < crate::path::LAYER_COUNT)
    };
    let (land, water) = (layer_of(land_layer)?, layer_of(water_layer)?);
    let region = sim.state.path.region_id(land, unit_cell.0, unit_cell.1);
    let region2 = sim.state.path.region_id(water, ship_cell.0, ship_cell.1);
    let (w, h) = {
        let view = sim.state.path.view(land);
        (view.width(), view.height())
    };
    let mut last = None;
    let mut eval = |sim: &mut Sim, x: i32, z: i32| -> bool {
        if sim.state.path.region_id(land, x, z) != region {
            return false;
        }
        match nearest_cell_in_region(sim, water, (x, z), region2, inner_max_r) {
            Some(b) => {
                last = Some(((x, z), b));
                true
            }
            None => false,
        }
    };
    // Limit -1: with a second layer the original passes `maxR` on to the inner stage.
    ring_scan(
        sim,
        (w, h),
        ship_cell,
        ring_limit(w, ship_cell.0, -1),
        region == BLOCKED,
        &mut eval,
    )?;
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::highpath::ring_search;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    const TYPES: &str =
        "GUID 1200\nNAME Krieger\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10004 6 1 512\n";

    fn world(walls: &[(i32, i32)]) -> (Rules, SimState) {
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        for &(x, z) in walls {
            state.path.mark_cell(1, x, z, true);
        }
        state.path.build_regions(u32::MAX);
        (rules, state)
    }

    #[test]
    fn without_filter_and_target_it_finds_what_ring_search_finds() {
        // Wall x = 8: goals east of it lead to the nearest cell of the west region.
        let walls: Vec<_> = (0..32).map(|z| (8, z)).collect();
        let (rules, mut state) = world(&walls);
        let mut sim = Sim::new(&mut state, &rules);
        for goal in [(5, 5), (8, 3), (12, 0), (20, 31), (31, 15)] {
            let want = {
                let view = sim.state.path.view(0);
                let region = view.region_id(2, 2);
                ring_search(&view, region, goal)
            };
            let got = find_nearest_reachable_cell(&mut sim, 0, (2, 2), goal, -1, None, None);
            assert_eq!(got, want, "goal {goal:?}");
        }
        assert_eq!(
            find_nearest_reachable_cell(&mut sim, 9, (2, 2), (3, 3), -1, None, None),
            None
        );
        assert_eq!(
            find_nearest_reachable_cell(&mut sim, -1, (2, 2), (3, 3), -1, None, None),
            None
        );
    }

    #[test]
    fn the_filter_limits_the_band_the_tether_and_the_rings() {
        let (rules, mut state) = world(&[]);
        let mut sim = Sim::new(&mut state, &rules);
        let pos = [10.5 * 128.0, 0.0, 10.5 * 128.0];
        let target = create_object(&mut sim, &mut ObjectDesc::new(1200, 1, pos)).unwrap();
        let f = |min, max| ReachFilter {
            range_min: min,
            range_max: max,
            home: (-1, -1),
            tether: -1,
            line_of_sight: false,
        };
        let find = |sim: &mut Sim, r: i32, f: &ReachFilter| {
            find_nearest_reachable_cell(sim, 0, (2, 2), (10, 10), r, Some(f), Some(target))
        };
        // Target center on the cell center (10, 10): distance 0 -> the goal cell itself.
        assert_eq!(find(&mut sim, 3, &f(0, 2560)), Some((10, 10)));
        // Band [4096, 8192]: the goal cell (0) drops out; in ring 1 the smallest distance 4096 wins, on a
        // tie the one checked first: top (10, 9) before bottom (10, 11).
        assert_eq!(find(&mut sim, 3, &f(4096, 8192)), Some((10, 9)));
        // Ring limit 0 -> only the goal cell.
        assert_eq!(find(&mut sim, 0, &f(4096, 8192)), None);
        // Tether 2 tiles around home (2.5, 2.5): no cell close enough to the goal.
        let home = ((2 << 12) + 0x800, (2 << 12) + 0x800);
        let tethered = ReachFilter {
            home,
            tether: 8192,
            ..f(0, 2560)
        };
        assert_eq!(find(&mut sim, 3, &tethered), None);
    }

    #[test]
    fn line_of_sight_rejects_cells_behind_a_house_and_poisons_the_best_distance() {
        use crate::command::testkit::{HOUSE, rules as kit_rules, spawn, state as kit_state};
        let rules = kit_rules();
        let mut state = kit_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let target = spawn(&mut sim, crate::command::testkit::COLONIST, 1, (20, 10));
        let f = |los| ReachFilter {
            range_min: 4 << 12,
            range_max: 6 << 12,
            home: (-1, -1),
            tether: -1,
            line_of_sight: los,
        };
        let find = |sim: &mut Sim, los| {
            find_nearest_reachable_cell(sim, 0, (2, 2), (20, 10), 8, Some(&f(los)), Some(target))
        };
        // Without a house: ring 3 holds the corners (+-3, +-3) with distance 4.24 in the band; the first one
        // checked, (17, 7), wins (equally distant ones checked later do not, `d < best`).
        assert_eq!(find(&mut sim, true), Some((17, 7)));
        // House at (19, 9) on the diagonal (17, 7) -> goal: (17, 7) fails on the line of sight, but already lowers
        // the best distance to 4.24 (C-40). So (17, 13) with the same distance and a clear view loses too; a cell
        // with a clear view is not found until ring 4.
        spawn(&mut sim, HOUSE, 2, (19, 9));
        assert_eq!(find(&mut sim, false), Some((17, 7)));
        let got = find(&mut sim, true).expect("ring 4");
        assert!(got != (17, 7) && got != (17, 13), "{got:?}");
        assert!(
            got.0.abs_diff(20).max(got.1.abs_diff(10)) == 4,
            "ring 4: {got:?}"
        );
        let to = [(20 << 12) + 2048, (10 << 12) + 2048, 0xC00];
        let from = [(got.0 << 12) + 2048, (got.1 << 12) + 2048, 0xC00];
        assert!(crate::tile_map::has_line_of_sight_points(&sim, from, to));
    }

    #[test]
    fn boarding_cells_take_the_last_candidate_of_the_first_ring_with_hits() {
        // re/ships 9.2: unit (5, 10) layer 0, ship (14, 10) layer 6, inner_max_r 2
        // -> Some(((9, 14), (11, 14))). Rings 1-4 without a land cell; in ring 5 the last valid candidate in ring
        // order wins (W-196), not the nearest shore point (9, 10).
        use crate::module::boarding::testkit::{rules, shore_state};
        let rules = rules();
        let mut state = shore_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let find = |sim: &mut Sim, land, water, r| {
            find_boarding_cells(sim, land, (5, 10), (14, 10), water, r)
        };
        assert_eq!(find(&mut sim, 0, 6, 2), Some(((9, 14), (11, 14))));
        // Inner radius 1: from the shore to the ship water (x >= 11) it is two cells -> no hit.
        assert_eq!(find(&mut sim, 0, 6, 1), None);
        // Layer outside 0...8 (also -1 for "without walk") yields nothing.
        assert_eq!(find(&mut sim, 9, 6, 2), None);
        assert_eq!(find(&mut sim, 0, -1, 2), None);
    }

    #[test]
    fn the_inner_search_picks_the_nearest_water_cell_and_ties_keep_the_first() {
        // Um (9, 14): (11, 12) 11585, (11, 13) 9159, (11, 14) 8192 → (11, 14).
        use crate::module::boarding::testkit::{rules, shore_state};
        let rules = rules();
        let mut state = shore_state(&rules);
        // Water layer 4 is free from x >= 10; if (10, 14) is blocked, (10, 13) and (10, 15) are equally far with
        // 5792: the one checked first (top row before bottom) stays.
        let mut gap = shore_state(&rules);
        gap.path.mark_cell(1 << 4, 10, 14, true);
        gap.path.flush_dirty_rect();
        gap.path.build_regions(u32::MAX);
        let region6 = state.path.region_id(6, 14, 10);
        let region4 = gap.path.region_id(4, 14, 10);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(
            nearest_cell_in_region(&mut sim, 6, (9, 14), region6, 2),
            Some((11, 14))
        );
        // Outside the radius no cell of the region.
        assert_eq!(
            nearest_cell_in_region(&mut sim, 6, (9, 14), region6, 1),
            None
        );
        let mut sim = Sim::new(&mut gap, &rules);
        assert_eq!(
            nearest_cell_in_region(&mut sim, 4, (9, 14), region4, 1),
            Some((10, 13))
        );
    }

    /// The ring scan before bounding ([`ring_scan`]): reference for the bounded ring run.
    fn old_ring_scan(
        sim: &mut Sim,
        (w, h): (i32, i32),
        goal: (i32, i32),
        rmax: i32,
        eval: &mut impl FnMut(&mut Sim, i32, i32) -> bool,
    ) -> Option<(i32, i32)> {
        if eval(sim, goal.0, goal.1) {
            return Some(goal);
        }
        let mut found = None;
        let mut r = 1;
        while r <= rmax && found.is_none() {
            let (x0, x1, z0, z1) = (goal.0 - r, goal.0 + r, goal.1 - r, goal.1 + r);
            for x in x0.max(0)..=x1.min(w - 1) {
                if z0 >= 0 && eval(sim, x, z0) {
                    found = Some((x, z0));
                }
                if z1 < h && eval(sim, x, z1) {
                    found = Some((x, z1));
                }
            }
            for z in (z0 + 1).max(1)..z1.min(h) {
                if x0 >= 0 && eval(sim, x0, z) {
                    found = Some((x0, z));
                }
                if x1 < w && eval(sim, x1, z) {
                    found = Some((x1, z));
                }
            }
            r += 1;
        }
        found
    }

    #[test]
    fn the_bounded_ring_scan_finds_what_the_old_one_found() {
        // For goals inside the map and up to 40 cells before and behind it (where the reference version does not
        // panic) every result stays the same - also with a blocked start (region 0xFFFF, cells outside hit) and
        // with a filter that rejects the goal itself (distance 0).
        let walls: Vec<_> = (0..32)
            .map(|z| (8, z))
            .chain([(20, 20), (21, 20)])
            .collect();
        let (rules, mut state) = world(&walls);
        let mut sim = Sim::new(&mut state, &rules);
        let free = sim.state.path.region_id(0, 2, 2);
        let make = |c: (i32, i32), region: u16, min_d: i32| {
            let mut best = i32::MAX;
            move |sim: &mut Sim, x: i32, z: i32| {
                let p = cell_center((x, z));
                let d = fixed_hypot(p.0.wrapping_sub(c.0), p.1.wrapping_sub(c.1));
                if d >= best || sim.state.path.region_id(0, x, z) != region || d < min_d {
                    return false;
                }
                best = d;
                true
            }
        };
        for region in [free, BLOCKED] {
            for min_d in [0, 1] {
                for gx in -40..72 {
                    for gz in -40..72 {
                        let c = cell_center((gx, gz));
                        for lim in [-1, 0, 2, 7, 100] {
                            let rmax = ring_limit(32, gx, lim);
                            let old = old_ring_scan(
                                &mut sim,
                                (32, 32),
                                (gx, gz),
                                i32::try_from(rmax).unwrap(),
                                &mut make(c, region, min_d),
                            );
                            let new = ring_scan(
                                &mut sim,
                                (32, 32),
                                (gx, gz),
                                rmax,
                                region == BLOCKED,
                                &mut make(c, region, min_d),
                            );
                            assert_eq!(
                                new, old,
                                "goal ({gx}, {gz}), limit {lim}, region {region}, from {min_d}"
                            );
                        }
                    }
                }
            }
        }
        // Blocked start (wall (8, 3)), goal outside, filter rejects the goal: as in the original the first cell
        // outside in ring 1 (right column, z = 5).
        let away = ReachFilter {
            range_min: 1,
            range_max: i32::MAX,
            home: (-1, -1),
            tether: -1,
            line_of_sight: false,
        };
        assert_eq!(
            find_nearest_reachable_cell(&mut sim, 0, (8, 3), (-10, 5), -1, Some(&away), None),
            Some((-9, 5))
        );
        // Without a filter the goal itself hits (0x4E34/0x4E35 pass no filter).
        assert_eq!(
            find_nearest_reachable_cell(&mut sim, 0, (8, 3), (-10, 5), -1, None, None),
            Some((-10, 5))
        );
    }

    #[test]
    fn rings_far_outside_the_map_terminate() {
        // Cells far outside neither overflow nor run endlessly.
        let (rules, mut state) = world(&[(3, 3)]);
        let mut sim = Sim::new(&mut state, &rules);
        let far = [
            i32::MIN,
            -1_000_000_000,
            -1,
            0,
            31,
            32,
            1_000_000_000,
            i32::MAX,
        ];
        let inside = |c: Option<(i32, i32)>| {
            c.is_none_or(|(x, z)| (0..32).contains(&x) && (0..32).contains(&z))
        };
        for &x in &far {
            for &z in &far {
                let g = (x, z);
                for lim in [-1, i32::MAX] {
                    let got = find_nearest_reachable_cell(&mut sim, 0, (2, 2), g, lim, None, None);
                    assert!(inside(got), "{g:?} -> {got:?}");
                }
                // Start = goal always belongs to its own region (outside: 0xFFFF).
                assert_eq!(
                    find_nearest_reachable_cell(&mut sim, 0, g, g, -1, None, None),
                    Some(g)
                );
                let free = sim.state.path.layers[0].find_free_cell_near(x, z);
                assert!(inside(free), "{g:?} -> {free:?}");
                let _ = find_boarding_cells(&mut sim, 0, (2, 2), g, 0, i32::MAX);
                let _ = find_boarding_cells(&mut sim, 0, g, g, 0, -1);
            }
        }
        let l = &sim.state.path.layers[0];
        // Ring 10^9 - 31 hits the column x = 31; the smallest Manhattan distance has z = 5.
        assert_eq!(l.find_free_cell_near(1_000_000_000, 5), Some((31, 5)));
        assert_eq!(l.find_free_cell_near(i32::MAX, i32::MAX), Some((31, 31)));
        // The columns begin at z = 1 (quirk): row 0 reaches only a row edge.
        assert_eq!(l.find_free_cell_near(i32::MIN, 0), Some((0, 1)));
        // Ring limit from x only (here 26): z far outside never reaches the map.
        assert_eq!(l.find_free_cell_near(5, i32::MIN), None);
        assert_eq!(l.find_free_cell_near(5, 1_000_000_000), None);
    }
}
