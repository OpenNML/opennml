//! Coverage and hostile arguments: every world builtin ID is exactly one query or one command; the handler table
//! matches the signatures; no query builtin and no query code panics, changes the state (except through [`Direct`]) or
//! unbalances the stack differently than documented (E-34); every command survives hostile, combined and near-limit
//! arguments without a panic and with a loadable state.

use std::collections::BTreeSet;

use super::handlers::{ROWS, Run, row};
use super::tests_player::{FLOOR, ME};
use super::tests_search::{ME_I, put, scene};
use super::{SimKiWorld, is_query, query_once};
use crate::combat::testkit::{self as ck, ARCHER, HQ, WARRIOR};
use crate::command::Outcome;
use crate::command::testkit::{self as ct, BASKET, COLONIST, HOUSE, site};
use crate::id::ObjectId;
use crate::ki::builtins::{has_case, is_local};
use crate::ki::command::{self, is_command};
use crate::ki::signature::signature;
use crate::ki::testkit::{int, list, proto};
use crate::ki::vm::Thread;
use crate::ki::world::{BuiltinCx, KiNote, KiWorld};
use crate::module::ability_testkit as kit;
use crate::object::status;
use crate::player::Sight;
use crate::rules::Rules;
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::{Control, ReportTile, SimEvent, SimState};

/// World builtins: `case` in the interpreter and no VM-own implementation.
fn world_ids() -> BTreeSet<u16> {
    (1..=757u16)
        .filter(|&id| has_case(u32::from(id)) && !is_local(u32::from(id)))
        .collect()
}

#[test]
fn every_world_builtin_is_a_query_or_a_command() {
    // Each of the 362 world IDs has exactly one query row or one command row; there is no fallback answer by
    // signature.
    let world = world_ids();
    let queries: BTreeSet<u16> = (0..=u16::MAX).filter(|&id| is_query(id)).collect();
    let commands: BTreeSet<u16> = (0..=u16::MAX).filter(|&id| is_command(id)).collect();
    assert_eq!(world.len(), 362);
    assert!(queries.is_subset(&world), "query without world ID");
    assert!(commands.is_subset(&world), "command without world ID");
    let both: Vec<u16> = queries.intersection(&commands).copied().collect();
    assert!(both.is_empty(), "query and command at once: {both:x?}");
    let missing: Vec<u16> = world
        .iter()
        .filter(|id| !queries.contains(id) && !commands.contains(id))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "neither query nor command: {missing:x?}"
    );
    assert_eq!((queries.len(), commands.len()), (264, 98));
}

#[test]
fn query_rows_match_the_e1a_signatures() {
    // Where a signature exists (corpus IDs), the handler normally has the same pops, push, output positions and pause;
    // the code matches on every query row.
    let mut compared = 0;
    for r in ROWS {
        for id in r.lo..=r.hi {
            let Some(sig) = signature(id) else { continue };
            compared += 1;
            let outs: BTreeSet<u8> = r.outs.iter().map(|&(a, _)| a).collect();
            let sig_outs: BTreeSet<u8> = sig.outs.iter().copied().collect();
            assert_eq!(
                (r.pops, r.push, outs, r.pause_ms),
                (sig.pops, sig.push, sig_outs, sig.pause_ms),
                "{id:#x}"
            );
            if matches!(r.run, Run::Ask { .. }) {
                assert_eq!(r.code, sig.code, "{id:#x}");
            }
        }
    }
    // 148 of the 225 corpus world IDs with a signature are queries, the other 77 commands (measured).
    assert_eq!(compared, 148);
}

/// Hostile values for every argument position.
const HOSTILE: [i32; 10] = [
    0,
    -1,
    1,
    8,
    9,
    31,
    1000,
    i32::MIN,
    i32::MAX,
    0x7FFF_FFFF >> 12,
];

/// Extreme values of the position pairs: "center +- radius/half-width" with two different extremes is the overflow
/// class (e.g. `x = i32::MIN, r = 1`).
const EXTREMES: [i32; 4] = [i32::MIN, i32::MAX, -1, 1];

/// Filter IDs without an entry, in addition to the object IDs of the scene.
const FILTER_IDS: [i32; 2] = [-5, 999];

