//! Module 19 `CPropertyFightFar` (re/combat-ranged 1): ranges (cached,
//! C-84), ship flag (C-82) and the shot order `0xA13` -> synchronous `{0x10A, target, zone,
//! side}` to the shoot module (10) with the feedback messages of the attack. No tick, never busy.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_fault};
use crate::object::status;
use crate::sim::Sim;

use super::ModuleKind;

pub const INDEX: usize = 0x13;

/// `CPropertyFightFar` (`0x80` bytes); in the savegame as `0x00513D60`. The target of the running
/// `0xA13` (`+0x7C`, not stored) lives only in the call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FightFar {
    /// `m_MinDist` (`+0x70`): Attribut 34 `<< 10`.
    pub min_dist: i32,
    /// `m_MaxDist` (`+0x74`): `min(21, 18) << 10` (sight caps it).
    pub max_dist: i32,
    /// `m_IsShip` (`+0x78`): `200001 & 0x113` (C-82).
    pub is_ship: bool,
}

pub(crate) fn fight_far<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s FightFar> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::FightFar(f) => Some(f),
        _ => None,
    }
}

/// `Init` (`0x00521920` = ReadParams, return value 1).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    read_params(sim, id, desc);
}

/// `PropertyFightFar_ReadParams` (`0x005138D0`): flags 200001, ranges from the current
/// values (= maximum with upgrades) at the time of the call; `FillCurrent(20)` has no effect.
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = desc.value_int(200001).unwrap_or(0) as u32;
    let a = |n| sim.attribute(id, n);
    let (r21, r18) = (a(21).wrapping_shl(10), a(18).wrapping_shl(10));
    let max = if r21 < r18 { a(21) } else { a(18) }.wrapping_shl(10);
    let f = FightFar {
        min_dist: a(34).wrapping_shl(10),
        max_dist: max,
        is_ship: flags & 0x113 != 0,
    };
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        m.kind = ModuleKind::FightFar(Box::new(f));
    }
}

fn dead(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_none_or(|o| o.status & status::DIED != 0)
}

/// `PropertyFightFar_NotifyGhost` (`0x00513980`): with a ghost module (`0x1B`) `0x71B` - end sucking
/// ([`crate::module::ghosts::ghost`], C-3).
fn notify_ghost(sim: &mut Sim, id: ObjectId) {
    if sim.object(id).is_some_and(|o| o.has_module(0x1B, false)) {
        send(sim, id, &Message::new(0x71B, &[]));
    }
}

/// `PropertyFightFar_HandleMessage` (`0x005139E0`, Slot 7).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    if dead(sim, id) && msg.code != 0xC13 {
        return -1;
    }
    let Some(f) = fight_far(sim, id).copied() else {
        return -1;
    };
    match msg.code {
        0xB13 => {
            msg.set_arg(0, f.min_dist);
            msg.set_arg(1, f.max_dist);
            0
        }
        0x113 if msg.args.len() == 1 => send(sim, id, &Message::new(0x150A, &[msg.arg(0)])),
        0x513 => i32::from(f.is_ship),
        0xC13 => {
            notify_ghost(sim, id);
            0
        }
        _ => -1,
    }
}

/// `FixedPos_RelativeAngleDeg` (`0x004EC880`): angle to the target relative to the viewing direction,
/// same position -> 0.
fn relative_angle(sim: &Sim, id: ObjectId, t: ObjectId) -> i32 {
    let (Some(s), Some(o)) = (sim.object(id), sim.object(t)) else {
        return 0;
    };
    let (dx, dz) = (
        o.grid.x.0.wrapping_sub(s.grid.x.0),
        o.grid.z.0.wrapping_sub(s.grid.z.0),
    );
    if (dx, dz) == (0, 0) {
        return 0;
    }
    crate::world::wrap_angle(crate::math::atan2_deg(dz, dx).wrapping_sub(s.direction.0))
}

/// `PropertyFightFar_OnCommand` (`0x00513CD0`, slot 12): dead -> only `0xC13`; only `0xA13` has an effect
/// (take over feedback messages, `StartShot`), other commands count as accepted.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if dead(sim, id) && env.msg.code != 0xC13 {
        return false;
    }
    if env.msg.code != 0xA13 {
        return true;
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_success(env.success.as_ref(), env.success_target);
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
    }
    start_shot(sim, id, ObjectId::from_i32(env.msg.arg(0)))
}

