//! The AI modules of the game against the real world: a multiplayer map with two AI slots, 600 steps
//! of simulation with the AI tick in `tick::step` (start in `new_match` via
//! `MatchSetup::ki.enabled`), without a limit, with saving and loading in the middle. Calls and
//! notes are collected by `ki::run::observe`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use opennml_rda::Vfs;
use opennml_rda::testutil::corpus_game_dir;
use opennml_sim::command::Outcome;
use opennml_sim::ki::KiPause;
use opennml_sim::ki::command::is_command;
use opennml_sim::ki::query::is_query;
use opennml_sim::ki::run::{KiObservation, observe};
use opennml_sim::ki::start::{SET_LEVEL, START_PLAYER};
use opennml_sim::ki::vm::op;
use opennml_sim::ki::world::KiNote;
use opennml_sim::player::Sight;
use opennml_sim::rules::Rules;
use opennml_sim::save::{SaveGame, checksum};
use opennml_sim::setup::{MatchSetup, SlotKind, new_match, start_package};
use opennml_sim::sim::Sim;
use opennml_sim::state::{Control, SimEvent, SimState};
use opennml_sim::tick::step;
use opennml_world::level::load_level;

const LEVEL: &str = "data/levels/multiplayer/[4players]-exile.lvl";
/// Steps of the match.
const STEPS: usize = 600;
/// Save point in the middle of the run.
const SAVE_AT: usize = STEPS / 2;
/// Lobby slots with AI.
const AI_LOBBY: [usize; 2] = [1, 3];
/// Caught crash cases that the run may expect (`builtin`, `what`); empty.
const EXPECTED_GUARDED: &[(u16, &str)] = &[];

/// Log of the run: calls per builtin ID and all notes.
type Log = KiObservation;
/// AI commands of the run per (network command, result with reason) from the `KiCommand` events.
type Commands = BTreeMap<(u32, String), u64>;

/// Result of an AI command as text for the statistics.
fn outcome_label(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Accepted => "accepted".into(),
        Outcome::Partial(why) => format!("partial: {why}"),
        Outcome::Rejected(why) => format!("rejected: {why}"),
    }
}

/// Match with two AI slots and AI on: `new_match` creates the engines and starts
/// `setschwierigkeitsgrad` synchronously for each AI slot and `multi_startplayer(slot, nation, 5 - handicap)`.
fn ai_match(rules: &Rules) -> (MatchSetup, SimState, Vec<u8>, Log) {
    let mut setup = MatchSetup::default_for(rules);
    assert!(setup.lobby.len() > AI_LOBBY[1], "{LEVEL}: too few slots");
    for i in AI_LOBBY {
        setup.lobby[i].kind = SlotKind::Ai;
    }
    setup.ki.enabled = true;
    let (state, log) = observe(|| new_match(rules, &setup).unwrap());
    let ai: Vec<u8> = (0..8u8)
        .filter(|&s| state.players[usize::from(s)].control == Control::Ai)
        .collect();
    assert_eq!(ai.len(), 2, "{ai:?}");
    for slot in 0..8u8 {
        let occupied = state.players[usize::from(slot)].control != Control::Empty;
        assert_eq!(
            state.ki.engines[usize::from(slot)].is_some(),
            occupied,
            "engine in slot {slot}"
        );
    }
    // `setschwierigkeitsgrad` itself starts threads; `multi_startplayer` comes last.
    for &slot in &ai {
        let e = state.ki.engines[usize::from(slot)].as_ref().unwrap();
        let p = &state.players[usize::from(slot)];
        let last = e.threads.get(e.next_thread_id - 1).expect("start thread");
        assert_eq!(last.module, START_PLAYER, "slot {slot}");
        assert_eq!(last.stack, [i32::from(slot), p.nation, 5 - p.handicap]);
    }
    (setup, state, ai, log)
}

