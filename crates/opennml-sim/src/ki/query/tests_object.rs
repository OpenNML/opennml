//! Tests of the object queries (re/ki-queries 5.14, 5.15, 5.20): every code against a living, a dead and a missing
//! object, plus the quirks E-55, E-69 ... E-74.

use std::collections::BTreeMap;

use opennml_formats::property::{ModuleDesc, ObjectType};
use opennml_formats::targets::parse_targets;

use super::codes_object::is_attacking;
use super::tests_player::{FLOOR, ME, ask, run, run_with};
use super::{is_query, query_once};
use crate::combat::testkit::{ARCHER, HQ, WARRIOR, defines, rules as combat_rules, spawn, state};
use crate::command::testkit::{COLONIST, fogged_state};
use crate::id::ObjectId;
use crate::ki::testkit::int;
use crate::module::ModuleKind;
use crate::module::attack::attack_mut;
use crate::module::building::building_mut;
use crate::object::status;
use crate::player::Sight;
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;

/// Type with a riding module (module 13), otherwise empty.
const RIDER: u32 = 2999;
/// Filter ID 182: "Corpse" (`MM_RETURN_DIED` + own list), as in `flowcontrol_targets`.
const CORPSE_FILTER: i32 = 182;
const ME_I: i32 = ME as i32;

/// Rules: combat world, gold (26) and food (24) of the warrior, a riding type and the filter 182.
fn scene() -> (Rules, SimState) {
    let mut rules = combat_rules();
    let values = rules.defaults.entry(WARRIOR).or_default();
    values.insert(26, 40);
    values.insert(24, 15);
    rules.types.insert(
        RIDER,
        ObjectType {
            guid: RIDER,
            name: "Reiter".into(),
            values: BTreeMap::new(),
            modules: vec![ModuleDesc {
                index: 13,
                mask: u32::MAX,
                values: BTreeMap::new(),
            }],
        },
    );
    let text = "BEGIN\nBUTTONMSG 182\nMY_MM\nMM_MODE MM_RETURN_DIED\nMM_MODE MM_HL_MYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n";
    rules.flowcontrol_targets = parse_targets(text, &defines()).entries;
    let state = state(&rules);
    (rules, state)
}

/// Object IDs of the test world (raw as `i32`, as the AI knows them).
struct Ids {
    /// Living warrior of slot 1 on tile (10, 10).
    warrior: i32,
    /// Dead (not removed) warrior of slot 1 on tile (14, 12).
    dead_warrior: i32,
    /// Archer of slot 2 (enemy) on (20, 20).
    archer: i32,
    /// Headquarters (building and container module) of slot 1.
    hq: i32,
    /// Dead, not destroyed headquarters.
    dead_hq: i32,
    /// Colonist (no attack, building or container module).
    colonist: i32,
    /// ID of a removed object (generation stale).
    stale: i32,
}

impl Ids {
    /// IDs that name no object: 0, -1, stale, outside the table.
    fn missing(&self) -> [i32; 4] {
        [0, -1, self.stale, ObjectId::new(900, 3, 1).as_i32()]
    }
}

fn ids(sim: &mut Sim) -> Ids {
    let warrior = spawn(sim, WARRIOR, 1, (10, 10));
    let dead_warrior = spawn(sim, WARRIOR, 1, (14, 12));
    let archer = spawn(sim, ARCHER, 2, (20, 20));
    let hq = spawn(sim, HQ, 1, (30, 30));
    let dead_hq = spawn(sim, HQ, 1, (40, 40));
    let colonist = spawn(sim, COLONIST, 1, (50, 50));
    let gone = spawn(sim, COLONIST, 1, (52, 50));
    for dead in [dead_warrior, dead_hq] {
        sim.object_mut(dead).unwrap().status |= status::DIED;
    }
    crate::world::destroy_object(sim, gone);
    Ids {
        warrior: warrior.as_i32(),
        dead_warrior: dead_warrior.as_i32(),
        archer: archer.as_i32(),
        hq: hq.as_i32(),
        dead_hq: dead_hq.as_i32(),
        colonist: colonist.as_i32(),
        stale: gone.as_i32(),
    }
}

fn oid(id: i32) -> ObjectId {
    ObjectId::from_i32(id)
}

#[test]
fn alive_query_and_its_negation() {
    // `0x4E2A` is the only query with `GetObjectById(id, 1)`; `0x4F65` negates it.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for (id, alive) in [
        (o.warrior, 1),
        (o.dead_warrior, 0),
        (o.hq, 1),
        (o.dead_hq, 0),
    ] {
        assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I, id]), alive, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x4F65, &[ME_I, id]), 1 - alive, "{id:#x}");
        for b in 0x74..=0x76 {
            assert_eq!(run(&mut sim, b, &[id]), [alive], "{b:#x}");
        }
        assert_eq!(run(&mut sim, 0x140, &[id]), [1 - alive]);
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I, id]), 0, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x4F65, &[3, id]), 1, "{id:#x}");
        assert_eq!(run(&mut sim, 0x74, &[id]), [0]);
        assert_eq!(run(&mut sim, 0x140, &[id]), [1]);
    }
    // Wrong argc: `0x4E2A` -> 0 (`0x004AA799`), `0x4F65` leaves the field.
    assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I]), 0);
    assert_eq!(ask(&mut sim, 0x4F65, &[ME_I]), ME_I);
}

