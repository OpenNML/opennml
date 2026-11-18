//! Walk side of the groups and formations (re/groups-formations 2.1, 3.5-3.7, 4.3, 12.7): group
//! queries, rank, formation march (states 5, 10, 0xB), catching up, collision partners.

use super::goto::{on_msg_path_to, stop_moving};
use super::{abort, project::project_trajectory, walk, walk_mut};
use crate::fx::fixed_hypot;
use crate::group::{self, NO_GROUP, march};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{send, send_mut};
use crate::object::status;
use crate::sim::Sim;

/// Formation queries of `PropertyWalk_Query` (`0x0053B5A0`); `None` = not responsible.
pub(crate) fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> Option<i32> {
    match msg.code {
        0x1300 => {
            let num = formation_num(sim, id);
            let v = if num == 0 {
                -1
            } else {
                let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
                group::find(sim, gid).map_or(0, |g| g.type_id())
            };
            msg.set_arg(0, v);
            Some(0)
        }
        0x1500 => Some(walk(sim, id).map_or(NO_GROUP, |w| w.group)),
        0x1600 => {
            set_group(sim, id, msg.arg(0));
            Some(0)
        }
        0x1700 => {
            set_cruising(sim, id, msg.arg(0), msg.arg(1) == 1);
            Some(0)
        }
        0x1200 => Some(on_msg_formation_path(sim, id, msg)),
        0x2000 => Some(formation_num(sim, id)),
        0x2300 => {
            let (x, z, d) = predicted_point(sim, id, msg.arg(0));
            msg.set_arg(0, x);
            msg.set_arg(1, z);
            msg.set_arg(2, d);
            Some(1)
        }
        0x2A00 => Some(on_msg_formation_start(sim, id)),
        0x3A00 => {
            super::set_state(sim, id, 5);
            super::set_status(sim, id, super::WAITING, false);
            super::set_status(sim, id, super::REEVALUATE, true);
            Some(1)
        }
        // `SetLayerNxN` (`0x005326E0`, re/groups-formations 12.7): only a valid layer is accepted. The
        // original checks only ">= 0"; a value >= 9 would be an index beyond the layer table there (A7).
        0xF00 => {
            if let Some(w) = walk_mut(sim, id)
                && let Ok(l) = usize::try_from(msg.arg(0))
                && l < crate::path::LAYER_COUNT
            {
                w.layer = l;
            }
            Some(0)
        }
        _ => None,
    }
}

/// `0x1600 {id}`: set only on change; -1 in state 5/0xB/10 aborts.
pub(crate) fn set_group(sim: &mut Sim, id: ObjectId, gid: i32) {
    let Some(w) = walk_mut(sim, id) else { return };
    if w.group == gid {
        return;
    }
    w.group = gid;
    if gid == NO_GROUP && matches!(w.state, 5 | 0xA | 0xB) {
        abort(sim, id);
    }
}

/// `0x1700 {v, now}` (re/groups-formations 12.7): v = -1 -> `DefaultSpeed`; now in state 2 also `Cur`.
fn set_cruising(sim: &mut Sim, id: ObjectId, v: i32, now: bool) {
    let Some(w) = walk_mut(sim, id) else { return };
    let v = if v == -1 { w.speed.default } else { v };
    w.speed.set_cruising_speed(v);
    if now && w.state == 2 && w.speed.cruising != w.speed.cur {
        let c = w.speed.cruising;
        w.speed.set_cur_speed(c);
        project_trajectory(sim, id, true, false);
    }
}

/// `PropertyWalk_GetFormationNum` (`0x00533A70`): connected (`CARRIING`) and the partner answers
/// `0x2000` != -1 -> its value; otherwise mounted -> 10012, otherwise 10011.
pub(crate) fn formation_num(sim: &mut Sim, id: ObjectId) -> i32 {
    let (st, target) = sim
        .object(id)
        .map_or((0, ObjectId::NONE), |o| (o.status, o.target));
    if st & status::CARRIING != 0 && sim.object(target).is_some() {
        let v = send(sim, target, &Message::new(0x2000, &[]));
        if v != -1 {
            return v;
        }
    }
    walk(sim, id).map_or(0, |w| {
        if st & status::ISMOUNTED != 0 {
            w.formation_num_mounted
        } else {
            w.formation_num
        }
    })
}

