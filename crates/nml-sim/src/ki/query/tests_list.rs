//! Tests of the list queries `0x112`-`0x115`, `0x117`, `0x11E`, `0x11F`, `0x121` (re/ki-queries 5.8 and 5.15.1; E-28,
//! E-32, E-56).
//!
//! The scene has the filters 4, 5, 182, 183 and additionally 179 (`0xB3`) as "own" (`MM_HL_MYLIST`); the real `0xB3` of
//! the game is "status MOVE and module PROP_PTTOPT", here only the ID counts.

use nml_formats::targets::parse_targets;

use super::tests_search::{CORPSE, MINE, TARGETS, oid, put};
use super::{SimKiWorld, query_once};
use crate::combat::testkit::{WARRIOR, defines, rules as combat_rules, state};
use crate::ki::testkit::{int, list, proto};
use crate::ki::vm::Thread;
use crate::ki::world::{BuiltinCx, KiNote, KiWorld};
use crate::module::attack::attack_mut;
use crate::object::status;
use crate::player::Sight;
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;

/// Bottom stack value: shows that a builtin does not pop more than intended.
const FLOOR: i32 = -7;
/// List variable 0.
const L0: i32 = list(0) as i32;
/// Filter ID 183: "More than 70 % Health" (`MM2_MORETHAN70`): every undamaged object passes.
const HEALTHY: i32 = 183;
/// Filter ID `0xB3`, defined here as "own".
const B3: i32 = 0xB3;

/// The builtins `(list, start, n, x, z, R)`.
const DISTANCE: [u16; 2] = [0x112, 0x117];
/// The "all match" builtins `(list, start, n, filter)`.
const ALL: [u16; 2] = [0x113, 0x11E];
/// The "one matches" builtins `(list, start, n, filter)`.
const ANY: [u16; 2] = [0x115, 0x121];
/// The builtins `(list, start, n, A, B)`.
const PAIR: [u16; 2] = [0x114, 0x11F];

fn scene() -> (Rules, SimState) {
    let mut rules = combat_rules();
    let text = format!(
        "{TARGETS}BEGIN\nBUTTONMSG 179\nMY_MM\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_ANDALL\nMM_END\nTARGET_MM\nMM_END\nEND\n"
    );
    rules.flowcontrol_targets = parse_targets(&text, &defines()).entries;
    let state = state(&rules);
    (rules, state)
}

/// Thread of module 7 for the player `me`: integer variables 0-7, lists 0 and 1; list 0 holds `entries`.
fn thread(me: u8, stack: &[i32], entries: &[(i32, i32)]) -> Thread {
    let ranges = [(0, 8), (list(0), 2)];
    let mut t = Thread::from_prototype(&proto(7, &ranges, &[], &[]), 2, me, stack.to_vec());
    for &(k, v) in entries {
        t.vars.list_at_mut(L0).expect("list").insert(k, v);
    }
    t
}

/// Builtin `id` for the player `me` with the arguments `args` (source order) on a thread with the list `entries`;
/// thread and notes afterwards.
fn call_on(
    sim: &mut Sim,
    id: u16,
    me: u8,
    entries: &[(i32, i32)],
    args: &[i32],
) -> (Thread, Vec<KiNote>) {
    let mut stack = vec![FLOOR];
    stack.extend_from_slice(args);
    let mut t = thread(me, &stack, entries);
    let mut w = SimKiWorld::new(sim, Sight::Omniscient);
    w.builtin(id, &mut BuiltinCx::new(me, &mut t));
    (t, w.notes)
}

/// Pushed values of the call; the stack floor stays untouched, there is no anomaly.
fn run(sim: &mut Sim, id: u16, me: u8, entries: &[(i32, i32)], args: &[i32]) -> Vec<i32> {
    let (t, notes) = call_on(sim, id, me, entries, args);
    assert!(notes.is_empty(), "{id:#x}: {notes:?}");
    assert_eq!(t.stack.first(), Some(&FLOOR), "{id:#x} pops too much");
    t.stack[1..].to_vec()
}

/// List with the IDs `ids` from key 5.
fn ids_from_5(ids: &[i32]) -> Vec<(i32, i32)> {
    (5..).zip(ids.iter().copied()).collect()
}

