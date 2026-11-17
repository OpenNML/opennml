//! Scripts (`.abl`) in the simulation.
//!
//! The immutable script rules live in [`rules`] (`Rules::scripts`), the mutable state in [`ScriptState`]
//! (`SimState::script`). For its tick a container is taken out of the state with `Option::take` and put
//! back afterwards (like `ki::run::with_ki`); this way conditions and actions can change the whole
//! simulation (`&mut Sim`) and the container at the same time.
//!
//! Position in the tick: after the world tick and the AI, `tick::step` calls [`tick_level`] (only without
//! a dialog), [`tick_dialog`] and [`tick_speech`]. Without [`ScriptSetup::enabled`] there is no container
//! and all three do nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use nml_formats::config::lower_german;
use nml_formats::property::is_c_space;

use crate::rng::CrtRand;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};

pub mod actions;
pub mod command;
pub mod conditions;
pub mod container;
pub mod present;
pub mod query;
pub mod rules;
pub mod run;
pub mod speech;
#[doc(hidden)]
pub mod testing;
pub mod ui_mirror;
pub mod world;

#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod present_tests;
#[cfg(test)]
mod query_object_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_ki_tests;
#[cfg(test)]
mod world_tests;

pub use container::{BlockState, Container, ScriptSource};
pub use present::{FogMemory, InfoFrameMemory, PortraitMemory, Presentation, PresentationMemory};
pub use rules::{LoadedScript, ScriptRules, SpeechLen};
pub use speech::{SpeechLine, SpeechState};

/// Duration of a script tick in ms (`GameEngine_TickLevelScripts(50)`, `GameEngine_TickDialogScript(50)`).
pub const SCRIPT_TICK_MS: i32 = crate::tick::STEP_MS as i32;

/// ID of the dialog container (`FUN_00564010(engine, -1)` in `GameEngine_StartDialog`).
pub const DIALOG_CONTAINER_ID: i32 = -1;

/// Script setup of a match (`MatchSetup::script`). Not `Copy`: the profile variables are a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSetup {
    /// Start the level script (default: off).
    pub enabled: bool,
    /// Speaking time per character without a speech file (`Engine.ini` `TimePerChar`, default 55).
    pub time_per_char: i32,
    /// Profile variables at start (`m_VarMap` of the profile): name (lowercase) -> value; [`start_match`] copies
    /// them to [`MissionState::profile_vars`]. In JSON input (viewer config, control channel) the field may
    /// be missing; `FORMAT_VERSION` protects the postcard savegames (postcard is not self-describing).
    #[serde(default)]
    pub profile_vars: BTreeMap<String, i32>,
}

impl Default for ScriptSetup {
    fn default() -> Self {
        Self {
            enabled: false,
            time_per_char: 55,
            profile_vars: BTreeMap::new(),
        }
    }
}

/// ID of the level container: the game engine constructor `FUN_0043BB70` sets `m_LastTriggerEngineID` to 1
/// (E-185); the dialog container has [`DIALOG_CONTAINER_ID`] (-1).
pub const LEVEL_CONTAINER_ID: i32 = 1;

/// Script state of a match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptState {
    /// Level container (`m_TriggerEngineMap`, at most one).
    pub level: Option<Container>,
    /// Dialog container (`GameEngine+0x25E0`); `Some` <=> dialog running (`+0x601`).
    pub dialog: Option<Container>,
    /// Next container ID (`m_LastTriggerEngineID`).
    pub next_container_id: i32,
    /// Script randomness (actions 33, 180), seeded from `MatchSetup::seed`.
    pub rand: CrtRand,
    /// Speaking time per character from the setup (the step does not see the setup).
    pub time_per_char: i32,
    pub mission: MissionState,
    /// Area rectangles `[x0, z0, x1, z1]` per name changed at run time (E-190): condition 6 and query 300
    /// swap corners, 211/214 enlarge them; they take precedence over `Rules::scripts.area` and survive
    /// saving and loading.
    pub area_overrides: BTreeMap<String, [i32; 4]>,
}