#[test]
fn health_percent_reads_the_fraction_and_is_zero_for_the_dead_and_missing() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    assert_eq!(ask(&mut sim, 0x4E2B, &[ME_I, o.warrior]), 100);
    sim.object_mut(oid(o.warrior))
        .unwrap()
        .attributes
        .set_current_fraction(14, 45_678);
    assert_eq!(ask(&mut sim, 0x4E2B, &[ME_I, o.warrior]), 45);
    assert_eq!(run(&mut sim, 0x78, &[o.warrior]), [45]);
    assert_eq!(run(&mut sim, 0x7A, &[o.warrior]), [45]);
    // A dead one usually has a fraction <= 0 (negative reads as 0).
    let dead = sim.object_mut(oid(o.dead_warrior)).unwrap();
    dead.attributes.set_current_fraction(14, 0);
    dead.attributes.add_fraction(14, -5000);
    assert_eq!(ask(&mut sim, 0x4E2B, &[ME_I, o.dead_warrior]), 0);
    assert_eq!(run(&mut sim, 0x79, &[o.dead_warrior]), [0]);
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4E2B, &[3, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x78, &[id]), [0]);
    }
    assert_eq!(ask(&mut sim, 0x4E2B, &[3]), 3, "wrong argc");
}

#[test]
fn position_query_returns_the_tile_and_writes_both_outputs() {
    // re/ki-queries 5.15.1: tile of the current position = `grid >> 12`; the handler writes z first, then x.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for (id, x, z) in [
        (o.warrior, 10, 10),
        (o.dead_warrior, 14, 12),
        (o.hq, 30, 30),
    ] {
        let (req, direct) =
            query_once(&mut sim, ME, Sight::Omniscient, 0x4E2C, &[ME_I, id, 77, 88]);
        assert_eq!((req[2], req[3], req[4], req[5]), (1, x, z, 88), "{id:#x}");
        assert!(direct.is_empty());
        let (t, notes) = run_with(&mut sim, 0x7B, &[FLOOR, id, 1, 2]);
        assert!(notes.is_empty());
        assert_eq!(t.stack, [FLOOR, 1]);
        assert_eq!((t.vars.get(int(1)), t.vars.get(int(2))), (x, z));
    }
    // Arithmetic (direction -infinity): just west of tile 0 lies on tile -1.
    sim.object_mut(oid(o.warrior)).unwrap().grid.x = crate::fx::Fx(-1);
    assert_eq!(
        query_once(
            &mut sim,
            ME,
            Sight::Omniscient,
            0x4E2C,
            &[ME_I, o.warrior, 0, 0]
        )
        .0[3],
        -1
    );
    // One variable for both outputs: `x` is written last and wins.
    sim.object_mut(oid(o.warrior)).unwrap().grid.x = crate::fx::Fx::from_int(12);
    let (t, _) = run_with(&mut sim, 0x7C, &[FLOOR, o.warrior, 4, 4]);
    assert_eq!(t.vars.get(int(4)), 12);
}

// QUIRK E-33, E-73: deviation
#[test]
fn position_query_without_object_writes_zero() {
    // E-73: without an object `[2] = 0`, the outputs are 0 instead of the stack leftovers of the original; the
    // variables are written anyway.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in o.missing() {
        let (req, _) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E2C, &[ME_I, id, 77, 88]);
        assert_eq!((req[2], req[3], req[4], req[5]), (0, 0, 0, 88), "{id:#x}");
        let (t, _) = run_with(&mut sim, 0x7D, &[FLOOR, id, 1, 2]);
        assert_eq!(t.stack, [FLOOR, 0], "{id:#x}");
        assert_eq!((t.vars.get(int(1)), t.vars.get(int(2))), (0, 0), "{id:#x}");
    }
    // Wrong argc leaves the field.
    let (req, _) = query_once(
        &mut sim,
        ME,
        Sight::Omniscient,
        0x4E2C,
        &[ME_I, o.warrior, 7],
    );
    assert_eq!((req[2], req[3], req[4]), (ME_I, o.warrior, 7));
}

// QUIRK E-27: kept
#[test]
fn filter_queries_check_the_object_against_a_filter() {
    // re/ki-queries 5.7: filter ID >= 1000 = GUID filter ("belongs to `[2]` and has the GUID"); dead -> 0; missing ->
    // `[2]` stays. The player is `[2]`, not the owner.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    let g = WARRIOR as i32;
    for code in [0x4E28, 0x4E29] {
        assert_eq!(ask(&mut sim, code, &[1, o.warrior, g]), 1);
        assert_eq!(ask(&mut sim, code, &[2, o.warrior, g]), 0, "foreign player");
        assert_eq!(ask(&mut sim, code, &[1, o.warrior, g + 1]), 0, "other GUID");
        assert_eq!(
            ask(&mut sim, code, &[1, o.warrior, 999]),
            0,
            "unknown filter ID"
        );
        assert_eq!(ask(&mut sim, code, &[1, o.warrior, -5]), 0);
        assert_eq!(
            ask(&mut sim, code, &[1, o.dead_warrior, g]),
            0,
            "dead: DIED precheck"
        );
        assert_eq!(
            ask(&mut sim, code, &[3, o.dead_warrior, g]),
            0,
            "dead replaces `[2]`"
        );
        // Corpse (182): only this filter lets the dead through.
        assert_eq!(ask(&mut sim, code, &[1, o.dead_warrior, CORPSE_FILTER]), 1);
        assert_eq!(ask(&mut sim, code, &[1, o.warrior, CORPSE_FILTER]), 1);
        assert_eq!(
            ask(&mut sim, code, &[3, o.dead_warrior, CORPSE_FILTER]),
            0,
            "own list of player 3"
        );
        for id in o.missing() {
            assert_eq!(
                ask(&mut sim, code, &[3, id, g]),
                3,
                "missing leaves `[2]`: {id:#x}"
            );
        }
    }
    // Wrong argc leaves the field.
    assert_eq!(ask(&mut sim, 0x4E28, &[3, o.warrior]), 3);
    // Builtins 0x6E-0x73 `(obj, filter)`: player `me`; missing pushes `me` (E-55).
    for id in 0x6E..=0x73 {
        assert_eq!(run(&mut sim, id, &[o.warrior, g]), [1], "{id:#x}");
        assert_eq!(run(&mut sim, id, &[o.warrior, 5]), [0], "{id:#x}");
        assert_eq!(run(&mut sim, id, &[o.dead_warrior, g]), [0], "{id:#x}");
        assert_eq!(run(&mut sim, id, &[0, g]), [ME_I], "{id:#x}");
        assert_eq!(run(&mut sim, id, &[o.stale, g]), [ME_I], "{id:#x}");
    }
}

