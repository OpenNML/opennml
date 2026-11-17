//! Tests of the AI command layer and of the commands for movement, economy and construction (re/ki-queries 7.1/7.2,
//! E-89 ... E-91).
//!
//! Every row is called against the real world ([`SimKiWorld::builtin`]); checked are the stack, outputs, the
//! `KiCommand` event and the effect: the same state as after the expected [`CommandKind`] via `submit` with
//! [`Origin::Ki`] (same checksum, same outcome).

use std::collections::BTreeSet;

use super::tests_abilities::ability_ids;
use super::tests_control::control_ids;
use super::{ROWS, is_command, row};
use crate::command::testkit::{
    BOAT, COLONIST, FARM, HOUSE, HQ, MINE, SOIL, SOIL_TYPE, TREE, TYPES, account, fogged_state,
    rules, rules_from, site, spawn, state,
};
use crate::command::{Command, CommandKind, F32Bits, Origin, Outcome, TrainOp};
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::ki::builtins::{has_case, is_local};
use crate::ki::query::{SimKiWorld, is_query};
use crate::ki::signature::signature;
use crate::ki::stub::StubWorld;
use crate::ki::testkit::{catalog, int, list, proto, word};
use crate::ki::vm::{Thread, op};
use crate::ki::world::{BuiltinCx, KiNote, KiWorld};
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::placement::{find_building_placement_near, worker_layer_region};
use crate::player::{Sight, submit};
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};

/// Slot and `m_iSpieler` of the test instance.
pub(super) const ME: u8 = 1;
pub(super) const ME_I: i32 = ME as i32;
/// Bottom stack value: shows that a builtin does not pop more than intended.
pub(super) const FLOOR: i32 = -7;

/// A reported AI command: slot, network command, outcome.
pub(super) type Issued = (u8, u32, Outcome);

/// Thread 2 of module 7 with the integer variables 0-7 and the lists `list(0)`, `list(1)`.
pub(super) fn thread(stack: &[i32]) -> Thread {
    let ranges = [(0, 8), (list(0), 2)];
    Thread::from_prototype(&proto(7, &ranges, &[], &[]), 2, ME, stack.to_vec())
}

/// [`thread`] with `args` over [`FLOOR`].
pub(super) fn thread_with(args: &[i32]) -> Thread {
    let mut stack = vec![FLOOR];
    stack.extend_from_slice(args);
    thread(&stack)
}

/// The `KiCommand` events of the state (drained).
pub(super) fn ki_commands(sim: &mut Sim) -> Vec<Issued> {
    sim.state
        .events
        .drain(..)
        .filter_map(|e| match e {
            SimEvent::KiCommand {
                player,
                code,
                outcome,
            } => Some((player, code, outcome)),
            _ => None,
        })
        .collect()
}

/// Builtin `id` on thread `t` against the real world (omniscient): notes and AI commands.
pub(super) fn run_on(sim: &mut Sim, id: u16, t: &mut Thread) -> (Vec<KiNote>, Vec<Issued>) {
    let notes = {
        let mut w = SimKiWorld::new(sim, Sight::Omniscient);
        w.builtin(id, &mut BuiltinCx::new(ME, t));
        w.notes
    };
    (notes, ki_commands(sim))
}

/// Builtin `id` with the arguments `args`: the stack over [`FLOOR`] afterwards and the AI commands; no note, not popped
/// too much.
pub(super) fn run(sim: &mut Sim, id: u16, args: &[i32]) -> (Vec<i32>, Vec<Issued>) {
    let mut t = thread_with(args);
    let (notes, issued) = run_on(sim, id, &mut t);
    assert!(notes.is_empty(), "{id:#x}: {notes:?}");
    assert_eq!(t.stack.first(), Some(&FLOOR), "{id:#x} pops too much");
    (t.stack[1..].to_vec(), issued)
}

