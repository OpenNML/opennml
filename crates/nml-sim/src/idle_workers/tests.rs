//! Tests of the idle workers (re/game-ui D5.4, O-293): filling at level start, message `0x87F`,
//! maintenance in the 100 ms tick, savegame and replay.

use nml_formats::config::LevelPlayer;
use nml_formats::wld::WorldObject;

use super::*;
use crate::combat::testkit::{SABRE, rules as combat_rules, spawn, state as combat_state};
use crate::command::testkit::{COLONIST, world};
use crate::command::{Command, CommandKind, NO_SPREAD};
use crate::message::Message;
use crate::messaging::send;
use crate::motion::NO_DIRECTION;
use crate::rules::Rules;
use crate::save::{RecordedCommand, Replay, SaveGame, checksum};
use crate::setup::{MatchSetup, new_match, start_package};
use crate::state::SimState;
use crate::tick::step;

/// `.wld` object of color `color` at the center of tile `tile`.
fn world_object(guid: u32, color: i32, tile: (i32, i32)) -> WorldObject {
    WorldObject {
        guid,
        position: [world(tile.0), 0.0, world(tile.1)],
        scale: [1.0; 3],
        rotation: [0.0; 3],
        color: Some(color),
        name: None,
        config: Default::default(),
    }
}

/// Campaign map with the humans on slots 0 and 1: two colonists and a saber fighter on
/// slot 0, one colonist on slot 1.
fn level() -> (Rules, MatchSetup) {
    let mut r = combat_rules();
    r.level.players = [(0u8, "Held"), (1, "Gegner")]
        .iter()
        .map(|&(slot, name)| LevelPlayer {
            slot,
            name: name.into(),
            nation: 0,
            kind: 0,
            resources: Vec::new(),
        })
        .collect();
    r.world_objects = vec![
        world_object(COLONIST, 0, (5, 5)),
        world_object(SABRE, 0, (7, 5)),
        world_object(COLONIST, 1, (20, 20)),
        world_object(COLONIST, 0, (9, 5)),
    ];
    (r, MatchSetup::default())
}

/// IDs of the objects of slot `slot` with type `guid` in table order.
fn ids(state: &SimState, slot: usize, guid: u32) -> Vec<ObjectId> {
    state.tables[slot]
        .objects
        .iter()
        .flatten()
        .filter(|o| o.guid == guid)
        .map(|o| o.id)
        .collect()
}

/// Move command of slot 0 to the center of tile `tile`.
fn go(units: &[ObjectId], tile: (i32, i32)) -> Command {
    Command {
        player: 0,
        kind: CommandKind::Move {
            units: units.to_vec(),
            x: (tile.0 << 12) + 0x800,
            z: (tile.1 << 12) + 0x800,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        },
    }
}

#[test]
fn idle_workers_fill_at_level_start() {
    let (rules, setup) = level();
    let state = new_match(&rules, &setup).unwrap();
    let own = ids(&state, 0, COLONIST);
    let foreign = ids(&state, 1, COLONIST);
    assert_eq!((own.len(), foreign.len()), (2, 1));
    // Only the workers (module 8), per slot in table order; the saber fighter is missing.
    assert_eq!(state.idle_workers.list(0), own.as_slice());
    assert_eq!(state.idle_workers.list(1), foreign.as_slice());
    assert!((2..8).all(|p| state.idle_workers.list(p).is_empty()));
    assert!(state.idle_workers.list(8).is_empty());
    let sabre = ids(&state, 0, SABRE)[0];
    assert!(
        !state
            .idle_workers
            .lists
            .iter()
            .flatten()
            .any(|&id| id == sabre)
    );
}

#[test]
fn idle_worker_message_appends_once() {
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, COLONIST, 0, (5, 5));
    let b = spawn(&mut sim, COLONIST, 0, (7, 5));
    let sabre = spawn(&mut sim, SABRE, 0, (9, 5));
    let neutral = spawn(&mut sim, COLONIST, 8, (11, 5));
    assert!(sim.state.idle_workers.list(0).is_empty(), "no level start");
    // `0x87F` twice: one entry; the arguments do not count (the original does not read them).
    assert_eq!(send(&mut sim, b, &Message::new(0x87F, &[a.as_i32()])), 0);
    assert_eq!(send(&mut sim, b, &Message::new(0x87F, &[])), 0);
    assert_eq!(sim.state.idle_workers.list(0), [b]);
    // Via `0x97F` (event bit 1 and message to itself) the next one arrives at the back.
    send(&mut sim, a, &Message::new(0x97F, &[]));
    assert_eq!(sim.state.idle_workers.list(0), [b, a]);
    // No worker (module 8 missing) and slot >= 8 (A7): nothing.
    send(&mut sim, sabre, &Message::new(0x87F, &[]));
    send(&mut sim, neutral, &Message::new(0x87F, &[]));
    assert_eq!(sim.state.idle_workers.list(0), [b, a]);
    assert!(sim.state.idle_workers.lists[1..].iter().all(Vec::is_empty));
    // A busy worker is reported too: `0x114` does not read the state.
    let c = spawn(&mut sim, COLONIST, 0, (13, 5));
    sim.object_mut(c).unwrap().status |= status::MOVE;
    send(&mut sim, c, &Message::new(0x87F, &[]));
    assert_eq!(sim.state.idle_workers.list(0), [b, a, c]);
}

