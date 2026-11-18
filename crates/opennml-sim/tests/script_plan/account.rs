//! Shared evaluation of the script corpus runs (dry run and campaign run): statistics per action number and
//! condition from the `SimEvent::Script` events, the allowed rejections of the script path and the shared
//! expectations ([`assert_clean`]).

use std::collections::{BTreeMap, BTreeSet};

use opennml_sim::command::Outcome;
use opennml_sim::rules::Rules;
use opennml_sim::script::{BlockState, Container, LoadedScript, ScriptTrace};
use opennml_sim::state::{SimEvent, SimState};

/// Duration of a script tick for `update_wait` (like `SCRIPT_TICK_MS`).
pub const DT: i32 = opennml_sim::script::SCRIPT_TICK_MS;

/// Kind of a run; also a column of the statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Level script, conditions as implemented (version (a)).
    LevelA = 0,
    /// Level script, all conditions true (version (b)).
    LevelB = 1,
    /// Single dialog script, all conditions true.
    Dialog = 2,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::LevelA => "Level (a)",
            Kind::LevelB => "Level (b)",
            Kind::Dialog => "Dialog (b)",
        }
    }

    pub fn all_true(self) -> bool {
        self != Kind::LevelA
    }
}

/// Counters per action number, per run kind.
#[derive(Debug, Default, Clone, Copy)]
pub struct ActionStat {
    pub reached: [u64; 3],
    pub resumed: [u64; 3],
    pub suspended: [u64; 3],
    pub pending: [u64; 3],
}

/// A `Guarded`, `Limit` or panic with its location.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub run: String,
    pub what: String,
}

/// Key of the command count: action number, command type, verdict (reasons with digits replaced by `#`,
/// so that object IDs do not produce a separate row per case).
pub type CommandKey = (i32, u32, String);

/// Statistics over all runs.
#[derive(Default)]
pub struct Stats {
    pub actions: BTreeMap<i32, ActionStat>,
    /// Commands of the script path per run kind, with the first occurrence (run, step, full reason).
    pub commands: BTreeMap<CommandKey, ([u64; 3], String)>,
    pub condition_pending: BTreeMap<i32, [u64; 3]>,
    /// Touched blocks `(script path, block index)` per run kind.
    pub fired_blocks: [BTreeSet<(String, u32)>; 3],
    /// Started dialogs (path) per run kind.
    pub dialogs_started: [BTreeMap<String, u64>; 3],
    /// Similar findings combined: text -> (count, first run and step).
    pub guarded: BTreeMap<String, (u64, String)>,
    pub limits: BTreeMap<String, (u64, String)>,
    pub panics: Vec<Finding>,
    pub runs: Vec<RunSummary>,
}

/// Key figures of a run.
#[derive(Debug, Default, Clone)]
pub struct RunSummary {
    pub name: String,
    pub kind: Option<Kind>,
    pub steps: usize,
    pub fired: u64,
    pub deleted: u64,
    pub suspended: u64,
    pub pending: u64,
    pub condition_pending: u64,
    pub dialogs_started: u64,
    pub dialogs_ended: u64,
    pub commands: u64,
    pub rejected: u64,
    pub guarded: u64,
    pub limits: u64,
    pub panicked: bool,
    pub millis: u128,
    /// Blocks waiting at the end of the run: `L`/`D` (level/dialog), block, `w` action with number, source
    /// line and remaining time (-1 = event waiting); plus a running dialog.
    pub waiting: Vec<String>,
}

/// Verdict of a script command as text: `Accepted`, `Partial: ...`, `Rejected: ...`.
pub fn outcome_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Accepted => "Accepted".to_owned(),
        Outcome::Partial(why) => format!("Partial: {why}"),
        Outcome::Rejected(why) => format!("Rejected: {why}"),
    }
}

