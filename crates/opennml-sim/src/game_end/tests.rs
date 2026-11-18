//! Tests of the game end: test world of the fortresses with rules as in the shipped data.

use opennml_formats::gametype_rules::parse_gametype_rules;
use opennml_formats::targets::{parse_target_defines, parse_targets};

use super::*;
use crate::command::{Command, CommandKind, Outcome, execute};
use crate::module::building::fort_testkit::{self as kit, HQ, SOLDIER, spawn};
use crate::rules::Rules;
use crate::save::{SaveGame, checksum};
use crate::setup::{MatchSetup, TIME_LIMIT_COUNTDOWN, new_match, start_package};
use crate::state::{Control, SimState};
use crate::tick::step;

/// Filter 8 (buildings) and 2 (units with walk) like `flowcontroltragets.ini` (re/ingame-menus 6.1).
const FLOWCONTROL: &str = concat!(
    "BEGIN\nBUTTONMSG 8\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\n",
    "MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 2\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\n",
    "MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_END\nTARGET_MM\nMM_END\nEND\n",
);

/// Defeat game type 0 like `gametypedefeat.ini`: no buildings.
const NO_BUILDINGS: &str = "AND\nMATCHLOWERTHAN 8 1\n/AND\n";
/// Victory game type 0 like `gametypevictory.ini`: no enemy left in the game.
const NO_ENEMY: &str = "OR\nCHECKENEMY\n/OR\n";
/// Empty root (OR without children: false).
const NEVER: &str = "OR\n/OR\n";
/// Nation 0 (`WID`, default of `Player::new`) has the hero 2720.
const HERO: u32 = 2720;

/// Rule file with `body` as root for all six nations of the game type `gametype`.
fn rule_file(gametype: i32, body: &str) -> String {
    let mut s = format!("GAMETYPE {gametype}\n");
    for nation in ["WID", "PRI", "SPA", "SID", "ENG", "PAT"] {
        s += &format!("NATION {nation}\n{body}/NATION\n");
    }
    s + "/GAMETYPE\n"
}

/// Test world of the fortresses (64 x 64, fully uncovered) with the hero 2720 (copy of the soldier), the
/// filters 8 and 2 and the rules `defeat`/`victory` for the game type `mode` (campaign: game type 0);
/// slots 0 ... `players − 1` active, human, in the game and enemies of each other.
fn world(mode: i32, players: usize, defeat: &str, victory: &str) -> (Rules, SimState) {
    let types = kit::types();
    let soldier = &types[types.find("GUID 1200").unwrap()..];
    let hero = soldier.replacen("GUID 1200\nNAME SPA_Soldat", "GUID 2720\nNAME WID_Held", 1);
    let mut rules = kit::rules_from(&format!("{types}{hero}"));
    let soldier_defaults = rules.defaults[&SOLDIER].clone();
    rules.defaults.insert(HERO, soldier_defaults);
    let defines = parse_target_defines(crate::command::testkit::DEFINES);
    rules.flowcontrol_targets = parse_targets(FLOWCONTROL, &defines).entries;
    let gametype = mode.max(0);
    rules.defeat = parse_gametype_rules(&rule_file(gametype, defeat));
    rules.victory = parse_gametype_rules(&rule_file(gametype, victory));
    let mut state = kit::state(&rules);
    state.mode = mode;
    for p in &mut state.players[..players] {
        p.slot_active = true;
        p.control = Control::Human;
        p.result = 0;
    }
    (rules, state)
}

fn results(state: &SimState) -> Vec<i32> {
    state.players.iter().map(|p| p.result).collect()
}

fn run(state: &mut SimState, rules: &Rules, n: usize) -> Vec<SimEvent> {
    (0..n).flat_map(|_| step(state, rules, &[])).collect()
}

fn player_results(events: &[SimEvent]) -> Vec<(u8, i32)> {
    events
        .iter()
        .filter_map(|e| match *e {
            SimEvent::PlayerResult { slot, result } => Some((slot, result)),
            _ => None,
        })
        .collect()
}

