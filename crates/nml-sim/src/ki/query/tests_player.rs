//! Tests of the query facade, the real AI world [`SimKiWorld`] and the player and match queries (re/ki-queries 2, 5.13,
//! 5.16, 5.19).

use std::collections::BTreeMap;

use super::{GROUPS, QUERY_WORDS, SimKiWorld, is_query, query_once};
use crate::command::testkit::{account, rules, state};
use crate::ki::KiState;
use crate::ki::MAX_STACK;
use crate::ki::testkit::{self, int, proto, word};
use crate::ki::vm::{Thread, op};
use crate::ki::world::{BuiltinCx, KiLimit, KiNote, KiWorld};
use crate::module::ability_testkit as tk;
use crate::player::{Sight, View};
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::{Control, SimState};

/// Slot and `m_iSpieler` of the test instance.
pub(super) const ME: u8 = 1;
/// Bottom stack value: shows that a builtin does not pop more than intended.
pub(super) const FLOOR: i32 = -7;

/// Query codes covered here (player and match).
const CODES: [u32; 18] = [
    0x4E20, 0x4E2D, 0x4E2E, 0x4E2F, 0x4E32, 0x4E37, 0x4E38, 0x4E39, 0x4E3C, 0x4E3E, 0x4E40, 0x4E51,
    0x4E53, 0x4E54, 0x4E55, 0x4EF7, 0x5118, 0x5130,
];

fn scene() -> (Rules, SimState) {
    let rules = rules();
    let state = state(&rules);
    (rules, state)
}

/// Thread 2 of module 7 with the integer variables 0-7, player [`ME`].
fn thread(stack: &[i32]) -> Thread {
    Thread::from_prototype(&proto(7, &[(0, 8)], &[], &[]), 2, ME, stack.to_vec())
}

/// Builtin `id` with the stack `stack` against the real world (omniscient); thread and notes afterwards.
pub(super) fn run_with(sim: &mut Sim, id: u16, stack: &[i32]) -> (Thread, Vec<KiNote>) {
    let mut t = thread(stack);
    let mut w = SimKiWorld::new(sim, Sight::Omniscient);
    w.builtin(id, &mut BuiltinCx::new(ME, &mut t));
    assert_eq!(w.counts, BTreeMap::from([(id, 1)]));
    (t, w.notes)
}

/// Stack after the builtin `id`; `stack` lies above [`FLOOR`].
pub(super) fn run(sim: &mut Sim, id: u16, stack: &[i32]) -> Vec<i32> {
    let mut full = vec![FLOOR];
    full.extend_from_slice(stack);
    let (t, notes) = run_with(sim, id, &full);
    assert!(notes.is_empty(), "{id:#x}: {notes:?}");
    assert_eq!(t.stack.first(), Some(&FLOOR), "{id:#x} pops too much");
    t.stack[1..].to_vec()
}

/// Result `[2]` of the query `code` with the words `words` from `[2]` (omniscient, slot [`ME`]).
pub(super) fn ask(sim: &mut Sim, code: u32, words: &[i32]) -> i32 {
    query_once(sim, ME, Sight::Omniscient, code, words).0[2]
}

#[test]
fn people_query_accepts_slots_0_to_7_only() {
    let (rules, mut state) = scene();
    state.players[3].nation = 4;
    state.players[7].nation = 5;
    state.players[usize::from(ME)].nation = 2;
    let mut sim = Sim::new(&mut state, &rules);
    let me = i32::from(ME);
    // Slots 0-7 without an occupancy check; a never defined slot has nation 0 (constructor).
    assert_eq!(ask(&mut sim, 0x4E20, &[me, 3]), 4);
    assert_eq!(ask(&mut sim, 0x4E20, &[me, 7]), 5);
    assert_eq!(ask(&mut sim, 0x4E20, &[me, 0]), 0);
    for p in [8, -1, 31, 1000] {
        assert_eq!(ask(&mut sim, 0x4E20, &[me, p]), -1, "slot {p}");
    }
    // 0x83: own nation `{2, me, me}`; 0x84: nation of `p` `{2, me, a1}`.
    assert_eq!(run(&mut sim, 0x83, &[]), [2]);
    assert_eq!(run(&mut sim, 0x84, &[3]), [4]);
    assert_eq!(run(&mut sim, 0x84, &[8]), [-1]);
}