/// `PropertyFightFar_StartShot` (`0x00513AC0`): `FIGHTING` required; target alive; ships choose the
/// side (`relative angle < 0` -> starboard 1, otherwise port 0; otherwise -1); buildings, patterns and
/// towers get a hit zone (`PickHitZoneTowards` from the world position of the shooter), units
/// zone 0. Synchronously `{0x10A, target, zone, side}` to the shoot module (or `{0x31B, target}` to the
/// ghost) with the feedback messages just taken over.
fn start_shot(sim: &mut Sim, id: ObjectId, target: ObjectId) -> bool {
    if sim
        .object(id)
        .is_none_or(|o| o.status & status::FIGHTING == 0)
    {
        log::debug!("Ranged combat {id:?}: Bug?  Ranged combat but HS_FIGHT false!");
        return false;
    }
    let Some(t) = sim.state.object(target, true) else {
        return false;
    };
    let structure =
        t.has_module(3, false) || t.has_module(0x29, false) || t.has_module(0x32, false);
    let Some((success, success_target, fault, fault_target)) = sim.module(id, INDEX).map(|m| {
        (
            m.base.success.clone(),
            m.base.success_target,
            m.base.fault.clone(),
            m.base.fault_target,
        )
    }) else {
        return false;
    };
    let msg = if sim.object(id).is_some_and(|o| o.has_module(0x1B, false)) {
        Message::new(0x31B, &[target.as_i32()])
    } else {
        let ship = fight_far(sim, id).is_some_and(|f| f.is_ship);
        let side = if ship {
            i32::from(relative_angle(sim, id, target) < 0)
        } else {
            -1
        };
        let zone = if structure {
            let from = crate::combat::hit_zone::world_pos(sim, id).unwrap_or([0.0; 3]);
            crate::combat::hit_zone::pick_hit_zone_towards(sim, target, from).map_or(0, |z| z.0)
        } else {
            0
        };
        Message::new(0x10A, &[target.as_i32(), zone, side])
    };
    let env = Envelope {
        msg,
        success,
        fault,
        success_target,
        fault_target,
    };
    dispatch_command(sim, id, &env) != 0
}

