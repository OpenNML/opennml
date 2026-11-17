//! Precompute and play back the trajectory (re/walk 4.2-4.4, 4.10): up to 101 points with the
//! movement model, set the head point per object tick and simulate one more point; if
//! the trajectory is blocked, waypoints are shifted away from blocked neighbor tiles or an intermediate point
//! is inserted at a tile center.

use crate::collision::{UnitView, footprint_terrain_cost, tile_cost};
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::Message;
use crate::motion::step::{COST_BLOCKED, COST_OUTSIDE, spread_turn_in_place};
use crate::motion::{NO_DIRECTION, TrajPoint, step, turn_towards};
use crate::path::grid::COST_FREE;
use crate::sim::Sim;
use crate::world::{set_direction, set_grid_position, wrap_angle};

use super::goto::{face_towards_point, is_facing_target_dir, on_target_reached};
use super::{REEVALUATE, direction, position, set_status, walk, walk_mut};

/// `PropertyWalk_ClearTrajectory` (`0x00533DA0`): trajectory empty, next waypoint = list head.
pub(crate) fn clear_trajectory(sim: &mut Sim, id: ObjectId) {
    if let Some(w) = walk_mut(sim, id) {
        w.trajectory.clear();
        w.next_hp = w.high_path.front().map(|p| p.id);
        w.following_bstar = false;
    }
}

/// `PropertyWalk_IsAtFinalGoal` (`0x00534A70`): no waypoint left, at the last point and with the
/// required facing direction.
pub(crate) fn is_at_final_goal(sim: &Sim, id: ObjectId, p: &TrajPoint) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    w.next_hp.is_none()
        && w.high_path.back().is_none_or(|t| (t.x, t.z) == (p.x, p.z))
        && ((w.target_dir == NO_DIRECTION && w.face_point.0 < 0)
            || is_facing_target_dir(sim, id, p.dir))
}

/// Current state of the unit as a trajectory point.
pub(crate) fn current_point(sim: &Sim, id: ObjectId) -> TrajPoint {
    let (x, z) = position(sim, id);
    let w = walk(sim, id);
    TrajPoint {
        x,
        z,
        dir: direction(sim, id),
        mode: 0,
        sense: 0,
        inside: w.is_some_and(|w| w.in_obstacle),
        speed: w.map_or(0, |w| w.speed.cur),
    }
}

