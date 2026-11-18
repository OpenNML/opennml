//! Tests of the command builtins for combat, behavior, AI modes, diplomacy, resources and removal (re/ki-queries
//! 7.1/7.2, E-88, E-92, E-99 ... E-106).
//!
//! As in [`super::tests_units`]: every row is called against the real world; checked are the stack, `KiCommand` events
//! and the effect (the same state as after the expected [`CommandKind`]s via `submit` with [`Origin::Ki`]).

use std::collections::BTreeSet;

use super::tests_units::{
    FLOOR, Issued, ME, ME_I, issues, kill, list_thread, run, run_on, thread_with,
};
use super::{ROWS, row};
use crate::combat::testkit as ck;
use crate::command::testkit::{HOUSE, spawn as spawn_at};
use crate::command::{Command, CommandKind, Origin, Outcome};
use crate::id::ObjectId;
use crate::ki::signature::signature;
use crate::ki::testkit::{int, list};
use crate::ki::vm::Thread;
use crate::ki::world::KiNote;
use crate::module::attack::attack;
use crate::module::building;
use crate::player::submit;
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::{Control, SimState};

/// All IDs implemented here (28), as ranges; the completeness check is in [`every_control_row_is_covered`].
const CONTROL_IDS: &[(u16, u16)] = &[
    (0x97, 0x97),
    (0xA1, 0xA3),
    (0xCE, 0xCE),
    (0x12F, 0x12F),
    (0x13B, 0x13B),
    (0x154, 0x15B),
    (0x18B, 0x18B),
    (0x198, 0x199),
    (0x19B, 0x19C),
    (0x1A2, 0x1A2),
    (0x1A4, 0x1A9),
    (0x1B8, 0x1B8),
];

/// The IDs of the control rows (for the counter-check in `tests_units`).
pub(super) fn control_ids() -> BTreeSet<u16> {
    CONTROL_IDS.iter().flat_map(|&(lo, hi)| lo..=hi).collect()
}

/// Scene: slot 0 a human, [`ME`] an AI (nation 2, main house 1000) with two fighters, a dead fighter, main house and
/// house; slot 0 has a fighter and a house.
struct Scene {
    rules: Rules,
    base: SimState,
    a: ObjectId,
    b: ObjectId,
    dead: ObjectId,
    foe: ObjectId,
    hq: ObjectId,
    house: ObjectId,
    foe_house: ObjectId,
}

fn scene() -> Scene {
    let rules = ck::rules();
    let mut base = ck::state(&rules);
    base.players[0].control = Control::Human;
    base.players[usize::from(ME)].control = Control::Ai;
    base.players[usize::from(ME)].nation = 2;
    let mut sim = Sim::new(&mut base, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, ME_I, (10, 10));
    let b = ck::spawn(&mut sim, ck::WARRIOR, ME_I, (11, 10));
    let dead = ck::spawn(&mut sim, ck::WARRIOR, ME_I, (12, 10));
    kill(&mut sim, dead);
    let foe = ck::spawn(&mut sim, ck::WARRIOR, 0, (30, 10));
    let hq = ck::spawn(&mut sim, ck::HQ, ME_I, (20, 20));
    let house = spawn_at(&mut sim, HOUSE, ME_I, (24, 20));
    let foe_house = spawn_at(&mut sim, HOUSE, 0, (40, 40));
    Scene {
        rules,
        base,
        a,
        b,
        dead,
        foe,
        hq,
        house,
        foe_house,
    }
}

/// Builtin `id` on thread `t` over a copy of `base` gives exactly the commands `kinds` in this order: the same
/// `KiCommand` events as `submit` with [`Origin::Ki`] on a second copy, the same checksum. Returns thread, outcomes and
/// state.
fn gives_on(
    rules: &Rules,
    base: &SimState,
    id: u16,
    mut t: Thread,
    kinds: Vec<CommandKind>,
) -> (Thread, Vec<Outcome>, SimState) {
    let mut a = base.clone();
    let issued = {
        let mut sim = Sim::new(&mut a, rules);
        let (notes, issued) = run_on(&mut sim, id, &mut t);
        assert!(notes.is_empty(), "{id:#x}: {notes:?}");
        issued
    };
    let mut b = base.clone();
    let mut sim = Sim::new(&mut b, rules);
    let mut expected: Vec<Issued> = Vec::new();
    let mut outcomes = Vec::new();
    for kind in kinds {
        let code = kind.type_code() & 0xFF;
        let outcome = submit(&mut sim, &Command { player: ME, kind }, Origin::Ki);
        expected.push((ME, code, outcome.clone()));
        outcomes.push(outcome);
    }
    assert_eq!(issued, expected, "{id:#x}");
    assert_eq!(checksum(&a), checksum(sim.state), "{id:#x}: effect");
    (t, outcomes, a)
}

