//! Collision handling between units (re/collision part B): process the reported records (hard and
//! soft map) per tick and decide who waits, overtakes, evades or replans. Prediction runs over the
//! current trajectories in lockstep.

use serde::{Deserialize, Serialize};

use crate::collision::records::PairKey;
use crate::collision::{is_collision_exempt, pair_overlap_cost};
use crate::fx::fixed_hypot;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::motion::step::COST_COLLISION;
use crate::object::status;
use crate::sim::Sim;
use crate::world::wrap_angle;

use super::project::project_trajectory;
use super::{
    REEVALUATE, WAITING, abort, direction, position, set_state, set_status, walk, walk_mut,
};

/// Reported pairs (`CMap<(HID, HID), bool>`, 17 buckets, re/collision B3.1): bucket
/// `(smaller ID >> 4) mod 17`, new entries at the front of the chain; traversed bucket 0...16.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairMap {
    pub buckets: Vec<Vec<PairKey>>,
}

impl PairMap {
    /// `PairFlagMap_GetOrInsert` (`0x005397F0`).
    pub fn insert(&mut self, key: PairKey) {
        if self.buckets.is_empty() {
            self.buckets = vec![Vec::new(); 17];
        }
        let b = ((key.0 as u32 >> 4) % 17) as usize;
        if !self.buckets[b].contains(&key) {
            self.buckets[b].insert(0, key);
        }
    }

    /// Entries in traversal order (`PairFlagMap_GetNextAssoc` `0x00533BE0`).
    pub fn keys(&self) -> Vec<PairKey> {
        self.buckets.iter().flatten().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.buckets.clear();
    }
}

fn record(sim: &Sim, key: PairKey) -> Option<crate::collision::records::Record> {
    sim.state.collision_records.records.get(&key).copied()
}

fn set_record(sim: &mut Sim, key: PairKey, f: impl FnOnce(&mut crate::collision::records::Record)) {
    if let Some(r) = sim.state.collision_records.records.get_mut(&key) {
        f(r);
    }
}

fn status_of(sim: &Sim, id: ObjectId) -> u32 {
    sim.object(id).map_or(0, |o| o.status)
}

/// Report of a record (`0x800 {lo, hi, hard}`, `PropertyWalk_OnMsgCollisionRecord`
/// `0x0053AB40`).
pub(crate) fn on_msg_collision_record(sim: &mut Sim, id: ObjectId, msg: &Message) -> bool {
    let key = (msg.arg(0), msg.arg(1));
    if let Some(w) = walk_mut(sim, id) {
        if msg.arg(2) != 0 {
            w.hard_pairs.insert(key);
        } else {
            w.soft_pairs.insert(key);
        }
    }
    true
}

/// `PropertyWalk_ProcessCollisions` (`0x0053C9D0`, every tick): first the hard, then the soft map;
/// abort as soon as the own unit waits/plans (`0x10`); afterwards both empty.
pub(crate) fn process_collisions(sim: &mut Sim, id: ObjectId) {
    for hard in [true, false] {
        let keys = walk(sim, id).map_or_else(Vec::new, |w| {
            if hard {
                w.hard_pairs.keys()
            } else {
                w.soft_pairs.keys()
            }
        });
        for key in keys {
            if status_of(sim, id) & WAITING != 0 {
                break;
            }
            if record(sim, key).is_some() {
                handle_collision(sim, id, key);
            }
        }
        if let Some(w) = walk_mut(sim, id) {
            if hard {
                w.hard_pairs.clear();
            } else {
                w.soft_pairs.clear();
            }
        }
    }
}

