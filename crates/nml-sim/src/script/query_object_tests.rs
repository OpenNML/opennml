//! Tests of query layer II ([`super::query`], object-related queries 1400, 1500, 2100, 2200, 2600) and
//! of conditions 28 to 33 and 56, with synthetic worlds without the corpus.

use nml_formats::targets::{parse_target_defines, parse_targets};

use super::conditions::{condition, shown_value};
use super::query;
use super::query_tests::{container, state, traces};
use super::tests::cond;
use super::*;
use crate::attributes::AttributeSet;
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::names;
use crate::object::status;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::tile_map::BuildingTileMap;
use crate::world::{ObjectDesc, create_object_named, destroy_object};

const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nGUID 1101\nNAME Held\nVALUEID 100 6 1 0\nPROPID 0\n";

/// Filter ID 5: all objects of the reference player (`MM_ANDALL`, `MM_HL_MYLIST`); filter ID 4: enemies of
/// the reference player (enemy list without `MM_ANDALL`, so after the list bit `0x100` "or itself");
/// 6 does not exist.
const FLOWCONTROL: &str = "BEGIN\nBUTTONMSG 5\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n\
BEGIN\nBUTTONMSG 4\nMY_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n";

fn rules() -> Rules {
    let mut rules = Rules::synthetic(TYPES, 16);
    rules.flowcontrol_targets = parse_targets(FLOWCONTROL, &parse_target_defines("")).entries;
    rules
}

/// Object of type `guid` on the tile center `tile` for the slot `owner`; `name` registers it under the
/// name. Hit points 200, sight `sight` (attribute 18).
fn spawn(
    sim: &mut Sim,
    guid: u32,
    owner: i32,
    tile: (i32, i32),
    name: Option<&str>,
    sight: i32,
) -> ObjectId {
    let pos = [
        tile.0 as f32 * 128.0 + 64.0,
        0.0,
        tile.1 as f32 * 128.0 + 64.0,
    ];
    let mut desc = ObjectDesc::new(guid, owner, pos);
    let id = create_object_named(sim, &mut desc, name).expect("object");
    sim.object_mut(id).unwrap().attributes = AttributeSet::initial(
        |a| match a {
            14 => 200,
            18 => sight,
            _ => 0,
        },
        false,
    );
    id
}

fn face(sim: &mut Sim, id: ObjectId, degrees: i32) {
    sim.object_mut(id).unwrap().direction = Fx(degrees << 12);
}

fn hp_fraction(sim: &mut Sim, id: ObjectId, fraction: i32) {
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_fraction(14, fraction);
}

// QUIRK E-203: kept
#[test]
fn query_1400_sees_dying_and_gives_zero_for_stale_ids() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let held = spawn(&mut sim, 1100, 0, (3, 3), Some("Held"), 0);
    // Name unknown -> -1; first ID 0 -> -1 (a value "0" means "none").
    assert_eq!(query::hp_percent(&sim, "nirgends"), -1);
    names::register(&mut sim.state.names, "null", ObjectId(0));
    assert_eq!(query::hp_percent(&sim, "null"), -1);
    // Full = 100; `current_fraction / 1000` truncates (99,999 -> 99, 999 -> 0).
    assert_eq!(query::hp_percent(&sim, "held"), 100);
    assert_eq!(query::hp_percent(&sim, "HELD"), 100, "looked up lowercased");
    assert_eq!(query::hp_percent(&sim, " held"), -1, "not trimmed");
    for (fraction, percent) in [(50_000, 50), (99_999, 99), (1_000, 1), (999, 0), (0, 0)] {
        hp_fraction(&mut sim, held, fraction);
        assert_eq!(
            query::hp_percent(&sim, "held"),
            percent,
            "fraction {fraction}"
        );
    }
    // A negative raw fraction (after damage) reads as 0.
    sim.object_mut(held)
        .unwrap()
        .attributes
        .add_fraction(14, -200_000);
    assert!(sim.object(held).unwrap().attributes.raw_fraction(14) < 0);
    assert_eq!(query::hp_percent(&sim, "held"), 0);
    // Dying ones count (`GetObjectById(id, 0)`).
    hp_fraction(&mut sim, held, 30_000);
    sim.object_mut(held).unwrap().status |= status::DIED;
    assert_eq!(query::hp_percent(&sim, "held"), 30);
    // A group reads its first ID.
    let second = spawn(&mut sim, 1100, 0, (4, 4), None, 0);
    hp_fraction(&mut sim, second, 70_000);
    sim.state
        .names
        .set("gruppe", format!("{};{}", second.as_i32(), held.as_i32()));
    assert_eq!(query::hp_percent(&sim, "gruppe"), 70);
    // Removed without death: the name still points to the old ID -> 0, not -1.
    destroy_object(&mut sim, held);
    assert!(sim.state.object(held, false).is_none());
    assert_eq!(query::hp_percent(&sim, "held"), 0);
}

