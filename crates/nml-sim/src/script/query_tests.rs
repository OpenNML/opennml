//! Tests of query layer I ([`super::query`]) and of the conditions on counters and areas, with
//! synthetic worlds without the corpus.

use nml_formats::abl::ScriptFile;
use nml_formats::targets::{parse_target_defines, parse_targets};

use super::conditions::{condition, shown_value};
use super::query;
use super::tests::{cond, rules_with_areas, setup, spawn};
use super::*;
use crate::id::ObjectId;
use crate::object::status;
use crate::rules::Rules;
use crate::setup::new_match;
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::{object_event_bits, set_object_event_bit};

/// Filter ID 5: all objects of the reference player (`MM_ANDALL`, `MM_HL_MYLIST`); 6 does not exist.
const FLOWCONTROL: &str = "BEGIN\nBUTTONMSG 5\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n";

const FELD: [i32; 4] = [2, 2, 5, 5];

fn rules() -> Rules {
    let mut rules = rules_with_areas(
        "",
        &[
            ("feld", FELD),
            ("verdreht", [5, 5, 2, 2]),
            ("halb", [5, 2, 2, 5]),
        ],
    );
    rules.flowcontrol_targets = parse_targets(FLOWCONTROL, &parse_target_defines("")).entries;
    rules
}

pub(super) fn state(rules: &Rules) -> SimState {
    new_match(rules, &setup(false)).unwrap()
}

/// Object of type 1100 on the tile `tile` for the slot `owner`.
fn spawn_for(sim: &mut Sim, owner: i32, tile: (i32, i32)) -> ObjectId {
    let pos = [
        tile.0 as f32 * 128.0 + 64.0,
        0.0,
        tile.1 as f32 * 128.0 + 64.0,
    ];
    let mut desc = crate::world::ObjectDesc::new(1100, owner, pos);
    crate::world::create_object_named(sim, &mut desc, None).expect("object")
}

pub(super) fn container() -> Container {
    let source = ScriptSource {
        path: String::new(),
        checksum: 0,
    };
    Container::new(1, source, &ScriptFile { blocks: Vec::new() })
}

