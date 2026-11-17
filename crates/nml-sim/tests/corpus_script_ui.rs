//! Selection mirror and last button on the tutorial scripts: conditions 35 and 38 of the shipped
//! data wait for the host commands `ScriptSelection` and `ScriptButton`. The runs need the
//! original installation (`NML_GAME_DIR`) and skip silently without it.

mod script_plan;

use nml_formats::abl::RtCondition;
use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_sim::command::{Command, CommandKind};
use nml_sim::id::ObjectId;
use nml_sim::names;
use nml_sim::rules::Rules;
use nml_sim::script::{LEVEL_CONTAINER_ID, ScriptTrace, condition_values};
use nml_sim::state::{SimEvent, SimState};
use nml_sim::tick::step;

const TUTORIAL_1: &str = "data/levels/tutorial0-1/kamp0-1.lvl";
const TUTORIAL_2: &str = "data/levels/tutorial0-2/kamp0-2.lvl";

/// Blocks of the level script that have a condition with `matches`.
fn blocks_with(rules: &Rules, matches: impl Fn(&RtCondition) -> bool) -> Vec<u32> {
    let script = rules.scripts.level.as_ref().expect("level script");
    script
        .file
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.conditions.iter().any(&matches))
        .map(|(i, _)| i as u32)
        .collect()
}

/// Blocks that fired in the step.
fn fired(events: &[SimEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Script(ScriptTrace::Fired { container, block })
                if *container == LEVEL_CONTAINER_ID =>
            {
                Some(*block)
            }
            _ => None,
        })
        .collect()
}

/// Starts the level script (step 0 starts the intro dialog) and skips the dialogs (acceptance
/// helper `ScriptSkipDialog`) until none is running: the level script does not tick while a dialog
/// is running (E-139). Returns: number of steps afterwards.
fn skip_intro(state: &mut SimState, rules: &Rules) -> usize {
    step(state, rules, &[]);
    let skip = Command {
        player: 0,
        kind: CommandKind::ScriptSkipDialog,
    };
    for n in 0..600 {
        if state.script.dialog.is_none() {
            return n;
        }
        step(state, rules, std::slice::from_ref(&skip));
    }
    panic!("the intro dialog does not end");
}

/// `n` steps without commands; returns: all fired blocks.
fn run(state: &mut SimState, rules: &Rules, n: usize) -> Vec<u32> {
    (0..n)
        .flat_map(|_| fired(&step(state, rules, &[])))
        .collect()
}

fn selection(ids: Vec<ObjectId>) -> Command {
    Command {
        player: 0,
        kind: CommandKind::ScriptSelection { ids },
    }
}

fn button(word: i32) -> Command {
    Command {
        player: 0,
        kind: CommandKind::ScriptButton { word },
    }
}

#[test]
fn tutorial0_1_lesson_a_waits_for_the_selection() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let (rules, mut state) = script_plan::start_level(&vfs, TUTORIAL_1, false);
    assert!(rules.scripts.uses_ui_conditions);
    // Block "Lektion A Bewegen": condition 35 with the name `umak` (not via the block name).
    let blocks = blocks_with(&rules, |c| c.number == 35 && c.params[0] == "umak");
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    let lesson = blocks[0];
    let umak = names::ids(&state.names, "umak");
    assert!(!umak.is_empty());
    assert!(umak.iter().all(|&id| state.object(id, false).is_some()));

    skip_intro(&mut state, &rules);
    // Without selection: 35 is false (umak exists), the block does not fire, not even after two seconds.
    let early = run(&mut state, &rules, 40);
    assert!(!early.contains(&lesson), "{early:?}");
    let values = |state: &SimState| condition_values(state, &rules, LEVEL_CONTAINER_ID, lesson);
    assert_eq!(values(&state), Some(vec![Some(false)]));
    assert!(state.script.mission.selection.is_empty());

    // Counter-check: a foreign object alone (the selection list takes that too) is not enough.
    let other = state
        .objects()
        .map(|o| o.id)
        .find(|id| !umak.contains(id))
        .expect("a second object");
    let events = step(&mut state, &rules, &[selection(vec![other])]);
    assert!(!fired(&events).contains(&lesson));
    assert_eq!(state.script.mission.selection, [other]);
    assert_eq!(values(&state), Some(vec![Some(false)]));

    // All IDs of `umak`: the condition is true and the block fires in the same step (host commands
    // take effect before the script tick).
    let mut ids = umak.clone();
    ids.push(other);
    let events = step(&mut state, &rules, &[selection(ids)]);
    assert!(fired(&events).contains(&lesson), "{:?}", fired(&events));
}

#[test]
fn tutorial0_2_button_blocks_fire_on_2904_2921_140() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    for word in [2904, 2921, 140] {
        let (rules, mut state) = script_plan::start_level(&vfs, TUTORIAL_2, false);
        let word_text = word.to_string();
        let blocks = blocks_with(&rules, |c| c.number == 38 && c.params[1] == word_text);
        assert_eq!(blocks.len(), 1, "button {word}: {blocks:?}");
        let block = blocks[0];
        let index = rules.scripts.level.as_ref().unwrap().file.blocks[block as usize]
            .conditions
            .iter()
            .position(|c| c.number == 38)
            .unwrap();
        let values = |state: &SimState| {
            condition_values(state, &rules, LEVEL_CONTAINER_ID, block).expect("values")[index]
        };
        skip_intro(&mut state, &rules);
        // The block "Hammer" (140) additionally demands 250 gold of player 2 (condition 2).
        state.players[2].set_resource(1, 1000);
        // Without button (start 0): false, the block does not fire.
        assert_eq!(values(&state), Some(false));
        let early = run(&mut state, &rules, 20);
        assert!(!early.contains(&block), "button {word}: {early:?}");

        // A different button does not trigger it.
        let events = step(&mut state, &rules, &[button(word + 1)]);
        assert!(!fired(&events).contains(&block));
        assert_eq!(values(&state), Some(false));

        // The host command makes the condition true; the block fires in the same step.
        let events = step(&mut state, &rules, &[button(word)]);
        assert_eq!(state.script.mission.last_button, word);
        assert!(fired(&events).contains(&block), "button {word}");
    }
}
