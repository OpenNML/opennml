//! Tests of the nearest searches, the AI mark and the production check (re/ki-queries 5.3-5.6, 5.9, 5.10; E-29, E-49
//! ... E-54, E-58).

use nml_formats::targets::parse_targets;

use super::handlers::{ROWS, row};
use super::tests_player::{FLOOR, ME, ask, run, run_with};
use super::{Direct, SimKiWorld, is_query, query_once};
use crate::combat::testkit::{ARCHER, HQ, WARRIOR, defines, rules as combat_rules, spawn, state};
use crate::command::testkit::{BASKET, COLONIST, HOUSE, TREE, fogged_state, site};
use crate::id::ObjectId;
use crate::ki::signature::signature;
use crate::ki::testkit::{int, list, proto};
use crate::ki::vm::Thread;
use crate::ki::world::{BuiltinCx, KiNote, KiWorld};
use crate::object::status;
use crate::player::Sight;
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;

pub(super) const ME_I: i32 = ME as i32;
/// Filter ID 4: "Enemies" (`MM_HL_ENEMYLIST`).
const ENEMY: i32 = 4;
/// Filter ID 5: "own" (`MM_HL_MYLIST`, `MM_ANDALL`).
pub(super) const MINE: i32 = 5;
/// Filter ID 182: "Corpse" (`MM_RETURN_DIED`, `MM_HL_MYLIST`).
pub(super) const CORPSE: i32 = 182;
/// Filter ID 183: "More than 70 % Health" (`MM2_MORETHAN70`, the only `MM_MODE2` filter).
const HEALTHY: i32 = 183;
const W: i32 = WARRIOR as i32;

pub(super) const TARGETS: &str = concat!(
    "BEGIN\nBUTTONMSG 4\nMY_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 5\nMY_MM\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_ANDALL\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 182\nMY_MM\nMM_MODE MM_RETURN_DIED\nMM_MODE MM_HL_MYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 183\nMY_MM\nMM_MODE2 MM2_MORETHAN70\nMM_END\nTARGET_MM\nMM_END\nEND\n",
);

/// Combat rules with the filter IDs 4, 5, 182, 183 (`flowcontroltragets.ini`).
pub(super) fn search_rules() -> Rules {
    let mut rules = combat_rules();
    rules.flowcontrol_targets = parse_targets(TARGETS, &defines()).entries;
    rules
}

/// [`search_rules`] and a world uncovered for everyone.
pub(super) fn scene() -> (Rules, SimState) {
    let rules = search_rules();
    let state = state(&rules);
    (rules, state)
}

pub(super) fn oid(id: i32) -> ObjectId {
    ObjectId::from_i32(id)
}

/// Object `guid` of slot `slot` on the tile center of `tile`, as a raw ID.
pub(super) fn put(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> i32 {
    spawn(sim, guid, slot, tile).as_i32()
}

fn marks(sim: &Sim, id: i32) -> u8 {
    sim.state.object(oid(id), false).expect("object").ai_marks
}

// QUIRK E-35: kept
#[test]
fn marking_search_marks_the_hit_and_skips_it_next_time() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let near = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let far = put(&mut sim, WARRIOR, ME_I, (14, 10));
    let _colonist = put(&mut sim, COLONIST, ME_I, (11, 10));
    // The mark of another slot does not interfere (bit `[2] & 31`).
    sim.state.object_mut(oid(near), false).unwrap().ai_marks = 1 << 3;
    // 0x55: `(x, z, f1, f2)`, head ID 0 (all objects of `me`), following filter GUID warrior.
    let (t, notes) = run_with(&mut sim, 0x55, &[FLOOR, 10, 10, 0, W]);
    assert!(notes.is_empty());
    assert_eq!(t.stack, [FLOOR, near]);
    assert_eq!(t.pause_ms, 500);
    assert_eq!(marks(&sim, near), 1 << 3 | 1 << ME);
    assert_eq!(run(&mut sim, 0x55, &[10, 10, 0, W]), [far]);
    assert_eq!(marks(&sim, far), 1 << ME);
    assert_eq!(run(&mut sim, 0x55, &[10, 10, 0, W]), [0]);
    // Clearing the mark (0x9E) frees the hit again.
    assert_eq!(run(&mut sim, 0x9E, &[near]), []);
    assert_eq!(marks(&sim, near), 1 << 3);
    assert_eq!(run(&mut sim, 0x58, &[ME_I, 10, 10, 0, W]), [near]);
}

/// Two warriors of [`ME`]: `a` and `b` in this entry order.
fn two_warriors(a: (i32, i32), b: (i32, i32), a_first: bool) -> (Rules, SimState, i32, i32) {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let (ia, ib) = if a_first {
        let ia = put(&mut sim, WARRIOR, ME_I, a);
        (ia, put(&mut sim, WARRIOR, ME_I, b))
    } else {
        let ib = put(&mut sim, WARRIOR, ME_I, b);
        (put(&mut sim, WARRIOR, ME_I, a), ib)
    };
    (rules, state, ia, ib)
}