// QUIRK E-27: kept
#[test]
fn sight_range_is_attribute_18_in_whole_tiles() {
    // `GetCurrentValue(0x12) / 4` rounded toward 0; object missing -> `[2]` stays.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in [o.warrior, o.dead_warrior, o.archer] {
        let v = sim.attribute(oid(id), 18);
        assert_eq!(v, 40, "test value");
        assert_eq!(ask(&mut sim, 0x4F2E, &[3, id]), 10, "{id:#x}");
    }
    sim.object_mut(oid(o.warrior))
        .unwrap()
        .attributes
        .set_modifier(18, 3, false, 5);
    let v = sim.attribute(oid(o.warrior), 18);
    assert_eq!(ask(&mut sim, 0x4F2E, &[3, o.warrior]), v / 4);
    for id in [0x18E, 0x18F, 0x197] {
        assert_eq!(run(&mut sim, id, &[o.archer]), [10], "{id:#x}");
        assert_eq!(
            run(&mut sim, id, &[o.dead_warrior]),
            [v_of(&sim, o.dead_warrior) / 4]
        );
        assert_eq!(run(&mut sim, id, &[0]), [ME_I], "missing pushes `me`");
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4F2E, &[3, id]), 3, "{id:#x}");
    }
    assert_eq!(ask(&mut sim, 0x4F2E, &[3]), 3, "wrong argc");
}

fn v_of(sim: &Sim, id: i32) -> i32 {
    sim.attribute(oid(id), 18)
}

#[test]
fn type_guid_query_gives_minus_one_for_missing_objects() {
    // `0x4FC9`: `[2]` is the object ID itself (no player), result `GO+0x54`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for (id, guid) in [
        (o.warrior, WARRIOR),
        (o.dead_warrior, WARRIOR),
        (o.hq, HQ),
        (o.archer, ARCHER),
    ] {
        assert_eq!(ask(&mut sim, 0x4FC9, &[id]), guid as i32, "{id:#x}");
        for b in [0x175, 0x179, 0x17A] {
            assert_eq!(run(&mut sim, b, &[id]), [guid as i32], "{b:#x}");
        }
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4FC9, &[id]), -1, "{id:#x}");
        assert_eq!(run(&mut sim, 0x175, &[id]), [-1]);
    }
    // Wrong argc (2 instead of 1) leaves the object ID in place.
    assert_eq!(ask(&mut sim, 0x4FC9, &[o.warrior, 0]), o.warrior);
}

// QUIRK E-70: kept
#[test]
fn mana_of_an_object_without_mana_is_hundred() {
    // E-70: attribute 15 starts full for every object; object missing -> 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in [o.warrior, o.dead_warrior, o.hq, o.colonist] {
        assert_eq!(ask(&mut sim, 0x4F9C, &[ME_I, id]), 100, "{id:#x}");
        assert_eq!(run(&mut sim, 0x1A0, &[id]), [100]);
    }
    sim.object_mut(oid(o.warrior))
        .unwrap()
        .attributes
        .set_current_fraction(15, 42_999);
    assert_eq!(ask(&mut sim, 0x4F9C, &[ME_I, o.warrior]), 42);
    assert_eq!(run(&mut sim, 0x1A0, &[o.warrior]), [42]);
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4F9C, &[3, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x1A0, &[id]), [0]);
    }
    assert_eq!(ask(&mut sim, 0x4F9C, &[3]), 3, "wrong argc");
}

#[test]
fn stamina_percent_reads_attribute_28() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for (id, f, pct) in [(o.warrior, 73_999, 73), (o.dead_warrior, 5_000, 5)] {
        sim.object_mut(oid(id))
            .unwrap()
            .attributes
            .set_current_fraction(28, f);
        assert_eq!(ask(&mut sim, 0x4FA1, &[ME_I, id]), pct);
        for b in [0x19F, 0x1AB] {
            assert_eq!(run(&mut sim, b, &[id]), [pct], "{b:#x}");
        }
    }
    assert_eq!(
        ask(&mut sim, 0x4FA1, &[ME_I, o.archer]),
        0,
        "start fraction 0"
    );
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4FA1, &[3, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x19F, &[id]), [0]);
    }
}