// QUIRK E-75: kept
#[test]
fn single_player_is_mode_below_zero() {
    // E-75: campaign/direct start (-1) and menu level (-2) -> 1, free play and LAN (0...6) -> 0.
    let (rules, mut state) = scene();
    for (mode, single) in [(-1, 1), (-2, 1), (0, 0), (3, 0), (6, 0)] {
        state.mode = mode;
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(run(&mut sim, 0x7F, &[]), [single], "mode {mode}");
        assert_eq!(run(&mut sim, 0x80, &[]), [mode], "mode {mode}");
    }
}

// QUIRK E-76: kept
#[test]
fn defined_slots_count_the_level_slots_not_the_participants() {
    // E-76: `Player+0x218` over 8 slots, whatever the control kind.
    let (rules, mut state) = scene();
    for slot in [0, 2, 5] {
        state.players[slot].slot_active = true;
    }
    state.players[3].control = Control::Human;
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(run(&mut sim, 0x81, &[]), [3]);
}

#[test]
fn control_query_maps_the_control_type() {
    let (rules, mut state) = scene();
    state.players[1].control = Control::Human;
    state.players[2].control = Control::Ai;
    state.players[3].control = Control::Passive;
    let mut sim = Sim::new(&mut state, &rules);
    for (p, want) in [(0, 0), (1, 1), (2, 2), (3, 3), (8, 0), (-1, 0)] {
        assert_eq!(run(&mut sim, 0x85, &[p]), [want], "slot {p}");
    }
}

// QUIRK E-83: kept
#[test]
fn water_share_is_percent_of_the_counted_tiles() {
    let (mut rules, mut state) = scene();
    (rules.water_tiles, rules.tile_count) = (30, 120);
    assert_eq!(run(&mut Sim::new(&mut state, &rules), 0x86, &[]), [25]);
    (rules.water_tiles, rules.tile_count) = (1, 3);
    assert_eq!(run(&mut Sim::new(&mut state, &rules), 0x86, &[]), [33]);
    // Without tiles as before loading (1/1 = 100 %), instead of the division by 0 of the original.
    (rules.water_tiles, rules.tile_count) = (0, 0);
    assert_eq!(run(&mut Sim::new(&mut state, &rules), 0x86, &[]), [100]);
}

#[test]
fn map_size_queries_read_the_terrain_dimensions() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(run(&mut sim, 0x8A, &[]), [rules.map.dim_x]);
    assert_eq!(run(&mut sim, 0x8B, &[]), [rules.map.dim_z]);
}

// QUIRK E-41: deviation
#[test]
fn difficulty_maps_only_in_the_campaign_and_clamps_negative_to_one() {
    let (rules, mut state) = scene();
    let me = i32::from(ME);
    // (mode, value, query, builtin 0x8C): only at mode -1 1 -> 3 and 2 -> 5; the handler turns negatives into 1, 0
    // stays 0.
    for (mode, v, query, builtin) in [
        (-1, 0, 0, 0),
        (-1, 1, 3, 3),
        (-1, 2, 5, 5),
        (-1, -4, -4, 1),
        (-2, 1, 1, 1),
        (-2, 2, 2, 2),
        (0, 2, 2, 2),
        (0, -1, -1, 1),
        (0, 0, 0, 0),
    ] {
        state.mode = mode;
        state.campaign_difficulty = v;
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(
            ask(&mut sim, 0x4E39, &[me]),
            query,
            "mode {mode}, value {v}"
        );
        assert_eq!(
            run(&mut sim, 0x8C, &[]),
            [builtin],
            "mode {mode}, value {v}"
        );
    }
}

