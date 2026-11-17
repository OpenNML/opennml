//! Tests of the command path of the script actions (`script::command`), with synthetic worlds and scripts
//! without a corpus. Pattern for the later numbers: only 191 (halt per ID) here.

use super::command::{issue, sender};
use super::query_tests::traces;
use super::tests::script;
use super::*;
use crate::command::testkit::{self, COLONIST};
use crate::command::{Command, CommandKind, Origin, Outcome, owns};
use crate::id::ObjectId;
use crate::object::status;
use crate::rules::Rules;
use crate::save::{Replay, checksum};
use crate::setup::new_match;
use crate::tick::step;
use crate::world::destroy_object;

/// Rules with a level script whose only block has the actions `actions`.
fn rules_for(actions: &[&str]) -> Rules {
    let mut rules = testkit::rules();
    rules.scripts = testing::rules_with(&script(&[(&[], actions)]), &[], &[]);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    rules
}

/// Empty test world with a started level script.
fn started(rules: &Rules) -> SimState {
    let mut state = testkit::state(rules);
    let mut sim = Sim::new(&mut state, rules);
    let setup = ScriptSetup {
        enabled: true,
        time_per_char: 55,
        ..Default::default()
    };
    start_match(&mut sim, &setup, 7).expect("script start");
    state
}

fn name_ids(sim: &mut Sim, name: &str, ids: &[ObjectId]) {
    let list = ids
        .iter()
        .map(|id| id.as_i32().to_string())
        .collect::<Vec<_>>()
        .join(";");
    sim.state.names.set(name, list);
}

/// Commands of the script path from the messages.
fn commands(traces: &[ScriptTrace]) -> Vec<&ScriptTrace> {
    traces
        .iter()
        .filter(|t| matches!(t, ScriptTrace::Command { .. }))
        .collect()
}

/// The walk module of the unit is working (status `MOVE`, which `AbortAllModules` clears via `stop_moving`).
fn moving(sim: &Sim, unit: ObjectId) -> bool {
    sim.object(unit)
        .is_some_and(|o| o.status & status::MOVE != 0)
}

/// A march to tile (2, 2); the queue of the unit works it off in the next tick.
fn start_walking(sim: &mut Sim, unit: ObjectId) {
    let walk = CommandKind::Move {
        units: vec![unit],
        x: 2 << 12,
        z: 2 << 12,
        target: -1,
        formation: 0,
        direction: crate::motion::NO_DIRECTION,
        spread: crate::command::NO_SPREAD,
        flags: 0,
    };
    let owner = unit.slot();
    // Animals (slot 8) are no sender: for them the march goes via the script path.
    let command = Command {
        player: if owner < 8 { owner } else { 0 },
        kind: walk,
    };
    let outcome = if owner < 8 {
        crate::command::execute(sim, &command, true)
    } else {
        crate::player::submit_script(sim, &command)
    };
    assert_eq!(outcome, Outcome::Accepted);
}

#[test]
fn script_path_owns_any_existing_object() {
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let own = testkit::spawn(&mut sim, COLONIST, 0, (10, 10));
    let foreign = testkit::spawn(&mut sim, COLONIST, 3, (12, 10));
    let quartered = testkit::spawn(&mut sim, COLONIST, 3, (14, 10));
    sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
    let dying = testkit::spawn(&mut sim, COLONIST, 4, (16, 10));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    let gone = testkit::spawn(&mut sim, COLONIST, 3, (18, 10));
    destroy_object(&mut sim, gone);

    // Input: only own, not quartered, not dying units of the sender.
    assert!(owns(&sim, 0, own));
    for id in [foreign, quartered, dying, gone, ObjectId::NONE] {
        assert!(!owns(&sim, 0, id), "input: {id:?}");
    }

    // Script path: every existing object, independent of the sender (E-214); not the stale ID.
    sim.origin = Origin::Script;
    sim.verdict.replace(Default::default());
    for id in [own, foreign, quartered, dying] {
        assert!(owns(&sim, 0, id), "script path: {id:?}");
    }
    assert!(owns(&sim, 7, foreign), "the sender does not matter");
    assert!(!owns(&sim, 0, gone), "stale ID");
    assert!(!owns(&sim, 0, ObjectId::NONE), "no ID");
    // The verdict counts as before: four let through (without the second call for `foreign`).
    let verdict = sim.verdict.replace(Default::default());
    assert_eq!(verdict.passed, 5);
    assert!(verdict.filtered.is_some());
}

/// A foreign unit (slot 3) marches; the level script executes `actions`. Return value: state after one
/// step, the unit and the script messages of the step.
fn walking_foreign_unit(actions: &[&str]) -> (Rules, SimState, ObjectId, Vec<ScriptTrace>) {
    let rules = rules_for(actions);
    let mut state = started(&rules);
    let unit = {
        let mut sim = Sim::new(&mut state, &rules);
        let unit = testkit::spawn(&mut sim, COLONIST, 3, (10, 10));
        name_ids(&mut sim, "feind", &[unit]);
        start_walking(&mut sim, unit);
        unit
    };
    let traces = traces(step(&mut state, &rules, &[]));
    (rules, state, unit, traces)
}

#[test]
fn script_halt_stops_a_foreign_unit_and_is_traced() {
    // Counter-check: a script without halt lets the unit march.
    let (rules, mut state, unit, _) = walking_foreign_unit(&["n12;ohne"]);
    {
        let sim = Sim::new(&mut state, &rules);
        assert!(moving(&sim, unit), "the unit marches");
    }

    // The input of player 0 must not halt this unit (A8) ...
    {
        let mut sim = Sim::new(&mut state, &rules);
        let outcome = crate::command::execute(
            &mut sim,
            &Command {
                player: 0,
                kind: CommandKind::Halt { units: vec![unit] },
            },
            true,
        );
        assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
        assert!(moving(&sim, unit));
    }

    // ... the script does: a command `0x3C`, accepted, modules aborted, block executed.
    let (rules, mut state, unit, traces) = walking_foreign_unit(&["n191;feind"]);
    assert_eq!(
        commands(&traces),
        [&ScriptTrace::Command {
            container: LEVEL_CONTAINER_ID,
            block: 0,
            action: 191,
            code: 0x3C,
            outcome: Outcome::Accepted,
        }],
        "{traces:?}"
    );
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
        "{traces:?}"
    );
    assert!(state.script.level.as_ref().unwrap().blocks.is_empty());
    let sim = Sim::new(&mut state, &rules);
    assert!(!moving(&sim, unit), "modules aborted");
}

#[test]
fn script_commands_leave_no_entry_in_the_command_log() {
    // Script commands run in the script tick of the simulation and are never in the command list that the
    // host passes to the step and keeps in the log (E-162): a run in which the script sends a command yields
    // the same checksum from the start and an empty log.
    let level = script(&[(
        &[],
        &["n151;3;1100;feind;area feld", "n191;feind", "n12;fertig"],
    )]);
    let rules = super::tests::rules_with_areas(&level, &[("feld", [2, 2, 5, 5])]);
    let setup = super::tests::setup(true);
    let mut state = new_match(&rules, &setup).expect("match with script");
    let events = step(&mut state, &rules, &[]);
    let all = traces(events);
    let issued = commands(&all);
    assert_eq!(issued.len(), 1);
    assert!(matches!(
        issued[0],
        ScriptTrace::Command {
            action: 191,
            code: 0x3C,
            outcome: Outcome::Accepted,
            ..
        }
    ));
    let replay = Replay {
        start: crate::setup::start_package(&rules, &setup),
        commands: Vec::new(),
    };
    let at = replay.play(&rules, 1).expect("replay");
    assert_eq!(checksum(&at), checksum(&state));
}

#[test]
fn script_halt_on_a_stale_id_is_rejected_and_traced() {
    let rules = rules_for(&["n191;feind"]);
    let mut state = started(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        let live = testkit::spawn(&mut sim, COLONIST, 3, (10, 10));
        let gone = testkit::spawn(&mut sim, COLONIST, 3, (12, 10));
        destroy_object(&mut sim, gone);
        name_ids(&mut sim, "feind", &[gone, live]);
    }
    let traces = traces(step(&mut state, &rules, &[]));
    let outcomes: Vec<_> = traces
        .iter()
        .filter_map(|t| match t {
            ScriptTrace::Command {
                code: 0x3C,
                outcome,
                ..
            } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes.len(), 2, "{traces:?}");
    assert!(matches!(outcomes[0], Outcome::Rejected(_)), "{outcomes:?}");
    assert_eq!(outcomes[1], &Outcome::Accepted);
}

#[test]
fn script_command_sender_rule() {
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let animal = testkit::spawn(&mut sim, COLONIST, 8, (10, 10));
    let owned5 = testkit::spawn(&mut sim, COLONIST, 5, (12, 10));
    let gone = testkit::spawn(&mut sim, COLONIST, 2, (14, 10));
    destroy_object(&mut sim, gone);
    let tree = testkit::spawn(&mut sim, COLONIST, 9, (16, 10));

    // Slot 8 (animals), 9 and 10 are no senders: slot 0, the command is not rejected.
    assert_eq!(sender(&sim, &[animal]), 0);
    assert_eq!(sender(&sim, &[tree]), 0);
    assert_eq!(sender(&sim, &[owned5]), 5);
    // The first existing unit decides (the stale ID does not count) ...
    assert_eq!(sender(&sim, &[gone, owned5]), 5);
    // ... even if it is an animal.
    assert_eq!(sender(&sim, &[animal, owned5]), 0);
    // Nothing present: slot 0.
    assert_eq!(sender(&sim, &[]), 0);
    assert_eq!(sender(&sim, &[gone, ObjectId::NONE]), 0);

    let mut out = Vec::new();
    let from = sender(&sim, &[animal]);
    issue(
        &mut sim,
        &mut out,
        from,
        CommandKind::Halt {
            units: vec![animal],
        },
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, 0x3C);
    assert_eq!(out[0].outcome, Outcome::Accepted);
    assert_eq!(sim.origin, Origin::External, "origin restored");
}

#[test]
fn script_halt_does_not_depend_on_the_sender() {
    // The sender serves only the log; no effect derives from it.
    let rules = testkit::rules();
    let run = |player: u8| {
        let mut state = testkit::state(&rules);
        let unit = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = testkit::spawn(&mut sim, COLONIST, 3, (10, 10));
            start_walking(&mut sim, unit);
            unit
        };
        step(&mut state, &rules, &[]);
        let mut sim = Sim::new(&mut state, &rules);
        assert!(moving(&sim, unit), "the unit marches");
        let mut out = Vec::new();
        issue(
            &mut sim,
            &mut out,
            player,
            CommandKind::Halt { units: vec![unit] },
        );
        assert_eq!(out[0].outcome, Outcome::Accepted, "sender {player}");
        assert!(!moving(&sim, unit), "sender {player}");
        checksum(&state)
    };
    let reference = run(0);
    for player in 1..8 {
        assert_eq!(run(player), reference, "sender {player}");
    }
}

#[test]
fn unknown_name_sends_nothing_and_traces_nothing() {
    let rules = rules_for(&["n191;gibtsnicht", "n12;danach"]);
    let mut state = started(&rules);
    let before = checksum(&state);
    let traces = traces(step(&mut state, &rules, &[]));
    assert!(commands(&traces).is_empty(), "{traces:?}");
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
        "{traces:?}"
    );
    // The action counts as executed: the block continues and ends (`m_bDeleteMe`).
    let level = state.script.level.as_ref().unwrap();
    assert!(level.blocks.is_empty());
    assert!(level.switches.contains_key("danach"));
    assert_ne!(before, checksum(&state), "the step keeps counting");
}

// --- Moving, trains, gates (actions 8, 153, 154, 163-166, 181, 188, 373-376, 390) ---

use super::actions::{Issued, MsgValue, ScriptMsg};
use super::command::{
    Look, dispatch_command, plan_move_area, plan_move_named, plan_move_to_object, plan_turn_named,
};
use crate::group::{self, NO_GROUP};
use crate::math::atan2_deg;
use crate::message::Posted;
use crate::motion::NO_DIRECTION;
use crate::world::{object_event_bits, set_object_event_bit};

/// Rules with areas `areas` (name -> `[x0, z0, x1, z1]`) on top of the command world.
fn rules_with_areas(areas: &[(&str, [i32; 4])]) -> Rules {
    let mut rules = testkit::rules();
    rules.scripts = testing::rules_with("", &[], areas);
    rules
}

/// Finished unit of the slot `owner` on the tile `tile`.
fn unit(sim: &mut Sim, owner: i32, tile: (i32, i32)) -> ObjectId {
    testkit::spawn(sim, COLONIST, owner, tile)
}

/// Execute a command action with the value slots `values`.
fn call(sim: &mut Sim, number: i32, values: Vec<MsgValue>) -> Vec<Issued> {
    dispatch_command(sim, &ScriptMsg { number, values }).expect("command action")
}

fn txt(s: &str) -> MsgValue {
    MsgValue::Text(s.to_owned())
}

fn bit_1(sim: &Sim, id: ObjectId) -> bool {
    object_event_bits(sim, id) & 2 != 0
}

/// The most recently queued command envelope of the unit (the message to the walk module).
fn last_walk_message(sim: &Sim, unit: ObjectId) -> crate::message::Message {
    match sim.object(unit).unwrap().fifo.back() {
        Some(Posted::Command { env, .. }) => env.msg.clone(),
        other => panic!("no command in the FIFO: {other:?}"),
    }
}

/// Fixed-point world position of the tile center.
fn fixed_centre(c: i32) -> i32 {
    (c << 12) + 0x800
}

fn grid(sim: &Sim, id: ObjectId) -> (i32, i32) {
    let g = sim.object(id).unwrap().grid;
    (g.x.0, g.z.0)
}

#[test]
fn move_153_sends_one_group_move_with_raw_count_and_clears_bit_1() {
    let rules = rules_with_areas(&[("ziel", [20, 10, 22, 12]), ("blick", [20, 20, 22, 22])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = unit(&mut sim, 3, (10, 10));
    let b = unit(&mut sim, 4, (12, 10));
    let gone = unit(&mut sim, 3, (14, 10));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "trupp", &[a, gone, b]);
    let ids = [a, gone, b];
    let preset = |sim: &mut Sim| {
        for id in ids {
            set_object_event_bit(sim, id, 1, true);
            assert!(bit_1(sim, id));
        }
    };

    // The command: all three IDs (n raw), center of "ziel", direction toward the center of "blick".
    preset(&mut sim);
    let plan = plan_move_named(&mut sim, "trupp", "ziel", "blick", 2);
    assert_eq!(
        plan,
        [CommandKind::Move {
            units: vec![a, gone, b],
            x: fixed_centre(21),
            z: fixed_centre(11),
            target: -1,
            formation: 0,
            direction: atan2_deg(10 << 12, 0),
            spread: 0,
            flags: 2,
        }]
    );
    assert!(
        ids.iter().all(|&id| !bit_1(&sim, id)),
        "bit 1 cleared for all three, also the dead one"
    );

    // Executed: one command; n = 3 selects the group path, the group consists of the two living ones.
    preset(&mut sim);
    let out = call(&mut sim, 153, vec![txt("trupp"), txt("ziel"), txt("blick")]);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].code, 0x04);
    assert!(matches!(out[0].outcome, Outcome::Partial(_)), "{out:?}");
    assert!(ids.iter().all(|&id| !bit_1(&sim, id)));
    let g = group::group_of(&mut sim, a);
    assert_ne!(g, NO_GROUP, "group path");
    assert_eq!(group::group_of(&mut sim, b), g);
    assert_eq!(group::member_count(&sim, g), 2);
    assert_eq!(
        group::find(&sim, g).unwrap().anchor,
        (fixed_centre(21), fixed_centre(11), atan2_deg(10 << 12, 0))
    );
    assert_eq!(sim.origin, Origin::External);
}

