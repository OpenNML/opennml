//! Tests of the world actions (`script::world`), with synthetic worlds without a corpus.

use nml_formats::abl::RtAction;

use super::actions::{Dispatched, build_msg, dispatch};
use super::query_tests::{container, traces};
use super::tests::{rules_with_areas, script, setup};
use super::world::create::{place_named, spawn_at_tile, spawn_in_area};
use super::*;
use crate::combat::testkit as ck;
use crate::id::ObjectId;
use crate::module::attack::attack;
use crate::names;
use crate::rules::Rules;
use crate::setup::new_match;
use crate::sim::Sim;
use crate::state::{Control, SimEvent, SimState};
use crate::tick::step;
use crate::upgrades::objects_of_slot;

/// Rider type of the test world: translated into a fighter (`SABRE`) and a horse (`PIKEMAN`).
const RIDER: u32 = 9100;
/// `0.017453292f32` (`[0x00616254]`).
const DEG_TO_RAD: f32 = f32::from_bits(0x3C8E_FA35);
/// `pi` and `1/180` as `f32` (`[0x00618A4C]`, `[0x0061900C]`).
const PI: f32 = f32::from_bits(0x4049_0FDB);
const INV_180: f32 = f32::from_bits(0x3BB6_0B61);

/// Areas of the test map (64 x 64 tiles): rectangles `[x0, z0, x1, z1]`.
const AREAS: [(&str, [i32; 4]); 8] = [
    ("drei", [2, 2, 4, 4]),
    ("zwei", [2, 2, 3, 3]),
    ("flach", [2, 2, 6, 2]),
    ("verdreht", [5, 5, 2, 2]),
    ("negativ", [-3, -3, 0, 0]),
    ("rand", [62, 5, 65, 5]),
    ("einzeln", [10, 10, 10, 10]),
    ("fern", [30, 30, 33, 30]),
];

fn rules() -> Rules {
    let mut rules = ck::rules_with("");
    rules.scripts = testing::rules_with("", &[], &AREAS);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    rules
        .translations
        .insert(RIDER, vec![ck::SABRE, ck::PIKEMAN]);
    rules
}

/// Empty world; slot 0 is a human, slot 1 an AI, slot 2 a passive AI, the others are empty.
fn state(rules: &Rules) -> SimState {
    let mut s = ck::state(rules);
    s.players[0].control = Control::Human;
    s.players[1].control = Control::Ai;
    s.players[2].control = Control::Passive;
    s
}

/// The objects created since the last call, in creation order.
fn created(sim: &mut Sim) -> Vec<ObjectId> {
    std::mem::take(&mut sim.state.events)
        .into_iter()
        .filter_map(|e| match e {
            SimEvent::Created { id } => Some(id),
            _ => None,
        })
        .collect()
}

fn xz(sim: &Sim, id: ObjectId) -> (f32, f32) {
    let o = sim.object(id).expect("object");
    (o.world_pos[0], o.world_pos[2])
}

/// An action via layer D ([`dispatch`]), message per `actions::message`.
fn act(sim: &mut Sim, number: i32, params: &[&str]) -> Dispatched {
    let action = RtAction {
        wait: false,
        number,
        params: params.iter().map(|p| (*p).to_owned()).collect(),
        line: 1,
    };
    let mut msg = build_msg(sim, &action);
    dispatch(sim, &mut container(), &mut msg)
}

// QUIRK E-150: kept
#[test]
fn place_151_rounds_centre_towards_zero_and_adds_one_from_3x3() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cell = 128.0;
    let place = |sim: &mut Sim, area: &str| {
        let id = place_named(sim, 0, ck::SABRE as i32, area, "ziel", -1);
        id.map(|id| xz(sim, id))
    };
    // 3 x 3 tiles: center (3, 3) + 1 = the corner (x1, z1).
    assert_eq!(place(&mut sim, "drei"), Some((4.5 * cell, 4.5 * cell)));
    // 2 x 2: center (2, 2) (5 / 2 rounded toward 0), no addition.
    assert_eq!(place(&mut sim, "zwei"), Some((2.5 * cell, 2.5 * cell)));
    // The addition requires both sides above 1: here only the width (5 tiles), the height is 1.
    assert_eq!(place(&mut sim, "flach"), Some((4.5 * cell, 2.5 * cell)));
    // Swapped corners are not normalized: the differences are negative, no addition.
    assert_eq!(place(&mut sim, "verdreht"), Some((3.5 * cell, 3.5 * cell)));
    // Negative sum -3: rounded toward 0 -1, +1 = 0 (rounded down it would be tile -1: not created).
    assert_eq!(place(&mut sim, "negativ"), Some((0.5 * cell, 0.5 * cell)));
    // A single tile.
    assert_eq!(place(&mut sim, "einzeln"), Some((10.5 * cell, 10.5 * cell)));
}

// QUIRK E-150: kept
#[test]
fn place_151_draws_one_random_angle_217_none() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let guid = ck::SABRE as i32;
    let rotation =
        |sim: &Sim, id: Option<ObjectId>| sim.object(id.expect("object")).unwrap().rotation[1];

    // 151: exactly one draw, `rand() % 360` degrees, rotation `degrees * 0.017453292f`.
    let before = sim.state.script.rand;
    let mut expected = before;
    let degrees = expected.rand() % 360;
    let id = place_named(&mut sim, 0, guid, "drei", "a", -1);
    assert_eq!(sim.state.script.rand, expected, "exactly one draw");
    assert_eq!(rotation(&sim, id), degrees as f32 * DEG_TO_RAD);

    // 217 with angle >= 0: no draw, the angle is not reduced to 360.
    let before = sim.state.script.rand;
    let id = place_named(&mut sim, 0, guid, "drei", "b", 450);
    assert_eq!(sim.state.script.rand, before);
    assert_eq!(rotation(&sim, id), 450.0 * DEG_TO_RAD);
    let id = place_named(&mut sim, 0, guid, "drei", "c", 0);
    assert_eq!(sim.state.script.rand, before);
    assert_eq!(rotation(&sim, id), 0.0);

    // 217 with angle < 0 draws like 151 (via layer D: value slot 2 of the message).
    let mut expected = sim.state.script.rand;
    let degrees = expected.rand() % 360;
    assert_eq!(
        act(&mut sim, 217, &["0", "1107", "d", "area drei", "-5"]),
        Dispatched::Done
    );
    assert_eq!(sim.state.script.rand, expected);
    let d = names::first_id(&sim.state.names, "d");
    assert_eq!(rotation(&sim, d), degrees as f32 * DEG_TO_RAD);
}

#[test]
fn place_151_registers_name_and_horse_suffix() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Ordinary type: the name points to the object (entered in lowercase).
    let plain = place_named(&mut sim, 0, ck::SABRE as i32, "drei", "Bote", -1).unwrap();
    assert_eq!(names::first_id(&sim.state.names, "bote"), Some(plain));
    assert_eq!(sim.object(plain).unwrap().guid, ck::SABRE);
    // Rider type: the return value is the rider; the horse is named `name + "pferd"`.
    let rider = place_named(&mut sim, 0, RIDER as i32, "drei", "reiter", -1).unwrap();
    let horse = names::first_id(&sim.state.names, "reiterpferd").expect("horse");
    assert_eq!(names::first_id(&sim.state.names, "reiter"), Some(rider));
    assert_eq!(sim.object(rider).unwrap().guid, ck::SABRE);
    assert_eq!(sim.object(horse).unwrap().guid, ck::PIKEMAN);
    // Empty name: no entry, not even a "pferd".
    let before = sim.state.names.len();
    place_named(&mut sim, 0, RIDER as i32, "drei", "", -1).unwrap();
    assert_eq!(sim.state.names.len(), before);
    // If creating fails (unknown type), there is neither an object nor a name.
    let count = objects_of_slot(sim.state, 0).len();
    assert_eq!(place_named(&mut sim, 0, 4242, "drei", "leer", -1), None);
    assert_eq!(names::first_id(&sim.state.names, "leer"), None);
    assert_eq!(objects_of_slot(sim.state, 0).len(), count);
}

#[test]
fn place_unknown_area_does_nothing_and_draws_nothing() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let rand = sim.state.script.rand;
    let names_before = sim.state.names.len();
    created(&mut sim);
    assert_eq!(
        place_named(&mut sim, 1, ck::SABRE as i32, "nirgends", "x", -1),
        None
    );
    assert_eq!(
        act(&mut sim, 151, &["1", "1107", "x", "area nirgends"]),
        Dispatched::Done
    );
    assert_eq!(
        act(&mut sim, 217, &["1", "1107", "-1", "x", "area nirgends"]),
        Dispatched::Done
    );
    spawn_in_area(&mut sim, 1, 3, ck::SABRE as i32, "nirgends", Some("x"));
    assert_eq!(sim.state.script.rand, rand, "no randomness consumed");
    assert_eq!(
        sim.state.names.len(),
        names_before,
        "no name, not even an empty one"
    );
    assert!(created(&mut sim).is_empty());
}

// QUIRK E-150: kept
#[test]
fn spawn_3_walks_tiles_row_by_row_and_wraps() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let rand = sim.state.script.rand;
    created(&mut sim);
    // 2 x 2 tiles, six units: row by row, then from the start again.
    spawn_in_area(&mut sim, 0, 6, ck::SABRE as i32, "zwei", None);
    let ids = created(&mut sim);
    let tiles = [(2, 2), (3, 2), (2, 3), (3, 3), (2, 2), (3, 2)];
    assert_eq!(ids.len(), 6);
    for (id, (tx, tz)) in ids.iter().zip(tiles) {
        // Position `cell * tile + 10`, not the tile center.
        assert_eq!(
            xz(&sim, *id),
            ((128 * tx) as f32 + 10.0, (128 * tz) as f32 + 10.0)
        );
        // The rotation is never written: 0.
        assert_eq!(sim.object(*id).unwrap().rotation[1], 0.0);
        assert_eq!(id.slot(), 0);
    }
    assert_eq!(sim.state.script.rand, rand, "3 draws nothing");
    // Swapped corners are normalized: the same start as with "zwei".
    spawn_in_area(&mut sim, 0, 2, ck::SABRE as i32, "verdreht", None);
    let ids = created(&mut sim);
    assert_eq!(xz(&sim, ids[0]), (266.0, 266.0));
    assert_eq!(xz(&sim, ids[1]), (394.0, 266.0));
    // Count <= 0: nothing.
    spawn_in_area(&mut sim, 0, 0, ck::SABRE as i32, "zwei", None);
    spawn_in_area(&mut sim, 0, -4, ck::SABRE as i32, "zwei", None);
    assert!(created(&mut sim).is_empty());
}

// QUIRK E-150: kept
#[test]
fn spawn_3_stops_at_first_failure() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    created(&mut sim);
    // Tiles 62, 63 lie on the 64 x 64 map, 64 does not (x = 8202 >= 8192): after that it is over.
    spawn_in_area(&mut sim, 0, 5, ck::SABRE as i32, "rand", None);
    assert_eq!(created(&mut sim).len(), 2);
    // Unknown type: over at the first attempt.
    spawn_in_area(&mut sim, 0, 5, 4242, "zwei", None);
    assert!(created(&mut sim).is_empty());
}

/// Value of the name `name` in the name table.
fn value(sim: &Sim, name: &str) -> Option<String> {
    sim.state.names.get(name).map(str::to_owned)
}