// QUIRK E-82: kept
#[test]
fn constant_parity_and_island_queries() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    // 0x4E3C: `GameEngine+0x5FC`, set to 2 only in the constructor.
    assert_eq!(run(&mut sim, 0x8F, &[]), [2]);
    // 0x5118: `[2] % 2` as the C remainder.
    assert_eq!(run(&mut sim, 0x144, &[]), [i32::from(ME) % 2]);
    assert_eq!(ask(&mut sim, 0x5118, &[-3]), -1);
    assert_eq!(ask(&mut sim, 0x5118, &[i32::MIN]), 0);
    // 0x5130: fixed map (`random_map` = `None`) -> never an island (E-82).
    assert_eq!(run(&mut sim, 0x23B, &[]), [0]);
}

/// `0x5130` reads the island flag of the random map (`GameEngine+0x30C`, E-82).
// QUIRK E-82: kept
#[test]
fn island_query_reads_the_random_map_flag() {
    use crate::random_map::{RandomMapRules, RandomMapSpec};
    for (island, expected) in [(true, 1), (false, 0)] {
        let mut rules = rules();
        rules.random_map = Some(RandomMapRules {
            spec: RandomMapSpec {
                seed_text: "ZKA".into(),
                size: 1,
                players: 4,
                terrain: 3,
                landscape: 0,
                daytime: 0,
                game_type: 0,
            },
            slots: [true, true, true, true, false, false, false, false],
            island,
            crt_start: 1,
            digest: 0,
        });
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(run(&mut sim, 0x23B, &[]), [expected], "island {island}");
        assert_eq!(ask(&mut sim, 0x5130, &[]), expected, "island {island}");
    }
}

#[test]
fn resource_queries_read_own_and_any_player() {
    let (rules, mut state) = scene();
    state.economy.max_living_space = 40;
    let mut sim = Sim::new(&mut state, &rules);
    account(&mut sim, usize::from(ME), [100, 200, 300, 4, 50]);
    account(&mut sim, 0, [7, 8, 9, 3, 10]);
    // 0x92 (r) own resource, 0x93 (p, r) any player; invalid -> 0.
    assert_eq!(run(&mut sim, 0x92, &[2]), [300]);
    assert_eq!(run(&mut sim, 0x92, &[4]), [40], "living space limited");
    assert_eq!(run(&mut sim, 0x93, &[0, 1]), [8]);
    assert_eq!(run(&mut sim, 0x93, &[0, 7]), [0]);
    assert_eq!(run(&mut sim, 0x93, &[9, 0]), [0]);
    assert_eq!(run(&mut sim, 0x93, &[-1, 0]), [0]);
    // 0xD2 living space limit; 0xD3/0xD4 free living space R4 - R3; 0xD5/0xD6 R4 (limited).
    assert_eq!(run(&mut sim, 0xD2, &[]), [40]);
    assert_eq!(run(&mut sim, 0xD3, &[]), [36]);
    assert_eq!(run(&mut sim, 0xD4, &[0]), [7]);
    assert_eq!(run(&mut sim, 0xD4, &[8]), [0]);
    assert_eq!(run(&mut sim, 0xD5, &[]), [40]);
    assert_eq!(run(&mut sim, 0xD6, &[0]), [10]);
    assert_eq!(run(&mut sim, 0xD6, &[-1]), [0]);
}

// QUIRK E-81: kept
#[test]
fn relation_bits_include_self_five_and_slot_eight_enemy() {
    // E-81: defaults of `GameWorld_InitDiplomacy`: p == q -> friend + sight = 5, p != q -> enemy = 2, slot 8 is an
    // enemy of all slots 0-7 but is in no list; directed.
    let (rules, mut state) = scene();
    state.diplomacy.friend[1] |= 1 << 2;
    state.diplomacy.enemy[1] &= !(1 << 2);
    state.diplomacy.view[1] |= 1 << 3;
    let mut sim = Sim::new(&mut state, &rules);
    for (p, q, bits) in [
        (0, 0, 5),
        (0, 1, 2),
        (1, 2, 1),
        (2, 1, 2),
        (1, 3, 6),
        (8, 1, 2),
        (8, 8, 0),
        (1, 8, 0),
        (-1, 0, 0),
        (0, 31, 0),
    ] {
        assert_eq!(run(&mut sim, 0x96, &[p, q]), [bits], "{p} -> {q}");
    }
}