#[test]
fn move_with_one_dead_id_still_takes_the_group_path() {
    // E-215: n is the raw number of IDs. With the dead ID n = 2 (group path, one group of the one living
    // unit); the input filters first and would go individually as n = 1.
    let rules = rules_with_areas(&[("ziel", [20, 10, 22, 12]), ("blick", [20, 20, 22, 22])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let live = unit(&mut sim, 0, (10, 10));
    let gone = unit(&mut sim, 0, (12, 10));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "trupp", &[gone, live]);
    let out = call(&mut sim, 153, vec![txt("trupp"), txt("ziel"), txt("blick")]);
    assert_eq!(out.len(), 1);
    assert_ne!(
        group::group_of(&mut sim, live),
        NO_GROUP,
        "script path: group"
    );

    // Counter-check: the same list as input of the owner (player 0) is filtered first.
    let other = unit(&mut sim, 0, (10, 14));
    let kind = CommandKind::Move {
        units: vec![gone, other],
        x: fixed_centre(21),
        z: fixed_centre(11),
        target: -1,
        formation: 0,
        direction: -1,
        spread: 0,
        flags: 2,
    };
    crate::command::execute(&mut sim, &Command { player: 0, kind }, true);
    assert_eq!(
        group::group_of(&mut sim, other),
        NO_GROUP,
        "input: individually"
    );
    assert!(
        !sim.object(other).unwrap().fifo.is_empty(),
        "input: goes out anyway"
    );
}

#[test]
fn move_flags_follow_the_action() {
    // 153 -> 2, 188 -> 3, 374 -> 0, 376 -> 1 (walk flags: bit 1 auto attack, bit 0 run); for one unit (n = 1)
    // they show in the message to the walk module: walk flags `0x201`, `| 0x20` for bit 1, `| 0x100` for
    // bit 0 (and the code `0x100` instead of `0x200` when running).
    let rules = rules_with_areas(&[("ziel", [20, 10, 22, 12]), ("blick", [20, 20, 22, 22])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = unit(&mut sim, 0, (10, 10));
    name_ids(&mut sim, "solo", &[u]);
    for (number, flags, walk, code) in [
        (153, 2, 0x221, 0x200),
        (188, 3, 0x321, 0x100),
        (374, 0, 0x201, 0x200),
        (376, 1, 0x301, 0x100),
    ] {
        sim.object_mut(u).unwrap().fifo.clear();
        let out = call(
            &mut sim,
            number,
            vec![txt("solo"), txt("ziel"), txt("blick")],
        );
        assert_eq!(out.len(), 1, "action {number}");
        assert_eq!(out[0].outcome, Outcome::Accepted, "action {number}");
        let msg = last_walk_message(&sim, u);
        assert_eq!(msg.code, code, "action {number}");
        assert_eq!(msg.args[5], walk, "action {number}");
        let plan = plan_move_named(&mut sim, "solo", "ziel", "blick", flags);
        assert!(matches!(plan[0], CommandKind::Move { flags: f, .. } if f == flags));
    }
}

#[test]
fn unknown_area_or_name_sends_nothing_and_keeps_bit_1() {
    // Ghidra pre-check: `FUN_00436710` looks up both areas before clearing bit 1.
    let rules = rules_with_areas(&[("ziel", [20, 10, 22, 12])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = unit(&mut sim, 0, (10, 10));
    name_ids(&mut sim, "solo", &[u]);
    set_object_event_bit(&mut sim, u, 1, true);
    assert!(
        call(
            &mut sim,
            153,
            vec![txt("solo"), txt("ziel"), txt("nirgends")]
        )
        .is_empty()
    );
    assert!(
        call(
            &mut sim,
            153,
            vec![txt("solo"), txt("nirgends"), txt("ziel")]
        )
        .is_empty()
    );
    assert!(bit_1(&sim, u), "unknown area: bit 1 stays");
    assert!(call(&mut sim, 153, vec![txt("keiner"), txt("ziel"), txt("ziel")]).is_empty());
    assert!(sim.object(u).unwrap().fifo.is_empty());
}

#[test]
fn same_centre_gives_direction_minus_1() {
    // E-151: same center of both areas -> direction -1 (a real target direction, tolerance 0): the unit
    // turns at the target to ~0 degrees (+x) although it comes from the north.
    let rules = rules_with_areas(&[("feld", [12, 10, 12, 10]), ("anders", [12, 2, 12, 2])]);
    let mut state = testkit::state(&rules);
    let u = {
        let mut sim = Sim::new(&mut state, &rules);
        let u = unit(&mut sim, 0, (12, 14));
        name_ids(&mut sim, "solo", &[u]);
        let plan = plan_move_named(&mut sim, "solo", "feld", "feld", 2);
        assert!(
            matches!(plan[0], CommandKind::Move { direction: -1, .. }),
            "{plan:?}"
        );
        // Different centers: the angle (here to the north, -90 degrees).
        let plan = plan_move_named(&mut sim, "solo", "feld", "anders", 2);
        assert!(
            matches!(plan[0], CommandKind::Move { direction, .. } if direction == atan2_deg(-8 << 12, 0)),
            "{plan:?}"
        );
        call(&mut sim, 153, vec![txt("solo"), txt("feld"), txt("feld")]);
        assert_eq!(last_walk_message(&sim, u).args[3], -1);
        u
    };
    for _ in 0..400 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    let o = sim.object(u).unwrap();
    assert_eq!(o.tile, (12, 10), "arrived");
    assert!(
        o.direction.0.abs() <= 2 << 12,
        "facing ~0 degrees: {}",
        o.direction.0
    );

    // Counter-check: 8/373 send `NO_DIRECTION`; the facing stays at the march direction (-z = -90 degrees).
    let rules = rules_with_areas(&[("start", [12, 14, 12, 14]), ("feld", [12, 10, 12, 10])]);
    let mut state = testkit::state(&rules);
    let u = {
        let mut sim = Sim::new(&mut state, &rules);
        let u = unit(&mut sim, 0, (12, 14));
        let args = vec![
            MsgValue::Int(0),
            MsgValue::Int(1100),
            txt("start"),
            txt("feld"),
        ];
        assert_eq!(call(&mut sim, 373, args).len(), 1);
        assert_eq!(last_walk_message(&sim, u).args[3], NO_DIRECTION);
        u
    };
    for _ in 0..400 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    let o = sim.object(u).unwrap();
    assert_eq!(o.tile, (12, 10), "arrived");
    assert!(
        (o.direction.0 - (-90 << 12)).abs() <= 10 << 12,
        "facing stays at -90 degrees: {}",
        o.direction.0
    );
}

#[test]
fn turn_154_moves_each_unit_onto_itself() {
    let rules = rules_with_areas(&[("blick", [10, 10, 10, 10])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let far = unit(&mut sim, 3, (14, 10));
    let on_spot = unit(&mut sim, 0, (10, 10));
    let gone = unit(&mut sim, 0, (16, 10));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "trupp", &[far, gone, on_spot]);
    for id in [far, on_spot] {
        set_object_event_bit(&mut sim, id, 1, true);
    }
    let centre = (fixed_centre(10), fixed_centre(10));
    assert_eq!(
        grid(&sim, on_spot),
        centre,
        "test setup: position = area center"
    );

    let plan = plan_turn_named(&sim, "trupp", Look::Area("blick"));
    let (fx, fz) = grid(&sim, far);
    assert_eq!(
        plan,
        [
            CommandKind::Move {
                units: vec![far],
                x: fx,
                z: fz,
                target: -1,
                formation: 0,
                direction: atan2_deg(centre.1 - fz, centre.0 - fx),
                spread: -1,
                flags: 0,
            },
            CommandKind::Move {
                units: vec![on_spot],
                x: centre.0,
                z: centre.1,
                target: -1,
                formation: 0,
                direction: -1,
                spread: -1,
                flags: 0,
            },
        ],
        "a dead ID is skipped"
    );
    let out = call(&mut sim, 154, vec![txt("trupp"), txt("blick")]);
    assert_eq!(out.len(), 2);
    assert!(
        out.iter()
            .all(|i| i.code == 0x04 && i.outcome == Outcome::Accepted)
    );
    assert!(
        bit_1(&sim, far) && bit_1(&sim, on_spot),
        "154 does not clear bit 1"
    );
    // Each unit stands alone (n = 1): no group path.
    assert_eq!(group::group_of(&mut sim, far), NO_GROUP);

    // 390: look-at target = position of the first object of the second name (here the unit on the center).
    name_ids(&mut sim, "ziel", &[on_spot]);
    assert_eq!(
        plan_turn_named(&sim, "trupp", Look::Object("ziel")),
        plan,
        "390 looks at the first object like 154 at the area center"
    );
    assert_eq!(
        call(&mut sim, 390, vec![txt("trupp"), txt("ziel")]).len(),
        2
    );
    assert!(plan_turn_named(&sim, "trupp", Look::Object("nirgends")).is_empty());
    destroy_object(&mut sim, on_spot);
    assert!(
        plan_turn_named(&sim, "trupp", Look::Object("ziel")).is_empty(),
        "look-at target without an object: nothing"
    );
    assert!(plan_turn_named(&sim, "trupp", Look::Area("nirgends")).is_empty());
}

#[test]
fn move_181_targets_the_tile_of_the_first_object() {
    let rules = rules_with_areas(&[]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let goal = unit(&mut sim, 3, (20, 12));
    let at_goal = unit(&mut sim, 0, (20, 12));
    let away = unit(&mut sim, 0, (10, 10));
    let gone = unit(&mut sim, 0, (14, 10));
    destroy_object(&mut sim, gone);
    // The first existing unit already stands at the goal (no angle); the second supplies it.
    name_ids(&mut sim, "trupp", &[at_goal, away, gone]);
    name_ids(&mut sim, "ziel", &[goal]);
    let goal_pos = grid(&sim, goal);
    let from = grid(&sim, away);
    let expected = |flags| CommandKind::Move {
        units: vec![at_goal, away, gone],
        x: goal_pos.0,
        z: goal_pos.1,
        target: -1,
        formation: 0,
        direction: atan2_deg(goal_pos.1 - from.1, goal_pos.0 - from.0),
        spread: -1,
        flags,
    };
    for id in [at_goal, away, gone] {
        set_object_event_bit(&mut sim, id, 1, true);
    }
    assert_eq!(
        plan_move_to_object(&mut sim, "trupp", "ziel", 2),
        [expected(2)]
    );
    assert!(!bit_1(&sim, at_goal) && !bit_1(&sim, away));
    assert!(bit_1(&sim, gone), "bit 1 only for existing IDs");
    // 375: the same, with flags 0.
    assert_eq!(
        plan_move_to_object(&mut sim, "trupp", "ziel", 0),
        [expected(0)]
    );

    for number in [181, 375] {
        let out = call(&mut sim, number, vec![txt("trupp"), txt("ziel")]);
        assert_eq!(out.len(), 1, "action {number}");
        assert_eq!(out[0].code, 0x04);
    }

    // All stand at the goal: direction -1. Target without an object / unknown name: nothing.
    name_ids(&mut sim, "dort", &[goal, at_goal]);
    let plan = plan_move_to_object(&mut sim, "dort", "ziel", 2);
    assert!(
        matches!(plan[0], CommandKind::Move { direction: -1, .. }),
        "{plan:?}"
    );
    assert!(plan_move_to_object(&mut sim, "trupp", "nirgends", 2).is_empty());
    assert!(plan_move_to_object(&mut sim, "keiner", "ziel", 2).is_empty());
    destroy_object(&mut sim, goal);
    assert!(plan_move_to_object(&mut sim, "trupp", "ziel", 2).is_empty());
}

#[test]
fn move_8_selects_by_filter_and_uses_no_direction() {
    let rules = rules_with_areas(&[("von", [8, 8, 12, 12]), ("nach", [30, 20, 32, 22])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = unit(&mut sim, 0, (10, 10));
    let b = unit(&mut sim, 0, (11, 11));
    let _foreign = unit(&mut sim, 3, (9, 9));
    let _outside = unit(&mut sim, 0, (30, 30));
    for id in [a, b] {
        set_object_event_bit(&mut sim, id, 1, true);
    }
    let mut both = vec![a, b];
    both.sort_by_key(|u| u.0);
    let expected = |flags| CommandKind::Move {
        units: both.clone(),
        x: fixed_centre(31),
        z: fixed_centre(21),
        target: -1,
        formation: 0,
        direction: NO_DIRECTION,
        spread: 0,
        flags,
    };
    // 8 -> flags 2, 373 -> flags 0; only the own units of the type in the area (filter as for 6).
    for flags in [2, 0] {
        let mut plan = plan_move_area(&mut sim, 0, 1100, "von", "nach", flags);
        let CommandKind::Move { units, .. } = &mut plan[0] else {
            panic!("no walk: {plan:?}")
        };
        units.sort_by_key(|u| u.0);
        assert_eq!(plan, [expected(flags)]);
    }
    assert!(bit_1(&sim, a) && bit_1(&sim, b), "no clearing of bit 1");

    let args = || {
        vec![
            MsgValue::Int(0),
            MsgValue::Int(1100),
            txt("von"),
            txt("nach"),
        ]
    };
    let out = call(&mut sim, 8, args());
    assert_eq!(out.len(), 1, "one command for both");
    assert_eq!(out[0].code, 0x04);
    assert_ne!(group::group_of(&mut sim, a), NO_GROUP);
    assert_eq!(group::group_of(&mut sim, a), group::group_of(&mut sim, b));
    assert_eq!(call(&mut sim, 373, args()).len(), 1);

    // No hits, unknown area or unknown filter ID: nothing.
    assert!(plan_move_area(&mut sim, 5, 1100, "von", "nach", 2).is_empty());
    assert!(plan_move_area(&mut sim, 0, 1100, "nirgends", "nach", 2).is_empty());
    assert!(plan_move_area(&mut sim, 0, 1100, "von", "nirgends", 2).is_empty());
    assert!(plan_move_area(&mut sim, 0, 999, "von", "nach", 2).is_empty());
}

#[test]
fn move_of_a_foreign_unit_is_accepted() {
    // E-214: the script path does not ask for the owner; the verdict is `Accepted`, the unit walks.
    let rules = rules_with_areas(&[("ziel", [20, 10, 22, 12]), ("blick", [20, 20, 22, 22])]);
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    for owner in [3, 8] {
        let u = unit(&mut sim, owner, (10, 10 + owner));
        name_ids(&mut sim, "fremd", &[u]);
        let out = call(&mut sim, 153, vec![txt("fremd"), txt("ziel"), txt("blick")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].outcome, Outcome::Accepted, "owner {owner}");
        assert!(!sim.object(u).unwrap().fifo.is_empty(), "owner {owner}");
    }
    // A dying command recipient counts too (n = 1, module 0).
    let dying = unit(&mut sim, 2, (14, 14));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    name_ids(&mut sim, "sterbend", &[dying]);
    let out = call(
        &mut sim,
        153,
        vec![txt("sterbend"), txt("ziel"), txt("blick")],
    );
    assert_eq!(out[0].outcome, Outcome::Accepted);
}

/// Train and gate worlds for the scripts: the rules of the respective test kit with the script rules `level`.
fn with_script(mut rules: Rules, level: &str, areas: &[(&str, [i32; 4])]) -> Rules {
    rules.scripts = testing::rules_with(level, &[], areas);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    rules
}

/// Start the level script of the rules in `state`.
fn start_level(state: &mut SimState, rules: &Rules) {
    let mut sim = Sim::new(state, rules);
    let setup = ScriptSetup {
        enabled: true,
        time_per_char: 55,
        ..Default::default()
    };
    start_match(&mut sim, &setup, 7).expect("script start");
}

#[test]
fn train_163_clears_bit_1_and_waits_for_bit_0() {
    use crate::module::train::status as train_status;
    use crate::module::train::testkit as tk;
    use crate::module::train::train;

    let level = script(&[(&[], &["w163;zug;area stadt", "n12;fertig"])]);
    let rules = with_script(tk::rules(), &level, &[("stadt", [34, 19, 36, 21])]);
    let mut state = tk::state(&rules);
    start_level(&mut state, &rules);
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        tk::lay_track(&mut sim, (10, 20), (49, 20));
        // A foreign locomotive (slot 3): the script path does not ask for the owner.
        let loco = tk::spawn(&mut sim, tk::LOCO, 3, (30, 20));
        name_ids(&mut sim, "zug", &[loco]);
        set_object_event_bit(&mut sim, loco, 1, true);
        loco
    };
    let traces = traces(step(&mut state, &rules, &[]));
    assert_eq!(
        commands(&traces),
        [&ScriptTrace::Command {
            container: LEVEL_CONTAINER_ID,
            block: 0,
            action: 163,
            code: 0x29,
            outcome: Outcome::Accepted,
        }],
        "{traces:?}"
    );
    {
        let sim = Sim::new(&mut state, &rules);
        let t = train(&sim, loco).expect("train module");
        assert_eq!(t.target, Some((35, 20)), "tile center of the area");
        assert_ne!(t.status & train_status::FORWARD, 0, "drives forward");
        assert!(!bit_1(&sim, loco), "bit 1 cleared");
        let level = state.script.level.as_ref().unwrap();
        assert!(!level.switches.contains_key("fertig"), "the block waits");
        assert!(!level.blocks.is_empty());
    }

    // Bit 1 ("idle") does not end the wait: it waits for bit 0 (E-148).
    {
        let mut sim = Sim::new(&mut state, &rules);
        set_object_event_bit(&mut sim, loco, 1, true);
    }
    step(&mut state, &rules, &[]);
    assert!(
        !state
            .script
            .level
            .as_ref()
            .unwrap()
            .switches
            .contains_key("fertig")
    );
    {
        let mut sim = Sim::new(&mut state, &rules);
        set_object_event_bit(&mut sim, loco, 0, true);
    }
    for _ in 0..3 {
        step(&mut state, &rules, &[]);
    }
    assert!(
        state
            .script
            .level
            .as_ref()
            .unwrap()
            .switches
            .contains_key("fertig"),
        "bit 0 resumes the block"
    );
}

#[test]
fn train_164_drives_backwards_and_unknown_names_do_nothing() {
    use crate::module::train::status as train_status;
    use crate::module::train::testkit as tk;
    use crate::module::train::train;

    let rules = with_script(tk::rules(), "", &[("stadt", [34, 19, 36, 21])]);
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    tk::lay_track(&mut sim, (10, 20), (49, 20));
    let loco = tk::spawn(&mut sim, tk::LOCO, 0, (30, 20));
    let colonist = tk::spawn(&mut sim, tk::COLONIST, 0, (20, 20));
    name_ids(&mut sim, "zug", &[loco]);
    name_ids(&mut sim, "kein_zug", &[colonist]);
    let out = call(&mut sim, 164, vec![txt("zug"), txt("stadt")]);
    assert_eq!(
        out,
        [Issued {
            code: 0x2A,
            outcome: Outcome::Accepted
        }]
    );
    let t = train(&sim, loco).unwrap();
    assert_ne!(t.status & train_status::BACKWARD, 0);
    assert_eq!(t.target, Some((35, 20)));

    // Unknown name or area: nothing. An object without a train module: rejected with a reason.
    assert!(call(&mut sim, 163, vec![txt("niemand"), txt("stadt")]).is_empty());
    assert!(call(&mut sim, 163, vec![txt("zug"), txt("nirgends")]).is_empty());
    let out = call(&mut sim, 163, vec![txt("kein_zug"), txt("stadt")]);
    assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");
}

/// Gate state `(TOROPEN, TORCLOSE)` of the building module.
fn gate_state(sim: &Sim, id: ObjectId) -> (bool, bool) {
    use crate::module::building::{self, flags};
    let m = sim.module(id, building::INDEX).expect("building module");
    (
        m.base.has_flags(flags::TOROPEN),
        m.base.has_flags(flags::TORCLOSE),
    )
}

#[test]
fn gate_165_opens_a_foreign_gate_and_id_0_is_rejected_with_reason() {
    use crate::module::building::fort_testkit as fk;

    let rules = with_script(fk::rules(), "", &[]);
    let mut state = fk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // A gate of slot 7 (foreign for every player of the test world), finished and open.
    let gate = fk::spawn_at(&mut sim, fk::GATE, 7, (10.0 * 128.0, 40.5 * 128.0), 0.0);
    name_ids(&mut sim, "tor", &[gate]);
    assert_eq!(gate_state(&sim, gate), (true, false));

    // The input of another player must not do that (A8) ...
    let close = CommandKind::CloseGate { gate };
    assert!(matches!(
        crate::command::execute(
            &mut sim,
            &Command {
                player: 0,
                kind: close
            },
            true
        ),
        Outcome::Rejected(_)
    ));
    assert_eq!(gate_state(&sim, gate), (true, false));

    // ... the script does: 166 closes, 165 opens again.
    let out = call(&mut sim, 166, vec![txt("tor")]);
    assert_eq!(
        out,
        [Issued {
            code: 0x1A,
            outcome: Outcome::Accepted
        }]
    );
    assert_eq!(gate_state(&sim, gate), (false, true));
    let out = call(&mut sim, 165, vec![txt("tor")]);
    assert_eq!(
        out,
        [Issued {
            code: 0x19,
            outcome: Outcome::Accepted
        }]
    );
    assert_eq!(gate_state(&sim, gate), (true, false));
    // Opening an open gate: no effect, but accepted (the state is no check for the script).
    let out = call(&mut sim, 165, vec![txt("tor")]);
    assert_eq!(out[0].outcome, Outcome::Accepted);

    // A dying gate (`m_IsDead`) closes too (the original does not check death).
    crate::module::building::building_mut(&mut sim, gate)
        .expect("building")
        .is_dead = true;
    assert_eq!(
        call(&mut sim, 166, vec![txt("tor")])[0].outcome,
        Outcome::Accepted
    );
    assert_eq!(gate_state(&sim, gate), (false, true));

    // Unknown name: the original sends ID 0, the handler finds nothing; here: rejected with a reason.
    let before = checksum(sim.state);
    for number in [165, 166] {
        let out = call(&mut sim, number, vec![txt("niemand")]);
        assert_eq!(out.len(), 1, "action {number}");
        let Outcome::Rejected(why) = &out[0].outcome else {
            panic!("{out:?}")
        };
        assert!(why.contains("not present"), "{why}");
    }
    assert_eq!(checksum(sim.state), before, "ID 0 changes nothing");
}

#[test]
fn gate_165_on_a_building_without_gate_flag_works_like_the_original() {
    // E-216: the original opens and closes every building (`PropertyBuilding_OpenDoor` checks only "already
    // open"); the reimplementation adopts that (deviation from the original: rejecting only where the
    // module cannot cope with it). Check: no panic, the state loads, the world keeps running.
    use crate::module::building::fort_testkit as fk;

    let rules = with_script(fk::rules(), "", &[]);
    let mut state = fk::state(&rules);
    let hq = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq = fk::spawn(&mut sim, fk::HQ, 0, (44, 20));
        let colonist = fk::spawn(&mut sim, fk::COLONIST, 0, (48, 20));
        name_ids(&mut sim, "hq", &[hq]);
        name_ids(&mut sim, "kolonist", &[colonist]);
        for number in [166, 165, 166] {
            let out = call(&mut sim, number, vec![txt("hq")]);
            assert_eq!(out[0].outcome, Outcome::Accepted, "action {number}");
        }
        // No building: no effect, rejected with a reason (the original sends to an object without a module).
        let out = call(&mut sim, 165, vec![txt("kolonist")]);
        assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");
        hq
    };
    crate::save::check_state(&state).expect("savegame loads");
    for _ in 0..10 {
        step(&mut state, &rules, &[]);
    }
    crate::save::check_state(&state).expect("savegame loads after the run");
    assert!(Sim::new(&mut state, &rules).object(hq).is_some());
}

// --- Attack, stance, halt, kill, stop, special commands (167, 174, 179, 189, 190, 193, 34) ---

use crate::combat::testkit as ck;
use crate::command::QueuedCommand;
use crate::module::ability_testkit as ak;
use crate::save::SaveGame;
use crate::setup::MatchSetup;
use crate::setup::start_package;
use crate::state::SimEvent;

/// The messages in the FIFO of the unit: `(code, arguments, abort)`.
fn fifo_messages(sim: &Sim, id: ObjectId) -> Vec<(u32, Vec<i32>, bool)> {
    sim.object(id)
        .unwrap()
        .fifo
        .iter()
        .map(|p| match p {
            Posted::Message { msg, abort } => (msg.code, msg.args.clone(), *abort),
            Posted::Command { env, abort } => (env.msg.code, env.msg.args.clone(), *abort),
        })
        .collect()
}

/// An entry in the command queue of the unit (filler that a command with `direct` is to empty).
fn fill_queue(sim: &mut Sim, unit: ObjectId) {
    crate::command::enqueue(
        sim,
        unit,
        QueuedCommand {
            cyclic: false,
            command: Command {
                player: 0,
                kind: CommandKind::Halt { units: vec![unit] },
            },
        },
    );
    assert_eq!(sim.object(unit).unwrap().commands.len(), 1);
}

fn queue_len(sim: &Sim, unit: ObjectId) -> usize {
    sim.object(unit).unwrap().commands.len()
}

fn accepted(code: u32) -> Issued {
    Issued {
        code,
        outcome: Outcome::Accepted,
    }
}

#[test]
fn attack_167_uses_the_first_target_id_and_unknown_target_sends_nothing() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 3, (4, 4));
    let b = ck::spawn(&mut sim, ck::PIKEMAN, 4, (6, 4));
    let t1 = ck::spawn(&mut sim, ck::WARRIOR, 5, (20, 4));
    let t2 = ck::spawn(&mut sim, ck::WARRIOR, 5, (22, 4));
    name_ids(&mut sim, "trupp", &[a, b]);
    name_ids(&mut sim, "ziel", &[t1, t2]);
    fill_queue(&mut sim, a);
    sim.state.events.clear();

    // One command for all attackers; the target is only the first ID of the second name (modifier 0).
    let out = call(&mut sim, 167, vec![txt("trupp"), txt("ziel")]);
    assert_eq!(out, [accepted(0x11)]);
    for unit in [a, b] {
        assert_eq!(
            fifo_messages(&sim, unit),
            [(0x111, vec![t1.as_i32(), 0], true)],
            "unit {unit:?}"
        );
    }
    // Direct: acknowledgment 0xE and emptying the queue per attacker.
    assert_eq!(queue_len(&sim, a), 0);
    assert!(sim.state.events.contains(&SimEvent::UnitResponse {
        unit: a,
        action: 14,
        owner: 3,
        guid: ck::SABRE
    }));

    // Counter-check: the input of another player must not send foreign units (A8).
    let before = fifo_messages(&sim, a);
    let outcome = crate::command::execute(
        &mut sim,
        &Command {
            player: 0,
            kind: CommandKind::Attack {
                units: vec![a],
                target: t2,
                pos: None,
                mods: 0,
            },
        },
        true,
    );
    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
    assert_eq!(fifo_messages(&sim, a), before);

    // Unknown target or unknown attackers: no command, no message.
    assert!(call(&mut sim, 167, vec![txt("trupp"), txt("niemand")]).is_empty());
    assert!(call(&mut sim, 167, vec![txt("niemand"), txt("ziel")]).is_empty());

    // The target is not checked: even a stale ID goes out.
    let gone = ck::spawn(&mut sim, ck::WARRIOR, 5, (24, 4));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "alt", &[gone]);
    let out = call(&mut sim, 167, vec![txt("trupp"), txt("alt")]);
    assert_eq!(out, [accepted(0x11)]);
    assert_eq!(
        fifo_messages(&sim, b).last(),
        Some(&(0x111, vec![gone.as_i32(), 0], true))
    );
}

#[test]
fn behavior_179_per_id_and_mode_5_is_rejected_with_reason() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 2, (4, 4));
    let hq = ck::spawn(&mut sim, ck::HQ, 3, (12, 12));
    let gone = ck::spawn(&mut sim, ck::SABRE, 2, (6, 4));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "trupp", &[a, hq, gone]);

    // One command per ID (buildings report the mode, units attack mode and walk behavior); the stale ID is
    // rejected, the others continue.
    let out = call(&mut sim, 179, vec![MsgValue::Int(2), txt("trupp")]);
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(out[0], accepted(0x12));
    assert_eq!(out[1], accepted(0x12));
    assert!(matches!(out[2].outcome, Outcome::Rejected(_)), "{out:?}");
    assert_eq!(crate::module::attack::attack(&sim, a).unwrap().mode, 2);
    assert_eq!(
        crate::messaging::send(&mut sim, hq, &crate::message::Message::new(0x3603, &[])),
        2
    );

    // Counter-check: the input of another player does not change the foreign unit (A8).
    ck::run(
        &mut sim,
        0,
        CommandKind::SetBehavior {
            units: vec![a],
            mode: 4,
        },
    );
    assert_eq!(crate::module::attack::attack(&sim, a).unwrap().mode, 2);

    // Stance 5 (and -1) does not exist (A7): every ID is rejected with a reason, nothing changes.
    let before = checksum(sim.state);
    for mode in [5, -1] {
        let out = call(&mut sim, 179, vec![MsgValue::Int(mode), txt("trupp")]);
        assert_eq!(out.len(), 3, "stance {mode}");
        for issued in &out {
            let Outcome::Rejected(why) = &issued.outcome else {
                panic!("{out:?}")
            };
            assert!(why.contains("stance"), "{why}");
            assert!(why.contains(&mode.to_string()), "{why}");
        }
    }
    assert_eq!(checksum(sim.state), before);
    // Unknown name: nothing.
    assert!(call(&mut sim, 179, vec![MsgValue::Int(1), txt("niemand")]).is_empty());
}

#[test]
fn kill_34_sends_one_die_for_the_area_selection() {
    let rules = with_script(ck::rules(), "", &[("feld", [2, 2, 8, 8])]);
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let in1 = ck::spawn(&mut sim, ck::SABRE, 3, (3, 3));
    let in2 = ck::spawn(&mut sim, ck::SABRE, 3, (5, 6));
    let other_type = ck::spawn(&mut sim, ck::PIKEMAN, 3, (4, 4));
    let other_owner = ck::spawn(&mut sim, ck::SABRE, 4, (6, 6));
    let outside = ck::spawn(&mut sim, ck::SABRE, 3, (20, 20));
    sim.state.events.clear();

    // Reference player 3, filter "Saber fighter": only its saber fighters in the area, in one command.
    let out = call(
        &mut sim,
        34,
        vec![
            MsgValue::Int(3),
            MsgValue::Int(ck::SABRE as i32),
            txt("feld"),
        ],
    );
    assert_eq!(out, [accepted(0x10)]);
    for hit in [in1, in2] {
        assert_eq!(
            fifo_messages(&sim, hit),
            [(0x110, vec![], true)],
            "hit {hit:?}"
        );
        assert!(
            sim.state
                .events
                .contains(&SimEvent::Deselected { object: hit })
        );
    }
    for miss in [other_type, other_owner, outside] {
        assert!(fifo_messages(&sim, miss).is_empty(), "{miss:?}");
    }

    // No hits, unknown area or unknown filter ID: no command.
    let none = |sim: &mut Sim, player: i32, guid: i32, area: &str| {
        call(
            sim,
            34,
            vec![MsgValue::Int(player), MsgValue::Int(guid), txt(area)],
        )
    };
    assert!(none(&mut sim, 5, ck::SABRE as i32, "feld").is_empty());
    assert!(none(&mut sim, 3, ck::SABRE as i32, "nirgends").is_empty());
    assert!(none(&mut sim, 3, 5, "feld").is_empty());
}

#[test]
fn kill_193_reaches_a_dying_foreign_unit() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        let dying = ck::spawn(&mut sim, ck::SABRE, 4, (4, 4));
        sim.object_mut(dying).unwrap().status |= status::DIED;
        let foreign = ck::spawn(&mut sim, ck::SABRE, 5, (6, 4));
        let hq = ck::spawn(&mut sim, ck::HQ, 5, (14, 14));
        name_ids(&mut sim, "opfer", &[dying, foreign, hq]);
        sim.state.events.clear();

        // One command per ID, without ownership, building or death check (E-149, E-214).
        let out = call(&mut sim, 193, vec![txt("opfer")]);
        assert_eq!(out, [accepted(0x10), accepted(0x10), accepted(0x10)]);
        for id in [dying, foreign, hq] {
            assert!(
                sim.state
                    .events
                    .contains(&SimEvent::Deselected { object: id }),
                "deselected: {id:?}"
            );
        }
        assert_eq!(fifo_messages(&sim, foreign), [(0x110, vec![], true)]);

        // Counter-check: the input may only kill own, living units (A8).
        let outcome = crate::command::execute(
            &mut sim,
            &Command {
                player: 0,
                kind: CommandKind::Die {
                    units: vec![dying, foreign],
                },
            },
            true,
        );
        assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");

        // Unknown name: nothing. Stale ID: rejected with a reason.
        assert!(call(&mut sim, 193, vec![txt("niemand")]).is_empty());
        let gone = ck::spawn(&mut sim, ck::SABRE, 5, (8, 4));
        destroy_object(&mut sim, gone);
        name_ids(&mut sim, "alt", &[gone]);
        let out = call(&mut sim, 193, vec![txt("alt")]);
        assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");
    }
    // The step afterwards copes with the death of dying units and buildings.
    for _ in 0..20 {
        step(&mut state, &rules, &[]);
    }
}

