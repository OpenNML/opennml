//! Tests of the path and fog queries (re/ki-queries 5.17, 5.18, 5.20; E-33, E-77, E-78, E-79).

use super::handlers::{ROWS, row};
use super::tests_player::{FLOOR, ME, run, run_with};
use super::tests_search::{ME_I, oid, put, scene, search_rules};
use super::{dispatch, is_query, query_once};
use crate::combat::testkit::{HQ, WARRIOR};
use crate::command::testkit::{fogged_state, rules as plain_rules};
use crate::ki::signature::signature;
use crate::ki::testkit::int;
use crate::object::status;
use crate::path::LAYER_COUNT;
use crate::player::{Sight, View};
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;

/// Map width and height of the test world.
const SIZE: i32 = 64;

/// Hostile values for cells, radii and players.
const HOSTILE: [i32; 9] = [
    i32::MIN,
    -1_000_000_000,
    -1,
    0,
    SIZE - 1,
    SIZE,
    1_000_000_000,
    i32::MAX,
    5,
];

/// World in which `block` marks the blocked cells of layer 0 before the regions are built (`mark_cell` after the build
/// would only update the regions).
fn blocked_world(block: impl Fn(i32, i32) -> bool) -> (Rules, SimState) {
    let rules = search_rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    for x in 0..SIZE {
        for z in 0..SIZE {
            if block(x, z) {
                state.path.mark_cell(1, x, z, true);
            }
        }
    }
    state.path.build_regions(u32::MAX);
    (rules, state)
}

/// World with a wall in column `x` (layer 0).
fn walled(x: i32) -> (Rules, SimState) {
    blocked_world(|cx, _| cx == x)
}

/// `0x4E34` field with argc 5: `[3]`, `[4]` cell, `[5]`, `[6]` with placeholders.
fn reach_field(sim: &mut Sim, x: i32, z: i32) -> (i32, i32) {
    let (req, direct) = query_once(sim, ME, Sight::Omniscient, 0x4E34, &[ME_I, x, z, -11, -22]);
    assert!(direct.is_empty());
    assert_eq!((req[2], req[3], req[4]), (ME_I, x, z), "input stays");
    (req[5], req[6])
}

#[test]
fn reachable_cell_query_returns_its_input() {
    // E-77: start = goal of the ring search in layer 4; the goal cell always fits, even blocked and outside the map.
    let (rules, mut state) = walled(20);
    let mut sim = Sim::new(&mut state, &rules);
    for (x, z) in [(10, 10), (20, 10), (0, 0), (63, 63), (-5, 7), (100, 100)] {
        assert_eq!(reach_field(&mut sim, x, z), (x, z), "cell ({x}, {z})");
    }
    for &x in &HOSTILE {
        for &z in &HOSTILE {
            assert_eq!(reach_field(&mut sim, x, z), (x, z), "cell ({x}, {z})");
        }
    }
    // Builtin 0x87 `(x, z, &x', &z')`: no push, `var(a3) = x`, `var(a4) = z`.
    let (t, notes) = run_with(&mut sim, 0x87, &[FLOOR, 7, 9, int(0) as i32, int(1) as i32]);
    assert!(notes.is_empty());
    assert_eq!(t.stack, [FLOOR]);
    assert_eq!((t.vars.get(int(0)), t.vars.get(int(1))), (7, 9));
    // If both outputs point to the same variable, the second (`z`) wins.
    let (t, _) = run_with(&mut sim, 0x87, &[FLOOR, 7, 9, int(2) as i32, int(2) as i32]);
    assert_eq!(t.vars.get(int(2)), 9);
    // Wrong argc: the field stays.
    let (req, _) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E34, &[ME_I, 3, 4, -11]);
    assert_eq!((req[5], req[6]), (-11, 0));
}

/// Result of `0x88 (obj, x, z, &x', &z')`: (push, x', z') with the variables 0 and 1.
fn object_reach(sim: &mut Sim, id: u16, obj: i32, x: i32, z: i32) -> (i32, i32, i32) {
    let args = [FLOOR, obj, x, z, int(0) as i32, int(1) as i32];
    let (t, notes) = run_with(sim, id, &args);
    assert!(notes.is_empty());
    assert_eq!(t.stack[..1], [FLOOR], "pops too much");
    assert_eq!(t.stack.len(), 2, "exactly one push");
    (t.stack[1], t.vars.get(int(0)), t.vars.get(int(1)))
}