/// Argument sequences of length `n`: every value everywhere; every value from [`HOSTILE`], `extra` (object IDs of the
/// scene) and [`FILTER_IDS`] at each position (rest 0); with `pairs` all pairs of positions with values from
/// [`EXTREMES`] (rest 0); 16 fixed pseudo-random mixes.
fn hostile_args(n: usize, seed: u32, extra: &[i32], pairs: bool) -> Vec<Vec<i32>> {
    let mut sets: Vec<Vec<i32>> = HOSTILE.iter().map(|&v| vec![v; n]).collect();
    for i in 0..n {
        for &v in HOSTILE.iter().chain(extra).chain(&FILTER_IDS) {
            let mut a = vec![0; n];
            a[i] = v;
            sets.push(a);
        }
    }
    if pairs {
        for i in 0..n {
            for j in i + 1..n {
                for a in EXTREMES {
                    for b in EXTREMES {
                        let mut v = vec![0; n];
                        (v[i], v[j]) = (a, b);
                        sets.push(v);
                    }
                }
            }
        }
    }
    let mut x = seed;
    for _ in 0..16 {
        sets.push(
            (0..n)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    HOSTILE[(x >> 16) as usize % HOSTILE.len()]
                })
                .collect(),
        );
    }
    sets.sort();
    sets.dedup();
    sets
}

/// List builtins: the first argument is a list variable, the third is `n`.
fn is_list_builtin(id: u16) -> bool {
    matches!(id, 0x105..=0x107 | 0x112..=0x115 | 0x117 | 0x11E | 0x11F | 0x121)
}

/// Command builtins with list arguments `(&obj, start, n, …)`: group movement, behavior and attack for lists (pattern D
/// and E, re/ki-queries 7.1).
fn is_list_command(id: u16) -> bool {
    matches!(
        id,
        0xF7 | 0xF8 | 0xFB | 0xFC | 0xFF | 0x100 | 0x103 | 0x104 | 0x19B | 0x19C | 0x1A7..=0x1A9
    )
}

/// List addresses of the test instance: list 0 (filled), list 1 (empty), integer variable 1 with a pointer without a
/// list, variable 2 with pointer 0, a nonexistent list.
fn list_addresses() -> [i32; 5] {
    [
        list(0) as i32,
        list(1) as i32,
        int(1) as i32,
        int(2) as i32,
        list(5) as i32,
    ]
}

/// Value of integer variable 1: a pointer with no list behind it.
const NO_LIST: i32 = 12_345;

/// Thread of module 7 for [`ME`]: integer variables 0-7, lists 0 and 1; list 0 holds valid, missing and stale object
/// IDs from key 0.
fn thread(stack: Vec<i32>, entries: &[i32]) -> Thread {
    thread_of(ME, stack, entries)
}

/// [`thread`] for the instance of slot `me`.
fn thread_of(me: u8, stack: Vec<i32>, entries: &[i32]) -> Thread {
    let ranges = [(0, 8), (list(0), 2)];
    let mut t = Thread::from_prototype(&proto(7, &ranges, &[], &[]), 2, me, stack);
    t.vars.set(int(1), NO_LIST);
    let l = t.vars.list_at_mut(list(0) as i32).expect("list 0");
    for (k, &v) in (0..).zip(entries) {
        l.insert(k, v);
    }
    t
}

/// Query codes that may produce direct changes: searches with a mark, set/clear mark, clear the marks of a list, take a
/// report.
const WRITING_CODES: [u32; 10] = [
    0x4E24, 0x4E25, 0x4E26, 0x4E27, 0x50C3, 0x50C8, 0x4E44, 0x4E45, 0x4FA6, 0x512E,
];

/// Test world: objects of several slots (alive, dead, construction site, fishing basket, main house), marks and attack
/// reports; returns the object IDs for the lists.
fn hostile_scene() -> (Rules, SimState, Vec<i32>) {
    let (rules, mut state) = scene();
    let mut sim = Sim::new(&mut state, &rules);
    let mine = put(&mut sim, WARRIOR, ME_I, (10, 10));
    let enemy = put(&mut sim, WARRIOR, 2, (12, 10));
    let archer = put(&mut sim, ARCHER, 3, (20, 20));
    let hq = put(&mut sim, HQ, ME_I, (30, 30));
    let colonist = put(&mut sim, COLONIST, 2, (11, 11));
    let basket = put(&mut sim, BASKET, ME_I, (13, 13));
    let unfinished = site(&mut sim, HOUSE, 2, (40, 40)).as_i32();
    let dead = put(&mut sim, WARRIOR, 3, (14, 14));
    sim.object_mut(ObjectId::from_i32(dead)).unwrap().status |= status::DIED;
    sim.state
        .object_mut(ObjectId::from_i32(mine), false)
        .unwrap()
        .ai_marks = 1 << 3;
    let stale = ObjectId::new(ObjectId::from_i32(enemy).index() as u16, 2, 1000).as_i32();
    for (victim, x, z) in [(enemy, 12, 10), (archer, 20, 20), (dead, 14, 14)] {
        sim.state.attack_reports[usize::from(ME)].insert(victim, ReportTile { x, z });
    }
    let entries = vec![
        mine, enemy, archer, hq, colonist, basket, unfinished, dead, 0, -1, stale,
    ];
    (rules, state, entries)
}

