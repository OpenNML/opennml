//! Tests of hiding: synthetic combat world without corpus.

use std::cell::RefCell;

use super::*;
use crate::combat::testkit as ck;
use crate::command::testkit::fogged_state;
use crate::module::building::building;
use crate::rules::Rules;
use crate::save::checksum;
use crate::save::testkit::save_and_load;
use crate::state::SimState;
use crate::tick::{step, test_hook};
use crate::world::{change_owner, process_destroy_queue, queue_destroy};

thread_local! {
    /// Object ticks of the running test with the phase of the world tick.
    static TICKED: RefCell<Vec<(ObjectId, u8)>> = const { RefCell::new(Vec::new()) };
}

fn record(sim: &mut Sim, id: ObjectId) {
    let phase = sim.state.phase;
    TICKED.with(|t| t.borrow_mut().push((id, phase)));
}

/// The object ticks of `n` steps.
fn ticks(state: &mut SimState, rules: &Rules, n: usize) -> Vec<(ObjectId, u8)> {
    TICKED.with(|t| t.borrow_mut().clear());
    test_hook::with(record, || {
        for _ in 0..n {
            step(state, rules, &[]);
        }
    });
    TICKED.with(|t| t.borrow().clone())
}

fn ticked_ids(ticks: &[(ObjectId, u8)]) -> Vec<ObjectId> {
    ticks.iter().map(|&(id, _)| id).collect()
}

fn in_collision(state: &SimState, id: ObjectId) -> bool {
    state.collision.all.iter().any(|l| l.contains(&id))
}

fn in_groups(state: &SimState, id: ObjectId) -> bool {
    state.tick_groups.iter().any(|g| g.contains(&id))
}

fn blocked(sim: &Sim, id: ObjectId) -> bool {
    building(sim, id).expect("building").blocked
}

#[test]
fn deactivate_removes_from_tick_collision_and_area_queries_keeps_tables_and_counts() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, hq, other, key);
    {
        let mut sim = Sim::new(&mut state, &rules);
        a = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
        hq = ck::spawn(&mut sim, ck::HQ, 1, (20, 20));
        other = ck::spawn(&mut sim, ck::SABRE, 2, (6, 6));
        key = crate::area::register(&mut sim, crate::area::ENEMY_WATCH, [0, 0, 10, 10], 0);
        sim.state.area_queries.queries.get_mut(&key).unwrap().list = vec![a, other];
        assert!(blocked(&sim, hq));
        // A pair record, so that emptying the collision logs becomes visible.
        sim.state.collision_records.soft.push((1, 2));
        sim.state.collision_records.records.insert(
            (1, 2),
            crate::collision::records::Record {
                state: 0,
                hard: false,
                kind: 0,
            },
        );
    }
    let group_of_a = state.object(a, false).unwrap().tick_group;
    let counts = (
        state.tables[1].count,
        state.guid_counts[1].get(&ck::SABRE).copied(),
    );
    {
        let mut sim = Sim::new(&mut state, &rules);
        deactivate_slot(&mut sim, 1);
        assert!(!sim.state.slot_running[1]);
        for id in [a, hq] {
            assert!(sim.object(id).is_some(), "stays in the slot table");
            assert!(!in_collision(sim.state, id), "removed");
            assert!(!in_groups(sim.state, id), "without tick group");
            assert!(is_hidden(sim.state, id));
        }
        assert!(!blocked(&sim, hq), "building releases its tiles");
        assert_eq!(sim.state.area_queries.queries[&key].list, [other]);
        assert_eq!(sim.object(a).unwrap().tick_group, group_of_a, "field stays");
        assert_eq!(
            (
                sim.state.tables[1].count,
                sim.state.guid_counts[1].get(&ck::SABRE).copied()
            ),
            counts
        );
        assert!(sim.state.inactive_lists[1].is_empty());
        assert_eq!(
            sim.state.collision_records,
            crate::collision::records::CollisionRecords::default()
        );
        assert!(in_collision(sim.state, other) && !is_hidden(sim.state, other));
        // A second 159 does nothing.
        let before = checksum(sim.state);
        deactivate_slot(&mut sim, 1);
        assert_eq!(checksum(sim.state), before);
        // Slots outside 0...10 have no effect.
        deactivate_slot(&mut sim, 11);
        deactivate_slot(&mut sim, -1);
        assert_eq!(checksum(sim.state), before);
    }
    let t = ticked_ids(&ticks(&mut state, &rules, 4));
    assert!(!t.contains(&a) && !t.contains(&hq));
    assert!(t.contains(&other));
}