/// `PropertyWalk_HandleCollision` (`0x0053C700`).
fn handle_collision(sim: &mut Sim, id: ObjectId, key: PairKey) {
    let (hi, lo) = (ObjectId::from_i32(key.1), ObjectId::from_i32(key.0));
    let (Some(b), Some(a)) = (sim.object(hi), sim.object(lo)) else {
        set_record(sim, key, |r| r.state = 2);
        return;
    };
    let (bs, as_, bt, at) = (b.status, a.status, b.target, a.target);
    if send(sim, hi, &Message::new(0x2400, &[])) == 1
        || send(sim, lo, &Message::new(0x2400, &[])) == 1
    {
        log::debug!("Walk {id:?}: Ignoring collision, one of us is in an obstacle");
        set_record(sim, key, |r| r.state = 2);
        return;
    }
    if (bs | as_) & 0xA000 != 0
        || (bs & status::CARRIING != 0 && bt == lo)
        || (as_ & status::CARRIING != 0 && at == hi)
    {
        set_record(sim, key, |r| r.state = 2);
        return;
    }
    let o = if hi == id { lo } else { hi };
    let moving = |s: u32| s & status::MOVE != 0 && s & WAITING == 0;
    let idle = |s: u32| s & status::MOVE == 0 && s & WAITING == 0;
    let (ms, os) = (status_of(sim, id), status_of(sim, o));
    if moving(ms) {
        if moving(os) {
            move_move(sim, id, key, o);
        } else {
            move_stand(sim, id, key, o);
        }
    } else if idle(ms) && idle(os) {
        stand_stand(sim, id, key, o);
    }
}

/// `GameObject_CompareCollisionPriority` (`0x0046FD70`): <= 1 = I decide. Order: mobile, idle,
/// smaller hard radius, smaller ID.
fn compare(sim: &mut Sim, me: ObjectId, o: ObjectId) -> u8 {
    let a = send(sim, me, &Message::new(0x2800, &[])) == 1;
    let b = send(sim, o, &Message::new(0x2800, &[])) == 1;
    if a != b {
        return if b { 3 } else { 0 };
    }
    let (Some(m), Some(x)) = (sim.object(me), sim.object(o)) else {
        return 0;
    };
    let (ia, ib) = (m.status & 0xFFFE_0FFF == 0, x.status & 0xFFFE_0FFF == 0);
    if ia != ib {
        return if ia { 0 } else { 3 };
    }
    if m.hard_radius == x.hard_radius {
        return if me.as_i32() > o.as_i32() { 2 } else { 1 };
    }
    if m.hard_radius <= x.hard_radius { 0 } else { 3 }
}

/// `PropertyWalk_CanOvertake(X, Y)` (`0x00533090`): faster and at most 23 degrees difference in direction.
fn can_overtake(sim: &Sim, x: ObjectId, y: ObjectId) -> bool {
    let cur = |u| walk(sim, u).map_or(0, |w| w.speed.cur);
    cur(x) > cur(y)
        && wrap_angle(direction(sim, x).wrapping_sub(direction(sim, y))).wrapping_abs() <= 23 << 12
}

/// `PropertyWalk_ShouldIgnoreCollisionWith(o, p3, chase)` (`0x0053AB80`, re/collision B4.5): in an
/// obstacle, barging against the evade target or a non-enemy, working together, with `chase` chase
/// partner, quarters partner, with `p3` group partner (`IsGroupPartnerForCollision`
/// `0x00539710`, re/groups-formations 12.7). `move_move`/`move_stand` pass `p3 = true` (`0x0053C2C0`,
/// `0x0053C4BC`), the query `0x3000` `p3 = hard`; all callers pass `chase = true`.
fn should_ignore(sim: &mut Sim, me: ObjectId, o: ObjectId, p3: bool, chase: bool) -> bool {
    let Some(w) = walk(sim, me) else { return false };
    if w.in_obstacle {
        return true;
    }
    let enemy = sim
        .state
        .diplomacy
        .is_enemy(i32::from(me.slot()), i32::from(o.slot()));
    if w.barging && (w.hid_to_escape == o || !enemy) {
        return true;
    }
    let (ms, os) = (status_of(sim, me), status_of(sim, o));
    if me.slot() == o.slot() && ms & 0x6E8 != 0 && os & 0x6E8 != 0 {
        return true;
    }
    // `PropertyWalk_IsChasePartner` (`0x00533360`): I chase `o` or `o` chases me.
    if chase
        && (walk(sim, me).is_some_and(|w| w.quarry == o)
            || send(sim, o, &Message::new(0x1F00, &[])) == me.as_i32())
    {
        return true;
    }
    send(sim, me, &Message::new(0x2317, &[o.as_i32()])) == 1
        || send(sim, o, &Message::new(0x2317, &[me.as_i32()])) == 1
        || (p3 && super::formation::is_group_partner(sim, me, o))
}

