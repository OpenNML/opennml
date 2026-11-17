//! Replanning with the B* local planner (re/walk 4.5-4.7, 5, 16.3): if the trajectory is blocked,
//! the unit replans to the next waypoint (state 4, one planner call per player and 100 ms),
//! frees itself from obstacles (CHECKPOS), waits patiently (state 8) or repeats the
//! tile path (A* retry).

use crate::bstar::{
    Request, continue_path, end_incomplete_path, fetch_result_path, find_path, planner_index,
};
use crate::collision::{cost_at, footprint_terrain_cost, is_collision_exempt};
use crate::fx::fixed_hypot;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::motion::step::{COST_BLOCKED, COST_COLLISION};
use crate::motion::{NO_DIRECTION, SpeedInfo};
use crate::object::status;
use crate::sim::Sim;

use super::goto::{clear_seek_target, replies};
use super::project::{clear_trajectory, is_at_final_goal, project_trajectory};
use super::{
    HighPoint, REEVALUATE, WAITING, abort, on_message, position, query, set_state, set_status,
    walk, walk_mut,
};

/// Planner of the owner.
fn slot(id: ObjectId) -> usize {
    planner_index(id.slot())
}

/// `PathMsg_ConstructIndependent` (`0x004E54C0`) without points: `[x, z, 0, flags, radius, direction,
/// tolerance]`.
fn path_msg_independent(
    x: i32,
    z: i32,
    radius: i32,
    dir: i32,
    tol: i32,
    run: bool,
    flags: i32,
) -> Message {
    let code = if run { 0x600 } else { 0x500 };
    Message::new(code, &[x, z, 0, flags, radius, dir, tol])
}

/// `PropertyWalk_OnMsgRepath` (query `0x1000 {avoid}`, `0x00538AC0`): switch to state 4
/// (from rest with `MOVE`), discard the trajectory; the avoid handle is remembered.
pub(crate) fn on_msg_repath(sim: &mut Sim, id: ObjectId, avoid: ObjectId) -> i32 {
    let Some(w) = walk(sim, id) else { return 0 };
    if !w.can_move {
        return 0;
    }
    // Formation march: catch up first (re/groups-formations 3.6), then plan from state 0xB.
    if w.state == 5 {
        super::formation::enter_formation_walk(sim, id);
    }
    let Some(w) = walk(sim, id) else { return 0 };
    let (state, restore) = (w.state, w.restore_state);
    if state == 8 || state == 3 {
        set_state(sim, id, restore);
    }
    let state = walk(sim, id).map_or(0, |w| w.state);
    if state != 4 && state != 7 {
        if state == 6 || state == 1 {
            set_state(sim, id, 2);
            set_status(sim, id, status::MOVE, true);
            if let Some(w) = walk_mut(sim, id) {
                w.speed.ensure_moving();
            }
        }
        let state = walk(sim, id).map_or(0, |w| w.state);
        if let Some(w) = walk_mut(sim, id) {
            w.restore_state = state;
            w.state = 4;
        }
        set_status(sim, id, WAITING, true);
        clear_trajectory(sim, id);
        if let Some(w) = walk_mut(sim, id) {
            w.last_bstar_result = 8;
            w.hid_to_escape = avoid;
            w.phase = 0;
        }
    }
    1
}

/// `PropertyWalk_OnMsgWaitPatiently` (query `0x2E00`, `0x00538A50`): running -> state 8.
pub(crate) fn on_msg_wait_patiently(sim: &mut Sim, id: ObjectId) -> i32 {
    if walk(sim, id).is_some_and(|w| w.state == 5) {
        super::formation::enter_formation_walk(sim, id);
    }
    let moving = sim.object(id).is_some_and(|o| o.status & status::MOVE != 0);
    let Some(w) = walk_mut(sim, id) else { return 0 };
    if !moving || (w.state != 2 && w.state != 0xB) {
        return 0;
    }
    w.wait_counter = 0;
    w.restore_state = w.state;
    w.state = 8;
    set_status(sim, id, WAITING, true);
    1
}