/// [`gives_on`] with `args` over [`FLOOR`]; the stack is empty afterwards.
fn gives(s: &Scene, id: u16, args: &[i32], kinds: Vec<CommandKind>) -> (Vec<Outcome>, SimState) {
    let (t, outcomes, after) = gives_on(&s.rules, &s.base, id, thread_with(args), kinds);
    assert_eq!(t.stack, [FLOOR], "{id:#x} pops incorrectly");
    (outcomes, after)
}

/// Builtin `id` gives no command, leaves the state unchanged and pops exactly its arguments.
fn gives_nothing(s: &Scene, id: u16, args: &[i32]) {
    let mut state = s.base.clone();
    let mut sim = Sim::new(&mut state, &s.rules);
    assert_eq!(run(&mut sim, id, args), (vec![], vec![]), "{id:#x}");
    assert_eq!(checksum(sim.state), checksum(&s.base), "{id:#x}");
}

fn sell(id: ObjectId) -> CommandKind {
    CommandKind::SellBuildings {
        buildings: vec![id],
    }
}

fn die(id: ObjectId) -> CommandKind {
    CommandKind::Die { units: vec![id] }
}

fn behavior(id: ObjectId, mode: i32) -> CommandKind {
    CommandKind::SetBehavior {
        units: vec![id],
        mode,
    }
}

fn attack_cmd(unit: ObjectId, target: ObjectId) -> CommandKind {
    CommandKind::Attack {
        units: vec![unit],
        target,
        pos: None,
        mods: 0,
    }
}

fn is_rejected(o: &Outcome) -> bool {
    matches!(o, Outcome::Rejected(_))
}

// QUIRK O-322: kept
#[test]
fn diplomacy_builtin_sends_vision_then_relation() {
    let s = scene();
    let other = 0;
    // `m & 3`: 0 -> neutral (2), 1 -> friend (0), 2 -> enemy (1), 3 -> no diplomacy; bit 2 = shared
    // vision.
    for m in 0..16 {
        let mut kinds = vec![CommandKind::SharedVision {
            p: ME_I,
            q: other,
            on: (m >> 2) & 1 != 0,
        }];
        match m & 3 {
            0 => kinds.push(CommandKind::Diplomacy {
                p: ME_I,
                q: other,
                mode: 2,
            }),
            1 => kinds.push(CommandKind::Diplomacy {
                p: ME_I,
                q: other,
                mode: 0,
            }),
            2 => kinds.push(CommandKind::Diplomacy {
                p: ME_I,
                q: other,
                mode: 1,
            }),
            _ => {}
        }
        let n = kinds.len();
        let (outcomes, after) = gives(&s, 0x97, &[ME_I, other, m], kinds);
        assert_eq!(outcomes.len(), n, "m = {m}");
        assert!(outcomes.iter().all(|o| *o == Outcome::Accepted), "m = {m}");
        // Enemy (`m & 3 == 2`) makes the other slot an enemy, friend a friend.
        match m & 3 {
            2 => assert!(after.diplomacy.is_enemy(ME_I, other), "m = {m}"),
            1 => assert!(!after.diplomacy.is_enemy(ME_I, other), "m = {m}"),
            _ => {}
        }
    }
    // Foreign slot as `p`, the AI as `q`: the vision applies ("other sees what I see", like the vision checkbox of the
    // diplomacy dialog), the relation of another slot is changed only by that slot itself (`Diplomacy` rejected); both
    // commands are sent.
    let kinds = vec![
        CommandKind::SharedVision {
            p: other,
            q: ME_I,
            on: true,
        },
        CommandKind::Diplomacy {
            p: other,
            q: ME_I,
            mode: 1,
        },
    ];
    let (outcomes, after) = gives(&s, 0x97, &[other, ME_I, 4 | 2], kinds);
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_eq!(outcomes[0], Outcome::Accepted, "{outcomes:?}");
    assert!(is_rejected(&outcomes[1]), "{outcomes:?}");
    assert!(after.diplomacy.shares_vision(other, ME_I));
    assert_eq!(after.diplomacy.enemy, s.base.diplomacy.enemy);
    assert_eq!(after.diplomacy.friend, s.base.diplomacy.friend);
    // Without the AI in `p` or `q` both reject, the state stays.
    let kinds = vec![
        CommandKind::SharedVision {
            p: other,
            q: 2,
            on: true,
        },
        CommandKind::Diplomacy {
            p: other,
            q: 2,
            mode: 1,
        },
    ];
    let (outcomes, after) = gives(&s, 0x97, &[other, 2, 4 | 2], kinds);
    assert!(outcomes.iter().all(is_rejected), "{outcomes:?}");
    assert_eq!(checksum(&after), checksum(&s.base));
}