#[test]
fn object_reachable_cell_handles_missing_dead_and_walkless() {
    let (rules, mut state) = walled(20);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let dead = put(&mut sim, WARRIOR, ME_I, (11, 10));
    sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
    let hq = put(&mut sim, HQ, ME_I, (30, 30));
    for id in [0x88, 0xB2] {
        // Goal in the region of the object's tile: returned unchanged.
        assert_eq!(
            object_reach(&mut sim, id, walker, 12, 14),
            (1, 12, 14),
            "{id:#x}"
        );
        // Goal behind the wall: the nearest cell of the west region.
        assert_eq!(
            object_reach(&mut sim, id, walker, 30, 10),
            (1, 19, 10),
            "{id:#x}"
        );
        // The goal itself is a wall cell: the nearest free cell of the west region.
        let (found, x, z) = object_reach(&mut sim, id, walker, 20, 10);
        assert_eq!((found, x, z), (1, 19, 10), "{id:#x}");
        // Object missing (0, -1, never assigned): push 0, the outputs are their own addresses (E-33).
        for missing in [0, -1, 77_777, i32::MIN, i32::MAX] {
            assert_eq!(
                object_reach(&mut sim, id, missing, 12, 14),
                (0, int(0) as i32, int(1) as i32),
                "{id:#x}: object {missing}"
            );
        }
        // Dead or without walk (layer -1): the input cell, push 0.
        for obj in [dead, hq] {
            assert_eq!(
                object_reach(&mut sim, id, obj, 30, 10),
                (0, 30, 10),
                "{id:#x}: object {obj:#x}"
            );
        }
    }
    // With a wall, map cells carry the blocked region of the tile outside the map: the goal finds the nearest wall
    // cell (the region of the object's tile is that of the blocked cells).
    sim.object_mut(oid(walker)).unwrap().tile = (-50, -50);
    assert_eq!(object_reach(&mut sim, 0x88, walker, 12, 14), (1, 20, 14));
}

#[test]
fn failed_ring_search_writes_zero_instead_of_stack_leftovers() {
    // E-33: the ring search fails if no map cell carries the region of the object's tile. The original would then write
    // stack leftovers of the dispatcher; the reimplementation 0 and 0 (push 0).
    let (rules, mut state) = blocked_world(|_, _| false);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    assert_eq!(object_reach(&mut sim, 0x88, walker, 12, 14), (1, 12, 14));
    // Outside the map the region is blocked (0xFFFF), and no map cell is.
    sim.object_mut(oid(walker)).unwrap().tile = (-50, -50);
    for id in [0x88, 0xB2] {
        assert_eq!(
            object_reach(&mut sim, id, walker, 12, 14),
            (0, 0, 0),
            "{id:#x}"
        );
    }
}

#[test]
fn object_reachable_cell_survives_hostile_cells() {
    // Hostile goals and object tiles neither overflow nor loop endlessly.
    let (rules, mut state) = walled(20);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let inside = |x: i32, z: i32| (0..SIZE).contains(&x) && (0..SIZE).contains(&z);
    for &x in &HOSTILE {
        for &z in &HOSTILE {
            let (found, gx, gz) = object_reach(&mut sim, 0x88, walker, x, z);
            assert!(
                found == 0 || inside(gx, gz),
                "goal ({x}, {z}) -> ({gx}, {gz})"
            );
        }
    }
    // Hostile object tiles: the region outside is blocked, the goal itself then fits.
    for &x in &HOSTILE {
        sim.object_mut(oid(walker)).unwrap().tile = (x, x);
        for &z in &HOSTILE {
            let _ = object_reach(&mut sim, 0x88, walker, x, z);
            let _ = object_reach(&mut sim, 0x88, walker, z, x);
        }
    }
}