/// Let all units run (march to tile (2, 2)) and fast-forward one step so that the walk module is
/// working.
fn walk_all(state: &mut SimState, rules: &Rules, units: &[ObjectId]) {
    {
        let mut sim = Sim::new(state, rules);
        for &u in units {
            start_walking(&mut sim, u);
        }
    }
    for _ in 0..3 {
        step(state, rules, &[]);
    }
    let sim = Sim::new(state, rules);
    for &u in units {
        assert!(moving(&sim, u), "{u:?} marches");
    }
}

#[test]
fn halt_174_aborts_all_owners_but_not_dying_quartered_or_hidden() {
    let rules = with_script(ck::rules(), "", &[("feld", [8, 8, 20, 20])]);
    let mut state = ck::state(&rules);
    let spawn = |state: &mut SimState, slot: i32, tile: (i32, i32)| {
        let mut sim = Sim::new(state, &rules);
        ck::spawn(&mut sim, ck::SABRE, slot, tile)
    };
    let own = spawn(&mut state, 0, (10, 10));
    let foreign = spawn(&mut state, 3, (12, 10));
    let animal = spawn(&mut state, 8, (14, 10));
    let dying = spawn(&mut state, 4, (10, 12));
    let quartered = spawn(&mut state, 4, (12, 12));
    let hidden_slot = spawn(&mut state, 5, (14, 12));
    let revived = spawn(&mut state, 5, (16, 12));
    let outside = spawn(&mut state, 3, (40, 40));
    let all = [
        own,
        foreign,
        animal,
        dying,
        quartered,
        hidden_slot,
        revived,
        outside,
    ];
    walk_all(&mut state, &rules, &all);
    let mut sim = Sim::new(&mut state, &rules);
    sim.object_mut(dying).unwrap().status |= status::DIED;
    sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
    crate::script::world::hide::deactivate(&mut sim, 5).unwrap();
    // A single re-shown object of the hidden slot is in the collision manager and is stopped (162 shows
    // it).
    name_ids(&mut sim, "zurueck", &[revived]);
    crate::script::world::hide::named(&mut sim, "zurueck", true);
    fill_queue(&mut sim, own);

    let mut msg = ScriptMsg {
        number: 174,
        values: vec![txt("feld")],
    };
    let done = super::world::dispatch_world(&mut sim, &mut msg);
    assert_eq!(done, Some(Ok(())), "174 is a world action");

    // All owners (also opponents and animals) stand still ...
    for u in [own, foreign, animal, revived] {
        assert!(!moving(&sim, u), "stopped: {u:?}");
    }
    // ... dying, quartered, hidden ones and objects outside do not.
    for u in [dying, quartered, hidden_slot, outside] {
        assert!(moving(&sim, u), "untouched: {u:?}");
    }
    // No emptying of the queue (E-156).
    assert_eq!(queue_len(&sim, own), 1);

    // Unknown area: nothing.
    let before = checksum(sim.state);
    let mut msg = ScriptMsg {
        number: 174,
        values: vec![txt("nirgends")],
    };
    assert_eq!(
        super::world::dispatch_world(&mut sim, &mut msg),
        Some(Ok(()))
    );
    assert_eq!(checksum(sim.state), before);
}

/// Evoke world (evoker, archer, assassin, ranged fighter, colonist, building).
fn ability_world() -> (Rules, SimState) {
    let rules = ak::evoke_rules();
    let state = ak::state(&rules);
    (rules, state)
}

#[test]
fn special_189_codes_follow_the_table() {
    use crate::module::{arming, assassin, evoke_animal, ghosts, masking};
    let (rules, mut state) = ability_world();
    let mut sim = Sim::new(&mut state, &rules);
    // All carriers belong to slot 1 and have no prerequisite unlocked: no menu, no lock, no module - the
    // script path sends anyway (E-214).
    let ghost = ak::spawn(&mut sim, ak::GHOST_EVOKER, 1, (10, 10));
    let animal = ak::spawn(&mut sim, ak::ANIMAL_EVOKER, 1, (12, 10));
    let missionary = ak::spawn(&mut sim, ak::MISSIONARY, 1, (14, 10));
    let archer = ak::spawn(&mut sim, ak::ARCHER, 1, (16, 10));
    let assassin_unit = ak::spawn(&mut sim, ak::ASSASSIN, 1, (18, 10));
    let colonist = ak::spawn(&mut sim, ak::COLONIST, 1, (20, 10));
    let ranged = ak::spawn(&mut sim, ak::RANGED, 1, (22, 10));
    let fixture = [
        (ghost, 0),
        (animal, 1),
        (missionary, 4),
        (archer, 5),
        (assassin_unit, 8),
        (colonist, 7),
        (ranged, 7),
    ];
    for (unit, code) in fixture {
        name_ids(&mut sim, &format!("u{}", unit.0), &[unit]);
        fill_queue(&mut sim, unit);
        let out = call(
            &mut sim,
            189,
            vec![MsgValue::Int(code), txt(&format!("u{}", unit.0))],
        );
        assert_eq!(out.len(), 1, "code {code}: {out:?}");
        assert_eq!(out[0].outcome, Outcome::Accepted, "code {code}: {out:?}");
        assert_eq!(queue_len(&sim, unit), 0, "code {code}: empty (direct)");
    }
    let msgs = |sim: &Sim, u| fifo_messages(sim, u);
    // Code 0 ghost (button 436), 1 wolf (kind 0, button 441), 4 banishment (button 439), 5 camouflage, 8 dig in.
    assert_eq!(
        msgs(&sim, ghost),
        [(ghosts::evoke::msg::EVOKE, vec![436], true)]
    );
    assert_eq!(
        msgs(&sim, animal),
        [(evoke_animal::msg::CAST, vec![0, 441], true)]
    );
    assert_eq!(
        msgs(&sim, missionary),
        [(ghosts::banish::msg::START, vec![439], true)]
    );
    assert_eq!(msgs(&sim, archer), [(masking::msg::START, vec![], true)]);
    assert_eq!(
        msgs(&sim, assassin_unit),
        [(assassin::msg::GO_DOWN, vec![], true)]
    );
    // Code 7 arm: only without the ranged-combat module `0x13` switched on.
    assert_eq!(
        msgs(&sim, colonist),
        [(arming::msg::GO_TO_ARMORY, vec![], true)]
    );
    assert!(
        msgs(&sim, ranged).is_empty(),
        "ranged combat on: no message"
    );

    // Codes 2, 3 (bear, cat), 6 (uncamouflage), 9 (dig out).
    for (code, unit, expected) in [
        (2, animal, (evoke_animal::msg::CAST, vec![1, 442], true)),
        (3, animal, (evoke_animal::msg::CAST, vec![2, 443], true)),
        (6, archer, (masking::msg::END, vec![], true)),
        (9, assassin_unit, (assassin::msg::GO_UP, vec![], true)),
    ] {
        let before = msgs(&sim, unit).len();
        let out = call(
            &mut sim,
            189,
            vec![MsgValue::Int(code), txt(&format!("u{}", unit.0))],
        );
        assert_eq!(out.len(), 1, "code {code}");
        // Uncamouflage reads the count as ID 1; this object (slot 0) does not exist here: the handler ignores
        // that like the original, the reimplementation reports it as partial success.
        if code == 6 {
            assert!(
                matches!(out[0].outcome, Outcome::Partial(_)),
                "code {code}: {out:?}"
            );
        } else {
            assert_eq!(out[0].outcome, Outcome::Accepted, "code {code}");
        }
        assert_eq!(msgs(&sim, unit)[before..], [expected], "code {code}");
    }

    // Code 10 and higher (and negative): nothing.
    for code in [10, 11, -1] {
        let name = format!("u{}", ghost.0);
        assert!(call(&mut sim, 189, vec![MsgValue::Int(code), txt(&name)]).is_empty());
    }
    // Unknown name: nothing.
    assert!(call(&mut sim, 189, vec![MsgValue::Int(0), txt("niemand")]).is_empty());

    // Counter-check: the same unit for its owner without unlocking (no menu button) and for a stranger - the
    // input rejects both (A8).
    for player in [1u8, 0] {
        let outcome = crate::command::execute(
            &mut sim,
            &Command {
                player,
                kind: CommandKind::EvokeGhost {
                    unit: ghost,
                    button: 436,
                },
            },
            true,
        );
        assert!(
            matches!(outcome, Outcome::Rejected(_)),
            "{player}: {outcome:?}"
        );
    }
    assert_eq!(msgs(&sim, ghost).len(), 1, "no further message");
}