// QUIRK E-71: kept
#[test]
fn attack_mode_of_the_dead_is_minus_one() {
    // E-71: the dead and objects without a (switched-on) attack module -> -1, only a missing one -> 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    attack_mut(&mut sim, oid(o.warrior)).unwrap().mode = 3;
    assert_eq!(ask(&mut sim, 0x4F79, &[ME_I, o.warrior]), 3);
    assert_eq!(run(&mut sim, 0x19D, &[o.warrior]), [3]);
    assert_eq!(run(&mut sim, 0x19E, &[o.warrior]), [3]);
    assert_eq!(ask(&mut sim, 0x4F79, &[ME_I, o.dead_warrior]), -1, "dead");
    assert_eq!(
        ask(&mut sim, 0x4F79, &[ME_I, o.colonist]),
        -1,
        "without attack module"
    );
    assert_eq!(ask(&mut sim, 0x4F79, &[ME_I, o.hq]), -1, "building");
    sim.object_mut(oid(o.archer))
        .unwrap()
        .set_module_enabled(crate::module::attack::INDEX, false);
    assert_eq!(ask(&mut sim, 0x4F79, &[ME_I, o.archer]), -1, "switched off");
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4F79, &[3, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x19D, &[id]), [0]);
    }
    assert_eq!(ask(&mut sim, 0x4F79, &[3]), 3, "wrong argc");
}

#[test]
fn attack_status_queries_report_the_target_only_while_attacking() {
    // `0xC11 {0}`: status 1/2 = attacks. `0x4FB5` -> 1; `0x4FAB` -> target ID (unchecked), otherwise 0; the dead and
    // objects without a module answer -1 -> 0; `0x4FAB` leaves `[2]` for a missing object.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    let target = oid(o.archer);
    for (status, want) in [(0, false), (1, true), (2, true), (3, false), (-1, false)] {
        {
            let a = attack_mut(&mut sim, oid(o.warrior)).unwrap();
            (a.status, a.target) = (status, target);
        }
        let (flag, goal) = (i32::from(want), if want { o.archer } else { 0 });
        assert_eq!(
            ask(&mut sim, 0x4FB5, &[ME_I, o.warrior]),
            flag,
            "status {status}"
        );
        assert_eq!(
            ask(&mut sim, 0x4FAB, &[3, o.warrior]),
            goal,
            "status {status}"
        );
        for b in [0x170, 0x171] {
            assert_eq!(run(&mut sim, b, &[o.warrior]), [flag]);
        }
        for b in 0x16A..=0x16C {
            assert_eq!(run(&mut sim, b, &[o.warrior]), [goal]);
        }
        let mut view = crate::player::View::new(&mut sim, ME, Sight::Omniscient);
        assert_eq!(is_attacking(&mut view, ME_I, o.warrior), flag);
    }
    // A target that no longer exists is still returned.
    {
        let a = attack_mut(&mut sim, oid(o.warrior)).unwrap();
        (a.status, a.target) = (1, oid(o.stale));
    }
    assert_eq!(ask(&mut sim, 0x4FAB, &[3, o.warrior]), o.stale);
    // Dead, without a module: -1 -> 0 (`0x4FAB` 0, not `[2]`).
    for id in [o.dead_warrior, o.colonist, o.hq] {
        assert_eq!(ask(&mut sim, 0x4FB5, &[ME_I, id]), 0, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x4FAB, &[3, id]), 0, "{id:#x}");
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4FB5, &[ME_I, id]), 0, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x4FAB, &[3, id]), 3, "{id:#x}");
        assert_eq!(run(&mut sim, 0x16A, &[id]), [ME_I]);
        let mut view = crate::player::View::new(&mut sim, ME, Sight::Omniscient);
        assert_eq!(is_attacking(&mut view, ME_I, id), 0);
    }
}

// QUIRK E-74: kept
#[test]
fn commanded_target_is_one_only_for_target_id_one() {
    // E-74: `0x1411` compares the argument counter (1) with the target ID and requires stubbornness > 3; the target
    // argument never has an effect. Dead / without a module -1, missing 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    {
        let a = attack_mut(&mut sim, oid(o.warrior)).unwrap();
        a.target = oid(1);
        a.stubbornness[1] = 4;
    }
    for b in [o.warrior, o.archer, 0, 1, 12345] {
        assert_eq!(
            ask(&mut sim, 0x5014, &[ME_I, o.warrior, b]),
            1,
            "target {b}"
        );
    }
    for id in 0x1B9..=0x1BB {
        assert_eq!(run(&mut sim, id, &[o.warrior, o.archer]), [1], "{id:#x}");
    }
    attack_mut(&mut sim, oid(o.warrior)).unwrap().stubbornness[1] = 3;
    assert_eq!(
        ask(&mut sim, 0x5014, &[ME_I, o.warrior, 1]),
        0,
        "stubbornness 3"
    );
    {
        let a = attack_mut(&mut sim, oid(o.warrior)).unwrap();
        (a.stubbornness[1], a.target) = (4, oid(o.archer));
    }
    assert_eq!(
        ask(&mut sim, 0x5014, &[ME_I, o.warrior, o.archer]),
        0,
        "target not 1"
    );
    for id in [o.dead_warrior, o.colonist, o.hq] {
        assert_eq!(ask(&mut sim, 0x5014, &[ME_I, id, 1]), -1, "{id:#x}");
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x5014, &[3, id, 1]), 0, "{id:#x}");
    }
    assert_eq!(ask(&mut sim, 0x5014, &[3, o.warrior]), 3, "wrong argc");
}

