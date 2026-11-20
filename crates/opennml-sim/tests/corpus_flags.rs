//! Domination and King of the Hill with the maps of the installation (re/domination F.1 to F.5 and
//! F.9): start flags, capture, victory after the time limit, loading with a running timer and the
//! color white.

use std::sync::OnceLock;

use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_sim::command::{Command, CommandKind, NO_SPREAD};
use opennml_sim::economy::message::flags::QUEUE;
use opennml_sim::id::ObjectId;
use opennml_sim::ki::query::query_once;
use opennml_sim::module::natur::adjust_flag_count;
use opennml_sim::motion::NO_DIRECTION;
use opennml_sim::object::GameObject;
use opennml_sim::player::Sight;
use opennml_sim::rules::Rules;
use opennml_sim::save::{SaveGame, checksum};
use opennml_sim::setup::{
    MatchSetup, SlotKind, TIME_LIMIT_COUNTDOWN, ki_kind_handicap, new_match, start_package,
};
use opennml_sim::sim::Sim;
use opennml_sim::state::{SimEvent, SimState};
use opennml_sim::tick::step;
use opennml_sim::world::destroy_object;
use opennml_world::level::load_level;

const BATTLE_BEACH: &str = "data/levels/multiplayer/[2players]-battlebeach.lvl";
const DONUT_ISLE: &str = "data/levels/multiplayer/[4players]-donut-isle.lvl";
const VULCANO_ISLAND: &str = "data/levels/multiplayer/[2players]-vulcanoisland.lvl";

/// `Spieler_Fahne`.
const FLAG: u32 = 4300;

/// Step (counter value before) in which the start flags on `BattleBeach` are checked for the first
/// time: measured, ten nature ticks of two steps each from the first step.
const FIRST_CAPTURE_STEP: u64 = 19;

fn rules_for(level: &str) -> Option<Rules> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, level).unwrap();
    Some(Rules::load(&vfs, &scene).unwrap_or_else(|e| panic!("{level}: {e:#}")))
}

/// Is the installation the German edition (reference of the measured numbers)?
fn is_german() -> bool {
    let dir = corpus_game_dir().expect("corpus present");
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    game_version(&vfs).is_ok_and(|v| v == GameVersion::German)
}

fn setup_for(rules: &Rules, mode: i32) -> MatchSetup {
    MatchSetup {
        mode,
        ..MatchSetup::default_for(rules)
    }
}

/// All flags in table order.
fn flags(state: &SimState) -> Vec<&GameObject> {
    state.objects().filter(|o| o.guid == FLAG).collect()
}

/// Colors of the flags, sorted ascending.
fn flag_colors(state: &SimState) -> Vec<i32> {
    let mut colors: Vec<i32> = flags(state).iter().map(|o| o.color).collect();
    colors.sort_unstable();
    colors
}

/// Messages to players as (receiver, message).
fn messages(events: &[SimEvent]) -> Vec<(u8, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::PlayerMessage {
                to: Some(to), id, ..
            } => Some((*to, *id)),
            _ => None,
        })
        .collect()
}

/// Results of the players as (step before the call, slot, result).
fn results(events: &[SimEvent], step_before: u64) -> Vec<(u64, u8, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::PlayerResult { slot, result } => Some((step_before, *slot, *result)),
            _ => None,
        })
        .collect()
}

/// Runs steps without commands until `done` becomes true (at most `max`); returns all events with
/// the step before.
fn run_until(
    state: &mut SimState,
    rules: &Rules,
    max: usize,
    mut done: impl FnMut(&SimState) -> bool,
) -> Vec<(u64, SimEvent)> {
    let mut out = Vec::new();
    for _ in 0..max {
        let before = state.steps;
        for e in step(state, rules, &[]) {
            out.push((before, e));
        }
        if done(state) {
            break;
        }
    }
    out
}