/// `STEPS` steps (AI in the step); with `save_at` saved and loaded after that step.
fn run(rules: &Rules, save_at: Option<usize>) -> (Vec<u64>, SimState, Log, Vec<u8>, Commands) {
    let (setup, mut state, ai, mut log) = ai_match(rules);
    let mut sums = Vec::with_capacity(STEPS);
    let mut commands = Commands::new();
    let ((), seen) = observe(|| {
        for n in 1..=STEPS {
            for e in step(&mut state, rules, &[]) {
                if let SimEvent::KiCommand { code, outcome, .. } = e {
                    *commands.entry((code, outcome_label(&outcome))).or_default() += 1;
                }
            }
            sums.push(checksum(&state));
            if save_at == Some(n) {
                let save = SaveGame {
                    start: start_package(rules, &setup),
                    state: state.clone(),
                    commands: Vec::new(),
                };
                let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
                loaded
                    .check(rules, opennml_sim::save::RulesCompare::Full)
                    .unwrap();
                assert_eq!(checksum(&loaded.state), checksum(&state));
                state = loaded.state;
            }
        }
    });
    for (id, n) in seen.counts {
        *log.counts.entry(id).or_default() += n;
    }
    log.notes.extend(seen.notes);
    assert_eq!(state.ki.paused, KiPause::Running);
    (sums, state, log, ai, commands)
}

/// Statistics of the run: calls per ID with kind, AI commands per code and result, notes, threads
/// at the end.
fn report(log: &Log, state: &SimState, ai: &[u8], issued: &Commands) -> String {
    let mut out = format!(
        "AI against the real world: {LEVEL}, AI slots {ai:?}, {STEPS} steps, omniscient. \
         Calls per world ID (query or command)\n"
    );
    let (mut queries, mut commands) = (0u64, 0u64);
    for (&id, &n) in &log.counts {
        let kind = if is_query(id) {
            "query"
        } else {
            assert!(is_command(id), "world ID {id:#x} without a line");
            "command"
        };
        if is_query(id) {
            queries += n;
        } else {
            commands += n;
        }
        let _ = writeln!(out, "{id:#05x} {kind:<8} {n}");
    }
    let _ = writeln!(
        out,
        "IDs {}, calls: queries {queries}, commands {commands}",
        log.counts.len()
    );
    let mut notes: BTreeMap<String, usize> = BTreeMap::new();
    for n in &log.notes {
        let key = match n {
            KiNote::Limit { limit, .. } => format!("limit {limit:?}"),
            KiNote::MissingModule { target, spawn, .. } => {
                format!("module missing {target} (spawn {spawn})")
            }
            KiNote::Guarded { builtin, what, .. } => format!("Guarded {builtin:#x} {what}"),
        };
        *notes.entry(key).or_default() += 1;
    }
    let _ = writeln!(out, "Notes: {notes:?}");
    let rejected: u64 = issued
        .iter()
        .filter(|((_, outcome), _)| outcome.starts_with("rejected"))
        .map(|(_, n)| n)
        .sum();
    let total: u64 = issued.values().sum();
    let _ = writeln!(out, "AI commands: {total}, of which rejected {rejected}");
    let _ = writeln!(out, "AI commands per network command and result:");
    for ((code, outcome), n) in issued {
        let _ = writeln!(out, "{code:#04x} {n:>6} {outcome}");
    }
    for (slot, e) in state.ki.engines.iter().enumerate() {
        if let Some(e) = e {
            let _ = writeln!(
                out,
                "Slot {slot}: threads {} (started {}), values {}",
                e.threads.len(),
                e.next_thread_id,
                e.mem.values.len()
            );
        }
    }
    out
}

/// Every note of the run is an expected caught crash case ([`EXPECTED_GUARDED`]): no
/// `KiNote::Limit`, no missing module, no unexpected `Guarded`.
fn assert_expected_notes(log: &Log, what: &str) {
    let unexpected: Vec<&KiNote> = log
        .notes
        .iter()
        .filter(|n| match n {
            KiNote::Guarded { builtin, what, .. } => !EXPECTED_GUARDED.contains(&(*builtin, *what)),
            _ => true,
        })
        .collect();
    assert!(unexpected.is_empty(), "{what}: {unexpected:?}");
}

/// Warrior of nation 0 (attacker of the acceptance scene).
const WARRIOR: u32 = 1400;
/// Nearest search with marking, head filter ID and next filter ID (re/ki-queries 5.3).
const MARKING_SEARCH: u32 = 0x4E24;