/// `0x1A1 (obj, sx, sz, tx, tz)`: the result `[2]`.
fn path_len(sim: &mut Sim, obj: i32, s: (i32, i32), g: (i32, i32)) -> i32 {
    let stack = [FLOOR, obj, s.0, s.1, g.0, g.1];
    let (t, notes) = run_with(sim, 0x1A1, &stack);
    assert!(notes.is_empty());
    assert_eq!(t.stack.len(), 2, "exactly one push");
    t.stack[1]
}

#[test]
fn tile_path_length_counts_cells_and_zero_means_none() {
    let (rules, mut state) = walled(20);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let dead = put(&mut sim, WARRIOR, ME_I, (11, 10));
    sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
    let hq = put(&mut sim, HQ, ME_I, (30, 30));
    // Cell count without the start cell, straight lines and diagonals.
    assert_eq!(path_len(&mut sim, walker, (10, 10), (15, 10)), 5);
    assert_eq!(path_len(&mut sim, walker, (10, 10), (10, 3)), 7);
    assert_eq!(path_len(&mut sim, walker, (2, 2), (9, 9)), 7);
    // Start = goal (E-78): 0.
    assert_eq!(path_len(&mut sim, walker, (10, 10), (10, 10)), 0);
    // Goal behind the wall: it moves into the start region (`FindTilePath` relocates it by ring search).
    assert_eq!(path_len(&mut sim, walker, (10, 10), (30, 10)), 9);
    // Goal outside the map: no path.
    for goal in [(64, 10), (10, 64), (-1, 10), (10, -1), (i32::MAX, 3)] {
        assert_eq!(path_len(&mut sim, walker, (10, 10), goal), 0, "{goal:?}");
    }
    // Object missing, dead, without walk (layer -1): 0, never the player number (E-27 does not apply here).
    for obj in [0, -1, 77_777, dead, hq] {
        assert_eq!(path_len(&mut sim, obj, (10, 10), (15, 10)), 0, "{obj:#x}");
    }
    // Wrong argc: the field stays (`[2]` is the slot).
    let (req, _) = query_once(
        &mut sim,
        ME,
        Sight::Omniscient,
        0x4FB0,
        &[ME_I, walker, 10, 10, 15],
    );
    assert_eq!(req[2], ME_I);
}

#[test]
fn tile_path_start_is_not_checked_for_freedom() {
    // E-78: the start cell moves to the nearest free cell; the original does not check whether that succeeded. The
    // reimplementation then leaves the start where it is and asks for the path anyway.
    let (rules, mut state) = walled(20);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let v = View::new(&mut sim, ME, Sight::Omniscient);
    let moved = v.free_cell(0, (20, 10)).expect("free cell");
    assert_ne!(moved, (20, 10), "the wall cell is not free");
    let want = v.tile_path_len(0, moved, (25, 10)).unwrap_or(0);
    assert_eq!(
        path_len(&mut sim, walker, (20, 10), (25, 10)) as usize,
        want
    );
    // Whole map blocked: no free cell, the start stays.
    let (rules, mut state) = blocked_world(|_, _| true);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let v = View::new(&mut sim, ME, Sight::Omniscient);
    assert_eq!(v.free_cell(0, (10, 10)), None);
    let want = v.tile_path_len(0, (10, 10), (15, 10)).unwrap_or(0);
    assert_eq!(
        path_len(&mut sim, walker, (10, 10), (15, 10)) as usize,
        want
    );
    assert_eq!(path_len(&mut sim, walker, (3, 3), (3, 3)), 0);
}

#[test]
fn tile_path_length_survives_hostile_cells() {
    let (rules, mut state) = walled(20);
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (10, 10));
    for &a in &HOSTILE {
        for &b in &HOSTILE {
            let n = path_len(&mut sim, walker, (a, b), (b, a));
            assert!((0..=1000).contains(&n), "({a}, {b}) -> {n}");
            let m = path_len(&mut sim, walker, (10, 10), (a, b));
            assert!((0..=1000).contains(&m), "-> ({a}, {b}) -> {m}");
            let k = path_len(&mut sim, walker, (a, b), (10, 10));
            assert!((0..=1000).contains(&k), "({a}, {b}) -> {k}");
        }
    }
}