// QUIRK E-195: kept
#[test]
fn spawn_396_appends_new_ids_after_existing_ones() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    created(&mut sim);
    let guid = ck::SABRE as i32;
    // Existing IDs first (text order), then the new ones in creation order, `;` in between.
    sim.state.names.set("trupp", "7;9".to_owned());
    spawn_in_area(&mut sim, 0, 2, guid, "zwei", Some("Trupp"));
    let ids = created(&mut sim);
    assert_eq!(
        value(&sim, "trupp"),
        Some(format!("7;9;{};{}", ids[0].as_i32(), ids[1].as_i32()))
    );
    // Once more: appended at the end, nothing overwritten.
    spawn_in_area(&mut sim, 0, 1, guid, "zwei", Some("trupp"));
    let third = created(&mut sim)[0];
    assert_eq!(
        value(&sim, "trupp"),
        Some(format!(
            "7;9;{};{};{}",
            ids[0].as_i32(),
            ids[1].as_i32(),
            third.as_i32()
        ))
    );
    // New name: without a leading `;`.
    spawn_in_area(&mut sim, 0, 2, guid, "zwei", Some("neu"));
    let ids = created(&mut sim);
    assert_eq!(
        value(&sim, "neu"),
        Some(format!("{};{}", ids[0].as_i32(), ids[1].as_i32()))
    );
    // If creating fails, the name is not written at all (E-195): the old value is not extended and
    // no new entry is created - not even for the units before.
    sim.state.names.set("alt", "5".to_owned());
    spawn_in_area(&mut sim, 0, 5, guid, "rand", Some("alt"));
    spawn_in_area(&mut sim, 0, 5, guid, "rand", Some("frisch"));
    assert_eq!(created(&mut sim).len(), 4);
    assert_eq!(value(&sim, "alt"), Some("5".to_owned()));
    assert_eq!(value(&sim, "frisch"), None);
    // Count <= 0: the old value is only rewritten (via `atol` and `Format`), an unknown name gets the
    // empty value.
    sim.state.names.set("roh", "7; 9;x".to_owned());
    spawn_in_area(&mut sim, 0, 0, guid, "zwei", Some("roh"));
    spawn_in_area(&mut sim, 0, 0, guid, "zwei", Some("leer"));
    assert_eq!(value(&sim, "roh"), Some("7;9;0".to_owned()));
    assert_eq!(value(&sim, "leer"), Some(String::new()));
    assert!(created(&mut sim).is_empty());
    // Via layer D: value slots `{slot, count, guid, area, name}`.
    assert_eq!(
        act(&mut sim, 396, &["0", "1", "1107", "area zwei", "dsp"]),
        Dispatched::Done
    );
    let id = created(&mut sim)[0];
    assert_eq!(value(&sim, "dsp"), Some(id.as_i32().to_string()));
}

// QUIRK E-150: kept
#[test]
fn spawn_14_uses_slot_8_and_draws_before_creating() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    created(&mut sim);
    let rotation_for = |r: i32| ((r % 180) as f32 * PI) * INV_180;

    // 14 via layer D: `{guid, x, z}`, the slot is fixed at 8.
    let mut expected = sim.state.script.rand;
    let r = expected.rand();
    assert_eq!(act(&mut sim, 14, &["1107", "3", "4"]), Dispatched::Done);
    let id = created(&mut sim)[0];
    assert_eq!(id.slot(), 8);
    assert_eq!(sim.state.script.rand, expected, "exactly one draw");
    // Tile corner `cell * tile` (no +0.5, no +10), rotation 0...179 degrees in radians.
    assert_eq!(xz(&sim, id), (384.0, 512.0));
    assert_eq!(sim.object(id).unwrap().rotation[1], rotation_for(r));
    assert_eq!(sim.object(id).unwrap().color, 8);

    // A failure (tile 64 lies outside) has drawn beforehand as well.
    let mut expected = sim.state.script.rand;
    expected.rand();
    assert_eq!(spawn_at_tile(&mut sim, 8, ck::SABRE as i32, 64, 4), None);
    assert_eq!(sim.state.script.rand, expected);
    assert!(created(&mut sim).is_empty());

    // 36 `{guid, x, z, slot}`: slot 2 with its color.
    let mut expected = sim.state.script.rand;
    let r = expected.rand();
    assert_eq!(
        act(&mut sim, 36, &["1107", "5", "6", "2"]),
        Dispatched::Done
    );
    let id = created(&mut sim)[0];
    assert_eq!(id.slot(), 2);
    assert_eq!(xz(&sim, id), (640.0, 768.0));
    assert_eq!(sim.object(id).unwrap().rotation[1], rotation_for(r));
    assert_eq!(sim.object(id).unwrap().color, sim.state.players[2].color);
    // Slot 9 (invalid): the object is created on slot 0 with color 8.
    act(&mut sim, 36, &["1107", "7", "7", "9"]);
    let id = created(&mut sim)[0];
    assert_eq!(id.slot(), 0);
    assert_eq!(sim.object(id).unwrap().color, 8);
}

/// Aggressiveness, stance and stubbornness of the attack module.
fn posture(sim: &Sim, id: ObjectId) -> (i32, i32, i32) {
    let a = attack(sim, id).expect("attack module");
    (a.aggressiveness, a.mode, a.stubbornness[0])
}

#[test]
fn ai_posture_message_for_non_human_slots_mode_2_and_3() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let guid = ck::SABRE as i32;
    let human = place_named(&mut sim, 0, guid, "drei", "mensch", -1).unwrap();
    assert_ne!(posture(&sim, human), (4, 3, 1));

    // Stance 2: `0x611 {4, 3, 1, 0}` for AI (1), passive AI (2) and empty slots (3), not for humans.
    for slot in [1, 2, 3] {
        sim.state.combat.behavior_mode[slot as usize] = 2;
        let id = place_named(&mut sim, slot, guid, "drei", "", -1).unwrap();
        assert_eq!(posture(&sim, id), (4, 3, 1), "slot {slot}");
    }
    // Stance 3: `{4, 3, 2, 0}`.
    for slot in [1, 2, 3] {
        sim.state.combat.behavior_mode[slot as usize] = 3;
        let id = place_named(&mut sim, slot, guid, "drei", "", -1).unwrap();
        assert_eq!(posture(&sim, id), (4, 3, 2), "slot {slot}");
    }
    // Human slot: no message, not even with stance 2 and 3 (the module only sets its mode).
    for mode in [2, 3] {
        sim.state.combat.behavior_mode[0] = mode;
        let id = place_named(&mut sim, 0, guid, "drei", "", -1).unwrap();
        assert_ne!(posture(&sim, id), (4, 3, mode - 1), "stance {mode}");
    }
    // Other stances (0, 1, 4) send nothing additional.
    for mode in [0, 1, 4] {
        sim.state.combat.behavior_mode[1] = mode;
        let id = place_named(&mut sim, 1, guid, "drei", "", -1).unwrap();
        assert_eq!(posture(&sim, id).1, mode, "stance {mode}");
        assert_ne!(posture(&sim, id), (4, 3, 1));
        assert_ne!(posture(&sim, id), (4, 3, 2));
    }
    // 3/396: the stance is read once before the loop and applies to all new units.
    sim.state.combat.behavior_mode[1] = 2;
    spawn_in_area(&mut sim, 1, 3, guid, "zwei", None);
    let ids = created(&mut sim);
    let last = ids[ids.len() - 3..].to_vec();
    assert!(last.iter().all(|id| posture(&sim, *id) == (4, 3, 1)));
    // Slot 8 (neutral, 14) and slots outside 0...7 never get a stance: the same values as with an
    // object that is created without a script.
    sim.state.combat.behavior_mode[8] = 2;
    sim.state.combat.behavior_mode[0] = 2;
    let neutral = spawn_at_tile(&mut sim, 8, guid, 20, 20).unwrap();
    let plain = ck::spawn(&mut sim, ck::SABRE, 8, (22, 22));
    assert_eq!(posture(&sim, neutral), posture(&sim, plain));
    let wild = spawn_at_tile(&mut sim, 9, guid, 21, 21).unwrap();
    let plain = ck::spawn(&mut sim, ck::SABRE, 0, (23, 23));
    assert_eq!(posture(&sim, wild), posture(&sim, plain));
}

#[test]
fn all_creation_actions_are_done_in_a_running_script() {
    // A match with a level script: 151, 217, 3, 396, 14 and 36 run without "no effect yet".
    let level = script(&[(
        &[],
        &[
            "n151;0;1100;bote;area feld",
            "n217;0;1100;bote2;area feld;90",
            "n3;0;2;1100;area feld",
            "n396;0;2;1100;area feld;trupp",
            "n14;1100;5;5",
            "n36;1100;6;6;0",
        ],
    )]);
    let rules = rules_with_areas(&level, &[("feld", [2, 2, 5, 5])]);
    let mut state = new_match(&rules, &setup(true)).expect("match with script");
    let events = step(&mut state, &rules, &[]);
    let created = events
        .iter()
        .filter(|e| matches!(e, SimEvent::Created { .. }))
        .count();
    let traces = traces(events);
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
        "{traces:?}"
    );
    assert_eq!(created, 1 + 1 + 2 + 2 + 1 + 1);
    assert!(names::first_id(&state.names, "bote").is_some());
    assert!(names::first_id(&state.names, "bote2").is_some());
    assert_eq!(names::ids(&state.names, "trupp").len(), 2);
}

// ---------------------------------------------------------------------------------------------
// Removing, ownership, hit points, attributes, clearing the event bit

/// Objects of the slot in table order.
fn of_slot(sim: &Sim, slot: u8) -> Vec<ObjectId> {
    objects_of_slot(sim.state, slot)
}

/// Tick end: destroy list and change of owner (like `world_tick` phase 0).
fn flush(sim: &mut Sim) {
    crate::world::process_destroy_queue(sim);
    crate::world::process_owner_changes(sim);
}

/// Set the name of the name table to the IDs.
fn name_ids(sim: &mut Sim, name: &str, ids: &[ObjectId]) {
    let list = ids
        .iter()
        .map(|id| id.as_i32().to_string())
        .collect::<Vec<_>>()
        .join(";");
    sim.state.names.set(name, list);
}

/// The action must be executed (no "no effect yet").
fn act_done(sim: &mut Sim, number: i32, params: &[&str]) {
    assert_eq!(
        act(sim, number, params),
        Dispatched::Done,
        "action {number}"
    );
}

// QUIRK E-149: kept
#[test]
fn remove_6_195_196_queue_without_death_or_bit_2() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let guid = ck::SABRE;
    for number in [6, 195, 196] {
        // "drei" = tiles 2...4: two own units, a foreign one, an own one outside.
        let inside = [
            ck::spawn(&mut sim, guid, 0, (3, 3)),
            ck::spawn(&mut sim, guid, 0, (2, 4)),
        ];
        let foreign = ck::spawn(&mut sim, guid, 1, (3, 2));
        let outside = ck::spawn(&mut sim, guid, 0, (10, 10));
        act_done(&mut sim, number, &["0", "1107", "area drei"]);
        // Only queued, nothing destroyed yet.
        assert!(sim.object(inside[0]).is_some());
        let queued = sim.state.destroy_queue.clone();
        assert_eq!(queued.len(), 2, "action {number}");
        assert!(inside.iter().all(|id| queued.contains(id)));
        crate::world::process_destroy_queue(&mut sim);
        // Without death: gone, without event bit 2 (unit died).
        for id in inside {
            assert!(sim.object(id).is_none());
            assert_eq!(crate::world::object_event_bits(&sim, id) & 4, 0);
        }
        assert!(sim.object(foreign).is_some() && sim.object(outside).is_some());
        // Cleaning up for the next pass.
        crate::world::queue_destroy(&mut sim, foreign);
        crate::world::queue_destroy(&mut sim, outside);
        crate::world::process_destroy_queue(&mut sim);
    }
}

