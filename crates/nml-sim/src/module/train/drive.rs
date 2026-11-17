//! Driving of the train (re/train-multipart 3.2-3.5): departure, braking, tick with braking and
//! acceleration step, drive step with track look-ahead and leaving the map. Everything is integer
//! (fixed point 20.12, table sine); only the locomotive (GUID 2613) drives, tender and wagon are
//! pushed along rigidly (K-240).

use nml_formats::targets::MatchFilter;

use crate::collision::find_nearest_matching;
use crate::fx::{FxVec3, fixed_mul};
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::Message;
use crate::module::events::update_events;
use crate::object::status as object_status;
use crate::rules::MapInfo;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{set_grid_position, set_object_event_bit};

use super::curve::{brake_distance, time_for_speed, travel_distance};
use super::{INDEX, LOCO, MAX_ACC_TIME, blocking, status, supply, train, train_mut};

/// Group of the train's self messages (`Module_PostDelayedMessage(..., 0, 0x80)`).
const GROUP: u8 = 0x80;
/// Whistle steam every 900 ms (`Train_UpdateWhistleSteam` `0x00530020`).
const WHISTLE_MS: i32 = 900;
/// Whistle steam at the nodes `toeff01`/`toeff02` of the locomotive.
const WHISTLE_EFFECT: &str = "data/effects/lok_toeff.pef";
/// From more than 15 tiles outside, the locomotive leaves the map (`0x00530CE0`, K-244).
const LEAVE_MAP_OVERSHOOT: i32 = 15 << 12;
/// Self-departure: new counter value after an attempt (3.2).
const SELF_DEPART_RETRY_MS: i32 = 1000;

/// Sounds (sound database, flags `0x14`, re/train-multipart 7): moving, departing, braking.
mod sound {
    pub const MOVING: i32 = 11;
    pub const DEPART: i32 = 12;
    pub const BRAKE: i32 = 13;
}

fn is_loco(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_some_and(|o| o.guid == LOCO)
}

/// `SoundManager_PlayObjectSound3D(action, 0x14, world position, GUID, -1, 1)`.
fn play_sound(sim: &mut Sim, id: ObjectId, action: i32) {
    let Some(o) = sim.object(id) else { return };
    let (pos, guid) = (o.world_pos, o.guid);
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action,
        flags: 0x14,
        pos,
        guid,
    });
}

/// `GameObject_SetAnimation(anim, 128.0, 0, 0, 0)` on the locomotive and slaves: all get the
/// animation number of the locomotive (requested, without restart, handover in the next object tick).
fn set_train_animation(sim: &mut Sim, id: ObjectId, anim: i32) {
    let slaves = train(sim, id).map_or_else(Vec::new, |t| t.slaves.clone());
    for o in std::iter::once(id).chain(slaves) {
        if let Some(o) = sim.object_mut(o) {
            o.set_animation(anim, 0, false, false, false);
        }
    }
}

/// `Train_DepartForward` (`0x005311D0`) or `Train_DepartBackward` (`0x00531380`): only the locomotive
/// at standstill (bit 2 clear). The target tile applies only with exactly two arguments (`target`),
/// otherwise it is invalid. Travel vector `+-(cos, sin)` of the **current** direction.
pub fn depart(sim: &mut Sim, id: ObjectId, backward: bool, target: Option<(i32, i32)>) {
    if !is_loco(sim, id) || train(sim, id).is_none_or(|t| t.status & status::MOVING != 0) {
        return;
    }
    let Some(direction) = sim.object(id).map(|o| o.direction.0) else {
        return;
    };
    let Some(t) = train_mut(sim, id) else { return };
    t.target = target;
    let anim = if backward { t.back_anim } else { t.fwd_anim };
    set_train_animation(sim, id, anim);
    let (c, s) = (cos_deg(direction), sin_deg(direction));
    let Some(t) = train_mut(sim, id) else { return };
    t.acc_time = MAX_ACC_TIME;
    t.speed = 0;
    if backward {
        t.status = status::BACKWARD | status::MOVING | status::ACCEL;
        t.dir = (c.wrapping_neg(), s.wrapping_neg());
    } else {
        t.status = status::FORWARD | status::MOVING | status::ACCEL;
        t.dir = (c, s);
    }
    if !t.is_slave {
        whistle(sim, id);
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(object_status::MOVE, true);
    }
    play_sound(sim, id, sound::DEPART);
}

