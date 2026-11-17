use super::goto::find_to_msg;
use super::*;
use crate::combat::testkit as ck;
use crate::command::{Command, CommandKind};
use crate::group::NO_GROUP;
use crate::messaging::{post_command, send_mut};
use crate::module::quarter::walk_behavior_msg;
use crate::rules::Rules;
use crate::state::SimState;
use crate::tick::step;
use crate::world::{ObjectDesc, create_object};

/// Colonist with walk (land 1x1, human 512/1024, turn 200, walking 3, running 4) and stand;
/// observer without modules.
const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME Beobachter\nVALUEID 100 6 1 0\n";

pub(super) fn rules() -> Rules {
    let mut r = Rules::synthetic(TYPES, 32);
    r.defaults.insert(1100, [(16, 183)].into_iter().collect());
    r
}

pub(super) fn tile_center(t: i32) -> i32 {
    (t << 12) + 0x800
}

/// Unit at the center of tile `(x, z)` and an observer that receives the feedback messages as carrier
/// (`0xA7F`): success 111, fault 222.
pub(super) fn scene(state: &mut SimState, rules: &Rules, tile: (i32, i32)) -> (ObjectId, ObjectId) {
    let mut sim = Sim::new(state, rules);
    let world = |t: i32| t as f32 * 128.0 + 64.0;
    let unit = create_object(
        &mut sim,
        &mut ObjectDesc::new(1100, 0, [world(tile.0), 0.0, world(tile.1)]),
    )
    .unwrap();
    let observer =
        create_object(&mut sim, &mut ObjectDesc::new(1000, 0, [10.0, 0.0, 10.0])).unwrap();
    (unit, observer)
}

/// FINDTO like the move command (`0x201`), with feedback messages to the observer.
pub(super) fn go(sim: &mut Sim, unit: ObjectId, observer: ObjectId, to: (i32, i32)) {
    let env = Envelope {
        msg: find_to_msg(to.0, to.1, -1, crate::motion::NO_DIRECTION, 0, false, 0x201),
        success: Some(Message::new(0xA7F, &[111])),
        fault: Some(Message::new(0xA7F, &[222])),
        success_target: observer,
        fault_target: observer,
    };
    post_command(sim, unit, env, true);
}

fn reply(state: &SimState, observer: ObjectId) -> u32 {
    state.object(observer, false).unwrap().carrier.0
}

#[test]
fn a_unit_walks_straight_to_its_goal_and_reports_success() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    let goal = (tile_center(12), tile_center(5));
    go(&mut Sim::new(&mut state, &rules), unit, observer, goal);
    let mut last = (tile_center(5), tile_center(5));
    let mut steps = 0;
    while reply(&state, observer) != 111 {
        step(&mut state, &rules, &[]);
        steps += 1;
        assert!(steps < 200, "no success");
        let o = state.object(unit, false).unwrap();
        let now = (o.grid.x.0, o.grid.z.0);
        // At most one step per object tick, on the straight line z = center.
        assert!(now.0 - last.0 <= 584 && now.1 == tile_center(5), "{now:?}");
        last = now;
        if o.status & status::MOVE != 0 {
            assert_eq!(o.anim.wanted, 3);
            assert_eq!(o.anim.speed, 182.0);
        }
    }
    let o = state.object(unit, false).unwrap();
    let w = walk(&Sim::new(&mut state.clone(), &rules), unit)
        .unwrap()
        .clone();
    // At the goal (goal radius a quarter tile), stopped, facing direction +x.
    assert!((goal.0 - o.grid.x.0).abs() <= 0x400);
    assert_eq!((w.state, o.status & status::MOVE, o.direction.0), (1, 0, 0));
    assert!(w.high_path.is_empty() && w.trajectory.is_empty());
    assert_eq!(steps, 101);
    // The frame update of the stand module saw the state change in the same tick (the
    // synthetic rules have no animation data, the idle choice is checked by the corpus test).
    assert!(!o.dirty);
}

#[test]
fn units_walk_around_walls_on_the_tile_path() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    for z in 2..9 {
        state.path.mark_cell(1, 8, z, true);
    }
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    let goal = (tile_center(12), tile_center(5));
    go(&mut Sim::new(&mut state, &rules), unit, observer, goal);
    let mut steps = 0;
    while reply(&state, observer) != 111 {
        step(&mut state, &rules, &[]);
        steps += 1;
        assert!(steps < 400, "no success");
        let t = state.object(unit, false).unwrap().tile;
        assert!(
            state.path.cell_cost(t.0, t.1, 0) == crate::path::grid::COST_FREE,
            "{t:?}"
        );
    }
    let o = state.object(unit, false).unwrap();
    assert!((goal.0 - o.grid.x.0).abs() <= 0x400 && (goal.1 - o.grid.z.0).abs() <= 0x400);
}

#[test]
fn halting_stops_on_the_current_point_and_reports_failure() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    go(
        &mut Sim::new(&mut state, &rules),
        unit,
        observer,
        (tile_center(20), tile_center(5)),
    );
    for _ in 0..10 {
        step(&mut state, &rules, &[]);
    }
    let before = state.object(unit, false).unwrap().grid;
    let halt = Command {
        player: 0,
        kind: CommandKind::Halt { units: vec![unit] },
    };
    step(&mut state, &rules, &[halt]);
    assert_eq!(reply(&state, observer), 222);
    for _ in 0..4 {
        step(&mut state, &rules, &[]);
    }
    let o = state.object(unit, false).unwrap();
    // No snapping: the fixed-point position stays.
    assert_eq!(o.grid, before);
    assert_eq!(o.status & (status::MOVE | WAITING | REEVALUATE), 0);
    assert_ne!(before.x.0 & 0xFFF, 0x800);
}

#[test]
fn a_goal_within_the_radius_succeeds_at_once() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    go(
        &mut Sim::new(&mut state, &rules),
        unit,
        observer,
        (tile_center(5) + 0x300, tile_center(5)),
    );
    // The command is executed in the next object tick of the unit.
    step(&mut state, &rules, &[]);
    step(&mut state, &rules, &[]);
    assert_eq!(reply(&state, observer), 111);
    let o = state.object(unit, false).unwrap();
    assert_eq!((o.grid.x.0, o.status & status::MOVE), (tile_center(5), 0));
}

/// Steps until the unit has sent a feedback message (111/222) to the observer and
/// stands; at most `limit`.
fn run_until_reply(state: &mut SimState, rules: &Rules, observer: ObjectId, limit: usize) -> usize {
    for steps in 1..=limit {
        step(state, rules, &[]);
        if matches!(reply(state, observer), 111 | 222) {
            return steps;
        }
    }
    panic!("no feedback message");
}

