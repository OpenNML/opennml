//! Tests of the animal evocation (re/evoke-aura): init, cast, reserve, position search, abort and tick
//! (2.1, 3.1, 3.3-3.5, 8 no. 1, 2, 5-9, 19, 9 V1 to V5), appearing, animal `0x314` and death
//! callbacks (2.2, 3.2, 3.2a, 3.10, 4, 8 no. 3, 4, 10, 9 V1, V4 to V6). "N" is the object tick in which the FIFO
//! delivers `0x120`; ticks are object ticks (two `tick::step`).

use std::cell::RefCell;

use super::*;
use crate::command::{Command, CommandKind, NO_SPREAD};
use crate::fx::{Fx, FxVec3, fixed_mul};
use crate::math::{cos_deg, sin_deg};
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, post_message, send};
use crate::module::ability_testkit as tk;
use crate::module::{attack, spell, test_hook};
use crate::object::status;
use crate::state::SimEvent;
use crate::world::{
    destroy_object, object_event_bits, set_direction, set_grid_position, set_object_event_bit,
};

/// Buttons "wolf" (cost 25) and "bear" (cost 50) of the test world.
const WOLF_BUTTON: i32 = 441;
const BEAR_BUTTON: i32 = 442;
/// One tile in fixed point.
const TILE: i32 = 4096;

thread_local! {
    /// Queries `(object, module, code)` during [`observed`].
    static SEEN: RefCell<Vec<(ObjectId, usize, u32)>> = const { RefCell::new(Vec::new()) };
}

fn record(_: &mut Sim, id: ObjectId, index: usize, m: &Message) {
    SEEN.with(|s| s.borrow_mut().push((id, index, m.code)));
}

/// Runs `f` with the observer and returns all delivered queries in order.
fn observed(f: impl FnOnce()) -> Vec<(ObjectId, usize, u32)> {
    SEEN.with(|s| s.borrow_mut().clear());
    test_hook::observe(record, f);
    SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// Number of `0x220` to module 32 (whichever ID).
fn evokes(seen: &[(ObjectId, usize, u32)]) -> usize {
    seen.iter()
        .filter(|&&(_, index, code)| index == INDEX && code == msg::EVOKE)
        .count()
}

/// `n` object ticks; return value: all events.
fn ticks(sim: &mut Sim, n: usize) -> Vec<SimEvent> {
    let mut events = Vec::new();
    for _ in 0..2 * n {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    }
    events
}

fn set_mana(sim: &mut Sim, id: ObjectId, v: i32) {
    let p = spell::mana_percent(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_current_value(MANA, v, p);
    }
}

fn mana(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, MANA)
}

fn fraction(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id)
        .expect("object")
        .attributes
        .current_fraction(MANA)
}

fn reserve(sim: &Sim, id: ObjectId) -> i32 {
    evoke_animal(sim, id).expect("module 32").reserve
}

fn evoking(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.status & status::EVOKING != 0)
}

fn pending_evoke(sim: &Sim, id: ObjectId) -> bool {
    sim.module(id, INDEX)
        .is_some_and(|m| m.base.has_pending_timer(msg::EVOKE))
}

/// Animal evoker 1412 in slot 0 with mana 100 (explicitly set).
fn evoker(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    let id = tk::spawn(sim, tk::ANIMAL_EVOKER, 0, tile);
    set_mana(sim, id, 100);
    id
}

fn cast_message(art: i32, button: i32) -> Message {
    Message::new(msg::CAST, &[art, button])
}

/// Posts `0x120 {0, 441}` with abort and lets tick N run.
fn start(sim: &mut Sim, id: ObjectId) -> Vec<SimEvent> {
    assert!(post_message(sim, id, cast_message(0, WOLF_BUTTON), true));
    let events = ticks(sim, 1);
    assert!(evoking(sim, id), "N: EVOKING");
    events
}

/// Grid position `(x, z)`.
fn grid(sim: &Sim, id: ObjectId) -> (i32, i32) {
    let g = sim.object(id).expect("object").grid;
    (g.x.0, g.z.0)
}

/// Creates `guid` and sets it exactly to `(x, z)` (fixed point).
fn place(sim: &mut Sim, guid: u32, x: i32, z: i32) -> ObjectId {
    let id = tk::spawn(sim, guid, 0, (x >> 12, z >> 12));
    set_grid_position(sim, id, FxVec3::new(Fx(x), Fx::ZERO, Fx(z)), true);
    assert_eq!(grid(sim, id), (x, z));
    id
}

/// Position on ring `r` at angle `a` (degrees * 4096) like `0x0050FF10`.
fn ring_point(x0: i32, z0: i32, a: i32, r: i32) -> (i32, i32) {
    (x0 + fixed_mul(cos_deg(a), r), z0 + fixed_mul(sin_deg(a), r))
}

