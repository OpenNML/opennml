//! Corpus run of all campaign and tutorial missions.
//!
//! The 32 missions are `kampagne1` (6), `kampagne2` (10), `kampagne3` (14) and
//! `tutorial0-1`/`tutorial0-2` (2) from `level_paths`; `kampagne3/kamp3-4/oxdebug.lvl` does not
//! belong to them (file name differs from folder name). Each runs with difficulty 1, AI and script
//! on, without commands and without control-channel helpers, 24,000 steps (20 game minutes) or
//! until the local player result (slot 0) is determined.
//!
//! Expectation per mission: no panic, no `Guarded`, no `Pending`, no `ConditionPending`,
//! rejections of the script path only with `ALLOWED_REJECTIONS` (if a new reason appears with AI,
//! that is a finding), at least one block after the start block (a `Fired` from step 2). Whether
//! and when objectives (status 3/6/10) are set is a measured value of the report, not an expectation;
//! if a mission stays without an objective, the report names the blocks with objective action (27/182) that
//! never fired, together with their conditions.
//!
//! Always running: the mission count, a short run `tutorial0-1` (2,000 steps) and `kamp1-1` to the
//! end of the intro. The full run (`#[ignore]`, release) writes `$NML_SHOTS_OUT/corpus-run.md`
//! (per mission: fired blocks, dialogs, `Guarded`, script commands per verdict, objectives, `Present`
//! per kind, "148 without 147", game result, steps to the first objective; plus sums and the camera
//! binding 175 at the end). Without the original installation the tests skip silently.

mod script_plan;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use nml_rda::Vfs;
use nml_rda::testutil::{corpus_game_dir, game_version};
use nml_sim::game_end::{DEFEAT, VICTORY};
use nml_sim::script::{Presentation, Quest, ScriptTrace};
use nml_sim::state::SimEvent;
use nml_sim::tick::step;
use nml_world::level::level_paths;

use script_plan::account::{
    ALLOWED_REJECTIONS, Finding, Kind, RunSummary, Stats, account, assert_clean, waiting_blocks,
};

/// Steps per mission in the full run: 20 game minutes of 50 ms.
const FULL_STEPS: usize = 24_000;
/// Short run of the tutorial.
const SHORT_TUTORIAL: &str = "data/levels/tutorial0-1/kamp0-1.lvl";
const SHORT_TUTORIAL_STEPS: usize = 2_000;
/// Short run of the first campaign mission to the end of the intro (step 1,600).
const SHORT_CAMPAIGN: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
const SHORT_CAMPAIGN_LIMIT: usize = 3_000;
/// Name of the intro dialog of `kamp1-1` (end of the dialog = end of the intro).
const INTRO_DIALOG_END: &str = "dialogszene intro.abl";
/// Maximum number of worker threads in the full run (the missions are independent and each
/// deterministic on its own).
const WORKERS: usize = 4;
/// Status "set" of the actions 27/182.
const QUEST_SET: i32 = 3;

/// The 32 missions, sorted: campaigns (file name = folder name) and tutorials (`tutorial*`).
fn campaign_levels(vfs: &Vfs) -> Vec<String> {
    let mut out: Vec<String> = level_paths(vfs)
        .into_iter()
        .filter(|p| {
            let parts: Vec<&str> = p.split('/').collect();
            let stem = parts
                .last()
                .and_then(|f| f.strip_suffix(".lvl"))
                .unwrap_or("");
            match parts.as_slice() {
                ["data", "levels", group, folder, _] => {
                    group.starts_with("kampagne") && *folder == stem
                }
                ["data", "levels", group, _] => group.starts_with("tutorial"),
                _ => false,
            }
        })
        .collect();
    out.sort();
    out
}

/// Short name of the mission: folder name (campaigns) or file name (tutorials).
fn mission_name(lvl: &str) -> String {
    lvl.rsplit('/')
        .next()
        .and_then(|f| f.strip_suffix(".lvl"))
        .unwrap_or(lvl)
        .to_owned()
}

/// Kind of a `Present` event: the name of the variant (`Debug` up to the first `{`).
fn present_kind(p: &Presentation) -> String {
    format!("{p:?}")
        .split('{')
        .next()
        .unwrap_or("?")
        .trim_end()
        .to_owned()
}