#[test]
fn reach_query_is_the_maximum_ranged_distance_clamped_at_zero() {
    // `0xD11`: fixed point, 4096 = 1 tile; pure melee fighters, the dead and objects without an attack module answer -1
    // -> 0. The ID is in `[2]` (no player).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    // Archer: min(range 28, sight 40) quarter tiles << 10.
    assert_eq!(ask(&mut sim, 0x4FF1, &[o.archer]), 28 << 10);
    for b in [0x1A3, 0x1AA] {
        assert_eq!(run(&mut sim, b, &[o.archer]), [28 << 10], "{b:#x}");
    }
    for id in [o.warrior, o.dead_warrior, o.colonist, o.hq] {
        assert_eq!(ask(&mut sim, 0x4FF1, &[id]), 0, "{id:#x}");
    }
    sim.object_mut(oid(o.archer)).unwrap().status |= status::DIED;
    assert_eq!(ask(&mut sim, 0x4FF1, &[o.archer]), 0, "dead");
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4FF1, &[id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x1A3, &[id]), [0]);
    }
}

#[test]
fn stubbornness_query_is_true_only_for_exactly_four() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for (s, want) in [
        ([4, 0, 0], 1),
        ([5, 0, 0], 0),
        ([2, 0, 0], 0),
        ([0, 4, 0], 1),
        ([0, 0, 6], 0),
    ] {
        attack_mut(&mut sim, oid(o.warrior)).unwrap().stubbornness = s;
        assert_eq!(ask(&mut sim, 0x502D, &[ME_I, o.warrior]), want, "{s:?}");
        assert_eq!(run(&mut sim, 0x16D, &[o.warrior]), [want]);
    }
    for id in [o.dead_warrior, o.colonist, o.hq] {
        assert_eq!(ask(&mut sim, 0x502D, &[ME_I, id]), 0, "{id:#x}");
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x502D, &[3, id]), 0, "{id:#x}");
    }
    assert_eq!(ask(&mut sim, 0x502D, &[3]), 3, "wrong argc");
}

#[test]
fn gold_and_food_queries_read_attributes_26_and_24() {
    // Objects without the "current" flag report the maximum; object missing -> 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in [o.warrior, o.dead_warrior] {
        assert_eq!(ask(&mut sim, 0x50D7, &[ME_I, id]), 40, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x50DC, &[ME_I, id]), 15, "{id:#x}");
        assert_eq!(run(&mut sim, 0xBF, &[id]), [40]);
        assert_eq!(run(&mut sim, 0xC0, &[id]), [15]);
    }
    assert_eq!(
        ask(&mut sim, 0x50D7, &[ME_I, o.archer]),
        0,
        "without attribute"
    );
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x50D7, &[3, id]), 0, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x50DC, &[3, id]), 0, "{id:#x}");
    }
    assert_eq!(ask(&mut sim, 0x50D7, &[3]), 3, "wrong argc");
}

// QUIRK E-69: kept
#[test]
fn production_queries_read_the_building_module() {
    // 0xF03 (`0x4E89` GUID, `0x4F38` percent): running order; without an order or module 0 or `[2]` stays; dead
    // buildings keep module 3 (re/ki-queries 5.20) and return the frozen state.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in [o.hq, o.dead_hq] {
        // No order.
        assert_eq!(ask(&mut sim, 0x4E89, &[ME_I, id]), 0);
        assert_eq!(
            ask(&mut sim, 0x4F38, &[3, id]),
            3,
            "without an order `[2]` stays"
        );
        let b = building_mut(&mut sim, oid(id)).unwrap();
        (b.make_guid, b.make_start, b.make_time) = (1100, 15_000, 10_000);
        assert_eq!(ask(&mut sim, 0x4E89, &[ME_I, id]), 1100);
        assert_eq!(run(&mut sim, 0x131, &[id]), [1100]);
        // 1 - 10000/15000 = 0.3333... as f32, times 100.0f32, truncated.
        assert_eq!(ask(&mut sim, 0x4F38, &[3, id]), 33);
        assert_eq!(run(&mut sim, 0x137, &[id]), [33]);
        let b = building_mut(&mut sim, oid(id)).unwrap();
        b.make_time = 0;
        assert_eq!(ask(&mut sim, 0x4F38, &[3, id]), 100);
        let b = building_mut(&mut sim, oid(id)).unwrap();
        b.make_start = 0;
        assert_eq!(ask(&mut sim, 0x4F38, &[3, id]), 0, "before the start 0 %");
        let b = building_mut(&mut sim, oid(id)).unwrap();
        (b.make_guid, b.make_start, b.make_time) = (0, 0, 0);
    }
    // Unit without a building module, missing object: GUID 0, percent stays `[2]`.
    for id in [o.warrior, o.colonist] {
        assert_eq!(ask(&mut sim, 0x4E89, &[ME_I, id]), 0);
        assert_eq!(ask(&mut sim, 0x4F38, &[3, id]), 3);
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4E89, &[3, id]), 0, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x4F38, &[3, id]), 3, "{id:#x}");
        assert_eq!(run(&mut sim, 0x137, &[id]), [ME_I], "missing pushes `me`");
    }
    assert_eq!(ask(&mut sim, 0x4F38, &[3]), 3, "wrong argc");
}

