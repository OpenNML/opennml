//! Catalog of the AI prototypes from the shipped data, first test vectors from the corpus against
//! the stand-in game, dry run of all modules, `KIVM_GOLDEN` and saving/loading in the middle of a
//! run.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use nml_formats::kimodule::{DB_PATH, MODULE_DIR, parse, parse_db};
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};
use nml_sim::ki::builtins::{has_case, is_local};
use nml_sim::ki::engine::{Engine, KiCtx};
use nml_sim::ki::save::check_state;
use nml_sim::ki::signature::signature;
use nml_sim::ki::stub::{StubCall, StubWorld};
use nml_sim::ki::vm::{Thread, op};
use nml_sim::ki::world::{BuiltinCx, KiNote, KiTrace, KiWorld};
use nml_sim::ki::{KiPause, KiState, MAX_CALL_DEPTH, MAX_STACK, MAX_THREADS};
use nml_sim::rules::{Fnv64, Rules};
use nml_sim::save::checksum_of;
use nml_world::level::load_level;

const LEVEL: &str = "data/levels/multiplayer/[4players]-exile.lvl";

fn load_rules(vfs: &Vfs) -> Rules {
    let scene = load_level(vfs, LEVEL).unwrap();
    Rules::load(vfs, &scene).unwrap_or_else(|e| panic!("{LEVEL}: {e:#}"))
}

#[test]
fn catalog_holds_the_280_database_modules() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    let ki = &rules.ki;
    assert_eq!(ki.modules.len(), 280);

    // The 11 modules without an entry in `nml.db` are missing from the catalog.
    let db = parse_db(&vfs.read(DB_PATH).unwrap()).unwrap();
    let listed: BTreeSet<u32> = db.entries.iter().map(|e| e.id).collect();
    assert_eq!(listed.len(), 280);
    let prefix = format!("{MODULE_DIR}/");
    let unlisted: Vec<u32> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.starts_with(&prefix) && p.ends_with(".mod"))
        .map(|p| p[prefix.len()..p.len() - 4].parse::<u32>().unwrap())
        .filter(|id| !listed.contains(id))
        .collect();
    assert_eq!(unlisted.len(), 11);
    for id in unlisted {
        assert!(ki.get(id).is_none(), "{id}");
    }

    assert!(ki.get(52554).unwrap().code.is_empty());
    let start = ki.get(351152).unwrap();
    assert_eq!((start.code.len(), start.params), (62, 0));
    assert_eq!(ki.get(55909).unwrap().params, 11);
    assert_eq!(ki.get(195235).unwrap().params, 3);

    // Loaded twice: same checksums.
    let again = load_rules(&vfs);
    assert_eq!(again.checksum, rules.checksum);
    for (id, proto) in &ki.modules {
        assert_eq!(again.ki.get(*id).unwrap().checksum, proto.checksum, "{id}");
    }
}

/// All `CALLB` IDs of the 291 module files of the shipped data.
fn corpus_builtin_ids(vfs: &Vfs) -> BTreeSet<u32> {
    let prefix = format!("{MODULE_DIR}/");
    let mut ids = BTreeSet::new();
    let paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.starts_with(&prefix) && p.ends_with(".mod"))
        .collect();
    assert_eq!(paths.len(), 291);
    for path in paths {
        let m = parse(&vfs.read(&path).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"));
        ids.extend(
            m.code
                .iter()
                .filter(|&&w| w >> 28 == op::CALLB)
                .map(|&w| w & 0x0FFF_FFFF),
        );
    }
    ids
}

