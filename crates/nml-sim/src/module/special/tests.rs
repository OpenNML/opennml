//! Tests of the special attacks (re/special-attacks): states and `Init` of the modules 36, 37 and
//! 49 (re/special-attacks 1.2-1.4, 10.1) and the flows of the attacks.

use crate::combat::projectile::{Projectile, ProjectileKind};
use crate::command::CommandKind;
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{post_message, send};
use crate::module::ability_testkit as tk;
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};

use super::bomberman::msg as bm;
use super::killer::msg as km;
use super::special_move::msg as sm;
use super::{bomberman, killer, special_move};

#[test]
fn init_reads_the_bomberman_values() {
    let rules = tk::spell_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let patriot = tk::spawn(&mut sim, tk::PATRIOT, 0, (10, 10));
    let b = bomberman::bomberman(&sim, patriot).expect("module 36");
    // 370002 is in the test world as the full path of the shipped data.
    assert_eq!(
        b.model,
        "data/r3d/misc/Tools/Dynamitbuendel/dynamitbuendel.rdo"
    );
    // 370003 put (building), 370004 charge (building), 370005 put (ship), 370006 charge (ship).
    assert_eq!(
        (
            b.put_building_anim,
            b.building_anim,
            b.put_ship_anim,
            b.ship_anim
        ),
        (1, 2, 2, 3)
    );
    assert_eq!(b.target, ObjectId::NONE);
    assert_eq!((b.extra_energy, b.return_point, b.busy), (0, [0; 3], false));
}

#[test]
fn init_reads_the_killer_values() {
    let rules = tk::spell_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let shadow = tk::spawn(&mut sim, tk::SHADOW_WARRIOR, 0, (10, 10));
    let pistolero = tk::spawn(&mut sim, tk::PISTOLERO, 0, (12, 10));
    // 380002 near animation, 380003 far animation, 380004 near damage, 380005 far damage.
    let k = killer::killer(&sim, shadow).expect("module 37");
    assert_eq!(
        (k.near_anim, k.far_anim, k.near_damage, k.far_damage),
        (5, 0, 300, 0)
    );
    let k = killer::killer(&sim, pistolero).expect("module 37");
    assert_eq!(
        (k.near_anim, k.far_anim, k.near_damage, k.far_damage),
        (0, 9, 0, 300)
    );
    // Deviation D-240: target and cost are 0/-1 before the first order instead of memory garbage.
    for id in [shadow, pistolero] {
        let k = killer::killer(&sim, id).expect("module 37");
        assert_eq!((k.target, k.extra_energy), (ObjectId::NONE, 0));
    }
}

#[test]
fn init_reads_the_special_moves() {
    let rules = tk::spell_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let smith = tk::spawn(&mut sim, tk::SMITH, 0, (10, 10));
    let s = special_move::special_move(&sim, smith).expect("module 49");
    assert_eq!(s.anims, vec![4, 5]);
    assert!(!s.busy);
    assert_eq!(s.mode, 0);
}

// ---------------------------------------------------------------------------------------------
// Module 36 demolition expert: order, placing, igniting, abort (re/special-attacks 3.1-3.3, 3.9, 9 T1 to T7).
// "N" = object tick of the arrival (placing begins). Actor and victim have behavior 0 (no
// retaliation).

/// Mana (attribute 15) and hit points (14).
fn mana(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, 15)
}

fn hp(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, 14)
}

fn bomber<'s>(sim: &'s Sim, id: ObjectId) -> &'s bomberman::Bomberman {
    bomberman::bomberman(sim, id).expect("module 36")
}

fn status_on(sim: &Sim, id: ObjectId, bit: u32) -> bool {
    sim.object(id).unwrap().status & bit != 0
}

/// Attack module (17) in the tick list.
fn attack_on(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).unwrap().tick_list.contains(&0x11)
}

fn fire_pending(sim: &Sim, id: ObjectId, code: u32) -> bool {
    sim.module(id, bomberman::INDEX)
        .unwrap()
        .base
        .has_pending_timer(code)
}

fn rank_and_fraction(sim: &Sim, id: ObjectId) -> (u8, i32) {
    let o = sim.object(id).unwrap();
    (o.rank, o.attributes.current_fraction(30))
}

/// Dynamite charges of the projectile list.
fn charges(sim: &Sim) -> Vec<Projectile> {
    sim.state
        .projectiles
        .list
        .iter()
        .filter(|p| p.kind == ProjectileKind::Dynamite)
        .cloned()
        .collect()
}

fn find_to(sim: &Sim, id: ObjectId) -> Option<Message> {
    crate::module::walk::walk(sim, id).and_then(|w| w.find_to.clone())
}

fn grid_of(sim: &Sim, id: ObjectId) -> [i32; 3] {
    let g = sim.object(id).unwrap().grid;
    [g.x.0, g.y.0, g.z.0]
}

fn edge(sim: &mut Sim, target: ObjectId, id: ObjectId) -> i32 {
    crate::collision::edge_distance_to_object(sim, target, id)
}

/// Spell world with behavior 0 on the slots 0 and 1 (before creating).
fn quiet_state(rules: &crate::rules::Rules) -> SimState {
    let mut state = tk::state(rules);
    state.combat.behavior_mode[0] = 0;
    state.combat.behavior_mode[1] = 0;
    state
}

/// Patriot (slot 0) on `tile`, mana explicitly `mana`, rank 0 with empty experience.
fn patriot(sim: &mut Sim, tile: (i32, i32), mana: i32) -> ObjectId {
    attacker(sim, tk::PATRIOT, tile, mana)
}

/// Actor `guid` (slot 0) on `tile`, mana explicitly `mana`, rank 0 with empty experience.
fn attacker(sim: &mut Sim, guid: u32, tile: (i32, i32), mana: i32) -> ObjectId {
    let p = tk::spawn(sim, guid, 0, tile);
    sim.object_mut(p)
        .unwrap()
        .attributes
        .set_current_value(15, mana, 0);
    crate::combat::rank::set_rank_reset_experience(sim, p, 0);
    p
}

/// Order `{target, button}` via the FIFO with abort (like the command `0x30`/`0x31`).
fn order(sim: &mut Sim, p: ObjectId, code: u32, target: ObjectId, button: i32) {
    assert!(post_message(
        sim,
        p,
        Message::new(code, &[target.as_i32(), button]),
        true
    ));
}

/// Ticks until the start of placing (`busy`); returns N, counted from the first tick after the order.
fn until_laying(sim: &mut Sim, p: ObjectId) -> usize {
    for n in 1..=300 {
        tk::run_ticks(sim, 1);
        if bomber(sim, p).busy {
            return n;
        }
    }
    panic!("demolition expert never places");
}

/// Way back `0x100` to the return point (argc 6, without target direction, C-326).
fn run_back(rp: [i32; 3]) -> Message {
    Message::new(0x100, &[rp[0], rp[2], -1, NO_DIRECTION, -1, 1])
}

/// Object ticks until the explosion of the (only) charge.
fn until_blast(sim: &mut Sim) -> usize {
    for n in 1..=200 {
        tk::run_ticks(sim, 1);
        if charges(sim).is_empty() {
            return n;
        }
    }
    panic!("charge never explodes");
}

/// Measured: object ticks from the tick of the placing (`0x324`/`0x624` in N+54) to the tick of the
/// explosion (100th effect tick).
const BLAST_AFTER: usize = 99;