/// Number of entries of list 0 after the call.
fn list_len(t: &Thread) -> usize {
    t.vars.list_at(L0).expect("list").len()
}

#[test]
fn all_match_checks_later_objects_for_player_one() {
    // E-28: `[2] = me` is set only before the loop; after the first matching object `[2] = 1`, all further ones are
    // checked for player 1. Filter "own": does the object belong to the player?
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mine = put(&mut sim, WARRIOR, 3, (10, 10));
    let mine_too = put(&mut sim, WARRIOR, 3, (12, 10));
    let player_one = put(&mut sim, WARRIOR, 1, (14, 10));
    for id in ALL {
        let check = |sim: &mut Sim, ids: &[i32]| {
            let n = ids.len() as i32;
            run(sim, id, 3, &ids_from_5(ids), &[L0, 5, n, MINE])
        };
        // The second object belongs to player 1: checked for player 1 it is "own" -> all match.
        assert_eq!(check(&mut sim, &[mine, player_one]), [1], "{id:#x}");
        // A second object of player 3 does not pass for player 1.
        assert_eq!(check(&mut sim, &[mine, mine_too]), [0], "{id:#x}");
        // The first object is still checked for `me`: if it belongs to player 1, it fails at once.
        assert_eq!(check(&mut sim, &[player_one, mine]), [0], "{id:#x}");
        assert_eq!(check(&mut sim, &[mine]), [1], "{id:#x}");
    }
}

#[test]
fn any_match_checks_later_objects_for_player_zero() {
    // E-28: after the first failure `[2] = 0`, all further ones are checked for player 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mine = put(&mut sim, WARRIOR, 3, (10, 10));
    let other = put(&mut sim, WARRIOR, 2, (12, 10));
    let player_zero = put(&mut sim, WARRIOR, 0, (14, 10));
    for id in ANY {
        let check = |sim: &mut Sim, ids: &[i32]| {
            let n = ids.len() as i32;
            run(sim, id, 3, &ids_from_5(ids), &[L0, 5, n, MINE])
        };
        // The second object belongs to player 0 and passes for player 0.
        assert_eq!(check(&mut sim, &[other, player_zero]), [1], "{id:#x}");
        // An object of player 3 does not pass for player 0 although `me` = 3.
        assert_eq!(check(&mut sim, &[other, mine]), [0], "{id:#x}");
        assert_eq!(check(&mut sim, &[mine]), [1], "{id:#x}");
        assert_eq!(check(&mut sim, &[other]), [0], "{id:#x}");
    }
}

#[test]
fn any_match_with_a_missing_first_object_is_one() {
    // E-56: if the first object is missing, `[2] = me` stays; for `me ≠ 0` the answer is 1 at once. For `me = 0`,
    // `[2] = 0` stays and the loop continues.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let other = put(&mut sim, WARRIOR, 2, (12, 10));
    let player_zero = put(&mut sim, WARRIOR, 0, (14, 10));
    let gone = put(&mut sim, WARRIOR, 2, (16, 10));
    crate::world::destroy_object(&mut sim, oid(gone));
    for id in ANY {
        for missing in [0, -1, gone] {
            let args = |n| [L0, 5, n, MINE];
            let one = ids_from_5(&[missing]);
            assert_eq!(run(&mut sim, id, 3, &one, &args(1)), [1], "{id:#x}");
            assert_eq!(run(&mut sim, id, 0, &one, &args(1)), [0], "{id:#x}");
            let two = ids_from_5(&[missing, other]);
            assert_eq!(run(&mut sim, id, 3, &two, &args(2)), [1], "{id:#x}");
            assert_eq!(run(&mut sim, id, 0, &two, &args(2)), [0], "{id:#x}");
            let two = ids_from_5(&[missing, player_zero]);
            assert_eq!(run(&mut sim, id, 0, &two, &args(2)), [1], "{id:#x}");
        }
    }
    // "All match" also leaves a missing object alone: for `me = 0` an immediate 0.
    for id in ALL {
        let one = ids_from_5(&[0]);
        assert_eq!(
            run(&mut sim, id, 3, &one, &[L0, 5, 1, MINE]),
            [1],
            "{id:#x}"
        );
        assert_eq!(
            run(&mut sim, id, 0, &one, &[L0, 5, 1, MINE]),
            [0],
            "{id:#x}"
        );
    }
}

