//! Tests of the late ability builtins `0x258…0x2F5` and of the AI check path (re/ki-queries 7.1 pattern G, 7.2, 7.4;
//! E-93 to E-96).
//!
//! Pattern as in [`super::tests_units`]: the builtin runs against the real world on a copy, the expected
//! [`CommandKind`] via `submit` with [`Origin::Ki`] on a second one (same `KiCommand` event, same checksum). For each
//! quirk E-93 to E-95 the same command is additionally run from outside ([`Origin::External`]) and is rejected as
//! usual (state unchanged).

use std::collections::BTreeSet;

use opennml_formats::temp_upgrades::parse_temp_upgrades;

use super::row;
use super::tests_units::{FLOOR, ME, ME_I, issues_on, kill, run_on, thread_with};
use crate::combat::hit_zone::world_pos;
use crate::command::{Command, CommandKind, Origin, Outcome};
use crate::id::ObjectId;
use crate::ki::signature::signature;
use crate::ki::vm::Thread;
use crate::module::ModuleKind;
use crate::module::ability_testkit as kit;
use crate::module::spell::MANA;
use crate::module::{ghosts, special, spell};
use crate::object::status;
use crate::player::submit;
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;
use crate::tick::object_tick;
use crate::upgrades::testkit as up;

/// The 31 IDs of these builtins (re/ki-queries 7.1 pattern G).
const ABILITY_IDS: [u16; 31] = [
    0x258, 0x259, 0x26C, 0x26D, 0x26E, 0x26F, 0x280, 0x281, 0x282, 0x283, 0x284, 0x294, 0x295,
    0x296, 0x297, 0x2A8, 0x2A9, 0x2AA, 0x2AB, 0x2AC, 0x2BC, 0x2BD, 0x2BE, 0x2EE, 0x2EF, 0x2F0,
    0x2F1, 0x2F2, 0x2F3, 0x2F4, 0x2F5,
];

/// These IDs (for the counter-check in `tests_units`).
pub(super) fn ability_ids() -> BTreeSet<u16> {
    ABILITY_IDS.into_iter().collect()
}

fn i(id: ObjectId) -> i32 {
    id.as_i32()
}

/// Builtin `id` with `args` gives exactly `kind` (event and effect like `submit` with [`Origin::Ki`]) and pops exactly
/// its arguments; returns result, thread and state afterwards.
fn ki_gives(
    rules: &Rules,
    base: &SimState,
    id: u16,
    args: &[i32],
    kind: CommandKind,
) -> (Outcome, Thread, SimState) {
    let (t, outcome, after) = issues_on(rules, base, id, thread_with(args), kind);
    assert_eq!(t.stack, [FLOOR], "{id:#x} pops incorrectly");
    (outcome, t, after)
}

/// Builtin `id` gives no command, leaves the state and pops exactly its arguments; returns the thread afterwards
/// (pause).
fn ki_gives_nothing(rules: &Rules, base: &SimState, id: u16, args: &[i32]) -> Thread {
    let mut state = base.clone();
    let mut t = thread_with(args);
    let (notes, issued) = run_on(&mut Sim::new(&mut state, rules), id, &mut t);
    assert!(notes.is_empty() && issued.is_empty(), "{id:#x}: {issued:?}");
    assert_eq!(t.stack, [FLOOR], "{id:#x} pops incorrectly");
    assert_eq!(checksum(&state), checksum(base), "{id:#x}");
    t
}

/// The same command from outside is rejected as usual; the state stays.
fn external_rejected(rules: &Rules, base: &SimState, kind: CommandKind) {
    let mut state = base.clone();
    let cmd = Command {
        player: ME,
        kind: kind.clone(),
    };
    let outcome = submit(&mut Sim::new(&mut state, rules), &cmd, Origin::External);
    assert!(
        matches!(outcome, Outcome::Rejected(_)),
        "{kind:?}: {outcome:?}"
    );
    assert_eq!(checksum(&state), checksum(base), "{kind:?}");
}

fn mana(rules: &Rules, state: &mut SimState, id: ObjectId) -> i32 {
    Sim::new(state, rules).attribute(id, MANA)
}

/// `n` object ticks (100 ms) only for `id`: the FIFO delivers the posted message.
fn tick_object(rules: &Rules, state: &mut SimState, id: ObjectId, n: usize) {
    let mut sim = Sim::new(state, rules);
    for _ in 0..n {
        object_tick(&mut sim, id, 100);
    }
}

fn temp_guids(state: &SimState, id: ObjectId) -> Vec<u32> {
    state
        .object(id, false)
        .expect("object")
        .temp_upgrades
        .iter()
        .map(|e| e.guid)
        .collect()
}

