//! Tests of the ghosts and banishing (re/ghosts-banish): module 26 "Evoke ghost" (2.1, 3.1-3.3,
//! 8 no. 1-5, 9.1, 9.2), module 27 "Ghost" and module 31 "Banish". "N" is the object tick in
//! which `0x41A` takes effect; ticks are object ticks (two `tick::step`).

use std::cell::RefCell;

use crate::combat::rank::set_rank_reset_experience;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{abort_all_modules, dispatch_command, post_message, send};
use crate::module::ability_testkit as tk;
use crate::module::{ModuleKind, attack, dying, spell, test_hook};
use crate::object::status;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::world::{destroy_object, object_event_bits, set_object_event_bit};

use super::evoke::{self, evoke_ghost, msg};
use super::{banish, ghost};

/// Mana (attribute 15).
const MANA: u32 = 15;
/// Button "Evoke ghost" (cost 25).
const BUTTON: i32 = 436;
/// Death flag `MID_PROP_DIE_GHOST` (corpse consumed, C-154).
const GHOST_FLAG: u32 = 0x100;

thread_local! {
    /// Queries `(object, module, code)` during [`observed`].
    static SEEN: RefCell<Vec<(ObjectId, usize, u32)>> = const { RefCell::new(Vec::new()) };
}

fn record(_: &mut Sim, id: ObjectId, index: usize, m: &Message) {
    SEEN.with(|s| s.borrow_mut().push((id, index, m.code)));
}

/// Runs `f` with the observer (`module::test_hook::observe`) and returns all delivered
/// queries in order (e.g. `0x21B` at the ghost, `0x511`/`0x411` at Attack).
fn observed(f: impl FnOnce()) -> Vec<(ObjectId, usize, u32)> {
    SEEN.with(|s| s.borrow_mut().clear());
    test_hook::observe(record, f);
    SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

fn got(seen: &[(ObjectId, usize, u32)], id: ObjectId, index: usize, code: u32) -> bool {
    seen.contains(&(id, index, code))
}

/// `n` object ticks; returns: all events.
fn ticks(sim: &mut Sim, n: usize) -> Vec<SimEvent> {
    let mut events = Vec::new();
    for _ in 0..2 * n {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    }
    events
}

fn world(rules: &Rules) -> SimState {
    tk::state(rules)
}

fn set_mana(sim: &mut Sim, id: ObjectId, v: i32) {
    let p = spell::mana_percent(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_current_value(MANA, v, p);
    }
}

/// `(base value, max, current)` of the mana.
fn mana(sim: &Sim, id: ObjectId) -> (i32, i32, i32) {
    let p = spell::mana_percent(sim, id);
    let a = &sim.object(id).expect("object").attributes;
    (a.base(MANA), a.max_value(MANA, p), a.current_value(MANA, p))
}

/// Ghost evoker on slot 0 with mana 150 (explicitly set).
fn evoker(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    let id = tk::spawn(sim, tk::GHOST_EVOKER, 0, tile);
    set_mana(sim, id, 150);
    id
}

/// Corpses: create victims, send `0x110` and run one object tick. `DIED` is set immediately,
/// the new ID (ownership at slot 8) only comes in phase 0.
fn corpses(sim: &mut Sim, victims: &[(u32, i32, (i32, i32))]) -> Vec<ObjectId> {
    let ids: Vec<ObjectId> = victims
        .iter()
        .map(|&(guid, slot, tile)| tk::spawn(sim, guid, slot, tile))
        .collect();
    for &v in &ids {
        send(sim, v, &Message::new(0x110, &[]));
    }
    let events = ticks(sim, 1);
    ids.iter()
        .map(|&v| {
            events
                .iter()
                .find_map(|e| match e {
                    SimEvent::OwnerChanged { old, new } if *old == v => Some(*new),
                    _ => None,
                })
                .expect("corpse on slot 8")
        })
        .collect()
}

fn corpse(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    corpses(sim, &[(guid, slot, tile)])[0]
}

fn evoking(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.status & status::EVOKING != 0)
}

fn ghosts(sim: &Sim) -> Vec<ObjectId> {
    sim.state
        .objects()
        .filter(|o| o.guid == tk::GHOST)
        .map(|o| o.id)
        .collect()
}

fn ghost_flag(sim: &Sim, body: ObjectId) -> bool {
    sim.module(body, dying::INDEX)
        .is_some_and(|m| m.base.flags & GHOST_FLAG != 0)
}

fn post_evoke(sim: &mut Sim, id: ObjectId) {
    assert!(post_message(
        sim,
        id,
        Message::new(msg::EVOKE, &[BUTTON]),
        true
    ));
}

/// Posts `0x11A {436}` and runs until tick N (`EVOKING` on); returns: the number of ticks.
fn start(sim: &mut Sim, id: ObjectId) -> usize {
    post_evoke(sim, id);
    for k in 1..=60 {
        ticks(sim, 1);
        if evoking(sim, id) {
            return k;
        }
    }
    panic!("no evocation");
}

/// A complete evocation up to N+40; returns: the new ghost.
fn evoke_fully(sim: &mut Sim, id: ObjectId) -> ObjectId {
    start(sim, id);
    ticks(sim, 40);
    assert!(!evoking(sim, id), "N+40: EVOKING off");
    let g = evoke_ghost(sim, id).expect("module 26").ghost;
    assert!(sim.object(g).is_some(), "ghost present");
    g
}

fn listens(sim: &Sim, ghost: ObjectId, evoker: ObjectId) -> bool {
    match sim.module(ghost, dying::INDEX).map(|m| &m.kind) {
        Some(ModuleKind::Dying(d)) => d
            .listeners
            .iter()
            .any(|(h, m)| *h == evoker && m.code == msg::GHOST_DIED),
        _ => false,
    }
}

const ATTACK: u8 = attack::INDEX as u8;