fn messages(events: &[SimEvent]) -> Vec<(u8, i32)> {
    events
        .iter()
        .filter_map(|e| match *e {
            SimEvent::PlayerMessage {
                to: Some(player),
                id,
                tile,
                flags,
            } => {
                assert_eq!(tile, (-1, -1));
                // `GameEngine_SetPlayerResult` `0x00430CE0`: priority 4, enqueue.
                assert_eq!(flags, 0x0401_0000);
                Some((player, id))
            }
            _ => None,
        })
        .collect()
}

/// Message `id` to all slots 0 ... 7 except `except`.
fn to_others(except: u8, id: i32) -> Vec<(u8, i32)> {
    (0..8u8).filter(|&q| q != except).map(|q| (q, id)).collect()
}

/// One headquarters per slot (slot `p` on tile `(10 + 10p, 10)`), except for the slots in
/// `without`.
fn headquarters(state: &mut SimState, rules: &Rules, players: u8, without: &[u8]) -> Vec<ObjectId> {
    let mut sim = Sim::new(state, rules);
    (0..players)
        .filter(|p| !without.contains(p))
        .map(|p| spawn(&mut sim, HQ, i32::from(p), (10 + 10 * i32::from(p), 10)))
        .collect()
}

// QUIRK K-5: kept
#[test]
fn no_check_before_one_second() {
    let (rules, mut state) = world(0, 2, NO_BUILDINGS, NO_ENEMY);
    headquarters(&mut state, &rules, 2, &[1]);
    // `+0x2CC8 += 50`, the check does not trigger until `> 999`: the 19th step does not check, the 20th does.
    let events = run(&mut state, &rules, 19);
    assert!(player_results(&events).is_empty());
    assert_eq!(results(&state)[..2], [0, 0]);
    let events = step(&mut state, &rules, &[]);
    assert_eq!(player_results(&events), [(1, DEFEAT), (0, VICTORY)]);
    assert_eq!(state.steps, 20);
}

#[test]
fn game_end_messages_carry_priority_4() {
    // `GameEngine_SetPlayerResult` `0x00430CE0`: flag `0x04010000`, to every slot except the
    // decided one, without a tile; the helper `messages` checks flag and location.
    let (rules, mut state) = world(0, 3, NO_BUILDINGS, NO_ENEMY);
    let hq = headquarters(&mut state, &rules, 3, &[]);
    assert!(player_results(&run(&mut state, &rules, 25)).is_empty());
    {
        let mut sim = Sim::new(&mut state, &rules);
        crate::world::destroy_object(&mut sim, hq[1]);
    }
    let events = step(&mut state, &rules, &[]);
    let flags: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::PlayerMessage { flags, .. } => Some(*flags),
            _ => None,
        })
        .collect();
    assert_eq!(flags, vec![crate::economy::message::flags::GAME_END; 7]);
    assert_eq!(flags[0], 0x0401_0000);
    assert_eq!(messages(&events), to_others(1, 201));
}

// QUIRK K-5: kept
#[test]
fn normal_mode_defeat_without_buildings() {
    let (rules, mut state) = world(0, 3, NO_BUILDINGS, NO_ENEMY);
    let hq = headquarters(&mut state, &rules, 3, &[]);
    assert!(player_results(&run(&mut state, &rules, 25)).is_empty());
    // Slot 1 loses its only building; units do not count in game type 0.
    {
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, SOLDIER, 1, (30, 30));
        crate::world::destroy_object(&mut sim, hq[1]);
    }
    let events = step(&mut state, &rules, &[]);
    assert_eq!(player_results(&events), [(1, DEFEAT)]);
    assert_eq!(results(&state)[..3], [0, 1, 0]);
    // Message 200 + slot to all other slots (free play, mode >= 0).
    assert_eq!(messages(&events), to_others(1, 201));
    // A decided slot is not checked again; the counter stays above 999 (K-5).
    assert!(player_results(&run(&mut state, &rules, 5)).is_empty());
}

