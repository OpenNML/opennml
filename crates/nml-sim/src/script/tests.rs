//! Individual tests of the script run time with synthetic scripts (without a corpus).

use nml_formats::abl::{RtCondition, ScriptFile};

use super::conditions::{condition, query_1000, query_1300};
use super::*;
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::ki::KiSetup;
use crate::object::status;
use crate::rng::CrtRand;
use crate::rules::Rules;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::{StepOptions, step, step_with};
use crate::world::{ObjectDesc, create_object_named, destroy_object};

const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\n";
const SEED: u32 = 7;
/// Map side of the test rules in tiles.
const DIM: i32 = 16;

/// Script text from blocks `(conditions, actions)` in the `.abl` format (head before the `:`).
pub(super) fn script(blocks: &[(&[&str], &[&str])]) -> String {
    let mut text = String::new();
    for (i, (conditions, actions)) in blocks.iter().enumerate() {
        text += &format!("{{\r\n\t[BESCHREIBUNG=block{i}]\r\n");
        for c in *conditions {
            text += &format!("\t[BEDINGUNG={c}:Text]\r\n");
        }
        for a in *actions {
            text += &format!("\t[AKTION={a}:Text]\r\n");
        }
        text += "}\r\n";
    }
    text
}

pub(super) fn rules_with_areas(level: &str, areas: &[(&str, [i32; 4])]) -> Rules {
    let mut rules = Rules::synthetic(TYPES, DIM as u32);
    rules.scripts = testing::rules_with(level, &[], areas);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    rules
}

pub(super) fn rules_for(level: &str) -> Rules {
    rules_with_areas(level, &[])
}

pub(super) fn setup(enabled: bool) -> MatchSetup {
    MatchSetup {
        mode: -1,
        seed: SEED,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: KiSetup::default(),
        script: ScriptSetup {
            enabled,
            time_per_char: 55,
            ..Default::default()
        },
        ..MatchSetup::default()
    }
}

fn start(rules: &Rules) -> SimState {
    new_match(rules, &setup(true)).expect("match with script")
}

fn level(state: &SimState) -> &Container {
    state.script.level.as_ref().expect("level container")
}

fn level_mut(state: &mut SimState) -> &mut Container {
    state.script.level.as_mut().expect("level container")
}

/// `n` steps; returns the script messages per step.
fn run(state: &mut SimState, rules: &Rules, n: usize) -> Vec<Vec<ScriptTrace>> {
    (0..n).map(|_| traces(step(state, rules, &[]))).collect()
}