/// `PropertyWalk_RequestRepathAround(record, X, Y, hard)` (`0x00533130`): if X ignores the pair, the
/// record counts as ignored (3); otherwise X replans (for a hard record around Y).
fn request_repath(sim: &mut Sim, key: PairKey, x: ObjectId, y: ObjectId, hard: bool) -> bool {
    let r = send(
        sim,
        x,
        &Message::new(0x3000, &[y.as_i32(), i32::from(hard)]),
    );
    if r != 0 {
        set_record(sim, key, |rec| rec.state = 3);
        return false;
    }
    let avoid = if record(sim, key).is_some_and(|rec| rec.hard) {
        y.as_i32()
    } else {
        -1
    };
    send(sim, x, &Message::new(0x1000, &[avoid]));
    true
}

/// Query `0x3000 {hid, hard}`: -1 if the object is missing, otherwise "ignore?" with `p3 = hard == 1`
/// (re/collision B4.5 `RequestRepathAround`).
pub(crate) fn query_should_ignore(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    let o = ObjectId::from_i32(msg.arg(0));
    if sim.object(o).is_none() {
        return -1;
    }
    i32::from(should_ignore(sim, id, o, msg.arg(1) == 1, true))
}

fn trajectory_len(sim: &Sim, id: ObjectId) -> usize {
    walk(sim, id).map_or(0, |w| w.trajectory.len())
}

fn point(sim: &Sim, id: ObjectId, i: usize) -> (i32, i32) {
    walk(sim, id).map_or((0, 0), |w| (w.trajectory[i].x, w.trajectory[i].z))
}

/// End marker (state 1) of a future trajectory point: the trajectory ends there (clipping).
fn mark_end(sim: &mut Sim, id: ObjectId, i: usize) {
    if let Some(w) = walk_mut(sim, id) {
        w.trajectory[i].mode = 1;
    }
}

/// `GameObject_PredictCollisionWith(me, o, without o trajectory, only first point)` (`0x0046E850`):
/// do the trajectories cross (in lockstep, up to the shorter one) or does a trajectory hit the
/// fixed position of the other? With a B* trajectory (`0x3400`, quirk: both queried at me) a later
/// collision is not reported, instead the trajectory is clipped there.
fn predict(sim: &mut Sim, me: ObjectId, o: ObjectId, ignore_other: bool, first: bool) -> bool {
    let tm = trajectory_len(sim, me);
    let to = if ignore_other {
        0
    } else {
        trajectory_len(sim, o)
    };
    let mark = send(sim, me, &Message::new(0x3400, &[])) != 0;
    let (mark_me, mark_o) = (mark && tm > 0, mark && to > 0);
    let barging = send(sim, me, &Message::new(0x1E00, &[])) == 1
        || send(sim, o, &Message::new(0x1E00, &[])) == 1;
    let exempt = |sim: &Sim, u: ObjectId| {
        sim.object(u)
            .is_some_and(|x| is_collision_exempt(sim, x, None, false))
    };
    match (tm > 0, to > 0) {
        (false, false) => pair_overlap_cost(sim, o, me, None, None, false, false) >= COST_COLLISION,
        (true, true) => {
            for i in 0..tm.min(to) {
                let (pm, po) = (point(sim, me, i), point(sim, o, i));
                if pair_overlap_cost(sim, o, me, Some(pm), Some(po), false, barging)
                    >= COST_COLLISION
                {
                    let mut res = false;
                    if mark_me {
                        if i == 0 {
                            res = true;
                        } else {
                            mark_end(sim, me, i);
                        }
                    }
                    if mark_o {
                        if i == 0 {
                            res = true;
                        } else {
                            mark_end(sim, o, i);
                        }
                    }
                    return if mark_me || mark_o { res } else { true };
                }
                if first {
                    return false;
                }
            }
            false
        }
        (walker_me, _) => {
            // One walks against the fixed position of the other.
            let (fixed, walker, len, mark) = if walker_me {
                (o, me, tm, mark_me)
            } else {
                (me, o, to, mark_o)
            };
            if exempt(sim, fixed) {
                return false;
            }
            for i in 0..len {
                let p = point(sim, walker, i);
                if pair_overlap_cost(sim, fixed, walker, Some(p), None, false, barging)
                    >= COST_COLLISION
                {
                    if mark && i != 0 {
                        mark_end(sim, walker, i);
                        return false;
                    }
                    return true;
                }
                if first {
                    return false;
                }
            }
            false
        }
    }
}

