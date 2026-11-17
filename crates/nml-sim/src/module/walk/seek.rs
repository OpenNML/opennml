//! Walk combat part (re/combat-attack 7a): FINDTO_UNIT (`0x00539860`), pursuit
//! (`UpdateSeekTarget` `0x00536FD0`), attack position search at the target (`0x00534360`), path to the target
//! (`PlanHighPathToUnit` `0x00534AE0`, push-out `0x005346A0`) and "surrounded" (`0x3100`,
//! state 0xC). Walls (`ONWALL`, PatternWalk) are dead in the original (re/special-movement 4).

use nml_formats::targets::MatchFilter;

use crate::collision::{edge_distance_to_object, edge_distance_to_point, fixed_pos_distance};
use crate::filter::{matches, matches_class, matches_with_cell};
use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::math::{atan2_deg, cos_deg, sin_deg};
use crate::message::{Envelope, Message};
use crate::messaging::{send, send_mut, send_success};
use crate::motion::NO_DIRECTION;
use crate::motion::step::COST_COLLISION;
use crate::object::status;
use crate::path::{LAYER_MASKS, layer_from_move_mask};
use crate::sim::Sim;

use super::goto::{
    apply_move_flags, clear_high_path, clear_seek_target, facing_dir_with_offset, is_beyond_tether,
    is_facing_target_dir, set_facing_tolerance, start_walking, stop_moving,
};
use super::project::{clear_trajectory, project_trajectory};
use super::{
    INDEX, REEVALUATE, WAITING, abort, direction, position, set_state, set_status, walk, walk_mut,
};

/// Minimum width of the filter band (`[0x00672DE0]` = 1/2*sqrt(2) tile, re/combat-attack 7a.1).
pub const MIN_FILTER_BAND: i32 = 2896;
/// Factor 1.5 of the push-out (`[0x00672DE8]`).
pub const PUSH_OUT: i32 = 6144;
/// Target drift from which the pursuit replans (`[0x00672E48]`).
pub const REPLAN_DRIFT: i32 = 4096;

fn hard(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).map_or(0, |o| o.hard_radius.max(0))
}

fn soft(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).map_or(0, |o| o.soft_radius)
}

fn st(sim: &Sim, id: ObjectId) -> u32 {
    sim.object(id).map_or(0, |o| o.status)
}

/// Movement mask of the NxN layer.
fn layer_mask(sim: &Sim, id: ObjectId) -> u32 {
    walk(sim, id)
        .and_then(|w| LAYER_MASKS.get(w.layer).copied())
        .unwrap_or(0)
}

/// `GetQuarryPosition` (`0x005340E0`): building `0xB03` (return value ignored, C-196), otherwise position.
fn quarry_position(sim: &mut Sim, id: ObjectId, t: ObjectId) -> (i32, i32) {
    if sim.object(t).is_some_and(|o| o.has_module(3, false)) {
        let me = position(sim, id);
        let mut m = Message::new(0xB03, &[me.0, me.1, -1, -1]);
        send_mut(sim, t, &mut m);
        return (m.arg(0), m.arg(1));
    }
    position(sim, t)
}

/// `SetSeekTarget` (`0x00536D40`): dead/missing targets -> `None` (everything stays), otherwise quarry,
/// last position, counter 0, escort/attack.
fn set_seek_target(
    sim: &mut Sim,
    id: ObjectId,
    hid: i32,
    escort: bool,
    attack: bool,
) -> Option<ObjectId> {
    let t = ObjectId::from_i32(hid);
    sim.state.object(t, true)?;
    let p = quarry_position(sim, id, t);
    let w = walk_mut(sim, id)?;
    w.quarry = t;
    w.last_known_pos = p;
    w.ticks_since_update = 0;
    w.escort = escort;
    w.attack = attack;
    Some(t)
}

/// Target offset of the walk: (offset, in object space).
fn target_offset(sim: &Sim, id: ObjectId) -> ((i32, i32), bool) {
    walk(sim, id).map_or(((0, 0), false), |w| {
        (w.target_offset, w.offset_in_object_space)
    })
}