/// An object of slot `slot` that attacks (`attack status` 1 = approach).
fn attacker(sim: &mut Sim, slot: i32, tile: (i32, i32)) -> i32 {
    let id = put(sim, WARRIOR, slot, tile);
    attack_mut(sim, oid(id)).expect("attack module").status = 1;
    id
}

#[test]
fn attacking_check_of_0x114_breaks_like_the_original() {
    // E-56: break with 0 only if the 0xB3 filter fails, the other passes and the object attacks. The player is carried
    // A -> B -> next A (A = last script argument); filter 183 passes every object, filter 5/179 ("own") only the
    // objects of the checked player.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    // Objects of slot 0 (me = 1): "own" fails for player 1, passes for player 0.
    let strange = attacker(&mut sim, 0, (10, 10));
    let strange_calm = put(&mut sim, WARRIOR, 0, (12, 10));
    // Object of slot 1: "own" passes for player 1.
    let own = attacker(&mut sim, 1, (14, 10));
    for id in PAIR {
        // Script `f(list, start, n, B, A)`: A is asked first (see `pair_asks_…`).
        let check = |sim: &mut Sim, obj: i32, a: i32, b: i32| {
            run(sim, id, 1, &ids_from_5(&[obj]), &[L0, 5, 1, b, a])
        };
        // B = 0xB3 fails (for player 1), A passes, the object attacks -> 0.
        assert_eq!(check(&mut sim, strange, HEALTHY, B3), [0], "{id:#x}");
        // The same without an attack -> 1.
        assert_eq!(check(&mut sim, strange_calm, HEALTHY, B3), [1], "{id:#x}");
        // A = 0xB3 fails for player 1, B passes for the carried player 0 -> 0.
        assert_eq!(check(&mut sim, strange, B3, HEALTHY), [0], "{id:#x}");
        assert_eq!(check(&mut sim, strange_calm, B3, HEALTHY), [1], "{id:#x}");
        // No 0xB3 filter: never a break, even if A passes, B fails and the object attacks.
        assert_eq!(check(&mut sim, strange, HEALTHY, MINE), [1], "{id:#x}");
        assert_eq!(check(&mut sim, strange, MINE, HEALTHY), [1], "{id:#x}");
        // Both pass -> no break.
        assert_eq!(check(&mut sim, own, HEALTHY, B3), [1], "{id:#x}");
        // A missing object: both queries leave `[2]` alone, the attack does not count.
        assert_eq!(check(&mut sim, 0, HEALTHY, B3), [1], "{id:#x}");
    }
    // Several objects: the second breaks after the first has passed both filters.
    let ids = [own, strange, own];
    let entries = ids_from_5(&ids);
    for id in PAIR {
        assert_eq!(
            run(&mut sim, id, 1, &entries, &[L0, 5, 3, B3, HEALTHY]),
            [0],
            "{id:#x}"
        );
        assert_eq!(
            run(&mut sim, id, 1, &entries, &[L0, 5, 1, B3, HEALTHY]),
            [1],
            "{id:#x}"
        );
    }
    // The attack state is read before the filter: a dead object no longer attacks.
    sim.object_mut(oid(strange)).unwrap().status |= status::DIED;
    for id in PAIR {
        let args = [L0, 5, 1, B3, HEALTHY];
        assert_eq!(run(&mut sim, id, 1, &ids_from_5(&[strange]), &args), [1]);
    }
}