fn traces(events: Vec<SimEvent>) -> Vec<ScriptTrace> {
    events
        .into_iter()
        .filter_map(|e| match e {
            SimEvent::Script(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn fired(traces: &[ScriptTrace], block: u32) -> bool {
    traces.iter().any(|t| {
        *t == ScriptTrace::Fired {
            container: LEVEL_CONTAINER_ID,
            block,
        }
    })
}

// QUIRK E-185: kept
#[test]
fn level_container_starts_at_id_one() {
    // E-185: the game engine constructor `FUN_0043BB70` sets `m_LastTriggerEngineID` to 1.
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    let state = start(&rules);
    assert_eq!(level(&state).id, LEVEL_CONTAINER_ID);
    assert_eq!(level(&state).id, 1);
    assert_eq!(state.script.next_container_id, LEVEL_CONTAINER_ID + 1);
    assert_eq!(state.script.next_container_id, 2);
    // Without a script the default state stays (never read).
    let off = new_match(&rules, &setup(false)).unwrap();
    assert_eq!(off.script, ScriptState::default());
    assert_eq!(off.script.next_container_id, 0);
}

// QUIRK E-185: kept
#[test]
fn start_creates_the_level_container() {
    let rules = rules_for(&script(&[(&[], &["n12;a"]), (&[], &["n12;b"])]));
    let state = start(&rules);
    let c = level(&state);
    assert_eq!(c.id, LEVEL_CONTAINER_ID);
    assert_eq!(c.counter, -1);
    assert_eq!(c.source.path, testing::LEVEL_PATH);
    assert_eq!(
        c.source.checksum,
        rules.scripts.level.as_ref().unwrap().checksum
    );
    assert_eq!(c.blocks, [BlockState::new(0), BlockState::new(1)]);
    assert_eq!(
        c.blocks[0],
        BlockState {
            index: 0,
            delete_me: false,
            deleteable: true,
            running: false,
            cur_action: 0,
            timeout: -1,
        }
    );
    assert_eq!(state.script.next_container_id, LEVEL_CONTAINER_ID + 1);
    assert_eq!(state.script.rand, CrtRand::new(SEED));
    assert_eq!(state.script.time_per_char, 55);
    assert!(state.script.dialog.is_none());
    assert!(!dialog_running(&state));
}

// QUIRK E-129: kept
#[test]
fn timer_counts_from_next_tick_countdown_from_set_tick() {
    // E-129: timers are decreased before the blocks (from the following tick), the countdown after them
    // (already in the set tick); both stay at 0, negative timers stay.
    let rules = rules_for(&script(&[
        (&[], &["n25;t;100", "n25;neg;-5"]),
        (&[], &["n11;1"]),
        // Behind `n11` and therefore before it in the tick: does not see the countdown until the following tick.
        (&["&4;0"], &["n361;hits;1", "n10"]),
    ]));
    let mut state = start(&rules);
    let timer = |s: &SimState| level(s).timers["t"];
    run(&mut state, &rules, 1);
    assert_eq!(timer(&state), 100);
    assert_eq!(level(&state).timers["neg"], -5);
    assert_eq!(level(&state).counter, 950);
    run(&mut state, &rules, 1);
    assert_eq!(timer(&state), 50);
    run(&mut state, &rules, 1);
    assert_eq!(timer(&state), 0);
    run(&mut state, &rules, 1);
    assert_eq!(timer(&state), 0);
    assert_eq!(level(&state).timers["neg"], -5);
    // Countdown 1000 - 20 * 50 = 0 after tick 20; condition 4 is not true until after that and stays so.
    run(&mut state, &rules, 16);
    assert_eq!(level(&state).counter, 0);
    assert_eq!(level(&state).variables.get("hits"), None);
    run(&mut state, &rules, 3);
    assert_eq!(level(&state).counter, 0);
    assert_eq!(level(&state).variables["hits"], 3);
}

#[test]
fn condition_4_is_false_without_countdown() {
    let rules = rules_for(&script(&[(&["&4;100"], &["n12;a"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 3);
    assert!(level(&state).switches.is_empty());
}

// QUIRK E-124: kept
#[test]
fn first_prefix_is_ignored_no_short_circuit() {
    // E-124: the prefix of the first condition has no effect, afterwards strictly left to right without
    // precedence and without short-circuit.
    let rules = rules_for(&script(&[
        (&["|5;x"], &["n12;a"]),
        (&["&18;x"], &["n12;b"]),
        (&["&18;x", "|5;x", "&5;x"], &["n12;c"]),
        (&["&5;x", "&19;5"], &["n12;d"]),
        (&["&5;x", "|1"], &["n12;e"]),
    ]));
    let mut state = start(&rules);
    state.script.mission.speech.events = vec![5];
    run(&mut state, &rules, 1);
    let switches: Vec<&str> = level(&state).switches.keys().map(String::as_str).collect();
    assert_eq!(switches, ["b", "e"]);
    // Block 3: the AND chain is already false after `5`, condition 19 runs anyway and consumes the text
    // event (observer for "no short-circuit").
    assert!(speech(&state).events.is_empty());
}

// QUIRK E-126: kept
#[test]
fn block_without_effect_stays() {
    // E-126: numbers without a case (41) and 510 without variables do not set `m_bDeleteMe`.
    let rules = rules_for(&script(&[(&[], &["n41"]), (&[], &["n510;a;b"])]));
    let mut state = start(&rules);
    let steps = run(&mut state, &rules, 3);
    assert_eq!(level(&state).blocks.len(), 2);
    for t in &steps {
        assert!(fired(t, 0) && fired(t, 1), "{t:?}");
    }
    assert!(level(&state).blocks.iter().all(|b| !b.delete_me));
}

#[test]
fn executed_blocks_are_deleted() {
    let rules = rules_for(&script(&[(&[], &["n12;a"]), (&["&5;nie"], &["n12;b"])]));
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    assert!(first.contains(&ScriptTrace::Deleted {
        container: LEVEL_CONTAINER_ID,
        block: 0
    }));
    let indices: Vec<u32> = level(&state).blocks.iter().map(|b| b.index).collect();
    assert_eq!(indices, [1]);
}

/// First step (1-based) after which the switch `done` is set, at most `limit`.
fn step_of_done(rules: &Rules, limit: usize) -> Option<usize> {
    let mut state = start(rules);
    (1..=limit).find(|_| {
        run(&mut state, rules, 1);
        level(&state).switches.contains_key("done")
    })
}

// QUIRK E-130: kept
#[test]
fn wait_40_ends_after_20_ticks_and_continues_same_tick() {
    // E-130: `max(1, ceil(ms/50))` ticks, following action in the same tick.
    let wait = |ms: &str| rules_for(&script(&[(&[], &[&format!("w40;{ms}"), "n12;done"])]));
    assert_eq!(step_of_done(&wait("1000"), 100), Some(21));
    assert_eq!(step_of_done(&wait("0"), 100), Some(2));
    assert_eq!(step_of_done(&wait("1"), 100), Some(2));
    assert_eq!(step_of_done(&wait("51"), 100), Some(3));
    assert_eq!(step_of_done(&wait("-5"), 100), Some(2));
    assert_eq!(step_of_done(&wait("-1"), 100), None);
    // Without `w` no waiting: following action in the first step.
    let plain = rules_for(&script(&[(&[], &["n40;1000", "n12;done"])]));
    assert_eq!(step_of_done(&plain, 100), Some(1));
}

// QUIRK E-130: kept
#[test]
fn wait_minus_one_reruns_the_wait_action_every_tick() {
    // `w40;-1` is event waiting without an event: action 40 reruns every tick (the block fires every
    // tick), the action before it only in the first run (the block continues at the wait index).
    let rules = rules_for(&script(&[(&[], &["n361;v;1", "w40;-1", "n12;done"])]));
    let mut state = start(&rules);
    let steps = run(&mut state, &rules, 30);
    assert!(steps.iter().all(|t| fired(t, 0)));
    let c = level(&state);
    assert_eq!(c.variables["v"], 1);
    assert!(!c.switches.contains_key("done"));
    assert_eq!(
        c.blocks[0],
        BlockState {
            index: 0,
            delete_me: true,
            deleteable: true,
            running: true,
            cur_action: 1,
            timeout: -1,
        }
    );
}

// QUIRK E-125: kept
#[test]
fn repeatable_block_restarts_after_the_wait() {
    // E-125: the action index stays after the wait, `n12` runs only once, `n25` every time.
    let rules = rules_for(&script(&[(&[], &["n12;a", "w40;100", "n25;t;7", "n10"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 2);
    assert!(!level(&state).timers.contains_key("t"));
    run(&mut state, &rules, 1);
    assert_eq!(level(&state).timers["t"], 7);
    assert_eq!(level(&state).blocks[0].cur_action, 2);
    for _ in 0..3 {
        let c = level_mut(&mut state);
        c.switches.clear();
        c.timers.insert("t".into(), 1000);
        run(&mut state, &rules, 1);
        let c = level(&state);
        assert!(c.switches.is_empty(), "n12 ran again");
        assert_eq!(c.timers["t"], 7, "n25 did not run");
        assert_eq!(c.blocks.len(), 1);
        assert_eq!(c.blocks[0].cur_action, 2);
    }
}

// QUIRK E-172: kept
#[test]
fn wait_without_w_resets_the_start_index() {
    // E-172: `n40;5` calls `SuspendAtAction` without `w` and sets the start index to 0.
    let rules = rules_for(&script(&[(
        &[],
        &["n12;a", "w40;100", "n25;t;1", "n40;5", "n10"],
    )]));
    let mut state = start(&rules);
    run(&mut state, &rules, 3);
    let c = level(&state);
    assert_eq!(c.timers["t"], 1);
    assert_eq!((c.blocks[0].cur_action, c.blocks[0].running), (0, false));
    level_mut(&mut state).switches.clear();
    run(&mut state, &rules, 1);
    let c = level(&state);
    assert!(
        c.switches.contains_key("a"),
        "n12 runs again in the second run"
    );
    assert_eq!(
        (
            c.blocks[0].cur_action,
            c.blocks[0].running,
            c.blocks[0].timeout
        ),
        (1, true, 100)
    );
}

// QUIRK E-132: kept
#[test]
fn missing_variable_takes_the_operand() {
    // E-132: 361-363 set a missing variable to the operand.
    let rules = rules_for(&script(&[(
        &[],
        &[
            "n361;p;5",
            "n362;m;5",
            "n363;x;5",
            "n360;q;10",
            "n362;q;3",
            "n360;r;4",
            "n363;r;-2",
            "n361;r;1",
        ],
    )]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    let v = &level(&state).variables;
    assert_eq!((v["p"], v["m"], v["x"]), (5, 5, 5));
    assert_eq!((v["q"], v["r"]), (7, -7));
}

// QUIRK E-128: kept
#[test]
fn variable_actions_359_and_510_to_512() {
    let rules = rules_for(&script(&[
        (&[], &["n359;0;anzahl;1100", "n359;9;fremd;1100"]),
        (
            &[],
            &[
                "n360;a;6",
                "n360;b;4",
                "n510;a;b",
                "n360;c;6",
                "n511;c;b",
                "n360;d;6",
                "n512;d;b",
                "n510;a;fehlt",
            ],
        ),
    ]));
    let mut state = new_match(&rules, &setup(true)).unwrap();
    state.add_guid_count(0, 1100, 3);
    run(&mut state, &rules, 1);
    let c = level(&state);
    let v = &c.variables;
    assert_eq!((v["a"], v["c"], v["d"]), (10, 2, 24));
    assert_eq!((v["anzahl"], v["fremd"]), (3, 0));
    // 510-512 do not set `m_bDeleteMe`, but 360 before it does: the second block is deleted, the first
    // (359) too.
    assert!(c.blocks.is_empty());
}

// QUIRK E-144: deviation
#[test]
fn random_switch_and_timer_formulas() {
    // 33: `(rand * Z1) / 32767`; 180: `(rand * (Z2 - Z1)) / 32767 + Z1`, one CRT `rand()` each from the
    // script randomness (seed of the match).
    let rules = rules_for(&script(&[(
        &[],
        &["n33;r;100", "n180;t;10;20", "n33;gross;2000000"],
    )]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    let mut crt = CrtRand::new(SEED);
    let (r1, r2, r3) = (crt.rand(), crt.rand(), crt.rand());
    let c = level(&state);
    assert_eq!(c.random_switches["r"], r1 * 100 / 32767);
    assert_eq!(c.timers["t"], r2 * 10 / 32767 + 10);
    assert_eq!(
        c.random_switches["gross"],
        r3.wrapping_mul(2_000_000) / 32767
    );
    assert_eq!(state.script.rand, crt);
}

#[test]
fn timer_from_timer_233() {
    // `trunc(f32(pct * f32(t * 0.01)))`; missing source timer -> 0.
    let rules = rules_for(&script(&[(
        &[],
        &[
            "n25;a;58",
            "n233;b;50;a",
            "n25;c;106",
            "n233;d;50;c",
            "n233;e;50;fehlt",
        ],
    )]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    let t = &level(&state).timers;
    assert_eq!((t["b"], t["d"], t["e"]), (29, 52, 0));
    assert_eq!(actions::percent_time(50, 1000), 500);
    assert_eq!(actions::percent_time(i32::MAX, 1000), i32::MIN);
}

/// Number without a case in layer D (the catalog does not know it): the safety net "no effect yet"
/// stays in the code, even if no catalog number falls there any more.
const NUMBER_WITHOUT_CASE: i32 = 9999;

#[test]
fn pending_actions_are_traced_and_deleted() {
    // Every catalog number has an effect (`no_number_is_pending`); a number without a case no longer
    // reaches layer D through any script (layer E skips it). The safety net "no effect yet" is
    // therefore checked via the entry `actions::send` directly: trace `Pending` and `m_bDeleteMe`.
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    let mut state = start(&rules);
    let mut c = state.script.level.take().expect("level container");
    {
        let mut sim = Sim::new(&mut state, &rules);
        actions::send(&mut sim, &mut c, 0, &action(NUMBER_WITHOUT_CASE, &[]), 0).expect("no guard");
    }
    assert!(c.blocks[0].delete_me);
    assert!(
        traces(state.events.clone()).contains(&ScriptTrace::Pending {
            container: LEVEL_CONTAINER_ID,
            block: 0,
            action: NUMBER_WITHOUT_CASE,
        })
    );
}

/// Condition with number and parameters (AND).
pub(super) fn cond(number: i32, params: &[&str]) -> RtCondition {
    RtCondition {
        or: false,
        number,
        params: params.iter().map(|p| p.to_string()).collect(),
        line: 1,
    }
}

pub(super) fn spawn(sim: &mut Sim, name: Option<&str>, tile: (i32, i32)) -> ObjectId {
    let pos = [
        tile.0 as f32 * 128.0 + 64.0,
        0.0,
        tile.1 as f32 * 128.0 + 64.0,
    ];
    let id = create_object_named(sim, &mut ObjectDesc::new(1100, 0, pos), name).expect("object");
    let o = sim.object_mut(id).unwrap();
    o.grid.x = Fx(tile.0 << 12);
    o.grid.z = Fx(tile.1 << 12);
    id
}

// QUIRK E-135: kept
#[test]
fn conditions_23_24_27() {
    let rules = rules_with_areas(
        "",
        &[
            ("feld", [2, 2, 5, 5]),
            ("rand", [0, 0, 0, 15]),
            ("ende", [15, 0, 15, 15]),
            ("verdreht", [5, 5, 2, 2]),
        ],
    );
    let mut state = new_match(&rules, &setup(false)).unwrap();
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, Some("lebt"), (3, 3));
    let b = spawn(&mut sim, None, (5, 2));
    let dying = spawn(&mut sim, None, (2, 5));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    let gone = spawn(&mut sim, Some("weg"), (3, 3));
    destroy_object(&mut sim, gone);
    let outside = spawn(&mut sim, None, (6, 3));
    let left = spawn(&mut sim, Some("links"), (0, 4));
    sim.object_mut(left).unwrap().grid.x = Fx(-5 << 12);
    let right = spawn(&mut sim, Some("rechts"), (15, 4));
    sim.object_mut(right).unwrap().grid.x = Fx(40 << 12);
    let list = |ids: &[ObjectId]| {
        ids.iter()
            .map(|id| id.as_i32().to_string())
            .collect::<Vec<_>>()
            .join(";")
    };
    sim.state.names.set("gruppe", list(&[a, b]));
    sim.state.names.set("sterbend", list(&[a, dying]));
    sim.state.names.set("mitweg", list(&[a, gone]));
    sim.state.names.set("teils", list(&[a, outside]));
    sim.state.names.set("nurweg", list(&[gone]));

    // 1000/1100: list not empty, all alive (without `DIED`).
    assert!(query_1000(&sim, "lebt"));
    assert!(query_1000(&sim, "gruppe"));
    assert!(!query_1000(&sim, "sterbend"));
    assert!(!query_1000(&sim, "weg"));
    assert!(!query_1000(&sim, "mitweg"));
    assert!(!query_1000(&sim, "unbekannt"));

    // 1300: unknown area false; removed ones skipped, dying ones count; at least one.
    assert!(query_1300(&sim, "lebt", "feld"));
    assert!(query_1300(&sim, "gruppe", "feld"), "edge inclusive");
    assert!(query_1300(&sim, "sterbend", "feld"));
    assert!(query_1300(&sim, "mitweg", "feld"));
    assert!(!query_1300(&sim, "nurweg", "feld"));
    assert!(!query_1300(&sim, "teils", "feld"));
    assert!(!query_1300(&sim, "unbekannt", "feld"));
    assert!(!query_1300(&sim, "lebt", "kein bereich"));
    assert!(!query_1300(&sim, "lebt", "verdreht"), "no corner swap");
    assert!(query_1300(&sim, "links", "rand"), "x < 0 clamped to tile 0");
    assert!(
        query_1300(&sim, "rechts", "ende"),
        "x >= W clamped to W - 1"
    );

    // Via the conditions: 24 is the negation of 23, 27 shortens the area with `Mid(5)`.
    let file = ScriptFile { blocks: Vec::new() };
    let source = ScriptSource {
        path: String::new(),
        checksum: 0,
    };
    let mut c = Container::new(0, source, &file);
    let mut check = |n: i32, params: &[&str]| condition(&mut sim, &mut c, 0, &cond(n, params));
    assert!(check(23, &["lebt"]));
    assert!(!check(24, &["lebt"]));
    assert!(!check(23, &["sterbend"]));
    assert!(check(24, &["sterbend"]));
    assert!(check(24, &["weg"]));
    assert!(check(24, &["unbekannt"]));
    assert!(check(27, &["sterbend", "area feld"]));
    assert!(!check(27, &["lebt", "area kein bereich"]));
    assert!(!check(27, &["lebt", "feld"]), "Mid(5) cuts blindly");
}

// QUIRK E-128: kept
#[test]
fn script_conditions_read_the_container() {
    let file = ScriptFile { blocks: Vec::new() };
    let rules = rules_for("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    let mut sim = Sim::new(&mut state, &rules);
    let source = ScriptSource {
        path: String::new(),
        checksum: 0,
    };
    let mut c = Container::new(0, source, &file);
    c.timers.insert("null".into(), 0);
    c.timers.insert("neg".into(), -3);
    c.timers.insert("laeuft".into(), 50);
    c.random_switches.insert("w".into(), 4);
    c.variables.insert("v".into(), 10);
    let mut check = |n: i32, params: &[&str]| condition(&mut sim, &mut c, 0, &cond(n, params));
    assert!(check(1, &[]));
    assert!(check(16, &["null"]) && check(16, &["neg"]));
    assert!(!check(16, &["laeuft"]) && !check(16, &["fehlt"]));
    assert!(check(17, &["w", "4"]) && !check(17, &["w", "5"]) && !check(17, &["x", "0"]));
    assert!(check(45, &["v", "10"]) && !check(45, &["fehlt", "0"]));
    assert!(check(46, &["v", "11"]) && !check(46, &["v", "10"]));
    assert!(check(47, &["v", "9"]) && !check(47, &["v", "10"]));
    // Without a case: false without a message. With a case: implemented (every number with a case); 35
    // with an unknown name is true (E-135) and reports nothing.
    assert!(!check(15, &[]) && !check(57, &[]) && !check(-1, &[]));
    assert!(check(35, &["x"]));
    let events = std::mem::take(&mut sim.state.events);
    assert!(traces(events).is_empty());
}

#[test]
fn difficulty_condition_reads_campaign_difficulty() {
    let rules = rules_for(&script(&[
        (&["&22;2"], &["n12;zwei"]),
        (&["&22;1"], &["n12;eins"]),
    ]));
    let mut s = setup(true);
    s.ki.campaign_difficulty = 2;
    let mut state = new_match(&rules, &s).unwrap();
    run(&mut state, &rules, 1);
    let switches: Vec<&str> = level(&state).switches.keys().map(String::as_str).collect();
    assert_eq!(switches, ["zwei"]);
}

#[test]
fn all_conditions_true_is_a_test_switch_outside_the_state() {
    let rules = rules_for(&script(&[(&["&5;nie", "&19;5"], &["n12;a"])]));
    let mut state = start(&rules);
    state.script.mission.speech.events = vec![5];
    let options = StepOptions {
        all_conditions_true: true,
    };
    step_with(&mut state, &rules, &[], options);
    assert!(level(&state).switches.contains_key("a"));
    // The conditions run anyway: 19 consumes the text event.
    assert!(speech(&state).events.is_empty());
}

#[test]
fn script_off_has_no_container() {
    // Without `enabled` exactly the default state stays, the script calls do nothing.
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    let mut state = new_match(&rules, &setup(false)).unwrap();
    assert_eq!(state.script, ScriptState::default());
    let steps = run(&mut state, &rules, 3);
    assert!(steps.iter().all(Vec::is_empty));
    assert_eq!(state.script, ScriptState::default());
    assert_eq!(ScriptState::default().time_per_char, 55);
    assert_eq!(ScriptState::default().rand, CrtRand::new(0));
}

// QUIRK E-120: deviation
#[test]
fn bad_script_fails_new_match_in_german() {
    // Load errors only make the start with a script fail.
    let mut rules = Rules::synthetic(TYPES, DIM as u32);
    rules.scripts = testing::rules_with(&script(&[(&[], &["n25;t"])]), &[], &[]);
    let err = new_match(&rules, &setup(true)).unwrap_err();
    assert_eq!(
        err,
        "Script synthetisch.abl, line 3: action 25 needs at least 2 parameters (has 1)"
    );
    assert!(new_match(&rules, &setup(false)).is_ok());
    // A missing dialog is reported as well.
    let dialogs = testing::rules_with(&script(&[(&[], &["n39;fehlt"])]), &[], &[]);
    assert_eq!(dialogs.errors.len(), 1, "{:?}", dialogs.errors);
}

// ---------------------------------------------------------------------------------------------
// Layer E of all numbers

use nml_formats::abl::RtAction;
use nml_formats::abl::catalog::{ACTIONS, Effect, ParamKind};

use super::actions::{
    Dispatched, MsgValue, ScriptMsg, build_msg, dispatch, percent_len, percent_time,
};
use super::conditions::query_2500;
use crate::world::{object_event_bits, set_object_event_bit};

/// Example parameter of a kind: areas with `area `, everything else `1` (slot, number, name, dialog).
fn sample(kind: ParamKind) -> &'static str {
    match kind {
        ParamKind::Area => "area feld",
        _ => "1",
    }
}

fn action(number: i32, params: &[&str]) -> RtAction {
    RtAction {
        wait: false,
        number,
        params: params.iter().map(|p| p.to_string()).collect(),
        line: 1,
    }
}

/// Value slots of the message per re/script-actions A.8 with `n` parameters; `None` = no message
/// (internal numbers). 0/1 (word count) is checked by `ki_start_counts_words`.
fn a8_values(number: u16, n: usize) -> Option<usize> {
    Some(match number {
        2 | 7 | 20 | 24 | 347 | 372 => 2,
        3 => 4,
        4 | 17 | 9 | 13 => 1,
        5 | 205 | 14 | 21 | 22 | 183 | 27 | 182 | 200 | 28..=32 | 36 | 37 | 203 => n,
        6 | 195 | 34 | 196 | 35 => 3,
        8 | 373 => 4,
        15 | 16 | 18 | 19 => 0,
        38 | 366 => 2,
        39 => 1,
        141 => 2,
        142 => 3,
        143 | 144 | 147 | 148 | 176 => 0,
        145 | 146 | 207..=210 | 216 => 1,
        149 => 3,
        150 => 2,
        151 => 4,
        152 | 191 | 215 => 1,
        153 | 188 | 374 | 376 => 3,
        154 | 390 => 2,
        155 | 171 | 156 => 1,
        157 => 3,
        158 | 161 | 162 | 165 | 166 | 170 | 174 | 175 | 177 | 178 | 193 | 194 => 1,
        159 | 160 | 169 | 172 | 173 => 1,
        163 | 164 | 167 | 181 | 375 => 2,
        168 => 4,
        179 => 2,
        184..=187 | 198 | 199 => 1,
        189 | 192 | 201 | 231 => 2,
        190 | 197 => 3,
        202 => 2,
        204 => 4,
        211 | 214 => 3,
        212 => 2,
        213 => 4,
        217 => 5,
        220 | 240 => 3,
        221 | 241 => 2,
        222 | 223 | 242 | 243 => 1,
        334..=337 | 402 | 403 => 2,
        338 => 1,
        340 => 3,
        341 => 2,
        342 | 343 => 1,
        344..=346 => 3,
        350..=353 => 2,
        354 => 1,
        365 => 2,
        370 | 371 => 1,
        380 => 4,
        381 => 0,
        382 => 2,
        383 => 1,
        384 | 385 => 4,
        391..=394 => 2,
        395 => 1,
        396 => 5,
        397 => 3,
        501..=504 | 508 => 0,
        505 => 3,
        506 => 4,
        507 => 1,
        _ => return None,
    })
}

/// Number of the message to layer D (re/script-actions A.10: 220/240 -> 142, 221/241 -> 212,
/// 222/242 -> 216, 223/243 -> 172).
fn a10_number(number: u16) -> i32 {
    match number {
        220 | 240 => 142,
        221 | 241 => 212,
        222 | 242 => 216,
        223 | 243 => 172,
        n => i32::from(n),
    }
}

/// [`build_msg`] on a fresh match without a script.
fn msg_of(rules: &Rules, a: &RtAction) -> ScriptMsg {
    let mut state = new_match(rules, &setup(false)).unwrap();
    build_msg(&Sim::new(&mut state, rules), a)
}

/// Catalog numbers with a case (not `NoCase`/`Unreachable`).
fn catalog_specs() -> impl Iterator<Item = &'static nml_formats::abl::catalog::ActionSpec> {
    ACTIONS
        .iter()
        .filter(|a| !matches!(a.effect, Effect::NoCase | Effect::Unreachable))
}

/// Runs every catalog number with a case in its own block (all conditions true, example values)
/// for five steps and returns all script messages.
fn catalog_traces() -> Vec<ScriptTrace> {
    let mut text = String::new();
    for spec in catalog_specs() {
        let full: Vec<&str> = spec.params.iter().map(|k| sample(*k)).collect();
        let line = if full.is_empty() {
            format!("n{}", spec.number)
        } else {
            format!("n{};{}", spec.number, full.join(";"))
        };
        text += &script(&[(&[], &[line.as_str()])]);
    }
    let dialog = script(&[(&[], &["n12;x"])]);
    let mut rules = Rules::synthetic(TYPES, DIM as u32);
    rules.scripts = testing::rules_with(&text, &[("1", &dialog)], &[("feld", [2, 2, 5, 5])]);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    let mut state = start(&rules);
    let options = StepOptions {
        all_conditions_true: true,
    };
    (0..5)
        .flat_map(|_| traces(step_with(&mut state, &rules, &[], options)))
        .collect()
}

/// No catalog number with a case reports "no effect yet" (`ScriptTrace::Pending`);
/// the mechanism stays as a safety net (`pending_actions_are_traced_and_deleted`).
#[test]
fn no_number_is_pending() {
    let pending: Vec<i32> = catalog_traces()
        .iter()
        .filter_map(|t| match t {
            ScriptTrace::Pending { action, .. } => Some(*action),
            _ => None,
        })
        .collect();
    assert!(pending.is_empty(), "no effect yet: {pending:?}");
}

// QUIRK E-133: kept
#[test]
fn every_catalog_number_builds_its_message() {
    let rules = rules_with_areas("", &[("feld", [2, 2, 5, 5])]);
    for spec in catalog_specs() {
        let full: Vec<&str> = spec.params.iter().map(|k| sample(*k)).collect();
        for n in [usize::from(spec.min_params), full.len()] {
            let a = action(i32::from(spec.number), &full[..n]);
            let msg = msg_of(&rules, &a);
            if matches!(spec.number, 0 | 1) {
                assert_eq!(msg.number, i32::from(spec.number));
                continue;
            }
            match a8_values(spec.number, n) {
                Some(count) => {
                    assert_eq!(msg.values.len(), count, "action {} with {n}", spec.number);
                    assert_eq!(
                        msg.number,
                        a10_number(spec.number),
                        "action {}",
                        spec.number
                    );
                }
                None => {
                    assert_eq!(spec.effect, Effect::Internal, "action {}", spec.number);
                    assert!(msg.values.is_empty(), "action {}", spec.number);
                }
            }
        }
    }
    // Every number in its own block, all conditions true: no panic, no guard message.
    let events = catalog_traces();
    assert!(
        !events
            .iter()
            .any(|t| matches!(t, ScriptTrace::Guarded { .. })),
        "{events:?}"
    );
}

#[test]
fn ki_start_counts_words() {
    // re/ki-lifecycle 1.2: `area ` parameters count two words (area center), `W - 2` stands before the
    // third parameter.
    let rules = rules_with_areas("", &[("centrum eng", [34, 98, 39, 101])]);
    let a = action(
        1,
        &[
            "3",
            "55909",
            "3",
            "4",
            "2",
            "0",
            "area centrum eng",
            "0",
            "1",
            "3",
            "0",
            "1",
        ],
    );
    let msg = msg_of(&rules, &a);
    let ints: Vec<i32> = msg
        .values
        .iter()
        .map(|v| match v {
            MsgValue::Int(i) => *i,
            MsgValue::Text(t) => panic!("text {t}"),
        })
        .collect();
    assert_eq!(ints, [3, 55909, 11, 3, 4, 2, 0, 36, 99, 0, 1, 3, 0, 1]);
    // With two parameters no word counter (the slot stays unwritten), without parameters nothing.
    assert_eq!(
        msg_of(&rules, &action(0, &["3", "55909"])).values,
        [MsgValue::Int(3), MsgValue::Int(55909)]
    );
    assert!(msg_of(&rules, &action(0, &[])).values.is_empty());
    // Unknown area -> (0, 0); the case of the prefix does not matter.
    assert_eq!(
        msg_of(&rules, &action(0, &["1", "2", "AREA nix"])).values,
        [
            MsgValue::Int(1),
            MsgValue::Int(2),
            MsgValue::Int(2),
            MsgValue::Int(0),
            MsgValue::Int(0)
        ]
    );
}

// QUIRK E-123: kept
#[test]
fn messages_cut_areas_and_keep_texts() {
    let rules = rules_for("");
    let msg = msg_of(&rules, &action(153, &["drake", "area tor", "area feld"]));
    assert_eq!(
        msg,
        ScriptMsg {
            number: 153,
            values: vec![
                MsgValue::Text("drake".into()),
                MsgValue::Text("tor".into()),
                MsgValue::Text("feld".into()),
            ],
        }
    );
    // 35 rearranges: `Z0, Z2, M1`; 384 appends `number == 384`.
    assert_eq!(
        msg_of(&rules, &action(35, &["5", "area x", "2"])).values,
        [
            MsgValue::Int(5),
            MsgValue::Int(2),
            MsgValue::Text("x".into())
        ]
    );
    assert_eq!(
        msg_of(&rules, &action(384, &["7", "held", "9"])).values,
        [
            MsgValue::Int(7),
            MsgValue::Int(9),
            MsgValue::Int(1),
            MsgValue::Text("held".into())
        ]
    );
}

#[test]
fn percent_vectors() {
    // Single precision like x87 under Direct3D (PC24); in `f64` it would be 28 and 53 respectively.
    assert_eq!(percent_len(29, 100), 29);
    assert_eq!(percent_len(211, 100), 210);
    assert_eq!(percent_len(16_060, 37), 5_942);
    assert_eq!(percent_time(50, 1_000), 500);
    assert_eq!(percent_time(50, 58), 29);
    assert_eq!(percent_time(50, 106), 52);
    assert_eq!(percent_len(i32::MAX, 1_000), i32::MIN);
    assert_eq!(percent_len(-29, 100), -29);
}

/// Ticks of waiting until the following action `n12;done` (0 = in the first step).
fn ticks_until_done(actions: &[&str]) -> Option<usize> {
    let mut list: Vec<&str> = actions.to_vec();
    list.push("n12;done");
    let rules = rules_for(&script(&[(&[], &list)]));
    step_of_done(&rules, 200).map(|s| s - 1)
}

// QUIRK E-131: kept
#[test]
fn waits_follow_a4() {
    assert_eq!(ticks_until_done(&["w149;1;2;3"]), Some(12));
    assert_eq!(ticks_until_done(&["w156;1"]), Some(12));
    assert_eq!(ticks_until_done(&["w142;0;k;1000"]), Some(20));
    assert_eq!(ticks_until_done(&["w145;100"]), Some(2));
    assert_eq!(ticks_until_done(&["w146;0"]), Some(1));
    assert_eq!(ticks_until_done(&["w216;150"]), Some(3));
    assert_eq!(ticks_until_done(&["w172;100"]), Some(2));
    assert_eq!(ticks_until_done(&["w212;5;100"]), Some(2));
    // 240: `(t * Z0) / 100` with `t = 50 % of 1000` and `Z0 = 2` (player) -> 10 ms.
    assert_eq!(ticks_until_done(&["n25;t;1000", "w240;2;k;50;t"]), Some(1));
    // 241: `(Z0 * t) / 100` = (300 * 500) / 100 = 1500 ms.
    assert_eq!(ticks_until_done(&["n25;t;1000", "w241;300;50;t"]), Some(30));
    // 242: fixed at 0 (E-131) -> one tick.
    assert_eq!(ticks_until_done(&["w242;50;t"]), Some(1));
    // 220-223 and 243 do not wait, nor do numbers without a wait case (`w152`).
    assert_eq!(ticks_until_done(&["w220;0;k;50;1"]), Some(0));
    assert_eq!(ticks_until_done(&["w243;50;t"]), Some(0));
    assert_eq!(ticks_until_done(&["w152;x"]), Some(0));
    // Text waits until the text event: line 5 lasts 1 * 55 ms, tip 5 `55 * 3 / 2` = 82 ms; both run out
    // in the second step, the block continues in the third. A tip outside slots 0/1 never reports
    // anything: forever.
    assert_eq!(ticks_until_done(&["w150;5;0"]), Some(2));
    assert_eq!(ticks_until_done(&["w183;5;0"]), Some(2));
    assert_eq!(ticks_until_done(&["w183;5;2"]), None);
    // Without `w` no waiting.
    assert_eq!(ticks_until_done(&["n149;1;2;3"]), Some(0));
}

#[test]
fn text_wait_continues_on_its_text_event() {
    // Query 600: continues as soon as a text event of the ID is present; the first hit is
    // consumed.
    // Line 100 lasts 3000 ms and does not run out in these steps.
    let rules = rules_with_speech(&script(&[(&[], &["w150;100;0", "n12;done"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 3);
    assert!(!level(&state).switches.contains_key("done"));
    state.script.mission.speech.events = vec![4, 100, 100];
    run(&mut state, &rules, 1);
    assert!(level(&state).switches.contains_key("done"));
    assert_eq!(state.script.mission.speech.events, [4, 100]);
}

#[test]
fn event_wait_ends_when_all_named_objects_have_the_bit() {
    let rules = rules_with_areas(
        &script(&[
            (&[], &["w163;zug;area feld", "n12;done"]),
            (&[], &["w163;;area feld", "n12;leer"]),
        ]),
        &[("feld", [2, 2, 5, 5])],
    );
    let mut state = start(&rules);
    let (a, b) = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, None, (3, 3));
        let b = spawn(&mut sim, None, (4, 4));
        let list = format!("{};{}", a.as_i32(), b.as_i32());
        sim.state.names.set("zug", list);
        (a, b)
    };
    run(&mut state, &rules, 3);
    assert!(level(&state).switches.is_empty());
    {
        let mut sim = Sim::new(&mut state, &rules);
        set_object_event_bit(&mut sim, a, 0, true);
    }
    run(&mut state, &rules, 2);
    assert!(level(&state).switches.is_empty(), "only one has the bit");
    {
        let mut sim = Sim::new(&mut state, &rules);
        set_object_event_bit(&mut sim, b, 0, true);
        set_object_event_bit(&mut sim, b, 1, true);
    }
    run(&mut state, &rules, 1);
    assert!(level(&state).switches.contains_key("done"));
    {
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(object_event_bits(&sim, a), 0, "bit 0 cleared");
        assert_eq!(object_event_bits(&sim, b), 2, "only bit 0 cleared");
    }
    // Empty name waits forever.
    run(&mut state, &rules, 20);
    assert!(!level(&state).switches.contains_key("leer"));
}

#[test]
fn query_2500_needs_one_object_and_clears_only_when_true() {
    let rules = rules_for("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, Some("a"), (1, 1));
    assert!(!query_2500(&mut sim, "a", 1, true));
    assert!(!query_2500(&mut sim, "unbekannt", 1, true));
    set_object_event_bit(&mut sim, a, 1, true);
    assert!(query_2500(&mut sim, "A", 1, false));
    assert_eq!(object_event_bits(&sim, a), 2);
    assert!(query_2500(&mut sim, "a", 1, true));
    assert_eq!(object_event_bits(&sim, a), 0);
}

// QUIRK E-172: kept
#[test]
fn w334_with_empty_names_does_not_wait() {
    // Case `0x51`: message, `m_bDeleteMe` and waiting only with `L > 0`.
    let rules = rules_for(&script(&[
        (&[], &["w334;;;", "n12;done"]),
        (&[], &["w334;;;"]),
        (&[], &["w334;a;b", "n12;b"]),
    ]));
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    let c = level(&state);
    assert!(c.switches.contains_key("done"));
    assert!(!c.switches.contains_key("b"));
    let indices: Vec<u32> = c.blocks.iter().map(|b| b.index).collect();
    assert_eq!(
        indices,
        [1, 2],
        "block 1 without effect stays, block 2 waits"
    );
    assert!(c.blocks[1].running);
    // 334 is a command action: unknown names yield no ID, so no command and no message;
    // the waiting stays.
    assert!(
        !first
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Command { .. })),
        "{first:?}"
    );
}

// QUIRK E-133: kept
#[test]
fn no_effect_numbers_pass_layer_e_without_pending() {
    // E-133: 17, 344-346, 350-352 and 403 reach empty D cases; 403 waits anyway.
    let rules = rules_for(&script(&[
        (&[], &["n17;1", "n344;1;2;x", "n345;1;2;x", "n346;1;2;x"]),
        (&[], &["n350;a;5", "n351;a;5", "n352;a;5"]),
        (&[], &["w403;a;b", "n12;nie"]),
    ]));
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    assert!(
        !first
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. })),
        "{first:?}"
    );
    let c = level(&state);
    let indices: Vec<u32> = c.blocks.iter().map(|b| b.index).collect();
    assert_eq!(indices, [2]);
    assert!(c.blocks[0].running && c.blocks[0].delete_me);
    run(&mut state, &rules, 5);
    assert!(level(&state).switches.is_empty());
}

// QUIRK E-181: kept
// QUIRK E-166: deviation
#[test]
fn profile_variables_382_383() {
    // 382 creates `Var[p0]` with 0 (also with an empty name, E-181) and writes it into the mirror; 383
    // reads the mirror (missing 0) back into `Var[p0]`. Empty name: no message, no `m_bDeleteMe`.
    // In detail: `present_tests::profile_382_writes_mirror_and_event_383_reads_it`.
    let rules = rules_for(&script(&[
        (&[], &["n382;v"]),
        (&[], &["n382;;x"]),
        (&[], &["n383;w"]),
        (&[], &["n383;;x"]),
    ]));
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    let c = level(&state);
    assert_eq!(c.variables.get("v"), Some(&0));
    assert_eq!(c.variables.get(""), Some(&0));
    assert_eq!(c.variables.get("w"), Some(&0));
    let indices: Vec<u32> = c.blocks.iter().map(|b| b.index).collect();
    assert_eq!(indices, [1, 3]);
    assert_eq!(
        state.script.mission.profile_vars.get("v"),
        Some(&0),
        "382 writes the mirror"
    );
    assert_eq!(state.script.mission.profile_vars.len(), 1);
    assert!(
        !first
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. })),
        "{first:?}"
    );
}

fn rules_with_speech(level: &str) -> Rules {
    let mut rules = rules_for(level);
    rules.scripts = testing::with_speech(rules.scripts, &[(100, 3000, 20), (200, 0, 20)]);
    rules
}

// QUIRK E-141: deviation
#[test]
fn timer_from_speech_length() {
    // 230: `percent_len(speech length, Z1)`; 232: old value + speech length (if missing, the length).
    // Speech length: OGG, otherwise text length * 55, unknown ID: digit count * 55.
    let rules = rules_with_speech(&script(&[(
        &[],
        &[
            "n230;a;50;100",
            "n230;b;100;200",
            "n232;c;200",
            "n25;d;5",
            "n232;d;100",
            "n230;e;100;12345",
        ],
    )]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    let t = &level(&state).timers;
    assert_eq!(
        (t["a"], t["b"], t["c"], t["d"], t["e"]),
        (1500, 1100, 1100, 3005, 275)
    );
}

#[test]
fn speech_length_is_d_case_0x89() {
    let rules = rules_with_speech("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    let mut sim = Sim::new(&mut state, &rules);
    let file = ScriptFile { blocks: Vec::new() };
    let source = ScriptSource {
        path: String::new(),
        checksum: 0,
    };
    let mut c = Container::new(0, source, &file);
    let mut msg = ScriptMsg {
        number: 500,
        values: vec![MsgValue::Int(200)],
    };
    assert_eq!(dispatch(&mut sim, &mut c, &mut msg), Dispatched::Done);
    assert_eq!(msg.values, [MsgValue::Int(1100)]);
    let mut msg = ScriptMsg {
        number: NUMBER_WITHOUT_CASE,
        values: vec![MsgValue::Int(1)],
    };
    assert_eq!(dispatch(&mut sim, &mut c, &mut msg), Dispatched::Pending);
    let mut msg = ScriptMsg {
        number: 17,
        values: vec![MsgValue::Int(1)],
    };
    assert_eq!(dispatch(&mut sim, &mut c, &mut msg), Dispatched::Done);
}

#[test]
fn dialog_action_always_waits_and_ends_the_block_loop() {
    // Case `0x24`: forced waiting, the blocks in front drop out in this tick, continuation at the
    // next tick without a check (the dialog already ends in the start tick).
    let mut rules = Rules::synthetic(TYPES, DIM as u32);
    let dialog = script(&[(&[], &["n12;x"])]);
    rules.scripts = testing::rules_with(
        &script(&[(&[], &["n12;a"]), (&[], &["n39;d", "n12;b"])]),
        &[("d", &dialog)],
        &[],
    );
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    assert!(first.contains(&ScriptTrace::Suspended {
        container: LEVEL_CONTAINER_ID,
        block: 1,
        action: 0,
    }));
    assert!(level(&state).switches.is_empty());
    run(&mut state, &rules, 1);
    let switches: Vec<&str> = level(&state).switches.keys().map(String::as_str).collect();
    assert_eq!(switches, ["a", "b"]);
}

#[test]
fn attributes_384_385_with_empty_name_do_nothing() {
    // Case `0x66`: reads `Z0`, `p1`, `Z2`; with an empty `p1` neither a message nor `m_bDeleteMe`
    // (re/script-actions A.3, `0x0056CA99`-`0x0056CACB`).
    let rules = rules_for(&script(&[
        (&[], &["n384;1;;2"]),
        (&[], &["n385;1;;2"]),
        (&[], &["n384;1;held;2"]),
        (&[], &["n385;1;held;2"]),
    ]));
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    let indices: Vec<u32> = level(&state).blocks.iter().map(|b| b.index).collect();
    assert_eq!(indices, [0, 1]);
    let pending: Vec<(u32, i32)> = first
        .iter()
        .filter_map(|t| match t {
            ScriptTrace::Pending { block, action, .. } => Some((*block, *action)),
            _ => None,
        })
        .collect();
    // 384/385 with a name run as a world action (an unknown name "held" stays without effect).
    assert_eq!(pending, []);
}

// --- Dialog scripts, resting of AI and level scripts ----------------------------------------

/// Rules with the level script `level` and the dialogs `(name, text)`.
fn rules_with_dialogs(level: &str, dialogs: &[(&str, &str)]) -> Rules {
    let mut rules = Rules::synthetic(TYPES, DIM as u32);
    rules.scripts = testing::rules_with(level, dialogs, &[]);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    rules
}

fn dialog_fired(traces: &[ScriptTrace]) -> bool {
    traces.iter().any(|t| {
        matches!(
            t,
            ScriptTrace::Fired {
                container: DIALOG_CONTAINER_ID,
                ..
            }
        )
    })
}

fn level_switches(state: &SimState) -> Vec<&str> {
    level(state).switches.keys().map(String::as_str).collect()
}

#[test]
fn dialog_start_ends_the_block_loop_same_step() {
    // Block B (at the back, runs first) starts the dialog; block A drops out in this tick, the
    // countdown decreases anyway. The dialog ticks in the same step and waits 100 ms (end in step 3);
    // the level block continues in step 4 without a check.
    let dialog = script(&[(&[], &["w40;100", "n12;x"])]);
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n12;a"]), (&[], &["n39;d", "n12;b"])]),
        &[("d", &dialog)],
    );
    let mut state = start(&rules);
    level_mut(&mut state).counter = 1000;

    let first = run(&mut state, &rules, 1).remove(0);
    assert!(fired(&first, 1));
    assert!(!fired(&first, 0), "block A drops out in the start tick");
    assert!(first.contains(&ScriptTrace::DialogStarted {
        path: "d.abl".into()
    }));
    assert!(dialog_fired(&first), "the dialog ticks in the start tick");
    assert!(dialog_running(&state));
    let d = state.script.dialog.as_ref().expect("dialog container");
    assert_eq!(d.id, DIALOG_CONTAINER_ID);
    assert_eq!(d.source.path, "d.abl");
    assert_eq!(
        d.source.checksum, rules.scripts.dialogs["d"].checksum,
        "checksum of the dialog file"
    );
    assert!(level_switches(&state).is_empty());
    assert_eq!(
        level(&state).counter,
        950,
        "countdown decreases in the start tick"
    );
    assert_eq!(
        level(&state).blocks[1],
        BlockState {
            index: 1,
            delete_me: true,
            deleteable: true,
            running: true,
            cur_action: 0,
            timeout: -1,
        }
    );

    let second = run(&mut state, &rules, 1).remove(0);
    assert!(
        !fired(&second, 0) && !fired(&second, 1),
        "level script rests"
    );
    assert_eq!(level(&state).counter, 950, "countdown rests in the dialog");
    assert!(dialog_running(&state));

    let third = run(&mut state, &rules, 1).remove(0);
    assert!(third.contains(&ScriptTrace::DialogEnded));
    assert!(
        !fired(&third, 0) && !fired(&third, 1),
        "dialog ends not until after the level script"
    );
    assert!(!dialog_running(&state));
    assert!(level_switches(&state).is_empty());

    let fourth = run(&mut state, &rules, 1).remove(0);
    assert!(fired(&fourth, 1) && fired(&fourth, 0));
    assert_eq!(level_switches(&state), ["a", "b"]);
    assert_eq!(level(&state).counter, 900);
    assert!(level(&state).blocks.is_empty());
}

#[test]
fn dialog_ticks_in_the_same_step_and_ends_after_its_last_block() {
    let dialog = script(&[(&[], &["n12;x"])]);
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n12;a"]), (&[], &["n39;d", "n12;b"])]),
        &[("d", &dialog)],
    );
    let mut state = start(&rules);
    let first = run(&mut state, &rules, 1).remove(0);
    let tail: Vec<&ScriptTrace> = first
        .iter()
        .skip_while(|t| !matches!(t, ScriptTrace::DialogStarted { .. }))
        .collect();
    assert_eq!(
        tail,
        [
            &ScriptTrace::DialogStarted {
                path: "d.abl".into()
            },
            &ScriptTrace::Suspended {
                container: LEVEL_CONTAINER_ID,
                block: 1,
                action: 0
            },
            &ScriptTrace::Fired {
                container: DIALOG_CONTAINER_ID,
                block: 0
            },
            &ScriptTrace::Deleted {
                container: DIALOG_CONTAINER_ID,
                block: 0
            },
            &ScriptTrace::DialogEnded,
        ]
    );
    assert!(!dialog_running(&state));
    assert!(level_switches(&state).is_empty());
    run(&mut state, &rules, 1);
    assert_eq!(level_switches(&state), ["a", "b"]);
}

// QUIRK E-139: kept
#[test]
fn ki_and_level_rest_during_dialog() {
    use crate::ki::start::testkit::{ai_setup, ki_rules};
    use crate::save::checksum_of;

    let mut rules = ki_rules(false);
    rules.scripts = testing::rules_with(
        &script(&[(&[], &["n12;a"]), (&[], &["n39;d"])]),
        &[("d", &script(&[(&[], &["w40;500", "n12;x"])]))],
        &[],
    );
    assert!(rules.scripts.errors.is_empty());
    let mut setup = ai_setup(
        0,
        KiSetup {
            enabled: true,
            ..KiSetup::default()
        },
    );
    setup.script.enabled = true;
    let mut state = new_match(&rules, &setup).expect("match with AI and script");
    assert!(state.ki.engines.iter().any(Option::is_some));

    // Step 1: the AI still ticks (before the level scripts), then the dialog starts.
    let before = checksum_of(&state.ki);
    run(&mut state, &rules, 1);
    assert_ne!(checksum_of(&state.ki), before, "AI ticks in the start tick");
    assert!(dialog_running(&state));

    // 500 ms of waiting: the dialog ends in step 11; AI and level script rest until then.
    let ki = checksum_of(&state.ki);
    let level_state = level(&state).clone();
    for n in 2..=11 {
        let traces = run(&mut state, &rules, 1).remove(0);
        assert_eq!(checksum_of(&state.ki), ki, "AI rests in step {n}");
        assert_eq!(
            *level(&state),
            level_state,
            "level script rests in step {n}"
        );
        assert_eq!(
            traces.contains(&ScriptTrace::DialogEnded),
            n == 11,
            "step {n}"
        );
    }
    assert!(!dialog_running(&state));

    // Step 12: AI and level script run again.
    run(&mut state, &rules, 1);
    assert_ne!(checksum_of(&state.ki), ki, "AI runs after the dialog");
    assert_eq!(level_switches(&state), ["a"]);
    assert!(level(&state).blocks.is_empty());
}

#[test]
fn dialog_waits_on_its_own_timers() {
    // `w40;100` in the dialog: start and first tick in step 1, two ticks of waiting, end in step 3
    // (after 2 more steps).
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n39;d", "n12;b"])]),
        &[("d", &script(&[(&[], &["w40;100"])]))],
    );
    let mut state = start(&rules);
    let steps = run(&mut state, &rules, 4);
    let ended: Vec<bool> = steps
        .iter()
        .map(|t| t.contains(&ScriptTrace::DialogEnded))
        .collect();
    assert_eq!(ended, [false, false, true, false]);
    assert!(steps[0].contains(&ScriptTrace::Suspended {
        container: DIALOG_CONTAINER_ID,
        block: 0,
        action: 0,
    }));
    assert_eq!(level_switches(&state), ["b"]);
}

// QUIRK E-143: kept
#[test]
fn start_dialog_replaces_and_guards() {
    // `testing::start_dialog` (dry run) without a level script; a running dialog is replaced (fresh
    // container). An unknown name creates no container (`Guarded`) and ends a running dialog like the
    // original (E-143: `FUN_0042DC40` clears the flag beforehand).
    let rules = rules_with_dialogs(
        "",
        &[
            ("d", &script(&[(&[], &["w40;-1"])])),
            ("e", &script(&[(&[], &["n12;y"]), (&[], &["w40;-1"])])),
        ],
    );
    let mut state = start(&rules);
    let start_events = |state: &mut SimState, name: &str| {
        let mut sim = Sim::new(state, &rules);
        testing::start_dialog(&mut sim, name);
        traces(std::mem::take(&mut sim.state.events))
    };
    let events = start_events(&mut state, "fehlt");
    assert!(matches!(
        events[..],
        [ScriptTrace::Guarded { number: 39, .. }]
    ));
    assert!(!dialog_running(&state));

    start_events(&mut state, "d");
    assert!(dialog_running(&state));
    run(&mut state, &rules, 2);
    assert!(state.script.dialog.as_ref().unwrap().blocks[0].running);

    let events = start_events(&mut state, "e");
    assert_eq!(
        events,
        [ScriptTrace::DialogStarted {
            path: "e.abl".into()
        }]
    );
    let d = state.script.dialog.as_ref().unwrap();
    assert_eq!(d.source.path, "e.abl");
    assert_eq!(d.blocks, [BlockState::new(0), BlockState::new(1)]);

    let events = start_events(&mut state, "fehlt");
    assert!(matches!(
        events[..],
        [
            ScriptTrace::Guarded { number: 39, .. },
            ScriptTrace::DialogEnded
        ]
    ));
    assert!(!dialog_running(&state));
}

// QUIRK E-174: deviation
#[test]
fn dialog_start_from_a_dialog_container_is_guarded() {
    // E-174 (deviating): the load check rejects 39 in dialog scripts; if the message nevertheless comes
    // from the dialog container, nothing starts (the running container would otherwise be replaced
    // while its tick is still running).
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n39;d"])]),
        &[("d", &script(&[(&[], &["w40;-1"])]))],
    );
    let mut state = start(&rules);
    let file = &rules.scripts.dialogs["d"];
    let source = ScriptSource {
        path: file.path.clone(),
        checksum: file.checksum,
    };
    let mut c = Container::new(DIALOG_CONTAINER_ID, source, &file.file);
    let mut sim = Sim::new(&mut state, &rules);
    let mut msg = ScriptMsg {
        number: 39,
        values: vec![MsgValue::Text("d".into())],
    };
    assert_eq!(dispatch(&mut sim, &mut c, &mut msg), Dispatched::Done);
    assert!(sim.state.script.dialog.is_none());
    let events = traces(std::mem::take(&mut sim.state.events));
    assert!(matches!(
        events[..],
        [ScriptTrace::Guarded { number: 39, .. }]
    ));
}

#[test]
fn dialog_without_its_file_ends() {
    // Reachable only via a corrupted savegame (loading does not check this): the dialog ends with
    // `Guarded` and `DialogEnded` instead of running forever.
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n12;a"])]),
        &[("d", &script(&[(&[], &["w40;-1"])]))],
    );
    let mut state = start(&rules);
    let file = &rules.scripts.dialogs["d"];
    let source = ScriptSource {
        path: "weg.abl".into(),
        checksum: file.checksum,
    };
    state.script.dialog = Some(Container::new(DIALOG_CONTAINER_ID, source, &file.file));
    let first = run(&mut state, &rules, 1).remove(0);
    assert!(first.iter().any(|t| matches!(
        t,
        ScriptTrace::Guarded {
            container: DIALOG_CONTAINER_ID,
            ..
        }
    )));
    assert!(first.contains(&ScriptTrace::DialogEnded));
    assert!(!dialog_running(&state));
    assert!(
        level_switches(&state).is_empty(),
        "level script was still resting"
    );
    run(&mut state, &rules, 1);
    assert_eq!(level_switches(&state), ["a"]);
}

// --- Speech state, tips, text events (E-173) ---

fn speech(state: &SimState) -> &SpeechState {
    &state.script.mission.speech
}

fn limits(events: &[SimEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Script(ScriptTrace::Limit { what }) => Some(what.clone()),
            _ => None,
        })
        .collect()
}

// QUIRK E-141: deviation
#[test]
fn line_ends_after_its_length() {
    // Text 100: OGG 3000 ms -> expiry in step 60 (3000 - 60 * 50 = 0 < 1); `w150` continues in the
    // following step and consumes the event.
    let rules = rules_with_speech(&script(&[(&[], &["w150;100;1", "n12;done"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    assert_eq!(
        speech(&state).line,
        Some(SpeechLine {
            text: 100,
            remaining_ms: 2950,
            slot: 1,
        })
    );
    run(&mut state, &rules, 58);
    assert!(speech(&state).events.is_empty());
    assert_eq!(speech(&state).line.map(|l| l.remaining_ms), Some(50));
    run(&mut state, &rules, 1);
    assert_eq!(speech(&state).line, None);
    assert_eq!(speech(&state).events, [100]);
    assert!(!level(&state).switches.contains_key("done"));
    run(&mut state, &rules, 1);
    assert!(level(&state).switches.contains_key("done"));
    assert!(speech(&state).events.is_empty());
}

// QUIRK E-173: kept
#[test]
fn new_line_reports_the_old_one_without_removing() {
    // E-173: the interrupted line 7 is appended at the back without first removing an equal entry;
    // then the new line 8 removes the first entry 8.
    let rules = rules_with_speech("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    let mut sim = Sim::new(&mut state, &rules);
    speech::play_line(&mut sim, 7, 1);
    sim.state.script.mission.speech.events = vec![7, 8];
    speech::play_line(&mut sim, 8, 0);
    assert_eq!(sim.state.script.mission.speech.events, [7, 7]);
    assert_eq!(
        sim.state.script.mission.speech.line,
        Some(SpeechLine {
            text: 8,
            remaining_ms: 55,
            slot: 0,
        })
    );
    // The line itself writes nothing into the info history.
    assert!(sim.state.script.mission.info_history.is_empty());
}

// QUIRK E-173: kept
#[test]
fn expiry_removes_one_equal_entry_first() {
    let rules = rules_with_speech("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    state.script.mission.speech.line = Some(SpeechLine {
        text: 5,
        remaining_ms: 50,
        slot: 1,
    });
    state.script.mission.speech.events = vec![5, 9, 5];
    let mut sim = Sim::new(&mut state, &rules);
    speech::tick(&mut sim, 50);
    assert_eq!(sim.state.script.mission.speech.events, [9, 5, 5]);
    assert_eq!(sim.state.script.mission.speech.line, None);
    // A tip expires the same way.
    sim.state.script.mission.speech.tip_ms = [0, 30];
    sim.state.script.mission.speech.tip_text = [0, 9];
    speech::tick(&mut sim, 50);
    assert_eq!(sim.state.script.mission.speech.events, [5, 5, 9]);
    assert_eq!(sim.state.script.mission.speech.tip_ms, [0, -20]);
    // Expired stays expired (only times > 0 count).
    speech::tick(&mut sim, 50);
    assert_eq!(sim.state.script.mission.speech.events, [5, 5, 9]);
}

// QUIRK E-173: kept
// QUIRK E-141: deviation
#[test]
fn tip_time_is_three_halves_and_overwrite_is_silent() {
    // Text 200: 20 UTF-16 units, `tpc` 55 -> 20 * 55 * 3 / 2 = 1650 ms -> event in step 33.
    let rules = rules_with_speech(&script(&[(&[], &["n183;200;1"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    assert_eq!(speech(&state).tip_text, [0, 200]);
    assert_eq!(speech(&state).tip_ms, [0, 1600]);
    assert_eq!(state.script.mission.info_history, [200]);
    run(&mut state, &rules, 31);
    assert!(speech(&state).events.is_empty());
    run(&mut state, &rules, 1);
    assert_eq!(speech(&state).events, [200]);

    // Second tip in the same slot: overwrites time and text without a message; `w183` on the first one
    // waits forever.
    let rules = rules_with_speech(&script(&[(&[], &["w183;200;0", "n12;done"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    {
        let mut sim = Sim::new(&mut state, &rules);
        speech::show_tip(&mut sim, 100, 0);
    }
    assert_eq!(speech(&state).tip_text, [100, 0]);
    assert_eq!(speech(&state).tip_ms, [1650, 0]);
    assert!(speech(&state).events.is_empty());
    assert_eq!(state.script.mission.info_history, [200, 100]);
    run(&mut state, &rules, 200);
    assert_eq!(speech(&state).events, [100]);
    assert!(!level(&state).switches.contains_key("done"));
}

#[test]
fn tip_slot_outside_0_1_does_nothing() {
    let rules = rules_with_speech("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    let before = state.script.mission.clone();
    let mut sim = Sim::new(&mut state, &rules);
    speech::show_tip(&mut sim, 200, 2);
    speech::show_tip(&mut sim, 200, -1);
    assert_eq!(sim.state.script.mission, before);
}

#[test]
fn info_window_213_plays_on_slot_plus_4() {
    // 150, 183 and 213 are implemented in layer D (no `Pending`). 213 appends the text to the info
    // history and plays the line on slot `Pos + 4`; line 100 from 150 is reported and removed again
    // immediately.
    let rules = rules_with_speech(&script(&[(
        &[],
        &["n150;100;1", "n183;200;1", "n213;100;0;1;2"],
    )]));
    let mut state = start(&rules);
    let traces = run(&mut state, &rules, 1).remove(0);
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. })),
        "{traces:?}"
    );
    assert_eq!(
        speech(&state).line,
        Some(SpeechLine {
            text: 100,
            remaining_ms: 2950,
            slot: 4,
        })
    );
    assert!(speech(&state).events.is_empty());
    assert_eq!(state.script.mission.info_history, [200, 100]);
    assert_eq!(speech(&state).tip_ms, [0, 1600]);
}

// QUIRK E-124: kept
#[test]
fn condition_19_consumes_even_if_block_is_false() {
    // E-124: all conditions run; 19 consumes the event although `5;x` blocks the block.
    let rules = rules_for(&script(&[
        (&["&5;x", "&19;5"], &["n12;a"]),
        (&["&19;7"], &["n12;b"]),
    ]));
    let mut state = start(&rules);
    state.script.mission.speech.events = vec![5, 7, 5];
    let traces = run(&mut state, &rules, 1).remove(0);
    assert_eq!(level_switches(&state), ["b"]);
    assert_eq!(speech(&state).events, [5]);
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::ConditionPending { .. })),
        "{traces:?}"
    );
}

#[test]
fn query_600_removes_only_the_first_match() {
    let rules = rules_for("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    state.script.mission.speech.events = vec![3, 4, 3];
    assert!(speech::take_event(&mut state, 3));
    assert_eq!(speech(&state).events, [4, 3]);
    assert!(!speech::take_event(&mut state, 9));
    assert_eq!(speech(&state).events, [4, 3]);
}

// QUIRK E-173: kept
#[test]
fn event_list_drops_the_oldest_at_the_limit() {
    let rules = rules_with_speech("");
    let mut state = new_match(&rules, &setup(false)).unwrap();
    state.script.mission.speech.events = (0..speech::MAX_TEXT_EVENTS as i32).collect();
    state.script.mission.speech.line = Some(SpeechLine {
        text: 5000,
        remaining_ms: 50,
        slot: 1,
    });
    state.script.mission.info_history = (0..speech::MAX_INFO_HISTORY as i32).collect();
    {
        let mut sim = Sim::new(&mut state, &rules);
        speech::tick(&mut sim, 50);
        speech::show_tip(&mut sim, 200, 0);
    }
    let events = &speech(&state).events;
    assert_eq!(events.len(), speech::MAX_TEXT_EVENTS);
    assert_eq!((events[0], events[events.len() - 1]), (1, 5000));
    let history = &state.script.mission.info_history;
    assert_eq!(history.len(), speech::MAX_INFO_HISTORY);
    assert_eq!((history[0], history[history.len() - 1]), (1, 200));
    assert_eq!(limits(&state.events).len(), 2);
}

// --- Saving and loading in the middle of a run, replay -------------------------------------

/// Saves `state` and loads it again with a check (like a savegame on disk).
fn save_and_load(rules: &Rules, setup: &MatchSetup, state: &SimState) -> SimState {
    let save = crate::save::SaveGame {
        start: crate::setup::start_package(rules, setup),
        state: state.clone(),
        commands: Vec::new(),
    };
    let loaded = crate::save::SaveGame::from_bytes(&save.to_bytes()).expect("savegame readable");
    loaded.check(rules).expect("savegame valid");
    assert_eq!(
        crate::save::checksum(&loaded.state),
        crate::save::checksum(state)
    );
    loaded.state
}

/// `n` steps; returns the checksum after every step and all script messages.
fn sums_and_traces(
    state: &mut SimState,
    rules: &Rules,
    n: usize,
) -> (Vec<u64>, Vec<Vec<ScriptTrace>>) {
    let mut sums = Vec::new();
    let mut all = Vec::new();
    for _ in 0..n {
        all.push(traces(step(state, rules, &[])));
        sums.push(crate::save::checksum(state));
    }
    (sums, all)
}

// QUIRK E-142: deviation
#[test]
fn save_load_mid_dialog_and_mid_wait_keeps_the_sequence() {
    // Part 1: saving in a `w40` of the dialog (the level block hangs on action 39 of the dialog start).
    // The dialog waits 1000 ms (steps 2 to 21), then 500 ms; 50 steps.
    let dialog = script(&[(&[], &["w40;1000", "n12;x", "w40;500", "n12;y"])]);
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n39;d", "n12;after"])]),
        &[("d", &dialog)],
    );
    let setup = setup(true);
    let mut plain = start(&rules);
    run(&mut plain, &rules, 5);
    assert!(dialog_running(&plain), "saved in the middle of the dialog");
    let waiting = plain.script.dialog.as_ref().expect("dialog container");
    assert!(waiting.blocks[0].running && waiting.blocks[0].timeout > 0);
    let mut loaded = save_and_load(&rules, &setup, &plain);
    let (a, ta) = sums_and_traces(&mut plain, &rules, 50);
    let (b, tb) = sums_and_traces(&mut loaded, &rules, 50);
    assert_eq!(a, b, "checksums without and with loading");
    assert_eq!(ta, tb, "script messages without and with loading");
    assert!(
        ta.iter().flatten().any(|t| *t == ScriptTrace::DialogEnded),
        "the dialog ends in the run"
    );
    assert_eq!(level_switches(&plain), ["after"]);
    assert_eq!(level_switches(&loaded), ["after"]);

    // Part 2: saving in the event wait of the level (`w163` on two objects of a name); the event bit
    // does not come until after loading, in both runs in the same step.
    let rules = rules_with_areas(
        &script(&[(&[], &["w163;zug;area feld", "n12;done"])]),
        &[("feld", [2, 2, 5, 5])],
    );
    let mut plain = start(&rules);
    let ids = {
        let mut sim = Sim::new(&mut plain, &rules);
        let a = spawn(&mut sim, None, (3, 3));
        let b = spawn(&mut sim, None, (4, 4));
        sim.state
            .names
            .set("zug", format!("{};{}", a.as_i32(), b.as_i32()));
        [a, b]
    };
    run(&mut plain, &rules, 7);
    let block = &level(&plain).blocks[0];
    assert!(block.running && block.timeout == -1, "waits for the event");
    let mut loaded = save_and_load(&rules, &setup, &plain);
    let mut both = [&mut plain, &mut loaded];
    let mut sums: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
    for n in 0..50 {
        for (i, state) in both.iter_mut().enumerate() {
            if n == 10 {
                let mut sim = Sim::new(state, &rules);
                for id in ids {
                    set_object_event_bit(&mut sim, id, 0, true);
                }
            }
            step(state, &rules, &[]);
            sums[i].push(crate::save::checksum(state));
        }
    }
    assert_eq!(sums[0], sums[1], "checksums without and with loading");
    assert_eq!(level_switches(&plain), ["done"]);
    assert_eq!(level_switches(&loaded), ["done"]);
}

// QUIRK E-140: deviation
#[test]
fn replay_from_command_log_matches() {
    // Replay from the command log gives the same sequence as the run, also across a running
    // dialog.
    use crate::command::{Command, CommandKind};
    use crate::save::{RecordedCommand, Replay};

    let dialog = script(&[(&[], &["w40;500", "n12;x"])]);
    let rules = rules_with_dialogs(
        &script(&[(&[], &["n39;d", "n12;after"])]),
        &[("d", &dialog)],
    );
    let setup = setup(true);
    let diplomacy = |mode| Command {
        player: 0,
        kind: CommandKind::Diplomacy { p: 0, q: 1, mode },
    };
    let replay = Replay {
        start: crate::setup::start_package(&rules, &setup),
        commands: vec![
            RecordedCommand {
                step: 3,
                command: diplomacy(0),
            },
            RecordedCommand {
                step: 9,
                command: diplomacy(1),
            },
        ],
    };
    let replay = Replay::from_bytes(&replay.to_bytes()).unwrap();

    // Run by hand with the commands at their ticks.
    let mut state = start(&rules);
    let mut live = Vec::new();
    for n in 0..30u64 {
        let due: Vec<Command> = replay
            .commands
            .iter()
            .filter(|c| c.step == n)
            .map(|c| c.command.clone())
            .collect();
        step(&mut state, &rules, &due);
        live.push(crate::save::checksum(&state));
    }
    assert!(state.diplomacy.is_enemy(0, 1));
    assert_eq!(level_switches(&state), ["after"]);
    // Replay up to every tick gives the checksum of the run.
    for steps in [1u64, 5, 9, 10, 15, 30] {
        let at = replay.play(&rules, steps).expect("replay");
        assert_eq!(at.steps, steps);
        assert_eq!(
            crate::save::checksum(&at),
            live[steps as usize - 1],
            "tick {steps}"
        );
    }
}

// --- Condition values for the control channel ---------------------------------------------------

#[test]
fn condition_values_evaluate_on_a_copy() {
    // Block 0: 19 (consumes events), 35 (unknown name: true), 15 (without a case: false without a
    // message), 5 (switch). Block 1: 19 twice on the same ID - one after the other on the same copy.
    let rules = rules_for(&script(&[
        (&["|19;5", "&35;x", "|15", "&5;an"], &["n12;an"]),
        (&["|19;5", "|19;5"], &["n12;zwei"]),
    ]));
    let mut state = start(&rules);
    state.script.mission.speech.events.push(5);
    let before = crate::save::checksum(&state);
    let copy = state.clone();
    assert_eq!(
        condition_values(&state, &rules, LEVEL_CONTAINER_ID, 0),
        Some(vec![Some(true), Some(true), Some(false), Some(false)])
    );
    assert_eq!(
        condition_values(&state, &rules, LEVEL_CONTAINER_ID, 1),
        Some(vec![Some(true), Some(false)])
    );
    // The real state stays entirely the same (also the text events that 19 consumes on the copy).
    assert_eq!(state, copy);
    assert_eq!(crate::save::checksum(&state), before);
    assert_eq!(state.script.mission.speech.events, [5]);
    // Unknown container or block beyond the end of the file.
    assert_eq!(condition_values(&state, &rules, 0, 0), None);
    assert_eq!(condition_values(&state, &rules, 2, 0), None);
    assert_eq!(
        condition_values(&state, &rules, DIALOG_CONTAINER_ID, 0),
        None
    );
    assert_eq!(
        condition_values(&state, &rules, LEVEL_CONTAINER_ID, 2),
        None
    );
    // A deleted block still has its conditions in the file.
    level_mut(&mut state).blocks.remove(1);
    assert_eq!(
        condition_values(&state, &rules, LEVEL_CONTAINER_ID, 1),
        Some(vec![Some(true), Some(false)])
    );
    // Without a script there is no container.
    let off = new_match(&rules, &setup(false)).unwrap();
    assert_eq!(condition_values(&off, &rules, LEVEL_CONTAINER_ID, 0), None);
}

// --- Countdown display ----------------------------------------------------------------------------

fn countdown_s(state: &SimState) -> i32 {
    state.script.mission.countdown_s
}

// QUIRK E-205: kept
#[test]
fn countdown_display_follows_the_announced_container_and_truncates() {
    // `n15` registers the level container, `n11;5` sets the counter; the display follows the counter
    // **after** the container tick (4950 -> 4, E-205).
    let rules = rules_for(&script(&[(&[], &["n15", "n11;5"])]));
    let mut state = start(&rules);
    assert_eq!(state.script.mission.countdown_container, 0);
    run(&mut state, &rules, 1);
    assert_eq!(state.script.mission.countdown_container, LEVEL_CONTAINER_ID);
    assert_eq!(level(&state).counter, 4950);
    assert_eq!(countdown_s(&state), 4);
    // Truncated toward zero: 1000 -> 1, 950 -> 0.
    level_mut(&mut state).counter = 1050;
    run(&mut state, &rules, 1);
    assert_eq!(level(&state).counter, 1000);
    assert_eq!(countdown_s(&state), 1);
    run(&mut state, &rules, 1);
    assert_eq!(level(&state).counter, 950);
    assert_eq!(countdown_s(&state), 0);
    // Counter off (-1) shows 0, not -1.
    state.script.mission.countdown_s = 7;
    level_mut(&mut state).counter = -1;
    run(&mut state, &rules, 1);
    assert_eq!(level(&state).counter, -1);
    assert_eq!(countdown_s(&state), 0);
    // 999 -> 0 (the division truncates).
    state.script.mission.countdown_s = 7;
    level_mut(&mut state).counter = 1049;
    run(&mut state, &rules, 1);
    assert_eq!(level(&state).counter, 999);
    assert_eq!(countdown_s(&state), 0);
}

// QUIRK E-205: kept
#[test]
fn countdown_16_stops_the_display_and_keeps_the_last_value() {
    // Block 1 unregisters as soon as the rest is at most 1000 ms (condition 4 checks before the tick).
    let rules = rules_for(&script(&[(&[], &["n15", "n11;5"]), (&["&4;1"], &["n16"])]));
    let mut state = start(&rules);
    run(&mut state, &rules, 80);
    assert_eq!(level(&state).counter, 1000);
    assert_eq!(countdown_s(&state), 1);
    assert_eq!(state.script.mission.countdown_container, LEVEL_CONTAINER_ID);
    // Tick 81: 16 takes effect, the counter drops to 950, the display stays at 1 (no container any more).
    run(&mut state, &rules, 1);
    assert_eq!(state.script.mission.countdown_container, 0);
    assert_eq!(level(&state).counter, 950);
    assert_eq!(countdown_s(&state), 1);
    run(&mut state, &rules, 3);
    assert_eq!(countdown_s(&state), 1);
}

// QUIRK E-205: kept
#[test]
fn n15_and_n16_differ_in_the_level_script() {
    // E-185: the level container has the ID 1 (as in the original). With ID 0 the unregistering 16
    // (`countdown_container = 0`) would hit the level container and `n16` would act like `n15`.
    let off = rules_for(&script(&[(&[], &["n16", "n11;5"])]));
    let mut state = start(&off);
    assert_eq!(level(&state).id, 1);
    run(&mut state, &off, 3);
    assert_eq!(level(&state).counter, 4850);
    assert_eq!(countdown_s(&state), 0, "16 registers nothing");

    let on = rules_for(&script(&[(&[], &["n15", "n11;5"])]));
    let mut state = start(&on);
    run(&mut state, &on, 3);
    assert_eq!(
        countdown_s(&state),
        4,
        "15 shows the counter of the level container"
    );

    // 16 cancels an earlier 15 (block 1 runs in the following tick).
    let both = rules_for(&script(&[(&[], &["n15", "n11;5"]), (&["&4;4"], &["n16"])]));
    let mut state = start(&both);
    run(&mut state, &both, 1);
    assert_eq!(countdown_s(&state), 4);
    assert_eq!(state.script.mission.countdown_container, LEVEL_CONTAINER_ID);
    run(&mut state, &both, 21);
    assert_eq!(state.script.mission.countdown_container, 0);
}

// QUIRK E-205: kept
#[test]
fn countdown_display_pauses_during_dialog() {
    let dialog = script(&[(&[], &["w40;5000", "n12;x"])]);
    let rules = rules_with_dialogs(&script(&[(&[], &["n15", "n11;5"])]), &[("d", &dialog)]);
    let mut state = start(&rules);
    run(&mut state, &rules, 1);
    assert_eq!(countdown_s(&state), 4);
    state.script.mission.countdown_s = 99;
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert!(actions::start_dialog(&mut sim, "d"));
    }
    run(&mut state, &rules, 5);
    assert!(dialog_running(&state));
    assert_eq!(
        level(&state).counter,
        4950,
        "level script rests in the dialog"
    );
    assert_eq!(countdown_s(&state), 99, "display rests in the dialog");
}

// --- Skipping the dialog --------------------------------------------------------------------------

/// The command `ScriptSkipDialog` from slot 0.
fn skip_command() -> crate::command::Command {
    crate::command::Command {
        player: 0,
        kind: crate::command::CommandKind::ScriptSkipDialog,
    }
}

/// Rules with a dialog that waits 1000 ms; the level script starts it and then sets "after".
fn rules_with_long_dialog() -> Rules {
    let dialog = script(&[(&[], &["w40;1000", "n12;x"])]);
    rules_with_dialogs(
        &script(&[(&[], &["n39;d", "n12;after"])]),
        &[("d", &dialog)],
    )
}

// QUIRK E-226: deviation
#[test]
fn skip_dialog_ends_the_dialog_in_the_same_step() {
    // The dialog waits in `w40;1000`; the command arrives at the tick boundary (commands run before
    // the scripts). The dialog tick of the same step ends the container without a tick: `DialogEnded`,
    // neither dialog block nor level script run. The level script continues in the next step.
    let rules = rules_with_long_dialog();
    let mut state = start(&rules);
    run(&mut state, &rules, 3);
    assert!(dialog_running(&state), "dialog still waiting");

    let step_with_skip = traces(step(&mut state, &rules, &[skip_command()]));
    assert!(step_with_skip.contains(&ScriptTrace::DialogEnded));
    assert!(!dialog_running(&state));
    assert!(
        !dialog_fired(&step_with_skip),
        "the dialog container no longer ticks"
    );
    assert!(
        level_switches(&state).is_empty(),
        "level script was still resting"
    );

    let next = run(&mut state, &rules, 1).remove(0);
    assert!(fired(&next, 0), "level script continues");
    assert_eq!(level_switches(&state), ["after"]);

    // Counter-check without the command: the dialog keeps running (end not until after 1000 ms).
    let mut plain = start(&rules);
    run(&mut plain, &rules, 4);
    assert!(dialog_running(&plain));
    assert!(level_switches(&plain).is_empty());
}

// QUIRK E-226: deviation
#[test]
fn skip_without_dialog_is_rejected() {
    use crate::command::{Outcome, execute};
    let rules = rules_with_long_dialog();
    let mut state = start(&rules);
    let before = crate::save::checksum(&state);
    let mut sim = Sim::new(&mut state, &rules);
    let outcome = execute(&mut sim, &skip_command(), true);
    assert!(
        matches!(&outcome, Outcome::Rejected(why) if why.contains("no dialog running")),
        "without a dialog: {outcome:?}"
    );
    assert_eq!(crate::save::checksum(sim.state), before, "nothing changed");
    // Also without a script (no container) and from the script and AI origins rejected alike.
    let off = new_match(&rules, &setup(false)).unwrap();
    let mut off = off;
    let mut sim = Sim::new(&mut off, &rules);
    for origin in [
        crate::command::Origin::External,
        crate::command::Origin::Ki,
        crate::command::Origin::Script,
    ] {
        sim.origin = origin;
        assert!(
            matches!(
                execute(&mut sim, &skip_command(), true),
                Outcome::Rejected(_)
            ),
            "{origin:?}"
        );
    }
    sim.origin = crate::command::Origin::External;
    // With a dialog every origin works (no exception A8).
    let mut state = start(&rules);
    run(&mut state, &rules, 2);
    let mut sim = Sim::new(&mut state, &rules);
    sim.origin = crate::command::Origin::Ki;
    assert_eq!(execute(&mut sim, &skip_command(), true), Outcome::Accepted);
    assert!(sim.state.script.dialog.as_ref().unwrap().skipped);
}

// QUIRK E-226: deviation
#[test]
fn skipped_is_saved_and_survives_loading() {
    use crate::command::{Outcome, execute};
    let rules = rules_with_long_dialog();
    let setup = setup(true);
    let mut state = start(&rules);
    run(&mut state, &rules, 3);
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(execute(&mut sim, &skip_command(), true), Outcome::Accepted);
    }
    assert!(state.script.dialog.as_ref().unwrap().skipped);
    let plain_before = crate::save::checksum(&state);

    // Saved between command and dialog tick: the flag stays, the loaded run ends the dialog in the next
    // step like the unloaded one.
    let mut loaded = save_and_load(&rules, &setup, &state);
    assert!(loaded.script.dialog.as_ref().unwrap().skipped);
    assert_eq!(crate::save::checksum(&loaded), plain_before);
    let (a, ta) = sums_and_traces(&mut state, &rules, 5);
    let (b, tb) = sums_and_traces(&mut loaded, &rules, 5);
    assert_eq!(a, b);
    assert_eq!(ta, tb);
    assert!(ta[0].contains(&ScriptTrace::DialogEnded));
    assert_eq!(level_switches(&loaded), ["after"]);

    // The flag is in the checksum (counter-check: without it the checksum is a different one).
    let mut other = start(&rules);
    run(&mut other, &rules, 3);
    assert_ne!(crate::save::checksum(&other), plain_before);
}