// QUIRK E-69: kept
#[test]
fn building_list_of_a_unit_is_forty() {
    // E-69: without a building module (or with a switched-off one) the initial `argc` 40 of the buffer stays; dead
    // buildings return their frozen list; missing object 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in [o.warrior, o.dead_warrior, o.archer, o.colonist] {
        assert_eq!(ask(&mut sim, 0x4F42, &[ME_I, id]), 40, "{id:#x}");
        assert_eq!(run(&mut sim, 0x138, &[id]), [40]);
    }
    for id in [o.hq, o.dead_hq] {
        assert_eq!(ask(&mut sim, 0x4F42, &[ME_I, id]), 0, "empty list");
        for n in [1usize, 3, 40, 41, 100] {
            let q = &mut building_mut(&mut sim, oid(id)).unwrap().make_queue;
            q.clear();
            q.extend(std::iter::repeat_n(1100, n));
            // If the list does not fit into the 40-entry buffer, the query reports the reallocation with `n`.
            assert_eq!(ask(&mut sim, 0x4F42, &[ME_I, id]), n as i32, "{id:#x}: {n}");
        }
        assert_eq!(run(&mut sim, 0x138, &[id]), [100]);
        building_mut(&mut sim, oid(id)).unwrap().make_queue.clear();
    }
    // Module 3 switched off by a variant -> like an object without a module.
    sim.object_mut(oid(o.hq))
        .unwrap()
        .set_module_enabled(3, false);
    assert_eq!(ask(&mut sim, 0x4F42, &[ME_I, o.hq]), 40);
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x4F42, &[3, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x138, &[id]), [0]);
    }
    // Without an argc check: also with only the object in `[3]`.
    assert_eq!(
        query_once(&mut sim, ME, Sight::Omniscient, 0x4F42, &[ME_I, o.warrior]).0[2],
        40
    );
}

#[test]
fn type_module_queries_look_for_modules_3_8_and_13() {
    // `0x4F6F` building (3), `0x4F94` woodcutting (8), `0x4FC4` riding (13): `[2]` = GUID, no player; unknown GUID ->
    // 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for (code, builtin, yes, no) in [
        (0x4F6F, 0x9A, HQ, WARRIOR),
        (0x4F94, 0x141, COLONIST, HQ),
        (0x4FC4, 0x7E, RIDER, COLONIST),
    ] {
        assert_eq!(ask(&mut sim, code, &[yes as i32]), 1, "{code:#x}");
        assert_eq!(run(&mut sim, builtin, &[yes as i32]), [1], "{code:#x}");
        assert_eq!(ask(&mut sim, code, &[no as i32]), 0, "{code:#x}");
        assert_eq!(run(&mut sim, builtin, &[no as i32]), [0], "{code:#x}");
        for guid in [0, -1, 99_999, i32::MIN] {
            assert_eq!(ask(&mut sim, code, &[guid]), 0, "{code:#x} GUID {guid}");
            assert_eq!(run(&mut sim, builtin, &[guid]), [0]);
        }
        // Wrong argc leaves the GUID in place.
        assert_eq!(ask(&mut sim, code, &[yes as i32, 0]), yes as i32);
    }
}

#[test]
fn container_count_query_has_no_death_check_and_gives_minus_one_without_a_container() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    for id in [o.hq, o.dead_hq] {
        let module = sim.module_mut(oid(id), 23).unwrap();
        let ModuleKind::Quarter(q) = &mut module.kind else {
            panic!("container module")
        };
        q.cur = 3;
        assert_eq!(ask(&mut sim, 0x5113, &[ME_I, id]), 3, "{id:#x}");
        assert_eq!(run(&mut sim, 0x143, &[id]), [3]);
    }
    for id in [o.warrior, o.dead_warrior, o.colonist] {
        assert_eq!(ask(&mut sim, 0x5113, &[ME_I, id]), -1, "{id:#x}");
    }
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x5113, &[ME_I, id]), -1, "{id:#x}");
        assert_eq!(run(&mut sim, 0x143, &[id]), [-1]);
    }
}