/// Acceptance scene "AI slot under fire": lobby slot 1 is AI, three warriors of slot 0 attack its
/// main house and a settler, the AI marks two of the attackers via the marking nearest search
/// `0x4E24` (as in the AI tick: `query_once`, then `apply`). The savegame loads with a check and
/// the same checksum; with `NML_SHOTS_OUT` it is written as `ai_fire.nmlsave` for the
/// acceptance in the viewer (`sim_load`).
#[test]
fn ai_slot_under_fire_scene_saves_and_loads() {
    use opennml_sim::command::{Command, CommandKind};
    use opennml_sim::ki::query::{apply, query_once};
    use opennml_sim::world::{ObjectDesc, create_object, desc_flags};

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    let mut setup = MatchSetup::default_for(&rules);
    setup.lobby[AI_LOBBY[0]].kind = SlotKind::Ai;
    let mut state = new_match(&rules, &setup).unwrap();
    let ai = (0..8u8)
        .find(|&s| state.players[usize::from(s)].control == Control::Ai)
        .expect("AI slot");
    let is_building = |guid: u32| rules.types.get(&guid).is_some_and(|t| t.has_module(3));
    let own = |state: &SimState, building: bool| {
        state
            .objects()
            .find(|o| o.id.slot() == ai && is_building(o.guid) == building)
            .map(|o| (o.id, o.world_pos, o.tile))
            .expect("start object of the AI slot")
    };
    let (hq, hq_pos, hq_tile) = own(&state, true);
    let (settler, settler_pos, _) = own(&state, false);
    let attackers: Vec<_> = [(hq_pos, 384.0), (hq_pos, -384.0), (settler_pos, 256.0)]
        .into_iter()
        .map(|(p, dx)| {
            let mut sim = Sim::new(&mut state, &rules);
            let mut desc = ObjectDesc::new(WARRIOR, 0, [p[0] + dx, p[1], p[2] + 256.0]);
            desc.flags |= desc_flags::PLACED;
            create_object(&mut sim, &mut desc).expect("warrior created")
        })
        .collect();
    let attack = |units: &[_], target| Command {
        player: 0,
        kind: CommandKind::Attack {
            units: units.to_vec(),
            target,
            pos: None,
            mods: 0,
        },
    };
    step(
        &mut state,
        &rules,
        &[
            attack(&attackers[..2], hq),
            attack(&attackers[2..], settler),
        ],
    );
    let reports = |state: &SimState| state.attack_reports[usize::from(ai)].len();
    for _ in 0..600 {
        if reports(&state) >= 2 {
            break;
        }
        step(&mut state, &rules, &[]);
    }
    assert_eq!(
        reports(&state),
        2,
        "reports for the main house and the settler"
    );
    // The AI marks the two nearest warriors around its main house (head 0 = all objects of slot 0,
    // next filter = GUID filter 1400; the search skips marked ones, bit 0x80000).
    let words = [i32::from(ai), 0, hq_tile.0, hq_tile.1, 0, WARRIOR as i32];
    for _ in 0..2 {
        let mut sim = Sim::new(&mut state, &rules);
        let (_, direct) = query_once(&mut sim, ai, Sight::Omniscient, MARKING_SEARCH, &words);
        assert_eq!(direct.len(), 1, "{direct:?}");
        apply(&mut sim, &direct);
    }
    let marked = attackers
        .iter()
        .filter(|&&id| {
            state
                .object(id, false)
                .is_some_and(|o| o.ai_marks == 1 << ai)
        })
        .count();
    assert_eq!(marked, 2, "two attackers marked for the AI slot");

    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: state.clone(),
        commands: Vec::new(),
    };
    let bytes = save.to_bytes();
    let loaded = SaveGame::from_bytes(&bytes).unwrap();
    loaded
        .check(&rules, opennml_sim::save::RulesCompare::Full)
        .unwrap();
    assert_eq!(checksum(&loaded.state), checksum(&state));
    assert_eq!(loaded.state.attack_reports, state.attack_reports);
    if let Some(dir) = std::env::var_os("NML_SHOTS_OUT") {
        std::fs::write(std::path::Path::new(&dir).join("ai_fire.nmlsave"), &bytes).unwrap();
    }
}