#[test]
fn very_long_paths_count_as_none() {
    // Over 1000 cells the original overflows the buffer; the reimplementation reports 0. Serpentine path: wall rows z =
    // 1, 3, 5 ... each with a gap alternately at the right and left edge.
    let (rules, mut state) = blocked_world(|x, z| {
        let gap = if (z / 2) % 2 == 0 { SIZE - 1 } else { 0 };
        z % 2 == 1 && z < SIZE - 1 && x != gap
    });
    let mut sim = Sim::new(&mut state, &rules);
    let walker = put(&mut sim, WARRIOR, ME_I, (0, 0));
    // One turn away: a path under the limit returns its cell count.
    let near = path_len(&mut sim, walker, (0, 0), (0, 2));
    assert!(near > 60, "one turn: {near}");
    let mid = path_len(&mut sim, walker, (0, 0), (0, 20));
    assert!(
        (300..=1000).contains(&mid),
        "a good ten turns under the limit: {mid}"
    );
    // At the end of the serpentine there are about 1900 cells: 0 instead of the overflow.
    assert_eq!(path_len(&mut sim, walker, (0, 0), (0, 62)), 0);
    // The counterpart through the view: the same path is too long (`None`).
    let v = View::new(&mut sim, ME, Sight::Omniscient);
    assert_eq!(v.tile_path_len(0, (0, 0), (0, 62)), None);
}

/// Fog world (nothing uncovered) with the rules of the test world.
fn fog_world() -> (Rules, SimState) {
    let rules = plain_rules();
    let state = fogged_state(&rules);
    (rules, state)
}

/// Square query `code` (`0x4FF6`/`0x500A`) with argc 5 and `[2] = me`, omniscient.
fn fog_hit(sim: &mut Sim, code: u32, p: i32, x: i32, z: i32, r: i32) -> i32 {
    query_once(sim, ME, Sight::Omniscient, code, &[ME_I, p, x, z, r]).0[2]
}

#[test]
fn fog_square_is_inclusive_and_clamped_to_the_map() {
    let (rules, mut state) = fog_world();
    state.fog.seen.or(10, 10, 1 << 2);
    state.fog.seen.or(0, 0, 1 << 2);
    state.fog.seen.or(SIZE - 1, SIZE - 1, 1 << 3);
    let mut sim = Sim::new(&mut state, &rules);
    for code in [0x500A, 0x4FF6] {
        let hit = |sim: &mut Sim, x, z, r| fog_hit(sim, code, 2, x, z, r);
        // r = 0: exactly the one tile.
        assert_eq!(hit(&mut sim, 10, 10, 0), 1, "{code:#x}");
        assert_eq!(hit(&mut sim, 11, 10, 0), 0);
        assert_eq!(hit(&mut sim, 10, 11, 0), 0);
        // Both bounds inclusive: the corner of the square hits, one tile further does not.
        assert_eq!(hit(&mut sim, 12, 12, 2), 1);
        assert_eq!(hit(&mut sim, 12, 12, 1), 0);
        assert_eq!(hit(&mut sim, 8, 8, 2), 1);
        assert_eq!(hit(&mut sim, 8, 12, 2), 1);
        assert_eq!(hit(&mut sim, 13, 10, 2), 0);
        // Negative r acts like |r|.
        assert_eq!(hit(&mut sim, 12, 12, -2), 1);
        assert_eq!(hit(&mut sim, 12, 12, -1), 0);
        // A huge r is limited to the map and hits tile (0, 0) from the edge.
        assert_eq!(hit(&mut sim, 5, 5, 10_000), 1);
        assert_eq!(hit(&mut sim, SIZE - 1, SIZE - 1, 10_000), 1);
        assert_eq!(hit(&mut sim, -3, -3, 3), 1, "square extends past the edge");
        assert_eq!(hit(&mut sim, -3, -3, 2), 0);
        assert_eq!(
            hit(&mut sim, SIZE + 2, SIZE + 2, 3),
            0,
            "slot 3 is a different one"
        );
        // Slot 3 sees only the corner (63, 63).
        assert_eq!(fog_hit(&mut sim, code, 3, SIZE + 2, SIZE + 2, 3), 1);
        assert_eq!(fog_hit(&mut sim, code, 3, SIZE + 2, SIZE + 2, 2), 0);
    }
    // Wrapped bounds with a swap instead of `abs`. `x = i32::MAX, r = 1`: `x − r = MAX − 1`, `x + r` overflows to
    // `MIN`, the swapped bounds `[MIN, MAX − 1]` cover the map.
    assert_eq!(fog_hit(&mut sim, 0x500A, 2, i32::MAX, 10, 1), 1);
    assert_eq!(
        fog_hit(&mut sim, 0x500A, 2, i32::MAX, 20, 1),
        0,
        "row 20 is empty"
    );
    // `r = i32::MIN`: both bounds fall on the same value `x ^ 0x8000_0000` (wrapped), the square is one tile. Only
    // `x = z = i32::MIN` hits (0, 0), all others lie outside.
    for x in [i32::MAX, 0, 10, i32::MIN] {
        for z in [i32::MAX, 0, 10, i32::MIN] {
            for code in [0x500A, 0x4FF6] {
                let got = fog_hit(&mut sim, code, 2, x, z, i32::MIN);
                let expected = i32::from(x == i32::MIN && z == i32::MIN);
                assert_eq!(got, expected, "{code:#x} ({x}, {z})");
            }
        }
    }
    assert_eq!(fog_hit(&mut sim, 0x500A, 2, i32::MAX, 0, i32::MIN), 0);
    // E-79 (exception A7): upper bound exactly `i32::MAX` (`x = i32::MAX`, `r = 0`). The `<=` loop of the
    // original wrapped over `i32::MIN` and would hit tile (10, 10) late in row 10; the reimplementation evaluates only
    // the map tiles of the square - here none - and returns 0.
    assert_eq!(fog_hit(&mut sim, 0x500A, 2, i32::MAX, 10, 0), 0);
    // All hostile radii and centers without a panic; endless loops are also ruled out.
    for &x in &HOSTILE {
        for &z in &HOSTILE {
            for &r in &HOSTILE {
                let _ = fog_hit(&mut sim, 0x500A, 2, x, z, r);
            }
        }
    }
    // The query does not check `argc` (5.17).
    let mut req = [0x500A, 0, 0, 2, 10, 10, 0];
    dispatch(
        &mut View::new(&mut sim, ME, Sight::Omniscient),
        &mut req,
        &mut Vec::new(),
    );
    assert_eq!(req[2], 1);
}

