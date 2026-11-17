//! Tests of the counts and object counters (re/ki-queries 5.12; E-30, E-64 ... E-67): object count per GUID and filter,
//! construction orders, rectangle counts, sum over the reference players, buildability and purchasability,
//! counter-upgrades.

use nml_formats::targets::parse_targets;

use super::handlers::{ROWS, row};
use super::tests_player::{ME, ask, run, run_with};
use super::tests_search::{ME_I, MINE, oid, put, search_rules};
use super::{dispatch, is_query, query_once};
use crate::combat::testkit::{HQ, WARRIOR, defines, state};
use crate::command::testkit::{COLONIST, HOUSE, fogged_state, site};
use crate::economy::add_pending_guid_count;
use crate::ki::signature::signature;
use crate::object::status;
use crate::player::{Sight, View};
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;
use crate::upgrades::CounterUpgrade;

const W: i32 = WARRIOR as i32;
/// Filter ID 7: warriors of **all** owners (`MM_CHECKGUID`, no list mode).
const ANY_WARRIOR: i32 = 7;
/// Filter ID 8: warriors of the reference player (`MM_HL_MYLIST`).
const MY_WARRIOR: i32 = 8;

const EXTRA_TARGETS: &str = concat!(
    "BEGIN\nBUTTONMSG 7\nMY_MM\nMM_MODE MM_CHECKGUID\nMM_MODE MM_ANDALL\nMM_GUID 1400\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 8\nMY_MM\nMM_MODE MM_CHECKGUID\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_GUID 1400\nMM_END\nTARGET_MM\nMM_END\nEND\n",
);

/// Search rules (filter IDs 4, 5, 182, 183) with the filter IDs 7 and 8.
fn count_rules() -> Rules {
    let mut rules = search_rules();
    rules
        .flowcontrol_targets
        .extend(parse_targets(EXTRA_TARGETS, &defines()).entries);
    rules
}

fn scene() -> (Rules, SimState) {
    let rules = count_rules();
    let state = state(&rules);
    (rules, state)
}

/// Query field `code`, `argc`, from `[2]` the words `words` (rest 0), omniscient from the view of [`ME`].
fn field(sim: &mut Sim, sight: Sight, code: u32, argc: i32, words: &[i32]) -> Vec<i32> {
    let mut req = vec![0; 2 + 16];
    req[0] = code as i32;
    req[1] = argc;
    req[2..2 + words.len()].copy_from_slice(words);
    dispatch(&mut View::new(sim, ME, sight), &mut req, &mut Vec::new());
    req
}

#[test]
fn guid_presence_and_owned_counts_follow_the_player_tables() {
    // 0x94/0x95 -> 0x4E3F, 0xA5/0xA6 -> 0x4E47: GUID from the object counter map, filter ID from the table.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for tile in [(10, 10), (12, 10)] {
        put(&mut sim, WARRIOR, ME_I, tile);
    }
    put(&mut sim, COLONIST, ME_I, (14, 10));
    put(&mut sim, WARRIOR, 0, (18, 10));
    // 0x94 (type): object count of `me` > 0; 0x95 (p, type). A filter ID is only a map lookup (0).
    assert_eq!(run(&mut sim, 0x94, &[W]), [1]);
    assert_eq!(run(&mut sim, 0x94, &[HQ as i32]), [0]);
    assert_eq!(
        run(&mut sim, 0x94, &[MINE]),
        [0],
        "filter ID: map lookup only"
    );
    assert_eq!(run(&mut sim, 0x95, &[0, W]), [1]);
    assert_eq!(run(&mut sim, 0x95, &[2, W]), [0]);
    for p in [8, -1, 1000] {
        assert_eq!(run(&mut sim, 0x95, &[p, W]), [0], "slot {p}");
    }
    // 0xA5 (type): GUID -> object count; filter ID -> objects of the table with the reference `me`.
    assert_eq!(run(&mut sim, 0xA5, &[W]), [2]);
    assert_eq!(run(&mut sim, 0xA5, &[COLONIST as i32]), [1]);
    assert_eq!(run(&mut sim, 0xA5, &[MINE]), [3]);
    assert_eq!(
        run(&mut sim, 0xA5, &[ANY_WARRIOR]),
        [2],
        "only the table of `me`"
    );
    // Unknown filter ID (also 0 and negative): 0 (E-66).
    for id in [0, 999, -5, i32::MIN] {
        assert_eq!(run(&mut sim, 0xA5, &[id]), [0], "ID {id}");
    }
    // 0xA6 (p, type).
    assert_eq!(run(&mut sim, 0xA6, &[0, W]), [1]);
    assert_eq!(run(&mut sim, 0xA6, &[0, MINE]), [1]);
    assert_eq!(run(&mut sim, 0xA6, &[ME_I, MINE]), [3]);
    for p in [8, 9, -1, 31, i32::MAX] {
        assert_eq!(run(&mut sim, 0xA6, &[p, W]), [0], "GUID, slot {p}");
        assert_eq!(run(&mut sim, 0xA6, &[p, MINE]), [0], "filter, slot {p}");
    }
    // Wrong argc (4 instead of 3): the field stays, `[2]` = player.
    assert_eq!(ask(&mut sim, 0x4E47, &[ME_I, ME_I, W, 0]), ME_I);
    assert_eq!(ask(&mut sim, 0x4E3F, &[ME_I, W, 0]), ME_I);
}