/// `GetDistanceToQuarryGoal` (`0x00534D00`): without a target offset the edge distance target to self, otherwise
/// the center distance to the offset point (without radii).
fn distance_to_quarry_goal(sim: &mut Sim, id: ObjectId, t: ObjectId) -> i32 {
    let (off, obj_space) = target_offset(sim, id);
    if off == (0, 0) {
        return edge_distance_to_object(sim, t, id);
    }
    let p = offset_point(sim, t, off, obj_space);
    fixed_pos_distance(position(sim, id), p)
}

/// Offset helpers `0x00401470` or `0x004795C0` (re/combat-attack 7a.4).
fn offset_point(sim: &Sim, t: ObjectId, off: (i32, i32), obj_space: bool) -> (i32, i32) {
    let p = position(sim, t);
    let dir = direction(sim, t);
    if obj_space {
        let a = crate::world::wrap_angle((90 << 12) + dir).wrapping_neg();
        let (c, s) = (cos_deg(a), sin_deg(a));
        (
            fixed_mul(off.0, c) + fixed_mul(off.1, s) + p.0,
            fixed_mul(off.0, s.wrapping_neg()) + fixed_mul(off.1, c) + p.1,
        )
    } else {
        let (c, s) = (cos_deg(dir), sin_deg(dir));
        (
            p.0 - fixed_mul(s, off.1) + fixed_mul(c, off.0),
            p.1 + fixed_mul(c, off.1) + fixed_mul(s, off.0),
        )
    }
}

/// `CHECKLINEOFSIGHT` of the attacker (for `CARRIING` of the carried object).
fn needs_los(sim: &mut Sim, id: ObjectId) -> bool {
    let (s, t) = sim
        .object(id)
        .map_or((0, ObjectId::NONE), |o| (o.status, o.target));
    let who = if s & status::CARRIING != 0 && sim.object(t).is_some() {
        t
    } else {
        id
    };
    matches_class(sim, who, 0x10_0011)
}

/// `IsWithinAttackRange` (`0x00533610`); when attacking with a line-of-sight class (`needs_los`) additionally
/// the line of sight to the prey via the building map (`0x00491C10`).
fn is_within_attack_range(sim: &mut Sim, id: ObjectId, d: i32) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    if d > w.max_attack_radius || d < w.min_attack_radius {
        return false;
    }
    let attack = w.attack;
    if st(sim, id) & status::SWIMMING != 0 && d > 4096 {
        return false;
    }
    if !attack {
        return true;
    }
    if !needs_los(sim, id) {
        return true;
    }
    let quarry = walk(sim, id).map_or(ObjectId::NONE, |w| w.quarry);
    crate::tile_map::has_line_of_sight(sim, id, quarry)
}

/// `IsQuarryTargetable` (`0x00534280`): not diving (an aggressive animal is targetable anyway), not
/// quartered, not dead; without fog.
fn is_quarry_targetable(sim: &mut Sim, id: ObjectId, t: ObjectId) -> bool {
    let mut f = MatchFilter {
        mode: 0x2010,
        status: 0x2_0100,
        ..MatchFilter::constructed()
    };
    if send(sim, id, &Message::new(0xA14, &[])) == 1 {
        f.status &= !0x2_0000;
    }
    matches(sim, t, &f, 0)
}

/// `0x00532AA0`: the target swims or stands on a pure water layer.
fn is_quarry_in_water(sim: &mut Sim, t: ObjectId) -> bool {
    if st(sim, t) & status::SWIMMING != 0 {
        return true;
    }
    let l = send(sim, t, &Message::new(0x1400, &[]));
    usize::try_from(l)
        .ok()
        .and_then(|l| LAYER_MASKS.get(l))
        .is_some_and(|m| m & 0x20 != 0 && m & 0x08 == 0)
}

