//! Victory and defeat of `kamp1-1` via the scripts: scene like `SCRIPT_GOLDEN` (seed "NML2", AI
//! on, difficulty 1, script on, 55 ms per character). After the intro the acceptance helper
//! `DebugDestroy` (no counterpart in the original, sender = owner) destroys the objects the level
//! script waits for; victory and defeat are set solely by the script (actions 13/4, command `0x56`
//! via the script path).
//!
//! - **Defeat:** "SPA Haupthaus" destroyed -> block "Wenn Spieler verliert" (condition 24) -> `n4` ->
//!   result of slot 6 = 1 and `SimEvent::PlayerResult`.
//! - **Victory:** "Drake Hauptquartier", "ENG HAFEN1", "ENG HAFEN2" destroyed -> Drake appears
//!   (switch 26 at difficulty 1); `DRAKE1` below 50 % hit points (like script action 201,
//!   `script::world::objects::set_hp`) -> switch 50 (condition 28) -> timer 35 (15 s) -> dialog
//!   "Endsequenz" and switch "ende" -> after the dialog block "Wenn Gewonnen": two blocking tips
//!   (20118, 20119), then `n13` -> slot 6 wins, every slot that was still playing and is not
//!   mutually friendly loses (`game_end::set_player_result`).
//!   `DebugDestroy` on `DRAKE1` does **not** set switch 50: the unit dies with full hit points,
//!   its death removes the name immediately (slot 8, E-145), query 1400 then returns -1 and
//!   condition 28 stays false (as in the original; quirk E-227, counter-check in the victory
//!   test).
//!
//! At most [`MAX_STEPS`] steps per run. The report (block sequence, steps, commands of the script
//! path, results) goes to `$NML_SHOTS_OUT/kamp1-1-end.md` (one section per test); without the
//! variable the test writes nothing. Without the original installation it skips silently.

use std::fmt::Write as _;
use std::sync::Mutex;

use nml_rda::Vfs;
use nml_rda::testutil::{corpus_game_dir, game_version};
use nml_sim::command::{Command, CommandKind, Origin, Outcome};
use nml_sim::rules::Rules;
use nml_sim::script::ScriptTrace;
use nml_sim::setup::{MatchSetup, new_match};
use nml_sim::sim::Sim;
use nml_sim::state::{SimEvent, SimState};
use nml_sim::tick::step;
use nml_world::level::load_level;

/// Map of the mission.
const LEVEL: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
/// Dialog that the level script starts in step 1.
const INTRO_DIALOG: &str = "data/levels/kampagne1/kamp1-1/dialogszene intro.abl";
/// Dialog before the victory (block "Drake Final", action 39).
const END_DIALOG: &str = "data/levels/kampagne1/kamp1-1/endsequenz.abl";
/// Start index of the random sequence like `SCRIPT_GOLDEN` ("NML2").
const SEED: u32 = 0x4E4D_4C32;
/// Maximum number of steps per run.
const MAX_STEPS: u64 = 40_000;
/// Slot of the player (Spaniards, "Player 6" in the script).
const PLAYER: u8 = 6;
/// Results (`GameEngine_SetPlayerResult`).
const DEFEAT: i32 = 1;
const VICTORY: i32 = 2;
/// Command type "set game result".
const SET_PLAYER_RESULT: u32 = 0x56;
/// Tips of the block "Wenn Gewonnen" (`w183`).
const VICTORY_TIPS: [i32; 2] = [20118, 20119];
/// Steps of the counter-check "DebugDestroy on DRAKE1".
const PROBE_STEPS: u64 = 200;
/// Hit points for the battered Drake (clearly below 50 % of the warship). Set in the step in which
/// Drake appears: the maximum is then still that of type 2010 (1,600, i.e. 6 %); one step later
/// the block "Wenn Drake erscheint" (switch 8, difficulty 1) sets it to 8,000 with action 192.
const DRAKE_HP: i32 = 100;
/// Attribute hit points.
const HIT_POINTS: u32 = 14;
/// Timer 35 of the block "Drake Final" in ms.
const FINAL_TIMER_MS: i32 = 15_000;

/// Shared report of both tests (one section per test).
static REPORT: Mutex<()> = Mutex::new(());

/// Fired block: step, container, block, script path.
struct Fired {
    step: u64,
    container: i32,
    block: u32,
    path: String,
}

