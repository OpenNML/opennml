//! Dry run of all level and dialog scripts of the original.
//!
//! AI off. Layer D takes effect for all numbers (presentation: route P); none reports `Pending`
//! anymore. The commands of the script path (route B) report `ScriptTrace::Command` per command; the
//! report counts them per action number, command type and verdict. Each map with a level script
//! runs in two versions of the conditions: (a) as implemented (all, including 35-38), (b) every
//! condition link true (`StepOptions::all_conditions_true`). Each reachable dialog script
//! additionally runs individually in version (b): match with script, level container removed,
//! dialog via `script::testing::start_dialog`.
//!
//! Expectation: no `ScriptTrace::Guarded`, no panic, `ScriptTrace::Limit` only for the info
//! history in version (b) (`LIMIT_ONLY_IN_B`), rejections of the script path only for
//! non-existent objects (`ALLOWED_REJECTIONS`); `Pending` not at all anymore (all actions are
//! implemented), `ConditionPending` not anymore (35-38 are implemented); in (b) every level script
//! with action 39 starts at least one dialog. The statistics per action number and per condition
//! arise solely from the events (`SimEvent::Script`) and the script files; the report goes as
//! Markdown to `$NML_SHOTS_OUT/dry-run.md` (without the variable only to `stderr`).
//!
//! "Reached" counts per touched block (`Fired`) the actions that `execute_block` passes through:
//! from the start action (from the block state before the step and `update_wait`) to the action at
//! which the block stops with `w` (`Suspended`), to the block end or to a `Guarded`. If a waiting
//! block (event waiting, time -1) continues, the wait action counts as "continuation", not as
//! reached (unless it stops again, e.g. `w40;-1`).

mod script_plan;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use nml_formats::abl::catalog::{self, CondEffect, Effect};
use nml_rda::Vfs;
use nml_rda::testutil::{corpus_game_dir, game_version};
use nml_sim::rules::Rules;
use nml_sim::script::{LoadedScript, testing};
use nml_sim::setup::{MENU_LEVEL_MODE, MatchSetup, new_match};
use nml_sim::sim::Sim;
use nml_sim::tick::{StepOptions, step_with};
use nml_world::level::{level_paths, load_level};

use script_plan::account::{
    Finding, Kind, RunSummary, Stats, account, assert_clean, waiting_blocks,
};

/// Map of the short run.
const SHORT_LEVEL: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
/// Steps of the short run per version.
const SHORT_STEPS: usize = 2_000;
/// Steps per map and version in the full run.
const LEVEL_STEPS: usize = 20_000;
/// Steps per dialog script in the full run.
const DIALOG_STEPS: usize = 5_000;
/// Maps with level script (`<base>.abl` next to the `.lvl`), sorted.
fn script_maps(vfs: &Vfs) -> Vec<String> {
    let mut maps: Vec<String> = level_paths(vfs)
        .into_iter()
        .filter(|lvl| vfs.exists(&format!("{}abl", lvl.trim_end_matches("lvl"))))
        .collect();
    maps.sort();
    maps
}

fn load_rules(vfs: &Vfs, lvl: &str) -> Rules {
    let scene = load_level(vfs, lvl).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
    let rules = Rules::load(vfs, &scene).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
    assert!(
        rules.scripts.errors.is_empty(),
        "{lvl}: {:?}",
        rules.scripts.errors
    );
    rules
}

/// Match with script, without AI. Maps and dialogs under `menülevel/` run in game type -2 like in
/// the original: without lobby, the slots come solely from the `.lvl`.
fn dry_setup(rules: &Rules, name: &str) -> MatchSetup {
    let mut setup = MatchSetup::default_for(rules);
    if name.contains("menülevel/") {
        setup.mode = MENU_LEVEL_MODE;
        setup.lobby.clear();
    }
    setup.ki.enabled = false;
    setup.script.enabled = true;
    setup
}