// QUIRK E-203: kept
#[test]
fn conditions_28_and_30_read_1400_and_a_stale_id_is_below_any_positive_limit() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let held = spawn(&mut sim, 1100, 0, (3, 3), Some("held"), 0);
    hp_fraction(&mut sim, held, 50_000);
    let mut c = container();
    let mut check = |sim: &mut Sim, n: i32, name: &str, limit: &str| {
        condition(sim, &mut c, 0, &cond(n, &[name, limit]))
    };
    // 28: percent < n and name known; 30: percent >= n (signed).
    assert!(check(&mut sim, 28, "held", "51") && !check(&mut sim, 28, "held", "50"));
    assert!(check(&mut sim, 30, "held", "50") && !check(&mut sim, 30, "held", "51"));
    assert!(!check(&mut sim, 28, "held", "-1") && check(&mut sim, 30, "held", "-1"));
    // Unknown name (-1): 28 never true; 30 only for n <= -1.
    assert!(!check(&mut sim, 28, "weg", "100") && !check(&mut sim, 28, "weg", "0"));
    assert!(!check(&mut sim, 30, "weg", "0") && check(&mut sim, 30, "weg", "-1"));
    // Removed without death -> 1400 returns 0, 28 is true for n > 0 (E-203).
    destroy_object(&mut sim, held);
    assert!(check(&mut sim, 28, "held", "1"));
    assert!(!check(&mut sim, 28, "held", "0"));
    assert!(check(&mut sim, 30, "held", "0") && !check(&mut sim, 30, "held", "1"));
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
}

#[test]
fn query_1500_reads_ki_global_list_and_guards_negative_index() {
    let rules = rules();
    let mut state = state(&rules);
    let sim = Sim::new(&mut state, &rules);
    // Without an engine -> 0, also with index < 0 (the engine check comes first).
    assert_eq!(query::ki_global(&sim, 1, 0), Ok(0));
    assert_eq!(query::ki_global(&sim, 1, -1), Ok(0));
    sim.state.ki.engine_mut(1).mem.global_list = vec![5, 7, -3];
    assert_eq!(query::ki_global(&sim, 1, 0), Ok(5));
    assert_eq!(query::ki_global(&sim, 1, 1), Ok(7));
    assert_eq!(query::ki_global(&sim, 1, 2), Ok(-3));
    // Index >= length -> 0; other slots have their own list.
    assert_eq!(query::ki_global(&sim, 1, 3), Ok(0));
    assert_eq!(query::ki_global(&sim, 1, i32::MAX), Ok(0));
    assert_eq!(query::ki_global(&sim, 2, 0), Ok(0));
    // Slot not in 0 to 7 -> 0.
    for slot in [-1, 8, 9, 31] {
        assert_eq!(query::ki_global(&sim, slot, 0), Ok(0));
    }
    // Index < 0 with an engine: an exception in the original, here an error with an error message (exception A7).
    let why = query::ki_global(&sim, 1, -1).unwrap_err();
    assert!(why.contains("index") && why.contains("-1"), "{why}");
    // Also with an empty list (-1 >= 0 is false).
    sim.state.ki.engine_mut(3);
    assert!(query::ki_global(&sim, 3, -4).is_err());
}