#[test]
fn unmask_189_code_6_reads_the_count_as_id() {
    use crate::module::masking;
    let (rules, mut state) = ability_world();
    let mut sim = Sim::new(&mut state, &rules);
    // The first object of the slot table of slot 0 (index 0, generation 1) is the pseudo ID 1.
    let first = ak::spawn(&mut sim, ak::ARCHER, 0, (10, 10));
    assert_eq!(first, ObjectId(1), "first object of slot 0");
    let theirs = ak::spawn(&mut sim, ak::ARCHER, 1, (30, 10));
    name_ids(&mut sim, "feind", &[theirs]);
    fill_queue(&mut sim, first);

    // `{0x1E, 2, 1, id}` (E-222): the original reads the count 1 as the first ID; without an ownership
    // check it hits `first`, no matter whom it belongs to - and the named unit takes effect too.
    let out = call(&mut sim, 189, vec![MsgValue::Int(6), txt("feind")]);
    assert_eq!(out, [accepted(0x1E)]);
    assert_eq!(
        fifo_messages(&sim, first),
        [(masking::msg::END, vec![], true)]
    );
    assert_eq!(
        fifo_messages(&sim, theirs),
        [(masking::msg::END, vec![], true)]
    );
    assert_eq!(
        queue_len(&sim, first),
        0,
        "direct: their queue is empty too"
    );

    // Counter-check: as input of slot 1, A8 filters out the foreign pseudo ID.
    let before = fifo_messages(&sim, first).len();
    crate::command::execute(
        &mut sim,
        &Command {
            player: 1,
            kind: CommandKind::Unmask {
                units: vec![theirs],
            },
        },
        true,
    );
    assert_eq!(fifo_messages(&sim, first).len(), before);
}

#[test]
fn special_190_codes_follow_the_table() {
    use crate::module::gather::{harvest, lumber, mining};
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let worker = ck::spawn(&mut sim, COLONIST, 3, (4, 4));
    let worker2 = ck::spawn(&mut sim, COLONIST, 4, (5, 4));
    let hq = ck::spawn(&mut sim, ck::HQ, 5, (30, 30));
    let farm = ck::spawn(&mut sim, testkit::FARM, 5, (40, 30));
    let mine = ck::spawn(&mut sim, testkit::MINE, 5, (50, 30));
    let house = ck::spawn(&mut sim, testkit::HOUSE, 5, (30, 40));
    let tree = ck::spawn(&mut sim, testkit::TREE, 9, (20, 20));
    let enemy = ck::spawn(&mut sim, ck::SABRE, 5, (26, 4));
    name_ids(&mut sim, "arbeiter", &[worker, worker2]);
    for (name, id) in [
        ("hq", hq),
        ("farm", farm),
        ("mine", mine),
        ("haus", house),
        ("baum", tree),
        ("feind", enemy),
    ] {
        name_ids(&mut sim, name, &[id]);
    }
    let run190 = |sim: &mut Sim, code: i32, target: &str| {
        call(
            sim,
            190,
            vec![MsgValue::Int(code), txt("arbeiter"), txt(target)],
        )
    };

    // Code 2 harvest at a foreign farm: one command per unit, the acknowledgment and the emptying follow.
    fill_queue(&mut sim, worker);
    sim.state.events.clear();
    let out = run190(&mut sim, 2, "farm");
    assert_eq!(out, [accepted(0x07), accepted(0x07)]);
    for w in [worker, worker2] {
        assert_eq!(
            fifo_messages(&sim, w),
            [(harvest::msg::START, vec![farm.as_i32()], true)]
        );
    }
    assert_eq!(queue_len(&sim, worker), 0);
    assert!(sim.state.events.contains(&SimEvent::UnitResponse {
        unit: worker,
        action: 12,
        owner: 3,
        guid: COLONIST
    }));
    // Counter-check: the input must not harvest at a foreign farm (A8).
    let outcome = crate::command::execute(
        &mut sim,
        &Command {
            player: 3,
            kind: CommandKind::Harvest {
                farm,
                units: vec![worker],
            },
        },
        true,
    );
    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
    assert_eq!(fifo_messages(&sim, worker).len(), 1);

    // Code 1 gold at a foreign mine; code 4 attack (modifier 0); code 0 construct at a foreign building.
    let out = run190(&mut sim, 1, "mine");
    assert_eq!(out, [accepted(0x03), accepted(0x03)]);
    assert_eq!(
        fifo_messages(&sim, worker).last(),
        Some(&(mining::msg::START, vec![mine.as_i32()], true))
    );
    let out = run190(&mut sim, 4, "feind");
    assert_eq!(out, [accepted(0x11), accepted(0x11)]);
    assert_eq!(
        fifo_messages(&sim, worker).last(),
        Some(&(0x111, vec![enemy.as_i32(), 0], true))
    );
    let out = run190(&mut sim, 0, "hq");
    assert_eq!(out, [accepted(0x02), accepted(0x02)]);
    assert_eq!(
        fifo_messages(&sim, worker).last().map(|m| m.0),
        Some(crate::module::construct::msg::CONSTRUCT)
    );

    // Code 3 deliver: load kind 0 to 2 is handed in (also at a house that does not accept it); the original
    // ignores kind 3 (no command, no emptying).
    {
        let w = sim.object_mut(worker).unwrap();
        w.load_kind = crate::module::gather::load::WOOD;
        w.load_amount = 5;
        let w2 = sim.object_mut(worker2).unwrap();
        w2.load_kind = 3;
        w2.load_amount = 5;
    }
    let len2 = fifo_messages(&sim, worker2).len();
    fill_queue(&mut sim, worker2);
    let out = run190(&mut sim, 3, "haus");
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0], accepted(0x09));
    assert!(matches!(out[1].outcome, Outcome::Rejected(_)), "{out:?}");
    assert_eq!(
        fifo_messages(&sim, worker).last(),
        Some(&(lumber::msg::DELIVER, vec![house.as_i32(), 1], true))
    );
    assert_eq!(
        fifo_messages(&sim, worker2).len(),
        len2,
        "load kind 3 ignored"
    );
    assert_eq!(queue_len(&sim, worker2), 1, "and not emptied");

    // Code 6 wood: the cyclic sequence "next tree" carries the slot of the unit (not that of the first
    // name) so that it continues later without an ownership check.
    let out = run190(&mut sim, 6, "baum");
    assert_eq!(out, [accepted(0x06), accepted(0x06)]);
    assert_eq!(
        fifo_messages(&sim, worker2).last(),
        Some(&(lumber::msg::START, vec![tree.as_i32()], true))
    );
    let entry = sim
        .object(worker2)
        .unwrap()
        .commands
        .last()
        .cloned()
        .unwrap();
    assert!(entry.cyclic);
    assert_eq!(entry.command.player, 4, "slot of the unit");

    // Code 7 and higher, negative codes, unknown target and unit names: nothing.
    assert!(run190(&mut sim, 7, "hq").is_empty());
    assert!(run190(&mut sim, 8, "hq").is_empty());
    assert!(run190(&mut sim, -1, "hq").is_empty());
    assert!(run190(&mut sim, 2, "niemand").is_empty());
    assert!(
        call(
            &mut sim,
            190,
            vec![MsgValue::Int(2), txt("niemand"), txt("farm")]
        )
        .is_empty()
    );
}

/// The state loads, saves and loads again with the same checksum.
fn assert_saves_and_loads(state: &SimState, rules: &Rules, what: &str) {
    crate::save::check_state(state).unwrap_or_else(|e| panic!("{what}: {e}"));
    let save = SaveGame {
        start: start_package(rules, &MatchSetup::default()),
        state: state.clone(),
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(checksum(&loaded.state), checksum(state), "{what}");
    crate::save::check_state(&loaded.state).unwrap_or_else(|e| panic!("{what} (loaded): {e}"));
}

/// Command actions 167, 179, 190 (code 0 to 6), 193 and 34 against the target `target` with the units
/// `actors`; then 40 steps run, and the state loads.
fn hammer_economy(
    rules: &Rules,
    state: &mut SimState,
    actors: &[ObjectId],
    target: ObjectId,
    what: &str,
) {
    let mut issued = Vec::new();
    {
        let mut sim = Sim::new(state, rules);
        name_ids(&mut sim, "akteure", actors);
        name_ids(&mut sim, "ziel", &[target]);
        issued.extend(call(&mut sim, 167, vec![txt("akteure"), txt("ziel")]));
        issued.extend(call(&mut sim, 179, vec![MsgValue::Int(3), txt("akteure")]));
        for code in 0..=6 {
            issued.extend(call(
                &mut sim,
                190,
                vec![MsgValue::Int(code), txt("akteure"), txt("ziel")],
            ));
        }
        issued.extend(call(&mut sim, 193, vec![txt("ziel")]));
        issued.extend(call(&mut sim, 193, vec![txt("akteure")]));
        issued.extend(call(
            &mut sim,
            34,
            vec![MsgValue::Int(-1), MsgValue::Int(1100), txt("alles")],
        ));
    }
    // The commands really arrive (not all are discarded because of the target).
    assert!(
        issued.iter().any(|i| i.outcome == Outcome::Accepted),
        "{what}: {issued:?}"
    );
    for _ in 0..40 {
        step(state, rules, &[]);
    }
    assert_saves_and_loads(state, rules, what);
}

#[test]
fn script_commands_survive_any_target() {
    // 1. Economy and combat: every target against units of all kinds.
    let rules = with_script(ck::rules(), "", &[("alles", [0, 0, 63, 63])]);
    let kinds = [
        "baum",
        "fremdes_gebaeude",
        "baustelle",
        "sterbend",
        "einquartiert",
        "veraltet",
        "id0",
        "tier",
    ];
    for kind in kinds {
        let mut state = ck::state(&rules);
        let (actors, target) = {
            let mut sim = Sim::new(&mut state, &rules);
            let worker = ck::spawn(&mut sim, COLONIST, 0, (4, 4));
            let foreign = ck::spawn(&mut sim, ck::SABRE, 3, (6, 4));
            let dying = ck::spawn(&mut sim, ck::SABRE, 4, (8, 4));
            sim.object_mut(dying).unwrap().status |= status::DIED;
            let hq = ck::spawn(&mut sim, ck::HQ, 4, (30, 30));
            let tree = ck::spawn(&mut sim, testkit::TREE, 9, (20, 20));
            let gone = ck::spawn(&mut sim, ck::SABRE, 4, (10, 4));
            destroy_object(&mut sim, gone);
            let target = match kind {
                "baum" => tree,
                "fremdes_gebaeude" => hq,
                "baustelle" => testkit::site(&mut sim, testkit::HOUSE, 4, (40, 40)),
                "sterbend" => dying,
                "einquartiert" => {
                    let u = ck::spawn(&mut sim, COLONIST, 4, (12, 4));
                    sim.object_mut(u).unwrap().status |= status::QUARTERED;
                    u
                }
                "veraltet" => gone,
                "id0" => ObjectId(0),
                _ => ck::spawn(&mut sim, ck::WOLF, 8, (16, 4)),
            };
            // Also actors without a unit type: building, tree, ID 0, stale ID.
            (
                vec![worker, foreign, dying, hq, tree, gone, ObjectId(0)],
                target,
            )
        };
        if target == ObjectId(0) {
            // `first_id` ignores the ID 0 (original: "unknown"): the name is an entry "0".
            let mut sim = Sim::new(&mut state, &rules);
            name_ids(&mut sim, "ziel", &[target]);
            assert!(call(&mut sim, 167, vec![txt("akteure"), txt("ziel")]).is_empty());
        }
        hammer_economy(&rules, &mut state, &actors, target, kind);
    }

    // 1b. A really quartered colonist (in the quarters of the headquarters) as actor and as target; the
    // headquarters itself as the target of `Die`.
    {
        let mut state = ck::state(&rules);
        let (worker, hq) = {
            let mut sim = Sim::new(&mut state, &rules);
            let hq = ck::spawn(&mut sim, ck::HQ, 0, (30, 30));
            let worker = ck::spawn(&mut sim, COLONIST, 0, (26, 30));
            ck::run(
                &mut sim,
                0,
                CommandKind::Quarter {
                    container: hq,
                    units: vec![worker],
                },
            );
            (worker, hq)
        };
        let n = testkit::run_until(&mut state, &rules, 600, |sim| {
            sim.object(worker)
                .is_some_and(|o| o.status & status::QUARTERED != 0)
        });
        assert!(n > 0);
        hammer_economy(
            &rules,
            &mut state,
            &[worker, hq],
            worker,
            "really quartered",
        );
        let mut state = ck::state(&rules);
        let (worker, hq) = {
            let mut sim = Sim::new(&mut state, &rules);
            let hq = ck::spawn(&mut sim, ck::HQ, 0, (30, 30));
            let worker = ck::spawn(&mut sim, COLONIST, 0, (26, 30));
            (worker, hq)
        };
        hammer_economy(&rules, &mut state, &[worker], hq, "headquarters as target");
    }

    // 2. Abilities: every kind of carrier with the codes 0 to 9.
    let (rules, mut state) = ability_world();
    {
        let mut sim = Sim::new(&mut state, &rules);
        let mut carriers = Vec::new();
        for (i, guid) in [
            ak::GHOST_EVOKER,
            ak::ANIMAL_EVOKER,
            ak::MISSIONARY,
            ak::ARCHER,
            ak::ASSASSIN,
            ak::COLONIST,
            ak::RANGED,
            ak::SHIPYARD,
            ak::MONASTERY,
        ]
        .into_iter()
        .enumerate()
        {
            carriers.push(ak::spawn(
                &mut sim,
                guid,
                (i % 2) as i32,
                (8 + 4 * i as i32, 10),
            ));
        }
        let dying = ak::spawn(&mut sim, ak::ARCHER, 1, (10, 30));
        sim.object_mut(dying).unwrap().status |= status::DIED;
        let quartered = ak::spawn(&mut sim, ak::COLONIST, 1, (14, 30));
        sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
        let gone = ak::spawn(&mut sim, ak::COLONIST, 1, (18, 30));
        destroy_object(&mut sim, gone);
        carriers.extend([dying, quartered, gone, ObjectId(0)]);
        name_ids(&mut sim, "traeger", &carriers);
        for code in 0..=9 {
            let out = call(&mut sim, 189, vec![MsgValue::Int(code), txt("traeger")]);
            assert_eq!(out.len(), carriers.len(), "Code {code}");
        }
    }
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
    }
    assert_saves_and_loads(&state, &rules, "abilities");

    // 3. Gates: a construction site can also be opened and closed.
    use crate::module::building::fort_testkit as fk;
    let rules = with_script(fk::rules(), "", &[]);
    let mut state = fk::state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        let mut desc =
            crate::world::ObjectDesc::new(fk::GATE, 7, [10.0 * 128.0, 0.0, 40.5 * 128.0]);
        desc.rotation = [0.0, 0.0, 0.0];
        desc.flags |= crate::world::desc_flags::ROT_Y;
        let site = crate::world::create_object(&mut sim, &mut desc).expect("construction site");
        name_ids(&mut sim, "baustelle", &[site]);
        // It really is a construction site: building module present, but not finished.
        assert!(sim.object(site).is_some_and(|o| o.has_module(3, false)));
        assert!(!crate::command::validate::is_finished(&sim, site));
        for number in [165, 166, 165] {
            let out = call(&mut sim, number, vec![txt("baustelle")]);
            assert_eq!(out.len(), 1, "action {number}");
            assert_eq!(out[0].outcome, Outcome::Accepted, "action {number}");
            assert!(
                !crate::command::validate::is_finished(&sim, site),
                "action {number}: stays a construction site"
            );
        }
        // The gate state follows the last command (open), even without a finished building.
        assert_eq!(gate_state(&sim, site), (true, false));
    }
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
    }
    assert_saves_and_loads(&state, &rules, "gate at construction site");

    // 4. Quarter, boat, horse, dismount: containers without a quarters module, foreign and unfinished
    // quarters, already mounted horses, quartered units (arrival and effect per case, see the helper
    // functions).
    hammer_quartering();
    hammer_riding();
}

// --- Arrival and work at the target (module phases) ---

/// Targets for the runs of the special commands; each lies next to the actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Tree,
    ForeignHq,
    Site,
    ForeignFarm,
    ForeignMine,
    House,
    Dying,
    Stale,
    Animal,
}

const TARGETS: [Target; 9] = [
    Target::Tree,
    Target::ForeignHq,
    Target::Site,
    Target::ForeignFarm,
    Target::ForeignMine,
    Target::House,
    Target::Dying,
    Target::Stale,
    Target::Animal,
];

/// What a single run has seen.
#[derive(Debug, Default)]
struct Observed {
    /// The message was out of the FIFO after a few steps (delivered).
    delivered: bool,
    /// The actor was on its way (`MOVE`).
    moved: bool,
    /// The actor once stood at most 2 tiles from the target.
    arrived: bool,
    /// Smallest distance to the target in tiles (Chebyshev) over the run.
    closest: i32,
    /// The module belonging to the action (construct, gold, wood, harvest, attack) left the idle state,
    /// independent of the march.
    module_worked: bool,
    /// State of this module and `MOVE` at the end of the run.
    end: (i32, bool),
}