#[test]
fn pending_orders_add_to_the_count_only_for_guids() {
    // 0xAF/0xB0 -> 0x4EFD: GUID -> object count + construction orders, filter ID -> like 0x4E47; 0x139 -> 0x4F6A.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for tile in [(10, 10), (12, 10)] {
        put(&mut sim, WARRIOR, ME_I, tile);
    }
    add_pending_guid_count(sim.state, ME_I, WARRIOR, 3);
    add_pending_guid_count(sim.state, 0, WARRIOR, 1);
    assert_eq!(run(&mut sim, 0xA5, &[W]), [2], "without orders");
    assert_eq!(run(&mut sim, 0xAF, &[W]), [5]);
    assert_eq!(run(&mut sim, 0xB0, &[ME_I, W]), [5]);
    assert_eq!(run(&mut sim, 0xB0, &[0, W]), [1]);
    assert_eq!(
        run(&mut sim, 0xAF, &[MINE]),
        [2],
        "orders have no effect on filters"
    );
    assert_eq!(run(&mut sim, 0xB0, &[8, W]), [0]);
    assert_eq!(run(&mut sim, 0xAF, &[999]), [0]);
    // 0x4F6A: the number of orders, not 0/1.
    assert_eq!(run(&mut sim, 0x139, &[W]), [3]);
    assert_eq!(ask(&mut sim, 0x4F6A, &[0, W]), 1);
    assert_eq!(ask(&mut sim, 0x4F6A, &[8, W]), 0);
    assert_eq!(ask(&mut sim, 0x4F6A, &[ME_I, W, 0]), ME_I, "wrong argc");
    // A deduction below 0 stays a negative number.
    add_pending_guid_count(sim.state, ME_I, WARRIOR, -10);
    assert_eq!(run(&mut sim, 0x139, &[W]), [-7]);
    assert_eq!(run(&mut sim, 0xAF, &[W]), [-5]);
}

#[test]
fn unit_count_skips_quartered_guid_count_does_not() {
    // E-66: 0x4E49 (0xA9/0xAA, module 0) and 0x4E48 (0xA7/0xA8, module 3) count via the filter:
    // without dead and quartered ones. 0x4E47 with a GUID counts quartered ones too (object counter map).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    put(&mut sim, WARRIOR, ME_I, (10, 10));
    let w2 = put(&mut sim, WARRIOR, ME_I, (12, 10));
    let w3 = put(&mut sim, WARRIOR, ME_I, (16, 10));
    put(&mut sim, COLONIST, ME_I, (14, 10));
    put(&mut sim, WARRIOR, 0, (18, 10));
    sim.object_mut(oid(w3)).unwrap().status |= status::QUARTERED;
    assert_eq!(run(&mut sim, 0xA5, &[W]), [3], "GUID: with quartered ones");
    assert_eq!(
        run(&mut sim, 0xA5, &[MINE]),
        [3],
        "filter: without quartered ones"
    );
    assert_eq!(run(&mut sim, 0xA9, &[]), [3]);
    assert_eq!(run(&mut sim, 0xAA, &[ME_I]), [3]);
    assert_eq!(run(&mut sim, 0xAA, &[0]), [1]);
    // Dead ones do not count; a dead one still holds the GUID counter (map).
    sim.object_mut(oid(w2)).unwrap().status |= status::DIED;
    assert_eq!(run(&mut sim, 0xAA, &[ME_I]), [2]);
    assert_eq!(run(&mut sim, 0xA5, &[W]), [3]);
    // Buildings: finished ones and construction sites carry module 3; units do not.
    assert_eq!(run(&mut sim, 0xA7, &[]), [0]);
    put(&mut sim, HQ, ME_I, (30, 30));
    site(&mut sim, HOUSE, ME_I, (40, 40));
    put(&mut sim, HQ, 0, (50, 50));
    assert_eq!(run(&mut sim, 0xA7, &[]), [2]);
    assert_eq!(run(&mut sim, 0xA8, &[0]), [1]);
    assert_eq!(run(&mut sim, 0xA8, &[ME_I]), [2]);
    assert_eq!(run(&mut sim, 0xAA, &[ME_I]), [2], "buildings are not units");
    for p in [8, 10, -1, 31, i32::MAX] {
        assert_eq!(run(&mut sim, 0xA8, &[p]), [0], "slot {p}");
        assert_eq!(run(&mut sim, 0xAA, &[p]), [0], "slot {p}");
    }
    // argc 1 is only a label, `[3]` is read (re/ki-queries 4 no. 14); other argc: field stays.
    for (code, want) in [(0x4E48, 1), (0x4E49, 1)] {
        assert_eq!(
            field(&mut sim, Sight::Omniscient, code, 1, &[9, 0])[2],
            want
        );
        assert_eq!(field(&mut sim, Sight::Omniscient, code, 2, &[9, 0])[2], 9);
        assert_eq!(field(&mut sim, Sight::Omniscient, code, 0, &[9, 0])[2], 9);
    }
}

