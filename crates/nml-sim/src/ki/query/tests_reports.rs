//! Tests of taking attack reports `0x512E` and of the queries on dead and vanished objects (re/ki-queries 5.1, 5.11;
//! E-55, E-60, E-62).

use super::tests_player::{FLOOR, ME, ask, run, run_with};
use super::tests_search::{CORPSE, ME_I, oid, put, scene};
use super::{Direct, query_once};
use crate::combat::testkit::WARRIOR;
use crate::id::ObjectId;
use crate::ki::testkit::int;
use crate::object::status;
use crate::player::Sight;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::{Control, ReportTile};

/// Report `victim → (x, z)` in the table of slot [`ME`] (like `report_attack`: new at the front of the bucket).
fn report(sim: &mut Sim, victim: i32, x: i32, z: i32) {
    sim.state.attack_reports[usize::from(ME)].insert(victim, ReportTile { x, z });
}

/// Keys of the reports of [`ME`] in MFC order.
fn keys(sim: &Sim) -> Vec<i32> {
    sim.state.attack_reports[usize::from(ME)]
        .iter()
        .map(|(k, _)| k)
        .collect()
}

/// 0x16F `(x, z, &a, &b)`: push and `(a, b)` (variables 0 and 1).
fn take(sim: &mut Sim, x: i32, z: i32) -> (i32, i32, i32) {
    let (t, notes) = run_with(sim, 0x16F, &[FLOOR, x, z, int(0) as i32, int(1) as i32]);
    assert!(notes.is_empty());
    assert_eq!(t.stack[..1], [FLOOR]);
    (t.stack[1], t.vars.get(int(0)), t.vars.get(int(1)))
}

#[test]
fn nearest_attack_report_wins_and_ties_follow_mfc_order() {
    // re/ki-queries 5.11.3, E-62: `Fixed_Hypot` on whole tiles, (5, 0), (5, 3), (4, 4) -> 5 each; strictly smaller
    // wins, so the first in MFC order (bucket `(hid >> 4) % 17` ascending, in the bucket the most recently created
    // first).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    // Indices 0, 17, 34 of slot 1 (generation 1) lie in the same bucket (128 - i) mod 17.
    let [a, b, c] = [0, 17, 34].map(|i| ObjectId::new(i, ME, 1).as_i32());
    let far = ObjectId::new(5, ME, 1).as_i32();
    report(&mut sim, a, 5, 0);
    report(&mut sim, b, 5, 3);
    report(&mut sim, c, 4, 4);
    report(&mut sim, far, 9, 9);
    let order = keys(&sim);
    let first_tie = *order.iter().find(|k| **k != far).unwrap();
    assert_eq!(first_tie, c, "most recently created first in the bucket");
    // Query from (0, 0): `[2] = 1`, a <- x, b <- z of the victim tile; the report is taken.
    assert_eq!(take(&mut sim, 0, 0), (1, 4, 4));
    assert_eq!(
        keys(&sim),
        order
            .iter()
            .copied()
            .filter(|&k| k != c)
            .collect::<Vec<_>>()
    );
    assert_eq!(take(&mut sim, 0, 0), (1, 5, 3));
    assert_eq!(take(&mut sim, 0, 0), (1, 5, 0));
    assert_eq!(take(&mut sim, 0, 0), (1, 9, 9));
    // Without a report: `[2] = 0`, the variables get the inputs.
    assert_eq!(take(&mut sim, 7, -2), (0, 7, -2));
    assert!(keys(&sim).is_empty());
    // A later hit creates the report anew (at the front of its bucket).
    report(&mut sim, a, 1, 1);
    report(&mut sim, b, 1, 1);
    assert_eq!(keys(&sim), [b, a]);
    // The facade alone takes nothing but returns the direct change.
    let (req, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x512E, &[ME_I, 0, 0]);
    assert_eq!((req[2], req[3], req[4]), (1, 1, 1));
    assert_eq!(
        direct,
        [Direct::TakeReport {
            player: ME,
            victim: b
        }]
    );
    assert_eq!(keys(&sim), [b, a]);
}

#[test]
fn taking_a_report_into_one_variable_keeps_the_second_write() {
    // `0x16F (x, z, &a, &b)` writes first `var(b) = [4]`, then `var(a) = [3]`; if both point to the same variable, the
    // second write `[3]` (the tile x) wins. Without a report `[3]`/`[4]` stay the inputs, so the input x wins there.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let v = ObjectId::new(3, ME, 1).as_i32();
    let same = int(0) as i32;
    let (t, notes) = run_with(&mut sim, 0x16F, &[FLOOR, 3, 9, same, same]);
    assert!(notes.is_empty());
    assert_eq!((t.stack.clone(), t.vars.get(int(0))), (vec![FLOOR, 0], 3));
    report(&mut sim, v, 5, 7);
    let (t, notes) = run_with(&mut sim, 0x16F, &[FLOOR, 0, 0, same, same]);
    assert!(notes.is_empty());
    assert_eq!((t.stack.clone(), t.vars.get(int(0))), (vec![FLOOR, 1], 5));
}