/// Default: `time_per_char = 55`, `rand = CrtRand::new(0)`, otherwise empty. `new_match` without a
/// script leaves exactly this state in place.
impl Default for ScriptState {
    fn default() -> Self {
        Self {
            level: None,
            dialog: None,
            next_container_id: 0,
            rand: CrtRand::new(0),
            time_per_char: ScriptSetup::default().time_per_char,
            mission: MissionState::default(),
            area_overrides: BTreeMap::new(),
        }
    }
}

/// A mission objective (main or secondary objective, `GameEngine` tables): status and text ID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quest {
    pub status: i32,
    pub text: i32,
}

/// Mission state: speech state and info history (`m_InfoHistory`), mission objectives, countdown display
/// and task status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionState {
    pub speech: SpeechState,
    pub info_history: Vec<i32>,
    /// Main objectives 0 to 9.
    pub main_quests: [Quest; 10],
    /// Secondary objectives 0 to 9.
    pub sub_quests: [Quest; 10],
    /// ID of the container whose counter is displayed (action 15 sets it, 16 sets 0; 0 never matches a
    /// container, start 0).
    pub countdown_container: i32,
    /// Displayed countdown seconds (`counter / 1000`).
    pub countdown_s: i32,
    /// Task status of action 200 (deviation from the original): task -> 0/1.
    pub task_status: BTreeMap<i32, u8>,
    /// Input lock (actions 18/19): `true` = input locked (E-142).
    pub ui_locked: bool,
    /// Cinemascope (147 on, 148 off): letterbox bars instead of the command bar.
    pub cinemascope: bool,
    /// Selection mirror (host command `ScriptSelection`, at most 30 entries, E-228/E-230): the selection of the
    /// local game host in reported order; conditions 35-37 evaluate it.
    pub selection: Vec<crate::id::ObjectId>,
    /// Last button (host command `ScriptButton`, start 0, E-229): condition 38.
    pub last_button: i32,
    /// Profile variables of the match: name (lowercase) -> value; start from [`ScriptSetup::profile_vars`],
    /// 382 writes, 383 reads.
    pub profile_vars: BTreeMap<String, i32>,
    /// Presentation state.
    pub presentation: PresentationMemory,
}

/// Flow message of the scripts to the host (`SimEvent::Script`); not part of the state.
/// `block` is the index into `ScriptFile::blocks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptTrace {
    /// A block executes its actions (conditions true, or continuation after waiting).
    Fired {
        container: i32,
        block: u32,
    },
    /// A block was deleted after executing.
    Deleted {
        container: i32,
        block: u32,
    },
    /// A block stops at an action (waiting).
    Suspended {
        container: i32,
        block: u32,
        action: u32,
    },
    /// Action with a case whose effect (layer D) is still missing.
    Pending {
        container: i32,
        block: u32,
        action: i32,
    },
    /// Condition with a case that is not implemented yet (false).
    ConditionPending {
        container: i32,
        block: u32,
        condition: i32,
    },
    DialogStarted {
        path: String,
    },
    DialogEnded,
    /// Run-time error that the original answers with an exception (A7): no effect.
    Guarded {
        container: i32,
        block: u32,
        number: i32,
        why: String,
    },
    /// Command of a script action via the command path ([`command`]): `code` is the command type of the
    /// original (`0x3C` Halt ...), `outcome` the verdict of the execution.
    Command {
        container: i32,
        block: u32,
        action: i32,
        code: u32,
        outcome: crate::command::Outcome,
    },
    /// A limit was reached (the oldest entries drop out).
    Limit {
        what: String,
    },
}

/// Key of an area name in `area_overrides` and `Rules::scripts.area`: lowercased and trimmed like
/// `FUN_00435A60`.
pub(crate) fn area_key(name: &str) -> String {
    lower_german(name.trim_matches(is_c_space))
}