/// A run over `steps` steps. `dialog`: name of a dialog that runs individually (level container
/// removed). Panics are caught and counted as findings.
fn run(
    rules: &Rules,
    name: &str,
    kind: Kind,
    steps: usize,
    dialog: Option<&str>,
    stats: &mut Stats,
) -> RunSummary {
    let started = Instant::now();
    let mut summary = RunSummary {
        name: name.to_owned(),
        kind: Some(kind),
        ..RunSummary::default()
    };
    let options = StepOptions {
        all_conditions_true: kind.all_true(),
    };
    let label = format!("{name} {}", kind.label());
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut state = new_match(rules, &dry_setup(rules, name)).unwrap_or_else(|e| panic!("{e}"));
        if let Some(dialog) = dialog {
            state.script.level = None;
            let mut sim = Sim::new(&mut state, rules);
            testing::start_dialog(&mut sim, dialog);
            let events = std::mem::take(&mut sim.state.events);
            account(
                rules,
                kind,
                &label,
                0,
                None,
                None,
                &state,
                &events,
                stats,
                &mut summary,
            );
        }
        for n in 1..=steps {
            let pre_level = state.script.level.clone();
            let pre_dialog = state.script.dialog.clone();
            let events = step_with(&mut state, rules, &[], options);
            account(
                rules,
                kind,
                &label,
                n,
                pre_level.as_ref(),
                pre_dialog.as_ref(),
                &state,
                &events,
                stats,
                &mut summary,
            );
            summary.steps = n;
        }
        summary.waiting = waiting_blocks(rules, &state);
    }));
    if let Err(payload) = result {
        let msg = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic without text".to_owned());
        summary.panicked = true;
        stats.panics.push(Finding {
            run: format!("{label}, after step {}", summary.steps),
            what: msg,
        });
    }
    summary.millis = started.elapsed().as_millis();
    summary
}

/// Does the level script contain action 39?
fn starts_dialogs(rules: &Rules) -> bool {
    rules
        .scripts
        .level
        .iter()
        .flat_map(|s| &s.file.blocks)
        .flat_map(|b| &b.actions)
        .any(|a| a.number == 39)
}

/// Occurrences of every action and condition number in the loaded scripts (each file once).
#[derive(Default)]
struct Inventory {
    paths: BTreeSet<String>,
    actions: BTreeMap<i32, u64>,
    conditions: BTreeMap<i32, u64>,
    blocks: u64,
    /// Actions and conditions with fewer parameters than value slots in the catalog (the load check
    /// demands only the minimum number): `(path, block, line, text)`.
    short: Vec<(String, u32, u32, String)>,
}

impl Inventory {
    fn add(&mut self, script: &LoadedScript) {
        if !self.paths.insert(script.path.clone()) {
            return;
        }
        for (bi, b) in script.file.blocks.iter().enumerate() {
            self.blocks += 1;
            for a in &b.actions {
                *self.actions.entry(a.number).or_default() += 1;
                let declared = catalog::action(a.number).map_or(0, |s| s.params.len());
                if a.params.len() < declared {
                    self.short.push((
                        script.path.clone(),
                        bi as u32,
                        a.line,
                        format!(
                            "action {}{} with {} of {declared} parameters",
                            if a.wait { "w" } else { "" },
                            a.number,
                            a.params.len()
                        ),
                    ));
                }
            }
            for c in &b.conditions {
                *self.conditions.entry(c.number).or_default() += 1;
                let declared = catalog::condition(c.number).map_or(0, |s| s.params.len());
                if c.params.len() < declared {
                    self.short.push((
                        script.path.clone(),
                        bi as u32,
                        c.line,
                        format!(
                            "condition {} with {} of {declared} parameters",
                            c.number,
                            c.params.len()
                        ),
                    ));
                }
            }
        }
    }
}

fn triple(v: [u64; 3]) -> String {
    format!("{} / {} / {}", v[0], v[1], v[2])
}

fn action_note(number: i32) -> &'static str {
    match catalog::action(number).map(|s| s.effect) {
        None => "without catalog",
        Some(Effect::Internal) => "internal",
        Some(Effect::World) => "world",
        Some(Effect::Command) => "command",
        Some(Effect::Presentation) => "presentation",
        Some(Effect::NoEffect) => "no effect",
        Some(Effect::Unreachable) => "unreachable",
        Some(Effect::NoCase) => "no case",
    }
}