/// Builtins whose flow depends on the nation of `me` (`0x45`, `0xA4`, `0x116`, `0x133`-`0x135`).
const NATION_BUILTINS: [u16; 6] = [0x45, 0xA4, 0x116, 0x133, 0x134, 0x135];

/// Result of a handler run as the test expects it from the arguments.
struct Expect {
    pushes: usize,
    guarded: Option<&'static str>,
}

/// Expected stack behavior of `id` (normal case from the table, exceptions per E-34): lists without a pointer or list
/// push nothing, `n ∉ (0, 100)` pushes 0; nation buildings without a valid nation push nothing; `0xE6` with a slot
/// outside 0-7 reports `Guarded`.
fn expect(id: u16, args: &[i32], t: &Thread, nation: i32) -> Expect {
    let r = row(id).expect("row");
    let normal = usize::from(r.push);
    if is_list_builtin(id) {
        let pointer = t.vars.get(args[0] as u32);
        let n = args[2];
        return if pointer == 0 {
            Expect {
                pushes: 0,
                guarded: None,
            }
        } else if !(1..100).contains(&n) {
            Expect {
                pushes: normal,
                guarded: None,
            }
        } else if t.vars.list_at(pointer).is_none() {
            Expect {
                pushes: 0,
                guarded: Some("list pointer without a list"),
            }
        } else {
            Expect {
                pushes: normal,
                guarded: None,
            }
        };
    }
    match id {
        0x133..=0x135 if !(0..6).contains(&nation) => Expect {
            pushes: 0,
            guarded: None,
        },
        0xE6 if !(0..8).contains(&args[0]) => Expect {
            pushes: normal,
            guarded: Some("slot outside 0-7"),
        },
        _ => Expect {
            pushes: normal,
            guarded: None,
        },
    }
}

/// All argument sequences of a query builtin; position pairs only with `pairs`.
fn builtin_args(id: u16, pops: u8, entries: &[i32], pairs: bool) -> Vec<Vec<i32>> {
    let sets = hostile_args(usize::from(pops), u32::from(id), entries, pairs);
    if !is_list_builtin(id) && !is_list_command(id) {
        return sets;
    }
    let mut out = Vec::new();
    for addr in list_addresses() {
        for n in [0, 1, 3, 11, 12, 99, 100] {
            for start in [0, 5, -1, i32::MAX] {
                let mut a = sets[0].clone();
                (a[0], a[1], a[2]) = (addr, start, n);
                out.push(a);
            }
        }
        for &v in &HOSTILE {
            let mut a = vec![v; usize::from(pops)];
            a[0] = addr;
            out.push(a);
        }
    }
    out.extend(sets);
    out
}

#[test]
fn hostile_arguments_never_panic() {
    // Handler runs through SimKiWorld: no panic, stack height and pause as in the table or the documented exception,
    // notes exactly as expected; without a Direct code the checksum stays the same. The builtins that read the nation
    // of `me` additionally run with an invalid nation (9, E-34 at 0x133-0x135). Position pairs of the extreme values
    // only omniscient (run time).
    for nation in [0, 9] {
        for sight in [Sight::Omniscient, Sight::Fog] {
            let (rules, mut base, entries) = hostile_scene();
            base.players[usize::from(ME)].nation = nation;
            // Instead of cloning the whole state per run: the queries change only `ai_marks` of the objects and the
            // attack reports (`Direct`). Exactly that is reset; the checksum comparison per ID proves that nothing else
            // changed.
            let marks: Vec<Option<u8>> = base
                .tables
                .iter()
                .flat_map(|t| &t.objects)
                .map(|o| o.as_ref().map(|o| o.ai_marks))
                .collect();
            let reports = base.attack_reports.clone();
            let mut state = base;
            let mut sim = Sim::new(&mut state, &rules);
            for r in ROWS {
                let writing = WRITING_CODES.contains(&r.code);
                for id in r.lo..=r.hi {
                    if nation != 0 && !NATION_BUILTINS.contains(&id) {
                        continue;
                    }
                    let before = (checksum(sim.state), sim.state.events.len());
                    let pairs = sight == Sight::Omniscient;
                    for args in builtin_args(id, r.pops, &entries, pairs) {
                        let mut stack = vec![FLOOR];
                        stack.extend_from_slice(&args);
                        let mut t = thread(stack, &entries);
                        let e = expect(id, &args, &t, nation);
                        let mut w = SimKiWorld::new(&mut sim, sight);
                        w.builtin(id, &mut BuiltinCx::new(ME, &mut t));
                        let case = || format!("{id:#x} {args:?} {sight:?} nation {nation}");
                        assert_eq!(t.stack.first(), Some(&FLOOR), "{}: pops too much", case());
                        assert_eq!(t.stack.len(), 1 + e.pushes, "{}: stack height", case());
                        assert_eq!(t.pause_ms, r.pause_ms, "{}: pause", case());
                        let notes: Vec<&'static str> = w
                            .notes
                            .iter()
                            .map(|n| match n {
                                KiNote::Guarded { builtin, what, .. } if *builtin == id => *what,
                                other => panic!("{id:#x}: unexpected note {other:?}"),
                            })
                            .collect();
                        assert_eq!(notes, Vec::from_iter(e.guarded), "{}: notes", case());
                        if writing {
                            let slots = sim.state.tables.iter_mut().flat_map(|t| &mut t.objects);
                            for (o, &m) in slots.zip(&marks) {
                                if let (Some(o), Some(m)) = (o, m) {
                                    o.ai_marks = m;
                                }
                            }
                            sim.state.attack_reports = reports.clone();
                        }
                    }
                    assert_eq!(
                        (checksum(sim.state), sim.state.events.len()),
                        before,
                        "{id:#x} {sight:?}: state changed"
                    );
                }
            }
        }
    }
}