/// Reason without digits (object IDs, slots) as a count key.
pub fn digits_masked(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Waiting blocks of a state (for the column "waiting at the end").
pub fn waiting_blocks(rules: &Rules, state: &SimState) -> Vec<String> {
    let mut out = Vec::new();
    for (tag, c) in [("L", &state.script.level), ("D", &state.script.dialog)] {
        let Some(c) = c else { continue };
        if tag == "D" {
            out.push(format!("Dialog running ({} blocks)", c.blocks.len()));
        }
        let file = rules.scripts.script(&c.source.path).map(|s| &s.file);
        for b in c.blocks.iter().filter(|b| b.running) {
            let (number, line) = file
                .and_then(|f| f.blocks.get(b.index as usize))
                .and_then(|fb| fb.actions.get(b.cur_action as usize))
                .map_or((-1, 0), |a| (a.number, a.line));
            out.push(format!(
                "{tag}{}:w{number} (line {line}, t {})",
                b.index, b.timeout
            ));
        }
    }
    out
}

/// Start state of a block before its actions: the state before the step, after `update_wait`.
/// Returns `(cur_action, running)`.
pub fn start_of(pre: Option<&BlockState>, block: u32) -> (i32, bool) {
    let mut b = pre.copied().unwrap_or_else(|| BlockState::new(block));
    if b.timeout != -1 {
        let t = b.timeout.wrapping_sub(DT).max(0);
        if t == 0 {
            b.running = false;
            b.cur_action = b.cur_action.wrapping_add(1);
        }
    }
    (b.cur_action, b.running)
}

pub fn block_of(c: Option<&Container>, block: u32) -> Option<&BlockState> {
    c?.blocks.iter().find(|b| b.index == block)
}

/// Evaluates the events of one step. `pre_level`/`pre_dialog`: containers before the step,
/// `post`: state after it.
#[allow(clippy::too_many_arguments)]
pub fn account(
    rules: &Rules,
    kind: Kind,
    run: &str,
    step: usize,
    pre_level: Option<&Container>,
    pre_dialog: Option<&Container>,
    post: &SimState,
    events: &[SimEvent],
    stats: &mut Stats,
    summary: &mut RunSummary,
) {
    let k = kind as usize;
    let traces: Vec<&ScriptTrace> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Script(t) => Some(t),
            _ => None,
        })
        .collect();
    // Dialog to which the messages with container -1 belong, and its state before the step (after a
    // restart within the step: fresh).
    let mut dialog_path = pre_dialog.map(|c| c.source.path.clone());
    let mut dialog_pre = pre_dialog;
    let mut dialog_ended = false;
    for (p, t) in traces.iter().enumerate() {
        match t {
            ScriptTrace::Fired { container, block } => {
                summary.fired += 1;
                let (path, pre, post_c) = if *container < 0 {
                    let post_c = if dialog_ended {
                        None
                    } else {
                        post.script.dialog.as_ref()
                    };
                    (dialog_path.clone(), dialog_pre, post_c)
                } else {
                    (
                        pre_level.map(|c| c.source.path.clone()),
                        pre_level,
                        post.script.level.as_ref(),
                    )
                };
                let path = path.expect("message without container");
                let script = rules.scripts.script(&path).expect("script in the rules");
                stats.fired_blocks[k].insert((path.clone(), *block));
                // Messages of this block up to the next touched block.
                let segment = traces[p + 1..]
                    .iter()
                    .take_while(|t| !matches!(t, ScriptTrace::Fired { .. }));
                let mut suspended_at = None;
                let mut guarded_number = None;
                for t in segment {
                    match t {
                        ScriptTrace::Suspended {
                            container: c,
                            block: b,
                            action,
                        } if c == container && b == block => suspended_at = Some(*action as i32),
                        ScriptTrace::Guarded {
                            container: c,
                            block: b,
                            number,
                            ..
                        } if c == container && b == block => guarded_number = Some(*number),
                        _ => {}
                    }
                }
                count_reached(
                    script,
                    *block,
                    start_of(block_of(pre, *block), *block),
                    block_of(post_c, *block),
                    suspended_at,
                    guarded_number,
                    k,
                    stats,
                );
            }
            ScriptTrace::Deleted { .. } => summary.deleted += 1,
            // Commands of the script path: count per action, command type and verdict.
            ScriptTrace::Command {
                action,
                code,
                outcome,
                ..
            } => {
                summary.commands += 1;
                if matches!(outcome, Outcome::Rejected(_)) {
                    summary.rejected += 1;
                }
                let full = outcome_text(outcome);
                let e = stats
                    .commands
                    .entry((*action, *code, digits_masked(&full)))
                    .or_insert(([0; 3], format!("{run}, step {step}: {full}")));
                e.0[k] += 1;
            }
            ScriptTrace::Suspended {
                container,
                block,
                action,
            } => {
                summary.suspended += 1;
                let path = if *container < 0 {
                    dialog_path.clone()
                } else {
                    pre_level.map(|c| c.source.path.clone())
                };
                let number = path
                    .and_then(|p| rules.scripts.script(&p))
                    .and_then(|s| s.file.blocks.get(*block as usize))
                    .and_then(|b| b.actions.get(*action as usize))
                    .map_or(-1, |a| a.number);
                stats.actions.entry(number).or_default().suspended[k] += 1;
            }
            ScriptTrace::Pending { action, .. } => {
                summary.pending += 1;
                stats.actions.entry(*action).or_default().pending[k] += 1;
            }
            ScriptTrace::ConditionPending { condition, .. } => {
                summary.condition_pending += 1;
                stats.condition_pending.entry(*condition).or_default()[k] += 1;
            }
            ScriptTrace::DialogStarted { path } => {
                summary.dialogs_started += 1;
                *stats.dialogs_started[k].entry(path.clone()).or_default() += 1;
                dialog_path = Some(path.clone());
                dialog_pre = None;
                dialog_ended = false;
            }
            ScriptTrace::DialogEnded => {
                summary.dialogs_ended += 1;
                dialog_ended = true;
            }
            ScriptTrace::Guarded {
                container,
                block,
                number,
                why,
            } => {
                summary.guarded += 1;
                let path = if *container < 0 {
                    dialog_path.clone()
                } else {
                    pre_level.map(|c| c.source.path.clone())
                };
                let key = format!(
                    "{}, block {block}, number {number}: {why}",
                    path.as_deref().unwrap_or("?")
                );
                let e = stats
                    .guarded
                    .entry(key)
                    .or_insert((0, format!("{run}, step {step}")));
                e.0 += 1;
            }
            ScriptTrace::Limit { what } => {
                summary.limits += 1;
                // The run is part of the key, so that the exception [`LIMIT_ONLY_IN_B`] applies only to its map.
                let e = stats
                    .limits
                    .entry(format!("{}: {what} in {run}", kind.label()))
                    .or_insert((0, format!("{run}, step {step}")));
                e.0 += 1;
            }
        }
    }
}