/// Group present and a formation (`Find(+0x208)?.vf1C()`).
fn has_formation(sim: &Sim, gid: i32) -> bool {
    group::find(sim, gid).is_some_and(|g| g.formation.is_some())
}

/// `PropertyWalk_GetPredictedTrajectoryPoint` (`0x00534BE0`, `0x2300 {n}`,
/// re/groups-formations 12.7): `n == 0` or trajectory empty -> own grid position and direction;
/// `n > |trajectory|` -> last point; otherwise point `n - 1`. The addition `+0x168` at status
/// `0x5000000` belongs to the siege bits (SiegeEngine; riding does not set them, re/riding 13.5)
/// and is omitted as in `walk::direction` (formations do not accept such units,
/// re/groups-formations 1.7).
fn predicted_point(sim: &Sim, id: ObjectId, n: i32) -> (i32, i32, i32) {
    match walk(sim, id).map(|w| &w.trajectory) {
        Some(t) if n != 0 && !t.is_empty() => {
            // A negative `n` reads before the list in the original; here as `n > |trajectory|` (A7).
            let p = match usize::try_from(n) {
                Ok(k) if k <= t.len() => t.get(k - 1),
                _ => t.back(),
            };
            p.map_or((0, 0, 0), |p| (p.x, p.z, p.dir))
        }
        _ => {
            let (x, z) = super::position(sim, id);
            (x, z, super::direction(sim, id))
        }
    }
}

/// `0x2A00` (`0x00535740`, re/groups-formations 12.7, K-221): without `m_CanMove` abort instead of
/// report; without a group 0; otherwise stop, discard the stored FINDTO (`0x004E31F0(0)`) and report
/// to the formation.
fn on_msg_formation_start(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(w) = walk(sim, id) else { return 0 };
    if !w.can_move {
        abort(sim, id);
        return 0;
    }
    let gid = w.group;
    if gid == NO_GROUP {
        return 0;
    }
    stop_moving(sim, id, false);
    if let Some(w) = walk_mut(sim, id) {
        w.find_to = None;
    }
    if has_formation(sim, gid) {
        march::on_member_report(sim, gid, id);
    } else {
        abort(sim, id);
    }
    1
}

/// `PropertyWalk_OnMsgFormationPath` (`0x0053A340`, `0x1200`, re/groups-formations 12.7): path like
/// PATH without feedback messages and without projection, common speed, state 5; if the unit
/// already stands on the first waypoint, it counts as reached.
fn on_msg_formation_path(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    let env = Envelope::bare(msg.clone());
    let r = on_msg_path_to(sim, id, &env, false, false);
    let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
    // The original reads the group without a NULL check (A7: without a group the speed stays unchanged).
    if let Some(v) = group::find(sim, gid).map(|g| g.matched_speed)
        && let Some(w) = walk_mut(sim, id)
    {
        w.speed.set_cur_speed(v);
    }
    super::set_state(sim, id, 5);
    let pos = super::position(sim, id);
    if walk(sim, id)
        .and_then(|w| w.high_path.front())
        .is_some_and(|h| (h.x, h.z) == pos)
    {
        on_formation_high_point_reached(sim, id);
    }
    project_trajectory(sim, id, false, false);
    i32::from(r)
}

/// `PropertyWalk_OnFormationHighPointReached` (`0x00538180`, re/groups-formations 3.5 step 4, 12.7):
/// on the place in the foremost frame -> state 10 (wait); otherwise state 0xB with 1.5x and catch up at once.
pub(crate) fn on_formation_high_point_reached(sim: &mut Sim, id: ObjectId) {
    let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
    if !has_formation(sim, gid) {
        abort(sim, id);
        return;
    }
    if walk(sim, id).is_some_and(|w| w.state == 0xB)
        && let Some(w) = walk_mut(sim, id)
    {
        w.speed.reset_to_cruising();
    }
    super::set_state(sim, id, 10);
    super::set_status(sim, id, super::WAITING, true);
    if !march::on_member_arrived(sim, gid, id) {
        super::set_state(sim, id, 0xB);
        super::set_status(sim, id, super::WAITING, false);
        if let Some(w) = walk_mut(sim, id) {
            w.speed.raise_to_one_and_half();
        }
        tick_formation_chase(sim, id, true);
    }
}