/// Path layer 1x1 of the evoker (`0xB00`).
fn cost_layer(sim: &mut Sim, id: ObjectId) -> usize {
    send(sim, id, &Message::new(0xB00, &[])) as usize
}

fn block_cells(sim: &mut Sim, layer: usize, cells: &[(i32, i32)]) {
    for &(x, z) in cells {
        sim.state.path.mark_cell(1 << layer, x, z, true);
    }
    sim.state.path.flush_dirty_rect();
}

#[test]
fn init_reads_the_anim_and_stops_the_regeneration() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let evoker = tk::spawn(&mut sim, tk::ANIMAL_EVOKER, 0, (10, 10));
    // 1412 has no module 38: module 32 switches the regeneration off itself (base value 1).
    assert!(!sim.object(evoker).unwrap().has_module(0x26, true));
    assert_eq!(evoke_animal(&sim, evoker), Some(&EvokeAnimal::default()));
    let e = evoke_animal(&sim, evoker).unwrap();
    assert_eq!((e.anim, e.reserve), (0, 0));
    assert_eq!(sim.object(evoker).unwrap().attributes.regen_rate(MANA), 0);
    assert_eq!(sim.module(evoker, INDEX).unwrap().base.flags, 0);
}

// QUIRK C-276, C-283: kept
#[test]
fn casting_pays_at_once_and_reserves() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    // Attack off the tick list beforehand, so that switching it on (C-283) becomes visible.
    {
        let o = sim.object_mut(ev).unwrap();
        o.set_module_enabled(attack::INDEX, false);
        o.apply_module_activation_changes();
        assert!(!o.tick_list.contains(&(attack::INDEX as u8)));
    }
    set_object_event_bit(&mut sim, ev, 4, true);
    let events = start(&mut sim, ev);
    // N: mana 75 at once, reserve 25, animation 0, attack switched on, event bit 4 off.
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (75, 25));
    let o = sim.object(ev).unwrap();
    assert_eq!(o.anim.wanted, 0);
    assert!(o.tick_list.contains(&(attack::INDEX as u8)));
    assert!(o.has_module(attack::INDEX, false));
    assert_eq!(object_event_bits(&sim, ev) & 0x10, 0, "event bit 4 off");
    let e = evoke_animal(&sim, ev).unwrap();
    assert_eq!((e.art, e.cost, e.button), (0, 25, WOLF_BUTTON));
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::Sound3d { object, action: 0x14, flags: 3, guid, .. }
            if *object == ev && *guid == tk::ANIMAL_EVOKER)));
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::Sound3d { object, action: 0x1E, flags: 0x42, guid: 441, .. } if *object == ev)));
    assert!(pending_evoke(&sim, ev));
    // N+1 ... N+80: mana stays 75 (cap max - reserve), no `0x220` yet.
    let seen = observed(|| {
        for k in 1..=80 {
            ticks(&mut sim, 1);
            assert_eq!(mana(&sim, ev), 75, "N+{k}");
        }
    });
    assert_eq!(evokes(&seen), 0);
    // N+81 (marker 8000 ms): `0x220` to module 32.
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert!(seen.contains(&(ev, INDEX, msg::EVOKE)));
}

// QUIRK C-280: kept
#[test]
fn the_first_free_spot_is_one_tile_ahead() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    set_direction(&mut sim, ev, 0);
    let (x0, z0) = grid(&sim, ev);
    assert_eq!(
        spawn::find_spawn_pos(&mut sim, ev),
        Some([x0 + TILE, 0, z0])
    );
    assert_eq!(send(&mut sim, ev, &cast_message(0, WOLF_BUTTON)), 1);
    assert_eq!(evoke_animal(&sim, ev).unwrap().spawn, [x0 + TILE, 0, z0]);
}