/// Warriors of the slots `ME`, 0, 2 and a colonist of `ME` around the tile (11, 10) and a warrior far outside.
fn rect_scene(sim: &mut Sim) {
    put(sim, WARRIOR, ME_I, (10, 10));
    put(sim, WARRIOR, 0, (11, 10));
    put(sim, WARRIOR, 2, (12, 10));
    put(sim, COLONIST, ME_I, (11, 11));
    put(sim, WARRIOR, ME_I, (30, 30));
}

#[test]
fn rect_counts_ignore_the_owner_without_list_mode() {
    // E-65: 0x4E4B counts objects of all owners in the inclusive rectangle; only the list mode of the filter restricts
    // to the reference player `[3]`. 0xAD `(type, x, z, r)`, 0xAE `(p, type, x, z, r)`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    rect_scene(&mut sim);
    // Center (11, 10), r = 1 -> rectangle (10, 9)-(12, 11), edges inclusive.
    assert_eq!(run(&mut sim, 0xAD, &[ANY_WARRIOR, 11, 10, 1]), [3]);
    assert_eq!(run(&mut sim, 0xAD, &[MY_WARRIOR, 11, 10, 1]), [1]);
    assert_eq!(
        run(&mut sim, 0xAD, &[W, 11, 10, 1]),
        [1],
        "GUID filter: list of the reference"
    );
    assert_eq!(run(&mut sim, 0xAD, &[COLONIST as i32, 11, 10, 1]), [1]);
    assert_eq!(
        run(&mut sim, 0xAD, &[MINE, 11, 10, 1]),
        [2],
        "colonist and warrior of `me`"
    );
    // r = 0: only the center; edge inclusive at r = 1 (x = 10 and x = 12 count).
    assert_eq!(run(&mut sim, 0xAD, &[ANY_WARRIOR, 11, 10, 0]), [1]);
    assert_eq!(run(&mut sim, 0xAD, &[ANY_WARRIOR, 10, 10, 0]), [1]);
    assert_eq!(
        run(&mut sim, 0xAD, &[ANY_WARRIOR, 11, 10, 20]),
        [4],
        "all four warriors"
    );
    // Negative half-width: empty rectangle.
    for r in [-1, -100, i32::MIN] {
        assert_eq!(
            run(&mut sim, 0xAD, &[ANY_WARRIOR, 11, 10, r]),
            [0],
            "r = {r}"
        );
    }
    // The reference player: 0xAE `(p, type, x, z, r)` with the list of `p`.
    assert_eq!(run(&mut sim, 0xAE, &[0, MY_WARRIOR, 11, 10, 1]), [1]);
    assert_eq!(run(&mut sim, 0xAE, &[2, MY_WARRIOR, 11, 10, 1]), [1]);
    assert_eq!(run(&mut sim, 0xAE, &[5, MY_WARRIOR, 11, 10, 1]), [0]);
    assert_eq!(
        run(&mut sim, 0xAE, &[5, ANY_WARRIOR, 11, 10, 1]),
        [3],
        "without list: owner does not matter"
    );
    // Unknown filter ID: 0 (the filter is missing).
    for id in [0, 999, -3] {
        assert_eq!(run(&mut sim, 0xAD, &[id, 11, 10, 1]), [0], "ID {id}");
    }
    // Wrong argc (7 instead of 6) -> field stays.
    assert_eq!(
        ask(&mut sim, 0x4E4B, &[ME_I, ME_I, ANY_WARRIOR, 11, 10, 1, 0]),
        ME_I
    );
}