// QUIRK E-54: kept
#[test]
fn searches_start_at_the_tile_corner() {
    // E-54: center `(x << 12, z << 12)`. From the tile center (10.5, 10.5), (9, 10) and (11, 10) are equally far (1
    // tile), from the corner (10, 10) only (9, 10) is near (0.71 against 1.58).
    for a_first in [true, false] {
        let (rules, mut state, a, _) = two_warriors((9, 10), (11, 10), a_first);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, ME_I, W, 10, 10, 0]), a);
        assert_eq!(ask(&mut sim, 0x4F24, &[ME_I, ME_I, 10, 10, 0, W]), a);
        assert_eq!(ask(&mut sim, 0x50CD, &[ME_I, ME_I, 0, 10, 10, W, MINE]), a);
    }
    // From the center (11, 10) would be first (1.0 against 1.41), from the corner (9, 9) (0.71 against 1.58).
    let (rules, mut state, a, _) = two_warriors((9, 9), (11, 10), false);
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(ask(&mut sim, 0x4E25, &[ME_I, ME_I, W, 10, 10, 0]), a);
    assert_eq!(ask(&mut sim, 0x4E24, &[ME_I, ME_I, 10, 10, 0, W]), a);
}

// QUIRK E-53: kept
#[test]
fn fish_basket_ends_the_search_with_zero() {
    // E-53: a hit with GUID 3505 returns 0, without searching further and without a mark. The basket belongs to slot 10
    // (nature); reference player -1 admits any owner.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let basket = put(&mut sim, BASKET, ME_I, (10, 10));
    assert_eq!(oid(basket).slot(), 10);
    let warrior = put(&mut sim, WARRIOR, ME_I, (12, 10));
    assert_eq!(run(&mut sim, 0x58, &[-1, 10, 10, 0, MINE]), [0]);
    assert_eq!((marks(&sim, basket), marks(&sim, warrior)), (0, 0));
    assert_eq!(ask(&mut sim, 0x4F24, &[ME_I, -1, 10, 10, 0, MINE]), 0);
    assert_eq!(ask(&mut sim, 0x50C3, &[ME_I, -1, 10, 10, 0, MINE, MINE]), 0);
    // With the center nearer to the warrior, the warrior wins.
    assert_eq!(run(&mut sim, 0x58, &[-1, 13, 10, 0, MINE]), [warrior]);
    assert_eq!(marks(&sim, warrior), 1 << ME);
}

// QUIRK E-49: kept
#[test]
fn heads_or_their_list_bits_into_file_filters() {
    // E-49: at the head, `MM_HL_MYLIST` of the dispatcher is OR-ed with `MM_HL_ENEMYLIST` of the file filter: the enemy
    // filter also finds own objects of `[3]`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let own = put(&mut sim, WARRIOR, ME_I, (11, 10));
    let enemy = put(&mut sim, WARRIOR, 2, (12, 10));
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, ME_I, W, 10, 10, ENEMY]), own);
    assert_eq!(
        ask(&mut sim, 0x4F25, &[ME_I, ME_I, W, 13, 10, ENEMY]),
        enemy
    );
    // Without OR-ing (0x4E28) the enemy filter does not match the own object.
    assert_eq!(ask(&mut sim, 0x4E28, &[ME_I, own, ENEMY]), 0);
    assert_eq!(ask(&mut sim, 0x4E28, &[ME_I, enemy, ENEMY]), 1);
    // As a following filter (not OR-ed) it stays a pure enemy filter.
    assert_eq!(ask(&mut sim, 0x4F24, &[ME_I, ME_I, 10, 10, W, ENEMY]), 0);
}