fn condition_note(number: i32) -> &'static str {
    match catalog::condition(number).map(|s| s.effect) {
        None => "without catalog",
        Some(CondEffect::NoCase) => "no case",
        Some(_) => "",
    }
}

/// Markdown report.
fn report(stats: &Stats, inventory: &Inventory, version: &str, total_ms: u128) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "# Dry run of all scripts\n");
    let _ = writeln!(
        w,
        "Edition: {version}. AI off, layer D implemented. Level (a): conditions as implemented \
         (all), {LEVEL_STEPS} steps; \
         Level (b): all conditions true, {LEVEL_STEPS} steps; Dialog (b): every reachable dialog script individually (level container \
         removed), all conditions true, {DIALOG_STEPS} steps. Columns with three values: \
         Level (a) / Level (b) / Dialog (b).\n"
    );
    let _ = writeln!(
        w,
        "Total run time (release, with loading the rules): {:.1} s\n",
        total_ms as f64 / 1000.0
    );

    // Summen.
    let mut sums: BTreeMap<Kind, RunSummary> = BTreeMap::new();
    for r in &stats.runs {
        let s = sums.entry(r.kind.unwrap()).or_default();
        s.steps += r.steps;
        s.fired += r.fired;
        s.deleted += r.deleted;
        s.suspended += r.suspended;
        s.pending += r.pending;
        s.condition_pending += r.condition_pending;
        s.dialogs_started += r.dialogs_started;
        s.dialogs_ended += r.dialogs_ended;
        s.commands += r.commands;
        s.rejected += r.rejected;
        s.guarded += r.guarded;
        s.limits += r.limits;
        s.millis += r.millis;
        s.name = format!(
            "{} runs",
            stats.runs.iter().filter(|x| x.kind == r.kind).count()
        );
    }
    let _ = writeln!(w, "## Sums\n");
    let _ = writeln!(
        w,
        "| Kind | Runs | Steps | Fired | Deleted | Suspended | Pending | ConditionPending | Dialog starts | Dialog ends | Commands | rejected | Guarded | Limit | Run time s |"
    );
    let _ = writeln!(
        w,
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for (kind, s) in &sums {
        let _ = writeln!(
            w,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.1} |",
            kind.label(),
            s.name,
            s.steps,
            s.fired,
            s.deleted,
            s.suspended,
            s.pending,
            s.condition_pending,
            s.dialogs_started,
            s.dialogs_ended,
            s.commands,
            s.rejected,
            s.guarded,
            s.limits,
            s.millis as f64 / 1000.0
        );
    }
    let total_blocks = inventory.blocks;
    let _ = writeln!(
        w,
        "\nScript files: {}, blocks: {total_blocks}; touched blocks (distinct): {} / {} / {}\n",
        inventory.paths.len(),
        stats.fired_blocks[0].len(),
        stats.fired_blocks[1].len(),
        stats.fired_blocks[2].len()
    );

    let _ = writeln!(w, "## Findings\n");
    let _ = writeln!(w, "### Guarded\n");
    if stats.guarded.is_empty() {
        let _ = writeln!(w, "none\n");
    }
    for (what, (n, first)) in &stats.guarded {
        let _ = writeln!(w, "- {n}x {what} (first {first})");
    }
    let _ = writeln!(w, "\n### Limit\n");
    if stats.limits.is_empty() {
        let _ = writeln!(w, "none\n");
    }
    for (what, (n, first)) in &stats.limits {
        let _ = writeln!(w, "- {n}x {what} (first {first})");
    }
    let _ = writeln!(w, "\n### Panics\n");
    if stats.panics.is_empty() {
        let _ = writeln!(w, "none\n");
    }
    for f in &stats.panics {
        let _ = writeln!(w, "- {}: {}", f.run, f.what);
    }

    let _ = writeln!(w, "\n### Missing value slots\n");
    let _ = writeln!(
        w,
        "Actions and conditions with fewer parameters than in the catalog (permitted from the minimum number); \
         \"touched\": block executed at least once in Level (a) / Level (b) / Dialog (b).\n"
    );
    if inventory.short.is_empty() {
        let _ = writeln!(w, "none\n");
    }
    for (path, block, line, what) in &inventory.short {
        let key = (path.clone(), *block);
        let touched = [0, 1, 2].map(|k| {
            if stats.fired_blocks[k].contains(&key) {
                "yes"
            } else {
                "no"
            }
        });
        let _ = writeln!(
            w,
            "- {path} line {line} (block {block}): {what}; touched {} / {} / {}",
            touched[0], touched[1], touched[2]
        );
    }

    let _ = writeln!(w, "\n## Actions\n");
    let _ = writeln!(
        w,
        "\"Occurrences\": number of actions with this number in all script files. \"Reached\": passed through by \
         `execute_block`; \"continuation\": wait action of a continuing block; \
         \"suspended\": `Suspended`; \"Pending\": effect not yet implemented.\n"
    );
    let _ = writeln!(
        w,
        "| No. | Kind | Group | Occurrences | reached | continuation | suspended | Pending |"
    );
    let _ = writeln!(w, "|---|---|---|---|---|---|---|---|");
    let numbers: BTreeSet<i32> = inventory
        .actions
        .keys()
        .chain(stats.actions.keys())
        .copied()
        .collect();
    for n in numbers {
        let s = stats.actions.get(&n).copied().unwrap_or_default();
        let _ = writeln!(
            w,
            "| {n} | {} | {} | {} | {} | {} | {} | {} |",
            action_note(n),
            script_plan::plan_of(n),
            inventory.actions.get(&n).copied().unwrap_or(0),
            triple(s.reached),
            triple(s.resumed),
            triple(s.suspended),
            triple(s.pending)
        );
    }
    let never: Vec<i32> = inventory
        .actions
        .keys()
        .filter(|n| {
            stats
                .actions
                .get(n)
                .is_none_or(|s| s.reached.iter().all(|&x| x == 0))
        })
        .copied()
        .collect();
    let _ = writeln!(w, "\nNever reached action numbers: {never:?}\n");

    let _ = writeln!(w, "## Commands of the script path\n");
    let _ = writeln!(
        w,
        "Per `ScriptTrace::Command`: action number, command type of the original, verdict (digits in the reason \
         replaced by `#`), count per run kind and first occurrence.\n"
    );
    let _ = writeln!(w, "| Action | Command | Verdict | Count | first |");
    let _ = writeln!(w, "|---|---|---|---|---|");
    if stats.commands.is_empty() {
        let _ = writeln!(w, "| - | - | - | - | - |");
    }
    for ((action, code, outcome), (v, first)) in &stats.commands {
        let _ = writeln!(
            w,
            "| {action} | `{code:#04x}` | {outcome} | {} | {first} |",
            triple(*v)
        );
    }
    let _ = writeln!(w);

    let _ = writeln!(w, "## Conditions\n");
    let _ = writeln!(w, "| No. | Kind | Occurrences | ConditionPending |");
    let _ = writeln!(w, "|---|---|---|---|");
    let numbers: BTreeSet<i32> = inventory
        .conditions
        .keys()
        .chain(stats.condition_pending.keys())
        .copied()
        .collect();
    for n in numbers {
        let _ = writeln!(
            w,
            "| {n} | {} | {} | {} |",
            condition_note(n),
            inventory.conditions.get(&n).copied().unwrap_or(0),
            triple(stats.condition_pending.get(&n).copied().unwrap_or_default())
        );
    }

    let _ = writeln!(w, "\n## Dialogs\n");
    let mut all: BTreeSet<&String> = BTreeSet::new();
    for m in &stats.dialogs_started {
        all.extend(m.keys());
    }
    let _ = writeln!(w, "| Dialog | Starts |");
    let _ = writeln!(w, "|---|---|");
    for d in all {
        let v = [0, 1, 2].map(|k| stats.dialogs_started[k].get(d).copied().unwrap_or(0));
        let _ = writeln!(w, "| {d} | {} |", triple(v));
    }

    let _ = writeln!(w, "\n## Runs\n");
    let _ = writeln!(
        w,
        "| Run | Kind | Steps | Fired | Deleted | Suspended | Pending | CondPending | Dialog starts | Commands | rejected | Guarded | Limit | Panic | ms | waiting at the end |"
    );
    let _ = writeln!(
        w,
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for r in &stats.runs {
        let _ = writeln!(
            w,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.name,
            r.kind.unwrap().label(),
            r.steps,
            r.fired,
            r.deleted,
            r.suspended,
            r.pending,
            r.condition_pending,
            r.dialogs_started,
            r.commands,
            r.rejected,
            r.guarded,
            r.limits,
            if r.panicked { "yes" } else { "" },
            r.millis,
            r.waiting.join(", ")
        );
    }
    out
}