/// `GameObject_DoPathEndpointsCollide(me, o, without o trajectory)` (`0x0046EB80`): do the
/// endpoints meet (with trajectories of unequal length, the point at the end of the shorter one)?
fn endpoints_collide(sim: &mut Sim, me: ObjectId, o: ObjectId, ignore_other: bool) -> bool {
    if send(sim, me, &Message::new(0x3400, &[])) != 0
        || send(sim, o, &Message::new(0x3400, &[])) != 0
    {
        return false;
    }
    let tm = trajectory_len(sim, me);
    let to = if ignore_other {
        0
    } else {
        trajectory_len(sim, o)
    };
    let done = |sim: &Sim, u: ObjectId| walk(sim, u).is_some_and(|w| w.ende == 0);
    let (pm, po) = match (tm > 0, to > 0) {
        (true, true) => {
            let (end_m, end_o) = (point(sim, me, tm - 1), point(sim, o, to - 1));
            if tm == to {
                (end_m, end_o)
            } else if tm > to {
                (
                    if done(sim, o) {
                        end_m
                    } else {
                        point(sim, me, to - 1)
                    },
                    end_o,
                )
            } else {
                (
                    end_m,
                    if done(sim, me) {
                        end_o
                    } else {
                        point(sim, o, tm - 1)
                    },
                )
            }
        }
        (true, false) => (point(sim, me, tm - 1), position(sim, o)),
        (false, true) => (position(sim, me), point(sim, o, to - 1)),
        (false, false) => (position(sim, me), position(sim, o)),
    };
    let barging = send(sim, me, &Message::new(0x1E00, &[])) == 1
        || send(sim, o, &Message::new(0x1E00, &[])) == 1;
    pair_overlap_cost(sim, o, me, Some(pm), Some(po), false, barging) >= COST_COLLISION
}

fn idle_at_goal(sim: &mut Sim, id: ObjectId) -> bool {
    send(sim, id, &Message::new(0x1D00, &[])) != 0
}

/// `PropertyWalk_HandleMoveMoveCollision` (`0x0053C290`): both walk; only the unit with priority
/// decides.
fn move_move(sim: &mut Sim, me: ObjectId, key: PairKey, o: ObjectId) {
    set_record(sim, key, |r| r.kind = 1);
    if compare(sim, me, o) > 1 {
        return;
    }
    if should_ignore(sim, me, o, true, true) {
        set_record(sim, key, |r| r.state = 3);
        return;
    }
    let Some(r) = record(sim, key) else { return };
    match r.state {
        0 => {
            if r.hard {
                set_record(sim, key, |r| r.state = 1);
                if !idle_at_goal(sim, me) {
                    if request_repath(sim, key, me, o, true) {
                        send(sim, o, &Message::new(0x700, &[me.as_i32()]));
                    }
                } else {
                    request_repath(sim, key, o, me, true);
                }
                return;
            }
            if !predict(sim, me, o, false, false) {
                set_record(sim, key, |r| r.state = 4);
                return;
            }
            resolve_move_move(sim, me, key, o);
        }
        1 => {
            if !predict(sim, me, o, false, false) {
                set_record(sim, key, |r| r.state = 2);
                return;
            }
            set_record(sim, key, |r| r.state = 0);
            send(sim, me, &Message::new(0x2E00, &[]));
        }
        4 if endpoints_collide(sim, me, o, false) => resolve_move_move(sim, me, key, o),
        _ => {}
    }
}

/// `PropertyWalk_ResolveMoveMove` (`0x00536750`): who waits, who overtakes, who replans.
fn resolve_move_move(sim: &mut Sim, me: ObjectId, key: PairKey, o: ObjectId) {
    set_record(sim, key, |r| r.state = 1);
    let wait_for_me = Message::new(0x700, &[me.as_i32()]);
    if predict(sim, me, o, true, false) {
        // Both trajectories meet or I am faster (overtaking): I replan, he waits.
        if predict(sim, o, me, true, false) || can_overtake(sim, me, o) {
            if request_repath(sim, key, me, o, true) {
                send(sim, o, &wait_for_me);
            }
        } else if idle_at_goal(sim, o) {
            request_repath(sim, key, me, o, true);
        } else {
            send(sim, me, &Message::new(0x700, &[o.as_i32()]));
        }
    } else if can_overtake(sim, o, me) {
        if request_repath(sim, key, o, me, false) {
            send(sim, me, &Message::new(0x700, &[o.as_i32()]));
        }
    } else if idle_at_goal(sim, me) {
        request_repath(sim, key, o, me, true);
    } else {
        send(sim, o, &wait_for_me);
    }
}