/// Builtin `id` on thread `t` over a copy of `base` gives exactly the command `kind`: one `KiCommand` with its type
/// code and the outcome that `kind` has via `submit` with [`Origin::Ki`] on a second copy, and the same effect
/// (checksum). Returns thread, outcome and state.
pub(super) fn issues_on(
    rules: &Rules,
    base: &SimState,
    id: u16,
    mut t: Thread,
    kind: CommandKind,
) -> (Thread, Outcome, SimState) {
    let mut a = base.clone();
    let issued = {
        let mut sim = Sim::new(&mut a, rules);
        let (notes, issued) = run_on(&mut sim, id, &mut t);
        assert!(notes.is_empty(), "{id:#x}: {notes:?}");
        issued
    };
    let mut b = base.clone();
    let code = kind.type_code() & 0xFF;
    let outcome = submit(
        &mut Sim::new(&mut b, rules),
        &Command { player: ME, kind },
        Origin::Ki,
    );
    assert_eq!(issued, [(ME, code, outcome.clone())], "{id:#x}");
    assert_eq!(checksum(&a), checksum(&b), "{id:#x}: effect");
    (t, outcome, a)
}

/// [`issues_on`] with `args` over [`FLOOR`]; returns the stack over [`FLOOR`] and the outcome.
pub(super) fn issues(
    rules: &Rules,
    base: &SimState,
    id: u16,
    args: &[i32],
    kind: CommandKind,
) -> (Vec<i32>, Outcome) {
    let (t, outcome, _) = issues_on(rules, base, id, thread_with(args), kind);
    assert_eq!(t.stack.first(), Some(&FLOOR), "{id:#x} pops too much");
    (t.stack[1..].to_vec(), outcome)
}

/// `Move` as the AI builds it: target `Fixed(x), Fixed(z)`, target radius -1, no direction, spread `Fixed(1)`.
fn walk(units: Vec<ObjectId>, x: i32, z: i32, formation: i32, flags: i32) -> CommandKind {
    CommandKind::Move {
        units,
        x: x << 12,
        z: z << 12,
        target: -1,
        formation,
        direction: NO_DIRECTION,
        spread: 4096,
        flags,
    }
}

pub(super) fn kill(sim: &mut Sim, id: ObjectId) {
    sim.object_mut(id).unwrap().status |= status::DIED;
}

fn objects_of(state: &SimState, guid: u32) -> Vec<ObjectId> {
    state
        .objects()
        .filter(|o| o.guid == guid)
        .map(|o| o.id)
        .collect()
}

#[test]
fn rows_are_sorted_and_disjoint_from_queries() {
    for pair in ROWS.windows(2) {
        assert!(
            pair[0].hi < pair[1].lo,
            "{:#x} before {:#x}",
            pair[0].hi,
            pair[1].lo
        );
    }
    for r in ROWS {
        assert!(r.lo <= r.hi, "{:#x}", r.lo);
        for id in r.lo..=r.hi {
            let id32 = u32::from(id);
            assert!(
                has_case(id32) && !is_local(id32),
                "{id:#x} is not a world builtin"
            );
            assert!(!is_query(id), "{id:#x} is also a query");
            assert!(is_command(id));
            assert_eq!(row(id).map(|x| (x.lo, x.hi)), Some((r.lo, r.hi)));
        }
        for &a in r.outs {
            assert!(a < r.pops, "{:#x}: output a{}", r.lo, a + 1);
        }
    }
    assert!(!is_command(0) && !is_command(u16::MAX));
}

#[test]
fn command_rows_match_the_e1a_signatures() {
    // Where a signature is known (corpus IDs), the command row has the same pops, push, output
    // positions, pause and the same query code.
    let mut compared = 0;
    let mut ids = 0;
    for r in ROWS {
        for id in r.lo..=r.hi {
            ids += 1;
            let Some(sig) = signature(id) else { continue };
            compared += 1;
            let outs: BTreeSet<u8> = r.outs.iter().copied().collect();
            let sig_outs: BTreeSet<u8> = sig.outs.iter().copied().collect();
            assert_eq!(
                (r.pops, r.push, outs, r.pause_ms, r.code),
                (sig.pops, sig.push, sig_outs, sig.pause_ms, sig.code),
                "{id:#x}"
            );
        }
    }
    // The movement group has 39 IDs, 32 of them in the corpus (without signature: 0xC9, 0xFC, 0x104, 0x130, 0x15D,
    // 0x176, 0x19A); the control group 28 IDs, 14 of them in the corpus (`tests_control`); the abilities group 31 IDs,
    // all in the corpus (`tests_abilities`).
    assert_eq!((ids, compared), (39 + 28 + 31, 32 + 14 + 31));
}