#[test]
fn explicit_rect_counts_use_the_corners_directly() {
    // 0xB7 `(type, x1, z1, x2, z2)` / 0xB8 `(p, type, …)` -> 0x4F8D: corners directly, swapped corners empty; corner
    // `i32::MAX` gives an empty rectangle (`PtInRect` with a wrapping `+ 1`).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    rect_scene(&mut sim);
    assert_eq!(run(&mut sim, 0xB7, &[ANY_WARRIOR, 10, 10, 12, 10]), [3]);
    assert_eq!(run(&mut sim, 0xB7, &[ANY_WARRIOR, 11, 10, 12, 10]), [2]);
    assert_eq!(run(&mut sim, 0xB7, &[MY_WARRIOR, 10, 10, 12, 10]), [1]);
    assert_eq!(run(&mut sim, 0xB7, &[ANY_WARRIOR, 0, 0, 63, 63]), [4]);
    assert_eq!(
        run(&mut sim, 0xB7, &[ANY_WARRIOR, 12, 10, 10, 10]),
        [0],
        "swapped"
    );
    assert_eq!(
        run(&mut sim, 0xB7, &[ANY_WARRIOR, 10, 12, 12, 10]),
        [0],
        "z swapped"
    );
    for rect in [
        [0, 0, i32::MAX, i32::MAX],
        [0, 0, i32::MAX, 63],
        [0, 0, 63, i32::MAX],
        [i32::MIN, i32::MIN, i32::MAX, i32::MAX],
    ] {
        let args = [&[ANY_WARRIOR][..], &rect].concat();
        assert_eq!(run(&mut sim, 0xB7, &args), [0], "{rect:?}");
    }
    assert_eq!(
        run(&mut sim, 0xB7, &[ANY_WARRIOR, i32::MIN, i32::MIN, 63, 63]),
        [4]
    );
    assert_eq!(run(&mut sim, 0xB8, &[0, MY_WARRIOR, 10, 10, 12, 10]), [1]);
    assert_eq!(run(&mut sim, 0xB8, &[7, MY_WARRIOR, 10, 10, 12, 10]), [0]);
    assert_eq!(run(&mut sim, 0xB8, &[7, ANY_WARRIOR, 10, 10, 12, 10]), [3]);
    assert_eq!(
        ask(
            &mut sim,
            0x4F8D,
            &[ME_I, ME_I, ANY_WARRIOR, 0, 0, 63, 63, 0]
        ),
        ME_I
    );
}

#[test]
fn sum_over_players_counts_an_object_once_per_reference_player() {
    // E-64, E-30: 0xB1 `(filter, x, z, r)` -> 0x4F56 sums over the reference players 0-6 (slot 7 is missing): without
    // list mode an object counts 7 times, with `MM_HL_MYLIST` once (slots 0-6).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    rect_scene(&mut sim);
    // Center (11, 10), r = 1: three warriors (ME, 0, 2) and the colonist.
    assert_eq!(run(&mut sim, 0xB1, &[ANY_WARRIOR, 11, 10, 1]), [21]);
    assert_eq!(run(&mut sim, 0xB1, &[MY_WARRIOR, 11, 10, 1]), [3]);
    assert_eq!(
        run(&mut sim, 0xB1, &[MINE, 11, 10, 1]),
        [4],
        "list of the reference: every object of the slots 0-6 once"
    );
    // Slot 7 and the neutrals (8) in the rectangle: without list mode 7 times, with a list not at all.
    put(&mut sim, WARRIOR, 7, (11, 9));
    put(&mut sim, WARRIOR, 8, (10, 11));
    assert_eq!(run(&mut sim, 0xB1, &[ANY_WARRIOR, 11, 10, 1]), [35]);
    assert_eq!(
        run(&mut sim, 0xB1, &[MY_WARRIOR, 11, 10, 1]),
        [3],
        "slot 7 is missing, 8 without a list"
    );
    // GUID filter (list of the reference): once per object of the slots 0-6.
    assert_eq!(run(&mut sim, 0xB1, &[W, 11, 10, 1]), [3]);
    // Unknown filter ID, negative half-width: 0.
    assert_eq!(run(&mut sim, 0xB1, &[999, 11, 10, 1]), [0]);
    assert_eq!(run(&mut sim, 0xB1, &[ANY_WARRIOR, 11, 10, -1]), [0]);
    assert_eq!(
        ask(&mut sim, 0x4F56, &[ANY_WARRIOR, 11, 10, 1, 0, 0]),
        ANY_WARRIOR,
        "wrong argc"
    );
    // The sum wraps: no overflow in the debug build.
    for r in [i32::MAX, i32::MIN] {
        let _ = run(&mut sim, 0xB1, &[ANY_WARRIOR, 11, 10, r]);
        let _ = run(&mut sim, 0xB1, &[ANY_WARRIOR, i32::MAX, i32::MAX, r]);
    }
}