/// Switch the swim layer off or back on for combat (`+0x288`).
fn set_swim_layer(sim: &mut Sim, id: ObjectId, disallow: bool) {
    let Some(w) = walk_mut(sim, id) else { return };
    let mask = LAYER_MASKS.get(w.layer).copied().unwrap_or(0);
    w.swim_disallowed_for_combat = disallow;
    w.layer = layer_from_move_mask(if disallow { mask & !0x20 } else { mask | 0x20 });
}

/// Filter construction of `FindAttackPositionAtQuarry` (`0x00534360`, re/combat-attack 7a.4): built-in
/// distance check to the attacker with a band (C-197), fog except slot 8, home/tether without
/// `MobileHome`, line-of-sight byte when attacking. Without a walk a filter that never matches.
pub(crate) fn attack_filter(sim: &mut Sim, id: ObjectId, t: ObjectId) -> MatchFilter {
    let Some(w) = walk(sim, id) else {
        // `mode & 0x800_0000`: "never matches".
        return MatchFilter {
            mode: 0x800_0000,
            ..MatchFilter::constructed()
        };
    };
    let (layer, min_r, max_r, mobile_home, home, tether, attack) = (
        w.layer,
        w.min_attack_radius,
        w.max_attack_radius,
        w.mobile_home,
        (w.home.0, w.home.1),
        w.tether,
        w.attack,
    );
    let (rt, rg) = (hard(sim, t), hard(sim, id));
    let owner = i32::from(id.slot());
    let mut f = MatchFilter {
        mode: if owner == 8 { 0x30 } else { 0x200_0030 },
        ref_object: id.as_i32(),
        layer: layer as i32,
        region: -1,
        range_max: max_r.wrapping_add(rt).wrapping_add(rg),
        range_min: if min_r > 0 {
            min_r.wrapping_add(rt).wrapping_add(rg)
        } else {
            min_r
        },
        fog_player: owner,
        ..MatchFilter::constructed()
    };
    if f.range_max.wrapping_sub(f.range_min) <= MIN_FILTER_BAND {
        f.range_max = f.range_min.wrapping_add(MIN_FILTER_BAND + 1); // C-197
    }
    if !mobile_home {
        f.home = home;
        f.tether = tether;
    }
    f.line_of_sight = attack && needs_los(sim, id);
    f
}

/// `FindAttackPositionAtQuarry(t, out)` (`0x00534360`): whether the target fits the attack filter; if so, writes
/// the found cell (or the target offset if the cell is that of the target) to `out`.
pub(crate) fn find_attack_position(
    sim: &mut Sim,
    id: ObjectId,
    t: ObjectId,
    out: Option<&mut (i32, i32)>,
) -> bool {
    let f = attack_filter(sim, id, t);
    let mut cell = None;
    let owner = i32::from(id.slot());
    if !matches_with_cell(sim, t, &f, owner, Some(owner), &mut cell) {
        return false;
    }
    if let Some(o) = out {
        // C-190: without output of the cell search (tether 0, quartered) the last known position.
        *o = cell.unwrap_or_else(|| walk(sim, id).map_or((0, 0), |w| w.last_known_pos));
        let (off, obj_space) = target_offset(sim, id);
        let tp = position(sim, t);
        if (o.0 >> 12, o.1 >> 12) == (tp.0 >> 12, tp.1 >> 12) && off != (0, 0) {
            *o = offset_point(sim, t, off, obj_space);
        }
    }
    true
}