#[test]
fn condition_29_compares_equal_and_guards_negative_index() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.ki.engine_mut(1).mem.global_list = vec![5, 7];
    let mut c = container();
    let mut check =
        |sim: &mut Sim, params: [&str; 3]| condition(sim, &mut c, 0, &cond(29, &params));
    assert!(check(&mut sim, ["1", "0", "5"]) && !check(&mut sim, ["1", "0", "6"]));
    assert!(check(&mut sim, ["1", "1", "7"]));
    assert!(check(&mut sim, ["1", "2", "0"]), "beyond the end reads 0");
    assert!(
        check(&mut sim, ["0", "0", "0"]),
        "without an engine reads 0"
    );
    assert!(!check(&mut sim, ["0", "0", "1"]));
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
    // Index < 0: false with `Guarded`.
    assert!(!check(&mut sim, ["1", "-1", "0"]));
    let found = traces(std::mem::take(&mut sim.state.events));
    assert!(
        matches!(found.as_slice(), [ScriptTrace::Guarded { number: 29, .. }]),
        "{found:?}"
    );
}

// QUIRK E-134: kept
#[test]
fn condition_31_is_always_false() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 0, (3, 3), Some("a"), 8);
    spawn(&mut sim, 1100, 0, (3, 3), Some("b"), 8);
    let mut c = container();
    for params in [["a", "b"], ["a", "a"], ["x", "y"]] {
        assert!(!condition(&mut sim, &mut c, 0, &cond(31, &params)));
    }
    // Implemented: no `ConditionPending`, a display value.
    assert_eq!(
        shown_value(&mut sim, &c, &cond(31, &["a", "b"])),
        Some(false)
    );
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
}

// QUIRK E-192: deviation
#[test]
fn query_2100_counts_the_observer_and_same_owner_types_within_current_sight() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Sight 12 = 3 tiles (`value << 10`, one tile = 4096 = value 4).
    let observer = spawn(&mut sim, 1100, 0, (8, 8), Some("Beob"), 12);
    spawn(&mut sim, 1100, 0, (10, 8), None, 0); // 2 tiles
    spawn(&mut sim, 1100, 0, (11, 8), None, 0); // exactly 3 tiles: <= counts
    spawn(&mut sim, 1100, 0, (12, 8), None, 0); // 4 tiles: outside
    spawn(&mut sim, 1100, 1, (9, 8), None, 0); // foreign owner
    spawn(&mut sim, 1101, 0, (8, 9), None, 0); // other type, same owner
    // Type GUID: type and owner of the observer; the observer counts itself.
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), 3);
    assert_eq!(query::sees_count(&mut sim, 1101, "beob"), 1);
    // Filter ID 5: all objects of the observer's owner.
    assert_eq!(query::sees_count(&mut sim, 5, "beob"), 4);
    // The radius is the current value of attribute 18: smaller -> fewer.
    sim.object_mut(observer)
        .unwrap()
        .attributes
        .set_modifier(18, 8, false, 0);
    assert_eq!(sim.attribute(observer, 18), 8);
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), 2);
    sim.object_mut(observer)
        .unwrap()
        .attributes
        .set_modifier(18, 0, false, 0);
    assert_eq!(
        query::sees_count(&mut sim, 1100, "beob"),
        1,
        "radius 0: only itself"
    );
    sim.object_mut(observer)
        .unwrap()
        .attributes
        .set_modifier(18, 12, false, 0);
    // Camouflaged ones count, dying and quartered ones do not (filter).
    let ghost = spawn(&mut sim, 1100, 0, (9, 9), None, 0);
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), 4);
    sim.object_mut(ghost).unwrap().status |= status::MASKED;
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), 4);
    sim.object_mut(ghost).unwrap().status |= status::DIED;
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), 3);
    // A dying observer no longer finds itself, but sees (not -1).
    sim.object_mut(observer).unwrap().status |= status::DIED;
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), 2);
}