/// `PropertyWalk_AbortAndResetRestore` (`0x00532BE0`).
fn abort_and_reset_restore(sim: &mut Sim, id: ObjectId) -> bool {
    abort(sim, id);
    if let Some(w) = walk_mut(sim, id) {
        w.restore_state = 1;
    }
    true
}

/// `PropertyWalk_HasQuarry` (`0x00533CD0`): if the own unit is dead, `ClearSeekTarget` first;
/// then "quarry set".
fn has_quarry(sim: &mut Sim, id: ObjectId) -> bool {
    if sim.object(id).is_some_and(|o| o.status & status::DIED != 0) {
        clear_seek_target(sim, id);
    }
    walk(sim, id).is_some_and(|w| !w.quarry.is_none())
}

/// `PropertyWalk_TickPlanning` (`0x0053C670`, state 4).
pub(crate) fn tick_planning(sim: &mut Sim, id: ObjectId) {
    if !sim.state.planners[slot(id)].try_acquire(id) {
        return;
    }
    if !set_new_path(sim, id) {
        return;
    }
    let restore = walk(sim, id).map_or(1, |w| w.restore_state);
    set_state(sim, id, restore);
    let moving = sim.object(id).is_some_and(|o| o.status & status::MOVE != 0);
    if moving && (restore == 2 || restore == 0xB) {
        set_status(sim, id, WAITING, false);
        set_status(sim, id, REEVALUATE, true);
    } else {
        // Quirk: also after `AbortAndResetRestore` (double abort).
        abort(sim, id);
    }
}

/// `GameObject_GetEdgeDistanceToPoint` (`0x00468930`, re/walk 16.2) for a unit:
/// `max(0, |p − position| − hard radius)`.
fn edge_distance(sim: &Sim, id: ObjectId, p: (i32, i32)) -> i32 {
    let (x, z) = position(sim, id);
    let r = sim.object(id).map_or(0, |o| o.hard_radius);
    let d = fixed_hypot(p.0.wrapping_sub(x), p.1.wrapping_sub(z));
    let d = if r > 0 { d - r } else { d };
    d.max(0)
}

/// `PropertyWalk_SetNewPath` (`0x0053B170`, re/walk 4.6): continue the planner phase or start
/// anew; `false` = still in progress.
fn set_new_path(sim: &mut Sim, id: ObjectId) -> bool {
    let s = slot(id);
    let Some(w) = walk_mut(sim, id) else {
        return false;
    };
    if w.phase == 2 && w.speed.default > 10 {
        w.phase = 5;
    }
    let (last, phase, speed, mut req) = (w.last_bstar_result, w.phase, w.speed, w.request);
    let r = if last == 2 {
        continue_path(sim, s, id)
    } else if phase == 0 {
        let Some(w) = walk(sim, id) else { return false };
        let (layer, escape, target) =
            (w.layer_1x1(), w.hid_to_escape, w.high_path.front().copied());
        let pos = position(sim, id);
        let c = cost_at(sim, pos, Some(id), layer, false, false);
        let in_obstacle = c >= COST_BLOCKED;
        let soft = !in_obstacle && c >= COST_COLLISION;
        if let Some(w) = walk_mut(sim, id) {
            w.in_obstacle = in_obstacle;
            if w.trajectory.is_empty() && w.high_path.is_empty() {
                w.find_to = None;
            }
        }
        if in_obstacle {
            req = Request::escape(sim, id, layer, false);
        } else if !escape.is_none() {
            if !soft {
                project_trajectory(sim, id, true, false);
                return true;
            }
            req = Request::escape(sim, id, layer, true);
        } else if let Some(t) = target {
            let t = (t.x, t.z);
            // With a quarry only the terrain at the goal point counts (the opponent itself occupies it).
            let blocked = if has_quarry(sim, id) {
                footprint_terrain_cost(sim, t.0, t.1, id, layer) >= COST_BLOCKED
            } else {
                cost_at(sim, t, Some(id), layer, true, false) >= COST_COLLISION
            };
            if blocked && edge_distance(sim, id, t) < 0x401 {
                log::debug!("Walk {id:?}: Close enough");
                if let Some(w) = walk_mut(sim, id) {
                    w.high_path.pop_front();
                }
                project_trajectory(sim, id, true, false);
                return true;
            }
            let barging = soft || walk(sim, id).is_some_and(|w| w.barging);
            req = Request::go_to(sim, id, layer, t, blocked, -1, barging);
            if let Some(w) = walk(sim, id)
                && w.high_path.len() == 1
                && w.restore_state != 0xB
            {
                req.radius = w.max_goal_radius;
                // Last leg to the quarry: goal radius larger by both hard radii, unless the
                // unit is collision-free at the goal point (`IsCollisionExempt(t, 1)` `0x0046BA80`).
                let quarry = w.quarry;
                let exempt = sim
                    .object(id)
                    .is_some_and(|o| is_collision_exempt(sim, o, Some(t), true));
                if has_quarry(sim, id) && !exempt {
                    let Some(q) = sim.state.object(quarry, true) else {
                        return abort_and_reset_restore(sim, id);
                    };
                    let rq = q.hard_radius;
                    let rg = sim.object(id).map_or(0, |o| o.hard_radius);
                    req.radius = req.radius.wrapping_add(rg.wrapping_add(rq));
                }
            }
        } else {
            return abort_and_reset_restore(sim, id);
        }
        if let Some(w) = walk_mut(sim, id) {
            w.request = req;
        }
        find_path(sim, s, &speed, &req)
    } else if phase == 2 {
        // Coarse: larger steps or soft costs count as free.
        match req.mode {
            0 | 1 => req.mult = if speed.cur != 0 { 0x800 / speed.cur } else { 1 },
            2 | 3 => req.cost_limit = COST_COLLISION,
            _ => {}
        }
        if let Some(w) = walk_mut(sim, id) {
            w.request = req;
        }
        find_path(sim, s, &speed, &req)
    } else if phase < 5 {
        continue_path(sim, s, id)
    } else {
        end_incomplete_path(sim, s)
    };
    if let Some(w) = walk_mut(sim, id) {
        w.last_bstar_result = r;
    }
    handle_path_result(sim, id)
}