/// `PropertyFightFar_Abort` (`0x00513AA0`): notify the ghost, fault message, -> 1.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    notify_ghost(sim, id);
    send_fault(sim, id, INDEX);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::messaging::{send, send_mut};

    // QUIRK C-84: kept
    #[test]
    fn ranges_come_from_34_and_min_of_21_and_18_and_stay_cached() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ARCHER, 0, (5, 5));
        let mut m = Message::new(0xB13, &[-1, -1]);
        assert_eq!(send_mut(&mut sim, a, &mut m), 0);
        assert_eq!(
            (m.arg(0), m.arg(1)),
            (4 << 10, 28 << 10),
            "34 = 4, min(28, 40) = 28"
        );
        // Cached (C-84): a range bonus takes effect only after ReadParams.
        sim.object_mut(a)
            .unwrap()
            .attributes
            .set_modifier(21, 50, true, 77);
        let mut m = Message::new(0xB13, &[-1, -1]);
        send_mut(&mut sim, a, &mut m);
        assert_eq!(m.arg(1), 28 << 10);
        let desc = rules.types[&ARCHER].module(INDEX as u32).unwrap().clone();
        crate::module::read_params(&mut sim, a, INDEX, &desc);
        let mut m = Message::new(0xB13, &[-1, -1]);
        send_mut(&mut sim, a, &mut m);
        assert_eq!(m.arg(1), 40 << 10, "28 * 1.5 = 42 > sight 40");
        assert_eq!(send(&mut sim, a, &Message::new(0x513, &[])), 0);
    }

    // QUIRK C-82: kept
    #[test]
    fn the_ship_flag_is_200001_masked_with_the_mid_0x113() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let s = spawn(&mut sim, WARSHIP, 0, (5, 5));
        assert_eq!(
            send(&mut sim, s, &Message::new(0x513, &[])),
            1,
            "200001 = 0x100 (C-82)"
        );
        // Dead: only 0xC13 answers.
        sim.object_mut(s).unwrap().status |= status::DIED;
        assert_eq!(send(&mut sim, s, &Message::new(0x513, &[])), -1);
        assert_eq!(send(&mut sim, s, &Message::new(0xC13, &[])), 0);
    }

    // QUIRK C-91: kept
    #[test]
    fn a_shot_needs_fighting_and_a_living_target_and_turns_building_zones() {
        let mut rules = rules();
        rules.hit_zones.insert(
            HQ,
            vec![
                [100.0, 0.0, 0.0],
                [-100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0],
                [0.0, 0.0, -100.0],
            ],
        );
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ARCHER, 0, (5, 5));
        let w = spawn(&mut sim, WARRIOR, 1, (8, 5));
        let hq = spawn(&mut sim, HQ, 1, (12, 12));
        let order = |t: ObjectId| Envelope {
            msg: Message::new(0xA13, &[t.as_i32()]),
            success: Some(Message::new(0x511, &[])),
            fault: Some(Message::new(0x411, &[])),
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        let counter = |sim: &Sim| sim.object(hq).unwrap().hit_zone_counter;
        // Without FIGHTING: "Bug? Ranged combat but HS_FIGHT false!" -> not accepted, and the zone of the
        // building target is not even chosen (observable on the counter).
        assert!(!on_message(&mut sim, a, &order(w)));
        assert!(!on_message(&mut sim, a, &order(hq)));
        assert_eq!(counter(&sim), 0, "without FIGHTING no zone choice");
        sim.object_mut(a).unwrap().status |= status::FIGHTING;
        // With module 10 the archer accepts the shot (return value of `dispatch_command`).
        assert!(on_message(&mut sim, a, &order(w)));
        // Building target: the zone is chosen, the counter of the target advances.
        assert!(on_message(&mut sim, a, &order(hq)));
        assert_eq!(counter(&sim), 1, "h = 2: counter 0 -> 1");
        // Unit target: zone 0 without a choice, the counter of the warrior stays.
        assert_eq!(sim.object(w).unwrap().hit_zone_counter, 0);
        // Dead target: not accepted.
        sim.object_mut(w).unwrap().status |= status::DIED;
        assert!(!on_message(&mut sim, a, &order(w)));
        // Dead building target: no zone choice, the counter stays (the alive check is observable).
        sim.object_mut(hq).unwrap().status |= status::DIED;
        on_message(&mut sim, a, &order(hq));
        assert_eq!(counter(&sim), 1, "dead target: counter unchanged");
        // Other commands: accepted, without effect.
        assert!(on_message(
            &mut sim,
            a,
            &Envelope::bare(Message::new(0x713, &[]))
        ));
    }

    // QUIRK C-94: kept
    #[test]
    fn ships_aim_with_the_side_of_the_target() {
        // Ship side in `fight_far` (re/ships 9.1): ship at (81920, 65536)
        // (test world deviation: re/ships 9.1 gives (40960, 40960), which is land or a mixed tile here; the
        // geometry is the same), direction 0, target 40960 further south (z + 40960) -> relative angle
        // +90 degrees -> port (`backcannonNN`, smoke rotation +pi/2); target 40960 north -> -90 degrees ->
        // starboard (`steuercannonNN`, -pi/2).
        // The target is abeam: the ship does not turn. The side is not set in the test but
        // is read from the launches. The feedback messages of the order go to the
        // shoot module and from there with `Finish` to the sender.
        use crate::module::boarding::testkit as ships;
        let half_pi = std::f32::consts::FRAC_PI_2;
        let rules = ships::rules();
        for (z, prefix, yaw) in [
            (106496, "backcannon", half_pi),
            (24576, "steuercannon", -half_pi),
        ] {
            let mut state = ships::shore_state(&rules);
            crate::fog::reveal_map(&mut state, -1);
            let mut sim = Sim::new(&mut state, &rules);
            let ship = ships::spawn(&mut sim, ships::WARSHIP, 0, (20, 16));
            let target = ships::spawn(&mut sim, ships::COLONIST, 1, (5, 5));
            for (id, pos) in [(ship, (81920, 65536)), (target, (81920, z))] {
                crate::world::set_grid_position(
                    &mut sim,
                    id,
                    crate::fx::FxVec3::new(
                        crate::fx::Fx(pos.0),
                        crate::fx::Fx(0),
                        crate::fx::Fx(pos.1),
                    ),
                    true,
                );
            }
            sim.object_mut(ship).unwrap().direction = crate::fx::Fx(0);
            send(
                &mut sim,
                ship,
                &crate::module::quarter::attack_mode_msg(0, false),
            );
            sim.object_mut(ship).unwrap().status |= status::FIGHTING;
            assert_eq!(relative_angle(&sim, ship, target).abs(), 90 << 12);
            let done = Message::new(0x13B, &[]);
            let order = Envelope {
                msg: Message::new(0xA13, &[target.as_i32()]),
                success: Some(done.clone()),
                fault: Some(Message::new(0x23B, &[])),
                success_target: ObjectId::NONE,
                fault_target: ObjectId::NONE,
            };
            assert!(on_message(&mut sim, ship, &order));
            // The feedback messages of the attack are with the shoot module (not with FightFar).
            let shoot = sim.module(ship, crate::module::shoot::INDEX).unwrap();
            assert_eq!(shoot.base.success, Some(done));
            assert_eq!(shoot.base.fault, Some(Message::new(0x23B, &[])));
            let mut seen = Vec::new();
            for _ in 0..3 {
                crate::tick::object_tick(&mut sim, ship, 100);
                for e in std::mem::take(&mut sim.state.events) {
                    if let crate::state::SimEvent::ProjectileLaunched {
                        node, yaw_extra, ..
                    } = e
                    {
                        seen.push((node, yaw_extra));
                    }
                }
            }
            assert_eq!(
                seen,
                vec![(format!("{prefix}02"), yaw), (format!("{prefix}01"), yaw),],
                "target at z = {z}"
            );
            assert_eq!(sim.object(ship).unwrap().direction.0, 0, "no rotation");
        }
    }
}
