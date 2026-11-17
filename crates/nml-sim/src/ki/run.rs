//! AI tick in the simulation: the AI state is taken out of `SimState` for the duration of an AI run
//! (tick, start) so that the AI can read the whole simulation and issue commands through [`SimKiWorld`];
//! afterwards it is back in the state.
//!
//! What the AI reports outside the state (notes, calls per world builtin) goes into the log
//! (`log::debug`) and to an observer ([`observe`]), never into the state.

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::query::SimKiWorld;
use super::start::{run_sync, spawn_thread};
use super::world::KiNote;
use super::{KiPause, KiState};
use crate::command::{CommandKind, reject, reject_as};
use crate::player::Sight;
use crate::sim::Sim;

/// What AI runs report outside the state: calls per world builtin ID and notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KiObservation {
    pub counts: BTreeMap<u16, u64>,
    pub notes: Vec<KiNote>,
}

impl KiObservation {
    fn absorb(&mut self, counts: &BTreeMap<u16, u64>, notes: &[KiNote]) {
        for (&id, &n) in counts {
            *self.counts.entry(id).or_default() += n;
        }
        self.notes.extend_from_slice(notes);
    }
}

std::thread_local! {
    /// Observer of [`observe`] (per program thread); without influence on the simulation.
    static OBSERVER: RefCell<Option<KiObservation>> = const { RefCell::new(None) };
}

/// Runs `f` and collects what all AI runs of this program thread report outside the state in the
/// meantime (start in `new_match`, tick in `tick::step`). For host statistics and corpus tests; state and
/// checksum stay untouched. When nested, the outer observer also sees the reports of the inner one.
pub fn observe<R>(f: impl FnOnce() -> R) -> (R, KiObservation) {
    let outer = OBSERVER.with(|o| o.replace(Some(KiObservation::default())));
    let r = f();
    let seen = OBSERVER.with(|o| o.replace(outer)).unwrap_or_default();
    OBSERVER.with(|o| {
        if let Some(outer) = o.borrow_mut().as_mut() {
            outer.absorb(&seen.counts, &seen.notes);
        }
    });
    (r, seen)
}

/// `f` with the extracted AI state and an omniscient real world over the rest of the simulation. While
/// `f` runs, `state.ki` holds an empty placeholder (`KiState::new(0)`) and `sim.in_ki_step` is set;
/// afterwards the real state is back in `state.ki`. Notes go to the log and to [`observe`].
pub(crate) fn with_ki<R>(
    sim: &mut Sim,
    f: impl FnOnce(&mut KiState, &mut SimKiWorld<'_, '_>) -> R,
) -> R {
    let mut ki = std::mem::replace(&mut sim.state.ki, KiState::new(0));
    let was_in_step = std::mem::replace(&mut sim.in_ki_step, true);
    let (r, counts, notes) = {
        let mut world = SimKiWorld::new(sim, Sight::Omniscient);
        let r = f(&mut ki, &mut world);
        (r, world.counts, world.notes)
    };
    for note in &notes {
        log::debug!("AI: {note:?}");
    }
    OBSERVER.with(|o| {
        if let Some(seen) = o.borrow_mut().as_mut() {
            seen.absorb(&counts, &notes);
        }
    });
    // Nobody may have written to the placeholder during the AI run.
    debug_assert_eq!(
        sim.state.ki,
        KiState::new(0),
        "placeholder changed in the AI run"
    );
    sim.in_ki_step = was_in_step;
    sim.state.ki = ki;
    r
}

/// Maximum number of arguments of `KiStart` (A7).
pub const MAX_START_ARGS: usize = 64;
/// Maximum number of passes of `KiStep` (A7).
pub const MAX_STEP_PASSES: u32 = 1000;
/// Reason with which every AI control command is rejected in the AI step.
pub const IN_KI_STEP: &str = "AI control command in the AI step";
/// Reason with which `KiStep` is rejected while the AI is running.
pub const KI_RUNNING: &str = "AI is running";