/// `PropertyWalk_ProjectTrajectory(multi, keep_last)` (`0x005369A0`): trajectory from the current
/// position anew, up to 101 points; `multi` allows shifting waypoints. `true` if
/// the goal is reached or there are points.
pub(crate) fn project_trajectory(
    sim: &mut Sim,
    id: ObjectId,
    multi: bool,
    keep_last: bool,
) -> bool {
    set_status(sim, id, REEVALUATE, true);
    let pos = position(sim, id);
    if let Some(w) = walk_mut(sim, id)
        && let Some(&h) = w.high_path.front()
        && (h.x, h.z) == pos
    {
        if w.next_hp == Some(h.id) {
            w.next_hp = w.high_path.get(1).map(|p| p.id);
        }
        w.high_path.pop_front();
    }
    clear_trajectory(sim, id);
    let start = current_point(sim, id);
    if let Some(w) = walk_mut(sim, id) {
        w.trajectory.push_front(start);
        w.ende = 1;
    }
    if is_at_final_goal(sim, id, &start) {
        if let Some(w) = walk_mut(sim, id) {
            w.ende = 0;
        }
        super::goto::clear_high_path(sim, id);
    }
    loop {
        let Some(w) = walk(sim, id) else { return false };
        if w.ende != 1 || w.trajectory.len() > 100 {
            break;
        }
        let tail = *w.trajectory.back().expect("start point");
        let mut np = TrajPoint::default();
        let ende = take_one_step(sim, id, &tail, &mut np);
        if let Some(w) = walk_mut(sim, id) {
            w.ende = ende;
        }
        match ende {
            0 | 1 => {
                if let Some(w) = walk_mut(sim, id) {
                    w.trajectory.push_back(np);
                }
            }
            2 => {
                if !multi {
                    if let Some(w) = walk_mut(sim, id) {
                        w.ende = 3;
                    }
                } else {
                    project_waypoints(sim, id, (np.x, np.z));
                    if walk(sim, id).is_some_and(|w| w.ende == 1) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let dir = direction(sim, id);
    let Some(w) = walk_mut(sim, id) else {
        return false;
    };
    if !keep_last
        && w.trajectory
            .front()
            .is_some_and(|h| (h.x, h.z) == pos && h.dir == dir)
    {
        w.trajectory.pop_front();
    }
    w.ende == 0 || !w.trajectory.is_empty()
}

/// `PropertyWalk_TakeOneStep(from, out)` (`0x005357C0`): one point toward the next waypoint; without a
/// waypoint only turn in place toward the target direction. 0 = goal, 1 = continue, 2 = blocked.
pub(crate) fn take_one_step(
    sim: &mut Sim,
    id: ObjectId,
    from: &TrajPoint,
    out: &mut TrajPoint,
) -> i32 {
    *out = *from;
    let Some(w) = walk(sim, id) else { return 4 };
    let Some(i) = w.index_of(w.next_hp) else {
        if w.face_point.0 != -1 {
            face_towards_point(sim, id, (from.x, from.z, from.dir));
        }
        let Some(w) = walk_mut(sim, id) else { return 4 };
        if w.target_dir == from.dir {
            // Quirk: the same direction is adjusted by one.
            w.target_dir = wrap_angle(from.dir.wrapping_add(1));
        }
        out.dir = turn_towards(
            from.dir,
            wrap_angle(w.target_dir.wrapping_sub(from.dir)),
            w.speed.spin,
            0,
        );
        out.mode = 2;
        return if is_facing_target_dir(sim, id, out.dir) {
            0
        } else {
            1
        };
    };
    let hp = w.high_path[i];
    let mut speed = w.speed;
    let layer = w.layer_1x1();
    let r = step(
        out,
        (hp.x, hp.z),
        &mut speed,
        1,
        &mut UnitView {
            sim,
            unit: Some(id),
            layer,
        },
    );
    let Some(w) = walk_mut(sim, id) else { return 4 };
    w.speed = speed;
    if speed.instant_turn && out.mode == 4 {
        // Distribute the instant turn over the already projected points.
        w.trajectory.push_back(*out);
        let last = w.trajectory.len() - 1;
        spread_turn_in_place(&mut w.trajectory, Some(last), speed.turn);
        *out = w.trajectory.pop_back().expect("just appended");
    }
    match r {
        0 => 1,
        1 => 2,
        2 => {
            w.next_hp = w.high_path.get(i + 1).map(|p| p.id);
            if is_at_final_goal(sim, id, out) { 0 } else { 1 }
        }
        _ => 4,
    }
}

/// `PropertyWalk_StepAlongPath(point)` (`0x0053A3F0`): check the point and set position/direction
/// directly. 0 = set, 1 = blocked, 2 = suspended (collision predicted).
pub(crate) fn step_along_path(sim: &mut Sim, id: ObjectId, p: &TrajPoint) -> i32 {
    let Some(w) = walk(sim, id) else { return 1 };
    let (layer, in_obstacle) = (w.layer_1x1(), w.in_obstacle);
    let c = footprint_terrain_cost(sim, p.x, p.z, id, layer);
    if in_obstacle {
        if c == COST_OUTSIDE {
            return 1;
        }
    } else {
        if c >= COST_BLOCKED {
            return 1;
        }
        if super::collide::check_soft_collisions_ahead(sim, id) {
            return 2;
        }
    }
    set_grid_position(sim, id, FxVec3::new(Fx(p.x), Fx::ZERO, Fx(p.z)), true);
    set_direction(sim, id, p.dir);
    let mut formation_point = false;
    if let Some(w) = walk_mut(sim, id)
        && let Some(&h) = w.high_path.front()
        && (h.x, h.z) == (p.x, p.z)
    {
        // Quirk: the follow pointer is cleared, not set to the next point.
        if w.next_hp == Some(h.id) {
            w.next_hp = None;
        }
        w.high_path.pop_front();
        formation_point = matches!(w.state, 5 | 0xB);
    }
    // Formation march (re/groups-formations 3.5 step 4, 12.7): waypoint reached -> check the place in the frame.
    if formation_point {
        super::formation::on_formation_high_point_reached(sim, id);
    }
    if p.mode == 1 {
        // End of a B* trajectory.
        if let Some(w) = walk_mut(sim, id) {
            w.barging = false;
            w.following_bstar = false;
            if w.speed.cur == 10 {
                let d = w.speed.default;
                w.speed.set_cur_speed(d);
            }
        }
        set_status(sim, id, REEVALUATE, true);
    }
    let (x, z) = position(sim, id);
    if let Some(w) = walk_mut(sim, id)
        && w.home.0 != -1
        && w.mobile_home
    {
        w.home = (x, z, NO_DIRECTION);
    }
    0
}

/// `PropertyWalk_TickWalking` (`0x0053AEB0`, states 2/5/0xB).
pub(crate) fn tick_walking(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk_mut(sim, id) else { return };
    // An orphaned follow pointer jumps to the list head (empty counts as valid).
    if let Some(n) = w.next_hp
        && !w.high_path.iter().any(|p| p.id == n)
    {
        w.next_hp = w.high_path.front().map(|p| p.id);
    }
    let repath = Message::new(0x1000, &[-1]);
    let Some(&head) = w.trajectory.front() else {
        if w.ende != 2 && w.ende != 3 {
            on_target_reached(sim, id);
        } else {
            super::query(sim, id, &mut repath.clone());
        }
        return;
    };
    match step_along_path(sim, id, &head) {
        1 => {
            log::debug!("Walk {id:?}: DoWalk GoTo failed");
            super::query(sim, id, &mut repath.clone());
        }
        0 => {
            let Some(w) = walk_mut(sim, id) else { return };
            let Some(&tail) = w.trajectory.back() else {
                return;
            };
            w.trajectory.pop_front();
            if w.ende == 1 && w.trajectory.len() < 100 {
                let last = w.trajectory.back().copied().unwrap_or(tail);
                let mut np = TrajPoint::default();
                let ende = take_one_step(sim, id, &last, &mut np);
                if let Some(w) = walk_mut(sim, id) {
                    w.ende = ende;
                    w.trajectory.push_back(np);
                }
            }
            let Some(w) = walk(sim, id) else { return };
            if w.ende == 2
                && w.state != 5
                && !w.following_bstar
                && let Some(t) = w.trajectory.back()
            {
                let t = (t.x, t.z);
                project_waypoints(sim, id, t);
            }
        }
        _ => {}
    }
}

/// `PropertyWalk_ProjectWaypoints(p)` (`0x00535F10`, re/walk 4.10): if the trajectory is blocked at point
/// `p`, shift the current waypoint by a quarter tile away from blocked neighbor tiles (in the NxN layer);
/// if that does not help, insert an intermediate point at a tile center. `true` if
/// no longer blocked afterwards.
pub(crate) fn project_waypoints(sim: &mut Sim, id: ObjectId, p: (i32, i32)) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    let Some(node) = w.index_of(w.next_hp) else {
        return false;
    };
    let node_id = w.high_path[node].id;
    let saved = w.high_path[node];
    let (nxn, l1) = (w.layer, w.layer_1x1());
    let r = sim.object(id).map_or(0, |o| o.hard_radius);
    let (tpx, tpz) = (p.0 >> 12, p.1 >> 12);
    let (hx, hz) = (saved.x >> 12, saved.z >> 12);
    let (cx, cz) = ((hx << 12) + 0x800, (hz << 12) + 0x800);
    const C: i32 = 0x1000;
    let cost = |sim: &Sim, x: i32, z: i32, l: usize| tile_cost(sim, x, z, l) != 0;
    if cost(sim, p.0, p.1, l1) {
        set_ende(sim, id, 3);
        return false;
    }
    let (mut a, mut b) = (0i32, 0i32);
    let (minus_x, plus_x) = (tpx != (p.0 - r) >> 12, tpx != (p.0 + r) >> 12);
    let (minus_z, plus_z) = (tpz != (p.1 - r) >> 12, tpz != (p.1 + r) >> 12);
    if minus_x {
        if minus_z && cost(sim, p.0 - C, p.1 - C, nxn) {
            a = 1;
            b = 1;
        }
        if plus_z && cost(sim, p.0 - C, p.1 + C, nxn) {
            if a > -1 {
                a -= 1;
            }
            b = 1;
        } else if b < 1 && cost(sim, p.0 - C, p.1, nxn) {
            b = 1;
        }
    }
    if plus_x {
        if minus_z && cost(sim, p.0 + C, p.1 - C, nxn) {
            if a < 1 {
                a += 1;
            }
            if b > -1 {
                b -= 1;
            }
        }
        let mut mid = true;
        if plus_z && cost(sim, p.0 + C, p.1 + C, nxn) {
            if a > -1 {
                a -= 1;
            }
            if b > -1 {
                b -= 1;
            } else {
                mid = false;
            }
        }
        if mid && b > -1 && cost(sim, p.0 + C, p.1, nxn) {
            b -= 1;
        }
    }
    if a < 1 && minus_z && cost(sim, p.0, p.1 - C, nxn) {
        a += 1;
    }
    if a > -1 && plus_z && cost(sim, p.0, p.1 + C, nxn) {
        a -= 1;
    }
    if a == 0 && b == 0 {
        set_ende(sim, id, 3);
        return false;
    }
    if a == 0 {
        a = (hz - tpz).signum();
    } else if b == 0 {
        b = (hx - tpx).signum();
    }
    let mut hp = saved;
    let mut moved = false;
    if a == 1 && hp.z <= cz {
        hp.z = cz + 0x400;
        moved = true;
    } else if a == -1 && hp.z >= cz {
        hp.z = cz - 0x400;
        moved = true;
    }
    if b == 1 && hp.x <= cx {
        hp.x = cx + 0x400;
        moved = true;
    } else if b == -1 && hp.x >= cx {
        hp.x = cx - 0x400;
        moved = true;
    }
    if moved {
        set_point(sim, id, node_id, (hp.x, hp.z));
        let at_pos = (hp.x, hp.z) == position(sim, id);
        log::debug!("Walk {id:?}: Waypoint shifted to ({}, {})", hp.x, hp.z);
        if !project_trajectory(sim, id, false, false) {
            if at_pos {
                if let Some(w) = walk_mut(sim, id) {
                    let p = w.point(saved.x, saved.z);
                    w.high_path.push_front(p);
                }
            } else {
                set_point(sim, id, node_id, (saved.x, saved.z));
                hp = saved;
            }
            project_trajectory(sim, id, false, false);
            set_ende(sim, id, 3);
        }
    }
    let Some(w) = walk(sim, id) else { return false };
    let blocked = w.ende == 2 || w.ende == 3;
    let at_node = w
        .index_of(w.next_hp)
        .is_some_and(|i| (w.high_path[i].x, w.high_path[i].z) == (hp.x, hp.z));
    if blocked && at_node {
        let center = |t: i32| (t << 12) + 0x800;
        let mut q = (hp.x, hp.z);
        if tpz < hz {
            if a == -1 {
                q = (center(hx), center(tpz));
            } else if b != 0 {
                q = (center(tpx), center(hz));
            }
        } else if a == 1 {
            q = (center(hx), center(tpz));
        } else if b != 0 {
            q = (center(tpx), center(hz));
        }
        let qt = (q.0 >> 12, q.1 >> 12);
        let node = w.high_path.iter().position(|p| p.id == node_id);
        let prev_differs = node.is_some_and(|n| {
            n == 0 || {
                let pr = w.high_path[n - 1];
                (pr.x, pr.z) != q
            }
        });
        if let Some(n) = node
            && qt != (hx, hz)
            && sim.state.path.cell_cost(qt.0, qt.1, l1) == COST_FREE
            && q != position(sim, id)
            && prev_differs
        {
            let Some(w) = walk_mut(sim, id) else {
                return false;
            };
            let ins = w.point(q.0, q.1);
            w.high_path.insert(n, ins);
            w.next_hp = Some(ins.id);
            project_trajectory(sim, id, false, false);
            if walk(sim, id).is_some_and(|w| w.ende == 3) {
                if let Some(w) = walk_mut(sim, id) {
                    w.next_hp = Some(node_id);
                    w.high_path.retain(|p| p.id != ins.id);
                }
                if !project_trajectory(sim, id, false, false) {
                    let start = current_point(sim, id);
                    if let Some(w) = walk_mut(sim, id) {
                        w.trajectory.push_front(start);
                    }
                }
            } else {
                log::debug!("Walk {id:?}: Added waypoint ({}, {})", q.0, q.1);
            }
        }
    }
    if walk(sim, id).is_some_and(|w| w.ende == 2) {
        set_ende(sim, id, 3);
        return false;
    }
    true
}

fn set_ende(sim: &mut Sim, id: ObjectId, ende: i32) {
    if let Some(w) = walk_mut(sim, id) {
        w.ende = ende;
    }
}

fn set_point(sim: &mut Sim, id: ObjectId, point: u32, xz: (i32, i32)) {
    if let Some(w) = walk_mut(sim, id)
        && let Some(p) = w.high_path.iter_mut().find(|p| p.id == point)
    {
        (p.x, p.z) = xz;
    }
}
