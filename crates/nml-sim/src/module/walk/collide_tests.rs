//! Vector tests of the collision handling (re/collision B10, vectors T1, T2 and T9 to T11): the
//! decision functions one by one, on small setups with units (hard radius 512, soft 1024, same player).

use super::*;
use crate::collision::pair_overlap_cost;
use crate::collision::records::{PairKey, Record, pair_key};
use crate::fx::Fx;
use crate::module::walk::tests::rules;
use crate::state::SimState;
use crate::world::{ObjectDesc, create_object};

/// Status bit "unit is walking".
const MOVE: u32 = status::MOVE;

/// Setup: units at grid positions `(x, z)` (fixed point, 0x1000 per tile) in order of creation,
/// i.e. with ascending IDs.
fn setup(positions: &[(i32, i32)]) -> (crate::rules::Rules, SimState, Vec<ObjectId>) {
    let rules = rules();
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let ids = {
        let mut sim = Sim::new(&mut state, &rules);
        positions
            .iter()
            .map(|&(x, z)| {
                let pos = [x as f32 / 32.0, 0.0, z as f32 / 32.0];
                create_object(&mut sim, &mut ObjectDesc::new(1100, 0, pos)).unwrap()
            })
            .collect()
    };
    (rules, state, ids)
}

fn put_status(sim: &mut Sim, id: ObjectId, flags: u32, on: bool) {
    sim.object_mut(id).unwrap().set_status_flags(flags, on);
}

fn insert_record(sim: &mut Sim, a: ObjectId, b: ObjectId, hard: bool) -> PairKey {
    let key = pair_key(a, b);
    let recs = &mut sim.state.collision_records;
    recs.records.insert(
        key,
        Record {
            state: 0,
            hard,
            kind: 0,
        },
    );
    if hard {
        recs.hard.push(key);
    } else {
        recs.soft.push(key);
    }
    key
}

fn record_of(sim: &Sim, key: PairKey) -> Record {
    sim.state.collision_records.records[&key]
}

#[test]
fn collision_priority_orders_by_mobility_idleness_radius_and_handle() {
    // T1.
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), (0x4000, 0x2000)]);
    let mut sim = Sim::new(&mut state, &rules);
    let (a, b) = (ids[0], ids[1]);
    assert!(a.as_i32() < b.as_i32());
    // Same radii: the smaller handle 1, the larger 2.
    assert_eq!((compare(&mut sim, a, b), compare(&mut sim, b, a)), (1, 2));
    // Smaller radius 0, larger 3.
    sim.object_mut(a).unwrap().hard_radius = 256;
    assert_eq!((compare(&mut sim, a, b), compare(&mut sim, b, a)), (0, 3));
    sim.object_mut(a).unwrap().hard_radius = 512;
    // I walk, the other is idle: 3; the other way round 0.
    put_status(&mut sim, a, MOVE, true);
    assert_eq!((compare(&mut sim, a, b), compare(&mut sim, b, a)), (3, 0));
    put_status(&mut sim, a, MOVE, false);
    // The other cannot move: 0 (it then has no priority); the other way round 3.
    walk_mut(&mut sim, b).unwrap().can_move = false;
    assert_eq!((compare(&mut sim, a, b), compare(&mut sim, b, a)), (0, 3));
}

#[test]
fn overtaking_needs_more_speed_and_at_most_23_degrees_difference() {
    // T2: 170 degrees against -170 degrees (20 degrees difference) -> yes, against -166 degrees (24 degrees) -> no.
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), (0x4000, 0x2000)]);
    let mut sim = Sim::new(&mut state, &rules);
    let (x, y) = (ids[0], ids[1]);
    walk_mut(&mut sim, x).unwrap().speed.cur = 200;
    walk_mut(&mut sim, y).unwrap().speed.cur = 100;
    sim.object_mut(x).unwrap().direction = Fx(170 << 12);
    sim.object_mut(y).unwrap().direction = Fx(-170 << 12);
    assert!(can_overtake(&sim, x, y));
    sim.object_mut(y).unwrap().direction = Fx(-166 << 12);
    assert!(!can_overtake(&sim, x, y));
    // Limit: exactly 23 degrees still count, likewise the same direction; equally fast never.
    sim.object_mut(y).unwrap().direction = Fx(-167 << 12);
    assert!(can_overtake(&sim, x, y));
    sim.object_mut(y).unwrap().direction = Fx(170 << 12);
    assert!(can_overtake(&sim, x, y));
    walk_mut(&mut sim, y).unwrap().speed.cur = 200;
    assert!(!can_overtake(&sim, x, y));
}

#[test]
fn a_move_stand_target_inside_the_idle_unit_shifts_to_the_own_position() {
    // T9: last waypoint (0x2800, 0x2000) lies in the idle unit at 0x2900, no goal radius.
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), (0x2900, 0x2000)]);
    let mut sim = Sim::new(&mut state, &rules);
    let (me, o) = (ids[0], ids[1]);
    let key = insert_record(&mut sim, me, o, false);
    let w = walk_mut(&mut sim, me).unwrap();
    let head = w.point(0x2800, 0x2000);
    w.high_path.push_back(head);
    w.next_hp = Some(head.id);
    w.max_goal_radius = -1;
    // Home and remembered FINDTO carry the same target and move along.
    w.home = (0x2800, 0x2000, 0);
    w.find_to = Some(Message::new(0x200, &[0x2800, 0x2000, -1, 0]));
    predict_move_stand(&mut sim, me, key, o);
    let w = walk(&sim, me).unwrap();
    assert_eq!((w.home.0, w.home.1), (0x2000, 0x2000));
    assert_eq!(w.find_to.as_ref().unwrap().args[..2], [0x2000, 0x2000]);
    // The waypoint now lies on the own position and is consumed in the projection.
    assert!(w.high_path.is_empty());
    // If the target does not lie in the idle unit, it stays: idle unit far away.
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), (0x6900, 0x2000)]);
    let mut sim = Sim::new(&mut state, &rules);
    let (me, o) = (ids[0], ids[1]);
    let key = insert_record(&mut sim, me, o, false);
    let w = walk_mut(&mut sim, me).unwrap();
    let head = w.point(0x2800, 0x2000);
    w.high_path.push_back(head);
    w.next_hp = Some(head.id);
    predict_move_stand(&mut sim, me, key, o);
    let w = walk(&sim, me).unwrap();
    assert_eq!((w.high_path[0].x, w.high_path[0].z), (0x2800, 0x2000));
}