#[test]
fn a_blocked_start_tile_is_moved_to_the_nearest_free_tile() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    // A unit on a blocked tile is set to the center of the next free tile.
    state.path.mark_cell(1, 5, 5, true);
    let (unit, _) = scene(&mut state, &rules, (5, 5));
    let o = state.object(unit, false).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (tile_center(5), tile_center(4)));
}

#[test]
fn a_goal_beyond_the_map_edge_is_walked_to_at_the_edge_and_then_fails() {
    // re/walk 2.3/4.9: FINDTO clamps the goal (`ClampPosToMap`) and the unit walks to the
    // edge; `OnTargetReached` however checks the goal radius against the unclamped goal of the
    // stored message -> a repeat FINDTO (flag 4), then the fault.
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (25, 5));
    go(
        &mut Sim::new(&mut state, &rules),
        unit,
        observer,
        (tile_center(40), tile_center(5)),
    );
    run_until_reply(&mut state, &rules, observer, 400);
    assert_eq!(reply(&state, observer), 222);
    let o = state.object(unit, false).unwrap();
    // At the right edge of the map (last tile), not beyond it, in the row of the goal.
    assert!(
        (31 << 12..32 << 12).contains(&o.grid.x.0) && o.grid.z.0 == tile_center(5),
        "{:?}",
        o.grid
    );
    assert_eq!(o.status & status::MOVE, 0);
}

#[test]
fn a_goal_walled_in_is_replaced_and_the_unit_reports_failure_next_to_it() {
    // re/walk 9: if the goal lies in another region, the unit walks to the substitute goal in its
    // own region, reports no success there (outside the goal radius), repeats once
    // (flag 4) and then reports the fault.
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    for (x, z) in [(10, 3), (10, 5), (9, 4), (11, 4)] {
        state.path.mark_cell(1, x, z, true);
    }
    state.path.build_regions(u32::MAX);
    let (unit, observer) = scene(&mut state, &rules, (5, 4));
    let goal = (tile_center(10), tile_center(4));
    go(&mut Sim::new(&mut state, &rules), unit, observer, goal);
    let mut moved = false;
    let mut steps = 0;
    while !matches!(reply(&state, observer), 111 | 222) {
        step(&mut state, &rules, &[]);
        steps += 1;
        assert!(steps < 600, "no feedback message");
        let o = state.object(unit, false).unwrap();
        moved |= o.status & status::MOVE != 0;
        // The unit never enters a blocked tile.
        let t = o.tile;
        assert!(
            state.path.cell_cost(t.0, t.1, 0) == crate::path::grid::COST_FREE,
            "{t:?}"
        );
    }
    assert!(moved, "no path to the substitute goal");
    assert_eq!(reply(&state, observer), 222);
    let o = state.object(unit, false).unwrap();
    // Stands next to the wall: close to the goal, but at a distance of at least one tile (wall).
    let (dx, dz) = (o.grid.x.0 - goal.0, o.grid.z.0 - goal.1);
    let dist = crate::fx::fixed_hypot(dx, dz);
    assert!((0x1000..=3 * 0x1000).contains(&dist), "{dist:#x}");
    assert_eq!(o.status & status::MOVE, 0);
}

#[test]
fn a_goal_behind_a_wall_across_the_whole_map_ends_at_the_wall_with_a_failure() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    for z in 0..32 {
        state.path.mark_cell(1, 20, z, true);
    }
    state.path.build_regions(u32::MAX);
    let (unit, observer) = scene(&mut state, &rules, (5, 4));
    go(
        &mut Sim::new(&mut state, &rules),
        unit,
        observer,
        (tile_center(25), tile_center(4)),
    );
    run_until_reply(&mut state, &rules, observer, 800);
    assert_eq!(reply(&state, observer), 222);
    // The substitute goal is the reachable tile nearest to the wall (19, 4).
    let o = state.object(unit, false).unwrap();
    assert!(o.tile.0 == 19 && (o.tile.1 - 4).abs() <= 1, "{:?}", o.tile);
}

#[test]
fn a_unit_walled_in_reports_failure_and_stays() {
    // re/walk 9: if the unit is walled in, every goal lies in another region; there is
    // no path, the unit stays on its tile and reports the fault.
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    for dz in -1..=1 {
        for dx in -1..=1 {
            if (dx, dz) != (0, 0) {
                state.path.mark_cell(1, 5 + dx, 5 + dz, true);
            }
        }
    }
    state.path.build_regions(u32::MAX);
    go(
        &mut Sim::new(&mut state, &rules),
        unit,
        observer,
        (tile_center(15), tile_center(5)),
    );
    run_until_reply(&mut state, &rules, observer, 400);
    assert_eq!(reply(&state, observer), 222);
    let o = state.object(unit, false).unwrap();
    assert_eq!(o.tile, (5, 5));
    assert_eq!(o.status & status::MOVE, 0);
}

#[test]
fn blocked_trajectories_are_replanned_with_the_local_planner() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    go(
        &mut Sim::new(&mut state, &rules),
        unit,
        observer,
        (tile_center(20), tile_center(5)),
    );
    for _ in 0..6 {
        step(&mut state, &rules, &[]);
    }
    // An obstacle on the already projected trajectory.
    state.path.mark_cell(1, 10, 5, true);
    let mut planned = false;
    let mut steps = 0;
    while reply(&state, observer) != 111 {
        step(&mut state, &rules, &[]);
        steps += 1;
        assert!(steps < 400, "no success");
        let mut copy = state.clone();
        let sim = Sim::new(&mut copy, &rules);
        planned |= walk(&sim, unit).is_some_and(|w| w.state == 4);
        let t = state.object(unit, false).unwrap().tile;
        assert_ne!(t, (10, 5));
    }
    assert!(planned);
    let o = state.object(unit, false).unwrap();
    assert!((tile_center(20) - o.grid.x.0).abs() <= 0x400);
}

