//! Match against the computer on real multiplayer maps: `KI_GOLDEN` (golden scene) with
//! saving/loading and replay from the command log, the build order of the first five game minutes
//! and the nation run with a report to `$NML_SHOTS_OUT/nation-run.md`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use opennml_rda::Vfs;
use opennml_rda::testutil::corpus_game_dir;
use opennml_sim::command::validate::{is_dead, is_finished};
use opennml_sim::command::{Command, CommandKind, Outcome};
use opennml_sim::economy::{guid_count, pending_guid_count};
use opennml_sim::id::ObjectId;
use opennml_sim::ki::KiPause;
use opennml_sim::ki::vm::Thread;
use opennml_sim::module::attack;
use opennml_sim::module::gather::{harvest, lumber, mining};
use opennml_sim::module::riding::RIDING;
use opennml_sim::object::status::{DIED, ISMOUNTED, ISMOUNTEDONBOAT, QUARTERED};
use opennml_sim::path::grid::COST_BLOCKED;
use opennml_sim::rules::{Fnv64, Rules};
use opennml_sim::save::{RecordedCommand, Replay, SaveGame, checksum};
use opennml_sim::setup::{MatchSetup, SlotKind, ki_kind_handicap, new_match, start_package};
use opennml_sim::sim::Sim;
use opennml_sim::state::{Control, SimEvent, SimState};
use opennml_sim::tick::step;
use opennml_world::classes::module;
use opennml_world::level::load_level;

/// Map of the golden scene and first map of the nation run.
const EXILE: &str = "data/levels/multiplayer/[4players]-exile.lvl";
/// Second map of the nation run. Listing of all 20 multiplayer maps (slots, tiles):
/// 2 slots: battlebeach 64^2, tintebariver 64^2, bumblebeeforrest, crossrail, ironhorsehunt,
/// railroadrage, vulcanoisland 128^2 each; 3 slots: canonballcanyon, tropical islands 128^2 each;
/// 4 slots: donut-isle, exile, four points, little big valley, mount nibbler, snowdream, soupcan,
/// summerheat 128^2 each; 5 slots: viciouscircle 128^2; 6 and 8 slots: legends, tribes 256^2 each.
/// The start objects of all slots are connected over land (layer 0) on every map
/// ([`starts_connected_by_land`]), none forces ships. Medium size is 128^2 (64^2, 128^2, 256^2).
/// Among the maps with at least four slots and 128^2, "four points" is the first besides exile
/// without an island in its name (donut-isle is connected over land, but carries the island in its
/// name and would be the weaker choice against the island constraint).
const SECOND: &str = "data/levels/multiplayer/[4players]-four points.lvl";
/// Start index of the random sequence of the golden scene ("NML1").
const SEED: u32 = 0x4E4D_4C31;
/// Steps of `KI_GOLDEN`: one game minute.
const GOLDEN_STEPS: usize = 1200;
/// Save point in the middle of the scene.
const SAVE_AT: usize = GOLDEN_STEPS / 2;
/// Steps of the build order: five game minutes.
const BUILD_ORDER_STEPS: usize = 6000;
/// AI kind of the AI slots: 3 -> handicap 0, stage `5 - 0`.
const KI_KIND: u8 = 3;

/// Checksum over the 1200 checksums of the golden scene: the same in debug and release. Measured.
/// Changed: state fields `script`, `names` and evocation counter (serialization, behavior the
/// same; counter-check with `serde(skip)`).
/// Changed: event bits 1, 2, 3, 5, 7, kill counter, message sequence `0x2100`/`0x611` and new
/// world fields (counter-check: without setter and with `serde(skip)` the values of the preceding step).
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: serialization of `accounting` and `countdowns` added to the value of the preceding
/// step (counter-check: with `serde(skip)` on both, the values of the preceding step).
/// Changed: serialization of new fields (presentation state, mirror); behavior the same
/// (counter-check with `serde(skip)`).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26);
/// behavior the same (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and
/// `time_limit`, FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all values
/// unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same;
/// counter-check with `serde(skip)`: all values unchanged).
const KI_GOLDEN: u64 = 0xa45d_5802_958f_754b;

/// Construction sites of the AI slots in the first 6000 steps of the golden scene: (step, slot,
/// GUID) from `SimEvent::Created` with a building module. Measured.
const KI_BUILD_ORDER: &[(u64, u8, u32)] = &[
    (99, 1, 1603),
    (196, 3, 2503),
    (258, 1, 1605),
    (613, 3, 2505),
    (617, 1, 1602),
    (801, 3, 2502),
    (1323, 3, 2506),
    (1444, 3, 2516),
    (1575, 1, 1604),
    (1653, 1, 1610),
    (1671, 3, 2504),
    (1832, 1, 1606),
    (1868, 1, 1607),
    (2180, 3, 2510),
    (2292, 3, 2515),
    (2648, 3, 2507),
    (3069, 3, 2505),
    (3092, 3, 2505),
    (3156, 3, 2509),
    (3173, 3, 2509),
    (3557, 3, 2509),
    (3715, 3, 2516),
    (4317, 3, 2511),
    (5287, 3, 2501),
    (5320, 1, 1601),
    (5932, 1, 1601),
];

fn rules_for(level: &str) -> Option<Rules> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, level).unwrap();
    Some(Rules::load(&vfs, &scene).unwrap_or_else(|e| panic!("{level}: {e:#}")))
}

/// Lobby slot `i` as an AI of kind [`KI_KIND`] with nation `nation`.
fn set_ai(setup: &mut MatchSetup, i: usize, nation: i32) {
    let e = &mut setup.lobby[i];
    e.kind = SlotKind::Ai;
    e.nation = nation;
    e.handicap = ki_kind_handicap(KI_KIND).unwrap();
}