/// `PropertyWalk_HandleMoveStandCollision` (`0x0053C4A0`): only the walker acts.
fn move_stand(sim: &mut Sim, me: ObjectId, key: PairKey, o: ObjectId) {
    set_record(sim, key, |r| r.kind = 2);
    if should_ignore(sim, me, o, true, true) {
        set_record(sim, key, |r| r.state = 3);
        return;
    }
    let Some(r) = record(sim, key) else { return };
    if r.hard {
        if r.state == 0 {
            if compare(sim, me, o) == 3 {
                // The idle standing unit evades.
                send(sim, o, &Message::new(0x1000, &[me.as_i32()]));
            } else {
                send(sim, me, &Message::new(0x1000, &[o.as_i32()]));
            }
            set_record(sim, key, |r| r.state = 1);
        }
        return;
    }
    match r.state {
        0 => predict_move_stand(sim, me, key, o),
        1 => {
            if predict(sim, me, o, true, false) {
                set_record(sim, key, |r| r.state = 0);
                send(sim, me, &Message::new(0x2E00, &[]));
            } else {
                set_record(sim, key, |r| r.state = 4);
            }
        }
        4 if endpoints_collide(sim, me, o, true) => predict_move_stand(sim, me, key, o),
        _ => {}
    }
}

/// `PropertyWalk_PredictMoveStand` (`0x00539230`): if the last target lies in the standing unit,
/// shift the target to the own position or give up; if the trajectory hits it, replan.
fn predict_move_stand(sim: &mut Sim, me: ObjectId, key: PairKey, o: ObjectId) {
    let mut give_up = false;
    let last_leg = walk(sim, me).is_some_and(|w| w.high_path.len() == 1);
    if last_leg && idle_at_goal(sim, o) {
        let Some(w) = walk(sim, me) else { return };
        let goal = *w.high_path.back().expect("one waypoint");
        let (oh, mh) = (
            sim.object(o).map_or(0, |x| x.hard_radius),
            sim.object(me).map_or(0, |x| x.hard_radius),
        );
        let op = position(sim, o);
        let ov = oh
            .wrapping_sub(fixed_hypot(
                op.0.wrapping_sub(goal.x),
                op.1.wrapping_sub(goal.z),
            ))
            .wrapping_add(mh);
        if ov > 0 {
            let mp = position(sim, me);
            let d2 = fixed_hypot(mp.0.wrapping_sub(goal.x), mp.1.wrapping_sub(goal.z));
            let maxr = w.max_goal_radius;
            if maxr == -1 || (d2 < maxr && d2 < ov) {
                log::debug!("Walk {me:?}: MoveStand shifting target to current pos");
                if let Some(w) = walk_mut(sim, me) {
                    if (goal.x, goal.z) == (w.home.0, w.home.1) {
                        (w.home.0, w.home.1) = mp;
                    }
                    if let Some(t) = w.high_path.back_mut() {
                        (t.x, t.z) = mp;
                    }
                    if let Some(m) = w.find_to.as_mut()
                        && m.args.len() >= 2
                    {
                        (m.args[0], m.args[1]) = mp;
                    }
                }
                project_trajectory(sim, me, true, false);
            } else if maxr <= ov {
                give_up = true;
            }
        }
    }
    if predict(sim, me, o, true, false) {
        if compare(sim, me, o) == 3 && send(sim, o, &Message::new(0x3E00, &[me.as_i32()])) != 0 {
            return;
        }
        if give_up {
            log::debug!("Walk {me:?}: MoveStand giving up, max goal radius isn't sufficient");
            abort(sim, me);
            return;
        }
        let avoid = if walk(sim, me).is_some_and(|w| w.high_path.is_empty()) {
            o.as_i32()
        } else {
            -1
        };
        send(sim, me, &Message::new(0x1000, &[avoid]));
        set_record(sim, key, |r| r.state = 1);
        return;
    }
    set_record(sim, key, |r| r.state = 4);
}