// QUIRK E-72: kept
#[test]
fn color_queries_follow_the_display_color() {
    // E-72: the display color `GO+0x2D0` is compared, not the owner.
    let (rules, mut state) = scene();
    state.players[2].color = 5;
    state.players[3].color = 5;
    state.diplomacy.friend[2] |= 1 << 1;
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    let color = |sim: &mut Sim, id: i32, c: i32| sim.object_mut(oid(id)).unwrap().color = c;
    assert_eq!(sim.object(oid(o.warrior)).unwrap().color, 1);
    // 0x511D: color of the object == color of slot `[2]`.
    assert_eq!(ask(&mut sim, 0x511D, &[1, o.warrior]), 1);
    assert_eq!(ask(&mut sim, 0x511D, &[0, o.warrior]), 0);
    assert_eq!(
        ask(&mut sim, 0x511D, &[1, o.archer]),
        0,
        "enemy with another color"
    );
    assert_eq!(
        ask(&mut sim, 0x511D, &[1, o.dead_warrior]),
        1,
        "the dead keep their color"
    );
    color(&mut sim, o.archer, 1);
    assert_eq!(ask(&mut sim, 0x511D, &[1, o.archer]), 1, "color, not owner");
    assert_eq!(run(&mut sim, 0x147, &[o.archer]), [1]);
    // Player not in 0-7 -> color 8 (`GetPlayerColor`); an object with color 8 then matches.
    color(&mut sim, o.colonist, 8);
    for p in [8, -1, 99] {
        assert_eq!(ask(&mut sim, 0x511D, &[p, o.colonist]), 1, "slot {p}");
        assert_eq!(ask(&mut sim, 0x511D, &[p, o.warrior]), 0, "slot {p}");
    }
    // 0x5122: q = first slot with this color; result "q regards `[2]` as a friend".
    color(&mut sim, o.archer, 5);
    assert_eq!(
        ask(&mut sim, 0x5122, &[1, o.archer]),
        1,
        "q = 2, slot 2 has 1 as a friend"
    );
    assert_eq!(
        ask(&mut sim, 0x5122, &[0, o.archer]),
        0,
        "slot 2 does not have 0 as a friend"
    );
    assert_eq!(
        ask(&mut sim, 0x5122, &[1, o.warrior]),
        1,
        "own color: everyone is their own friend"
    );
    assert_eq!(
        ask(&mut sim, 0x5122, &[0, o.warrior]),
        0,
        "slot 1 has 0 as an enemy"
    );
    assert_eq!(ask(&mut sim, 0x5122, &[1, o.dead_warrior]), 1);
    assert_eq!(
        ask(&mut sim, 0x5122, &[1, o.colonist]),
        0,
        "color 8 has no slot"
    );
    assert_eq!(run(&mut sim, 0x15C, &[o.archer]), [1]);
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x511D, &[1, id]), 0, "{id:#x}");
        assert_eq!(ask(&mut sim, 0x5122, &[1, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x147, &[id]), [0]);
        assert_eq!(run(&mut sim, 0x15C, &[id]), [0]);
    }
}

// QUIRK E-55: kept
#[test]
fn last_attacker_query_counts_dead_attackers() {
    // re/ki-queries 5.14: both objects must be findable under their ID (also dead); otherwise 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    let set = |sim: &mut Sim, victim: i32, attacker: ObjectId| {
        sim.object_mut(oid(victim)).unwrap().last_attacker = attacker;
    };
    set(&mut sim, o.warrior, oid(o.archer));
    assert_eq!(ask(&mut sim, 0x50BE, &[ME_I, o.warrior]), 1);
    for b in 0x122..=0x124 {
        assert_eq!(run(&mut sim, b, &[o.warrior]), [1], "{b:#x}");
    }
    set(&mut sim, o.warrior, oid(o.dead_warrior));
    assert_eq!(
        ask(&mut sim, 0x50BE, &[ME_I, o.warrior]),
        1,
        "dead attacker counts"
    );
    set(&mut sim, o.dead_warrior, oid(o.archer));
    assert_eq!(
        ask(&mut sim, 0x50BE, &[ME_I, o.dead_warrior]),
        1,
        "dead victim counts"
    );
    for gone in o.missing() {
        set(&mut sim, o.warrior, oid(gone));
        assert_eq!(
            ask(&mut sim, 0x50BE, &[ME_I, o.warrior]),
            0,
            "attacker {gone:#x}"
        );
    }
    set(&mut sim, o.warrior, ObjectId::NONE);
    assert_eq!(
        ask(&mut sim, 0x50BE, &[ME_I, o.warrior]),
        0,
        "never attacked"
    );
    for id in o.missing() {
        assert_eq!(ask(&mut sim, 0x50BE, &[ME_I, id]), 0, "{id:#x}");
        assert_eq!(run(&mut sim, 0x122, &[id]), [0]);
    }
    assert_eq!(ask(&mut sim, 0x50BE, &[3]), 3, "wrong argc");
}

// QUIRK E-55: kept
#[test]
fn object_queries_find_dead_objects_except_alive_check() {
    // E-55: all object queries except `0x4E2A` find a dead (not yet removed) object.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    let d = o.dead_warrior;
    assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I, d]), 0);
    assert_eq!(ask(&mut sim, 0x4F65, &[ME_I, d]), 1);
    let (req, _) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E2C, &[ME_I, d, 0, 0]);
    assert_eq!(
        (req[2], req[3], req[4]),
        (1, 14, 12),
        "position of the dead"
    );
    assert_eq!(ask(&mut sim, 0x4FC9, &[d]), WARRIOR as i32);
    assert_eq!(ask(&mut sim, 0x4F2E, &[3, d]), 10);
    assert_eq!(ask(&mut sim, 0x4F9C, &[ME_I, d]), 100);
    assert_eq!(
        ask(&mut sim, 0x4E28, &[3, d, WARRIOR as i32]),
        0,
        "checked, not \"missing\""
    );
    assert_eq!(ask(&mut sim, 0x4E28, &[1, d, CORPSE_FILTER]), 1);
    sim.object_mut(oid(o.warrior)).unwrap().last_attacker = oid(d);
    assert_eq!(ask(&mut sim, 0x50BE, &[ME_I, o.warrior]), 1);
    // After removal it is a missing object.
    crate::world::destroy_object(&mut sim, oid(d));
    assert_eq!(ask(&mut sim, 0x4F65, &[ME_I, d]), 1);
    assert_eq!(ask(&mut sim, 0x4FC9, &[d]), -1);
    assert_eq!(ask(&mut sim, 0x4E28, &[3, d, CORPSE_FILTER]), 3);
    assert_eq!(ask(&mut sim, 0x50BE, &[ME_I, o.warrior]), 0);
}