#[test]
fn dynamite_follows_its_timeline() {
    // T1, K-320, C-326, C-327: patriot (mana 100, button 454 costs 70) against an enemy,
    // unoccupied monastery (HP 10,000).
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    let start = grid_of(&sim, p);
    let start_tile = sim.object(p).unwrap().tile;
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    tk::run_ticks(&mut sim, 1);
    // Order: return point, cost, target; walk FINDTO argc 3 to the monastery; no deduction.
    let b = bomber(&sim, p);
    assert_eq!(
        (b.target, b.extra_energy, b.return_point),
        (monastery, 70, start)
    );
    assert_eq!(
        find_to(&sim, p),
        Some(Message::new(0x200, &[monastery.as_i32(), -1, 1]))
    );
    assert!(
        !status_on(&sim, p, status::EXEMPT_ZONE),
        "building without exemption"
    );
    assert_eq!(mana(&sim, p), 100);
    let n = until_laying(&mut sim, p);
    assert!(n > 2, "run to the monastery ({n} ticks)");
    // N: placing - lock, animation 370003 = 1, attack out of the tick, timer 0x324.
    assert!(status_on(&sim, p, status::ANIM_LOCKED));
    assert_eq!(sim.object(p).unwrap().anim.wanted, 1);
    assert!(!attack_on(&sim, p));
    assert!(fire_pending(&sim, p, bm::FIRE_BUILDING));
    assert!(edge(&mut sim, monastery, p) <= bomberman::REACH);
    assert_eq!(mana(&sim, p), 100, "deduction not until the effect (K-320)");
    // N+1 ... N+53: the placing runs.
    tk::run_ticks(&mut sim, 53);
    assert!(bomber(&sim, p).busy && charges(&sim).is_empty());
    assert_eq!(mana(&sim, p), 100);
    let at = crate::combat::hit_zone::world_pos(&sim, p).unwrap();
    // N+54: 0x324 - mana 30, charge at the position of the patriot with animation 370004 = 2, way back.
    tk::run_ticks(&mut sim, 1);
    let b = bomber(&sim, p);
    assert!(!b.busy);
    assert_eq!(
        b.target, monastery,
        "building: no abort, target stays (C-327)"
    );
    assert!(!status_on(&sim, p, status::ANIM_LOCKED) && attack_on(&sim, p));
    assert_eq!(mana(&sim, p), 30);
    let c = charges(&sim);
    assert_eq!(c.len(), 1);
    assert_eq!(
        (c[0].shooter, c[0].target, c[0].target_pos, c[0].zone),
        (p, monastery, at, 2)
    );
    assert_eq!(find_to(&sim, p), Some(run_back(start)));
    // Explosion in the 100th effect tick: -4000 -20 % = -3200; unoccupied: no experience.
    assert_eq!(until_blast(&mut sim), BLAST_AFTER);
    assert_eq!(hp(&sim, monastery), 6800);
    assert_eq!(rank_and_fraction(&sim, p), (0, 0));
    // Back at the start position.
    assert_eq!(sim.object(p).unwrap().tile, start_tile);
}

#[test]
fn too_little_mana_does_nothing() {
    // T6: 60 < 70 -> silently 0: no run, no target, no message.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 60);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    let start = grid_of(&sim, p);
    tk::run_ticks(&mut sim, 1);
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    let events = tk::run_ticks_collect(&mut sim, 10);
    let events: Vec<_> = events
        .iter()
        .filter(|e| !matches!(e, SimEvent::AttackPoints { .. }))
        .collect();
    assert!(events.is_empty(), "no message: {events:?}");
    let b = bomber(&sim, p);
    assert_eq!(b.target, ObjectId::NONE);
    // Return point and cost are already in the state before the check (re/special-attacks 3.1).
    assert_eq!((b.return_point, b.extra_energy, b.busy), (start, 70, false));
    assert_eq!(find_to(&sim, p), None);
    assert_eq!(grid_of(&sim, p), start);
    assert!(mana(&sim, p) >= 60, "no deduction (regeneration only)");
}

#[test]
fn a_stop_while_laying_still_blows_and_runs_back() {
    // T7, K-318: Halt 1 s after the start of placing aborts walk but not the demolition expert.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    let start = grid_of(&sim, p);
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    until_laying(&mut sim, p);
    tk::run_ticks(&mut sim, 10);
    crate::command::testkit::run(&mut sim, 0, CommandKind::Halt { units: vec![p] });
    // Abort refused (returns false): busy, lock, target and timer stay.
    assert!(!crate::module::abort(&mut sim, p, bomberman::INDEX));
    let b = bomber(&sim, p);
    assert!(b.busy);
    assert_eq!(b.target, monastery);
    assert!(status_on(&sim, p, status::ANIM_LOCKED));
    assert!(fire_pending(&sim, p, bm::FIRE_BUILDING));
    // N+11 ... N+53 without, N+54 with blast and way back as without Halt.
    tk::run_ticks(&mut sim, 43);
    assert!(charges(&sim).is_empty() && bomber(&sim, p).busy);
    tk::run_ticks(&mut sim, 1);
    assert!(!bomber(&sim, p).busy);
    assert_eq!(mana(&sim, p), 30);
    let c = charges(&sim);
    assert_eq!((c.len(), c[0].target), (1, monastery));
    assert_eq!(find_to(&sim, p), Some(run_back(start)));
    assert_eq!(until_blast(&mut sim), BLAST_AFTER);
    assert_eq!(hp(&sim, monastery), 6800);
}

#[test]
fn a_new_order_while_laying_redirects_the_charge() {
    // C-327 (re/special-attacks 4.1, hypothesis in the interplay): a second order during
    // placing overwrites target, cost and return point; the running time marker blasts the new
    // target without a distance check and deducts its cost. Behavior recorded.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let first = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    let second = tk::spawn(&mut sim, tk::MONASTERY, 1, (14, 40));
    order(&mut sim, p, bm::ORDER_BUILDING, first, 454);
    until_laying(&mut sim, p);
    tk::run_ticks(&mut sim, 9);
    order(&mut sim, p, bm::ORDER_BUILDING, second, 454);
    let laying_at = grid_of(&sim, p);
    // N+10: order from the FIFO - abort refused, state overwritten (return point =
    // position at the order), placing continues.
    tk::run_ticks(&mut sim, 1);
    let b = bomber(&sim, p);
    assert!(b.busy);
    assert_eq!(
        (b.target, b.extra_energy, b.return_point),
        (second, 70, laying_at)
    );
    // The new run moves the patriot already during the placing (the lock does not stop walk).
    tk::run_ticks(&mut sim, 43);
    assert!(charges(&sim).is_empty());
    assert_ne!(grid_of(&sim, p), laying_at);
    let far = edge(&mut sim, second, p);
    assert!(far > bomberman::REACH, "new target out of reach ({far})");
    // N+54: charge at the new target, cost of the new order deducted, way back to the position at
    // the second order.
    tk::run_ticks(&mut sim, 1);
    assert_eq!(mana(&sim, p), 30);
    let c = charges(&sim);
    assert_eq!((c.len(), c[0].target), (1, second));
    assert_eq!(find_to(&sim, p), Some(run_back(laying_at)));
    assert_eq!(bomber(&sim, p).target, second);
    // The run to the second monastery is replaced by the way back: no second placing.
    assert_eq!(until_blast(&mut sim), BLAST_AFTER);
    assert_eq!((hp(&sim, first), hp(&sim, second)), (10_000, 6800));
    assert!(!bomber(&sim, p).busy);
    assert!(charges(&sim).is_empty());
}

#[test]
fn an_unreached_building_hangs_without_abort() {
    // C-327: "could not reach target" does not abort for a building.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (30, 10));
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    tk::run_ticks(&mut sim, 1);
    // (a) 0x224 directly at > 4 tiles: no placing, target stays.
    assert!(edge(&mut sim, monastery, p) > bomberman::REACH);
    assert_eq!(send(&mut sim, p, &Message::new(bm::AT_BUILDING, &[])), 0);
    let b = bomber(&sim, p);
    assert!(!b.busy);
    assert_eq!(b.target, monastery);
    assert!(!status_on(&sim, p, status::ANIM_LOCKED));
    assert!(!fire_pending(&sim, p, bm::FIRE_BUILDING));
    // (b) Walk failure (walk cannot move): the order has no failure message -> no 0x224, the
    // target stays, nothing happens.
    crate::module::walk::walk_mut(&mut sim, p).unwrap().can_move = false;
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    tk::run_ticks(&mut sim, 80);
    let b = bomber(&sim, p);
    assert!(!b.busy);
    assert_eq!(b.target, monastery);
    assert!(charges(&sim).is_empty());
    assert_eq!(mana(&sim, p), 100);
}

/// Enemy ship (slot 1, HP 10,000) on `tile`.
fn ship(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    tk::spawn(sim, tk::SHIP, 1, tile)
}