pub(super) fn traces(events: Vec<SimEvent>) -> Vec<ScriptTrace> {
    events
        .into_iter()
        .filter_map(|e| match e {
            SimEvent::Script(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn query_100_reads_raw_shifted_and_caps_housing() {
    let rules = rules();
    let mut state = state(&rules);
    let sim = Sim::new(&mut state, &rules);
    sim.state.economy.max_living_space = 20;
    let p = &mut sim.state.players[0];
    p.set_resource_raw(0, (5 << 12) + 0xFFF);
    p.set_resource_raw(1, -1);
    p.set_resource_raw(2, -(3 << 12));
    p.set_resource_raw(3, 7 << 12);
    p.set_resource_raw(4, 50 << 12);
    // `raw >> 12` arithmetic: fractions drop off, negatives round toward -infinity.
    assert_eq!(query::resource(&sim, 0, 0), 5);
    assert_eq!(query::resource(&sim, 0, 1), -1);
    assert_eq!(query::resource(&sim, 0, 2), -3);
    assert_eq!(query::resource(&sim, 0, 3), 7);
    // Kind 4 (living space) at most the maximum; the stored amount itself stays.
    assert_eq!(query::resource(&sim, 0, 4), 20);
    assert_eq!(sim.state.players[0].resource_raw(4), 50 << 12);
    sim.state.players[0].set_resource_raw(4, 20 << 12);
    assert_eq!(query::resource(&sim, 0, 4), 20);
    sim.state.players[0].set_resource_raw(4, 12 << 12);
    assert_eq!(query::resource(&sim, 0, 4), 12);
    // Kind not in 0 to 4 -> 0; slot not in 0 to 7 -> 0 (exception A7).
    for kind in [-1, 5, 99] {
        assert_eq!(query::resource(&sim, 0, kind), 0);
    }
    for slot in [-1, 8, 9, 31] {
        assert_eq!(query::resource(&sim, slot, 0), 0);
    }
}

// QUIRK E-193: deviation
#[test]
fn conditions_2_and_12_compare_signed_and_guard_invalid_players() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.players[0].set_resource_raw(0, 5 << 12);
    sim.state.players[0].set_resource_raw(1, -(2 << 12));
    let mut c = container();
    let mut check =
        |sim: &mut Sim, n: i32, params: &[&str]| condition(sim, &mut c, 0, &cond(n, params));
    assert!(check(&mut sim, 2, &["0", "5", "0"]));
    assert!(!check(&mut sim, 2, &["0", "6", "0"]));
    assert!(check(&mut sim, 2, &["0", "-2", "1"]), "signed: -2 >= -2");
    assert!(!check(&mut sim, 2, &["0", "-1", "1"]));
    assert!(check(&mut sim, 12, &["0", "6", "0"]));
    assert!(!check(&mut sim, 12, &["0", "5", "0"]));
    assert!(check(&mut sim, 12, &["0", "-1", "1"]));
    // Kind outside 0 to 4 reads 0, also with a player outside 0 to 7 (no access in the original).
    assert!(check(&mut sim, 2, &["0", "0", "7"]));
    assert!(check(&mut sim, 2, &["9", "0", "7"]));
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
    // Player not in 0 to 7 with kind 0 to 4: null pointer in the original, the load check rejects it; at run
    // time false with `Guarded`.
    assert!(!check(&mut sim, 2, &["8", "0", "0"]));
    assert!(!check(&mut sim, 12, &["-1", "5", "4"]));
    let found = traces(std::mem::take(&mut sim.state.events));
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .all(|t| matches!(t, ScriptTrace::Guarded { .. }))
    );
}

// QUIRK E-138: kept
#[test]
fn query_200_counts_by_tile_inclusive_without_dying_or_quartered() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 0, (5, 5)); // corner: inclusive
    spawn_for(&mut sim, 0, (2, 2)); // corner: inclusive
    spawn_for(&mut sim, 0, (6, 3)); // right next to it
    spawn_for(&mut sim, 0, (1, 3));
    let dying = spawn_for(&mut sim, 0, (4, 4));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    let quartered = spawn_for(&mut sim, 0, (4, 3));
    sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
    spawn_for(&mut sim, 1, (3, 4));
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "feld"), 3);
    assert_eq!(query::count_in_area(&mut sim, 1, 1100, "feld"), 1);
    assert_eq!(query::count_in_area(&mut sim, 2, 1100, "feld"), 0);
    // Name as the conditions deliver it: searched trimmed and lowercased.
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, " FELD "), 3);
    // Unknown area -> 0.
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "weg"), 0);
}

// QUIRK E-190: deviation
#[test]
fn query_200_with_swapped_corners_counts_zero_until_condition_6_normalised_it() {
    // Query 200 does not swap the corners (0), condition 6/query 300 swaps them in the stored rectangle,
    // afterwards 200 (and every reader via `script::area`) counts normally.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 0, (4, 5));
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "verdreht"), 0);
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "halb"), 0);
    assert!(sim.state.script.area_overrides.is_empty());
    assert_eq!(super::area(&sim, "verdreht"), Some([5, 5, 2, 2]));
    // A slot not in 0 to 7 swaps nothing.
    assert!(!query::area_seen(&mut sim, 8, "verdreht"));
    assert!(!query::area_seen(&mut sim, -1, "verdreht"));
    assert!(sim.state.script.area_overrides.is_empty());
    // An unknown area neither.
    assert!(!query::area_seen(&mut sim, 0, "weg"));
    assert!(sim.state.script.area_overrides.is_empty());
    // Valid player: swap in the stored rectangle (also without sight), per axis individually.
    assert!(!query::area_seen(&mut sim, 0, " Verdreht"));
    assert_eq!(
        sim.state.script.area_overrides.get("verdreht"),
        Some(&[2, 2, 5, 5])
    );
    assert!(!query::area_seen(&mut sim, 0, "halb"));
    assert_eq!(
        sim.state.script.area_overrides.get("halb"),
        Some(&[2, 2, 5, 5])
    );
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "verdreht"), 2);
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "halb"), 2);
    // The rules stay; a normal area writes nothing.
    assert_eq!(sim.rules.scripts.area("verdreht"), Some([5, 5, 2, 2]));
    assert!(!query::area_seen(&mut sim, 0, "feld"));
    assert_eq!(sim.state.script.area_overrides.len(), 2);
}