// QUIRK C-280: kept
#[test]
fn a_blocked_spot_moves_round_the_ring() {
    let rules = tk::evoke_rules();
    // A blocking object (hard radius 512) one tile ahead; 0 degrees (distance 0) and 10 degrees
    // (~714 < 512 + 512) are blocked, 20 degrees is free.
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    set_direction(&mut sim, ev, 0);
    let (x0, z0) = grid(&sim, ev);
    let blocker = place(&mut sim, tk::WARRIOR, x0 + TILE, z0);
    let (x, z) = ring_point(x0, z0, 20 << 12, TILE);
    assert_eq!((x, z), (x0 + 3848, z0 + 1400));
    assert_eq!(spawn::find_spawn_pos(&mut sim, ev), Some([x, 0, z]));
    // Counter-check with a cell lock on the tile ahead: the footprint (+-512) of the evoker reaches into the tile up to
    // 30 degrees; 40 degrees is the first free angle.
    destroy_object(&mut sim, blocker);
    let layer = cost_layer(&mut sim, ev);
    block_cells(&mut sim, layer, &[(11, 10)]);
    let (x, z) = ring_point(x0, z0, 40 << 12, TILE);
    assert_eq!((x, z), (x0 + 3137, z0 + 2632));
    assert_eq!(spawn::find_spawn_pos(&mut sim, ev), Some([x, 0, z]));

    // Other region: facing 45 degrees, the first point lies with its whole footprint in tile (11, 11).
    // Two locks beside it leave it connected - it is taken; with all four neighbors
    // locked it is a region of its own and is skipped.
    let first = |sim: &mut Sim, walls: &[(i32, i32)]| {
        let ev = evoker(sim, (10, 10));
        set_direction(sim, ev, 45 << 12);
        let layer = cost_layer(sim, ev);
        block_cells(sim, layer, walls);
        let found = spawn::find_spawn_pos(sim, ev).expect("position");
        let region = |x: i32, z: i32| sim.state.path.region_id(layer, x >> 12, z >> 12);
        let (x0, z0) = grid(sim, ev);
        let same = region(found[0], found[2]) == region(x0, z0);
        let pocket = sim.state.path.region_id(layer, 11, 11) != region(x0, z0);
        (ring_point(x0, z0, 45 << 12, TILE), found, same, pocket)
    };
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (ahead, found, same, pocket) = first(&mut sim, &[(11, 10), (10, 11)]);
    assert!(!pocket);
    assert_eq!(found, [ahead.0, 0, ahead.1]);
    assert!(same);
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (ahead, found, same, pocket) = first(&mut sim, &[(11, 10), (10, 11), (12, 11), (11, 12)]);
    assert!(pocket, "tile (11, 11) is a region of its own");
    assert_ne!(found, [ahead.0, 0, ahead.1]);
    assert!(same, "hit in the region of the evoker");
}

// QUIRK C-280: kept
#[test]
fn ring_five_is_shifted_unchecked() {
    // C-280: block rings 1-4 only via collision cost (one object with a
    // hard radius exactly there per check point), everything synchronous without `step`.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (20, 20));
    set_direction(&mut sim, ev, 0);
    let (x0, z0) = grid(&sim, ev);
    for ring in 1..=4 {
        for step in 0..36 {
            let (x, z) = ring_point(x0, z0, step * (10 << 12), ring * TILE);
            place(&mut sim, tk::WARRIOR, x, z);
        }
    }
    let want = [x0 + 5 * TILE + cos_deg(0), 0, z0 + sin_deg(0)];
    assert_eq!(want, [x0 + 6 * TILE, 0, z0]);
    assert_eq!(send(&mut sim, ev, &cast_message(0, WOLF_BUTTON)), 1);
    assert_eq!(evoke_animal(&sim, ev).unwrap().spawn, want);
    // Counter-check: an object exactly on the offset position changes nothing (unchecked).
    place(&mut sim, tk::WARRIOR, want[0], want[2]);
    assert_eq!(spawn::find_spawn_pos(&mut sim, ev), Some(want));
}

// QUIRK K-312: deviation
#[test]
fn a_full_map_fails_the_cast_before_paying() {
    // K-312 (A7): without a free place the search ends at the map boundary; the cast fails before the
    // mana deduction.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    let layer = cost_layer(&mut sim, ev);
    let (w, h) = (rules.map.dim_x, rules.map.dim_z);
    let cells: Vec<(i32, i32)> = (0..h)
        .flat_map(|z| (0..w).map(move |x| (x, z)))
        .filter(|&c| c != (10, 10))
        .collect();
    block_cells(&mut sim, layer, &cells);
    assert_eq!(spawn::find_spawn_pos(&mut sim, ev), None);
    assert_eq!(send(&mut sim, ev, &cast_message(0, WOLF_BUTTON)), 0);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (100, 0));
    assert!(!evoking(&sim, ev));
    assert!(!pending_evoke(&sim, ev));
}

/// Walk command as the UI gives it (`direct`: with abort).
fn walk_to(unit: ObjectId, tile: (i32, i32)) -> Command {
    Command {
        player: 0,
        kind: CommandKind::Move {
            units: vec![unit],
            x: (tile.0 << 12) + 0x800,
            z: (tile.1 << 12) + 0x800,
            target: -1,
            formation: 0,
            direction: crate::motion::NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        },
    }
}

// QUIRK C-277: kept
#[test]
fn a_walk_order_refunds_the_evocation() {
    // V3, C-277.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    ticks(&mut sim, 39);
    assert!(evoking(&sim, ev));
    // N+40: walk command.
    crate::tick::step(sim.state, sim.rules, &[walk_to(ev, (20, 10))]);
    crate::tick::step(sim.state, sim.rules, &[]);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (100, 0));
    assert!(!evoking(&sim, ev));
    assert!(!pending_evoke(&sim, ev));
    let seen = observed(|| {
        ticks(&mut sim, 45);
    });
    assert_eq!(evokes(&seen), 0, "no 0x220 any more");
}