#[test]
fn every_corpus_builtin_is_local_or_has_a_signature() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let ids = corpus_builtin_ids(&vfs);
    assert_eq!(ids.len(), 325);
    assert_eq!((1..=757).filter(|&id| has_case(id)).count(), 502);
    let mut local = 0;
    let mut world = 0;
    for &id in &ids {
        assert!(has_case(id), "{id:#x} without case");
        let sig = signature(id as u16);
        assert_ne!(is_local(id), sig.is_some(), "{id:#x}");
        if let Some(sig) = sig {
            world += 1;
            assert!(
                sig.outs.iter().all(|&o| o < sig.pops),
                "{id:#x}: output outside"
            );
        } else {
            local += 1;
        }
    }
    assert_eq!((local, world), (100, 225));
}

/// Engine of slot `slot` and the environment of the AI state for one call.
fn with_engine<R>(
    ki: &mut KiState,
    slot: u8,
    rules: &Rules,
    world: &mut StubWorld,
    f: impl FnOnce(&mut Engine, &mut KiCtx<'_>) -> R,
) -> R {
    let KiState { engines, rand, .. } = ki;
    let engine = engines[usize::from(slot)].get_or_insert_with(|| Engine::new(slot));
    let mut cx = KiCtx::new(&rules.ki, rand, world);
    f(engine, &mut cx)
}

/// Innermost instance of a thread (last child of the `CALLM` chain).
fn innermost(t: &Thread) -> &Thread {
    let mut t = t;
    while let Some(c) = &t.child {
        t = c;
    }
    t
}

fn call(id: u16, args: &[i32]) -> StubCall {
    StubCall {
        slot: 3,
        thread: 0,
        id,
        args: args.to_vec(),
    }
}

#[test]
fn setschwierigkeitsgrad_sets_the_level_values() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    for s in 0..=5 {
        let mut ki = KiState::new(0);
        let mut world = StubWorld {
            answers: BTreeMap::from([(140, s)]),
            record: true,
            ..Default::default()
        };
        let id = with_engine(&mut ki, 3, &rules, &mut world, |e, cx| {
            e.run_sync(cx, 351152, &[]).map(|r| r.id)
        });
        assert_eq!(id, Some(0), "stage {s}");
        let e = ki.engines[3].as_ref().unwrap();
        let level = [1500, 1000, 750, 500, 250, 0][s as usize];
        assert_eq!(
            e.mem.values,
            BTreeMap::from([(215, 0), (235, s), (302, level)]),
            "stage {s}"
        );
        let threads: BTreeMap<i32, (u32, Vec<i32>)> = e
            .threads
            .iter()
            .map(|(id, t)| (id, (t.module, t.stack.clone())))
            .collect();
        assert_eq!(
            threads,
            BTreeMap::from([
                (1, (599511, vec![3])),
                (2, (403951, vec![])),
                (3, (840794, vec![])),
                (4, (1370541, vec![])),
            ]),
            "stage {s}"
        );
        assert_eq!(e.next_thread_id, 5);
        assert_eq!(world.calls, [call(140, &[])], "stage {s}");
        assert!(world.notes.is_empty(), "{:?}", world.notes);
    }
}

#[test]
fn stopki_sets_value_255_only() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    let mut ki = KiState::new(0);
    let mut world = StubWorld::default();
    with_engine(&mut ki, 2, &rules, &mut world, |e, cx| {
        assert_eq!(e.spawn(cx, 9826, &[5]), Some(0));
        e.tick(cx);
    });
    let e = ki.engines[2].as_ref().unwrap();
    assert_eq!(e.mem.values, BTreeMap::from([(255, 1)]));
    assert!(e.threads.is_empty());
    assert!(world.counts.is_empty(), "{:?}", world.counts);
    assert!(world.notes.is_empty(), "{:?}", world.notes);
}