fn move_command(player: u8, units: Vec<ObjectId>, x: i32, z: i32) -> Command {
    Command {
        player,
        kind: CommandKind::Move {
            units,
            x,
            z,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        },
    }
}

/// D-293: Domination on `BattleBeach` begins with two neutral flags and two capture messages per
/// slot.
// QUIRK D-293: kept
#[test]
fn battle_beach_start_flags_turn_to_the_players() {
    let Some(rules) = rules_for(BATTLE_BEACH) else {
        return;
    };
    let mut state = new_match(&rules, &setup_for(&rules, 2)).unwrap();
    // Start: two flags in slot 8 in the neutral color 9, both count for slot 8.
    let start = flags(&state);
    assert_eq!(start.len(), 2);
    for f in &start {
        assert_eq!((f.owner(), f.color), (8, 9));
    }
    let tiles: Vec<(i32, i32)> = start.iter().map(|f| f.tile).collect();
    assert_eq!(state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 2]);
    assert_eq!(state.flag_leader, 8);
    assert!(state.countdowns.is_empty());
    // First check after ten nature ticks: the next start unit takes each flag.
    let events = run_until(&mut state, &rules, 100, |s| s.flag_counts[8] == 0);
    let sent: Vec<&(u64, SimEvent)> = events
        .iter()
        .filter(|(_, e)| matches!(e, SimEvent::PlayerMessage { .. }))
        .collect();
    assert_eq!(sent.len(), 16, "two flags, eight receivers");
    for (at, e) in sent {
        assert_eq!(*at, FIRST_CAPTURE_STEP);
        let SimEvent::PlayerMessage { tile, flags, .. } = e else {
            unreachable!()
        };
        assert!(tiles.contains(tile), "location is the flag tile");
        assert_eq!(*flags, QUEUE);
    }
    let events: Vec<SimEvent> = events.into_iter().map(|(_, e)| e).collect();
    let all = messages(&events);
    for q in 0..8u8 {
        let mut got: Vec<i32> = all.iter().filter(|m| m.0 == q).map(|m| m.1).collect();
        got.sort_unstable();
        // 228 "You have captured a flag!" for one's own, 220 + p "[PLAYERNAME p] has captured a flag!".
        let wanted = match q {
            0 => [221, 228],
            1 => [220, 228],
            _ => [220, 221],
        };
        assert_eq!(got, wanted, "slot {q}");
    }
    // After the first check: colors = player colors, counters [1, 1, 0 ...], leader still 8.
    assert_eq!(flag_colors(&state), [0, 1]);
    assert_eq!(
        flags(&state)[0].color,
        0,
        "the first flag belongs to slot 0"
    );
    assert_eq!(state.flag_counts, [1, 1, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(state.flag_leader, 8);
    assert!(state.countdowns.is_empty());
    // The flags stay in slot 8 (only the color changes).
    assert!(flags(&state).iter().all(|f| f.owner() == 8));
}

// QUIRK D-293: kept
#[test]
fn donut_isle_has_four_flags_and_needs_three() {
    let Some(rules) = rules_for(DONUT_ISLE) else {
        return;
    };
    let setup = setup_for(&rules, 2);
    assert_eq!(setup.lobby.len(), 4);
    let mut state = new_match(&rules, &setup).unwrap();
    assert_eq!(flags(&state).len(), 4);
    assert_eq!(state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 4]);
    run_until(&mut state, &rules, 100, |s| s.flag_counts[8] == 0);
    assert_eq!(state.flag_counts, [1, 1, 1, 1, 0, 0, 0, 0, 0]);
    assert_eq!(flag_colors(&state), [0, 1, 2, 3]);
    assert_eq!(state.flag_leader, 8);
    assert!(state.countdowns.is_empty());
    // Sum 4, half 2: a slot with two flags does not lead, with three it does (D-295). The counters
    // go through the counting function of the capture (`GameWorld_AdjustFlagCount`).
    let mut sim = Sim::new(&mut state, &rules);
    adjust_flag_count(&mut sim, 1, false);
    adjust_flag_count(&mut sim, 0, true);
    assert_eq!(sim.state.flag_counts[..4], [2, 0, 1, 1]);
    assert_eq!((sim.state.flag_leader, sim.state.countdowns.len()), (8, 0));
    adjust_flag_count(&mut sim, 2, false);
    adjust_flag_count(&mut sim, 0, true);
    assert_eq!(sim.state.flag_counts[..4], [3, 0, 0, 1]);
    assert_eq!(sim.state.flag_leader, 0);
    // Default 20 minutes: `TimeLimit * 60 * 1000` ms.
    assert_eq!(sim.state.countdowns[&TIME_LIMIT_COUNTDOWN], 1_200_000);
    // The same leader with the fourth flag keeps the remaining time.
    step(&mut state, &rules, &[]);
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 1_199_950);
    let mut sim = Sim::new(&mut state, &rules);
    adjust_flag_count(&mut sim, 3, false);
    adjust_flag_count(&mut sim, 0, true);
    assert_eq!(sim.state.flag_counts[..4], [4, 0, 0, 0]);
    assert_eq!(sim.state.flag_leader, 0);
    assert_eq!(sim.state.countdowns[&TIME_LIMIT_COUNTDOWN], 1_199_950);
}