#[test]
fn a_ship_target_takes_4000() {
    // T4, C-329 (ship part): button 455 costs 40; damage -4000 without building reduction, 4000
    // experience (rank 1, fraction 0); after triggering `abort` (EXEMPT_ZONE off, target NONE).
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let s = ship(&mut sim, (16, 10));
    let start = grid_of(&sim, p);
    let at = grid_of(&sim, s);
    order(&mut sim, p, bm::ORDER_SHIP, s, 455);
    tk::run_ticks(&mut sim, 1);
    // Order: collision exemption with the ship, FINDTO argc 6 to the ship position.
    assert!(status_on(&sim, p, status::EXEMPT_ZONE));
    assert_eq!(sim.object(p).unwrap().exempt.object, s);
    assert_eq!(
        find_to(&sim, p),
        Some(Message::new(
            0x200,
            &[at[0], at[2], -1, NO_DIRECTION, -1, 1]
        ))
    );
    assert_eq!(bomber(&sim, p).extra_energy, 40);
    until_laying(&mut sim, p);
    // N: placing animation 370005 = 2, timer 0x624.
    assert_eq!(sim.object(p).unwrap().anim.wanted, 2);
    assert!(fire_pending(&sim, p, bm::FIRE_SHIP));
    tk::run_ticks(&mut sim, 53);
    assert!(charges(&sim).is_empty());
    assert_eq!(mana(&sim, p), 100);
    // N+54: 0x624 - mana 60, charge with animation 370006 = 3 at the ship, way back, abort.
    tk::run_ticks(&mut sim, 1);
    assert_eq!(mana(&sim, p), 60);
    let c = charges(&sim);
    assert_eq!((c.len(), c[0].target, c[0].zone), (1, s, 3));
    let b = bomber(&sim, p);
    assert!(!b.busy);
    assert_eq!(b.target, ObjectId::NONE);
    assert!(!status_on(&sim, p, status::EXEMPT_ZONE));
    assert!(!status_on(&sim, p, status::ANIM_LOCKED));
    assert_eq!(find_to(&sim, p), Some(run_back(start)));
    assert_eq!(until_blast(&mut sim), BLAST_AFTER);
    assert_eq!(hp(&sim, s), 6000);
    // 4000 * 100,000 / 1000 >= 100,000 -> rank 1, excess is lost.
    assert_eq!(rank_and_fraction(&sim, p), (1, 0), "+4000 experience");
}

#[test]
fn the_ship_charge_needs_the_ship_close() {
    // T5, C-328: ship moved > 1 tile before triggering -> charge without target, no
    // damage, mana 60 anyway; `target` stays until the following `abort`.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let s = ship(&mut sim, (16, 10));
    order(&mut sim, p, bm::ORDER_SHIP, s, 455);
    until_laying(&mut sim, p);
    tk::run_ticks(&mut sim, 20);
    let g = sim.object(s).unwrap().grid;
    let away = FxVec3::new(Fx(g.x.0 + 4 * 4096), g.y, g.z);
    crate::world::set_grid_position(&mut sim, s, away, true);
    assert!(edge(&mut sim, s, p) > bomberman::SHIP_REACH);
    tk::run_ticks(&mut sim, 33);
    assert!(charges(&sim).is_empty());
    tk::run_ticks(&mut sim, 1);
    assert_eq!(mana(&sim, p), 60);
    let c = charges(&sim);
    assert_eq!((c.len(), c[0].target), (1, ObjectId::NONE));
    assert_eq!(
        bomber(&sim, p).target,
        ObjectId::NONE,
        "abort after triggering"
    );
    until_blast(&mut sim);
    assert_eq!(hp(&sim, s), 10_000);
    assert_eq!(rank_and_fraction(&sim, p), (0, 0));
}

#[test]
fn the_abort_clears_the_exempt_bit() {
    // K-319: the abort clears EXEMPT_ZONE as soon as a target is set - also after the
    // building branch, which never sets the bit; without a target it stays.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (30, 10));
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    tk::run_ticks(&mut sim, 1);
    assert!(!status_on(&sim, p, status::EXEMPT_ZONE));
    sim.object_mut(p)
        .unwrap()
        .set_status_flags(status::EXEMPT_ZONE, true);
    crate::command::testkit::run(&mut sim, 0, CommandKind::Halt { units: vec![p] });
    assert!(!status_on(&sim, p, status::EXEMPT_ZONE));
    assert_eq!(bomber(&sim, p).target, ObjectId::NONE);
    // Without a target the bit stays.
    sim.object_mut(p)
        .unwrap()
        .set_status_flags(status::EXEMPT_ZONE, true);
    crate::command::testkit::run(&mut sim, 0, CommandKind::Halt { units: vec![p] });
    assert!(status_on(&sim, p, status::EXEMPT_ZONE));
}

#[test]
fn unknown_queries_answer_zero() {
    // K-321: module 36 answers unknown codes with 0 (not -1).
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    for code in [0x24, 0x724, 0x1B24, 0xFF24] {
        assert_eq!(send(&mut sim, p, &Message::new(code, &[])), 0, "{code:#x}");
    }
    assert_eq!(bomber(&sim, p).target, ObjectId::NONE);
}

#[test]
fn no_counterattack_while_laying() {
    // K-322 (observation; the only test with behavior 2 = default): retaliation during
    // placing. Result: it does not reach the switched-off attack module; the placing runs
    // undisturbed.
    let rules = tk::spell_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = patriot(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
    until_laying(&mut sim, p);
    let t = sim.object(p).unwrap().tile;
    let sabre = tk::spawn(&mut sim, tk::SABRE, 1, (t.0 - 2, t.1));
    let guid = sim.object(sabre).unwrap().guid;
    let laying = find_to(&sim, p);
    // N+5: hit by the saber fighter. The attack module has been switched off since the end of N
    // (`GameObject_ApplyModuleActivationChanges` `0x0046CA60` sets `m_Enabled` 0), so
    // `GameObject_SendMessage` (`0x0046E800`, enabled modules only) discards the retaliation `0x911`.
    tk::run_ticks(&mut sim, 5);
    assert!(!sim.object(p).unwrap().has_module(0x11, false));
    let hit = crate::combat::damage::explicit_damage_message(-10, sabre, guid);
    send(&mut sim, p, &hit);
    let attack_target = |sim: &Sim, id| crate::module::attack::attack(sim, id).unwrap().target;
    assert_eq!(attack_target(&sim, p), ObjectId::NONE, "no retaliation");
    // The placing runs undisturbed until N+53, N+54 blasts as without a hit.
    tk::run_ticks(&mut sim, 48);
    assert_eq!(attack_target(&sim, p), ObjectId::NONE);
    assert_eq!(find_to(&sim, p), laying);
    assert_eq!(sim.object(p).unwrap().anim.wanted, 1);
    assert!(bomber(&sim, p).busy && charges(&sim).is_empty());
    tk::run_ticks(&mut sim, 1);
    assert_eq!(charges(&sim).len(), 1);
    assert!(attack_on(&sim, p));

    // Counter-check: the same hit on an idle patriot sets the attack target.
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let q = patriot(&mut sim, (10, 10), 100);
    let sabre = tk::spawn(&mut sim, tk::SABRE, 1, (8, 10));
    tk::run_ticks(&mut sim, 1);
    let hit = crate::combat::damage::explicit_damage_message(-10, sabre, guid);
    send(&mut sim, q, &hit);
    assert_eq!(attack_target(&sim, q), sabre);
}

#[test]
fn the_dynamite_survives_its_bomber() {
    // re/special-attacks 4.4, C-329: a laid charge also explodes after the death of the
    // demolition expert; the source of the damage message `0x17F` is his ID at placing. Measured:
    // dead as well as destroyed -4000 (monastery 6000), no attacker calculation. Death changes
    // the owner to slot 8 with a new ID (re/combat-death 2.1, `QueueOwnerChange` in the phase-0
    // step of the next tick); the old ID of the charge then finds nobody. The building reduction
    // (-3200) would apply only until this change of owner.
    type Vanish = fn(&mut Sim, ObjectId);
    let cases: [(&str, Vanish); 2] = [
        ("dead", |sim, p| {
            send(sim, p, &Message::new(0x110, &[]));
        }),
        ("destroyed", |sim, p| crate::world::destroy_object(sim, p)),
    ];
    for (what, vanish) in cases {
        let rules = tk::spell_rules();
        let mut state = quiet_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let p = patriot(&mut sim, (10, 10), 100);
        let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
        order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
        until_laying(&mut sim, p);
        tk::run_ticks(&mut sim, 54);
        assert_eq!(charges(&sim).len(), 1, "{what}");
        vanish(&mut sim, p);
        assert_eq!(until_blast(&mut sim), BLAST_AFTER, "{what}");
        assert!(sim.object(p).is_none(), "{what}: old ID invalid");
        assert_eq!(hp(&sim, monastery), 6000, "{what}");
    }
}

/// Runs `n` object ticks and follows the ID `id` across the change of owner of the carcass
/// (slot 8, C-142); returns the ID afterwards.
fn follow(sim: &mut Sim, mut id: ObjectId, n: usize) -> ObjectId {
    for e in tk::run_ticks_collect(sim, n) {
        if let SimEvent::OwnerChanged { old, new } = e
            && old == id
        {
            id = new;
        }
    }
    id
}

#[test]
fn a_bomber_dying_while_laying_lays_only_in_the_tick_after() {
    // A demolition expert that dies while `busy` (re/special-attacks 3.9, C-150, K-318): death
    // after object tick N+d, `0x324` is due for N+54. `AbortAllModules` from `StartDying` is
    // refused, the timers stay; the pending list does not take module 36 out of the tick until the end
    // of N+d+1. Measured: d = 52 -> no charge, no deduction, `busy` stays; d = 53 -> the carcass
    // still places the charge in N+54 (mana 30), `0x005039D0`/`0x00503860` does not check `DIED` and
    // switches the attack module back on via the pending list (after the switching off from
    // `StartDying`, so it stays in the tick). The way back `0x100` goes to a dead walk module (only
    // `0xC00`, re/walk 2.1): the carcass does not move. The charge carries the old ID and hits
    // with -4000 as in `the_dynamite_survives_its_bomber`.
    for d in [52, 53] {
        let rules = tk::spell_rules();
        let mut state = quiet_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let p = patriot(&mut sim, (10, 10), 100);
        let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
        order(&mut sim, p, bm::ORDER_BUILDING, monastery, 454);
        until_laying(&mut sim, p);
        tk::run_ticks(&mut sim, d);
        let laying = find_to(&sim, p);
        let at = grid_of(&sim, p);
        send(&mut sim, p, &Message::new(0x110, &[]));
        assert!(status_on(&sim, p, status::DIED), "d = {d}");
        // Abort refused: `busy`, lock and timers stay.
        assert!(bomber(&sim, p).busy, "d = {d}");
        assert!(fire_pending(&sim, p, bm::FIRE_BUILDING), "d = {d}");
        // Until N+54.
        let c = follow(&mut sim, p, 54 - d);
        assert_ne!(c, p, "d = {d}: carcass on slot 8");
        assert_eq!(c.slot(), 8, "d = {d}");
        assert!(
            !sim.object(c)
                .unwrap()
                .tick_list
                .contains(&(bomberman::INDEX as u8)),
            "d = {d}"
        );
        if d == 52 {
            assert!(charges(&sim).is_empty(), "d = {d}: no charge");
            assert_eq!(mana(&sim, c), 100, "d = {d}: no deduction");
            assert!(bomber(&sim, c).busy, "d = {d}: busy stays");
            assert!(fire_pending(&sim, c, bm::FIRE_BUILDING), "d = {d}");
            assert!(!attack_on(&sim, c), "d = {d}");
        } else {
            let charge = charges(&sim);
            assert_eq!(charge.len(), 1, "d = {d}: the carcass still places");
            assert_eq!((charge[0].shooter, charge[0].target), (p, monastery));
            assert_eq!(mana(&sim, c), 30, "d = {d}");
            assert!(!bomber(&sim, c).busy, "d = {d}");
            assert!(
                attack_on(&sim, c),
                "d = {d}: attack module back in the tick"
            );
        }
        // The carcass does not move; the walk module keeps the old run.
        let c = follow(&mut sim, c, 150);
        assert_eq!(grid_of(&sim, c), at, "d = {d}");
        assert_eq!(find_to(&sim, c), laying, "d = {d}");
        assert!(charges(&sim).is_empty(), "d = {d}");
        let hp_after = if d == 52 { 10_000 } else { 6000 };
        assert_eq!(hp(&sim, monastery), hp_after, "d = {d}");
    }
}

// ---------------------------------------------------------------------------------------------
// Module 37 killer: near with the knife, far with the shot (re/special-attacks 3.4, 3.5, 3.9, 9
// T8 to T12). "N" = object tick in which the animation begins (arrival or end of the
// turn). Actor and victim have behavior 0; the targets have max HP >= 500 so that
// the hits stay readable (rifleman 1403 with 1000, medicine man 1716 with
// 800, missionary 1113 with 1000).

fn kil<'s>(sim: &'s Sim, id: ObjectId) -> &'s killer::Killer {
    killer::killer(sim, id).expect("module 37")
}