/// When a run ends early.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Only at the steps or at the end of the game.
    Never,
    /// Also at the end of the intro dialog.
    IntroEnd,
}

/// Measured values of a mission.
struct MissionRun {
    lvl: String,
    name: String,
    steps: usize,
    /// Step and text of the game end (result of slot 0), otherwise `None`.
    end: Option<(usize, String)>,
    stats: Stats,
    summary: RunSummary,
    /// Step of the first `Fired` from step 2.
    first_fired_after_start: Option<usize>,
    /// End of the intro dialog (step), if there was one.
    intro_end: Option<usize>,
    present: BTreeMap<String, u64>,
    /// Number of `Cinemascope { on: true }` and `{ on: false }`.
    cinemascope_on: u64,
    cinemascope_off: u64,
    /// `Cinemascope { on: false }` without the last `Cinemascope` having been `on: true` (action 148 without 147).
    cinemascope_off_without_on: u64,
    /// Is a camera binding still set at the end (175 without a later 176 to 178)?
    camera_bound_at_end: bool,
    /// Per status (3/6/10) the objectives (kind, index) that ever reached it.
    goals_reached: BTreeMap<i32, BTreeSet<(&'static str, usize)>>,
    /// Step in which an objective first got status 3.
    first_goal_step: Option<usize>,
    /// `PlayerResult` in the reported order: (step, slot, result).
    results: Vec<(usize, u8, i32)>,
    /// Main and sub objectives at the end.
    end_quests: ([Quest; 10], [Quest; 10]),
    /// Blocks with objective action (27/182) that never fired, with their conditions (at most six).
    unfired_goal_blocks: Vec<String>,
    waiting: Vec<String>,
    millis: u128,
}

impl MissionRun {
    fn end_text(&self) -> String {
        self.end
            .as_ref()
            .map_or("step limit".to_owned(), |(n, t)| format!("{t} (step {n})"))
    }