// QUIRK E-92: kept
#[test]
fn remove_builtin_sells_buildings_and_kills_units() {
    let s = scene();
    let i = ObjectId::as_i32;
    for id in [0xA1, 0xA2, 0xA3, 0x12F] {
        // Building with module 3 (present and switched on) -> Sell, unit -> Die.
        let (stack, outcome) = issues(&s.rules, &s.base, id, &[i(s.house)], sell(s.house));
        assert!(stack.is_empty(), "{id:#x}");
        assert!(!is_rejected(&outcome), "{id:#x}: {outcome:?}");
        let (_, outcome) = issues(&s.rules, &s.base, id, &[i(s.a)], die(s.a));
        assert_eq!(outcome, Outcome::Accepted, "{id:#x}");
        // A dead object is served as well (`GetObjectById(id, 0)`); the simulation rejects Die for a dead object
        // (exception A8).
        let (_, outcome) = issues(&s.rules, &s.base, id, &[i(s.dead)], die(s.dead));
        assert!(is_rejected(&outcome), "{id:#x}: {outcome:?}");
        // Foreign building and foreign unit: the command is sent and rejected (exception A8).
        let (_, outcome) = issues(&s.rules, &s.base, id, &[i(s.foe_house)], sell(s.foe_house));
        assert!(is_rejected(&outcome), "{id:#x}: {outcome:?}");
        let (_, outcome) = issues(&s.rules, &s.base, id, &[i(s.foe)], die(s.foe));
        assert!(is_rejected(&outcome), "{id:#x}: {outcome:?}");
    }
    // Module 3 switched off: the building dies (E-92).
    let mut base = s.base.clone();
    {
        let mut sim = Sim::new(&mut base, &s.rules);
        sim.object_mut(s.house)
            .unwrap()
            .module_mut(building::INDEX)
            .unwrap()
            .base
            .enabled = false;
    }
    let (_, outcome) = issues(&s.rules, &base, 0xA1, &[i(s.house)], die(s.house));
    assert!(!is_rejected(&outcome), "{outcome:?}");
    // Missing object (destroyed, never present, 0): no command.
    let mut gone_state = s.base.clone();
    let gone = {
        let mut sim = Sim::new(&mut gone_state, &s.rules);
        let gone = ck::spawn(&mut sim, ck::WARRIOR, ME_I, (14, 10));
        crate::world::destroy_object(&mut sim, gone);
        gone
    };
    let t = Scene {
        base: gone_state,
        ..scene()
    };
    for id in [0xA1, 0xA2, 0xA3, 0x12F] {
        for obj in [i(gone), -1, 0, 12_345] {
            gives_nothing(&t, id, &[obj]);
        }
    }
}

// QUIRK E-51: kept
#[test]
fn headhunter_uses_the_first_finished_hq_in_table_order() {
    let mut s = scene();
    // The table of ME: first a construction site of the main house (not finished), then the first, then the second
    // finished one. The first finished one in table order counts, not the construction site and not the one built last.
    let (site, first, second) = {
        let mut sim = Sim::new(&mut s.base, &s.rules);
        crate::world::destroy_object(&mut sim, s.hq);
        let site = crate::command::testkit::site(&mut sim, ck::HQ, ME_I, (50, 10));
        let first = ck::spawn(&mut sim, ck::HQ, ME_I, (54, 10));
        let second = ck::spawn(&mut sim, ck::HQ, ME_I, (58, 10));
        (site, first, second)
    };
    let table = crate::upgrades::objects_of_slot(&s.base, ME);
    let pos = |id| table.iter().position(|&x| x == id).unwrap();
    assert!(
        pos(site) < pos(first) && pos(first) < pos(second),
        "{table:?}"
    );
    let hire = |building| CommandKind::HireHeadhunter {
        art: 2,
        target_player: 0,
        target_guid: 1112,
        building,
    };
    // Script sequence `(target_player, guid, kind)`; the simulation rejects the hiring (the menu of the scene has no
    // headhunter button), the command is still sent to the main house.
    let args = [0, 1112, 2];
    let (outcomes, _) = gives(&s, 0xCE, &args, vec![hire(first)]);
    assert_eq!(outcomes.len(), 1);
    // If the first finished one is gone, the next one counts.
    let mut rest = s.base.clone();
    crate::world::destroy_object(&mut Sim::new(&mut rest, &s.rules), first);
    let (t, _, _) = gives_on(
        &s.rules,
        &rest,
        0xCE,
        thread_with(&args),
        vec![hire(second)],
    );
    assert_eq!(t.stack, [FLOOR]);
    // If only the construction site remains, a main house of another nation or of a foreign owner: no command, but all
    // three arguments are popped.
    crate::world::destroy_object(&mut Sim::new(&mut rest, &s.rules), second);
    gives_nothing(
        &Scene {
            base: rest,
            ..scene()
        },
        0xCE,
        &args,
    );
    let mut other_nation = s.base.clone();
    other_nation.players[usize::from(ME)].nation = 0;
    gives_nothing(
        &Scene {
            base: other_nation,
            ..scene()
        },
        0xCE,
        &args,
    );
    let mut foreign = s.base.clone();
    {
        let mut sim = Sim::new(&mut foreign, &s.rules);
        for id in [first, second] {
            crate::world::destroy_object(&mut sim, id);
        }
        ck::spawn(&mut sim, ck::HQ, 0, (50, 30));
    }
    gives_nothing(
        &Scene {
            base: foreign,
            ..scene()
        },
        0xCE,
        &args,
    );
}