/// A run with log.
struct Run<'r> {
    rules: &'r Rules,
    state: SimState,
    level_path: String,
    dialog_path: Option<String>,
    fired: Vec<Fired>,
    /// Commands of the script path: step, action, command type, verdict.
    commands: Vec<(u64, i32, u32, Outcome)>,
    /// `SimEvent::PlayerResult`: step, slot, result.
    results: Vec<(u64, u8, i32)>,
    dialogs: Vec<(u64, String)>,
    dialog_ends: Vec<(u64, String)>,
    guarded: Vec<String>,
    /// Milestones for the report: step, text.
    milestones: Vec<(u64, String)>,
}

impl<'r> Run<'r> {
    fn new(rules: &'r Rules) -> Self {
        let mut setup = MatchSetup::default_for(rules);
        setup.seed = SEED;
        setup.ki.enabled = true;
        setup.ki.campaign_difficulty = 1;
        setup.script.enabled = true;
        setup.script.time_per_char = 55;
        let state = new_match(rules, &setup).unwrap();
        let level_path = state
            .script
            .level
            .as_ref()
            .expect("level container")
            .source
            .path
            .clone();
        Self {
            rules,
            state,
            level_path,
            dialog_path: None,
            fired: Vec::new(),
            commands: Vec::new(),
            results: Vec::new(),
            dialogs: Vec::new(),
            dialog_ends: Vec::new(),
            guarded: Vec::new(),
            milestones: Vec::new(),
        }
    }

    fn steps(&self) -> u64 {
        self.state.steps
    }

    fn mark(&mut self, what: impl Into<String>) {
        let n = self.steps();
        self.milestones.push((n, what.into()));
    }

