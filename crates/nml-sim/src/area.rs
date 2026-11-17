//! Area queries in the spatial grid (`AreaQuery`, `SpatialGrid_GetAreaQueryCount`; re/arming-gates
//! 1.D, 5, 10.1): a query holds a tile rectangle, a filter mode and the list of the
//! objects that matched the filter when they were entered or tracked. The watches of the buildings
//! compare their length with a threshold. Registering with the initial fill
//! ([`register`]), tracking at the insert/remove points of the collision manager
//! ([`notify_insert`], [`notify_remove`], `collision::insert`/`remove`, `world::change_owner`) and
//! counting ([`AreaQueries::count`]) live here; the building side (enemy watch and capturing) lives in
//! [`crate::module::building::watch`].
//!
//! Deviations from the original: the queries are part of the savegame
//! (the original registers them anew after loading, K-297); rectangle queries only (the circle mode 2
//! of `AreaQuery_Construct` has no caller in the shipped data); only the filter mode is stored,
//! the `MatchFilter` is built from it when needed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use nml_formats::targets::MatchFilter;

use crate::collision::BUCKET_TILES;
use crate::id::ObjectId;
use crate::object::status;
use crate::sim::Sim;

/// Filter mode of the enemy watch (`0xA410`: enemy, alive, no ghost; re/arming-gates 3.D1).
pub const ENEMY_WATCH: u32 = 0xA410;
/// Filter mode of the capture watch (`0x4A010`: objects of slots 0-7, alive, no ghost;
/// re/arming-gates 3.D2).
pub const CAPTURE_WATCH: u32 = 0x4A010;

/// An area query (`AreaQuery`, re/arming-gates 5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaQuery {
    /// Filter mode ([`ENEMY_WATCH`] or [`CAPTURE_WATCH`]).
    pub mode: u32,
    /// Rectangle `{x0, z0, x1, z1}` in tiles (inclusive).
    pub rect: [i32; 4],
    /// Reference player of the filter (`< 0` = owner of the object).
    pub player: i32,
    /// Listed objects in entry order.
    pub list: Vec<ObjectId>,
}

/// Registry of all area queries (`GameWorld`/spatial grid): in key order, so that
/// notification and checksum stay deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaQueries {
    /// Counter of the keys handed out (the last key handed out; keys start at 1).
    pub counter: i32,
    /// Queries by key.
    pub queries: BTreeMap<i32, AreaQuery>,
}

impl AreaQueries {
    /// `SpatialGrid_GetAreaQueryCount`: length of the list; an unknown key counts 0.
    pub fn count(&self, key: i32) -> i32 {
        self.queries
            .get(&key)
            .map_or(0, |q| i32::try_from(q.list.len()).unwrap_or(i32::MAX))
    }

    /// Query for the key.
    pub fn get(&self, key: i32) -> Option<&AreaQuery> {
        self.queries.get(&key)
    }

    /// Change of owner (`GameWorld_ChangeObjectOwner` `0x00481730`): the original holds pointers, so the
    /// lists keep their object; the reimplementation replaces the ID at the same index
    /// (deviation from the original, like `CollisionGrid::replace`).
    pub fn replace(&mut self, old: ObjectId, new: ObjectId) {
        for q in self.queries.values_mut() {
            for id in q.list.iter_mut().filter(|id| **id == old) {
                *id = new;
            }
        }
    }
}

/// The filter of the watches from their mode: `mode`, `status = DIED`,
/// `prop_id = 27` (`0x1B`), all other words 0 (`PropertyBuilding_RegisterEnemyAreaWatch`
/// `0x00507DA0`, `…OnMsgRegisterCaptureWatch` `0x00508CF0`; re/arming-gates 3.D1/3.D2).
pub fn filter(mode: u32) -> MatchFilter {
    MatchFilter {
        mode,
        status: status::DIED,
        prop_id: 27,
        ..MatchFilter::default()
    }
}