#[test]
fn fog_bits_follow_explored_and_seen() {
    let (rules, mut state) = fog_world();
    // Slot 2: (1, 1) only explored, (2, 1) seen, (3, 1) scouted.
    state.fog.explored.or(1, 1, 1 << 2);
    state.fog.seen.or(2, 1, 1 << 2);
    state.fog.cleared.or(3, 1, 1 << 2);
    // Slot 4 sees what slot 5 sees (one-sided).
    state.fog.seen.or(7, 7, 1 << 5);
    state.diplomacy.set_vision(4, 5, true);
    let mut sim = Sim::new(&mut state, &rules);
    // (tile, bit "explored" 0x4FF6, bit "seen" 0x500A)
    for ((x, z), explored, seen) in [
        ((1, 1), 1, 0),
        ((2, 1), 1, 1),
        ((3, 1), 1, 1),
        ((4, 1), 0, 0),
    ] {
        assert_eq!(
            fog_hit(&mut sim, 0x4FF6, 2, x, z, 0),
            explored,
            "({x}, {z})"
        );
        assert_eq!(fog_hit(&mut sim, 0x500A, 2, x, z, 0), seen, "({x}, {z})");
    }
    // A hit anywhere in the square is enough.
    assert_eq!(fog_hit(&mut sim, 0x4FF6, 2, 2, 2, 1), 1);
    assert_eq!(fog_hit(&mut sim, 0x500A, 2, 1, 2, 1), 1);
    assert_eq!(
        fog_hit(&mut sim, 0x500A, 2, 0, 2, 1),
        0,
        "only (1, 1) explored"
    );
    // Shared vision counts; it applies in one direction only.
    assert_eq!(fog_hit(&mut sim, 0x500A, 4, 7, 7, 0), 1);
    assert_eq!(fog_hit(&mut sim, 0x4FF6, 4, 7, 7, 0), 1);
    assert_eq!(fog_hit(&mut sim, 0x500A, 5, 7, 7, 0), 1);
    assert_eq!(fog_hit(&mut sim, 0x500A, 6, 7, 7, 0), 0);
    state_sees_nothing_for_foreign_slots(&mut sim);
}

