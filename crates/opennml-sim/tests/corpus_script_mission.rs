//! Mission run `kamp1-1`: five game minutes like the scene of `SCRIPT_GOLDEN` (seed "NML2", AI on,
//! difficulty 1, script on, 55 ms per character, without commands).
//!
//! Expectation: the intro ends (the movement 153 brings the ship of Carvinez into the harbor,
//! condition 27 becomes true, intro block 2 runs), after that at least one main objective stands at
//! status 3 (set). The AI starts of the level script (actions 0/1, route W) take effect: no
//! `Pending` for them, their modules run in the engines of the named slots. No
//! `ScriptTrace::Guarded`, no `Limit`, no rejected script command; `Pending` and
//! `ConditionPending` no longer occur (all actions and the conditions 35-38 are implemented). The
//! report (block sequence, set objectives, commands of the script path, `Pending` per number, AI
//! starts) goes to `$NML_SHOTS_OUT/kamp1-1-mission.md`; without the variable the test writes
//! nothing. Without the original installation it skips silently.

mod script_plan;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use opennml_rda::Vfs;
use opennml_rda::testutil::{corpus_game_dir, game_version};
use opennml_sim::command::Outcome;
use opennml_sim::rules::Rules;
use opennml_sim::script::{Presentation, Quest, ScriptTrace};
use opennml_sim::setup::{MatchSetup, new_match};
use opennml_sim::state::SimEvent;
use opennml_sim::tick::step;
use opennml_world::level::load_level;

use script_plan::{KI_START_ACTIONS, plan_of};

/// Map of the mission.
const LEVEL: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
/// Dialog that the level script starts in step 1.
const INTRO_DIALOG: &str = "data/levels/kampagne1/kamp1-1/dialogszene intro.abl";
/// Start index of the random sequence like `SCRIPT_GOLDEN` ("NML2").
const SEED: u32 = 0x4E4D_4C32;
/// Five game minutes like `SCRIPT_GOLDEN`.
const STEPS: u64 = 6000;
/// Status "set" of the actions 27/182.
const QUEST_SET: i32 = 3;
/// Block "Wenn Schiff von Carvinez ankert" of the intro (condition 27); without it the intro does not end.
const INTRO_SHIP_BLOCK: u32 = 2;
/// Block of the level script with the AI starts (actions 0/1) that runs within the five minutes.
const KI_START_BLOCK: u32 = 67;

/// AI start action of the level script: block, number (0 synchronous, 1 thread), slot, module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KiStartAction {
    block: u32,
    number: i32,
    slot: usize,
    module: u32,
}

/// Observation of an AI start action in the run: step of the block, next thread ID of the engine
/// before and after the step, thread of the module present after the step.
#[derive(Debug, Clone, Copy)]
struct KiStartSeen {
    action: KiStartAction,
    step: u64,
    id_before: Option<i32>,
    id_after: Option<i32>,
    thread_after: bool,
}

/// Fired block: step, container, block, script path.
struct Fired {
    step: u64,
    container: i32,
    block: u32,
    path: String,
}

/// Change of a mission objective: step, "Main"/"Sub", index, old and new state.
struct QuestChange {
    step: u64,
    kind: &'static str,
    index: usize,
    old: Quest,
    new: Quest,
}