fn killer_pending(sim: &Sim, id: ObjectId, code: u32) -> bool {
    sim.module(id, killer::INDEX)
        .unwrap()
        .base
        .has_pending_timer(code)
}

/// Ticks up to the tick N in which the timer `code` of the killer is due; returns the
/// events of tick N.
fn until_timer(sim: &mut Sim, k: ObjectId, code: u32) -> Vec<SimEvent> {
    for _ in 0..300 {
        let events = tk::run_ticks_collect(sim, 1);
        if killer_pending(sim, k, code) {
            return events;
        }
    }
    panic!("timer {code:#x} never comes");
}

fn world_of(sim: &Sim, id: ObjectId) -> [f32; 3] {
    crate::combat::hit_zone::world_pos(sim, id).unwrap()
}

/// Sound and effect events (without accompanying messages like `UnderAttack`).
fn shot_events(events: &[SimEvent]) -> Vec<SimEvent> {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                SimEvent::Sound3d { .. } | SimEvent::EffectAttached { .. }
            )
        })
        .cloned()
        .collect()
}

/// Near attack up to the hit: order `{target, button}`, arrival in N, hit in N + `hit`.
/// Checks order, arrival (animation, lock, sound, timer) and the hit tick.
fn stab(sim: &mut Sim, k: ObjectId, v: ObjectId, button: i32, anim: i32, hit: usize) {
    let mana_before = mana(sim, k);
    order(sim, k, km::KILL, v, button);
    tk::run_ticks(sim, 1);
    // Order: target remembered, walk FINDTO_UNIT to the victim (maximum radius 2048).
    assert_eq!(kil(sim, k).target, v);
    let w = crate::module::walk::walk(sim, k).unwrap();
    assert_eq!((w.quarry, w.max_attack_radius), (v, killer::WALK_MAX));
    let events = until_timer(sim, k, km::KILL_HIT);
    // N: knife animation, lock, attack out of the tick, sound 6/0x42 at the own position.
    assert_eq!(sim.object(k).unwrap().anim.wanted, anim);
    assert!(status_on(sim, k, status::ANIM_LOCKED) && !attack_on(sim, k));
    assert!(edge(sim, k, v) <= killer::REACH);
    let guid = sim.object(k).unwrap().guid;
    assert_eq!(
        shot_events(&events),
        vec![SimEvent::Sound3d {
            object: k,
            action: 6,
            flags: 0x42,
            pos: world_of(sim, k),
            guid,
        }]
    );
    let full = hp(sim, v);
    tk::run_ticks(sim, hit - 1);
    assert_eq!(hp(sim, v), full, "no hit yet");
    assert!(status_on(sim, k, status::ANIM_LOCKED));
    assert_eq!(
        mana(sim, k),
        mana_before,
        "deduction not until the hit (K-320)"
    );
    tk::run_ticks(sim, 1);
    assert!(!status_on(sim, k, status::ANIM_LOCKED) && attack_on(sim, k));
}

#[test]
fn the_shadow_warrior_kills_with_the_bonus() {
    // T8: shadow warrior 1714 (mana 200, button 548 costs 50) against the pure ranged fighter
    // 1403: hit N+11, -300 * 1.5 = -450 (close against ranged, re/special-attacks 3.4).
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::SHADOW_WARRIOR, (10, 10), 200);
    let v = tk::spawn(&mut sim, tk::RANGED, 1, (16, 10));
    stab(&mut sim, k, v, 548, 5, 11);
    assert_eq!(hp(&sim, v), 550);
    assert_eq!(mana(&sim, k), 150);
    assert_eq!(kil(&sim, k).extra_energy, 50);
    // Experience 450 (-d/1000, d = -450,000): Max(30) = 100 at rank 0, scale 10 ->
    // fraction 450 * 100,000 / 1000 = 45,000, no rank-up (measured).
    assert_eq!(rank_and_fraction(&sim, k), (0, 45_000));
}

#[test]
fn the_trapper_stabs_a_walker() {
    // T9: trapper 2015 (mana 150, button 456 costs 60) against an enemy unit without a
    // ranged module (medicine man 1716 instead of colonist): -300 in N+8.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::TRAPPER, (10, 10), 150);
    let v = tk::spawn(&mut sim, tk::MEDICINE_MAN, 1, (15, 12));
    stab(&mut sim, k, v, 456, 8, 8);
    assert_eq!(hp(&sim, v), 500);
    assert_eq!(mana(&sim, k), 90);
}

/// Face order of sniping (re/special-attacks 3.5): FINDTO argc 7, target = own grid position,
/// look-at point = grid position of the target.
fn snipe_turn(sim: &Sim, k: ObjectId, v: ObjectId) -> Message {
    let (g, t) = (grid_of(sim, k), grid_of(sim, v));
    Message::new(0x200, &[g[0], g[2], -1, t[0], t[2], -1, 1])
}