/// Golden scene: exile, mode 0, seed [`SEED`], slot 0 human (passive, nation 0), slot 1 AI nation 1,
/// slot 2 human (passive, nation 2), slot 3 AI nation 3, AI on, difficulty 1.
fn golden_setup(rules: &Rules) -> MatchSetup {
    let mut setup = MatchSetup::default_for(rules);
    assert_eq!(setup.lobby.len(), 4, "{EXILE}");
    setup.mode = 0;
    setup.seed = SEED;
    for (i, e) in setup.lobby.iter_mut().enumerate() {
        e.kind = SlotKind::Human;
        e.nation = i as i32;
        e.handicap = 0;
    }
    set_ai(&mut setup, 1, 1);
    set_ai(&mut setup, 3, 3);
    setup.ki.enabled = true;
    setup.ki.campaign_difficulty = 1;
    setup
}

/// Slots with control kind AI.
fn ai_slots(state: &SimState) -> Vec<u8> {
    (0..8u8)
        .filter(|&s| state.players[usize::from(s)].control == Control::Ai)
        .collect()
}

/// `steps` steps from `new_match(setup)` with the commands `commands` (tick like
/// [`Replay::play`]); after every step the checksum and `on_step(step, state, events)`.
/// After every step in `save_at` it is saved and loaded with a check.
fn play(
    rules: &Rules,
    setup: &MatchSetup,
    steps: usize,
    save_at: &[usize],
    commands: &[RecordedCommand],
    mut on_step: impl FnMut(u64, &SimState, &[SimEvent]),
) -> (Vec<u64>, SimState) {
    let mut state = new_match(rules, setup).unwrap();
    let mut sums = Vec::with_capacity(steps);
    let mut next = 0;
    for n in 1..=steps {
        let now = state.steps;
        let first = next;
        while next < commands.len() && commands[next].step == now {
            next += 1;
        }
        let due: Vec<Command> = commands[first..next]
            .iter()
            .map(|c| c.command.clone())
            .collect();
        let events = step(&mut state, rules, &due);
        sums.push(checksum(&state));
        on_step(n as u64, &state, &events);
        if save_at.contains(&n) {
            let save = SaveGame {
                start: start_package(rules, setup),
                state: state.clone(),
                commands: Vec::new(),
            };
            let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
            loaded.check(rules).unwrap();
            assert_eq!(checksum(&loaded.state), checksum(&state));
            state = loaded.state;
        }
    }
    assert_eq!(next, commands.len(), "commands after the last step");
    (sums, state)
}

/// Checksum of a sequence like the other golden sequences (FNV-1a 64 over the checksums in little
/// endian).
fn fold(sums: &[u64]) -> u64 {
    let mut h = Fnv64::default();
    for c in sums {
        h.write(&c.to_le_bytes());
    }
    h.0
}

/// Result of an AI command as text.
fn outcome_label(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Accepted => "accepted".into(),
        Outcome::Partial(why) => format!("partial: {why}"),
        Outcome::Rejected(why) => format!("rejected: {why}"),
    }
}

/// The sequences belong to the golden checksums of the project; the module is called `checksums`
/// so that the filter `cargo test -- checksums` hits them.
mod checksums {
    use super::*;

    /// AI commands of a run per (slot, network command, result).
    type Issued = BTreeMap<(u8, u32, String), u64>;