// QUIRK C-277: kept
#[test]
fn death_during_the_evocation_refunds() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    ticks(&mut sim, 39);
    // N+40: hit points 0, `check_death` after the module ticks -> `start_dying` -> abort.
    let p = sim.type_upgrade_percent(0, tk::ANIMAL_EVOKER, 14);
    sim.object_mut(ev)
        .unwrap()
        .attributes
        .set_current_value(14, 0, p);
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    // Refund at the evoker (or at the carcass if the ownership has already changed).
    let dead = sim
        .state
        .objects()
        .find(|o| o.guid == tk::ANIMAL_EVOKER)
        .map(|o| o.id)
        .expect("carcass");
    assert!(sim.object(dead).unwrap().status & status::DIED != 0);
    assert_eq!((mana(&sim, dead), reserve(&sim, dead)), (100, 0));
    assert!(!evoking(&sim, dead));
    let seen2 = observed(|| {
        ticks(&mut sim, 45);
    });
    assert_eq!(evokes(&seen) + evokes(&seen2), 0, "0x220 does not fire");
}

// QUIRK C-277: kept
#[test]
fn a_new_cast_aborts_the_running_one() {
    let rules = tk::evoke_rules();
    // Via `send` (synchronous, before tick N+10): the cast aborts itself and refunds.
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    ticks(&mut sim, 9);
    assert_eq!(send(&mut sim, ev, &cast_message(0, WOLF_BUTTON)), 1);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (75, 25));
    assert!(evoking(&sim, ev) && pending_evoke(&sim, ev));
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    let base = &sim.module(ev, INDEX).unwrap().base;
    assert_eq!(
        (base.events.len(), base.timers_ws.len()),
        (1, 0),
        "one timer"
    );
    let seen2 = observed(|| {
        ticks(&mut sim, 90);
    });
    assert_eq!(evokes(&seen) + evokes(&seen2), 1);
    // Via the FIFO with abort (`abort_all_modules`, then the cast).
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    ticks(&mut sim, 9);
    assert!(post_message(
        &mut sim,
        ev,
        cast_message(0, WOLF_BUTTON),
        true
    ));
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (75, 25));
    assert!(evoking(&sim, ev) && pending_evoke(&sim, ev));
    let base = &sim.module(ev, INDEX).unwrap().base;
    assert_eq!(
        (base.events.len(), base.timers_ws.len()),
        (1, 0),
        "one timer"
    );
    let seen2 = observed(|| {
        ticks(&mut sim, 90);
    });
    assert_eq!(evokes(&seen) + evokes(&seen2), 1);
}

// QUIRK C-276: kept
#[test]
fn mana_regenerates_up_to_max_minus_reserve() {
    // +100 per tick up to 74,500, then `current_value` shows 75 and the tick sets
    // 75,000.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    evoke_animal_mut(&mut sim, ev).unwrap().reserve = 25;
    sim.object_mut(ev)
        .unwrap()
        .attributes
        .set_current_fraction(MANA, 50_000);
    for k in 1..=245 {
        ticks(&mut sim, 1);
        assert_eq!(fraction(&sim, ev), 50_000 + 100 * k, "tick {k}");
    }
    assert_eq!(fraction(&sim, ev), 74_500);
    for _ in 0..5 {
        ticks(&mut sim, 1);
        assert_eq!(fraction(&sim, ev), 75_000);
    }
}

// QUIRK C-276: kept
#[test]
fn release_frees_the_reserve() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    evoke_animal_mut(&mut sim, ev).unwrap().reserve = 40;
    assert_eq!(send(&mut sim, ev, &Message::new(msg::RELEASE, &[0, 25])), 1);
    assert_eq!(reserve(&sim, ev), 15);
}

// QUIRK C-293: kept
#[test]
fn a_cast_without_label_does_nothing_but_clears_bit_4() {
    // C-293: animation 5 has no marker.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    evoke_animal_mut(&mut sim, ev).unwrap().anim = 5;
    set_object_event_bit(&mut sim, ev, 4, true);
    assert_eq!(send(&mut sim, ev, &cast_message(1, BEAR_BUTTON)), 0);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (100, 0));
    assert_eq!(object_event_bits(&sim, ev) & 0x10, 0, "event bit 4 cleared");
    let e = evoke_animal(&sim, ev).unwrap();
    assert_eq!((e.cost, e.button, e.art), (50, BEAR_BUTTON, 0));
    assert!(!evoking(&sim, ev));
    assert!(!pending_evoke(&sim, ev));
}