/// `PropertyWalk_IsEscapeDone` (`0x00533DC0`, re/walk 16.3): the escape target stands (or has
/// no active walk module) and touches the head waypoint.
fn is_escape_done(sim: &mut Sim, id: ObjectId, esc: ObjectId) -> bool {
    let Some(head) = walk(sim, id).and_then(|w| w.high_path.front().copied()) else {
        return false;
    };
    if sim.object(esc).is_none() {
        return false;
    }
    if crate::messaging::send(sim, esc, &Message::new(0x1D00, &[])) == 0 {
        return false;
    }
    let (Some(e), Some(o)) = (sim.object(esc), sim.object(id)) else {
        return false;
    };
    fixed_hypot(
        e.grid.x.0.wrapping_sub(head.x),
        e.grid.z.0.wrapping_sub(head.z),
    ) < e.hard_radius.wrapping_add(o.hard_radius)
}

/// `PropertyWalk_HandlePathResult` (`0x0053A6A0`, re/walk 4.6).
fn handle_path_result(sim: &mut Sim, id: ObjectId) -> bool {
    let s = slot(id);
    let Some(w) = walk(sim, id) else { return false };
    let (last, phase, mode, retries) = (w.last_bstar_result, w.phase, w.request.mode, w.retries);
    match last {
        1 => {
            if let Some(w) = walk_mut(sim, id) {
                w.phase = w.phase.wrapping_add(1);
            }
            false
        }
        0 if mode <= 1 => {
            if phase < 2 {
                if let Some(w) = walk_mut(sim, id) {
                    w.phase = 2;
                }
                return false;
            }
            if retries >= 2 {
                abort(sim, id);
                return false;
            }
            if let Some(w) = walk_mut(sim, id) {
                w.retries = w.retries.wrapping_add(1);
            }
            retry_trapped_with_astar(sim, id)
        }
        0 | 7 => {
            if let Some(w) = walk_mut(sim, id) {
                w.retries = 0;
            }
            if phase < 2 {
                if mode == 2 || mode == 3 {
                    if let Some(w) = walk_mut(sim, id) {
                        w.phase = 2;
                        w.last_bstar_result = 1;
                    }
                    return false;
                }
                let Some(w) = walk(sim, id) else { return false };
                if w.speed.cur > 10 {
                    // Retry slowly (10), in the same tick without allocation.
                    let mut slow = SpeedInfo::default();
                    slow.set_default_speed(10);
                    slow.set_cruising_speed(10);
                    slow.set_cur_speed(10);
                    slow.set_turn_speed(w.speed.turn);
                    slow.instant_turn = w.speed.instant_turn;
                    let req = w.request;
                    let r = find_path(sim, s, &slow, &req);
                    let Some(w) = walk_mut(sim, id) else {
                        return false;
                    };
                    w.last_bstar_result = r;
                    if r == 7 {
                        w.phase = w.phase.wrapping_add(1);
                        return false;
                    }
                    w.speed.set_cur_speed(10);
                    return handle_path_result(sim, id);
                }
            }
            let restore = walk(sim, id).map_or(1, |w| w.restore_state);
            set_state(sim, id, restore);
            query(sim, id, &mut Message::new(0x2E00, &[]));
            false
        }
        3..=6 => {
            let mut traj = std::collections::VecDeque::new();
            let following = fetch_result_path(sim, s, &mut traj);
            let Some(w) = walk_mut(sim, id) else {
                return false;
            };
            w.retries = 0;
            w.trajectory = traj;
            w.following_bstar = following;
            if w.speed.cur == 10 {
                thin_slow_trajectory(sim, id);
            }
            let Some(w) = walk(sim, id) else { return false };
            let tail = *w
                .trajectory
                .back()
                .expect("fetching returns at least one point");
            let (escape, restore) = (w.hid_to_escape, w.restore_state);
            if last == 4 && !is_escape_done(sim, id, escape) {
                log::debug!("Walk {id:?}: Escaping to ({}, {})", tail.x, tail.z);
                let Some(w) = walk_mut(sim, id) else {
                    return false;
                };
                let p = w.point(tail.x, tail.z);
                w.high_path.push_front(p);
                if w.high_path.get(1).is_some_and(|n| (n.x, n.z) == (p.x, p.z)) {
                    w.high_path.pop_front();
                }
                w.next_hp = w.high_path.get(1).map(|p| p.id);
            } else if last != 6 && restore != 0xB {
                let Some(w) = walk_mut(sim, id) else {
                    return false;
                };
                if w.high_path.is_empty() {
                    let p = w.point(-1, -1);
                    w.high_path.push_front(p);
                }
                // "Replacing target point": head waypoint := end point of the B* trajectory.
                let head: &mut HighPoint = w.high_path.front_mut().expect("just filled");
                (head.x, head.z) = (tail.x, tail.z);
                w.next_hp = w.high_path.get(1).map(|p| p.id);
            }
            if !following {
                if !project_trajectory(sim, id, true, false) {
                    abort_and_reset_restore(sim, id);
                }
                return true;
            }
            let Some(w) = walk_mut(sim, id) else {
                return false;
            };
            if w.request.mode == 2 || w.request.mode == 3 || w.request.barging {
                w.barging = true;
            }
            let at_goal = is_at_final_goal(sim, id, &tail);
            if let Some(w) = walk_mut(sim, id) {
                w.ende = if at_goal { 0 } else { 1 };
            }
            true
        }
        _ => false,
    }
}