#[test]
fn objects_created_while_hidden_are_listed_ticked_in_phase_1_and_destroyed_on_activate() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (g0, g1, c);
    {
        let mut sim = Sim::new(&mut state, &rules);
        g0 = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
        g1 = ck::spawn(&mut sim, ck::SABRE, 0, (6, 5));
        deactivate_slot(&mut sim, 1);
        c = ck::spawn(&mut sim, ck::SABRE, 1, (7, 7));
        assert_eq!(sim.state.inactive_lists[1], [c]);
        let o = sim.object(c).unwrap();
        assert_eq!(o.tick_group, -1);
        assert!(!in_groups(sim.state, c));
        assert!(in_collision(sim.state, c), "registered normally");
        assert!(!is_hidden(sim.state, c));
        assert_eq!(sim.state.tick_groups[0], [g0]);
        assert_eq!(sim.state.tick_groups[1], [g1]);
    }
    // Phase 1 (first step): group 0, then the special list; phase 0: group 1.
    let t = ticks(&mut state, &rules, 4);
    assert_eq!(
        t,
        [(g0, 1), (c, 1), (g1, 0), (g0, 1), (c, 1), (g1, 0)],
        "special list once per 100 ms directly after group 0"
    );
    let mut sim = Sim::new(&mut state, &rules);
    activate_slot(&mut sim, 1);
    assert!(sim.object(c).is_none(), "160 destroys the special list");
    assert!(sim.state.inactive_lists[1].is_empty());
    assert!(sim.state.slot_running[1]);
}

#[test]
fn activate_flushes_the_whole_destroy_queue_immediately() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let foreign = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
    let hidden_owner = ck::spawn(&mut sim, ck::SABRE, 1, (6, 6));
    deactivate_slot(&mut sim, 1);
    queue_destroy(&mut sim, foreign);
    // 160 without a special list: the foreign destruction happens at once anyway, not at the end of the tick.
    activate_slot(&mut sim, 1);
    assert!(sim.object(foreign).is_none());
    assert!(sim.state.destroy_queue.is_empty());
    assert!(sim.object(hidden_owner).is_some());
    // A 160 for a running slot does nothing, not even to the queue.
    let late = ck::spawn(&mut sim, ck::SABRE, 0, (8, 8));
    queue_destroy(&mut sim, late);
    activate_slot(&mut sim, 1);
    assert!(sim.object(late).is_some());
    assert_eq!(sim.state.destroy_queue, [late]);
}

#[test]
fn activate_reinserts_remaining_objects_in_table_order() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
    let b = ck::spawn(&mut sim, ck::SABRE, 1, (5, 6));
    let z = ck::spawn(&mut sim, ck::SABRE, 0, (6, 5));
    let hq = ck::spawn(&mut sim, ck::HQ, 1, (20, 20));
    let bucket = |s: &SimState| {
        s.collision
            .all
            .iter()
            .find(|l| l.contains(&z))
            .cloned()
            .unwrap()
    };
    assert_eq!(bucket(sim.state), [a, b, z]);
    assert_eq!(sim.state.tick_groups, [vec![a, z], vec![b, hq]]);
    deactivate_slot(&mut sim, 1);
    assert_eq!(sim.state.tick_groups, [vec![z], vec![]]);
    activate_slot(&mut sim, 1);
    // Registered at the back again, in table order a, b, hq; tick group by "shorter list".
    assert_eq!(bucket(sim.state), [z, a, b]);
    assert_eq!(sim.state.tick_groups, [vec![z, b], vec![a, hq]]);
    assert_eq!(sim.object(a).unwrap().tick_group, 1);
    assert_eq!(sim.object(b).unwrap().tick_group, 0);
    assert!(blocked(&sim, hq) && in_collision(sim.state, hq));
    assert!(!is_hidden(sim.state, a));
}