// QUIRK E-50: kept
#[test]
fn seven_codes_clear_mode2_of_the_head() {
    // E-50: `0x4E25`-`0x4E27`, `0x4F24`-`0x4F28` clear `mode2` at the head; filter 183 (`MM2_MORETHAN70`) has no effect
    // there, at `0x4E24`/`0x50Cx` and as a following filter it does.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let hurt = put(&mut sim, WARRIOR, ME_I, (11, 10));
    let fit = put(&mut sim, WARRIOR, ME_I, (13, 10));
    sim.object_mut(oid(hurt))
        .unwrap()
        .attributes
        .set_current_fraction(14, 50_000);
    for code in [0x4E25, 0x4E27, 0x4F25, 0x4F28] {
        assert_eq!(
            ask(&mut sim, code, &[ME_I, ME_I, W, 10, 10, HEALTHY]),
            hurt,
            "{code:#x}"
        );
    }
    for code in [0x4E26, 0x4F24, 0x4F26] {
        let words = [ME_I, ME_I, 10, 10, HEALTHY, W];
        assert_eq!(ask(&mut sim, code, &words), hurt, "{code:#x}");
    }
    assert_eq!(
        ask(&mut sim, 0x4E24, &[ME_I, ME_I, 10, 10, HEALTHY, W]),
        fit
    );
    assert_eq!(
        ask(&mut sim, 0x50C3, &[ME_I, ME_I, 10, 10, HEALTHY, W, MINE]),
        fit
    );
    for code in [0x50C8, 0x50CD] {
        let words = [ME_I, ME_I, HEALTHY, 10, 10, W, MINE];
        assert_eq!(ask(&mut sim, code, &words), fit, "{code:#x}");
    }
    // As a following filter 183 also works with the seven codes.
    assert_eq!(
        ask(&mut sim, 0x4F24, &[ME_I, ME_I, 10, 10, W, HEALTHY]),
        fit
    );
}

// QUIRK E-29: kept
#[test]
fn radius_of_0x4f24_with_seven_words_is_raw() {
    // E-29: the center (13.5, 10.5) is about 3.54 tiles from the corner (10, 10).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let w = put(&mut sim, WARRIOR, ME_I, (13, 10));
    let chain = |r: i32| [ME_I, ME_I, 10, 10, 0, W, r];
    let guid = |r: i32| [ME_I, ME_I, W, 10, 10, 0, r];
    // Raw: 4 = 4/4096 tile -> nothing; 15,000 (3.66 tiles) is enough.
    assert_eq!(ask(&mut sim, 0x4F24, &chain(4)), 0);
    assert_eq!(ask(&mut sim, 0x4F25, &guid(4)), 0);
    assert_eq!(ask(&mut sim, 0x4F24, &chain(15_000)), w);
    assert_eq!(ask(&mut sim, 0x4F25, &guid(15_000)), w);
    // Scaled (*4096): 4 tiles are enough, 3 are not.
    for code in [0x4E24, 0x4E26, 0x4F26] {
        assert_eq!(ask(&mut sim, code, &chain(4)), w, "{code:#x}");
        assert_eq!(ask(&mut sim, code, &chain(3)), 0, "{code:#x}");
    }
    for code in [0x4E25, 0x4E27, 0x4F28] {
        assert_eq!(ask(&mut sim, code, &guid(4)), w, "{code:#x}");
        assert_eq!(ask(&mut sim, code, &guid(3)), 0, "{code:#x}");
    }
    // Negative, also when it only turns negative through the wrapped multiplication (2^19 * 4096 = i32::MIN):
    // unlimited.
    assert_eq!(ask(&mut sim, 0x4F24, &chain(-1)), w);
    assert_eq!(ask(&mut sim, 0x4F26, &chain(1 << 19)), w);
    assert_eq!(ask(&mut sim, 0x4F26, &chain(1 << 18)), w);
    // Builtins with a radius: 0x216 (`0x4F24`, raw) and 0x222 (`0x4F26`, scaled).
    assert_eq!(run(&mut sim, 0x216, &[10, 10, 0, W, 4]), [0]);
    assert_eq!(run(&mut sim, 0x222, &[10, 10, 0, W, 4]), [w]);
    assert_eq!(run(&mut sim, 0x219, &[ME_I, 10, 10, 0, W, 15_000]), [w]);
}

// QUIRK E-52: kept
#[test]
fn unknown_filter_id_as_head_finds_all_of_the_owner() {
    // E-52 and re/ki-queries 5.2: an unknown head (empty filter) becomes "all objects of `[3]`" through the OR-ed bits,
    // at `0x4E24` additionally "not marked by `[2]`" (`0x80000`).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let colonist = put(&mut sim, COLONIST, ME_I, (11, 10));
    let warrior = put(&mut sim, WARRIOR, ME_I, (13, 10));
    let _enemy = put(&mut sim, ARCHER, 2, (10, 10));
    for head in [0, -5, 999, 1] {
        assert_eq!(
            ask(&mut sim, 0x4E24, &[ME_I, ME_I, 10, 10, head, MINE]),
            colonist,
            "head {head}"
        );
        // Head of `0x4F25`: "GUID `[4]` of `[3]`".
        assert_eq!(
            ask(&mut sim, 0x4F25, &[ME_I, ME_I, W, 10, 10, head]),
            warrior
        );
        // As a following filter it never matches.
        assert_eq!(ask(&mut sim, 0x4E24, &[ME_I, ME_I, 10, 10, MINE, head]), 0);
    }
    // `0x80000` at the empty head: the marked colonist is skipped.
    assert_eq!(run(&mut sim, 0x9B, &[colonist]), []);
    assert_eq!(run(&mut sim, 0x55, &[10, 10, 999, MINE]), [warrior]);
    // `0x4F24` does not carry the bit and still finds it.
    assert_eq!(
        ask(&mut sim, 0x4F24, &[ME_I, ME_I, 10, 10, 999, MINE]),
        colonist
    );
}

