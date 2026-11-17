//! Tests of robbing (re/headhunter-robbing). This file covers state and `Init` of module 48
//! (1.3, 10.1) and the flow (3.8, 3.9, 9.3). The command tests of `0x4C` are with the other
//! ability commands (`command::ability`).

use crate::combat::damage::explicit_damage_message;
use crate::command::{Command, CommandKind, NO_SPREAD};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::ability_testkit as tk;
use crate::module::{attack, dying, walk};
use crate::object::status;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::queue_destroy;

use super::{ATTACK, INDEX, msg, robbing};

/// Headhunter world with research 3326 for slot 0 (button 540).
fn rob_state(rules: &Rules) -> SimState {
    let mut state = tk::hunter_state(rules);
    tk::unlock(&mut state, 0, tk::ROB_UPGRADE);
    state
}

/// Robber of slot 0 at `tile` with mana 150 (undertaker: maximum 150).
fn robber(sim: &mut Sim, guid: u32, tile: (i32, i32)) -> ObjectId {
    let id = tk::spawn(sim, guid, 0, tile);
    let pct = sim.type_upgrade_percent(0, guid, 15);
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_value(15, 150, pct);
    id
}

/// Command `0x4C` with button 540 in one step; then one more step (the FIFO has run).
fn start(sim: &mut Sim, u: ObjectId) {
    let command = Command {
        player: 0,
        kind: CommandKind::RobCorpses {
            unit: u,
            button: tk::ROB_BUTTON,
        },
    };
    crate::tick::step(sim.state, sim.rules, &[command]);
    crate::tick::step(sim.state, sim.rules, &[]);
}

fn gold(sim: &Sim) -> i32 {
    sim.state.resource(0, 0)
}

fn robbable(sim: &Sim, c: ObjectId) -> bool {
    sim.module(c, dying::INDEX)
        .is_some_and(|m| m.base.flags & dying::CANROBBED != 0)
}

fn locked(sim: &Sim, u: ObjectId) -> bool {
    sim.object(u).unwrap().status & status::ANIM_LOCKED != 0
}

/// Corpse of the running walk order (success message `{0x230, [corpse]}`), otherwise `None`.
fn heading_for(sim: &Sim, u: ObjectId) -> Option<ObjectId> {
    let base = &sim.module(u, walk::INDEX)?.base;
    base.success
        .as_ref()
        .filter(|m| m.code == msg::ARRIVED)
        .map(|m| ObjectId::from_i32(m.arg(0)))
}

/// Corpse of the pending timer `0x330`, otherwise `None`.
fn rob_timer(sim: &Sim, u: ObjectId) -> Option<ObjectId> {
    let base = &sim.module(u, INDEX)?.base;
    base.timers_ws
        .iter()
        .chain(&base.events)
        .find(|e| e.msg.code == msg::ROB)
        .map(|e| ObjectId::from_i32(e.msg.arg(0)))
}

/// Single object ticks until `done` holds (at most `max`); returns the number.
fn ticks_until(sim: &mut Sim, max: usize, done: impl Fn(&Sim) -> bool) -> usize {
    for n in 1..=max {
        tk::run_ticks(sim, 1);
        if done(sim) {
            return n;
        }
    }
    panic!("condition not reached after {max} ticks");
}

/// Walk command `0x04` to the tile center `tile` (with abort of all modules).
fn move_to(u: ObjectId, tile: (i32, i32)) -> CommandKind {
    CommandKind::Move {
        units: vec![u],
        x: (tile.0 << 12) + 0x800,
        z: (tile.1 << 12) + 0x800,
        target: -1,
        formation: 0,
        direction: crate::motion::NO_DIRECTION,
        spread: NO_SPREAD,
        flags: 0,
    }
}

/// Create corpses and wait until all are robbable.
fn corpses(sim: &mut Sim, list: &[(i32, (i32, i32))]) -> Vec<ObjectId> {
    let ids: Vec<ObjectId> = list
        .iter()
        .map(|&(slot, tile)| tk::corpse(sim, slot, tile))
        .collect();
    for &c in &ids {
        tk::run_until_robbable(sim, c);
    }
    ids
}