    fn cinemascope_on_off(&self) -> String {
        format!("{}/{}", self.cinemascope_on, self.cinemascope_off)
    }
}

/// Result of a slot as text.
fn result_text(result: i32) -> String {
    match result {
        DEFEAT => "lost".to_owned(),
        VICTORY => "won".to_owned(),
        other => format!("result {other}"),
    }
}

/// Run one mission: difficulty 1, AI and script on, without commands.
fn run_mission(vfs: &Vfs, lvl: &str, max_steps: usize, stop: Stop) -> MissionRun {
    let started = Instant::now();
    let (rules, mut state) = script_plan::start_level(vfs, lvl, true);
    let name = mission_name(lvl);
    let label = format!("{name} KI");
    let mut m = MissionRun {
        lvl: lvl.to_owned(),
        name,
        steps: 0,
        end: None,
        stats: Stats::default(),
        summary: RunSummary {
            name: lvl.to_owned(),
            kind: Some(Kind::LevelA),
            ..RunSummary::default()
        },
        first_fired_after_start: None,
        intro_end: None,
        present: BTreeMap::new(),
        cinemascope_on: 0,
        cinemascope_off: 0,
        cinemascope_off_without_on: 0,
        camera_bound_at_end: false,
        goals_reached: BTreeMap::new(),
        first_goal_step: None,
        results: Vec::new(),
        end_quests: Default::default(),
        unfired_goal_blocks: Vec::new(),
        waiting: Vec::new(),
        millis: 0,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut cinemascope = false;
        let mut dialog_path = String::new();
        for n in 1..=max_steps {
            let pre_level = state.script.level.clone();
            let pre_dialog = state.script.dialog.clone();
            let before = (
                state.script.mission.main_quests,
                state.script.mission.sub_quests,
            );
            let events = step(&mut state, &rules, &[]);
            account(
                &rules,
                Kind::LevelA,
                &label,
                n,
                pre_level.as_ref(),
                pre_dialog.as_ref(),
                &state,
                &events,
                &mut m.stats,
                &mut m.summary,
            );
            m.steps = n;
            m.summary.steps = n;
            let mut intro_ended = false;
            for e in &events {
                match e {
                    SimEvent::Script(ScriptTrace::Fired { .. })
                        if n >= 2 && m.first_fired_after_start.is_none() =>
                    {
                        m.first_fired_after_start = Some(n);
                    }
                    SimEvent::Script(ScriptTrace::DialogStarted { path }) => {
                        dialog_path = path.clone();
                    }
                    SimEvent::Script(ScriptTrace::DialogEnded)
                        if stop == Stop::IntroEnd && dialog_path.ends_with(INTRO_DIALOG_END) =>
                    {
                        m.intro_end = Some(n);
                        intro_ended = true;
                    }
                    SimEvent::Present(p) => {
                        *m.present.entry(present_kind(p)).or_default() += 1;
                        match p {
                            Presentation::Cinemascope { on } => {
                                if *on {
                                    m.cinemascope_on += 1;
                                } else {
                                    m.cinemascope_off += 1;
                                    if !cinemascope {
                                        m.cinemascope_off_without_on += 1;
                                    }
                                }
                                cinemascope = *on;
                            }
                            Presentation::CameraBind { id } => {
                                m.camera_bound_at_end = id.is_some();
                            }
                            _ => {}
                        }
                    }
                    SimEvent::PlayerResult { slot, result } => {
                        m.results.push((n, *slot, *result));
                        if *slot == 0 && m.end.is_none() {
                            m.end = Some((n, result_text(*result)));
                        }
                    }
                    _ => {}
                }
            }
            let after = (
                state.script.mission.main_quests,
                state.script.mission.sub_quests,
            );
            for (kind, old, new) in [("Main", &before.0, &after.0), ("Sub", &before.1, &after.1)] {
                for (index, (o, q)) in old.iter().zip(new).enumerate() {
                    if o.status != q.status {
                        m.goals_reached
                            .entry(q.status)
                            .or_default()
                            .insert((kind, index));
                        if q.status == QUEST_SET && m.first_goal_step.is_none() {
                            m.first_goal_step = Some(n);
                        }
                    }
                }
            }
            if m.end.is_some() || intro_ended {
                break;
            }
        }
        m.end_quests = (
            state.script.mission.main_quests,
            state.script.mission.sub_quests,
        );
        m.waiting = waiting_blocks(&rules, &state);
    }));
    if let Err(payload) = outcome {
        let msg = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic without text".to_owned());
        m.stats.panics.push(Finding {
            run: format!("{label}, after step {}", m.steps),
            what: msg,
        });
    }
    if let Some(script) = &rules.scripts.level {
        let fired: BTreeSet<u32> = m.stats.fired_blocks[0]
            .iter()
            .filter(|(path, _)| *path == script.path)
            .map(|(_, b)| *b)
            .collect();
        for (i, b) in script.file.blocks.iter().enumerate() {
            if fired.contains(&(i as u32)) || m.unfired_goal_blocks.len() >= 6 {
                continue;
            }
            if b.actions.iter().any(|a| a.number == 27 || a.number == 182) {
                let conditions: Vec<String> = b
                    .conditions
                    .iter()
                    .map(|c| {
                        format!(
                            "{}{}({})",
                            if c.or { "|" } else { "&" },
                            c.number,
                            c.params.join(",")
                        )
                    })
                    .collect();
                m.unfired_goal_blocks.push(format!(
                    "Block {i} (line {}) waits for {}",
                    b.line,
                    conditions.join(" ")
                ));
            }
        }
    }
    m.millis = started.elapsed().as_millis();
    m
}

/// The common expectations for a mission (module doc comment).
fn assert_mission(m: &MissionRun) {
    assert_clean(&m.stats);
    assert!(
        m.first_fired_after_start.is_some(),
        "{}: no block after the start block (Fired from step 2)",
        m.name
    );
}

/// Counts the verdicts of the commands of a mission: (accepted, partial, rejected).
fn outcome_counts(stats: &Stats) -> (u64, u64, u64) {
    let mut c = (0, 0, 0);
    for ((_, _, o), (n, _)) in &stats.commands {
        let n: u64 = n.iter().sum();
        if o == "Accepted" {
            c.0 += n;
        } else if o.starts_with("Partial") {
            c.1 += n;
        } else {
            c.2 += n;
        }
    }
    c
}

fn quest_line(quests: &[Quest; 10]) -> String {
    let parts: Vec<String> = quests
        .iter()
        .enumerate()
        .filter(|(_, q)| q.status != 0)
        .map(|(i, q)| format!("{i}:{}", q.status))
        .collect();
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join(" ")
    }
}