// QUIRK C-145, C-255: kept
#[test]
fn the_evocation_follows_its_timeline() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let body = corpse(&mut sim, tk::COLONIST, 1, (10, 5));
    // Already facing the corpse (created objects face -z, direction -90 degrees): FINDTO
    // reports success synchronously.
    let ev = evoker(&mut sim, (10, 10));
    set_object_event_bit(&mut sim, ev, 4, true);
    post_evoke(&mut sim, ev);
    // N: mana, EVOKING, attack off (taken over after the tick), animation 0, corpse
    // consumed.
    let events = ticks(&mut sim, 1);
    let o = sim.object(ev).unwrap();
    assert!(evoking(&sim, ev));
    assert_eq!(o.anim.wanted, 0);
    assert!(!o.tick_list.contains(&ATTACK));
    assert!(!o.has_module(attack::INDEX, false));
    assert!(o.has_module(attack::INDEX, true));
    assert_eq!(object_event_bits(&sim, ev) & 0x10, 0, "event bit 4 off");
    assert_eq!(mana(&sim, ev), (125, 125, 125));
    let e = evoke_ghost(&sim, ev).unwrap().clone();
    assert_eq!((e.dead_body, e.extra_energy, e.spell_guid), (body, 25, 436));
    let g = sim.object(body).unwrap().grid;
    assert_eq!(e.dead_body_pos, [g.x.0, g.y.0, g.z.0]);
    assert!(ghost_flag(&sim, body), "0x510 {{1}} to the corpse");
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::Sound3d { object, action: 0x1E, flags: 0x42, guid: 436, .. } if *object == ev)));
    let base = &sim.module(ev, evoke::INDEX).unwrap().base;
    assert!(base.has_pending_timer(msg::CREATE));
    assert!(base.has_pending_timer(msg::STOP_ANIM));
    // N+1, N+2: no ghost yet.
    ticks(&mut sim, 2);
    assert!(ghosts(&sim).is_empty());
    // N+3: ghost 1413 on slot 0 in the tile of the corpse, callback registered.
    let events = ticks(&mut sim, 1);
    let gs = ghosts(&sim);
    assert_eq!(gs.len(), 1);
    let ghost = gs[0];
    assert_eq!(ghost.slot(), 0);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().ghost, ghost);
    let (go, bo) = (sim.object(ghost).unwrap(), sim.object(body).unwrap());
    assert_eq!(go.tile, bo.tile);
    // Position via `grid_to_world` (f32) and back: not guaranteed bit-identical.
    assert!((go.grid.x.0 - bo.grid.x.0).abs() <= 1, "x");
    assert!((go.grid.z.0 - bo.grid.z.0).abs() <= 1, "z");
    assert_eq!(go.color, crate::setup::player_color(&sim, 0));
    assert!(listens(&sim, ghost, ev));
    let gpos = go.world_pos;
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::Sound3d { object, action: 0x1F, flags: 0x42, guid: 436, pos }
            if *object == ghost && *pos == gpos)));
    assert!(evoking(&sim, ev));
    // N+39 still on, N+40 (marker 3,999 ms): EVOKING off, attack on again, bit 4 on.
    ticks(&mut sim, 36);
    assert!(evoking(&sim, ev));
    ticks(&mut sim, 1);
    assert!(!evoking(&sim, ev));
    assert_ne!(object_event_bits(&sim, ev) & 0x10, 0, "event bit 4 on");
    let o = sim.object(ev).unwrap();
    assert!(o.tick_list.contains(&ATTACK));
    assert!(o.has_module(attack::INDEX, false));
    assert_eq!(mana(&sim, ev), (125, 125, 125));
}

// QUIRK C-255: kept
#[test]
fn the_evoker_turns_before_it_starts() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Corpse behind the evoker (facing -z, corpse at +z).
    let body = corpse(&mut sim, tk::COLONIST, 1, (10, 15));
    let ev = evoker(&mut sim, (10, 10));
    let pos = sim.object(ev).unwrap().grid;
    post_evoke(&mut sim, ev);
    ticks(&mut sim, 1);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, body);
    assert!(!evoking(&sim, ev), "turn first");
    assert!(!ghost_flag(&sim, body));
    let mut k = 1;
    while !evoking(&sim, ev) {
        ticks(&mut sim, 1);
        k += 1;
        assert!(k < 60, "no evocation");
    }
    // `0x41A` comes after turning in the walk tick.
    assert_eq!(k, TURN_TICKS);
    assert!(ghost_flag(&sim, body));
    ticks(&mut sim, 5);
    let o = sim.object(ev).unwrap();
    assert_eq!(
        (o.grid.x, o.grid.z),
        (pos.x, pos.z),
        "the evoker does not walk"
    );
}

/// Measured: after a turn of 180 degrees `0x41A` takes effect in the third object tick after posting (with the
/// evoker facing the corpse: in the first, synchronously in the FIFO).
const TURN_TICKS: usize = 3;

// QUIRK C-257: kept
#[test]
fn six_ghosts_bind_all_mana() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let bodies = corpses(
        &mut sim,
        &[
            (tk::COLONIST, 1, (14, 10)),
            (tk::COLONIST, 1, (10, 14)),
            (tk::COLONIST, 1, (6, 10)),
            (tk::COLONIST, 1, (10, 6)),
            (tk::COLONIST, 1, (13, 13)),
            (tk::COLONIST, 1, (7, 7)),
            (tk::COLONIST, 1, (13, 7)),
        ],
    );
    let ev = evoker(&mut sim, (10, 10));
    let mut made = Vec::new();
    for expect in [125, 100, 75, 50, 25, 0] {
        made.push(evoke_fully(&mut sim, ev));
        let (base, max, cur) = mana(&sim, ev);
        assert_eq!((base, max), (expect, expect), "base value and max");
        assert!(cur <= max);
    }
    assert_eq!(bodies.iter().filter(|&&b| ghost_flag(&sim, b)).count(), 6);
    // The seventh: current 0 < 25 -> -1, no effect (the seventh corpse stays free).
    assert_eq!(send(&mut sim, ev, &Message::new(msg::EVOKE, &[BUTTON])), -1);
    assert!(!evoking(&sim, ev));
    assert!(!ghost_flag(&sim, bodies[6]));
    // Ghost 3 dies: base value +25, current value unchanged (C-257).
    let before = mana(&sim, ev).2;
    send(&mut sim, made[2], &Message::new(0x110, &[]));
    assert_eq!(mana(&sim, ev).0, 25);
    assert_eq!(mana(&sim, ev).1, 25);
    assert_eq!(mana(&sim, ev).2, before);
}

// QUIRK C-258: kept
#[test]
fn aborting_in_the_window_refunds_twice() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpse(&mut sim, tk::COLONIST, 1, (10, 5));
    let ev = evoker(&mut sim, (10, 10));
    assert_eq!(start(&mut sim, ev), 1);
    ticks(&mut sim, 3);
    let ghost = evoke_ghost(&sim, ev).unwrap().ghost;
    assert!(sim.object(ghost).is_some(), "N+3: ghost there");
    ticks(&mut sim, 2);
    assert!(evoking(&sim, ev));
    // Abort in N+5 (window `0x21A`...`0x31A`): costs back, the ghost gets `0x21B` (C-258).
    let seen = observed(|| abort_all_modules(&mut sim, ev));
    assert!(got(&seen, ghost, ghost::INDEX, 0x21B));
    assert!(!evoking(&sim, ev));
    assert_eq!(mana(&sim, ev).0, 150);
    // Timers stay (C-260); attack queued on and taken over after the tick.
    assert!(
        sim.module(ev, evoke::INDEX)
            .unwrap()
            .base
            .has_pending_timer(msg::STOP_ANIM)
    );
    assert!(
        sim.object(ev)
            .unwrap()
            .pending_activation
            .contains(&(attack::INDEX as i32))
    );
    ticks(&mut sim, 1);
    let o = sim.object(ev).unwrap();
    assert!(o.tick_list.contains(&ATTACK) && o.has_module(attack::INDEX, false));
    // Death of the ghost (normally after 34 ticks of fade-out; here directly): another +25.
    send(&mut sim, ghost, &Message::new(0x110, &[]));
    assert_eq!(mana(&sim, ev).0, 175);
}

// QUIRK C-259: kept
#[test]
fn aborting_before_the_ghost_fades_the_previous_one() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpses(
        &mut sim,
        &[(tk::COLONIST, 1, (10, 5)), (tk::COLONIST, 1, (10, 3))],
    );
    let ev = evoker(&mut sim, (10, 10));
    let a = evoke_fully(&mut sim, ev);
    assert_eq!(mana(&sim, ev).0, 125);
    assert_eq!(start(&mut sim, ev), 1);
    assert_eq!(mana(&sim, ev).0, 100);
    ticks(&mut sim, 1);
    // Abort in N+1 (before `0x21A`): `ghost` is still A - A gets `0x21B` (C-259).
    let seen = observed(|| abort_all_modules(&mut sim, ev));
    assert!(got(&seen, a, ghost::INDEX, 0x21B));
    assert_eq!(mana(&sim, ev).0, 125);
    // The old `0x21A` in N+3 does nothing without EVOKING.
    ticks(&mut sim, 2);
    assert_eq!(ghosts(&sim), [a]);
    // A's death returns its costs: base value 150 again.
    send(&mut sim, a, &Message::new(0x110, &[]));
    assert_eq!(mana(&sim, ev).0, 150);
}