#[test]
fn the_cast_works_as_an_envelope() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    let other = evoker(&mut sim, (20, 20));
    let ok = Message::new(0x777, &[]);
    let fail = Message::new(0x778, &[]);
    let env = Envelope {
        msg: cast_message(0, WOLF_BUTTON),
        success: Some(ok.clone()),
        fault: Some(fail.clone()),
        success_target: other,
        fault_target: ObjectId::NONE,
    };
    assert_eq!(dispatch_command(&mut sim, ev, &env), 1);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (75, 25));
    assert!(evoking(&sim, ev) && pending_evoke(&sim, ev));
    // Both feedback messages set, none sent.
    let base = &sim.module(ev, INDEX).unwrap().base;
    assert_eq!(
        (base.success.as_ref(), base.success_target),
        (Some(&ok), other)
    );
    assert_eq!(
        (base.fault.as_ref(), base.fault_target),
        (Some(&fail), ObjectId::NONE)
    );
    // Other envelopes: `false`, no effect.
    let env = Envelope {
        msg: Message::new(msg::RELEASE, &[0, 25]),
        success: None,
        fault: None,
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    assert_eq!(dispatch_command(&mut sim, ev, &env), 0);
    assert_eq!(reserve(&sim, ev), 25);
    assert_eq!(
        sim.module(ev, INDEX).unwrap().base.success.as_ref(),
        Some(&ok)
    );
}

// ---- Appearing, animal `0x314`, death callbacks (re/evoke-aura 3.2, 3.2a, 3.10, 4) ----

/// Button "cat" (cost 30).
const CAT_BUTTON: i32 = 443;
/// Module index dying.
const DYING: usize = 0x10;

/// The object of the GUID `guid` created in `events`.
fn created(sim: &Sim, events: &[SimEvent], guid: u32) -> Option<ObjectId> {
    events.iter().find_map(|e| match e {
        SimEvent::Created { id } if sim.object(*id).is_some_and(|o| o.guid == guid) => Some(*id),
        _ => None,
    })
}

/// Posts `0x120 {kind, button}` with abort and lets N ... N+81 run (82 object ticks); return value:
/// the animal.
fn evoke(sim: &mut Sim, ev: ObjectId, art: i32, button: i32) -> ObjectId {
    assert!(post_message(sim, ev, cast_message(art, button), true));
    let events = ticks(sim, 82);
    let guid = ANIMAL_GUIDS[art as usize];
    created(sim, &events, guid).expect("animal appeared")
}

/// Hit points to 0; the object dies in the next object tick (`check_death`).
fn kill(sim: &mut Sim, id: ObjectId) {
    let o = sim.object(id).expect("object");
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 14);
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_value(14, 0, p);
}

/// `(current, max)` of the hit points.
fn hitpoints(sim: &Sim, id: ObjectId) -> (i32, i32) {
    let o = sim.object(id).expect("object");
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 14);
    (
        o.attributes.current_value(14, p),
        o.attributes.max_value(14, p),
    )
}

/// Cap of the regeneration `Max(15) − reserve`.
fn cap(sim: &Sim, id: ObjectId) -> i32 {
    let p = spell::mana_percent(sim, id);
    sim.object(id).unwrap().attributes.max_value(MANA, p) - reserve(sim, id)
}

fn alpha(sim: &Sim, id: ObjectId) -> f32 {
    crate::module::animal::animal(sim, id)
        .expect("animal")
        .alpha
}

fn listeners(sim: &Sim, id: ObjectId) -> Vec<(ObjectId, Message)> {
    match &sim.module(id, DYING).expect("dying module").kind {
        ModuleKind::Dying(d) => d.listeners.iter().cloned().collect(),
        _ => unreachable!(),
    }
}

/// The only living or dead object with the GUID `guid`.
fn only(sim: &Sim, guid: u32) -> ObjectId {
    let all: Vec<ObjectId> = sim
        .state
        .objects()
        .filter(|o| o.guid == guid)
        .map(|o| o.id)
        .collect();
    assert_eq!(all.len(), 1, "exactly one object {guid}");
    all[0]
}

fn dead(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_some_and(|o| o.status & status::DIED != 0)
}

/// Number of `0x320` to module 32.
fn releases(seen: &[(ObjectId, usize, u32)]) -> usize {
    seen.iter()
        .filter(|&&(_, index, code)| index == INDEX && code == msg::RELEASE)
        .count()
}