/// Arrival and robbing of a corpse: ticks until the lock (arrival in tick N), then until the
/// lock falls; returns (corpse of the timer, ticks from N to robbing, yield).
fn rob_next(sim: &mut Sim, u: ObjectId, max_walk: usize) -> (ObjectId, usize, i32) {
    ticks_until(sim, max_walk, |s| locked(s, u));
    let c = rob_timer(sim, u).expect("timer 0x330");
    assert_eq!(sim.object(u).unwrap().anim.wanted, 3, "robbing animation");
    let before = gold(sim);
    let k = ticks_until(sim, 30, |s| !locked(s, u));
    (c, k, gold(sim) - before)
}

#[test]
fn init_reads_the_rob_animations() {
    // `0x00523C10`: `m_RobAnim` = `IntList` from 490002, module flags 490001 (256 =
    // `CANROBDEADUNITS`).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    for (guid, anim) in [(tk::UNDERTAKER, 3), (tk::SANDERS, 7)] {
        let id = tk::spawn(&mut sim, guid, 1, (10 + anim, 10));
        assert_eq!(
            robbing(&sim, id).expect("module 48").rob_anim,
            [anim],
            "{guid}"
        );
        assert_eq!(sim.module(id, INDEX).unwrap().base.flags, 256, "{guid}");
    }
}

// QUIRK C-321: kept
#[test]
fn robbing_loops_until_the_last_corpse() {
    // re/headhunter-robbing 9.3, C-321: every corpse with `CANROBBED`, without list filter - the
    // corpses all belong to slot 8, distinguished only by the former owner (slot 0 own, slot 2
    // allied, slot 1 hostile). The next one in turn; per corpse arrival N, robbing at N + 25
    // (time marker 1 = 2499 ms, floor(2499/100) + 1) with +25 gold on the account of the
    // robber, the corpse is not robbable afterwards; then quiet.
    let rules = tk::hunter_rules();
    let mut state = rob_state(&rules);
    for (a, b) in [(0, 2), (2, 0)] {
        state.diplomacy.set_enemy(a, b, false);
        state.diplomacy.set_friend(a, b, true);
    }
    let mut sim = Sim::new(&mut state, &rules);
    let list = corpses(&mut sim, &[(0, (12, 10)), (2, (14, 11)), (1, (17, 10))]);
    let u = robber(&mut sim, tk::UNDERTAKER, (10, 10));
    sim.state.players[0].set_resource(0, 1000);
    start(&mut sim, u);
    assert_eq!(sim.attribute(u, 15), 140);
    for (k, &c) in list.iter().enumerate() {
        let (robbed, ticks, gain) = rob_next(&mut sim, u, 100);
        assert_eq!((robbed, ticks, gain), (c, 25, 25), "corpse {k}");
        assert!(!robbable(&sim, c), "corpse {k} emptied");
        assert_eq!(gold(&sim), 1000 + 25 * (k as i32 + 1));
    }
    // Quiet: no order, no lock, no timer, no more gold.
    tk::run_ticks(&mut sim, 30);
    assert_eq!(heading_for(&sim, u), None);
    assert!(!locked(&sim, u));
    assert_eq!(rob_timer(&sim, u), None);
    assert_eq!(gold(&sim), 1075);
}