// QUIRK E-33: deviation
#[test]
fn headhunter_query_without_hunter_writes_addresses() {
    // E-33: no headhunter -> push -1, the output variables get their own address.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for art in [0, 2, 3, -1] {
        let (t, notes) = run_with(&mut sim, 0xCD, &[FLOOR, art, 1, 2, 3]);
        assert!(notes.is_empty());
        assert_eq!(t.stack, [FLOOR, -1], "kind {art}");
        assert_eq!(
            (t.vars.get(int(1)), t.vars.get(int(2)), t.vars.get(int(3))),
            (1, 2, 3)
        );
    }
}

#[test]
fn headhunter_query_reads_target_customer_guid_and_price() {
    // re/ki-queries 5.16.1: push target player (`+0x78`), a = customer (`+0x80`), b = target GUID (`+0x70`), c = price
    // (`+0x7C`).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hh = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (10, 10));
    {
        let h = crate::module::headhunter::headhunter_mut(&mut sim, hh).expect("headhunter");
        (h.cur_reward, h.target_player, h.target_guid, h.customer) = (450, 1, 2615, 0);
    }
    let art = crate::module::headhunter::GUIDS
        .iter()
        .position(|&g| g == tk::HUNTER_SPA)
        .expect("kind") as i32;
    let (before, events) = (checksum(sim.state), sim.state.events.clone());
    let (t, _) = run_with(&mut sim, 0xCD, &[FLOOR, art, 1, 2, 3]);
    assert_eq!(t.stack, [FLOOR, 1]);
    assert_eq!(
        (t.vars.get(int(1)), t.vars.get(int(2)), t.vars.get(int(3))),
        (0, 2615, 450)
    );
    assert_eq!(checksum(sim.state), before);
    assert_eq!(sim.state.events, events);
    // Dead, not yet destroyed headhunter (module 0x22 switched off): push 0, all three variables 0
    // (re/ki-queries 5.16.1); the headhunter stays registered.
    {
        let h = sim.object_mut(hh).expect("headhunter");
        h.status |= crate::object::status::DIED;
        h.set_module_enabled(crate::module::headhunter::INDEX, false);
    }
    let (before, events) = (checksum(sim.state), sim.state.events.clone());
    let (t, notes) = run_with(&mut sim, 0xCD, &[FLOOR, art, 1, 2, 3]);
    assert_eq!(checksum(sim.state), before);
    assert_eq!(sim.state.events, events);
    assert!(notes.is_empty());
    assert_eq!(t.stack, [FLOOR, 0]);
    assert_eq!(
        (t.vars.get(int(1)), t.vars.get(int(2)), t.vars.get(int(3))),
        (0, 0, 0)
    );
    // Headhunter ID registered, object gone: like "no headhunter" (-1, variables keep their addresses).
    crate::world::destroy_object(&mut sim, hh);
    sim.state.headhunters[art as usize] = Some(hh);
    let (t, notes) = run_with(&mut sim, 0xCD, &[FLOOR, art, 1, 2, 3]);
    assert!(notes.is_empty());
    assert_eq!(t.stack, [FLOOR, -1]);
    assert_eq!(
        (t.vars.get(int(1)), t.vars.get(int(2)), t.vars.get(int(3))),
        (1, 2, 3)
    );
    let (before, events) = (checksum(sim.state), sim.state.events.clone());
    let _ = run_with(&mut sim, 0xCD, &[FLOOR, art, 1, 2, 3]);
    assert_eq!(checksum(sim.state), before);
    assert_eq!(sim.state.events, events);
}