#[test]
fn victory_when_no_enemy_is_left() {
    let (rules, mut state) = world(0, 3, NO_BUILDINGS, NO_ENEMY);
    headquarters(&mut state, &rules, 3, &[]);
    // Slot 2 is not an enemy of slot 0 in either direction: it does not stop the victory of 0.
    state.diplomacy.set_enemy(0, 2, false);
    state.diplomacy.set_enemy(2, 0, false);
    // Slot 1 is an enemy of slot 0 in one direction only: that suffices (`FUN_0042E900`).
    state.diplomacy.set_enemy(0, 1, false);
    assert!(player_results(&run(&mut state, &rules, 25)).is_empty());
    {
        let mut sim = Sim::new(&mut state, &rules);
        set_player_result(&mut sim, 1, DEFEAT);
        sim.state.events.clear();
    }
    let events = step(&mut state, &rules, &[]);
    // Slot 0 wins; slot 2 is not a mutual friend and loses beforehand as well.
    assert_eq!(player_results(&events), [(2, DEFEAT), (0, VICTORY)]);
    assert_eq!(
        messages(&events),
        [to_others(2, 202), to_others(0, 210)].concat()
    );
    assert_eq!(results(&state)[..3], [2, 1, 1]);
}

#[test]
fn defeat_runs_before_victory_in_slot_order() {
    let (rules, mut state) = world(0, 3, NO_BUILDINGS, NO_ENEMY);
    // Slots 0 and 2 without buildings, slot 1 with: first 0 and 2 lose (slot order), then 1 wins
    // in the same step - with the victory first, 1 would still have had enemies in the game.
    headquarters(&mut state, &rules, 3, &[0, 2]);
    let events = run(&mut state, &rules, 20);
    assert_eq!(
        player_results(&events),
        [(0, DEFEAT), (2, DEFEAT), (1, VICTORY)]
    );
}

// QUIRK O-181: kept
#[test]
fn allies_win_together_and_others_lose() {
    // Slots 0 and 1 are mutual friends without enmity; slot 2 is the enemy of both and has
    // no buildings. Slot 3 is nobody's enemy, slot 0 calls it a friend one-sidedly.
    let (rules, mut state) = world(0, 4, NO_BUILDINGS, NO_ENEMY);
    headquarters(&mut state, &rules, 4, &[2]);
    let d = &mut state.diplomacy;
    for (p, q) in [(0, 1), (1, 0)] {
        d.set_friend(p, q, true);
        d.set_enemy(p, q, false);
    }
    for q in [0, 1, 2] {
        d.set_enemy(3, q, false);
        d.set_enemy(q, 3, false);
    }
    d.set_friend(0, 3, true);
    let events = run(&mut state, &rules, 20);
    // Defeat of 2; then 0 wins - its victory makes 3 lose (O-181), although 3 would have won itself in
    // the same pass; 1 stays in the game at the victory of 0 and wins afterwards via its
    // own rule.
    assert_eq!(
        player_results(&events),
        [(2, DEFEAT), (3, DEFEAT), (0, VICTORY), (1, VICTORY)]
    );
    assert_eq!(results(&state)[..4], [2, 2, 1, 1]);
}

// QUIRK O-181: kept
#[test]
fn set_player_result_keeps_mutual_friends_and_one_sided_friends_lose() {
    let (rules, mut state) = world(0, 4, NO_BUILDINGS, NO_ENEMY);
    let d = &mut state.diplomacy;
    d.set_friend(0, 1, true);
    d.set_friend(1, 0, true);
    d.set_friend(0, 2, true);
    d.set_friend(3, 0, true);
    let mut sim = Sim::new(&mut state, &rules);
    set_player_result(&mut sim, 0, VICTORY);
    // Mutual friends (1) stay with 0, one-sided ones (2, 3) lose; empty slots stay 3.
    assert_eq!(results(sim.state), [2, 0, 1, 1, 3, 3, 3, 3]);
    let events = std::mem::take(&mut sim.state.events);
    assert_eq!(
        player_results(&events),
        [(2, DEFEAT), (3, DEFEAT), (0, VICTORY)]
    );
}

#[test]
fn campaign_mode_never_checks_rules() {
    // Campaign (mode -1): `GameTypeRules_Evaluate` returns false for every slot, even if the
    // table has rules for game type 0.
    let (rules, mut state) = world(-1, 2, NO_BUILDINGS, NO_ENEMY);
    assert!(player_results(&run(&mut state, &rules, 40)).is_empty());
    assert_eq!(results(&state)[..2], [0, 0]);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.mode = 0;
    assert!(evaluate(&mut sim, &rules.defeat, 0, 1));
    // Nation outside 0 ... 5 and game type 6 (no row in the shipped data): false.
    assert!(!evaluate(&mut sim, &rules.defeat, 6, 1));
    assert!(!evaluate(&mut sim, &rules.defeat, -1, 1));
    sim.state.mode = 6;
    assert!(!evaluate(&mut sim, &rules.defeat, 0, 1));
    // Empty rule table (like `Rules::synthetic`): never true.
    sim.state.mode = 0;
    assert!(!evaluate(&mut sim, &GameTypeRules::default(), 0, 1));
}

