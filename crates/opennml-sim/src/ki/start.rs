//! AI start: the start in `new_match` like `FUN_0043D560` and the start API for level scripts (`.abl` `n0`/`n1`) like
//! `FUN_004A51D0`/`FUN_004A5120`.
//! See re/ki-queries 7.5, re/ki-lifecycle 1.4, 2.1.

use super::KiPause;
use super::run::with_ki;
use crate::sim::Sim;
use crate::state::Control;

/// `setschwierigkeitsgrad`: runs synchronously for each AI slot, without arguments.
pub const SET_LEVEL: u32 = 351_152;
/// `multi_startplayer`: thread per AI slot with `{slot, nation, level}` (not in the campaign).
pub const START_PLAYER: u32 = 195_235;
/// Game type "King of the Hill": level fixed at 5 (`0x0043D652`).
const KING_OF_THE_HILL: i32 = 3;
/// Highest level of `multi_startplayer`.
const TOP_LEVEL: i32 = 5;

/// AI start of the new match (`FUN_0043D560` from `0x0043D5C5`): **one** loop over slots 0-7 - control kind not empty
/// -> engine (also human and passive, E-19); control kind AI -> `setschwierigkeitsgrad` synchronously, then, only
/// outside the campaign (mode > -1), `multi_startplayer` as a thread with level 5 in mode 3, otherwise `5 − Handicap`
/// (E-23). The synchronous level of slot p therefore runs before the engines of the higher slots exist; the threads do
/// not run until the next AI tick. The original's replay and network checks are omitted (the simulation is the host; a
/// replay restarts the AI because AI commands are not in the command log). Loading never calls this (engines come from
/// the savegame).
pub fn start_match(sim: &mut Sim) {
    for slot in 0..8u8 {
        let p = &sim.state.players[usize::from(slot)];
        let (control, nation, handicap) = (p.control, p.nation, p.handicap);
        if control == Control::Empty {
            continue;
        }
        sim.state.ki.engine_mut(slot);
        if control != Control::Ai {
            continue;
        }
        let s = i32::from(slot);
        run_sync(sim, s, SET_LEVEL, &[]);
        if sim.state.mode <= -1 {
            continue;
        }
        let level = if sim.state.mode == KING_OF_THE_HILL {
            TOP_LEVEL
        } else {
            TOP_LEVEL.wrapping_sub(handicap)
        };
        spawn_thread(sim, s, START_PLAYER, &[s, nation, level]);
    }
}

/// Engine of the slot if it lies in 0-7 and has an engine (the original's `platz <= 8` is E-21, exception A7).
fn has_engine(sim: &Sim, slot: i32) -> Option<u8> {
    let s = u8::try_from(slot).ok().filter(|&s| s < 8)?;
    sim.state.ki.engines[usize::from(s)].as_ref().map(|_| s)
}

/// Synchronous module start `FUN_004A51D0` (`.abl` `n0`, `setschwierigkeitsgrad`; E-12): `false` only for a slot
/// outside 0-7 or without an engine; an unknown module silently runs into the void and reports `true` (E-16). `0x18C`
/// in the run does not pause the AI until after the run (as it does after the tick, re/ki-vm 12.3 P12).
pub fn run_sync(sim: &mut Sim, slot: i32, module: u32, args: &[i32]) -> bool {
    let Some(slot) = has_engine(sim, slot) else {
        return false;
    };
    let rules = sim.rules;
    with_ki(sim, |ki, world| {
        let run = ki
            .with_engine(slot, &rules.ki, world, |e, cx| e.run_sync(cx, module, args))
            .flatten();
        if run.is_some_and(|r| r.pause) {
            ki.paused = KiPause::Paused;
        }
    });
    true
}

/// Thread start `FUN_004A5120` (`.abl` `n1`, `multi_startplayer`): `true` if the thread is registered; `false` for a
/// slot outside 0-7, without an engine, for an unknown module (E-16) or at the limit `MAX_THREADS`. The thread does not
/// run until the next AI tick.
pub fn spawn_thread(sim: &mut Sim, slot: i32, module: u32, args: &[i32]) -> bool {
    let Some(slot) = has_engine(sim, slot) else {
        return false;
    };
    let rules = sim.rules;
    with_ki(sim, |ki, world| {
        ki.with_engine(slot, &rules.ki, world, |e, cx| e.spawn(cx, module, args))
            .flatten()
            .is_some()
    })
}