/// `PropertyWalk_EnterFormationWalk` (`0x00535430`, re/groups-formations 3.6, 12.7): from state 5 on
/// waiting or replanning take out of `ready` and catch up (state 0xB, 1.5x); without a formation abort.
pub(crate) fn enter_formation_walk(sim: &mut Sim, id: ObjectId) -> bool {
    let gid = walk(sim, id).map_or(NO_GROUP, |w| w.group);
    if !has_formation(sim, gid) {
        abort(sim, id);
        return false;
    }
    march::leave(sim, gid, id);
    if let Some(w) = walk_mut(sim, id) {
        w.speed.raise_to_one_and_half();
    }
    super::set_state(sim, id, 0xB);
    if let Some(w) = walk_mut(sim, id) {
        w.ticks_since_chase = 0;
    }
    true
}

/// `PropertyWalk_TickFormationChase(force)` (`0x00536DC0`, re/groups-formations 3.6, 12.7): every
/// 19 ticks (or at once) look for the own place next to the reference; in range -> in lockstep
/// again (state 5), otherwise insert the predicted place as a new waypoint at the front.
pub(crate) fn tick_formation_chase(sim: &mut Sim, id: ObjectId, force: bool) {
    let Some(w) = walk(sim, id) else { return };
    if w.high_path.is_empty() || w.trajectory.len() < 6 {
        return;
    }
    if !force {
        let Some(w) = walk_mut(sim, id) else { return };
        w.ticks_since_chase = w.ticks_since_chase.wrapping_add(1);
        if w.ticks_since_chase < 19 {
            return;
        }
    }
    let Some(w) = walk_mut(sim, id) else { return };
    w.ticks_since_chase = 0;
    let gid = w.group;
    if !has_formation(sim, gid) {
        abort(sim, id);
        return;
    }
    let Some(w) = walk_mut(sim, id) else { return };
    if force {
        w.chase_pos = false;
    } else if w.chase_pos && w.high_path.len() > 1 {
        // `0x00536E5B`-`0x00536E70`: only `CPtrList::RemoveHead` (`0x004083E0`) and free the node;
        // `+0x27C` (`next_hp`) stays. If the function then returns without projection, the follow
        // pointer is orphaned and `TickWalking` (`0x0053AEB0`) falls back to the new head
        // (`tick_walking`); otherwise `ClearTrajectory` sets it to the head.
        w.high_path.pop_front();
        w.chase_pos = false;
    }
    let pos = super::position(sim, id);
    let cur = walk(sim, id).map_or(0, |w| w.speed.cur);
    match march::chase_slot(sim, gid, id, 0) {
        Some(p) if fixed_hypot(pos.0.wrapping_sub(p.0), pos.1.wrapping_sub(p.1)) < cur => {
            if !march::rejoin(sim, gid, id) {
                return;
            }
            super::set_state(sim, id, 5);
            if let Some(w) = walk_mut(sim, id) {
                w.speed.reset_to_cruising();
            }
        }
        _ => {
            let Some(p1) = march::chase_slot(sim, gid, id, 1) else {
                return;
            };
            let Some(w) = walk_mut(sim, id) else { return };
            if w.high_path.front().is_some_and(|h| (h.x, h.z) == p1) {
                return;
            }
            let h = w.point(p1.0, p1.1);
            w.high_path.push_front(h);
            w.phase = 0;
            w.barging = false;
            w.following_bstar = false;
            w.chase_pos = true;
        }
    }
    project_trajectory(sim, id, true, force);
}