#[test]
fn units_inside_obstacles_escape_to_a_free_tile() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, _) = scene(&mut state, &rules, (5, 5));
    for x in 4..7 {
        for z in 4..7 {
            state.path.mark_cell(1, x, z, true);
        }
    }
    // Stand asks CHECKPOS every tick at rest: escape with the planner (mode 2).
    let mut steps = 0;
    loop {
        step(&mut state, &rules, &[]);
        steps += 1;
        assert!(steps < 200, "not freed");
        let o = state.object(unit, false).unwrap();
        let free = state.path.cell_cost(o.tile.0, o.tile.1, 0) == crate::path::grid::COST_FREE;
        if free && o.status & status::MOVE == 0 {
            break;
        }
    }
    let t = state.object(unit, false).unwrap().tile;
    assert!(!(4..7).contains(&t.0) || !(4..7).contains(&t.1), "{t:?}");
}

#[test]
fn slow_trajectories_keep_every_kth_point_and_prepend_the_last() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, _) = scene(&mut state, &rules, (5, 5));
    let mut sim = Sim::new(&mut state, &rules);
    for (n, want) in [(11, vec![11, 1, 4, 7, 10]), (7, vec![1, 4, 7])] {
        let w = walk_mut(&mut sim, unit).unwrap();
        w.speed.set_default_speed(30);
        w.trajectory = (1..=n)
            .map(|x| crate::motion::TrajPoint {
                x,
                ..Default::default()
            })
            .collect();
        super::plan::thin_slow_trajectory(&mut sim, unit);
        let w = walk(&sim, unit).unwrap();
        assert_eq!(w.trajectory.iter().map(|p| p.x).collect::<Vec<_>>(), want);
        assert_eq!(w.speed.cur, 30);
    }
}

#[test]
fn pair_maps_iterate_by_bucket_and_newest_first() {
    let mut m = PairMap::default();
    // Bucket (lo >> 4) mod 17: 0x00010001 -> 16, 0x00020001 -> 15, 0x10 -> 1, 0x21 -> 2.
    for key in [
        (0x0001_0001, 9),
        (0x0002_0001, 9),
        (0x10, 3),
        (0x21, 4),
        (0x10, 5),
        (0x10, 3),
    ] {
        m.insert(key);
    }
    assert_eq!(
        m.keys(),
        [
            (0x10, 5),
            (0x10, 3),
            (0x21, 4),
            (0x0002_0001, 9),
            (0x0001_0001, 9)
        ]
    );
    assert_eq!(m.len(), 5);
    m.clear();
    assert!(m.is_empty());
}

/// Two units from `a` to `b` and vice versa (same row); steps until both report success
/// and whether one of them waited or replanned.
fn swap(rules: &Rules, a: (i32, i32), b: (i32, i32), stand_still: bool) -> (usize, bool, SimState) {
    let mut state = SimState::new(0, 0);
    state.init_map(rules);
    let (u1, o1) = scene(&mut state, rules, a);
    let (u2, o2) = scene(&mut state, rules, b);
    {
        let mut sim = Sim::new(&mut state, rules);
        go(&mut sim, u1, o1, (tile_center(b.0 + 2), tile_center(b.1)));
        if !stand_still {
            go(&mut sim, u2, o2, (tile_center(a.0), tile_center(a.1)));
        }
    }
    let mut handled = false;
    let mut steps = 0;
    while reply(&state, o1) != 111 || (!stand_still && reply(&state, o2) != 111) {
        step(&mut state, rules, &[]);
        steps += 1;
        assert!(steps < 1200, "no arrival");
        let mut copy = state.clone();
        let sim = Sim::new(&mut copy, rules);
        handled |= [u1, u2]
            .into_iter()
            .any(|u| walk(&sim, u).is_some_and(|w| matches!(w.state, 3 | 4 | 8)));
    }
    (steps, handled, state)
}

#[test]
fn units_meeting_head_on_resolve_their_collision() {
    let rules = rules();
    let (steps, handled, state) = swap(&rules, (5, 5), (15, 5), false);
    // Pinned value (without oncoming traffic about 140 steps).
    assert_eq!(steps, 179);
    assert!(handled);
    assert!(state.collision_records.is_consistent());
    assert_no_unit_overlap(&state);
}

#[test]
fn walkers_push_idle_units_aside_or_go_around() {
    let rules = rules();
    let (steps, handled, state) = swap(&rules, (5, 5), (10, 5), true);
    // Pinned value (without the standing one about 100 steps).
    assert_eq!(steps, 107);
    assert!(handled);
    assert_no_unit_overlap(&state);
}

fn move_cmd(player: u8, unit: ObjectId, tile: (i32, i32), queue: Option<bool>) -> Command {
    let kind = CommandKind::Move {
        units: vec![unit],
        x: tile_center(tile.0),
        z: tile_center(tile.1),
        target: -1,
        formation: 0,
        direction: crate::motion::NO_DIRECTION,
        spread: crate::command::NO_SPREAD,
        flags: 0,
    };
    Command {
        player,
        kind: match queue {
            Some(cyclic) => CommandKind::Queue {
                cyclic,
                inner: Box::new(kind),
            },
            None => kind,
        },
    }
}

#[test]
fn move_commands_walk_and_queued_moves_follow_one_after_another() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, _) = scene(&mut state, &rules, (5, 5));
    // Foreign player: rejected (exception A8).
    step(&mut state, &rules, &[move_cmd(1, unit, (9, 5), None)]);
    step(&mut state, &rules, &[]);
    assert_eq!(state.object(unit, false).unwrap().status & status::MOVE, 0);
    // Two queued goals: the second not until after arrival at the first.
    step(
        &mut state,
        &rules,
        &[
            move_cmd(0, unit, (9, 5), Some(false)),
            move_cmd(0, unit, (9, 9), Some(false)),
        ],
    );
    // The first entry is already executed in the same object tick (the unit was at rest).
    assert_eq!(state.object(unit, false).unwrap().commands.len(), 1);
    let mut corners = Vec::new();
    for _ in 0..400 {
        step(&mut state, &rules, &[]);
        let o = state.object(unit, false).unwrap();
        if o.status & status::MOVE == 0 && corners.last() != Some(&o.tile) {
            corners.push(o.tile);
        }
    }
    // Rest points: the start (until the FINDTO runs from the FIFO), then both goals.
    assert_eq!(corners, [(5, 5), (9, 5), (9, 9)]);
    assert!(state.object(unit, false).unwrap().commands.is_empty());
    // A direct command empties the queue.
    step(&mut state, &rules, &[move_cmd(0, unit, (2, 2), Some(true))]);
    step(&mut state, &rules, &[move_cmd(0, unit, (12, 9), None)]);
    assert!(state.object(unit, false).unwrap().commands.is_empty());
}

/// Event bit 1 of the unit (scripts).
fn idle_bit(state: &SimState, id: ObjectId) -> bool {
    state.object_event_bits.get(&id).is_some_and(|w| w & 2 != 0)
}