/// The savegame loads with a check (no value limit violated).
fn assert_loads(state: &SimState, rules: &Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules, crate::save::RulesCompare::Full)
        .expect("savegame valid");
}

// --- E-93: temporary upgrades `0x5E` --------------------------------------------------------------

const SHADOW_WARRIOR: u32 = 1714;
const SMITH: u32 = 2013;
const FLAG_BEARER: u32 = 2313;
/// Records of the execution lists (property.ini 450001): combat view, taxes, build time of the governor; shadow warrior
/// 4453, smith 4454, flag bearer 4455 (morale +), 4456 (morale -).
const BATTLE_SIGHT: u32 = 4452;
const TAXES: u32 = 4450;
const BUILD_TIME: u32 = 4451;
const SHADOW_RECORD: u32 = 4453;
const ARMOR: u32 = 4454;
const MORALE_UP: u32 = 4455;
const MORALE_DOWN: u32 = 4456;

/// Unit with Walk, Stand, Dying and module 44 with the execution list `list` (like the governor of the upgrade test
/// world, without spell 38).
fn exec_unit(guid: u32, name: &str, list: &str) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\n\
         PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n\
         VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n\
         PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n\
         PROPID 44\nVALUEID 450001 1 1 {list}\n"
    )
}

/// Upgrade test world ([`up::rules_from`]) with shadow warrior `[4453]`, smith `[4454]` and flag bearer `[4455, 4456]`
/// (each mana 200, without command bar) and the costs of the AI buttons as in `default.dat` (549: 50, 557: 90, 559: 60;
/// 560/561 test value 30; **522 without value set**). The records 4453-4456 act here on all own objects of the type
/// (handle list) so that the effect becomes visible according to the index.
fn exec_rules() -> Rules {
    let types = format!(
        "{}{}{}{}",
        up::TYPES,
        exec_unit(SHADOW_WARRIOR, "PRI_Schattenkrieger", "4453"),
        exec_unit(SMITH, "ENG_Schmied", "4454"),
        exec_unit(FLAG_BEARER, "PAT_Fahnentraeger", "4455 4456"),
    );
    let mut r = up::rules_from(&types);
    for guid in [SHADOW_WARRIOR, SMITH, FLAG_BEARER] {
        r.defaults
            .insert(guid, [(14, 1000), (15, 200)].into_iter().collect());
    }
    for (button, cost) in [(549, 50), (557, 90), (559, 60), (560, 30), (561, 30)] {
        r.defaults
            .insert(button, [(15, cost)].into_iter().collect());
    }
    let text: String = [SHADOW_RECORD, ARMOR, MORALE_UP, MORALE_DOWN]
        .iter()
        .map(|g| {
            format!("TEMPUPGRADE {g}\nTEMPHANDLELISTUPGRADE\nEFFECT 0 data/effects/t{g}.pef\nEND\n")
        })
        .collect();
    for t in parse_temp_upgrades(&text).expect("synthetic tempupgrades.ini") {
        r.defaults.insert(t.guid, [(13, 60)].into_iter().collect());
        r.upgrades.insert(t.guid);
        r.temp_upgrades.insert(t.guid, t);
    }
    r.upgrade_types.insert(SHADOW_RECORD, vec![SHADOW_WARRIOR]);
    r.upgrade_types.insert(ARMOR, vec![SMITH]);
    r.upgrade_types.insert(MORALE_UP, vec![FLAG_BEARER]);
    r.upgrade_types.insert(MORALE_DOWN, vec![FLAG_BEARER]);
    r
}

struct ExecScene {
    rules: Rules,
    base: SimState,
    governor: ObjectId,
    shadow: ObjectId,
    smith: ObjectId,
    flag: ObjectId,
    colonist: ObjectId,
    dead: ObjectId,
    foreign: ObjectId,
}

fn exec_scene() -> ExecScene {
    let rules = exec_rules();
    let mut base = up::state(&rules);
    let mut sim = Sim::new(&mut base, &rules);
    let governor = up::spawn(&mut sim, up::GOVERNOR, ME_I, (10, 10));
    let shadow = up::spawn(&mut sim, SHADOW_WARRIOR, ME_I, (30, 10));
    let smith = up::spawn(&mut sim, SMITH, ME_I, (30, 20));
    let flag = up::spawn(&mut sim, FLAG_BEARER, ME_I, (30, 30));
    let colonist = up::spawn(&mut sim, up::COLONIST, ME_I, (40, 40));
    let dead = up::spawn(&mut sim, up::GOVERNOR, ME_I, (50, 10));
    kill(&mut sim, dead);
    let foreign = up::spawn(&mut sim, up::GOVERNOR, 0, (50, 50));
    ExecScene {
        rules,
        base,
        governor,
        shadow,
        smith,
        flag,
        colonist,
        dead,
        foreign,
    }
}