// QUIRK E-104: kept
#[test]
fn occupancy_border_builtin_sends_the_border() {
    let s = scene();
    for border in [0, 1, -1, 2, -2, 1000] {
        let kind = CommandKind::OccupancyBorder {
            building: s.hq,
            border,
        };
        let (stack, outcome) = issues(&s.rules, &s.base, 0x13B, &[s.hq.as_i32(), border], kind);
        assert!(stack.is_empty());
        assert_eq!(
            is_rejected(&outcome),
            border == 1000,
            "{border}: {outcome:?}"
        );
    }
    // Foreign building: rejected.
    let kind = CommandKind::OccupancyBorder {
        building: s.foe_house,
        border: 1,
    };
    let (_, outcome) = issues(&s.rules, &s.base, 0x13B, &[s.foe_house.as_i32(), 1], kind);
    assert!(is_rejected(&outcome));
}

// QUIRK E-101: kept
#[test]
fn ai_player_modes_use_the_stack_slot_or_me() {
    let s = scene();
    // `0x154` (slot from the stack) and `0x156` (own slot): `0x68 {slot, a, b}`.
    for (id, args) in [(0x154, vec![ME_I, 4, 2]), (0x156, vec![4, 2])] {
        let kind = CommandKind::AiPlayerAttackMode {
            slot: ME_I,
            aggressiveness: 4,
            stubbornness: 2,
        };
        let (outcomes, after) = gives(&s, id, &args, vec![kind]);
        assert_eq!(outcomes, [Outcome::Accepted], "{id:#x}");
        assert_ne!(checksum(&after), checksum(&s.base), "{id:#x}");
        let mut state = after.clone();
        let sim = Sim::new(&mut state, &s.rules);
        for unit in [s.a, s.b] {
            let a = attack(&sim, unit).unwrap();
            assert_eq!((a.aggressiveness, a.stubbornness[0]), (4, 2), "{id:#x}");
        }
    }
    // `0x155` (slot from the stack) and `0x157`: `0x6A {slot, a == 1, b, c == 1, d == 1}`.
    for (a, b, c, d) in [(1, 7, 1, 1), (0, -3, 0, 0), (2, 100, 1, 0), (1, -2, 0, 1)] {
        let kind = CommandKind::AiPlayerWalkFlags {
            slot: ME_I,
            mobile_home: a == 1,
            tether: b,
            follow: c == 1,
            abort: d == 1,
        };
        let (outcomes, _) = gives(&s, 0x155, &[ME_I, a, b, c, d], vec![kind.clone()]);
        assert_eq!(outcomes, [Outcome::Accepted], "{a} {b} {c} {d}");
        let (outcomes, _) = gives(&s, 0x157, &[a, b, c, d], vec![kind]);
        assert_eq!(outcomes, [Outcome::Accepted], "{a} {b} {c} {d}");
    }
    // The tether upper limit is rejected by the simulation; the command is still sent.
    let kind = CommandKind::AiPlayerWalkFlags {
        slot: ME_I,
        mobile_home: true,
        tether: i32::MAX,
        follow: false,
        abort: false,
    };
    let (outcomes, _) = gives(&s, 0x157, &[1, i32::MAX, 0, 0], vec![kind]);
    assert!(is_rejected(&outcomes[0]));
}