#[test]
fn ki_runs_against_a_real_match_without_limits() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    let (plain, state, log, ai, issued) = run(&rules, None);
    let out = report(&log, &state, &ai, &issued);
    match std::env::var_os("NML_SHOTS_OUT") {
        Some(dir) => std::fs::write(std::path::Path::new(&dir).join("real-run.txt"), &out).unwrap(),
        None => eprint!("{out}"),
    }
    assert_expected_notes(&log, "without saving");
    // The AI works: both AI slots have threads after the run, and it has made queries.
    for &slot in &ai {
        let e = state.ki.engines[usize::from(slot)].as_ref().unwrap();
        assert!(!e.threads.is_empty(), "slot {slot} without threads");
    }
    assert!(log.counts.keys().any(|&id| is_query(id)));
    // The AI issues real commands and none is rejected: a rejected AI command would be an error of
    // the reimplementation (or a command that the original never sends like that).
    assert!(log.counts.keys().any(|&id| is_command(id)));
    assert!(!issued.is_empty(), "no AI command in {STEPS} steps");
    let rejected: Vec<_> = issued
        .iter()
        .filter(|((_, outcome), _)| outcome.starts_with("rejected"))
        .collect();
    assert!(rejected.is_empty(), "rejected AI commands: {rejected:?}");

    let (saved, _, saved_log, _, _) = run(&rules, Some(SAVE_AT));
    assert_expected_notes(&saved_log, "with saving");
    let first_diff = saved.iter().zip(&plain).position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "first deviation after step");
}

/// The modules started synchronously in the corpus (`.abl` `n0`, re/ki-lifecycle 1.4) except
/// `setschwierigkeitsgrad`, by their name in `nml.db`.
const SYNC_STARTED: [&str; 9] = [
    "setmaxguid",
    "add_waypoint",
    "enableaddon",
    "buildAt",
    "set_attacker",
    "add_attacker",
    "setsammelpunkt",
    "add_attacker_random",
    "truppenrückzug_abschalten",
];

/// `0x18C` in a synchronous run takes effect in the reimplementation not until after the run. That is
/// equivalent because neither `setschwierigkeitsgrad` nor any of the synchronously started corpus
/// modules contains `CALLB 0x18C` - not even in modules that they call with `CALLM` in the same
/// run (`SPAWN` starts threads that do not run until the tick).
// QUIRK E-12: kept
#[test]
fn sync_started_corpus_modules_do_not_pause_the_ki() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
    let mut roots = vec![SET_LEVEL];
    for name in SYNC_STARTED {
        let ids: Vec<u32> = rules
            .ki
            .modules
            .values()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.id)
            .collect();
        assert_eq!(ids.len(), 1, "module {name}: {ids:?}");
        roots.extend(ids);
    }
    let mut seen = BTreeSet::new();
    let mut todo = roots.clone();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(p) = rules.ki.get(id) else {
            continue;
        };
        for &w in &p.code {
            let (opcode, operand) = (w >> 28, w & 0x0FFF_FFFF);
            assert!(
                !(opcode == op::CALLB && operand == 0x18C),
                "module {id} ({}) pauses the AI",
                p.name
            );
            if opcode == op::CALLM {
                todo.push(operand);
            }
        }
    }
    eprintln!(
        "started synchronously: {} modules, with CALLM targets {} modules without CALLB 0x18C",
        roots.len(),
        seen.len()
    );
    // Counter-check of the decoding: `CALLB 0x8E` (sleep) is in many modules; `CALLB 0x18C` in no
    // module of the catalog (re/ki-builtins: 0 calls in the corpus).
    let calling = |id: u32| {
        rules
            .ki
            .modules
            .values()
            .filter(|p| {
                p.code
                    .iter()
                    .any(|&w| w >> 28 == op::CALLB && w & 0x0FFF_FFFF == id)
            })
            .count()
    };
    let (sleeping, pausing) = (calling(0x8E), calling(0x18C));
    eprintln!("Modules with CALLB 0x8E: {sleeping}, with CALLB 0x18C: {pausing}");
    assert!(sleeping > 0);
    assert_eq!(pausing, 0);
}