// QUIRK E-190: deviation
#[test]
fn condition_6_swaps_through_the_condition_and_1300_follows() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let id = spawn(&mut sim, Some("held"), (3, 3));
    assert_eq!(sim.object(id).unwrap().tile, (3, 3));
    assert!(!super::conditions::query_1300(&sim, "held", "verdreht"));
    let mut c = container();
    // Condition 6 with the `area ` prefix; nothing seen -> false, but the rectangle is swapped.
    assert!(!condition(
        &mut sim,
        &mut c,
        0,
        &cond(6, &["0", "area verdreht"])
    ));
    assert!(super::conditions::query_1300(&sim, "held", "verdreht"));
    assert_eq!(
        shown_value(&mut sim, &c, &cond(6, &["0", "area feld"])),
        Some(false)
    );
}

// QUIRK E-138: kept
#[test]
fn query_200_guid_zero_or_unknown_filter_counts_zero() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 0, (4, 4));
    spawn_for(&mut sim, 1, (4, 3));
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "feld"), 2);
    // GUID 0 and an unknown filter ID count 0 (not "all objects of the reference player").
    assert_eq!(query::count_in_area(&mut sim, 0, 0, "feld"), 0);
    assert_eq!(query::count_in_area(&mut sim, 0, 6, "feld"), 0);
    assert_eq!(query::count_in_area(&mut sim, 0, -3, "feld"), 0);
    // Filter ID 5 from `flowcontroltragets.ini`: all objects of the reference player.
    assert_eq!(query::count_in_area(&mut sim, 0, 5, "feld"), 2);
    assert_eq!(query::count_in_area(&mut sim, 1, 5, "feld"), 1);
    // An unknown type is a valid filter without hits.
    assert_eq!(query::count_in_area(&mut sim, 0, 1999, "feld"), 0);
}

// QUIRK E-138: kept
#[test]
fn query_200_player_minus_one_counts_all_owners() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 1, (4, 3));
    spawn_for(&mut sim, 7, (4, 4));
    assert_eq!(query::count_in_area(&mut sim, -1, 1100, "feld"), 3);
    assert_eq!(query::count_in_area(&mut sim, 0, 1100, "feld"), 1);
    assert_eq!(query::count_in_area(&mut sim, 7, 1100, "feld"), 1);
}

#[test]
fn conditions_3_11_13_14_compare_signed() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 0, (4, 4));
    let mut c = container();
    let mut check = |sim: &mut Sim, n: i32, k: &str| {
        condition(sim, &mut c, 0, &cond(n, &["0", k, "1100", "area feld"]))
    };
    let s = &mut sim;
    assert!(check(s, 3, "2") && !check(s, 3, "3") && check(s, 3, "-1"));
    assert!(check(s, 11, "2") && !check(s, 11, "1") && !check(s, 11, "-2"));
    assert!(check(s, 13, "1") && !check(s, 13, "2") && check(s, 13, "-1"));
    assert!(check(s, 14, "3") && !check(s, 14, "2") && !check(s, 14, "-1"));
    // An area without `area ` is blindly shortened by five characters (no area -> 0).
    assert!(!condition(
        &mut sim,
        &mut c,
        0,
        &cond(3, &["0", "1", "1100", "feld"])
    ));
}

// QUIRK E-190: deviation
#[test]
fn query_300_needs_seen_bit_and_rejects_players_outside_0_to_7() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Only explored is not enough: it must be "currently seen" (`& 2`).
    sim.state.fog.explored.or(3, 3, 1);
    assert!(!query::area_seen(&mut sim, 0, "feld"));
    // Seen on a partial area suffices, also at the edge (inclusive).
    sim.state.fog.seen.or(5, 5, 1);
    assert!(query::area_seen(&mut sim, 0, "feld"));
    // Other slots do not see this, except with shared vision.
    assert!(!query::area_seen(&mut sim, 1, "feld"));
    sim.state.diplomacy.set_vision(1, 0, true);
    assert!(query::area_seen(&mut sim, 1, "feld"));
    // A tile outside the rectangle does not count.
    sim.state.fog.seen.clear();
    sim.state.fog.seen.or(6, 6, 1);
    assert!(!query::area_seen(&mut sim, 0, "feld"));
    // Revealed (stage 7) counts as seen.
    sim.state.fog.cleared.or(2, 2, 1);
    assert!(query::area_seen(&mut sim, 0, "feld"));
    // Slot not in 0 to 7 -> false, also with a full fog state.
    sim.state.fog.seen.fill(0xFF);
    sim.state.fog.cleared.fill(0xFF);
    for slot in [-1, 8, 9, 31] {
        assert!(!query::area_seen(&mut sim, slot, "feld"));
    }
    assert!(!query::area_seen(&mut sim, 0, "weg"));
}