/// AI control commands `KiStart`, `KiPause`, `KiStep`, called by `command::execute` at the tick boundary.
/// They are interventions by the host (console, debugger, control channel) and are in the command log; the
/// slot of the sender does not matter. In the AI step (`sim.in_ki_step`, i.e. from a world builtin) always
/// rejected: there `state.ki` holds only the placeholder.
pub(crate) fn control(sim: &mut Sim, player: u8, kind: &CommandKind) {
    if sim.in_ki_step {
        log::debug!("AI control command from slot {player} rejected in the AI step");
        reject_as(sim, IN_KI_STEP.into());
        return;
    }
    match kind {
        CommandKind::KiStart {
            slot,
            module,
            args,
            sync,
        } => start(sim, player, *slot, *module, args, *sync),
        CommandKind::KiPause { paused } => pause(sim, *paused),
        CommandKind::KiStep { slot, passes } => step(sim, player, *slot, *passes),
        _ => debug_assert!(false, "not an AI control command: {kind:?}"),
    }
}

/// Is there an engine for slot `slot` (slot 0-7)? Otherwise rejection with a reason.
fn engine_ready(sim: &Sim, player: u8, what: &str, slot: u8) -> bool {
    if usize::from(slot) >= sim.state.ki.engines.len() {
        reject(
            sim,
            player,
            what,
            format!("slot {slot} invalid (allowed 0 to 7)"),
        );
        return false;
    }
    if sim.state.ki.engines[usize::from(slot)].is_none() {
        reject(sim, player, what, format!("slot {slot} has no AI engine"));
        return false;
    }
    true
}

/// `KiStart`: module `module` with `args` on the engine of `slot`, synchronously ([`run_sync`], `.abl`
/// `n0`) or as a thread ([`spawn_thread`], `n1`). Rejected: more than [`MAX_START_ARGS`]
/// arguments (A7), slot outside 0-7 or without an engine, unknown module (no effect as in the
/// original, but with a reason), thread limit `MAX_THREADS`.
fn start(sim: &mut Sim, player: u8, slot: u8, module: u32, args: &[i32], sync: bool) {
    const WHAT: &str = "AI start";
    if args.len() > MAX_START_ARGS {
        let n = args.len();
        reject(
            sim,
            player,
            WHAT,
            format!("{n} arguments (at most {MAX_START_ARGS})"),
        );
        return;
    }
    if !engine_ready(sim, player, WHAT, slot) {
        return;
    }
    if sim.rules.ki.get(module).is_none() {
        reject(sim, player, WHAT, format!("module {module} unknown"));
        return;
    }
    let s = i32::from(slot);
    if sync {
        run_sync(sim, s, module, args);
    } else if !spawn_thread(sim, s, module, args) {
        let max = super::MAX_THREADS;
        reject(
            sim,
            player,
            WHAT,
            format!("slot {slot} already has {max} threads"),
        );
    }
}

/// `KiPause`: pause the whole AI or let it run on (`m_iKiPaused` 2 or 0).
fn pause(sim: &mut Sim, paused: bool) {
    sim.state.ki.paused = if paused {
        KiPause::Paused
    } else {
        KiPause::Running
    };
}

/// `KiStep`: only with a paused AI, otherwise [`KI_RUNNING`]; `passes` (1 ...
/// [`MAX_STEP_PASSES`]) passes in which each thread of the engine of `slot` takes exactly one
/// step with `dt = 20` (`Engine::step_threads`, like the debugger `0x4B0` for one engine).
/// The world stands still meanwhile; a `0x18C` leaves the AI paused. No new state.
fn step(sim: &mut Sim, player: u8, slot: u8, passes: u32) {
    const WHAT: &str = "AI single step";
    if !(1..=MAX_STEP_PASSES).contains(&passes) {
        let why = format!("{passes} passes (allowed 1 to {MAX_STEP_PASSES})");
        reject(sim, player, WHAT, why);
        return;
    }
    if !engine_ready(sim, player, WHAT, slot) {
        return;
    }
    if sim.state.ki.paused != KiPause::Paused {
        reject_as(sim, KI_RUNNING.into());
        return;
    }
    let rules = sim.rules;
    with_ki(sim, |ki, world| {
        let mut pause = false;
        for _ in 0..passes {
            pause |= ki
                .with_engine(slot, &rules.ki, world, |e, cx| e.step_threads(cx))
                .unwrap_or(false);
        }
        if pause {
            ki.paused = KiPause::Paused;
        }
    });
}