// QUIRK E-25: kept
#[test]
fn fog_searches_skip_unseen_objects() {
    // `0x4E26`/`0x4F26` carry `MM_CHECKFOG` for `[2]` (original); under `Sight::Fog` every search sees only what has
    // been seen (checked per candidate).
    let rules = search_rules();
    let mut state = fogged_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hidden = put(&mut sim, WARRIOR, 2, (11, 10));
    let seen = put(&mut sim, WARRIOR, 2, (14, 10));
    sim.state.fog.seen.or(14, 10, 1 << ME);
    let chain = [ME_I, 2, 10, 10, 0, W];
    for code in [0x4E26, 0x4F26] {
        assert_eq!(ask(&mut sim, code, &chain), seen, "{code:#x}");
    }
    assert_eq!(ask(&mut sim, 0x4E24, &chain), hidden);
    assert_eq!(ask(&mut sim, 0x4F24, &chain), hidden);
    let fog =
        |sim: &mut Sim, code, words: &[i32]| query_once(sim, ME, Sight::Fog, code, words).0[2];
    for code in [0x4E24, 0x4E26, 0x4F24, 0x4F26] {
        assert_eq!(fog(&mut sim, code, &chain), seen, "{code:#x}");
    }
    assert_eq!(fog(&mut sim, 0x4F25, &[ME_I, 2, W, 10, 10, 0]), seen);
    // Own objects the slot always sees, even in fog.
    let own = put(&mut sim, WARRIOR, ME_I, (12, 10));
    assert_eq!(fog(&mut sim, 0x4F25, &[ME_I, ME_I, W, 10, 10, 0]), own);
    // With `MM_CHECKFOG` for another `[2]` (sight of slot 2 on own objects).
    sim.state.fog.seen.or(11, 10, 1 << 2);
    assert_eq!(ask(&mut sim, 0x4F26, &[2, 2, 10, 10, 0, W]), hidden);
}

// QUIRK E-58: kept
#[test]
fn ref_player_nine_searches_trees() {
    // E-58: reference player 9 searches the tree list (list B), otherwise only list A.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let tree = put(&mut sim, TREE, 0, (12, 10));
    assert_eq!(oid(tree).slot(), 9);
    let t = TREE as i32;
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, 9, t, 10, 10, 0]), tree);
    assert_eq!(ask(&mut sim, 0x4F24, &[ME_I, 9, 10, 10, 0, MINE]), tree);
    // Every other reference player searches list A: also -1 (any owner) finds no tree.
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, -1, t, 10, 10, 0]), 0);
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, ME_I, t, 10, 10, 0]), 0);
}

// QUIRK E-51, E-54: kept
#[test]
fn hq_search_uses_the_map_centre_and_finds_unfinished_hqs() {
    // re/ki-queries 5.4, E-51: main house GUID of the nation of `[3]` (nation 2 -> 1000), center = half the map as
    // fixed point (64 tiles -> corner (32, 32)), filter without `HANDLETYPE`: also construction sites.
    let (rules, mut state) = scene();
    state.players[usize::from(ME)].nation = 2;
    state.players[4].nation = 6;
    let mut sim = Sim::new(&mut state, &rules);
    let done = put(&mut sim, HQ, ME_I, (40, 40));
    let unfinished = site(&mut sim, HQ, ME_I, (30, 31)).as_i32();
    assert_eq!(run(&mut sim, 0x45, &[]), [HQ as i32]);
    assert_eq!(run(&mut sim, 0x46, &[]), [unfinished]);
    assert_eq!(run(&mut sim, 0x47, &[ME_I]), [unfinished]);
    let (t, _) = run_with(&mut sim, 0x46, &[FLOOR]);
    assert_eq!(t.pause_ms, 0);
    // Slot 3 (nation 0 -> 1300) has no main house: `[2] = 0`, `[3] = 1300` in both branches.
    let req = query_once(&mut sim, ME, Sight::Omniscient, 0x4E21, &[ME_I, 3]).0;
    assert_eq!((req[2], req[3]), (0, 1300));
    // Nation outside 0-5 or slot outside 0-7: GUID 0, nothing found.
    for p in [4, 8, -1] {
        let req = query_once(&mut sim, ME, Sight::Omniscient, 0x4E21, &[ME_I, p]).0;
        assert_eq!((req[2], req[3]), (0, 0), "slot {p}");
    }
    // Wrong argc: field unchanged.
    let req = query_once(&mut sim, ME, Sight::Omniscient, 0x4E21, &[ME_I, ME_I, 7]).0;
    assert_eq!((req[2], req[3]), (ME_I, ME_I));
    // The filter ID 1000 (`HANDLETYPE 0x403`), on the other hand, finds only the finished main house.
    let hq = HQ as i32;
    assert_eq!(ask(&mut sim, 0x4F25, &[ME_I, ME_I, hq, 32, 32, hq]), done);
}