// QUIRK E-138: kept
#[test]
fn query_400_counts_stock_with_quartered_and_filters_below_1000() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 0, (9, 9));
    let quartered = spawn_for(&mut sim, 0, (4, 3));
    sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
    let dying = spawn_for(&mut sim, 0, (4, 4));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    spawn_for(&mut sim, 8, (1, 1));
    sim.state.add_guid_count(0, 1100, 4);
    sim.state.add_guid_count(1, 1100, 9);
    // Type GUID: the object counter (counts quartered and dying ones too).
    assert_eq!(query::count(&mut sim, 0, 1100), 4);
    assert_eq!(query::count(&mut sim, 1, 1100), 9);
    assert_eq!(query::count(&mut sim, 0, 1101), 0);
    assert_eq!(query::guid_count(&sim, 0, 1100), 4);
    // Filter ID 5 (< 1000): the object table of the slot without dead and quartered.
    assert_eq!(query::count(&mut sim, 0, 5), 2);
    assert_eq!(query::count(&mut sim, 1, 5), 0);
    // Unknown filter ID -> 0, not "all".
    assert_eq!(query::count(&mut sim, 0, 6), 0);
    // Neutral (8) and all slots not in 0 to 7 -> 0, also with objects and counters.
    sim.state.add_guid_count(8, 1100, 1);
    for slot in [-1, 8, 9, 10, 31] {
        assert_eq!(query::count(&mut sim, slot, 5), 0, "slot {slot}");
        assert_eq!(query::count(&mut sim, slot, 1100), 0, "slot {slot}");
        assert_eq!(query::guid_count(&sim, slot, 1100), 0, "slot {slot}");
    }
}

// QUIRK E-134: kept
#[test]
fn conditions_7_and_9_are_equal_and_8_and_10_are_equal() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.add_guid_count(0, 1100, 3);
    let mut c = container();
    let mut check =
        |sim: &mut Sim, n: i32, k: &str| condition(sim, &mut c, 0, &cond(n, &["0", k, "1100"]));
    for (k, less, more) in [
        ("4", true, false),
        ("3", false, false),
        ("2", false, true),
        ("-1", false, true),
    ] {
        assert_eq!(check(&mut sim, 7, k), less, "7 with {k}");
        assert_eq!(check(&mut sim, 9, k), less, "9 with {k}");
        assert_eq!(check(&mut sim, 8, k), more, "8 with {k}");
        assert_eq!(check(&mut sim, 10, k), more, "10 with {k}");
    }
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
}

// QUIRK E-134: kept
#[test]
fn condition_21_is_true_for_one_letter_names() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let mut c = container();
    let mut check = |sim: &mut Sim, name: &str| condition(sim, &mut c, 0, &cond(21, &[name]));
    assert!(check(&mut sim, "a"));
    assert!(check(&mut sim, "ä"), "character, not bytes");
    assert!(!check(&mut sim, "ab"));
    assert!(!check(&mut sim, ""));
    assert!(!check(&mut sim, "held 1"));
}