#[test]
fn startplayer_queries_until_the_first_pause() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    let mut ki = KiState::new(0);
    let mut world = StubWorld {
        answers: BTreeMap::from([(138, 128), (139, 128)]),
        record: true,
        ..Default::default()
    };
    let args = [3, 4, 2, 0, 36, 99, 0, 1, 3, 0, 1];
    with_engine(&mut ki, 3, &rules, &mut world, |e, cx| {
        assert_eq!(e.spawn(cx, 55909, &args), Some(0));
        for n in 1..=24 {
            e.tick(cx);
            for (_, t) in e.threads.iter() {
                assert_eq!(innermost(t).pause_ms, 0, "tick {n}");
            }
        }
        e.tick(cx);
    });
    let e = ki.engines[3].as_ref().unwrap();
    assert_eq!(e.threads.len(), 1);
    assert_eq!(e.next_thread_id, 1);
    let t = e.threads.get(0).unwrap();
    let child = t.child.as_deref().expect("child 599511");
    assert_eq!(child.module, 599511);
    assert!(child.child.is_none());
    assert_eq!(child.pause_ms, 5487);

    let mut expected = vec![call(70, &[]), call(123, &[0, 0x0100_0000, 0x0100_0001])];
    expected.extend([0, 1, 2, 4, 5, 6, 7].map(|p| call(133, &[p])));
    expected.push(call(127, &[]));
    assert_eq!(world.calls, expected);
    assert_eq!(
        e.mem.values,
        BTreeMap::from([(218, 1), (250, 0), (251, 0), (252, 0)])
    );
    assert!(world.notes.is_empty(), "{:?}", world.notes);
}

/// Ticks of the dry run and of the scenes with a checksum sequence.
const TICKS: usize = 600;

/// Startup modules with "busy threads" in the dry run (measured): threads that are still running
/// after `TICKS` ticks without ever having had a tick without an instruction since their first
/// instruction. A startup module also counts with the busy threads it starts (e.g. `startplayer`).
/// Causes, clarified for each busy thread via the loop of the last tick and a run over 60,000
/// ticks:
/// - Sleep duration `m_iValues[302]` is missing because the module runs alone
///   (`setschwierigkeitsgrad` sets it, but also to 0 at stage 5) -> `0x8E` with 0: all six
///   `*_normal_upgrader` (also `sid`/`spa`), `*_upgradeanalyser`, `allg_normal_firstattack`,
///   `pri_normal_buildingverwaltung` (929838).
/// - Module without any pause in the code (endless in the original, too):
///   `allg_normal_lagerposcheck`, `allg_normal_repairverwaltung`.
/// - Main loop `while m_iValues[255] == 0` pauses only in branches that the answers of the
///   stand-in game (0 on `0x80`, `0x85`, `0x92`, `0xA5`, `0x132`, `0xD2`/`0xD3`/`0xD5`) or missing
///   engine values and tables never reach: `leben`, `heerde_areachecker`, `*_situationanalyser`
///   (branch on `0x80() == 5`), the five other `*_buildingverwaltung`, `allg_normal_gold-`,
///   `-nahrungs-` and `-wohnhausverwaltung`.
/// - Long calculation without a pause that does not end until after `TICKS`: `multi_wegpunkte_generieren`,
///   `multi_attackpunkte_generieren`.
const BUSY_MODULES: &[u32] = &[
    5774, 55909, 70794, 160202, 191157, 218822, 261639, 261640, 261643, 261672, 261717, 261748,
    350619, 351152, 351743, 460928, 580804, 659452, 664989, 664990, 664993, 665022, 665067, 665098,
    674322, 742262, 838136, 838137, 838140, 838169, 838214, 838245, 840794, 909460, 929760, 929761,
    929764, 929793, 929838, 929869, 936540, 1035225, 1035552, 1135065, 1221804,
];

/// Statistics of a thread in the dry run (via `KiWorld::trace`).
#[derive(Debug, Default)]
struct ThreadStats {
    /// Tick of the first instruction.
    first: usize,
    /// Ticks without an instruction since the first (pause, semaphore or `stop`).
    idle_ticks: usize,
    /// Transitions from a tick with an instruction to one without.
    rests: usize,
    /// Executed an instruction in the last tick.
    was_active: bool,
}