// QUIRK E-103: kept
#[test]
fn ai_object_modes_take_the_object_from_the_stack() {
    let s = scene();
    let i = ObjectId::as_i32;
    // `0x158`/`0x15A`: `0x6B {obj, a, b}`; `0x159`/`0x15B`: `0x6C {obj, a == 1, b, c == 1, d == 1}`;
    // also a dead own object (no death filter).
    for obj in [s.a, s.dead] {
        for id in [0x158, 0x15A] {
            let kind = CommandKind::AiObjectAttackMode {
                object: obj,
                aggressiveness: 3,
                stubbornness: 5,
            };
            let (outcomes, _) = gives(&s, id, &[i(obj), 3, 5], vec![kind]);
            assert_eq!(outcomes, [Outcome::Accepted], "{id:#x}");
        }
        for id in [0x159, 0x15B] {
            let kind = CommandKind::AiObjectWalkFlags {
                object: obj,
                mobile_home: true,
                tether: 9,
                follow: false,
                abort: true,
            };
            let (outcomes, _) = gives(&s, id, &[i(obj), 1, 9, 0, 1], vec![kind]);
            assert_eq!(outcomes, [Outcome::Accepted], "{id:#x}");
        }
    }
    // The object of a foreign slot: the command is sent, the simulation rejects it.
    let kind = CommandKind::AiObjectAttackMode {
        object: s.foe,
        aggressiveness: 3,
        stubbornness: 5,
    };
    let (outcomes, after) = gives(&s, 0x158, &[i(s.foe), 3, 5], vec![kind]);
    assert!(is_rejected(&outcomes[0]));
    assert_eq!(checksum(&after), checksum(&s.base));
}

// QUIRK E-105: kept
#[test]
fn resource_builtin_minus_one_means_me() {
    let s = scene();
    let gold = |st: &SimState| st.players[usize::from(ME)].resources_fx[0];
    for slot in [-1, ME_I] {
        let kind = CommandKind::AddResource {
            slot: ME_I,
            resource: 0,
            amount: 100,
        };
        let (outcomes, after) = gives(&s, 0x18B, &[slot, 0, 100], vec![kind]);
        assert_eq!(outcomes, [Outcome::Accepted], "slot {slot}");
        assert_eq!(gold(&after) - gold(&s.base), 100 << 12, "slot {slot}");
    }
    // Negative, population (3) and living space (4): without limit.
    for (resource, amount) in [(0, -50), (3, 7), (4, 9)] {
        let kind = CommandKind::AddResource {
            slot: ME_I,
            resource,
            amount,
        };
        let (outcomes, _) = gives(&s, 0x18B, &[-1, resource, amount], vec![kind]);
        assert_eq!(outcomes, [Outcome::Accepted], "{resource}");
    }
    // Resource 5 does not exist; the command is sent and rejected.
    let kind = CommandKind::AddResource {
        slot: ME_I,
        resource: 5,
        amount: 1,
    };
    let (outcomes, after) = gives(&s, 0x18B, &[-1, 5, 1], vec![kind]);
    assert!(is_rejected(&outcomes[0]));
    assert_eq!(checksum(&after), checksum(&s.base));
}

#[test]
fn behavior_rows_send_the_mode_for_one_unit() {
    let s = scene();
    for id in [0x198, 0x199] {
        for mode in [0, 2, 4] {
            let kinds = vec![behavior(s.a, mode)];
            let (outcomes, _) = gives(&s, id, &[s.a.as_i32(), mode], kinds);
            assert_eq!(outcomes, [Outcome::Accepted], "{id:#x} {mode}");
        }
        // Mode 7 and a foreign unit are rejected by the simulation.
        let (outcomes, _) = gives(&s, id, &[s.a.as_i32(), 7], vec![behavior(s.a, 7)]);
        assert!(is_rejected(&outcomes[0]), "{id:#x}");
        let (outcomes, _) = gives(&s, id, &[s.foe.as_i32(), 1], vec![behavior(s.foe, 1)]);
        assert!(is_rejected(&outcomes[0]), "{id:#x}");
    }
}