// QUIRK E-137: kept
#[test]
fn conditions_25_26_read_kill_counts() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.kill_counts[0].insert(1100, 5);
    sim.state.kill_counts[1].insert(1100, 1);
    let mut c = container();
    let mut check = |sim: &mut Sim, n: i32, p: &str, g: &str, k: &str| {
        condition(sim, &mut c, 0, &cond(n, &[p, g, k]))
    };
    let s = &mut sim;
    // 25: counter != 0 and > n.
    assert!(check(s, 25, "0", "1100", "4") && !check(s, 25, "0", "1100", "5"));
    assert!(check(s, 25, "0", "1100", "-1"));
    assert!(
        !check(s, 25, "2", "1100", "-1"),
        "counter 0 is never \"more\""
    );
    assert!(!check(s, 25, "0", "1101", "-5"));
    // 26: 0 < counter < n.
    assert!(check(s, 26, "0", "1100", "6") && !check(s, 26, "0", "1100", "5"));
    assert!(
        !check(s, 26, "2", "1100", "10"),
        "counter 0 is never \"less\""
    );
    assert!(check(s, 26, "1", "1100", "2"));
    // Slots not in 0 to 7 count 0.
    for p in ["-1", "8", "9"] {
        assert!(!check(s, 25, p, "1100", "-1"));
        assert!(!check(s, 26, p, "1100", "9"));
    }
    // 40/41 read the object counter.
    sim.state.add_guid_count(0, 1100, 2);
    let mut check = |sim: &mut Sim, n: i32, p: &str, extra: &[&str]| {
        let mut params = vec![p, "1100"];
        params.extend_from_slice(extra);
        condition(sim, &mut c, 0, &cond(n, &params))
    };
    let s = &mut sim;
    assert!(check(s, 40, "0", &["2"]) && !check(s, 40, "0", &["3"]) && check(s, 40, "0", &["-1"]));
    assert!(check(s, 41, "0", &[]) && !check(s, 41, "1", &[]) && !check(s, 41, "8", &[]));
}

#[test]
fn conditions_50_to_55_compare_with_variable_and_ignore_missing() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn_for(&mut sim, 0, (3, 3));
    spawn_for(&mut sim, 0, (4, 4));
    sim.state.add_guid_count(0, 1100, 3);
    let mut c = container();
    c.variables.insert("v".into(), 3);
    c.variables.insert("klein".into(), 1);
    c.variables.insert("neg".into(), -1);
    c.variables.insert("gross".into(), 4);
    c.variables.insert("zwei".into(), 2);
    let stock = |sim: &mut Sim, c: &mut Container, n: i32, var: &str| {
        condition(sim, c, 0, &cond(n, &["0", var, "1100"]))
    };
    // Object counter 3 against the variable: 50 `<`, 51 `>`, 52 `==`.
    assert!(!stock(&mut sim, &mut c, 50, "v") && !stock(&mut sim, &mut c, 51, "v"));
    assert!(stock(&mut sim, &mut c, 52, "v"));
    assert!(!stock(&mut sim, &mut c, 50, "klein") && stock(&mut sim, &mut c, 51, "klein"));
    assert!(!stock(&mut sim, &mut c, 52, "klein"));
    assert!(stock(&mut sim, &mut c, 51, "neg"), "signed");
    assert!(stock(&mut sim, &mut c, 50, "gross"));
    // Missing variable -> false, no message.
    for n in [50, 51, 52] {
        assert!(!stock(&mut sim, &mut c, n, "fehlt"));
    }
    // 53 to 55: number in the area (2) against the variable: 53 `>`, 54 `<`, 55 `==`.
    let area = |sim: &mut Sim, c: &mut Container, n: i32, var: &str| {
        condition(sim, c, 0, &cond(n, &["0", var, "1100", "area feld"]))
    };
    assert!(area(&mut sim, &mut c, 53, "klein") && !area(&mut sim, &mut c, 53, "v"));
    assert!(area(&mut sim, &mut c, 54, "v") && !area(&mut sim, &mut c, 54, "klein"));
    assert!(area(&mut sim, &mut c, 55, "zwei") && !area(&mut sim, &mut c, 55, "v"));
    assert!(area(&mut sim, &mut c, 53, "neg"));
    for n in [53, 54, 55] {
        assert!(!area(&mut sim, &mut c, n, "fehlt"));
    }
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
}