// QUIRK E-148: kept
#[test]
fn a_move_command_sets_bit_1_on_arrival_and_on_abort() {
    // The walk command `0x04` (`move_unit`, FINDTO with `0x97F` as success and fault like
    // `GameWorld_CmdMoveSelection` `0x0047CB27`) sets bit 1 via `RaiseIdleEvent` ->
    // `0x37F`: on arrival (success) and on every abort of walking (fault), because
    // `PropertyWalk_Abort` (`0x00535120`) sends the fault message - by a new direct
    // walk command (abort when executing the envelope) and by halt `0x3C` (`GameWorld_CmdHalt`
    // `0x004770A0` -> `AbortAllModules`). While walking the bit stays off.
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (a, _) = scene(&mut state, &rules, (5, 5));
    let (b, _) = scene(&mut state, &rules, (5, 12));
    let (c, _) = scene(&mut state, &rules, (5, 20));
    let moving = |state: &SimState, id| state.object(id, false).unwrap().status & status::MOVE != 0;
    step(
        &mut state,
        &rules,
        &[
            move_cmd(0, a, (9, 5), None),
            move_cmd(0, b, (28, 12), None),
            move_cmd(0, c, (28, 20), None),
        ],
    );
    for _ in 0..12 {
        step(&mut state, &rules, &[]);
        for u in [a, b, c] {
            assert!(
                moving(&state, u) && !idle_bit(&state, u),
                "{u:?} walks, bit 1 off"
            );
        }
    }
    // Abort: b gets a new direct walk command, c halt.
    let halt = Command {
        player: 0,
        kind: CommandKind::Halt { units: vec![c] },
    };
    step(&mut state, &rules, &[move_cmd(0, b, (28, 14), None), halt]);
    assert!(
        !moving(&state, c) && idle_bit(&state, c),
        "halt: bit 1 immediately"
    );
    step(&mut state, &rules, &[]);
    assert!(
        idle_bit(&state, b),
        "new walk command: bit 1 from the abort"
    );
    assert!(moving(&state, b), "b walks on to the new goal");
    // Arrival: a arrives (success); the bit stays set until a script clears it.
    let mut n = 0;
    while moving(&state, a) {
        step(&mut state, &rules, &[]);
        n += 1;
        assert!(n < 200, "a does not arrive");
    }
    step(&mut state, &rules, &[]);
    assert_eq!(state.object(a, false).unwrap().tile, (9, 5));
    assert!(idle_bit(&state, a), "arrival: bit 1");
    // Arrival after the bit was cleared sets it again (success of the second command of b).
    state.object_event_bits.insert(b, 0);
    let mut n = 0;
    while moving(&state, b) {
        step(&mut state, &rules, &[]);
        n += 1;
        assert!(n < 600, "b does not arrive");
    }
    step(&mut state, &rules, &[]);
    assert_eq!(state.object(b, false).unwrap().tile, (28, 14));
    assert!(idle_bit(&state, b), "arrival after the abort: bit 1 again");
}

/// Checksum over the checksum sequence of [`walking_checksums`].
/// Changed: walk combat fields and behavior default Defensive (home follows, tether sight*1024);
/// positions, status and animation of the objects at the end the same as before.
/// Changed: new state fields (building map, projectiles, hit zone counter), behavior unchanged.
/// Changed: new state fields groups/walk (FORMAT_VERSION 8), behavior unchanged.
/// Changed: new state fields (event bits, last track, meeting point), behavior unchanged.
/// Changed: new state fields (upgrades, counter-upgrades, temporary upgrades).
/// Changed: time-of-day upgrades at level start.
/// Changed: new state fields (gate guard at the building, area queries), behavior unchanged
/// (counter-check with `serde(skip)`).
/// Changed: world field `headhunters` (serialization, behavior the same).
/// Changed: state field `ki` (AI state, serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: state fields `ai_marks`, `attack_reports` and `campaign_difficulty` (serialization,
/// behavior the same; counter-check with `serde(skip)`).
/// Changed: state fields `script`, `names` and evocation counter (serialization, behavior the
/// same; counter-check with `serde(skip)`).
/// Changed: event bits 1, 2, 3, 5, 7, kill counter, message sequence `0x2100`/`0x611` and new
/// world fields (counter-check: without setter and with `serde(skip)` the values of the preceding step).
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: serialization of `accounting` and `countdowns` added to the value of the preceding
/// step (counter-check: with `serde(skip)` on both, the values of the preceding step).
/// Changed: serialization of new fields (presentation state, mirror); behavior the same
/// (counter-check with `serde(skip)`).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26);
/// behavior the same (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and
/// `time_limit`, FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all values
/// unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same;
/// counter-check with `serde(skip)`: all values unchanged).
const WALK_GOLDEN: u64 = 0xdec2_2b9e_bda7_72c6;

/// Three units: two meet head-on, the third stands in the way; 240 ticks.
fn walking_checksums() -> u64 {
    let rules = rules();
    let mut state = crate::setup::new_match(
        &rules,
        &crate::setup::MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: crate::ki::KiSetup::default(),
            script: crate::script::ScriptSetup::default(),
            ..crate::setup::MatchSetup::default()
        },
    )
    .unwrap();
    let (a, _) = scene(&mut state, &rules, (4, 6));
    let (b, _) = scene(&mut state, &rules, (14, 6));
    scene(&mut state, &rules, (9, 6));
    let mut h = crate::rules::Fnv64::default();
    for i in 0..240 {
        let commands = if i == 0 {
            vec![move_cmd(0, a, (16, 6), None), move_cmd(0, b, (2, 6), None)]
        } else {
            Vec::new()
        };
        step(&mut state, &rules, &commands);
        h.write(&crate::save::checksum(&state).to_le_bytes());
    }
    h.0
}

#[test]
fn the_walking_checksum_sequence_is_fixed_in_debug_and_release() {
    assert_eq!(
        walking_checksums(),
        WALK_GOLDEN,
        "{:#x}",
        walking_checksums()
    );
}

