//! Tests of the AI actions 0, 1, 37 and 340 ([`super::world::ki`]) with the test world of the AI start
//! tests (`ki::start::testkit`: slots 1 and 3 are AI, slot 0 a human, slot 2 empty).

use opennml_formats::abl::RtAction;

use super::actions::{Dispatched, build_msg, dispatch};
use super::query_tests::{container, traces};
use super::tests::script;
use super::*;
use crate::ki::KiSetup;
use crate::ki::start::START_PLAYER;
use crate::ki::start::testkit::{ELEVEN_WORD_MODULE, MARK_MODULE, ai_setup, ki_rules};
use crate::rules::Rules;
use crate::setup::new_match;
use crate::sim::Sim;
use crate::state::SimState;
use crate::tick::step;

const ON: KiSetup = KiSetup {
    enabled: true,
    campaign_difficulty: 1,
};

/// Rules with the area "centrum eng" (example re/ki-lifecycle 1.3, center (36, 99)).
fn rules_with_area() -> Rules {
    let mut rules = ki_rules(false);
    rules.scripts = testing::rules_with("", &[], &[("centrum eng", [34, 98, 39, 101])]);
    assert!(rules.scripts.errors.is_empty());
    rules
}

fn state(rules: &Rules) -> SimState {
    new_match(rules, &ai_setup(0, ON)).expect("match with AI")
}

/// An action via layer D ([`dispatch`]).
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

fn engine<'a>(sim: &'a Sim, slot: usize) -> &'a crate::ki::engine::Engine {
    sim.state.ki.engines[slot].as_ref().expect("Engine")
}

/// Thread stack of the module `module` on the slot (the thread must occur exactly once).
fn stack_of(sim: &Sim, slot: usize, module: u32) -> Vec<i32> {
    let found: Vec<_> = engine(sim, slot)
        .threads
        .iter()
        .filter(|(_, t)| t.module == module)
        .map(|(_, t)| t.stack.clone())
        .collect();
    assert_eq!(found.len(), 1, "thread of module {module}");
    found.into_iter().next().unwrap()
}

#[test]
fn n1_from_kamp1_1_spawns_startplayer_with_eleven_words() {
    // Example re/ki-lifecycle 1.3: `n1;Player 3;55909;Player 3;4;2;0;Area Centrum ENG;0;1;3;0;1`:
    // slot 3 (an AI sits there here), eleven words, the area counts two (center 36, 99).
    let rules = rules_with_area();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let params = [
        "3",
        "55909",
        "3",
        "4",
        "2",
        "0",
        "area centrum eng",
        "0",
        "1",
        "3",
        "0",
        "1",
    ];
    assert_eq!(act(&mut sim, 1, &params), Dispatched::Done);
    assert_eq!(
        stack_of(&sim, 3, ELEVEN_WORD_MODULE),
        [3, 4, 2, 0, 36, 99, 0, 1, 3, 0, 1]
    );
    // The thread does not run until the next AI tick; the start ID 1 was already taken.
    assert_eq!(engine(&sim, 3).next_thread_id, 3);
    // The other slots stay untouched.
    assert_eq!(engine(&sim, 1).threads.len(), 1);
}

#[test]
fn n0_runs_synchronously() {
    let rules = rules_with_area();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(!engine(&sim, 1).mem.values.contains_key(&500));
    let before = engine(&sim, 1).threads.len();
    assert_eq!(act(&mut sim, 0, &["1", "78"]), Dispatched::Done);
    // The module ran through immediately (value 500 = 1) and left no thread.
    assert_eq!(engine(&sim, 1).mem.values.get(&500), Some(&1));
    assert_eq!(engine(&sim, 1).threads.len(), before);
    // Slot without an engine or outside 0-7: silently nothing.
    let ki = sim.state.ki.clone();
    for slot in ["2", "8", "-1", "99"] {
        assert_eq!(act(&mut sim, 0, &[slot, "78"]), Dispatched::Done);
        assert_eq!(act(&mut sim, 1, &[slot, "78"]), Dispatched::Done);
    }
    assert_eq!(sim.state.ki, ki);
}

#[test]
fn two_params_give_no_args() {
    let rules = rules_with_area();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // At most two parameters: no words, not even a count word.
    assert_eq!(act(&mut sim, 1, &["1", "78"]), Dispatched::Done);
    assert_eq!(stack_of(&sim, 1, MARK_MODULE), Vec::<i32>::new());
    // For comparison three parameters: one word.
    assert_eq!(act(&mut sim, 1, &["3", "78", "42"]), Dispatched::Done);
    assert_eq!(stack_of(&sim, 3, MARK_MODULE), [42]);
    // An area as the third parameter counts as two words.
    assert_eq!(
        act(&mut sim, 1, &["0", "78", "area centrum eng"]),
        Dispatched::Done
    );
    assert_eq!(stack_of(&sim, 0, MARK_MODULE), [36, 99]);
}