// QUIRK E-149: kept
#[test]
fn remove_6_unknown_filter_or_area_does_nothing() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    // Unknown area; filter ID < 1000 without an entry (0 and 999); GUID on another slot.
    act_done(&mut sim, 6, &["0", "1107", "area nirgends"]);
    act_done(&mut sim, 6, &["0", "0", "area drei"]);
    act_done(&mut sim, 6, &["0", "999", "area drei"]);
    act_done(&mut sim, 6, &["3", "1107", "area drei"]);
    // Swapped corners: empty (E-190).
    act_done(&mut sim, 6, &["0", "1107", "area verdreht"]);
    assert!(sim.state.destroy_queue.is_empty());
}

// QUIRK E-149, E-184: kept
#[test]
fn remove_194_queues_stale_ids_silently() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    let stale = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let b = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
    crate::world::queue_destroy(&mut sim, stale);
    crate::world::process_destroy_queue(&mut sim);
    assert!(sim.object(stale).is_none());
    // The name points to living, stale and empty pieces (empty = ID 0): all are queued.
    sim.state.names.set(
        "trupp",
        format!("{};{};;{}", a.as_i32(), stale.as_i32(), b.as_i32()),
    );
    let events = sim.state.events.len();
    act_done(&mut sim, 194, &["Trupp"]);
    assert_eq!(sim.state.destroy_queue.len(), 4);
    crate::world::process_destroy_queue(&mut sim);
    assert!(sim.object(a).is_none() && sim.object(b).is_none());
    // No run-time error, no script event.
    assert!(
        !sim.state.events[events..]
            .iter()
            .any(|e| matches!(e, SimEvent::Script(_)))
    );
    // Unknown name: nothing.
    act_done(&mut sim, 194, &["gibtsnicht"]);
    assert!(sim.state.destroy_queue.is_empty());
    // The name itself stays (points to dead IDs).
    assert!(sim.state.names.get("trupp").is_some());
}

// QUIRK E-196: kept
#[test]
fn give_197_ignores_owner_colours_with_slot_index_and_changes_at_tick_end() {
    let rules = rules();
    let mut state = state(&rules);
    // The color of slot 2 is not 2: the original colors immediately with the slot index.
    state.players[2].color = 5;
    let mut sim = Sim::new(&mut state, &rules);
    let guid = ck::SABRE;
    for number in [35, 197] {
        let of0 = ck::spawn(&mut sim, guid, 0, (3, 3));
        let of1 = ck::spawn(&mut sim, guid, 1, (2, 2));
        let own = ck::spawn(&mut sim, guid, 2, (4, 4));
        let outside = ck::spawn(&mut sim, guid, 0, (10, 10));
        // Value slots {guid, area, slot}; reference player -1: objects of all owners.
        act_done(&mut sim, number, &["1107", "area drei", "2"]);
        for id in [of0, of1, own] {
            assert_eq!(
                sim.object(id).unwrap().color,
                2,
                "immediately the slot index"
            );
        }
        assert_eq!(sim.object(outside).unwrap().color, 0);
        // Change of owner not until the tick end; the same owner makes no entry.
        assert_eq!(of_slot(&sim, 2).len(), 1);
        assert_eq!(sim.state.owner_changes.len(), 2);
        assert!(
            sim.state
                .owner_changes
                .iter()
                .all(|(id, s)| *s == 2 && *id != own)
        );
        flush(&mut sim);
        let moved = of_slot(&sim, 2);
        assert_eq!(moved.len(), 3);
        for id in moved {
            let c = sim.object(id).unwrap().color;
            // Whoever switched got the player color; the old owner stayed wrongly colored (E-196).
            assert_eq!(c, if id == own { 2 } else { 5 }, "action {number}");
        }
        // Cleaning up.
        for id in of_slot(&sim, 2).into_iter().chain(of_slot(&sim, 0)) {
            crate::world::queue_destroy(&mut sim, id);
        }
        crate::world::process_destroy_queue(&mut sim);
    }
}

// QUIRK E-157: kept
#[test]
fn give_197_uses_raw_guid_filter_for_unknown_ids_and_all_owners() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    ck::spawn(&mut sim, ck::PIKEMAN, 0, (3, 4));
    // Unknown filter ID 999: the filter "GUID = raw value" matches nothing (no unit of this type).
    act_done(&mut sim, 197, &["999", "area drei", "1"]);
    assert!(sim.state.owner_changes.is_empty());
    assert_eq!(sim.object(a).unwrap().color, 0);
    // Slot 8 (neutral) is a valid target.
    act_done(&mut sim, 197, &["1107", "area drei", "8"]);
    flush(&mut sim);
    assert_eq!(of_slot(&sim, 8).len(), 1);
    // Unknown area: nothing.
    act_done(&mut sim, 197, &["1107", "area nirgends", "1"]);
    assert!(sim.state.owner_changes.is_empty());
}

/// Rider `name` and its horse `name + "pferd"` on `slot`, near the center of "drei".
fn rider_with_horse(sim: &mut Sim, slot: i32, name: &str) -> (ObjectId, ObjectId) {
    let rider = place_named(sim, slot, RIDER as i32, "drei", name, 0).expect("rider");
    let horse = names::first_id(&sim.state.names, &format!("{name}pferd")).expect("horse");
    // The test type has no riding module: set the link (`+0x2CC` on both sides) directly.
    crate::module::riding::testkit::link(sim, rider, horse);
    (rider, horse)
}

// QUIRK E-197: deviation
#[test]
fn give_202_renames_to_last_transferred_and_keeps_name_for_slot_8() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let units: Vec<_> = (0..3)
        .map(|i| ck::spawn(&mut sim, ck::SABRE, 0, (3 + i, 3)))
        .collect();
    // A stale ID in the middle is skipped.
    let stale = ck::spawn(&mut sim, ck::SABRE, 0, (8, 8));
    crate::world::queue_destroy(&mut sim, stale);
    crate::world::process_destroy_queue(&mut sim);
    let mut with_stale = units.clone();
    with_stale.insert(1, stale);
    name_ids(&mut sim, "trupp", &with_stale);

    // Immediately (no waiting for the tick end), the name points to the last transferred unit at the end.
    act_done(&mut sim, 202, &["Trupp", "1"]);
    assert!(sim.state.owner_changes.is_empty());
    assert!(units.iter().all(|id| sim.object(*id).is_none()));
    let moved = of_slot(&sim, 1);
    assert_eq!(moved.len(), 3);
    assert_eq!(
        sim.state.names.get("trupp"),
        Some(moved[2].as_i32().to_string().as_str())
    );

    // Slot 8: the name is set to the new ID anyway (`ChangeObjectOwner` itself would remove it).
    let solo = ck::spawn(&mut sim, ck::SABRE, 0, (6, 6));
    name_ids(&mut sim, "solo", &[solo]);
    act_done(&mut sim, 202, &["solo", "8"]);
    let neutral = names::first_id(&sim.state.names, "solo").expect("name stays");
    assert_eq!(neutral.slot(), 8);
    assert!(sim.object(neutral).is_some());
    // Unknown name: nothing.
    act_done(&mut sim, 202, &["gibtsnicht", "1"]);
}

// QUIRK E-197: deviation
#[test]
fn give_202_same_owner_keeps_rider_link() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (rider, horse) = rider_with_horse(&mut sim, 0, "r");
    let before = (
        sim.object(rider).unwrap().target,
        sim.object(horse).unwrap().target,
    );
    assert_eq!(before.0, horse, "rider knows its horse");
    // Deviation from the original: same owner, no name change, the back link stays.
    act_done(&mut sim, 202, &["r", "0"]);
    assert_eq!(names::first_id(&sim.state.names, "r"), Some(rider));
    assert_eq!(
        (
            sim.object(rider).unwrap().target,
            sim.object(horse).unwrap().target
        ),
        before
    );
}

// QUIRK E-197: deviation
#[test]
fn give_202_moves_carried_horse_without_name() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (rider, horse) = rider_with_horse(&mut sim, 0, "r");
    let horse_target = sim.object(horse).unwrap().target;
    act_done(&mut sim, 202, &["r", "1"]);
    let new_rider = names::first_id(&sim.state.names, "r").expect("rider");
    assert_eq!(new_rider.slot(), 1);
    assert!(sim.object(rider).is_none() && sim.object(horse).is_none());
    let new_horse = sim.object(new_rider).unwrap().target;
    assert_eq!(new_horse.slot(), 1, "the linked horse switched too");
    assert_eq!(sim.object(new_horse).unwrap().guid, ck::PIKEMAN);
    // The back link points to the new rider ID, as far as it pointed to the rider before.
    if horse_target == rider {
        assert_eq!(sim.object(new_horse).unwrap().target, new_rider);
    }
    assert_eq!(of_slot(&sim, 1).len(), 2);
    assert!(of_slot(&sim, 0).is_empty());
}

// QUIRK E-157: kept
#[test]
fn give_341_whole_slot_including_dying_never_to_8() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    let dying = ck::spawn(&mut sim, ck::PIKEMAN, 0, (50, 50));
    ck::spawn(&mut sim, ck::SABRE, 2, (4, 4));
    sim.object_mut(dying).unwrap().status |= crate::object::status::DIED;
    // Invalid slots and q == p: nothing.
    for (p, q) in [
        ("0", "8"),
        ("8", "1"),
        ("-1", "1"),
        ("0", "9"),
        ("0", "-1"),
        ("0", "0"),
    ] {
        act_done(&mut sim, 341, &[p, q]);
        assert!(sim.state.owner_changes.is_empty(), "{p} to {q}");
    }
    act_done(&mut sim, 341, &["0", "1"]);
    // All entries of the slot table, also the dying one; only queued.
    assert_eq!(sim.state.owner_changes.len(), 2);
    assert_eq!(of_slot(&sim, 1).len(), 0);
    flush(&mut sim);
    assert_eq!(of_slot(&sim, 1).len(), 2);
    assert!(of_slot(&sim, 0).is_empty());
    assert_eq!(of_slot(&sim, 2).len(), 1);
    assert!(sim.object(a).is_none());
    // No immediate color as with 35/197: the player color comes from the change of owner (a carcass
    // keeps the color of the old owner, C-142).
    for id in of_slot(&sim, 1) {
        let o = sim.object(id).unwrap();
        let owner = if o.status & crate::object::status::DIED != 0 {
            0
        } else {
            1
        };
        assert_eq!(o.color, sim.state.players[owner].color);
    }
}

/// (Maximum, current value, fraction) of the attribute `attribute`.
fn gauge(sim: &Sim, id: ObjectId, attribute: u32) -> (i32, i32, i32) {
    let o = sim.object(id).unwrap();
    let p = sim.type_upgrade_percent(o.owner(), o.guid, attribute);
    (
        o.attributes.max_value(attribute, p),
        o.attributes.current_value(attribute, p),
        o.attributes.raw_fraction(attribute),
    )
}

