//! Accepting and reaching goals (re/walk 2, 4.8, 4.9, 16): FINDTO with a tile path, PATH with
//! waypoints, halting, goal radius and facing direction, CHECKPOS, returning home and the
//! message builders.

use crate::collision::footprint_terrain_cost;
use crate::fx::fixed_hypot;
use crate::group::NO_GROUP;
use crate::id::ObjectId;
use crate::math::atan2_deg;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_mut, send_success};
use crate::motion::NO_DIRECTION;
use crate::motion::step::COST_BLOCKED;
use crate::object::status;
use crate::path::highpath::plan_high_path;
use crate::path::{LAYER_MASKS, layer_from_move_mask};
use crate::sim::Sim;
use crate::world::wrap_angle;

use super::project::{clear_trajectory, project_trajectory};
use super::{
    INDEX, REEVALUATE, WAITING, abort, direction, position, set_state, set_status, walk, walk_mut,
};

/// Last argument of a FINDTO message (flags, `Message_GetLastArg` `0x004E5200`).
pub(crate) fn last_arg(msg: &Message) -> i32 {
    msg.args.last().copied().unwrap_or(0)
}

/// Tolerance (`FindToMsg_GetTolerance` `0x004E5250`): second-to-last argument.
fn tolerance_arg(msg: &Message) -> i32 {
    let n = msg.args.len();
    if n >= 2 { msg.args[n - 2] } else { 0 }
}

/// `FindToMsg_Construct` (`0x004E4F00`): `[x, z, radius, direction, tolerance, flags]`.
pub fn find_to_msg(
    x: i32,
    z: i32,
    radius: i32,
    dir: i32,
    tol: i32,
    run: bool,
    flags: i32,
) -> Message {
    let code = if run { 0x100 } else { 0x200 };
    Message::new(code, &[x, z, radius, dir, tol, flags])
}

/// `PropertyWalk_GetGoalRadius` (`0x00533850`): below half the cruising speed the hard radius,
/// at least a quarter tile.
pub fn goal_radius(sim: &Sim, id: ObjectId, r: i32) -> i32 {
    let cruising = walk(sim, id).map_or(0, |w| w.speed.cruising);
    if r < cruising / 2 {
        sim.object(id).map_or(0x400, |o| o.hard_radius.max(0x400))
    } else {
        r
    }
}

/// `PropertyWalk_SetFacingTolerance` (`0x005336D0`): 0..180 degrees, otherwise 5 degrees.
pub(crate) fn set_facing_tolerance(sim: &mut Sim, id: ObjectId, t: i32) {
    if let Some(w) = walk_mut(sim, id) {
        w.tolerance = if (0..=180).contains(&t) {
            t << 12
        } else {
            5 << 12
        };
    }
}

/// `PropertyWalk_IsFacingTargetDir` (`0x005335C0`); always true when quartered.
pub(crate) fn is_facing_target_dir(sim: &Sim, id: ObjectId, dir: i32) -> bool {
    if sim
        .object(id)
        .is_some_and(|o| o.status & status::QUARTERED != 0)
    {
        return true;
    }
    walk(sim, id)
        .is_some_and(|w| wrap_angle(w.target_dir.wrapping_sub(dir)).wrapping_abs() <= w.tolerance)
}

/// `FixedPos_FacingDirWithOffset` (`0x004EC8C0`): direction to the point plus or minus the offset, whichever is
/// closer to the own one (tie: minus).
pub fn facing_dir_with_offset(p: (i32, i32, i32), q: (i32, i32), off: i32) -> i32 {
    let a = atan2_deg(q.1.wrapping_sub(p.1), q.0.wrapping_sub(p.0));
    let c1 = wrap_angle(a.wrapping_add(off));
    let c2 = wrap_angle(c1.wrapping_sub(off.wrapping_add(off)));
    let d2 = wrap_angle(c2.wrapping_sub(p.2)).wrapping_abs();
    let d1 = wrap_angle(c1.wrapping_sub(p.2)).wrapping_abs();
    if d1 < d2 { c1 } else { c2 }
}