    fn count_ki_commands(issued: &mut Issued) -> impl FnMut(u64, &SimState, &[SimEvent]) + '_ {
        |_, _, events| {
            for e in events {
                if let SimEvent::KiCommand {
                    player,
                    code,
                    outcome,
                } = e
                {
                    *issued
                        .entry((*player, *code, outcome_label(outcome)))
                        .or_default() += 1;
                }
            }
        }
    }

    #[test]
    fn ki_golden() {
        let Some(rules) = rules_for(EXILE) else {
            return;
        };
        let setup = golden_setup(&rules);
        let mut issued = Issued::new();
        let (sums, state) = play(
            &rules,
            &setup,
            GOLDEN_STEPS,
            &[],
            &[],
            count_ki_commands(&mut issued),
        );
        // Counter-check before the golden: two AI slots (1 and 3) with threads, the AI issues commands.
        assert_eq!(ai_slots(&state), [1, 3]);
        for slot in [1usize, 3] {
            let e = state.ki.engines[slot].as_ref().unwrap();
            assert!(!e.threads.is_empty(), "slot {slot} without threads");
        }
        assert!(!issued.is_empty(), "no AI command in {GOLDEN_STEPS} steps");
        eprintln!("AI commands of the golden scene: {issued:?}");
        let h = fold(&sums);
        assert_eq!(h, KI_GOLDEN, "{h:#018x}");
    }

    /// Saving/loading in the middle of the match: saved after step 600 and loaded with a check, the
    /// same sequence as without.
    #[test]
    fn ki_golden_survives_saving_and_loading() {
        let Some(rules) = rules_for(EXILE) else {
            return;
        };
        let setup = golden_setup(&rules);
        let (saved, _) = play(&rules, &setup, GOLDEN_STEPS, &[SAVE_AT], &[], |_, _, _| {});
        // Same sequence as `ki_golden` (the checksum over the sequence covers every step).
        assert_eq!(fold(&saved), KI_GOLDEN);
    }

    /// State of the threads of a slot: [`thread_mark`] per thread ID.
    type ThreadMarks = Vec<(i32, Vec<i64>)>;

    /// [`ThreadMarks`] of slot `slot`.
    fn threads_of(state: &SimState, slot: usize) -> ThreadMarks {
        state.ki.engines[slot]
            .as_ref()
            .map(|e| {
                e.threads
                    .iter()
                    .map(|(id, t)| (id, thread_mark(t)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Replay of the match from the command log gives the same sequence: the run pauses the AI at
    /// step 300, sends three single steps to slot 1 at 305 and lets it continue at 310; after step
    /// 305 (paused, before the single step) it is saved and loaded. The log contains only these
    /// three control commands; the AI commands are created anew on replay.
    // QUIRK E-97: deviation
    #[test]
    fn ki_golden_replays_from_the_command_log() {
        let Some(rules) = rules_for(EXILE) else {
            return;
        };
        let setup = golden_setup(&rules);
        let control = |step: u64, kind: CommandKind| RecordedCommand {
            step,
            command: Command { player: 0, kind },
        };
        let log = vec![
            control(300, CommandKind::KiPause { paused: true }),
            control(305, CommandKind::KiStep { slot: 1, passes: 3 }),
            control(310, CommandKind::KiPause { paused: false }),
        ];
        let mut issued = Issued::new();
        let mut paused = Vec::new();
        // Threads of slots 1 and 3 after steps 304 to 307.
        let mut marks: BTreeMap<u64, [ThreadMarks; 2]> = BTreeMap::new();
        let (live, live_state) = {
            let mut count = count_ki_commands(&mut issued);
            play(
                &rules,
                &setup,
                GOLDEN_STEPS,
                &[305],
                &log,
                |n, state, events| {
                    count(n, state, events);
                    paused.push(state.ki.paused == KiPause::Paused);
                    if (304..=307).contains(&n) {
                        marks.insert(n, [threads_of(state, 1), threads_of(state, 3)]);
                    }
                },
            )
        };
        assert!(!issued.is_empty(), "no AI command in the run");
        // The control commands take effect at the tick boundary before steps 301, 306 and 311
        // (`steps` 300, 305, 310): paused exactly after steps 301 to 310.
        let held: Vec<usize> = (1..=GOLDEN_STEPS).filter(|&n| paused[n - 1]).collect();
        assert_eq!(held, (301..=310).collect::<Vec<_>>());
        // Every engine is paused (304 -> 305, 306 -> 307); the single step (step 306) moves only the
        // threads of slot 1 - so it was accepted.
        assert_eq!(marks[&304], marks[&305], "paused, without a single step");
        assert_eq!(marks[&306], marks[&307], "paused after the single step");
        assert_ne!(
            marks[&305][0], marks[&306][0],
            "single step at slot 1 without effect"
        );
        assert_eq!(marks[&305][1], marks[&306][1], "single step moves slot 3");
        assert_ne!(fold(&live), KI_GOLDEN);

        // Replay from the saved log: `Replay::play` to the end against the live run, plus step by
        // step beyond the control commands (step 311).
        let replay = Replay {
            start: start_package(&rules, &setup),
            commands: log,
        };
        let replay = Replay::from_bytes(&replay.to_bytes()).unwrap();
        assert_eq!(
            replay.commands.len(),
            3,
            "only the control commands in the log"
        );
        let end = replay.play(&rules, GOLDEN_STEPS as u64).unwrap();
        assert_eq!(end.steps, GOLDEN_STEPS as u64);
        assert_eq!(checksum(&end), checksum(&live_state));
        assert_eq!(checksum(&end), live[GOLDEN_STEPS - 1]);
        let (again, _) = play(
            &rules,
            &replay.start.setup,
            311,
            &[],
            &replay.commands,
            |_, _, _| {},
        );
        let first_diff = again.iter().zip(&live).position(|(a, b)| a != b);
        assert_eq!(first_diff, None, "first deviation after step");
    }
}

/// Type with module `index`.
fn has_module(rules: &Rules, guid: u32, index: u32) -> bool {
    rules.types.get(&guid).is_some_and(|t| t.has_module(index))
}

/// Building (module 3) of an AI slot from a `Created` event: (slot, GUID).
fn created_building(rules: &Rules, state: &SimState, e: &SimEvent) -> Option<(u8, u32)> {
    let SimEvent::Created { id } = e else {
        return None;
    };
    let o = state.object(*id, false)?;
    (has_module(rules, o.guid, module::BUILDING)
        && state.players[usize::from(id.slot())].control == Control::Ai)
        .then_some((id.slot(), o.guid))
}

/// Build order of the first five game minutes as a key figure (release only).
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn ki_build_order_of_the_first_five_minutes() {
    let Some(rules) = rules_for(EXILE) else {
        return;
    };
    let setup = golden_setup(&rules);
    let mut order = Vec::new();
    play(
        &rules,
        &setup,
        BUILD_ORDER_STEPS,
        &[],
        &[],
        |n, state, events| {
            for e in events {
                if let Some((slot, guid)) = created_building(&rules, state, e) {
                    order.push((n, slot, guid));
                }
            }
        },
    );
    eprintln!("Build order: {order:?}");
    assert!(!order.is_empty(), "no construction site in five minutes");
    assert_eq!(order, KI_BUILD_ORDER);
}

/// The start objects with a walk module of all occupied slots are connected over land (layer 0,
/// neighbors in four directions): no map with an island constraint. (reached, slots)
fn starts_connected_by_land(rules: &Rules, state: &SimState) -> (usize, usize) {
    let starts: Vec<(i32, i32)> = (0..8u8)
        .filter(|&s| state.players[usize::from(s)].control != Control::Empty)
        .filter_map(|s| {
            state
                .objects()
                .find(|o| o.id.slot() == s && has_module(rules, o.guid, module::WALK))
                .map(|o| o.tile)
        })
        .collect();
    let (w, h) = (state.path.width, state.path.height);
    let index = |(x, z): (i32, i32)| (z * w + x) as usize;
    let mut seen = vec![false; (w * h) as usize];
    let mut todo = VecDeque::from([starts[0]]);
    seen[index(starts[0])] = true;
    while let Some((x, z)) = todo.pop_front() {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let c = (x + dx, z + dz);
            if c.0 < 0 || c.1 < 0 || c.0 >= w || c.1 >= h || seen[index(c)] {
                continue;
            }
            if state.path.cell_cost(c.0, c.1, 0) < COST_BLOCKED {
                seen[index(c)] = true;
                todo.push_back(c);
            }
        }
    }
    let reached = starts.iter().filter(|&&t| seen[index(t)]).count();
    (reached, starts.len())
}

/// Steps of the nation run: 20 game minutes.
const SURVEY_STEPS: u64 = 24_000;
/// Stall of a thread: no progress (counter, status, pause, semaphore, children) over this many
/// steps (2 game minutes).
const THREAD_STALL: u64 = 2400;
/// Stall of a slot: no AI command over this many steps (5 game minutes).
const SLOT_STALL: u64 = 6000;
/// Measuring points of the army size: after 10 and 20 minutes.
const ARMY_AT: [u64; 2] = [12_000, 24_000];

/// Kind of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opponents {
    /// One AI slot (lobby 1, nation n) against passive slots.
    Passive,
    /// AI against AI: lobby 1 nation n, lobby 3 nation n + 1 mod 6; the others passive.
    Ki,
}

/// State of a thread for stall detection: counter, status, pause, semaphore and the counters of
/// the children.
fn thread_mark(t: &Thread) -> Vec<i64> {
    let mut out = Vec::new();
    let mut node = Some(t);
    while let Some(t) = node {
        out.extend([
            i64::from(t.pc),
            i64::from(t.status),
            i64::from(t.pause_ms),
            i64::from(t.wait_semaphore),
            i64::from(t.module),
        ]);
        node = t.child.as_deref();
    }
    out
}

/// A stall of a thread.
#[derive(Debug, Clone)]
struct Stall {
    slot: u8,
    thread: i32,
    module: u32,
    since: u64,
    until: u64,
    /// State during the stall: counter, status, pause, semaphore (top thread), child.
    detail: String,
    /// Not ended by the end of the run.
    open: bool,
}

/// Key figures of an AI slot.
#[derive(Debug, Default, Clone)]
struct SlotStats {
    nation: i32,
    first_building: Option<(u64, u32)>,
    first_unit: Option<(u64, u32)>,
    army: [usize; 2],
    first_attack: Option<(u64, u32)>,
    last_command: u64,
    longest_silence: (u64, u64),
    commands: u64,
}

/// Result of a run.
#[derive(Debug, Default)]
struct RunResult {
    title: String,
    slots: BTreeMap<u8, SlotStats>,
    /// (slot, network command, result with [`normalized`] IDs) -> count.
    issued: BTreeMap<(u8, u32, String), u64>,
    /// (network command, result with [`normalized`] IDs) -> first occurrence (step, slot, result
    /// verbatim).
    first_seen: BTreeMap<(u32, String), (u64, u8, String)>,
    stalls: Vec<Stall>,
    times: Vec<Duration>,
    threads_end: BTreeMap<u8, usize>,
}

/// Causes of the rejections and partial effects in the nation run: (network command, part of the
/// result after [`normalized`] with the condition from [`condition`] in brackets, cause). Each part
/// names the case exactly (concrete GUID, condition); a new condition finds no entry. Determined by
/// tracing back to the AI module (thread and code location of the `CALLB`) and the handler of the
/// original in Ghidra. "same": the handler of the original does nothing in this case either;
/// "deviation": the original takes effect, the reimplementation rejects it according to exceptions
/// A7/A8 (these checks are kept for the AI).
const CAUSES: &[(u32, &str, &str)] = &[
    (
        0x01,
        "prerequisite for 1605 missing",
        "Deviation (exception A8, REQUIRED_GUID): `allg_normal_build` places 1605 after the loss of \
         the main house 1600; `GameWorld_CmdPlaceSingleBuilding` (0x004896D0) checks only costs and \
         the single main house and builds",
    ),
    (
        0x04,
        "unit N not present, foreign, quartered or dead [gone]",
        "same: stale ID (unit died since the query, \
         `multi_normal_checkbattles` or `allg_normal_gohealing`); the original resolves the ID \
         to NULL and skips it",
    ),
    (
        0x0a,
        "cannot mount [mounted]",
        "same: `allg_normal_pferd_besatteln` sends the mounting in a loop also to \
         already mounted units (status 0x2000); `GameWorld_CmdMountHorse` (0x0047CFB0) \
         skips units with 0xA000",
    ),
    (
        0x0a,
        "cannot mount [no free horse]",
        "same (handler): several threads `allg_normal_pferd_besatteln` of the same slot choose \
         the same horse (measured: 3410, status 0x4000, the rider changes among the threads); if \
         it already carries, `GameWorld_CmdMountHorse` finds no free saddle horse in the vicinity \
         and skips the unit (list empty)",
    ),
    (
        0x0a,
        "horse N not present",
        "same: the horse has disappeared since the query (`allg_normal_pferd_besatteln`); \
         `GameWorld_CmdMountHorse` ends at once without a horse",
    ),
    (
        0x0e,
        "exists only once [in progress]",
        "same: two threads `allg_normal_produce` queue the same one-time GUID in the same \
         step (object counter 0, in progress 1); the handler of the original also rejects when \"in progress\" > \
         0 (re/buildings 6.1)",
    ),
    (
        0x12,
        "unit 0 not present, foreign, quartered or dead [ID 0]",
        "same: `allg_normal_groupattack` sets the stance for gaps of its list (`0x37` creates \
         missing keys with 0); ID 0 is never an object in the original (generation from \
         1), `GameWorld_CmdSetBehavior` (0x004762E0) skips it",
    ),
    (
        0x12,
        "unit N not present, foreign, quartered or dead [gone]",
        "same: stale ID in `allg_normal_groupattack`/`allg_normal_searchfor` (unit \
         died); the original skips NULL",
    ),
    (
        0x12,
        "stance -1",
        "Deviation (exception A7, value limit 0-4): `allg_normal_goto` passes the combat status -1 of its \
         caller on to `0x198`; `GameWorld_CmdSetBehavior` sets -1 unchecked, the reimplementation \
         rejects (an earlier load check would have rejected the savegame)",
    ),
    (
        0x13,
        "unit N not present, foreign, quartered or dead [quartered]",
        "same: `allg_normal_einquartierverwaltung` sends already quartered units \
         (carrier set) to the quarters again; in the container the unit has no running walk, \
         the command remains without consequence",
    ),
    (
        0x13,
        "unit N not present, foreign, quartered or dead [gone]",
        "same: `allg_normal_einquartierverwaltung` sends a dead unit; the \
         original skips NULL",
    ),
    (
        0x13,
        "container N not present",
        "same: container destroyed since the query; `GameWorld_CmdQuarterUnits` (0x0047D8D0) \
         ends at once without a container",
    ),
    (
        0x13,
        "is dead, unfinished, full or hidden [dead]",
        "Deviation (exception A8, target like the UI): `allg_normal_einquartierverwaltung` sends \
         units into the destroyed main house (status DIED); the original does not check the \
         container and lets the units walk there",
    ),
    (
        0x13,
        "is dead, unfinished, full or hidden [full]",
        "Deviation (exception A8, target like the UI): `allg_normal_einquartierverwaltung` sends \
         units into a building without free space (`FREEFORQUARTER` off); the original does not check \
         the container, the units walk there and `AcceptUnit` rejects them on \
         arrival",
    ),
    (
        0x1e,
        "unit 1 not present, foreign, quartered or dead [foreign]",
        "same except for slot 0: quirk K-79 (re/masking V8) - `0x1E` reads the count as the first ID \
         (`allg_normal_tarnen`, ID 1 = first object of slot 0); the reimplementation filters the \
         foreign ID (exception A8), the original uncamouflaged a camouflaged object of slot 0; \
         \"rejected\" if the own unit is no longer selectable either",
    ),
    (
        0x2c,
        "target GUID N not selectable at slot",
        "Deviation (exception A8, deviation from K-316): `allg_normal_kopfgeldjäger` hires on the main house \
         of the opponent before there is a headhunter; the command bar offers the main house not until afterwards, \
         `GameWorld_CmdHireHeadhunter` (0x0048A8F0) does not check the target GUID",
    ),
    (
        0x2f,
        "already has a running counter-upgrade",
        "same: `wid_normal_waldläufer` starts 3377 while the slot already has a counter-upgrade \
         running; `GameWorld_CmdStartCounterUpgrade` (0x004848F0) also aborts when the slot has a \
         counter-upgrade running",
    ),
    (
        0x3c,
        "unit 0 not present, foreign, quartered or dead [ID 0]",
        "same: `allg_normal_ernten`, `allg_normal_goldabbau` and `allg_normal_buildandget_*` \
         halt all entries of their worker list, including gaps (`0x37` creates missing keys \
         with 0); ID 0 is never an object in the original, `GameWorld_CmdHalt` (0x004770A0) \
         skips it",
    ),
    (
        0x5e,
        "has no execute module (44)",
        "same except for the sound: `eng_normal_schmied` sends `0x296` (button 522) to a smith \
         without module 44; button 522 has no value set in the shipped data (measured, both maps, \
         all slots), so `GameWorld_CmdExecTempUpgrade` (0x0047EE90) deducts nothing, plays \
         the sound and posts `0x12C`, which lapses without a module",
    ),
    (
        0x5e,
        "too little mana",
        "same: `pat_normal_militärberater`/`pat_normal_fahnenträger` do not check the mana; \
         `GameWorld_CmdExecTempUpgrade` does nothing with too little mana, the reimplementation counts \
         the case as a rejection",
    ),
    (
        0x5f,
        "module 49 missing or switched off",
        "same except for sound and queue: `eng_normal_schmied` sends `0x295` to a smith whose \
         module 49 is missing or switched off (the reason names no ID; both cases act the same in \
         the original); `GameWorld_CmdDamageBuilding` (0x0047EF50) posts `0x231`, which lapses \
         without a switched-on module (before that sound and emptying of the \
         queue on a direct command)",
    ),
];

/// Cause of a rejection or partial effect from [`CAUSES`].
fn cause(code: u32, outcome: &str) -> Option<&'static str> {
    CAUSES
        .iter()
        .find(|(c, part, _)| *c == code && outcome.contains(part))
        .map(|(_, _, why)| *why)
}

/// Result text with IDs as `N` so that equal reasons coincide; the IDs 0 and 1 stay (0 is never an
/// object, 1 is the first object of slot 0).
fn normalized(label: &str) -> String {
    const BEFORE: [&str; 7] = [
        "unit",
        "horse",
        "object",
        "Quarters:",
        "container",
        "Production:",
        "GUID",
    ];
    let mut out: Vec<String> = Vec::new();
    for word in label.split(' ') {
        let id = word.len() > 1 && word.bytes().all(|b| b.is_ascii_digit());
        if id && out.last().is_some_and(|w| BEFORE.contains(&w.as_str())) {
            out.push("N".into());
        } else {
            out.push(word.into());
        }
    }
    out.join(" ")
}

/// Army of a slot: living units with a walk module and an attack module (17) without a gathering
/// module (gold 7, wood 8, harvest 12).
fn army(rules: &Rules, state: &SimState, slot: u8) -> usize {
    const GATHER: [usize; 3] = [mining::INDEX, lumber::INDEX, harvest::INDEX];
    state
        .objects()
        .filter(|o| o.id.slot() == slot && o.status & DIED == 0)
        .filter(|o| {
            has_module(rules, o.guid, module::WALK)
                && has_module(rules, o.guid, attack::INDEX as u32)
                && !GATHER.iter().any(|&m| has_module(rules, o.guid, m as u32))
        })
        .count()
}

/// ID after `prefix` in the reason text.
fn id_after(label: &str, prefix: &str) -> Option<u32> {
    let rest = &label[label.find(prefix)? + prefix.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Condition behind a collective reason (one reason text for several cases), determined from the
/// state at the end of the step with the ID from the reason; `None` without a collective reason.
/// Each condition needs its own cause in [`CAUSES`].
fn condition(rules: &Rules, state: &mut SimState, player: u8, label: &str) -> Option<String> {
    let unit = |state: &SimState, v: u32| {
        let id = ObjectId(v);
        match state.object(id, false) {
            _ if v == 0 => "ID 0",
            _ if id.slot() != player => "foreign",
            None => "gone",
            Some(o) if o.status & DIED != 0 => "dead",
            Some(o) if o.status & QUARTERED != 0 => "quartered",
            Some(_) => "selectable at the end of the step",
        }
    };
    let found = if label.contains("not present, foreign, quartered or dead") {
        unit(state, id_after(label, "unit ")?)
    } else if label.contains("cannot mount") {
        let v = id_after(label, "unit ")?;
        match state.object(ObjectId(v), false) {
            None => "gone",
            Some(o) if !o.has_module(RIDING, false) => "without riding module",
            Some(o) if o.status & (ISMOUNTED | ISMOUNTEDONBOAT) != 0 => "mounted",
            Some(_) => "no free horse",
        }
    } else if label.contains("is dead, unfinished, full or hidden") {
        let id = ObjectId(id_after(label, "Quarters: ")?);
        let sim = Sim::new(state, rules);
        if sim.object(id).is_none() {
            "gone"
        } else if is_dead(&sim, id) {
            "dead"
        } else if !is_finished(&sim, id) {
            "unfinished"
        } else if sim
            .object(id)
            .is_some_and(|o| o.has_module(module::WALK as usize, true))
        {
            "vehicle hidden"
        } else {
            "full"
        }
    } else if label.contains("exists only once") {
        let guid = id_after(label, "Production: ")?;
        let sim = Sim::new(state, rules);
        let slot = i32::from(player);
        match (
            guid_count(&sim, slot, guid),
            pending_guid_count(&sim, slot, guid),
        ) {
            (0, 0) => "free at the end of the step",
            (0, _) => "in progress",
            _ => "existing",
        }
    } else {
        return None;
    };
    Some(found.to_string())
}

fn survey_run(rules: &Rules, level: &str, nation: i32, opponents: Opponents) -> RunResult {
    let mut setup = MatchSetup::default_for(rules);
    assert!(setup.lobby.len() >= 4, "{level}");
    setup.seed = SEED;
    set_ai(&mut setup, 1, nation);
    if opponents == Opponents::Ki {
        set_ai(&mut setup, 3, (nation + 1) % 6);
    }
    setup.ki.enabled = true;
    let mut state = new_match(rules, &setup).unwrap();
    let (reached, starts) = starts_connected_by_land(rules, &state);
    assert_eq!(
        reached, starts,
        "{level}: start slots not connected over land"
    );
    let ai = ai_slots(&state);
    let mut r = RunResult {
        title: format!(
            "{} · nation {nation} · {}",
            level.rsplit('/').next().unwrap(),
            match opponents {
                Opponents::Passive => "AI against passive".to_string(),
                Opponents::Ki => format!("AI against AI (nation {})", (nation + 1) % 6),
            }
        ),
        ..Default::default()
    };
    for &s in &ai {
        r.slots.insert(
            s,
            SlotStats {
                nation: state.players[usize::from(s)].nation,
                ..Default::default()
            },
        );
    }
    // (slot, thread) -> (state, step of the last change).
    let mut marks: BTreeMap<(u8, i32), (Vec<i64>, u64)> = BTreeMap::new();
    let mut open: BTreeMap<(u8, i32), Stall> = BTreeMap::new();
    r.times.reserve(SURVEY_STEPS as usize);
    for n in 1..=SURVEY_STEPS {
        let t0 = Instant::now();
        let events = step(&mut state, rules, &[]);
        r.times.push(t0.elapsed());
        for e in &events {
            match e {
                SimEvent::KiCommand {
                    player,
                    code,
                    outcome,
                } => {
                    let label = outcome_label(outcome);
                    let norm = match condition(rules, &mut state, *player, &label) {
                        Some(c) => format!("{} [{c}]", normalized(&label)),
                        None => normalized(&label),
                    };
                    r.first_seen
                        .entry((*code, norm.clone()))
                        .or_insert_with(|| (n, *player, label));
                    *r.issued.entry((*player, *code, norm)).or_default() += 1;
                    if let Some(s) = r.slots.get_mut(player) {
                        let silence = n - s.last_command;
                        if silence > s.longest_silence.1 - s.longest_silence.0 {
                            s.longest_silence = (s.last_command, n);
                        }
                        s.last_command = n;
                        s.commands += 1;
                    }
                }
                SimEvent::Created { id } => {
                    let Some(s) = r.slots.get_mut(&id.slot()) else {
                        continue;
                    };
                    let Some(o) = state.object(*id, false) else {
                        continue;
                    };
                    if has_module(rules, o.guid, module::BUILDING) {
                        s.first_building.get_or_insert((n, o.guid));
                    } else if has_module(rules, o.guid, module::WALK) {
                        s.first_unit.get_or_insert((n, o.guid));
                    }
                }
                SimEvent::UnderAttack { object, owner, .. } => {
                    let attacker = state
                        .object(*object, false)
                        .map_or(ObjectId::NONE, |o| o.last_attacker);
                    if attacker == ObjectId::NONE || *owner >= 8 || *owner == attacker.slot() {
                        continue;
                    }
                    if let Some(s) = r.slots.get_mut(&attacker.slot()) {
                        let guid = state.object(attacker, false).map_or(0, |o| o.guid);
                        s.first_attack.get_or_insert((n, guid));
                    }
                }
                _ => {}
            }
        }
        for (i, &at) in ARMY_AT.iter().enumerate() {
            if n == at {
                for (&slot, s) in r.slots.iter_mut() {
                    s.army[i] = army(rules, &state, slot);
                }
            }
        }
        // Threads: compare the state; a state that stays the same for THREAD_STALL steps is a
        // stall (open until it changes or the thread ends).
        let mut alive = Vec::new();
        for &slot in &ai {
            let Some(e) = state.ki.engines[usize::from(slot)].as_ref() else {
                continue;
            };
            for (id, t) in e.threads.iter() {
                let key = (slot, id);
                alive.push(key);
                let mark = thread_mark(t);
                let entry = marks.entry(key).or_insert_with(|| (mark.clone(), n));
                if entry.0 != mark {
                    if let Some(mut stall) = open.remove(&key) {
                        stall.until = n;
                        r.stalls.push(stall);
                    }
                    *entry = (mark, n);
                } else if n - entry.1 >= THREAD_STALL && !open.contains_key(&key) {
                    let child = t.child.as_deref().map_or("-".to_string(), |c| {
                        format!("module {} pc {} status {:#x}", c.module, c.pc, c.status)
                    });
                    open.insert(
                        key,
                        Stall {
                            slot,
                            thread: id,
                            module: t.module,
                            since: entry.1,
                            until: n,
                            detail: format!(
                                "pc {} status {:#x} pause {} semaphore {} child {child}",
                                t.pc, t.status, t.pause_ms, t.wait_semaphore
                            ),
                            open: true,
                        },
                    );
                }
            }
        }
        marks.retain(|k, _| alive.contains(k));
        let ended: Vec<_> = open
            .keys()
            .filter(|k| !alive.contains(k))
            .copied()
            .collect();
        for k in ended {
            let mut stall = open.remove(&k).unwrap();
            stall.until = n;
            stall.open = false;
            r.stalls.push(stall);
        }
    }
    for (_, mut stall) in open {
        stall.until = SURVEY_STEPS;
        r.stalls.push(stall);
    }
    for (&slot, s) in r.slots.iter_mut() {
        let silence = SURVEY_STEPS - s.last_command;
        if silence > s.longest_silence.1 - s.longest_silence.0 {
            s.longest_silence = (s.last_command, SURVEY_STEPS);
        }
        r.threads_end.insert(
            slot,
            state.ki.engines[usize::from(slot)]
                .as_ref()
                .map_or(0, |e| e.threads.len()),
        );
    }
    r
}

/// Mean, 99% quantile and maximum in milliseconds.
fn timing(times: &[Duration]) -> (f64, f64, f64) {
    let mut ms: Vec<f64> = times.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let mean = ms.iter().sum::<f64>() / ms.len() as f64;
    let p99 = ms[(ms.len() * 99 / 100).min(ms.len() - 1)];
    (mean, p99, *ms.last().unwrap())
}

/// Step as game time "m:ss" (20 steps per second).
fn clock(step: u64) -> String {
    let s = step / 20;
    format!("{}:{:02}", s / 60, s % 60)
}

fn first(v: Option<(u64, u32)>) -> String {
    v.map_or("-".into(), |(n, guid)| format!("{} ({guid})", clock(n)))
}

fn report(runs: &[RunResult]) -> String {
    let mut out = String::from(
        "# Nation run\n\n\
         For each nation 0-5 on two maps, AI against passive slots and AI against AI (nation n against n + 1 mod 6), \
         24,000 steps each (20 game minutes), seed 0x4E4D4C31, AI kind 3 (handicap 0, stage 5), \
         release. Generated by `corpus_ki_match::ki_nation_survey`.\n\n",
    );
    let rejected: u64 = runs
        .iter()
        .flat_map(|r| &r.issued)
        .filter(|((_, _, o), _)| o.starts_with("rejected"))
        .map(|(_, n)| n)
        .sum();
    let total: u64 = runs.iter().flat_map(|r| r.issued.values()).sum();
    let thread_stalls: usize = runs.iter().map(|r| r.stalls.len()).sum();
    let slot_stalls = runs
        .iter()
        .flat_map(|r| r.slots.values())
        .filter(|s| s.longest_silence.1 - s.longest_silence.0 > SLOT_STALL)
        .count();
    let _ = writeln!(
        out,
        "## Overview\n\nAI commands {total}, of which rejected {rejected}; thread stalls (>= {THREAD_STALL} steps) \
         {thread_stalls}; slots without a command over {SLOT_STALL} steps {slot_stalls}.\n"
    );
    let _ = writeln!(
        out,
        "| Run | Slot (nation) | first building | first unit | army 10 min | army 20 min | first attack | commands | longest pause without a command | threads at end | tick mean / 99% / max (ms) |\n\
         |---|---|---|---|---|---|---|---|---|---|---|"
    );
    for r in runs {
        let (mean, p99, max) = timing(&r.times);
        for (slot, s) in &r.slots {
            let _ = writeln!(
                out,
                "| {} | {slot} ({}) | {} | {} | {} | {} | {} | {} | {} ({}-{}) | {} | {mean:.3} / {p99:.3} / {max:.3} |",
                r.title,
                s.nation,
                first(s.first_building),
                first(s.first_unit),
                s.army[0],
                s.army[1],
                first(s.first_attack),
                s.commands,
                s.longest_silence.1 - s.longest_silence.0,
                clock(s.longest_silence.0),
                clock(s.longest_silence.1),
                r.threads_end[slot],
            );
        }
    }
    let _ = writeln!(out, "\n## Performance (tick time without checksum)\n");
    for (what, filter) in [
        ("one AI slot", Opponents::Passive),
        ("two AI slots", Opponents::Ki),
    ] {
        let times: Vec<Duration> = runs
            .iter()
            .filter(|r| (r.slots.len() == 2) == (filter == Opponents::Ki))
            .flat_map(|r| r.times.iter().copied())
            .collect();
        let (mean, p99, max) = timing(&times);
        let _ = writeln!(
            out,
            "- {what}: mean {mean:.3} ms, 99% {p99:.3} ms, maximum {max:.3} ms ({} ticks)",
            times.len()
        );
    }
    let _ = writeln!(
        out,
        "\n## Rejected and partially effective AI commands by reason\n\n\
         | Network command | Result | Count | Runs | first occurrence | Cause |\n|---|---|---|---|---|---|"
    );
    let mut reasons: BTreeMap<(u32, &str), (u64, usize, String)> = BTreeMap::new();
    for r in runs {
        let mut seen_here: BTreeMap<(u32, &str), u64> = BTreeMap::new();
        for ((_, code, outcome), n) in &r.issued {
            if outcome != "accepted" {
                *seen_here.entry((*code, outcome.as_str())).or_default() += n;
            }
        }
        for (key, n) in seen_here {
            let (step, slot, raw) = &r.first_seen[&(key.0, key.1.to_string())];
            let e = reasons.entry(key).or_insert_with(|| {
                (
                    0,
                    0,
                    format!("{} · slot {slot} · {} · \"{raw}\"", r.title, clock(*step)),
                )
            });
            e.0 += n;
            e.1 += 1;
        }
    }
    if reasons.is_empty() {
        let _ = writeln!(out, "| - | none | | | | |");
    }
    for ((code, outcome), (n, in_runs, first)) in &reasons {
        let _ = writeln!(
            out,
            "| {code:#05x} | {outcome} | {n} | {in_runs} | {first} | {} |",
            cause(*code, outcome).unwrap_or("**no cause**")
        );
    }
    let _ = writeln!(out, "\n## Thread stalls\n");
    if runs.iter().all(|r| r.stalls.is_empty()) {
        let _ = writeln!(out, "none");
    }
    for r in runs {
        for s in &r.stalls {
            let _ = writeln!(
                out,
                "- {} · slot {} thread {} module {}: {}-{} ({} steps{}), {}",
                r.title,
                s.slot,
                s.thread,
                s.module,
                clock(s.since),
                clock(s.until),
                s.until - s.since,
                if s.open { ", open" } else { ", thread ended" },
                s.detail
            );
        }
    }
    let _ = writeln!(
        out,
        "\n## AI commands per network command (all runs)\n\n| Network command | accepted | partial | rejected |\n|---|---|---|---|"
    );
    let mut by_code: BTreeMap<u32, [u64; 3]> = BTreeMap::new();
    for r in runs {
        for ((_, code, outcome), n) in &r.issued {
            let column = if outcome == "accepted" {
                0
            } else if outcome.starts_with("partial") {
                1
            } else {
                2
            };
            by_code.entry(*code).or_default()[column] += n;
        }
    }
    for (code, [a, p, x]) in by_code {
        let _ = writeln!(out, "| {code:#05x} | {a} | {p} | {x} |");
    }
    out
}

/// Nation run: release only and on demand (`--ignored`), report to
/// `$NML_SHOTS_OUT/nation-run.md`. Checks that the maps have no island constraint; rejections
/// and stalls are in the report and are explained there.
// QUIRK E-107, E-108, E-109, E-110, E-111, E-112: kept
// QUIRK E-113: deviation
#[test]
#[ignore]
fn ki_nation_survey() {
    if cfg!(debug_assertions) {
        panic!("Nation run only in release (cargo test --release)");
    }
    let mut runs = Vec::new();
    for level in [EXILE, SECOND] {
        let Some(rules) = rules_for(level) else {
            return;
        };
        assert_eq!(rules.map.dim_x, 128, "{level}");
        for nation in 0..6 {
            for opponents in [Opponents::Passive, Opponents::Ki] {
                let r = survey_run(&rules, level, nation, opponents);
                eprintln!("{} fertig: {:?}", r.title, timing(&r.times));
                runs.push(r);
            }
        }
    }
    let out = report(&runs);
    match std::env::var_os("NML_SHOTS_OUT") {
        Some(dir) => {
            std::fs::write(std::path::Path::new(&dir).join("nation-run.md"), &out).unwrap()
        }
        None => eprint!("{out}"),
    }
    // Every rejection and partial effect has a cause; no thread stands still.
    let unexplained: BTreeSet<(u32, &str)> = runs
        .iter()
        .flat_map(|r| r.issued.keys())
        .filter(|(_, code, o)| o != "accepted" && cause(*code, o).is_none())
        .map(|(_, code, o)| (*code, o.as_str()))
        .collect();
    assert!(unexplained.is_empty(), "without a cause: {unexplained:#?}");
    let stalls: usize = runs.iter().map(|r| r.stalls.len()).sum();
    assert_eq!(stalls, 0, "thread stalls, see report");
    // No slot stays 5 minutes without a command, not even after a victory against passive slots
    // (measured: longest pause 872 steps).
    let silent: Vec<(&str, u8, u64)> = runs
        .iter()
        .flat_map(|r| {
            r.slots.iter().map(move |(&slot, s)| {
                (
                    r.title.as_str(),
                    slot,
                    s.longest_silence.1 - s.longest_silence.0,
                )
            })
        })
        .filter(|&(_, _, n)| n > SLOT_STALL)
        .collect();
    assert!(silent.is_empty(), "slots without a command: {silent:?}");
}