// QUIRK C-284: kept
#[test]
fn a_wolf_appears_at_n_plus_81() {
    // V1, C-284 corrected.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    let spawn = evoke_animal(&sim, ev).unwrap().spawn;
    let events = ticks(&mut sim, 80);
    assert_eq!(created(&sim, &events, tk::WOLF), None, "no wolf until N+80");
    // N+81.
    let events = ticks(&mut sim, 1);
    let wolf = created(&sim, &events, tk::WOLF).expect("wolf in N+81");
    assert_eq!(wolf.slot(), 0);
    assert_eq!(grid(&sim, wolf), (spawn[0], spawn[2]), "exactly at spawn");
    assert!(!evoking(&sim, ev));
    assert_ne!(object_event_bits(&sim, ev) & 0x10, 0, "event bit 4 on");
    let o = sim.object(wolf).unwrap();
    assert_eq!(
        sim.module(wolf, crate::module::animal::INDEX)
            .unwrap()
            .base
            .flags,
        0x2100 | 0x200,
        "IS_EVOKED"
    );
    assert!(o.has_module(crate::module::edible::INDEX, true));
    assert!(
        !o.has_module(crate::module::edible::INDEX, false),
        "module 21 off"
    );
    // End state after tick N+81: the pending `0x15` off (re/evoke-aura 3.2a) is applied.
    assert!(!o.tick_list.contains(&(crate::module::edible::INDEX as u8)));
    assert!(o.pending_activation.is_empty());
    let a = crate::module::animal::animal(&sim, wolf).unwrap();
    assert!(!a.aggressive && !a.shy && !a.roaming);
    assert_eq!(
        hitpoints(&sim, wolf),
        (160, 160),
        "C-284: base value doubled"
    );
    // Callbacks at the wolf: `0x114` to itself, `0x320 {0, 25}` to the evoker.
    let mut l = listeners(&sim, wolf);
    l.sort_by_key(|e| e.0 != wolf);
    assert_eq!(
        l,
        [
            (wolf, Message::new(0x114, &[])),
            (ev, Message::new(msg::RELEASE, &[0, 25])),
        ]
    );
    let pos = sim.object(wolf).unwrap().world_pos;
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::Sound3d { object, action: 0x1E, flags: 0x42, pos: p, guid: 441 }
            if *object == wolf && *p == pos)));
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::EffectOnObject { object, path }
            if *object == wolf && path.as_str() == "data/effects/Wolf_erscheinen.pef")));
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (75, 25));
    // Alpha: measured, the wolf already ticks in N+81 (`0x414` before its first tick). 0.0125 in
    // `f32` added 80 times is 0.99999934, 1.0 only in the 81st wolf tick (clamp).
    assert_eq!(alpha(&sim, wolf), 0.0125, "one wolf tick in N+81");
    ticks(&mut sim, 79);
    let a80 = alpha(&sim, wolf);
    assert!((0.9999..1.0).contains(&a80), "after 80 wolf ticks {a80}");
    ticks(&mut sim, 1);
    assert_eq!(alpha(&sim, wolf), 1.0);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (75, 25));
}

#[test]
fn a_spawn_outside_the_map_aborts_and_refunds() {
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    // Test hook: position outside the map -> `create_object` fails.
    evoke_animal_mut(&mut sim, ev).unwrap().spawn = [-TILE, 0, -TILE];
    let events = ticks(&mut sim, 81);
    assert_eq!(created(&sim, &events, tk::WOLF), None);
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (100, 0));
    assert!(!evoking(&sim, ev));
}

/// Hunting filter like `hunting::search_prey` without fog and cell search: module `0x15` (switched on)
/// with "has meat", also dead.
fn huntable(sim: &mut Sim, id: ObjectId) -> bool {
    let f = opennml_formats::targets::MatchFilter {
        mode: 0x400_0016,
        mode2: 0,
        match_bits: 0x215,
        prop_id: 0x15,
        ..opennml_formats::targets::MatchFilter::constructed()
    };
    crate::filter::matches(sim, id, &f, -1)
}