#[test]
fn behavior_list_sends_one_command_per_entry_without_filter() {
    let mut s = scene();
    let gone = {
        let mut sim = Sim::new(&mut s.base, &s.rules);
        let gone = ck::spawn(&mut sim, ck::WARRIOR, ME_I, (14, 10));
        crate::world::destroy_object(&mut sim, gone);
        gone
    };
    let l0 = list(0) as i32;
    // List 5...10: a, dead, missing, a twice, b, foreign: six commands in list order, nothing is filtered (the
    // simulation decides per unit).
    let entries = [s.a, s.dead, gone, s.a, s.b, s.foe];
    let pairs: Vec<(i32, i32)> = entries
        .iter()
        .enumerate()
        .map(|(i, u)| (5 + i as i32, u.as_i32()))
        .collect();
    for id in [0x19B, 0x19C] {
        let t = list_thread(&pairs, &[l0, 5, 6, 2]);
        let kinds = entries.iter().map(|&u| behavior(u, 2)).collect();
        let (t, outcomes, _) = gives_on(&s.rules, &s.base, id, t, kinds);
        assert_eq!(t.stack, [FLOOR], "{id:#x}");
        assert_eq!(outcomes.len(), 6);
        assert_eq!(outcomes[0], Outcome::Accepted);
        assert!(is_rejected(&outcomes[2]), "missing unit: {outcomes:?}");
        assert!(is_rejected(&outcomes[5]), "foreign unit: {outcomes:?}");
    }
    // Missing keys are created with 0 (`FUN_004E7B30`) and given as unit 0.
    let t = list_thread(&pairs[..1], &[l0, 5, 3, 1]);
    let kinds = vec![
        behavior(s.a, 1),
        behavior(ObjectId::from_i32(0), 1),
        behavior(ObjectId::from_i32(0), 1),
    ];
    let (t, _, _) = gives_on(&s.rules, &s.base, 0x19B, t, kinds);
    assert_eq!(t.vars.list_at(l0).unwrap().len(), 3);
}

// QUIRK E-34: deviation
#[test]
fn list_rows_ignore_bad_lists_and_sizes() {
    let s = scene();
    let l0 = list(0) as i32;
    let pairs = [(0, s.a.as_i32())];
    for id in [0x19B, 0x1A7] {
        // Pointer 0 (variable 3 is 0) and n not in (0, 100): no command, nothing created.
        for (obj, n) in [(int(3) as i32, 2), (l0, 0), (l0, 100), (l0, -1)] {
            let mut state = s.base.clone();
            let mut sim = Sim::new(&mut state, &s.rules);
            let mut t = list_thread(&pairs, &[obj, 0, n, 1]);
            assert_eq!(run_on(&mut sim, id, &mut t), (vec![], vec![]), "{id:#x}");
            assert_eq!(t.stack, [FLOOR], "{id:#x}");
            assert_eq!(t.vars.list_at(l0).unwrap().len(), 1);
        }
        // A pointer without a list: `Guarded`, no command, stack unchanged.
        let mut state = s.base.clone();
        let mut sim = Sim::new(&mut state, &s.rules);
        let mut t = thread_with(&[int(3) as i32, 0, 2, 1]);
        t.vars.set(int(3), 12_345);
        let (notes, issued) = run_on(&mut sim, id, &mut t);
        assert!(
            matches!(notes[..], [KiNote::Guarded { builtin, .. }] if builtin == id),
            "{notes:?}"
        );
        assert!(issued.is_empty());
        assert_eq!(t.stack, [FLOOR]);
    }
}

#[test]
fn arm_builtin_toggles_arming_on() {
    let s = scene();
    let kind = CommandKind::ToggleArming {
        units: vec![s.a],
        arm: true,
    };
    let (stack, _) = issues(&s.rules, &s.base, 0x1A2, &[s.a.as_i32()], kind);
    assert!(stack.is_empty());
    // Foreign unit: the command is sent and the simulation rejects it.
    let kind = CommandKind::ToggleArming {
        units: vec![s.foe],
        arm: true,
    };
    let (_, outcome) = issues(&s.rules, &s.base, 0x1A2, &[s.foe.as_i32()], kind);
    assert!(is_rejected(&outcome), "{outcome:?}");
}