/// `PropertyWalk_ThinSlowTrajectory` (`0x00538EF0`): keep every `k`-th point (`k = default / 10`),
/// speed `10 · k`; if the last one is missing, it is inserted **at the front** (quirk).
pub(crate) fn thin_slow_trajectory(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk_mut(sim, id) else { return };
    let k = w.speed.default / 10;
    if k <= 1 {
        return;
    }
    let mut thin: std::collections::VecDeque<_> =
        w.trajectory.iter().step_by(k as usize).copied().collect();
    if let (Some(&old), Some(new)) = (w.trajectory.back(), thin.back())
        && (old.x, old.z) != (new.x, new.z)
    {
        thin.push_front(old);
    }
    w.trajectory = thin;
    w.speed.set_cur_speed(10 * k);
}

/// `PropertyWalk_RetryTrappedWithAStar` (`0x00535BE0`): new FINDTO (flags 1) to the end point or
/// to the goal object with the stored feedback messages; the tolerance passes through in fixed point as a
/// degree argument (-> 5 degrees, quirk). With a quarry (even dead) a new FINDTO_UNIT with flags 0;
/// if it fails, `restore_state = 1` applies.
fn retry_trapped_with_astar(sim: &mut Sim, id: ObjectId) -> bool {
    log::debug!("Walk {id:?}: Trapped! Calling A*.");
    if sim.object(id).is_some_and(|o| o.status & status::DIED != 0) {
        clear_seek_target(sim, id);
    }
    let Some(w) = walk(sim, id) else { return false };
    if !w.quarry.is_none() {
        if sim.state.object(w.quarry, false).is_none() {
            return abort_and_reset_restore(sim, id);
        }
        let msg = Message::new(
            0x400,
            &[
                w.quarry.as_i32(),
                w.min_attack_radius,
                w.max_attack_radius,
                w.target_dir_offset,
                w.tolerance,
                0,
                w.target_offset.0,
                w.target_offset.1,
            ],
        );
        let env = replies(sim, id, msg);
        if !on_message(sim, id, &env)
            && let Some(w) = walk_mut(sim, id)
        {
            w.restore_state = 1;
        }
        return true;
    }
    // Formation (re/groups-formations 12.4, K-253): from catching up, back to the group anchor and leave.
    if walk(sim, id).is_some_and(|w| w.dest.is_none() && w.restore_state == 0xB) {
        let gid = walk(sim, id).map_or(crate::group::NO_GROUP, |w| w.group);
        // Without a group the original crashes (K-253, A7): then only the previous state to 2.
        if let Some(anchor) = crate::group::find(sim, gid).map(|g| g.anchor) {
            super::goto::clear_high_path(sim, id);
            if let Some(w) = walk_mut(sim, id) {
                let h = w.point(anchor.0, anchor.1);
                w.high_path.push_front(h);
                w.next_hp = Some(h.id);
            }
            crate::group::remove_unit(sim, gid, id, id, false);
        }
        if let Some(w) = walk_mut(sim, id) {
            w.restore_state = 2;
        }
    }
    let Some(w) = walk(sim, id) else { return false };
    let msg = if w.dest.is_none() {
        let Some(e) = w.high_path.back() else {
            return false;
        };
        if w.target_dir == NO_DIRECTION {
            Message::new(
                0x200,
                &[
                    e.x,
                    e.z,
                    w.max_goal_radius,
                    w.face_point.0,
                    w.face_point.1,
                    w.tolerance,
                    1,
                ],
            )
        } else {
            super::goto::find_to_msg(
                e.x,
                e.z,
                w.max_goal_radius,
                w.target_dir,
                w.tolerance,
                false,
                1,
            )
        }
    } else {
        let code = if w.speed.running { 0x100 } else { 0x200 };
        Message::new(code, &[w.dest.as_i32(), w.tolerance, 1])
    };
    let env = replies(sim, id, msg);
    if !on_message(sim, id, &env) {
        abort_and_reset_restore(sim, id);
    }
    true
}