/// `.abl` 37 "Halt AI" (`FUN_0042DB40` -> `FUN_004A4AF0`, E-221): deletes the engine of the slot together with its
/// threads (the table entry becomes 0). `true` if an engine was deleted; slot outside 0-7 (the original writes behind
/// the table there, exception A7, the load check of the `.abl` rejects it) or without an engine: nothing. Afterwards
/// [`run_sync`] and [`spawn_thread`] do nothing for the slot.
pub fn delete_engine(sim: &mut Sim, slot: i32) -> bool {
    let Some(slot) = has_engine(sim, slot) else {
        return false;
    };
    sim.state.ki.engines[usize::from(slot)] = None;
    true
}

/// `.abl` 340 "Set AI value" (`FUN_00431640`, E-220): `m_listGlobal[index] = value` of the engine of the slot. `true`
/// if written; slot outside 0-7, without an engine or index outside the list (`0 ≤ index < length`; the original throws
/// an exception for a negative index, exception A7, the load check of the `.abl` rejects it): nothing, and no engine is
/// created.
pub fn set_global(sim: &mut Sim, slot: i32, index: i32, value: i32) -> bool {
    let Some(slot) = has_engine(sim, slot) else {
        return false;
    };
    let Some(engine) = sim.state.ki.engines[usize::from(slot)].as_mut() else {
        return false;
    };
    let entry = usize::try_from(index)
        .ok()
        .and_then(|i| engine.mem.global_list.get_mut(i));
    match entry {
        Some(entry) => {
            *entry = value;
            true
        }
        None => false,
    }
}

/// Matches with AI slots without game files (tests of start and tick).
#[cfg(test)]
pub(crate) mod testkit {
    use opennml_formats::config::LevelPlayer;

    use super::{SET_LEVEL, START_PLAYER};
    use crate::ki::KiSetup;
    use crate::ki::testkit::{catalog, int, proto, word};
    use crate::ki::vm::op;
    use crate::rules::Rules;
    use crate::setup::{LobbyEntry, MatchSetup, SlotKind, new_match};
    use crate::state::SimState;

    /// Sleep that the test `multi_startplayer` sets per round.
    pub(crate) const START_MODULE_PAUSE: i32 = 100_000;
    /// Test module: pauses the AI (`CALLB 0x18C`).
    pub(crate) const PAUSE_MODULE: u32 = 77;
    /// Test module: sets value 500 to 1.
    pub(crate) const MARK_MODULE: u32 = 78;
    /// Test module: eight `PUSHA` in a row (stack depth = executed instructions), then end.
    pub(crate) const COUNT_MODULE: u32 = 79;
    /// Test module `startplayer` of `kamp1-1` (example re/ki-lifecycle 1.3): eleven parameter words, sleeps.
    pub(crate) const ELEVEN_WORD_MODULE: u32 = 55_909;
    /// Nation and handicap of slots 1 and 3.
    pub(crate) const AI_SLOTS: [(u8, i32, i32); 2] = [(1, 4, 2), (3, 2, 0)];

    fn push(v: u32) -> u32 {
        word(op::PUSHA, v)
    }

    fn callb(id: u32) -> u32 {
        word(op::CALLB, id)
    }

    /// `m_iValues[k] = v`.
    fn set_value(k: u32, v: u32) -> [u32; 3] {
        [push(k), push(v), callb(0x1E)]
    }

    /// `m_iValues[k] = value 7 of the engine in slot `slot`` (`0xE6`, -1 without an engine).
    fn probe(k: u32, slot: u32) -> [u32; 5] {
        [push(k), push(slot), push(7), callb(0xE6), callb(0x1E)]
    }

    /// Test `setschwierigkeitsgrad`: value 302 = 1000, value 900 = probe of slot 3, value 901 =
    /// probe of slot 0.
    fn set_level_code() -> Vec<u32> {
        [&set_value(302, 1000)[..], &probe(900, 3), &probe(901, 0)].concat()
    }