// QUIRK C-325: kept
#[test]
fn an_emptied_corpse_yields_nothing() {
    // C-325: arrival does not check `CANROBBED`. A second robber (Sanders, next to the corpse)
    // empties it while the first is on its way: the first plays the animation anyway,
    // the yield is 0.
    let rules = tk::hunter_rules();
    let mut state = rob_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let c = corpses(&mut sim, &[(1, (16, 10))])[0];
    let first = robber(&mut sim, tk::UNDERTAKER, (10, 10));
    let second = robber(&mut sim, tk::SANDERS, (17, 10));
    sim.state.players[0].set_resource(0, 1000);
    start(&mut sim, second);
    ticks_until(&mut sim, 30, |s| locked(s, second));
    // 20 ticks later (the second robs 25 ticks after its arrival) the first still selects
    // the same corpse.
    tk::run_ticks(&mut sim, 20);
    assert!(robbable(&sim, c));
    start(&mut sim, first);
    assert_eq!(heading_for(&sim, first), Some(c));
    // The second robs and then walks away (it would otherwise stand on the target point of the first).
    ticks_until(&mut sim, 10, |s| !locked(s, second));
    assert!(!robbable(&sim, c));
    assert_eq!(gold(&sim), 1025, "the second has robbed");
    crate::tick::step(
        sim.state,
        sim.rules,
        &[Command {
            player: 0,
            kind: move_to(second, (17, 4)),
        }],
    );
    ticks_until(&mut sim, 100, |s| locked(s, first));
    assert!(!robbable(&sim, c), "already emptied on arrival");
    assert_eq!(rob_timer(&sim, first), Some(c));
    assert_eq!(sim.object(first).unwrap().anim.wanted, 3, "animation runs");
    let k = ticks_until(&mut sim, 30, |s| !locked(s, first));
    assert_eq!(k, 25);
    assert_eq!(gold(&sim), 1025, "yield 0");

    // A rotted (destroyed) corpse: `0x230` does not find it and searches for the next one.
    let [rotten, next] = corpses(&mut sim, &[(1, (17, 13)), (1, (16, 14))])[..] else {
        unreachable!()
    };
    queue_destroy(&mut sim, rotten);
    tk::run_ticks(&mut sim, 1);
    assert!(sim.object(rotten).is_none());
    assert_eq!(
        send(
            &mut sim,
            first,
            &Message::new(msg::ARRIVED, &[rotten.as_i32()])
        ),
        0
    );
    assert_eq!(heading_for(&sim, first), Some(next), "next search");
    assert!(!locked(&sim, first));
    assert_eq!(rob_timer(&sim, first), None);
}

// QUIRK C-322: kept
#[test]
fn an_unreachable_corpse_ends_the_loop() {
    // C-322: the walk failure is forced via `Walk.can_move = false` - test hook instead of
    // terrain, because the region already filters unreachable corpses. `0x430
    // {c1}` searches anew, c1 is the nearest again -> end; the farther c2 stays where it is.
    let rules = tk::hunter_rules();
    let mut state = rob_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let [c1, c2] = corpses(&mut sim, &[(1, (13, 10)), (1, (16, 10))])[..] else {
        unreachable!()
    };
    let u = robber(&mut sim, tk::UNDERTAKER, (10, 10));
    sim.state.players[0].set_resource(0, 1000);
    walk::walk_mut(&mut sim, u).unwrap().can_move = false;
    start(&mut sim, u);
    // The order was for c1 (FINDTO remembered), the failure message `0x430` is used up, no new
    // order.
    assert_eq!(
        walk::walk(&sim, u).unwrap().find_to,
        Some(Message::new(0x200, &[c1.as_i32(), -1, 1]))
    );
    let base = &sim.module(u, walk::INDEX).unwrap().base;
    assert_eq!((base.success.clone(), base.fault.clone()), (None, None));
    tk::run_ticks(&mut sim, 30);
    assert_eq!(heading_for(&sim, u), None);
    assert!(!locked(&sim, u));
    assert_eq!(rob_timer(&sim, u), None);
    assert_eq!(gold(&sim), 1000);
    assert!(robbable(&sim, c1) && robbable(&sim, c2));
    // The same corpse reported once more: end without order. A different one than "unreachable"
    // reported: the nearest (c1) is approached again.
    assert_eq!(
        send(&mut sim, u, &Message::new(msg::WALK_FAULT, &[c1.as_i32()])),
        0
    );
    assert_eq!(heading_for(&sim, u), None);
    walk::walk_mut(&mut sim, u).unwrap().can_move = true;
    send(&mut sim, u, &Message::new(msg::WALK_FAULT, &[c2.as_i32()]));
    assert_eq!(heading_for(&sim, u), Some(c1));
}