/// `PropertyWalk_FaceTowardsPoint` (`0x00533880`): target direction toward the look-at point, look-at point consumed.
pub(crate) fn face_towards_point(sim: &mut Sim, id: ObjectId, p: (i32, i32, i32)) {
    if let Some(w) = walk_mut(sim, id) {
        w.target_dir = if (p.0, p.1) == w.face_point {
            p.2
        } else {
            facing_dir_with_offset(p, w.face_point, w.target_dir_offset)
        };
        w.face_point = (-1, -1);
    }
}

/// `PropertyWalk_ClampPosToMap` (`0x00533410`, re/walk 16.6): moved to the hard radius from the map edge only
/// on a tile change; `false` if the tile lies outside afterwards.
pub fn clamp_pos_to_map(sim: &Sim, id: ObjectId, p: &mut (i32, i32)) -> bool {
    let (w, h) = (sim.state.path.width, sim.state.path.height);
    let (kx, kz) = (p.0 >> 12, p.1 >> 12);
    let cx = kx.max(0).min(w - 1);
    let cz = kz.max(0).min(h - 1);
    let r = sim.object(id).map_or(0, |o| o.hard_radius);
    if cx != kx {
        p.0 = if cx == 0 {
            r
        } else {
            (cx << 12).wrapping_add(0x1000 - r)
        };
    }
    if cz != kz {
        p.1 = if cz == 0 {
            r
        } else {
            (cz << 12).wrapping_add(0x1000 - r)
        };
    }
    sim.state.path.inside(p.0 >> 12, p.1 >> 12)
}

/// `PropertyWalk_IsBeyondTether` (`0x00533520`).
pub(crate) fn is_beyond_tether(sim: &Sim, id: ObjectId, p: (i32, i32)) -> bool {
    walk(sim, id).is_some_and(|w| {
        !w.mobile_home
            && w.home.0 != -1
            && w.tether >= 0
            && fixed_hypot(w.home.0.wrapping_sub(p.0), w.home.1.wrapping_sub(p.1)) > w.tether
    })
}

/// `PropertyWalk_IsIdleAt` (`0x00534CA0`, query `0x1D00`): without arguments "stands at the final destination".
pub(crate) fn is_idle_at(sim: &Sim, id: ObjectId, msg: &Message) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    let formation = w.state == 5 || w.restore_state == 5;
    if msg.args.len() == 2 {
        let at = (msg.arg(0), msg.arg(1));
        match w.high_path.back() {
            None => !formation && position(sim, id) == at,
            Some(t) => (t.x, t.z) == at,
        }
    } else {
        w.high_path.is_empty() && !formation
    }
}

/// `PropertyWalk_ClearHighPath` (`0x00533D40`).
pub(crate) fn clear_high_path(sim: &mut Sim, id: ObjectId) {
    if let Some(w) = walk_mut(sim, id) {
        w.high_path.clear();
        w.next_hp = None;
        // `+0x2C8 = 0` (re/walk 1.2): no catch-up point ahead any more.
        w.chase_pos = false;
    }
}

/// `PropertyWalk_StopMoving(keep_seek)` (`0x00533F20`, re/walk 16.1): waypoints and trajectory
/// gone, state 1, status bits `MOVE`/`0x10`/`0x800` off; the unit stays at its fixed-point
/// position (no snapping). `cur`/`running` are reset only if `cur != 0`.
pub(crate) fn stop_moving(sim: &mut Sim, id: ObjectId, keep_seek: bool) {
    clear_high_path(sim, id);
    set_status(sim, id, WAITING, false);
    set_status(sim, id, status::MOVE, false);
    set_status(sim, id, REEVALUATE, false);
    let Some(w) = walk_mut(sim, id) else { return };
    w.state = 1;
    w.wait_counter = 0;
    w.phase = 0;
    w.barging = false;
    w.ticks_since_chase = -1;
    if w.speed.cur != 0 {
        w.speed.cur = 0;
        w.speed.running = false;
    }
    clear_trajectory(sim, id);
    let Some(w) = walk_mut(sim, id) else { return };
    w.face_point = (-1, -1);
    w.target_dir = NO_DIRECTION;
    w.target_dir_offset = 0;
    w.target_offset = (0, 0);
    let dest = std::mem::replace(&mut w.dest, ObjectId::NONE);
    // Acknowledgment to the attack module, also to the carried object.
    let ack = Message::new(0xA11, &[]);
    send(sim, id, &ack);
    if let Some(t) = carried(sim, id) {
        send(sim, t, &ack);
    }
    if !dest.is_none() && sim.object(dest).is_some() {
        send(sim, dest, &Message::new(0xD03, &[id.as_i32()]));
        send(sim, dest, &Message::new(0x1232, &[id.as_i32()]));
    }
    if let Some(w) = walk_mut(sim, id) {
        w.last_status = u32::MAX;
    }
    if !keep_seek {
        clear_seek_target(sim, id);
        // Swim layer back (re/walk 16.1): the layer index is read before the check, the
        // flag stays set (quirk).
        if let Some(w) = walk_mut(sim, id)
            && w.swim_disallowed_for_combat
        {
            w.layer = layer_from_move_mask(LAYER_MASKS[w.layer] | 0x20);
        }
    }
}