/// Report to `$NML_SHOTS_OUT/dry-run.md`, without the variable to `stderr`.
fn write_report(text: &str) {
    match std::env::var_os("NML_SHOTS_OUT") {
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("dry-run.md");
            std::fs::write(&path, text).unwrap();
            eprintln!("Report: {}", path.display());
        }
        None => eprintln!("{text}"),
    }
}

/// Short run (always): `kamp1-1` 2,000 steps each in both versions of the conditions.
#[test]
fn dry_run_kamp1_1_short() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs, SHORT_LEVEL);
    let mut stats = Stats::default();
    for kind in [Kind::LevelA, Kind::LevelB] {
        let summary = run(&rules, SHORT_LEVEL, kind, SHORT_STEPS, None, &mut stats);
        eprintln!("{summary:?}");
        stats.runs.push(summary);
    }
    assert_clean(&stats);
    let b = &stats.runs[1];
    assert_eq!(b.steps, SHORT_STEPS);
    assert!(b.dialogs_started >= 1, "no dialog in (b)");
    assert!(stats.runs[0].fired > 0, "no block in (a)");

    // Counter-check of the evaluation: an unknown dialog reports `Guarded`, and the statistics see it.
    let mut probe = Stats::default();
    let summary = run(
        &rules,
        "Counter-check",
        Kind::Dialog,
        1,
        Some("does not exist"),
        &mut probe,
    );
    assert_eq!(summary.guarded, 1);
    assert_eq!(probe.guarded.len(), 1, "{:?}", probe.guarded);
}