#[test]
fn n1_in_a_dialog_runs_after_the_dialog() {
    // The dialog creates the thread, the AI rests (E-139), the thread does not run until after the
    // dialog.
    let mut rules = ki_rules(false);
    rules.scripts = testing::rules_with(
        &script(&[(&[], &["n39;d"])]),
        &[("d", &script(&[(&[], &["n1;1;78", "w40;500"])]))],
        &[],
    );
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    let mut setup = ai_setup(0, ON);
    setup.script.enabled = true;
    let mut state = new_match(&rules, &setup).expect("match with AI and script");
    let marked = |s: &SimState| {
        s.ki.engines[1]
            .as_ref()
            .unwrap()
            .mem
            .values
            .get(&500)
            .copied()
    };
    let pending = |s: &SimState| {
        s.ki.engines[1]
            .as_ref()
            .unwrap()
            .threads
            .iter()
            .filter(|(_, t)| t.module == MARK_MODULE)
            .count()
    };

    // Step 1: the AI ticks, then the dialog starts and creates the thread.
    step(&mut state, &rules, &[]);
    assert!(dialog_running(&state));
    assert_eq!((pending(&state), marked(&state)), (1, None));
    // Steps 2 to 11: the dialog runs (end in step 11), the AI rests, the thread waits.
    for n in 2..=11 {
        let events = traces(step(&mut state, &rules, &[]));
        assert_eq!((pending(&state), marked(&state)), (1, None), "step {n}");
        assert_eq!(
            events.contains(&ScriptTrace::DialogEnded),
            n == 11,
            "step {n}"
        );
    }
    assert!(!dialog_running(&state));
    // Step 12: the AI runs again, the thread sets the value.
    step(&mut state, &rules, &[]);
    assert_eq!(marked(&state), Some(1));
    assert_eq!(pending(&state), 0, "thread finished");
}

// QUIRK E-221: kept
#[test]
fn n37_deletes_the_engine_and_later_starts_do_nothing() {
    let rules = rules_with_area();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let slot3 = sim.state.ki.engines[3].clone();
    assert_eq!(act(&mut sim, 37, &["1"]), Dispatched::Done);
    assert!(sim.state.ki.engines[1].is_none());
    assert_eq!(sim.state.ki.engines[3], slot3, "other slots stay");
    // n0/n1 for the deleted slot do nothing and do not create a new engine (E-221).
    let ki = sim.state.ki.clone();
    assert_eq!(act(&mut sim, 0, &["1", "78"]), Dispatched::Done);
    assert_eq!(act(&mut sim, 1, &["1", "78", "5"]), Dispatched::Done);
    assert_eq!(act(&mut sim, 37, &["1"]), Dispatched::Done, "second 37");
    assert_eq!(sim.state.ki, ki);
    // Slot outside 0-7: nothing (no writing beyond the table, deviation from the original).
    for slot in ["8", "-1", "99"] {
        assert_eq!(act(&mut sim, 37, &[slot]), Dispatched::Done);
    }
    assert_eq!(sim.state.ki, ki);
    // The AI keeps running for the remaining slots.
    for _ in 0..3 {
        step(sim.state, &rules, &[]);
    }
    assert!(sim.state.ki.engines[1].is_none());
    assert_eq!(
        engine(&sim, 3).threads.get(1).map(|t| t.module),
        Some(START_PLAYER)
    );
}

// QUIRK E-220: deviation
#[test]
fn n340_sets_a_global_and_ignores_bad_index_or_missing_engine() {
    let rules = rules_with_area();
    let mut state = state(&rules);
    state.ki.engines[3].as_mut().unwrap().mem.global_list = vec![5, 7, -3];
    let mut sim = Sim::new(&mut state, &rules);
    let list = |sim: &Sim| engine(sim, 3).mem.global_list.clone();
    assert_eq!(act(&mut sim, 340, &["3", "1", "99"]), Dispatched::Done);
    assert_eq!(list(&sim), [5, 99, -3]);
    assert_eq!(act(&mut sim, 340, &["3", "2", "-8"]), Dispatched::Done);
    assert_eq!(list(&sim), [5, 99, -8]);
    // Index beyond the end, negative (exception in the original, silent here), slot without an engine,
    // slot outside 0-7: nothing, and no engine is created.
    let ki = sim.state.ki.clone();
    for (slot, index) in [
        ("3", "3"),
        ("3", "-1"),
        ("3", "-2147483648"),
        ("2", "0"),
        ("8", "0"),
        ("-1", "0"),
    ] {
        assert_eq!(act(&mut sim, 340, &[slot, index, "1"]), Dispatched::Done);
    }
    assert_eq!(sim.state.ki, ki);
    // After 37: no engine, nothing.
    assert_eq!(act(&mut sim, 37, &["3"]), Dispatched::Done);
    assert_eq!(act(&mut sim, 340, &["3", "0", "1"]), Dispatched::Done);
    assert!(sim.state.ki.engines[3].is_none());
}