/// `PropertyWalk_ClearSeekTarget` (`0x00533B10`): quarry, minimum attack radius and goal radius
/// to -1.
pub(crate) fn clear_seek_target(sim: &mut Sim, id: ObjectId) {
    if let Some(w) = walk_mut(sim, id) {
        w.quarry = ObjectId::NONE;
        w.min_attack_radius = -1;
        w.max_goal_radius = -1;
    }
}

/// Carried object for `CARRIING`.
fn carried(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let o = sim.object(id)?;
    let t = o.target;
    (o.status & status::CARRIING != 0 && !t.is_none() && t.0 != 0 && sim.object(t).is_some())
        .then_some(t)
}

/// `Module_SetFault/SuccessMessage(envelope)`.
fn take_replies(sim: &mut Sim, id: ObjectId, env: &Envelope) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
}

/// `PropertyWalk_ApplyMoveFlags(envelope, run, flags)` (`0x00535150`): tether (`0x02`),
/// halting (`0x80` keeps the target), acknowledgment to the attack module (without `0x1000`), taking over
/// the feedback messages, sprint, "in the obstacle" (terrain footprint >= `0x3FFFFFFE`).
pub(crate) fn apply_move_flags(sim: &mut Sim, id: ObjectId, env: &Envelope, run: bool, flags: i32) {
    if let Some(w) = walk_mut(sim, id)
        && w.home.0 != -1
    {
        w.mobile_home = flags & 0x02 == 0;
    }
    // Without flag 0x08|0x10 the unit leaves its group (re/walk 2.3, `0x0056F280`).
    let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
    if flags & (0x08 | 0x10) == 0 && gid != NO_GROUP {
        crate::group::remove_unit(sim, gid, id, id, false);
    }
    stop_moving(sim, id, flags & 0x80 != 0);
    if flags & 0x1000 == 0 {
        let (mut a0, mut a1) = (4, 2);
        if flags & 0x20 != 0 {
            a0 = 1;
        } else if flags & 0x40 != 0 {
            a1 = 1;
            a0 = if flags & 0x400 != 0 { 3 } else { 4 };
        } else if flags & 0x400 != 0 {
            a0 = 2;
        }
        let ack = Message::new(0xA11, &[a0, a1, 1]);
        send(sim, id, &ack);
        if let Some(t) = carried(sim, id) {
            send(sim, t, &ack);
        }
        if flags & 0x200 != 0 {
            send(sim, id, &Message::new(0xE11, &[]));
        }
    }
    take_replies(sim, id, env);
    let sprint = run
        && sim
            .module(id, INDEX)
            .is_some_and(|m| m.base.has_flags(0x4000));
    if sprint && let Some(w) = walk_mut(sim, id) {
        w.speed.start_sprint();
    }
    let layer = walk(sim, id).map_or(0, |w| w.layer_1x1());
    let (x, z) = position(sim, id);
    let c = footprint_terrain_cost(sim, x, z, id, layer);
    if let Some(w) = walk_mut(sim, id) {
        w.in_obstacle = c >= COST_BLOCKED;
    }
}