/// Savegame of `BattleBeach` (Domination, `TimeLimit 1`) in the step in which slot 1 takes the flag
/// of slot 0: slot 0 sends its units away (otherwise they would hold the flag), a unit of slot 1
/// walks there. Built once, cloned by several tests.
struct Scenario {
    rules: Rules,
    setup: MatchSetup,
    /// State right after the step in which the leader became 1.
    state: SimState,
    /// Step in which that happened (counter value before).
    t0: u64,
}

fn flag_scenario() -> Option<&'static Scenario> {
    static SCENARIO: OnceLock<Option<Scenario>> = OnceLock::new();
    SCENARIO.get_or_init(|| build_scenario(false)).as_ref()
}

/// Builds the scenario; `wild` leaves the animals of the map standing. Wild animals (slot 8) take
/// flags like opponents (D-294): an animal on `BattleBeach` walks to the flag after about 700
/// steps, kills the runner and colors it with 8. The scenario for the time limit only measures the
/// timer, so the animals disappear there.
fn build_scenario(wild: bool) -> Option<Scenario> {
    let rules = rules_for(BATTLE_BEACH)?;
    let mut setup = setup_for(&rules, 2);
    setup.time_limit = 1;
    let mut state = new_match(&rules, &setup).unwrap();
    run_until(&mut state, &rules, 100, |s| s.flag_counts[8] == 0);
    assert_eq!(state.flag_counts[..2], [1, 1]);
    if !wild {
        let animals: Vec<ObjectId> = state
            .objects()
            .filter(|o| o.owner() == 8 && o.has_module(0, false))
            .map(|o| o.id)
            .collect();
        assert_eq!(animals.len(), 2, "BattleBeach has two animals");
        let mut sim = Sim::new(&mut state, &rules);
        for id in animals {
            destroy_object(&mut sim, id);
        }
    }
    let flag = flags(&state)[0].grid;
    let leaving: Vec<ObjectId> = state
        .objects()
        .filter(|o| o.owner() == 0 && o.has_module(0, false))
        .map(|o| o.id)
        .collect();
    let runner = state
        .objects()
        .find(|o| o.owner() == 1 && o.has_module(0, false))
        .unwrap()
        .id;
    let commands = [
        // 14 tiles south of the flag, far outside the four tiles.
        move_command(0, leaving, flag.x.0, flag.z.0 + (14 << 12)),
        // Two tiles east of the flag of slot 0.
        move_command(1, vec![runner], flag.x.0 + (2 << 12), flag.z.0),
    ];
    step(&mut state, &rules, &commands);
    for _ in 0..6000 {
        let before = state.steps;
        step(&mut state, &rules, &[]);
        if state.flag_leader == 1 {
            return Some(Scenario {
                rules,
                setup,
                state,
                t0: before,
            });
        }
    }
    panic!("slot 1 did not take the flag");
}