// QUIRK E-51: kept
#[test]
fn building_and_type_searches_use_fresh_filters_and_never_mark() {
    // re/ki-queries 5.3: `0x4E22` = module 3 (`0x80114`, `f[4] = 3`), `0x4E23` = GUID `[4]` (`0x80118`); both carry
    // `0x80000` (skip marked ones), but do not mark and do not pause.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let near = put(&mut sim, HQ, ME_I, (14, 10));
    let far = put(&mut sim, HQ, ME_I, (30, 10));
    let colonist = put(&mut sim, COLONIST, ME_I, (10, 10));
    let (t, _) = run_with(&mut sim, 0x48, &[FLOOR, 10, 10, 7, 7]);
    assert_eq!((t.stack[1], t.pause_ms), (near, 0));
    assert_eq!(marks(&sim, near), 0);
    assert_eq!(run(&mut sim, 0x4B, &[ME_I, 10, 10, 0, 0]), [near]);
    assert_eq!(run(&mut sim, 0x4B, &[2, 10, 10, 0, 0]), [0]);
    assert_eq!(
        run(&mut sim, 0x4E, &[COLONIST as i32, 12, 10, 0]),
        [colonist]
    );
    assert_eq!(run(&mut sim, 0x51, &[ME_I, HQ as i32, 31, 10, 0]), [far]);
    // Marked by the own slot: skipped.
    assert_eq!(run(&mut sim, 0x9B, &[near]), []);
    assert_eq!(run(&mut sim, 0x48, &[10, 10, 0, 0]), [far]);
    assert_eq!(run(&mut sim, 0x4E, &[HQ as i32, 10, 10, 0]), [far]);
    // argc != 6: field stays.
    assert_eq!(ask(&mut sim, 0x4E22, &[ME_I, ME_I, 10, 10, 0, 0, 5]), ME_I);
}

// QUIRK E-86: deviation
#[test]
fn production_check_of_0x50d2() {
    // re/ki-queries 5.10: all objects of the filter `[3]` of slot `[2]` (tile 0...1000) have the building flag
    // `0x4000000` ("currently producing"); empty -> 1, objects without a building module -> 0.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let a = put(&mut sim, HQ, ME_I, (20, 20));
    let b = put(&mut sim, HQ, ME_I, (30, 20));
    let _colonist = put(&mut sim, COLONIST, ME_I, (10, 10));
    let hq = HQ as i32;
    let producing = |sim: &mut Sim, id: i32| {
        let m = sim.object_mut(oid(id)).unwrap().module_mut(3).unwrap();
        m.base.flags |= 0x400_0000;
    };
    assert_eq!(run(&mut sim, 0x13A, &[hq]), [0]);
    producing(&mut sim, a);
    assert_eq!(run(&mut sim, 0x13A, &[hq]), [0]);
    producing(&mut sim, b);
    assert_eq!(run(&mut sim, 0x13A, &[hq]), [1]);
    // No object of the filter (house, unknown ID): 1.
    assert_eq!(run(&mut sim, 0x13A, &[HOUSE as i32]), [1]);
    assert_eq!(run(&mut sim, 0x13A, &[999]), [1]);
    // The colonist has no building module.
    assert_eq!(run(&mut sim, 0x13A, &[MINE]), [0]);
    // Another slot: slot 2 has nothing; slot outside 0-7: empty.
    assert_eq!(ask(&mut sim, 0x50D2, &[2, MINE]), 1);
    assert_eq!(ask(&mut sim, 0x50D2, &[8, MINE]), 1);
    // `FUN_0047EA20` reads the object table of slots 0-10 (the counting codes only 0-7): a building on slot 8 (neutral)
    // or 10 (nature) counts, slot > 10 reads behind the table in the original (exception A7) and is empty here.
    let neutral = put(&mut sim, HQ, 8, (40, 20));
    assert_eq!(oid(neutral).slot(), 8);
    assert_eq!(
        ask(&mut sim, 0x50D2, &[8, hq]),
        0,
        "slot 8, building does not produce"
    );
    producing(&mut sim, neutral);
    assert_eq!(ask(&mut sim, 0x50D2, &[8, hq]), 1);
    let nature = put(&mut sim, BASKET, ME_I, (12, 12));
    assert_eq!(oid(nature).slot(), 10);
    assert_eq!(
        ask(&mut sim, 0x50D2, &[10, BASKET as i32]),
        0,
        "slot 10, no building module"
    );
    for slot in [11, 31, 1000, -1, i32::MIN] {
        assert_eq!(ask(&mut sim, 0x50D2, &[slot, hq]), 1, "slot {slot} empty");
    }
    assert_eq!(ask(&mut sim, 0x50D2, &[ME_I, MINE, 0]), ME_I, "wrong argc");
}