// QUIRK C-260: kept
#[test]
fn a_quick_restart_makes_two_ghosts() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Two corpses on the same facing axis, the second one a tile behind.
    let bodies = corpses(
        &mut sim,
        &[(tk::COLONIST, 1, (10, 5)), (tk::COLONIST, 1, (10, 4))],
    );
    let ev = evoker(&mut sim, (10, 10));
    assert_eq!(start(&mut sim, ev), 1);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, bodies[0]);
    // N+1: new `0x11A` with abort - the abort refunds, the search finds corpse 2, `0x41A` synchronously.
    post_evoke(&mut sim, ev);
    ticks(&mut sim, 1);
    assert!(evoking(&sim, ev));
    assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, bodies[1]);
    assert_eq!(mana(&sim, ev).0, 125);
    ticks(&mut sim, 1);
    assert!(ghosts(&sim).is_empty(), "N+2");
    // N+3: the old `0x21A` creates ghost A at the new corpse position.
    ticks(&mut sim, 1);
    let gs = ghosts(&sim);
    assert_eq!(gs.len(), 1);
    let a = gs[0];
    let tile2 = sim.object(bodies[1]).unwrap().tile;
    assert_eq!(sim.object(a).unwrap().tile, tile2);
    assert_eq!(mana(&sim, ev).0, 125);
    // N+4: the new `0x21A` creates ghost B at the same position.
    ticks(&mut sim, 1);
    let gs = ghosts(&sim);
    assert_eq!(gs.len(), 2);
    let b = *gs.iter().find(|&&g| g != a).unwrap();
    assert_eq!(sim.object(b).unwrap().tile, tile2);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().ghost, b);
    assert_eq!(mana(&sim, ev).0, 125);
    // N+40: the old `0x31A` ends EVOKING (the new one would come in N+41).
    ticks(&mut sim, 35);
    assert!(evoking(&sim, ev), "N+39");
    ticks(&mut sim, 1);
    assert!(!evoking(&sim, ev), "N+40");
    assert_eq!(mana(&sim, ev).0, 125, "one payment");
}

// QUIRK C-154, C-256: kept
#[test]
fn the_corpse_search_takes_any_unclaimed_dead_walker() {
    let rules = tk::evoke_rules();
    // Own, enemy and animal corpses are found (sent synchronously, facing the corpse).
    for (guid, slot) in [(tk::COLONIST, 0), (tk::COLONIST, 1), (tk::CAT, 1)] {
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let body = corpse(&mut sim, guid, slot, (10, 5));
        let ev = evoker(&mut sim, (10, 10));
        assert_eq!(send(&mut sim, ev, &Message::new(msg::EVOKE, &[BUTTON])), 0);
        assert_eq!(
            evoke_ghost(&sim, ev).unwrap().dead_body,
            body,
            "{guid}/{slot}"
        );
        assert!(evoking(&sim, ev), "{guid}/{slot}");
        // Outside of a tick "attack off" is only queued.
        let o = sim.object(ev).unwrap();
        assert!(o.pending_activation.contains(&!(attack::INDEX as i32)));
        assert!(o.tick_list.contains(&ATTACK));
    }
    // An already consumed corpse (death flag `0x100`) is not: 0, no mana.
    {
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let body = corpse(&mut sim, tk::COLONIST, 1, (10, 5));
        send(&mut sim, body, &Message::new(0x510, &[1]));
        let ev = evoker(&mut sim, (10, 10));
        assert_eq!(send(&mut sim, ev, &Message::new(msg::EVOKE, &[BUTTON])), 0);
        let e = evoke_ghost(&sim, ev).unwrap();
        assert_eq!((e.dead_body, e.spell_guid), (ObjectId::NONE, 436));
        assert!(!evoking(&sim, ev));
        assert_eq!(mana(&sim, ev), (150, 150, 150));
    }
    // Two evokers in the same tick find the same corpse (both must turn first); the
    // flag only comes with `0x41A`, both evoke and pay.
    {
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let body = corpse(&mut sim, tk::COLONIST, 1, (10, 10));
        // A looks away from the corpse (180 degrees), B across to it (90 degrees).
        let a = evoker(&mut sim, (10, 7));
        let b = evoker(&mut sim, (13, 10));
        post_evoke(&mut sim, a);
        post_evoke(&mut sim, b);
        ticks(&mut sim, 1);
        for ev in [a, b] {
            assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, body);
            assert!(!evoking(&sim, ev));
        }
        assert!(!ghost_flag(&sim, body));
        for _ in 0..60 {
            if evoking(&sim, a) && evoking(&sim, b) {
                break;
            }
            ticks(&mut sim, 1);
        }
        assert!(evoking(&sim, a) && evoking(&sim, b));
        assert_eq!((mana(&sim, a).0, mana(&sim, b).0), (125, 125));
        assert!(ghost_flag(&sim, body));
    }
    // Building ruins never (no walk); nothing in the radius -> 0, no mana, no walk order.
    {
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Corpse 12 tiles away (outside 10 tiles + edge distance), dead ruin near.
        let far = corpse(&mut sim, tk::COLONIST, 1, (10, 8));
        let ruin = tk::spawn(&mut sim, tk::MONASTERY, 1, (10, 17));
        sim.object_mut(ruin)
            .unwrap()
            .set_status_flags(status::DIED, true);
        let ev = evoker(&mut sim, (10, 20));
        let seen = observed(|| {
            assert_eq!(send(&mut sim, ev, &Message::new(msg::EVOKE, &[BUTTON])), 0);
        });
        assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, ObjectId::NONE);
        assert!(!evoking(&sim, ev));
        assert_eq!(mana(&sim, ev), (150, 150, 150));
        assert!(!got(&seen, ev, evoke::INDEX, msg::START));
        assert!(!ghost_flag(&sim, far));
    }
}

#[test]
fn a_vanished_corpse_stops_the_evocation_without_cost() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let body = corpse(&mut sim, tk::COLONIST, 1, (10, 15));
    let ev = evoker(&mut sim, (10, 10));
    post_evoke(&mut sim, ev);
    ticks(&mut sim, 1);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, body);
    destroy_object(&mut sim, body);
    let seen = observed(|| {
        ticks(&mut sim, TURN_TICKS + 5);
    });
    assert!(
        got(&seen, ev, evoke::INDEX, msg::START),
        "0x41A after turning"
    );
    assert!(!evoking(&sim, ev));
    assert_eq!(mana(&sim, ev), (150, 150, 150));
    assert_eq!(send(&mut sim, ev, &Message::new(msg::START, &[])), 0);
    ticks(&mut sim, 5);
    assert!(ghosts(&sim).is_empty());
}

// QUIRK C-261: kept
#[test]
fn a_failed_ghost_keeps_the_mana_bound() {
    // Type 1413 is missing from the rules: `create_object` fails (C-261).
    let mut rules = tk::evoke_rules();
    rules.types.remove(&tk::GHOST);
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpse(&mut sim, tk::COLONIST, 1, (10, 5));
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    assert_eq!(mana(&sim, ev).0, 125);
    ticks(&mut sim, 3);
    assert!(ghosts(&sim).is_empty());
    assert_eq!(evoke_ghost(&sim, ev).unwrap().ghost, ObjectId::NONE);
    assert_eq!(mana(&sim, ev).0, 125);
    ticks(&mut sim, 37);
    assert!(!evoking(&sim, ev));
    assert_eq!(mana(&sim, ev).0, 125, "mana stays bound");
}