#[test]
fn condition_20_does_not_clear_bits() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, Some("a"), (1, 1));
    let b = spawn(&mut sim, Some("b"), (2, 2));
    set_object_event_bit(&mut sim, a, 1, true);
    set_object_event_bit(&mut sim, a, 4, true);
    let mut c = container();
    let mut check =
        |sim: &mut Sim, name: &str, bit: &str| condition(sim, &mut c, 0, &cond(20, &[name, bit]));
    assert!(check(&mut sim, "a", "1"));
    assert!(check(&mut sim, "a", "1"), "true twice: nothing was deleted");
    assert!(check(&mut sim, "a", "4") && !check(&mut sim, "a", "2"));
    assert!(!check(&mut sim, "b", "1") && !check(&mut sim, "unbekannt", "1"));
    assert_eq!(object_event_bits(&sim, a), (1 << 1) | (1 << 4));
    assert_eq!(object_event_bits(&sim, b), 0);
    // `shown_value` changes nothing either.
    assert_eq!(
        shown_value(&mut sim, &c, &cond(20, &["a", "1"])),
        Some(true)
    );
    assert_eq!(object_event_bits(&sim, a), (1 << 1) | (1 << 4));
}

#[test]
fn implemented_numbers_report_no_pending_and_show_a_value() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let mut c = container();
    c.variables.insert("v".into(), 1);
    let numbers: &[(i32, &[&str])] = &[
        (2, &["0", "0", "0"]),
        (3, &["0", "0", "1100", "area feld"]),
        (6, &["0", "area feld"]),
        (7, &["0", "0", "1100"]),
        (8, &["0", "0", "1100"]),
        (9, &["0", "0", "1100"]),
        (10, &["0", "0", "1100"]),
        (11, &["0", "0", "1100", "area feld"]),
        (12, &["0", "0", "0"]),
        (13, &["0", "0", "1100", "area feld"]),
        (14, &["0", "0", "1100", "area feld"]),
        (20, &["a", "1"]),
        (21, &["a"]),
        (25, &["0", "1100", "0"]),
        (26, &["0", "1100", "0"]),
        (40, &["0", "1100", "0"]),
        (41, &["0", "1100"]),
        (50, &["0", "v", "1100"]),
        (51, &["0", "v", "1100"]),
        (52, &["0", "v", "1100"]),
        (53, &["0", "v", "1100", "area feld"]),
        (54, &["0", "v", "1100", "area feld"]),
        (55, &["0", "v", "1100", "area feld"]),
    ];
    for (n, params) in numbers {
        let one = cond(*n, params);
        assert!(
            shown_value(&mut sim, &c, &one).is_some(),
            "condition {n} not implemented yet"
        );
        condition(&mut sim, &mut c, 0, &one);
    }
    let pending: Vec<_> = traces(std::mem::take(&mut sim.state.events))
        .into_iter()
        .filter(|t| matches!(t, ScriptTrace::ConditionPending { .. }))
        .collect();
    assert!(pending.is_empty(), "{pending:?}");
}

#[test]
fn query_500_uses_c_division_and_the_runtime_table() {
    let mut rules = rules();
    rules.scripts.areas.insert("negativ".into(), [-3, 0, 0, 5]);
    let mut state = state(&rules);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(query::area_center(&sim, "negativ"), (-1, 2));
    assert_eq!(query::area_center(&sim, "feld"), (3, 3));
    assert_eq!(query::area_center(&sim, "unbekannt"), (0, 0));
    // A change in the savegame (condition 6, later 211/214) applies immediately.
    sim.state
        .script
        .area_overrides
        .insert("feld".into(), [0, 0, 10, 20]);
    assert_eq!(query::area_center(&sim, " Feld "), (5, 10));
}

#[test]
fn ki_start_center_follows_area_overrides() {
    // The AI start (action 0/1, query 500) also reads the run-time table, not the rules.
    use super::actions::{MsgValue, build_msg};
    use nml_formats::abl::RtAction;
    let rules = rules();
    let mut state = state(&rules);
    let sim = Sim::new(&mut state, &rules);
    let action = RtAction {
        wait: false,
        number: 0,
        params: vec!["3".to_string(), "area feld".to_string()],
        line: 1,
    };
    let center = |sim: &Sim| build_msg(sim, &action).values;
    assert_eq!(
        center(&sim),
        [MsgValue::Int(3), MsgValue::Int(3), MsgValue::Int(3)]
    );
    // An enlargement in the savegame (211/214) moves the center.
    sim.state
        .script
        .area_overrides
        .insert("feld".into(), [2, 2, 9, 7]);
    assert_eq!(
        center(&sim),
        [MsgValue::Int(3), MsgValue::Int(5), MsgValue::Int(4)]
    );
}