/// `PropertyWalk_ResolveGoalPosition` (`0x005379C0`): point goal or object goal (unit:
/// its position). A building (module 3 active) assigns a docking place (re/buildings):
/// already registered (`0x3403 {ownID, -1, -1}` != 0) -> that place, otherwise `0xB03 {x, z,
/// ownID, layer}` with the own position and the layer `m_PPHandleNxN` -> at index >= 0 the
/// place and the registration `0xC03 {index, ownID}`; at -1 ("Have no position at the building")
/// `false`, the fallback result stays unused. The direction comes back in whole degrees. The
/// tower branch (module `0x32`, `0x1132`/`0xE32`/`0x1032`) is omitted: module 50 has no class.
fn resolve_goal_position(
    sim: &mut Sim,
    id: ObjectId,
    msg: Option<&Message>,
    goal: &mut (i32, i32, i32),
) -> bool {
    let (x, z) = position(sim, id);
    let dir = msg
        .filter(|m| m.args.len() == 6)
        .map_or(NO_DIRECTION, |m| m.arg(3));
    *goal = (x, z, dir);
    let Some(msg) = msg else { return true };
    if msg.args.len() != 3 {
        goal.0 = msg.arg(0);
        goal.1 = msg.arg(1);
        return true;
    }
    let Some(t) = sim.object(ObjectId::from_i32(msg.arg(0))) else {
        return true;
    };
    let target = t.id;
    if !t.has_module(3, false) {
        goal.0 = t.grid.x.0;
        goal.1 = t.grid.z.0;
        return true;
    }
    let me = id.as_i32();
    let mut ask = Message::new(0x3403, &[me, -1, -1]);
    if send_mut(sim, target, &mut ask) != 0 {
        *goal = (ask.arg(0), ask.arg(1), ask.arg(2).wrapping_shl(12));
        return true;
    }
    let layer = walk(sim, id).map_or(0, |w| w.layer as i32);
    let mut dock = Message::new(0xB03, &[goal.0, goal.1, me, layer]);
    let index = send_mut(sim, target, &mut dock);
    if index < 0 {
        log::debug!("Walk {id:?}: no position at the building {target:?}");
        return false;
    }
    *goal = (dock.arg(0), dock.arg(1), dock.arg(2).wrapping_shl(12));
    send(sim, target, &Message::new(0xC03, &[index, me]));
    true
}

/// `PropertyWalk_IsWithinGoalRadius` (`0x00537660`): within the goal radius (not in the obstacle), then
/// goal = own position (quirk), convert the look-at point into a direction and check the direction.
fn is_within_goal_radius(sim: &mut Sim, id: ObjectId, goal: &mut (i32, i32, i32)) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    let (in_obstacle, max) = (w.in_obstacle, w.max_goal_radius);
    let r = goal_radius(sim, id, max);
    let (x, z) = position(sim, id);
    if fixed_hypot(x.wrapping_sub(goal.0), z.wrapping_sub(goal.1)) > r || in_obstacle {
        return false;
    }
    goal.0 = x;
    goal.1 = z;
    let dir = direction(sim, id);
    if walk(sim, id).is_some_and(|w| w.face_point.0 != -1) {
        face_towards_point(sim, id, (x, z, dir));
    }
    if walk(sim, id).is_some_and(|w| w.target_dir == NO_DIRECTION) {
        return true;
    }
    is_facing_target_dir(sim, id, dir)
}