    /// One step without commands; messages into the log.
    fn step(&mut self) {
        assert!(
            self.steps() < MAX_STEPS,
            "limit of {MAX_STEPS} steps reached"
        );
        let events = step(&mut self.state, self.rules, &[]);
        let n = self.steps();
        for e in &events {
            match e {
                SimEvent::PlayerResult { slot, result } => self.results.push((n, *slot, *result)),
                SimEvent::Script(t) => match t {
                    ScriptTrace::Fired { container, block } => {
                        let path = if *container < 0 {
                            self.dialog_path.clone().unwrap_or_else(|| "?".to_owned())
                        } else {
                            self.level_path.clone()
                        };
                        self.fired.push(Fired {
                            step: n,
                            container: *container,
                            block: *block,
                            path,
                        });
                    }
                    ScriptTrace::Command {
                        action,
                        code,
                        outcome,
                        ..
                    } => self.commands.push((n, *action, *code, outcome.clone())),
                    ScriptTrace::DialogStarted { path } => {
                        self.dialogs.push((n, path.clone()));
                        self.dialog_path = Some(path.clone());
                    }
                    ScriptTrace::DialogEnded => {
                        let path = self.dialog_path.clone().unwrap_or_default();
                        self.dialog_ends.push((n, path));
                    }
                    ScriptTrace::Guarded {
                        container,
                        block,
                        number,
                        why,
                    } => self.guarded.push(format!(
                        "step {n}, container {container}, block {block}, number {number}: {why}"
                    )),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Steps until `done` holds; fails at [`MAX_STEPS`] with `what`.
    fn run_until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        while !done(self) {
            assert!(
                self.steps() < MAX_STEPS,
                "{what}: not reached in {MAX_STEPS} steps"
            );
            self.step();
        }
        self.mark(what);
    }

    fn intro_ended(&self) -> bool {
        self.dialog_ends.iter().any(|(_, p)| p == INTRO_DIALOG)
    }

    /// Switches of the level container (names are stored in lowercase in the state).
    fn switch_set(&self, name: &str) -> bool {
        self.state
            .script
            .level
            .as_ref()
            .is_some_and(|c| c.switches.contains_key(&name.to_lowercase()))
    }

    /// `DebugDestroy` on all IDs of the names, one command per owner with him as sender (only own
    /// objects). Every command must take full effect.
    fn destroy(&mut self, names: &[&str]) {
        let mut by_owner: std::collections::BTreeMap<u8, Vec<_>> = Default::default();
        for name in names {
            let ids = nml_sim::names::ids(&self.state.names, name);
            let alive: Vec<_> = ids
                .into_iter()
                .filter(|id| self.state.object(*id, true).is_some())
                .collect();
            assert!(!alive.is_empty(), "\"{name}\" has no living object");
            for id in alive {
                by_owner.entry(id.slot()).or_default().push(id);
            }
        }
        for (owner, objects) in by_owner {
            let cmd = Command {
                player: owner,
                kind: CommandKind::DebugDestroy { objects },
            };
            let mut sim = Sim::new(&mut self.state, self.rules);
            let outcome = nml_sim::player::submit(&mut sim, &cmd, Origin::External);
            assert_eq!(outcome, Outcome::Accepted, "{cmd:?}");
        }
        self.mark(format!("DebugDestroy {names:?}"));
    }

    fn command_steps(&self, action: i32) -> Vec<(u64, &Outcome)> {
        self.commands
            .iter()
            .filter(|c| c.1 == action)
            .map(|c| (c.0, &c.3))
            .collect()
    }
}

fn rules() -> Option<(Rules, String)> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap_or_else(|e| panic!("{LEVEL}: {e:#}"));
    Some((rules, format!("{version:?}")))
}

#[test]
fn kamp1_1_ends_in_defeat() {
    let Some((rules, version)) = rules() else {
        return;
    };
    let mut run = Run::new(&rules);
    run.run_until("intro ended", Run::intro_ended);
    assert_eq!(run.state.players[usize::from(PLAYER)].result, 0);
    run.destroy(&["SPA Haupthaus"]);
    run.run_until("result of slot 6 set", |r| {
        r.state.players[usize::from(PLAYER)].result != 0
    });
    // One step more: the rest of the block and later blocks keep running.
    run.step();

    let defeat = run.command_steps(4);
    let section = report_section("Defeat", &run, &version);
    write_report("## Defeat", &section);

    assert!(run.guarded.is_empty(), "Guarded: {:#?}", run.guarded);
    assert_eq!(run.state.players[usize::from(PLAYER)].result, DEFEAT);
    assert_eq!(defeat.len(), 1, "n4 exactly once: {defeat:?}");
    assert_eq!(*defeat[0].1, Outcome::Accepted);
    assert!(
        run.commands
            .iter()
            .any(|c| c.1 == 4 && c.2 == SET_PLAYER_RESULT),
        "n4 not as command 0x56"
    );
    assert!(
        run.results.contains(&(defeat[0].0, PLAYER, DEFEAT)),
        "SimEvent::PlayerResult missing: {:?}",
        run.results
    );
    // No victory.
    assert!(run.command_steps(13).is_empty());
}

#[test]
fn kamp1_1_ends_in_victory() {
    let Some((rules, version)) = rules() else {
        return;
    };
    let mut run = Run::new(&rules);
    run.run_until("intro ended", Run::intro_ended);
    run.destroy(&["Drake Hauptquartier", "ENG HAFEN1", "ENG HAFEN2"]);
    // Difficulty 1: Drake appears with two companions (block "Drake erscheint ...", switch 26).
    run.run_until("Drake appears (switch 26)", |r| r.switch_set("26"));
    assert!(
        nml_sim::names::first_id(&run.state.names, "DRAKE1")
            .is_some_and(|id| run.state.object(id, true).is_some()),
        "DRAKE1 did not appear"
    );
    assert!(!run.switch_set("50"), "switch 50 before Drake's end");
    // Counter-check: `DebugDestroy` on DRAKE1 does not set switch 50. `Die` leaves the hit points
    // full, and the death removes the name immediately (change of owner to slot 8, E-145);
    // condition 28 then sees -1 ("name unknown") instead of 0 and stays false.
    {
        let mut probe = Run::new(&rules);
        probe.state = run.state.clone();
        probe.destroy(&["DRAKE1"]);
        for _ in 0..PROBE_STEPS {
            probe.step();
        }
        assert!(
            nml_sim::names::first_id(&probe.state.names, "DRAKE1").is_none(),
            "name DRAKE1 still there after the death"
        );
        assert!(!probe.switch_set("50"), "switch 50 after DebugDestroy");
        run.mark(format!(
            "Counter-check: DebugDestroy on DRAKE1 - name gone, switch 50 not set after {PROBE_STEPS} \
             steps (condition 28 returns -1)"
        ));
    }
    // Drake battered like in the game: hit points below 50 % via the path of action 201 (set
    // current hit points), then he flees (block "Drake KI" at difficulty 1: switch 50).
    {
        let mut sim = Sim::new(&mut run.state, &rules);
        nml_sim::script::world::objects::set_hp(&mut sim, "DRAKE1", DRAKE_HP);
    }
    let drake = nml_sim::names::first_id(&run.state.names, "DRAKE1").unwrap();
    let percent = run
        .state
        .object(drake, false)
        .unwrap()
        .attributes
        .current_fraction(HIT_POINTS)
        / 1000;
    assert!(percent < 50, "DRAKE1 at {percent} %");
    run.mark(format!(
        "DRAKE1 set to {DRAKE_HP} hit points (like action 201), {percent} %"
    ));
    run.run_until("switch 50 set", |r| r.switch_set("50"));
    run.run_until("timer 35 set", |r| {
        r.state
            .script
            .level
            .as_ref()
            .is_some_and(|c| c.timers.contains_key("35"))
    });
    let timer_step = run.steps();
    let timer = run.state.script.level.as_ref().unwrap().timers["35"];
    run.run_until("end sequence started", |r| {
        r.dialogs.iter().any(|(_, p)| p == END_DIALOG)
    });
    let end_start = run.steps();
    // Action 39 holds the block until the dialog ends; only then does it set "Ende".
    run.run_until("end sequence ended", |r| {
        r.dialog_ends.iter().any(|(_, p)| p == END_DIALOG)
    });
    run.run_until("switch \"Ende\" set", |r| r.switch_set("Ende"));
    let ende_step = run.steps();
    // Results before the victory (for the expectation of the chain defeat).
    let before: Vec<i32> = run.state.players.iter().map(|p| p.result).collect();
    assert!(
        run.results.is_empty(),
        "result before the victory: {:?}",
        run.results
    );
    run.run_until("result of slot 6 set", |r| {
        r.state.players[usize::from(PLAYER)].result != 0
    });
    let win_step = run.steps();
    run.step();

    let section = report_section("Victory", &run, &version);
    write_report("## Victory", &section);

    assert!(run.guarded.is_empty(), "Guarded: {:#?}", run.guarded);
    assert_eq!(timer, FINAL_TIMER_MS);
    // 15 s timer: the dialog starts in the script tick in which it expired (50 ms per step).
    let tick = nml_sim::script::SCRIPT_TICK_MS;
    assert_eq!(
        end_start - timer_step,
        u64::from((FINAL_TIMER_MS / tick) as u32),
        "timer step {timer_step}, end sequence step {end_start}"
    );
    // Two tips before the victory in the info history, in order; the blocking `w183` hold `n13`
    // back behind the switch "Ende".
    let history = &run.state.script.mission.info_history;
    let at: Vec<usize> = VICTORY_TIPS
        .iter()
        .map(|t| {
            history
                .iter()
                .rposition(|h| h == t)
                .unwrap_or_else(|| panic!("tip {t} missing: {history:?}"))
        })
        .collect();
    assert!(at[0] < at[1], "tips in the wrong order: {history:?}");
    assert!(
        win_step > ende_step + 1,
        "n13 without waiting time of the tips"
    );
    let win = run.command_steps(13);
    assert_eq!(win.len(), 1, "n13 exactly once: {win:?}");
    assert_eq!((win[0].0, win[0].1), (win_step, &Outcome::Accepted));
    assert!(run.command_steps(4).is_empty(), "defeat reported");
    assert_eq!(run.state.players[usize::from(PLAYER)].result, VICTORY);
    assert!(run.results.contains(&(win_step, PLAYER, VICTORY)));
    // Chain defeat: every slot that was still playing and is not mutually friendly with 6 loses in
    // the same step; mutual friends and already decided ones stay.
    let d = &run.state.diplomacy;
    let p = i32::from(PLAYER);
    let mut losers = 0;
    for q in 0..8u8 {
        if q == PLAYER {
            continue;
        }
        let r = run.state.players[usize::from(q)].result;
        let mutual = d.is_friend(p, i32::from(q)) && d.is_friend(i32::from(q), p);
        if before[usize::from(q)] == 0 && !mutual {
            assert_eq!(r, DEFEAT, "slot {q} does not lose");
            assert!(run.results.contains(&(win_step, q, DEFEAT)));
            losers += 1;
        } else {
            assert_eq!(r, before[usize::from(q)], "slot {q} changed");
        }
    }
    assert!(losers > 0, "no slot loses");
}

/// Section of the report for a run.
fn report_section(title: &str, run: &Run, version: &str) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "## {title}\n");
    let _ = writeln!(
        w,
        "Edition {version}; steps total: {} (limit {MAX_STEPS}). Guarded: {}.\n",
        run.steps(),
        run.guarded.len()
    );
    let _ = writeln!(w, "### Flow\n");
    let mut lines: Vec<(u64, String)> = run.milestones.clone();
    lines.extend(
        run.dialogs
            .iter()
            .map(|(n, p)| (*n, format!("Dialog started {}", short(p)))),
    );
    lines.extend(
        run.dialog_ends
            .iter()
            .map(|(n, p)| (*n, format!("Dialog ended {}", short(p)))),
    );
    // Stable by step: milestones of a step before the dialog messages.
    lines.sort_by_key(|(n, _)| *n);
    for (n, m) in &lines {
        let _ = writeln!(w, "- Step {n}: {m}");
    }
    let _ = writeln!(w, "\n### Game results (`SimEvent::PlayerResult`)\n");
    for (n, slot, result) in &run.results {
        let _ = writeln!(w, "- Step {n}: slot {slot} -> {result}");
    }
    let _ = writeln!(w, "\nFinal state:");
    for (i, p) in run.state.players.iter().enumerate() {
        let _ = writeln!(w, "- Slot {i}: result {}", p.result);
    }
    let _ = writeln!(w, "\n### Commands of the script path\n");
    let _ = writeln!(w, "| Step | Action | Command | Verdict |");
    let _ = writeln!(w, "|---|---|---|---|");
    for (n, a, code, o) in &run.commands {
        let _ = writeln!(w, "| {n} | {a} | `{code:#04x}` | {o:?} |");
    }
    let _ = writeln!(w, "\n### Block sequence\n");
    let _ = writeln!(
        w,
        "Every `Fired` message in reported order (container -1 = dialog); equal blocks in \
         consecutive messages merged.\n"
    );
    let _ = writeln!(w, "| Step(s) | Count | Container | File | Block | Head |");
    let _ = writeln!(w, "|---|---|---|---|---|---|");
    let mut i = 0;
    while i < run.fired.len() {
        let f = &run.fired[i];
        let mut j = i + 1;
        while j < run.fired.len()
            && run.fired[j].container == f.container
            && run.fired[j].block == f.block
            && run.fired[j].path == f.path
        {
            j += 1;
        }
        let last = run.fired[j - 1].step;
        let steps = if last == f.step {
            f.step.to_string()
        } else {
            format!("{}-{last}", f.step)
        };
        let label = run
            .rules
            .scripts
            .script(&f.path)
            .and_then(|s| s.file.blocks.get(f.block as usize))
            .map_or_else(
                || "?".to_owned(),
                |b| format!("line {} \"{}\"", b.line, b.name.trim()),
            );
        let _ = writeln!(
            w,
            "| {steps} | {} | {} | {} | {} | {label} |",
            j - i,
            f.container,
            short(&f.path),
            f.block
        );
        i = j;
    }
    out
}

fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Replaces the section `heading` in `$NML_SHOTS_OUT/kamp1-1-end.md` (order defeat, victory);
/// without the variable nothing.
fn write_report(heading: &str, section: &str) {
    let Some(dir) = std::env::var_os("NML_SHOTS_OUT") else {
        return;
    };
    let _guard = REPORT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kamp1-1-end.md");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut sections: Vec<(String, String)> = Vec::new();
    for part in old.split("\n## ").skip(1) {
        let name = format!("## {}", part.lines().next().unwrap_or(""));
        sections.push((name, format!("## {part}")));
    }
    sections.retain(|(h, _)| h != heading);
    sections.push((heading.to_owned(), section.to_owned()));
    sections.sort_by_key(|(h, _)| if h == "## Defeat" { 0 } else { 1 });
    let mut text = String::from(
        "# Victory and defeat of kamp1-1\n\nScene like `SCRIPT_GOLDEN`: \
         `kamp1-1`, seed `0x4e4d4c32`, AI on, difficulty 1, script on, 55 ms per character. Test \
         `crates/nml-sim/tests/corpus_script_end.rs`; acceptance helper `DebugDestroy` (no counterpart \
         in the original), sender per owner.\n",
    );
    for (_, s) in sections {
        text.push('\n');
        text.push_str(s.trim_end());
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
    eprintln!("Report: {}", path.display());
}