/// Terrain obstacle at CHECKPOS (re/walk 2.7): take the planner slot (otherwise state 7), search mode 2
/// completely and smooth, then walk to the end point by an independent PATH (the
/// B* trajectory itself is discarded, quirk).
pub(crate) fn escape_obstacle(sim: &mut Sim, id: ObjectId, env: &Envelope) {
    let s = slot(id);
    if !sim.state.planners[s].try_acquire(id) {
        log::debug!("Walk {id:?}: No time for more BStar paths this tick.");
        set_state(sim, id, 7);
        return;
    }
    let Some(w) = walk_mut(sim, id) else { return };
    w.speed.ensure_moving();
    let (speed, layer) = (w.speed, w.layer_1x1());
    let req = Request::escape(sim, id, layer, false);
    let mut r = find_path(sim, s, &speed, &req);
    while r == 2 {
        r = continue_path(sim, s, id);
    }
    if r != 4 {
        log::debug!("Walk {id:?}: PMSG_PTTOPT_CHECKPOS: no way out found");
        sim.state.planners[s].forget_unit(id);
        return;
    }
    let mut traj = std::collections::VecDeque::new();
    fetch_result_path(sim, s, &mut traj);
    let Some(tail) = traj.back().copied() else {
        return;
    };
    if let Some(w) = walk_mut(sim, id) {
        w.trajectory = traj;
        w.barging = true;
    }
    let path = Envelope {
        msg: path_msg_independent(tail.x, tail.z, -1, NO_DIRECTION, -1, false, 1),
        success: env.success.clone(),
        fault: env.fault.clone(),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    on_message(sim, id, &path);
}

/// State 7: repeat CHECKPOS with the own feedback messages.
pub(crate) fn tick_check_pos(sim: &mut Sim, id: ObjectId) {
    let env = replies(sim, id, Message::new(0x900, &[]));
    super::goto::on_msg_check_pos(sim, id, &env);
}

/// `PropertyWalk_TickWaitPatiently` (`0x00535B60`, state 8): replan after 23 ticks.
pub(crate) fn tick_wait_patiently(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk_mut(sim, id) else { return };
    w.wait_counter = w.wait_counter.wrapping_add(1);
    if w.wait_counter <= 22 {
        return;
    }
    let (restore, escape) = (w.restore_state, w.hid_to_escape);
    if sim.object(id).is_some_and(|o| o.status & status::MOVE != 0) {
        set_status(sim, id, REEVALUATE, true);
    }
    set_state(sim, id, restore);
    query(sim, id, &mut Message::new(0x1000, &[escape.as_i32()]));
}

/// `PropertyWalk_TickRetryFindTo` (`0x00532B60`, state 9): repeat the FINDTO after 23 ticks
/// (the counter is not reset in the process, quirk).
pub(crate) fn tick_retry_find_to(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk_mut(sim, id) else { return };
    w.wait_counter = w.wait_counter.wrapping_add(1);
    if w.wait_counter <= 22 {
        return;
    }
    let Some(msg) = w.find_to.clone() else { return };
    let env = replies(sim, id, msg);
    on_message(sim, id, &env);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::execute;
    use crate::group::testkit::{INFANTRY, mv, rules, spawn, state, tile_center};
    use crate::group::{NO_GROUP, find};

    #[test]
    fn retrying_out_of_the_formation_chase_heads_for_the_anchor_and_leaves_the_group() {
        // re/groups-formations 12.4: `restore_state == 0xB` without a goal -> HighPath = group anchor, leave,
        // previous state 2, then FINDTO to the (new) last waypoint.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u: Vec<_> = (0..3)
            .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
            .collect();
        execute(&mut sim, &mv(&u, (20, 20), 1, NO_DIRECTION), true);
        let anchor = find(&sim, 0).unwrap().anchor;
        let w = walk_mut(&mut sim, u[0]).unwrap();
        w.restore_state = 0xB;
        let p = w.point(tile_center(30), tile_center(30));
        w.high_path.push_back(p);
        assert!(retry_trapped_with_astar(&mut sim, u[0]));
        let w = walk(&sim, u[0]).unwrap();
        assert_eq!(w.group, NO_GROUP);
        assert!(!find(&sim, 0).unwrap().members.contains(&u[0]));
        let f = w.find_to.as_ref().expect("FINDTO stored");
        assert_eq!((f.arg(0), f.arg(1)), (anchor.0, anchor.1));
        assert_eq!(w.restore_state, 2);
    }

    #[test]
    fn retrying_out_of_the_chase_without_a_group_keeps_the_path() {
        // K-253 (A7): the original crashes without a group; here only `restore_state = 2`.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, INFANTRY, (10, 10));
        let w = walk_mut(&mut sim, a).unwrap();
        (w.restore_state, w.group) = (0xB, 7);
        let p = w.point(tile_center(30), tile_center(30));
        w.high_path.push_back(p);
        assert!(retry_trapped_with_astar(&mut sim, a));
        let w = walk(&sim, a).unwrap();
        let f = w.find_to.as_ref().expect("FINDTO stored");
        assert_eq!((f.arg(0), f.arg(1)), (tile_center(30), tile_center(30)));
        assert_eq!(w.restore_state, 2);
    }
}