// QUIRK C-146, C-276, C-278: kept
#[test]
fn a_dead_wolf_frees_the_reserve_and_stays_an_ordinary_corpse() {
    // V6, C-146, C-278.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    let wolf = evoke(&mut sim, ev, 0, WOLF_BUTTON);
    ticks(&mut sim, 5);
    assert_eq!(fraction(&sim, ev), 75_000);
    // Tick T: hit points 0 -> `check_death` -> `start_dying`, ownership to 8 in phase 0.
    kill(&mut sim, wolf);
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert_eq!(releases(&seen), 0, "T: no callback yet");
    let corpse = only(&sim, tk::WOLF);
    assert_ne!(corpse, wolf, "new ID");
    assert_eq!(corpse.slot(), 8);
    assert!(dead(&sim, corpse));
    assert_eq!(reserve(&sim, ev), 25);
    // T+1: dying tick after the change of ownership - `0x320 {carcass, 25}` to the evoker.
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert_eq!(releases(&seen), 1);
    assert!(seen.contains(&(ev, INDEX, msg::RELEASE)));
    assert_eq!(reserve(&sim, ev), 0);
    // `0x114` was lost on the old ID: no fade-out, animal module never asked.
    assert!(!seen.iter().any(|&(_, i, c)| i == 0x14 && c == 0x114));
    assert_eq!(
        crate::module::animal::animal(&sim, corpse)
            .unwrap()
            .alpha_speed,
        0.0125,
        "keeps fading in"
    );
    assert!(listeners(&sim, corpse).is_empty());
    // Mana fraction +100 per tick up to 99,500, then 100,000.
    let f1 = fraction(&sim, ev);
    assert_eq!(f1, 75_000, "the evoker ticks in T+1 before the callback");
    let mut k = 0;
    while fraction(&sim, ev) < 99_500 {
        let before = fraction(&sim, ev);
        ticks(&mut sim, 1);
        k += 1;
        assert_eq!(fraction(&sim, ev), before + 100);
    }
    assert_eq!(mana(&sim, ev), 100, "current_value rounds up");
    ticks(&mut sim, 1);
    assert_eq!(fraction(&sim, ev), 100_000);
    assert_eq!(k, 245, "current_value shows 100 after 245 ticks");
    // Carcass: no `0x215`, module 21 off, no hunting; `0x210` after 30 s, removed afterward.
    let base = &sim.module(corpse, DYING).unwrap().base;
    assert!(!base.has_pending_timer(0x215));
    assert!(!huntable(&mut sim, corpse));
    let mut decay = None;
    let mut t = 1 + k + 1;
    loop {
        if sim.object(corpse).is_none() {
            break;
        }
        let seen = observed(|| {
            ticks(&mut sim, 1);
        });
        t += 1;
        if seen.contains(&(corpse, DYING, 0x210)) {
            decay = Some(t);
        }
        assert!(!seen.iter().any(|&(_, _, c)| c == 0x215));
        assert!(t < 1000);
    }
    // As in re/evoke-aura 3.10: `0x210` in T+303, removed in T+332 (re/combat-death 2.4).
    assert_eq!((decay, t), (Some(303), 332), "decay/removed in T+...");
}

// QUIRK C-146: kept
#[test]
fn a_wild_wolf_corpse_is_huntable() {
    // Counter-check to C-146: without `0x314` module 21 stays on and the carcass gets `0x215`.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let wolf = tk::spawn(&mut sim, tk::WOLF, 0, (10, 10));
    kill(&mut sim, wolf);
    ticks(&mut sim, 2);
    let corpse = only(&sim, tk::WOLF);
    assert!(dead(&sim, corpse));
    assert!(huntable(&mut sim, corpse));
    let base = &sim.module(corpse, DYING).unwrap().base;
    assert!(base.has_pending_timer(0x215));
}

// QUIRK C-146: kept
#[test]
fn the_evoked_message_queues_the_meat_module_off() {
    // re/evoke-aura 3.2a, `0x004FEC23`: `GameObject_QueueModuleTickChange(0x15, 0)` instead of
    // `Module_SetEnabled` - `m_Enabled` stays until it is applied in the next object tick.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let wolf = tk::spawn(&mut sim, tk::WOLF, 0, (10, 10));
    ticks(&mut sim, 1);
    let meat = crate::module::edible::INDEX;
    send(&mut sim, wolf, &Message::new(0x314, &[0]));
    let o = sim.object(wolf).unwrap();
    assert!(o.has_module(meat, false), "module 21 on until applied");
    assert!(o.tick_list.contains(&(meat as u8)));
    assert_eq!(o.pending_activation, [!(meat as i32)]);
    // After the object tick: off the tick and off, pending list empty.
    ticks(&mut sim, 1);
    let o = sim.object(wolf).unwrap();
    assert!(
        o.has_module(meat, true) && !o.has_module(meat, false),
        "module 21 off"
    );
    assert!(!o.tick_list.contains(&(meat as u8)));
    assert!(o.pending_activation.is_empty());
}

// QUIRK C-279: kept
#[test]
fn the_evoker_death_leaves_the_wolves() {
    // C-279.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    let wolf = evoke(&mut sim, ev, 0, WOLF_BUTTON);
    kill(&mut sim, ev);
    ticks(&mut sim, 3);
    let body = only(&sim, tk::ANIMAL_EVOKER);
    assert!(dead(&sim, body));
    assert_ne!(body, ev);
    assert!(
        sim.object(wolf)
            .is_some_and(|o| o.status & status::DIED == 0)
    );
    assert_eq!(wolf.slot(), 0);
    assert_eq!(hitpoints(&sim, wolf), (160, 160));
    // The wolf dies afterward: `0x320` reaches nobody (old ID of the evoker).
    kill(&mut sim, wolf);
    let seen = observed(|| {
        ticks(&mut sim, 3);
    });
    assert!(dead(&sim, only(&sim, tk::WOLF)));
    assert_eq!(releases(&seen), 0);
    assert_eq!(reserve(&sim, body), 25);
}