/// `PropertyWalk_IsGroupPartnerForCollision` (`0x00539710`, re/groups-formations 4.3, 12.7): group
/// present (vf20 reports 1 for all types), the other reports the same ID (`0x1500`), both layers are
/// 1x1 layers (mask bit `0x01`, layers 0-4). Formations -> partners; type 0 (and 5, which reports 0)
/// only if the current high points are **not** close together (`0x00537D30`).
pub(crate) fn is_group_partner(sim: &mut Sim, me: ObjectId, o: ObjectId) -> bool {
    let gid = walk(sim, me).map_or(NO_GROUP, |w| w.group);
    let Some(g) = group::find(sim, gid) else {
        return false;
    };
    let (g_id, type_id) = (g.id, g.type_id());
    if sim.object(o).is_none() || send(sim, o, &Message::new(0x1500, &[])) != g_id {
        return false;
    }
    let other = send(sim, o, &Message::new(0x1400, &[]));
    let own = walk(sim, me).map_or(-1, |w| w.layer as i32);
    let one_by_one = |l: i32| {
        usize::try_from(l)
            .ok()
            .and_then(|l| crate::path::LAYER_MASKS.get(l))
            .is_some_and(|m| m & 1 != 0)
    };
    if !one_by_one(own) || !one_by_one(other) {
        return false;
    }
    if type_id != 0 {
        return true;
    }
    !next_high_points_close(sim, me, o)
}

/// `PropertyWalk_AreNextHighPointsClose` (`0x00537D30`): own current high point and that of the
/// other (`0xA00`, without a waypoint the grid position) at most R(o) + R(me) (hard radii) apart.
fn next_high_points_close(sim: &mut Sim, me: ObjectId, o: ObjectId) -> bool {
    let mut mine = Message::new(0xA00, &[-1, -1]);
    send_mut(sim, me, &mut mine);
    let mut other = Message::new(0xA00, &[-1, -1]);
    send_mut(sim, o, &mut other);
    let r = |id: ObjectId| sim.object(id).map_or(0, |x| x.hard_radius);
    let d = fixed_hypot(
        other.arg(0).wrapping_sub(mine.arg(0)),
        other.arg(1).wrapping_sub(mine.arg(1)),
    );
    d <= r(o).wrapping_add(r(me))
}