// QUIRK E-80: kept
// QUIRK E-34: deviation
#[test]
fn nation_tables_follow_the_own_nation() {
    let (rules, mut state) = scene();
    let nation = |state: &mut SimState, v: i32| state.players[usize::from(ME)].nation = v;
    // 0xA4: mounted -> unmounted per nation; nation 2 additionally 1100 -> 1101 (E-80).
    nation(&mut state, 2);
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(run(&mut sim, 0xA4, &[1104]), [1103]);
        assert_eq!(run(&mut sim, 0xA4, &[1100]), [1101]);
        assert_eq!(run(&mut sim, 0xA4, &[1406]), [1406], "foreign nation");
        assert_eq!(run(&mut sim, 0x133, &[]), [1002]);
        assert_eq!(run(&mut sim, 0x116, &[]), [0]);
    }
    nation(&mut state, 0);
    assert_eq!(
        run(&mut Sim::new(&mut state, &rules), 0xA4, &[1406]),
        [1403]
    );
    nation(&mut state, 5);
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(run(&mut sim, 0xA4, &[2309]), [2308]);
        assert_eq!(run(&mut sim, 0x116, &[]), [2306]);
        assert_eq!(run(&mut sim, 0x133, &[]), [2202]);
        assert_eq!(run(&mut sim, 0x134, &[]), [2204]);
        assert_eq!(run(&mut sim, 0x135, &[]), [2203]);
    }
    nation(&mut state, 4);
    assert_eq!(run(&mut Sim::new(&mut state, &rules), 0x116, &[]), [2005]);
    // Nation not in 0-5 (only from level data): 0x133-0x135 push nothing (E-34), 0xA4 leaves the type.
    nation(&mut state, 6);
    {
        let mut sim = Sim::new(&mut state, &rules);
        for id in 0x133..=0x135 {
            assert!(run(&mut sim, id, &[]).is_empty(), "{id:#x}");
        }
        assert_eq!(run(&mut sim, 0xA4, &[1104]), [1104]);
        // 0x120: nation from the stack; 0x108: mounted GUID, both without a query.
        for (v, guid) in [(4, 2005), (5, 2306), (3, 0), (-1, 0)] {
            assert_eq!(run(&mut sim, 0x120, &[v]), [guid], "nation {v}");
        }
        for (g, mounted) in [(1101, 1), (1114, 1), (2619, 1), (1100, 0), (0, 0)] {
            assert_eq!(run(&mut sim, 0x108, &[g]), [mounted], "GUID {g}");
        }
    }
}

// QUIRK E-27: kept
#[test]
fn wrong_argc_leaves_the_own_player() {
    // E-27: wrong argument count or unknown code -> field unchanged, `[2]` stays `me`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let me = i32::from(ME);
    let (req, direct) = query_once(&mut sim, ME, Sight::Omniscient, 0x4E37, &[me, 9]);
    assert_eq!((req[1], req[2], req[3]), (2, me, 9));
    assert!(direct.is_empty());
    assert_eq!(ask(&mut sim, 0x4E20, &[me]), me);
    assert_eq!(ask(&mut sim, 0x4E3E, &[me, 0]), me);
    assert_eq!(ask(&mut sim, 0x4E40, &[me, 0, 0, 0]), me);
    // Codes without a case in the dispatcher (E-37) and the weather (E-39 does not apply).
    for code in [
        0x4E42, 0x4E43, 0x4E4A, 0x4EFC, 0x4EE8, 0x4FD3, 0x500F, 0x5000, 0x9999,
    ] {
        assert_eq!(ask(&mut sim, code, &[me, 1, 2, 3]), me, "{code:#x}");
    }
}