#[test]
fn dead_ids_stay_in_the_list_and_are_skipped() {
    let rules = ck::rules();
    let mut state = fogged_state(&rules);
    let (c, d);
    {
        let mut sim = Sim::new(&mut state, &rules);
        deactivate_slot(&mut sim, 1);
        c = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
        d = ck::spawn(&mut sim, ck::SABRE, 1, (40, 40));
        queue_destroy(&mut sim, c);
        process_destroy_queue(&mut sim);
        assert!(sim.object(c).is_none());
        assert_eq!(sim.state.inactive_lists[1], [c, d], "the dead ID stays");
        assert!(!is_hidden(sim.state, c), "listed, so not hidden");
    }
    // Tick and fog skip it; a whole fog cycle runs through.
    let t = ticks(&mut state, &rules, 40);
    assert!(!t.iter().any(|&(id, _)| id == c));
    assert_eq!(t.iter().filter(|&&(id, _)| id == d).count(), 20);
    assert_eq!(state.inactive_lists[1], [c, d]);
    assert!(
        state.fog.seen.get(40, 40).unwrap() & 0b10 != 0,
        "d gives sight"
    );
    // 160 skips it when queueing and empties the list.
    let mut sim = Sim::new(&mut state, &rules);
    activate_slot(&mut sim, 1);
    assert!(sim.object(d).is_none());
    assert!(sim.state.inactive_lists[1].is_empty());
    assert!(sim.state.destroy_queue.is_empty());
}

#[test]
fn owner_change_into_and_out_of_hidden_slots() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hidden = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
    let running = ck::spawn(&mut sim, ck::SABRE, 2, (6, 6));
    deactivate_slot(&mut sim, 1);
    let listed = ck::spawn(&mut sim, ck::SABRE, 1, (7, 7));
    let hq = ck::spawn(&mut sim, ck::HQ, 2, (20, 20));

    // Hidden -> running: ticked, but stays removed (E-198).
    let h2 = change_owner(&mut sim, hidden, 2).unwrap();
    assert!(in_groups(sim.state, h2));
    assert!(!in_collision(sim.state, h2));
    assert!(!is_hidden(sim.state, h2));

    // Running -> hidden: into the special list without registering (stays registered), `tick_group = 0`.
    let r1 = change_owner(&mut sim, running, 1).unwrap();
    assert_eq!(sim.state.inactive_lists[1], [listed, r1]);
    assert!(!in_groups(sim.state, r1));
    assert!(in_collision(sim.state, r1));
    assert_eq!(sim.object(r1).unwrap().tick_group, 0);
    let hq1 = change_owner(&mut sim, hq, 1).unwrap();
    assert!(blocked(&sim, hq1), "occupancy stays as it was");
    assert!(!in_groups(sim.state, hq1));

    // Shown -> running: out of the special list, without removing.
    let l2 = change_owner(&mut sim, listed, 2).unwrap();
    assert_eq!(sim.state.inactive_lists[1], [r1, hq1]);
    assert!(in_groups(sim.state, l2) && in_collision(sim.state, l2));

    // Hidden -> hidden (slot 3 off as well): only the list changes.
    deactivate_slot(&mut sim, 3);
    let r3 = change_owner(&mut sim, r1, 3).unwrap();
    assert_eq!(sim.state.inactive_lists[1], [hq1]);
    assert_eq!(sim.state.inactive_lists[3], [r3]);
    assert!(in_collision(sim.state, r3) && !in_groups(sim.state, r3));
}

