//! Broadside of the warships (shoot module 10 with `SHIP`; re/ships 2.3, 3.6, 4.4, 6, 9.1, re/combat-ranged 3.6,
//! re/combat-projectiles 4): `AimShip` (`0x005251E0`) creates for each cannon of a side a timer at the animation marker
//! `i + 2`, `FireShipBroadside` (`0x00527020`) fires it. The final message (index cannon count + 1) ends the shot.

use crate::combat::hit_zone::{pick_hit_zone_towards, world_pos};
use crate::combat::projectile::{BallLaunch, launch_ball};
use crate::id::ObjectId;
use crate::message::Message;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::shoot::{
    self, GROUP, GROUP_KEEP, finish, label, post, set_animation, shoot_mut, vision,
};

/// Animation of the chosen side: starboard `110002[0]`, port `110003[0]` (`+0xAC`/`+0xA0`, re/ships 3.6). At the same
/// time the key of the node table `Rules::cannon_nodes` (the side decides, not the animation currently set - after a
/// side change (C-213) the ship still carries the animation of the old side).
fn side_anim(s: &shoot::Shoot) -> i32 {
    let list = if s.is_steuerboard {
        &s.shoot_anim
    } else {
        &s.present_anim
    };
    list.first().copied().unwrap_or(0)
}

/// Number of consecutively present cannon nodes `…cannon01…` of the side at animation `anim`
/// (`ModelInstance_GetHelperNodePosition` up to the first missing one, re/ships 4.4).
fn cannon_count(sim: &Sim, guid: u32, anim: i32, starboard: bool) -> i32 {
    sim.rules
        .cannon_nodes
        .get(&(guid, anim, starboard))
        .map_or(0, |n| n.len() as i32)
}

/// `PropertyShootProjectile_AimShip` (`0x005251E0`, re/ships 3.6): side from `args[2]` - **before** the `CanShoot`
/// check, so a targeting order during a running broadside changes side and target (C-213). Locked: return without
/// `SendDone` (C-94, the order waits without a shot for `SHIPDELAY`). Otherwise animation of the side, counter 1, for
/// each present node `i` a `0xD0A {i}` at marker `i + 2` (at least 100 ms), the final message `0xD0A {n + 1}` at marker
/// 1 and `SHIPDELAY` (`0xE0A`) after `ShootDelay` in group `0x81`; busy, locked, vision source 4.
pub fn aim_ship(sim: &mut Sim, id: ObjectId, msg: &Message) {
    let Some(s) = shoot_mut(sim, id) else {
        return;
    };
    s.is_steuerboard = msg.arg(2) != 0;
    if !s.can_shoot {
        return;
    }
    let (starboard, delay, target) = (s.is_steuerboard, s.shoot_delay, s.target);
    let anim = side_anim(s);
    // The animation comes before the node search (the nodes hang on the sub-objects of the animation).
    set_animation(sim, id, anim);
    let guid = sim.object(id).map_or(0, |o| o.guid);
    // The number of cannons is not limited (original: up to the first missing node); the shipped data has at most 6 per
    // side.
    let n = cannon_count(sim, guid, anim, starboard);
    for i in 1..=n {
        let d = label(sim, id, anim, i + 2).max(100);
        post(sim, id, d, Message::new(0xD0A, &[i]), GROUP);
    }
    let end = label(sim, id, anim, 1).max(100);
    post(sim, id, end, Message::new(0xD0A, &[n + 1]), GROUP);
    post(sim, id, delay, Message::new(0xE0A, &[]), GROUP_KEEP);
    if let Some(s) = shoot_mut(sim, id) {
        s.cannon_counter = n + 1;
        s.is_busy = true;
        s.can_shoot = false;
    }
    vision(sim, id, 4, target);
}