/// State of the module that drives the action `number`/`code` when working on the target: construct
/// (`construct.status`), gold (`mining`), harvest (`harvest`), wood and delivering wood (`lumber`),
/// attack (`attack.status`, plus the combat status of the object). Never the march (`MOVE`).
fn module_state(sim: &Sim, unit: ObjectId, number: i32, code: i32) -> i32 {
    use crate::module::gather::{harvest::harvest, lumber::lumber, mining::mining};
    match (number, code) {
        (190, 0) => crate::module::construct::construct(sim, unit).map_or(0, |c| c.status),
        (190, 1) => mining(sim, unit).map_or(0, |m| m.gather.status),
        (190, 2) => harvest(sim, unit).map_or(0, |m| m.gather.status),
        (190, 3 | 6) => lumber(sim, unit).map_or(0, |m| m.gather.status),
        (190, 4) | (167, _) => {
            let fighting = sim
                .object(unit)
                .is_some_and(|o| o.status & status::FIGHTING != 0);
            crate::module::attack::attack(sim, unit).map_or(0, |a| a.status)
                + if fighting { 1000 } else { 0 }
        }
        _ => unreachable!("{number}/{code}"),
    }
}

/// A run: the actor (slot 0) stands at (10, 10), the target at (14, 10); the command action `number` with
/// the values `values` (actor name "akteur", target name "ziel") goes out once, without further commands;
/// then 300 steps. Afterwards the state loads.
fn reach_target(
    rules: &Rules,
    number: i32,
    code: i32,
    actor_guid: u32,
    load: Option<i32>,
    target: Target,
) -> Observed {
    const GOAL: (i32, i32) = (14, 10);
    let mut state = ck::state(rules);
    let actor = {
        let mut sim = Sim::new(&mut state, rules);
        let actor = ck::spawn(&mut sim, actor_guid, 0, (10, 10));
        if let Some(kind) = load {
            let o = sim.object_mut(actor).unwrap();
            o.load_kind = kind;
            o.load_amount = 5;
        }
        let goal = match target {
            Target::Tree => ck::spawn(&mut sim, testkit::TREE, 9, GOAL),
            Target::ForeignHq => ck::spawn(&mut sim, ck::HQ, 4, GOAL),
            Target::Site => testkit::site(&mut sim, testkit::HOUSE, 4, GOAL),
            Target::ForeignFarm => ck::spawn(&mut sim, testkit::FARM, 4, GOAL),
            Target::ForeignMine => ck::spawn(&mut sim, testkit::MINE, 4, GOAL),
            Target::House => ck::spawn(&mut sim, testkit::HOUSE, 4, GOAL),
            Target::Dying => {
                let u = ck::spawn(&mut sim, ck::SABRE, 4, GOAL);
                sim.object_mut(u).unwrap().status |= status::DIED;
                u
            }
            Target::Stale => {
                let u = ck::spawn(&mut sim, ck::SABRE, 4, GOAL);
                destroy_object(&mut sim, u);
                u
            }
            Target::Animal => ck::spawn(&mut sim, ck::WOLF, 8, GOAL),
        };
        name_ids(&mut sim, "akteur", &[actor]);
        name_ids(&mut sim, "ziel", &[goal]);
        actor
    };
    let before = {
        let sim = Sim::new(&mut state, rules);
        module_state(&sim, actor, number, code)
    };
    {
        let mut sim = Sim::new(&mut state, rules);
        let values = if number == 167 {
            vec![txt("akteur"), txt("ziel")]
        } else {
            vec![MsgValue::Int(code), txt("akteur"), txt("ziel")]
        };
        let out = call(&mut sim, number, values);
        assert_eq!(out.len(), 1, "{number}/{code} {target:?}: {out:?}");
        // Felling wood needs an existing tree (the original reads its tile): a stale target is rejected by the
        // reimplementation with a reason, without effect.
        if (number, code, target) == (190, 6, Target::Stale) {
            assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");
            return Observed::default();
        }
        assert_eq!(
            out[0].outcome,
            Outcome::Accepted,
            "{number}/{code} {target:?}"
        );
    }
    let mut seen = Observed {
        closest: i32::MAX,
        ..Observed::default()
    };
    for n in 0..300 {
        step(&mut state, rules, &[]);
        let sim = Sim::new(&mut state, rules);
        if n == 4 {
            seen.delivered = sim.object(actor).is_some_and(|o| o.fifo.is_empty());
        }
        if sim.object(actor).is_none() {
            continue;
        }
        seen.moved |= moving(&sim, actor);
        let at = sim.object(actor).map_or((-99, -99), |o| o.tile);
        let distance = (at.0 - GOAL.0).abs().max((at.1 - GOAL.1).abs());
        seen.closest = seen.closest.min(distance);
        seen.arrived |= distance <= 2;
        seen.module_worked |= module_state(&sim, actor, number, code) != before;
        seen.end = (module_state(&sim, actor, number, code), moving(&sim, actor));
    }
    assert_saves_and_loads(
        &state,
        rules,
        &format!("action {number}/{code} against {target:?}"),
    );
    seen
}

/// Expectation per action and target: `(module works, actor arrives)`, as the reimplementation shows it.
/// Where the target does not fit, the module delivers the message and ends it immediately without work
/// (construct/deliver at a non-building, wood at a non-tree, everything at a stale ID, attack on dying
/// ones). Exceptions with work without arrival: harvest at a building without a farm - the harvest module
/// asks for a place, gets no helper node `farmerNN` from the building and sends the farmer to the
/// coordinates (-1, -1); he walks to the map corner and stays in the harvest state (100) (no crash,
/// savegame loads; the original runs the same module sequence, **hypothesis**, E-214).
fn expectation(number: i32, code: i32, target: Target) -> (bool, bool) {
    use Target::*;
    let building = matches!(target, ForeignHq | Site | ForeignFarm | ForeignMine | House);
    match (number, code) {
        // Construct and repair: only a building (also a foreign construction site) attracts the construction worker.
        (190, 0) => (building, building),
        // Gold: runs to every existing target (the module does not check it until arrival).
        (190, 1) => (target != Stale, target != Stale),
        // Harvest: the same, but without a helper node the farmer does not arrive (see above).
        (190, 2) => (
            target != Stale,
            !matches!(target, ForeignHq | Site | ForeignMine | House | Stale),
        ),
        // Deliver: only to a finished building.
        (190, 3) => (building && target != Site, building && target != Site),
        // Attack: every living target; dying and stale ones are not attacked by the attack module.
        (190, 4) | (167, _) => {
            let reached = !matches!(target, Dying | Stale);
            (reached, reached)
        }
        // Wood: only a tree.
        (190, 6) => (target == Tree, target == Tree),
        _ => unreachable!("{number}/{code}"),
    }
}

#[test]
fn script_commands_reach_module_work_at_any_target() {
    // One run per action and target with the target next to the actor, without further commands, with so
    // many steps that arrival and work are reached: the message is delivered (FIFO empty), the work module
    // starts the work (or ends it immediately where the target does not fit), and the state loads
    // (`reach_target`).
    let rules = with_script(ck::rules(), "", &[]);
    let wood = Some(crate::module::gather::load::WOOD);
    // 190: construct (0), gold (1), harvest (2), deliver (3), attack (4), wood (6); plus 167.
    let cases: [(i32, i32, u32, Option<i32>); 7] = [
        (190, 0, COLONIST, None),
        (190, 1, COLONIST, None),
        (190, 2, COLONIST, None),
        (190, 3, COLONIST, wood),
        (190, 4, ck::SABRE, None),
        (190, 6, COLONIST, None),
        (167, 0, ck::SABRE, None),
    ];
    for (number, code, guid, load) in cases {
        for target in TARGETS {
            let seen = reach_target(&rules, number, code, guid, load, target);
            if (number, code, target) == (190, 6, Target::Stale) {
                continue;
            }
            let what = format!("{number}/{code} against {target:?}: {seen:?}");
            let (works, arrives) = expectation(number, code, target);
            assert!(seen.delivered, "{what}");
            // The module belonging to the action leaves the idle state (not just the march) ...
            assert_eq!(seen.module_worked, works, "{what}");
            // ... and the actor arrives at the target (at most 2 tiles) or stays where it stood.
            assert_eq!(seen.arrived, arrives, "{what}");
            if arrives {
                assert!(seen.moved && seen.closest <= 2, "{what}");
            }
        }
    }

    // Kill (193) on its own: a living foreign unit really dies, a building and a tree do not fall via `Die`;
    // the state loads afterwards.
    let mut state = ck::state(&rules);
    let (victim, hq, tree) = {
        let mut sim = Sim::new(&mut state, &rules);
        let victim = ck::spawn(&mut sim, ck::SABRE, 4, (10, 10));
        let hq = ck::spawn(&mut sim, ck::HQ, 4, (20, 20));
        let tree = ck::spawn(&mut sim, testkit::TREE, 9, (30, 30));
        name_ids(&mut sim, "opfer", &[victim, hq, tree]);
        let out = call(&mut sim, 193, vec![txt("opfer")]);
        assert_eq!(out, [accepted(0x10), accepted(0x10), accepted(0x10)]);
        (victim, hq, tree)
    };
    for _ in 0..300 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(
        sim.object(victim)
            .is_none_or(|o| o.status & status::DIED != 0),
        "the unit dies"
    );
    assert!(sim.object(hq).is_some_and(|o| o.status & status::DIED == 0));
    assert!(sim.object(tree).is_some());
    drop(sim);
    assert_saves_and_loads(&state, &rules, "killing");
}

#[test]
fn chop_continuation_stops_for_units_of_slot_8() {
    // E-224: the cyclic sequence "next tree" of 190 code 6 carries the slot of the unit (0 to 7) so that it
    // passes the ownership check as a later input. Units of slot 8 (animals; without a lumberjack module in
    // the shipped data, synthetic with it here) keep the sender 0: their sequence fails later, the unit seeks
    // no further tree - unlike the original. Slots 9 and 10 (trees, nature) accept no units: `create_object`
    // places them in slot 0.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let settler = ck::spawn(&mut sim, COLONIST, 4, (4, 4));
    let animal = ck::spawn(&mut sim, COLONIST, 8, (14, 4));
    assert_eq!(sim.object(animal).unwrap().owner(), 8);
    let others = [animal];
    let tree = ck::spawn(&mut sim, testkit::TREE, 9, (20, 20));
    let _spare = ck::spawn(&mut sim, testkit::TREE, 9, (22, 20));
    let mut all = vec![settler];
    all.extend(&others);
    name_ids(&mut sim, "holzer", &all);
    name_ids(&mut sim, "baum", &[tree]);
    let out = call(
        &mut sim,
        190,
        vec![MsgValue::Int(6), txt("holzer"), txt("baum")],
    );
    assert_eq!(out.len(), all.len(), "{out:?}");
    for unit in &all {
        let entry = sim.object(*unit).unwrap().commands.last().cloned();
        let entry = entry.unwrap_or_else(|| panic!("{unit:?}: no follow-up entry"));
        assert!(entry.cyclic);
        // The later execution (like `process_queue`: input, not `direct`).
        let before = fifo_messages(&sim, *unit).len();
        let outcome = crate::command::execute(&mut sim, &entry.command, false);
        let continued = fifo_messages(&sim, *unit).len() > before;
        if *unit == settler {
            assert_eq!(outcome, Outcome::Accepted, "settler continues the sequence");
            assert!(continued);
        } else {
            assert!(
                matches!(outcome, Outcome::Rejected(_)),
                "{unit:?}: {outcome:?}"
            );
            assert!(!continued, "{unit:?} seeks no further tree");
        }
    }
}

// --- Quarter, boat, horse; 402 (actions 334 to 338, 402) ---

use crate::module::riding::testkit as rk;

/// Riding world (colonist, horse, canoe, headquarters with quarters) with script rules.
fn riding_world() -> (Rules, SimState) {
    let rules = with_script(rk::rules(), "", &[]);
    let state = rk::state(&rules);
    (rules, state)
}

/// Acknowledgments (`SimEvent::UnitResponse`) in order: `(unit, action)`.
fn responses(sim: &Sim) -> Vec<(ObjectId, i32)> {
    sim.state
        .events
        .iter()
        .filter_map(|e| match *e {
            SimEvent::UnitResponse { unit, action, .. } => Some((unit, action)),
            _ => None,
        })
        .collect()
}

fn quartered(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.status & status::QUARTERED != 0)
}

#[test]
fn quarter_334_into_a_foreign_container_without_owner_checks() {
    let rules = with_script(ck::rules(), "", &[]);
    let mut state = ck::state(&rules);
    let (own, foreign, on_site, hq, site) = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq = ck::spawn(&mut sim, ck::HQ, 4, (30, 30));
        let site = testkit::site(&mut sim, ck::HQ, 4, (40, 40));
        // Unit of the owner (slot 4), a foreign one (slot 0) and one in front of the construction site.
        let own = ck::spawn(&mut sim, COLONIST, 4, (26, 30));
        let foreign = ck::spawn(&mut sim, COLONIST, 0, (26, 34));
        let on_site = ck::spawn(&mut sim, COLONIST, 4, (36, 40));
        for u in [own, foreign, on_site] {
            fill_queue(&mut sim, u);
        }
        name_ids(&mut sim, "trupp", &[own, foreign]);
        name_ids(&mut sim, "baustelle_trupp", &[on_site]);
        name_ids(&mut sim, "haus", &[hq]);
        name_ids(&mut sim, "rohbau", &[site]);

        // Counter-check (A8): the input sends neither foreign units nor into foreign quarters and not into a
        // construction site.
        for (player, units, container) in [(0, vec![foreign], hq), (4, vec![on_site], site)] {
            let outcome = crate::command::execute(
                &mut sim,
                &Command {
                    player,
                    kind: CommandKind::Quarter { container, units },
                },
                true,
            );
            assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
        }
        sim.state.events.clear();

        // 334: one command per ID, without ownership, finished and slot check.
        let out = call(&mut sim, 334, vec![txt("trupp"), txt("haus")]);
        assert_eq!(out, [accepted(0x13), accepted(0x13)]);
        for u in [own, foreign] {
            assert_eq!(
                fifo_messages(&sim, u),
                [(0x200, vec![hq.as_i32(), -1, 1], true)],
                "{u:?}"
            );
            assert_eq!(queue_len(&sim, u), 0, "direct: queue empty");
        }
        // 335 is an alias of 334 (case `0x6A`), also for the construction site.
        let out = call(&mut sim, 335, vec![txt("baustelle_trupp"), txt("rohbau")]);
        assert_eq!(out, [accepted(0x13)]);
        assert_eq!(
            fifo_messages(&sim, on_site),
            [(0x200, vec![site.as_i32(), -1, 1], true)]
        );
        // Acknowledgment 0xC per unit, in name order.
        assert_eq!(
            responses(&sim),
            [(own, 12), (foreign, 12), (on_site, 12)],
            "acknowledgments"
        );
        (own, foreign, on_site, hq, site)
    };
    // The owner moves in; the foreign unit arrives and is turned away by the quarters (`AcceptUnit` rejects
    // foreign units); the one in front of the construction site moves into it (E-217).
    testkit::run_until(&mut state, &rules, 600, |sim| quartered(sim, own));
    for _ in 0..200 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(quartered(&sim, own));
    assert!(!quartered(&sim, foreign), "foreign unit is turned away");
    // The original does not check whether the target is finished: the construction site also takes the unit.
    assert!(
        quartered(&sim, on_site),
        "also moves into the construction site"
    );
    let near = |u: ObjectId, c: ObjectId| {
        let (a, b) = (sim.object(u).unwrap().tile, sim.object(c).unwrap().tile);
        (a.0 - b.0).abs().max((a.1 - b.1).abs()) <= 4
    };
    assert!(near(foreign, hq), "it walked to the quarters");
    assert!(near(on_site, site), "it walked to the construction site");
    drop(sim);
    assert_saves_and_loads(&state, &rules, "quartering into foreign containers");
}

#[test]
fn quarter_334_with_unknown_container_does_nothing_and_waits_on_bit_1() {
    // E-154: the dispatcher sends the command also with container 0; the handler then does nothing (no
    // sound, no emptying). The wait for bit 1 of the first name remains (E-148); an old bit 1 ("idle") ends
    // it immediately.
    let level = script(&[(&[], &["w334;trupp;niemand", "n12;fertig"])]);
    let rules = with_script(rk::rules(), &level, &[]);
    let run = |old_bit_1: bool| {
        let mut state = rk::state(&rules);
        start_level(&mut state, &rules);
        let unit = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = rk::spawn(&mut sim, rk::COLONIST, 3, (10, 10));
            name_ids(&mut sim, "trupp", &[unit]);
            fill_queue(&mut sim, unit);
            if old_bit_1 {
                set_object_event_bit(&mut sim, unit, 1, true);
            }
            unit
        };
        let first = traces(step(&mut state, &rules, &[]));
        // One verdict per ID, without effect: "container 0 not present".
        let issued = commands(&first);
        assert_eq!(issued.len(), 1, "{first:?}");
        assert!(
            matches!(
                issued[0],
                ScriptTrace::Command {
                    action: 334,
                    code: 0x13,
                    outcome: Outcome::Rejected(_),
                    ..
                }
            ),
            "{first:?}"
        );
        assert!(
            !first
                .iter()
                .any(|t| matches!(t, ScriptTrace::Pending { .. } | ScriptTrace::Guarded { .. })),
            "{first:?}"
        );
        for _ in 0..4 {
            step(&mut state, &rules, &[]);
        }
        let done = state
            .script
            .level
            .as_ref()
            .unwrap()
            .switches
            .contains_key("fertig");
        let sim = Sim::new(&mut state, &rules);
        assert!(fifo_messages(&sim, unit).is_empty(), "nothing delivered");
        assert!(responses(&sim).is_empty(), "no sound");
        // The wait clears the bit only when it ends (query 2500).
        (done, bit_1(&sim, unit))
    };
    assert_eq!(run(false), (false, false), "without bit 1 the block waits");
    assert_eq!(
        run(true),
        (true, false),
        "an old bit 1 ends the wait immediately and is cleared"
    );

    // The command itself: no emptying of the queue (the handler returns before emptying).
    let mut state = rk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = rk::spawn(&mut sim, rk::COLONIST, 3, (10, 10));
    name_ids(&mut sim, "trupp", &[unit]);
    fill_queue(&mut sim, unit);
    let out = call(&mut sim, 334, vec![txt("trupp"), txt("niemand")]);
    assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");
    assert_eq!(queue_len(&sim, unit), 1, "no emptying");
    assert!(fifo_messages(&sim, unit).is_empty());
    assert!(responses(&sim).is_empty(), "no sound");
}