#[test]
fn add_inactive_needs_hidden_owner_slot_and_no_duplicates() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
    let b = ck::spawn(&mut sim, ck::SABRE, 1, (6, 6));
    let hq = ck::spawn(&mut sim, ck::HQ, 1, (20, 20));
    // Slot running: no effect.
    add_inactive(&mut sim, a, false);
    assert!(sim.state.inactive_lists[1].is_empty());

    deactivate_slot(&mut sim, 1);
    add_inactive(&mut sim, a, false);
    add_inactive(&mut sim, a, false);
    assert_eq!(sim.state.inactive_lists[1], [a], "no duplicates");
    assert!(in_collision(sim.state, a) && !in_groups(sim.state, a));
    assert_eq!(sim.object(a).unwrap().tick_group, 0);
    add_inactive(&mut sim, hq, false);
    assert!(blocked(&sim, hq), "shown building blocks again");
    // Without registering (`no_block`): listed, but not in the collision manager.
    add_inactive(&mut sim, b, true);
    assert_eq!(sim.state.inactive_lists[1], [a, hq, b]);
    assert!(!in_collision(sim.state, b));
    // Stale ID: nothing.
    let stale = ObjectId::new(a.index() as u16, 1, a.generation().wrapping_add(1));
    add_inactive(&mut sim, stale, false);
    assert_eq!(sim.state.inactive_lists[1].len(), 3);

    // Removing: only listed ones; the existing object leaves the collision manager.
    remove_inactive(&mut sim, a);
    assert_eq!(sim.state.inactive_lists[1], [hq, b]);
    assert!(!in_collision(sim.state, a) && is_hidden(sim.state, a));
    assert_eq!(sim.object(a).unwrap().tick_group, 0, "field stays");
    remove_inactive(&mut sim, hq);
    assert!(!blocked(&sim, hq));
    let before = checksum(sim.state);
    remove_inactive(&mut sim, a);
    assert_eq!(checksum(sim.state), before, "not listed: nothing");
}

#[test]
fn remove_inactive_advances_the_fog_cursor() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    deactivate_slot(&mut sim, 1);
    let ids: Vec<ObjectId> = (0..4)
        .map(|i| ck::spawn(&mut sim, ck::SABRE, 1, (5 + i, 5)))
        .collect();
    sim.state.fog.player = 1;
    sim.state.fog.inactive_pos = 2;
    // The pointer stands on ids[2]: removed -> it points to ids[3], so index 2 again.
    remove_inactive(&mut sim, ids[2]);
    assert_eq!(sim.state.fog.inactive_pos, 2);
    // An entry before it: the index moves along, the pointer stays on ids[3].
    remove_inactive(&mut sim, ids[0]);
    assert_eq!(sim.state.fog.inactive_pos, 1);
    assert_eq!(sim.state.inactive_lists[1], [ids[1], ids[3]]);
    // An entry behind it: nothing.
    remove_inactive(&mut sim, ids[3]);
    assert_eq!(sim.state.fog.inactive_pos, 1);
    // End of list (null pointer): a new entry does not count until the next cycle, the pointer stays at the end.
    let late = ck::spawn(&mut sim, ck::SABRE, 1, (9, 9));
    assert_eq!(sim.state.inactive_lists[1], [ids[1], late]);
    assert_eq!(sim.state.fog.inactive_pos, 2);
    // If the fog stands at another slot, its index stays untouched.
    sim.state.fog.player = 2;
    sim.state.fog.inactive_pos = 0;
    remove_inactive(&mut sim, ids[1]);
    ck::spawn(&mut sim, ck::SABRE, 1, (10, 9));
    assert_eq!(sim.state.fog.inactive_pos, 0);
}