#[test]
fn a_dead_evoker_loses_the_refund() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpse(&mut sim, tk::COLONIST, 1, (10, 5));
    let ev = evoker(&mut sim, (10, 10));
    let ghost = evoke_fully(&mut sim, ev);
    send(&mut sim, ev, &Message::new(0x110, &[]));
    let events = ticks(&mut sim, 1);
    let dead = events
        .iter()
        .find_map(|e| match e {
            SimEvent::OwnerChanged { old, new } if *old == ev => Some(*new),
            _ => None,
        })
        .expect("evoker on slot 8");
    assert!(sim.object(ev).is_none());
    // The callback hangs on the old ID: it lapses, `0x61A` goes to nobody.
    let seen = observed(|| {
        send(&mut sim, ghost, &Message::new(0x110, &[]));
    });
    assert!(!seen.iter().any(|&(_, _, code)| code == msg::GHOST_DIED));
    assert_eq!(mana(&sim, dead).0, 125);
}

#[test]
fn a_button_without_values_is_free() {
    // Button without a value set: `extra_energy` 0, no check (remainder of C-281).
    let mut rules = tk::evoke_rules();
    rules.defaults.remove(&436);
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpse(&mut sim, tk::COLONIST, 1, (10, 5));
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().extra_energy, 0);
    assert_eq!(mana(&sim, ev), (150, 150, 150));
    ticks(&mut sim, 3);
    let ghost = evoke_ghost(&sim, ev).unwrap().ghost;
    assert!(sim.object(ghost).is_some());
    send(&mut sim, ghost, &Message::new(0x110, &[]));
    assert_eq!(mana(&sim, ev).0, 150);
}

// ---------------------------------------------------------------------------------------------
// Module 27 "Ghost" (re/ghosts-banish 2.2, 3.4, 3.5, 8 no. 6-9, 9.2, 9.3).
// ---------------------------------------------------------------------------------------------

/// Hit points (attribute 14) and experience (attribute 30).
const HP: u32 = 14;
const EXPERIENCE: u32 = 30;

/// Ghost 1413 on slot 0.
fn ghost_at(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    tk::spawn(sim, tk::GHOST, 0, tile)
}

fn gstate(sim: &Sim, g: ObjectId) -> ghost::Ghost {
    ghost::ghost(sim, g).expect("module 27").clone()
}

fn hp(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, HP)
}

fn set_hp(sim: &mut Sim, id: ObjectId, v: i32) {
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_current_value(HP, v, 0);
    }
}

fn locked(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.status & status::ANIM_LOCKED != 0)
}

fn died(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_some_and(|o| o.status & status::DIED != 0)
}

/// `0x31B {target}` as an envelope like from FightFar `StartShot` (fault `0x411`, success `0x311` to
/// the own Attack); returns the return value of `dispatch_command`.
fn suck_env(sim: &mut Sim, g: ObjectId, target: ObjectId) -> i32 {
    let env = Envelope {
        msg: Message::new(ghost::msg::SUCK, &[target.as_i32()]),
        success: Some(Message::new(0x311, &[])),
        fault: Some(Message::new(0x411, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, g, &env)
}

/// Alpha sequence of the fade-out: per step `a = a - 0.03` in `f32` (9.2).
fn alpha_after(k: usize) -> f32 {
    let mut a = 1.0_f32;
    for _ in 0..k {
        a -= ghost::ALPHA_STEP;
    }
    a
}

// QUIRK C-262: kept
#[test]
fn a_ghost_appears_and_becomes_selectable() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    // A timer of group 0x80 in `events` (taken over after a tick) and one in
    // `timers_ws`.
    sim.module_mut(g, ghost::INDEX)
        .unwrap()
        .base
        .post_delayed_message(5000, Message::new(0x61B, &[]), false, 0x80);
    ticks(&mut sim, 1);
    sim.module_mut(g, ghost::INDEX)
        .unwrap()
        .base
        .post_delayed_message(5000, Message::new(0x51B, &[]), false, 0x80);
    sim.object_mut(g).unwrap().selectable = false;
    sim.state.events.clear();
    assert_eq!(send(&mut sim, g, &Message::new(ghost::msg::APPEAR, &[])), 1);
    assert_eq!(gstate(&sim, g).status, ghost::state::APPEARING);
    assert!(locked(&sim, g), "ANIM_LOCKED on");
    assert!(
        !sim.object(g).unwrap().selectable,
        "selectability unchanged"
    );
    assert_eq!(sim.object(g).unwrap().anim.wanted, 0, "animation 280001");
    assert!(sim.state.events.iter().any(|e| matches!(e,
        SimEvent::EffectOnObject { object, path }
            if *object == g && path == "data/effects/Geist_erscheinen.pef")));
    let base = &sim.module(g, ghost::INDEX).unwrap().base;
    assert!(base.has_pending_timer(ghost::msg::APPEARED));
    // Group 0x80 cleared: `timers_ws` immediately, `events` in the next call.
    assert!(!base.timers_ws.iter().any(|e| e.msg.code == 0x51B));
    assert!(base.events.iter().any(|e| e.msg.code == 0x61B));
    assert!(!base.has_pending_timer(0x61B));
    // The first ghost tick only takes over `0x41B` (it does not fire yet): still state 1.
    ticks(&mut sim, 1);
    assert!(
        !sim.module(g, ghost::INDEX)
            .unwrap()
            .base
            .events
            .iter()
            .any(|e| e.msg.code == 0x61B)
    );
    assert_eq!(gstate(&sim, g).status, ghost::state::APPEARING);
    assert!(locked(&sim, g));
    // Second tick: `0x41B` (0 ms) fires - state 0, selectable, lock bit off.
    ticks(&mut sim, 1);
    assert_eq!(gstate(&sim, g).status, ghost::state::IDLE);
    assert!(sim.object(g).unwrap().selectable);
    assert!(!locked(&sim, g));
}

// QUIRK C-264: kept
#[test]
fn fading_takes_34_ticks_in_f32() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    ticks(&mut sim, 1);
    // Kill the ghost: the ghost branch sends `0x21B` itself (re/combat-death 2.6).
    sim.state.events.clear();
    send(&mut sim, g, &Message::new(0x110, &[]));
    assert!(died(&sim, g));
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.alpha), (ghost::state::FADING, 1.0));
    assert!(!sim.object(g).unwrap().selectable);
    assert!(sim.state.events.iter().any(|e| matches!(e,
        SimEvent::EffectOnObject { object, path }
            if *object == g && path == "data/effects/Geist_verschwinden.pef")));
    for k in 1..=33 {
        ticks(&mut sim, 1);
        let a = gstate(&sim, g).alpha;
        assert_eq!(a.to_bits(), alpha_after(k).to_bits(), "tick {k}");
    }
    // Rounded step by step, not `1 - 0.03*33` as one expression.
    assert_eq!(gstate(&sim, g).alpha, 0.010_000_439_f32);
    assert_ne!(gstate(&sim, g).alpha, 1.0_f32 - 0.03 * 33.0);
    assert!(alpha_after(34) <= 0.0 && alpha_after(34) > -0.02);
    // 34th tick (group 0 ticks in the first `step`): alpha 0, state 0, in the destroy list; the
    // phase-0 step after that removes it.
    assert_eq!(sim.object(g).unwrap().tick_group, 0);
    crate::tick::step(sim.state, sim.rules, &[]);
    let s = gstate(&sim, g);
    assert_eq!((s.alpha, s.status), (0.0, ghost::state::IDLE));
    assert!(sim.state.destroy_queue.contains(&g));
    crate::tick::step(sim.state, sim.rules, &[]);
    assert!(sim.object(g).is_none(), "removed in phase 0");
}