fn exec(object: ObjectId, button: i32, idx: i32) -> CommandKind {
    CommandKind::ExecTempUpgrade {
        object,
        button,
        idx,
    }
}

// QUIRK E-93, K-90: kept
#[test]
fn governor_pays_by_button_and_acts_by_index() {
    // E-93: button and index do not match; paid by the button, effect by the index (list 0 combat view, 1 taxes, 2
    // build time). 543 needs 2828 in the command bar - the AI bypasses the lock.
    let s = exec_scene();
    let g = s.governor;
    for (id, button, idx, cost, guid) in [
        (0x258, 543, 0, 75, BATTLE_SIGHT),
        (0x259, 544, 1, 50, TAXES),
        (0x2F0, 545, 2, 40, BUILD_TIME),
    ] {
        let kind = exec(g, button, idx);
        let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, id, &[i(g)], kind.clone());
        assert_eq!(outcome, Outcome::Accepted, "{id:#x}");
        assert_eq!(mana(&s.rules, &mut after, g), 100 - cost, "{id:#x}");
        tick_object(&s.rules, &mut after, g, 1);
        assert_eq!(temp_guids(&after, g), [guid], "{id:#x}");
        assert_loads(&after, &s.rules);
        external_rejected(&s.rules, &s.base, kind);
    }
}

// QUIRK E-93: kept
#[test]
fn shadow_warrior_index_two_costs_mana_without_effect() {
    // E-93: `0x284` sends `{549, 2}` to a list with one entry: the mana (50) is gone, `0x12C {2}` finds no record; the
    // savegame stays valid.
    let s = exec_scene();
    let kind = exec(s.shadow, 549, 2);
    let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, 0x284, &[i(s.shadow)], kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(mana(&s.rules, &mut after, s.shadow), 150);
    tick_object(&s.rules, &mut after, s.shadow, 3);
    assert!(temp_guids(&after, s.shadow).is_empty());
    assert_loads(&after, &s.rules);
    external_rejected(&s.rules, &s.base, kind);
}

// QUIRK E-93: kept
#[test]
fn smith_button_522_is_free() {
    // E-93: `0x296` sends button 522 without a value set: Armor (index 0) without cost.
    let s = exec_scene();
    let kind = exec(s.smith, 522, 0);
    let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, 0x296, &[i(s.smith)], kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(mana(&s.rules, &mut after, s.smith), 200);
    tick_object(&s.rules, &mut after, s.smith, 1);
    assert_eq!(temp_guids(&after, s.smith), [ARMOR]);
    external_rejected(&s.rules, &s.base, kind);
}

// QUIRK E-93: kept
#[test]
fn flag_bearer_lowers_morale_twice() {
    // E-93: `0x2A8 {559, 1}` and `0x2AC {557, 1}` both act on index 1 (lower morale), but pay 60 and 90 respectively;
    // "raise morale" (index 0) is never executed by the AI. One after the other: both paid, the second entry does not
    // stack (re/temp-upgrades 3.1).
    let s = exec_scene();
    let f = s.flag;
    let mut state = s.base.clone();
    for (id, button, cost) in [(0x2A8, 559, 60), (0x2AC, 557, 90)] {
        let kind = exec(f, button, 1);
        let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, id, &[i(f)], kind.clone());
        assert_eq!(outcome, Outcome::Accepted, "{id:#x}");
        assert_eq!(mana(&s.rules, &mut after, f), 200 - cost, "{id:#x}");
        tick_object(&s.rules, &mut after, f, 1);
        assert_eq!(temp_guids(&after, f), [MORALE_DOWN], "{id:#x}");
        external_rejected(&s.rules, &s.base, kind.clone());
        let (_, _, next) = ki_gives(&s.rules, &state, id, &[i(f)], kind);
        state = next;
        tick_object(&s.rules, &mut state, f, 1);
    }
    assert_eq!(mana(&s.rules, &mut state, f), 200 - 60 - 90);
    assert_eq!(temp_guids(&state, f), [MORALE_DOWN]);
    assert!(!temp_guids(&state, f).contains(&MORALE_UP));
}

// QUIRK E-26: deviation
#[test]
fn ki_path_keeps_owner_death_and_module_checks() {
    // Ownership, death and module also apply to the AI; without module 44, dead or foreign -> rejected, state unchanged
    // (the command is still sent and reported).
    let s = exec_scene();
    for unit in [s.colonist, s.dead, s.foreign] {
        let (outcome, _, after) =
            ki_gives(&s.rules, &s.base, 0x258, &[i(unit)], exec(unit, 543, 0));
        assert!(
            matches!(outcome, Outcome::Rejected(_)),
            "{unit:?}: {outcome:?}"
        );
        assert_eq!(checksum(&after), checksum(&s.base), "{unit:?}");
    }
    // Also the spell kind 4 (`0x2BD`) needs module 38 and an index in the list (exception A7: the spell state of the
    // savegame checks it).
    let (outcome, _, _) = ki_gives(
        &s.rules,
        &s.base,
        0x2BD,
        &[i(s.shadow)],
        cast(s.shadow, 4, 0, 0, 563),
    );
    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
    let (outcome, _, after) = ki_gives(
        &s.rules,
        &s.base,
        0x2BD,
        &[i(s.governor)],
        cast(s.governor, 4, 0, 0, 563),
    );
    assert_eq!(outcome, Outcome::Accepted);
    assert_loads(&after, &s.rules);
}