/// Thread with integer variables 0-7 and the lists `list(0)`, `list(1)`.
fn list_thread(stack: &[i32]) -> Thread {
    let ranges = [(0, 8), (list(0), 2)];
    Thread::from_prototype(&proto(7, &ranges, &[], &[]), 2, ME, stack.to_vec())
}

/// Builtin `id` on the thread `t` against the real world.
fn run_on(sim: &mut Sim, id: u16, t: &mut Thread) -> Vec<KiNote> {
    let mut w = SimKiWorld::new(sim, Sight::Omniscient);
    w.builtin(id, &mut BuiltinCx::new(ME, t));
    w.notes
}

// QUIRK E-55: kept
#[test]
fn mark_codes_touch_dead_objects_but_not_missing_ones() {
    // re/ki-queries 5.9: `FUN_0042DA90` searches with `GetObjectById(id, 0)`: the dead are marked and unmarked, missing
    // ones are not (no panic).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let alive = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let dead = put(&mut sim, WARRIOR, ME_I, (12, 10));
    let gone = put(&mut sim, WARRIOR, ME_I, (14, 10));
    sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
    crate::world::destroy_object(&mut sim, oid(gone));
    let before = checksum(sim.state);
    for id in [gone, 0, -1] {
        assert_eq!(run(&mut sim, 0x9B, &[id]), []);
        assert_eq!(run(&mut sim, 0x1AE, &[id]), [0], "missing");
    }
    assert_eq!(checksum(sim.state), before);
    for id in [alive, dead] {
        assert_eq!(run(&mut sim, 0x9C, &[id]), []);
        assert_eq!(marks(&sim, id), 1 << ME);
        assert_eq!(run(&mut sim, 0x1AF, &[id]), [1], "the dead count");
        assert_eq!(run(&mut sim, 0xA0, &[id]), []);
        assert_eq!(marks(&sim, id), 0);
    }
    // Wrong argc: no change.
    let (_, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E44, &[ME_I]);
    assert!(direct.is_empty());
    // 0x105-0x107 `(list, start, n)`: `{0x4FA6, n + 2, me, n, ids…}`, 0 < n < 100; the handler reads the IDs and
    // creates missing keys (E-32), the list through `FUN_00413840` without a reference (re/ki-queries 5.8).
    for id in [alive, dead] {
        sim.state.object_mut(oid(id), false).unwrap().ai_marks = 1 << ME | 1 << 2;
    }
    let l0 = list(0) as i32;
    let mut t = list_thread(&[FLOOR, l0, 5, 4]);
    for (k, v) in [(5, alive), (6, gone), (7, dead)] {
        t.vars.list_at_mut(l0).unwrap().insert(k, v);
    }
    assert!(run_on(&mut sim, 0x105, &mut t).is_empty());
    assert_eq!(t.stack, [FLOOR]);
    assert_eq!((marks(&sim, alive), marks(&sim, dead)), (1 << 2, 1 << 2));
    // Key 8 lay beyond the end of the list and is now created with 0.
    assert_eq!(t.vars.list_at(l0).unwrap().get(8), Some(&0));
    // n not in (0, 100): nothing, no creating either.
    for id in [alive, dead] {
        sim.state.object_mut(oid(id), false).unwrap().ai_marks = 1 << ME;
    }
    for n in [0, 100, -3] {
        let mut t = list_thread(&[FLOOR, l0, 20, n]);
        assert!(run_on(&mut sim, 0x106, &mut t).is_empty());
        assert_eq!(t.stack, [FLOOR]);
        assert_eq!(t.vars.list_at(l0).unwrap().len(), 0, "n = {n}");
    }
    assert_eq!(marks(&sim, alive), 1 << ME);
    // n = 99 is the largest effective number: all 99 keys are read (and created), the last one (`start + 98`) also
    // clears the mark.
    let mut t = list_thread(&[FLOOR, l0, 3, 99]);
    t.vars.list_at_mut(l0).unwrap().insert(3 + 98, alive);
    assert!(run_on(&mut sim, 0x105, &mut t).is_empty());
    assert_eq!(t.stack, [FLOOR]);
    assert_eq!(t.vars.list_at(l0).unwrap().len(), 99);
    assert_eq!(marks(&sim, alive), 0);
    sim.state.object_mut(oid(alive), false).unwrap().ai_marks = 1 << ME;
    // Pointer 0 (integer variable with 0): nothing. Pointer without a list: `Guarded`, nothing.
    let mut t = list_thread(&[FLOOR, int(3) as i32, 0, 2]);
    assert!(run_on(&mut sim, 0x107, &mut t).is_empty());
    t.vars.set(int(3), 12_345);
    t.stack = vec![FLOOR, int(3) as i32, 0, 2];
    let notes = run_on(&mut sim, 0x107, &mut t);
    assert!(matches!(
        notes[..],
        [KiNote::Guarded { builtin: 0x107, .. }]
    ));
    assert_eq!(t.stack, [FLOOR]);
}