// QUIRK C-3, C-263: kept
#[test]
fn sucking_hits_minus_five_both_ways() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    // Colonist on slot 1 (enemy); rank set so that experience counts.
    let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
    set_rank_reset_experience(&mut sim, g, 0);
    assert_eq!(suck_env(&mut sim, g, c), 1);
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.target), (ghost::state::SUCKING, c));
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    // 9.3: colonist 160 -> 155 (fraction -3,125), ghost 250 -> 245 (fraction 98,000), experience +5.
    assert_eq!(hp(&sim, c), 155);
    assert_eq!(
        sim.object(c).unwrap().attributes.raw_fraction(HP),
        100_000 - 3_125
    );
    assert_eq!(hp(&sim, g), 245);
    assert_eq!(sim.object(g).unwrap().attributes.raw_fraction(HP), 98_000);
    assert_eq!(
        sim.object(g)
            .unwrap()
            .attributes
            .current_fraction(EXPERIENCE),
        500
    );
    assert!(got(&seen, g, attack::INDEX, 0x511), "0x511 to Attack");
    // Without a running attack Attack ends after `0x511` (`end_attack` -> `0xC13` -> `0x71B`):
    // state 0, no target.
    assert!(got(&seen, g, ghost::INDEX, ghost::msg::STOP_SUCK));
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.target), (ghost::state::IDLE, ObjectId::NONE));
}

// QUIRK C-263: kept
#[test]
fn a_ghost_dies_after_fifty_sucks() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    // Target with more than 250 HP: enemy 1411 (900 HP).
    let t = tk::spawn(&mut sim, tk::GHOST_EVOKER, 1, (12, 10));
    for k in 1..=49 {
        assert_eq!(suck_env(&mut sim, g, t), 1, "strike {k}");
        ticks(&mut sim, 1);
    }
    assert_eq!(hp(&sim, g), 5);
    assert_eq!(hp(&sim, t), 900 - 245);
    assert!(!died(&sim, g));
    // 50th strike: HP 0, `0x71B` and `0x21B`, `DIED` in the same object tick (`check_death`).
    assert_eq!(suck_env(&mut sim, g, t), 1);
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert_eq!(hp(&sim, g), 0);
    assert!(got(&seen, g, ghost::INDEX, ghost::msg::STOP_SUCK));
    assert!(got(&seen, g, ghost::INDEX, ghost::msg::FADE));
    assert!(died(&sim, g));
    assert_eq!(hp(&sim, t), 650, "exactly 250 HP lost");
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.alpha), (ghost::state::FADING, 1.0));
    assert_eq!(s.target, ObjectId::NONE);
    // Another `0x31B` fails on the own HP (< 1).
    assert_eq!(suck_env(&mut sim, g, t), 0);
    // 34 ticks of fade-out, then removed (group 0: in the 34th tick into the destroy list, phase 0
    // of the same object tick).
    ticks(&mut sim, 33);
    assert!(sim.object(g).is_some());
    assert_eq!(gstate(&sim, g).alpha.to_bits(), alpha_after(33).to_bits());
    ticks(&mut sim, 1);
    assert!(sim.object(g).is_none(), "removed after 34 ticks");
    assert_eq!(hp(&sim, t), 650);
}

#[test]
fn the_ghost_sucks_through_attack() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
    ticks(&mut sim, 1);
    // Attack like `CommandKind::Attack` (`command/economy.rs`): `{0x111, [target, 0]}` with abort.
    assert!(post_message(
        &mut sim,
        g,
        Message::new(0x111, &[c.as_i32(), 0]),
        true
    ));
    let mut hits = Vec::new();
    let mut last = hp(&sim, c);
    for k in 1..=20 {
        ticks(&mut sim, 1);
        let now = hp(&sim, c);
        if now != last {
            assert_eq!(last - now, 5, "tick {k}");
            hits.push(k);
        }
        last = now;
    }
    // Measured: tick 1 (FIFO) starts the attack, from tick 2 one suck strike per object tick -
    // the hypothesis "one per tick" (re/ghosts-banish 3.5, measurement 11) is confirmed by the reimplementation:
    // Attack/FightFar send `0x31B` before the ghost module, `0x511` restarts immediately.
    assert_eq!(hits, (2..=20).collect::<Vec<_>>());
    assert_eq!(hp(&sim, g), 250 - 5 * hits.len() as i32);
    assert_eq!(gstate(&sim, g).target, c);
}

// QUIRK C-263: kept
#[test]
fn a_building_target_faults_and_a_dead_one_is_ignored() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    // Building -> fault message `0x411` to Attack, `false`, state stays (C-263).
    let b = tk::spawn(&mut sim, tk::MONASTERY, 1, (16, 10));
    let mut r = 1;
    let seen = observed(|| r = suck_env(&mut sim, g, b));
    assert_eq!(r, 0);
    assert!(got(&seen, g, attack::INDEX, 0x411));
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.target), (ghost::state::IDLE, ObjectId::NONE));
    // Missing target -> `false` without a message.
    let gone = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 12));
    destroy_object(&mut sim, gone);
    let seen = observed(|| r = suck_env(&mut sim, g, gone));
    assert_eq!(r, 0);
    assert!(!seen.iter().any(|&(_, _, code)| code == 0x411));
    assert_eq!(gstate(&sim, g).status, ghost::state::IDLE);
    // Ghost with HP 0 -> `false` without a message (the HP limit applies to the ghost).
    let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
    set_hp(&mut sim, g, 0);
    let seen = observed(|| r = suck_env(&mut sim, g, c));
    assert_eq!(r, 0);
    assert!(!seen.iter().any(|&(_, _, code)| code == 0x411));
    assert_eq!(gstate(&sim, g).target, ObjectId::NONE);
    set_hp(&mut sim, g, 250);
    // Dead, still present target -> `true`, state 3, target set; the tick
    // does no damage and no self-deduction, `0x511` comes.
    set_hp(&mut sim, c, 0);
    send(&mut sim, c, &Message::new(0x110, &[]));
    assert!(died(&sim, c));
    assert_eq!(suck_env(&mut sim, g, c), 1);
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.target), (ghost::state::SUCKING, c));
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert_eq!(hp(&sim, g), 250);
    assert!(got(&seen, g, attack::INDEX, 0x511));
    assert_eq!(gstate(&sim, g).status, ghost::state::IDLE);
}

// QUIRK C-263: kept
#[test]
fn sucking_a_dead_target_does_nothing() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
    assert_eq!(suck_env(&mut sim, g, c), 1);
    // The target dies between `0x31B` and the tick (HP 0).
    set_hp(&mut sim, c, 0);
    send(&mut sim, c, &Message::new(0x110, &[]));
    let seen = observed(|| {
        ticks(&mut sim, 1);
    });
    assert_eq!(hp(&sim, g), 250, "no self-deduction");
    assert!(got(&seen, g, attack::INDEX, 0x511));
    assert_eq!(gstate(&sim, g).status, ghost::state::IDLE);
}