#[test]
fn kamp1_1_runs_five_minutes_with_quests() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap_or_else(|e| panic!("{LEVEL}: {e:#}"));
    let mut setup = MatchSetup::default_for(&rules);
    setup.seed = SEED;
    setup.ki.enabled = true;
    setup.ki.campaign_difficulty = 1;
    setup.script.enabled = true;
    setup.script.time_per_char = 55;

    // AI starts of the level script from the file.
    let ki_actions: Vec<KiStartAction> = rules
        .scripts
        .level
        .iter()
        .flat_map(|s| s.file.blocks.iter().enumerate())
        .flat_map(|(bi, b)| b.actions.iter().map(move |a| (bi as u32, a)))
        .filter(|(_, a)| a.number == 0 || a.number == 1)
        .map(|(block, a)| KiStartAction {
            block,
            number: a.number,
            slot: a.params[0].trim().parse().unwrap(),
            module: a.params[1].trim().parse().unwrap(),
        })
        .collect();
    assert!(
        ki_actions.iter().any(|k| k.block == KI_START_BLOCK),
        "no AI starts in block {KI_START_BLOCK}: {ki_actions:?}"
    );

    let mut state = new_match(&rules, &setup).unwrap();
    let level_path = state
        .script
        .level
        .as_ref()
        .expect("level container")
        .source
        .path
        .clone();
    let mut dialog_path: Option<String> = None;
    let mut fired: Vec<Fired> = Vec::new();
    let mut quests: Vec<QuestChange> = Vec::new();
    let mut pending: BTreeMap<i32, u64> = BTreeMap::new();
    let mut condition_pending: BTreeMap<i32, u64> = BTreeMap::new();
    // AI starts: (step, container, block, action).
    let mut ki_starts: Vec<(u64, i32, u32, i32)> = Vec::new();
    let mut ki_seen: Vec<KiStartSeen> = Vec::new();
    // Commands of the script path: (action, command type, verdict) -> count, first step.
    let mut commands: BTreeMap<(i32, u32, String), (u64, u64)> = BTreeMap::new();
    let mut guarded: Vec<String> = Vec::new();
    // Presentation events: step and event.
    let mut presents: Vec<(u64, Presentation)> = Vec::new();
    let mut limits: Vec<String> = Vec::new();
    let mut dialogs: Vec<(u64, String)> = Vec::new();
    let mut dialog_ends: Vec<u64> = Vec::new();
    let mut intro_end: Option<u64> = None;
    let mut quest_set_after_intro: Option<u64> = None;

    for n in 1..=STEPS {
        let before = state.script.mission.clone();
        let ids_before: Vec<Option<i32>> = state
            .ki
            .engines
            .iter()
            .map(|e| e.as_ref().map(|e| e.next_thread_id))
            .collect();
        let events = step(&mut state, &rules, &[]);
        for e in &events {
            if let SimEvent::Present(p) = e {
                presents.push((n, p.clone()));
                continue;
            }
            let SimEvent::Script(t) = e else { continue };
            match t {
                ScriptTrace::Fired { container, block } => {
                    let path = if *container < 0 {
                        dialog_path.clone().unwrap_or_else(|| "?".to_owned())
                    } else {
                        level_path.clone()
                    };
                    fired.push(Fired {
                        step: n,
                        container: *container,
                        block: *block,
                        path,
                    });
                    if *container >= 0 {
                        for k in ki_actions.iter().filter(|k| k.block == *block) {
                            let engine = state.ki.engines[k.slot].as_ref();
                            ki_seen.push(KiStartSeen {
                                action: *k,
                                step: n,
                                id_before: ids_before[k.slot],
                                id_after: engine.map(|e| e.next_thread_id),
                                thread_after: engine.is_some_and(|e| {
                                    e.threads.iter().any(|(_, t)| t.module == k.module)
                                }),
                            });
                        }
                    }
                }
                ScriptTrace::Command {
                    action,
                    code,
                    outcome,
                    ..
                } => {
                    let text = match outcome {
                        Outcome::Accepted => "Accepted".to_owned(),
                        Outcome::Partial(why) => format!("Partial: {why}"),
                        Outcome::Rejected(why) => format!("Rejected: {why}"),
                    };
                    commands.entry((*action, *code, text)).or_insert((0, n)).0 += 1;
                }
                ScriptTrace::Pending {
                    container,
                    block,
                    action,
                } => {
                    *pending.entry(*action).or_default() += 1;
                    if KI_START_ACTIONS.contains(action) {
                        ki_starts.push((n, *container, *block, *action));
                    }
                }
                ScriptTrace::ConditionPending { condition, .. } => {
                    *condition_pending.entry(*condition).or_default() += 1;
                }
                ScriptTrace::DialogStarted { path } => {
                    dialogs.push((n, path.clone()));
                    dialog_path = Some(path.clone());
                }
                ScriptTrace::DialogEnded => {
                    dialog_ends.push(n);
                    if dialog_path.as_deref() == Some(INTRO_DIALOG) {
                        intro_end.get_or_insert(n);
                    }
                }
                ScriptTrace::Guarded {
                    container,
                    block,
                    number,
                    why,
                } => guarded.push(format!(
                    "step {n}, container {container}, block {block}, number {number}: {why}"
                )),
                ScriptTrace::Limit { what } => limits.push(format!("step {n}: {what}")),
                ScriptTrace::Deleted { .. } | ScriptTrace::Suspended { .. } => {}
            }
        }
        let after = &state.script.mission;
        for (kind, old, new) in [
            ("Main", &before.main_quests, &after.main_quests),
            ("Sub", &before.sub_quests, &after.sub_quests),
        ] {
            for (index, (o, q)) in old.iter().zip(new).enumerate() {
                if o != q {
                    quests.push(QuestChange {
                        step: n,
                        kind,
                        index,
                        old: *o,
                        new: *q,
                    });
                }
            }
        }
        if intro_end.is_some()
            && quest_set_after_intro.is_none()
            && after.main_quests.iter().any(|q| q.status == QUEST_SET)
        {
            quest_set_after_intro = Some(n);
        }
    }

    // No action and no condition reports `Pending`/`ConditionPending` anymore.
    let unexpected_pending: Vec<i32> = pending.keys().copied().collect();
    let unexpected_conditions: Vec<i32> = condition_pending.keys().copied().collect();

    let text = report(&Report {
        version: &format!("{version:?}"),
        rules: &rules,
        fired: &fired,
        quests: &quests,
        end_quests: (
            &state.script.mission.main_quests,
            &state.script.mission.sub_quests,
        ),
        pending: &pending,
        condition_pending: &condition_pending,
        ki_starts: &ki_starts,
        ki_seen: &ki_seen,
        commands: &commands,
        ki_engines: state.ki.engines.iter().filter(|e| e.is_some()).count(),
        dialogs: &dialogs,
        dialog_ends: &dialog_ends,
        intro_end,
        quest_set_after_intro,
        guarded: &guarded,
        limits: &limits,
        countdown: (
            state.script.mission.countdown_container,
            state.script.mission.countdown_s,
        ),
    });
    if let Some(dir) = std::env::var_os("NML_SHOTS_OUT") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kamp1-1-mission.md");
        std::fs::write(&path, &text).unwrap();
        eprintln!("Report: {}", path.display());
    }

    assert_eq!(
        dialogs.first(),
        Some(&(1, INTRO_DIALOG.to_owned())),
        "intro starts in step 1"
    );
    assert!(guarded.is_empty(), "Guarded: {guarded:#?}");
    assert!(limits.is_empty(), "Limit: {limits:#?}");
    assert!(
        unexpected_pending.is_empty(),
        "Pending (no action may report it anymore): {unexpected_pending:?}"
    );
    assert!(
        unexpected_conditions.is_empty(),
        "ConditionPending (no condition may report it anymore): {unexpected_conditions:?}"
    );
    // The intro ends: the movement 153 brings the ship into the harbor, condition 27 becomes true,
    // intro block 2 runs. The level script sets the objectives afterwards (block 10, condition 5 on
    // variable 1).
    assert!(
        commands
            .keys()
            .any(|(a, _, o)| *a == 153 && o == "Accepted"),
        "movement 153 not accepted: {commands:?}"
    );
    assert!(
        fired
            .iter()
            .any(|f| f.container < 0 && f.block == INTRO_SHIP_BLOCK),
        "block {INTRO_SHIP_BLOCK} of the intro never ran"
    );
    let end = intro_end.expect("intro does not end");
    assert!(
        quest_set_after_intro.is_some(),
        "no main objective with status {QUEST_SET} after the intro (end step {end})"
    );
    // Presentation: the intro locks the controls and turns cinemascope on (step 2), speaks
    // (measured 11 `Speech`, all in the intro) and releases both in the step of its end;
    // afterwards the main objectives 20100/20101 stand at status 3.
    let intro_presents = || presents.iter().filter(|(n, _)| *n <= end).map(|(_, p)| p);
    let is_ui_lock = |p: &&Presentation| matches!(p, Presentation::UiLock { on: true });
    let is_cinema_on = |p: &&Presentation| matches!(p, Presentation::Cinemascope { on: true });
    assert!(
        intro_presents().any(|p| is_ui_lock(&p)),
        "no UiLock in the intro"
    );
    assert!(
        intro_presents().any(|p| is_cinema_on(&p)),
        "no cinemascope on in the intro"
    );
    let speeches = intro_presents()
        .filter(|p| matches!(p, Presentation::Speech { .. }))
        .count();
    assert!(speeches >= 10, "only {speeches} Speech in the intro");
    assert!(
        presents
            .iter()
            .any(|(n, p)| *n >= end && matches!(p, Presentation::Cinemascope { on: false })),
        "no cinemascope off after the intro"
    );
    assert!(
        presents
            .iter()
            .any(|(n, p)| *n >= end && matches!(p, Presentation::UiLock { on: false })),
        "no release of the controls after the intro"
    );
    for text in [20100, 20101] {
        assert!(
            state
                .script
                .mission
                .main_quests
                .iter()
                .any(|q| q.text == text && q.status == QUEST_SET),
            "main objective {text} not at status {QUEST_SET}: {:?}",
            state.script.mission.main_quests
        );
    }
    // AI starts from block 67: no `Pending`, every action ran, and every thread start (1) stands
    // afterwards as a thread of its module in the engine. Every action of the engine of a slot
    // consumes exactly one thread ID: the number of actions of the block for this slot is the
    // increase of the thread ID in the step of the block (an `n0` that did nothing would show up
    // here).
    assert!(ki_starts.is_empty(), "AI starts as Pending: {ki_starts:?}");
    let block_actions: Vec<_> = ki_actions
        .iter()
        .filter(|k| k.block == KI_START_BLOCK)
        .collect();
    for k in &block_actions {
        let seen = ki_seen
            .iter()
            .find(|s| s.action == **k)
            .unwrap_or_else(|| panic!("AI start {k:?} never ran"));
        let (Some(before), Some(after)) = (seen.id_before, seen.id_after) else {
            panic!("AI start without engine: {seen:?}");
        };
        let count = block_actions.iter().filter(|o| o.slot == k.slot).count() as i32;
        assert_eq!(
            after - before,
            count,
            "thread IDs of slot {} in step {}: {seen:?}",
            k.slot,
            seen.step
        );
        if k.number == 1 {
            assert!(seen.thread_after, "thread missing: {seen:?}");
        }
    }
    let rejected: Vec<_> = commands
        .keys()
        .filter(|(_, _, o)| o.starts_with("Rejected"))
        .collect();
    assert!(
        rejected.is_empty(),
        "rejected script commands: {rejected:?}"
    );
}