/// Counts the actions passed through in a touched block (explanation in the module header).
#[allow(clippy::too_many_arguments)]
pub fn count_reached(
    script: &LoadedScript,
    block: u32,
    (start, running): (i32, bool),
    post: Option<&BlockState>,
    suspended_at: Option<i32>,
    guarded_number: Option<i32>,
    k: usize,
    stats: &mut Stats,
) {
    let Some(file_block) = script.file.blocks.get(block as usize) else {
        return;
    };
    let actions = &file_block.actions;
    let count = actions.len() as i32;
    let mut first = start;
    if running {
        // Continuation branch at `start`: if it stops there again, the action ran anew.
        if suspended_at == Some(start) {
            // Executed anew (time wait with -1 and the like).
        } else {
            if let Some(a) = actions.get(start as usize) {
                stats.actions.entry(a.number).or_default().resumed[k] += 1;
            }
            first = start + 1;
            // Not ready yet: the block keeps waiting at the same place.
            let still_waiting = post.is_some_and(|b| b.running && b.cur_action == start);
            if still_waiting && suspended_at.is_none() {
                return;
            }
        }
    }
    let last = suspended_at.unwrap_or(count - 1).min(count - 1);
    for i in first.max(0)..=last {
        let number = actions[i as usize].number;
        stats.actions.entry(number).or_default().reached[k] += 1;
        if guarded_number == Some(number) {
            return;
        }
    }
}

/// `Limit` that is allowed only in version (b) and only for `kamp3-11`: the info history (limit of the
/// reimplementation 4,096, the oldest entries drop out; quirks E-173). In (b) the dialog of `kamp3-11` runs on past
/// a movement, and block 2 "rip" (`n23`, `n213`, `n10`) fires in every step and fills
/// the history (15,309 messages from step 4,692). In the game (version a), in all dialog runs and on every
/// other map, a limit message is a finding. Pair (start of the key, map in the run).
pub const LIMIT_ONLY_IN_B: (&str, &str) = ("Level (b): info history", "/kamp3-11/kamp3-11.lvl");

/// Reasons for which the script path may reject (digits replaced by `#`): the named object or the container
/// is no longer present (under `Origin::Script` only "present" counts; dead and removed objects remain in the
/// name table with a stale ID, E-145, or the ID is 0).
/// "Quarters: unit # cannot be quartered": the unit does not meet the class `0x40000` (for example a cannon
/// that is already quartered); the original silently leaves it out (E-217).
pub const ALLOWED_REJECTIONS: &[&str] = &[
    "Rejected: object # not present",
    "Rejected: Quarters: container # not present",
    "Rejected: Quarters: unit # cannot be quartered",
];

/// The shared expectations: no `Guarded`, no panic, `Limit` only [`LIMIT_ONLY_IN_B`] (version (b) of
/// `kamp3-11`); neither `Pending` (all actions are implemented) nor `ConditionPending` (35-38 are implemented);
/// rejections of the script path only with [`ALLOWED_REJECTIONS`], no `Partial`.
pub fn assert_clean(stats: &Stats) {
    assert!(stats.panics.is_empty(), "Panics: {:?}", stats.panics);
    assert!(stats.guarded.is_empty(), "Guarded: {:#?}", stats.guarded);
    let limits: Vec<&String> = stats
        .limits
        .keys()
        .filter(|k| !(k.starts_with(LIMIT_ONLY_IN_B.0) && k.contains(LIMIT_ONLY_IN_B.1)))
        .collect();
    assert!(limits.is_empty(), "Limit: {:#?}", stats.limits);
    let outcomes: Vec<&CommandKey> = stats
        .commands
        .keys()
        .filter(|(_, _, o)| o != "Accepted" && !ALLOWED_REJECTIONS.contains(&o.as_str()))
        .collect();
    assert!(
        outcomes.is_empty(),
        "unexpected verdicts of the script path: {outcomes:#?}"
    );
    let pending: Vec<i32> = stats
        .actions
        .iter()
        .filter(|(_, s)| s.pending.iter().any(|&x| x > 0))
        .map(|(n, _)| *n)
        .collect();
    assert!(
        pending.is_empty(),
        "Pending (no action may report it anymore): {pending:?}"
    );
    let conditions: Vec<i32> = stats
        .condition_pending
        .iter()
        .filter(|(_, v)| v.iter().any(|&x| x > 0))
        .map(|(n, _)| *n)
        .collect();
    assert!(
        conditions.is_empty(),
        "ConditionPending (no condition may report it anymore): {conditions:?}"
    );
}