// QUIRK C-264: kept
#[test]
fn a_living_fading_ghost_returns_to_full_alpha_when_it_dies() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpse(&mut sim, tk::COLONIST, 1, (10, 5));
    let ev = evoker(&mut sim, (10, 10));
    start(&mut sim, ev);
    ticks(&mut sim, 5);
    let g = evoke_ghost(&sim, ev).unwrap().ghost;
    assert_eq!(gstate(&sim, g).status, ghost::state::IDLE, "appeared");
    assert!(sim.object(g).unwrap().selectable);
    assert_eq!(mana(&sim, ev).0, 125);
    // Abort of the evoker in the window: `0x21B` at the living ghost (C-258), base value 150.
    abort_all_modules(&mut sim, ev);
    assert_eq!(mana(&sim, ev).0, 150);
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.alpha), (ghost::state::FADING, 1.0));
    assert!(!died(&sim, g));
    assert!(!sim.object(g).unwrap().selectable);
    ticks(&mut sim, 33);
    assert_eq!(gstate(&sim, g).alpha.to_bits(), alpha_after(33).to_bits());
    assert!(!died(&sim, g));
    // 34th tick: `0x110` -> dying via the ghost branch (callback `0x61A`: base value +25), `0x21B`
    // again -> alpha 1.0 until removal (C-264).
    assert_eq!(sim.object(g).unwrap().tick_group, 0);
    let seen = observed(|| {
        crate::tick::step(sim.state, sim.rules, &[]);
    });
    assert!(got(&seen, g, dying::INDEX, 0x110));
    assert!(got(&seen, ev, evoke::INDEX, msg::GHOST_DIED));
    assert!(died(&sim, g));
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.alpha), (ghost::state::FADING, 1.0));
    assert!(sim.state.destroy_queue.contains(&g));
    assert_eq!(mana(&sim, ev).0, 175);
    crate::tick::step(sim.state, sim.rules, &[]);
    assert!(sim.object(g).is_none(), "removed in phase 0");
}

// QUIRK C-265: kept
#[test]
fn fading_is_interrupted_by_suck_and_appear() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
    // Living ghost in the fade-out: `0x31B` -> state 3, alpha stays; `0x71B` -> state 0 (C-265).
    send(&mut sim, g, &Message::new(ghost::msg::FADE, &[]));
    ticks(&mut sim, 3);
    let a3 = alpha_after(3);
    assert_eq!(gstate(&sim, g).alpha.to_bits(), a3.to_bits());
    assert_eq!(suck_env(&mut sim, g, c), 1);
    assert_eq!(gstate(&sim, g).status, ghost::state::SUCKING);
    send(&mut sim, g, &Message::new(ghost::msg::STOP_SUCK, &[]));
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.target), (ghost::state::IDLE, ObjectId::NONE));
    ticks(&mut sim, 2);
    let s = gstate(&sim, g);
    assert_eq!(
        (s.status, s.alpha.to_bits()),
        (ghost::state::IDLE, a3.to_bits())
    );
    assert!(!sim.object(g).unwrap().selectable, "not selectable");
    assert!(!died(&sim, g));
    // `0x41B` after `0x21B` -> state 0, selectable again, alpha stays.
    send(&mut sim, g, &Message::new(ghost::msg::FADE, &[]));
    ticks(&mut sim, 1);
    let a1 = alpha_after(1);
    send(&mut sim, g, &Message::new(ghost::msg::APPEARED, &[]));
    ticks(&mut sim, 2);
    let s = gstate(&sim, g);
    assert_eq!(
        (s.status, s.alpha.to_bits()),
        (ghost::state::IDLE, a1.to_bits())
    );
    assert!(sim.object(g).unwrap().selectable);
}

// QUIRK C-266: kept
#[test]
fn envelopes_set_the_replies_even_without_0x31b() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    let other = tk::spawn(&mut sim, tk::COLONIST, 0, (12, 10));
    let env = Envelope {
        msg: Message::new(0x91B, &[]),
        success: Some(Message::new(0x123, &[7])),
        fault: Some(Message::new(0x456, &[8])),
        success_target: other,
        fault_target: ObjectId::NONE,
    };
    assert_eq!(dispatch_command(&mut sim, g, &env), 0);
    let base = &sim.module(g, ghost::INDEX).unwrap().base;
    assert_eq!(base.success, Some(Message::new(0x123, &[7])));
    assert_eq!(base.fault, Some(Message::new(0x456, &[8])));
    assert_eq!(base.success_target, other);
    assert_eq!(gstate(&sim, g).status, ghost::state::IDLE);
}

// QUIRK C-3: kept
#[test]
fn stop_suck_from_fight_far_ends_the_target() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 10));
    let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
    assert_eq!(suck_env(&mut sim, g, c), 1);
    // FightFar abort -> `0x71B` (`notify_ghost`, C-3): no target, state 0.
    let seen = observed(|| {
        crate::module::abort(&mut sim, g, crate::module::fight_far::INDEX);
    });
    assert!(got(&seen, g, ghost::INDEX, ghost::msg::STOP_SUCK));
    let s = gstate(&sim, g);
    assert_eq!((s.status, s.target), (ghost::state::IDLE, ObjectId::NONE));
    ticks(&mut sim, 1);
    assert_eq!(hp(&sim, c), 160, "no suck strike");
}

/// re/world-display W.4.2: if the victim dies or vanishes while being sucked via Attack, the attack
/// ends (`start_attack` fails at the dead or missing target -> `end_attack` -> `0xC13` -> FightFar
/// `notify_ghost` -> `0x71B`) and `Ghost.target` becomes `NONE`; the host hangs the end of the
/// suction beam on that.
// QUIRK D-242: kept
#[test]
fn a_victim_lost_while_sucked_ends_the_suck() {
    // 0: killed from outside, 1: killed by the suck strike (HP 5), 2: removed.
    for case in 0..3 {
        let rules = tk::evoke_rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = ghost_at(&mut sim, (10, 10));
        let c = tk::spawn(&mut sim, tk::COLONIST, 1, (12, 10));
        ticks(&mut sim, 1);
        assert!(post_message(
            &mut sim,
            g,
            Message::new(0x111, &[c.as_i32(), 0]),
            true
        ));
        ticks(&mut sim, 3);
        assert!(hp(&sim, c) < 160, "case {case}: sucks");
        assert_eq!(gstate(&sim, g).target, c, "case {case}");
        match case {
            0 => {
                set_hp(&mut sim, c, 0);
                send(&mut sim, c, &Message::new(0x110, &[]));
                assert!(died(&sim, c));
                assert_eq!(gstate(&sim, g).target, c, "until the attack end");
            }
            1 => set_hp(&mut sim, c, 5),
            _ => destroy_object(&mut sim, c),
        }
        // `0x71B` comes with the next attack end.
        let mut n = 0;
        let mut stopped = false;
        while !stopped && n < 5 {
            let seen = observed(|| {
                ticks(&mut sim, 1);
            });
            stopped = got(&seen, g, ghost::INDEX, ghost::msg::STOP_SUCK);
            n += 1;
        }
        assert!(stopped, "case {case}: 0x71B");
        // Measured: in the next object tick; if the strike itself kills, one tick later (the
        // strike sets HP 0, the attack only ends at the dead target).
        assert_eq!(n, [1, 2, 1][case], "case {case}: ticks until 0x71B");
        let s = gstate(&sim, g);
        assert_eq!(
            (s.status, s.target),
            (ghost::state::IDLE, ObjectId::NONE),
            "case {case}"
        );
        assert!(
            sim.object(c).is_none_or(|o| o.status & status::DIED != 0),
            "case {case}: victim dead or gone"
        );
        // No new sucking without a target.
        ticks(&mut sim, 3);
        assert_eq!(gstate(&sim, g).target, ObjectId::NONE, "case {case}");
    }
}