#[test]
fn code_0x4f92_sums_over_the_reference_players_with_explicit_corners() {
    // `FUN_00430FF0` gets the corners `{[4], [5], [6], [7]}` (like 0xB1 with an explicit rectangle) and checks argc 5
    // but reads `[7]`, one word behind the arguments. The handler of the builtin `0xB9` sends argc 6 and stays without
    // effect (E-37): it pushes the own player.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    rect_scene(&mut sim);
    let corners = |sim: &mut Sim, id, rect: [i32; 4]| {
        field(
            sim,
            Sight::Omniscient,
            0x4F92,
            5,
            &[ME_I, id, rect[0], rect[1], rect[2], rect[3]],
        )[2]
    };
    // Corners (10, 9)-(12, 11): three warriors, without list mode 7 times each; with a list once.
    assert_eq!(corners(&mut sim, ANY_WARRIOR, [10, 9, 12, 11]), 21);
    assert_eq!(corners(&mut sim, MY_WARRIOR, [10, 9, 12, 11]), 3);
    // Corners, not center and half-width: (11, 10, 1) would be a different set.
    assert_eq!(corners(&mut sim, ANY_WARRIOR, [11, 10, 12, 10]), 14);
    assert_eq!(
        corners(&mut sim, ANY_WARRIOR, [12, 10, 10, 10]),
        0,
        "swapped"
    );
    assert_eq!(
        corners(&mut sim, ANY_WARRIOR, [0, 0, i32::MAX, i32::MAX]),
        0
    );
    assert_eq!(corners(&mut sim, 999, [0, 0, 63, 63]), 0, "filter missing");
    // Via `ki_query` with five words `[7]` is the padding 0: z0 = 9 > 0 -> empty.
    let words = [ME_I, ANY_WARRIOR, 10, 9, 12];
    assert_eq!(
        query_once(&mut sim, ME, Sight::Omniscient, 0x4F92, &words).0[2],
        0
    );
    assert_eq!(
        ask(&mut sim, 0x4F92, &[ME_I, ANY_WARRIOR, 10, 9, 12, 11, 0]),
        ME_I,
        "argc 7"
    );
    assert_eq!(
        ask(&mut sim, 0x4F92, &[ME_I, ANY_WARRIOR, 10, 9, 12, 11]),
        ME_I,
        "argc 6"
    );
    assert_eq!(run(&mut sim, 0xB9, &[ANY_WARRIOR, 10, 9, 12, 11]), [ME_I]);
}

#[test]
fn counter_upgrade_queries_match_the_counter_list() {
    // 0xCF -> 0x510E (without argc check): running counter-upgrades with source on slot `[3]`;
    // 0x230 -> 0x5032: the counter-upgrade with GUID `[3]` runs somewhere.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let own = put(&mut sim, HQ, ME_I, (30, 30));
    let other = put(&mut sim, HQ, 0, (40, 40));
    assert_eq!(run(&mut sim, 0xCF, &[ME_I]), [0]);
    assert_eq!(run(&mut sim, 0x230, &[3370]), [0]);
    sim.state.counter_upgrades.push(CounterUpgrade {
        guid: 3370,
        rest_ms: 60_000,
        source: oid(own),
    });
    assert_eq!(run(&mut sim, 0xCF, &[ME_I]), [1]);
    assert_eq!(run(&mut sim, 0xCF, &[0]), [0]);
    assert_eq!(run(&mut sim, 0x230, &[3370]), [1]);
    assert_eq!(run(&mut sim, 0x230, &[3371]), [0]);
    sim.state.counter_upgrades.push(CounterUpgrade {
        guid: 3372,
        rest_ms: 1000,
        source: oid(other),
    });
    sim.state.counter_upgrades.push(CounterUpgrade {
        guid: 3373,
        rest_ms: 1000,
        source: oid(own),
    });
    assert_eq!(
        run(&mut sim, 0xCF, &[ME_I]),
        [1],
        "count > 0, not the count"
    );
    assert_eq!(run(&mut sim, 0xCF, &[0]), [1]);
    assert_eq!(run(&mut sim, 0x230, &[3372]), [1], "any player");
    for p in [2, 7, 8, -1, 31, 32, 255, 256, 1000, i32::MIN] {
        assert_eq!(run(&mut sim, 0xCF, &[p]), [0], "slot {p}");
    }
    // No argc check: even with argc 0 the query reads `[3]` (here 0).
    assert_eq!(field(&mut sim, Sight::Omniscient, 0x510E, 0, &[9, 0])[2], 1);
    assert_eq!(
        field(&mut sim, Sight::Omniscient, 0x510E, 7, &[9, ME_I])[2],
        1
    );
    assert_eq!(field(&mut sim, Sight::Omniscient, 0x510E, 2, &[9, 2])[2], 0);
    // 0x5032 checks argc 2.
    assert_eq!(
        field(&mut sim, Sight::Omniscient, 0x5032, 3, &[9, 3370])[2],
        9
    );
    assert_eq!(
        field(&mut sim, Sight::Omniscient, 0x5032, 2, &[9, 3370])[2],
        1
    );
    // Counter-upgrades are public (also under fog); fog hides foreign slots only for 0x510E.
    assert_eq!(field(&mut sim, Sight::Fog, 0x5032, 2, &[9, 3372])[2], 1);
    assert_eq!(field(&mut sim, Sight::Fog, 0x510E, 2, &[9, 0])[2], 0);
    assert_eq!(field(&mut sim, Sight::Fog, 0x510E, 2, &[9, ME_I])[2], 1);
}