#[test]
fn object_queries_under_fog_treat_unseen_foreign_objects_as_missing() {
    let rules = scene().0;
    let mut state = fogged_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let o = ids(&mut sim);
    let fog = |sim: &mut Sim, code, words: &[i32]| query_once(sim, ME, Sight::Fog, code, words).0;
    // Own objects the slot always sees, the enemy at (20, 20) lies in fog.
    assert_eq!(fog(&mut sim, 0x4E2A, &[ME_I, o.warrior])[2], 1);
    assert_eq!(fog(&mut sim, 0x4E2A, &[ME_I, o.archer])[2], 0);
    assert_eq!(fog(&mut sim, 0x4FC9, &[o.archer])[2], -1);
    assert_eq!(fog(&mut sim, 0x4FC9, &[o.warrior])[2], WARRIOR as i32);
    let req = fog(&mut sim, 0x4E2C, &[ME_I, o.archer, 0, 0]);
    assert_eq!((req[2], req[3], req[4]), (0, 0, 0));
    assert_eq!(fog(&mut sim, 0x4F79, &[3, o.archer])[2], 0);
    assert_eq!(fog(&mut sim, 0x4F2E, &[3, o.archer])[2], 3);
    // Omniscient, on the other hand, finds it.
    assert_eq!(ask(&mut sim, 0x4E2A, &[ME_I, o.archer]), 1);
}

#[test]
fn every_object_builtin_is_registered_as_a_query() {
    let ids = [
        0x6E..=0x76,
        0x78..=0x7E,
        0x9A..=0x9A,
        0xBF..=0xC0,
        0x122..=0x124,
        0x131..=0x131,
        0x137..=0x138,
        0x140..=0x141,
        0x143..=0x143,
        0x147..=0x147,
        0x15C..=0x15C,
        0x16A..=0x16D,
        0x170..=0x171,
        0x175..=0x175,
        0x179..=0x17A,
        0x18E..=0x18F,
        0x197..=0x197,
        0x19D..=0x1A0,
        0x1A3..=0x1A3,
        0x1AA..=0x1AB,
        0x1B9..=0x1BB,
    ];
    let mut n = 0;
    for r in ids {
        for id in r {
            assert!(is_query(id), "{id:#x}");
            n += 1;
        }
    }
    assert_eq!(n, 52, "number of builtins covered here");
    assert!(!is_query(0x77), "0x77 is not a world builtin");
}

/// Query codes covered here.
const CODES: [u32; 28] = [
    0x4E28, 0x4E29, 0x4E2A, 0x4E2B, 0x4E2C, 0x4E89, 0x4F2E, 0x4F38, 0x4F42, 0x4F65, 0x4F6F, 0x4F79,
    0x4F94, 0x4F9C, 0x4FA1, 0x4FAB, 0x4FB5, 0x4FC4, 0x4FC9, 0x4FF1, 0x502D, 0x5014, 0x50BE, 0x50D7,
    0x50DC, 0x5113, 0x511D, 0x5122,
];

#[test]
fn object_queries_leave_the_checksum_unchanged() {
    let (rules, mut state) = scene();
    let o = ids(&mut Sim::new(&mut state, &rules));
    // A running attack, an order and occupants: the messages hit real modules.
    {
        let mut sim = Sim::new(&mut state, &rules);
        let a = attack_mut(&mut sim, oid(o.warrior)).unwrap();
        (a.status, a.target) = (2, oid(o.archer));
        let b = building_mut(&mut sim, oid(o.hq)).unwrap();
        (b.make_guid, b.make_start, b.make_time) = (1100, 15_000, 10_000);
    }
    let (before, events) = (checksum(&state), state.events.clone());
    let mut sim = Sim::new(&mut state, &rules);
    let objects = [
        o.warrior,
        o.dead_warrior,
        o.archer,
        o.hq,
        o.dead_hq,
        o.colonist,
        o.stale,
        0,
        -1,
        i32::MIN,
        i32::MAX,
    ];
    for code in CODES {
        for &a in &objects {
            for &b in &objects {
                let words = [ME_I, a, b, b, a];
                let _ = query_once(&mut sim, ME, Sight::Omniscient, code, &words);
                let _ = query_once(&mut sim, 0, Sight::Fog, code, &words);
                let _ = query_once(&mut sim, ME, Sight::Omniscient, code, &words[1..3]);
            }
        }
    }
    let builtins = [
        0x6E, 0x71, 0x74, 0x78, 0x7B, 0x7E, 0x9A, 0xBF, 0xC0, 0x122, 0x131, 0x137, 0x138, 0x140,
        0x141, 0x143, 0x147, 0x15C, 0x16A, 0x16D, 0x170, 0x175, 0x179, 0x18E, 0x197, 0x19D, 0x19F,
        0x1A0, 0x1A3, 0x1AA, 0x1AB, 0x1B9,
    ];
    for id in builtins {
        for &v in &objects {
            let _ = run_with(&mut sim, id, &[v, v, v, v, v]);
            let _ = run_with(&mut sim, id, &[FLOOR, o.warrior, v, v]);
            let _ = run_with(&mut sim, id, &[]);
        }
    }
    for &v in &objects {
        let mut view = crate::player::View::new(&mut sim, ME, Sight::Omniscient);
        let _ = is_attacking(&mut view, ME_I, v);
    }
    assert_eq!(checksum(&state), before);
    assert_eq!(state.events, events);
}