/// Finding (D-294 in practice): if the animals of `BattleBeach` are left standing, one takes the
/// flag of slot 1 before the time limit expires - the leader falls to 8, the timer disappears, and
/// nobody wins by the flag.
// QUIRK D-294: kept
#[test]
fn wild_animal_takes_the_flag_before_the_time_limit() {
    let Some(scenario) = build_scenario(true) else {
        return;
    };
    let mut state = scenario.state;
    let mut taken = None;
    for n in 1..=1300u64 {
        let before = state.steps;
        let events = step(&mut state, &scenario.rules, &[]);
        assert!(results(&events, before).is_empty(), "step +{n}");
        if state.flag_leader == 8 {
            taken = Some(n);
            break;
        }
    }
    let n = taken.expect("no animal took the flag");
    // Measured on the German edition with the patch: 700 steps after `t0`, well before the victory
    // step 1180; the English edition was not measured.
    assert!(n < 1180, "before the victory step, measured +{n}");
    if is_german() {
        assert_eq!(n, 700);
    }
    assert!(state.countdowns.is_empty());
    assert_eq!(flag_colors(&state), [1, 8]);
    assert_eq!(state.flag_counts[..2], [0, 1]);
    assert_eq!(state.flag_counts[8], 1);
}

/// The victory by flag comes after the time limit. The timer is created in step `t0` (nature tick
/// of the capture) and `tick_countdowns` already subtracts 50 ms in the same step
/// (`GameEngine_GameTick`: world tick, then timer list, then `CheckVictoryDefeat`). After step
/// `t0 + k` the remaining time is `60 000 - 50 * (k + 1)`; `CHECKTIMER 0` is true from 999 ms
/// (`ms / 1000 < 1`), first at `k = 1180` (remainder 950 ms; at `k = 1179` it is 1000 ms). A
/// formula that does not count the creation step (`60 000 - 50 * k <= 999`) would give 1181.
#[test]
fn flag_victory_after_the_time_limit() {
    let Some(scenario) = flag_scenario() else {
        return;
    };
    let (rules, t0) = (&scenario.rules, scenario.t0);
    let mut state = scenario.state.clone();
    assert_eq!(state.steps, t0 + 1);
    // Slot 1 leads, the flag has its color, counters [0, 2], timer 0 after the 50 ms subtraction.
    assert_eq!((state.flag_leader, flag_colors(&state)), (1, vec![1, 1]));
    assert_eq!(state.flag_counts[..2], [0, 2]);
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 60_000 - 50);
    let k = (0..).find(|k| 60_000 - 50 * (k + 1) <= 999).unwrap();
    assert_eq!(k, 1180);
    let mut result = Vec::new();
    for n in 1..=1200u64 {
        let before = state.steps;
        let events = step(&mut state, rules, &[]);
        result = results(&events, before);
        if !result.is_empty() {
            // Measured: the victory falls in step `t0 + k`, remainder 950 ms.
            assert_eq!((before, n), (t0 + k, k), "victory step");
            assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 950);
            break;
        }
        assert!(
            state.countdowns[&TIME_LIMIT_COUNTDOWN] >= 1000,
            "step {n}: no result, but the remainder is below 1000 ms"
        );
    }
    // Victory (result 2) for slot 1 and defeat (1) for slot 0 in the same step.
    result.sort_unstable_by_key(|r| r.1);
    assert_eq!(result, [(t0 + k, 0, 1), (t0 + k, 1, 2)]);
    assert_eq!(state.flag_leader, 1);
}