#[test]
fn kill_hero_defeat_when_the_hero_dies() {
    // Game type 1 like `gametypedefeat.ini`: hero dead or (no units and no buildings).
    let defeat =
        "OR\nMATCHLOWERTHAN 2720 1\nAND\nMATCHLOWERTHAN 2 1\nMATCHLOWERTHAN 8 1\n/AND\n/OR\n";
    let (rules, mut state) = world(1, 2, defeat, NO_ENEMY);
    headquarters(&mut state, &rules, 2, &[]);
    let heroes: Vec<ObjectId> = {
        let mut sim = Sim::new(&mut state, &rules);
        (0..2)
            .map(|p| {
                spawn(&mut sim, SOLDIER, p, (12 + 10 * p, 14));
                spawn(&mut sim, HERO, p, (14 + 10 * p, 14))
            })
            .collect()
    };
    assert!(player_results(&run(&mut state, &rules, 25)).is_empty());
    {
        let mut sim = Sim::new(&mut state, &rules);
        crate::world::destroy_object(&mut sim, heroes[0]);
    }
    let events = step(&mut state, &rules, &[]);
    // Buildings and units do not save slot 0; slot 1 then has no enemy left.
    assert_eq!(player_results(&events), [(0, DEFEAT), (1, VICTORY)]);
}

// QUIRK O-180: kept
#[test]
fn check_timer_without_timer_is_true() {
    // Game type 4: without an entry in the timer list, `CHECKTIMER 0` is true at once (O-180).
    let defeat = "AND\nCHECKTIMER 0\nCHECKPLAYERID 1\n/AND\n";
    let (rules, mut state) = world(4, 2, defeat, NEVER);
    headquarters(&mut state, &rules, 2, &[]);
    assert!(state.countdowns.is_empty());
    let events = run(&mut state, &rules, 20);
    assert_eq!(player_results(&events), [(1, DEFEAT)]);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(countdown_seconds(&sim, 0), 0);
}

// QUIRK O-201: kept
#[test]
fn countdown_runs_out_and_check_timer_turns_true_one_second_before() {
    let defeat = "AND\nCHECKTIMER 0\nCHECKPLAYERID 1\n/AND\n";
    let (rules, mut state) = world(4, 2, defeat, NEVER);
    headquarters(&mut state, &rules, 2, &[]);
    state.countdowns.insert(TIME_LIMIT_COUNTDOWN, 3000);
    // Per step -50 before the check: after 40 steps 1000 ms (= 1 s, false), after 41 steps
    // 950 ms - `950 / 1000 = 0 < 1`, so true although the timer is still running (O-201).
    assert!(player_results(&run(&mut state, &rules, 40)).is_empty());
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 1000);
    assert_eq!(
        player_results(&step(&mut state, &rules, &[])),
        [(1, DEFEAT)]
    );
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 950);
    // Remaining time 0 stays for one tick, only `−50 < 0` removes the timer.
    run(&mut state, &rules, 19);
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 0);
    step(&mut state, &rules, &[]);
    assert!(state.countdowns.is_empty());
}

#[test]
fn railroad_start_creates_the_time_limit_countdown() {
    // `FUN_004453D0`: game type 4 only, `TimeLimit · 60 · 1000` ms.
    let rules = crate::command::testkit::rules();
    let railroad = MatchSetup {
        mode: 4,
        time_limit: 7,
        ..MatchSetup::default()
    };
    let state = new_match(&rules, &railroad).unwrap();
    assert_eq!(
        state.countdowns.iter().collect::<Vec<_>>(),
        [(&TIME_LIMIT_COUNTDOWN, &420_000)]
    );
    for mode in [-1, 0, 1, 3, 5] {
        let setup = MatchSetup {
            mode,
            time_limit: 7,
            ..MatchSetup::default()
        };
        assert!(
            new_match(&rules, &setup).unwrap().countdowns.is_empty(),
            "{mode}"
        );
    }
    // Default 20 minutes (`Engine.ini`).
    assert_eq!(MatchSetup::default().time_limit, 20);
}