/// Ranged attack to the end: order `{target, button}`, turn, animation in N, shot in
/// N + `shot`, end in N + `end`. Checks face order, animation, shot events and end;
/// returns the mana right after the shot (it regenerates again until the end).
fn snipe(
    sim: &mut Sim,
    k: ObjectId,
    v: ObjectId,
    button: i32,
    anim: i32,
    times: (usize, usize),
) -> i32 {
    let (shot, end) = times;
    let mana_before = mana(sim, k);
    let own = grid_of(sim, k);
    let turn = snipe_turn(sim, k, v);
    order(sim, k, km::SNIPE, v, button);
    let mut turning = 0;
    loop {
        tk::run_ticks(sim, 1);
        if killer_pending(sim, k, km::SNIPE_SHOT) {
            break;
        }
        assert_eq!(find_to(sim, k), Some(turn.clone()), "face order");
        turning += 1;
        assert!(turning < 50, "turn never ends");
    }
    assert!(turning > 0, "turn takes ticks");
    assert_eq!(kil(sim, k).target, v);
    assert_eq!(grid_of(sim, k), own, "on the spot");
    // N: fire animation, lock, attack out of the tick; shot and end are due.
    assert_eq!(sim.object(k).unwrap().anim.wanted, anim);
    assert!(status_on(sim, k, status::ANIM_LOCKED) && !attack_on(sim, k));
    assert!(killer_pending(sim, k, km::SNIPE_END));
    let full = hp(sim, v);
    tk::run_ticks(sim, shot - 1);
    assert_eq!(hp(sim, v), full, "no shot yet");
    assert_eq!(mana(sim, k), mana_before);
    // N + shot: sound 1/3 at the own position, muzzle `gun01`, blood at the target, hit, mana.
    let events = tk::run_ticks_collect(sim, 1);
    let guid = sim.object(k).unwrap().guid;
    let shown = shot_events(&events);
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert_eq!(
        shown[0],
        SimEvent::Sound3d {
            object: k,
            action: 1,
            flags: 3,
            pos: world_of(sim, k),
            guid,
        }
    );
    assert_eq!(
        shown[1],
        SimEvent::EffectAttached {
            object: k,
            path: "data/effects/normaler_schuss.pef".into(),
            node: "gun01".into(),
            offset: [0.0; 3],
        }
    );
    let SimEvent::EffectAttached {
        object, path, node, ..
    } = &shown[2]
    else {
        panic!("blood expected: {:?}", shown[2]);
    };
    assert_eq!(
        (*object, path.as_str(), node.as_str()),
        (v, "data/effects/blood.pef", "")
    );
    let paid = mana(sim, k);
    assert!(
        status_on(sim, k, status::ANIM_LOCKED),
        "locked until the end"
    );
    tk::run_ticks(sim, end - shot - 1);
    assert!(status_on(sim, k, status::ANIM_LOCKED) && !attack_on(sim, k));
    // N + end: 0x625 - lock off, attack back in the tick.
    tk::run_ticks(sim, 1);
    assert!(!status_on(sim, k, status::ANIM_LOCKED) && attack_on(sim, k));
    paid
}

#[test]
fn the_pistolero_snipes_after_turning() {
    // T10: pistoleer 1115 (mana 200, button 546 costs 40) against the missionary 1113: shot
    // N+17 after the turn, -300 (no module 18 -> no bonus), end N+38.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::PISTOLERO, (10, 10), 200);
    let v = tk::spawn(&mut sim, tk::MISSIONARY, 1, (13, 16));
    let paid = snipe(&mut sim, k, v, 546, 9, (17, 38));
    assert_eq!(hp(&sim, v), 700);
    assert_eq!(paid, 160);
    assert_eq!(kil(&sim, k).extra_energy, 40);
}

#[test]
fn the_sharpshooter_gets_the_melee_bonus() {
    // T11, re/special-attacks 8 no. 8: sharpshooter 2616 (mana 250, button 457 costs 50) with
    // module 18 against the pure ranged fighter: -450, shot N+14, end N+31.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::SHARPSHOOTER, (10, 10), 250);
    let v = tk::spawn(&mut sim, tk::RANGED, 1, (18, 13));
    let paid = snipe(&mut sim, k, v, 457, 1, (14, 31));
    assert_eq!(hp(&sim, v), 550);
    assert_eq!(paid, 200);
}

#[test]
fn a_swimming_killer_does_nothing() {
    // T12: when swimming, sniping (`0x425`, before everything else) and arrival (`0x225`, after
    // the distance check) abort silently.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = attacker(&mut sim, tk::PISTOLERO, (10, 10), 200);
    let s = attacker(&mut sim, tk::SHADOW_WARRIOR, (20, 10), 200);
    let v = tk::spawn(&mut sim, tk::MEDICINE_MAN, 1, (21, 10));
    tk::run_ticks(&mut sim, 1);
    for id in [p, s] {
        sim.object_mut(id)
            .unwrap()
            .set_status_flags(status::SWIMMING, true);
    }
    let order = Message::new(km::SNIPE, &[v.as_i32(), 546]);
    assert_eq!(send(&mut sim, p, &order), 0);
    let k = kil(&sim, p);
    assert_eq!((k.target, k.extra_energy), (ObjectId::NONE, 0));
    assert_eq!(find_to(&sim, p), None);
    killer::killer_mut(&mut sim, s).unwrap().target = v;
    assert!(edge(&mut sim, s, v) <= killer::REACH);
    assert_eq!(send(&mut sim, s, &Message::new(km::KILL_AT, &[])), 0);
    assert!(!status_on(&sim, s, status::ANIM_LOCKED));
    assert!(!killer_pending(&sim, s, km::KILL_HIT));
    tk::run_ticks(&mut sim, 20);
    assert_eq!(hp(&sim, v), 800);
    assert!(mana(&sim, p) >= 200 && mana(&sim, s) >= 200, "no deduction");
}

#[test]
fn a_runaway_victim_is_not_hit() {
    // re/special-attacks 3.4: if the victim at arrival (`0x225`, also as fault of the aborted
    // run) is > 2 tiles away, "victim ran away": no hit, no mana.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::SHADOW_WARRIOR, (10, 10), 200);
    let v = tk::spawn(&mut sim, tk::MEDICINE_MAN, 1, (18, 10));
    order(&mut sim, k, km::KILL, v, 548);
    tk::run_ticks(&mut sim, 2);
    let g = sim.object(v).unwrap().grid;
    let away = FxVec3::new(Fx(g.x.0 + 20 * 4096), g.y, g.z);
    crate::world::set_grid_position(&mut sim, v, away, true);
    assert!(edge(&mut sim, k, v) > killer::REACH);
    // 0x225 directly.
    assert_eq!(send(&mut sim, k, &Message::new(km::KILL_AT, &[])), 0);
    assert!(!status_on(&sim, k, status::ANIM_LOCKED));
    assert!(!killer_pending(&sim, k, km::KILL_HIT));
    // Abort walk -> failure message 0x225 -> likewise nothing.
    assert!(crate::module::abort(&mut sim, k, 0));
    assert!(!killer_pending(&sim, k, km::KILL_HIT));
    tk::run_ticks(&mut sim, 30);
    assert!(!status_on(&sim, k, status::ANIM_LOCKED));
    assert_eq!(hp(&sim, v), 800);
    assert_eq!(mana(&sim, k), 200);
    assert_eq!(kil(&sim, k).target, v, "target stays");
}

#[test]
fn the_hit_ignores_a_late_escape() {
    // C-331: the hit checks no distance - the victim is far away after the animation start and
    // is hit at marker 1 anyway.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::SHADOW_WARRIOR, (10, 10), 200);
    let v = tk::spawn(&mut sim, tk::MEDICINE_MAN, 1, (14, 10));
    order(&mut sim, k, km::KILL, v, 548);
    until_timer(&mut sim, k, km::KILL_HIT);
    tk::run_ticks(&mut sim, 2);
    let g = sim.object(v).unwrap().grid;
    let away = FxVec3::new(Fx(g.x.0 + 20 * 4096), g.y, g.z);
    crate::world::set_grid_position(&mut sim, v, away, true);
    assert!(edge(&mut sim, k, v) > killer::REACH);
    tk::run_ticks(&mut sim, 8);
    assert_eq!(hp(&sim, v), 800);
    // N+11.
    tk::run_ticks(&mut sim, 1);
    assert_eq!(hp(&sim, v), 500);
    assert_eq!(mana(&sim, k), 150);
}