#[test]
fn build_and_make_check_follows_the_original_checks() {
    // 0x132 -> 0x4E8A (`Player_CanBuildOrMakeGuid`, 5.12.4): cost, lock, toolbar entry, one-time types,
    // `REQUIRED_GUID`; silent (E-67).
    // House costs 150 wood, headquarters 500 (config 11 of the test rules).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let (house, hq, mine) = (HOUSE as i32, HQ as i32, 1004);
    sim.state.players[usize::from(ME)].set_resource(1, 100);
    assert_eq!(run(&mut sim, 0x132, &[house]), [0], "too expensive");
    sim.state.players[usize::from(ME)].set_resource(1, 200);
    assert_eq!(run(&mut sim, 0x132, &[house]), [1]);
    // Slot 0 has only gold and food (`set_resource`), no wood.
    sim.state.players[0].set_resource(1, 0);
    assert_eq!(ask(&mut sim, 0x4E8A, &[0, house]), 0, "slot 0 has no wood");
    assert_eq!(ask(&mut sim, 0x4E8A, &[ME_I, house]), 1);
    assert!(sim.state.events.is_empty(), "silent");
    // No toolbar entry, negative and huge GUID.
    for guid in [0, 9999, -1, i32::MIN] {
        assert_eq!(run(&mut sim, 0x132, &[guid]), [0], "GUID {guid}");
    }
    // Prerequisite: the gold mine (250 wood) needs the HQ.
    sim.state.players[usize::from(ME)].set_resource(1, 600);
    assert_eq!(run(&mut sim, 0x132, &[mine]), [0]);
    assert_eq!(
        run(&mut sim, 0x132, &[hq]),
        [1],
        "HQ itself without object count"
    );
    put(&mut sim, HQ, ME_I, (30, 30));
    assert_eq!(run(&mut sim, 0x132, &[mine]), [1]);
    assert_eq!(
        run(&mut sim, 0x132, &[hq]),
        [0],
        "one-time type with object count"
    );
    // Invalid slots: `Player_CanAffordObject` is true without a player (economy.rs), the rest of the check
    // continues; without a toolbar entry it stays 0.
    for p in [8, -1, 31, 1000, i32::MIN] {
        assert_eq!(
            ask(&mut sim, 0x4E8A, &[p, 9999]),
            0,
            "slot {p}, without button"
        );
        // Ghidra: for p >= 8 the original returns 1 if a button exists, the value set does not lock the GUID and no
        // `REQUIRED_GUID` is set (house).
        assert_eq!(ask(&mut sim, 0x4E8A, &[p, house]), 1, "slot {p}, house");
    }
    assert_eq!(ask(&mut sim, 0x4E8A, &[ME_I, house, 0]), ME_I, "wrong argc");
    // Under fog the account of a foreign slot is hidden.
    assert_eq!(field(&mut sim, Sight::Fog, 0x4E8A, 2, &[ME_I, house])[2], 1);
    assert_eq!(field(&mut sim, Sight::Fog, 0x4E8A, 2, &[0, house])[2], 0);
}