// QUIRK E-158, E-194: kept
#[test]
fn hp_actions_hit_dying_objects() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    name_ids(&mut sim, "u", &[unit]);
    // Dying: the original takes GetObjectById(id, 0), so these too.
    sim.object_mut(unit).unwrap().status |= crate::object::status::DIED;
    let (max, _, _) = gauge(&sim, unit, 14);
    assert!(max >= 10, "test type with hit points: {max}");

    // 201 sets the current value (fraction = min(value, max) * 100,000 / max).
    act_done(&mut sim, 201, &["u", "3"]);
    assert_eq!(gauge(&sim, unit, 14).1, 3);
    act_done(&mut sim, 201, &["u", "-5"]);
    assert_eq!(gauge(&sim, unit, 14).2, 0);
    act_done(&mut sim, 201, &["u", "999999"]);
    assert_eq!(gauge(&sim, unit, 14).2, 100_000);

    // 353 adds value / 100 hit points via the fraction (E-194), not value percent.
    act_done(&mut sim, 201, &["u", "0"]);
    act_done(&mut sim, 353, &["u", &(5 * 100).to_string()]);
    assert_eq!(gauge(&sim, unit, 14).2, 5 * 100_000 / max);
    act_done(&mut sim, 353, &["u", "99999999"]);
    assert_eq!(gauge(&sim, unit, 14).2, 100_000, "capped at the top");
    // Open at the bottom: a negative value can push the fraction below 0.
    act_done(&mut sim, 201, &["u", "0"]);
    act_done(&mut sim, 353, &["u", "-100"]);
    assert!(gauge(&sim, unit, 14).2 < 0);

    // 354 fills, 215 fills the special energy (attribute 15).
    act_done(&mut sim, 354, &["u"]);
    assert_eq!(gauge(&sim, unit, 14).2, 100_000);
    sim.object_mut(unit)
        .unwrap()
        .attributes
        .set_current_fraction(15, 0);
    act_done(&mut sim, 215, &["u"]);
    assert_eq!(gauge(&sim, unit, 15).2, 100_000);

    // 192: absolute modifier with key 0, a second call replaces the first; the fraction stays, so
    // the current hit points scale along.
    act_done(&mut sim, 201, &["u", &(max / 2).to_string()]);
    let fraction = gauge(&sim, unit, 14).2;
    act_done(&mut sim, 192, &["u", "40"]);
    let after_first = gauge(&sim, unit, 14);
    act_done(&mut sim, 192, &["u", "20"]);
    let after_second = gauge(&sim, unit, 14);
    assert_eq!(after_first.0 - after_second.0, 20, "key 0 replaces");
    assert_eq!(after_first.2, fraction);
    assert_eq!(after_second.2, fraction);

    // Stale IDs and unknown names: nothing, no error.
    let gone = ck::spawn(&mut sim, ck::SABRE, 0, (8, 8));
    name_ids(&mut sim, "weg", &[gone]);
    crate::world::queue_destroy(&mut sim, gone);
    crate::world::process_destroy_queue(&mut sim);
    let before = crate::save::checksum(sim.state);
    for number in [192, 201, 353] {
        act_done(&mut sim, number, &["weg", "7"]);
        act_done(&mut sim, number, &["gibtsnicht", "7"]);
    }
    for number in [354, 215] {
        act_done(&mut sim, number, &["weg"]);
        act_done(&mut sim, number, &["gibtsnicht"]);
    }
    assert_eq!(crate::save::checksum(sim.state), before);
}

// QUIRK E-158: kept
#[test]
fn attribute_385_only_if_present_and_384_xp_via_rank() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    name_ids(&mut sim, "u", &[unit]);

    // 385: absolute modifier with key 0 and refill, only if the attribute is present.
    assert!(!sim.object(unit).unwrap().attributes.is_present(27));
    let before = crate::save::checksum(sim.state);
    act_done(&mut sim, 385, &["27", "u", "50"]);
    act_done(&mut sim, 384, &["27", "u", "50"]);
    assert_eq!(
        crate::save::checksum(sim.state),
        before,
        "not present: nothing"
    );
    {
        let o = sim.object_mut(unit).unwrap();
        o.attributes.set_modifier(19, 10, false, 5);
        o.attributes.set_current_fraction(19, 0);
    }
    // The base value of the test type sits in a modifier with key 0; 385 replaces it.
    act_done(&mut sim, 385, &["19", "u", "40"]);
    let (max, _, fraction) = gauge(&sim, unit, 19);
    assert_eq!(max, 10 + 40, "key 0 next to key 5");
    assert_eq!(fraction, 100_000, "refilled");
    act_done(&mut sim, 385, &["19", "u", "30"]);
    assert_eq!(gauge(&sim, unit, 19).0, 10 + 30, "key 0 replaces");

    // 384, ordinary attribute: current value (capped at the maximum).
    act_done(&mut sim, 384, &["19", "u", "7"]);
    // (Attribute 19 does not carry a current value, but the fraction is set.)
    assert_eq!(gauge(&sim, unit, 19).2, 7 * 100_000 / 40);

    // 384, attribute 30 (experience): the value runs via the rank ladder. Rank 0 has maximum 100.
    let (xp_max, xp_now, _) = gauge(&sim, unit, 30);
    assert_eq!(
        (xp_max, xp_now, sim.object(unit).unwrap().rank),
        (100, 0, 0)
    );
    act_done(&mut sim, 384, &["30", "u", "40"]);
    assert_eq!(gauge(&sim, unit, 30).1, 40);
    assert_eq!(sim.object(unit).unwrap().rank, 0);
    // Another 150: 60 fill rank 0, the remaining 90 stand in rank 1 (maximum 200).
    act_done(&mut sim, 384, &["30", "u", "150"]);
    assert_eq!(sim.object(unit).unwrap().rank, 1);
    assert_eq!(gauge(&sim, unit, 30).0, 200);
    assert_eq!(gauge(&sim, unit, 30).1, 90);
}

// QUIRK E-158: kept
#[test]
fn attribute_385_refreshes_module_parameters() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    name_ids(&mut sim, "u", &[unit]);
    sim.object_mut(unit)
        .unwrap()
        .attributes
        .set_modifier(19, 10, false, 5);
    let index = crate::module::attack::INDEX;
    let flags = |sim: &Sim| sim.module(unit, index).unwrap().base.flags;
    let original = flags(&sim);
    assert_ne!(original, 0);
    // RefreshModuleParams rereads the parameters of the enabled modules from the type description.
    sim.module_mut(unit, index).unwrap().base.flags = 0;
    act_done(&mut sim, 385, &["19", "u", "1"]);
    assert_eq!(flags(&sim), original);
    sim.module_mut(unit, index).unwrap().base.flags = 0;
    act_done(&mut sim, 384, &["19", "u", "1"]);
    assert_eq!(flags(&sim), original);
    // 384 with attribute 30 does not call it.
    sim.module_mut(unit, index).unwrap().base.flags = 0;
    act_done(&mut sim, 384, &["30", "u", "1"]);
    assert_eq!(flags(&sim), 0);
}

#[test]
fn clear_event_bit_231_for_all_ids() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    let b = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let stale = ck::spawn(&mut sim, ck::SABRE, 0, (8, 8));
    crate::world::queue_destroy(&mut sim, stale);
    crate::world::process_destroy_queue(&mut sim);
    for id in [a, b, stale] {
        crate::world::set_object_event_bit(&mut sim, id, 2, true);
        crate::world::set_object_event_bit(&mut sim, id, 5, true);
    }
    name_ids(&mut sim, "g", &[a, b, stale]);
    // {bit, name}: the bit is cleared for each ID (without checking whether the object lives), others stay.
    act_done(&mut sim, 231, &["g", "2"]);
    for id in [a, b, stale] {
        assert_eq!(crate::world::object_event_bits(&sim, id), 1 << 5);
    }
    act_done(&mut sim, 231, &["gibtsnicht", "5"]);
    assert_eq!(crate::world::object_event_bits(&sim, a), 1 << 5);
}

#[test]
fn all_remove_give_hp_actions_are_done_in_a_running_script() {
    let level = script(&[(
        &[],
        &[
            "n194;gibtsnicht",
            "n6;0;1100;area feld",
            "n35;1100;area feld;1",
            "n197;1100;area feld;1",
            "n202;gibtsnicht;1",
            "n341;0;1",
            "n192;gibtsnicht;5",
            "n201;gibtsnicht;5",
            "n353;gibtsnicht;5",
            "n354;gibtsnicht",
            "n215;gibtsnicht",
            "n384;19;gibtsnicht;5",
            "n385;19;gibtsnicht;5",
            "n231;gibtsnicht;2",
        ],
    )]);
    let rules = rules_with_areas(&level, &[("feld", [2, 2, 5, 5])]);
    let mut state = new_match(&rules, &setup(true)).expect("match with script");
    let traces = traces(step(&mut state, &rules, &[]));
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
        "{traces:?}"
    );
}

// QUIRK E-208: deviation
#[test]
fn attribute_384_xp_continues_when_the_rounded_rest_is_zero() {
    // Fraction 99,500 at maximum 100: the rounded current value is already 100, so the remainder is 0. The
    // original then calls `AddExperience(0)` and continues (`FUN_00479380`); the experience must not get lost.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    name_ids(&mut sim, "u", &[unit]);
    let set_fraction = |sim: &mut Sim| {
        sim.object_mut(unit)
            .unwrap()
            .attributes
            .set_current_fraction(30, 99_500);
    };
    set_fraction(&mut sim);
    assert_eq!(
        (gauge(&sim, unit, 30).0, gauge(&sim, unit, 30).1),
        (100, 100)
    );
    // Value 30 (< maximum): the last credit `30 * 10` fills the rank -> rank 1, the new rank starts at 0.
    act_done(&mut sim, 384, &["30", "u", "30"]);
    assert_eq!(sim.object(unit).unwrap().rank, 1);
    assert_eq!(gauge(&sim, unit, 30).1, 0);

    // Value 150 (> maximum 100): remainder 0 -> `AddExperience(0)`, value stays 150, remainder = max = 100 < 150
    // -> `AddExperience(1100)` raises the rank, value 50, remainder = max(rank) = 200 -> finally 50 in the new rank.
    let unit = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    name_ids(&mut sim, "v", &[unit]);
    sim.object_mut(unit)
        .unwrap()
        .attributes
        .set_current_fraction(30, 99_500);
    act_done(&mut sim, 384, &["30", "v", "150"]);
    assert_eq!(sim.object(unit).unwrap().rank, 1);
    assert_eq!(gauge(&sim, unit, 30).0, 200);
    assert_eq!(gauge(&sim, unit, 30).1, 50);
}

// ---------------------------------------------------------------------------------------------
// Player actions, groups and collision

/// Resource stock `kind` of the player `slot` in whole units (`raw >> 12`, arithmetic).
fn stock(sim: &Sim, slot: usize, kind: usize) -> i32 {
    sim.state.players[slot].resources_fx[kind] >> 12
}

/// The action is rejected as a run-time error (A7): layer D reports `Guarded`.
fn act_guarded(sim: &mut Sim, number: i32, params: &[&str]) {
    assert!(
        matches!(act(sim, number, params), Dispatched::Guarded(_)),
        "action {number}"
    );
}

// QUIRK E-159: deviation
#[test]
fn resources_21_22_allow_negative_stock_and_kinds_3_4() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // {slot, amount, kind}: 21 gives, 22 takes - without a lower bound (E-159).
    assert_eq!(stock(&sim, 0, 1), 30_000);
    act_done(&mut sim, 21, &["0", "500", "1"]);
    assert_eq!(stock(&sim, 0, 1), 30_500);
    act_done(&mut sim, 22, &["0", "40000", "0"]);
    assert_eq!(stock(&sim, 0, 0), -10_000, "22 can make the stock negative");
    act_done(&mut sim, 22, &["0", "-7", "2"]);
    assert_eq!(stock(&sim, 0, 2), 30_007, "a negative amount reverses 22");
    // Kinds 3 (population used) and 4 (living space) are resources like the others.
    act_done(&mut sim, 21, &["2", "5", "3"]);
    act_done(&mut sim, 22, &["2", "2", "4"]);
    assert_eq!((stock(&sim, 2, 3), stock(&sim, 2, 4)), (5, -2));
    // Empty slots get the resources as well (the player object exists for all eight).
    act_done(&mut sim, 21, &["6", "1", "0"]);
    assert_eq!(stock(&sim, 6, 0), 30_001);
    // Kind >= 5 or < 0: no resource, no effect; a slot outside 0...7 (the load check does not allow it)
    // changes nothing and does not crash.
    let before = sim.state.players.clone();
    act_done(&mut sim, 21, &["0", "9", "5"]);
    act_done(&mut sim, 21, &["0", "9", "-1"]);
    act_done(&mut sim, 22, &["8", "9", "0"]);
    act_done(&mut sim, 22, &["-1", "9", "0"]);
    assert_eq!(sim.state.players, before);
}