fn goals_at(m: &MissionRun, status: i32) -> usize {
    m.goals_reached.get(&status).map_or(0, BTreeSet::len)
}

fn results_text(m: &MissionRun) -> String {
    if m.results.is_empty() {
        return "open".to_owned();
    }
    m.results
        .iter()
        .map(|(n, slot, r)| format!("{slot}:{} (step {n})", result_text(*r)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn report(runs: &[MissionRun], version: &str, wall_ms: u128) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "# Corpus run of the campaign and tutorial missions\n");
    let _ = writeln!(
        w,
        "Edition {version}; {} missions, difficulty 1, AI and script on, without commands, at most {FULL_STEPS} \
         steps (20 game minutes) or until slot 0 has a result. Run time {} s (wall clock, {WORKERS} threads), \
         sum of the missions {} s.\n",
        runs.len(),
        wall_ms / 1000,
        runs.iter().map(|m| m.millis).sum::<u128>() / 1000
    );

    let _ = writeln!(w, "## Overview\n");
    let _ = writeln!(
        w,
        "| Mission | Steps | End | Blocks (distinct / firings) | Dialogs | Guarded | Pending | Commands ok/partial/rejected | \
         Objectives 3/6/10 | to 1st objective | Speech | Cinemascope on/off | 148 without 147 | Binding 175 at end | Game result |"
    );
    let _ = writeln!(
        w,
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for m in runs {
        let (acc, part, rej) = outcome_counts(&m.stats);
        let _ = writeln!(
            w,
            "| {} | {} | {} | {} / {} | {} | {} | {} | {acc}/{part}/{rej} | {}/{}/{} | {} | {} | {} | {} | {} | {} |",
            m.name,
            m.steps,
            m.end_text(),
            m.stats.fired_blocks[0].len(),
            m.summary.fired,
            m.summary.dialogs_started,
            m.summary.guarded,
            m.summary.pending + m.summary.condition_pending,
            goals_at(m, 3),
            goals_at(m, 6),
            goals_at(m, 10),
            m.first_goal_step.map_or("-".to_owned(), |n| n.to_string()),
            m.present.get("Speech").copied().unwrap_or(0),
            m.cinemascope_on_off(),
            m.cinemascope_off_without_on,
            if m.camera_bound_at_end { "yes" } else { "no" },
            results_text(m),
        );
    }

    let _ = writeln!(w, "\n## Sums\n");
    let bound = runs.iter().filter(|m| m.camera_bound_at_end).count();
    let off_without_on: u64 = runs.iter().map(|m| m.cinemascope_off_without_on).sum();
    let off_missions = runs
        .iter()
        .filter(|m| m.cinemascope_off_without_on > 0)
        .count();
    let _ = writeln!(
        w,
        "- Camera binding 175 still stands at the end of the mission (without a later 176 to 178): {bound} of {} missions.",
        runs.len()
    );
    let _ = writeln!(
        w,
        "- \"148 without 147\" (`Cinemascope {{ on: false }}`, without the last `Cinemascope` having been on): {off_without_on} \
         times in {off_missions} missions.",
    );
    let no_goal: Vec<&str> = runs
        .iter()
        .filter(|m| m.first_goal_step.is_none())
        .map(|m| m.name.as_str())
        .collect();
    let _ = writeln!(
        w,
        "- Missions without an objective with status 3 in the run: {} ({}).",
        no_goal.len(),
        no_goal.join(", ")
    );
    let ended: Vec<String> = runs
        .iter()
        .filter_map(|m| {
            m.end
                .as_ref()
                .map(|(n, t)| format!("{} {t} (step {n})", m.name))
        })
        .collect();
    let _ = writeln!(
        w,
        "- Missions with game end before the step limit: {} ({}).",
        ended.len(),
        ended.join(", ")
    );
    let mut total: BTreeMap<&str, (u64, usize)> = BTreeMap::new();
    for m in runs {
        for (k, n) in &m.present {
            let e = total.entry(k.as_str()).or_default();
            e.0 += n;
            e.1 += 1;
        }
    }
    let _ = writeln!(w, "\n### `Present` per kind over all missions\n");
    let _ = writeln!(w, "| Kind | Events | Missions |");
    let _ = writeln!(w, "|---|---|---|");
    for (k, (n, missions)) in &total {
        let _ = writeln!(w, "| {k} | {n} | {missions} |");
    }

    let _ = writeln!(
        w,
        "\n### Rejections and verdicts of the script path (all missions)\n"
    );
    let mut by_outcome: BTreeMap<(i32, String), (u64, String)> = BTreeMap::new();
    for m in runs {
        for ((action, _, o), (n, first)) in &m.stats.commands {
            if o != "Accepted" {
                let e = by_outcome
                    .entry((*action, o.clone()))
                    .or_insert((0, format!("{}: {first}", m.name)));
                e.0 += n.iter().sum::<u64>();
            }
        }
    }
    if by_outcome.is_empty() {
        let _ = writeln!(w, "none");
    } else {
        let _ = writeln!(
            w,
            "| Action | Verdict | allowed | Count | first occurrence |"
        );
        let _ = writeln!(w, "|---|---|---|---|---|");
        for ((action, o), (n, first)) in &by_outcome {
            let allowed = ALLOWED_REJECTIONS.contains(&o.as_str());
            let _ = writeln!(
                w,
                "| {action} | {o} | {} | {n} | {first} |",
                if allowed { "yes" } else { "NO" }
            );
        }
    }

    let _ = writeln!(w, "\n## Individual missions\n");
    for m in runs {
        let _ = writeln!(w, "### {} (`{}`)\n", m.name, m.lvl);
        let _ = writeln!(
            w,
            "- Steps {}, run time {} ms, end: {}",
            m.steps,
            m.millis,
            m.end
                .as_ref()
                .map_or("step limit".to_owned(), |(n, t)| format!("{t} in step {n}"))
        );
        let _ = writeln!(
            w,
            "- Blocks: {} distinct fired, {} firings, {} deleted, {} suspended; first `Fired` from step 2: {}",
            m.stats.fired_blocks[0].len(),
            m.summary.fired,
            m.summary.deleted,
            m.summary.suspended,
            m.first_fired_after_start
                .map_or("none".to_owned(), |n| format!("step {n}"))
        );
        let _ = writeln!(
            w,
            "- Dialogs started {}, ended {}; `Guarded` {}; `Limit` {}; panic: {}",
            m.summary.dialogs_started,
            m.summary.dialogs_ended,
            m.summary.guarded,
            m.summary.limits,
            if m.stats.panics.is_empty() {
                "no"
            } else {
                "YES"
            }
        );
        let _ = writeln!(
            w,
            "- Objectives at the end: Main {}; Sub {}; status 3 reached by {} objectives, 6 by {}, 10 by {}; first objective in step {}",
            quest_line(&m.end_quests.0),
            quest_line(&m.end_quests.1),
            goals_at(m, 3),
            goals_at(m, 6),
            goals_at(m, 10),
            m.first_goal_step.map_or("-".to_owned(), |n| n.to_string())
        );
        let _ = writeln!(w, "- Game result: {}", results_text(m));
        if !m.present.is_empty() {
            let kinds: Vec<String> = m.present.iter().map(|(k, n)| format!("{k} {n}")).collect();
            let _ = writeln!(w, "- `Present`: {}", kinds.join(", "));
        }
        let _ = writeln!(
            w,
            "- 148 without 147: {}; camera binding 175 at the end: {}",
            m.cinemascope_off_without_on,
            if m.camera_bound_at_end { "yes" } else { "no" }
        );
        if !m.stats.commands.is_empty() {
            let _ = writeln!(w, "- Script commands (action, code, verdict: count):");
            for ((action, code, o), (n, first)) in &m.stats.commands {
                let _ = writeln!(
                    w,
                    "  - {action}, `{code:#04x}`, {o}: {}{}",
                    n.iter().sum::<u64>(),
                    if o == "Accepted" {
                        String::new()
                    } else {
                        format!(" (first: {first})")
                    }
                );
            }
        }
        if m.first_goal_step.is_none() {
            let _ = writeln!(
                w,
                "- **Without objective.** Blocks with objective action that never fired:"
            );
            if m.unfired_goal_blocks.is_empty() {
                let _ = writeln!(
                    w,
                    "  - none (all objective blocks fired or the script has no objective action)"
                );
            }
            for b in &m.unfired_goal_blocks {
                let _ = writeln!(w, "  - {b}");
            }
        }
        if !m.waiting.is_empty() {
            let _ = writeln!(w, "- Waiting at the end: {}", m.waiting.join("; "));
        }
        let _ = writeln!(w);
    }
    out
}

fn write_report(text: &str) {
    match std::env::var_os("NML_SHOTS_OUT") {
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("corpus-run.md");
            std::fs::write(&path, text).unwrap();
            eprintln!("Report: {}", path.display());
        }
        None => eprintln!("{text}"),
    }
}

#[test]
fn the_campaign_corpus_has_32_missions() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let levels = campaign_levels(&vfs);
    let count = |prefix: &str| {
        levels
            .iter()
            .filter(|l| l.starts_with(&format!("data/levels/{prefix}")))
            .count()
    };
    assert_eq!(
        (
            count("kampagne1/"),
            count("kampagne2/"),
            count("kampagne3/"),
            count("tutorial")
        ),
        (6, 10, 14, 2),
        "{levels:#?}"
    );
    assert_eq!(levels.len(), 32);
    assert!(
        !levels.iter().any(|l| l.contains("oxdebug")),
        "oxdebug does not belong to the corpus"
    );
}