/// `Train_StartBraking` (`0x0052FEE0`): only the locomotive in motion (bit 2; a second call during
/// braking recalculates the curve from the current speed). Standing animation to all, `MOVE` off, sound 13.
/// The brake sparks (`0x921` to the slaves) are presentation.
pub fn start_braking(sim: &mut Sim, id: ObjectId) {
    if !is_loco(sim, id) {
        return;
    }
    let Some(t) = train_mut(sim, id) else { return };
    if t.status & status::MOVING == 0 {
        return;
    }
    let a = time_for_speed(t.speed) / 100 * 100;
    t.acc_time = a;
    t.speed = travel_distance(a - 100, a, 2, 1);
    t.next_speed = travel_distance(a - 200, a - 100, 2, 1);
    t.status = (t.status & !status::ACCEL) | status::MOVING | status::BRAKING;
    let anim = t.stop_anim;
    set_train_animation(sim, id, anim);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(object_status::MOVE, false);
    }
    play_sound(sim, id, sound::BRAKE);
}

/// `Train_UpdateWhistleSteam` (`0x00530020`, query `0x421`): only with bit 2 two whistle steam effects
/// at `toeff01`/`toeff02` and the next `0x421` after 900 ms.
pub fn whistle(sim: &mut Sim, id: ObjectId) {
    if train(sim, id).is_none_or(|t| t.status & status::MOVING == 0) {
        return;
    }
    for node in ["toeff01", "toeff02"] {
        sim.state.events.push(SimEvent::EffectAttached {
            object: id,
            path: WHISTLE_EFFECT.into(),
            node: node.into(),
            offset: [0.0; 3],
        });
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(WHISTLE_MS, Message::new(0x421, &[]), false, GROUP);
    }
}

/// `PropertyTrain_Tick` (`0x00531690`, 3.2): slaves do not tick. Timer, braking and
/// acceleration step, drive step, then the map check of the **unclamped** grid position
/// (`0x0052EFD0`) and the self-departure. `dt` does not enter the curve (K-241).
/// The vehicle fire (`UpdateDamageFires`) is the host's business (the locomotive has no `tzone`).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    if train(sim, id).is_none_or(|t| t.is_slave) {
        return;
    }
    update_events(sim, id, INDEX, dt);
    let st = |sim: &Sim| train(sim, id).map_or(0, |t| t.status);
    if st(sim) & status::BRAKING != 0 {
        brake_step(sim, id);
    }
    if st(sim) & status::ACCEL != 0 {
        accel_step(sim, id);
    }
    if st(sim) & status::MOVING != 0 {
        move_step(sim, id);
        let outside = loco_outside_map(sim, id);
        let Some(t) = train_mut(sim, id) else { return };
        if !outside {
            t.outside = 0;
        } else if t.outside == 0 {
            let forward = t.status & status::FORWARD != 0;
            t.outside = if forward { 2 } else { 1 };
            sim.state.events.push(SimEvent::Deselected { object: id });
            supply::on_left_map(sim, id, forward);
        }
    }
    let Some(t) = train_mut(sim, id) else { return };
    if t.outside != 0 && t.has_supply && t.time_counter != 0 {
        t.time_counter = t.time_counter.wrapping_sub(dt);
        if t.time_counter < 1 {
            depart(sim, id, false, None);
            if let Some(t) = train_mut(sim, id) {
                t.time_counter = SELF_DEPART_RETRY_MS;
            }
        }
    }
}

/// Braking step (`0x0052F960`): `v = Dist(A - 100, A)`, `A -= 100`; below 20 halt with event bit 0
/// (A then stays -100) and, if applicable, unloading of the supply
/// ([`supply::unload_if_due`]).
fn brake_step(sim: &mut Sim, id: ObjectId) {
    let Some(t) = train_mut(sim, id) else { return };
    let a = t.acc_time;
    t.speed = travel_distance(a.wrapping_sub(100), a, 2, 1);
    t.acc_time = a.wrapping_sub(100);
    t.next_speed = travel_distance(a.wrapping_sub(200), a.wrapping_sub(100), 2, 1);
    if t.speed < 20 {
        t.speed = 0;
        t.next_speed = 0;
        t.status = 0;
        set_object_event_bit(sim, id, 0, true);
        supply::unload_if_due(sim, id);
    }
}