#[test]
fn save_load_during_a_permanent_lock_and_after_n37() {
    // Saving and loading in the middle of the permanent lock (157 with -1, no 158) and after deleting
    // an AI engine (37) gives the same checksum sequence as the continuous run.
    use crate::command::testkit::{self, COLONIST};
    use crate::object::status;
    use crate::save::{SaveGame, checksum};
    use crate::setup::start_package;
    use crate::world::object_event_bits;

    let mut rules = ki_rules(false);
    rules.scripts = testing::rules_with(
        &script(&[
            (&[], &["n157;figur;3;-1"]),
            (&[], &["w40;500", "n37;1", "n12;gelöscht"]),
        ]),
        &[],
        &[],
    );
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    let mut setup = ai_setup(0, ON);
    setup.script.enabled = true;
    let mut plain = new_match(&rules, &setup).expect("match with AI and script");
    let unit = {
        let mut sim = Sim::new(&mut plain, &rules);
        let unit = testkit::spawn(&mut sim, COLONIST, 0, (10, 10));
        sim.state.names.set("figur", unit.as_i32().to_string());
        unit
    };
    let locked = |s: &mut SimState| {
        let sim = Sim::new(s, &rules);
        let o = sim.object(unit).expect("unit");
        (o.anim_lock_ms, o.status & status::ANIM_LOCKED != 0)
    };
    let save_and_load = |state: &SimState| {
        let save = SaveGame {
            start: start_package(&rules, &setup),
            state: state.clone(),
            commands: Vec::new(),
        };
        let loaded = SaveGame::from_bytes(&save.to_bytes()).expect("savegame readable");
        loaded.check(&rules).expect("savegame valid");
        assert_eq!(checksum(&loaded.state), checksum(state));
        loaded.state
    };

    // Step 1: 157 locks permanently. Saved in the middle of the lock, before the 37.
    for _ in 0..3 {
        step(&mut plain, &rules, &[]);
    }
    assert_eq!(locked(&mut plain), (-1000, true));
    assert!(plain.ki.engines[1].is_some(), "37 is still pending");
    let mut mid_lock = save_and_load(&plain);

    // 500 ms wait (10 steps), then 37 deletes the engine of slot 1; saved after 37.
    for _ in 0..12 {
        step(&mut plain, &rules, &[]);
    }
    assert!(plain.ki.engines[1].is_none(), "37 deleted the engine");
    assert_eq!(locked(&mut plain), (-1000, true), "lock persists");
    let mut after_n37 = save_and_load(&plain);

    // All three runs: the same checksum sequence, the lock stays, bit 3 never comes, slot 3 runs.
    let mut sums: [Vec<u64>; 3] = Default::default();
    for _ in 0..40 {
        for (i, state) in [&mut plain, &mut mid_lock, &mut after_n37]
            .into_iter()
            .enumerate()
        {
            step(state, &rules, &[]);
            sums[i].push(checksum(state));
        }
    }
    // `plain` and `after_n37` are at the same tick; `mid_lock` is 12 steps behind.
    assert_eq!(sums[0], sums[2], "run with and without loading after 37");
    let mut plain_again = new_match(&rules, &setup).expect("match");
    {
        let mut sim = Sim::new(&mut plain_again, &rules);
        let u = testkit::spawn(&mut sim, COLONIST, 0, (10, 10));
        assert_eq!(u, unit);
        sim.state.names.set("figur", u.as_i32().to_string());
    }
    let mut reference = Vec::new();
    for _ in 0..3 + 12 + 40 {
        step(&mut plain_again, &rules, &[]);
        reference.push(checksum(&plain_again));
    }
    assert_eq!(sums[0], reference[15..], "run from step 15");
    assert_eq!(
        sums[1],
        reference[3..43],
        "run loaded in the middle of the lock"
    );
    for state in [&mut plain, &mut mid_lock, &mut after_n37] {
        assert_eq!(locked(state), (-1000, true));
        let sim = Sim::new(state, &rules);
        assert_eq!(object_event_bits(&sim, unit) & 8, 0, "bit 3 never comes");
        assert!(sim.state.ki.engines[1].is_none());
        assert!(sim.state.ki.engines[3].is_some(), "slot 3 keeps running");
    }
}