// QUIRK E-27, E-37: kept
#[test]
fn codes_without_counterpart_push_the_own_player() {
    // E-37: the handler pushes the unchanged `[2] = me` (0x196: argc 2 instead of 3, 0xB9: 6 instead of 5).
    let (rules, mut state) = scene();
    let before = checksum(&state);
    let mut sim = Sim::new(&mut state, &rules);
    let me = i32::from(ME);
    for (id, args) in [
        (0x99, &[5, 6][..]),
        (0xAB, &[][..]),
        (0xAC, &[5][..]),
        (0x190, &[5][..]),
        (0x196, &[5][..]),
        (0xB9, &[0, 1, 2, 3, 4][..]),
    ] {
        assert_eq!(run(&mut sim, id, args), [me], "{id:#x}");
    }
    assert_eq!(checksum(&state), before);
}

// QUIRK E-37: kept
#[test]
fn codes_without_counterpart_push_nothing() {
    // E-37, push column of re/ki-builtins; 0xD0 (weather) pops its argument without effect (E-39).
    let (rules, mut state) = scene();
    let before = checksum(&state);
    let mut sim = Sim::new(&mut state, &rules);
    for (id, args) in [
        (0x98, &[1, 2, 3][..]),
        (0xB5, &[1, 2][..]),
        (0x12A, &[1, 2][..]),
        (0x12B, &[1][..]),
        (0x177, &[1, 2][..]),
        (0x1BC, &[1][..]),
        (0x1BD, &[1][..]),
        (0x1BE, &[1][..]),
        (0xD0, &[1][..]),
    ] {
        assert!(run(&mut sim, id, args).is_empty(), "{id:#x}");
    }
    assert_eq!(checksum(&state), before);
    assert!(state.events.is_empty());
}

#[test]
fn at_most_one_group_answers_each_code() {
    // `dispatch` takes the first group that knows the code; if two groups overlap, the later one never answers. Each
    // group gets its own copy of the field.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mut view = View::new(&mut sim, ME, Sight::Omniscient);
    let field = vec![0; 2 + QUERY_WORDS];
    for code in (0x4E20..=0x5130).chain([0x7530, 0x7531, 0, u32::MAX]) {
        let answering = GROUPS
            .iter()
            .filter(|group| group(&mut view, code, &mut field.clone(), &mut Vec::new()))
            .count();
        assert!(answering <= 1, "{code:#x}: {answering} groups answer");
    }
}

#[test]
fn query_once_with_short_words_never_panics() {
    // The field has at least 2 + 128 words; codes without an argc check read beyond that.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    for code in (0x4E20..=0x5130).chain([0x7530, 0x7531, 0, u32::MAX]) {
        for words in [&[][..], &[i32::from(ME)][..]] {
            let (req, _) = query_once(&mut sim, ME, Sight::Omniscient, code, words);
            assert_eq!(req.len(), 2 + 128);
            assert_eq!((req[0], req[1]), (code as i32, words.len() as i32));
        }
    }
    // More than 128 words go into the field entirely.
    let long = vec![3; 200];
    let (req, _) = query_once(&mut sim, ME, Sight::Fog, 0x4E20, &long);
    assert_eq!((req.len(), req[1], req[201]), (202, 200, 3));
}

/// Module 7: writes `0xE6(slot, key)` to `m_iValues[9]` (`0x1E`); variables 0 = slot, 1 = key.
fn peek_module(slot: i32, key: i32) -> crate::ki::catalog::KiCatalog {
    let code = [
        word(op::PUSHA, 9),
        word(op::LOAD, int(0)),
        word(op::LOAD, int(1)),
        word(op::CALLB, 0xE6),
        word(op::CALLB, 0x1E),
    ];
    testkit::catalog(vec![proto(7, &[(0, 8)], &[(0, slot), (1, key)], &code)])
}