#[test]
fn board_336_with_boat_0_aborts_and_clears() {
    // E-154: 336 sends the boarding also with boat 0 - an effect, no rejection: the running modules of the
    // unit abort, it acknowledges (0xC) and the queue is empty.
    let rules = with_script(ck::rules(), "", &[]);
    let mut state = ck::state(&rules);
    let (walker, foreigner) = {
        let mut sim = Sim::new(&mut state, &rules);
        let walker = unit(&mut sim, 0, (10, 10));
        let foreigner = unit(&mut sim, 3, (12, 10));
        name_ids(&mut sim, "trupp", &[walker, foreigner]);
        (walker, foreigner)
    };
    walk_all(&mut state, &rules, &[walker, foreigner]);
    {
        let mut sim = Sim::new(&mut state, &rules);
        for u in [walker, foreigner] {
            fill_queue(&mut sim, u);
        }
        sim.state.events.clear();
        let out = call(&mut sim, 336, vec![txt("trupp"), txt("niemand")]);
        assert_eq!(out, [accepted(0x0C), accepted(0x0C)], "effect, no error");
        for u in [walker, foreigner] {
            // Instead of the ID 0 the message carries `NONE` (deviation from the original, exception A8 in
            // `riding.rs`).
            assert_eq!(
                fifo_messages(&sim, u),
                [(0x10E, vec![ObjectId::NONE.as_i32()], true)],
                "{u:?}"
            );
            assert_eq!(queue_len(&sim, u), 0);
        }
        assert_eq!(responses(&sim), [(walker, 12), (foreigner, 12)]);
    }
    // The FIFO is delivered in the tick of the unit (one step later than the script tick).
    for _ in 0..3 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(!moving(&sim, walker) && !moving(&sim, foreigner), "aborted");
    drop(sim);
    for _ in 0..60 {
        step(&mut state, &rules, &[]);
    }
    assert_saves_and_loads(&state, &rules, "boarding with boat 0");
}

#[test]
fn mount_337_swaps_the_fields() {
    // 337 `{0x0A, 3, horse = ID from name 1, 1, rider = first ID of name 2}`: one command per horse for the
    // first rider (the fields are swapped compared with 334/336, E-154).
    let (rules, mut state) = riding_world();
    let mut sim = Sim::new(&mut state, &rules);
    let (h1, h2) = (
        rk::spawn(&mut sim, rk::HORSE, 1, (14, 10)),
        rk::spawn(&mut sim, rk::HORSE, 2, (16, 10)),
    );
    // Rider foreign to both horses; the second is not "the first" and stays untouched.
    let (rider, other) = (
        rk::spawn(&mut sim, rk::COLONIST, 3, (10, 10)),
        rk::spawn(&mut sim, rk::COLONIST, 3, (10, 12)),
    );
    name_ids(&mut sim, "pferde", &[h1]);
    name_ids(&mut sim, "zwei_pferde", &[h1, h2]);
    name_ids(&mut sim, "reiter", &[rider, other]);
    for u in [rider, other] {
        fill_queue(&mut sim, u);
    }
    sim.state.events.clear();

    let out = call(&mut sim, 337, vec![txt("pferde"), txt("reiter")]);
    assert_eq!(out, [accepted(0x0A)]);
    assert_eq!(
        fifo_messages(&sim, rider),
        [(0x10D, vec![h1.as_i32()], true)],
        "the rider mounts the horse from name 1"
    );
    assert!(
        fifo_messages(&sim, other).is_empty(),
        "only the first ID of name 2"
    );
    assert_eq!(responses(&sim), [(rider, 12)]);
    assert_eq!(queue_len(&sim, rider), 0);
    assert_eq!(queue_len(&sim, other), 1);

    // Two horses: one command per horse, the rider gets both orders in succession.
    let out = call(&mut sim, 337, vec![txt("zwei_pferde"), txt("reiter")]);
    assert_eq!(out, [accepted(0x0A), accepted(0x0A)]);
    assert_eq!(
        fifo_messages(&sim, rider)[1..],
        [
            (0x10D, vec![h1.as_i32()], true),
            (0x10D, vec![h2.as_i32()], true)
        ]
    );

    // The swapped reading would be wrong: horse "reiter" would make a colonist a mount.
    let before = fifo_messages(&sim, other).len();
    let _ = call(&mut sim, 337, vec![txt("reiter"), txt("pferde")]);
    assert_eq!(
        fifo_messages(&sim, other).len(),
        before,
        "the rider is the horse h1"
    );

    // Unknown rider: the ID 0 goes out and hits nothing. Unknown horses: no command.
    let before = fifo_messages(&sim, rider).len();
    let out = call(&mut sim, 337, vec![txt("pferde"), txt("niemand")]);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(fifo_messages(&sim, rider).len(), before);
    assert!(call(&mut sim, 337, vec![txt("niemand"), txt("reiter")]).is_empty());

    // A stale horse: the original ends silently, the reimplementation rejects with a reason.
    let gone = rk::spawn(&mut sim, rk::HORSE, 1, (18, 10));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "altes_pferd", &[gone]);
    let out = call(&mut sim, 337, vec![txt("altes_pferd"), txt("reiter")]);
    assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");
    assert_eq!(fifo_messages(&sim, rider).len(), before);
}

#[test]
fn dismount_338_per_id() {
    // 338 `{0x0B, 2, 1, id}` per ID: dismount with abort, without module test; direct only empties (no sound).
    let (rules, mut state) = riding_world();
    let mut sim = Sim::new(&mut state, &rules);
    let a = rk::spawn(&mut sim, rk::COLONIST, 0, (10, 10));
    let b = rk::spawn(&mut sim, rk::COLONIST, 3, (12, 10));
    let dying = rk::spawn(&mut sim, rk::COLONIST, 4, (14, 10));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    let horse = rk::spawn(&mut sim, rk::HORSE, 3, (16, 10));
    let all = [a, b, dying, horse];
    for u in all {
        fill_queue(&mut sim, u);
    }
    name_ids(&mut sim, "alle", &all);
    sim.state.events.clear();
    let out = call(&mut sim, 338, vec![txt("alle")]);
    assert_eq!(
        out,
        [
            accepted(0x0B),
            accepted(0x0B),
            accepted(0x0B),
            accepted(0x0B)
        ],
        "one command per ID, also foreign, dying, without riding module"
    );
    for u in all {
        assert_eq!(fifo_messages(&sim, u), [(0x20D, vec![], true)], "{u:?}");
        assert_eq!(queue_len(&sim, u), 0, "{u:?}");
    }
    assert!(responses(&sim).is_empty(), "no acknowledgment sound");

    // Unknown name: no command; stale ID: rejected with a reason (original: ignores silently).
    assert!(call(&mut sim, 338, vec![txt("niemand")]).is_empty());
    let gone = rk::spawn(&mut sim, rk::COLONIST, 0, (18, 10));
    destroy_object(&mut sim, gone);
    name_ids(&mut sim, "alt", &[gone]);
    let out = call(&mut sim, 338, vec![txt("alt")]);
    assert!(matches!(out[0].outcome, Outcome::Rejected(_)), "{out:?}");

    // Counter-check (A8): the input does not send foreign units.
    let outcome = crate::command::execute(
        &mut sim,
        &Command {
            player: 0,
            kind: CommandKind::Dismount { units: vec![b] },
        },
        true,
    );
    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
}

#[test]
fn action_402_has_no_effect_but_waits() {
    // E-219 (deviation from the original): 402 crews a siege engine in the original; the reimplementation has no
    // crewing. No command, no "no effect yet" message, but the wait for bit 1 of the first name remains (case `0x51`,
    // E-148).
    let level = script(&[(&[], &["w402;trupp;geraet", "n12;fertig"])]);
    let rules = with_script(rk::rules(), &level, &[]);
    let run = |old_bit_1: bool| {
        let mut state = rk::state(&rules);
        start_level(&mut state, &rules);
        let (crew, engine) = {
            let mut sim = Sim::new(&mut state, &rules);
            let crew = rk::spawn(&mut sim, rk::COLONIST, 0, (10, 10));
            let engine = rk::spawn(&mut sim, rk::COLONIST, 0, (20, 10));
            name_ids(&mut sim, "trupp", &[crew]);
            name_ids(&mut sim, "geraet", &[engine]);
            fill_queue(&mut sim, crew);
            if old_bit_1 {
                set_object_event_bit(&mut sim, crew, 1, true);
            }
            (crew, engine)
        };
        let first = traces(step(&mut state, &rules, &[]));
        assert!(
            !first.iter().any(|t| matches!(
                t,
                ScriptTrace::Command { .. }
                    | ScriptTrace::Pending { .. }
                    | ScriptTrace::Guarded { .. }
            )),
            "{first:?}"
        );
        for _ in 0..4 {
            step(&mut state, &rules, &[]);
        }
        let done = state
            .script
            .level
            .as_ref()
            .unwrap()
            .switches
            .contains_key("fertig");
        let sim = Sim::new(&mut state, &rules);
        for u in [crew, engine] {
            assert!(fifo_messages(&sim, u).is_empty(), "{u:?}");
        }
        assert!(responses(&sim).is_empty());
        done
    };
    assert!(!run(false), "without bit 1 the block waits");
    assert!(run(true), "an old bit 1 ends the wait immediately");

    // The dispatcher does not know 402 as a command: neither command action nor world action.
    let mut state = rk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let msg = ScriptMsg {
        number: 402,
        values: vec![txt("trupp"), txt("geraet")],
    };
    assert!(super::command::dispatch_command(&mut sim, &msg).is_none());
}

// --- Robustness: containers, mounts and units of all kinds ---

/// What a run of 334/190 (code 5)/336/337/338 has seen.
#[derive(Debug, Default)]
struct RideSeen {
    /// Verdicts of the command action.
    out: Vec<Issued>,
    /// The message was out of the FIFO of the actor after a few steps (delivered).
    delivered: bool,
    /// Smallest distance of the actor to the target (Chebyshev, tiles) over the run.
    closest: i32,
    /// State of the actor at the end: quartered, mounted, in the boat.
    quartered: bool,
    mounted: bool,
    on_boat: bool,
}

/// A run: `build` sets up the world and returns actor and target; then the action `number`
/// (334/335/336/337/338, 190 with code 5) goes out once, 300 steps follow, and the state loads. Target and
/// actor are called "akteur" and "ziel"; for 337 the target is the horse (name 1), the actor the rider
/// (name 2).
fn ride_run(
    rules: &Rules,
    state: &mut SimState,
    number: i32,
    what: &str,
    build: impl FnOnce(&mut Sim) -> (ObjectId, ObjectId),
) -> RideSeen {
    let (actor, target) = {
        let mut sim = Sim::new(state, rules);
        let (actor, target) = build(&mut sim);
        name_ids(&mut sim, "akteur", &[actor]);
        name_ids(&mut sim, "ziel", &[target]);
        (actor, target)
    };
    let mut seen = RideSeen {
        closest: i32::MAX,
        ..RideSeen::default()
    };
    {
        let mut sim = Sim::new(state, rules);
        seen.out = match number {
            190 => call(
                &mut sim,
                190,
                vec![MsgValue::Int(5), txt("akteur"), txt("ziel")],
            ),
            337 => call(&mut sim, 337, vec![txt("ziel"), txt("akteur")]),
            338 => call(&mut sim, 338, vec![txt("akteur")]),
            n => call(&mut sim, n, vec![txt("akteur"), txt("ziel")]),
        };
    }
    for n in 0..300 {
        step(state, rules, &[]);
        let sim = Sim::new(state, rules);
        if n == 4 {
            seen.delivered = sim.object(actor).is_some_and(|o| o.fifo.is_empty());
        }
        let (Some(a), Some(t)) = (sim.object(actor), sim.object(target)) else {
            continue;
        };
        seen.closest = seen
            .closest
            .min((a.tile.0 - t.tile.0).abs().max((a.tile.1 - t.tile.1).abs()));
    }
    {
        let sim = Sim::new(state, rules);
        if let Some(a) = sim.object(actor) {
            seen.quartered = a.status & status::QUARTERED != 0;
            seen.mounted = a.status & status::ISMOUNTED != 0;
            seen.on_boat = a.status & status::ISMOUNTEDONBOAT != 0;
        }
    }
    assert_saves_and_loads(state, rules, &format!("{what} ({number})"));
    // If the target then drops out (also a building or mount with a rider), the state stays loadable.
    {
        let mut sim = Sim::new(state, rules);
        if sim.object(target).is_some() {
            destroy_object(&mut sim, target);
        }
    }
    for _ in 0..60 {
        step(state, rules, &[]);
    }
    assert_saves_and_loads(
        state,
        rules,
        &format!("{what} ({number}), target destroyed"),
    );
    seen
}

fn all_accepted(out: &[Issued]) -> bool {
    !out.is_empty() && out.iter().all(|i| i.outcome == Outcome::Accepted)
}

fn all_rejected(out: &[Issued]) -> bool {
    !out.is_empty()
        && out
            .iter()
            .all(|i| matches!(i.outcome, Outcome::Rejected(_)))
}

/// Quartering (334, 335, 190 code 5) into containers of all kinds (E-217): a worker (slot 0) at (10, 10),
/// the container at (14, 10). Buildings and vehicles accept the command without ownership, finished and
/// quarters check; moving in succeeds only with fitting quarters.
fn hammer_quartering() {
    use crate::module::building::destroy::destroy;
    let rules = with_script(ck::rules(), "", &[]);
    let wolf = ck::WOLF;
    #[derive(Clone, Copy, Debug)]
    enum Holder {
        OwnHq,
        ForeignHq,
        OwnSite,
        House,
        Tree,
        Colonist,
        Boat,
        Dying,
        Animal,
        Stale,
        Id0,
    }
    // (container, accepted, actor arrives, actor is quartered)
    let cases = [
        (Holder::OwnHq, true, true, true),
        // Foreign quarters: the command goes out (no ownership), the quarters turn the foreign unit away on
        // arrival (`accept_unit`).
        (Holder::ForeignHq, true, true, false),
        // Construction site: the original does not check whether the target is finished; the quarters module takes
        // it in.
        (Holder::OwnSite, true, true, true),
        // Containers without a quarters module (E-217): they walk there, the moving in fizzles.
        (Holder::House, true, true, false),
        (Holder::Tree, true, true, false),
        // Mobile containers without a quarters module: meeting point, arrival, no moving in.
        (Holder::Colonist, true, true, false),
        (Holder::Dying, true, true, false),
        (Holder::Animal, true, true, false),
        // A boat on land lies in the same walk region: meeting point on the boat, no moving in.
        (Holder::Boat, true, true, false),
        // Without an object: rejected (original: end without sound).
        (Holder::Stale, false, false, false),
        (Holder::Id0, false, false, false),
    ];
    for number in [334, 335, 190] {
        for (kind, accepted_, arrives, in_container) in cases {
            let mut state = ck::state(&rules);
            let what = format!("{number} against {kind:?}");
            let seen = ride_run(&rules, &mut state, number, &what, |sim| {
                let worker = ck::spawn(sim, COLONIST, 0, (10, 10));
                let at = (14, 10);
                let target = match kind {
                    Holder::OwnHq => ck::spawn(sim, ck::HQ, 0, at),
                    Holder::ForeignHq => ck::spawn(sim, ck::HQ, 4, at),
                    Holder::OwnSite => testkit::site(sim, ck::HQ, 0, at),
                    Holder::House => ck::spawn(sim, testkit::HOUSE, 4, at),
                    Holder::Tree => ck::spawn(sim, testkit::TREE, 9, at),
                    Holder::Colonist => ck::spawn(sim, COLONIST, 4, at),
                    Holder::Boat => ck::spawn(sim, testkit::BOAT, 4, at),
                    Holder::Dying => {
                        let u = ck::spawn(sim, ck::SABRE, 4, at);
                        sim.object_mut(u).unwrap().status |= status::DIED;
                        u
                    }
                    Holder::Animal => ck::spawn(sim, wolf, 8, at),
                    Holder::Stale => {
                        let u = ck::spawn(sim, ck::SABRE, 4, at);
                        destroy_object(sim, u);
                        u
                    }
                    Holder::Id0 => ObjectId(0),
                };
                (worker, target)
            });
            let why = format!("{what}: {seen:?}");
            if accepted_ {
                assert!(all_accepted(&seen.out), "{why}");
                assert!(seen.delivered, "{why}");
            } else if number == 190 && matches!(kind, Holder::Id0) {
                // 190 reads the target with `first_id`: ID 0 counts as "unknown", nothing goes out.
                assert!(seen.out.is_empty(), "{why}");
            } else {
                assert!(all_rejected(&seen.out), "{why}");
            }
            if arrives {
                assert!(seen.closest <= 2, "{why}");
            }
            assert_eq!(seen.quartered, in_container, "{why}");
        }
    }

    // Units of all kinds against the own quarters: only living, not quartered units with `CANQUARTER` set
    // off (class `0x40000`); foreign ones go there and are turned away. An already quartered unit stays where
    // it is.
    let mut state = ck::state(&rules);
    let (hq, other_hq, inside, healthy, foreign, dying, flagged, stale, tree, animal) = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq = ck::spawn(&mut sim, ck::HQ, 0, (30, 30));
        let other_hq = ck::spawn(&mut sim, ck::HQ, 0, (40, 30));
        let inside = ck::spawn(&mut sim, COLONIST, 0, (36, 30));
        let healthy = ck::spawn(&mut sim, COLONIST, 0, (26, 30));
        let foreign = ck::spawn(&mut sim, COLONIST, 3, (26, 34));
        let dying = ck::spawn(&mut sim, COLONIST, 0, (26, 26));
        sim.object_mut(dying).unwrap().status |= status::DIED;
        let flagged = ck::spawn(&mut sim, COLONIST, 0, (22, 30));
        sim.object_mut(flagged).unwrap().status |= status::QUARTERED;
        let stale = ck::spawn(&mut sim, COLONIST, 0, (22, 34));
        destroy_object(&mut sim, stale);
        let tree = ck::spawn(&mut sim, testkit::TREE, 9, (22, 26));
        let animal = ck::spawn(&mut sim, wolf, 8, (18, 30));
        // Really quartered: into the second quarters.
        ck::run(
            &mut sim,
            0,
            CommandKind::Quarter {
                container: other_hq,
                units: vec![inside],
            },
        );
        (
            hq, other_hq, inside, healthy, foreign, dying, flagged, stale, tree, animal,
        )
    };
    testkit::run_until(&mut state, &rules, 600, |sim| quartered(sim, inside));
    {
        let mut sim = Sim::new(&mut state, &rules);
        let squad = [
            inside,
            healthy,
            foreign,
            dying,
            flagged,
            stale,
            tree,
            hq,
            animal,
            ObjectId(0),
        ];
        name_ids(&mut sim, "trupp", &squad);
        name_ids(&mut sim, "haus", &[hq]);
        let out = call(&mut sim, 334, vec![txt("trupp"), txt("haus")]);
        assert_eq!(out.len(), squad.len(), "{out:?}");
        for (i, u) in squad.iter().enumerate() {
            let walks = [healthy, foreign, animal].contains(u);
            assert_eq!(
                sim.object(*u).map_or(0, |o| o.fifo.len()),
                usize::from(walks),
                "unit {i}: {u:?}"
            );
            assert_eq!(
                out[i].outcome == Outcome::Accepted,
                walks,
                "unit {i}: {:?}",
                out[i]
            );
        }
    }
    for _ in 0..300 {
        step(&mut state, &rules, &[]);
    }
    {
        let sim = Sim::new(&mut state, &rules);
        assert!(quartered(&sim, healthy), "the healthy unit moves in");
        assert!(!quartered(&sim, foreign), "the foreign one is turned away");
        assert!(quartered(&sim, inside), "the quartered unit stays inside");
        assert_eq!(
            sim.object(inside).map(|o| o.carrier),
            Some(other_hq),
            "in the old quarters"
        );
        assert!(!quartered(&sim, animal), "animals are turned away");
    }
    assert_saves_and_loads(&state, &rules, "units of all kinds quartered");
    // Commands to really quartered units (boat, mount, dismount, quartering into other quarters): they go
    // out, but the occupant stays in the quarters, and the state loads.
    {
        let mut sim = Sim::new(&mut state, &rules);
        name_ids(&mut sim, "drin", &[inside]);
        name_ids(&mut sim, "pferd", &[healthy]);
        let boat = call(&mut sim, 336, vec![txt("drin"), txt("pferd")]);
        let mount = call(&mut sim, 337, vec![txt("pferd"), txt("drin")]);
        let dismount = call(&mut sim, 338, vec![txt("drin")]);
        let quarter = call(&mut sim, 334, vec![txt("drin"), txt("haus")]);
        assert_eq!(boat, [accepted(0x0C)]);
        // Mounting needs the riding module (the economy world has none): the reason is in the verdict.
        assert!(
            matches!(mount[0].outcome, Outcome::Rejected(_)),
            "{mount:?}"
        );
        assert_eq!(dismount, [accepted(0x0B)]);
        // The class `CANQUARTER` excludes quartered units: the command reports the reason.
        assert!(matches!(
            quarter[0].outcome,
            Outcome::Partial(_) | Outcome::Rejected(_)
        ));
    }
    for _ in 0..100 {
        step(&mut state, &rules, &[]);
    }
    {
        let sim = Sim::new(&mut state, &rules);
        assert!(quartered(&sim, inside), "the occupant stays quartered");
        assert_eq!(sim.object(inside).map(|o| o.carrier), Some(other_hq));
    }
    assert_saves_and_loads(&state, &rules, "commands to quartered units");
    // The occupants survive the destruction of the quarters without a panic; the state loads.
    {
        let mut sim = Sim::new(&mut state, &rules);
        destroy(&mut sim, hq);
        destroy(&mut sim, other_hq);
    }
    for _ in 0..100 {
        step(&mut state, &rules, &[]);
    }
    assert_saves_and_loads(&state, &rules, "quarters destroyed");
}