    pub(crate) fn ki_rules(campaign: bool) -> Rules {
        let mut r = Rules::synthetic(
            "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\n",
            32,
        );
        r.level_path = "data/levels/multiplayer/test.lvl".into();
        // Multiplayer: slots 0-3 from the lobby; campaign: control kind from the `.lvl`, slot 2 is missing.
        let players: &[(u8, i32, i32)] = if campaign {
            &[(0, 0, 0), (1, 4, 1), (3, 2, 1)]
        } else {
            &[(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0)]
        };
        for &(slot, nation, kind) in players {
            r.level.players.push(LevelPlayer {
                slot,
                name: format!("P{slot}"),
                nation,
                kind,
                resources: Vec::new(),
            });
        }
        let sleep_loop = [word(op::LOAD, int(0)), callb(0x8E), word(op::JMP, 0)];
        r.ki = catalog(vec![
            proto(SET_LEVEL, &[(0, 8)], &[], &set_level_code()),
            proto(
                START_PLAYER,
                &[(0, 8)],
                &[(0, START_MODULE_PAUSE)],
                &sleep_loop,
            ),
            proto(PAUSE_MODULE, &[(0, 8)], &[], &[callb(0x18C)]),
            proto(MARK_MODULE, &[(0, 8)], &[], &set_value(500, 1)),
            proto(COUNT_MODULE, &[(0, 8)], &[], &[push(1); 8]),
            proto(ELEVEN_WORD_MODULE, &[(0, 11)], &[], &sleep_loop),
        ]);
        r
    }

    /// Lobby: slot 0 human, 1 AI, 2 empty, 3 AI ([`AI_SLOTS`]).
    pub(crate) fn ai_setup(mode: i32, ki: KiSetup) -> MatchSetup {
        let entry = |kind, nation, handicap| LobbyEntry {
            kind,
            name: String::new(),
            nation,
            team: 0,
            color: 0,
            handicap,
        };
        let [(_, n1, h1), (_, n3, h3)] = AI_SLOTS;
        MatchSetup {
            mode,
            seed: 7,
            lobby: vec![
                entry(SlotKind::Human, 0, 0),
                entry(SlotKind::Ai, n1, h1),
                entry(SlotKind::Closed, 0, 0),
                entry(SlotKind::Ai, n3, h3),
            ],
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki,
            script: crate::script::ScriptSetup::default(),
            ..MatchSetup::default()
        }
    }