#[test]
fn fog_cycle_of_hidden_slot_reads_only_its_list() {
    let rules = ck::rules();
    let mut state = fogged_state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        let hidden = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
        ck::spawn(&mut sim, ck::SABRE, 0, (40, 5));
        deactivate_slot(&mut sim, 1);
        ck::spawn(&mut sim, ck::SABRE, 1, (40, 40));
        // A vision source bound to the hidden object (player 3) keeps stamping (E-198).
        crate::fog::set_vision_source(
            &mut sim.state.fog,
            crate::fog::VisionSource {
                x: -1,
                z_or_hid: hidden.as_i32(),
                size: 8,
                player: 3,
                remaining_ms: -1,
                flags: crate::fog::EXPLORE | crate::fog::SEE,
            },
        );
    }
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
    }
    let bit = |s: &SimState, x, z| s.fog.seen.get(x, z).unwrap();
    assert_eq!(bit(&state, 5, 5) & 0b10, 0, "hidden: no sight");
    assert_eq!(state.fog.explored.get(5, 5).unwrap() & 0b10, 0);
    assert_ne!(bit(&state, 40, 40) & 0b10, 0, "shown: sight");
    assert_ne!(bit(&state, 40, 5) & 0b01, 0, "other slots as before");
    assert_ne!(
        bit(&state, 5, 5) & 0b1000,
        0,
        "vision source on the hidden object"
    );
    // After 160 the slot sees through its table again.
    {
        let mut sim = Sim::new(&mut state, &rules);
        activate_slot(&mut sim, 1);
    }
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
    }
    assert_ne!(bit(&state, 5, 5) & 0b10, 0);
}

/// Hidden slot 1 with three entries in `L[1]` (one dead), slot 2 runs with a unit of its own.
fn hidden_scene() -> (Rules, SimState) {
    let rules = ck::rules();
    let mut state = fogged_state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        ck::spawn(&mut sim, ck::SABRE, 0, (40, 5));
        ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
        deactivate_slot(&mut sim, 1);
        for i in 0..4 {
            ck::spawn(&mut sim, ck::SABRE, 1, (30 + i, 30));
        }
        let dead = sim.state.inactive_lists[1][1];
        queue_destroy(&mut sim, dead);
        process_destroy_queue(&mut sim);
        ck::spawn(&mut sim, ck::SABRE, 2, (50, 50));
    }
    (rules, state)
}

/// 200 steps from the state and from its savegame: equal checksums at every step.
fn assert_same_after_reload(state: &SimState, rules: &Rules) {
    let mut loaded = save_and_load(state, rules);
    assert_eq!(checksum(&loaded), checksum(state));
    let mut original = state.clone();
    for n in 0..200 {
        step(&mut original, rules, &[]);
        step(&mut loaded, rules, &[]);
        assert_eq!(checksum(&loaded), checksum(&original), "step {n}");
    }
}

#[test]
fn save_and_load_during_hide_keeps_checksums() {
    let (rules, mut state) = hidden_scene();
    // Until the fog stands in the middle of `L[1]` (pointer neither at the start nor at the end).
    let mut n = 0;
    while !(state.fog.player == 1
        && state.fog.processed > 0
        && state.fog.inactive_pos > 0
        && (state.fog.inactive_pos as usize) < state.inactive_lists[1].len())
    {
        step(&mut state, &rules, &[]);
        n += 1;
        assert!(n < 200, "fog does not reach L[1]");
    }
    assert_same_after_reload(&state, &rules);
}

#[test]
fn save_and_load_after_fog_leaves_hidden_slot_for_running_slot() {
    let (rules, mut state) = hidden_scene();
    // First step in which the cycle, after the hidden slot 1, stands at the running slot 2.
    let mut n = 0;
    let mut was_hidden = false;
    loop {
        step(&mut state, &rules, &[]);
        n += 1;
        assert!(n < 200, "fog does not reach slot 2");
        if was_hidden && state.fog.player == 2 && state.fog.processed > 0 {
            break;
        }
        was_hidden = state.fog.player == 1 && state.fog.inactive_pos > 0;
    }
    assert_eq!(
        state.fog.inactive_pos, 0,
        "player change sets the pointer to the list head"
    );
    assert_same_after_reload(&state, &rules);
    // End of cycle (player 8): the pointer is back at 0; this state loads as well.
    while state.fog.player != 8 {
        step(&mut state, &rules, &[]);
    }
    assert_eq!(state.fog.inactive_pos, 0);
    assert_same_after_reload(&state, &rules);
}