// --- Commands: hostile arguments ---------------------------------------------------------------------

/// A test world of the command hostile test: rules, initial state with [`ME`] as AI slot and the object IDs of the
/// scene (own, foreign, dead, construction site, building, container) plus a stale one.
struct CommandScene {
    name: &'static str,
    rules: Rules,
    base: SimState,
    objects: Vec<i32>,
    /// [`NEAR`] and the type GUIDs of the objects (for `(building, guid)` commands).
    values: Vec<i32>,
}

/// Slot 0 a human, [`ME`] an AI; the object IDs of the state and a stale one.
fn finish_scene(name: &'static str, rules: Rules, mut base: SimState) -> CommandScene {
    base.events.clear();
    base.players[0].control = Control::Human;
    base.players[usize::from(ME)].control = Control::Ai;
    let mut objects: Vec<i32> = base.objects().map(|o| o.id.as_i32()).collect();
    let first = ObjectId::from_i32(objects[0]);
    objects.push(ObjectId::new(first.index() as u16, first.slot(), 1000).as_i32());
    let mut values = NEAR.to_vec();
    let guids: BTreeSet<i32> = base.objects().map(|o| o.guid as i32).collect();
    values.extend(guids);
    CommandScene {
        name,
        rules,
        base,
        objects,
        values,
    }
}

/// Economy world of the command tests: colonists (alive, dead, foreign), main house, store, construction site, finca,
/// gold mine, fishing boat (container), tree, fishing basket, building and construction site of slot 0.
fn economy_scene() -> CommandScene {
    let rules = ct::rules();
    let mut base = ct::state(&rules);
    let mut sim = Sim::new(&mut base, &rules);
    ct::spawn(&mut sim, COLONIST, ME_I, (10, 10));
    ct::spawn(&mut sim, COLONIST, ME_I, (11, 10));
    let dead = ct::spawn(&mut sim, COLONIST, ME_I, (12, 10));
    sim.object_mut(dead).unwrap().status |= status::DIED;
    ct::spawn(&mut sim, COLONIST, 0, (13, 10));
    ct::spawn(&mut sim, ct::HQ, ME_I, (20, 20));
    ct::spawn(&mut sim, ct::STORE, ME_I, (24, 20));
    site(&mut sim, HOUSE, ME_I, (30, 30));
    ct::spawn(&mut sim, ct::FARM, ME_I, (40, 10));
    ct::spawn(&mut sim, ct::MINE, ME_I, (40, 30));
    ct::spawn(&mut sim, ct::BOAT, ME_I, (50, 50));
    ct::spawn(&mut sim, ct::TREE, 10, (14, 12));
    ct::spawn(&mut sim, BASKET, ME_I, (13, 13));
    ct::spawn(&mut sim, ct::HQ, 0, (60, 60));
    site(&mut sim, HOUSE, 0, (60, 40));
    ct::account(&mut sim, usize::from(ME), [5000, 5000, 5000, 0, 50]);
    finish_scene("Economy", rules, base)
}