/// Acceleration step (`0x0052F3F0`): `v = Dist(2300 - A, 2400 - A, 5, 4)`,
/// `next = Dist(2400 - A, 2500 - A, 5, 4)`, `A -= 100`; at `A < 1` set `next = v` and bit 1 off.
/// The speed of the wheel animation (`RefreshAnimIfCurrent`) is presentation.
fn accel_step(sim: &mut Sim, id: ObjectId) {
    let Some(t) = train_mut(sim, id) else { return };
    let a = t.acc_time;
    t.speed = travel_distance(2300_i32.wrapping_sub(a), 2400_i32.wrapping_sub(a), 5, 4);
    t.next_speed = travel_distance(2400_i32.wrapping_sub(a), 2500_i32.wrapping_sub(a), 5, 4);
    t.acc_time = a.wrapping_sub(100);
    if t.acc_time < 1 {
        t.next_speed = t.speed;
        t.status &= !status::ACCEL;
    }
}

/// Outside check of the locomotive (`0x0052EFD0` -> `0x004F69C0`): the **unclamped** grid position
/// `(grid.x >> 12, grid.z >> 12)` is not in `0 <= x < width`, `0 <= z < height`. The tick
/// (outside status) and the unloading at the end of braking ([`supply::unload_if_due`]) use the same
/// check; it differs from [`grid_pos_in_map`] exactly at `grid == DIM << 12`.
/// A missing object does not count as outside.
pub(crate) fn loco_outside_map(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| !sim.state.path.inside(o.grid.x.0 >> 12, o.grid.z.0 >> 12))
}

/// `GameWorld_IsGridPosInMap` (`0x00477C70`): `0 <= x <= DIM_X << 12`, likewise z (upper bound
/// inclusive).
pub(crate) fn grid_pos_in_map(sim: &Sim, p: (i32, i32)) -> bool {
    let map = sim.rules.map;
    (0..=map.dim_x << 12).contains(&p.0) && (0..=map.dim_z << 12).contains(&p.1)
}

/// Overshoot outside the map (`0x0052EEB0`) in fixed point: `x < 0` -> `-x`, `x > DIM_X << 12` ->
/// `x - (DIM_X << 12)`, otherwise the same for z, otherwise 0. The original calculates in `f32` tiles;
/// `d > 15.0` is exactly `d > 15 << 12` for values below 2^24.
///
/// Public so that the host applies the fade-out (`max(0, 1 - 0.2 * d)`) with the same calculation.
pub fn overshoot(map: MapInfo, p: (i32, i32)) -> i32 {
    let (w, h) = (map.dim_x << 12, map.dim_z << 12);
    if p.0 < 0 {
        p.0.wrapping_neg()
    } else if p.0 > w {
        p.0 - w
    } else if p.1 < 0 {
        p.1.wrapping_neg()
    } else if p.1 > h {
        p.1 - h
    } else {
        0
    }
}

/// Shift the locomotive and slaves by the same offset (`GameObject_SetGridPosition(..., 1)`), the
/// locomotive to `p`, the slaves in list order; with `blocking` the track blocking in between after
/// the drive step (`Train_UpdateTrackBlocking`).
fn shift_train(sim: &mut Sim, id: ObjectId, p: FxVec3, blocking: bool) {
    let Some(old) = sim.object(id).map(|o| o.grid) else {
        return;
    };
    let (dx, dz) = (p.x.0.wrapping_sub(old.x.0), p.z.0.wrapping_sub(old.z.0));
    set_grid_position(sim, id, p, true);
    if blocking {
        blocking::update_track_blocking(sim, id);
    }
    let slaves = train(sim, id).map_or_else(Vec::new, |t| t.slaves.clone());
    for s in slaves {
        let Some(mut g) = sim.object(s).map(|o| o.grid) else {
            continue;
        };
        g.x.0 = g.x.0.wrapping_add(dx);
        g.z.0 = g.z.0.wrapping_add(dz);
        set_grid_position(sim, s, g, true);
    }
}