// QUIRK E-84: kept
#[test]
fn triple_search_builtins_read_head_and_centre_from_the_shifted_words() {
    // E-84: at 0x235-0x237 the handler builds `{me, me, a1…a5}`. `0x50C3` (0x235) reads the center from `[4]`/`[5]` and
    // the head from `[6]`; `0x50C8`/`0x50CD` (0x236/0x237) read the head from `[4]` and the center from `[5]`/`[6]`
    // (`0x004A3F80`): `run(0x236, [head, x, z, fB, fC])`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let far = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let near = put(&mut sim, WARRIOR, ME_I, (20, 10));
    // Nearer to the center, but not of type warrior (third filter).
    let _colonist = put(&mut sim, COLONIST, ME_I, (19, 10));
    // Center (20, 10): `near`. If the handler read the center from `a1, a2` = (5, 20), `far` would come.
    assert_eq!(run(&mut sim, 0x236, &[MINE, 20, 10, MINE, W]), [near]);
    assert_eq!(marks(&sim, near), 1 << ME, "0x50C8 marks");
    // 0x237 (0x50CD) does not mark and does not skip marked ones.
    assert_eq!(run(&mut sim, 0x237, &[MINE, 20, 10, MINE, W]), [near]);
    // The hit is marked: the next marking search returns the other one.
    assert_eq!(run(&mut sim, 0x236, &[MINE, 20, 10, MINE, W]), [far]);
    assert_eq!(marks(&sim, far), 1 << ME);
    assert_eq!(run(&mut sim, 0x236, &[MINE, 20, 10, MINE, W]), [0]);
    // 0x235: center `a1, a2`, head `a3`.
    sim.state.object_mut(oid(near), false).unwrap().ai_marks = 0;
    assert_eq!(run(&mut sim, 0x235, &[20, 10, MINE, MINE, W]), [near]);
    // No pause for the triple searches.
    let (t, _) = run_with(&mut sim, 0x236, &[FLOOR, MINE, 20, 10, MINE, W]);
    assert_eq!(t.pause_ms, 0);
}

// QUIRK E-57: kept
#[test]
fn mark_query_reads_bit_p_and_31() {
    // `0x4F97` reads bit `p & 31`; bits >= 8 do not exist (0). The writers produce `Direct::Mark` only for
    // `p & 31 < 8`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let w = put(&mut sim, WARRIOR, ME_I, (10, 10));
    sim.state.object_mut(oid(w), false).unwrap().ai_marks = 1 << 3;
    for (p, bit) in [(3, 1), (35, 1), (-29, 1), (4, 0), (8, 0), (40, 0), (31, 0)] {
        assert_eq!(run(&mut sim, 0x1B1, &[p, w]), [bit], "p = {p}");
        assert_eq!(ask(&mut sim, 0x4F97, &[p, w]), bit, "p = {p}");
    }
    assert_eq!(run(&mut sim, 0x1AE, &[w]), [0], "own bit 1");
    assert_eq!(ask(&mut sim, 0x4F97, &[3, w, 0]), 3, "wrong argc");
    let object = oid(w);
    for (p, q) in [
        (3, Some(3)),
        (35, Some(3)),
        (-25, Some(7)),
        (8, None),
        (40, None),
    ] {
        let (_, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E45, &[p, w]);
        let want: Vec<Direct> = q
            .map(|player| Direct::Mark {
                object,
                player,
                on: false,
            })
            .into_iter()
            .collect();
        assert_eq!(direct, want, "p = {p}");
    }
}

// QUIRK E-35: kept
#[test]
fn ki_query_does_not_apply_marks() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let w = put(&mut sim, WARRIOR, ME_I, (11, 10));
    let words = [ME_I, ME_I, 10, 10, 0, W];
    for _ in 0..2 {
        let (req, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E24, &words);
        assert_eq!(req[2], w);
        assert_eq!(
            direct,
            [Direct::Mark {
                object: oid(w),
                player: ME,
                on: true
            }]
        );
        assert_eq!(marks(&sim, w), 0);
    }
    // Without a mark (0x4F24) no direct change either.
    let (_, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x4F24, &words);
    assert!(direct.is_empty());
}