/// `PropertyWalk_OnMsgFindTo` (`0x00538590`, re/walk 2.3).
pub(crate) fn on_msg_find_to(sim: &mut Sim, id: ObjectId, env: &Envelope, run: bool) -> bool {
    let msg = env.msg.clone();
    let flags = last_arg(&msg);
    if let Some(w) = walk_mut(sim, id) {
        w.find_to = Some(msg.clone());
    }
    apply_move_flags(sim, id, env, run, flags);
    let mut goal = (0, 0, NO_DIRECTION);
    if !resolve_goal_position(sim, id, Some(&msg), &mut goal) {
        stop_moving(sim, id, false);
        set_state(sim, id, 9);
        set_status(sim, id, status::MOVE, true);
        set_status(sim, id, WAITING, true);
        if flags & 0x40 != 0 {
            send(sim, id, &Message::new(0xA11, &[0, 1, 0]));
        }
        return true;
    }
    if flags & 8 != 0 {
        let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
        if crate::group::find(sim, gid).is_some() {
            crate::group::on_leader_find_to(sim, gid, id);
        }
    }
    let mut g = (goal.0, goal.1);
    if !clamp_pos_to_map(sim, id, &mut g) {
        abort(sim, id);
        return false;
    }
    (goal.0, goal.1) = g;
    if g != position(sim, id) && walk(sim, id).is_some_and(|w| w.in_obstacle) {
        // First free the unit from the obstacle (CHECKPOS), then repeat the backed-up command
        // (`0x1B00` as success message).
        if let Some(w) = walk_mut(sim, id) {
            w.backup = Some(env.clone());
        }
        let check = Envelope {
            msg: Message::new(0x900, &[]),
            success: Some(Message::new(0x1B00, &[])),
            fault: env.fault.clone(),
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        dispatch_command(sim, id, &check);
        return true;
    }
    if let Some(w) = walk_mut(sim, id) {
        w.target_dir = goal.2;
    }
    set_facing_tolerance(sim, id, tolerance_arg(&msg));
    if msg.args.len() != 3 {
        let r = goal_radius(sim, id, msg.arg(2));
        let face = if msg.args.len() == 7 {
            (msg.arg(3), msg.arg(4))
        } else {
            (-1, -1)
        };
        if let Some(w) = walk_mut(sim, id) {
            w.max_goal_radius = r;
            w.face_point = face;
        }
        let mut f = face;
        if f.0 != -1 {
            if !clamp_pos_to_map(sim, id, &mut f) {
                abort(sim, id);
                return false;
            }
            if let Some(w) = walk_mut(sim, id) {
                w.face_point = f;
            }
        }
    } else {
        let dest = ObjectId::from_i32(msg.arg(0));
        let Some(t) = sim.object(dest) else {
            if let Some(w) = walk_mut(sim, id) {
                w.dest = ObjectId::NONE;
            }
            return false;
        };
        // Building radius also for module 0x32 (tower, `0x0053884E`); module 50 has no class, the
        // branch is dead in the original as it is here.
        let (train, building, tpos) = (
            t.has_module(0x21, false),
            t.has_module(3, false) || t.has_module(0x32, false),
            (t.grid.x.0, t.grid.z.0),
        );
        let default = walk(sim, id).map_or(0, |w| w.speed.default);
        if let Some(w) = walk_mut(sim, id) {
            w.dest = dest;
            w.max_goal_radius = if train {
                0x2000
            } else if building {
                default.max(0x100)
            } else {
                w.face_point = tpos;
                0x400
            };
        }
    }
    if is_within_goal_radius(sim, id, &mut goal) {
        stop_moving(sim, id, false);
        send_success(sim, id, INDEX);
        return true;
    }
    if walk(sim, id).is_some_and(|w| w.can_move) {
        let layer = walk(sim, id).map_or(0, |w| w.layer);
        let from = position(sim, id);
        match plan_high_path(
            &sim.state.path,
            layer,
            from,
            (goal.0, goal.1),
            msg.code != 0x200,
            flags,
        ) {
            Ok(path) => {
                // PATH synchronously to the unit itself.
                send(sim, id, &path.message());
                return true;
            }
            Err(e) => log::debug!("Walk {id:?}: PMSG_PTTOPT_FINDTO aborted ({e:?})"),
        }
    }
    abort(sim, id);
    false
}

/// `PropertyWalk_OnMsgPathTo(envelope, run, project)` (`0x00539E00`, re/walk 2.6).
pub(crate) fn on_msg_path_to(
    sim: &mut Sim,
    id: ObjectId,
    env: &Envelope,
    run: bool,
    project: bool,
) -> bool {
    if !walk(sim, id).is_some_and(|w| w.can_move) {
        abort(sim, id);
        return false;
    }
    let msg = &env.msg;
    let argc = msg.args.len() as i32;
    if let Some(w) = walk_mut(sim, id) {
        w.last_bstar_result = 8;
    }
    let start = position(sim, id);
    let end = (msg.arg(0), msg.arg(1));
    let flags = msg.arg(3);
    let head = if (argc - 7) % 2 == 1 { 8 } else { 7 };
    if msg.arg(2) == 0 {
        // Independent: take over the feedback messages and halt.
        if let Some(w) = walk_mut(sim, id) {
            w.find_to = None;
        }
        apply_move_flags(sim, id, env, run, flags);
        let r = goal_radius(sim, id, msg.arg(4));
        if let Some(w) = walk_mut(sim, id) {
            w.max_goal_radius = r;
        }
        if head == 7 {
            if let Some(w) = walk_mut(sim, id) {
                w.target_dir = msg.arg(5);
            }
            set_facing_tolerance(sim, id, msg.arg(6));
        } else {
            let mut f = (msg.arg(5), msg.arg(6));
            if let Some(w) = walk_mut(sim, id) {
                w.face_point = f;
            }
            set_facing_tolerance(sim, id, msg.arg(7));
            if f.0 != -1 {
                if !clamp_pos_to_map(sim, id, &mut f) {
                    abort(sim, id);
                    return false;
                }
                if let Some(w) = walk_mut(sim, id) {
                    w.face_point = f;
                }
            }
        }
        if run && let Some(w) = walk_mut(sim, id) {
            w.speed.start_sprint();
        }
    }
    let at_goal = walk(sim, id).is_some_and(|w| w.face_point == (-1, -1))
        && (walk(sim, id).is_some_and(|w| w.target_dir == NO_DIRECTION)
            || is_facing_target_dir(sim, id, direction(sim, id)));
    if start == end && at_goal {
        on_target_reached(sim, id);
        return true;
    }
    let mut k = head as usize;
    while k + 1 < msg.args.len() {
        let (x, z) = (msg.args[k], msg.args[k + 1]);
        k += 2;
        // Same as the predecessor: removed again (`HighPath_IsNodeDistinctFromNeighbours`).
        if walk(sim, id).is_some_and(|w| w.high_path.back().is_some_and(|b| (b.x, b.z) == (x, z))) {
            continue;
        }
        let mut c = (x, z);
        if !clamp_pos_to_map(sim, id, &mut c) || is_beyond_tether(sim, id, (x, z)) {
            abort(sim, id);
            return false;
        }
        if let Some(w) = walk_mut(sim, id) {
            let p = w.point(c.0, c.1);
            w.high_path.push_back(p);
        }
    }
    // End point without duplicate check and unclamped; only a copy is checked (quirk).
    if let Some(w) = walk_mut(sim, id) {
        let p = w.point(end.0, end.1);
        w.high_path.push_back(p);
        w.next_hp = w.high_path.front().map(|p| p.id);
    }
    let mut e = end;
    if !clamp_pos_to_map(sim, id, &mut e) {
        abort(sim, id);
        return false;
    }
    if sim.object(id).is_some_and(|o| o.status & status::DIED != 0) {
        clear_seek_target(sim, id);
    }
    // The tether at the end point applies only without a quarry (pursuit).
    if walk(sim, id).is_some_and(|w| w.quarry.is_none()) && is_beyond_tether(sim, id, e) {
        abort(sim, id);
        return false;
    }
    // Group members (flag 8): the group distributes the path (vf18); if it refuses -> abort.
    // Projection without waypoint shift (re/walk 2.6).
    let multi = if flags & 0x08 != 0 {
        let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
        if crate::group::find(sim, gid).is_some() && !crate::group::on_leader_path(sim, gid, id) {
            abort(sim, id);
            return false;
        }
        false
    } else {
        true
    };
    start_walking(sim, id, multi, project);
    true
}

/// `PropertyWalk_StartWalking(multi, project)` (`0x00539180`).
pub(crate) fn start_walking(sim: &mut Sim, id: ObjectId, multi: bool, project: bool) {
    set_state(sim, id, 2);
    set_status(sim, id, status::MOVE, true);
    set_status(sim, id, WAITING, false);
    let layer = walk(sim, id).map_or(0, |w| w.layer_1x1());
    if let Some(w) = walk_mut(sim, id) {
        w.last_status = u32::MAX;
        w.speed.ensure_moving();
    }
    let (x, z) = position(sim, id);
    let c = footprint_terrain_cost(sim, x, z, id, layer);
    if let Some(w) = walk_mut(sim, id) {
        w.in_obstacle = c > 0;
    }
    if project {
        project_trajectory(sim, id, multi, false);
    }
}

/// `PropertyWalk_OnTargetReached` (`0x00539500`, re/walk 4.9): success within the goal radius; otherwise
/// repeat FINDTO once (flag 4) if the goal is not hard-occupied; otherwise abort.
pub(crate) fn on_target_reached(sim: &mut Sim, id: ObjectId) {
    // With a quarry the pursuit takes over (re/combat-attack 7a.9): no retry, no success.
    if sim.object(id).is_some_and(|o| o.status & status::DIED != 0) {
        clear_seek_target(sim, id);
    }
    if walk(sim, id).is_some_and(|w| !w.quarry.is_none()) {
        super::seek::update_seek_target(sim, id, true, true);
        return;
    }
    // "Formation goal reached" (re/walk 4.9, re/groups-formations 3.5 step 5): state 5/0xB without a goal check.
    if walk(sim, id).is_some_and(|w| matches!(w.state, 5 | 0xB)) {
        stop_moving(sim, id, false);
        send_success(sim, id, INDEX);
        return;
    }
    let find_to = walk(sim, id).and_then(|w| w.find_to.clone());
    let mut goal = (0, 0, NO_DIRECTION);
    let ok = resolve_goal_position(sim, id, find_to.as_ref(), &mut goal);
    if ok && (find_to.is_none() || is_within_goal_radius(sim, id, &mut goal)) {
        stop_moving(sim, id, false);
        send_success(sim, id, INDEX);
        return;
    }
    if let Some(mut msg) = find_to
        && last_arg(&msg) & 4 == 0
    {
        let layer = walk(sim, id).map_or(0, |w| w.layer_1x1());
        let c = crate::collision::query_cost_at(sim, goal.0, goal.1, Some(id), layer, false, false);
        if c < crate::motion::step::COST_COLLISION {
            if let Some(last) = msg.args.last_mut() {
                *last |= 4;
            }
            if let Some(w) = walk_mut(sim, id) {
                w.find_to = Some(msg.clone());
            }
            let env = replies(sim, id, msg);
            super::on_message(sim, id, &env);
            return;
        }
    }
    abort(sim, id);
}

/// Envelope with the stored feedback messages of the module.
pub(crate) fn replies(sim: &Sim, id: ObjectId, msg: Message) -> Envelope {
    let base = sim.module(id, INDEX).map(|m| &m.base);
    Envelope {
        msg,
        success: base.and_then(|b| b.success.clone()),
        fault: base.and_then(|b| b.fault.clone()),
        success_target: base.map_or(ObjectId::NONE, |b| b.success_target),
        fault_target: base.map_or(ObjectId::NONE, |b| b.fault_target),
    }
}

/// `PropertyWalk_OnMsgCheckPos` (`0x00535490`, re/walk 2.7): free -> success (state stays, also
/// 7, quirk); in a terrain obstacle the unit frees itself with the B* planner (mode 2).
pub(crate) fn on_msg_check_pos(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let Some(w) = walk(sim, id) else { return true };
    if !w.can_move {
        return true;
    }
    if w.state != 7 {
        take_replies(sim, id, env);
    }
    let layer = walk(sim, id).map_or(0, |w| w.layer_1x1());
    let (x, z) = position(sim, id);
    let c = footprint_terrain_cost(sim, x, z, id, layer);
    if let Some(w) = walk_mut(sim, id) {
        w.in_obstacle = c > 0;
    }
    if c <= 0 {
        send_success(sim, id, INDEX);
        return true;
    }
    log::debug!("Walk {id:?}: PMSG_PTTOPT_CHECKPOS: blocked, leaving the blockage");
    if let Some(w) = walk_mut(sim, id) {
        w.mobile_home = true;
    }
    super::plan::escape_obstacle(sim, id, env);
    true
}

/// `PropertyWalk_TickIdleGoHome` (`0x00533210`, state 1): after 17 quiet ticks to the fixed
/// home; if that is not possible, the home is given up.
pub(crate) fn tick_idle_go_home(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk(sim, id) else { return };
    if w.home.0 == -1 || w.mobile_home || w.state != 1 {
        return;
    }
    if sim.object(id).is_none_or(|o| o.status & 0xFBFF_AFFF != 0) {
        return;
    }
    let home = w.home;
    let Some(w) = walk_mut(sim, id) else { return };
    w.home_timer = w.home_timer.wrapping_add(1);
    if w.home_timer < 17 || position(sim, id) == (home.0, home.1) {
        return;
    }
    if let Some(w) = walk_mut(sim, id) {
        w.home_timer = 0;
    }
    let mut m = find_to_msg(home.0, home.1, -1, home.2, -1, false, 0x22);
    if super::query(sim, id, &mut m) != 1 {
        let (x, z) = position(sim, id);
        if let Some(w) = walk_mut(sim, id) {
            w.home.0 = x;
            w.home.1 = z;
            w.mobile_home = true;
        }
    }
}