#[test]
fn save_game_carries_the_countdowns() {
    let rules = crate::command::testkit::rules();
    let setup = MatchSetup {
        mode: 4,
        time_limit: 1,
        ..MatchSetup::default()
    };
    let mut state = new_match(&rules, &setup).unwrap();
    run(&mut state, &rules, 3);
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 60_000 - 150);
    // Part of the state and thus of the checksum.
    let mut without = state.clone();
    without.countdowns.clear();
    assert_ne!(checksum(&state), checksum(&without));
    let save = |state: SimState| SaveGame {
        start: start_package(&rules, &setup),
        state,
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save(state.clone()).to_bytes()).unwrap();
    assert_eq!(loaded.state, state);
    loaded.check(&rules).unwrap();
}

#[test]
fn guid_trigger_reads_event_bits() {
    // Game type 5 like `gametypevictory.ini` (`CHECKGUIDTRIGGER 2613 8`, here the soldier 1200).
    let victory = "OR\nCHECKGUIDTRIGGER 1200 8\nCHECKGUIDTRIGGER 7 0\n/OR\n";
    let (rules, mut state) = world(5, 2, NEVER, victory);
    let hq = headquarters(&mut state, &rules, 2, &[]);
    let (first, second) = {
        let mut sim = Sim::new(&mut state, &rules);
        (
            spawn(&mut sim, SOLDIER, 0, (12, 14)),
            spawn(&mut sim, SOLDIER, 0, (13, 14)),
        )
    };
    // Only the first matching object of the own table counts; an object of another type (the
    // headquarters of slot 1) or another bit does not. Filter 7 has no entry: false, even
    // with bit 0 set.
    state.object_event_bits.insert(second, 1 << 8);
    state.object_event_bits.insert(hq[1], 1 << 8);
    state.object_event_bits.insert(first, 1 | 1 << 9);
    assert!(player_results(&run(&mut state, &rules, 25)).is_empty());
    state.object_event_bits.insert(first, 1 << 8);
    let events = step(&mut state, &rules, &[]);
    assert_eq!(player_results(&events), [(1, DEFEAT), (0, VICTORY)]);
    // Bit 40 is bit 8 (`1 << (n & 31)`).
    let mut sim = Sim::new(&mut state, &rules);
    assert!(guid_trigger(&mut sim, 0, 1200, 40));
    assert!(!guid_trigger(&mut sim, 0, 1200, 9));
}

#[test]
fn surrender_command_sets_defeat() {
    let (rules, mut state) = world(0, 2, NO_BUILDINGS, NO_ENEMY);
    headquarters(&mut state, &rules, 2, &[]);
    let command = |player, slot, result| Command {
        player,
        kind: CommandKind::SetPlayerResult { slot, result },
    };
    {
        let mut sim = Sim::new(&mut state, &rules);
        // Invalid: slot 8 or 255, result 0 or 3 - the state stays.
        let before = checksum(sim.state);
        for (slot, result) in [(8, 1), (0, 0), (0, 3), (255, 2)] {
            let outcome = execute(&mut sim, &command(0, slot, result), true);
            assert!(matches!(outcome, Outcome::Rejected(_)), "{slot} {result}");
        }
        assert_eq!(checksum(sim.state), before);
        // Surrender (`0x27` -> `{0x50000056, 2, slot, 1}`): defeat of the own slot.
        let outcome = execute(&mut sim, &command(1, 1, DEFEAT), true);
        assert_eq!(outcome, Outcome::Accepted);
        assert_eq!(results(sim.state)[..2], [0, 1]);
        sim.state.events.clear();
    }
    // Via the tick the command takes effect at the tick boundary; a foreign slot is allowed.
    let mut fresh = world(0, 2, NO_BUILDINGS, NO_ENEMY).1;
    let events = step(&mut fresh, &rules, &[command(1, 0, VICTORY)]);
    assert_eq!(player_results(&events), [(1, DEFEAT), (0, VICTORY)]);
}