#[test]
fn ranks_are_read_from_10011_and_10012() {
    // Type text like `TYPES` (colonist 1100), additionally VALUEID 10011 = 4 and 10012 = 1.
    let types = TYPES.replace(
        "VALUEID 10008 6 1 4
",
        "VALUEID 10008 6 1 4
VALUEID 10011 6 1 4
VALUEID 10012 6 1 1
",
    );
    assert_ne!(types, TYPES, "anchor line of the walk parameters not found");
    let rules = Rules::synthetic(&types, 32);
    let mut state = SimState::new(-1, 0);
    let (unit, _) = scene(&mut state, &rules, (4, 4));
    let sim = Sim::new(&mut state, &rules);
    let w = walk(&sim, unit).unwrap();
    assert_eq!(
        (w.formation_num, w.formation_num_mounted, w.group),
        (4, 1, NO_GROUP)
    );
    assert_eq!(w.ticks_since_chase, -1);
}

#[test]
fn frame_updates_only_reach_modules_of_the_tick_list() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, _) = scene(&mut state, &rules, (5, 5));
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(sim.object(unit).unwrap().tick_list, [0, 2]);
    // Without stand in the tick list the frame update stays off (`0x00470BD0`).
    sim.object_mut(unit).unwrap().tick_list.retain(|&n| n != 2);
    sim.object_mut(unit).unwrap().dirty = true;
    crate::module::frame_update(&mut sim, unit);
    assert!(sim.object(unit).unwrap().dirty);
    // With stand in the tick list it is served and the change is processed.
    sim.object_mut(unit).unwrap().tick_list.push(2);
    crate::module::frame_update(&mut sim, unit);
    assert!(!sim.object(unit).unwrap().dirty);
}

/// No two units (objects with a walk module) overlap hard: distance >= sum of the hard radii.
pub(super) fn assert_no_unit_overlap(state: &SimState) {
    let units: Vec<_> = state
        .objects()
        .filter(|o| matches!(o.module(0).map(|m| &m.kind), Some(ModuleKind::Walk(_))))
        .collect();
    assert!(units.len() >= 2);
    for (i, a) in units.iter().enumerate() {
        for b in &units[i + 1..] {
            let d = crate::fx::fixed_hypot(
                a.grid.x.0.wrapping_sub(b.grid.x.0),
                a.grid.z.0.wrapping_sub(b.grid.z.0),
            );
            assert!(
                d >= a.hard_radius + b.hard_radius,
                "{:?} and {:?} overlap: distance {d:#x} < {:#x}",
                a.id,
                b.id,
                a.hard_radius + b.hard_radius
            );
        }
    }
}

#[test]
fn a_direct_move_clears_the_queue_even_when_the_unit_cannot_walk() {
    // re/walk 15.1: first empty the queue, then check for module 0. The observer has no
    // walk module; a command from the queue (not direct) leaves it standing.
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    let mut sim = Sim::new(&mut state, &rules);
    let entry = crate::command::QueuedCommand {
        cyclic: false,
        command: move_cmd(0, observer, (9, 9), None),
    };
    sim.object_mut(observer).unwrap().commands = vec![entry.clone(), entry];
    assert!(!sim.object(observer).unwrap().has_module(0, false));
    crate::command::execute(&mut sim, &move_cmd(0, observer, (9, 5), None), false);
    assert_eq!(sim.object(observer).unwrap().commands.len(), 2);
    crate::command::execute(&mut sim, &move_cmd(0, observer, (9, 5), None), true);
    assert!(sim.object(observer).unwrap().commands.is_empty());
    // Without a walk module there is no movement and no command in the FIFO.
    assert!(sim.object(observer).unwrap().fifo.is_empty());
    // A unit with walk still gets its FINDTO.
    crate::command::execute(&mut sim, &move_cmd(0, unit, (9, 5), None), true);
    assert!(!sim.object(unit).unwrap().fifo.is_empty());
}

#[test]
fn several_units_sent_to_the_same_point_all_come_to_rest_without_hard_overlap() {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let units: Vec<ObjectId> = [(5, 3), (5, 4), (5, 5), (5, 6), (4, 4), (4, 6)]
        .into_iter()
        .map(|t| scene(&mut state, &rules, t).0)
        .collect();
    let kind = CommandKind::Move {
        units: units.clone(),
        x: tile_center(14),
        z: tile_center(5),
        target: -1,
        formation: 0,
        direction: crate::motion::NO_DIRECTION,
        spread: crate::command::NO_SPREAD,
        flags: 0,
    };
    step(&mut state, &rules, &[Command { player: 0, kind }]);
    let mut rest = 0;
    for _ in 0..1500 {
        step(&mut state, &rules, &[]);
        let walking = units
            .iter()
            .filter(|&&u| state.object(u, false).unwrap().status & (status::MOVE | WAITING) != 0)
            .count();
        rest = if walking == 0 { rest + 1 } else { 0 };
        if rest >= 20 {
            break;
        }
    }
    assert!(rest >= 20, "not all units came to rest");
    assert!(state.collision_records.is_consistent());
    assert_no_unit_overlap(&state);
    // All stand near the goal (jostling: a few tiles of leeway).
    for &u in &units {
        let o = state.object(u, false).unwrap();
        assert!(
            (o.grid.x.0 - tile_center(14)).abs() <= 4 * 0x1000
                && (o.grid.z.0 - tile_center(5)).abs() <= 4 * 0x1000,
            "{:?}",
            o.grid
        );
    }
}

#[test]
fn the_constructor_applies_the_player_behaviour_at_the_placement_position() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    state.combat.behavior_mode[1] = 0;
    state.combat.behavior_mode[2] = 4;
    let mut sim = Sim::new(&mut state, &rules);
    // Slot 0 (default 2 "Defensive"): MobileHome, home = position, tether = sight*1024.
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
    let w = walk(&sim, a).unwrap();
    let pos = position(&sim, a);
    assert!(w.try_to_follow && w.mobile_home);
    assert_eq!((w.home.0, w.home.1, w.home.2), (pos.0, pos.1, NO_DIRECTION));
    assert_eq!(w.tether, 32 << 10);
    assert_eq!(
        (w.quarry, w.min_attack_radius, w.max_attack_radius),
        (ObjectId::NONE, -1, -1)
    );
    assert_eq!(
        w.attackers_before_surrounded,
        (512 << 2) / 0x200,
        "(R << 2) / 0x200"
    );
    // Slot 1, mode 0: no home, no tether.
    let b = ck::spawn(&mut sim, ck::SABRE, 1, (9, 9));
    let w = walk(&sim, b).unwrap();
    assert!(!w.mobile_home && w.home.0 == -1 && w.tether == -1);
    // Slot 2, mode 4: tether Fixed(8).
    let c = ck::spawn(&mut sim, ck::SABRE, 2, (12, 12));
    assert_eq!(walk(&sim, c).unwrap().tether, 8 << 12);
}