// QUIRK C-279: kept
#[test]
fn the_evoker_owner_change_keeps_the_reserve() {
    // C-279: a living change of ownership changes the ID, later `0x320` lapse.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    let wolf = evoke(&mut sim, ev, 0, WOLF_BUTTON);
    crate::world::queue_owner_change(&mut sim, ev, 1);
    ticks(&mut sim, 1);
    let moved = only(&sim, tk::ANIMAL_EVOKER);
    assert_eq!(moved.slot(), 1);
    assert!(!dead(&sim, moved));
    assert_eq!(reserve(&sim, moved), 25);
    kill(&mut sim, wolf);
    let seen = observed(|| {
        ticks(&mut sim, 3);
    });
    assert!(dead(&sim, only(&sim, tk::WOLF)));
    assert_eq!(releases(&seen), 0);
    assert_eq!(reserve(&sim, moved), 25);
}

// QUIRK C-284: kept
#[test]
fn the_evoked_animal_doubles_its_base_hitpoints() {
    // C-284 corrected (Ghidra `0x00510A89…0x00510AAA`): key-0 absolute value = base value.
    let rules = tk::evoke_rules();
    for (art, button, guid, hp) in [
        (0, WOLF_BUTTON, tk::WOLF, 160),
        (1, BEAR_BUTTON, tk::BEAR, 240),
        (2, CAT_BUTTON, tk::CAT, 160),
    ] {
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ev = evoker(&mut sim, (10, 10));
        let animal = evoke(&mut sim, ev, art, button);
        assert_eq!(sim.object(animal).unwrap().guid, guid);
        assert_eq!(hitpoints(&sim, animal), (hp, hp), "kind {art}");
        assert_eq!(sim.object(animal).unwrap().attributes.base(14), hp);
    }
    // Config remainder of the last placed object (`world.rs` `apply_placement`): 10 -> 20.
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    sim.state.object_config.insert(14, 10);
    let wolf = evoke(&mut sim, ev, 0, WOLF_BUTTON);
    assert_eq!(hitpoints(&sim, wolf), (20, 20));
}

// QUIRK C-276: kept
#[test]
fn four_wolves_then_the_fifth_fails() {
    // V4.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    for (n, want) in [(1, 75), (2, 50), (3, 25), (4, 0)] {
        evoke(&mut sim, ev, 0, WOLF_BUTTON);
        ticks(&mut sim, 1);
        assert_eq!(mana(&sim, ev), want, "wolf {n}");
        assert_eq!(reserve(&sim, ev), 100 - want);
        assert_eq!(cap(&sim, ev), want);
    }
    // Fifth command with abort: the walk order ends, nothing else (mana 0 < 25).
    crate::tick::step(sim.state, sim.rules, &[walk_to(ev, (30, 10))]);
    crate::tick::step(sim.state, sim.rules, &[]);
    ticks(&mut sim, 5);
    let walking = grid(&sim, ev);
    ticks(&mut sim, 1);
    assert_ne!(grid(&sim, ev), walking, "walks");
    assert!(post_message(
        &mut sim,
        ev,
        cast_message(0, WOLF_BUTTON),
        true
    ));
    let events = ticks(&mut sim, 3);
    let stopped = grid(&sim, ev);
    let events2 = ticks(&mut sim, 85);
    assert_eq!(grid(&sim, ev), stopped, "walk order aborted");
    assert!(!evoking(&sim, ev) && !pending_evoke(&sim, ev));
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (0, 100));
    assert_eq!(created(&sim, &events, tk::WOLF), None);
    assert_eq!(created(&sim, &events2, tk::WOLF), None);
    let wolves = sim.state.objects().filter(|o| o.guid == tk::WOLF).count();
    assert_eq!(wolves, 4);
}

// QUIRK C-276: kept
#[test]
fn bears_and_cats_fit_by_cost() {
    // V5.
    let rules = tk::evoke_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    for _ in 0..2 {
        evoke(&mut sim, ev, 1, BEAR_BUTTON);
    }
    ticks(&mut sim, 1);
    assert_eq!(
        (mana(&sim, ev), reserve(&sim, ev), cap(&sim, ev)),
        (0, 100, 0)
    );
    assert!(post_message(
        &mut sim,
        ev,
        cast_message(1, BEAR_BUTTON),
        true
    ));
    let events = ticks(&mut sim, 85);
    assert_eq!(created(&sim, &events, tk::BEAR), None, "third bear fails");

    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = evoker(&mut sim, (10, 10));
    for _ in 0..3 {
        evoke(&mut sim, ev, 2, CAT_BUTTON);
    }
    ticks(&mut sim, 1);
    assert_eq!(
        (mana(&sim, ev), reserve(&sim, ev), cap(&sim, ev)),
        (10, 90, 10)
    );
    assert!(post_message(
        &mut sim,
        ev,
        cast_message(2, CAT_BUTTON),
        true
    ));
    let events = ticks(&mut sim, 85);
    assert_eq!(created(&sim, &events, tk::CAT), None, "fourth cat fails");
    assert_eq!((mana(&sim, ev), reserve(&sim, ev)), (10, 90));
}