/// Rectangle `[x0, z0, x1, z1]` of the named area at run time (E-190): first the changes of the savegame
/// (`ScriptState::area_overrides`), then the rectangles of the `.wld`. All script readers go through this
/// function; an unknown name returns `None`.
pub fn area(sim: &Sim, name: &str) -> Option<[i32; 4]> {
    sim.state
        .script
        .area_overrides
        .get(&area_key(name))
        .copied()
        .or_else(|| sim.rules.scripts.area(name))
}

/// Script message into the events of the step.
pub(crate) fn trace(sim: &mut Sim, trace: ScriptTrace) {
    sim.state.events.push(SimEvent::Script(trace));
}

/// Is a dialog running (`GameEngine+0x601`)? The AI and level scripts rest then (E-139). The flag hangs on
/// the dialog container: [`actions::start_dialog`] sets it, the dialog end in [`tick_dialog`] clears it
/// (as in the original, never a flag without a container, E-143).
pub fn dialog_running(state: &SimState) -> bool {
    state.script.dialog.is_some()
}

/// Script start from `new_match`: only with `enabled`; load errors of the scripts make the start fail with
/// the first message. With a level script the container [`LEVEL_CONTAINER_ID`] (1, E-185) is created and
/// `next_container_id = 2`; the randomness is seeded from `seed`, `time_per_char` is taken over.
pub fn start_match(sim: &mut Sim, setup: &ScriptSetup, seed: u32) -> Result<(), String> {
    if !setup.enabled {
        return Ok(());
    }
    let rules = &sim.rules.scripts;
    if let Some(first) = rules.errors.first() {
        return Err(first.clone());
    }
    let script = &mut sim.state.script;
    script.rand = CrtRand::new(seed);
    script.time_per_char = setup.time_per_char;
    script.mission.profile_vars = setup.profile_vars.clone();
    if let Some(level) = &rules.level {
        let source = ScriptSource {
            path: level.path.clone(),
            checksum: level.checksum,
        };
        script.level = Some(Container::new(LEVEL_CONTAINER_ID, source, &level.file));
        script.next_container_id = LEVEL_CONTAINER_ID + 1;
    }
    Ok(())
}

/// Run-time view of the file of a container from the rules.
fn file_of<'r>(
    rules: &'r Rules,
    source: &ScriptSource,
) -> Option<&'r nml_formats::abl::ScriptFile> {
    rules.scripts.script(&source.path).map(|s| &s.file)
}

/// `GameEngine_TickLevelScripts(50)` (`0x00430290`): one tick of the level container. The caller checks
/// [`dialog_running`]. **After** the container tick, the tick sets the countdown display `m_CountDown`
/// (`+0x60C`) to `counter / 1000` (C division, truncated toward zero, -1 -> 0) if the container is the
/// registered one ([`MissionState::countdown_container`], action 15); otherwise the value stays as it is
/// (E-205).
pub fn tick_level(sim: &mut Sim) {
    let Some(mut c) = sim.state.script.level.take() else {
        return;
    };
    let rules = sim.rules;
    match file_of(rules, &c.source) {
        Some(file) => {
            run::tick_container(sim, &mut c, file, SCRIPT_TICK_MS);
        }
        None => missing_file(sim, &c),
    }
    if c.id == sim.state.script.mission.countdown_container {
        sim.state.script.mission.countdown_s = c.counter / 1000;
    }
    sim.state.script.level = Some(c);
}