#[test]
fn two_idle_units_overlapping_hard_resolve_by_the_one_with_priority() {
    // T10: the smaller handle resolves: mobile home, `0x1000 {other}` -> state 4,
    // record state 2, kind 3.
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), (0x2100, 0x2000)]);
    let mut sim = Sim::new(&mut state, &rules);
    let (me, o) = (ids[0], ids[1]);
    let key = insert_record(&mut sim, me, o, true);
    // The one with the larger handle does not decide.
    stand_stand(&mut sim, o, key, me);
    assert_eq!(record_of(&sim, key).state, 0);
    assert_eq!(walk(&sim, o).unwrap().state, 1);
    stand_stand(&mut sim, me, key, o);
    let r = record_of(&sim, key);
    assert_eq!((r.state, r.kind), (2, 3));
    let w = walk(&sim, me).unwrap();
    assert!(w.mobile_home);
    assert_eq!((w.state, w.restore_state, w.hid_to_escape), (4, 2, o));
    assert_ne!(sim.object(me).unwrap().status & MOVE, 0);
    assert_ne!(sim.object(me).unwrap().status & WAITING, 0);
    // Soft overlap of two resting units is only settled (state 2), nobody plans.
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), (0x2700, 0x2000)]);
    let mut sim = Sim::new(&mut state, &rules);
    let key = insert_record(&mut sim, ids[0], ids[1], false);
    stand_stand(&mut sim, ids[0], key, ids[1]);
    assert_eq!(record_of(&sim, key).state, 2);
    assert_eq!(walk(&sim, ids[0]).unwrap().state, 1);
}

/// `me` waits (state 3, before that 2) for `o`; both walk.
fn waiting_pair(o_at: (i32, i32)) -> (crate::rules::Rules, SimState, ObjectId, ObjectId) {
    let (rules, mut state, ids) = setup(&[(0x2000, 0x2000), o_at]);
    let mut sim = Sim::new(&mut state, &rules);
    let (me, o) = (ids[0], ids[1]);
    for id in [me, o] {
        put_status(&mut sim, id, MOVE, true);
        walk_mut(&mut sim, id).unwrap().state = 2;
    }
    walk_mut(&mut sim, me).unwrap().speed.cur = 100;
    assert_eq!(
        on_msg_walk_wait(&mut sim, me, &Message::new(0x700, &[o.as_i32()])),
        1
    );
    let w = walk(&sim, me).unwrap();
    assert_eq!((w.state, w.restore_state, w.wait_for), (3, 2, o));
    (rules, state, me, o)
}

#[test]
fn waiting_ends_at_once_when_the_other_is_far_and_after_49_ticks_when_it_stays() {
    // T11, opponent far away: done in the first tick, state as before, no re-evaluation.
    let (rules, mut state, me, _) = waiting_pair((0x9000, 0x2000));
    let mut sim = Sim::new(&mut state, &rules);
    tick_waiting(&mut sim, me);
    let w = walk(&sim, me).unwrap();
    assert_eq!((w.state, w.wait_for), (2, ObjectId::NONE));
    assert_eq!(sim.object(me).unwrap().status & (WAITING | REEVALUATE), 0);

    // The opponent stays in the soft overlap (d = 0x700): keep waiting for 49 ticks, give up in the 50th.
    let (rules, mut state, me, o) = waiting_pair((0x2700, 0x2000));
    let mut sim = Sim::new(&mut state, &rules);
    assert!(pair_overlap_cost(&mut sim, o, me, None, None, false, false) > 0);
    for tick in 1..=49 {
        tick_waiting(&mut sim, me);
        let w = walk(&sim, me).unwrap();
        assert_eq!((w.state, w.wait_counter), (3, tick), "tick {tick}");
        assert_eq!(sim.object(me).unwrap().status & REEVALUATE, 0);
    }
    tick_waiting(&mut sim, me);
    let w = walk(&sim, me).unwrap();
    assert_eq!((w.state, w.wait_for), (2, ObjectId::NONE));
    let st = sim.object(me).unwrap().status;
    // "Gave up waiting": status bits 0x801 (walking, re-evaluate), waiting off.
    assert_eq!(st & (MOVE | REEVALUATE | WAITING), 0x801);

    // If the opponent stands still (no longer walking), it gives up at once.
    let (rules, mut state, me, o) = waiting_pair((0x2700, 0x2000));
    let mut sim = Sim::new(&mut state, &rules);
    put_status(&mut sim, o, MOVE, false);
    tick_waiting(&mut sim, me);
    assert_eq!(walk(&sim, me).unwrap().state, 2);
    assert_eq!(sim.object(me).unwrap().status & REEVALUATE, REEVALUATE);
}