#[test]
fn present_kind_is_the_variant_name() {
    assert_eq!(present_kind(&Presentation::CameraBackup), "CameraBackup");
    assert_eq!(
        present_kind(&Presentation::Cinemascope { on: true }),
        "Cinemascope"
    );
    assert_eq!(
        present_kind(&Presentation::Speech {
            text: 1,
            slot: 0,
            ms: 5
        }),
        "Speech"
    );
}

/// Short run (always): `tutorial0-1` 2,000 steps with AI.
#[test]
fn tutorial0_1_short_run_is_clean() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let m = run_mission(&vfs, SHORT_TUTORIAL, SHORT_TUTORIAL_STEPS, Stop::Never);
    eprintln!("{} steps, {:?}", m.steps, m.summary);
    assert_mission(&m);
    assert_eq!(
        m.steps, SHORT_TUTORIAL_STEPS,
        "game end before 2,000 steps: {:?}",
        m.end
    );
}

/// Short run (always): `kamp1-1` to the end of the intro. The intro shows presentation: cinemascope on and off.
#[test]
fn kamp1_1_runs_to_the_end_of_the_intro() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let m = run_mission(&vfs, SHORT_CAMPAIGN, SHORT_CAMPAIGN_LIMIT, Stop::IntroEnd);
    eprintln!(
        "{} steps, intro end {:?}, {:?}",
        m.steps, m.intro_end, m.present
    );
    assert_mission(&m);
    assert!(
        m.intro_end.is_some(),
        "intro does not end in {SHORT_CAMPAIGN_LIMIT} steps"
    );
    assert!(
        m.cinemascope_on >= 1 && m.cinemascope_off >= 1,
        "cinemascope on/off in the intro: {:?}",
        m.present
    );
}