#[test]
fn research_check_0x4f33_has_a_fixed_rule_for_the_headquarters() {
    // 0x136 -> 0x4F33: main houses (1000, 1300, 1600, 1900, 2200, 2500) with opponent upgrades 3350...3357 always 1;
    // otherwise the `toolbar.ini` menu of `[3]` has a button with `BUTTONMSG == [4]`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for hq in [1000, 1300, 1600, 1900, 2200, 2500] {
        for up in [3350, 3354, 3357] {
            assert_eq!(run(&mut sim, 0x136, &[hq, up]), [1], "{hq}/{up}");
        }
        assert_eq!(run(&mut sim, 0x136, &[hq, 3349]), [0], "{hq}/3349");
        assert_eq!(run(&mut sim, 0x136, &[hq, 3358]), [0], "{hq}/3358");
    }
    // Menu of HQ 1000 (toolbar of the test world): colonist 1100 and research 2806.
    assert_eq!(run(&mut sim, 0x136, &[1000, 1100]), [1]);
    assert_eq!(run(&mut sim, 0x136, &[1000, 2806]), [1]);
    assert_eq!(run(&mut sim, 0x136, &[1000, 1001]), [0]);
    // Build menu 110: house yes, but no opponent upgrade (no fixed rule for non-HQ).
    assert_eq!(run(&mut sim, 0x136, &[110, 1001]), [1]);
    assert_eq!(run(&mut sim, 0x136, &[110, 3350]), [0]);
    // No menu: 0. `[2]` is overwritten with 0 (input 5).
    for (a, b) in [(999, 1001), (0, 0), (-1, -1), (i32::MIN, i32::MAX)] {
        assert_eq!(run(&mut sim, 0x136, &[a, b]), [0], "{a}/{b}");
    }
    assert_eq!(ask(&mut sim, 0x4F33, &[5, 110, 9999]), 0);
    assert_eq!(
        ask(&mut sim, 0x4F33, &[5, 110]),
        5,
        "wrong argc: field stays"
    );
}

#[test]
fn count_rows_match_the_signatures() {
    // All builtins of this group are queries with the code, the pops and the push of the signature.
    let table: [(u16, u32); 20] = [
        (0x94, 0x4E3F),
        (0x95, 0x4E3F),
        (0xA5, 0x4E47),
        (0xA6, 0x4E47),
        (0xA7, 0x4E48),
        (0xA8, 0x4E48),
        (0xA9, 0x4E49),
        (0xAA, 0x4E49),
        (0xAD, 0x4E4B),
        (0xAE, 0x4E4B),
        (0xAF, 0x4EFD),
        (0xB0, 0x4EFD),
        (0xB1, 0x4F56),
        (0xB7, 0x4F8D),
        (0xB8, 0x4F8D),
        (0xCF, 0x510E),
        (0x132, 0x4E8A),
        (0x136, 0x4F33),
        (0x139, 0x4F6A),
        (0x230, 0x5032),
    ];
    let pops = [1, 2, 1, 2, 0, 1, 0, 1, 4, 5, 1, 2, 4, 5, 6, 1, 1, 2, 1, 1];
    for ((id, code), pops) in table.into_iter().zip(pops) {
        assert!(is_query(id), "{id:#x}");
        let r = row(id).expect("row");
        assert_eq!(
            (r.code, r.pops, r.push, r.pause_ms),
            (code, pops, true, 0),
            "{id:#x}"
        );
        assert!(r.outs.is_empty(), "{id:#x}");
        if let Some(sig) = signature(id) {
            assert_eq!(
                (sig.pops, sig.push, sig.pause_ms, sig.code, sig.outs.len()),
                (r.pops, r.push, r.pause_ms, r.code, 0),
                "{id:#x}"
            );
        }
        // The handler pops exactly `pops` arguments and pushes exactly one result.
        let (rules, mut state) = scene();
        let mut sim = Sim::new(&mut state, &rules);
        let args = vec![0; usize::from(pops)];
        assert_eq!(run(&mut sim, id, &args).len(), 1, "{id:#x}");
    }
    // Every row with a count code belongs in the table.
    for r in ROWS {
        if matches!(
            r.code,
            0x4E3F
                | 0x4E47
                | 0x4E48
                | 0x4E49
                | 0x4E4B
                | 0x4EFD
                | 0x4F56
                | 0x4F8D
                | 0x510E
                | 0x4E8A
                | 0x4F33
                | 0x4F6A
                | 0x5032
        ) {
            assert!(table.iter().any(|&(id, _)| id == r.lo), "{:#x}", r.lo);
        }
    }
}