/// Mounting (337), boarding (336) and dismounting (338) against mounts, boats and riders of all kinds (`Mount`
/// on an already mounted horse, commands to quartered units). Two worlds: the riding world on land
/// (colonist at (10, 10), target at (14, 10)) and a lake (`lake_rules`: warrior at (18, 30) on the shore,
/// canoe at (21, 30) in the water).
fn hammer_riding() {
    use crate::module::riding::testkit::lake_rules;
    let rules = with_script(rk::rules(), "", &[]);
    let at = (14, 10);
    let run = |number: i32, what: &str, build: &dyn Fn(&mut Sim, ObjectId) -> ObjectId| {
        let mut state = rk::state(&rules);
        ride_run(&rules, &mut state, number, what, |sim| {
            let rider = rk::spawn(sim, rk::COLONIST, 0, (10, 10));
            let target = build(sim, rider);
            (rider, target)
        })
    };

    // 337 mount. A free horse, also a foreign one (change of owner, K-224): the rider mounts.
    for (what, slot) in [("free horse", 0), ("foreign horse", 1)] {
        let seen = run(337, what, &|s, _| rk::spawn(s, rk::HORSE, slot, at));
        assert!(
            all_accepted(&seen.out) && seen.delivered,
            "{what}: {seen:?}"
        );
        assert!(seen.mounted && seen.closest <= 2, "{what}: {seen:?}");
    }
    // An already mounted horse (CARRIING): the list is empty, the circle search finds nothing; the rider
    // stays on the ground, the command reports the reason. With a free horse next to it he takes that.
    let ridden = |s: &mut Sim| {
        let dragoon = rk::spawn(s, rk::DRAGOON, 0, at);
        let horse = rk::spawn(s, rk::HORSE, 0, at);
        rk::link(s, dragoon, horse);
        horse
    };
    let seen = run(337, "mounted horse", &|s, _| ridden(s));
    assert!(!seen.mounted && !seen.out.is_empty(), "{seen:?}");
    assert!(
        matches!(
            seen.out[0].outcome,
            Outcome::Rejected(_) | Outcome::Partial(_)
        ),
        "{seen:?}"
    );
    let seen = run(337, "mounted horse, free one next to it", &|s, _| {
        let horse = ridden(s);
        rk::spawn(s, rk::HORSE, 0, (16, 10));
        horse
    });
    assert!(seen.mounted && all_accepted(&seen.out), "{seen:?}");
    // No horse: a building as "horse" goes to `mount_start` (the original does not check the saddle,
    // K-224); a dying, a quartered or the own object of the rider is never reached (K-266, K-269); a stale
    // horse and the ID 0 are rejected by the reimplementation with a reason.
    let seen = run(337, "building", &|s, _| rk::spawn(s, rk::HQ, 4, at));
    assert!(all_accepted(&seen.out) && seen.delivered, "{seen:?}");
    assert!(seen.mounted && seen.closest <= 2, "{seen:?}");
    for (what, make) in [
        ("dying horse", 0),
        ("quartered horse", 1),
        ("the rider himself", 2),
    ] {
        let seen = run(337, what, &|s, rider| match make {
            0 => {
                let h = rk::spawn(s, rk::HORSE, 0, at);
                s.object_mut(h).unwrap().status |= status::DIED;
                h
            }
            1 => {
                let h = rk::spawn(s, rk::HORSE, 0, at);
                s.object_mut(h).unwrap().status |= status::QUARTERED;
                h
            }
            _ => rider,
        });
        assert!(all_accepted(&seen.out) && !seen.mounted, "{what}: {seen:?}");
    }
    let seen = run(337, "stale horse", &|s, _| {
        let h = rk::spawn(s, rk::HORSE, 0, at);
        destroy_object(s, h);
        h
    });
    assert!(all_rejected(&seen.out) && !seen.mounted, "{seen:?}");
    let seen = run(337, "ID 0", &|_, _| ObjectId(0));
    assert!(all_rejected(&seen.out) && !seen.mounted, "{seen:?}");

    // 336 boarding on land: neither canoe nor horse or building is reachable for the rider; the command
    // nevertheless aborts, acknowledges and empties (effect, no rejection, E-154).
    for (what, make) in [
        ("canoe on land", 0),
        ("building", 1),
        ("stale canoe", 2),
        ("ID 0", 3),
        ("the rider himself", 4),
    ] {
        let seen = run(336, what, &|s, rider| match make {
            0 => rk::spawn(s, rk::CANOE, 1, at),
            1 => rk::spawn(s, rk::HQ, 4, at),
            2 => {
                let c = rk::spawn(s, rk::CANOE, 0, at);
                destroy_object(s, c);
                c
            }
            3 => ObjectId(0),
            _ => rider,
        });
        assert!(
            all_accepted(&seen.out) && seen.delivered,
            "{what}: {seen:?}"
        );
        assert!(!seen.on_boat && !seen.mounted, "{what}: {seen:?}");
    }

    // 338 dismount: the target is the actor itself. A rider dismounts; for all others the command is
    // ineffective without a module test, but accepted. A rider quartered (here only by status) does not get
    // the message delivered as long as he is quartered.
    let seen = run(338, "rider on horse", &|s, r| {
        let h = rk::spawn(s, rk::HORSE, 0, (10, 10));
        rk::link(s, r, h);
        r
    });
    assert!(all_accepted(&seen.out) && !seen.mounted, "{seen:?}");
    let seen = run(338, "rider in canoe", &|s, r| {
        let c = rk::spawn(s, rk::CANOE, 0, (10, 10));
        rk::link(s, r, c);
        r
    });
    assert!(all_accepted(&seen.out) && !seen.on_boat, "{seen:?}");
    for (what, make) in [("pedestrian", 0), ("dying one", 1), ("quartered one", 2)] {
        let seen = run(338, what, &|s, r| {
            match make {
                1 => s.object_mut(r).unwrap().status |= status::DIED,
                2 => s.object_mut(r).unwrap().status |= status::QUARTERED,
                _ => {}
            }
            r
        });
        assert!(all_accepted(&seen.out), "{what}: {seen:?}");
        assert_eq!(seen.delivered, make != 2, "{what}: {seen:?}");
    }

    // On the lake: the warrior boards a free canoe (also a foreign one), not an occupied one and not a
    // horse.
    let lake = with_script(lake_rules(-100.0), "", &[]);
    let board = |what: &str, number: i32, build: &dyn Fn(&mut Sim, ObjectId) -> ObjectId| {
        let mut state = rk::state(&lake);
        ride_run(&lake, &mut state, number, what, |sim| {
            let rider = rk::spawn(sim, rk::WARRIOR, 0, (18, 30));
            let target = build(sim, rider);
            (rider, target)
        })
    };
    for (what, slot) in [("free canoe", 0), ("foreign canoe", 1)] {
        let seen = board(what, 336, &|s, _| rk::spawn(s, rk::CANOE, slot, (21, 30)));
        assert!(
            all_accepted(&seen.out) && seen.delivered,
            "{what}: {seen:?}"
        );
        assert!(seen.on_boat && seen.closest <= 2, "{what}: {seen:?}");
    }
    let seen = board("occupied canoe", 336, &|s, _| {
        let other = rk::spawn(s, rk::WARRIOR, 0, (22, 30));
        let canoe = rk::spawn(s, rk::CANOE, 0, (21, 30));
        rk::link(s, other, canoe);
        canoe
    });
    assert!(all_accepted(&seen.out) && !seen.on_boat, "{seen:?}");
    // The boat is unchecked (`0x0047D2A0`, K-224): even a horse in the water takes the warrior as a "boat";
    // the module only checks whether the target is free.
    let seen = board("horse in the water", 336, &|s, _| {
        rk::spawn(s, rk::HORSE, 0, (21, 30))
    });
    assert!(all_accepted(&seen.out) && seen.on_boat, "{seen:?}");
    let seen = board("rider in canoe dismounts", 338, &|s, r| {
        let canoe = rk::spawn(s, rk::CANOE, 0, (21, 30));
        rk::link(s, r, canoe);
        r
    });
    // Dismounting (`0x20D`) goes to the riding module 13; the warrior has only the boat module 14
    // (disembarking would be `0x20E`, a different command, K-226): 338 stays accepted for him, but without
    // effect.
    assert!(all_accepted(&seen.out) && seen.on_boat, "{seen:?}");
}

#[test]
fn quarter_334_treats_a_container_with_disabled_walk_as_a_building() {
    // On the script path a container is mobile only with the walk module **switched on** (`HasModule(0, 0)`,
    // `0x0047D9EB`): a ship with walk switched off takes the building path (`FINDTO {container, -1, 1}`, three
    // arguments), the moving one the meeting-point path (`FINDTO` to the meeting point, six arguments). Both
    // paths run without ownership, finished and sight checks.
    use crate::module::boarding::testkit as bk;
    let rules = with_script(bk::rules(), "", &[]);
    for (walk_on, args) in [(true, 6), (false, 3)] {
        let mut state = bk::shore_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ship = bk::spawn(&mut sim, bk::TRANSPORT, 4, (14, 10));
        let unit = bk::spawn(&mut sim, bk::COLONIST, 0, (5, 10));
        sim.module_mut(ship, 0).expect("Walk").base.enabled = walk_on;
        name_ids(&mut sim, "trupp", &[unit]);
        name_ids(&mut sim, "schiff", &[ship]);
        let out = call(&mut sim, 334, vec![txt("trupp"), txt("schiff")]);
        assert_eq!(out, [accepted(0x13)], "Walk {walk_on}");
        let sent = fifo_messages(&sim, unit);
        assert_eq!(sent.len(), 1, "Walk {walk_on}: {sent:?}");
        assert_eq!(sent[0].0, 0x200, "Walk {walk_on}: {sent:?}");
        assert_eq!(sent[0].1.len(), args, "Walk {walk_on}: {sent:?}");
        drop(sim);
        for _ in 0..100 {
            step(&mut state, &rules, &[]);
        }
        assert_saves_and_loads(&state, &rules, "quartering into a ship");
    }

    // E-223: input and AI follow the original like the script path. A ship with the walk module switched
    // off is a building for them; the test ship has no building module (not "finished") and is rejected. The
    // building path itself is in `external_quarter_treats_a_container_with_disabled_walk_as_a_building`
    // (`command/economy.rs`).
    let mut state = bk::shore_state(&rules);
    crate::fog::reveal_map(&mut state, -1);
    let mut sim = Sim::new(&mut state, &rules);
    let ship = bk::spawn(&mut sim, bk::TRANSPORT, 4, (14, 10));
    let unit = bk::spawn(&mut sim, bk::COLONIST, 4, (5, 10));
    sim.module_mut(ship, 0).expect("Walk").base.enabled = false;
    let outcome = crate::command::execute(
        &mut sim,
        &Command {
            player: 4,
            kind: CommandKind::Quarter {
                container: ship,
                units: vec![unit],
            },
        },
        true,
    );
    assert!(
        matches!(&outcome, Outcome::Rejected(why) if why.contains("unfinished")),
        "input: building path, ship without building module rejected: {outcome:?}"
    );
    assert!(fifo_messages(&sim, unit).is_empty());
    // Counter-check: switched on it stays the vehicle path.
    sim.module_mut(ship, 0).expect("Walk").base.enabled = true;
    let outcome = crate::command::execute(
        &mut sim,
        &Command {
            player: 4,
            kind: CommandKind::Quarter {
                container: ship,
                units: vec![unit],
            },
        },
        true,
    );
    assert_eq!(outcome, Outcome::Accepted);
    let sent = fifo_messages(&sim, unit);
    assert_eq!(
        sent[0].1.len(),
        6,
        "input with walk: vehicle path: {sent:?}"
    );
}

// --- Animation lock 0x42/0x43 (actions 157/158, E-186, E-211 to E-213) ---

use crate::messaging::post_message;

fn int(v: i32) -> MsgValue {
    MsgValue::Int(v)
}

/// Lock time and status bit `ANIM_LOCKED` of the unit.
fn lock_of(sim: &Sim, id: ObjectId) -> (i32, bool) {
    let o = sim.object(id).unwrap();
    (o.anim_lock_ms, o.status & status::ANIM_LOCKED != 0)
}

/// Event bit 3 (end of the lock, E-186).
fn bit_3(sim: &Sim, id: ObjectId) -> bool {
    object_event_bits(sim, id) & 8 != 0
}

/// Steps until event bit 3 of the unit is set (at most `max`); `None` without the bit.
fn steps_until_bit_3(
    state: &mut SimState,
    rules: &Rules,
    id: ObjectId,
    max: usize,
) -> Option<usize> {
    for n in 1..=max {
        step(state, rules, &[]);
        if bit_3(&Sim::new(state, rules), id) {
            return Some(n);
        }
    }
    None
}

#[test]
fn lock_0x42_aborts_modules_sets_animation_and_time() {
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let u = {
        let mut sim = Sim::new(&mut state, &rules);
        let u = unit(&mut sim, 0, (10, 10));
        name_ids(&mut sim, "figur", &[u]);
        start_walking(&mut sim, u);
        u
    };
    for _ in 0..3 {
        step(&mut state, &rules, &[]);
    }
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert!(moving(&sim, u), "pre-check: the unit marches");
        fill_queue(&mut sim, u);
        // `n157;name;animation;seconds` -> value slots `{animation, seconds, name}`.
        let out = call(&mut sim, 157, vec![int(5), int(2), txt("figur")]);
        assert_eq!(out, [accepted(0x42)]);
        assert!(!moving(&sim, u), "modules aborted");
        let o = sim.object(u).unwrap();
        assert_eq!(o.anim.wanted, 5, "animation wanted");
        assert_eq!(o.anim.speed, 128.0);
        assert_eq!(lock_of(&sim, u), (2000, true));
        assert_eq!(queue_len(&sim, u), 0, "direct: queue emptied");
        assert!(!bit_3(&sim, u));
    }
    // 2000 ms at 100 ms per object tick: the bit does not come until the lock expires.
    let n = steps_until_bit_3(&mut state, &rules, u, 80).expect("event bit 3");
    assert!((20..=45).contains(&n), "after {n} steps");
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(lock_of(&sim, u), (0, false), "lock expired");
}

#[test]
fn lock_minus_1_is_permanent_until_0x43() {
    // E-211: `seconds < 0` is a permanent lock: the modules stand, bit 3 never comes.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let u = {
        let mut sim = Sim::new(&mut state, &rules);
        let u = unit(&mut sim, 0, (10, 10));
        name_ids(&mut sim, "figur", &[u]);
        call(&mut sim, 157, vec![int(3), int(-1), txt("figur")]);
        assert_eq!(lock_of(&sim, u), (-1000, true));
        u
    };
    // A march during the lock is delivered, but the modules do not tick.
    {
        let mut sim = Sim::new(&mut state, &rules);
        start_walking(&mut sim, u);
    }
    let start = grid(&Sim::new(&mut state, &rules), u);
    assert_eq!(steps_until_bit_3(&mut state, &rules, u, 150), None);
    {
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(lock_of(&sim, u), (-1000, true), "still locked");
        assert_eq!(grid(&sim, u), start, "modules stand: no step");
    }
    // 158 lifts it: time 0, bit off, animation stays, no event bit 3 (E-186).
    {
        let mut sim = Sim::new(&mut state, &rules);
        let out = call(&mut sim, 158, vec![txt("figur")]);
        assert_eq!(out, [accepted(0x43)]);
        assert_eq!(lock_of(&sim, u), (0, false));
        assert_eq!(sim.object(u).unwrap().anim.wanted, 3, "animation stays");
        assert!(!bit_3(&sim, u));
    }
    assert_eq!(steps_until_bit_3(&mut state, &rules, u, 50), None);
    // Counter-check: without a lock the delivered march now starts.
    assert_ne!(
        grid(&Sim::new(&mut state, &rules), u),
        start,
        "after 0x43 the modules run again"
    );
}