// QUIRK K-201, C-202: kept
#[test]
fn behaviour_messages_apply_back_up_and_restore() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
    let before = walk(&sim, a).unwrap().behaviour();
    // Mode 1 "Hold position" with backup: tether 0, home = position.
    assert_eq!(send(&mut sim, a, &walk_behavior_msg(1, true)), 0);
    let w = walk(&sim, a).unwrap();
    assert_eq!((w.tether, w.mobile_home), (0, true));
    assert_eq!(w.backup_behaviour, before, "backup before overwriting");
    assert_eq!(send(&mut sim, a, &Message::new(0x2200, &[])), 0);
    let mut home = Message::new(0x3300, &[0, 0]);
    assert_eq!(send_mut(&mut sim, a, &mut home), 1);
    assert_eq!((home.arg(0), home.arg(1)), position(&sim, a));
    // 0x2700 = MobileHome && home set (corrected, re/combat-attack 7a.11).
    assert_eq!(send(&mut sim, a, &Message::new(0x2700, &[])), 1);
    // 0x3D00 restores the backup.
    assert_eq!(send(&mut sim, a, &Message::new(0x3D00, &[])), 0);
    assert_eq!(walk(&sim, a).unwrap().behaviour(), before);
    // 0x2100 {} restores the constructor default (+0x19C), return-home counter -1.
    send(&mut sim, a, &Message::new(0x2100, &[]));
    let w = walk(&sim, a).unwrap();
    assert_eq!((w.home.0, w.tether, w.home_timer), (-1, -1, -1));
    // 0x2500 {x, z} sets the home, direction/tether stay.
    assert_eq!(send(&mut sim, a, &Message::new(0x2500, &[4096, 8192])), 1);
    assert_eq!(
        (walk(&sim, a).unwrap().home.0, walk(&sim, a).unwrap().home.1),
        (4096, 8192)
    );
    // 0x3F00 {old, new}: replace quarry and success HID (not the fault HID).
    assert_eq!(
        send(&mut sim, a, &Message::new(0x3F00, &[7, 9])),
        0,
        "without quarry 7 nothing"
    );
    let (old, new, fault) = (
        ObjectId::from_i32(0x10_0001),
        ObjectId::from_i32(0x20_0002),
        ObjectId::from_i32(0x30_0003),
    );
    {
        let m = sim.module_mut(a, INDEX).unwrap();
        m.base.success_target = old;
        m.base.fault_target = old;
        let ModuleKind::Walk(w) = &mut m.kind else {
            panic!("walk module")
        };
        w.quarry = old;
    }
    let swap = Message::new(0x3F00, &[old.as_i32(), new.as_i32()]);
    assert_eq!(send(&mut sim, a, &swap), 1, "quarry matches");
    let m = sim.module(a, INDEX).unwrap();
    assert_eq!(m.base.success_target, new, "success HID replaced");
    assert_eq!(m.base.fault_target, old, "fault HID stays");
    assert_eq!(walk(&sim, a).unwrap().quarry, new, "quarry replaced");
    // If the old ID does not match the quarry, everything stays; the success HID is replaced
    // anyway if it matches.
    sim.module_mut(a, INDEX).unwrap().base.success_target = fault;
    let again = Message::new(0x3F00, &[old.as_i32(), ObjectId::NONE.as_i32()]);
    assert_eq!(send(&mut sim, a, &again), 0, "quarry is now new");
    assert_eq!(walk(&sim, a).unwrap().quarry, new);
    assert_eq!(sim.module(a, INDEX).unwrap().base.success_target, fault);
}

#[test]
fn a_mobile_home_follows_every_step_but_a_tethered_one_stays() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, start) = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
        (a, position(&sim, a))
    };
    step(&mut state, &rules, &[move_cmd(0, a, (15, 5), None)]);
    for _ in 0..80 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    let w = walk(&sim, a).unwrap();
    assert_ne!(position(&sim, a), start, "the unit has walked");
    assert_eq!(
        (w.home.0, w.home.1),
        position(&sim, a),
        "home follows in StepAlongPath"
    );
}

/// World position for fixed point `fx` (tile = 4096 fixed point = 128 world units).
fn wpos(fx: i32) -> f32 {
    fx as f32 / 32.0
}

/// Two saber fighters (slot 0 and 1) at fixed-point positions; observer for the feedback messages.
fn duel(
    state: &mut SimState,
    rules: &Rules,
    a: (i32, i32),
    t: (i32, i32),
) -> (ObjectId, ObjectId, ObjectId) {
    let mut sim = Sim::new(state, rules);
    let mk = |sim: &mut Sim, slot, p: (i32, i32)| {
        create_object(
            sim,
            &mut ObjectDesc::new(ck::SABRE, slot, [wpos(p.0), 0.0, wpos(p.1)]),
        )
        .unwrap()
    };
    let attacker = mk(&mut sim, 0, a);
    let target = mk(&mut sim, 1, t);
    let observer = create_object(
        &mut sim,
        &mut ObjectDesc::new(ck::OBSERVER, 0, [10.0, 0.0, 10.0]),
    )
    .unwrap();
    (attacker, target, observer)
}

/// FINDTO_UNIT like `PropertyAttack_StartAttack` in melee: `[target, 0, 2048, 0, −1, 0x40, 0, 0]`.
fn attack_unit(sim: &mut Sim, unit: ObjectId, target: ObjectId, observer: ObjectId, flags: i32) {
    let env = Envelope {
        msg: Message::new(0x400, &[target.as_i32(), 0, 2048, 0, -1, flags, 0, 0]),
        success: Some(Message::new(0xA7F, &[111])),
        fault: Some(Message::new(0xA7F, &[222])),
        success_target: observer,
        fault_target: observer,
    };
    crate::messaging::dispatch_command(sim, unit, &env);
}

#[test]
fn find_to_unit_in_range_and_facing_succeeds_at_once() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    // Centers 3072 apart -> edge distance 3072 - 2*512 = 2048 = max_r.
    let base = tile_center(5);
    let (a, t, obs) = duel(&mut state, &rules, (base, base), (base + 3072, base));
    let mut sim = Sim::new(&mut state, &rules);
    sim.object_mut(a).unwrap().direction = crate::fx::Fx(0); // view +x = direction to the target
    attack_unit(&mut sim, a, t, obs, 0x40);
    assert_eq!(
        reply(sim.state, obs),
        111,
        "Already close enough -> success immediately"
    );
    let w = walk(&sim, a).unwrap();
    assert_eq!(
        (w.quarry, w.min_attack_radius, w.max_goal_radius),
        (ObjectId::NONE, -1, -1)
    );
    assert!(w.attack, "attack flag stays from SetSeekTarget");
}