/// Stand-in game with instruction statistics for the dry run.
#[derive(Default)]
struct DryWorld {
    stub: StubWorld,
    /// Executed instructions.
    ops: u64,
    /// Highest `CALLM` depth of an executed instruction.
    max_depth: usize,
    /// Threads with an instruction in the current tick.
    active: BTreeSet<i32>,
}

impl KiWorld for DryWorld {
    fn builtin(&mut self, id: u16, cx: &mut BuiltinCx<'_>) {
        self.stub.builtin(id, cx);
    }

    fn note(&mut self, note: KiNote) {
        self.stub.note(note);
    }

    fn trace(&mut self, t: &KiTrace) {
        self.ops += 1;
        self.max_depth = self.max_depth.max(t.depth);
        self.active.insert(t.thread);
    }
}

/// Largest stack across a thread and all its children.
fn max_stack(t: &Thread) -> usize {
    let mut n = t.stack.len();
    let mut node = t.child.as_deref();
    while let Some(c) = node {
        n = n.max(c.stack.len());
        node = c.child.as_deref();
    }
    n
}

/// Scene of the dry run: module `module` alone in slot 0 with zero arguments, stand-in game with
/// map size 128 x 128.
fn dry_scene(rules: &Rules, module: u32) -> (KiState, StubWorld) {
    let params = rules.ki.get(module).unwrap().params as usize;
    let mut ki = KiState::new(0);
    let mut world = StubWorld {
        answers: BTreeMap::from([(138, 128), (139, 128)]),
        ..Default::default()
    };
    with_engine(&mut ki, 0, rules, &mut world, |e, cx| {
        assert_eq!(e.spawn(cx, module, &vec![0; params]), Some(0), "{module}");
    });
    (ki, world)
}

/// Result of the dry run of a module.
struct DryRun {
    world: DryWorld,
    /// Highest stack depth, thread count and `CALLM` depth (read after every tick, the depth
    /// additionally per instruction).
    max_stack: usize,
    max_threads: usize,
    max_depth: usize,
    /// Started threads (`next_thread_id`).
    started: i32,
    rests: usize,
    /// Busy threads: (thread ID, tick of the first instruction, module).
    busy: Vec<(i32, usize, u32)>,
    /// Tick after which `0x18C` paused the AI.
    paused_at: Option<usize>,
    /// Time of the ticks without reading.
    elapsed: Duration,
}

/// Dry run of a module over `TICKS` ticks.
fn dry_run(rules: &Rules, module: u32) -> DryRun {
    let (mut ki, stub) = dry_scene(rules, module);
    let mut world = DryWorld {
        stub,
        ..Default::default()
    };
    let mut stats: BTreeMap<i32, ThreadStats> = BTreeMap::new();
    let (mut max_stack_len, mut max_threads, mut max_depth) = (0, 0, 0);
    let mut paused_at = None;
    let mut elapsed = Duration::ZERO;
    for n in 1..=TICKS {
        world.active.clear();
        let start = Instant::now();
        ki.tick_engines(&rules.ki, &mut world);
        elapsed += start.elapsed();
        if paused_at.is_none() && ki.paused == KiPause::Paused {
            paused_at = Some(n);
        }
        let e = ki.engines[0].as_ref().unwrap();
        max_threads = max_threads.max(e.threads.len());
        for (id, t) in e.threads.iter() {
            max_stack_len = max_stack_len.max(max_stack(t));
            max_depth = max_depth.max(t.depth());
            let active = world.active.contains(&id);
            match stats.get_mut(&id) {
                Some(s) => {
                    if !active {
                        s.idle_ticks += 1;
                        s.rests += usize::from(s.was_active);
                    }
                    s.was_active = active;
                }
                None if active => {
                    stats.insert(
                        id,
                        ThreadStats {
                            first: n,
                            was_active: true,
                            ..Default::default()
                        },
                    );
                }
                None => {}
            }
        }
    }
    let e = ki.engines[0].as_ref().unwrap();
    let busy = stats
        .iter()
        .filter(|(_, s)| s.idle_ticks == 0)
        .filter_map(|(&id, s)| e.threads.get(id).map(|t| (id, s.first, t.module)))
        .collect();
    DryRun {
        max_stack: max_stack_len,
        max_threads,
        max_depth: max_depth.max(world.max_depth),
        started: e.next_thread_id,
        rests: stats.values().map(|s| s.rests).sum(),
        busy,
        paused_at,
        elapsed,
        world,
    }
}