// --- E-94/E-95: wrong spell buttons, special attacks, Banish, Scout area -------------------------

fn cast(caster: ObjectId, art: i32, anim: i32, idx: i32, button: i32) -> CommandKind {
    CommandKind::CastSpell {
        caster,
        art,
        anim,
        idx,
        button,
    }
}

/// Spell world ([`kit::spell_rules`]): slot [`ME`] with trapper, sharpshooter, chief, medicine man, patriot, shadow
/// warrior, smith; slot 0 with colonists (targets) and a monastery.
struct SpellScene {
    rules: Rules,
    base: SimState,
    trapper: ObjectId,
    sharpshooter: ObjectId,
    chief: ObjectId,
    medicine: ObjectId,
    patriot: ObjectId,
    shadow: ObjectId,
    smith: ObjectId,
    foe: ObjectId,
    dead_foe: ObjectId,
    monastery: ObjectId,
}

fn spell_scene() -> SpellScene {
    let rules = kit::spell_rules();
    let mut base = kit::state(&rules);
    let mut sim = Sim::new(&mut base, &rules);
    let trapper = kit::spawn(&mut sim, kit::TRAPPER, ME_I, (10, 10));
    let sharpshooter = kit::spawn(&mut sim, kit::SHARPSHOOTER, ME_I, (10, 14));
    let chief = kit::spawn(&mut sim, kit::CHIEF, ME_I, (10, 18));
    let medicine = kit::spawn(&mut sim, kit::MEDICINE_MAN, ME_I, (10, 22));
    let patriot = kit::spawn(&mut sim, kit::PATRIOT, ME_I, (10, 26));
    let shadow = kit::spawn(&mut sim, kit::SHADOW_WARRIOR, ME_I, (10, 30));
    let smith = kit::spawn(&mut sim, kit::SMITH, ME_I, (10, 34));
    let foe = kit::spawn(&mut sim, kit::COLONIST, 0, (12, 12));
    let dead_foe = kit::spawn(&mut sim, kit::COLONIST, 0, (13, 22));
    kill(&mut sim, dead_foe);
    let monastery = kit::spawn(&mut sim, kit::MONASTERY, 0, (20, 26));
    SpellScene {
        rules,
        base,
        trapper,
        sharpshooter,
        chief,
        medicine,
        patriot,
        shadow,
        smith,
        foe,
        dead_foe,
        monastery,
    }
}

fn killer_cost(state: &SimState, id: ObjectId) -> i32 {
    match &state
        .object(id, false)
        .and_then(|o| o.module(special::killer::INDEX))
        .map(|m| &m.kind)
    {
        Some(ModuleKind::Killer(k)) => k.extra_energy,
        other => panic!("no killer: {other:?}"),
    }
}

// QUIRK E-95: kept
#[test]
fn trapper_kills_with_546() {
    // E-95: `0x2F5` sends button 546 (Pistoleer) instead of 456: the trapper pays 40 instead of 60.
    let s = spell_scene();
    let kind = CommandKind::KillUnit {
        unit: s.trapper,
        target: s.foe,
        button: 546,
    };
    let args = [i(s.trapper), i(s.foe)];
    let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, 0x2F5, &args, kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    tick_object(&s.rules, &mut after, s.trapper, 1);
    assert_eq!(killer_cost(&after, s.trapper), 40);
    external_rejected(&s.rules, &s.base, kind);
}

// QUIRK E-95: kept
#[test]
fn sniper_snipes_with_546() {
    // E-95: `0x2EF` always sends 546; the sharpshooter (command bar 457) pays 40 instead of 50.
    let s = spell_scene();
    let kind = CommandKind::SnipeUnit {
        unit: s.sharpshooter,
        target: s.foe,
        button: 546,
    };
    let args = [i(s.sharpshooter), i(s.foe)];
    let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, 0x2EF, &args, kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    tick_object(&s.rules, &mut after, s.sharpshooter, 1);
    assert_eq!(killer_cost(&after, s.sharpshooter), 40);
    external_rejected(&s.rules, &s.base, kind);
}