/// `GameEngine_TickDialogScript(50)` (`0x0043E1A0`): one tick of the dialog container; if it returns fewer
/// than one block, the dialog ends after this tick: container gone, [`ScriptTrace::DialogEnded`].
/// Dialogs are started by action 39 ([`actions::start_dialog`]).
///
/// During the tick the container is taken out, so [`dialog_running`] returns false there. Nobody asks
/// during the tick: the AI and level scripts run before it, action 39 from a dialog container is rejected
/// (E-174). The original does not check `+0x601` until **after** the tick (during the tick it is cleared only by
/// `FUN_0042DC40`, callers `GameEngine_StartDialog` and `FUN_0041A5D0` - not reachable from scripts; at
/// the dialog end this function itself sets it to 0 after the tick); the debug command `ScriptSkipDialog`
/// has its own flag [`Container::skipped`] for this.
///
/// If the file of the container is missing (only from a corrupted savegame), the dialog ends with
/// `Guarded` instead of running forever.
///
/// A skipped container ([`Container::skipped`], debug command `ScriptSkipDialog`) does not tick:
/// `ScriptContainer_TickUnlessSkipped` (`0x0056D900`) returns -1, the dialog ends as with "fewer than one
/// block" (re/campaign-ui 4).
pub fn tick_dialog(sim: &mut Sim) {
    let Some(mut c) = sim.state.script.dialog.take() else {
        return;
    };
    let rules = sim.rules;
    let remaining = match file_of(rules, &c.source) {
        _ if c.skipped => -1,
        Some(file) => run::tick_container(sim, &mut c, file, SCRIPT_TICK_MS),
        None => {
            missing_file(sim, &c);
            0
        }
    };
    if remaining < 1 {
        trace(sim, ScriptTrace::DialogEnded);
    } else {
        sim.state.script.dialog = Some(c);
    }
}

/// Debug command `ScriptSkipDialog`: sets the skip flag of the running dialog container; the next
/// [`tick_dialog`] ends it without a tick. Without a dialog `Err` with the reason (rejection of the
/// command). Skipping repeatedly changes nothing.
pub fn skip_dialog(sim: &mut Sim) -> Result<(), String> {
    match sim.state.script.dialog.as_mut() {
        Some(c) => {
            c.skipped = true;
            Ok(())
        }
        None => Err("no dialog running".to_string()),
    }
}

/// Speaking time and typing times in the 50 ms tick (`FUN_00444C10` + `FUN_00432B90`, [`speech::tick`]).
/// Without a script the speech state is empty and the tick does nothing.
pub fn tick_speech(sim: &mut Sim) {
    speech::tick(sim, SCRIPT_TICK_MS);
}

/// Values of the conditions of block `index` (index in the script file) in container `container` (level
/// [`LEVEL_CONTAINER_ID`]; dialog [`DIALOG_CONTAINER_ID`]) for the control channel (`script_state`):
/// evaluated one after another on **a copy** of the state, because conditions have side effects (19
/// consumes text events, query 2500 clears event bits). The real state and its checksum stay the same. The
/// state between two steps is evaluated (the tick counts the timers down beforehand). Per condition `None`
/// = not implemented yet ([`conditions::shown_value`]). `None` overall: no such container, or a block
/// beyond the end of the file (an already deleted block of the file still has values).
pub fn condition_values(
    state: &SimState,
    rules: &Rules,
    container: i32,
    index: u32,
) -> Option<Vec<Option<bool>>> {
    let mut copy = state.clone();
    let slot = if container == DIALOG_CONTAINER_ID {
        &mut copy.script.dialog
    } else {
        &mut copy.script.level
    };
    let c = slot.take().filter(|c| c.id == container)?;
    let block = file_of(rules, &c.source)?.blocks.get(index as usize)?;
    let mut sim = Sim::new(&mut copy, rules);
    Some(
        block
            .conditions
            .iter()
            .map(|cond| conditions::shown_value(&mut sim, &c, cond))
            .collect(),
    )
}

/// The file of a container is missing in the rules (only after a corrupted savegame): no tick (a dialog
/// ends).
fn missing_file(sim: &mut Sim, c: &Container) {
    trace(
        sim,
        ScriptTrace::Guarded {
            container: c.id,
            block: 0,
            number: -1,
            why: format!("script {} missing in the rules", c.source.path),
        },
    );
}