/// Content of the report.
struct Report<'a> {
    version: &'a str,
    rules: &'a Rules,
    fired: &'a [Fired],
    quests: &'a [QuestChange],
    end_quests: (&'a [Quest; 10], &'a [Quest; 10]),
    pending: &'a BTreeMap<i32, u64>,
    condition_pending: &'a BTreeMap<i32, u64>,
    ki_starts: &'a [(u64, i32, u32, i32)],
    ki_seen: &'a [KiStartSeen],
    commands: &'a BTreeMap<(i32, u32, String), (u64, u64)>,
    ki_engines: usize,
    dialogs: &'a [(u64, String)],
    dialog_ends: &'a [u64],
    intro_end: Option<u64>,
    quest_set_after_intro: Option<u64>,
    guarded: &'a [String],
    limits: &'a [String],
    countdown: (i32, i32),
}

/// Name and line of a block from the script file.
fn block_label(rules: &Rules, path: &str, block: u32) -> String {
    rules
        .scripts
        .script(path)
        .and_then(|s| s.file.blocks.get(block as usize))
        .map_or_else(
            || "?".to_owned(),
            |b| format!("line {} \"{}\"", b.line, b.name.trim()),
        )
}

fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn report(r: &Report) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "# Mission run kamp1-1\n");
    let _ = writeln!(
        w,
        "Edition: {}. Scene like `SCRIPT_GOLDEN`: `{LEVEL}`, seed `{SEED:#010x}`, AI on, difficulty 1, \
         script on, 55 ms per character, without commands, {STEPS} steps (five game minutes). Test \
         `crates/opennml-sim/tests/corpus_script_mission.rs`.\n",
        r.version
    );

    let _ = writeln!(w, "## Result\n");
    let _ = writeln!(
        w,
        "- Dialogs: {}",
        r.dialogs
            .iter()
            .map(|(n, p)| format!("step {n} {}", short(p)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(w, "- Dialog ends: steps {:?}", r.dialog_ends);
    let _ = writeln!(w, "- Intro end: {:?}", r.intro_end);
    let _ = writeln!(
        w,
        "- First main objective with status {QUEST_SET} after the intro: step {:?}",
        r.quest_set_after_intro
    );
    let _ = writeln!(
        w,
        "- Fired blocks: {} (level {}, dialog {})",
        r.fired.len(),
        r.fired.iter().filter(|f| f.container >= 0).count(),
        r.fired.iter().filter(|f| f.container < 0).count()
    );
    let _ = writeln!(
        w,
        "- Guarded: {}, Limit: {}",
        r.guarded.len(),
        r.limits.len()
    );
    let _ = writeln!(
        w,
        "- Countdown display at the end: container {}, {} s",
        r.countdown.0, r.countdown.1
    );
    let _ = writeln!(w, "- AI engines at the end: {}\n", r.ki_engines);
    for g in r.guarded.iter().chain(r.limits) {
        let _ = writeln!(w, "  - {g}");
    }

    let _ = writeln!(w, "## Mission objectives\n");
    let _ = writeln!(
        w,
        "| Step | Objective | Index | before (status/text) | after |"
    );
    let _ = writeln!(w, "|---|---|---|---|---|");
    if r.quests.is_empty() {
        let _ = writeln!(w, "| - | - | - | - | - |");
    }
    for q in r.quests {
        let _ = writeln!(
            w,
            "| {} | {} | {} | {}/{} | {}/{} |",
            q.step, q.kind, q.index, q.old.status, q.old.text, q.new.status, q.new.text
        );
    }
    let _ = writeln!(w, "\nState at the end (only used slots):\n");
    for (kind, list) in [("Main", r.end_quests.0), ("Sub", r.end_quests.1)] {
        for (i, q) in list.iter().enumerate() {
            if *q != Quest::default() {
                let _ = writeln!(
                    w,
                    "- {kind} objective {i}: status {}, text {}",
                    q.status, q.text
                );
            }
        }
    }

    let _ = writeln!(w, "\n## Commands of the script path\n");
    let _ = writeln!(w, "| Action | Command | Verdict | Count | first step |");
    let _ = writeln!(w, "|---|---|---|---|---|");
    if r.commands.is_empty() {
        let _ = writeln!(w, "| - | - | - | - | - |");
    }
    for ((a, code, outcome), (n, first)) in r.commands {
        let _ = writeln!(w, "| {a} | `{code:#04x}` | {outcome} | {n} | {first} |");
    }

    let _ = writeln!(w, "\n## Pending per action (layer D)\n");
    let _ = writeln!(w, "| Action | Name | Group | Count |");
    let _ = writeln!(w, "|---|---|---|---|");
    for (a, n) in r.pending {
        let name = opennml_formats::abl::catalog::action(*a).map_or("?", |s| s.name);
        let _ = writeln!(w, "| {a} | {name} | {} | {n} |", plan_of(*a));
    }
    let _ = writeln!(w, "\n## ConditionPending per condition\n");
    if r.condition_pending.is_empty() {
        let _ = writeln!(w, "none");
    }
    for (c, n) in r.condition_pending {
        let _ = writeln!(w, "- Condition {c}: {n}");
    }

    let _ = writeln!(
        w,
        "\n## AI starts (actions 0/1 of the level script, route W)\n"
    );
    let _ = writeln!(
        w,
        "`Pending` for 0/1/37/340: {} (expected 0).\n",
        r.ki_starts.len()
    );
    let _ = writeln!(
        w,
        "| Step | Block | Action | Slot | Module | next thread ID before -> after | Thread of the module afterwards |"
    );
    let _ = writeln!(w, "|---|---|---|---|---|---|---|");
    for s in r.ki_seen {
        let thread = match (s.action.number, s.thread_after) {
            (1, true) => "yes",
            (1, false) => "no",
            _ => "-",
        };
        let _ = writeln!(
            w,
            "| {} | {} | {} | {} | {} | {:?} -> {:?} | {thread} |",
            s.step,
            s.action.block,
            s.action.number,
            s.action.slot,
            s.action.module,
            s.id_before,
            s.id_after,
        );
    }

    let _ = writeln!(w, "\n## Block sequence\n");
    let _ = writeln!(
        w,
        "Every `Fired` message in reported order (container -1 = dialog); equal blocks in \
         consecutive messages merged.\n"
    );
    let _ = writeln!(w, "| Step(s) | Count | Container | File | Block | Head |");
    let _ = writeln!(w, "|---|---|---|---|---|---|");
    let mut i = 0;
    while i < r.fired.len() {
        let f = &r.fired[i];
        let mut j = i + 1;
        while j < r.fired.len()
            && r.fired[j].container == f.container
            && r.fired[j].block == f.block
            && r.fired[j].path == f.path
        {
            j += 1;
        }
        let last = r.fired[j - 1].step;
        let steps = if last == f.step {
            f.step.to_string()
        } else {
            format!("{}-{last}", f.step)
        };
        let _ = writeln!(
            w,
            "| {steps} | {} | {} | {} | {} | {} |",
            j - i,
            f.container,
            short(&f.path),
            f.block,
            block_label(r.rules, &f.path, f.block)
        );
        i = j;
    }
    out
}