/// Players not in 0-7 see nothing; `Sight::Fog` hides foreign slots.
fn state_sees_nothing_for_foreign_slots(sim: &mut Sim) {
    for p in [8, -1, 31, 1000, i32::MIN, i32::MAX] {
        for code in [0x4FF6, 0x500A] {
            assert_eq!(fog_hit(sim, code, p, 1, 1, 100), 0, "player {p}");
        }
    }
    // Under fog the foreign slot 2 is hidden, the own one (ME = 1) visible.
    let own = query_once(sim, ME, Sight::Fog, 0x500A, &[ME_I, 2, 2, 1, 10]).0[2];
    assert_eq!(own, 0);
    let omni = query_once(sim, ME, Sight::Omniscient, 0x500A, &[ME_I, 2, 2, 1, 10]).0[2];
    assert_eq!(omni, 1);
    sim.state.fog.seen.or(30, 30, 1 << ME);
    let mine = query_once(sim, ME, Sight::Fog, 0x500A, &[ME_I, ME_I, 30, 30, 0]).0[2];
    assert_eq!(mine, 1);
}

#[test]
fn fog_builtins_push_the_result_or_its_negation() {
    // 0xD7/0xD8 -> 0x500A push `[2]`; 0xD9/0xDA -> 0x4FF6 push `[2] == 0`; 0xD7/0xD9 take the player from the stack,
    // 0xD8/0xDA the own one.
    let (rules, mut state) = fog_world();
    state.fog.seen.or(10, 10, 1 << ME);
    state.fog.seen.or(20, 20, 1 << 3);
    state.fog.explored.or(30, 30, 1 << ME);
    let mut sim = Sim::new(&mut state, &rules);
    let me = ME_I;
    assert_eq!(run(&mut sim, 0xD7, &[me, 10, 10, 0]), [1]);
    assert_eq!(run(&mut sim, 0xD7, &[me, 11, 10, 0]), [0]);
    assert_eq!(run(&mut sim, 0xD7, &[me, 12, 10, 2]), [1]);
    assert_eq!(
        run(&mut sim, 0xD7, &[3, 20, 20, 0]),
        [1],
        "fog of another slot (E-25)"
    );
    assert_eq!(run(&mut sim, 0xD7, &[8, 20, 20, 5]), [0]);
    assert_eq!(run(&mut sim, 0xD8, &[10, 10, 0]), [1]);
    assert_eq!(run(&mut sim, 0xD8, &[20, 20, 0]), [0]);
    assert_eq!(run(&mut sim, 0xD8, &[8, 8, 3]), [1]);
    // Only explored (30, 30): "seen" no, "explored" yes, so "nothing explored" = 0.
    assert_eq!(run(&mut sim, 0xD8, &[30, 30, 0]), [0]);
    assert_eq!(run(&mut sim, 0xDA, &[30, 30, 0]), [0]);
    assert_eq!(run(&mut sim, 0xDA, &[10, 10, 0]), [0]);
    assert_eq!(run(&mut sim, 0xDA, &[40, 40, 3]), [1], "nothing explored");
    assert_eq!(run(&mut sim, 0xD9, &[me, 40, 40, 3]), [1]);
    assert_eq!(run(&mut sim, 0xD9, &[me, 10, 10, 1]), [0]);
    assert_eq!(
        run(&mut sim, 0xD9, &[3, 10, 10, 1]),
        [1],
        "slot 3 has explored nothing there"
    );
    assert_eq!(run(&mut sim, 0xD9, &[3, 20, 20, 1]), [0]);
    // Player not in 0-7: 0xD9 pushes 1 (nothing explored), 0xD7 pushes 0.
    for p in [8, -1, 99] {
        assert_eq!(run(&mut sim, 0xD9, &[p, 10, 10, 1]), [1], "player {p}");
        assert_eq!(run(&mut sim, 0xD7, &[p, 10, 10, 1]), [0], "player {p}");
    }
}