// QUIRK O-170: kept
#[test]
fn resources_21_22_book_the_statistic_by_kind() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.accounting.init_record(0, "Player", 0, 0, 0);
    // {slot, amount, kind}: statistic 10 for gold (0), 8 for food (2), 9 for every other kind - also a
    // kind without a resource (O-170); 22 books the negated amount.
    act_done(&mut sim, 21, &["0", "500", "0"]);
    act_done(&mut sim, 22, &["0", "40", "2"]);
    act_done(&mut sim, 21, &["0", "7", "1"]);
    act_done(&mut sim, 21, &["0", "3", "5"]);
    let booked: Vec<(i32, i32)> = sim.state.accounting.records[0]
        .entries
        .iter()
        .map(|e| (e.kind, e.parm))
        .collect();
    assert_eq!(booked, [(10, 500), (8, -40), (9, 7), (9, 3)]);
}

// QUIRK E-160: kept
#[test]
fn diplomacy_is_one_sided_and_silent() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.events.clear();
    let start = sim.state.diplomacy.clone();
    // 28/29: p sees what q sees - only the row of p.
    act_done(&mut sim, 28, &["1", "2"]);
    assert!(sim.state.diplomacy.shares_vision(1, 2));
    assert!(!sim.state.diplomacy.shares_vision(2, 1), "one-sided");
    act_done(&mut sim, 29, &["1", "2"]);
    assert_eq!(sim.state.diplomacy, start);
    // 30 allies: friend on, enemy off; 31 makes enemies: friend off, enemy on; 32 neutral: both off.
    act_done(&mut sim, 30, &["1", "2"]);
    assert!(sim.state.diplomacy.is_friend(1, 2) && !sim.state.diplomacy.is_enemy(1, 2));
    assert!(!sim.state.diplomacy.is_friend(2, 1), "one-sided");
    assert!(sim.state.diplomacy.is_enemy(2, 1));
    act_done(&mut sim, 31, &["1", "2"]);
    assert!(!sim.state.diplomacy.is_friend(1, 2) && sim.state.diplomacy.is_enemy(1, 2));
    act_done(&mut sim, 32, &["1", "2"]);
    assert!(!sim.state.diplomacy.is_friend(1, 2) && !sim.state.diplomacy.is_enemy(1, 2));
    // q == p is allowed and sets its own bit (E-160).
    act_done(&mut sim, 31, &["3", "3"]);
    assert!(sim.state.diplomacy.is_enemy(3, 3) && !sim.state.diplomacy.is_friend(3, 3));
    act_done(&mut sim, 30, &["3", "3"]);
    assert!(sim.state.diplomacy.is_friend(3, 3) && !sim.state.diplomacy.is_enemy(3, 3));
    // Slots outside 0...7 (also 8): silently without effect.
    let before = sim.state.diplomacy.clone();
    for (p, q) in [("8", "1"), ("1", "8"), ("-1", "2"), ("2", "9")] {
        for n in 28..=32 {
            act_done(&mut sim, n, &[p, q]);
        }
    }
    assert_eq!(sim.state.diplomacy, before);
    // No message `0xE7...0xE9` (the game command `0x64` reports, the script action does not), no event.
    assert!(sim.state.events.is_empty(), "{:?}", sim.state.events);
}

// QUIRK E-155: kept
#[test]
fn behavior_203_includes_humans_and_slot_8_sets_only_default() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let guid = ck::SABRE;
    let human = ck::spawn(&mut sim, guid, 0, (3, 3));
    let ai = ck::spawn(&mut sim, guid, 1, (4, 4));
    let neutral = ck::spawn(&mut sim, guid, 8, (5, 5));
    // Stance 3: the human only gets the attack mode, the AI additionally `{4, 3, 2, 0}`.
    act_done(&mut sim, 203, &["0", "3"]);
    act_done(&mut sim, 203, &["1", "3"]);
    assert_eq!(sim.state.combat.behavior_mode[..2], [3, 3]);
    assert_eq!(posture(&sim, human), (4, 3, 1), "humans too (E-155)");
    assert_eq!(posture(&sim, ai), (4, 3, 2));
    // Stance 2: AI `{4, 3, 1, 0}`, human `{2, 2, 1, 0}`.
    act_done(&mut sim, 203, &["0", "2"]);
    act_done(&mut sim, 203, &["1", "2"]);
    assert_eq!(posture(&sim, human), (2, 2, 1));
    assert_eq!(posture(&sim, ai), (4, 3, 1));
    // Slot 8 only sets the default of the neutrals; existing objects stay.
    let before = posture(&sim, neutral);
    act_done(&mut sim, 203, &["8", "0"]);
    assert_eq!(sim.state.combat.behavior_mode[8], 0);
    assert_eq!(posture(&sim, neutral), before);
    // Stance >= 5 or < 0: the original leaves words of the messages `0x611`/`0x2100` unset (heap remainders,
    // not deterministic, C-341): no effect and `Guarded`.
    let mode = sim.state.combat.behavior_mode;
    act_guarded(&mut sim, 203, &["0", "7"]);
    act_guarded(&mut sim, 203, &["0", "-1"]);
    assert_eq!(sim.state.combat.behavior_mode, mode);
    assert_eq!(posture(&sim, human), (2, 2, 1));
    // Slot outside 0...8: nothing.
    act_done(&mut sim, 203, &["9", "1"]);
    act_done(&mut sim, 203, &["-1", "1"]);
    assert_eq!(sim.state.combat.behavior_mode, mode);
}

thread_local! {
    /// `(object, code)` of the messages `0x2100`, `0x611` and `0x914` during [`message_log`].
    static MESSAGES: std::cell::RefCell<Vec<(ObjectId, u32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn record_message(_: &mut Sim, id: ObjectId, _: usize, m: &crate::message::Message) {
    if matches!(m.code, 0x2100 | 0x611 | 0x914) {
        MESSAGES.with(|b| b.borrow_mut().push((id, m.code)));
    }
}

fn message_log(f: impl FnOnce()) -> Vec<(ObjectId, u32)> {
    MESSAGES.with(|b| b.borrow_mut().clear());
    crate::module::test_hook::observe(record_message, f);
    MESSAGES.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

// QUIRK E-155: kept
#[test]
fn behavior_204_moves_animal_home_and_sets_ai_attack_for_humans() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let goat = ck::spawn(&mut sim, ck::GOAT, 8, (3, 3));
    let human = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let ai = ck::spawn(&mut sim, ck::SABRE, 1, (2, 4));
    // {player, guid, area, mode}: the animal home (x, z, height) moves to its own position. The filter of
    // a type GUID only takes the list of the reference player: the animal belongs to slot 8.
    crate::messaging::send(
        &mut sim,
        goat,
        &crate::message::Message::new(0x914, &[1, 2, 3]),
    );
    let grid = sim.object(goat).unwrap().grid;
    let home = |sim: &Sim| crate::module::animal::animal(sim, goat).unwrap().home;
    assert_ne!(home(&sim), (grid.x.0, grid.z.0, grid.y.0));
    let goat_guid = ck::GOAT.to_string();
    let log = message_log(|| act_done(&mut sim, 204, &["8", &goat_guid, "area drei", "2"]));
    assert_eq!(home(&sim), (grid.x.0, grid.z.0, grid.y.0));
    // The animal has no attack module, `0x611` stays unobserved; `0x914` follows `0x2100`.
    assert_eq!(log, [(goat, 0x2100), (goat, 0x914)]);

    // The addition applies regardless of the control kind: humans (0) like AI (1), mode 2 -> `{4, 3, 1, 0}`,
    // mode 3 -> `{4, 3, 2, 0}`; modes 0 and 1 get nothing added. Order per hit: `0x2100`, `0x611`,
    // (`0x914` - only the animal module sees it), the addition for mode 2 follows after.
    let guid = ck::SABRE.to_string();
    let log = message_log(|| act_done(&mut sim, 204, &["0", &guid, "area drei", "2"]));
    assert_eq!(log, [(human, 0x2100), (human, 0x611), (human, 0x611)]);
    assert_eq!(posture(&sim, human), (4, 3, 1));
    act_done(&mut sim, 204, &["1", &guid, "area drei", "3"]);
    assert_eq!(posture(&sim, ai), (4, 3, 2));
    act_done(&mut sim, 204, &["0", &guid, "area drei", "3"]);
    assert_eq!(posture(&sim, human), (4, 3, 2));
    let log = message_log(|| act_done(&mut sim, 204, &["0", &guid, "area drei", "0"]));
    assert_eq!(log, [(human, 0x2100), (human, 0x611)]);
    assert_eq!(posture(&sim, human), (1, 0, 4));
    // The stance of the slot stays untouched (204 sets no default).
    assert_eq!(sim.state.combat.behavior_mode[0], 2);
    // Unknown area and unknown filter ID (raw-value GUID without a hit): nothing.
    assert!(
        message_log(|| act_done(&mut sim, 204, &["0", &guid, "area nirgends", "2"])).is_empty()
    );
    assert!(message_log(|| act_done(&mut sim, 204, &["0", "999", "area drei", "2"])).is_empty());
    // Stance >= 5: the original leaves words of the messages `0x611`/`0x2100` unset (heap remainders, not
    // deterministic, C-341): no effect and `Guarded`.
    assert!(message_log(|| act_guarded(&mut sim, 204, &["0", &guid, "area drei", "9"])).is_empty());
    // Without a hit (known area, type without objects there) nothing happens - not even a message.
    let none = ck::PIKEMAN.to_string();
    act_done(&mut sim, 204, &["0", &none, "area drei", "9"]);
    act_done(&mut sim, 204, &["0", &guid, "area fern", "9"]);
}

// QUIRK E-165: kept
#[test]
fn hit_factor_343_sets_100_not_handicap() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Handicap factor of slot 0 (E-165): 343 does not restore it but sets 100.
    sim.state.combat.hit_factor[0] = 60;
    sim.state.combat.hit_factor[8] = 40;
    act_done(&mut sim, 342, &["0"]);
    assert_eq!(sim.state.combat.hit_factor[0], 0, "invulnerable");
    act_done(&mut sim, 343, &["0"]);
    assert_eq!(sim.state.combat.hit_factor[0], 100);
    act_done(&mut sim, 342, &["8"]);
    assert_eq!(sim.state.combat.hit_factor[8], 0, "slot 8 counts too");
    act_done(&mut sim, 343, &["8"]);
    assert_eq!(sim.state.combat.hit_factor[8], 100);
    // Outside 0...8 nothing.
    let before = sim.state.combat.hit_factor;
    for p in ["9", "-1", "10"] {
        act_done(&mut sim, 342, &[p]);
        act_done(&mut sim, 343, &[p]);
    }
    assert_eq!(sim.state.combat.hit_factor, before);
}

// QUIRK E-199: kept
#[test]
fn daytime_173_applies_upgrades() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let start = crate::save::checksum(sim.state);
    // 173 and 205 call `PlayerSetup_ApplyDaytimeUpgrades(t)` with the time of day `t = Z0`.
    act_done(&mut sim, 173, &["2"]);
    assert_eq!(sim.state.daytime, 2);
    let after = crate::save::checksum(sim.state);
    assert_ne!(after, start);
    // Counter-check: the same world, the time of day applied by hand.
    let mut twin_state = self::state(&rules);
    crate::upgrades::apply_daytime_upgrades(&mut Sim::new(&mut twin_state, &rules), 2);
    assert_eq!(after, crate::save::checksum(&twin_state));
    // The same time of day: nothing. 205 switches; -1 is allowed.
    act_done(&mut sim, 205, &["2"]);
    assert_eq!(crate::save::checksum(sim.state), after);
    act_done(&mut sim, 205, &["3"]);
    assert_eq!(sim.state.daytime, 3);
    act_done(&mut sim, 205, &["-1"]);
    assert_eq!(sim.state.daytime, -1);
    // Times of day outside -1...3 are passed on unchecked by the original (E-199): applied as by hand,
    // checksum after every step equal to that of a twin.
    crate::upgrades::apply_daytime_upgrades(&mut Sim::new(&mut twin_state, &rules), 2);
    crate::upgrades::apply_daytime_upgrades(&mut Sim::new(&mut twin_state, &rules), 3);
    crate::upgrades::apply_daytime_upgrades(&mut Sim::new(&mut twin_state, &rules), -1);
    assert_eq!(
        crate::save::checksum(sim.state),
        crate::save::checksum(&twin_state)
    );
    for (number, text, t) in [(173, "4", 4), (205, "10", 10), (173, "0", 0)] {
        act_done(&mut sim, number, &[text]);
        assert_eq!(sim.state.daytime, t);
        crate::upgrades::apply_daytime_upgrades(&mut Sim::new(&mut twin_state, &rules), t);
        assert_eq!(
            crate::save::checksum(sim.state),
            crate::save::checksum(&twin_state),
            "time of day {t}"
        );
    }
}