#[test]
fn lock_0_takes_label_1_and_without_label_keeps_the_bit() {
    // E-212: `seconds == 0` takes time marker 1 of the animation; without a marker the status bit stays, the
    // lock never expires, the command queue stays blocked (mask `0xF8EC0FFF`).
    let mut rules = testkit::rules();
    rules.set_anim_labels(COLONIST, 4, &[(2, 111), (1, 700)]);
    let mut state = testkit::state(&rules);
    let (with_label, without_label) = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = unit(&mut sim, 0, (10, 10));
        let b = unit(&mut sim, 0, (12, 10));
        name_ids(&mut sim, "mit", &[a]);
        name_ids(&mut sim, "ohne", &[b]);
        call(&mut sim, 157, vec![int(4), int(0), txt("mit")]);
        call(&mut sim, 157, vec![int(6), int(0), txt("ohne")]);
        assert_eq!(lock_of(&sim, a), (700, true), "marker 1, not marker 2");
        assert_eq!(lock_of(&sim, b), (0, true), "no marker: 0, bit stays");
        fill_queue(&mut sim, b);
        (a, b)
    };
    assert!(steps_until_bit_3(&mut state, &rules, with_label, 40).is_some());
    for _ in 0..30 {
        step(&mut state, &rules, &[]);
    }
    {
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(lock_of(&sim, with_label), (0, false));
        assert_eq!(lock_of(&sim, without_label), (0, true), "bit stays");
        assert!(!bit_3(&sim, without_label), "no event bit");
        assert_eq!(queue_len(&sim, without_label), 1, "queue blocked");
    }
    // Counter-check: 158 lifts the blockade, the queue runs off.
    {
        let mut sim = Sim::new(&mut state, &rules);
        call(&mut sim, 158, vec![txt("ohne")]);
        fill_queue(&mut sim, without_label);
    }
    for _ in 0..10 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(queue_len(&sim, without_label), 0, "queue runs");
}

#[test]
fn stop_0x43_clears_a_foreign_setter_bit_and_sets_no_event_bit() {
    // E-213: `0x43` clears the shared bit also for foreign setters (trance, banishing, assassin, ...) and
    // without a running lock; no event bit 3 (E-186); with `direct` it empties the queue.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let u = {
        let mut sim = Sim::new(&mut state, &rules);
        let u = unit(&mut sim, 0, (10, 10));
        name_ids(&mut sim, "figur", &[u]);
        sim.object_mut(u)
            .unwrap()
            .set_status_flags(status::ANIM_LOCKED, true);
        fill_queue(&mut sim, u);
        // A call without a lock (and without the bit) is harmless.
        let free = unit(&mut sim, 0, (12, 10));
        name_ids(&mut sim, "frei", &[free]);
        assert_eq!(call(&mut sim, 158, vec![txt("frei")]), [accepted(0x43)]);
        assert_eq!(lock_of(&sim, free), (0, false));
        let out = call(&mut sim, 158, vec![txt("figur")]);
        assert_eq!(out, [accepted(0x43)]);
        assert_eq!(lock_of(&sim, u), (0, false));
        assert_eq!(queue_len(&sim, u), 0);
        u
    };
    assert_eq!(steps_until_bit_3(&mut state, &rules, u, 30), None);
}

#[test]
fn lock_overwrites_a_running_lock() {
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = unit(&mut sim, 0, (10, 10));
    name_ids(&mut sim, "figur", &[u]);
    call(&mut sim, 157, vec![int(3), int(5), txt("figur")]);
    // A message in the FIFO stays through both locks (no emptying of the FIFO).
    assert!(post_message(
        &mut sim,
        u,
        crate::message::Message::new(0x7FFF, &[]),
        false
    ));
    let waiting = fifo_messages(&sim, u);
    assert_eq!(waiting.len(), 1);
    call(&mut sim, 157, vec![int(7), int(2), txt("figur")]);
    assert_eq!(lock_of(&sim, u), (2000, true), "duration overwritten");
    assert_eq!(sim.object(u).unwrap().anim.wanted, 7);
    assert_eq!(fifo_messages(&sim, u), waiting, "FIFO unchanged");
    // A permanent lock is also overwritten by a short one and vice versa.
    call(&mut sim, 157, vec![int(7), int(-1), txt("figur")]);
    assert_eq!(lock_of(&sim, u), (-1000, true));
    call(&mut sim, 157, vec![int(7), int(1), txt("figur")]);
    assert_eq!(lock_of(&sim, u), (1000, true));
}

#[test]
fn external_lock_needs_an_own_unit() {
    // A8: the input (and the AI) locks only own, living, not quartered units.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let own = unit(&mut sim, 0, (10, 10));
    let foreign = unit(&mut sim, 3, (12, 10));
    let run = |sim: &mut Sim, kind: CommandKind| {
        crate::command::execute(sim, &Command { player: 0, kind }, true)
    };
    let start = |unit| CommandKind::StartAnimationLock {
        unit,
        anim: 4,
        seconds: 3,
    };
    assert_eq!(run(&mut sim, start(own)), Outcome::Accepted);
    assert_eq!(lock_of(&sim, own), (3000, true));
    let outcome = run(&mut sim, start(foreign));
    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
    assert_eq!(lock_of(&sim, foreign), (0, false), "foreign unit untouched");
    // Lifting: the same check.
    sim.object_mut(foreign)
        .unwrap()
        .set_status_flags(status::ANIM_LOCKED, true);
    let outcome = run(&mut sim, CommandKind::StopAnimationLock { unit: foreign });
    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
    assert_eq!(lock_of(&sim, foreign), (0, true));
    assert_eq!(
        run(&mut sim, CommandKind::StopAnimationLock { unit: own }),
        Outcome::Accepted
    );
    assert_eq!(lock_of(&sim, own), (0, false));
}

#[test]
fn script_lock_reaches_a_quartered_foreign_unit() {
    // Script path (E-214): every existing object, also quartered, dying or foreign.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let quartered = unit(&mut sim, 3, (12, 10));
    sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
    let dying = unit(&mut sim, 4, (14, 10));
    sim.object_mut(dying).unwrap().status |= status::DIED;
    let animal = unit(&mut sim, 8, (16, 10));
    let stale = unit(&mut sim, 3, (18, 10));
    destroy_object(&mut sim, stale);
    name_ids(&mut sim, "alle", &[quartered, dying, animal, stale]);
    let out = call(&mut sim, 157, vec![int(3), int(4), txt("alle")]);
    assert_eq!(out.len(), 4);
    for (issued, id) in out.iter().zip([quartered, dying, animal]) {
        assert_eq!(issued, &accepted(0x42), "{id:?}");
        assert_eq!(lock_of(&sim, id), (4000, true), "{id:?}");
    }
    // A stale ID fails in the handler: rejected and reported, nothing locked.
    assert!(matches!(out[3].outcome, Outcome::Rejected(_)), "{out:?}");
    assert!(sim.object(stale).is_none());
    // The animal (slot 8) is not rejected because of the sender.
    let out = call(&mut sim, 158, vec![txt("alle")]);
    assert_eq!(out.len(), 4);
    assert_eq!(lock_of(&sim, animal), (0, false));
}

#[test]
fn action_157_locks_each_id_of_the_name() {
    // All the way through the level script: `n157;name;animation;seconds` locks every ID, `n158;name` lifts.
    let rules = rules_for(&["n157;trupp;5;2"]);
    let mut state = started(&rules);
    let ids = {
        let mut sim = Sim::new(&mut state, &rules);
        let ids = [
            unit(&mut sim, 0, (10, 10)),
            unit(&mut sim, 3, (12, 10)),
            unit(&mut sim, 5, (14, 10)),
        ];
        name_ids(&mut sim, "trupp", &ids);
        ids
    };
    let seen = traces(step(&mut state, &rules, &[]));
    let codes: Vec<_> = commands(&seen)
        .into_iter()
        .map(|t| match t {
            ScriptTrace::Command {
                action,
                code,
                outcome,
                ..
            } => (*action, *code, outcome.clone()),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(codes, vec![(157, 0x42, Outcome::Accepted); 3], "{seen:?}");
    let sim = Sim::new(&mut state, &rules);
    for id in ids {
        assert!(lock_of(&sim, id).1, "{id:?} locked");
    }
    drop(sim);

    // Unknown name: nothing sent, nothing reported.
    let rules = rules_for(&["n157;niemand;5;2", "n158;niemand"]);
    let mut state = started(&rules);
    let seen = traces(step(&mut state, &rules, &[]));
    assert!(commands(&seen).is_empty(), "{seen:?}");

    // 158 via the script.
    let rules = rules_for(&["n158;trupp"]);
    let mut state = started(&rules);
    let id = {
        let mut sim = Sim::new(&mut state, &rules);
        let id = unit(&mut sim, 2, (10, 10));
        name_ids(&mut sim, "trupp", &[id]);
        crate::world::start_animation_lock(&mut sim, id, 3, -1);
        id
    };
    let seen = traces(step(&mut state, &rules, &[]));
    assert_eq!(commands(&seen).len(), 1, "{seen:?}");
    assert_eq!(lock_of(&Sim::new(&mut state, &rules), id), (0, false));
}

#[test]
fn lock_survives_saving_and_loading_mid_lock() {
    // Saving/loading in the middle of the lock - permanent and temporary.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    let (timed, endless) = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = unit(&mut sim, 0, (10, 10));
        let b = unit(&mut sim, 0, (12, 10));
        crate::world::start_animation_lock(&mut sim, a, 4, 3);
        crate::world::start_animation_lock(&mut sim, b, 4, -1);
        (a, b)
    };
    for _ in 0..10 {
        step(&mut state, &rules, &[]);
    }
    assert_saves_and_loads(&state, &rules, "in the middle of the lock");
    let save = SaveGame {
        start: start_package(&rules, &MatchSetup::default()),
        state: state.clone(),
        commands: Vec::new(),
    };
    let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
    // Saved and loaded they continue the same: the temporary lock expires, the permanent one does not.
    let n_live = steps_until_bit_3(&mut state, &rules, timed, 80);
    let n_loaded = steps_until_bit_3(&mut loaded, &rules, timed, 80);
    assert!(n_live.is_some());
    assert_eq!(n_live, n_loaded);
    assert_eq!(checksum(&state), checksum(&loaded));
    assert_eq!(
        lock_of(&Sim::new(&mut loaded, &rules), endless),
        (-1000, true)
    );
}

// --- Victory and defeat, upgrade, headhunter (actions 4, 13, 38, 380) ---

use super::command::{player_result, upgrade};
use crate::game_end::{DEFEAT, VICTORY};

/// All eight slots are still playing (the test world has them as empty, result 3).
fn all_playing(state: &mut SimState) {
    for p in &mut state.players[..8] {
        p.result = 0;
    }
}

/// Results of slots 0 to 7.
fn results(sim: &Sim) -> Vec<i32> {
    sim.state.players[..8].iter().map(|p| p.result).collect()
}

/// `SimEvent::PlayerResult` in the order of their occurrence.
fn result_events(events: &[SimEvent]) -> Vec<(u8, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::PlayerResult { slot, result } => Some((*slot, *result)),
            _ => None,
        })
        .collect()
}

#[test]
fn n13_lets_every_non_mutual_friend_lose() {
    // E-161: slot 6 wins; slot 1 is a mutual ally (stays 0), slot 2 only a one-sided ally, slot 3 neutral
    // (both lose), the other slots are enemies and lose.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    all_playing(&mut state);
    state.diplomacy.set_friend(6, 1, true);
    state.diplomacy.set_friend(1, 6, true);
    state.diplomacy.set_friend(6, 2, true);
    for q in [1, 2, 3] {
        state.diplomacy.set_enemy(6, q, false);
        state.diplomacy.set_enemy(q, 6, false);
    }
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(results(&sim), [0; 8], "all are still playing");
    let out = call(&mut sim, 13, vec![int(6)]);
    assert_eq!(out, [accepted(0x56)], "{out:?}");
    // Mutual ally 1 stays in the game, slot 6 wins, all others lose.
    assert_eq!(results(&sim), [1, 0, 1, 1, 1, 1, 2, 1]);
    // Sequence of the recursion: ascending over the slots, the winner last.
    let events = std::mem::take(&mut sim.state.events);
    assert_eq!(
        result_events(&events),
        [
            (0, DEFEAT),
            (2, DEFEAT),
            (3, DEFEAT),
            (4, DEFEAT),
            (5, DEFEAT),
            (7, DEFEAT),
            (6, VICTORY)
        ]
    );
}

#[test]
fn n4_loses_only_the_named_slot() {
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    all_playing(&mut state);
    let mut sim = Sim::new(&mut state, &rules);
    let out = call(&mut sim, 4, vec![int(2)]);
    assert_eq!(out, [accepted(0x56)], "{out:?}");
    assert_eq!(results(&sim), [0, 0, 1, 0, 0, 0, 0, 0]);
    assert_eq!(result_events(&sim.state.events), [(2, DEFEAT)]);
}

#[test]
fn n4_does_not_overwrite_a_decided_slot() {
    // A won slot stays won, an empty slot (3) stays empty; no event.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    all_playing(&mut state);
    state.players[2].result = VICTORY;
    state.players[5].result = 3;
    let mut sim = Sim::new(&mut state, &rules);
    for slot in [2, 5] {
        let out = call(&mut sim, 4, vec![int(slot)]);
        assert_eq!(out, [accepted(0x56)], "{out:?}");
    }
    assert_eq!(results(&sim), [0, 0, 2, 0, 0, 3, 0, 0]);
    assert!(result_events(&sim.state.events).is_empty());
    // A second victory also does not change a lost slot.
    sim.state.players[4].result = DEFEAT;
    call(&mut sim, 13, vec![int(4)]);
    assert_eq!(sim.state.players[4].result, DEFEAT);
}

#[test]
fn n13_with_slot_9_is_rejected_and_the_block_goes_on() {
    // A slot outside 0 to 7 -> rejected without execution, no `Guarded`; the rest runs.
    let (rules, mut state, unit, traces) = walking_foreign_unit(&["n13;9", "n191;feind"]);
    let rejected = ScriptTrace::Command {
        container: LEVEL_CONTAINER_ID,
        block: 0,
        action: 13,
        code: 0x56,
        outcome: Outcome::Rejected("slot 9 (allowed 0 to 7)".into()),
    };
    assert_eq!(
        commands(&traces),
        [
            &rejected,
            &ScriptTrace::Command {
                container: LEVEL_CONTAINER_ID,
                block: 0,
                action: 191,
                code: 0x3C,
                outcome: Outcome::Accepted,
            }
        ],
        "{traces:?}"
    );
    assert!(
        !traces
            .iter()
            .any(|t| matches!(t, ScriptTrace::Guarded { .. } | ScriptTrace::Pending { .. })),
        "{traces:?}"
    );
    let sim = Sim::new(&mut state, &rules);
    assert!(!moving(&sim, unit), "halt after 13 ran");
    // Negative slots likewise; result 8 is no slot.
    let mut state = testkit::state(&rules);
    all_playing(&mut state);
    let mut sim = Sim::new(&mut state, &rules);
    for p in [-1, 8, i32::MAX, i32::MIN] {
        let mut out = Vec::new();
        player_result(&mut sim, &mut out, p, VICTORY);
        assert_eq!(
            out,
            [Issued {
                code: 0x56,
                outcome: Outcome::Rejected(format!("slot {p} (allowed 0 to 7)"))
            }]
        );
    }
    assert_eq!(results(&sim), [0; 8]);
}

#[test]
fn n13_mid_block_runs_the_rest() {
    // Victory of slot 6 in the middle of the block: result set, the following action runs in the same tick.
    let rules = rules_for(&["n13;6", "n191;feind"]);
    let mut state = started(&rules);
    all_playing(&mut state);
    let unit = {
        let mut sim = Sim::new(&mut state, &rules);
        let unit = testkit::spawn(&mut sim, COLONIST, 3, (10, 10));
        name_ids(&mut sim, "feind", &[unit]);
        start_walking(&mut sim, unit);
        unit
    };
    let events = step(&mut state, &rules, &[]);
    assert!(result_events(&events).contains(&(6, VICTORY)), "{events:?}");
    let traces = traces(events);
    let actions: Vec<i32> = commands(&traces)
        .into_iter()
        .map(|t| match t {
            ScriptTrace::Command { action, .. } => *action,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(actions, [13, 191]);
    assert!(state.script.level.as_ref().unwrap().blocks.is_empty());
    let sim = Sim::new(&mut state, &rules);
    assert!(!moving(&sim, unit));
    assert_eq!(sim.state.players[6].result, VICTORY);
}

#[test]
fn n38_applies_the_upgrade_twice_when_sent_twice() {
    // No duplicate check (re/script-actions C.6): sent twice it counts twice; slot 0 sends for slot 1.
    let rules = testkit::rules();
    let mut state = testkit::state(&rules);
    all_playing(&mut state);
    let mut sim = Sim::new(&mut state, &rules);
    let count =
        |sim: &Sim, slot: usize| sim.state.guid_counts[slot].get(&2800).copied().unwrap_or(0);
    for expected in [1, 2] {
        let out = call(&mut sim, 38, vec![int(1), int(2800)]);
        assert_eq!(out, [accepted(0x39)], "{out:?}");
        assert_eq!(count(&sim, 1), expected);
    }
    assert_eq!(count(&sim, 0), 0, "only the named slot");
    assert_eq!(sim.state.applied_upgrades[1], [2800, 2800]);
    // Slots 8 to 10 go (sender 0), slot 11 and negative ones are rejected by the command (A7).
    let mut out = Vec::new();
    upgrade(&mut sim, &mut out, 9, 2800);
    assert_eq!(out, [accepted(0x39)]);
    for p in [11, -1] {
        let mut out = Vec::new();
        upgrade(&mut sim, &mut out, p, 2800);
        assert!(
            matches!(
                out[..],
                [Issued {
                    code: 0x39,
                    outcome: Outcome::Rejected(_)
                }]
            ),
            "{out:?}"
        );
    }
    // `guid < 0`: all GUIDs 2800 to 3399.
    call(&mut sim, 38, vec![int(2), int(-1)]);
    assert_eq!(sim.state.applied_upgrades[2].len(), 600);
}