// QUIRK E-192: deviation
#[test]
fn query_2100_unknown_filter_or_missing_observer_is_minus_one() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let observer = spawn(&mut sim, 1100, 0, (8, 8), Some("beob"), 12);
    assert_eq!(query::sees_count(&mut sim, 1100, "weg"), -1);
    // Unknown filter ID (6, 0, negative) -> -1; an unknown type GUID is valid and counts 0.
    for guid in [6, 0, -1] {
        assert_eq!(
            query::sees_count(&mut sim, guid, "beob"),
            -1,
            "filter {guid}"
        );
    }
    assert_eq!(query::sees_count(&mut sim, 1999, "beob"), 0);
    // Removed observer (stale name) -> -1.
    destroy_object(&mut sim, observer);
    assert_eq!(query::sees_count(&mut sim, 1100, "beob"), -1);
    // Condition 32: `r >= n`, signed; -1 satisfies only n <= -1.
    let mut c = container();
    let mut check =
        |sim: &mut Sim, n: &str| condition(sim, &mut c, 0, &cond(32, &["beob", n, "1100"]));
    assert!(!check(&mut sim, "0") && check(&mut sim, "-1") && check(&mut sim, "-7"));
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
}

#[test]
fn condition_32_counts_at_least_n() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 0, (8, 8), Some("beob"), 8);
    spawn(&mut sim, 1100, 0, (9, 8), None, 0);
    let mut c = container();
    let mut check =
        |sim: &mut Sim, n: &str, g: &str| condition(sim, &mut c, 0, &cond(32, &["beob", n, g]));
    assert!(check(&mut sim, "2", "1100") && !check(&mut sim, "3", "1100"));
    assert!(check(&mut sim, "0", "1101") && !check(&mut sim, "1", "1101"));
}

// QUIRK E-191: deviation
#[test]
fn query_2200_uses_the_observers_tile_and_players_0_to_7() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let observer = spawn(&mut sim, 1100, 3, (6, 6), Some("Beob"), 0);
    spawn(&mut sim, 1100, 3, (9, 9), Some("fern"), 0);
    // Only explored is not enough: it must be "currently seen".
    sim.state.fog.explored.or(6, 6, 1);
    assert!(!query::tile_seen(&sim, 0, "beob"));
    sim.state.fog.seen.or(6, 6, 1);
    assert!(query::tile_seen(&sim, 0, "beob"));
    // The tile of the object counts (not the owner, not the surroundings); other slots see nothing.
    assert!(!query::tile_seen(&sim, 1, "beob"));
    assert!(!query::tile_seen(&sim, 0, "fern"));
    sim.state.fog.seen.or(7, 6, 1);
    assert!(!query::tile_seen(&sim, 0, "fern"));
    // Shared vision and revealed (stage 7) count.
    sim.state.diplomacy.set_vision(1, 0, true);
    assert!(query::tile_seen(&sim, 1, "beob"));
    sim.state.fog.cleared.or(9, 9, 4);
    assert!(!query::tile_seen(&sim, 1, "fern"));
    assert!(query::tile_seen(&sim, 2, "fern"));
    // Player not in 0 to 7 -> false (>= 8: no shared vision; < 0: local player, 0 in the reimplementation).
    sim.state.fog.seen.fill(0xFF);
    sim.state.fog.cleared.fill(0xFF);
    for slot in [-1, 8, 9, 31] {
        assert!(!query::tile_seen(&sim, slot, "beob"), "player {slot}");
    }
    // Missing name, removed object -> false; a dying observer still works.
    assert!(!query::tile_seen(&sim, 0, "weg"));
    sim.object_mut(observer).unwrap().status |= status::DIED;
    assert!(query::tile_seen(&sim, 0, "beob"));
    destroy_object(&mut sim, observer);
    assert!(!query::tile_seen(&sim, 0, "beob"));
}

// QUIRK E-191: deviation
#[test]
fn condition_33_compares_equal_one_and_guards_negative_players() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 3, (6, 6), Some("beob"), 0);
    sim.state.fog.seen.or(6, 6, 1);
    let mut c = container();
    let mut check = |sim: &mut Sim, p: &str| condition(sim, &mut c, 0, &cond(33, &[p, "beob"]));
    assert!(check(&mut sim, "0") && !check(&mut sim, "1") && !check(&mut sim, "8"));
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
    // Player < 0 (local player in the original, E-191): the load check rejects it; at run time false
    // with `Guarded`.
    assert!(!check(&mut sim, "-1"));
    let found = traces(std::mem::take(&mut sim.state.events));
    assert!(
        matches!(found.as_slice(), [ScriptTrace::Guarded { number: 33, .. }]),
        "{found:?}"
    );
}