#[test]
fn pair_asks_the_last_script_argument_first() {
    // Pops `A, B, n, start, list`: the first pop is the **last** script argument and is asked first (`0x004A3BE0`).
    // Script `f(list, start, n, B, A)`. An attacking object of slot 0, `me = 1`, filter "own" (5) and 0xB3 (here also
    // "own"), both player-dependent.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let strange = attacker(&mut sim, 0, (10, 10));
    for id in PAIR {
        let go = |sim: &mut Sim, script: [i32; 2]| {
            run(
                sim,
                id,
                1,
                &ids_from_5(&[strange]),
                &[L0, 5, 1, script[0], script[1]],
            )
        };
        // Script `(…, MINE, B3)`: A = 0xB3 for player 1 fails, B = MINE for player 0 passes, A is 0xB3 and B passes ->
        // break with 0.
        assert_eq!(go(&mut sim, [MINE, B3]), [0], "{id:#x}");
        // Script `(…, B3, MINE)`: A = MINE for player 1 fails, B = 0xB3 for player 0 passes; 0xB3 is B, but A did not
        // pass -> no break, result 1.
        assert_eq!(go(&mut sim, [B3, MINE]), [1], "{id:#x}");
    }
}

#[test]
fn pair_carries_the_result_of_b_to_the_next_a() {
    // The player of the next query A is the result of B of the previous object (not that of A): script `(…, MINE, B3)`
    // asks A = 0xB3, then B = MINE. Second object: attacking, slot 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let target = attacker(&mut sim, 0, (10, 10));
    // Slot 0, calm: ra = 0 (not player 1), rb = 1 (for player 0 "own") -> carry 1.
    let carry_one = put(&mut sim, WARRIOR, 0, (12, 10));
    // Slot 2, calm: ra = 0, rb = 0 -> carry 0.
    let carry_zero = put(&mut sim, WARRIOR, 2, (14, 10));
    for id in PAIR {
        let go = |sim: &mut Sim, first: i32| {
            let entries = ids_from_5(&[first, target]);
            run(sim, id, 1, &entries, &[L0, 5, 2, MINE, B3])
        };
        // Carry 1: A = 0xB3 for player 1 fails, B for player 0 passes -> break.
        assert_eq!(go(&mut sim, carry_one), [0], "{id:#x}");
        // Carry 0: A = 0xB3 passes for player 0, B fails for player 1 -> no break.
        // With the result of A (0) as carry, `carry_one` would give the same as here.
        assert_eq!(go(&mut sim, carry_zero), [1], "{id:#x}");
    }
}

#[test]
fn distance_list_counts_missing_ids_as_inside() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let near = put(&mut sim, WARRIOR, 1, (10, 10));
    let far = put(&mut sim, WARRIOR, 2, (13, 14));
    let gone = put(&mut sim, WARRIOR, 2, (30, 30));
    crate::world::destroy_object(&mut sim, oid(gone));
    for id in DISTANCE {
        let entries = ids_from_5(&[0, near, far, gone]);
        let go = |sim: &mut Sim, start: i32, n: i32, x: i32, z: i32, r: i32| {
            run(sim, id, 1, &entries, &[L0, start, n, x, z, r])
        };
        // Missing IDs (0, stale) count as inside, even with a negative radius.
        assert_eq!(go(&mut sim, 5, 1, 10, 10, -1), [1], "{id:#x}");
        assert_eq!(go(&mut sim, 8, 1, 0, 0, i32::MIN), [1], "{id:#x}");
        // `trunc(sqrt(dx² + dz²)) ≤ R`: (13, 14) is 5 away from (10, 10).
        assert_eq!(go(&mut sim, 5, 3, 10, 10, 4), [0], "{id:#x}");
        assert_eq!(go(&mut sim, 5, 3, 10, 10, 5), [1], "{id:#x}");
        // (10, 10) from (12, 13): d^2 = 13, truncated 3.
        assert_eq!(go(&mut sim, 5, 2, 12, 13, 3), [1], "{id:#x}");
        assert_eq!(go(&mut sim, 5, 2, 12, 13, 2), [0], "{id:#x}");
        // (1, 1): sqrt(2) truncated to 1.
        assert_eq!(go(&mut sim, 5, 2, 11, 11, 1), [1], "{id:#x}");
        assert_eq!(go(&mut sim, 5, 2, 11, 11, 0), [0], "{id:#x}");
        // A found object is always too far at a negative radius.
        assert_eq!(go(&mut sim, 5, 2, 10, 10, -1), [0], "{id:#x}");
        // A dead object also counts with its position.
        sim.object_mut(oid(far)).unwrap().status |= status::DIED;
        assert_eq!(go(&mut sim, 5, 3, 10, 10, 4), [0], "{id:#x}");
        sim.object_mut(oid(far)).unwrap().status &= !status::DIED;
        // Overflow: dx = 46341 gives dx^2 = 2^31 + ..., the sum turns negative -> the x87 computes with NaN and the
        // unit never counts as too far. With dx = 46340 the sum stays positive.
        assert_eq!(go(&mut sim, 6, 1, 10 - 46341, 10, 0), [1], "{id:#x}");
        assert_eq!(go(&mut sim, 6, 1, 10 - 46340, 10, 0), [0], "{id:#x}");
        // Extreme values do not crash.
        for v in [i32::MIN, i32::MAX, -1, 0, 1] {
            let _ = go(&mut sim, 6, 2, v, v, v);
        }
    }
}