// QUIRK E-95: kept
#[test]
fn chief_scouts_with_556() {
    // E-95: `0x2F1` always sends 556 (trapper) with 30/5000/1; the chief (command bar 484, 60) pays 25 in the command.
    let s = spell_scene();
    let kind = CommandKind::ScoutArea {
        unit: s.chief,
        x: 50,
        z: 40,
        size: 30,
        duration_ms: 5000,
        flag: 1,
        button: 556,
    };
    let (outcome, _, mut after) = ki_gives(
        &s.rules,
        &s.base,
        0x2F1,
        &[i(s.chief), 50, 40],
        kind.clone(),
    );
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(mana(&s.rules, &mut after, s.chief), 100 - 25);
    external_rejected(&s.rules, &s.base, kind);
}

// QUIRK E-95: kept
#[test]
fn storm_art_two_uses_480() {
    // E-95: `0x281` sends kind 2 (storm) with button 480 instead of 481 (same cost 80); target point the world position
    // of the target as raw `f32` bits.
    let s = spell_scene();
    let p = {
        let mut st = s.base.clone();
        world_pos(&Sim::new(&mut st, &s.rules), s.monastery).expect("target")
    };
    let kind = cast(
        s.medicine,
        2,
        p[0].to_bits() as i32,
        p[2].to_bits() as i32,
        480,
    );
    let args = [i(s.medicine), i(s.monastery)];
    let (outcome, _, mut after) = ki_gives(&s.rules, &s.base, 0x281, &args, kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    tick_object(&s.rules, &mut after, s.medicine, 1);
    let Some(ModuleKind::Spell(sp)) = after
        .object(s.medicine, false)
        .and_then(|o| o.module(spell::INDEX))
        .map(|m| &m.kind)
    else {
        panic!("no spell module");
    };
    assert_eq!((sp.art, sp.button, sp.cost), (2, 480, 80));
    external_rejected(&s.rules, &s.base, kind);
}

// QUIRK E-94: kept
#[test]
fn banish_always_uses_547() {
    // E-94: `0x2EE` always sends 547: the pilgrim (command bar 553 for 200) pays 25, the missionary (command bar 439)
    // pays as much as with his own button.
    let rules = kit::evoke_rules();
    let mut base = kit::state(&rules);
    let (pilgrim, missionary) = {
        let mut sim = Sim::new(&mut base, &rules);
        let units = (
            kit::spawn(&mut sim, kit::PILGRIM, ME_I, (10, 10)),
            kit::spawn(&mut sim, kit::MISSIONARY, ME_I, (20, 10)),
        );
        for id in [units.0, units.1] {
            let p = spell::mana_percent(&sim, id);
            let o = sim.object_mut(id).expect("object");
            o.attributes.set_current_value(MANA, 150, p);
        }
        units
    };
    for unit in [pilgrim, missionary] {
        let kind = CommandKind::Banish { unit, button: 547 };
        let (outcome, _, mut after) = ki_gives(&rules, &base, 0x2EE, &[i(unit)], kind.clone());
        assert_eq!(outcome, Outcome::Accepted, "{unit:?}");
        tick_object(&rules, &mut after, unit, 1);
        assert_eq!(mana(&rules, &mut after, unit), 150 - 25, "{unit:?}");
        let Some(ModuleKind::Banish(b)) = after
            .object(unit, false)
            .and_then(|o| o.module(ghosts::banish::INDEX))
            .map(|m| &m.kind)
        else {
            panic!("no banish module");
        };
        assert_eq!(b.spell_guid, 547);
        external_rejected(&rules, &base, kind);
    }
}

// QUIRK E-96: kept
#[test]
fn ki_skips_the_target_filter() {
    // Fog and range of the target filter (`target_fits`) are not checked by the original for the AI: Kill with 548 on a
    // target far outside sight/3 is accepted, rejected from outside.
    let s = spell_scene();
    let far = {
        let mut st = s.base.clone();
        let id = kit::spawn(&mut Sim::new(&mut st, &s.rules), kit::COLONIST, 0, (60, 60));
        (st, id)
    };
    let kind = CommandKind::KillUnit {
        unit: s.shadow,
        target: far.1,
        button: 548,
    };
    let args = [i(s.shadow), i(far.1)];
    let (outcome, _, _) = ki_gives(&s.rules, &far.0, 0x283, &args, kind.clone());
    assert_eq!(outcome, Outcome::Accepted);
    external_rejected(&s.rules, &far.0, kind);
}

// --- E-96: checks in the dispatch --------------------------------------------------------------

// QUIRK E-36, E-96: kept
#[test]
fn dynamite_sleeps_even_without_a_command() {
    // E-96: `0x2AA` sets the pause 5000 after every query, also without a command (actor missing or carries the lock
    // bit `0x800000`); `0x2AB` never sleeps. A dead actor is "present": the command goes out (the simulation rejects
    // it).
    let s = spell_scene();
    let (p, m) = (s.patriot, s.monastery);
    let mut locked = s.base.clone();
    locked.object_mut(p, false).unwrap().status |= status::ANIM_LOCKED;
    let mut dead = s.base.clone();
    dead.object_mut(p, false).unwrap().status |= status::DIED;
    for (id, pause) in [(0x2AA, 5000), (0x2AB, 0)] {
        let kind = |unit| match id {
            0x2AA => CommandKind::DynamiteBuilding {
                unit,
                target: m,
                button: 454,
            },
            _ => CommandKind::DynamiteShip {
                unit,
                target: m,
                button: 455,
            },
        };
        let (outcome, t, _) = ki_gives(&s.rules, &s.base, id, &[i(p), i(m)], kind(p));
        assert_eq!(outcome, Outcome::Accepted, "{id:#x}");
        assert_eq!(t.pause_ms, pause, "{id:#x}");
        for missing in [-1, i(s.foe) ^ 1, i(s.foe) + 0x10_0000] {
            let t = ki_gives_nothing(&s.rules, &s.base, id, &[missing, i(m)]);
            assert_eq!(t.pause_ms, pause, "{id:#x} actor {missing}");
        }
        let t = ki_gives_nothing(&s.rules, &locked, id, &[i(p), i(m)]);
        assert_eq!(t.pause_ms, pause, "{id:#x} locked");
        let (outcome, t, _) = ki_gives(&s.rules, &dead, id, &[i(p), i(m)], kind(p));
        assert!(
            matches!(outcome, Outcome::Rejected(_)),
            "{id:#x}: {outcome:?}"
        );
        assert_eq!(t.pause_ms, pause, "{id:#x} dead");
    }
}

// QUIRK E-96: kept
#[test]
fn cloak_only_for_one() {
    // E-96: `0x2F3` camouflages only for `on == 1`, otherwise uncamouflages (also 2 and -1); Start healing `0x2F2` and
    // Dig in `0x2F4` for `on ≠ 0`. Word layout `{1, id}` -> one unit.
    let rules = kit::rules();
    let mut base = kit::state(&rules);
    let (ranger, medic, assassin) = {
        let mut sim = Sim::new(&mut base, &rules);
        (
            kit::spawn(&mut sim, kit::RANGER, ME_I, (10, 10)),
            kit::spawn(&mut sim, kit::MEDIC, ME_I, (14, 10)),
            kit::spawn(&mut sim, kit::ASSASSIN, ME_I, (18, 10)),
        )
    };
    for on in [1, 0, 2, -1, i32::MAX, i32::MIN] {
        let units = vec![ranger];
        let kind = if on == 1 {
            CommandKind::Mask { units }
        } else {
            CommandKind::Unmask { units }
        };
        ki_gives(&rules, &base, 0x2F3, &[i(ranger), on], kind);
        let units = vec![medic];
        let kind = if on != 0 {
            CommandKind::StartHealing { units }
        } else {
            CommandKind::StopHealing { units }
        };
        ki_gives(&rules, &base, 0x2F2, &[i(medic), on], kind);
        let units = vec![assassin];
        let kind = if on != 0 {
            CommandKind::DigIn { units }
        } else {
            CommandKind::DigOut { units }
        };
        ki_gives(&rules, &base, 0x2F4, &[i(assassin), on], kind);
    }
    // The ranger camouflages itself (module 25 always on): accepted.
    let (outcome, _, _) = ki_gives(
        &rules,
        &base,
        0x2F3,
        &[i(ranger), 1],
        CommandKind::Mask {
            units: vec![ranger],
        },
    );
    assert_eq!(outcome, Outcome::Accepted);
}

// QUIRK E-96: kept
#[test]
fn spell_targets_dead_objects() {
    // E-96: spell kinds 1/2 take the world position of a dead (still present) target too; a missing target gives no
    // command (the stack is popped anyway).
    let s = spell_scene();
    let p = {
        let mut st = s.base.clone();
        world_pos(&Sim::new(&mut st, &s.rules), s.dead_foe).expect("corpse present")
    };
    for (id, art) in [(0x280, 1), (0x281, 2)] {
        let kind = cast(
            s.medicine,
            art,
            p[0].to_bits() as i32,
            p[2].to_bits() as i32,
            480,
        );
        let args = [i(s.medicine), i(s.dead_foe)];
        let (outcome, _, _) = ki_gives(&s.rules, &s.base, id, &args, kind);
        assert_eq!(outcome, Outcome::Accepted, "{id:#x}");
        for missing in [-1, i(s.foe) ^ 1, i(s.foe) + 0x10_0000] {
            ki_gives_nothing(&s.rules, &s.base, id, &[i(s.medicine), missing]);
        }
    }
}

// QUIRK E-96: kept
#[test]
fn damage_building_needs_module_3_at_the_target() {
    // `0x295`: only a target with building module 3 (present and switched on); second word the ID of the found object.
    // No target filter for the AI (from outside an enemy building in the fog would be necessary).
    let s = spell_scene();
    let kind = CommandKind::DamageBuilding {
        unit: s.smith,
        target: s.monastery,
        button: 551,
    };
    let args = [i(s.smith), i(s.monastery)];
    let (outcome, _, _) = ki_gives(&s.rules, &s.base, 0x295, &args, kind);
    assert_eq!(outcome, Outcome::Accepted);
    for target in [i(s.foe), i(s.dead_foe), i(s.foe) ^ 1, -1] {
        ki_gives_nothing(&s.rules, &s.base, 0x295, &[i(s.smith), target]);
    }
}

// --- Coverage, stack, signatures --------------------------------------------------------------------

#[test]
fn every_ability_row_gives_its_command() {
    // Each of the 31 rows builds the command from table 2 (word layout, button, index).
    let mut covered: BTreeSet<u16> = BTreeSet::new();
    let mut check = |rules: &Rules, base: &SimState, id: u16, args: &[i32], kind: CommandKind| {
        ki_gives(rules, base, id, args, kind);
        covered.insert(id);
    };
    let e = exec_scene();
    let g = e.governor;
    for (id, button, idx, unit) in [
        (0x258, 543, 0, g),
        (0x259, 544, 1, g),
        (0x284, 549, 2, e.shadow),
        (0x296, 522, 0, e.smith),
        (0x2A8, 559, 1, e.flag),
        (0x2A9, 560, 0, g),
        (0x2AC, 557, 1, e.flag),
        (0x2BC, 561, 0, g),
        (0x2F0, 545, 2, g),
    ] {
        check(&e.rules, &e.base, id, &[i(unit)], exec(unit, button, idx));
    }
    check(&e.rules, &e.base, 0x2BD, &[i(g)], cast(g, 4, 0, 0, 563));

    let s = spell_scene();
    let (target, p) = {
        let mut st = s.base.clone();
        let p = world_pos(&Sim::new(&mut st, &s.rules), s.foe).expect("target");
        (s.foe, p)
    };
    let (x, z) = (p[0].to_bits() as i32, p[2].to_bits() as i32);
    let m = s.medicine;
    check(
        &s.rules,
        &s.base,
        0x280,
        &[i(m), i(target)],
        cast(m, 1, x, z, 480),
    );
    check(
        &s.rules,
        &s.base,
        0x281,
        &[i(m), i(target)],
        cast(m, 2, x, z, 480),
    );
    check(&s.rules, &s.base, 0x282, &[i(m)], cast(m, 3, 0, 0, 482));
    let special = |unit, button, id: u16| match id {
        0x283 | 0x2F5 => CommandKind::KillUnit {
            unit,
            target,
            button,
        },
        0x2EF => CommandKind::SnipeUnit {
            unit,
            target,
            button,
        },
        0x2AA => CommandKind::DynamiteBuilding {
            unit,
            target,
            button,
        },
        _ => CommandKind::DynamiteShip {
            unit,
            target,
            button,
        },
    };
    for (id, unit, button) in [
        (0x283, s.shadow, 548),
        (0x2F5, s.trapper, 546),
        (0x2EF, s.sharpshooter, 546),
        (0x2AA, s.patriot, 454),
        (0x2AB, s.patriot, 455),
    ] {
        check(
            &s.rules,
            &s.base,
            id,
            &[i(unit), i(target)],
            special(unit, button, id),
        );
    }
    check(
        &s.rules,
        &s.base,
        0x294,
        &[i(s.smith)],
        CommandKind::Whirlwind {
            unit: s.smith,
            button: 542,
        },
    );
    check(
        &s.rules,
        &s.base,
        0x295,
        &[i(s.smith), i(s.monastery)],
        CommandKind::DamageBuilding {
            unit: s.smith,
            target: s.monastery,
            button: 551,
        },
    );
    check(
        &s.rules,
        &s.base,
        0x2F1,
        &[i(s.chief), 7, 9],
        CommandKind::ScoutArea {
            unit: s.chief,
            x: 7,
            z: 9,
            size: 30,
            duration_ms: 5000,
            flag: 1,
            button: 556,
        },
    );

    let ev = kit::evoke_rules();
    let mut eb = kit::state(&ev);
    let (ghost_evoker, animal_evoker, pilgrim) = {
        let mut sim = Sim::new(&mut eb, &ev);
        (
            kit::spawn(&mut sim, kit::GHOST_EVOKER, ME_I, (10, 10)),
            kit::spawn(&mut sim, kit::ANIMAL_EVOKER, ME_I, (14, 10)),
            kit::spawn(&mut sim, kit::PILGRIM, ME_I, (18, 10)),
        )
    };
    check(
        &ev,
        &eb,
        0x26C,
        &[i(ghost_evoker)],
        CommandKind::EvokeGhost {
            unit: ghost_evoker,
            button: 436,
        },
    );
    for (id, art, button) in [(0x26D, 0, 441), (0x26E, 1, 442), (0x26F, 2, 443)] {
        let kind = CommandKind::EvokeAnimal {
            unit: animal_evoker,
            art,
            button,
        };
        check(&ev, &eb, id, &[i(animal_evoker)], kind);
    }
    check(
        &ev,
        &eb,
        0x297,
        &[i(pilgrim)],
        CommandKind::SelfHeal {
            unit: pilgrim,
            button: 555,
        },
    );
    check(
        &ev,
        &eb,
        0x2EE,
        &[i(pilgrim)],
        CommandKind::Banish {
            unit: pilgrim,
            button: 547,
        },
    );
    check(
        &ev,
        &eb,
        0x2F2,
        &[i(pilgrim), 1],
        CommandKind::StartHealing {
            units: vec![pilgrim],
        },
    );
    check(
        &ev,
        &eb,
        0x2F3,
        &[i(pilgrim), 1],
        CommandKind::Mask {
            units: vec![pilgrim],
        },
    );
    check(
        &ev,
        &eb,
        0x2F4,
        &[i(pilgrim), 0],
        CommandKind::DigOut {
            units: vec![pilgrim],
        },
    );

    let hr = kit::hunter_rules();
    let mut hb = kit::hunter_state(&hr);
    let undertaker = kit::spawn(&mut Sim::new(&mut hb, &hr), kit::UNDERTAKER, ME_I, (10, 10));
    check(
        &hr,
        &hb,
        0x2BE,
        &[i(undertaker)],
        CommandKind::RobCorpses {
            unit: undertaker,
            button: 540,
        },
    );
    assert_eq!(covered, ability_ids());
}

// QUIRK E-34: deviation
#[test]
fn ability_rows_pop_exactly_their_pops_even_when_rejecting() {
    // Each row takes exactly its arguments from the stack and pushes nothing - also on rejection, missing, dead or
    // foreign object and nonsense values; pause only for `0x2AA`.
    let s = spell_scene();
    let values = [0, -1, 1, 2, 9, 31, 1000, i32::MAX, i32::MIN];
    let objects = [
        s.trapper,
        s.patriot,
        s.medicine,
        s.smith,
        s.foe,
        s.dead_foe,
        s.monastery,
    ];
    for id in ABILITY_IDS {
        let r = row(id).expect("row");
        assert!(!r.push && r.outs.is_empty(), "{id:#x}");
        let n = usize::from(r.pops);
        let mut sets: Vec<Vec<i32>> = values.iter().map(|&v| vec![v; n]).collect();
        for &v in &values {
            for at in 0..n {
                let mut a = vec![i(s.patriot); n];
                a[at] = v;
                sets.push(a);
            }
        }
        for obj in objects {
            for at in 0..n {
                let mut a = vec![1; n];
                a[at] = i(obj);
                sets.push(a);
            }
        }
        for args in sets {
            let mut state = s.base.clone();
            let mut sim = Sim::new(&mut state, &s.rules);
            let mut t = thread_with(&args);
            let (notes, issued) = run_on(&mut sim, id, &mut t);
            assert_eq!(t.stack, [FLOOR], "{id:#x} {args:?}");
            assert!(notes.is_empty(), "{id:#x} {args:?}: {notes:?}");
            assert!(issued.len() <= 1, "{id:#x} {args:?}: {issued:?}");
            let pause = if id == 0x2AA { 5000 } else { 0 };
            assert_eq!(t.pause_ms, pause, "{id:#x} {args:?}");
        }
    }
}

#[test]
fn ability_rows_match_the_e1a_signatures() {
    // All 31 IDs are in the corpus and have a signature: same pops, push, outputs, pause and the same query code (table
    // 2 `0x503C … 0x512F`).
    for id in ABILITY_IDS {
        let r = row(id).unwrap_or_else(|| panic!("{id:#x} without row"));
        let sig = signature(id).unwrap_or_else(|| panic!("{id:#x} without signature"));
        assert_eq!(
            (r.pops, r.push, r.outs.is_empty(), r.pause_ms, r.code),
            (
                sig.pops,
                sig.push,
                sig.outs.is_empty(),
                sig.pause_ms,
                sig.code
            ),
            "{id:#x}"
        );
    }
    assert!(ABILITY_IDS.windows(2).all(|w| w[0] < w[1]));
}