/// `PushOutPoint(p, r)` (`0x005346A0`): point at distance `1.5·r` around `p`, first in the direction
/// quarry -> `p` (or own facing direction), then alternating +-15 degree steps until the region equals the
/// own one; without a hit the 30th candidate (C-191). Without a living quarry `p` stays.
pub(crate) fn push_out_point(sim: &mut Sim, id: ObjectId, p: (i32, i32), r: i32) -> (i32, i32) {
    let Some(q) = walk(sim, id)
        .map(|w| w.quarry)
        .filter(|&q| sim.state.object(q, true).is_some())
    else {
        return p;
    };
    let rr = fixed_mul(r, PUSH_OUT);
    let qp = position(sim, q);
    let mut a = if p != qp {
        // Wrapped like x86: `p` can be the unchecked last known position from the savegame.
        atan2_deg(p.1.wrapping_sub(qp.1), p.0.wrapping_sub(qp.0))
    } else {
        direction(sim, id)
    };
    let mut step = 15 << 12;
    let own = crate::world::region_id_at(sim, id, -1, -1, -1);
    let mut i = 1;
    loop {
        // Wrapped like x86: the radius comes unchecked from the savegame.
        let x = p.0.wrapping_add(fixed_mul(rr, cos_deg(a)));
        let z = p.1.wrapping_add(fixed_mul(rr, sin_deg(a)));
        let ok = x >= 0 && z >= 0 && crate::world::region_id_at(sim, id, -1, x, z) == own;
        a = a.wrapping_add(i * step);
        i += 1;
        step = -step;
        if ok || i > 30 {
            break (x, z);
        }
    }
}

/// `PlanHighPathToUnit` (`0x00534AE0`): dead/missing quarry -> `false`; too close to the attack point
/// (own position, C-192) -> push out; then `GameWorld_PlanHighPath` (PATH synchronous).
fn plan_high_path_to_unit(sim: &mut Sim, id: ObjectId, mut p: (i32, i32), flags: i32) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    let (q, min_r, layer) = (w.quarry, w.min_attack_radius, w.layer);
    if sim.state.object(q, true).is_none() {
        return false;
    }
    let soll = min_r.wrapping_add(hard(sim, id)).wrapping_add(hard(sim, q));
    let from = position(sim, id);
    if fixed_pos_distance(from, p) < soll {
        p = push_out_point(sim, id, p, soll);
    }
    match crate::path::highpath::plan_high_path(&sim.state.path, layer, from, p, false, flags) {
        Ok(path) => {
            send(sim, id, &path.message());
            true
        }
        Err(e) => {
            log::debug!("Walk {id:?}: PlanHighPathToUnit failed ({e:?})");
            false
        }
    }
}