// QUIRK E-38: deviation
#[test]
fn engine_value_reads_own_and_foreign_engines() {
    // Own slot from the own memory, foreign ones through `Peers`; slot without an engine -1, slot outside 0-7 -1 with
    // `Guarded` (the original crashes at 8, E-38).
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mut ki = KiState::new(1);
    ki.engine_mut(1).mem.values.insert(5, 77);
    ki.engine_mut(3).mem.values.insert(5, 99);
    ki.engine_mut(3).mem.values.insert(6, -4);
    for (slot, key, want, guarded) in [
        (1, 5, 77, false),
        (3, 5, 99, false),
        (3, 6, -4, false),
        (3, 8, 0, false),
        (2, 5, -1, false),
        (7, 5, -1, false),
        (8, 5, -1, true),
        (-1, 5, -1, true),
        (1000, 5, -1, true),
    ] {
        let catalog = peek_module(slot, key);
        let mut w = SimKiWorld::new(&mut sim, Sight::Omniscient);
        let ran = ki.with_engine(1, &catalog, &mut w, |e, cx| e.run_sync(cx, 7, &[]));
        assert!(ran.flatten().is_some());
        assert_eq!(
            ki.engines[1].as_ref().unwrap().mem.values.get(&9),
            Some(&want),
            "slot {slot}, key {key}"
        );
        let notes: Vec<bool> = w
            .notes
            .iter()
            .map(|n| matches!(n, KiNote::Guarded { builtin: 0xE6, .. }))
            .collect();
        assert_eq!(
            notes,
            if guarded { vec![true] } else { vec![] },
            "slot {slot}"
        );
    }
    // `with_engine` knows only the slots 0-7.
    let catalog = peek_module(3, 5);
    let mut w = SimKiWorld::new(&mut sim, Sight::Omniscient);
    assert!(ki.with_engine(8, &catalog, &mut w, |_, _| ()).is_none());
}

#[test]
fn tick_engines_lets_each_engine_read_the_others() {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let catalog = peek_module(3, 5);
    let mut ki = KiState::new(1);
    ki.engine_mut(3).mem.values.insert(5, 99);
    let mut w = SimKiWorld::new(&mut sim, Sight::Omniscient);
    ki.with_engine(1, &catalog, &mut w, |e, cx| e.spawn(cx, 7, &[]))
        .expect("slot 1");
    ki.tick_engines(&catalog, &mut w);
    assert_eq!(
        ki.engines[1].as_ref().unwrap().mem.values.get(&9),
        Some(&99)
    );
    assert!(w.notes.is_empty());
}

#[test]
fn sim_world_collects_vm_notes() {
    // Notes of the VM (here a push discarded at a full stack) end up in `notes`.
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let catalog = testkit::catalog(vec![proto(7, &[(0, 8)], &[], &[word(op::CALLB, 0x8A)])]);
    let mut ki = KiState::new(1);
    let mut w = SimKiWorld::new(&mut sim, Sight::Omniscient);
    let args = vec![0; MAX_STACK];
    ki.with_engine(1, &catalog, &mut w, |e, cx| e.run_sync(cx, 7, &args))
        .expect("slot 1");
    assert_eq!(
        w.notes,
        [KiNote::Limit {
            slot: 1,
            thread: 0,
            module: 7,
            limit: KiLimit::Stack,
        }]
    );
    assert_eq!(w.counts, BTreeMap::from([(0x8A, 1)]));
}