/// Arguments of the builtin `id` for the list `list_addr` with `start` and `n`.
fn args_of(id: u16, list_addr: i32, start: i32, n: i32) -> Vec<i32> {
    if DISTANCE.contains(&id) {
        vec![list_addr, start, n, 10, 10, 3]
    } else if PAIR.contains(&id) {
        vec![list_addr, start, n, MINE, HEALTHY]
    } else {
        vec![list_addr, start, n, MINE]
    }
}

const EVERY: [u16; 8] = [0x112, 0x113, 0x114, 0x115, 0x117, 0x11E, 0x11F, 0x121];

#[test]
fn invalid_list_pushes_nothing_and_n_zero_pushes_zero() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for id in EVERY {
        // Pointer 0 (integer variable with 0): nothing, without an anomaly, also at n = 0.
        for n in [0, 1, 5] {
            let args = args_of(id, int(0) as i32, 0, n);
            assert_eq!(run(&mut sim, id, 3, &[], &args), [], "{id:#x}, n = {n}");
        }
        // Pointer without a list: for n not in (0, 100) first the 0 (without list access), otherwise nothing and
        // `Guarded` (an access into the void in the original).
        for n in [0, 100, 1, 5] {
            let args = args_of(id, int(1) as i32, 0, n);
            let mut stack = vec![FLOOR];
            stack.extend(args);
            let mut t = thread(3, &stack, &[]);
            t.vars.set(int(1), 12_345);
            let mut w = SimKiWorld::new(&mut sim, Sight::Omniscient);
            w.builtin(id, &mut BuiltinCx::new(3, &mut t));
            if (1..100).contains(&n) {
                assert!(
                    matches!(w.notes[..], [KiNote::Guarded { builtin, .. }] if builtin == id),
                    "{id:#x}: {:?}",
                    w.notes
                );
                assert_eq!(t.stack, [FLOOR], "{id:#x}, n = {n}");
            } else {
                assert!(w.notes.is_empty(), "{id:#x}: {:?}", w.notes);
                assert_eq!(t.stack, [FLOOR, 0], "{id:#x}, n = {n}");
            }
        }
        // n not in (0, 100): result 0, without touching a list.
        for n in [0, 100, 101, -1, i32::MIN, i32::MAX] {
            let args = args_of(id, L0, 5, n);
            let (t, notes) = call_on(&mut sim, id, 3, &[], &args);
            assert!(notes.is_empty());
            assert_eq!(t.stack, [FLOOR, 0], "{id:#x}, n = {n}");
            assert_eq!(list_len(&t), 0, "{id:#x}, n = {n}: nothing created");
        }
    }
}