/// `PropertyWalk_HandleStandStandCollision` (`0x00533E30`): two resting, hard-overlapping units;
/// the one with priority plans itself free, its home moves along.
fn stand_stand(sim: &mut Sim, me: ObjectId, key: PairKey, o: ObjectId) {
    let can_move = walk(sim, me).is_some_and(|w| w.can_move);
    if compare(sim, me, o) >= 2 || !can_move || status_of(sim, me) & 0xFFEC_0FFF != 0 {
        return;
    }
    let Some(r) = record(sim, key) else { return };
    if r.kind != 3 {
        set_record(sim, key, |r| {
            r.state = 0;
            r.kind = 3;
        });
    }
    let Some(r) = record(sim, key) else { return };
    if !r.hard {
        set_record(sim, key, |r| r.state = 2);
    } else if r.state == 0 {
        log::debug!("Walk {me:?}: resolving standing collision with {o:?}");
        set_record(sim, key, |r| r.state = 1);
        if let Some(w) = walk_mut(sim, me) {
            w.mobile_home = true;
        }
    }
    if record(sim, key).is_some_and(|r| r.state == 1) {
        set_record(sim, key, |r| r.state = 2);
        send(sim, me, &Message::new(0x1000, &[o.as_i32()]));
    }
}

/// `PropertyWalk_OnMsgWalkWait` (query `0x700 {hid}`, `0x005389E0`): walking -> state 3.
pub(crate) fn on_msg_walk_wait(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    if walk(sim, id).is_some_and(|w| w.state == 5) {
        super::formation::enter_formation_walk(sim, id);
    }
    let moving = status_of(sim, id) & status::MOVE != 0;
    let Some(w) = walk_mut(sim, id) else { return 0 };
    if !moving || (w.state != 2 && w.state != 0xB) {
        return 0;
    }
    w.restore_state = w.state;
    w.state = 3;
    w.wait_for = ObjectId::from_i32(msg.arg(0));
    w.wait_counter = 0;
    set_status(sim, id, WAITING, true);
    1
}

/// `PropertyWalk_TickWaiting` (`0x005359C0`, state 3): wait as long as the other walks and
/// overlaps (also softly), at most 49 ticks; afterwards continue without replanning (`0x800`).
pub(crate) fn tick_waiting(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk_mut(sim, id) else { return };
    w.wait_counter = w.wait_counter.wrapping_add(1);
    let (counter, other) = (w.wait_counter, w.wait_for);
    let head = w.trajectory.front().map(|p| (p.x, p.z));
    let mut gave_up = counter >= 50;
    let other_status = sim.state.object(other, true).map(|o| o.status);
    if !gave_up && let Some(s) = other_status {
        if s & status::MOVE == 0 {
            gave_up = true;
        } else {
            if pair_overlap_cost(sim, other, id, None, None, false, false) > 0 {
                return;
            }
            if let Some(h) = head
                && pair_overlap_cost(sim, other, id, Some(h), None, false, false) > 0
            {
                return;
            }
        }
    }
    if gave_up && status_of(sim, id) & status::MOVE != 0 {
        set_status(sim, id, REEVALUATE, true);
    }
    let restore = walk(sim, id).map_or(1, |w| w.restore_state);
    set_state(sim, id, restore);
    set_status(sim, id, WAITING, false);
    if let Some(w) = walk_mut(sim, id) {
        w.wait_for = ObjectId::NONE;
    }
}

/// `PropertyWalk_CheckSoftCollisionsAhead` (`0x00539090`): if the next point hits a unit of the
/// soft map (only without hard records), the unit pauses this tick.
pub(crate) fn check_soft_collisions_ahead(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(w) = walk(sim, id) else { return false };
    if !w.hard_pairs.is_empty() || w.trajectory.is_empty() {
        return false;
    }
    let keys = w.soft_pairs.keys();
    for key in keys {
        let Some(r) = record(sim, key) else { continue };
        if r.state == 3 {
            continue;
        }
        let other = ObjectId::from_i32(if key.1 == id.as_i32() { key.0 } else { key.1 });
        if sim.state.object(other, true).is_none() {
            continue;
        }
        if predict(sim, id, other, r.kind != 1, true) {
            set_record(sim, key, |r| r.state = 0);
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "collide_tests.rs"]
mod tests;