#[test]
fn sim_world_answers_commands_by_row() {
    // Commands go to `ki::command`.
    let (rules, mut state) = scene();
    let before = checksum(&state);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(!is_query(0x12D) && !is_query(0x2AA) && !is_query(0x97));
    // 0x12D: production `(building, guid) -> 100` with a command row; building 3 missing -> rejected.
    assert!(crate::ki::command::is_command(0x12D));
    assert_eq!(run(&mut sim, 0x12D, &[3, 1100]), [100]);
    let issued: Vec<_> = std::mem::take(&mut sim.state.events);
    assert!(matches!(
        issued[..],
        [crate::state::SimEvent::KiCommand {
            player: ME,
            code: 0x0E,
            outcome: crate::command::Outcome::Rejected(_),
        }]
    ));
    // 0x2AA (dynamite): command row with 2 pops and pause 5000; actor 1 missing -> no command, the pause still applies
    // (E-96).
    assert!(crate::ki::command::is_command(0x2AA));
    let (t, notes) = run_with(&mut sim, 0x2AA, &[FLOOR, 1, 2]);
    assert_eq!((t.stack, t.pause_ms, notes), (vec![FLOOR], 5000, vec![]));
    assert!(sim.state.events.is_empty());
    // 0x97 (`0x4E41` diplomacy, not in the corpus): command row; `m = 3` gives only the sight.
    assert!(crate::ki::command::is_command(0x97));
    let (t, notes) = run_with(&mut sim, 0x97, &[1, 2, 3]);
    assert!(t.stack.is_empty() && notes.is_empty());
    let issued: Vec<_> = std::mem::take(&mut sim.state.events);
    assert!(matches!(
        issued[..],
        [crate::state::SimEvent::KiCommand {
            player: ME,
            code: 0x63,
            outcome: crate::command::Outcome::Accepted,
        }]
    ));
    // Every world ID is a query or a command; the answer by signature belongs only to the `StubWorld`. An ID with
    // neither is unreachable in the shipped data and is caught: clear the stack, `Guarded`, no command (test
    // `unknown_world_id_is_guarded_without_a_command`).
    assert_eq!(checksum(&state), before);
    assert!(state.events.is_empty());
}

#[test]
fn unknown_world_id_is_guarded_without_a_command() {
    // Neither query nor command (here the non-world ID 0x3FF, which also has no signature, and 0x1 as a local VM
    // builtin) -> clear the stack, `Guarded`, no command, state unchanged.
    let (rules, mut state) = scene();
    let before = checksum(&state);
    let mut sim = Sim::new(&mut state, &rules);
    for id in [0x3FF, 0x1, 0x0] {
        assert!(
            !is_query(id) && !crate::ki::command::is_command(id),
            "{id:#x}"
        );
        let (t, notes) = run_with(&mut sim, id, &[FLOOR, 1, 2, 3]);
        assert!(t.stack.is_empty() && t.pause_ms == 0, "{id:#x}");
        assert!(
            matches!(notes[..], [KiNote::Guarded { builtin, what: "neither query nor command", .. }] if builtin == id),
            "{id:#x}: {notes:?}"
        );
    }
    assert!(sim.state.events.is_empty());
    assert_eq!(checksum(&state), before);
}

#[test]
fn player_queries_leave_the_checksum_unchanged() {
    let (rules, mut state) = scene();
    state.mode = -1;
    let (before, events) = (checksum(&state), state.events.clone());
    let mut sim = Sim::new(&mut state, &rules);
    let hostile = [0, 1, 7, 8, 9, 31, 1000, -1, i32::MIN, i32::MAX];
    for id in [
        0x7F, 0x80, 0x81, 0x83, 0x84, 0x85, 0x86, 0x8A, 0x8B, 0x8C, 0x8F, 0x92, 0x93, 0x96, 0x98,
        0x99, 0xA4, 0xAB, 0xAC, 0xB5, 0xB9, 0xCD, 0xD0, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xE6, 0x108,
        0x116, 0x120, 0x12A, 0x12B, 0x133, 0x134, 0x135, 0x144, 0x177, 0x190, 0x196, 0x1BC, 0x1BD,
        0x1BE, 0x23B,
    ] {
        assert!(is_query(id), "{id:#x}");
        for &v in &hostile {
            let _ = run_with(&mut sim, id, &[v, v, v, v, v, v]);
            let _ = run_with(&mut sim, id, &[]);
        }
    }
    for code in CODES {
        for &v in &hostile {
            let _ = query_once(&mut sim, ME, Sight::Omniscient, code, &[v, v, v, v, v]);
            let _ = query_once(&mut sim, 0, Sight::Fog, code, &[i32::from(ME), v, v, v]);
        }
    }
    assert_eq!(checksum(&state), before);
    assert_eq!(state.events, events);
}