/// Builtin IDs covered here (131).
pub(super) const SEARCH_BUILTINS: [std::ops::RangeInclusive<u16>; 10] = [
    0x45..=0x53,
    0x55..=0x6C,
    0x9B..=0xA0,
    0x105..=0x107,
    0x13A..=0x13A,
    0x16F..=0x16F,
    0x1AE..=0x1B3,
    0x1E5..=0x1FC,
    0x1FE..=0x22D,
    0x235..=0x237,
];

// QUIRK E-36: kept
#[test]
fn search_pauses_follow_the_signature() {
    // 500 ms after the searches `0x4E24`-`0x4E27`, `0x4F24`-`0x4F28`; otherwise no pause.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mut n = 0;
    for ids in SEARCH_BUILTINS {
        for id in ids {
            assert!(is_query(id), "{id:#x}");
            n += 1;
            let r = row(id).expect("row");
            let want = if matches!(r.code, 0x4E24..=0x4E27 | 0x4F24..=0x4F28) {
                500
            } else {
                0
            };
            assert_eq!(r.pause_ms, want, "{id:#x}");
            let (t, _) = run_with(&mut sim, id, &[FLOOR, 0, 0, 0, 0, 0, 0]);
            assert_eq!(t.pause_ms, want, "{id:#x}");
            if let Some(sig) = signature(id) {
                assert_eq!(
                    (sig.pops, sig.push, sig.pause_ms, sig.code),
                    (r.pops, r.push, r.pause_ms, r.code),
                    "{id:#x}"
                );
                let mut outs: Vec<u8> = r.outs.iter().map(|&(a, _)| a).collect();
                outs.sort_unstable();
                assert_eq!(outs, sig.outs, "{id:#x}");
            }
        }
    }
    assert_eq!(n, 131, "number of builtins covered here");
    assert!(!is_query(0x54) && !is_query(0x1FD), "gaps");
    // Every row of the search codes belongs to this group.
    for r in ROWS {
        if matches!(r.code, 0x4E21..=0x4E27 | 0x4F24..=0x4F28 | 0x50C3..=0x50CD) {
            assert!(SEARCH_BUILTINS.iter().any(|ids| ids.contains(&r.lo)));
        }
    }
}

/// Query codes covered here.
pub(super) const CODES: [u32; 20] = [
    0x4E21, 0x4E22, 0x4E23, 0x4E24, 0x4E25, 0x4E26, 0x4E27, 0x4E44, 0x4E45, 0x4F24, 0x4F25, 0x4F26,
    0x4F28, 0x4F97, 0x4FA6, 0x50C3, 0x50C8, 0x50CD, 0x50D2, 0x512E,
];

#[test]
fn search_queries_leave_the_checksum_unchanged() {
    let (rules, mut state) = scene();
    let o = {
        let mut sim = Sim::new(&mut state, &rules);
        let w = put(&mut sim, WARRIOR, ME_I, (10, 10));
        let dead = put(&mut sim, WARRIOR, 2, (12, 10));
        sim.object_mut(oid(dead)).unwrap().status |= status::DIED;
        let _ = put(&mut sim, BASKET, ME_I, (11, 11));
        let _ = put(&mut sim, TREE, 0, (14, 14));
        let hq = put(&mut sim, HQ, ME_I, (30, 30));
        crate::messaging::report_attack(&mut sim, oid(w));
        sim.state.players[usize::from(ME)].control = crate::state::Control::Ai;
        crate::messaging::report_attack(&mut sim, oid(w));
        [w, dead, hq]
    };
    assert_eq!(state.attack_reports[usize::from(ME)].len(), 1);
    let (before, events) = (checksum(&state), state.events.clone());
    let mut sim = Sim::new(&mut state, &rules);
    let values = [
        o[0],
        o[1],
        o[2],
        0,
        -1,
        1,
        3,
        8,
        9,
        31,
        1000,
        W,
        MINE,
        CORPSE,
        i32::MIN,
        i32::MAX,
    ];
    for code in CODES {
        for &a in &values {
            for &b in &values {
                let words = [ME_I, a, 10, 10, b, a, b, MINE, 3];
                for len in [9, 8, 7, 6, 2, 0] {
                    let _ = query_once(&mut sim, ME, Sight::Omniscient, code, &words[..len]);
                    let _ = query_once(&mut sim, ME, Sight::Fog, code, &words[..len]);
                }
                let words = [a, b, a, b, a, b, a];
                let _ = query_once(&mut sim, 0, Sight::Fog, code, &words);
            }
        }
    }
    assert_eq!(checksum(&state), before);
    assert_eq!(state.events, events);
}