/// `PropertyWalk_OnMsgFindToUnit` (`0x00539860`, re/combat-attack 7a.2): message
/// `[target, min, max (-7 = object space), direction offset, tolerance, flags, offset x, z]`.
pub(crate) fn on_msg_find_to_unit(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let m = &env.msg;
    let f = m.arg(5);
    apply_move_flags(sim, id, env, f & 0x100 != 0, f);
    let Some(t) = set_seek_target(sim, id, m.arg(0), f & 0x2000 != 0, f & 0x40 != 0) else {
        log::debug!("Walk {id:?}: Unit {:#x} doesn't exist", m.arg(0));
        abort(sim, id);
        return false;
    };
    if walk(sim, id).is_some_and(|w| w.attack) {
        if !is_quarry_targetable(sim, id, t) {
            abort(sim, id);
            return false;
        }
        let mask = layer_mask(sim, id);
        if is_quarry_in_water(sim, t)
            && st(sim, id) & status::SWIMMING == 0
            && mask & 0x08 != 0
            && mask & 0x20 != 0
        {
            set_swim_layer(sim, id, true);
        }
    }
    let min = m.arg(1).max(0);
    if let Some(w) = walk_mut(sim, id) {
        w.min_attack_radius = min;
        w.max_attack_radius = min.wrapping_add(1024).max(m.arg(2));
        w.offset_in_object_space = m.arg(2) == -7;
        if m.arg(2) == -7 {
            w.max_attack_radius = 0;
        }
        w.max_goal_radius = 1024;
        w.target_dir_offset = m.arg(3);
        w.target_offset = (m.arg(6), m.arg(7));
        w.target_dir = NO_DIRECTION;
        w.face_point = (-1, -1);
    }
    set_facing_tolerance(sim, id, m.arg(4));
    let d = distance_to_quarry_goal(sim, id, t);
    let last = walk(sim, id).map_or((0, 0), |w| w.last_known_pos);
    let on_wall = (st(sim, id) | st(sim, t)) & status::ONWALL != 0;
    if on_wall && f & 0x800 == 0 {
        log::debug!(
            "Walk {id:?}: wall (ClimbWallForUnit) is dead in the original (re/special-movement 4)"
        );
        return true;
    }
    if is_within_attack_range(sim, id, d) {
        if walk(sim, id).is_some_and(|w| w.escort) {
            return true;
        }
        if !on_wall && !find_attack_position(sim, id, t, None) {
            abort(sim, id); // C-185
            return false;
        }
        let off = walk(sim, id).map_or(0, |w| w.target_dir_offset);
        let me = (position(sim, id), direction(sim, id));
        let dir = if off == NO_DIRECTION {
            direction(sim, t) // C-186
        } else if me == (position(sim, t), direction(sim, t)) {
            me.1
        } else {
            facing_dir_with_offset((me.0.0, me.0.1, me.1), last, off)
        };
        if let Some(w) = walk_mut(sim, id) {
            w.target_dir = dir;
        }
        if is_facing_target_dir(sim, id, me.1) {
            log::debug!("Walk {id:?}: Already close enough ({d}) to {t:?}");
            clear_seek_target(sim, id);
            send_success(sim, id, INDEX);
            return true;
        }
        if walk(sim, id).is_some_and(|w| w.can_move) {
            start_walking(sim, id, true, true);
            return true;
        }
        abort(sim, id);
        return false;
    }
    if st(sim, id) & status::ONWALL != 0 {
        log::debug!(
            "Walk {id:?}: wall (ClimbWallForUnit) is dead in the original (re/special-movement 4)"
        );
        abort(sim, id);
        return false;
    }
    let Some(w) = walk(sim, id) else { return false };
    if !w.can_move {
        abort(sim, id);
        return false;
    }
    if w.home.0 != -1 && w.tether >= 0 && !w.mobile_home {
        let (home, tether, min_r, max_r) = (
            (w.home.0, w.home.1),
            w.tether,
            w.min_attack_radius,
            w.max_attack_radius,
        );
        let e = edge_distance_to_point(sim, t, home) - hard(sim, id);
        let dist = e.max(0);
        if dist.wrapping_sub(tether) > max_r || tether.wrapping_add(dist) < min_r {
            log::debug!("Walk {id:?}: Unit {t:?} is unreachable (tether {tether})");
            abort(sim, id);
            return false;
        }
    }
    if st(sim, id) & status::QUARTERED != 0 {
        log::debug!("Walk {id:?}: Unit {t:?} is unreachable (we are in a building)");
        abort(sim, id);
        return false;
    }
    let mut p = last;
    if !find_attack_position(sim, id, t, Some(&mut p)) {
        p = last;
    }
    if plan_high_path_to_unit(sim, id, p, f) {
        return true;
    }
    log::debug!("Walk {id:?}: Plan path failed in OnMsgFindToUnit!");
    abort(sim, id);
    false
}