/// Full run (`#[ignore]`, release): all 32 missions with AI, 24,000 steps or until the game end; report.
#[test]
#[ignore = "full run, long run time: cargo test --release -p nml-sim --test corpus_script_campaign -- --ignored"]
fn all_campaign_missions_run_clean() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let started = Instant::now();
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = format!("{:?}", game_version(&vfs).unwrap_or_else(|e| panic!("{e}")));
    let levels = campaign_levels(&vfs);
    assert_eq!(levels.len(), 32, "{levels:#?}");
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, MissionRun)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                let vfs = Vfs::open_game_dir(&dir).unwrap();
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(lvl) = levels.get(i) else { break };
                    let m = run_mission(&vfs, lvl, FULL_STEPS, Stop::Never);
                    eprintln!(
                        "{}: {} steps, {} ms, Fired {}, Guarded {}, objectives {}, end {:?}",
                        m.name,
                        m.steps,
                        m.millis,
                        m.summary.fired,
                        m.summary.guarded,
                        goals_at(&m, QUEST_SET),
                        m.end
                    );
                    done.lock().unwrap().push((i, m));
                }
            });
        }
    });
    let mut runs = done.into_inner().unwrap();
    runs.sort_by_key(|(i, _)| *i);
    let runs: Vec<MissionRun> = runs.into_iter().map(|(_, m)| m).collect();
    write_report(&report(&runs, &version, started.elapsed().as_millis()));
    for m in &runs {
        assert_mission(m);
    }
}