/// Muzzle of cannon `i` (from 1) of the side: world position of the ship + rest position of the node, rotated with the
/// simulation yaw ([`crate::math::rotate_y_by_direction`]). The original takes the animated node position and the
/// rendering yaw `+0x158` (view-dependent); the rest position with the simulation yaw is a deviation from the original
/// (re/ships 6, C-216). `None`: no node `i` on the side.
pub fn muzzle_world(
    sim: &Sim,
    id: ObjectId,
    anim: i32,
    starboard: bool,
    i: i32,
) -> Option<[f32; 3]> {
    let o = sim.object(id)?;
    let local = sim
        .rules
        .cannon_nodes
        .get(&(o.guid, anim, starboard))?
        .get(usize::try_from(i.checked_sub(1)?).ok()?)?;
    let w = world_pos(sim, id)?;
    let r = crate::math::rotate_y_by_direction(*local, o.direction);
    Some([r[0] + w[0], r[1] + w[1], r[2] + w[2]])
}

/// `PropertyShootProjectile_FireShipBroadside` (`0x00527020`, re/ships 3.6): `i < cannon counter`, target alive
/// (`GetObjectById(…, without removed)`) **and** node `i` of the current side -> ball; otherwise (also without a
/// target, i.e. at the final message) not busy, if the shooter is alive the standing animation `110004[0]`, then
/// `Finish`. Target point of a building or a unit of class `0x20000`: hit zone from the muzzle
/// ([`pick_hit_zone_towards`]), otherwise the target position with `y = max(terrain, water)`. Sound 1/`0x22` at the
/// muzzle; damage `−attribute 20`; muzzle smoke rotated by `+π/2` (port) or `−π/2` (starboard). Return value 0 as in
/// the original (`HandleMessage` `0x005277F0`).
pub fn fire_ship_broadside(sim: &mut Sim, id: ObjectId, i: i32) -> i32 {
    let Some(s) = shoot::shoot(sim, id).cloned() else {
        return 0;
    };
    let starboard = s.is_steuerboard;
    let alive = sim.state.object(s.target, true).is_some();
    let muzzle = if i < s.cannon_counter && alive {
        muzzle_world(sim, id, side_anim(&s), starboard, i)
    } else {
        None
    };
    let Some(start) = muzzle else {
        if let Some(x) = shoot_mut(sim, id) {
            x.is_busy = false;
        }
        if sim.object(id).is_some_and(|o| o.status & status::DIED == 0) {
            // `GameObject_SetAnimation(anim, 128.0, 0, 0, 0)` (`0x005273A2`): without restart.
            let a = s.reload_anim.first().copied().unwrap_or(0);
            if let Some(o) = sim.object_mut(id) {
                o.set_animation(a, 0, false, false, false);
            }
        }
        finish(sim, id);
        return 0;
    };
    let target = s.target;
    let building = sim.object(target).is_some_and(|o| o.has_module(3, false))
        || crate::filter::matches_class(sim, target, 0x2_0000);
    let own = world_pos(sim, target).unwrap_or(start);
    let target_pos = if building {
        pick_hit_zone_towards(sim, target, start).map_or(own, |z| z.1)
    } else {
        let ground = crate::height::terrain_height(sim.rules, own[0], own[1], own[2]);
        [own[0], ground.max(sim.rules.map.water_level), own[2]]
    };
    let guid = sim.object(id).map_or(0, |o| o.guid);
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: 1,
        flags: 0x22,
        pos: start,
        guid,
    });
    let amount = sim.attribute(id, 20).wrapping_neg();
    let damage = crate::combat::damage::damage_message(sim, id, amount);
    launch_ball(
        sim,
        BallLaunch {
            shooter: id,
            target,
            damage,
            start,
            target_pos,
            node: crate::rules::cannon_node_name(starboard, i),
            yaw_extra: if starboard {
                -std::f32::consts::FRAC_PI_2
            } else {
                std::f32::consts::FRAC_PI_2
            },
        },
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Fx, FxVec3};
    use crate::message::Envelope;
    use crate::module::boarding::testkit::{
        COLONIST, ENG_WARSHIP, HOUSE, WARSHIP, rules, shore_state, spawn,
    };
    use crate::module::shoot::{self, on_message};
    use crate::rules::Rules;
    use crate::state::{SimEvent, SimState};
    use crate::tick::object_tick;

    /// Done message of the targeting order: module `0x3B` is missing on the ship, the message fizzles - its consumption
    /// (`success == None`) shows that `Finish` ran.
    const DONE: u32 = 0x13B;
    const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
    /// Ship in deep water; targets 10 tiles north (z smaller, starboard) or south (port) at direction 0 (re/ships
    /// 9.1: the ship is at (40960, 40960) there).
    const SX: i32 = 81920;
    const SZ: i32 = 65536;
    const NORTH: i32 = SZ - 40960;
    const SOUTH: i32 = SZ + 40960;

    fn world(rules: &Rules) -> SimState {
        let mut state = shore_state(rules);
        crate::fog::reveal_map(&mut state, -1);
        state
    }

    fn put(sim: &mut Sim, id: ObjectId, x: i32, z: i32) {
        crate::world::set_grid_position(sim, id, FxVec3::new(Fx(x), Fx(0), Fx(z)), true);
    }

    /// Ship (slot 0) at (SX, SZ), direction 0, without auto attack (stance 0).
    fn ship(sim: &mut Sim, guid: u32) -> ObjectId {
        let id = spawn(sim, guid, 0, (20, 16));
        put(sim, id, SX, SZ);
        sim.object_mut(id).unwrap().direction = Fx(0);
        crate::messaging::send(sim, id, &crate::module::quarter::attack_mode_msg(0, false));
        id
    }

    /// Colonist (slot 1) at the grid position `(x, z)`.
    fn target(sim: &mut Sim, x: i32, z: i32) -> ObjectId {
        let id = spawn(sim, COLONIST, 1, (5, 5));
        put(sim, id, x, z);
        id
    }

    /// Targeting order `0x10A {target, −1, side}` as from FightFar (side 1 = starboard) with a done message.
    fn aim(sim: &mut Sim, ship: ObjectId, target: ObjectId, side: i32) {
        let env = Envelope {
            msg: Message::new(0x10A, &[target.as_i32(), -1, side]),
            success: Some(Message::new(DONE, &[])),
            fault: None,
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        assert!(on_message(sim, ship, &env), "0x10A is accepted");
    }

    /// One object tick of the ship (100 ms); the events of the tick.
    fn tick(sim: &mut Sim, ship: ObjectId) -> Vec<SimEvent> {
        object_tick(sim, ship, 100);
        std::mem::take(&mut sim.state.events)
    }

    /// (Node, extra yaw, start, target, target point) of a launch.
    type Launch = (String, f32, [f32; 3], ObjectId, [f32; 3]);
    /// (`is_busy`, `can_shoot`, done message consumed, wanted animation) after a tick.
    type Frame = (bool, bool, bool, i32);
    /// Ball names per tick, state per tick, cannon counter after aiming.
    type Timeline = (Vec<Vec<String>>, Vec<Frame>, i32);

    /// (Node, extra yaw, start, target) of the launches of a tick in message order.
    fn launches(events: &[SimEvent]) -> Vec<Launch> {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::ProjectileLaunched {
                    node,
                    yaw_extra,
                    start,
                    target,
                    target_pos,
                    ..
                } => Some((node.clone(), *yaw_extra, *start, *target, *target_pos)),
                _ => None,
            })
            .collect()
    }

    fn names(events: &[SimEvent]) -> Vec<String> {
        launches(events).into_iter().map(|l| l.0).collect()
    }

    fn state_of(sim: &Sim, ship: ObjectId) -> (bool, bool, i32, i32) {
        let s = shoot::shoot(sim, ship).unwrap();
        (
            s.is_busy,
            s.can_shoot,
            s.cannon_counter,
            sim.object(ship).unwrap().anim.wanted,
        )
    }

    fn finished(sim: &Sim, ship: ObjectId) -> bool {
        sim.module(ship, shoot::INDEX)
            .unwrap()
            .base
            .success
            .is_none()
    }

    fn arm(sim: &mut Sim, ship: ObjectId) {
        sim.module_mut(ship, shoot::INDEX)
            .unwrap()
            .base
            .set_success(Some(&Message::new(DONE, &[])), ObjectId::NONE);
    }

    /// Broadside of `guid` (starboard) against a colonist: ball names per tick 0...`ticks`, plus `is_busy`, `can_shoot`
    /// and done message after each tick.
    fn timeline(guid: u32, ticks: usize) -> Timeline {
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, guid);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        let counter = state_of(&sim, s).2;
        let (mut balls, mut flags) = (Vec::new(), Vec::new());
        for _ in 0..ticks {
            let ev = tick(&mut sim, s);
            balls.push(names(&ev));
            let (busy, can, _, anim) = state_of(&sim, s);
            flags.push((busy, can, finished(&sim, s), anim));
        }
        (balls, flags, counter)
    }

    fn at(balls: &[Vec<String>], tick: usize) -> Vec<&str> {
        balls[tick].iter().map(String::as_str).collect()
    }

    // QUIRK C-94, C-215: kept
    #[test]
    fn spa_warship_fires_six_balls_at_marks_i_plus_2() {
        // re/ships 9.1: balls in ticks 2 (02, then 01), 4 (03), 6 (04), 7 (05), 9 (06); finish (standing animation
        // 110004, Finish) in tick 21; can_shoot true again in tick 61; is_busy from tick 0 to 21.
        let (balls, flags, counter) = timeline(WARSHIP, 70);
        assert_eq!(counter, 7, "cannon count + 1");
        for (t, b) in balls.iter().enumerate() {
            let want: Vec<&str> = match t {
                2 => vec!["steuercannon02", "steuercannon01"],
                4 => vec!["steuercannon03"],
                6 => vec!["steuercannon04"],
                7 => vec!["steuercannon05"],
                9 => vec!["steuercannon06"],
                _ => vec![],
            };
            assert_eq!(at(&balls, t), want, "tick {t}: {b:?}");
        }
        for (t, &(busy, can, done, anim)) in flags.iter().enumerate() {
            assert_eq!(busy, t < 21, "is_busy after tick {t}");
            assert_eq!(can, t >= 61, "can_shoot after tick {t}");
            // Done message: finish (tick 21); SHIPDELAY (tick 61) then has nothing left to report.
            assert_eq!(done, t >= 21, "done message after tick {t}");
            assert_eq!(anim, if t < 21 { 2 } else { 0 }, "animation after tick {t}");
        }
    }

    // QUIRK C-94, C-215: kept
    #[test]
    fn eng_warship_fires_four_balls() {
        // 2010: ticks 2 (02, 01), 4, 6; finish 21.
        let (balls, flags, counter) = timeline(ENG_WARSHIP, 25);
        assert_eq!(counter, 5);
        for (t, frame) in flags.iter().enumerate() {
            let want: Vec<&str> = match t {
                2 => vec!["steuercannon02", "steuercannon01"],
                4 => vec!["steuercannon03"],
                6 => vec!["steuercannon04"],
                _ => vec![],
            };
            assert_eq!(at(&balls, t), want, "tick {t}");
            assert_eq!(frame.0, t < 21, "is_busy after tick {t}");
        }
    }

    // QUIRK C-216: deviation
    #[test]
    fn balls_start_at_the_rest_muzzle_turned_by_the_ship() {
        // BallLaunch.start = world position + rotate_y_by_direction(node, direction); damage -attribute 20 (50 per
        // ball); sound 1 with flags 0x22 per ball; target point of a land target on terrain height, of a water target
        // on water height.
        let rules = rules();
        for (dir, water) in [(0, true), (90, false)] {
            let mut state = world(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let s = ship(&mut sim, WARSHIP);
            sim.object_mut(s).unwrap().direction = Fx(dir << 12);
            // Water target (tile 14, terrain -16) or land target (tile 4, terrain 16).
            let t = target(&mut sim, if water { 60000 } else { 20000 }, SOUTH);
            aim(&mut sim, s, t, 1);
            let ship_pos = crate::combat::hit_zone::world_pos(&sim, s).unwrap();
            let tpos = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
            let mut got = Vec::new();
            for _ in 0..12 {
                let ev = tick(&mut sim, s);
                let sounds: Vec<[f32; 3]> = ev
                    .iter()
                    .filter_map(|e| match e {
                        SimEvent::Sound3d {
                            action: 1,
                            flags: 0x22,
                            pos,
                            ..
                        } => Some(*pos),
                        _ => None,
                    })
                    .collect();
                let l = launches(&ev);
                assert_eq!(sounds, l.iter().map(|x| x.2).collect::<Vec<_>>());
                got.extend(l);
            }
            assert_eq!(got.len(), 6);
            for (n, (node, yaw, start, tgt, tp)) in got.iter().enumerate() {
                // Order of the messages: 02, 01, 03, 04, 05, 06.
                let k = [2, 1, 3, 4, 5, 6][n];
                assert_eq!(node, &format!("steuercannon{k:02}"));
                assert_eq!(*yaw, -HALF_PI);
                assert_eq!(*tgt, t);
                let local = [-40.0 + 16.0 * (k - 1) as f32, 20.0, 30.0];
                // Direction 0: theta = 90 degrees -> (x, z) -> (-z, x);
                // direction 90 degrees: theta = 180 degrees -> (-x, -z).
                let rot = if dir == 0 {
                    [-local[2], local[1], local[0]]
                } else {
                    [-local[0], local[1], -local[2]]
                };
                assert_eq!(
                    rot,
                    crate::math::rotate_y_by_direction(local, Fx(dir << 12))
                );
                assert_eq!(
                    *start,
                    [
                        ship_pos[0] + rot[0],
                        ship_pos[1] + rot[1],
                        ship_pos[2] + rot[2]
                    ],
                    "muzzle of cannon {k}"
                );
                assert_eq!(*tp, [tpos[0], if water { 0.0 } else { 16.0 }, tpos[2]]);
            }
            let dmg: Vec<i32> = sim
                .state
                .projectiles
                .list
                .iter()
                .map(|p| p.damage.arg(0))
                .collect();
            assert_eq!(dmg, vec![-50; 6], "attribute 20 per ball");
        }
    }

    // QUIRK C-213: kept
    #[test]
    fn a_new_target_takes_the_remaining_cannons_from_its_side() {
        // C-213: after ball 03 a new 0x10A with a target on the other side while can_shoot == false -> is_steuerboard
        // and target change; the remaining 0xD0A fire from the nodes of the new side at the new target.
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let a = target(&mut sim, SX, SOUTH);
        let b = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, a, 0);
        let mut got = Vec::new();
        for t in 0..22 {
            if t == 4 {
                // Between ticks 3 and 4: ball 03 is still pending, can_shoot is false.
                assert!(!state_of(&sim, s).1);
                aim(&mut sim, s, b, 1);
                let sh = shoot::shoot(&sim, s).unwrap();
                assert!(sh.is_steuerboard && sh.target == b);
                assert_eq!(sh.cannon_counter, 7, "no new timers, counter stays");
            }
            got.extend(launches(&tick(&mut sim, s)));
        }
        let seen: Vec<(String, f32, ObjectId)> = got.into_iter().map(|l| (l.0, l.1, l.3)).collect();
        let back = |k: i32| (format!("backcannon{k:02}"), HALF_PI, a);
        let star = |k: i32| (format!("steuercannon{k:02}"), -HALF_PI, b);
        assert_eq!(
            seen,
            vec![back(2), back(1), star(3), star(4), star(5), star(6)]
        );
    }

    // QUIRK C-213: kept
    #[test]
    fn a_vanished_target_finishes_on_every_pending_cannon() {
        // Remove the target after ball 01 -> every further 0xD0A: no ball, is_busy false, standing animation, Finish.
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        for _ in 0..3 {
            tick(&mut sim, s);
        }
        crate::world::destroy_object(&mut sim, t);
        let mut starts_after_first = None;
        for n in 3..=21 {
            // Before each tick a fresh done message: only a Finish consumes it.
            arm(&mut sim, s);
            let ev = tick(&mut sim, s);
            assert!(
                launches(&ev).is_empty(),
                "tick {n}: no ball without a target"
            );
            let pending = matches!(n, 4 | 6 | 7 | 9 | 21);
            assert_eq!(
                finished(&sim, s),
                pending,
                "Finish only at the markers, tick {n}"
            );
            let (busy, _, _, anim) = state_of(&sim, s);
            if n >= 4 {
                assert!(!busy && anim == 0, "tick {n}: standing animation, not busy");
                // `SetAnimation(+0xB8[0], 128, 0)` without restart (`0x005273A2`): only the first pending `0xD0A`
                // changes the animation, the others do not restart it.
                let starts = sim.object(s).unwrap().anim.starts;
                assert_eq!(
                    *starts_after_first.get_or_insert(starts),
                    starts,
                    "tick {n}: standing animation not restarted"
                );
            }
        }
    }

    #[test]
    fn a_target_dying_between_two_cannons_ends_the_broadside() {
        // Target dies (DIED, object still present) after ball 01 -> the next 0xD0A does not fire
        // (`GetObjectById(…, without removed)`) but ends the shot with Finish.
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        let mut total = 0;
        for n in 0..=4 {
            if n == 3 {
                sim.object_mut(t).unwrap().status |= crate::object::status::DIED;
            }
            total += launches(&tick(&mut sim, s)).len();
        }
        assert_eq!(total, 2, "only the balls of tick 2");
        let (busy, _, _, anim) = state_of(&sim, s);
        assert!(!busy && anim == 0 && finished(&sim, s), "Finish in tick 4");
    }

    // QUIRK C-214: kept
    #[test]
    fn a_busy_ship_defers_new_orders() {
        // C-214: move command to the ship in tick 3 stays in the FIFO until the final message (tick 21).
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        let fifo = |sim: &Sim| sim.object(s).unwrap().fifo.len();
        for n in 0..=24 {
            if n == 3 {
                crate::command::execute(
                    &mut sim,
                    &crate::command::Command {
                        player: 0,
                        kind: crate::command::CommandKind::Move {
                            units: vec![s],
                            x: (20 << 12) + 0x800,
                            z: (20 << 12) + 0x800,
                            target: -1,
                            formation: 0,
                            direction: crate::motion::NO_DIRECTION,
                            spread: crate::command::NO_SPREAD,
                            flags: 0,
                        },
                    },
                    true,
                );
                assert!(fifo(&sim) > 0, "the command is in the FIFO");
            }
            tick(&mut sim, s);
            if (3..=21).contains(&n) {
                assert!(fifo(&sim) > 0, "tick {n}: ship busy, FIFO waits");
            } else if n >= 22 {
                assert_eq!(fifo(&sim), 0, "tick {n}: processed after the final message");
            }
        }
    }

    // QUIRK C-86: kept
    #[test]
    fn an_aborted_broadside_keeps_firing() {
        // C-86 (ship part): abort in tick 3 clears no timers; balls 03...06 are still fired.
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        let mut per_tick = Vec::new();
        for n in 0..65 {
            per_tick.push(names(&tick(&mut sim, s)));
            if n == 3 {
                assert_eq!(
                    crate::messaging::send(&mut sim, s, &Message::new(0xF0A, &[])),
                    1,
                    "abort accepted"
                );
                assert!(!state_of(&sim, s).0, "abort: no longer busy");
            }
        }
        for (n, expect) in [(4, "03"), (6, "04"), (7, "05"), (9, "06")] {
            assert_eq!(
                per_tick[n],
                vec![format!("steuercannon{expect}")],
                "tick {n}"
            );
        }
        let (busy, can, _, anim) = state_of(&sim, s);
        assert!(
            !busy && can && anim == 0,
            "the final message and SHIPDELAY keep running"
        );
    }

    // QUIRK C-86: kept
    #[test]
    fn a_sunk_shooter_fires_no_more() {
        // Ship dies in tick 3 (DIED) -> pending 0xD0A return -1, no balls.
        let rules = rules();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        let mut total = 0;
        for n in 0..30 {
            total += launches(&tick(&mut sim, s)).len();
            if n == 3 {
                sim.object_mut(s).unwrap().status |= crate::object::status::DIED;
            }
        }
        assert_eq!(
            total, 2,
            "only 02 and 01 (tick 2) were fired before the sinking"
        );
        assert_eq!(
            crate::messaging::send(&mut sim, s, &Message::new(0xD0A, &[4])),
            -1
        );
    }

    // QUIRK C-216: deviation
    #[test]
    fn broadsides_at_buildings_pick_the_zone_from_the_rest_muzzle() {
        // C-216: target HOUSE; per ball zone = pick_hit_zone_towards(start of the cannon).
        let rules = rules();
        let scene = |rules: &Rules| {
            let mut state = world(rules);
            let h = spawn(&mut Sim::new(&mut state, rules), HOUSE, 1, (5, 20));
            (state, h)
        };
        let (mut state, h) = scene(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        aim(&mut sim, s, h, 0);
        let mut got = Vec::new();
        for _ in 0..12 {
            got.extend(launches(&tick(&mut sim, s)));
        }
        assert_eq!(got.len(), 6);
        assert_eq!(
            sim.object(h).unwrap().hit_zone_counter,
            0,
            "6 picks over 2 zones: the counter is back at 0"
        );
        // Counter-check in a fresh world: the same muzzles one after another into the same pick.
        let (mut state2, h2) = scene(&rules);
        let mut sim2 = Sim::new(&mut state2, &rules);
        for (n, (node, _, start, _, tp)) in got.iter().enumerate() {
            let (_, pos) = crate::combat::hit_zone::pick_hit_zone_towards(&mut sim2, h2, *start)
                .expect("zone");
            assert_eq!(*tp, pos, "ball {n} ({node}): zone picked from the muzzle");
        }
        let distinct: std::collections::BTreeSet<[u32; 3]> =
            got.iter().map(|g| g.4.map(f32::to_bits)).collect();
        assert_eq!(distinct.len(), 2, "the zones alternate in turn (C-91)");
    }

    // QUIRK C-94: kept
    #[test]
    fn a_ship_without_cannon_nodes_only_waits_for_the_final_mark() {
        // Empty node table: counter 1, no ball, final message at marker 1, SHIPDELAY as usual.
        let mut rules = rules();
        rules.cannon_nodes.clear();
        let mut state = world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = ship(&mut sim, WARSHIP);
        let t = target(&mut sim, SX, NORTH);
        aim(&mut sim, s, t, 1);
        assert_eq!(state_of(&sim, s).2, 1);
        for n in 0..65 {
            assert!(launches(&tick(&mut sim, s)).is_empty());
            let (busy, can, _, _) = state_of(&sim, s);
            assert_eq!(busy, n < 21, "tick {n}");
            assert_eq!(can, n >= 61, "tick {n}");
        }
    }
}