// QUIRK E-202: deviation
#[test]
fn task_status_200_uses_second_value() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // `{Z0, Z1}`: the key is the second value (deviation from the docs, re/script-actions C.15), the value always 1.
    act_done(&mut sim, 200, &["3", "9"]);
    assert_eq!(
        sim.state.script.mission.task_status,
        [(9, 1)].into_iter().collect()
    );
    act_done(&mut sim, 200, &["0", "-4"]);
    act_done(&mut sim, 200, &["0", "9"]);
    assert_eq!(
        sim.state.script.mission.task_status,
        [(-4, 1), (9, 1)].into_iter().collect()
    );
}

#[test]
fn collision_391_exempts_392_clears_only_the_bit() {
    use crate::object::status::{DIED, EXEMPT_ZONE};
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    let b = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let dying = ck::spawn(&mut sim, ck::SABRE, 0, (5, 5));
    let stale = ck::spawn(&mut sim, ck::SABRE, 0, (8, 8));
    crate::world::queue_destroy(&mut sim, stale);
    crate::world::process_destroy_queue(&mut sim);
    sim.object_mut(dying).unwrap().status |= DIED;
    sim.object_mut(dying)
        .unwrap()
        .set_collision_exempt_circle(1, 2, 3);
    // An existing circle (mode 1) is replaced by "exemption object -1" (mode 0).
    sim.object_mut(a)
        .unwrap()
        .set_collision_exempt_circle(10, 20, 30);
    name_ids(&mut sim, "g", &[a, b, dying, stale]);
    act_done(&mut sim, 391, &["g"]);
    for id in [a, b] {
        let o = sim.object(id).unwrap();
        assert_ne!(o.status & EXEMPT_ZONE, 0, "{id:?}");
        assert_eq!((o.exempt.object, o.exempt.mode), (ObjectId::NONE, 0));
    }
    // Dying ones (`GetObjectById(id, 1)`) and stale IDs are skipped.
    let o = sim.object(dying).unwrap();
    assert_eq!(o.status & EXEMPT_ZONE, 0);
    assert_eq!((o.exempt.mode, o.exempt.radius), (1, 3), "circle stays");
    // 392 clears only the state bit; the exemption object -1 stays (without effect without the bit).
    sim.object_mut(a).unwrap().status |= 0x4;
    act_done(&mut sim, 392, &["g"]);
    let o = sim.object(a).unwrap();
    assert_eq!(o.status & EXEMPT_ZONE, 0);
    assert_eq!(o.status & 0x4, 0x4, "other state bits stay");
    assert_eq!(o.exempt.object, ObjectId::NONE);
    // Unknown name: nothing.
    act_done(&mut sim, 391, &["gibtsnicht"]);
}

/// Value of the name as IDs.
fn group(sim: &Sim, name: &str) -> Option<Vec<ObjectId>> {
    sim.state.names.get(name).map(|v| {
        v.split(';')
            .map(|p| ObjectId::from_i32(p.parse().unwrap()))
            .collect()
    })
}

// QUIRK E-195: kept
#[test]
fn group_393_skips_duplicates_394_keeps_old_value_when_empty_397_appends_duplicates() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    let b = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let c = ck::spawn(&mut sim, ck::SABRE, 0, (20, 20));
    name_ids(&mut sim, "a", &[a]);
    name_ids(&mut sim, "ab", &[a, b]);
    name_ids(&mut sim, "cc", &[c, c]);

    // 393 `{name, group}`: new group; the same ID does not get in twice.
    act_done(&mut sim, 393, &["a", "trupp"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![a]));
    act_done(&mut sim, 393, &["a", "trupp"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![a]), "no duplicate");
    // Old IDs first, then the new ones in the order of the name; duplicates in the name drop out as well.
    act_done(&mut sim, 393, &["ab", "trupp"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![a, b]));
    act_done(&mut sim, 393, &["cc", "trupp"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![a, b, c]));
    // An unknown name appends nothing and creates no group.
    act_done(&mut sim, 393, &["gibtsnicht", "trupp"]);
    act_done(&mut sim, 393, &["gibtsnicht", "leer"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![a, b, c]));
    assert_eq!(group(&sim, "leer"), None);
    // The group name is lowercased like the name.
    act_done(&mut sim, 393, &["a", "TRUPP2"]);
    assert_eq!(group(&sim, "trupp2"), Some(vec![a]));

    // 394 removes the first occurrence of each ID of the name.
    name_ids(&mut sim, "dopp", &[a, a, b]);
    act_done(&mut sim, 394, &["a", "dopp"]);
    assert_eq!(
        group(&sim, "dopp"),
        Some(vec![a, b]),
        "only the first occurrence"
    );
    act_done(&mut sim, 394, &["gibtsnicht", "dopp"]);
    assert_eq!(group(&sim, "dopp"), Some(vec![a, b]));
    act_done(&mut sim, 394, &["ab", "trupp"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![c]));
    // If the list becomes empty, nothing is written: the old value stays (the group cannot be emptied).
    act_done(&mut sim, 394, &["cc", "trupp"]);
    assert_eq!(group(&sim, "trupp"), Some(vec![c]), "old value stays");
    act_done(&mut sim, 394, &["a", "gibtsnicht"]);
    assert_eq!(group(&sim, "gibtsnicht"), None);

    // 397 `{player, area, group}`: all objects of the player in the area, without a duplicate check.
    act_done(&mut sim, 397, &["0", "area drei", "feld"]);
    let first = group(&sim, "feld").expect("group written");
    let mut sorted = first.clone();
    sorted.sort_by_key(|id| id.0);
    let mut expected = vec![a, b];
    expected.sort_by_key(|id| id.0);
    assert_eq!(sorted, expected);
    act_done(&mut sim, 397, &["0", "area drei", "feld"]);
    assert_eq!(
        group(&sim, "feld"),
        Some([first.clone(), first].concat()),
        "duplicates stay"
    );
    // The reference player counts: for player 1 there is nothing there, and without a hit nothing is written.
    act_done(&mut sim, 397, &["1", "area drei", "feld"]);
    act_done(&mut sim, 397, &["1", "area drei", "neu"]);
    assert_eq!(group(&sim, "neu"), None);
    assert_eq!(group(&sim, "feld").unwrap().len(), 4);
    // Unknown area: nothing.
    act_done(&mut sim, 397, &["0", "area nirgends", "neu"]);
    assert_eq!(group(&sim, "neu"), None);
}

// QUIRK E-210: kept
#[test]
fn remove_name_395_deletes_any_name() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (3, 3));
    name_ids(&mut sim, "held", &[a]);
    name_ids(&mut sim, "trupp", &[a, a]);
    // Object name like group: `RemoveKey` does not know the difference.
    act_done(&mut sim, 395, &["held"]);
    assert_eq!(group(&sim, "held"), None);
    assert_eq!(group(&sim, "trupp"), Some(vec![a, a]));
    act_done(&mut sim, 395, &["trupp"]);
    assert_eq!(group(&sim, "trupp"), None);
    // Unknown key: nothing.
    act_done(&mut sim, 395, &["gibtsnicht"]);
    assert!(sim.state.names.is_empty());
    // E-210: the dispatcher lowercases the text when unpacking (`FUN_004229F0`) with the German character
    // mapping, even without the parser (direct call); `register` lowercases the keys the same way.
    name_ids(&mut sim, "mädchen", &[a]);
    assert_eq!(group(&sim, "mädchen"), Some(vec![a]));
    act_done(&mut sim, 395, &["MÄDCHEN"]);
    assert!(sim.state.names.is_empty());
}

#[test]
fn all_player_group_actions_are_done_in_a_running_script() {
    let level = script(&[(
        &[],
        &[
            "n21;0;5;0",
            "n22;0;5;1",
            "n28;0;1",
            "n29;0;1",
            "n30;0;1",
            "n31;0;1",
            "n32;0;1",
            "n203;0;2",
            "n204;0;1100;area feld;2",
            "n342;0",
            "n343;0",
            "n173;1",
            "n205;2",
            "n200;1;2",
            "n391;gibtsnicht",
            "n392;gibtsnicht",
            "n393;gibtsnicht;gruppe",
            "n394;gibtsnicht;gruppe",
            "n395;gibtsnicht",
            "n397;0;area feld;gruppe",
        ],
    )]);
    let rules = rules_with_areas(&level, &[("feld", [2, 2, 5, 5])]);
    let mut state = new_match(&rules, &setup(true)).expect("match with script");
    let traces = traces(step(&mut state, &rules, &[]));
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
        "{traces:?}"
    );
}