#[test]
fn fog_counts_see_only_what_the_player_sees() {
    // Under fog rectangles count only seen objects; object counts and orders of foreign slots are hidden (0).
    let rules = count_rules();
    let mut state = fogged_state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        rect_scene(&mut sim);
        add_pending_guid_count(sim.state, 0, WARRIOR, 4);
    }
    state.fog.seen.clear();
    state.fog.cleared.clear();
    // Slot [ME] sees only the tile (11, 10) (warrior of slot 0 there).
    state.fog.seen.or(11, 10, 1 << ME);
    let mut sim = Sim::new(&mut state, &rules);
    let sum =
        |sim: &mut Sim, sight| field(sim, sight, 0x4F56, 5, &[ME_I, ANY_WARRIOR, 11, 10, 1])[2];
    assert_eq!(sum(&mut sim, Sight::Omniscient), 21);
    // Seen: the foreign warrior at (11, 10) and the own one at (10, 10): 2 objects x 7.
    assert_eq!(sum(&mut sim, Sight::Fog), 14);
    let rect = |sim: &mut Sim, sight| {
        field(sim, sight, 0x4E4B, 6, &[ME_I, ME_I, ANY_WARRIOR, 11, 10, 1])[2]
    };
    assert_eq!(rect(&mut sim, Sight::Omniscient), 3);
    assert_eq!(rect(&mut sim, Sight::Fog), 2);
    // Foreign object counts and orders: 0; own ones visible. 0x4F6A asks `[2]` = slot, `[3]` = GUID.
    let owned =
        |sim: &mut Sim, code, argc, words: &[i32], sight| field(sim, sight, code, argc, words)[2];
    for (sight, want) in [(Sight::Omniscient, [1, 5, 4]), (Sight::Fog, [0, 0, 0])] {
        assert_eq!(
            owned(&mut sim, 0x4E47, 3, &[ME_I, 0, W], sight),
            want[0],
            "{sight:?}"
        );
        assert_eq!(
            owned(&mut sim, 0x4EFD, 3, &[ME_I, 0, W], sight),
            want[1],
            "{sight:?}"
        );
        assert_eq!(
            owned(&mut sim, 0x4F6A, 2, &[0, W], sight),
            want[2],
            "{sight:?}"
        );
    }
    assert_eq!(owned(&mut sim, 0x4E47, 3, &[ME_I, ME_I, W], Sight::Fog), 2);
}

/// Query codes of this group.
const CODES: [u32; 14] = [
    0x4E3F, 0x4E47, 0x4E48, 0x4E49, 0x4E4B, 0x4E8A, 0x4EFD, 0x4F33, 0x4F56, 0x4F6A, 0x4F8D, 0x4F92,
    0x5032, 0x510E,
];

#[test]
fn count_queries_leave_the_checksum_unchanged() {
    let (rules, mut state) = scene();
    {
        let mut sim = Sim::new(&mut state, &rules);
        rect_scene(&mut sim);
        let q = put(&mut sim, WARRIOR, ME_I, (13, 10));
        sim.object_mut(oid(q)).unwrap().status |= status::QUARTERED;
        let dead = put(&mut sim, WARRIOR, 0, (13, 11));
        sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
        let hq = put(&mut sim, HQ, ME_I, (30, 30));
        sim.state.counter_upgrades.push(CounterUpgrade {
            guid: 3370,
            rest_ms: 5000,
            source: oid(hq),
        });
        add_pending_guid_count(sim.state, ME_I, WARRIOR, 2);
    }
    let (before, events) = (checksum(&state), state.events.clone());
    let mut sim = Sim::new(&mut state, &rules);
    let values = [
        0,
        1,
        2,
        7,
        8,
        9,
        10,
        11,
        31,
        63,
        1000,
        W,
        MINE,
        ANY_WARRIOR,
        MY_WARRIOR,
        3370,
        3350,
        -1,
        -5,
        i32::MIN,
        i32::MAX,
    ];
    for code in CODES {
        for &a in &values {
            for &b in &values {
                let words = [ME_I, a, b, 10, b, a, b, a, 3];
                for len in [9, 8, 7, 6, 5, 3, 2, 1, 0] {
                    let _ = query_once(&mut sim, ME, Sight::Omniscient, code, &words[..len]);
                    let _ = query_once(&mut sim, ME, Sight::Fog, code, &words[..len]);
                }
                let _ = query_once(&mut sim, 0, Sight::Fog, code, &[a, b, a, b, a, b, a]);
            }
        }
    }
    // All builtins of this group with hostile arguments.
    for id in [
        0x94, 0x95, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAD, 0xAE, 0xAF, 0xB0, 0xB1, 0xB7, 0xB8,
        0xB9, 0xCF, 0x132, 0x136, 0x139, 0x230,
    ] {
        for &v in &values {
            let _ = run_with(&mut sim, id, &[v, v, v, v, v, v]);
            let _ = run_with(&mut sim, id, &[]);
        }
    }
    assert_eq!(checksum(&state), before);
    assert_eq!(state.events, events);
}