// QUIRK C-189: kept
#[test]
fn find_to_unit_turns_first_when_not_facing_and_then_catches() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let base = tile_center(5);
    let (a, t, obs) = duel(&mut state, &rules, (base, base), (base + 3072, base));
    {
        let mut sim = Sim::new(&mut state, &rules);
        sim.object_mut(a).unwrap().direction = crate::fx::Fx(90 << 12);
        attack_unit(&mut sim, a, t, obs, 0x40);
        assert_eq!(
            reply(sim.state, obs),
            ObjectId::NONE.0,
            "no feedback message yet"
        );
        assert_eq!(
            walk(&sim, a).unwrap().quarry,
            t,
            "quarry stays while turning"
        );
    }
    let mut n = 0;
    while reply(&state, obs) != 111 {
        step(&mut state, &rules, &[]);
        n += 1;
        assert!(n < 20, "no Caught");
    }
    let o = state.object(a, false).unwrap();
    assert!(
        crate::world::wrap_angle(o.direction.0).wrapping_abs() <= 5 << 12,
        "turned to the target"
    );
}

// QUIRK C-193: kept
#[test]
fn find_to_unit_out_of_range_walks_up_and_catches() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, t, obs) = duel(
        &mut state,
        &rules,
        (tile_center(5), tile_center(5)),
        (tile_center(11), tile_center(5)),
    );
    attack_unit(
        &mut Sim::new(&mut state, &rules),
        a,
        t,
        obs,
        0x40 | 0x02 | 0x20,
    );
    let mut n = 0;
    while reply(&state, obs) != 111 {
        step(&mut state, &rules, &[]);
        n += 1;
        assert!(n < 300, "did not arrive");
        assert_ne!(reply(&state, obs), 222, "no abort");
    }
    let mut sim = Sim::new(&mut state, &rules);
    let d = crate::collision::edge_distance_to_object(&mut sim, t, a);
    assert!((0..=2048).contains(&d), "edge distance {d}");
}

// QUIRK C-15: kept
#[test]
fn seeking_a_vanished_quarry_aborts_with_0x311() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, t, obs) = duel(
        &mut state,
        &rules,
        (tile_center(5), tile_center(5)),
        (tile_center(14), tile_center(5)),
    );
    attack_unit(&mut Sim::new(&mut state, &rules), a, t, obs, 0x40);
    for _ in 0..4 {
        step(&mut state, &rules, &[]);
    }
    crate::world::queue_destroy(&mut Sim::new(&mut state, &rules), t);
    for _ in 0..4 {
        step(&mut state, &rules, &[]);
    }
    assert_eq!(
        reply(&state, obs),
        222,
        "\"no longer valid\" -> fault message"
    );
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(walk(&sim, a).unwrap().quarry, ObjectId::NONE);
}

#[test]
fn the_tether_rejects_targets_too_far_from_home() {
    // re/combat-attack 7a.14: tether 32,768, edge distance target-home 40,000 -> 39,488 - 32,768 > 2048.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let home = (tile_center(5), tile_center(5));
    let (a, t, obs) = duel(&mut state, &rules, home, (home.0 + 40_000 + 512, home.1));
    let mut sim = Sim::new(&mut state, &rules);
    {
        let w = walk_mut(&mut sim, a).unwrap();
        w.home = (home.0, home.1, NO_DIRECTION);
        w.tether = 8 << 12;
        w.mobile_home = true; // 0x02 in the order sets MobileHome = 0 (ApplyMoveFlags)
    }
    attack_unit(&mut sim, a, t, obs, 0x40 | 0x02);
    assert_eq!(reply(sim.state, obs), 222);
}

// QUIRK C-185: kept
#[test]
fn a_target_in_fog_and_in_range_is_not_attacked() {
    // C-185: position search with CHECKFOG fails in range -> abort.
    let rules = ck::rules();
    let mut state = crate::command::testkit::fogged_state(&rules);
    let base = tile_center(5);
    let (a, t, obs) = duel(&mut state, &rules, (base, base), (base + 3072, base));
    // Without a fog cycle nothing is seen; slot 0 does not see the target.
    attack_unit(&mut Sim::new(&mut state, &rules), a, t, obs, 0x40);
    assert_eq!(reply(&state, obs), 222);
}

// QUIRK C-191, C-197: kept
#[test]
fn the_attack_filter_widens_narrow_bands_and_push_out_uses_one_and_a_half() {
    // re/combat-attack 7a.14.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, t, _) = duel(
        &mut state,
        &rules,
        (tile_center(5), tile_center(5)),
        (tile_center(9), tile_center(5)),
    );
    let mut sim = Sim::new(&mut state, &rules);
    {
        let w = walk_mut(&mut sim, a).unwrap();
        w.min_attack_radius = 0;
        w.max_attack_radius = 1024;
        w.attack = true;
    }
    let f = super::seek::attack_filter(&mut sim, a, t);
    assert_eq!((f.mode, f.range_min, f.range_max), (0x200_0030, 0, 2897));
    assert_eq!(
        (f.home, f.tether),
        ((-1, -1), -1),
        "MobileHome: without home/tether"
    );
    {
        let w = walk_mut(&mut sim, a).unwrap();
        w.min_attack_radius = 4096;
        w.max_attack_radius = 28672;
    }
    let f = super::seek::attack_filter(&mut sim, a, t);
    assert_eq!((f.range_min, f.range_max), (5120, 29696));
    // Push-out: desired = 0 + 512 + 512 -> 1.5 * 1024 = 1536 around p, direction target -> p.
    walk_mut(&mut sim, a).unwrap().quarry = t;
    let p = (tile_center(8), tile_center(5)); // between a and t
    let q = super::seek::push_out_point(&mut sim, a, p, 1024);
    assert_eq!(
        q,
        (p.0 - 1536, p.1),
        "from t (x = 9) beyond p in direction -x"
    );
}