#[test]
fn an_aborted_kill_costs_nothing() {
    // re/special-attacks 4.1, 3.9: a Halt between arrival and hit aborts - no hit, mana full,
    // lock off, attack back in the tick; the target stays.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::SHADOW_WARRIOR, (10, 10), 200);
    let v = tk::spawn(&mut sim, tk::RANGED, 1, (14, 10));
    order(&mut sim, k, km::KILL, v, 548);
    until_timer(&mut sim, k, km::KILL_HIT);
    tk::run_ticks(&mut sim, 4);
    crate::command::testkit::run(&mut sim, 0, CommandKind::Halt { units: vec![k] });
    assert!(!status_on(&sim, k, status::ANIM_LOCKED));
    assert!(!killer_pending(&sim, k, km::KILL_HIT));
    assert_eq!(kil(&sim, k).target, v);
    tk::run_ticks(&mut sim, 20);
    assert!(attack_on(&sim, k));
    assert_eq!(hp(&sim, v), 1000);
    assert!(mana(&sim, k) >= 200, "no deduction");
}

#[test]
fn a_vanished_snipe_target_aborts() {
    // re/special-attacks 3.5, 3.9: target destroyed before marker 3 -> "target is dead", `abort`:
    // lock off, attack on, the end `0x625` is omitted, no mana, no shot events; the target stays.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::PISTOLERO, (10, 10), 200);
    let v = tk::spawn(&mut sim, tk::MISSIONARY, 1, (13, 16));
    order(&mut sim, k, km::SNIPE, v, 546);
    until_timer(&mut sim, k, km::SNIPE_SHOT);
    tk::run_ticks(&mut sim, 5);
    crate::world::destroy_object(&mut sim, v);
    assert!(sim.object(v).is_none());
    tk::run_ticks(&mut sim, 11);
    assert!(status_on(&sim, k, status::ANIM_LOCKED));
    // N+17: shot without target.
    let events = tk::run_ticks_collect(&mut sim, 1);
    assert_eq!(shot_events(&events), vec![]);
    assert!(!status_on(&sim, k, status::ANIM_LOCKED) && attack_on(&sim, k));
    assert!(!killer_pending(&sim, k, km::SNIPE_END));
    assert_eq!(kil(&sim, k).target, v);
    tk::run_ticks(&mut sim, 30);
    assert!(mana(&sim, k) >= 200, "no deduction");
}

#[test]
fn snipe_ignores_range_and_sight() {
    // C-332: the order directly at the module shoots without range, sight and fog check.
    let rules = tk::spell_rules();
    let mut state = tk::state_fogged(&rules);
    state.combat.behavior_mode[0] = 0;
    state.combat.behavior_mode[1] = 0;
    let mut sim = Sim::new(&mut state, &rules);
    let k = attacker(&mut sim, tk::PISTOLERO, (4, 4), 200);
    let v = tk::spawn(&mut sim, tk::MEDICINE_MAN, 1, (58, 60));
    assert!(edge(&mut sim, k, v) > 40 * 4096);
    order(&mut sim, k, km::SNIPE, v, 546);
    until_timer(&mut sim, k, km::SNIPE_SHOT);
    tk::run_ticks(&mut sim, 16);
    assert_eq!(hp(&sim, v), 800);
    tk::run_ticks(&mut sim, 1);
    assert_eq!(hp(&sim, v), 500);
    assert_eq!(mana(&sim, k), 160);
}

#[test]
fn no_counterattack_while_killing() {
    // K-322 (observation; the only killer test with behavior 2 = default): as with the demolition
    // expert the retaliation `0x911` does not reach the switched-off attack module
    // (`GameObject_ApplyModuleActivationChanges` `0x0046CA60` sets `m_Enabled` 0,
    // `GameObject_SendMessage` `0x0046E800` delivers only to enabled modules) - neither with the
    // knife nor with sniping; the hit comes as without retaliation.
    let rules = tk::spell_rules();
    // (actor, victim tile, order, button, hit timer, hit tick N+...)
    type Case = (u32, (i32, i32), u32, i32, u32, usize);
    let cases: [Case; 2] = [
        (
            tk::SHADOW_WARRIOR,
            (12, 10),
            km::KILL,
            548,
            km::KILL_HIT,
            11,
        ),
        (tk::PISTOLERO, (13, 16), km::SNIPE, 546, km::SNIPE_SHOT, 17),
    ];
    for (guid, tile, code, button, timer, hit) in cases {
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let k = attacker(&mut sim, guid, (10, 10), 200);
        let v = tk::spawn(&mut sim, tk::MISSIONARY, 1, tile);
        order(&mut sim, k, code, v, button);
        until_timer(&mut sim, k, timer);
        let sabre = tk::spawn(&mut sim, tk::SABRE, 1, (10, 7));
        let sabre_guid = sim.object(sabre).unwrap().guid;
        // N+1: the attack module has been switched off since the end of N.
        tk::run_ticks(&mut sim, 1);
        assert!(!sim.object(k).unwrap().has_module(0x11, false), "{guid}");
        let attack_target = |sim: &Sim| crate::module::attack::attack(sim, k).unwrap().target;
        let before = attack_target(&sim);
        let counter = Message::new(0x911, &[sabre.as_i32()]);
        assert_eq!(
            send(&mut sim, k, &counter),
            -1,
            "{guid}: retaliation discarded"
        );
        let blow = crate::combat::damage::explicit_damage_message(-10, sabre, sabre_guid);
        send(&mut sim, k, &blow);
        assert_eq!(attack_target(&sim), before, "{guid}: no retaliation");
        assert_ne!(attack_target(&sim), sabre, "{guid}");
        tk::run_ticks(&mut sim, hit - 2);
        assert_eq!(hp(&sim, v), 1000, "{guid}");
        tk::run_ticks(&mut sim, 1);
        assert_eq!(hp(&sim, v), 700, "{guid}: hit as without retaliation");
    }
}

// ---------------------------------------------------------------------------------------------
// Module 49 smith: whirlwind and "Damage building" (re/special-attacks 3.6, 3.7, 3.9, 9
// T13 to T16). "N" = object tick in which the animation begins (`busy`). The whirlwind comes
// directly as `0x131 {0, 0}` via the FIFO (without the command `0x4F`, which deducts the mana),
// the hammering as `0x231 {1, 1, target, 60}`. Actor and victim have behavior 0; the smith faces
// 0 degrees (`set_direction`).

thread_local! {
    /// Delivered hits `0x331` to module 49: target (argument 0) in order of delivery.
    static SMITH_HITS: std::cell::RefCell<Vec<ObjectId>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Delivered executions `0x131` to module 49.
    static SMITH_EXECUTES: std::cell::RefCell<usize> = const { std::cell::RefCell::new(0) };
}

fn record_smith(_: &mut Sim, _: ObjectId, index: usize, m: &Message) {
    if index != special_move::INDEX {
        return;
    }
    match m.code {
        sm::HIT => SMITH_HITS.with(|s| s.borrow_mut().push(ObjectId::from_i32(m.arg(0)))),
        sm::EXECUTE => SMITH_EXECUTES.with(|s| *s.borrow_mut() += 1),
        _ => {}
    }
}

/// `n` object ticks under observation: the hits `0x331` delivered in these ticks.
fn smith_hits_in(sim: &mut Sim, n: usize) -> Vec<ObjectId> {
    SMITH_HITS.with(|s| s.borrow_mut().clear());
    crate::module::test_hook::observe(record_smith, || tk::run_ticks(sim, n));
    SMITH_HITS.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

fn smove<'s>(sim: &'s Sim, id: ObjectId) -> &'s special_move::SpecialMove {
    special_move::special_move(sim, id).expect("module 49")
}

fn smith_pending(sim: &Sim, id: ObjectId, code: u32) -> bool {
    sim.module(id, special_move::INDEX)
        .unwrap()
        .base
        .has_pending_timer(code)
}

/// Smith 2013 (slot 0) on `tile` with mana `mana`, facing 0 degrees, rank 0.
fn smith(sim: &mut Sim, tile: (i32, i32), mana: i32) -> ObjectId {
    let s = attacker(sim, tk::SMITH, tile, mana);
    crate::world::set_direction(sim, s, 0);
    s
}

/// Enemy riflemen 1403 (slot 1, HP 1000) on `tile + offset`.
fn enemies_around(sim: &mut Sim, tile: (i32, i32), offsets: &[(i32, i32)]) -> Vec<ObjectId> {
    offsets
        .iter()
        .map(|&(dx, dz)| tk::spawn(sim, tk::RANGED, 1, (tile.0 + dx, tile.1 + dz)))
        .collect()
}

/// Whirlwind `0x131 {0, 0}` via the FIFO with abort.
fn whirl(sim: &mut Sim, s: ObjectId) {
    let m = Message::new(sm::EXECUTE, &[0, 0]);
    assert!(post_message(sim, s, m, true));
}

/// Object ticks up to N (`busy`); returns N and the events of tick N.
fn until_busy(sim: &mut Sim, s: ObjectId) -> (usize, Vec<SimEvent>) {
    for n in 1..=200 {
        let events = tk::run_ticks_collect(sim, 1);
        if smove(sim, s).busy {
            return (n, events);
        }
    }
    panic!("smith never begins");
}