// QUIRK C-321: kept
#[test]
fn robbing_stays_in_its_region() {
    // C-321: with a layer, robbing searches only in the own region (inner area of
    // `wall_ring` instead of water). The nearer corpse on the island (4 tiles) is not found,
    // the farther one outside (6 tiles) is; afterwards quiet, the island corpse stays robbable.
    let rules = tk::hunter_rules();
    let mut state = rob_state(&rules);
    tk::wall_ring(&mut state, (14, 10), 1, 2);
    let mut sim = Sim::new(&mut state, &rules);
    let [inside, outside] = corpses(&mut sim, &[(1, (14, 10)), (1, (10, 16))])[..] else {
        unreachable!()
    };
    let u = robber(&mut sim, tk::UNDERTAKER, (10, 10));
    sim.state.players[0].set_resource(0, 1000);
    start(&mut sim, u);
    assert_eq!(heading_for(&sim, u), Some(outside));
    let (robbed, ticks, gain) = rob_next(&mut sim, u, 100);
    assert_eq!((robbed, ticks, gain), (outside, 25, 25));
    tk::run_ticks(&mut sim, 10);
    assert_eq!(heading_for(&sim, u), None);
    assert!(!locked(&sim, u));
    assert!(robbable(&sim, inside));
    assert_eq!(gold(&sim), 1025);
}

// QUIRK K-322, C-320: kept
#[test]
fn robbing_leaves_the_attack_module_off() {
    // C-320, K-322: the natural end calls no `abort` - the attack module stays out of the tick list
    // and switched off: an enemy within the radius triggers neither retaliation (`0x911`
    // discarded) nor target search. A walk command (abort) switches it on again.
    let rules = tk::hunter_rules();
    let mut state = rob_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let c = corpses(&mut sim, &[(1, (12, 10))])[0];
    let u = robber(&mut sim, tk::UNDERTAKER, (10, 10));
    sim.state.players[0].set_resource(0, 1000);
    let attack_on = |sim: &Sim| {
        let o = sim.object(u).unwrap();
        (
            o.tick_list.contains(&(ATTACK as u8)),
            o.has_module(ATTACK, false),
        )
    };
    assert_eq!(attack_on(&sim), (true, true));
    start(&mut sim, u);
    let (robbed, _, gain) = rob_next(&mut sim, u, 100);
    assert_eq!((robbed, gain), (c, 25));
    tk::run_ticks(&mut sim, 2);
    assert_eq!(attack_on(&sim), (false, false), "off after the end");
    assert!(sim.object(u).unwrap().has_module(ATTACK, true));

    let sabre = tk::spawn(&mut sim, tk::SABRE, 1, (10, 12));
    let target = |sim: &Sim| attack::attack(sim, u).unwrap().target;
    let before = target(&sim);
    let counter = Message::new(0x911, &[sabre.as_i32()]);
    assert_eq!(send(&mut sim, u, &counter), -1, "retaliation discarded");
    let blow = explicit_damage_message(-10, sabre, tk::SABRE);
    send(&mut sim, u, &blow);
    tk::run_ticks(&mut sim, 5);
    assert_eq!(target(&sim), before, "no retaliation, no target search");
    assert_ne!(target(&sim), sabre);
    assert_eq!(attack_on(&sim), (false, false));

    // Walk command with abort: `abort` of module 48 switches the attack module on again.
    crate::tick::step(
        sim.state,
        sim.rules,
        &[Command {
            player: 0,
            kind: move_to(u, (20, 10)),
        }],
    );
    tk::run_ticks(&mut sim, 1);
    assert_eq!(attack_on(&sim), (true, true), "on again after the abort");
}