/// One line of the statistics `dryrun.txt`.
fn dry_run_line(out: &mut String, rules: &Rules, module: u32, r: &DryRun) {
    let p = rules.ki.get(module).unwrap();
    let notes = &r.world.stub.notes;
    let guarded: Vec<String> = notes
        .iter()
        .filter_map(|n| match n {
            KiNote::Guarded { builtin, what, .. } => Some(format!("{builtin:#x} {what}")),
            _ => None,
        })
        .collect();
    let missing: BTreeSet<u32> = notes
        .iter()
        .filter_map(|n| match n {
            KiNote::MissingModule { target, .. } => Some(*target),
            _ => None,
        })
        .collect();
    let calls: Vec<String> = r
        .world
        .stub
        .counts
        .iter()
        .map(|(id, n)| format!("{id:#x}:{n}"))
        .collect();
    let busy: Vec<String> = r
        .busy
        .iter()
        .map(|(id, first, m)| format!("T{id}@{first}:{m}"))
        .collect();
    let us_per_tick = r.elapsed.as_secs_f64() * 1e6 / TICKS as f64;
    let _ = writeln!(
        out,
        "{module:>8} {:<30} instructions={} threads={}/{} rests={} stack={} depth={} \
         paused={} guarded={} [{}] missing={missing:?} us/tick={us_per_tick:.1} world=[{}] \
         busy=[{}]",
        p.name,
        r.world.ops,
        r.max_threads,
        r.started,
        r.rests,
        r.max_stack,
        r.max_depth,
        r.paused_at.map_or("-".to_string(), |n| n.to_string()),
        guarded.len(),
        guarded.join(", "),
        calls.join(" "),
        busy.join(" "),
    );
}

/// Every `nml.db` module alone against the stand-in game, without a limit and with at most a
/// quarter of each limit. Statistics to `$NML_SHOTS_OUT/dryrun.txt`, to stderr without the
/// variable.
#[test]
fn all_reachable_modules_run_dry_against_the_stub() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    let mut out = String::from(
        "Dry run: each module alone in slot 0, zero arguments, answers {138: 128, 139: 128}, \
         600 ticks. threads = highest/started, stack and depth = maximum values, us/tick = \
         run time per tick, world = calls per world ID, busy = thread@first tick:module\n",
    );
    let (mut max_stack_len, mut max_threads, mut max_depth) = (0, 0, 0);
    let mut busy = Vec::new();
    let mut limits = Vec::new();
    for &module in rules.ki.modules.keys() {
        let r = dry_run(&rules, module);
        dry_run_line(&mut out, &rules, module, &r);
        max_stack_len = max_stack_len.max(r.max_stack);
        max_threads = max_threads.max(r.max_threads);
        max_depth = max_depth.max(r.max_depth);
        if !r.busy.is_empty() {
            busy.push(module);
        }
        limits.extend(
            r.world
                .stub
                .notes
                .iter()
                .filter(|n| matches!(n, KiNote::Limit { .. }))
                .cloned(),
        );
    }
    let _ = writeln!(
        out,
        "Modules {}: highest stack depth {max_stack_len}, threads {max_threads}, CALLM depth \
         {max_depth}; startup modules with busy threads {}: {busy:?}",
        rules.ki.modules.len(),
        busy.len()
    );
    match std::env::var_os("NML_SHOTS_OUT") {
        Some(dir) => std::fs::write(std::path::Path::new(&dir).join("dryrun.txt"), &out).unwrap(),
        None => eprint!("{out}"),
    }
    assert_eq!(rules.ki.modules.len(), 280);
    assert!(limits.is_empty(), "{limits:?}");
    // At most a quarter of each limit.
    assert!(max_stack_len <= MAX_STACK / 4, "stack {max_stack_len}");
    assert!(max_threads <= MAX_THREADS / 4, "threads {max_threads}");
    assert!(max_depth <= MAX_CALL_DEPTH / 4, "CALLM depth {max_depth}");
    assert_eq!(busy, BUSY_MODULES);
}