/// `Train_MoveStep` (`0x00530CE0`, 3.4). Locomotive, not braking: sound 11; target tile within the
/// first `braking distance + 1` tiles -> start of braking instead of a step; look-ahead
/// `braking distance + 4` (backward `+ 12`) tiles for finished tracks of any owner - outside the map
/// nothing is checked or counted further; no track -> start of braking, the step follows with the
/// new speed. Then: more than 15 tiles outside -> `LeaveMap`; otherwise the step `fixed_mul(dir, v)`.
/// Fade-out: host.
fn move_step(sim: &mut Sim, id: ObjectId) {
    let Some(t) = train(sim, id).cloned() else {
        return;
    };
    let Some(grid) = sim.object(id).map(|o| o.grid) else {
        return;
    };
    if !t.is_slave && t.status & status::BRAKING == 0 {
        play_sound(sim, id, sound::MOVING);
        let filter = MatchFilter {
            mode: 0x40,
            match_bits: 0x0200_0403,
            ..MatchFilter::constructed()
        };
        let n = brake_distance(t.speed) >> 12;
        let start = (grid.x.0, grid.z.0);
        let advance = |p: (i32, i32)| (p.0.wrapping_add(t.dir.0), p.1.wrapping_add(t.dir.1));
        if let Some(target) = t.target {
            let mut p = start;
            for _ in 0..n + 1 {
                if (p.0 >> 12, p.1 >> 12) == target {
                    start_braking(sim, id);
                    return;
                }
                p = advance(p);
            }
        }
        let look = n
            + 4
            + if t.status & status::BACKWARD != 0 {
                8
            } else {
                0
            };
        let player = i32::from(id.slot());
        let mut p = start;
        for _ in 0..look {
            if grid_pos_in_map(sim, p) {
                if find_nearest_matching(sim, p, &filter, 0x800, player, -1).is_none() {
                    start_braking(sim, id);
                    break;
                }
                p = advance(p);
            }
        }
    }
    let Some(t) = train(sim, id).cloned() else {
        return;
    };
    if overshoot(sim.rules.map, (grid.x.0, grid.z.0)) > LEAVE_MAP_OVERSHOOT
        && !t.is_slave
        && t.status & status::BRAKING == 0
    {
        leave_map(sim, id);
        return;
    }
    let mut p = grid;
    p.x.0 = p.x.0.wrapping_add(fixed_mul(t.dir.0, t.speed));
    p.z.0 = p.z.0.wrapping_add(fixed_mul(t.dir.1, t.speed));
    shift_train(sim, id, p, true);
}

/// `Train_LeaveMap` (`0x0052FA20`, query `0x721`, K-244): only the locomotive. Two drive steps back
/// (slaves by the same offset), event bit 8 (forward) or 9, halt (`v = 0`, status 0, `MOVE`
/// off, standing animation to all), event bit 0. No unblocking, no removal - the train stands
/// outside the map.
pub fn leave_map(sim: &mut Sim, id: ObjectId) {
    if !is_loco(sim, id) {
        return;
    }
    let Some(t) = train(sim, id).cloned() else {
        return;
    };
    let Some(mut p) = sim.object(id).map(|o| o.grid) else {
        return;
    };
    p.x.0 =
        p.x.0
            .wrapping_add(fixed_mul(t.dir.0, t.speed).wrapping_mul(-2));
    p.z.0 =
        p.z.0
            .wrapping_add(fixed_mul(t.dir.1, t.speed).wrapping_mul(-2));
    shift_train(sim, id, p, false);
    let bit = if t.status & status::FORWARD != 0 {
        8
    } else {
        9
    };
    set_object_event_bit(sim, id, bit, true);
    if let Some(t) = train_mut(sim, id) {
        t.speed = 0;
        t.status = 0;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(object_status::MOVE, false);
    }
    set_train_animation(sim, id, t.stop_anim);
    set_object_event_bit(sim, id, 0, true);
}