/// `PropertyWalk_UpdateSeekTarget(force, reached)` (`0x00536FD0`, re/combat-attack 7a.8): every
/// walk tick with `(false, false)`, at the end of the trajectory with `(true, true)`.
pub(crate) fn update_seek_target(sim: &mut Sim, id: ObjectId, force: bool, reached: bool) {
    if st(sim, id) & status::DIED != 0 {
        clear_seek_target(sim, id);
    }
    let Some(q) = walk(sim, id).map(|w| w.quarry).filter(|q| !q.is_none()) else {
        return;
    };
    if sim.state.object(q, true).is_none() {
        log::debug!("Walk {id:?}: Stopped seeking handle {q:?} because it is no longer valid.");
        abort(sim, id);
        return;
    }
    if !force {
        let n = walk_mut(sim, id).map_or(0, |w| {
            w.ticks_since_update = w.ticks_since_update.wrapping_add(1);
            w.ticks_since_update
        });
        if n < 10
            && fixed_pos_distance(position(sim, q), position(sim, id))
                > soft(sim, id) + soft(sim, q)
        {
            return;
        }
    }
    if let Some(w) = walk_mut(sim, id) {
        w.ticks_since_update = 0;
    }
    let attack = walk(sim, id).is_some_and(|w| w.attack);
    if (st(sim, id) ^ st(sim, q)) & status::ONWALL != 0
        && !(attack && sim.object(id).is_some_and(|o| o.has_module(0x13, false)))
    {
        log::debug!(
            "Walk {id:?}: PatternWalking 0xA2F is dead in the original (re/special-movement 4)"
        );
        return;
    }
    let d = distance_to_quarry_goal(sim, id, q);
    if attack {
        // NEED_INTERVISIBILITY (`0x10000A`) is never set in the shipped data; `0x004527C0` never returns true.
        let disallowed = walk(sim, id).is_some_and(|w| w.swim_disallowed_for_combat);
        let mask = layer_mask(sim, id);
        if is_quarry_in_water(sim, q) {
            if !disallowed && mask & 0x08 != 0 && mask & 0x20 != 0 {
                set_swim_layer(sim, id, true);
            }
        } else if disallowed {
            set_swim_layer(sim, id, false);
        }
    }
    let last = walk(sim, id).map_or((0, 0), |w| w.last_known_pos);
    let mut p = last; // C-190
    let found = find_attack_position(sim, id, q, Some(&mut p));
    if walk(sim, id).is_some_and(|w| w.escort) {
        let qp = quarry_position(sim, id, q);
        let max_r = walk(sim, id).map_or(0, |w| w.max_attack_radius);
        if d > max_r {
            let replan = if !reached {
                fixed_pos_distance(qp, last) > REPLAN_DRIFT
            } else {
                (last.0 >> 12, last.1 >> 12) != (qp.0 >> 12, qp.1 >> 12)
            };
            if replan {
                if let Some(w) = walk_mut(sim, id) {
                    w.last_known_pos = qp;
                }
                clear_high_path(sim, id); // C-190 (re/combat-attack 7a.16 no. 12): without ClearTrajectory
                plan_high_path_to_unit(sim, id, p, 0);
            } else if reached {
                stop_moving(sim, id, true);
            }
        } else if st(sim, id) & status::MOVE != 0 {
            stop_moving(sim, id, true);
        }
        return;
    }
    let targetable = !attack || is_quarry_targetable(sim, id, q);
    let on_wall = (st(sim, id) | st(sim, q)) & status::ONWALL != 0;
    if !targetable || (!on_wall && !found) {
        let follow = walk(sim, id).is_some_and(|w| w.try_to_follow);
        let me = position(sim, id);
        if follow && !reached && !is_beyond_tether(sim, id, me) {
            return; // C-195: tether at the own position
        }
        log::debug!("Walk {id:?}: Stopped seeking handle {q:?} because I can't find it.");
        abort(sim, id);
        return;
    }
    let qp = quarry_position(sim, id, q);
    if is_within_attack_range(sim, id, d) {
        let dir = direction(sim, id);
        if is_facing_target_dir(sim, id, dir) {
            log::debug!("Walk {id:?}: Caught handle {q:?}!");
            send_success(sim, id, INDEX); // C-189: zuerst Erfolg
            clear_seek_target(sim, id);
            stop_moving(sim, id, false);
            return;
        }
        let turning = walk(sim, id).is_some_and(|w| {
            matches!((w.trajectory.front(), w.trajectory.back()),
                (Some(h), Some(t)) if (h.x, h.z) == (t.x, t.z))
        });
        if turning {
            return;
        }
        log::debug!("Walk {id:?}: Reached position to attack handle {q:?}, turning....");
        // C-188: set anew only for NONE; an older direction stays.
        let q_dir = direction(sim, q);
        if let Some(w) = walk_mut(sim, id)
            && w.target_dir == NO_DIRECTION
        {
            if w.target_dir_offset == NO_DIRECTION {
                w.target_dir = q_dir;
            } else {
                w.face_point = qp;
            }
        }
        clear_high_path(sim, id);
        set_state(sim, id, 2);
        if let Some(w) = walk_mut(sim, id) {
            w.wait_counter = 0;
            w.phase = 0;
            w.barging = false;
            w.following_bstar = false;
        }
        project_trajectory(sim, id, true, false);
        return;
    }
    if !reached && fixed_pos_distance(qp, last) <= REPLAN_DRIFT {
        return;
    }
    // C-194: `0xD00` to the target has no effect - omitted.
    if let Some(w) = walk_mut(sim, id) {
        w.last_known_pos = qp;
    }
    clear_high_path(sim, id);
    clear_trajectory(sim, id);
    plan_high_path_to_unit(sim, id, p, 0); // C-193: flags 0, result ignored
    if !reached {
        return;
    }
    let me = position(sim, id);
    let ends_elsewhere =
        walk(sim, id).is_some_and(|w| w.trajectory.back().is_some_and(|t| (t.x, t.z) != me));
    if ends_elsewhere {
        return;
    }
    log::debug!(
        "Walk {id:?}: Stopped seeking handle {q:?} because we're as close as we can get to it, and that's too far."
    );
    abort(sim, id);
}