/// Combat world: saber fighter, warrior, archer, a dead warrior, main house and house of [`ME`]; warrior, main house
/// and house of slot 0.
fn combat_scene() -> CommandScene {
    let rules = ck::rules();
    let mut base = ck::state(&rules);
    let mut sim = Sim::new(&mut base, &rules);
    ck::spawn(&mut sim, ck::SABRE, ME_I, (10, 10));
    ck::spawn(&mut sim, WARRIOR, ME_I, (11, 10));
    ck::spawn(&mut sim, ARCHER, ME_I, (11, 12));
    let dead = ck::spawn(&mut sim, WARRIOR, ME_I, (12, 10));
    sim.object_mut(dead).unwrap().status |= status::DIED;
    ck::spawn(&mut sim, WARRIOR, 0, (30, 10));
    ck::spawn(&mut sim, HQ, ME_I, (20, 20));
    ct::spawn(&mut sim, HOUSE, ME_I, (24, 20));
    ck::spawn(&mut sim, HQ, 0, (50, 50));
    ct::spawn(&mut sim, HOUSE, 0, (40, 40));
    finish_scene("Combat", rules, base)
}

/// Spell world ([`kit::spell_rules`]): ability units of [`ME`], targets of slot 0 (alive, dead) and a monastery.
fn ability_scene() -> CommandScene {
    let rules = kit::spell_rules();
    let mut base = kit::state(&rules);
    let mut sim = Sim::new(&mut base, &rules);
    for (i, guid) in [
        kit::TRAPPER,
        kit::SHARPSHOOTER,
        kit::CHIEF,
        kit::MEDICINE_MAN,
        kit::PATRIOT,
        kit::SHADOW_WARRIOR,
        kit::SMITH,
    ]
    .into_iter()
    .enumerate()
    {
        kit::spawn(&mut sim, guid, ME_I, (10, 10 + 4 * i as i32));
    }
    kit::spawn(&mut sim, kit::COLONIST, 0, (12, 12));
    let dead = kit::spawn(&mut sim, kit::COLONIST, 0, (13, 22));
    sim.object_mut(dead).unwrap().status |= status::DIED;
    kit::spawn(&mut sim, kit::MONASTERY, 0, (20, 26));
    finish_scene("Abilities", rules, base)
}

/// Expectation of a list command (re/ki-queries 7.1 pattern D/E): pointer 0 or `n ∉ (0, 100)` -> no command and no
/// note; a pointer without a list -> `Guarded`, no command.
fn list_command_guarded(args: &[i32], t: &Thread) -> bool {
    let (pointer, n) = (t.vars.get(args[0] as u32), args[2]);
    pointer != 0 && (1..100).contains(&n) && t.vars.list_at(pointer).is_none()
}

/// Prints the case when the command panics (the panic itself does not name it).
struct CaseOnPanic<'a>(&'a str, u16, &'a [i32], Sight);

impl Drop for CaseOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Panic in world {}: {:#x} {:?} {:?}",
                self.0, self.1, self.2, self.3
            );
        }
    }
}

/// Load check of the state (`save::check_state`; against the rules `SaveGame::check` checks only the start package,
/// network flags and map size, which no command changes): `Err` with the reason.
fn load_check(state: &SimState) -> Result<(), String> {
    crate::save::check_state(state).map_err(|e| e.to_string())
}

/// Result of a call in the hostile command test.
struct CommandRun {
    /// At least one `KiCommand` was not rejected: the state may have changed.
    dirty: bool,
    /// `KiCommand` events per result: accepted, partial, rejected.
    outcomes: [u64; 3],
    /// Events that are not `KiCommand`.
    other_events: usize,
}

/// One call of the command builtin `r` (ID `id`) with `args` on `state`; checks everything except the state: stack
/// height, pause, notes, events.
fn run_command(
    scene: &CommandScene,
    state: &mut SimState,
    (me, sight): (u8, Sight),
    id: u16,
    args: &[i32],
) -> CommandRun {
    let r = command::row(id).expect("command row");
    let mut sim = Sim::new(state, &scene.rules);
    let mut stack = vec![FLOOR];
    stack.extend_from_slice(args);
    let mut t = thread_of(me, stack, &scene.objects);
    let guarded = is_list_command(id) && list_command_guarded(args, &t);
    let mut w = SimKiWorld::new(&mut sim, sight);
    w.builtin(id, &mut BuiltinCx::new(me, &mut t));
    let notes = std::mem::take(&mut w.notes);
    let case = || format!("{} {id:#x} {args:?} slot {me} {sight:?}", scene.name);
    assert_eq!(t.stack.first(), Some(&FLOOR), "{}: pops too much", case());
    assert_eq!(
        t.stack.len(),
        1 + usize::from(r.push),
        "{}: stack height",
        case()
    );
    assert_eq!(t.pause_ms, r.pause_ms, "{}: pause", case());
    match (&notes[..], guarded) {
        ([], false) => {}
        ([KiNote::Guarded { builtin, .. }], true) if *builtin == id => {}
        _ => panic!("{}: notes {notes:?} (expected {guarded})", case()),
    }
    let mut run = CommandRun {
        dirty: false,
        outcomes: [0; 3],
        other_events: 0,
    };
    for e in sim.state.events.drain(..) {
        if let SimEvent::KiCommand {
            player,
            code,
            outcome,
        } = e
        {
            let kind = match outcome {
                Outcome::Accepted => 0,
                Outcome::Partial(_) => 1,
                Outcome::Rejected(_) => 2,
            };
            run.outcomes[kind] += 1;
            run.dirty |= kind != 2;
            assert_eq!(player, me, "{}: slot", case());
            assert!(
                (1..=0x7E).contains(&code),
                "{}: network command {code:#x}",
                case()
            );
        } else {
            run.other_events += 1;
        }
    }
    run
}