#[test]
fn attack_row_needs_a_target_with_module_3_or_0() {
    let s = scene();
    let i = ObjectId::as_i32;
    for id in [0x1A4, 0x1A5, 0x1A6] {
        // Living foreign unit (module 0) and foreign building (module 3): attack.
        for target in [s.foe, s.foe_house] {
            let (stack, outcome) = issues(
                &s.rules,
                &s.base,
                id,
                &[i(s.a), i(target)],
                attack_cmd(s.a, target),
            );
            assert!(stack.is_empty(), "{id:#x}");
            assert!(!is_rejected(&outcome), "{id:#x}: {outcome:?}");
        }
        // A dead target (present, module 0 still on) is attacked as well.
        issues(
            &s.rules,
            &s.base,
            id,
            &[i(s.a), i(s.dead)],
            attack_cmd(s.a, s.dead),
        );
    }
    // Dying target with walk switched off, without building module: no target, no command. The same for a missing
    // target.
    let mut state = s.base.clone();
    let gone = {
        let mut sim = Sim::new(&mut state, &s.rules);
        let gone = ck::spawn(&mut sim, ck::WARRIOR, 0, (31, 10));
        crate::world::destroy_object(&mut sim, gone);
        let dying = ck::spawn(&mut sim, ck::WARRIOR, 0, (32, 10));
        sim.object_mut(dying)
            .unwrap()
            .module_mut(0)
            .unwrap()
            .base
            .enabled = false;
        (gone, dying)
    };
    let t = Scene {
        base: state,
        ..scene()
    };
    for target in [gone.0.as_i32(), gone.1.as_i32(), -1, 0] {
        gives_nothing(&t, 0x1A4, &[i(s.a), target]);
    }
    // With an invalid attacker the command is sent (unchecked in the original) and fails in the simulation.
    let (_, outcome) = issues(
        &s.rules,
        &s.base,
        0x1A4,
        &[12_345, i(s.foe)],
        attack_cmd(ObjectId::from_i32(12_345), s.foe),
    );
    assert!(is_rejected(&outcome), "{outcome:?}");
}

// QUIRK E-88: deviation
#[test]
fn list_attack_sends_one_attack_per_unit() {
    let s = scene();
    let l0 = list(0) as i32;
    // E-88: the original sends `n` as the count, the reimplementation exactly one attack of that unit per ID; without
    // filter (dead and duplicate ones go along) and without module check of the target.
    let entries = [s.a, s.b, s.dead, s.a];
    let pairs: Vec<(i32, i32)> = entries
        .iter()
        .enumerate()
        .map(|(i, u)| (2 + i as i32, u.as_i32()))
        .collect();
    for id in [0x1A7, 0x1A8, 0x1A9] {
        for target in [s.foe, s.foe_house] {
            let t = list_thread(&pairs, &[l0, 2, 4, target.as_i32()]);
            let kinds = entries.iter().map(|&u| attack_cmd(u, target)).collect();
            let (t, outcomes, _) = gives_on(&s.rules, &s.base, id, t, kinds);
            assert_eq!(t.stack, [FLOOR], "{id:#x}");
            assert_eq!(outcomes.len(), 4);
        }
    }
    // A missing target does not stop the commands (no module check): they are all sent.
    let t = list_thread(&pairs, &[l0, 2, 2, 12_345]);
    let missing = ObjectId::from_i32(12_345);
    let kinds = entries[..2]
        .iter()
        .map(|&u| attack_cmd(u, missing))
        .collect();
    let (_, outcomes, _) = gives_on(&s.rules, &s.base, 0x1A7, t, kinds);
    assert_eq!(outcomes.len(), 2);
}

// QUIRK E-100: kept
#[test]
fn behavior_builtin_sends_the_ai_mode_afterwards() {
    let s = scene();
    for mode in [0, 1, 2, 3, 4, 7, -1] {
        let mut kinds = vec![CommandKind::AiPlayerBehavior { slot: ME_I, mode }];
        let stubbornness = match mode {
            3 => Some(2),
            2 => Some(1),
            _ => None,
        };
        if let Some(stubbornness) = stubbornness {
            kinds.push(CommandKind::AiPlayerAttackMode {
                slot: ME_I,
                aggressiveness: 4,
                stubbornness,
            });
        }
        let n = kinds.len();
        let (outcomes, after) = gives(&s, 0x1B8, &[mode], kinds);
        assert_eq!(outcomes.len(), n, "mode {mode}");
        assert_eq!(
            is_rejected(&outcomes[0]),
            !(0..=4).contains(&mode),
            "mode {mode}"
        );
        if (0..=4).contains(&mode) {
            assert_eq!(after.combat.behavior_mode[usize::from(ME)], mode);
        }
    }
}