/// Full run (`#[ignore]`, release): every map with a level script 20,000 steps in (a) and (b),
/// every reachable dialog script individually 5,000 steps in (b); report. Run time release ~1.5 h.
#[test]
#[ignore = "full run, long run time: cargo test --release -p nml-sim --test corpus_script_dryrun -- --ignored"]
fn dry_run_all_scripts() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let started = Instant::now();
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = format!("{:?}", game_version(&vfs).unwrap_or_else(|e| panic!("{e}")));
    let mut stats = Stats::default();
    let mut inventory = Inventory::default();
    let mut dialogs_done: BTreeSet<String> = BTreeSet::new();
    let mut silent_maps = Vec::new();
    for lvl in script_maps(&vfs) {
        let rules = load_rules(&vfs, &lvl);
        for s in rules
            .scripts
            .level
            .iter()
            .chain(rules.scripts.dialogs.values())
        {
            inventory.add(s);
        }
        for kind in [Kind::LevelA, Kind::LevelB] {
            let summary = run(&rules, &lvl, kind, LEVEL_STEPS, None, &mut stats);
            eprintln!(
                "{lvl} {}: {} ms, Fired {}, Guarded {}",
                kind.label(),
                summary.millis,
                summary.fired,
                summary.guarded
            );
            if kind == Kind::LevelB && starts_dialogs(&rules) && summary.dialogs_started == 0 {
                silent_maps.push(lvl.clone());
            }
            stats.runs.push(summary);
        }
        for (name, script) in &rules.scripts.dialogs {
            if !dialogs_done.insert(script.path.clone()) {
                continue;
            }
            let summary = run(
                &rules,
                &script.path,
                Kind::Dialog,
                DIALOG_STEPS,
                Some(name),
                &mut stats,
            );
            assert!(summary.dialogs_started >= 1, "{}", script.path);
            stats.runs.push(summary);
        }
    }
    let text = report(&stats, &inventory, &version, started.elapsed().as_millis());
    write_report(&text);
    assert!(
        silent_maps.is_empty(),
        "Level scripts with action 39 without a dialog start in (b): {silent_maps:?}"
    );
    assert_clean(&stats);
}