/// After how many calls with an effect the state is reset to the initial world (a clone costs 3 ms in the debug build).
const DIRTY_BATCH: usize = 16;
/// After how many calls with an effect the load check runs (1.4 ms in the debug build; after each single one it would
/// be 21 s alone at about 15,000 calls with an effect).
const CHECK_EVERY: usize = 4;
/// Every how many calls without an effect the full checksum additionally proves that the state stayed the same (the
/// cheap marks are checked by [`cheap_mark`] on every call).
const UNCHANGED_SAMPLE: u64 = 64;

/// Cheap marks of the state for "a rejection changes nothing": object count and target/owner lists per table, resources
/// of all slots, counters of the queues. The full checksum remains a sample.
fn cheap_mark(state: &SimState) -> (Vec<usize>, Vec<[i32; 5]>, [usize; 4]) {
    (
        state.tables.iter().map(|t| t.count).collect(),
        state.players.iter().map(|p| p.resources_fx).collect(),
        [
            state.destroy_queue.len(),
            state.owner_changes.len(),
            state.extra_tick.len(),
            state.tick_groups.iter().map(Vec::len).sum(),
        ],
    )
}

/// Runs of the hostile test: (slot of the instance, sight, with position pairs of the extreme values, number of
/// sequences of the combined series [`combined_args`], fixed random selection). The slot of the scene ([`ME`])
/// omniscient with everything; in fog, the human slot 0 (the objects of [`ME`] are then foreign) and the empty slot 7
/// without objects only with a selection of the combined series.
const VARIANTS: [(u8, Sight, bool, usize); 4] = [
    (ME, Sight::Omniscient, true, usize::MAX),
    (ME, Sight::Fog, false, COMBINED_OTHER),
    (0, Sight::Omniscient, false, COMBINED_OTHER),
    (7, Sight::Omniscient, false, COMBINED_OTHER),
];
/// Sequences of the combined series per command in the secondary runs.
const COMBINED_OTHER: usize = 24;

/// Values of the remaining positions in the combined series: [`HOSTILE`] and the valid or near-limit values 2-7 (mode
/// 7, resource 5, border 1000 ...).
const NEAR: [i32; 16] = [
    0,
    -1,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    9,
    31,
    1000,
    i32::MIN,
    i32::MAX,
    0x7FFF_FFFF >> 12,
];

/// Rotations of the remaining positions per (position, object) in the combined series; one uniform run is added to
/// each.
const ROTATIONS: usize = 2;
/// Maximum number of object-pair sequences per command and test world (random, fixed selection).
const PAIR_CAP: usize = 48;

/// Fixed random selection of at most `cap` sequences (shuffling with a congruential generator, `seed` fixes the
/// selection), sorted afterwards.
fn sample(mut sets: Vec<Vec<i32>>, cap: usize, seed: u32) -> Vec<Vec<i32>> {
    let mut x = seed;
    for i in (1..sets.len()).rev() {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        sets.swap(i, (x >> 8) as usize % (i + 1));
    }
    sets.truncate(cap);
    sets.sort();
    sets
}