/// Saving with a running leader timer, loading and continuing gives the same checksum sequence and
/// the same victory step as without loading.
#[test]
fn flag_victory_comes_at_the_same_step_after_load() {
    let Some(scenario) = flag_scenario() else {
        return;
    };
    let rules = &scenario.rules;
    let mut original = scenario.state.clone();
    for _ in 0..600 {
        step(&mut original, rules, &[]);
    }
    assert_eq!(
        original.countdowns[&TIME_LIMIT_COUNTDOWN],
        60_000 - 50 * 601
    );
    let save = SaveGame {
        start: start_package(rules, &scenario.setup),
        state: original.clone(),
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    loaded
        .check(rules, opennml_sim::save::RulesCompare::Full)
        .unwrap();
    let mut loaded = loaded.state;
    assert_eq!(checksum(&loaded), checksum(&original));
    let mut won: [Vec<(u64, u8, i32)>; 2] = [Vec::new(), Vec::new()];
    for n in 0..700 {
        for (state, won) in [&mut original, &mut loaded].into_iter().zip(&mut won) {
            let before = state.steps;
            let events = step(state, rules, &[]);
            won.extend(results(&events, before));
        }
        if n % 50 == 0 {
            assert_eq!(checksum(&loaded), checksum(&original), "step +{n}");
        }
    }
    assert_eq!(checksum(&loaded), checksum(&original));
    assert_eq!(won[0].len(), 2, "victory and defeat");
    assert_eq!(won[0], won[1], "same step after loading");
    assert_eq!(won[0][0].0, scenario.t0 + 1180);
}

/// On the King of the Hill map `VulcanoIsland` a neutral flag lies in the center; the first capture
/// makes its holder the leader and starts the timer, the victory follows after the time limit in
/// the same step as in Domination.
#[test]
fn vulcano_island_king_of_the_hill() {
    let Some(rules) = rules_for(VULCANO_ISLAND) else {
        return;
    };
    let mut setup = setup_for(&rules, 3);
    setup.time_limit = 1;
    let mut state = new_match(&rules, &setup).unwrap();
    assert_eq!(flags(&state).len(), 1);
    let flag = flags(&state)[0];
    assert_eq!((flag.owner(), flag.color), (8, 9));
    let flag = flag.grid;
    assert_eq!(state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 1]);
    // Nobody is nearby: even after a few seconds it stays neutral, without a leader.
    run_until(&mut state, &rules, 100, |_| false);
    assert_eq!((state.flag_leader, flag_colors(&state)), (8, vec![9]));
    assert!(state.countdowns.is_empty());
    // A unit of slot 0 walks to the flag (the start points are far away).
    let runner = state
        .objects()
        .find(|o| o.owner() == 0 && o.has_module(0, false))
        .unwrap()
        .id;
    let command = move_command(0, vec![runner], flag.x.0, flag.z.0 - (2 << 12));
    step(&mut state, &rules, &[command]);
    let mut t0 = None;
    for _ in 0..6000 {
        let before = state.steps;
        step(&mut state, &rules, &[]);
        if state.flag_leader != 8 {
            t0 = Some(before);
            break;
        }
    }
    let t0 = t0.expect("the flag was not captured");
    assert_eq!(state.flag_leader, 0);
    assert_eq!(flag_colors(&state), [0]);
    assert_eq!(state.flag_counts, [1, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(state.countdowns[&TIME_LIMIT_COUNTDOWN], 60_000 - 50);
    // Victory for slot 0 after the time limit, defeat for slot 1 (no ally).
    let mut result = Vec::new();
    for _ in 0..1300 {
        let before = state.steps;
        let events = step(&mut state, &rules, &[]);
        result = results(&events, before);
        if !result.is_empty() {
            break;
        }
    }
    result.sort_unstable_by_key(|r| r.1);
    assert_eq!(result, [(t0 + 1180, 0, 2), (t0 + 1180, 1, 1)]);
}

/// Simulation: a white player (lobby color 8) starts the match. Neutral objects have color 8;
/// `GameWorld_GetPlayerColor(8) = 8` and `FUN_00478DE0(8)` return the slot of a white player when
/// there is one, the AI queries `0x511D` and `0x5122` treat them as white (D-298, D-299).
// QUIRK D-298: kept
#[test]
fn white_player_starts_and_neutral_objects_count_as_white() {
    let Some(rules) = rules_for(BATTLE_BEACH) else {
        return;
    };
    let mut setup = setup_for(&rules, 2);
    setup.lobby[1].color = 8;
    let mut state = new_match(&rules, &setup).unwrap();
    assert_eq!((state.players[0].color, state.players[1].color), (0, 8));
    // The match runs: the flag of the white player gets color 8, its counter counts for slot 1.
    run_until(&mut state, &rules, 100, |s| s.flag_counts[8] == 0);
    assert_eq!(flag_colors(&state), [0, 8]);
    assert_eq!(state.flag_counts[..2], [1, 1]);
    let tree = state
        .objects()
        .find(|o| o.owner() == 9)
        .expect("trees of the map")
        .id;
    assert_eq!(state.object(tree, false).unwrap().color, 8);
    let mut sim = Sim::new(&mut state, &rules);
    let ask = |sim: &mut Sim, code: u32, player: i32| {
        query_once(sim, 0, Sight::Omniscient, code, &[player, tree.0 as i32]).0[2]
    };
    // 0x511D: does the object have the color of the slot? White slot 1 and a neutral tree: yes;
    // slot 0 with color 0: no.
    assert_eq!(ask(&mut sim, 0x511D, 1), 1);
    assert_eq!(ask(&mut sim, 0x511D, 0), 0);
    // 0x5122: The first slot with color 8 is the white slot 1; it is its own friend, slot 0 is
    // not.
    assert_eq!(ask(&mut sim, 0x5122, 1), 1);
    assert_eq!(ask(&mut sim, 0x5122, 0), 0);
    // Without a white player the same query applies only to slot >= 8 (color 8, no slot has it).
    sim.state.players[1].color = 1;
    assert_eq!(ask(&mut sim, 0x511D, 1), 0);
    assert_eq!(ask(&mut sim, 0x511D, 8), 1);
}

// ---- AI trial (re/domination F.7) ----

/// Start index of the random sequence: 0 as in the game (`MatchSetup::default`, the menu and the
/// control channel set no other). The choice of the seed changes the capture step (seed 1: 7159,
/// `0x4E4D4C31` as `KI_GOLDEN`: 11,239).
const KI_SEED: u32 = 0;
/// `multi_normal_flaggenverwaltung`, the flag module of the AI in game type 2 and 3
/// (`multi_startplayer` starts it).
const FLAG_MODULE: u32 = 1_023_053;
/// Message "You have captured a flag!".
const CAPTURED_SELF: i32 = 228;

/// `Donut-Isle` in game type 2: slot 0 human without commands, slots 1 to 3 AI (kind 3 as in
/// `corpus_ki_match`, handicap 0), nation = slot, game seed 0.
fn ki_flag_setup(rules: &Rules) -> MatchSetup {
    let mut setup = setup_for(rules, 2);
    setup.seed = KI_SEED;
    assert_eq!(setup.lobby.len(), 4);
    for (i, e) in setup.lobby.iter_mut().enumerate() {
        e.kind = if i == 0 {
            SlotKind::Human
        } else {
            SlotKind::Ai
        };
        e.nation = i as i32;
        e.handicap = if i == 0 {
            0
        } else {
            ki_kind_handicap(3).unwrap()
        };
    }
    setup.ki.enabled = true;
    setup.ki.campaign_difficulty = 1;
    setup
}

/// A capture that one slot takes from another slot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Capture {
    /// Step (counter value before).
    step: u64,
    /// Index of the flag in table order.
    flag: usize,
    /// Tile of the flag.
    tile: (i32, i32),
    /// New holder (0 ... 7; 8 = capturer in slot 8: animal or hired headhunter).
    taker: u8,
    /// Previous holder (0 ... 7; 8 = slot 8 or neutral).
    holder: u8,
    counts: [i32; 9],
    leader: i32,
}

/// Tracks who really holds each flag. The color alone does not tell: a hired headhunter (slot 8)
/// carries the color of its customer and colors the flag with it (D-301); it would then look like
/// the customer's. An animal colors it with its own color 8 (D-294).
struct FlagWatch {
    colors: Vec<i32>,
    holders: Vec<u8>,
}

impl FlagWatch {
    fn new(state: &SimState) -> Self {
        let colors: Vec<i32> = flags(state).iter().map(|f| f.color).collect();
        let holders = vec![8; colors.len()];
        Self { colors, holders }
    }

    /// Evaluates a step: returns the captures in which an AI takes the flag from another slot 0 ... 7.
    /// The capturer is the receiver of message 228 at the flag tile (all eight: a capturer in
    /// slot 8, animal or headhunter).
    fn observe(&mut self, state: &SimState, before: u64, events: &[SimEvent]) -> Vec<Capture> {
        let mut found = Vec::new();
        for (i, f) in flags(state).iter().enumerate() {
            if f.color == self.colors[i] {
                continue;
            }
            self.colors[i] = f.color;
            let mut takers: Vec<u8> = events
                .iter()
                .filter_map(|e| match e {
                    SimEvent::PlayerMessage {
                        to: Some(to),
                        id: CAPTURED_SELF,
                        tile,
                        ..
                    } if *tile == f.tile => Some(*to),
                    _ => None,
                })
                .collect();
            takers.sort_unstable();
            let taker = match takers.as_slice() {
                [one] => *one,
                [..] if takers.len() == 8 => 8,
                other => panic!("step {before}: capturer of flag {i} unclear: {other:?}"),
            };
            let holder = std::mem::replace(&mut self.holders[i], taker);
            if (1..=3).contains(&taker) && holder < 8 && holder != taker {
                found.push(Capture {
                    step: before,
                    flag: i,
                    tile: f.tile,
                    taker,
                    holder,
                    counts: state.flag_counts,
                    leader: state.flag_leader,
                });
            }
        }
        found
    }
}

/// Slots that currently have a thread of the flag module.
fn flag_module_slots(state: &SimState) -> Vec<u8> {
    (0..8u8)
        .filter(|&s| {
            state.ki.engines[usize::from(s)]
                .as_ref()
                .is_some_and(|e| e.threads.iter().any(|(_, t)| t.module == FLAG_MODULE))
        })
        .collect()
}

/// Step (counter value before) of the first foreign flag that an AI takes on `Donut-Isle` -
/// measured on the German edition with the patch, game seed 0, debug build about 40 s: slot 1 takes
/// the start flag of slot 0 (tile 107/19), which gets no commands; its units are no longer there.
/// Until then every slot holds its start flag (the test checks it in every step). With a different
/// seed the step changes (seed 1: 7159, `0x4E4D4C31`: 11,239).
const KI_FIRST_FOREIGN_STEP: u64 = 11_799;

/// Steps of the repeat trial: until shortly after this capture.
const KI_REPEAT_STEPS: u64 = KI_FIRST_FOREIGN_STEP + 61;

/// The AI takes a foreign flag in Domination. Slot 0 (human without commands) and slots 1 to 3
/// (AI, kind 3) begin with four neutral start flags; `multi_startplayer` starts the module
/// `multi_normal_flaggenverwaltung` (1023053) for each AI, which sends units to the flags. The test
/// records the measured step of the first capture of a foreign flag, like golden checksums.
// QUIRK D-301: kept
#[test]
fn ki_takes_a_foreign_flag_on_donut_isle() {
    let Some(rules) = rules_for(DONUT_ISLE) else {
        return;
    };
    let setup = ki_flag_setup(&rules);
    let mut state = new_match(&rules, &setup).unwrap();
    // Setup: four neutral start flags, slots 1 to 3 are AI with threads, slot 0 has none.
    assert_eq!(flags(&state).len(), 4);
    assert_eq!(state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 4]);
    for s in 0..4usize {
        let threads = state.ki.engines[s].as_ref().map_or(0, |e| e.threads.len());
        assert_eq!(threads > 0, s > 0, "threads of slot {s}");
    }
    if !is_german() {
        // The numbers are measured on the German edition with the patch; here only setup and start-up.
        run_until(&mut state, &rules, 100, |_| false);
        assert_eq!(state.flag_counts[..4], [1, 1, 1, 1]);
        return;
    }
    let mut watch = FlagWatch::new(&state);
    let mut first = None;
    for _ in 0..=KI_FIRST_FOREIGN_STEP {
        let before = state.steps;
        let events = step(&mut state, &rules, &[]);
        let found = watch.observe(&state, before, &events);
        if let Some(c) = found.into_iter().next() {
            first = Some((c, messages(&events)));
            break;
        }
        // After the start flags (step 19) every slot carries its own color until the capture.
        if state.steps > 20 {
            assert_eq!(flag_colors(&state), [0, 1, 2, 3], "step {before}");
        }
    }
    let (c, sent) = first.expect("no AI has taken a foreign flag by the measured step");
    assert_eq!(
        c,
        Capture {
            step: KI_FIRST_FOREIGN_STEP,
            flag: 2,
            tile: (107, 19),
            taker: 1,
            holder: 0,
            counts: [0, 2, 1, 1, 0, 0, 0, 0, 0],
            leader: 8,
        }
    );
    // Flag 2 was the start flag of slot 0 and now has the color of slot 1; the others are
    // unchanged, nobody leads, no timer is running.
    assert_eq!(flag_colors(&state), [1, 1, 2, 3]);
    assert!(state.countdowns.is_empty());
    // 228 "You have captured a flag!" for the capturer slot 1, 220 + 1 for all others.
    let mut expected: Vec<(u8, i32)> = (0..8u8).map(|q| (q, 221)).collect();
    expected[1].1 = CAPTURED_SELF;
    assert_eq!(sent, expected);
    // The flag module runs for all three AIs.
    assert_eq!(flag_module_slots(&state), [1, 2, 3]);
}