// QUIRK E-155: deviation
#[test]
fn guarded_world_action_reports_the_real_block_and_ends_it() {
    // Block 1 (file index 1, not 0): 203 with stance 7 is a run-time error. The message carries block 1
    // and the number, the block ends (the following action 21 does not run, it is not deleted); block 2
    // keeps running untouched. The load check rejects the fixed stance 7; the test therefore sets it
    // into the loaded file after loading, to check the unreachable run-time guard.
    let level = script(&[
        (&[], &["n343;8"]),
        (&[], &["n203;0;4", "n21;0;100;0"]),
        (&[], &["n203;0;2", "n21;0;50;0"]),
    ]);
    assert!(
        testing::rules_with(&script(&[(&[], &["n203;0;7"])]), &[], &[])
            .errors
            .iter()
            .any(|e| e.contains("action 203 with invalid stance 7")),
        "load check rejects stance 7"
    );
    let mut rules = rules_with_areas(&level, &[]);
    let file = &mut rules.scripts.level.as_mut().expect("level script").file;
    file.blocks[1].actions[0].params[1] = "7".to_string();
    let mut state = new_match(&rules, &setup(true)).expect("match with script");
    let gold = state.players[0].resources_fx[0] >> 12;
    let traces = traces(step(&mut state, &rules, &[]));
    let guarded: Vec<_> = traces
        .iter()
        .filter_map(|t| match t {
            ScriptTrace::Guarded {
                block, number, why, ..
            } => Some((*block, *number, why.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(guarded.len(), 1, "{traces:?}");
    assert_eq!((guarded[0].0, guarded[0].1), (1, 203), "{guarded:?}");
    assert!(guarded[0].2.contains('7'), "{guarded:?}");
    assert_eq!(
        state.players[0].resources_fx[0] >> 12,
        gold + 50,
        "21 from block 1 does not run, the one from block 2 does"
    );
    assert_eq!(state.combat.behavior_mode[0], 2, "block 2 sets the stance");
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Deleted { block: 1, .. })),
        "{traces:?}"
    );
}

fn in_collision(sim: &Sim, id: ObjectId) -> bool {
    sim.state.collision.all.iter().any(|l| l.contains(&id))
}

// QUIRK E-153: kept
#[test]
fn actions_159_160_161_162_365_366_hide_and_show_slots_and_objects() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
    let b = ck::spawn(&mut sim, ck::PIKEMAN, 1, (6, 6));
    let c = ck::spawn(&mut sim, ck::PIKEMAN, 2, (7, 7));
    names::register(&mut sim.state.names, "held", a);
    // Slot runs: showing has no effect.
    assert_eq!(act(&mut sim, 162, &["held"]), Dispatched::Done);
    assert!(sim.state.inactive_lists[1].is_empty());

    assert_eq!(act(&mut sim, 159, &["1"]), Dispatched::Done);
    assert!(!sim.state.slot_running[1] && sim.state.slot_running[2]);
    assert!(crate::world::is_hidden(sim.state, a) && !in_collision(&sim, a));
    // 162/161 via the name.
    assert_eq!(act(&mut sim, 162, &["held"]), Dispatched::Done);
    assert_eq!(sim.state.inactive_lists[1], [a]);
    assert!(in_collision(&sim, a));
    assert_eq!(act(&mut sim, 161, &["held"]), Dispatched::Done);
    assert!(sim.state.inactive_lists[1].is_empty());
    assert!(!in_collision(&sim, a));
    // 366/365 by type GUID: only the slot of the first value.
    assert_eq!(
        act(&mut sim, 366, &["1", &ck::PIKEMAN.to_string()]),
        Dispatched::Done
    );
    assert_eq!(sim.state.inactive_lists[1], [b]);
    assert!(sim.state.inactive_lists[2].is_empty(), "{c:?} runs");
    assert_eq!(
        act(&mut sim, 365, &["1", &ck::PIKEMAN.to_string()]),
        Dispatched::Done
    );
    assert!(sim.state.inactive_lists[1].is_empty());
    // 365/366 only for slots 0...7: otherwise silently nothing.
    let before = crate::save::checksum(sim.state);
    assert_eq!(act(&mut sim, 366, &["8", "1106"]), Dispatched::Done);
    assert_eq!(act(&mut sim, 366, &["-1", "1106"]), Dispatched::Done);
    // Unknown category (no entry in `flowcontroltragets.ini`): no hit.
    assert!(sim.rules.flowcontrol_target(5).is_none());
    assert_eq!(act(&mut sim, 366, &["1", "5"]), Dispatched::Done);
    assert_eq!(crate::save::checksum(sim.state), before);
    // 159/160 with a slot outside 0...10: run-time error without effect.
    act_guarded(&mut sim, 159, &["11"]);
    act_guarded(&mut sim, 160, &["-1"]);
    assert_eq!(crate::save::checksum(sim.state), before);
    // 160: everything is back, registered and ticked.
    assert_eq!(act(&mut sim, 160, &["1"]), Dispatched::Done);
    assert!(sim.state.slot_running[1]);
    for id in [a, b] {
        assert!(in_collision(&sim, id) && !crate::world::is_hidden(sim.state, id));
        assert!(sim.state.tick_groups.iter().any(|g| g.contains(&id)));
    }
}

// QUIRK E-190: deviation
#[test]
fn actions_211_shows_214_hides_and_area_grows() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let sabre = ck::SABRE.to_string();
    let edge = ck::spawn(&mut sim, ck::SABRE, 1, (4, 4));
    let outside = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
    let other_type = ck::spawn(&mut sim, ck::PIKEMAN, 1, (3, 3));
    act(&mut sim, 159, &["1"]);
    // "drei" is [2, 2, 4, 4]: enlarged to [2, 2, 5, 5], so `PtInRect` hits up to and including 4.
    assert_eq!(
        act(&mut sim, 211, &["1", &sabre, "area drei"]),
        Dispatched::Done
    );
    assert_eq!(
        sim.state.inactive_lists[1],
        [edge],
        "{other_type:?} has a different type"
    );
    assert!(in_collision(&sim, edge));
    assert_eq!(area(&sim, "drei"), Some([2, 2, 5, 5]));
    // The next call reads the enlarged rectangle and enlarges it further.
    act(&mut sim, 211, &["1", &sabre, "area drei"]);
    assert_eq!(sim.state.inactive_lists[1], [edge, outside]);
    assert_eq!(area(&sim, "drei"), Some([2, 2, 6, 6]));
    // 214 hides (Remove), the area grows here as well.
    assert_eq!(
        act(&mut sim, 214, &["1", &sabre, "area drei"]),
        Dispatched::Done
    );
    assert!(sim.state.inactive_lists[1].is_empty());
    assert!(!in_collision(&sim, edge) && !in_collision(&sim, outside));
    assert_eq!(area(&sim, "drei"), Some([2, 2, 7, 7]));
    // Unknown area or unknown filter ID: nothing, the area stays.
    let before = crate::save::checksum(sim.state);
    act(&mut sim, 211, &["1", &sabre, "area nirgends"]);
    assert!(sim.rules.flowcontrol_target(5).is_none());
    act(&mut sim, 211, &["1", "5", "area drei"]);
    assert_eq!(crate::save::checksum(sim.state), before);
    // Slot outside 0...10: run-time error, the area does not grow.
    act_guarded(&mut sim, 211, &["11", &sabre, "area drei"]);
    act_guarded(&mut sim, 214, &["-1", &sabre, "area drei"]);
    assert_eq!(area(&sim, "drei"), Some([2, 2, 7, 7]));
    // Swapped corners: grows, but hits nothing.
    act(&mut sim, 211, &["1", &sabre, "area verdreht"]);
    assert_eq!(area(&sim, "verdreht"), Some([5, 5, 3, 3]));
    assert!(sim.state.inactive_lists[1].is_empty());
}

/// A fog cycle up to copying the background grids (state -1).
fn fog_cycle(sim: &mut Sim) {
    loop {
        crate::fog::fog_step(sim);
        if sim.state.fog.processed == -1 {
            break;
        }
    }
}

/// Clear all fog grids and the sight sources (the test world covers the map with one): the view
/// rectangles must refill the grids in the next cycle.
fn wipe_fog(sim: &mut Sim) {
    let fog = &mut sim.state.fog;
    fog.sources.clear();
    for grid in [
        &mut fog.explored,
        &mut fog.seen,
        &mut fog.cleared,
        &mut fog.seen_back,
        &mut fog.cleared_back,
    ] {
        grid.clear();
    }
}

// QUIRK E-163: kept
#[test]
fn area_view_20_stamps_explored_seen_and_cleared_every_cycle_until_24() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    wipe_fog(&mut sim);
    assert_eq!(act(&mut sim, 20, &["1", "area drei"]), Dispatched::Done);
    // Entry: rectangle of the area as created, slot 1, always with clearing, name lowercase.
    assert_eq!(
        sim.state.fog.area_views,
        [crate::fog::AreaView {
            name: "drei".into(),
            rect: [2, 2, 4, 4],
            player: 1,
            clear: true,
        }]
    );
    for round in 0..3 {
        fog_cycle(&mut sim);
        let fog = &sim.state.fog;
        // [2, 2, 4, 4] inclusive: 3 x 3 tiles, only bit 1, in all three grids.
        for (grid, name) in [
            (&fog.explored, "explored"),
            (&fog.seen, "seen"),
            (&fog.cleared, "cleared"),
        ] {
            assert_eq!(grid.count(0b10), 9, "{name}, cycle {round}");
            // All other slots have a foreign entry in front of them and therefore stamp the tile
            // (0, 0) (re/vision 3.4, E-163): 9 tiles + (0, 0) = 10.
            assert_eq!(grid.count(0xFF), 10, "{name}, cycle {round}");
            assert_eq!(grid.get(0, 0), Some(0xFD), "{name}");
            assert_eq!(grid.get(4, 4), Some(0b10));
            assert_eq!(grid.get(5, 4), Some(0));
        }
        // The next cycles stamp again, even if everything was cleared.
        wipe_fog(&mut sim);
    }
    // 24 removes the entry (slot and name; case-insensitive): afterwards everything stays empty.
    assert_eq!(act(&mut sim, 24, &["1", "area DREI"]), Dispatched::Done);
    assert!(sim.state.fog.area_views.is_empty());
    fog_cycle(&mut sim);
    let fog = &sim.state.fog;
    assert_eq!(
        (
            fog.explored.count(0xFF),
            fog.seen.count(0xFF),
            fog.cleared.count(0xFF)
        ),
        (0, 0, 0)
    );
}

// QUIRK E-163: kept
#[test]
fn area_view_20_twice_needs_24_twice() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    wipe_fog(&mut sim);
    // No duplicate check: two equal entries.
    act(&mut sim, 20, &["1", "area drei"]);
    act(&mut sim, 20, &["1", "area drei"]);
    act(&mut sim, 20, &["2", "area drei"]);
    assert_eq!(sim.state.fog.area_views.len(), 3);
    // 24 takes the **first** entry with the same slot and name; other slots and names stay.
    act(&mut sim, 24, &["3", "area drei"]);
    act(&mut sim, 24, &["1", "area zwei"]);
    assert_eq!(sim.state.fog.area_views.len(), 3, "no hit");
    act(&mut sim, 24, &["1", "area drei"]);
    assert_eq!(
        sim.state
            .fog
            .area_views
            .iter()
            .map(|v| v.player)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    fog_cycle(&mut sim);
    assert_eq!(sim.state.fog.explored.count(0b10), 9, "slot 1 still stamps");
    act(&mut sim, 24, &["1", "area drei"]);
    wipe_fog(&mut sim);
    fog_cycle(&mut sim);
    // Slot 1 has no entry of its own any more, but stamps the tile (0, 0) because of the foreign one (slot 2):
    // quirk of the original (re/vision 3.4, E-163).
    assert_eq!(sim.state.fog.explored.count(0b10), 1);
    assert_eq!(
        sim.state.fog.explored.get(0, 0),
        Some(0xFB),
        "(0, 0): all slots except 2 (own entry)"
    );
    assert_eq!(sim.state.fog.explored.count(0b100), 9, "slot 2 stays");
}

// QUIRK E-163: kept
#[test]
fn area_view_20_needs_a_player_0_to_7_and_a_known_area_and_24_checks_neither() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let before = crate::save::checksum(sim.state);
    for p in ["8", "9", "-1", "40"] {
        assert_eq!(act(&mut sim, 20, &[p, "area drei"]), Dispatched::Done);
    }
    assert_eq!(act(&mut sim, 20, &["1", "area nirgends"]), Dispatched::Done);
    assert_eq!(crate::save::checksum(sim.state), before);
    // 24 checks neither slot nor area: no error, nothing to remove.
    assert_eq!(act(&mut sim, 24, &["8", "area nirgends"]), Dispatched::Done);
    assert_eq!(crate::save::checksum(sim.state), before);
    // The rectangle comes from the area at run time (changes in the state first) and stays
    // unnormalized, as the area has it.
    sim.state
        .script
        .area_overrides
        .insert("drei".into(), [2, 2, 6, 6]);
    act(&mut sim, 20, &["0", "area drei"]);
    act(&mut sim, 20, &["0", "area verdreht"]);
    let rects: Vec<_> = sim.state.fog.area_views.iter().map(|v| v.rect).collect();
    assert_eq!(rects, [[2, 2, 6, 6], [5, 5, 2, 2]]);
    crate::save::check_state(sim.state).expect("the entries are loadable");
}

/// Map completely explored, seen and cleared.
fn explore_everything(sim: &mut Sim) {
    sim.state.fog.explored.fill(0xFF);
    sim.state.fog.seen.fill(0xFF);
    sim.state.fog.cleared.fill(0xFF);
}