/// Second argument series: one object of the scene at every position (own, foreign, dead, stale, building, construction
/// site, container ...), the remaining positions in turn with `values` ([`NEAR`] and the type GUIDs of the scene;
/// [`ROTATIONS`] offset rotations) and uniform; plus two object positions at once (attacker + target, unit + building)
/// with rest 0 and offset rest, all ordered position and object pairs, of which a fixed random selection of at most
/// [`PAIR_CAP`] sequences. List commands keep list, start and `n` valid (two presets); their remaining positions vary.
fn combined_args(id: u16, n: usize, objects: &[i32], values: &[i32]) -> Vec<Vec<i32>> {
    let prefixes: Vec<Vec<i32>> = if is_list_command(id) {
        vec![vec![list(0) as i32, 0, 3], vec![list(0) as i32, 5, 11]]
    } else {
        vec![Vec::new()]
    };
    let (mut singles, mut pairs) = (Vec::new(), Vec::new());
    for prefix in &prefixes {
        let first = prefix.len();
        if n <= first {
            continue;
        }
        let tail = n - first;
        let make = |value: &dyn Fn(usize) -> i32| -> Vec<i32> {
            let mut a = prefix.clone();
            a.extend((0..tail).map(value));
            a
        };
        for i in 0..tail {
            for (oi, &o) in objects.iter().enumerate() {
                for s in 0..ROTATIONS {
                    let r = 3 * oi + i + 5 * s;
                    let mut a = make(&|k| values[(r + k) % values.len()]);
                    a[first + i] = o;
                    singles.push(a);
                }
                let mut b = make(&|_| values[(oi + i) % values.len()]);
                b[first + i] = o;
                singles.push(b);
            }
        }
        for i in 0..tail {
            for j in 0..tail {
                if i == j {
                    continue;
                }
                for (oi, &o) in objects.iter().enumerate() {
                    for (oj, &o2) in objects.iter().enumerate() {
                        let rest = if (oi + oj) % 2 == 0 { 0 } else { 1 };
                        let mut a = make(&|k| values[(oi + oj + k) % values.len()] * rest);
                        (a[first + i], a[first + j]) = (o, o2);
                        pairs.push(a);
                    }
                }
            }
        }
    }
    let mut out = singles;
    out.extend(sample(pairs, PAIR_CAP, u32::from(id)));
    out.sort();
    out.dedup();
    out
}

#[test]
fn hostile_command_arguments_never_panic() {
    // Each of the 98 command IDs with the values from re/ki-queries 6.6 at every position, with the object IDs of the
    // scene (own, foreign, dead, construction site, building, container, stale), the filter IDs -5/999 and the list
    // addresses of the list commands, omniscient (with all position pairs of the extreme values) and in fog, in three
    // test worlds. Plus the combined series [`combined_args`]: valid objects with hostile and near-limit values, also
    // two objects at once. Per call: no panic, stack height as in the row (push yes/no), pause as in the row, notes
    // only `Guarded` for a list pointer without a list, every `KiCommand` with the slot of the instance, network
    // command `0x01…0x7E` and a result (commands with an empty unit list stay accepted).
    //
    // Run time: a fresh state per call (3 ms clone) would be too slow. Calls whose `KiCommand`s are all rejected (or
    // that give none) leave the state as it was: the cheap marks [`cheap_mark`] on every call, the full checksum on
    // every 64th. Calls with an effect run on top of each other; after each the load check (`save::check_state`)
    // passes, after every `DIRTY_BATCH` the initial world is restored. If the load check fails, the calls since the
    // reset are repeated one after the other on a clone of the initial world and the first case is named (if the
    // repetition does not fail, all arguments of the group are in the message).
    let scenes = [economy_scene(), combat_scene(), ability_scene()];
    let mut covered = BTreeSet::new();
    let mut effective = BTreeSet::new();
    let (mut calls, mut dirty_calls, mut outcomes) = (0u64, 0u64, [0u64; 3]);
    for scene in &scenes {
        let base_sum = checksum(&scene.base);
        load_check(&scene.base).expect("initial world loads");
        let mut state = scene.base.clone();
        for (me, sight, pairs, combined) in VARIANTS {
            for r in command::ROWS {
                for id in r.lo..=r.hi {
                    covered.insert(id);
                    // Calls with an effect since the last reset.
                    let mut batch: Vec<Vec<i32>> = Vec::new();
                    let mut quiet = 0u64;
                    let load_ok = |state: &SimState, batch: &[Vec<i32>]| {
                        if load_check(state).is_ok() {
                            return;
                        }
                        let mut again = scene.base.clone();
                        for args in batch {
                            run_command(scene, &mut again, (me, sight), id, args);
                            if let Err(why) = load_check(&again) {
                                panic!(
                                    "{} {id:#x} {args:?} slot {me} {sight:?}: load check: {why}",
                                    scene.name
                                );
                            }
                        }
                        panic!(
                            "{} {id:#x} slot {me} {sight:?}: load check only in the group, \
                             calls: {batch:?}",
                            scene.name
                        );
                    };
                    let mut sets = builtin_args(id, r.pops, &scene.objects, pairs);
                    let extra =
                        combined_args(id, usize::from(r.pops), &scene.objects, &scene.values);
                    sets.extend(if combined == usize::MAX {
                        extra
                    } else {
                        sample(extra, combined, u32::from(id) ^ u32::from(me))
                    });
                    for args in sets {
                        let mark = cheap_mark(&state);
                        let run = {
                            let _case = CaseOnPanic(scene.name, id, &args, sight);
                            run_command(scene, &mut state, (me, sight), id, &args)
                        };
                        calls += 1;
                        for (n, k) in outcomes.iter_mut().zip(run.outcomes) {
                            *n += k;
                        }
                        if run.dirty {
                            effective.insert(id);
                            dirty_calls += 1;
                            batch.push(args);
                            if batch.len().is_multiple_of(CHECK_EVERY) {
                                load_ok(&state, &batch);
                            }
                            if batch.len() >= DIRTY_BATCH {
                                state.clone_from(&scene.base);
                                batch.clear();
                            }
                        } else {
                            let case =
                                || format!("{} {id:#x} {args:?} slot {me} {sight:?}", scene.name);
                            assert_eq!(run.other_events, 0, "{}: foreign events", case());
                            assert_eq!(cheap_mark(&state), mark, "{}: state changed", case());
                            quiet += 1;
                            if batch.is_empty() && quiet.is_multiple_of(UNCHANGED_SAMPLE) {
                                assert_eq!(
                                    checksum(&state),
                                    base_sum,
                                    "{}: state changed without effect (checksum)",
                                    case()
                                );
                            }
                        }
                    }
                    if !batch.is_empty() {
                        if !batch.len().is_multiple_of(CHECK_EVERY) {
                            load_ok(&state, &batch);
                        }
                        state.clone_from(&scene.base);
                    }
                }
            }
        }
    }
    let all: BTreeSet<u16> = (0..=u16::MAX).filter(|&id| is_command(id)).collect();
    assert_eq!(covered, all);
    assert_eq!(all.len(), 98);
    let idle: Vec<u16> = all.difference(&effective).copied().collect();
    eprintln!("Command IDs without an accepted command in all runs: {idle:x?}");
    eprintln!(
        "{calls} calls, of which {dirty_calls} with effect; AI commands accepted {}, partial {}, rejected {}",
        outcomes[0], outcomes[1], outcomes[2]
    );
}