#[test]
fn reading_past_the_list_end_creates_entries() {
    // E-32: every key read is created with 0 (`FUN_004E7B30`); ID 0 is missing and leaves `[2]`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mine = put(&mut sim, WARRIOR, 3, (10, 10));
    let other = put(&mut sim, WARRIOR, 2, (12, 10));
    for id in EVERY {
        // An empty list, n = 5 from 7: "all" and "distance" run through (everything is missing), "any" and "pair" with
        // `me = 3` too (missing object = hit for "any": only the first object).
        let args = args_of(id, L0, 7, 5);
        let (t, notes) = call_on(&mut sim, id, 3, &[], &args);
        assert!(notes.is_empty());
        let want = if ANY.contains(&id) { 1 } else { 5 };
        assert_eq!(list_len(&t), want, "{id:#x}");
        for k in 7..7 + want as i32 {
            assert_eq!(t.vars.list_at(L0).unwrap().get(k), Some(&0), "{id:#x}");
        }
        // The key range runs past the end of `i32` (`start + i` wraps).
        let args = args_of(id, L0, i32::MAX - 1, 4);
        let (t, _) = call_on(&mut sim, id, 3, &[], &args);
        let keys: Vec<i32> = t.vars.list_at(L0).unwrap().iter().map(|(k, _)| k).collect();
        let mut want_keys = vec![i32::MAX - 1, i32::MAX, i32::MIN, i32::MIN + 1];
        want_keys.truncate(if ANY.contains(&id) { 1 } else { 4 });
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        want_keys.sort_unstable();
        assert_eq!(sorted, want_keys, "{id:#x}");
    }
    // The break stops the reading: later keys are not created.
    for id in ALL {
        let (t, _) = call_on(&mut sim, id, 3, &[(7, other)], &[L0, 7, 5, MINE]);
        assert_eq!(list_len(&t), 1, "{id:#x}: fails at the first object");
    }
    for id in ANY {
        let (t, _) = call_on(&mut sim, id, 3, &[(7, mine)], &[L0, 7, 5, MINE]);
        assert_eq!(list_len(&t), 1, "{id:#x}: hits at the first object");
    }
    // A key that already has a value stays unchanged.
    let (t, _) = call_on(&mut sim, 0x113, 3, &[(8, mine)], &[L0, 7, 3, MINE]);
    assert_eq!(t.vars.list_at(L0).unwrap().get(8), Some(&mine));
}

#[test]
fn list_queries_leave_the_checksum_unchanged() {
    // Queries do not change the game state; the lists of the AI live in the thread.
    let (rules, mut state) = scene();
    let ids = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = put(&mut sim, WARRIOR, 1, (10, 10));
        let b = attacker(&mut sim, 0, (12, 10));
        let dead = put(&mut sim, WARRIOR, 2, (14, 10));
        sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
        let gone = put(&mut sim, WARRIOR, 2, (16, 10));
        crate::world::destroy_object(&mut sim, oid(gone));
        [a, b, dead, gone, 0, -1, i32::MIN, i32::MAX]
    };
    let (before, events) = (checksum(&state), state.events.clone());
    let mut sim = Sim::new(&mut state, &rules);
    let entries = ids_from_5(&ids);
    let filters = [MINE, 4, CORPSE, HEALTHY, B3, 0, -5, 999, 1000, i32::MAX];
    for id in EVERY {
        for me in [0u8, 1, 3, 7] {
            for (start, n) in [
                (5, 1),
                (5, 8),
                (0, 99),
                (i32::MAX - 2, 5),
                (i32::MIN, 2),
                (5, 0),
            ] {
                for &f in &filters {
                    for g in [f, B3] {
                        let mut args = args_of(id, L0, start, n);
                        if DISTANCE.contains(&id) {
                            args[3..].copy_from_slice(&[f, g, f.wrapping_add(g)]);
                        } else {
                            args[3] = f;
                            if PAIR.contains(&id) {
                                args[4] = g;
                            }
                        }
                        let (t, _) = call_on(&mut sim, id, me, &entries, &args);
                        assert_eq!(t.stack.first(), Some(&FLOOR));
                    }
                }
            }
        }
    }
    // The facade itself stays read-only too.
    for code in [0x4E2C, 0x4E28, 0x4FB5] {
        let _ = query_once(&mut sim, 1, Sight::Fog, code, &[1, ids[0], MINE, 0]);
    }
    assert_eq!(checksum(&state), before);
    assert_eq!(state.events, events);
}

#[test]
fn list_builtins_are_world_queries() {
    // The real world answers all eight IDs itself (not the substitute by signature).
    for id in EVERY {
        assert!(super::is_query(id), "{id:#x}");
    }
}