#[test]
fn result_is_never_overwritten() {
    let (rules, mut state) = world(0, 2, NO_BUILDINGS, NO_ENEMY);
    {
        let mut sim = Sim::new(&mut state, &rules);
        set_player_result(&mut sim, 1, DEFEAT);
        set_player_result(&mut sim, 1, VICTORY);
        // Empty slot (3) and slots from 8 on stay untouched.
        set_player_result(&mut sim, 5, DEFEAT);
        set_player_result(&mut sim, 8, DEFEAT);
        assert_eq!(results(sim.state), [0, 1, 3, 3, 3, 3, 3, 3]);
        let events = std::mem::take(&mut sim.state.events);
        assert_eq!(player_results(&events), [(1, DEFEAT)]);
    }
    // The rule check overwrites nothing either: slot 1 has no buildings but stays at 1.
    headquarters(&mut state, &rules, 1, &[]);
    let events = run(&mut state, &rules, 25);
    assert_eq!(player_results(&events), [(0, VICTORY)]);
    assert_eq!(results(&state)[..2], [2, 1]);
}

#[test]
fn messages_depend_on_the_mode() {
    // Campaign (-1): defeat without a message, victory with 210 + slot; mode -2 reports nothing.
    let (rules, mut state) = world(-1, 3, NO_BUILDINGS, NO_ENEMY);
    state.diplomacy.set_friend(0, 2, true);
    state.diplomacy.set_friend(2, 0, true);
    let mut sim = Sim::new(&mut state, &rules);
    set_player_result(&mut sim, 1, DEFEAT);
    set_player_result(&mut sim, 0, VICTORY);
    let events = std::mem::take(&mut sim.state.events);
    assert_eq!(messages(&events), to_others(0, 210));
    sim.state.mode = -2;
    set_player_result(&mut sim, 2, VICTORY);
    let events = std::mem::take(&mut sim.state.events);
    assert!(messages(&events).is_empty());
    assert_eq!(player_results(&events), [(2, VICTORY)]);
}

// QUIRK O-180: kept
#[test]
fn check_flag_owner_reads_the_leader() {
    // `FUN_0042E970`: `m_FlagOwner == slot`. Without a leader (8) it holds for nobody.
    let victory = "OR\nCHECKFLAGOWNER\n/OR\n";
    let (rules, mut state) = world(2, 2, NEVER, victory);
    headquarters(&mut state, &rules, 2, &[]);
    assert_eq!(state.flag_leader, crate::flags::NO_LEADER);
    assert!(player_results(&run(&mut state, &rules, 25)).is_empty());
    state.flag_leader = 1;
    let events = step(&mut state, &rules, &[]);
    // The victory of the leader makes the non-friendly slot 0 lose.
    assert_eq!(player_results(&events), [(0, DEFEAT), (1, VICTORY)]);
}

// QUIRK O-180: kept
#[test]
fn defeated_leader_never_wins_by_the_flag() {
    // Slot 0 leads, then loses everything (result 1) and stays leader; its timer
    // runs out, but a defeated slot is no longer checked - no victory via the flag, the match goes
    // on, without a crash.
    let victory = "OR\nCHECKENEMY\nAND\nCHECKFLAGOWNER\nCHECKTIMER 0\n/AND\n/OR\n";
    let (rules, mut state) = world(2, 3, NEVER, victory);
    headquarters(&mut state, &rules, 3, &[]);
    state.time_limit = 3;
    {
        let mut sim = Sim::new(&mut state, &rules);
        crate::module::natur::adjust_flag_count(&mut sim, 0, true);
        crate::module::natur::adjust_flag_count(&mut sim, 0, true);
        assert_eq!(sim.state.flag_leader, 0);
        assert_eq!(sim.state.countdowns[&TIME_LIMIT_COUNTDOWN], 3000);
        set_player_result(&mut sim, 0, DEFEAT);
    }
    assert_eq!(state.players[0].result, DEFEAT);
    // Far beyond the expiry of the timer (3 s = 60 steps).
    let events = run(&mut state, &rules, 100);
    assert!(state.countdowns.is_empty());
    assert_eq!(state.flag_leader, 0);
    // The only result is the defeat of slot 0 itself (event of the call above).
    assert_eq!(player_results(&events), [(0, DEFEAT)]);
    assert_eq!(results(&state)[..3], [DEFEAT, 0, 0]);
}