#[test]
fn refresh_runs_only_at_phase_zero_and_keeps_order() {
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, COLONIST, 0, (5, 5));
    let b = spawn(&mut sim, COLONIST, 0, (7, 5));
    let c = spawn(&mut sim, COLONIST, 0, (9, 5));
    let sabre = spawn(&mut sim, SABRE, 0, (11, 5));
    init(&mut sim);
    assert_eq!(sim.state.idle_workers.list(0), [a, b, c]);
    // The tick: first out what does not fit, then missing fitting ones at the back (table order).
    sim.object_mut(a).unwrap().status |= status::MOVE;
    sim.state.idle_workers.lists[0].insert(0, sabre);
    refresh(&mut sim);
    assert_eq!(sim.state.idle_workers.list(0), [b, c]);
    sim.object_mut(a).unwrap().status &= !status::MOVE;
    sim.object_mut(b).unwrap().status |= status::QUARTERED;
    refresh(&mut sim);
    assert_eq!(sim.state.idle_workers.list(0), [c, a]);
    sim.object_mut(b).unwrap().status &= !status::QUARTERED;
    // A stale ID (wrong generation) drops out.
    let stale = ObjectId::new(c.index() as u16, 0, c.generation().wrapping_add(1));
    sim.state.idle_workers.lists[0].push(stale);
    refresh(&mut sim);
    assert_eq!(sim.state.idle_workers.list(0), [c, a, b]);

    // In the simulation step only in the phase 0 step (second half tick).
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, COLONIST, 0, (5, 5));
    let sabre = spawn(&mut sim, SABRE, 0, (11, 5));
    sim.state.idle_workers.lists[0].push(sabre);
    assert_eq!(state.phase, 0);
    step(&mut state, &rules, &[]);
    assert_eq!(
        state.idle_workers.list(0),
        [sabre],
        "first half tick: no change"
    );
    step(&mut state, &rules, &[]);
    assert_eq!(state.idle_workers.list(0), [a]);
    let mut sim = Sim::new(&mut state, &rules);
    let b = spawn(&mut sim, COLONIST, 0, (7, 5));
    step(&mut state, &rules, &[]);
    assert_eq!(
        state.idle_workers.list(0),
        [a],
        "first half tick: no change"
    );
    step(&mut state, &rules, &[]);
    assert_eq!(state.idle_workers.list(0), [a, b]);
}

#[test]
fn idle_workers_survive_save_load_and_replay() {
    // Five workers on slot 0 (a ... e) and one on slot 1.
    let (mut rules, setup) = level();
    rules.world_objects.extend([
        world_object(COLONIST, 0, (11, 5)),
        world_object(COLONIST, 0, (13, 5)),
        world_object(COLONIST, 0, (15, 5)),
    ]);
    let mut live = new_match(&rules, &setup).unwrap();
    let workers = ids(&live, 0, COLONIST);
    let [a, b, c, d, e] = workers[..] else {
        panic!("five workers expected: {workers:?}");
    };
    let foreign = ids(&live, 1, COLONIST)[0];
    assert_eq!(live.idle_workers.list(0), workers.as_slice());
    // Tick 0: worker b walks far away (busy).
    let command = go(&[b], (50, 50));
    step(&mut live, &rules, std::slice::from_ref(&command));
    assert_eq!(live.phase, 1);
    // In the middle between two 100 ms ticks:
    // - c dies: `DIED`, the object stays in the table until the destruction list after the
    //   tick;
    // - d becomes a soldier: module 8 off (the filter requires an active module 8);
    // - e changes to slot 1: new ID there, the old one in list 0 is stale;
    // - plus an inserted stale ID.
    let e_new = {
        let mut sim = Sim::new(&mut live, &rules);
        sim.object_mut(c).unwrap().status |= status::DIED;
        crate::world::queue_destroy(&mut sim, c);
        sim.object_mut(d)
            .unwrap()
            .module_mut(8)
            .unwrap()
            .base
            .enabled = false;
        crate::world::change_owner(&mut sim, e, 1).unwrap()
    };
    assert_eq!(e_new.slot(), 1);
    let stale = ObjectId::new(a.index() as u16, 0, a.generation().wrapping_add(1));
    live.idle_workers.lists[0].push(stale);
    // Between the ticks no list changes.
    assert_eq!(live.idle_workers.list(0), [a, b, c, d, e, stale]);
    assert_eq!(live.idle_workers.list(1), [foreign]);
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: live.clone(),
        commands: Vec::new(),
    };
    let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    loaded.check(&rules).unwrap();
    assert_eq!(loaded.state.idle_workers, live.idle_workers);
    assert_eq!(checksum(&loaded.state), checksum(&live));
    // Next tick: b (busy), c (dead, still in the table), d (soldier), the old ID
    // of e and the inserted one drop out; the new ID of e arrives at the back of list 1.
    // Loaded like live.
    step(&mut live, &rules, &[]);
    step(&mut loaded.state, &rules, &[]);
    assert_eq!(live.idle_workers.list(0), [a]);
    assert_eq!(live.idle_workers.list(1), [foreign, e_new]);
    assert!(
        live.object(c, false).is_none(),
        "c destroyed after the tick"
    );
    assert!(live.object(d, false).is_some());
    assert_eq!(checksum(&loaded.state), checksum(&live));
    assert_eq!(loaded.state, live);

    // The replay reaches the same state (without the inserted ID).
    let mut live = new_match(&rules, &setup).unwrap();
    step(&mut live, &rules, std::slice::from_ref(&command));
    for _ in 0..5 {
        step(&mut live, &rules, &[]);
    }
    let replay = Replay {
        start: start_package(&rules, &setup),
        commands: vec![RecordedCommand { step: 0, command }],
    };
    let played = Replay::from_bytes(&replay.to_bytes())
        .unwrap()
        .play(&rules, 6)
        .unwrap();
    assert_eq!(played.idle_workers.list(0), [a, c, d, e]);
    assert_eq!(checksum(&played), checksum(&live));
    assert_eq!(played, live);
}