/// World for the cone query: observer `beob` (type 1100, slot 0) at (8, 8) with sight 20 (5 tiles),
/// facing `facing` degrees.
fn cone_world(rules: &Rules, facing: i32) -> (SimState, ObjectId) {
    let mut state = state(rules);
    let mut sim = Sim::new(&mut state, rules);
    let observer = spawn(&mut sim, 1100, 0, (8, 8), Some("beob"), 20);
    face(&mut sim, observer, facing);
    (state, observer)
}

// QUIRK E-192: deviation
#[test]
fn query_2600_strict_cone_line_of_sight_and_or_filter() {
    let rules = rules();
    // A candidate of slot 1 exactly in front of the observer (+x, 3 tiles).
    let (mut state, _) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 1, (11, 8), None, 0);
    let sees =
        |sim: &mut Sim, g: i32, p2: i32, angle: i32| query::sees_in_cone(sim, "beob", g, p2, angle);
    // Cone `-h < delta < h`, `h = (angle << 12) / 2`: delta = 0 is inside for every angle > 0.
    assert!(
        sees(&mut sim, 1100, 1, 1) && sees(&mut sim, 1100, 1, 90) && sees(&mut sim, 1100, 1, 360)
    );
    assert!(!sees(&mut sim, 1100, 1, 0), "h = 0: empty cone");
    assert!(!sees(&mut sim, 1100, 1, -90), "h < 0: empty cone");
    // Type GUID: type and owner p2.
    assert!(!sees(&mut sim, 1101, 1, 90), "wrong type");
    assert!(!sees(&mut sim, 1100, 0, 90), "owner is not p2");
    assert!(!sees(&mut sim, 1100, 2, 90));
    // Edge cases of strictness: facing 90 degrees, candidate at 0 degrees -> delta = +/-90 degrees;
    // angle 180 is h = 90 degrees (equality: no).
    for (facing, delta_sign) in [(90, 1), (-90, -1)] {
        let (mut state, observer) = cone_world(&rules, 0);
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, 1100, 1, (11, 8), None, 0);
        face(&mut sim, observer, facing);
        assert!(
            !query::sees_in_cone(&mut sim, "beob", 1100, 1, 180),
            "delta = {}",
            90 * delta_sign
        );
        assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 181));
        assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 179));
    }
    // Behind the observer: candidate at -180 degrees (the angle wraps to [-180, 180) degrees), facing 0 degrees
    // -> delta = -180 degrees.
    let (mut state, observer) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 1, (5, 8), None, 0);
    assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 359));
    assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 360));
    assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 361));
    // An unwrapped facing (180 degrees) is wrapped: delta = wrap(180 - (-180) degrees) = 0.
    face(&mut sim, observer, 180);
    assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
    // Candidate to the side (+z, 90 degrees): facing 0 degrees -> delta = -90 degrees.
    let (mut state, _) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 1, (8, 11), None, 0);
    assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 180));
    assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 181));
    // Line of sight: a tall building in between blocks, without it he sees again.
    let (mut state, _) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    let ahead2 = spawn(&mut sim, 1100, 1, (11, 8), None, 0);
    sim.state.tile_map = BuildingTileMap::new(16, 16);
    sim.state.tile_map.set_cell(9, 8, 1 << 20, 0x9_0001, true);
    assert!(
        !query::sees_in_cone(&mut sim, "beob", 1100, 1, 90),
        "wall blocks"
    );
    sim.state.tile_map.set_cell(9, 8, 0, 0, true);
    assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
    // Outside the sight (6 tiles > 5): not in the preselection.
    sim.object_mut(ahead2).unwrap().status |= status::DIED;
    spawn(&mut sim, 1100, 1, (14, 8), None, 0);
    assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
}