/// AI tick (`KIManager_TickAllPlayers`) in the engine tick between world tick and
/// step counter: per engine `FUN_004acf60(3, 20)`. Without an engine nothing (state untouched);
/// when paused, `KiState::tick_engines` does nothing.
pub fn tick(sim: &mut Sim) {
    if sim.state.ki.engines.iter().all(Option::is_none) {
        return;
    }
    let rules = sim.rules;
    with_ki(sim, |ki, world| ki.tick_engines(&rules.ki, world));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ki::engine::{SLICE_DT, SLICES_PER_TICK, STEPS_PER_SLICE};
    use crate::ki::start::testkit::{START_MODULE_PAUSE, ai_match, ai_setup, ki_rules};
    use crate::ki::start::{START_PLAYER, spawn_thread};
    use crate::ki::{KiPause, KiSetup};
    use crate::save::checksum;
    use crate::tick::step;

    // QUIRK E-22: deviation
    #[test]
    fn the_ki_ticks_inside_step() {
        // re/ki-lifecycle 5.1: per thread and step 3 passes x step size 3 = 9 attempts,
        // each counts the sleep down by `dt = 20 / 3 = 6`.
        let per_step = SLICES_PER_TICK * STEPS_PER_SLICE as i32 * SLICE_DT;
        assert_eq!(per_step, 54);
        let (rules, mut state) = ai_match(
            0,
            KiSetup {
                enabled: true,
                ..KiSetup::default()
            },
        );
        let pause = |s: &crate::state::SimState| {
            s.ki.engines[1]
                .as_ref()
                .unwrap()
                .threads
                .get(1)
                .unwrap()
                .pause_ms
        };
        // First tick: `LOAD`, `CALLB 0x8E` (sleep), then 7 attempts of sleep.
        step(&mut state, &rules, &[]);
        assert_eq!(pause(&state), START_MODULE_PAUSE - 7 * SLICE_DT);
        for n in 1..=3 {
            let (before, steps) = (pause(&state), state.steps);
            step(&mut state, &rules, &[]);
            assert_eq!(pause(&state), before - 54, "tick {n}");
            assert_eq!(state.steps, steps + 1);
        }
        state.ki.paused = KiPause::Paused;
        let (before, steps) = (pause(&state), state.steps);
        step(&mut state, &rules, &[]);
        assert_eq!(pause(&state), before, "paused");
        assert_eq!(state.steps, steps + 1);
    }

    #[test]
    fn a_state_without_engines_is_left_alone() {
        let (rules, mut state) = ai_match(0, KiSetup::default());
        assert!(state.ki.engines.iter().all(Option::is_none));
        let before = (checksum(&state), state.ki.clone());
        let mut sim = Sim::new(&mut state, &rules);
        let (_, seen) = observe(|| tick(&mut sim));
        assert_eq!(seen, KiObservation::default());
        assert!(!sim.in_ki_step);
        assert_eq!((checksum(&state), state.ki.clone()), before);
    }

    #[test]
    fn the_ki_state_is_back_after_each_tick() {
        let (rules, mut state) = ai_match(
            0,
            KiSetup {
                enabled: true,
                ..KiSetup::default()
            },
        );
        let threads = |s: &crate::state::SimState| {
            s.ki.engines
                .iter()
                .flatten()
                .map(|e| e.threads.len())
                .sum::<usize>()
        };
        assert_eq!(threads(&state), 2);
        let mut sim = Sim::new(&mut state, &rules);
        let inside = with_ki(&mut sim, |ki, world| {
            // In the AI run: flag set, placeholder in the state, real state in `ki`.
            assert!(world.sim.in_ki_step);
            assert_eq!(world.sim.state.ki, KiState::new(0));
            ki.engines.iter().flatten().count()
        });
        assert_eq!(inside, 3);
        assert!(!sim.in_ki_step);
        for _ in 0..3 {
            tick(&mut sim);
            assert!(!sim.in_ki_step);
            assert_eq!(sim.state.ki.engines.iter().flatten().count(), 3);
        }
        assert_eq!(threads(&state), 2);
        // A start call in the middle of the AI run sees only the placeholder (no engine) and does
        // nothing.
        let mut sim = Sim::new(&mut state, &rules);
        let started = with_ki(&mut sim, |_, world| {
            spawn_thread(world.sim, 1, START_PLAYER, &[])
        });
        assert!(!started);
        assert!(!sim.in_ki_step);
        assert_eq!(threads(&state), 2);
    }

    #[test]
    fn observe_collects_counts_and_notes_and_nests() {
        let rules = ki_rules(false);
        let setup = ai_setup(
            0,
            KiSetup {
                enabled: true,
                ..KiSetup::default()
            },
        );
        // Start: two probes `0xE6` per AI slot in the synchronous `setschwierigkeitsgrad`.
        let ((mut state, inner), outer) =
            observe(|| observe(|| crate::setup::new_match(&rules, &setup).unwrap()));
        assert_eq!(inner.counts, BTreeMap::from([(0xE6, 4)]));
        assert!(inner.notes.is_empty());
        assert_eq!(inner, outer);
        // Tick: the start module calls only local builtins (sleep).
        let ((), seen) = observe(|| {
            step(&mut state, &rules, &[]);
        });
        assert_eq!(seen, KiObservation::default());
        // Notes arrive (missing module), the state stays.
        let before = state.ki.clone();
        let mut sim = Sim::new(&mut state, &rules);
        let (started, seen) = observe(|| crate::ki::start::run_sync(&mut sim, 1, 4711, &[]));
        assert!(started);
        assert_eq!(
            seen.notes,
            [KiNote::MissingModule {
                slot: 1,
                thread: -1,
                module: 4711,
                target: 4711,
                spawn: false,
            }]
        );
        assert_eq!(state.ki, before);
        // Without an observer nobody collects.
        OBSERVER.with(|o| assert!(o.borrow().is_none()));
    }

    // --- AI control commands ----------------------------------------------------------------------

    use crate::command::{Command, CommandKind, Origin, Outcome, execute};
    use crate::ki::start::testkit::{COUNT_MODULE, MARK_MODULE};
    use crate::ki::world::{BuiltinCx, KiWorld};
    use crate::player::submit;
    use crate::rules::Rules;
    use crate::state::SimState;

    const ON: KiSetup = KiSetup {
        enabled: true,
        campaign_difficulty: 1,
    };

    fn run(state: &mut SimState, rules: &Rules, kind: CommandKind) -> Outcome {
        let mut sim = Sim::new(state, rules);
        execute(&mut sim, &Command { player: 0, kind }, true)
    }

    /// (ID, module, pc, stack depth) of the threads of slot `slot`, by ID.
    fn threads_of(state: &SimState, slot: usize) -> Vec<(i32, u32, u32, usize)> {
        let mut v: Vec<_> = state.ki.engines[slot]
            .as_ref()
            .unwrap()
            .threads
            .iter()
            .map(|(id, t)| (id, t.module, t.pc, t.stack.len()))
            .collect();
        v.sort_unstable();
        v
    }

    fn start(slot: u8, module: u32, sync: bool) -> CommandKind {
        CommandKind::KiStart {
            slot,
            module,
            args: vec![],
            sync,
        }
    }

    fn step1(passes: u32) -> CommandKind {
        CommandKind::KiStep { slot: 1, passes }
    }

    // QUIRK E-19: kept
    // QUIRK E-21: deviation
    #[test]
    fn ki_start_pause_and_step_are_commands() {
        let (rules, mut state) = ai_match(0, ON);
        // Start as a thread: slot 1 gets thread 2 (counting module), it does not run until the tick.
        assert_eq!(
            run(&mut state, &rules, start(1, COUNT_MODULE, false)),
            Outcome::Accepted
        );
        assert_eq!(
            threads_of(&state, 1),
            [(1, START_PLAYER, 0, 3), (2, COUNT_MODULE, 0, 0)]
        );
        // Synchronous: slot 0 (human, E-19) runs the module immediately.
        assert_eq!(
            run(&mut state, &rules, start(0, MARK_MODULE, true)),
            Outcome::Accepted
        );
        let values =
            |s: &SimState, slot: usize| s.ki.engines[slot].as_ref().unwrap().mem.values.clone();
        assert_eq!(values(&state, 0).get(&500), Some(&1));
        // Single step with the AI running: rejected, nothing changed.
        let before = state.ki.clone();
        assert_eq!(
            run(&mut state, &rules, step1(2)),
            Outcome::Rejected(KI_RUNNING.into())
        );
        assert_eq!(state.ki, before);
        // Pause, then two passes: each thread of slot 1 executes exactly two commands
        // (`LOAD` + `CALLB 0x8E` or two `PUSHA`), slot 3 stays put.
        assert_eq!(
            run(&mut state, &rules, CommandKind::KiPause { paused: true }),
            Outcome::Accepted
        );
        assert_eq!(state.ki.paused, KiPause::Paused);
        let other = threads_of(&state, 3);
        assert_eq!(run(&mut state, &rules, step1(2)), Outcome::Accepted);
        assert_eq!(
            threads_of(&state, 1),
            [(1, START_PLAYER, 2, 3), (2, COUNT_MODULE, 2, 2)]
        );
        assert_eq!(threads_of(&state, 3), other);
        assert_eq!(state.ki.paused, KiPause::Paused);
        // The sleep counts down `dt = 20` per step: one more pass.
        let sleep = |s: &SimState| {
            s.ki.engines[1]
                .as_ref()
                .unwrap()
                .threads
                .get(1)
                .unwrap()
                .pause_ms
        };
        let slept = sleep(&state);
        assert_eq!(slept, START_MODULE_PAUSE);
        assert_eq!(run(&mut state, &rules, step1(1)), Outcome::Accepted);
        assert_eq!(sleep(&state), slept - 20);
        assert_eq!(threads_of(&state, 1)[1], (2, COUNT_MODULE, 3, 3));
        // Paused in the tick: nothing; as a command at the tick boundary of `step` it takes effect.
        let before = state.ki.clone();
        step(&mut state, &rules, &[]);
        assert_eq!(state.ki, before);
        let cmd = Command {
            player: 2,
            kind: step1(3),
        };
        step(&mut state, &rules, &[cmd]);
        assert_eq!(threads_of(&state, 1)[1], (2, COUNT_MODULE, 6, 6));
        assert_eq!(sleep(&state), slept - 4 * 20);
        // Resume.
        assert_eq!(
            run(&mut state, &rules, CommandKind::KiPause { paused: false }),
            Outcome::Accepted
        );
        assert_eq!(state.ki.paused, KiPause::Running);
        // Limits (A7), missing engine, unknown module, slot outside: rejected with a reason,
        // state unchanged.
        let before = (state.ki.clone(), checksum(&state));
        for kind in [
            step1(1),
            step1(0),
            step1(MAX_STEP_PASSES + 1),
            CommandKind::KiStep { slot: 8, passes: 1 },
            start(2, MARK_MODULE, true),
            start(2, MARK_MODULE, false),
            start(8, MARK_MODULE, false),
            start(255, MARK_MODULE, true),
            start(1, 4711, true),
            start(1, 4711, false),
            CommandKind::KiStart {
                slot: 1,
                module: MARK_MODULE,
                args: vec![0; MAX_START_ARGS + 1],
                sync: true,
            },
        ] {
            let outcome = run(&mut state, &rules, kind.clone());
            let Outcome::Rejected(why) = &outcome else {
                panic!("{kind:?}: {outcome:?}");
            };
            assert!(!why.is_empty(), "{kind:?}");
        }
        assert_eq!((state.ki.clone(), checksum(&state)), before);
        // Exactly 64 arguments work, in order on the stack.
        let args: Vec<i32> = (0..MAX_START_ARGS as i32).collect();
        let start64 = CommandKind::KiStart {
            slot: 3,
            module: COUNT_MODULE,
            args: args.clone(),
            sync: false,
        };
        assert_eq!(run(&mut state, &rules, start64), Outcome::Accepted);
        let e = state.ki.engines[3].as_ref().unwrap();
        assert_eq!(e.threads.get(2).unwrap().stack, args);
        // Paused with `MAX_STEP_PASSES` passes: accepted, no panic.
        run(&mut state, &rules, CommandKind::KiPause { paused: true });
        let max = CommandKind::KiStep {
            slot: 3,
            passes: MAX_STEP_PASSES,
        };
        assert_eq!(run(&mut state, &rules, max), Outcome::Accepted);
    }

    /// World builtin (a command ID with a case in the dispatcher) that is intercepted here and (with
    /// `submit`) tries to give all three AI control commands.
    const PROBE: u16 = 0x258;
    const PROBE_MODULE: u32 = 80;

    struct Probe<'w, 's, 'a> {
        inner: &'w mut SimKiWorld<'s, 'a>,
        submit: bool,
        outcomes: Vec<Outcome>,
    }

    impl KiWorld for Probe<'_, '_, '_> {
        fn builtin(&mut self, id: u16, cx: &mut BuiltinCx<'_>) {
            if id != PROBE {
                self.inner.builtin(id, cx);
                return;
            }
            if !self.submit {
                return;
            }
            for (kind, origin) in [
                (start(1, MARK_MODULE, true), Origin::Ki),
                (CommandKind::KiPause { paused: true }, Origin::External),
                (step1(1), Origin::Ki),
            ] {
                let c = Command {
                    player: cx.slot(),
                    kind,
                };
                self.outcomes.push(submit(self.inner.sim, &c, origin));
            }
        }

        fn note(&mut self, note: KiNote) {
            self.inner.note(note);
        }
    }

    #[test]
    fn ki_control_commands_are_rejected_inside_the_ki_step() {
        use crate::ki::testkit::{proto, word};
        use crate::ki::vm::op;
        let (mut rules, mut state) = ai_match(0, ON);
        let code = [word(op::CALLB, u32::from(PROBE))];
        rules
            .ki
            .modules
            .insert(PROBE_MODULE, proto(PROBE_MODULE, &[(0, 8)], &[], &code));
        let mut sim = Sim::new(&mut state, &rules);
        assert!(spawn_thread(&mut sim, 1, PROBE_MODULE, &[]));
        let mut reference = state.clone();
        let tick_with = |state: &mut SimState, submit: bool| {
            let mut sim = Sim::new(state, &rules);
            with_ki(&mut sim, |ki, world| {
                let mut probe = Probe {
                    inner: world,
                    submit,
                    outcomes: Vec::new(),
                };
                ki.tick_engines(&rules.ki, &mut probe);
                probe.outcomes
            })
        };
        let outcomes = tick_with(&mut state, true);
        assert_eq!(
            outcomes,
            vec![Outcome::Rejected(IN_KI_STEP.into()); 3],
            "every AI control command rejected in the AI step"
        );
        // The same tick without the attempts: AI state and checksum equal (placeholder untouched,
        // nothing started, not paused).
        assert!(tick_with(&mut reference, false).is_empty());
        assert_eq!(state.ki, reference.ki);
        assert_eq!(state.ki.paused, KiPause::Running);
        let e = state.ki.engines[1].as_ref().unwrap();
        assert!(!e.mem.values.contains_key(&500));
        assert_eq!(checksum(&state), checksum(&reference));
    }
}