/// Tiles where the bit `bit` is missing in "explored" (row order).
fn missing(sim: &Sim, bit: u8) -> Vec<(i32, i32)> {
    let g = &sim.state.fog.explored;
    let mut v = Vec::new();
    for z in 0..g.height {
        for x in 0..g.width {
            if g.get(x, z).unwrap() & (1 << bit) == 0 {
                v.push((x, z));
            }
        }
    }
    v
}

// QUIRK E-163, E-201: kept
#[test]
fn clear_explored_372_inclusive_swapped_and_noop_for_slot_8() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    explore_everything(&mut sim);
    // "verdreht" [5, 5, 2, 2]: corners swapped, both bounds included -> x, z in 2...5 (16 tiles),
    // only bit 0 and only "explored".
    assert_eq!(
        act(&mut sim, 372, &["0", "area verdreht"]),
        Dispatched::Done
    );
    let gone = missing(&sim, 0);
    assert_eq!(gone.len(), 16);
    assert!(
        gone.iter()
            .all(|&(x, z)| (2..=5).contains(&x) && (2..=5).contains(&z))
    );
    assert_eq!(
        sim.state.fog.explored.get(3, 3),
        Some(0xFE),
        "other bits stay"
    );
    assert_eq!(sim.state.fog.seen.count(0xFF), 64 * 64);
    assert_eq!(sim.state.fog.cleared.count(0xFF), 64 * 64);
    // Slot 8...31, negative: the 8-bit mask is 0 (`SHL DL,CL`), nothing happens - no run-time error.
    let before = crate::save::checksum(sim.state);
    for p in ["8", "9", "31", "-1", "40"] {
        assert_eq!(
            act(&mut sim, 372, &[p, "area drei"]),
            Dispatched::Done,
            "{p}"
        );
    }
    assert_eq!(crate::save::checksum(sim.state), before);
    // The shift counter has 5 bits: slot 33 acts like 1.
    act(&mut sim, 372, &["33", "area drei"]);
    assert_eq!(missing(&sim, 1).len(), 9);
    assert_eq!(missing(&sim, 0).len(), 16);
    // Unknown area: nothing.
    let before = crate::save::checksum(sim.state);
    act(&mut sim, 372, &["2", "area nirgends"]);
    assert_eq!(crate::save::checksum(sim.state), before);
    // Edges: "negativ" [-3, -3, 0, 0] hits only the tile (0, 0), "rand" [62, 5, 65, 5] two tiles.
    act(&mut sim, 372, &["2", "area negativ"]);
    act(&mut sim, 372, &["3", "area rand"]);
    assert_eq!(missing(&sim, 2), [(0, 0)]);
    assert_eq!(missing(&sim, 3), [(62, 5), (63, 5)]);
}

// QUIRK E-163, E-201: kept
#[test]
fn clear_explored_347_keeps_last_row() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    explore_everything(&mut sim);
    // "drei" [2, 2, 4, 4]: x inclusive, z half-open -> x 2...4, z 2...3 (6 tiles); the row z = 4 stays.
    assert_eq!(act(&mut sim, 347, &["1", "area drei"]), Dispatched::Done);
    assert_eq!(
        missing(&sim, 1),
        [(2, 2), (3, 2), (4, 2), (2, 3), (3, 3), (4, 3)]
    );
    assert_eq!(
        sim.state.fog.explored.get(3, 4),
        Some(0xFF),
        "last row stays"
    );
    assert_eq!(sim.state.fog.seen.count(0xFF), 64 * 64, "only \"explored\"");
    assert_eq!(sim.state.fog.cleared.count(0xFF), 64 * 64);
    // No corner swap and nothing at t >= b: "verdreht", "flach" [2, 2, 6, 2], "einzeln".
    let before = crate::save::checksum(sim.state);
    for name in ["verdreht", "flach", "einzeln"] {
        assert_eq!(
            act(&mut sim, 347, &["1", &format!("area {name}")]),
            Dispatched::Done
        );
    }
    assert_eq!(crate::save::checksum(sim.state), before);
    // Only slots 0...7 (unlike 372 without a shift counter): 8, -1, 32, 33 do nothing.
    for p in ["8", "-1", "33", "32"] {
        act(&mut sim, 347, &[p, "area drei"]);
    }
    assert_eq!(crate::save::checksum(sim.state), before);
    // Edges: "rand" [62, 5, 65, 5] has t = b -> nothing; "negativ" [-3, -3, 0, 0]: z -3...-1 lie outside.
    act(&mut sim, 347, &["4", "area rand"]);
    act(&mut sim, 347, &["4", "area negativ"]);
    assert_eq!(crate::save::checksum(sim.state), before);
    act(&mut sim, 347, &["5", "area nirgends"]);
    assert_eq!(crate::save::checksum(sim.state), before);
}

// QUIRK E-164: kept
#[test]
fn button_lock_207_208_write_the_entry_always() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(act(&mut sim, 207, &["1005"]), Dispatched::Done);
    assert_eq!(sim.state.button_locks.get(&1005), Some(&1));
    assert_eq!(act(&mut sim, 208, &["1005"]), Dispatched::Done);
    assert_eq!(
        sim.state.button_locks.get(&1005),
        Some(&0),
        "the entry stays"
    );
    // 208 also creates an entry with 0 for a GUID that was never locked (`operator[]`).
    act(&mut sim, 208, &["2222"]);
    assert_eq!(sim.state.button_locks.get(&2222), Some(&0));
    act(&mut sim, 207, &["2222"]);
    act(&mut sim, 207, &["2222"]);
    assert_eq!(sim.state.button_locks.get(&2222), Some(&1));
}

// --- Mission objectives -----------------------------------------------------------------------

use super::Quest;

fn quests_untouched(sim: &Sim) -> bool {
    sim.state.script.mission.main_quests == [Quest::default(); 10]
        && sim.state.script.mission.sub_quests == [Quest::default(); 10]
}

// QUIRK E-204: kept
#[test]
fn quests_ignore_numbers_outside_1_to_10() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Script number n is 1-based; `n - 1` outside 0...9 silently does nothing (E-204), not even a `Guarded`.
    for n in ["0", "-1", "11", "100"] {
        act_done(&mut sim, 27, &[n, "5"]);
        act_done(&mut sim, 182, &[n, "5"]);
        for number in 184..=187 {
            act_done(&mut sim, number, &[n]);
        }
    }
    assert!(quests_untouched(&sim));
    // The edges 1 and 10 work.
    act_done(&mut sim, 27, &["1", "41"]);
    act_done(&mut sim, 27, &["10", "42"]);
    act_done(&mut sim, 182, &["1", "43"]);
    act_done(&mut sim, 182, &["10", "44"]);
    let m = &sim.state.script.mission;
    assert_eq!(
        m.main_quests[0],
        Quest {
            status: 3,
            text: 41
        }
    );
    assert_eq!(
        m.main_quests[9],
        Quest {
            status: 3,
            text: 42
        }
    );
    assert_eq!(
        m.sub_quests[0],
        Quest {
            status: 3,
            text: 43
        }
    );
    assert_eq!(
        m.sub_quests[9],
        Quest {
            status: 3,
            text: 44
        }
    );
    assert!(
        (1..9).all(|i| m.main_quests[i] == Quest::default() && m.sub_quests[i] == Quest::default())
    );
}

// QUIRK E-204: kept
#[test]
fn status_setters_keep_the_text() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    act_done(&mut sim, 27, &["2", "77"]);
    act_done(&mut sim, 182, &["3", "55"]);
    // 185/187 main objective 6/10, 184/186 secondary objective 6/10; the text stays.
    act_done(&mut sim, 185, &["2"]);
    assert_eq!(
        sim.state.script.mission.main_quests[1],
        Quest {
            status: 6,
            text: 77
        }
    );
    act_done(&mut sim, 187, &["2"]);
    assert_eq!(
        sim.state.script.mission.main_quests[1],
        Quest {
            status: 10,
            text: 77
        }
    );
    act_done(&mut sim, 184, &["3"]);
    assert_eq!(
        sim.state.script.mission.sub_quests[2],
        Quest {
            status: 6,
            text: 55
        }
    );
    act_done(&mut sim, 186, &["3"]);
    assert_eq!(
        sim.state.script.mission.sub_quests[2],
        Quest {
            status: 10,
            text: 55
        }
    );
    // Main and secondary objectives are separate tables.
    assert_eq!(sim.state.script.mission.main_quests[2], Quest::default());
    assert_eq!(sim.state.script.mission.sub_quests[1], Quest::default());
    // An objective without a prior 27 has text 0.
    act_done(&mut sim, 185, &["5"]);
    assert_eq!(
        sim.state.script.mission.main_quests[4],
        Quest { status: 6, text: 0 }
    );
    // 27 sets the status back to 3 and replaces the text.
    act_done(&mut sim, 27, &["2", "88"]);
    assert_eq!(
        sim.state.script.mission.main_quests[1],
        Quest {
            status: 3,
            text: 88
        }
    );
}

// QUIRK E-204: kept
#[test]
fn prefill_198_199_sets_status_1_for_first_n() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    act_done(&mut sim, 182, &["2", "55"]);
    act_done(&mut sim, 184, &["2"]);
    act_done(&mut sim, 198, &["3"]);
    let m = &sim.state.script.mission;
    // 0...n-1 get status 1 (also over an existing status), the text stays.
    assert_eq!(m.sub_quests[0], Quest { status: 1, text: 0 });
    assert_eq!(
        m.sub_quests[1],
        Quest {
            status: 1,
            text: 55
        }
    );
    assert_eq!(m.sub_quests[2], Quest { status: 1, text: 0 });
    assert_eq!(m.sub_quests[3], Quest::default());
    assert_eq!(m.main_quests, [Quest::default(); 10]);
    act_done(&mut sim, 199, &["2"]);
    let m = &sim.state.script.mission;
    assert_eq!(m.main_quests[0].status, 1);
    assert_eq!(m.main_quests[1].status, 1);
    assert_eq!(m.main_quests[2].status, 0);
    // n > 10 only sets 0...9.
    act_done(&mut sim, 199, &["15"]);
    assert!(
        sim.state
            .script
            .mission
            .main_quests
            .iter()
            .all(|q| q.status == 1)
    );
    // n <= 0 sets nothing.
    let mut state2 = self::state(&rules);
    let mut sim2 = Sim::new(&mut state2, &rules);
    act_done(&mut sim2, 198, &["0"]);
    act_done(&mut sim2, 199, &["-4"]);
    assert!(quests_untouched(&sim2));
}

#[test]
fn quest_actions_run_in_a_script_without_pending() {
    let level = script(&[(
        &[],
        &[
            "n27;1;10",
            "n182;2;11",
            "n184;2",
            "n185;1",
            "n186;3",
            "n187;4",
            "n198;5",
            "n199;1",
            "n15",
            "n16",
        ],
    )]);
    let rules = rules_with_areas(&level, &[]);
    let mut state = new_match(&rules, &setup(true)).expect("match with script");
    let traces = traces(step(&mut state, &rules, &[]));
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
        "{traces:?}"
    );
    let m = &state.script.mission;
    // 198 (secondary objectives 0...4 status 1) runs after 184/186 and overwrites their status; the texts stay.
    assert_eq!(
        m.main_quests[0],
        Quest {
            status: 1,
            text: 10
        }
    );
    assert_eq!(
        m.main_quests[3],
        Quest {
            status: 10,
            text: 0
        }
    );
    assert_eq!(
        m.sub_quests[1],
        Quest {
            status: 1,
            text: 11
        }
    );
    assert_eq!(m.sub_quests[2], Quest { status: 1, text: 0 });
    assert_eq!(m.sub_quests[4].status, 1);
}