// QUIRK E-192: deviation
#[test]
fn query_2600_skips_itself_filters_unknown_ids_and_extends_lists_with_or() {
    let rules = rules();
    // Skips itself: observer and candidate of the same slot; unknown filter ID -> every object of p2 (here
    // p2 = slot of the observer).
    let (mut state, _) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(
        !query::sees_in_cone(&mut sim, "beob", 99, 0, 360),
        "only itself"
    );
    let friend = spawn(&mut sim, 1101, 0, (10, 8), None, 0);
    assert!(
        query::sees_in_cone(&mut sim, "beob", 99, 0, 90),
        "unknown ID: every object of p2"
    );
    assert!(
        !query::sees_in_cone(&mut sim, "beob", 99, 1, 90),
        "p2 = 1 has none"
    );
    assert!(
        !query::sees_in_cone(&mut sim, "beob", 1100, 0, 90),
        "type 1101 is not 1100"
    );
    // Filter ID 5 (ANDALL, MYLIST): objects of p2.
    assert!(query::sees_in_cone(&mut sim, "beob", 5, 0, 90));
    assert!(!query::sees_in_cone(&mut sim, "beob", 5, 1, 90));
    // Dying candidates drop out of the filter, camouflaged ones count.
    sim.object_mut(friend).unwrap().status |= status::MASKED;
    assert!(query::sees_in_cone(&mut sim, "beob", 5, 0, 90));
    sim.object_mut(friend).unwrap().status |= status::DIED;
    assert!(!query::sees_in_cone(&mut sim, "beob", 5, 0, 90));

    // Filter ID 4 (enemy list without ANDALL) | 0x100: "enemies of p2 or p2 itself".
    let (mut state, _) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 2, (10, 8), None, 0);
    // Slot 2 is an enemy of slot 1 (default): seen.
    assert!(query::sees_in_cone(&mut sim, "beob", 4, 1, 90));
    // No longer an enemy, but not p2 either: not seen.
    sim.state.diplomacy.set_enemy(1, 2, false);
    assert!(!query::sees_in_cone(&mut sim, "beob", 4, 1, 90));
    // p2 itself via the list bit 0x100.
    spawn(&mut sim, 1100, 1, (11, 8), None, 0);
    assert!(query::sees_in_cone(&mut sim, "beob", 4, 1, 90));
}

// QUIRK E-192: deviation
#[test]
fn query_2600_guards_missing_observers_and_empty_sight() {
    let rules = rules();
    let (mut state, observer) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 1, (10, 8), None, 0);
    assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
    assert!(!query::sees_in_cone(&mut sim, "weg", 1100, 1, 90));
    names::register(&mut sim.state.names, "null", ObjectId(0));
    assert!(!query::sees_in_cone(&mut sim, "null", 1100, 1, 90));
    // Sight 0 (`r < 1`) -> 0, also with an object on the same spot.
    sim.object_mut(observer)
        .unwrap()
        .attributes
        .set_modifier(18, 0, false, 0);
    assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
    sim.object_mut(observer)
        .unwrap()
        .attributes
        .set_modifier(18, 20, false, 0);
    // A dying observer still sees.
    sim.object_mut(observer).unwrap().status |= status::DIED;
    assert!(query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
    // Removed: the name points to a dead ID -> 0.
    destroy_object(&mut sim, observer);
    assert!(!query::sees_in_cone(&mut sim, "beob", 1100, 1, 90));
}

#[test]
fn condition_56_reads_2600_with_name_guid_player_angle() {
    let rules = rules();
    let (mut state, _) = cone_world(&rules, 0);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, 1100, 1, (10, 8), None, 0);
    let mut c = container();
    let mut check =
        |sim: &mut Sim, params: [&str; 4]| condition(sim, &mut c, 0, &cond(56, &params));
    assert!(check(&mut sim, ["beob", "1100", "1", "90"]));
    assert!(!check(&mut sim, ["beob", "1100", "1", "0"]));
    assert!(!check(&mut sim, ["beob", "1100", "0", "90"]));
    assert!(!check(&mut sim, ["weg", "1100", "1", "90"]));
    assert!(traces(std::mem::take(&mut sim.state.events)).is_empty());
}

#[test]
fn objectwise_numbers_report_no_pending_and_show_a_value() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let mut c = container();
    let numbers: &[(i32, &[&str])] = &[
        (28, &["x", "1"]),
        (29, &["0", "0", "0"]),
        (30, &["x", "1"]),
        (31, &["x", "y"]),
        (32, &["x", "1", "1100"]),
        (33, &["0", "x"]),
        (56, &["x", "1100", "0", "90"]),
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