/// Checksum over the 600 AI checksums of the scene `kivm_scene`.
/// Measured.
const KIVM_GOLDEN: u64 = 0x8042_30c2_e985_ef3d;

/// Scene: engines in slots 0-5, per slot `setschwierigkeitsgrad` synchronously, then
/// `multi_startplayer` with nation `p` and stage 5; stand-in game with map size 128 x 128 and
/// difficulty 1.
fn kivm_scene(rules: &Rules) -> (KiState, StubWorld) {
    let mut ki = KiState::new(0);
    let mut world = StubWorld {
        answers: BTreeMap::from([(138, 128), (139, 128), (140, 1)]),
        ..Default::default()
    };
    for p in 0..6u8 {
        with_engine(&mut ki, p, rules, &mut world, |e, cx| {
            assert!(e.run_sync(cx, 351152, &[]).is_some(), "slot {p}");
            let p = i32::from(p);
            assert!(e.spawn(cx, 195235, &[p, p, 5]).is_some(), "slot {p}");
        });
    }
    (ki, world)
}

/// `KiState` back and forth via `postcard`, with the load check.
fn round_trip(ki: &KiState) -> KiState {
    let bytes = postcard::to_allocvec(ki).unwrap();
    let back: KiState = postcard::from_bytes(&bytes).unwrap();
    check_state(&back).unwrap();
    assert_eq!(back, *ki);
    back
}

/// `TICKS` ticks of the scene `(ki, world)`: sequence of the checksums after every tick and state
/// at the end. With `save_at` it is saved and loaded after that tick; `at_save` checks the state
/// beforehand (save point as intended). No `KiNote::Limit`.
fn checksum_run(
    rules: &Rules,
    (mut ki, mut world): (KiState, StubWorld),
    save_at: Option<usize>,
    at_save: impl Fn(&KiState),
) -> (Vec<u64>, KiState) {
    let mut sequence = Vec::with_capacity(TICKS);
    for n in 1..=TICKS {
        ki.tick_engines(&rules.ki, &mut world);
        sequence.push(checksum_of(&ki));
        if save_at == Some(n) {
            at_save(&ki);
            ki = round_trip(&ki);
        }
    }
    let limits: Vec<&KiNote> = world
        .notes
        .iter()
        .filter(|n| matches!(n, KiNote::Limit { .. }))
        .collect();
    assert!(limits.is_empty(), "{limits:?}");
    (sequence, ki)
}

#[test]
fn kivm_checksums_match_the_golden_sequence() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    let (sequence, ki) = checksum_run(&rules, kivm_scene(&rules), None, |_| {});
    // Counter-check before the golden: every engine still has threads after the last tick.
    for p in 0..6 {
        let e = ki.engines[p].as_ref().unwrap();
        assert!(!e.threads.is_empty(), "slot {p} without threads");
    }
    let mut h = Fnv64::default();
    for c in &sequence {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, KIVM_GOLDEN, "{:#018x}", h.0);
}

/// Instances below the threads (children, grandchildren ...) with their depth from 1.
fn descendants(ki: &KiState) -> Vec<(usize, &Thread)> {
    let mut out = Vec::new();
    for (_, t) in ki.engines.iter().flatten().flat_map(|e| e.threads.iter()) {
        let mut node = t.child.as_deref();
        let mut depth = 1;
        while let Some(c) = node {
            out.push((depth, c));
            node = c.child.as_deref();
            depth += 1;
        }
    }
    out
}