/// `CollisionMgr_CollectInTileRect` (`0x004B7180`, re/ki-queries 5.12.2): the objects whose tile
/// `o.tile` lies in the tile rectangle `rect = [x0, z0, x1, z1]` (**all four edges inclusive**, K-295)
/// and that pass `filter::matches(sim, id, f, player)`. Buckets from `trunc(x0 / 8)` to
/// `trunc(x1 / 8)` (likewise z, C division) clamped to the grid, z outer, x inner, in each bucket the
/// list from the head (entry order). List B (trees, slot 9) instead of list A when `player == 9`
/// or `f.mode & 0x1000` (`MM_HL_TREELIST`).
///
/// **No ownership restriction:** objects of all slots count; only filters with a list mode
/// restrict (E-65). A rectangle with swapped corners is empty. The tile check is
/// `PtInRect` on `{x0, z0, x1 + 1, z1 + 1}` with wrapping addition: `x1` or `z1 = i32::MAX`
/// gives an empty rectangle.
///
/// [`register`] calls it with the slot of the watcher; that never passes 9 and the watch modes do not carry
/// `0x1000`, so its behavior stays the same.
pub fn collect_in_tile_rect(
    sim: &mut Sim,
    rect: [i32; 4],
    f: &MatchFilter,
    player: i32,
) -> Vec<ObjectId> {
    let [x0, z0, x1, z1] = rect;
    let (rows, cols) = (sim.state.collision.rows, sim.state.collision.cols);
    let mut list = Vec::new();
    if rows <= 0 || cols <= 0 {
        return list;
    }
    let clamp = |v: i32, n: i32| (v / BUCKET_TILES).clamp(0, n - 1);
    let (bx0, bx1) = (clamp(x0, cols), clamp(x1, cols));
    let (bz0, bz1) = (clamp(z0, rows), clamp(z1, rows));
    let trees = player == 9 || f.mode & 0x1000 != 0;
    // `PtInRect` with `right = x1 + 1`, `bottom = z1 + 1` (wrapped to 32 bits): `x1 = i32::MAX` gives an
    // empty rectangle.
    let (right, bottom) = (x1.wrapping_add(1), z1.wrapping_add(1));
    for bz in bz0..=bz1 {
        for bx in bx0..=bx1 {
            let Some(i) = sim.state.collision.index(bx, bz) else {
                continue;
            };
            // Copy of the bucket list: the filter takes `&mut Sim`.
            let bucket = if trees {
                sim.state.collision.trees[i].clone()
            } else {
                sim.state.collision.all[i].clone()
            };
            for id in bucket {
                let Some(tile) = sim.object(id).map(|o| o.tile) else {
                    continue;
                };
                let inside = x0 <= tile.0 && tile.0 < right && z0 <= tile.1 && tile.1 < bottom;
                if inside && crate::filter::matches(sim, id, f, player) {
                    list.push(id);
                }
            }
        }
    }
    list
}

/// `SpatialGrid_RegisterAreaQuery` (`0x004B81A0`, re/arming-gates 3.D3): the counter rises by 1 (32 bits,
/// as in the original), the query gets the key and is filled with the objects that
/// already stand in the area ([`collect_in_tile_rect`], the initial fill is **inclusive**, K-295; the
/// tracking, by contrast, is half-open). Returns the key.
pub fn register(sim: &mut Sim, mode: u32, rect: [i32; 4], player: i32) -> i32 {
    let key = sim.state.area_queries.counter.wrapping_add(1);
    sim.state.area_queries.counter = key;
    let list = collect_in_tile_rect(sim, rect, &filter(mode), player);
    sim.state.area_queries.queries.insert(
        key,
        AreaQuery {
            mode,
            rect,
            player,
            list,
        },
    );
    key
}

/// Unregister a query (`SpatialGrid_UnregisterAreaQuery` `0x004B68E0`); an unknown key does
/// nothing.
pub fn unregister(sim: &mut Sim, key: i32) {
    sim.state.area_queries.queries.remove(&key);
}