#[test]
fn find_to_unit_and_the_attack_filter_wrap_extreme_radii_like_the_original() {
    // The original calculates message arguments with 32-bit wraparound; the reimplementation must not abort.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let base = tile_center(5);
    let (a, t, obs) = duel(&mut state, &rules, (base, base), (base + 3072, base));
    let mut sim = Sim::new(&mut state, &rules);
    let env = Envelope {
        msg: Message::new(0x400, &[t.as_i32(), i32::MAX, 0, 0, -1, 0x40, 0, 0]),
        success: Some(Message::new(0xA7F, &[111])),
        fault: Some(Message::new(0xA7F, &[222])),
        success_target: obs,
        fault_target: obs,
    };
    crate::messaging::dispatch_command(&mut sim, a, &env);
    {
        let w = walk_mut(&mut sim, a).unwrap();
        // `max(min + 1024, max)` with a wrapping sum.
        w.min_attack_radius = i32::MAX;
        w.max_attack_radius = i32::MAX;
        w.attack = true;
    }
    let f = super::seek::attack_filter(&mut sim, a, t);
    let wrapped = i32::MAX.wrapping_add(512 + 512);
    assert_eq!(
        (f.range_min, f.range_max),
        (
            wrapped,
            wrapped.wrapping_add(super::seek::MIN_FILTER_BAND + 1)
        )
    );
}

// QUIRK C-198: kept
#[test]
fn the_surround_check_answers_free_and_state_0xc_ticks_every_17() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, t, _) = duel(
        &mut state,
        &rules,
        (tile_center(5), tile_center(5)),
        (tile_center(9), tile_center(5)),
    );
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(
        send(&mut sim, t, &Message::new(0x3100, &[a.as_i32()])),
        0,
        "points free"
    );
    assert_eq!(
        send(&mut sim, t, &Message::new(0x3100, &[-1])),
        1,
        "surrounded without R"
    );
    // State 0xC (only from savegames): after 17 ticks not surrounded, stands -> abort.
    {
        let w = walk_mut(&mut sim, a).unwrap();
        w.quarry = t;
        w.state = 0xC;
        w.wait_counter = 0;
    }
    for _ in 0..16 {
        super::seek::tick_quarry_surrounded(&mut sim, a);
    }
    assert_eq!(walk(&sim, a).unwrap().state, 0xC);
    super::seek::tick_quarry_surrounded(&mut sim, a);
    assert_eq!(
        walk(&sim, a).unwrap().state,
        1,
        "Abort -> StopMoving -> state 1"
    );
}

// QUIRK K-200: kept
#[test]
fn a_quarry_in_water_disables_swimming_and_stop_moving_restores_the_layer() {
    // re/combat-attack 7a.2, re/walk 16.1: land-and-water layer (0x29), target swims -> swimming
    // off for combat (layer 0x09); `StopMoving(0)` restores the layer, the flag stays.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, t, obs) = duel(
        &mut state,
        &rules,
        (tile_center(5), tile_center(5)),
        (tile_center(9), tile_center(5)),
    );
    let mut sim = Sim::new(&mut state, &rules);
    walk_mut(&mut sim, a).unwrap().layer = 1;
    sim.object_mut(t).unwrap().status |= status::SWIMMING;
    attack_unit(&mut sim, a, t, obs, 0x40);
    let w = walk(&sim, a).unwrap();
    assert_eq!(
        (w.swim_disallowed_for_combat, w.layer, w.quarry),
        (true, 0, t)
    );
    // Halt: `0xC00` without argument = Abort -> StopMoving(0).
    send(&mut sim, a, &Message::new(0xC00, &[]));
    let w = walk(&sim, a).unwrap();
    assert_eq!(
        (w.swim_disallowed_for_combat, w.layer, w.quarry),
        (true, 1, ObjectId::NONE),
        "layer back, flag stays (quirk)"
    );
}

#[test]
fn a_rifleman_does_not_shoot_through_a_house_but_an_archer_does() {
    use crate::combat::testkit::{self as ck, ARCHER, ENG_INFANTRY, WARRIOR};
    use crate::command::testkit::HOUSE;
    for (shooter, house, fights_at_once) in [
        (ENG_INFANTRY, true, false),
        (ENG_INFANTRY, false, true),
        (ARCHER, true, true),
    ] {
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ck::spawn(&mut sim, shooter, 0, (10, 10));
        let t = ck::spawn(&mut sim, WARRIOR, 1, (16, 10));
        // View +x to the target: no turn tick before reaching it (C-187).
        sim.object_mut(s).unwrap().direction = crate::fx::Fx(0);
        if house {
            ck::spawn(&mut sim, HOUSE, 2, (13, 10));
        }
        crate::messaging::post_message(&mut sim, s, Message::new(0x111, &[t.as_i32(), 0]), true);
        for _ in 0..2 {
            crate::tick::object_tick(&mut sim, s, 100);
        }
        let status = crate::module::attack::attack(&sim, s).unwrap().status;
        assert_eq!(
            status == 2,
            fights_at_once,
            "shooter {shooter}, house {house}: status {status}"
        );
    }
}

/// FINDTO at an object (argc 3) with the goal object in slot `slot` (`None`: without module).
fn find_to_object(slot: Option<usize>) -> (i32, i32) {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let (unit, observer) = scene(&mut state, &rules, (5, 5));
    let mut sim = Sim::new(&mut state, &rules);
    let target = create_object(
        &mut sim,
        &mut ObjectDesc::new(1000, 0, [12.0 * 128.0 + 64.0, 0.0, 5.0 * 128.0 + 64.0]),
    )
    .unwrap();
    if let Some(index) = slot {
        // Module 50 has no class in the original; the slot is occupied here only for the branch.
        sim.object_mut(target).unwrap().modules[index] = Some(crate::module::Module {
            base: crate::module::base::ModuleBase::new(index as u8),
            kind: crate::module::ModuleKind::Inert,
        });
    }
    let env = Envelope {
        msg: Message::new(0x200, &[target.as_i32(), -1, 0x201]),
        success: Some(Message::new(0xA7F, &[111])),
        fault: Some(Message::new(0xA7F, &[222])),
        success_target: observer,
        fault_target: observer,
    };
    crate::messaging::dispatch_command(&mut sim, unit, &env);
    // The radius survives the synchronously following PATH, the look-at point does not.
    let w = walk(&sim, unit).unwrap();
    (w.max_goal_radius, w.speed.default)
}

#[test]
fn find_to_a_tower_gets_the_building_radius() {
    // `PropertyWalk_OnMsgFindTo` `0x0053884E`: module 0x32 like module 3 -> max(default, 0x400 / 4).
    let (radius, default) = find_to_object(Some(0x32));
    assert_eq!(radius, default.max(0x100));
    assert_ne!(radius, 0x400);
    // Counter-check: an object without module 3/0x32 gets the unit radius 0x400.
    assert_eq!(find_to_object(None).0, 0x400);
}