/// Two runs of the AI trial until shortly after the capture give the same checksum: compared every
/// 500 steps and at the end (the number of steps is not a multiple of 500, the final comparison is
/// the important one), plus the same capture.
#[test]
fn ki_flag_run_is_bitwise_repeatable() {
    let Some(rules) = rules_for(DONUT_ISLE) else {
        return;
    };
    let setup = ki_flag_setup(&rules);
    let steps = if is_german() { KI_REPEAT_STEPS } else { 300 };
    let mut runs = [
        new_match(&rules, &setup).unwrap(),
        new_match(&rules, &setup).unwrap(),
    ];
    let mut watches = [FlagWatch::new(&runs[0]), FlagWatch::new(&runs[1])];
    let mut captures: [Vec<Capture>; 2] = [Vec::new(), Vec::new()];
    for n in 1..=steps {
        for ((state, watch), found) in runs.iter_mut().zip(&mut watches).zip(&mut captures) {
            let before = state.steps;
            let events = step(state, &rules, &[]);
            found.extend(watch.observe(state, before, &events));
        }
        if n % 500 == 0 {
            assert_eq!(checksum(&runs[0]), checksum(&runs[1]), "step {n}");
        }
    }
    assert_eq!(checksum(&runs[0]), checksum(&runs[1]));
    assert_eq!(runs[0].steps, steps);
    assert_eq!(captures[0], captures[1]);
    if is_german() {
        assert_eq!(captures[0].len(), 1);
        assert_eq!(captures[0][0].step, KI_FIRST_FOREIGN_STEP);
    }
}