/// Part of `PropertyWalk_OnOwnerChanged(new, old)` (`0x00532C20`, re/groups-formations 1.8, 4.2):
/// group deregistration with the old ID (the object already carries the new one), then the report of
/// the occupant to its carrier (quarters). Death is a change of owner to slot 8; `ready` stays
/// untouched (K-258).
pub(crate) fn on_owner_changed(sim: &mut Sim, new: ObjectId, old: ObjectId) {
    let gid = walk(sim, new).map_or(NO_GROUP, |w| w.group);
    if gid != NO_GROUP {
        group::remove_unit(sim, gid, new, old, false);
    }
    crate::module::quarter::notify_carrier_of_new_id(sim, new, old);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::testkit::{INFANTRY, rules, spawn, state, tile_center};

    fn ask(sim: &mut Sim, id: ObjectId, code: u32, args: &[i32]) -> (i32, Vec<i32>) {
        let mut m = Message::new(code, args);
        let r = send_mut(sim, id, &mut m);
        (r, m.args)
    }

    #[test]
    fn walk_answers_the_group_queries() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, INFANTRY, (5, 5));
        assert_eq!(ask(&mut sim, u, 0x1500, &[]).0, NO_GROUP);
        // 0x2000: rank from 10011; 0x1300 without a group -> 0, rank 0 -> -1 (re/groups-formations 12.7).
        assert_eq!(ask(&mut sim, u, 0x2000, &[]).0, 4);
        assert_eq!(ask(&mut sim, u, 0x1300, &[0]).1[0], 0);
        assert_eq!(ask(&mut sim, u, 0x1600, &[7]).0, 0);
        assert_eq!(ask(&mut sim, u, 0x1500, &[]).0, 7);
        // 0x1700 {-1, 0} -> own base speed as cruising speed.
        let default = walk(&sim, u).unwrap().speed.default;
        ask(&mut sim, u, 0x1700, &[default / 2, 0]);
        assert_eq!(walk(&sim, u).unwrap().speed.cruising, default / 2);
        ask(&mut sim, u, 0x1700, &[-1, 0]);
        assert_eq!(walk(&sim, u).unwrap().speed.cruising, default);
        // 0x1600 {-1} in state 5 aborts (state afterwards no longer 5).
        walk_mut(&mut sim, u).unwrap().state = 5;
        ask(&mut sim, u, 0x1600, &[-1]);
        assert_ne!(walk(&sim, u).unwrap().state, 5);
        assert_eq!(walk(&sim, u).unwrap().group, NO_GROUP);
    }

    #[test]
    fn walk_sets_the_nxn_layer_on_0xf00() {
        // re/groups-formations 12.7 `SetLayerNxN`: a valid layer is accepted, return value 0; -1 and >= 9 are not (A7).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, INFANTRY, (5, 5));
        assert_eq!(ask(&mut sim, u, 0xF00, &[3]).0, 0);
        assert_eq!(walk(&sim, u).unwrap().layer, 3);
        assert_eq!(ask(&mut sim, u, 0xF00, &[-1]).0, 0);
        assert_eq!(ask(&mut sim, u, 0xF00, &[9]).0, 0);
        assert_eq!(walk(&sim, u).unwrap().layer, 3);
        assert_eq!(ask(&mut sim, u, 0x1400, &[]).0, 3);
    }

    #[test]
    fn walk_answers_the_march_queries() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, INFANTRY, (5, 5));
        // 0x2300 {0}: own position and direction.
        let (r, a) = ask(&mut sim, u, 0x2300, &[0, 0, 0]);
        assert_eq!((r, a[0], a[1]), (1, tile_center(5), tile_center(5)));
        // 0x3A00: state 5, WAITING off, REEVALUATE on.
        assert_eq!(ask(&mut sim, u, 0x3A00, &[]).0, 1);
        assert_eq!(walk(&sim, u).unwrap().state, 5);
        let st = sim.object(u).unwrap().status;
        assert_eq!(
            (st & super::super::WAITING, st & super::super::REEVALUATE),
            (0, super::super::REEVALUATE)
        );
        // 0x2A00 without a group -> 0, nothing happens.
        walk_mut(&mut sim, u).unwrap().state = 1;
        assert_eq!(ask(&mut sim, u, 0x2A00, &[]).0, 0);
        assert_eq!(walk(&sim, u).unwrap().state, 1);
        // 0xF00 sets the layer.
        ask(&mut sim, u, 0xF00, &[3]);
        assert_eq!(walk(&sim, u).unwrap().layer, 3);
    }

    fn move_cmd(units: Vec<ObjectId>, formation: i32) -> crate::command::Command {
        crate::command::Command {
            player: 0,
            kind: crate::command::CommandKind::Move {
                units,
                x: tile_center(30),
                z: tile_center(30),
                target: -1,
                formation,
                direction: crate::motion::NO_DIRECTION,
                spread: crate::command::NO_SPREAD,
                flags: 0,
            },
        }
    }

    fn push_high_point(sim: &mut Sim, id: ObjectId, p: (i32, i32)) {
        let w = walk_mut(sim, id).unwrap();
        let h = w.point(p.0, p.1);
        w.high_path.push_back(h);
    }

    #[test]
    fn walk_answers_0xa00_with_the_head_of_the_high_path() {
        // re/walk 2.5/16.4 `GetCurrentHighPoint`: empty -> grid position, return value 0; otherwise head.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, INFANTRY, (5, 5));
        let (r, a) = ask(&mut sim, u, 0xA00, &[-1, -1]);
        assert_eq!((r, a[0], a[1]), (0, tile_center(5), tile_center(5)));
        push_high_point(&mut sim, u, (tile_center(9), tile_center(7)));
        push_high_point(&mut sim, u, (tile_center(20), tile_center(20)));
        let (r, a) = ask(&mut sim, u, 0xA00, &[-1, -1]);
        assert_eq!((r, a[0], a[1]), (1, tile_center(9), tile_center(7)));
        // Without arguments the response creates them.
        assert_eq!(ask(&mut sim, u, 0xA00, &[]).1.len(), 2);
    }

    #[test]
    fn formation_partners_ignore_each_other_on_1x1_layers() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, INFANTRY, (10, 10));
        let b = spawn(&mut sim, INFANTRY, (11, 10));
        let c = spawn(&mut sim, INFANTRY, (12, 10));
        crate::command::execute(&mut sim, &move_cmd(vec![a, b], 1), true);
        assert_eq!(group::find(&sim, 0).map(|g| g.kind), Some(1));
        assert!(is_group_partner(&mut sim, a, b));
        assert!(is_group_partner(&mut sim, b, a));
        assert!(!is_group_partner(&mut sim, a, c), "foreign");
        assert!(!is_group_partner(&mut sim, c, a), "without a group");
        // Query `0x3000 {hid, hard}` (re/collision B4.5): `p3 = hard == 1`.
        assert_eq!(ask(&mut sim, a, 0x3000, &[b.as_i32(), 1]).0, 1);
        assert_eq!(ask(&mut sim, a, 0x3000, &[b.as_i32(), 0]).0, 0);
        assert_eq!(ask(&mut sim, a, 0x3000, &[c.as_i32(), 1]).0, 0);
        // Layer 5 (mask 0x0A, 2x2) is not a 1x1 layer -> no partner, in both directions.
        walk_mut(&mut sim, b).unwrap().layer = 5;
        assert!(!is_group_partner(&mut sim, a, b));
        assert!(!is_group_partner(&mut sim, b, a));
    }

    #[test]
    fn type_0_members_are_partners_only_while_their_high_points_are_apart() {
        // re/groups-formations 4.3/12.7 (`0x00537D30`): partner exactly if hypot(own high point - 0xA00
        // of the other) > R(other) + R(self) (hard radii, here 512 + 512).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, INFANTRY, (10, 10));
        let b = spawn(&mut sim, INFANTRY, (11, 10));
        crate::command::execute(&mut sim, &move_cmd(vec![a, b], 0), true);
        assert_eq!(group::find(&sim, 0).map(|g| g.kind), Some(0));
        let r = sim.object(a).unwrap().hard_radius + sim.object(b).unwrap().hard_radius;
        assert!(r > 0 && r < 0x1000);
        // Without waypoints the grid positions count: one tile apart -> partners.
        assert!(is_group_partner(&mut sim, a, b));
        // High points far apart -> partners.
        push_high_point(&mut sim, a, (tile_center(20), tile_center(10)));
        push_high_point(&mut sim, b, (tile_center(30), tile_center(10)));
        assert!(is_group_partner(&mut sim, a, b));
        // Near high points (distance exactly R(a) + R(b), "<=") -> no partners, in both directions.
        assert_eq!(fixed_hypot(r, 0), r);
        walk_mut(&mut sim, b).unwrap().high_path.clear();
        push_high_point(&mut sim, b, (tile_center(20) + r, tile_center(10)));
        assert!(!is_group_partner(&mut sim, a, b));
        assert!(!is_group_partner(&mut sim, b, a));
        // A bit more distance -> partners again (`Fixed_Hypot` rounds roughly below one tile, hence
        // not just +1).
        let apart = r + 0x100;
        assert!(fixed_hypot(apart, 0) > r);
        walk_mut(&mut sim, b).unwrap().high_path.clear();
        push_high_point(&mut sim, b, (tile_center(20) + apart, tile_center(10)));
        assert!(is_group_partner(&mut sim, a, b));
    }

    #[test]
    fn type_5_reports_type_0_and_uses_the_high_point_rule() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, INFANTRY, (10, 10));
        let b = spawn(&mut sim, INFANTRY, (11, 10));
        crate::command::execute(&mut sim, &move_cmd(vec![a, b], 5), true);
        assert_eq!(group::find(&sim, 0).map(|g| g.kind), Some(5));
        push_high_point(&mut sim, a, (tile_center(20), tile_center(10)));
        push_high_point(&mut sim, b, (tile_center(20), tile_center(10)));
        assert!(!is_group_partner(&mut sim, a, b), "same high point");
    }
}