    /// New match after [`ai_setup`].
    pub(crate) fn ai_match(mode: i32, ki: KiSetup) -> (Rules, SimState) {
        let rules = ki_rules(mode < 0);
        let state = new_match(&rules, &ai_setup(mode, ki)).expect("match");
        (rules, state)
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use crate::ki::KiSetup;
    use crate::save::SaveGame;
    use crate::setup::start_package;
    use crate::state::SimState;

    const ON: KiSetup = KiSetup {
        enabled: true,
        campaign_difficulty: 1,
    };

    fn engine(state: &SimState, slot: usize) -> &crate::ki::engine::Engine {
        state.ki.engines[slot].as_ref().expect("Engine")
    }

    // QUIRK E-12, E-19, E-23: kept
    #[test]
    fn start_follows_the_original_loop() {
        let (_, state) = ai_match(0, ON);
        let present: Vec<bool> = state.ki.engines.iter().map(Option::is_some).collect();
        assert_eq!(
            present,
            [true, true, false, true, false, false, false, false]
        );
        // Human: empty engine.
        let human = engine(&state, 0);
        assert_eq!((human.next_thread_id, human.threads.len()), (0, 0));
        assert!(human.mem.values.is_empty());
        for (slot, nation, handicap) in AI_SLOTS {
            let e = engine(&state, usize::from(slot));
            // `setschwierigkeitsgrad` ran synchronously (ID 0, not in the table), then the
            // thread (ID 1) with `[slot, nation, 5 − handicap]`, top level.
            assert_eq!(e.mem.values.get(&302), Some(&1000), "slot {slot}");
            assert_eq!(e.next_thread_id, 2, "slot {slot}");
            let ids: Vec<i32> = e.threads.iter().map(|(id, _)| id).collect();
            assert_eq!(ids, [1], "slot {slot}");
            let t = e.threads.get(1).unwrap();
            assert_eq!(t.module, START_PLAYER);
            assert_eq!(t.stack, [i32::from(slot), nation, 5 - handicap]);
            assert_eq!(t.pc, 0, "does not run until the next tick");
        }
        // One loop: when slot 1 ran synchronously, the engine of slot 3 did not exist yet (-1), the one of
        // slot 0 did (0); slot 3 reads itself (0).
        let probes = |slot| {
            let v = &engine(&state, slot).mem.values;
            (v.get(&900).copied(), v.get(&901).copied())
        };
        assert_eq!(probes(1), (Some(-1), Some(0)));
        assert_eq!(probes(3), (Some(0), Some(0)));
        assert!(state.events.is_empty());
    }

    // QUIRK E-23: kept
    #[test]
    fn king_of_the_hill_starts_with_level_five() {
        let (_, state) = ai_match(3, ON);
        for (slot, nation, _) in AI_SLOTS {
            let t = engine(&state, usize::from(slot)).threads.get(1).unwrap();
            assert_eq!(t.stack, [i32::from(slot), nation, 5], "slot {slot}");
        }
    }

    #[test]
    fn the_campaign_starts_only_the_level_module() {
        // Mode <= -1 only `setschwierigkeitsgrad`, the start module comes from the level script.
        let (_, state) = ai_match(-1, ON);
        let present: Vec<bool> = state.ki.engines.iter().map(Option::is_some).collect();
        assert_eq!(
            present,
            [true, true, false, true, false, false, false, false]
        );
        for slot in [1, 3] {
            let e = engine(&state, slot);
            assert_eq!(e.mem.values.get(&302), Some(&1000), "slot {slot}");
            assert_eq!(e.next_thread_id, 1, "slot {slot}");
            assert!(e.threads.is_empty(), "slot {slot}");
        }
    }

    #[test]
    fn without_ki_no_engine_is_created() {
        for mode in [-1, 0, 3] {
            let (_, state) = ai_match(mode, KiSetup::default());
            assert!(state.ki.engines.iter().all(Option::is_none), "mode {mode}");
        }
    }

    #[test]
    fn loading_does_not_start_the_ki() {
        let (rules, mut state) = ai_match(0, ON);
        for e in state.ki.engines.iter_mut().flatten() {
            e.threads = crate::mfc::MfcMap::new(crate::ki::engine::THREAD_BUCKETS);
        }
        let save = SaveGame {
            start: start_package(&rules, &ai_setup(0, ON)),
            state: state.clone(),
            commands: Vec::new(),
        };
        let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
        loaded
            .check(&rules, crate::save::RulesCompare::Full)
            .unwrap();
        assert_eq!(loaded.state.ki, state.ki);
        assert!(
            loaded
                .state
                .ki
                .engines
                .iter()
                .flatten()
                .all(|e| e.threads.is_empty())
        );
    }

    // QUIRK E-12, E-16: kept
    // QUIRK E-21: deviation
    #[test]
    fn start_api_follows_n0_and_n1() {
        let (rules, mut state) = ai_match(0, ON);
        let mut sim = Sim::new(&mut state, &rules);
        // Slot outside 0-7 and slot without an engine: `false`, nothing created.
        for slot in [8, -1, 2, i32::MAX] {
            assert!(!run_sync(&mut sim, slot, MARK_MODULE, &[]), "slot {slot}");
            assert!(
                !spawn_thread(&mut sim, slot, MARK_MODULE, &[]),
                "slot {slot}"
            );
        }
        assert!(sim.state.ki.engines[2].is_none());
        // Unknown module: `true` synchronously, `false` as a thread, without effect.
        let before = sim.state.ki.clone();
        assert!(run_sync(&mut sim, 0, 4711, &[]));
        assert!(!spawn_thread(&mut sim, 0, 4711, &[]));
        assert_eq!(sim.state.ki, before);
        // Known module: thread or synchronous run on the human slot (E-19).
        assert!(spawn_thread(&mut sim, 0, MARK_MODULE, &[5, 6]));
        let e = sim.state.ki.engines[0].as_ref().unwrap();
        assert_eq!(e.threads.get(0).map(|t| t.stack.clone()), Some(vec![5, 6]));
        assert!(
            !e.mem.values.contains_key(&500),
            "thread does not run until the tick"
        );
        assert!(run_sync(&mut sim, 0, MARK_MODULE, &[]));
        let e = sim.state.ki.engines[0].as_ref().unwrap();
        assert_eq!((e.mem.values.get(&500), e.next_thread_id), (Some(&1), 2));
        assert_eq!(e.threads.len(), 1);
        assert_eq!(sim.state.ki.paused, KiPause::Running);
        // `0x18C` in the synchronous run pauses the AI after the run.
        assert!(run_sync(&mut sim, 3, PAUSE_MODULE, &[]));
        assert_eq!(sim.state.ki.paused, KiPause::Paused);
        assert!(!sim.in_ki_step);
    }

    // QUIRK E-221: kept
    #[test]
    fn delete_engine_removes_the_engine_and_later_starts_do_nothing() {
        let (rules, mut state) = ai_match(0, ON);
        let mut sim = Sim::new(&mut state, &rules);
        let others = (
            sim.state.ki.engines[0].clone(),
            sim.state.ki.engines[3].clone(),
        );
        assert!(delete_engine(&mut sim, 1));
        assert!(sim.state.ki.engines[1].is_none());
        // The remaining engines stay untouched.
        assert_eq!(
            (
                sim.state.ki.engines[0].clone(),
                sim.state.ki.engines[3].clone()
            ),
            others
        );
        // Second deletion, slot without an engine and slot outside 0-7: nothing.
        let before = sim.state.ki.clone();
        for slot in [1, 2, 8, -1, i32::MAX, i32::MIN] {
            assert!(!delete_engine(&mut sim, slot), "slot {slot}");
        }
        assert_eq!(sim.state.ki, before);
        // Starts for the deleted slot create neither an engine nor a thread (E-221).
        assert!(!run_sync(&mut sim, 1, MARK_MODULE, &[]));
        assert!(!spawn_thread(&mut sim, 1, MARK_MODULE, &[]));
        assert_eq!(sim.state.ki, before);
    }

    // QUIRK E-220: deviation
    #[test]
    fn set_global_writes_only_inside_the_list() {
        let (rules, mut state) = ai_match(0, ON);
        state.ki.engines[1].as_mut().unwrap().mem.global_list = vec![5, 7, -3];
        let mut sim = Sim::new(&mut state, &rules);
        assert!(set_global(&mut sim, 1, 0, 11));
        assert!(set_global(&mut sim, 1, 2, i32::MIN));
        let list = |sim: &Sim| {
            sim.state.ki.engines[1]
                .as_ref()
                .unwrap()
                .mem
                .global_list
                .clone()
        };
        assert_eq!(list(&sim), [11, 7, i32::MIN]);
        // Index past the end, negative, slot without an engine, slot outside 0-7: nothing (E-220).
        let before = sim.state.ki.clone();
        for (slot, index) in [(1, 3), (1, -1), (1, i32::MIN), (2, 0), (8, 0), (-1, 0)] {
            assert!(!set_global(&mut sim, slot, index, 99), "{slot}/{index}");
        }
        assert_eq!(sim.state.ki, before);
        assert!(sim.state.ki.engines[2].is_none(), "creates no engine");
    }

    // QUIRK E-221: kept
    #[test]
    fn ki_state_with_a_deleted_engine_loads() {
        // A savegame in which an occupied slot (lobby: AI in slot 1) no longer has an engine.
        let (rules, mut state) = ai_match(0, ON);
        {
            let mut sim = Sim::new(&mut state, &rules);
            assert!(delete_engine(&mut sim, 1));
        }
        assert!(state.ki.engines[1].is_none());
        let save = SaveGame {
            start: start_package(&rules, &ai_setup(0, ON)),
            state: state.clone(),
            commands: Vec::new(),
        };
        let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
        loaded
            .check(&rules, crate::save::RulesCompare::Full)
            .expect("loads with a deleted engine");
        assert_eq!(loaded.state.ki, state.ki);
        assert!(loaded.state.ki.engines[1].is_none());
    }
}