/// Scene: two colonists of [`ME`], a dead one, a foreign one; wood and gold plentiful.
fn unit_scene() -> (Rules, SimState, [ObjectId; 4]) {
    let rules = rules();
    let mut base = state(&rules);
    let ids = {
        let mut sim = Sim::new(&mut base, &rules);
        let a = spawn(&mut sim, COLONIST, ME_I, (10, 10));
        let b = spawn(&mut sim, COLONIST, ME_I, (11, 10));
        let dead = spawn(&mut sim, COLONIST, ME_I, (12, 10));
        kill(&mut sim, dead);
        let foreign = spawn(&mut sim, COLONIST, 0, (13, 10));
        [a, b, dead, foreign]
    };
    (rules, base, ids)
}

#[test]
fn single_move_skips_dead_units_and_sends_flags() {
    let (rules, base, [a, _, dead, _]) = unit_scene();
    // Pattern C: flags 2/0 (checked), 3/1 (unchecked), each with alias.
    for (id, flags) in [
        (0xF5, 2),
        (0xF6, 0),
        (0xF9, 2),
        (0xFA, 0),
        (0xFD, 3),
        (0xFE, 1),
        (0x101, 3),
        (0x102, 1),
    ] {
        let (stack, outcome) = issues(
            &rules,
            &base,
            id,
            &[a.as_i32(), 20, 22],
            walk(vec![a], 20, 22, 0, flags),
        );
        assert!(stack.is_empty(), "{id:#x}");
        assert_eq!(outcome, Outcome::Accepted, "{id:#x}");
    }
    // The flags take effect (other FINDTO flags): flags 2 and 0 give different states.
    let after = |flags| {
        let mut s = base.clone();
        let cmd = Command {
            player: ME,
            kind: walk(vec![a], 20, 22, 0, flags),
        };
        submit(&mut Sim::new(&mut s, &rules), &cmd, Origin::Ki);
        checksum(&s)
    };
    assert_ne!(after(2), after(0));
    // Dead or missing: the checked forms send nothing, the unchecked ones a command that the simulation rejects
    // (exception A8).
    let mut s = base.clone();
    let mut sim = Sim::new(&mut s, &rules);
    let gone = spawn(&mut sim, COLONIST, ME_I, (14, 10));
    crate::world::destroy_object(&mut sim, gone);
    let before = checksum(sim.state);
    for u in [dead, gone, ObjectId::NONE] {
        for id in [0xF5, 0xF6, 0xF9, 0xFA] {
            assert_eq!(run(&mut sim, id, &[u.as_i32(), 20, 22]), (vec![], vec![]));
        }
        for id in [0xFD, 0xFE, 0x101, 0x102] {
            let (stack, issued) = run(&mut sim, id, &[u.as_i32(), 20, 22]);
            assert!(stack.is_empty());
            assert!(
                matches!(issued[..], [(ME, 4, Outcome::Rejected(_))]),
                "{id:#x}: {issued:?}"
            );
        }
    }
    assert_eq!(checksum(sim.state), before);
}