#[test]
fn report_query_reads_only_slots_0_to_7_without_argc_check() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let v = ObjectId::new(3, ME, 1).as_i32();
    report(&mut sim, v, 6, 6);
    // Without an argc check (argc 0 or 9 alike).
    for words in [&[ME_I, 6, 6][..], &[ME_I, 6, 6, 0, 0, 0, 0, 0, 0]] {
        let (req, _) = query_once(&mut sim, ME, Sight::Omniscient, 0x512E, words);
        assert_eq!((req[2], req[3], req[4]), (1, 6, 6));
    }
    // Other or invalid slots: 0, inputs stay.
    for p in [0, 2, 8, -1, 31, 1000] {
        let (req, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x512E, &[p, 3, 4]);
        assert_eq!((req[2], req[3], req[4]), (0, 3, 4), "slot {p}");
        assert!(direct.is_empty());
    }
    // Under fog foreign reports are hidden, the own ones not.
    let (req, _) = query_once(&mut sim, 0, Sight::Fog, 0x512E, &[ME_I, 3, 4]);
    assert_eq!(req[2], 0);
    let (req, _) = query_once(&mut sim, ME, Sight::Fog, 0x512E, &[ME_I, 3, 4]);
    assert_eq!(req[2], 1);
    // Distance with wrapped differences: no panic at the edges.
    for (x, z) in [(i32::MIN, i32::MAX), (i32::MAX, i32::MIN)] {
        let (req, _) = query_once(&mut sim, ME, Sight::Omniscient, 0x512E, &[ME_I, x, z]);
        assert!(req[2] == 0 || (req[3], req[4]) == (6, 6));
    }
}

#[test]
fn report_of_a_removed_victim_is_still_returned() {
    // E-60: death and removal delete no report; it returns the last tile of the victim.
    let (rules, mut state) = scene();
    state.players[usize::from(ME)].control = Control::Ai;
    let mut sim = Sim::new(&mut state, &rules);
    let victim = put(&mut sim, WARRIOR, ME_I, (20, 21));
    let attacker = put(&mut sim, WARRIOR, 2, (22, 21));
    crate::messaging::notify_attacked_by(&mut sim, oid(victim), oid(attacker));
    assert_eq!(keys(&sim), [victim]);
    sim.object_mut(oid(victim)).unwrap().status |= status::DIED;
    crate::world::destroy_object(&mut sim, oid(victim));
    assert!(sim.state.object(oid(victim), false).is_none());
    assert_eq!(take(&mut sim, 0, 0), (1, 20, 21));
    assert!(keys(&sim).is_empty());
}

#[test]
fn dead_object_after_owner_change_to_slot_8() {
    // E-55, E-60: a dead object of a player changes to slot 8 and gets a new ID. The old one is missing, the new one is
    // dead and findable; the report for the old ID stays.
    let (rules, mut state) = scene();
    state.players[usize::from(ME)].control = Control::Ai;
    let mut sim = Sim::new(&mut state, &rules);
    let old = put(&mut sim, WARRIOR, ME_I, (16, 17));
    let attacker = put(&mut sim, WARRIOR, 2, (18, 17));
    crate::messaging::notify_attacked_by(&mut sim, oid(old), oid(attacker));
    sim.object_mut(oid(old)).unwrap().status |= status::DIED;
    let new = crate::world::change_owner(&mut sim, oid(old), 8)
        .expect("change of owner")
        .as_i32();
    let w = WARRIOR as i32;
    // Old ID: missing.
    let req = query_once(&mut sim, ME, Sight::Omniscient, 0x4E2C, &[ME_I, old, 0, 0]).0;
    assert_eq!((req[2], req[3], req[4]), (0, 0, 0));
    assert_eq!(ask(&mut sim, 0x4FC9, &[old]), -1);
    assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I, old]), 0);
    assert_eq!(ask(&mut sim, 0x4F97, &[ME_I, old]), 0);
    // New ID: dead but findable (`GetObjectById(id, 0)`), only `0x4E2A` excludes it.
    let req = query_once(&mut sim, ME, Sight::Omniscient, 0x4E2C, &[ME_I, new, 0, 0]).0;
    assert_eq!((req[2], req[3], req[4]), (1, 16, 17));
    assert_eq!(ask(&mut sim, 0x4FC9, &[new]), w);
    assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I, new]), 0);
    // Searches: the corpse now belongs to slot 8 (filter 182 "Corpse"), slot 1 has none anymore.
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, 8, w, 16, 17, CORPSE]), new);
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, ME_I, w, 16, 17, CORPSE]), 0);
    // Marking the old ID has no effect, on the new one it does (also dead).
    let before = checksum(sim.state);
    assert_eq!(run(&mut sim, 0x9B, &[old]), []);
    assert_eq!(checksum(sim.state), before);
    assert_eq!(run(&mut sim, 0x9B, &[new]), []);
    assert_eq!(sim.state.object(oid(new), false).unwrap().ai_marks, 1 << ME);
    // The report hangs on the old ID and returns the last tile.
    assert_eq!(keys(&sim), [old]);
    assert_eq!(take(&mut sim, 0, 0), (1, 16, 17));
}