#[test]
fn a_dying_ghost_is_a_corpse_for_the_evoker() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = ghost_at(&mut sim, (10, 5));
    send(&mut sim, g, &Message::new(0x110, &[]));
    ticks(&mut sim, 1);
    assert_eq!(gstate(&sim, g).status, ghost::state::FADING);
    // In the fade-out (dead, still present, slot 0) the corpse search finds the ghost.
    let ev = evoker(&mut sim, (10, 10));
    assert_eq!(send(&mut sim, ev, &Message::new(msg::EVOKE, &[BUTTON])), 0);
    assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, g);
}

// --- Module 31 "Banish" (re/ghosts-banish 2.3, 3.6, 8 no. 11-15, 9.4). "N" is here
// the object tick in which `0x11F` takes effect (from the FIFO or via `send` before it).

/// Button "Banish ghosts" of the ghost evoker (cost 25), of the missionary (25) and of the
/// pilgrim (200).
const BAN_BUTTON: i32 = 547;
const MISSIONARY_BUTTON: i32 = 439;
const PILGRIM_BUTTON: i32 = 553;
/// Sight (attribute 18).
const SIGHT: u32 = 18;
/// Banishing animation of the ghost evoker or the missionary; test banishing animation in the
/// saddle.
const BAN_ANIM: i32 = 10;
const MISSIONARY_ANIM: i32 = 6;
const MOUNTED_ANIM: i32 = 12;

/// Ghost evoker on slot 0 with mana 100 and a set rank (experience counts).
fn banisher(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    let id = tk::spawn(sim, tk::GHOST_EVOKER, 0, tile);
    set_mana(sim, id, 100);
    set_rank_reset_experience(sim, id, 0);
    id
}

fn post_banish(sim: &mut Sim, id: ObjectId, button: i32) {
    assert!(post_message(
        sim,
        id,
        Message::new(banish::msg::START, &[button]),
        true
    ));
}

fn ban_timer(sim: &Sim, id: ObjectId, code: u32) -> bool {
    sim.module(id, banish::INDEX)
        .expect("module 31")
        .base
        .has_pending_timer(code)
}

fn current_mana(sim: &Sim, id: ObjectId) -> i32 {
    mana(sim, id).2
}

fn hit_sound(events: &[SimEvent], id: ObjectId) -> bool {
    events.iter().any(|x| {
        matches!(x, SimEvent::Sound3d { object, action: 0x1F, flags: 0x42, .. } if *object == id)
    })
}

fn attack_on(sim: &Sim, id: ObjectId) -> bool {
    let o = sim.object(id).unwrap();
    o.tick_list.contains(&ATTACK) && o.has_module(attack::INDEX, false)
}

/// Ghost 1413 on slot `slot`.
fn ghost_of(sim: &mut Sim, slot: i32, tile: (i32, i32)) -> ObjectId {
    tk::spawn(sim, tk::GHOST, slot, tile)
}

// QUIRK C-267, C-270: kept
#[test]
fn banishing_kills_enemy_seen_ghosts_after_1500_ms() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = banisher(&mut sim, (20, 20));
    // Radius 52 * 4096 >> 3 = 26,624: 6 tiles (24,576) inside, 7 tiles (28,672) outside.
    let inner = ghost_of(&mut sim, 1, (26, 20));
    let own = ghost_of(&mut sim, 0, (20, 22));
    let outer = ghost_of(&mut sim, 1, (20, 13));
    // Evoked enemy wolf (`IS_EVOKED`): no module 27, never a target.
    let wolf = tk::spawn(&mut sim, tk::WOLF, 1, (17, 20));
    crate::module::set_match_bits(&mut sim, wolf, 0x214, true);
    let wolf_hp = hp(&sim, wolf);
    assert_eq!(send(&mut sim, ev, &Message::new(0x51F, &[])), -1);
    post_banish(&mut sim, ev, BAN_BUTTON);
    // N: mana paid immediately, sound, lock bit, attack off, banishing animation 10, three timers.
    let events = ticks(&mut sim, 1);
    assert_eq!(current_mana(&sim, ev), 75);
    assert!(events.iter().any(|x| matches!(x,
        SimEvent::Sound3d { object, action: 0x1E, flags: 0x43, guid: 547, .. } if *object == ev)));
    assert!(locked(&sim, ev));
    assert!(!sim.object(ev).unwrap().tick_list.contains(&ATTACK));
    assert_eq!(sim.object(ev).unwrap().anim.wanted, BAN_ANIM);
    assert_eq!(banish::banish(&sim, ev).unwrap().spell_guid, BAN_BUTTON);
    for code in [banish::msg::NOW, banish::msg::END, banish::msg::EFFECTS_OFF] {
        assert!(ban_timer(&sim, ev, code), "timer {code:#x}");
    }
    // N+15: no banishing yet.
    let events = ticks(&mut sim, 15);
    assert!(!hit_sound(&events, ev));
    assert_eq!(hp(&sim, inner), 250);
    // N+16 (floor(1500/100) + 1): only the enemy ghost within 6 tiles; experience +250 (fraction 25,000).
    let events = ticks(&mut sim, 1);
    assert!(hit_sound(&events, ev));
    assert_eq!(hp(&sim, inner), 0);
    for g in [own, outer] {
        assert!(hp(&sim, g) > 0 && !died(&sim, g), "ghost {g:?} stays");
    }
    assert_eq!(hp(&sim, wolf), wolf_hp, "evoked wolf stays");
    assert_eq!(
        sim.object(ev)
            .unwrap()
            .attributes
            .current_fraction(EXPERIENCE),
        25_000
    );
    assert!(!ban_timer(&sim, ev, banish::msg::NOW));
    // Death via the ghost's own `check_death`; the ghost is in the tick group behind the
    // banisher (measured: `DIED` already in N+16).
    assert!(died(&sim, inner), "DIED in N+16");
    assert_eq!(gstate(&sim, inner).status, ghost::state::FADING);
    // N+43 banishing spell still on, N+44 (marker 5,332 - 1,000) off.
    ticks(&mut sim, 27);
    assert!(ban_timer(&sim, ev, banish::msg::EFFECTS_OFF));
    ticks(&mut sim, 1);
    assert!(!ban_timer(&sim, ev, banish::msg::EFFECTS_OFF));
    assert!(locked(&sim, ev));
    // N+53 locked, N+54 (marker 5,332): lock bit off, attack back in the tick list.
    ticks(&mut sim, 9);
    assert!(locked(&sim, ev));
    assert!(!attack_on(&sim, ev));
    ticks(&mut sim, 1);
    assert!(!locked(&sim, ev));
    assert!(attack_on(&sim, ev));
}

// QUIRK C-267: kept
#[test]
fn the_radius_is_half_the_sight_at_the_time_of_the_ban() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = banisher(&mut sim, (20, 20));
    let seven = ghost_of(&mut sim, 1, (27, 20));
    let far = ghost_of(&mut sim, 1, (28, 23));
    post_banish(&mut sim, ev, BAN_BUTTON);
    ticks(&mut sim, 1);
    // Between command and banishing: sight 52 -> 64 (base value replaced, key 0).
    sim.object_mut(ev)
        .unwrap()
        .attributes
        .set_modifier(SIGHT, 64, false, 0);
    assert_eq!(sim.attribute(ev, SIGHT), 64);
    ticks(&mut sim, 16);
    // Radius 64 * 4096 >> 3 = 32,768: 7 tiles (28,672) inside, ~ 8.5 tiles (~ 34,997) outside.
    assert_eq!(hp(&sim, seven), 0);
    assert!(hp(&sim, far) > 0 && !died(&sim, far));
}