/// `notify_area_queries_insert` (`0x004B62A0`): every query in key order
/// (`AreaQuery_UpdateObject` `0x00406240`, re/collision A1.5, re/arming-gates 3.D3).
///
/// - If the object does **not** match the filter, everything stays as it is, including an existing entry
///   (K-296: death and diplomacy changes do not remove entries).
/// - Otherwise "inside" is half-open (`PtInRect`, `0x00405F30`): `x0 ≤ x < x1` and `z0 ≤ z < z1` with the
///   tile `o.tile` (K-295). Not listed and inside -> append at the tail; listed and outside ->
///   remove.
///
/// The filter takes `&mut Sim`: keys and, per query, `(mode, rectangle, player, listed?)`
/// are copied beforehand, the list is changed only after the check (no reference into the
/// registry across [`crate::filter::matches`]).
pub fn notify_insert(sim: &mut Sim, id: ObjectId) {
    if sim.state.area_queries.queries.is_empty() {
        return;
    }
    let Some(tile) = sim.object(id).map(|o| o.tile) else {
        return;
    };
    let keys: Vec<i32> = sim.state.area_queries.queries.keys().copied().collect();
    for key in keys {
        let Some(q) = sim.state.area_queries.queries.get(&key) else {
            continue;
        };
        let (mode, rect, player, listed) = (q.mode, q.rect, q.player, q.list.contains(&id));
        if !crate::filter::matches(sim, id, &filter(mode), player) {
            continue;
        }
        let [x0, z0, x1, z1] = rect;
        let inside = x0 <= tile.0 && tile.0 < x1 && z0 <= tile.1 && tile.1 < z1;
        let Some(q) = sim.state.area_queries.queries.get_mut(&key) else {
            continue;
        };
        match (listed, inside) {
            (false, true) => q.list.push(id),
            (true, false) => {
                if let Some(p) = q.list.iter().position(|&x| x == id) {
                    q.list.remove(p);
                }
            }
            _ => {}
        }
    }
}