// QUIRK E-26: deviation
#[test]
fn foreign_actor_is_rejected_and_reported() {
    let (rules, base, [_, _, _, foreign]) = unit_scene();
    let mut s = base.clone();
    let mut sim = Sim::new(&mut s, &rules);
    let before = checksum(sim.state);
    let (stack, issued) = run(&mut sim, 0xF5, &[foreign.as_i32(), 20, 22]);
    assert!(stack.is_empty());
    match &issued[..] {
        [(ME, 4, Outcome::Rejected(why))] => {
            assert!(why.contains(&foreign.0.to_string()), "{why}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(checksum(sim.state), before);
}

/// Thread with the list `list(0)` = `entries` (key, value) and the stack `args` over [`FLOOR`].
pub(super) fn list_thread(entries: &[(i32, i32)], args: &[i32]) -> Thread {
    let mut t = thread_with(args);
    let l0 = list(0) as i32;
    for &(k, v) in entries {
        t.vars.list_at_mut(l0).unwrap().insert(k, v);
    }
    t
}

// QUIRK E-90: kept
#[test]
fn group_move_filters_and_still_sends_an_empty_move() {
    let (rules, mut base, [a, b, dead, foreign]) = unit_scene();
    let gone = {
        let mut sim = Sim::new(&mut base, &rules);
        let gone = spawn(&mut sim, COLONIST, ME_I, (14, 10));
        crate::world::destroy_object(&mut sim, gone);
        gone
    };
    let l0 = list(0) as i32;
    // List 5...10: a, dead, missing, a twice, b, foreign -> Move with a, b, foreign in list order (the foreign one is
    // not filtered until the simulation: partial effect); formation 3 raw.
    let entries: Vec<(i32, i32)> = [a, dead, gone, a, b, foreign]
        .iter()
        .enumerate()
        .map(|(i, u)| (5 + i as i32, u.as_i32()))
        .collect();
    for (id, flags) in [
        (0xF7, 2),
        (0xF8, 0),
        (0xFB, 2),
        (0xFC, 0),
        (0xFF, 3),
        (0x100, 1),
        (0x103, 3),
        (0x104, 1),
    ] {
        let t = list_thread(&entries, &[l0, 5, 6, 30, 31, 3]);
        let kind = walk(vec![a, b, foreign], 30, 31, 3, flags);
        let (t, outcome, _) = issues_on(&rules, &base, id, t, kind);
        assert_eq!(t.stack, [FLOOR], "{id:#x}");
        assert!(
            matches!(outcome, Outcome::Partial(_)),
            "{id:#x}: {outcome:?}"
        );
    }
    // Only dead and missing: still a Move without units (E-90), without effect.
    let t = list_thread(
        &[(0, dead.as_i32()), (1, gone.as_i32())],
        &[l0, 0, 2, 30, 31, 0],
    );
    let (_, outcome, after) = issues_on(&rules, &base, 0xF7, t, walk(vec![], 30, 31, 0, 2));
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(checksum(&after), checksum(&base));
    // `var(&obj) = 0` or n not in (0, 100): nothing, not even creating. Pointer without a list: `Guarded`.
    let mut s = base.clone();
    let mut sim = Sim::new(&mut s, &rules);
    for (obj, n) in [(int(3) as i32, 2), (l0, 0), (l0, 100), (l0, -1)] {
        let mut t = list_thread(&entries, &[obj, 5, n, 30, 31, 0]);
        assert_eq!(run_on(&mut sim, 0xF7, &mut t), (vec![], vec![]), "n = {n}");
        assert_eq!(t.stack, [FLOOR]);
        assert_eq!(t.vars.list_at(l0).unwrap().len(), entries.len());
    }
    let mut t = thread_with(&[int(3) as i32, 0, 2, 30, 31, 0]);
    t.vars.set(int(3), 12_345);
    let (notes, issued) = run_on(&mut sim, 0xF7, &mut t);
    assert!(matches!(notes[..], [KiNote::Guarded { builtin: 0xF7, .. }]));
    assert!(issued.is_empty());
    assert_eq!(t.stack, [FLOOR]);
}

// QUIRK E-90, E-107: kept
#[test]
fn group_move_reads_missing_list_entries_as_zero() {
    let (rules, base, [a, ..]) = unit_scene();
    let l0 = list(0) as i32;
    // Only key 5 set; n = 3 reads 5, 6, 7 and creates 6 and 7 with 0 (`FUN_004E7B30`).
    let t = list_thread(&[(5, a.as_i32())], &[l0, 5, 3, 30, 31, 0]);
    let (t, outcome, _) = issues_on(&rules, &base, 0xF7, t, walk(vec![a], 30, 31, 0, 2));
    assert_eq!(outcome, Outcome::Accepted);
    let l = t.vars.list_at(l0).unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!((l.get(6), l.get(7)), (Some(&0), Some(&0)));
}

/// Economy scene: colonist `u` of [`ME`], headquarters, construction site of a house, finca, gold mine, fishing boat, a
/// tree; account full.
struct Economy {
    rules: Rules,
    base: SimState,
    u: ObjectId,
    hq: ObjectId,
    house: ObjectId,
    farm: ObjectId,
    mine: ObjectId,
    boat: ObjectId,
    tree: ObjectId,
}

fn economy() -> Economy {
    let rules = rules();
    let mut base = state(&rules);
    let mut sim = Sim::new(&mut base, &rules);
    let u = spawn(&mut sim, COLONIST, ME_I, (10, 10));
    let hq = spawn(&mut sim, HQ, ME_I, (20, 20));
    let house = site(&mut sim, HOUSE, ME_I, (30, 30));
    let farm = spawn(&mut sim, FARM, ME_I, (40, 10));
    let mine = spawn(&mut sim, MINE, ME_I, (40, 30));
    let boat = spawn(&mut sim, BOAT, ME_I, (50, 50));
    let tree = spawn(&mut sim, TREE, 10, (14, 12));
    account(&mut sim, usize::from(ME), [5000, 5000, 5000, 0, 50]);
    Economy {
        rules,
        base,
        u,
        hq,
        house,
        farm,
        mine,
        boat,
        tree,
    }
}

// QUIRK E-91: deviation
#[test]
fn production_pushes_hundred() {
    let e = economy();
    let (rules, hq) = (&e.rules, e.hq);
    let kind = CommandKind::EnqueueProduction {
        building: hq,
        guid: COLONIST,
    };
    let (t, outcome, after) = issues_on(
        rules,
        &e.base,
        0x12D,
        thread_with(&[hq.as_i32(), COLONIST as i32]),
        kind,
    );
    assert_eq!(t.stack, [FLOOR, 100]);
    assert_eq!(outcome, Outcome::Accepted);
    // `0x130` cancels the running order (`{bld, 0}`, `arg` 0, E-91).
    let cancel = CommandKind::CancelProduction {
        building: hq,
        index: 0,
        arg: 0,
    };
    let (stack, outcome) = issues(rules, &after, 0x130, &[hq.as_i32()], cancel);
    assert!(stack.is_empty());
    assert_eq!(outcome, Outcome::Accepted);
    // Foreign or missing building: still push 100, the command is rejected.
    let mut s = e.base.clone();
    let mut sim = Sim::new(&mut s, rules);
    let (stack, issued) = run(&mut sim, 0x12D, &[-1, COLONIST as i32]);
    assert_eq!(stack, [100]);
    assert!(matches!(issued[..], [(ME, 0x0E, Outcome::Rejected(_))]));
}

#[test]
fn tree_search_writes_the_tile_and_chops() {
    let e = economy();
    let (rules, u) = (&e.rules, e.u);
    let far = {
        let mut b = e.base.clone();
        let far = spawn(&mut Sim::new(&mut b, rules), TREE, 10, (40, 50));
        (b, far)
    };
    let (base, _) = far;
    // Nearest tree to the tile corner (10, 10) is the one at (14, 12); outputs = its tile.
    let (x, z) = (int(5) as i32, int(6) as i32);
    let kind = CommandKind::ChopWood {
        tree: e.tree,
        units: vec![u],
    };
    let (t, outcome, _) = issues_on(
        rules,
        &base,
        0x193,
        thread_with(&[u.as_i32(), 10, 10, x, z]),
        kind,
    );
    assert_eq!(t.stack, [FLOOR, 1]);
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!((t.vars.get(int(5)), t.vars.get(int(6))), (14, 12));
    // Without a tree: push 0, the output variables get their own address, no command.
    let mut s = e.base.clone();
    let mut sim = Sim::new(&mut s, rules);
    crate::world::destroy_object(&mut sim, e.tree);
    let mut t = thread_with(&[u.as_i32(), 10, 10, x, z]);
    assert_eq!(run_on(&mut sim, 0x193, &mut t), (vec![], vec![]));
    assert_eq!(t.stack, [FLOOR, 0]);
    assert_eq!((t.vars.get(int(5)), t.vars.get(int(6))), (x, z));
}

#[test]
fn unquarter_minus_one_unquarters_all() {
    let e = economy();
    let (rules, u, hq) = (&e.rules, e.u, e.hq);
    let all = CommandKind::UnquarterAll { container: hq };
    let (stack, outcome) = issues(rules, &e.base, 0x176, &[hq.as_i32(), -1], all.clone());
    assert!(stack.is_empty());
    assert_eq!(outcome, Outcome::Accepted);
    let one = CommandKind::Unquarter {
        container: hq,
        unit: u,
    };
    issues(rules, &e.base, 0x176, &[hq.as_i32(), u.as_i32()], one);
    // `0x23A` always all.
    let (_, outcome) = issues(rules, &e.base, 0x23A, &[hq.as_i32()], all);
    assert_eq!(outcome, Outcome::Accepted);
}

#[test]
fn every_unit_row_gives_its_command() {
    // The remaining rows: word layout from re/ki-queries 7.2, push like the row.
    let e = economy();
    let (u, hq, boat) = (e.u, e.hq, e.boat);
    let i = ObjectId::as_i32;
    let rows: Vec<(u16, Vec<i32>, CommandKind, Vec<i32>)> = vec![
        (
            0xB4,
            vec![i(u), i(e.house)],
            CommandKind::ConstructAt {
                building: e.house,
                units: vec![u],
            },
            vec![],
        ),
        (
            0xC8,
            vec![i(u), i(boat)],
            CommandKind::Mount {
                horse: boat,
                units: vec![u],
            },
            vec![0],
        ),
        (
            0xC9,
            vec![i(u)],
            CommandKind::Dismount { units: vec![u] },
            vec![0],
        ),
        (
            0x12E,
            vec![i(hq), 25, 26],
            CommandKind::SetRallyPoint {
                x: 25 << 12,
                z: 26 << 12,
                buildings: vec![hq],
            },
            vec![],
        ),
        (
            0x15D,
            vec![i(hq)],
            CommandKind::Train {
                train: hq,
                op: TrainOp::Stop,
                target: None,
            },
            vec![ME_I],
        ),
        (
            0x167,
            vec![i(hq)],
            CommandKind::Train {
                train: hq,
                op: TrainOp::Forward,
                target: None,
            },
            vec![],
        ),
        (
            0x168,
            vec![i(hq)],
            CommandKind::Train {
                train: hq,
                op: TrainOp::Backward,
                target: None,
            },
            vec![],
        ),
        (
            0x172,
            vec![i(u), i(hq)],
            CommandKind::Quarter {
                container: hq,
                units: vec![u],
            },
            vec![],
        ),
        (
            0x173,
            vec![i(u), i(hq)],
            CommandKind::Quarter {
                container: hq,
                units: vec![u],
            },
            vec![],
        ),
        (
            0x174,
            vec![i(u), i(boat)],
            CommandKind::BoardBoat {
                boat,
                units: vec![u],
            },
            vec![],
        ),
        (
            0x178,
            vec![i(u)],
            CommandKind::LeaveBoat { units: vec![u] },
            vec![],
        ),
        (
            0x191,
            vec![i(u)],
            CommandKind::Halt { units: vec![u] },
            vec![],
        ),
        (
            0x19A,
            vec![i(u)],
            CommandKind::Halt { units: vec![u] },
            vec![],
        ),
        (
            0x192,
            vec![i(u), i(e.farm)],
            CommandKind::Harvest {
                farm: e.farm,
                units: vec![u],
            },
            vec![0],
        ),
        (
            0x194,
            vec![i(u), i(e.tree)],
            CommandKind::Hunt {
                animal: e.tree,
                units: vec![u],
                gut: false,
            },
            vec![100],
        ),
        (
            0x195,
            vec![i(u), i(e.mine)],
            CommandKind::MineGold {
                mine: e.mine,
                units: vec![u],
            },
            vec![1],
        ),
        (
            0x231,
            vec![i(hq), 3350],
            CommandKind::CounterUpgrade {
                guid: 3350,
                source: hq,
            },
            vec![],
        ),
    ];
    let mut covered = BTreeSet::new();
    for (id, args, kind, push) in rows {
        let (stack, outcome) = issues(&e.rules, &e.base, id, &args, kind);
        assert_eq!(stack, push, "{id:#x}");
        eprintln!("{id:#x}: {outcome:?}");
        covered.insert(id);
    }
    // The unit commands with a suitable target take effect in this scene.
    for (id, args) in [
        (0xB4, vec![i(u), i(e.house)]),
        (0x191, vec![i(u)]),
        (0x192, vec![i(u), i(e.farm)]),
        (0x195, vec![i(u), i(e.mine)]),
        (0x12E, vec![i(hq), 25, 26]),
    ] {
        let mut s = e.base.clone();
        let (_, issued) = run(&mut Sim::new(&mut s, &e.rules), id, &args);
        assert!(
            matches!(issued[..], [(ME, _, Outcome::Accepted)]),
            "{id:#x}: {issued:?}"
        );
    }
    // Together with the other tests, every row of this group is covered.
    let elsewhere = [
        0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0xFE, 0xFF, 0x100, 0x101, 0x102,
        0x103, 0x104, 0x12C, 0x12D, 0x130, 0x176, 0x193, 0x23A,
    ];
    covered.extend(elsewhere);
    let all: BTreeSet<u16> = ROWS
        .iter()
        .flat_map(|r| r.lo..=r.hi)
        .filter(|id| !control_ids().contains(id) && !ability_ids().contains(id))
        .collect();
    assert_eq!(covered, all);
}

/// Account, worker of [`ME`] at (10, 10) and fertile soil at (20, 20) on a slope (height 37, like an object on the
/// terrain of a real map): the finca snaps in the check on the soil.
fn field_scene() -> (Rules, SimState, ObjectId) {
    let rules = rules_from(&format!("{TYPES}{SOIL_TYPE}"));
    let mut base = state(&rules);
    let w = {
        let mut sim = Sim::new(&mut base, &rules);
        let w = spawn(&mut sim, COLONIST, ME_I, (10, 10));
        let soil = spawn(&mut sim, SOIL, 10, (20, 20));
        sim.object_mut(soil).unwrap().world_pos[1] = 37.0;
        account(&mut sim, usize::from(ME), [0, 5000, 0, 0, 10]);
        w
    };
    (rules, base, w)
}

/// `0x01` like the AI: rotation 0, height 0, one worker, extra word `border`.
fn ki_place(guid: u32, (fx, fz): (f32, f32), worker: ObjectId, border: i32) -> CommandKind {
    CommandKind::PlaceBuilding {
        guid,
        rotation: F32Bits::new(0.0),
        x: F32Bits::new(fx),
        y: F32Bits::new(0.0),
        z: F32Bits::new(fz),
        workers: vec![worker],
        border: Some(border),
    }
}

/// Position of the building site search starting from tile `tile` with layer and region of the worker (on this map the
/// region of its own tile, the first of the five checked).
fn searched(rules: &Rules, base: &SimState, guid: u32, w: ObjectId, tile: i32) -> (f32, f32) {
    let mut s = base.clone();
    let mut sim = Sim::new(&mut s, rules);
    let (layer, region) = worker_layer_region(&mut sim, w);
    let (mut fx, mut fz) = ((tile * 128) as f32, (tile * 128) as f32);
    assert!(find_building_placement_near(
        &mut sim, &mut fx, &mut fz, guid, ME_I, layer, region
    ));
    (fx, fz)
}

// QUIRK D-288, E-36: kept
// QUIRK E-89: deviation
#[test]
fn ki_places_a_building_like_the_original() {
    // Position from the building site search, rotation 0, height 0.
    let (rules, base, w) = field_scene();
    let (x, z) = (int(5) as i32, int(6) as i32);
    let args = [w.as_i32(), FARM as i32, 20, 20, x, z, 0];
    let (fx, fz) = searched(&rules, &base, FARM, w, 20);
    let kind = ki_place(FARM, (fx, fz), w, 0);
    let (t, outcome, after) = issues_on(&rules, &base, 0x12C, thread_with(&args), kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(t.stack, [FLOOR, 1]);
    assert_eq!(t.pause_ms, 500);
    // Output tile `trunc(f64(fx) / cell)`.
    let tile = |v: f32| (f64::from(v) / 128.0).trunc() as i32;
    assert_eq!(
        (t.vars.get(int(5)), t.vars.get(int(6))),
        (tile(fx), tile(fz))
    );
    // The construction site stands at the position of the search, with rotation 0.
    let farms = objects_of(&after, FARM);
    assert_eq!(farms.len(), 1);
    let o = after.object(farms[0], false).unwrap();
    let grid = |v: f32| Fx(Fx::from_f32(v).0 / 128);
    assert_eq!((o.grid.x, o.grid.z), (grid(fx), grid(fz)));
    assert_eq!(o.rotation[1], 0.0);
    // The same command from outside: the snapped position of the preview would lie at the height of the ground
    // (exception A8, `check_site`) -> rejected, nothing built.
    let mut s = base.clone();
    let external = submit(
        &mut Sim::new(&mut s, &rules),
        &Command {
            player: ME,
            kind: kind.clone(),
        },
        Origin::External,
    );
    match external {
        Outcome::Rejected(why) => assert!(why.contains("snapped"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(objects_of(&s, FARM).is_empty());
    // On the AI check path `check_pose` remains as well: a non-finite position is rejected.
    let mut s = base.clone();
    let nan = submit(
        &mut Sim::new(&mut s, &rules),
        &Command {
            player: ME,
            kind: ki_place(FARM, (f32::NAN, fz), w, 0),
        },
        Origin::Ki,
    );
    assert!(
        matches!(&nan, Outcome::Rejected(why) if why.contains("not finite")),
        "{nan:?}"
    );
    // Worker missing (E-89): search without region binding, the ID bits point to `me` -> `0x01` is sent (push 1), the
    // simulation rejects it (not an own worker).
    let mut s = base.clone();
    let mut sim = Sim::new(&mut s, &rules);
    crate::world::destroy_object(&mut sim, w);
    let (stack, issued) = run(&mut sim, 0x12C, &args);
    assert_eq!(stack, [1]);
    assert!(
        matches!(issued[..], [(ME, 1, Outcome::Rejected(_))]),
        "{issued:?}"
    );
    assert!(objects_of(sim.state, FARM).is_empty());
    // Foreign worker: ID bits != `me` -> no command, push 0, outputs = own addresses.
    let mut s = base.clone();
    let mut sim = Sim::new(&mut s, &rules);
    let foreign = spawn(&mut sim, COLONIST, 0, (12, 10));
    let mut t = thread_with(&[foreign.as_i32(), FARM as i32, 20, 20, x, z, 0]);
    assert_eq!(run_on(&mut sim, 0x12C, &mut t), (vec![], vec![]));
    assert_eq!(t.stack, [FLOOR, 0]);
    assert_eq!((t.vars.get(int(5)), t.vars.get(int(6))), (x, z));
    assert_eq!(t.pause_ms, 500);
}

#[test]
fn ki_builds_on_land_it_has_not_seen() {
    // The sight (`check_site_fog`) is a UI check; the AI also builds in the fog, the same command from outside is
    // rejected.
    let rules = rules();
    let mut base = fogged_state(&rules);
    let w = {
        let mut sim = Sim::new(&mut base, &rules);
        let w = spawn(&mut sim, COLONIST, ME_I, (10, 10));
        account(&mut sim, usize::from(ME), [0, 5000, 0, 0, 10]);
        w
    };
    let (x, z) = (int(5) as i32, int(6) as i32);
    let pos = searched(&rules, &base, HOUSE, w, 40);
    let kind = ki_place(HOUSE, pos, w, 2);
    let args = [w.as_i32(), HOUSE as i32, 40, 40, x, z, 2];
    let (t, outcome, after) = issues_on(&rules, &base, 0x12C, thread_with(&args), kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(t.stack, [FLOOR, 1]);
    assert_eq!(objects_of(&after, HOUSE).len(), 1);
    let mut s = base.clone();
    let external = submit(
        &mut Sim::new(&mut s, &rules),
        &Command { player: ME, kind },
        Origin::External,
    );
    assert!(
        matches!(&external, Outcome::Rejected(why) if why.contains("not seen")),
        "{external:?}"
    );
}

// QUIRK E-24: kept
#[test]
fn ki_command_events_come_from_the_same_step() {
    // A thread of slot [`ME`] halts its unit in the first AI tick (`CALLB 0x191`); the event is in the return value of
    // exactly this `step`.
    const MODULE: u32 = 9;
    let mut rules = rules();
    let callb = |id| word(op::CALLB, id);
    let code = [
        callb(0x191),
        word(op::LOAD, int(0)),
        callb(0x8E),
        word(op::JMP, 1),
    ];
    rules.ki = catalog(vec![proto(MODULE, &[(0, 8)], &[(0, 1_000_000)], &code)]);
    let mut state = state(&rules);
    let u = spawn(&mut Sim::new(&mut state, &rules), COLONIST, ME_I, (10, 10));
    let started = state
        .ki
        .with_engine(ME, &rules.ki, &mut StubWorld::default(), |e, cx| {
            e.spawn(cx, MODULE, &[u.as_i32()])
        });
    assert!(matches!(started, Some(Some(_))));
    let events = crate::tick::step(&mut state, &rules, &[]);
    let issued: Vec<&SimEvent> = events
        .iter()
        .filter(|e| matches!(e, SimEvent::KiCommand { .. }))
        .collect();
    assert_eq!(
        issued,
        [&SimEvent::KiCommand {
            player: ME,
            code: 0x3C,
            outcome: Outcome::Accepted,
        }]
    );
    assert!(state.events.is_empty());
    // The next tick sleeps: no further command.
    let events = crate::tick::step(&mut state, &rules, &[]);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::KiCommand { .. }))
    );
}