// QUIRK C-268: kept
#[test]
fn banishing_pays_even_without_a_ghost() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Without a ghost: mana gone, no sound 0x1F (C-268).
    let ev = banisher(&mut sim, (20, 20));
    post_banish(&mut sim, ev, BAN_BUTTON);
    ticks(&mut sim, 1);
    assert_eq!(current_mana(&sim, ev), 75);
    let events = ticks(&mut sim, 16);
    assert!(!hit_sound(&events, ev));
    assert!(
        !ban_timer(&sim, ev, banish::msg::NOW),
        "N+16: banishing ran"
    );
    // Abort in N+10: mana stays gone, lock bit off, attack on, no banishing in N+16.
    let other = banisher(&mut sim, (40, 40));
    let g = ghost_of(&mut sim, 1, (43, 40));
    post_banish(&mut sim, other, BAN_BUTTON);
    ticks(&mut sim, 11);
    assert!(locked(&sim, other));
    // Mana tick (module 38) keeps regenerating; the abort returns nothing.
    let paid = current_mana(&sim, other);
    assert!(paid < 100);
    abort_all_modules(&mut sim, other);
    assert_eq!(current_mana(&sim, other), paid);
    assert!(!locked(&sim, other));
    assert!(!ban_timer(&sim, other, banish::msg::NOW));
    assert!(!ban_timer(&sim, other, banish::msg::EFFECTS_OFF));
    let events = ticks(&mut sim, 6);
    assert!(attack_on(&sim, other));
    assert!(!hit_sound(&events, other));
    assert_eq!(hp(&sim, g), 250);
}

// QUIRK C-267: kept
#[test]
fn fog_hides_ghosts_from_the_ban() {
    let rules = tk::evoke_rules();
    let mut state = tk::state_fogged(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ev = banisher(&mut sim, (20, 20));
    let g = ghost_of(&mut sim, 1, (24, 20));
    // Before the first `step`: the tile of the ghost is not seen for slot 0 (`MM_CHECKFOG`).
    assert_eq!(send(&mut sim, ev, &Message::new(banish::msg::NOW, &[])), 0);
    assert_eq!(hp(&sim, g), 250);
    // Counter-check on the same state: uncovered -> banished.
    crate::fog::reveal_map(sim.state, -1);
    assert_eq!(send(&mut sim, ev, &Message::new(banish::msg::NOW, &[])), 0);
    assert_eq!(hp(&sim, g), 0);
}

// QUIRK C-254, C-270: kept
#[test]
fn a_trance_lock_blocks_banishing() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Pilgrim (modules 29 and 31) in the trance transition: `0x11F` directly (past the
    // message filter) -> 0, no mana, no timer (C-270).
    let p = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    set_mana(&mut sim, p, 400);
    send(
        &mut sim,
        p,
        &Message::new(crate::module::healing::msg::BEGIN, &[]),
    );
    assert!(locked(&sim, p));
    let start = Message::new(banish::msg::START, &[PILGRIM_BUTTON]);
    assert_eq!(send(&mut sim, p, &start), 0);
    assert_eq!(current_mana(&sim, p), 400);
    assert!(!ban_timer(&sim, p, banish::msg::NOW));
    assert_eq!(banish::banish(&sim, p).unwrap().spell_guid, 0);
    // The abort of banishing clears the shared lock bit also in the trance (C-254).
    assert!(crate::module::abort(&mut sim, p, banish::INDEX));
    assert!(!locked(&sim, p));
    // During banishing `0x11D` aborts at the lock bit: no `HEAL`, no setup animation.
    let q = tk::spawn(&mut sim, tk::PILGRIM, 0, (30, 20));
    set_mana(&mut sim, q, 400);
    assert_eq!(send(&mut sim, q, &start), 0);
    assert_eq!(current_mana(&sim, q), 200);
    assert!(locked(&sim, q));
    send(
        &mut sim,
        q,
        &Message::new(crate::module::healing::msg::BEGIN, &[]),
    );
    let o = sim.object(q).unwrap();
    assert_eq!(o.status & status::HEAL, 0);
    assert_eq!(
        o.anim.wanted, MISSIONARY_ANIM,
        "banishing animation 6 stays"
    );
    assert!(
        !sim.module(q, crate::module::healing::INDEX)
            .unwrap()
            .base
            .has_pending_timer(crate::module::healing::msg::READY)
    );
}

// QUIRK C-269: kept
#[test]
fn a_mounted_banisher_uses_the_riding_anim() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = tk::spawn(&mut sim, tk::MISSIONARY, 0, (20, 20));
    set_mana(&mut sim, m, 200);
    sim.object_mut(m)
        .unwrap()
        .set_status_flags(status::ISMOUNTED, true);
    // Riding `0x50D {4}` -> list 140010 = 12 (!= banishing animation 6), without `RIDINGBANISH` (C-269).
    let riding = send(&mut sim, m, &Message::new(0x50D, &[4]));
    assert_eq!(riding, MOUNTED_ANIM);
    post_banish(&mut sim, m, MISSIONARY_BUTTON);
    ticks(&mut sim, 1);
    assert_eq!(current_mana(&sim, m), 175);
    assert_eq!(sim.object(m).unwrap().anim.wanted, riding);
    // Marker 2,000 ms: `0x41F` in N+11, `0x21F` unchanged in N+16, `0x31F` in N+21.
    ticks(&mut sim, 10);
    assert!(ban_timer(&sim, m, banish::msg::EFFECTS_OFF));
    ticks(&mut sim, 1);
    assert!(!ban_timer(&sim, m, banish::msg::EFFECTS_OFF));
    ticks(&mut sim, 4);
    assert!(ban_timer(&sim, m, banish::msg::NOW));
    ticks(&mut sim, 1);
    assert!(!ban_timer(&sim, m, banish::msg::NOW));
    ticks(&mut sim, 4);
    assert!(locked(&sim, m));
    ticks(&mut sim, 1);
    assert!(!locked(&sim, m));
}

// QUIRK C-269: kept
#[test]
fn the_missionary_bans_at_his_own_times() {
    let rules = tk::evoke_rules();
    let mut state = world(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = tk::spawn(&mut sim, tk::MISSIONARY, 0, (20, 20));
    set_mana(&mut sim, m, 200);
    post_banish(&mut sim, m, MISSIONARY_BUTTON);
    ticks(&mut sim, 1);
    assert_eq!(current_mana(&sim, m), 175);
    assert_eq!(sim.object(m).unwrap().anim.wanted, MISSIONARY_ANIM);
    // Anim 6, marker 9,332 ms: `0x41F` in N+84, `0x31F` in N+94.
    ticks(&mut sim, 83);
    assert!(ban_timer(&sim, m, banish::msg::EFFECTS_OFF));
    ticks(&mut sim, 1);
    assert!(!ban_timer(&sim, m, banish::msg::EFFECTS_OFF));
    ticks(&mut sim, 9);
    assert!(locked(&sim, m));
    ticks(&mut sim, 1);
    assert!(!locked(&sim, m));
    assert!(attack_on(&sim, m));
}