/// `notify_area_queries_remove` (`0x004B62F0`, `AreaQuery_RemoveObject` `0x00406100`): the first occurrence
/// from every list.
pub fn notify_remove(sim: &mut Sim, id: ObjectId) {
    for q in sim.state.area_queries.queries.values_mut() {
        if let Some(p) = q.list.iter().position(|&x| x == id) {
            q.list.remove(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::TREE;
    use crate::module::building::fort_testkit::{
        self as kit, COLONIST, HOUSE, SOLDIER, move_to, run_steps, spawn,
    };
    use crate::world::{destroy_object, process_owner_changes, queue_owner_change};

    fn list(sim: &Sim, key: i32) -> Vec<ObjectId> {
        sim.state.area_queries.get(key).unwrap().list.clone()
    }

    #[test]
    fn first_fill_is_inclusive_and_tracking_half_open() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Enemies in the right column, the bottom row and inside, before registering.
        let right = spawn(&mut sim, SOLDIER, 1, (15, 12));
        let bottom = spawn(&mut sim, SOLDIER, 1, (12, 15));
        let inner = spawn(&mut sim, SOLDIER, 1, (14, 12));
        // More enemies outside and an own unit in the area.
        let before_right = spawn(&mut sim, SOLDIER, 1, (16, 12));
        let to_leave = spawn(&mut sim, SOLDIER, 1, (14, 13));
        let corner = spawn(&mut sim, SOLDIER, 1, (9, 9));
        let own = spawn(&mut sim, COLONIST, 0, (12, 12));
        let key = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        assert_eq!(key, 1);
        // Initial fill inclusive; order = list order of the bucket (creation).
        assert_eq!(list(&sim, key), [right, bottom, inner, to_leave]);
        // Tracking half-open: nothing is entered into the right column (x = 15) ...
        move_to(&mut sim, before_right, (15, 12));
        assert!(!list(&sim, key).contains(&before_right));
        // ... whoever leaves the area is removed ...
        move_to(&mut sim, to_leave, (16, 13));
        assert!(!list(&sim, key).contains(&to_leave));
        // ... and tile (10, 10) enters, appended at the tail.
        move_to(&mut sim, corner, (10, 10));
        assert_eq!(list(&sim, key), [right, bottom, inner, corner]);
        // Whoever stands in the right column stays listed as long as it stands; every tile change
        // removes and re-checks, and (15, 13) lies outside the half-open area (K-295).
        assert!(list(&sim, key).contains(&right));
        move_to(&mut sim, right, (15, 13));
        assert!(!list(&sim, key).contains(&right));
        // Own unit: never listed, not even when entering.
        move_to(&mut sim, own, (11, 11));
        assert!(!list(&sim, key).contains(&own));
        assert_eq!(sim.state.area_queries.count(key), 3);
    }

    #[test]
    fn objects_that_stop_matching_stay_listed() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Enemy in the right column (x = 15): the inclusive initial fill lists it although it
        // stands outside for the half-open tracking.
        let enemy = spawn(&mut sim, SOLDIER, 1, (15, 12));
        let kept = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        let removed = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        assert_eq!(list(&sim, kept), [enemy]);
        assert_eq!(list(&sim, removed), [enemy]);
        // Counterpart: if the filter still matches (enemy) and it stands outside, `notify_insert`
        // removes it (branch "listed + outside -> remove" of `AreaQuery_UpdateObject`).
        notify_insert(&mut sim, enemy);
        assert!(list(&sim, kept).is_empty() && list(&sim, removed).is_empty());
        // Fill again, then diplomacy: slot 1 becomes a friend of slot 0 (no longer an enemy). The filter
        // no longer matches, `AreaQuery_UpdateObject` leaves the entry, although the object
        // stands outside the area and would be removed with a matching filter (K-296).
        let kept = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        assert_eq!(list(&sim, kept), [enemy]);
        sim.state.diplomacy.set_friend(0, 1, true);
        sim.state.diplomacy.set_enemy(0, 1, false);
        notify_insert(&mut sim, enemy);
        assert_eq!(list(&sim, kept), [enemy]);
        // Every movement is a tile change `remove` -> `insert` (`set_grid_position`): `remove`
        // removes unconditionally, `insert` does not take it up with the filter that no longer matches.
        move_to(&mut sim, enemy, (13, 12));
        assert!(list(&sim, kept).is_empty());
    }

    #[test]
    fn a_dying_enemy_stays_listed_and_a_destroyed_one_leaves() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let enemy = spawn(&mut sim, SOLDIER, 1, (12, 12));
        let key = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        assert_eq!(list(&sim, key), [enemy]);
        // Death: status `DIED`, then the change of owner to slot 8 via the queue.
        sim.object_mut(enemy).unwrap().status |= status::DIED;
        queue_owner_change(&mut sim, enemy, 8);
        process_owner_changes(&mut sim);
        let corpse = list(&sim, key);
        assert_eq!(corpse.len(), 1);
        assert_ne!(corpse[0], enemy, "new ID");
        assert_eq!(corpse[0].slot(), 8);
        assert!(sim.object(corpse[0]).is_some());
        // Destroying removes it from all lists.
        destroy_object(&mut sim, corpse[0]);
        assert!(list(&sim, key).is_empty());
    }

    #[test]
    fn owner_changes_replace_ids_in_area_lists() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, SOLDIER, 1, (11, 11));
        let b = spawn(&mut sim, SOLDIER, 1, (12, 12));
        let c = spawn(&mut sim, SOLDIER, 1, (13, 13));
        // Capture watch: objects of slots 0-7 (alive, no ghost), reference player 8.
        let key = register(&mut sim, CAPTURE_WATCH, [10, 10, 15, 15], 8);
        assert_eq!(list(&sim, key), [a, b, c]);
        queue_owner_change(&mut sim, b, 0);
        process_owner_changes(&mut sim);
        let l = list(&sim, key);
        assert_eq!(l.len(), 3, "no duplicate, no loss");
        assert_eq!((l[0], l[2]), (a, c));
        assert_ne!(l[1], b);
        assert_eq!(l[1].slot(), 0);
        // The new ID is a living object, the old one no longer exists.
        assert!(sim.object(l[1]).is_some() && sim.object(b).is_none());
        assert_eq!(sim.state.area_queries.count(key), 3);
    }

    #[test]
    fn count_and_unregister_follow_the_key() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, SOLDIER, 1, (12, 12));
        spawn(&mut sim, SOLDIER, 1, (13, 12));
        let k1 = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        let k2 = register(&mut sim, ENEMY_WATCH, [12, 12, 13, 12], 0);
        assert_eq!((k1, k2), (1, 2));
        assert_eq!(sim.state.area_queries.count(k1), 2);
        assert_eq!(sim.state.area_queries.count(k2), 2);
        // Unknown key: 0, unregistering has no effect.
        assert_eq!(sim.state.area_queries.count(99), 0);
        unregister(&mut sim, 99);
        assert_eq!(sim.state.area_queries.queries.len(), 2);
        unregister(&mut sim, k1);
        assert_eq!(sim.state.area_queries.count(k1), 0);
        assert!(sim.state.area_queries.get(k1).is_none());
        // The keys keep rising, also after `unregister`.
        let k3 = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        assert_eq!(k3, 3);
        assert_eq!(sim.state.area_queries.counter, 3);
        // The 32-bit counter overflows without panicking.
        sim.state.area_queries.counter = i32::MAX;
        assert_eq!(
            register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0),
            i32::MIN
        );
    }

    #[test]
    fn a_new_unit_is_listed_once() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let key = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        // `create_object` enters twice (before and after the modules); the query does not take up the
        // enemy until the second time, and the `remove` in between cleans up: exactly one entry.
        let enemy = spawn(&mut sim, SOLDIER, 1, (12, 12));
        assert_eq!(list(&sim, key), [enemy]);
        // A building of the opponent (house, module 3 switched on) also hits the enemy filter
        // (re/arming-gates 3.D1: "target needs module 3 switched on or Walk"), likewise once.
        let house = spawn(&mut sim, HOUSE, 1, (13, 13));
        assert_eq!(list(&sim, key), [enemy, house]);
        // Even a whole object tick changes nothing.
        run_steps(&mut sim, 2);
        assert_eq!(list(&sim, key), [enemy, house]);
    }

    /// Filter without a list mode (`MM_ANDALL`): every living object matches, whatever the slot.
    fn all_filter() -> MatchFilter {
        MatchFilter {
            mode: 0x10,
            ..MatchFilter::constructed()
        }
    }

    #[test]
    fn collect_in_tile_rect_is_inclusive_and_ignores_the_owner() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Rectangle (10, 10)-(13, 13), all in bucket (1, 1): the four edge tiles inside, four
        // tiles just outside, plus one object each of slots 0, 1 and of the neutral slot 8.
        let inside = [
            spawn(&mut sim, SOLDIER, 0, (10, 10)),
            spawn(&mut sim, SOLDIER, 1, (13, 10)),
            spawn(&mut sim, SOLDIER, 8, (10, 13)),
            spawn(&mut sim, SOLDIER, 1, (13, 13)),
            spawn(&mut sim, COLONIST, 0, (11, 12)),
        ];
        for tile in [(9, 10), (14, 13), (10, 9), (13, 14)] {
            spawn(&mut sim, SOLDIER, 1, tile);
        }
        let rect = [10, 10, 13, 13];
        // Without a list mode all owners count, the reference player plays no role.
        for player in [-1, 0, 1, 5] {
            let got = collect_in_tile_rect(&mut sim, rect, &all_filter(), player);
            assert_eq!(got, inside, "reference {player}");
        }
        // With `MM_HL_MYLIST` and reference 1: only the objects of slot 1 in the rectangle.
        let mine = MatchFilter {
            mode: 0x110,
            ..MatchFilter::constructed()
        };
        assert_eq!(
            collect_in_tile_rect(&mut sim, rect, &mine, 1),
            [inside[1], inside[3]]
        );
        // A rectangle with swapped corners is empty.
        assert!(collect_in_tile_rect(&mut sim, [13, 13, 10, 10], &all_filter(), 0).is_empty());
    }

    #[test]
    fn collect_in_tile_rect_tests_the_point_like_ptinrect_with_a_wrapping_edge() {
        // `right = x1 + 1`, `bottom = z1 + 1` (32 bits, wrapped), `PtInRect` tests
        // `left <= x < right`: a rectangle up to `i32::MAX` becomes `right = i32::MIN` and is empty
        // (re/ki-queries 5.12.2; the bucket loop still clamps to the grid).
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, SOLDIER, 0, (10, 10));
        let b = spawn(&mut sim, SOLDIER, 1, (40, 3));
        assert_eq!(
            collect_in_tile_rect(&mut sim, [0, 0, 63, 63], &all_filter(), 0),
            [b, a],
            "buckets row by row: z = 3 before z = 10"
        );
        for rect in [
            [0, 0, i32::MAX, i32::MAX],
            [0, 0, i32::MAX, 63],
            [0, 0, 63, i32::MAX],
            [i32::MIN, i32::MIN, i32::MAX, i32::MAX],
        ] {
            assert!(
                collect_in_tile_rect(&mut sim, rect, &all_filter(), 0).is_empty(),
                "{rect:?}"
            );
        }
        // Just below, the edge stays inclusive and the buckets are clamped.
        assert_eq!(
            collect_in_tile_rect(
                &mut sim,
                [0, 0, i32::MAX - 1, i32::MAX - 1],
                &all_filter(),
                0
            ),
            [b, a]
        );
    }

    #[test]
    fn collect_in_tile_rect_walks_buckets_row_by_row_and_clamps_to_the_grid() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Buckets are 8 x 8 tiles: z outer, x inner, in each bucket in entry order.
        let late = spawn(&mut sim, SOLDIER, 0, (5, 10));
        let first = spawn(&mut sim, SOLDIER, 0, (5, 5));
        let second = spawn(&mut sim, SOLDIER, 0, (10, 5));
        let same_bucket = spawn(&mut sim, SOLDIER, 0, (6, 6));
        assert_eq!(
            collect_in_tile_rect(&mut sim, [4, 4, 12, 12], &all_filter(), 0),
            [first, same_bucket, second, late]
        );
        // Negative edges and edges beyond the grid are clamped to the buckets; the
        // tile check stays strict (the map has 64 x 64 tiles).
        let corner = spawn(&mut sim, SOLDIER, 0, (1, 1));
        let far = spawn(&mut sim, SOLDIER, 0, (63, 63));
        assert_eq!(
            collect_in_tile_rect(&mut sim, [-9, -9, 3, 3], &all_filter(), 0),
            [corner]
        );
        assert_eq!(
            collect_in_tile_rect(&mut sim, [60, 60, 200, 200], &all_filter(), 0),
            [far]
        );
    }

    #[test]
    fn collect_in_tile_rect_reads_the_tree_list_for_player_9_or_the_tree_filter() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let soldier = spawn(&mut sim, SOLDIER, 0, (11, 11));
        let tree = spawn(&mut sim, TREE, 9, (12, 12));
        let rect = [10, 10, 13, 13];
        // Reference 9: list B (trees) instead of list A.
        assert_eq!(
            collect_in_tile_rect(&mut sim, rect, &all_filter(), 9),
            [tree]
        );
        // `MM_HL_TREELIST` in the filter selects the same list, even with another reference.
        let trees = MatchFilter {
            mode: 0x1010,
            ..MatchFilter::constructed()
        };
        assert_eq!(collect_in_tile_rect(&mut sim, rect, &trees, 0), [tree]);
        // Otherwise list A: the tree is not in it.
        assert_eq!(
            collect_in_tile_rect(&mut sim, rect, &all_filter(), 0),
            [soldier]
        );
        // No caller of `register` passes reference 9 or the tree list mode: the watches
        // take the slot of a building, their modes do not have the bit `0x1000` (the behavior of
        // `register` stays as it was before the extraction).
        assert_eq!((ENEMY_WATCH | CAPTURE_WATCH) & 0x1000, 0);
    }
}