// QUIRK E-26: deviation
#[test]
fn foreign_slot_is_rejected() {
    let s = scene();
    // `0x154` with a foreign slot (human, 0) and with a slot that is not the sender (2).
    for slot in [0, 2, 7, 31, -1] {
        let kind = CommandKind::AiPlayerAttackMode {
            slot,
            aggressiveness: 4,
            stubbornness: 2,
        };
        let (outcomes, after) = gives(&s, 0x154, &[slot, 4, 2], vec![kind]);
        assert!(is_rejected(&outcomes[0]), "slot {slot}: {outcomes:?}");
        assert_eq!(checksum(&after), checksum(&s.base), "slot {slot}");
    }
    let kind = CommandKind::AiPlayerWalkFlags {
        slot: 0,
        mobile_home: false,
        tether: 0,
        follow: false,
        abort: false,
    };
    let (outcomes, _) = gives(&s, 0x155, &[0, 0, 0, 0, 0], vec![kind]);
    assert!(is_rejected(&outcomes[0]));
    let kind = CommandKind::AddResource {
        slot: 0,
        resource: 0,
        amount: 5,
    };
    let (outcomes, after) = gives(&s, 0x18B, &[0, 0, 5], vec![kind]);
    assert!(is_rejected(&outcomes[0]));
    assert_eq!(checksum(&after), checksum(&s.base));
    // A human slot as own slot: the simulation rejects the AI modes and `0x1B8`.
    let mut human = s.base.clone();
    human.players[usize::from(ME)].control = Control::Human;
    let kind = CommandKind::AiPlayerBehavior {
        slot: ME_I,
        mode: 1,
    };
    let (_, outcomes, _) = gives_on(&s.rules, &human, 0x1B8, thread_with(&[1]), vec![kind]);
    assert!(is_rejected(&outcomes[0]));
}

// QUIRK E-34: deviation
#[test]
fn control_rows_pop_exactly_their_pops_even_when_rejecting() {
    // Every row takes exactly its arguments from the stack and pushes nothing - even on rejection, missing object,
    // foreign slot, nonsensical values and lists with a bad pointer or n.
    let s = scene();
    let l0 = list(0) as i32;
    let values = [0, -1, 1, 2, 9, 31, 1000, i32::MAX, i32::MIN];
    for &(lo, hi) in CONTROL_IDS {
        for id in lo..=hi {
            let r = row(id).expect("row");
            assert!(!r.push && r.outs.is_empty(), "{id:#x}");
            let n = usize::from(r.pops);
            let mut sets: Vec<Vec<i32>> = values.iter().map(|&v| vec![v; n]).collect();
            for &v in &values {
                for at in 0..n {
                    let mut a = vec![0; n];
                    a[at] = v;
                    sets.push(a);
                }
            }
            // Valid objects of the scene at every position.
            for obj in [s.a, s.b, s.dead, s.foe, s.hq, s.house, s.foe_house] {
                for at in 0..n {
                    let mut a = vec![1; n];
                    a[at] = obj.as_i32();
                    sets.push(a);
                }
            }
            if (0x19B..=0x19C).contains(&id) || (0x1A7..=0x1A9).contains(&id) {
                for addr in [l0, list(1) as i32, int(2) as i32] {
                    for n_ in [-1, 0, 1, 3, 99, 100] {
                        sets.push(vec![addr, 0, n_, 2]);
                    }
                }
            }
            for args in sets {
                let mut state = s.base.clone();
                let mut sim = Sim::new(&mut state, &s.rules);
                let mut t = list_thread(&[(0, s.a.as_i32()), (1, s.foe.as_i32())], &args);
                let (notes, _) = run_on(&mut sim, id, &mut t);
                assert_eq!(t.stack, [FLOOR], "{id:#x} {args:?}");
                assert!(
                    notes
                        .iter()
                        .all(|n| matches!(n, KiNote::Guarded { builtin, .. } if *builtin == id)),
                    "{id:#x} {args:?}: {notes:?}"
                );
            }
        }
    }
}

#[test]
fn control_rows_match_the_e1a_signatures() {
    // Where a signature is known (corpus IDs), the row has the same pops, push, outputs, pause and the same query code;
    // 14 of the 28 IDs are in the corpus.
    let mut compared = 0;
    for id in control_ids() {
        let r = row(id).expect("row");
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
    assert_eq!((control_ids().len(), compared), (28, 14));
}

#[test]
fn every_control_row_is_covered() {
    // Every row of this group has its own scene test: the tests above call exactly these IDs.
    let called: BTreeSet<u16> = [
        0x97, 0xA1, 0xA2, 0xA3, 0x12F, 0xCE, 0x13B, 0x154, 0x155, 0x156, 0x157, 0x158, 0x159,
        0x15A, 0x15B, 0x18B, 0x198, 0x199, 0x19B, 0x19C, 0x1A2, 0x1A4, 0x1A5, 0x1A6, 0x1A7, 0x1A8,
        0x1A9, 0x1B8,
    ]
    .into_iter()
    .collect();
    assert_eq!(called, control_ids());
    let all: BTreeSet<u16> = ROWS.iter().flat_map(|r| r.lo..=r.hi).collect();
    assert!(called.is_subset(&all));
}