/// Query codes with their own check: all of the table and those of the own flows.
fn known_codes() -> BTreeSet<u32> {
    let mut codes: BTreeSet<u32> = ROWS.iter().map(|r| r.code).filter(|&c| c != 0).collect();
    codes.extend([0x4E39, 0x4FF6, 0x4E28, 0x4E2C, 0x4FB5, 0x4FA6]);
    codes
}

#[test]
fn hostile_query_words_never_panic_or_write() {
    // `query_once` (without `apply`): every code 0x4E20...0x5130 with 0, 1, 2 and 128 words, the known codes
    // additionally with argc 1...9 (the argc of their handlers also with all position pairs of the extreme values,
    // omniscient for `me`); no panic, checksum and events the same per code, direct changes only from the writing
    // codes.
    let (rules, mut state, entries) = hostile_scene();
    let mut sim = Sim::new(&mut state, &rules);
    let known = known_codes();
    for code in 0x4E20..=0x5130u32 {
        let before = (checksum(sim.state), sim.state.events.len());
        let mut lens: BTreeSet<usize> = BTreeSet::from([0, 1, 2, 128]);
        let mut row_lens = BTreeSet::new();
        if known.contains(&code) {
            for r in ROWS.iter().filter(|r| r.code == code) {
                if let Run::Ask { argc, .. } = r.run {
                    row_lens.insert(usize::try_from(argc).unwrap());
                }
            }
            lens.extend(1..=9);
        }
        let ask = |sim: &mut Sim, me: u8, sight: Sight, words: &[i32]| {
            let (req, direct) = query_once(sim, me, sight, code, words);
            assert!(req.len() >= 2 + words.len(), "{code:#x}");
            assert!(
                direct.is_empty() || WRITING_CODES.contains(&code),
                "{code:#x} {words:?}: {direct:?}"
            );
        };
        for len in lens {
            let sets = if len > 9 || !known.contains(&code) {
                HOSTILE.iter().map(|&v| vec![v; len]).collect()
            } else {
                hostile_args(len, code, &entries, false)
            };
            for words in sets {
                for me in [0, ME, 7] {
                    for sight in [Sight::Omniscient, Sight::Fog] {
                        ask(&mut sim, me, sight, &words);
                    }
                }
            }
        }
        for len in row_lens {
            for words in hostile_args(len, code, &entries, true) {
                ask(&mut sim, ME, Sight::Omniscient, &words);
            }
        }
        assert_eq!(
            (checksum(sim.state), sim.state.events.len()),
            before,
            "{code:#x}: state changed"
        );
    }
}