#[test]
fn path_rows_match_the_signatures() {
    // (ID, code, pops, push, outputs)
    type Outs = &'static [(u8, u8)];
    let table: [(u16, u32, u8, bool, Outs); 8] = [
        (0x87, 0x4E34, 4, false, &[(2, 5), (3, 6)]),
        (0x88, 0x4E35, 5, true, &[(3, 6), (4, 7)]),
        (0xB2, 0x4E35, 5, true, &[(3, 6), (4, 7)]),
        (0x1A1, 0x4FB0, 5, true, &[]),
        (0xD7, 0x500A, 4, true, &[]),
        (0xD8, 0x500A, 3, true, &[]),
        (0xD9, 0x4FF6, 4, true, &[]),
        (0xDA, 0x4FF6, 3, true, &[]),
    ];
    for (id, code, pops, push, outs) in table {
        assert!(is_query(id), "{id:#x}");
        let r = row(id).expect("row");
        assert_eq!(
            (r.code, r.pops, r.push, r.pause_ms, r.outs),
            (code, pops, push, 0, outs),
            "{id:#x}"
        );
        if let Some(sig) = signature(id) {
            assert_eq!(
                (sig.pops, sig.push, sig.pause_ms, sig.code, sig.outs.len()),
                (r.pops, r.push, r.pause_ms, r.code, outs.len()),
                "{id:#x}"
            );
        }
        // The handler pops exactly `pops` arguments and pushes exactly what the row says.
        let (rules, mut state) = scene();
        let mut sim = Sim::new(&mut state, &rules);
        let args = vec![0; usize::from(pops)];
        assert_eq!(run(&mut sim, id, &args).len(), usize::from(push), "{id:#x}");
    }
    // Every row with a path or fog code belongs in the table.
    for r in ROWS {
        if matches!(r.code, 0x4E34 | 0x4E35 | 0x4FB0 | 0x4FF6 | 0x500A) {
            assert!(table.iter().any(|&(id, ..)| id == r.lo), "{:#x}", r.lo);
        }
    }
    assert_eq!(LAYER_COUNT, 9);
}

/// Query codes covered here.
const CODES: [u32; 5] = [0x4E34, 0x4E35, 0x4FB0, 0x4FF6, 0x500A];

#[test]
fn path_queries_leave_the_checksum_unchanged() {
    let (rules, mut state) = walled(20);
    let mut ids = Vec::new();
    {
        let mut sim = Sim::new(&mut state, &rules);
        ids.push(put(&mut sim, WARRIOR, ME_I, (10, 10)));
        ids.push(put(&mut sim, WARRIOR, 0, (12, 10)));
        let dead = put(&mut sim, WARRIOR, ME_I, (11, 10));
        sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
        ids.push(dead);
        ids.push(put(&mut sim, HQ, ME_I, (30, 30)));
    }
    state.fog.seen.or(10, 10, 1 << ME);
    let (before, events) = (checksum(&state), state.events.clone());
    let mut sim = Sim::new(&mut state, &rules);
    let mut values = HOSTILE.to_vec();
    values.extend(ids);
    for code in CODES {
        for &a in &values {
            for &b in &values {
                let words = [ME_I, a, b, 10, b, a, b, a, 3];
                for len in [9, 7, 6, 5, 3, 1, 0] {
                    let _ = query_once(&mut sim, ME, Sight::Omniscient, code, &words[..len]);
                    let _ = query_once(&mut sim, ME, Sight::Fog, code, &words[..len]);
                }
                let _ = query_once(&mut sim, 0, Sight::Fog, code, &[a, b, a, b, a, b, a]);
            }
        }
    }
    // All builtins covered here with hostile arguments.
    for id in [0x87, 0x88, 0xB2, 0x1A1, 0xD7, 0xD8, 0xD9, 0xDA] {
        for &v in &values {
            let _ = run_with(&mut sim, id, &[v, v, v, v, v, v]);
            let _ = run_with(&mut sim, id, &[FLOOR, v, 3, v, 4, v]);
            let _ = run_with(&mut sim, id, &[]);
        }
    }
    assert_eq!(checksum(&state), before);
    assert_eq!(state.events, events);
}