/// A thread has a child (or a deeper `CALLM`).
fn has_child(ki: &KiState) -> bool {
    !descendants(ki).is_empty()
}

/// A child or grandchild has a remaining pause.
fn has_resting_child(ki: &KiState) -> bool {
    descendants(ki).iter().any(|(_, c)| c.pause_ms > 0)
}

/// A thread has a grandchild (`CALLM` depth 2).
fn has_grandchild(ki: &KiState) -> bool {
    descendants(ki).iter().any(|&(depth, _)| depth == 2)
}

/// Save point in the middle of the scene `kivm_scene`.
const KIVM_SAVE_AT: usize = TICKS / 2;
/// Save point in the middle of a `CALLM` with a remaining pause (E-10, re/ki-vm 12.7). The scene
/// `kivm_scene` has no child in any of its 600 ticks (measured); hence the dry run of
/// `multi_normal_init`, first tick >= 300 with a remaining pause of a child (measured).
const CALLM_REST_MODULE: u32 = 191157;
const CALLM_REST_AT: usize = 300;
/// Save point with a grandchild: the only `CALLM` depth 2 in the dry run,
/// `multi_normal_haupthausattacke` after tick 16 (measured).
const GRANDCHILD_MODULE: u32 = 908477;
const GRANDCHILD_AT: usize = 16;

/// Scene with saving/loading after `save_at` against the same scene without.
fn assert_survives_saving(
    rules: &Rules,
    scene: impl Fn(&Rules) -> (KiState, StubWorld),
    save_at: usize,
    at_save: impl Fn(&KiState),
    what: &str,
) {
    let (plain, _) = checksum_run(rules, scene(rules), None, |_| {});
    let (saved, _) = checksum_run(rules, scene(rules), Some(save_at), at_save);
    let first_diff = saved.iter().zip(&plain).position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "{what}: first deviation after tick");
}

/// Deviation from E-10 (re/ki-vm 12.7): saving and loading changes neither the thread sequence nor
/// remaining pauses nor running children.
#[test]
fn kivm_checksums_survive_saving_and_loading() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    assert_survives_saving(&rules, kivm_scene, KIVM_SAVE_AT, |_| {}, "scene kivm_scene");

    assert_survives_saving(
        &rules,
        |r| dry_scene(r, CALLM_REST_MODULE),
        CALLM_REST_AT,
        |ki| assert!(has_resting_child(ki), "no child with a remaining pause"),
        "CALLM with a remaining pause",
    );
    assert_survives_saving(
        &rules,
        |r| dry_scene(r, GRANDCHILD_MODULE),
        GRANDCHILD_AT,
        |ki| assert!(has_grandchild(ki), "no grandchild"),
        "grandchild",
    );
}

/// The save points are the first of their kind (measured): the scene `kivm_scene` without a child
/// in all ticks, the `CALLM` with a remaining pause for the first time from tick 300, the
/// grandchild for the first time at all.
#[test]
fn kivm_save_points_are_the_first_of_their_kind() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let rules = load_rules(&vfs);
    let first =
        |(mut ki, mut world): (KiState, StubWorld), from: usize, found: fn(&KiState) -> bool| {
            (1..=TICKS).find(|&n| {
                ki.tick_engines(&rules.ki, &mut world);
                n >= from && found(&ki)
            })
        };
    assert_eq!(first(kivm_scene(&rules), 1, has_child), None);
    assert_eq!(
        first(
            dry_scene(&rules, CALLM_REST_MODULE),
            TICKS / 2,
            has_resting_child
        ),
        Some(CALLM_REST_AT)
    );
    assert_eq!(
        first(dry_scene(&rules, GRANDCHILD_MODULE), 1, has_grandchild),
        Some(GRANDCHILD_AT)
    );
}