/// `PropertyWalk_IsQuarrySurrounded(R)` (`0x005338D0`, query `0x3100` at the target `id`): points
/// at distance `R(id) + 1024 + R(R)` all around; if one is free for `R` (cost < `0x1000`), not
/// surrounded. If `R` is missing or has no layer, surrounded.
pub(crate) fn is_quarry_surrounded(sim: &mut Sim, id: ObjectId, r: ObjectId) -> bool {
    if sim.object(r).is_none() {
        return true;
    }
    let layer = send(sim, r, &Message::new(0xB00, &[]));
    let Ok(layer) = usize::try_from(layer) else {
        return true;
    };
    let (rr, ro) = (hard(sim, r), hard(sim, id));
    let dist = ro + 1024 + rr;
    let step = atan2_deg(rr, 2 * dist);
    if step <= 0 {
        return true;
    }
    let n = (360 << 12) / step;
    let p0 = position(sim, id);
    for i in 0..n {
        let a = i * step;
        let p = (
            p0.0 + fixed_mul(dist, cos_deg(a)),
            p0.1 + fixed_mul(dist, sin_deg(a)),
        );
        // Flags (0, 0, 1) of the original; the third has no effect (re/collision A2.5).
        let c = crate::collision::query_cost_at(sim, p.0, p.1, Some(r), layer, false, false);
        if c < COST_COLLISION {
            return false;
        }
    }
    true
}

/// `PropertyWalk_TickQuarrySurrounded` (`0x005353A0`, state 0xC, only from savegames): ask every 17
/// ticks whether the quarry is still surrounded; otherwise keep walking or abort.
pub(crate) fn tick_quarry_surrounded(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk_mut(sim, id) else { return };
    w.wait_counter = w.wait_counter.wrapping_add(1);
    if w.wait_counter < 17 {
        return;
    }
    w.wait_counter = 0;
    let q = w.quarry;
    // `PropertyWalk_QueryQuarrySurrounded` (`0x00532B00`): dead/missing quarry -> not surrounded.
    let surrounded = sim.state.object(q, true).is_some()
        && send(sim, q, &Message::new(0x3100, &[id.as_i32()])) == 1;
    if surrounded {
        return;
    }
    log::debug!("Walk {id:?}: Quarry is no longer surrounded.");
    if st(sim, id) & status::MOVE != 0 {
        let restore = walk(sim, id).map_or(1, |w| w.restore_state);
        set_state(sim, id, restore);
        set_status(sim, id, WAITING, false);
        set_status(sim, id, REEVALUATE, true);
    } else {
        abort(sim, id);
    }
}