/// Damage numbers `(target, points)` of the events.
fn damage_numbers(events: &[SimEvent]) -> Vec<(ObjectId, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::DamageNumber { object, points } => Some((*object, *points)),
            _ => None,
        })
        .collect()
}

/// Sectors 0 to 315 degrees at facing 0 degrees as tile offset (`x + cos`, `z + sin`).
const SECTORS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// Hit ticks of the sectors 0...7 relative to N (time markers: `t2 = 1283`, `step =
/// (1843 − 1283)/8 = 70`) and the end `0x431` (marker 1 = 3266).
const WHIRL_TICKS: [usize; 8] = [13, 14, 15, 15, 16, 17, 18, 18];
const WHIRL_END: usize = 33;

#[test]
fn the_whirlwind_hits_three_enemies_in_sectors() {
    // T13, C-333: three enemies on the neighboring tiles at 0, 45 and 90 degrees -> hits
    // N+13, N+14, N+15 each -60 % max HP with damage number; end N+33.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let foes = enemies_around(&mut sim, (10, 10), &SECTORS[..3]);
    whirl(&mut sim, s);
    let (_, events) = until_busy(&mut sim, s);
    // N: animation [0] = 4, lock, attack out of the tick, hit and end pending.
    assert_eq!(sim.object(s).unwrap().anim.wanted, 4);
    assert!(status_on(&sim, s, status::ANIM_LOCKED) && !attack_on(&sim, s));
    assert_eq!(smove(&sim, s).mode, 0);
    assert!(smith_pending(&sim, s, sm::HIT) && smith_pending(&sim, s, sm::END));
    assert!(damage_numbers(&events).is_empty());
    assert_eq!(
        mana(&sim, s),
        100,
        "only the command deducts the mana (K-320)"
    );
    let mut hit_at = Vec::new();
    for n in 1..=WHIRL_END {
        let events = tk::run_ticks_collect(&mut sim, 1);
        for (target, points) in damage_numbers(&events) {
            hit_at.push((n, target, points));
        }
        if n < WHIRL_END {
            assert!(smove(&sim, s).busy, "N+{n}");
            assert!(status_on(&sim, s, status::ANIM_LOCKED), "N+{n}");
        }
    }
    assert_eq!(
        hit_at,
        vec![
            (WHIRL_TICKS[0], foes[0], -600),
            (WHIRL_TICKS[1], foes[1], -600),
            (WHIRL_TICKS[2], foes[2], -600),
        ]
    );
    for &f in &foes {
        assert_eq!(hp(&sim, f), 400);
    }
    // N+33: `0x431` - lock off, attack back in the tick, no longer busy.
    assert!(!smove(&sim, s).busy);
    assert!(!status_on(&sim, s, status::ANIM_LOCKED) && attack_on(&sim, s));
}

#[test]
fn a_whirlwind_without_enemies_loses_nothing_but_time() {
    // T14: no enemy within radius 2 (one 3 tiles away, an own one next to it) -> "Get Nearest
    // has failed!": no animation, no lock, no timers, `busy` 0.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let far = tk::spawn(&mut sim, tk::RANGED, 1, (13, 10));
    let own = tk::spawn(&mut sim, tk::RANGED, 0, (11, 10));
    tk::run_ticks(&mut sim, 1);
    let anim = sim.object(s).unwrap().anim.wanted;
    whirl(&mut sim, s);
    let events = tk::run_ticks_collect(&mut sim, 2);
    assert!(!smove(&sim, s).busy);
    assert_eq!(sim.object(s).unwrap().anim.wanted, anim, "no animation");
    assert!(!status_on(&sim, s, status::ANIM_LOCKED) && attack_on(&sim, s));
    assert!(!smith_pending(&sim, s, sm::HIT) && !smith_pending(&sim, s, sm::END));
    assert!(damage_numbers(&events).is_empty());
    tk::run_ticks(&mut sim, WHIRL_END);
    assert_eq!((hp(&sim, far), hp(&sim, own)), (1000, 1000));
}

#[test]
fn same_tick_hits_land_in_reverse_sector_order() {
    // T16, C-333: eight enemies all around -> eight hits. The timers of a handler are in
    // reverse order in `m_Events`: in N+15 sector 3 hits before 2, in N+18 sector 7 before
    // 6 (delivery via `test_hook::observe`).
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let foes = enemies_around(&mut sim, (10, 10), &SECTORS);
    whirl(&mut sim, s);
    until_busy(&mut sim, s);
    let mut seen = Vec::new();
    for n in 1..=WHIRL_END {
        let hits = smith_hits_in(&mut sim, 1);
        if !hits.is_empty() {
            seen.push((n, hits));
        }
    }
    let f = |i: usize| foes[i];
    assert_eq!(
        seen,
        vec![
            (13, vec![f(0)]),
            (14, vec![f(1)]),
            (15, vec![f(3), f(2)]),
            (16, vec![f(4)]),
            (17, vec![f(5)]),
            (18, vec![f(7), f(6)]),
        ]
    );
    for &f in &foes {
        assert_eq!(hp(&sim, f), 400);
    }
    assert!(!smove(&sim, s).busy);
}

#[test]
fn a_stop_during_the_whirlwind_keeps_the_hits() {
    // K-318: Halt after the first hit - the abort is refused (`busy`): timers, lock and `busy`
    // stay, the remaining hits and `0x431` come as without Halt.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let foes = enemies_around(&mut sim, (10, 10), &SECTORS[..3]);
    whirl(&mut sim, s);
    until_busy(&mut sim, s);
    tk::run_ticks(&mut sim, WHIRL_TICKS[0]);
    assert_eq!(hp(&sim, foes[0]), 400);
    crate::command::testkit::run(&mut sim, 0, CommandKind::Halt { units: vec![s] });
    assert!(!crate::module::abort(&mut sim, s, special_move::INDEX));
    assert!(smove(&sim, s).busy);
    assert!(status_on(&sim, s, status::ANIM_LOCKED));
    assert!(smith_pending(&sim, s, sm::HIT) && smith_pending(&sim, s, sm::END));
    let rest = smith_hits_in(&mut sim, WHIRL_END - WHIRL_TICKS[0]);
    assert_eq!(rest, vec![foes[1], foes[2]]);
    assert_eq!((hp(&sim, foes[1]), hp(&sim, foes[2])), (400, 400));
    assert!(!smove(&sim, s).busy);
    assert!(!status_on(&sim, s, status::ANIM_LOCKED) && attack_on(&sim, s));
    // After the end the abort is possible again (`true`).
    assert!(crate::module::abort(&mut sim, s, special_move::INDEX));
}

#[test]
fn a_smith_dying_in_the_whirlwind_hits_only_in_the_tick_after() {
    // A smith dying while `busy` after object tick N+d (hits in N+13/14/15; re/special-attacks
    // 3.9, C-150, K-318). The abort is refused; module 49 ticks once more in N+d+1 and is off
    // afterwards. Measured: d = 11 -> no hit; d = 12 -> only the hit from N+13; `busy` stays at
    // the carcass.
    for (d, hits) in [(11, 0), (12, 1)] {
        let rules = tk::spell_rules();
        let mut state = quiet_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = smith(&mut sim, (10, 10), 100);
        let foes = enemies_around(&mut sim, (10, 10), &SECTORS[..3]);
        whirl(&mut sim, s);
        until_busy(&mut sim, s);
        tk::run_ticks(&mut sim, d);
        send(&mut sim, s, &Message::new(0x110, &[]));
        assert!(smove(&sim, s).busy, "d = {d}");
        let c = follow(&mut sim, s, WHIRL_END + 5 - d);
        assert_ne!(c, s, "d = {d}: carcass on slot 8");
        assert!(smove(&sim, c).busy, "d = {d}: no more `0x431`");
        assert!(
            !sim.object(c)
                .unwrap()
                .tick_list
                .contains(&(special_move::INDEX as u8)),
            "d = {d}"
        );
        let expected: Vec<i32> = (0..3).map(|i| if i < hits { 400 } else { 1000 }).collect();
        let got: Vec<i32> = foes.iter().map(|&f| hp(&sim, f)).collect();
        assert_eq!(got, expected, "d = {d}");
    }
}

/// "Damage building" `0x231 {1, 1, target, 60}` via the FIFO with abort (like the command
/// `0x5F` with button 551).
fn hammer_order(sim: &mut Sim, s: ObjectId, target: ObjectId) {
    let m = Message::new(sm::GOTO_BUILDING, &[1, 1, target.as_i32(), 60]);
    assert!(post_message(sim, s, m, true));
}

#[test]
fn the_hammer_hits_the_building_after_arrival() {
    // T15, C-334: mana 100, order against an enemy monastery (HP 10,000): run FINDTO argc 3
    // (tolerance argument 2048), on arrival mana 40, -40 % max HP in the next tick (animation 5
    // without marker 2), end N+34.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    hammer_order(&mut sim, s, monastery);
    tk::run_ticks(&mut sim, 1);
    let walk = [monastery.as_i32(), special_move::WALK_TOLERANCE, 1];
    assert_eq!(find_to(&sim, s), Some(Message::new(0x200, &walk)));
    assert_eq!(
        mana(&sim, s),
        100,
        "deduction not until after the run (K-320)"
    );
    let (n, events) = until_busy(&mut sim, s);
    assert!(n > 2, "run to the monastery ({n} ticks)");
    // N: arrival - mana deducted, animation [1] = 5, lock, attack off, mode 1.
    assert_eq!(mana(&sim, s), 40);
    assert_eq!(sim.object(s).unwrap().anim.wanted, 5);
    assert!(status_on(&sim, s, status::ANIM_LOCKED) && !attack_on(&sim, s));
    assert_eq!(smove(&sim, s).mode, 1);
    assert!(damage_numbers(&events).is_empty());
    assert_eq!(hp(&sim, monastery), 10_000);
    // N+1: hit -40 % of 10,000 with damage number.
    let events = tk::run_ticks_collect(&mut sim, 1);
    assert_eq!(damage_numbers(&events), vec![(monastery, -4000)]);
    assert_eq!(hp(&sim, monastery), 6000);
    // Busy until N+33, end N+34.
    tk::run_ticks(&mut sim, 32);
    assert!(smove(&sim, s).busy && status_on(&sim, s, status::ANIM_LOCKED));
    tk::run_ticks(&mut sim, 1);
    assert!(!smove(&sim, s).busy);
    assert!(!status_on(&sim, s, status::ANIM_LOCKED) && attack_on(&sim, s));
    assert_eq!(hp(&sim, monastery), 6000, "only one hit");

    // No distance and target check after the run (C-334): `0x131 {1, 1, target, 60}` directly
    // from 30 tiles still hits; against a vanished target the animation runs without a hit.
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (40, 10));
    let gone = tk::spawn(&mut sim, tk::MONASTERY, 1, (10, 40));
    crate::world::destroy_object(&mut sim, gone);
    // Created outside a tick, the timers do not go into `m_Events` until the end of the next
    // `update_events`: hit after two ticks (measured).
    let execute = |t: ObjectId| Message::new(sm::EXECUTE, &[1, 1, t.as_i32(), 30]);
    assert_eq!(send(&mut sim, s, &execute(monastery)), 0);
    assert!(smove(&sim, s).busy);
    let events = tk::run_ticks_collect(&mut sim, 2);
    assert_eq!(damage_numbers(&events), vec![(monastery, -4000)]);
    tk::run_ticks(&mut sim, 34);
    assert!(!smove(&sim, s).busy);
    let before = mana(&sim, s);
    assert_eq!(send(&mut sim, s, &execute(gone)), 0);
    assert!(smove(&sim, s).busy, "animation despite missing target");
    assert_eq!(mana(&sim, s), before - 30, "cost deducted anyway");
    let events = tk::run_ticks_collect(&mut sim, 2);
    assert!(damage_numbers(&events).is_empty());
}

#[test]
fn hammering_without_mana_after_the_walk_does_nothing() {
    // K-320: hammering checks again after the run. Mana set to 50 (< 60) during the run ->
    // `0x131` arrives, but no animation, no deduction, no hit.
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (18, 10));
    hammer_order(&mut sim, s, monastery);
    tk::run_ticks(&mut sim, 1);
    sim.object_mut(s)
        .unwrap()
        .attributes
        .set_current_value(15, 50, 0);
    SMITH_EXECUTES.with(|e| *e.borrow_mut() = 0);
    crate::module::test_hook::observe(record_smith, || {
        for _ in 0..80 {
            tk::run_ticks(&mut sim, 1);
            assert!(!smove(&sim, s).busy);
        }
    });
    assert_eq!(SMITH_EXECUTES.with(|e| *e.borrow()), 1, "arrival reported");
    assert!((50..60).contains(&mana(&sim, s)), "no deduction");
    assert!(!status_on(&sim, s, status::ANIM_LOCKED));
    assert_ne!(sim.object(s).unwrap().anim.wanted, 5);
    assert!(!smith_pending(&sim, s, sm::HIT));
    assert_eq!(hp(&sim, monastery), 10_000);
}

#[test]
fn percent_hits_give_experience_only_against_enemies() {
    // C-125: `0x27F` with attacker gives -d experience only for an enemy relation to
    // units or occupied buildings. Occupied = alarm post 1610 with a quartered colonist;
    // counter-checks: unoccupied enemy monastery, neutral colonist (slot 8).
    let rules = tk::spell_rules();
    let mut state = quiet_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let post = tk::spawn(&mut sim, tk::ALARM_POST, 1, (20, 10));
    let guest = tk::spawn(&mut sim, tk::COLONIST, 1, (20, 13));
    assert_eq!(
        crate::module::quarter::accept_unit(&mut sim, post, guest),
        0
    );
    assert!(
        crate::filter::matches_class(&mut sim, post, 0x217),
        "occupied"
    );
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 1, (10, 30));
    let neutral = tk::spawn(&mut sim, tk::COLONIST, 8, (30, 30));
    let strike = |sim: &mut Sim, t: ObjectId| {
        crate::combat::rank::set_rank_reset_experience(sim, s, 0);
        let execute = Message::new(sm::EXECUTE, &[1, 1, t.as_i32(), 0]);
        assert_eq!(send(sim, s, &execute), 0);
        let events = tk::run_ticks_collect(sim, 35);
        assert!(!smove(sim, s).busy);
        let numbers = damage_numbers(&events);
        assert_eq!(numbers.len(), 1);
        assert_eq!(numbers[0].0, t);
        (numbers[0].1, rank_and_fraction(sim, s))
    };
    // Occupied enemy alarm post (HP 1600): -40 % = -640; experience -d (measured).
    assert_eq!(hp(&sim, post), 1600);
    let (d, xp) = strike(&mut sim, post);
    assert_eq!((d, hp(&sim, post)), (-640, 960));
    // `add_scaled_to_current(30, 640, 10)` at Max(30) = 100: fraction 640 * 100,000 / 1000 =
    // 64,000, no rank-up (measured).
    assert_eq!(xp, (0, 64_000), "experience {d}");
    // Unoccupied enemy building and neutral unit: no experience.
    assert_eq!(strike(&mut sim, monastery), (-4000, (0, 0)));
    let (d, xp) = strike(&mut sim, neutral);
    assert!(d < 0);
    assert_eq!(xp, (0, 0), "neutral: no experience (C-125)");
}

#[test]
fn no_counterattack_while_whirling() {
    // K-322 (observation; the only smith test with behavior 2 = default): as with demolition
    // expert and killer the retaliation `0x911` does not reach the switched-off attack module
    // (`GameObject_ApplyModuleActivationChanges` `0x0046CA60`, `GameObject_SendMessage`
    // `0x0046E800`); the hits come as without retaliation.
    let rules = tk::spell_rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = smith(&mut sim, (10, 10), 100);
    let foes = enemies_around(&mut sim, (10, 10), &SECTORS[..3]);
    whirl(&mut sim, s);
    until_busy(&mut sim, s);
    let sabre = tk::spawn(&mut sim, tk::SABRE, 1, (10, 7));
    let sabre_guid = sim.object(sabre).unwrap().guid;
    // N+1: the attack module has been switched off since the end of N.
    tk::run_ticks(&mut sim, 1);
    assert!(!sim.object(s).unwrap().has_module(0x11, false));
    let attack_target = |sim: &Sim| crate::module::attack::attack(sim, s).unwrap().target;
    let before = attack_target(&sim);
    let counter = Message::new(0x911, &[sabre.as_i32()]);
    assert_eq!(send(&mut sim, s, &counter), -1, "retaliation discarded");
    let blow = crate::combat::damage::explicit_damage_message(-10, sabre, sabre_guid);
    send(&mut sim, s, &blow);
    assert_eq!(attack_target(&sim), before, "no retaliation");
    assert_ne!(attack_target(&sim), sabre);
    let hits = smith_hits_in(&mut sim, WHIRL_END - 1);
    assert_eq!(hits, foes, "hits as without retaliation");
    assert!(!smove(&sim, s).busy && attack_on(&sim, s));
}
