//! Formation march, group side (re/groups-formations 3.5, 12.2, 12.3, 12.9): the leader plans with the
//! largest footprint, frames arise from its path; swing around the inner wing end.

use super::formation::{form, form_mut};
use super::geometry::{mirror, pivot, rot};
use super::{MfcMap, find, find_mut, none};
use crate::fx::fixed_mul as mul;
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::math::atan2_deg;
use crate::message::Message;
use crate::messaging::{post_message, send, send_mut};
use crate::module::walk::walk;
use crate::motion::NO_DIRECTION;
use crate::path::grid::PathGrid;
use crate::path::{LAYER_MASKS, layer_from_move_mask};
use crate::sim::Sim;
use crate::world::{set_grid_position, wrap_angle};

/// 1x1 layer of the class of the group layer (start of `0x00428DA0`: `LayerFromMoveMask(MASK[layer]
/// & ~6 | 1)`). A layer outside the table (also -1) counts as land (A7; the original would read
/// behind the table).
pub fn corner_layer(g_layer: i32) -> usize {
    let mask = usize::try_from(g_layer)
        .ok()
        .and_then(|l| LAYER_MASKS.get(l).copied())
        .unwrap_or(0x09);
    layer_from_move_mask((mask & !6) | 1)
}

/// `GroupFormation_AdjustCornerPoint` (`0x00428DA0`, re/groups-formations 12.2); `m = Mul(corner_margin, spacing)`.
/// Shifts only if exactly one diagonal of the (possibly shifted) cell is blocked, away from the
/// blocked corner; the second attempt `0x00428C10` is a stub (always 0) and is omitted (K-254).
pub fn corner_adjust(grid: &PathGrid, layer: usize, m: i32, p: (i32, i32)) -> Option<(i32, i32)> {
    const Q: i32 = 0x400;
    let c = |x: i32, z: i32| grid.cell_cost(x, z, layer);
    let (cx, cz) = (p.0 >> 12, p.1 >> 12);
    if c(cx, cz) != 0 {
        return None;
    }
    let (mut ox, mut oz) = (cx, cz);
    'shift: {
        if c(cx - 1, cz) == 0 {
            if c(cx + 1, cz) == 0 {
                let zb = if c(cx, cz + 1) == 0 {
                    if c(cx, cz - 1) == 0 {
                        break 'shift;
                    }
                    cz - 1
                } else {
                    cz + 1
                };
                if c(cx + 1, zb) == 0 {
                    ox += 1;
                }
                if c(cx - 1, zb) == 0 {
                    ox -= 1;
                }
            } else {
                if c(cx + 1, cz + 1) == 0 {
                    oz = cz + 1;
                }
                if c(cx + 1, cz - 1) == 0 {
                    oz -= 1;
                }
            }
        } else {
            if c(cx - 1, cz + 1) == 0 {
                oz = cz + 1;
            }
            if c(cx - 1, cz - 1) == 0 {
                oz -= 1;
            }
        }
    }
    let (bx, bz) = (ox << 12, oz << 12);
    let lo = |b: i32| b.wrapping_add(Q).wrapping_sub(m);
    let hi = |b: i32| b.wrapping_add(3 * Q).wrapping_add(m);
    let (sw, se, nw, ne) = (
        c(ox - 1, oz - 1) == 0,
        c(ox + 1, oz - 1) == 0,
        c(ox - 1, oz + 1) == 0,
        c(ox + 1, oz + 1) == 0,
    );
    // Check order of the original: sw first; with sw free and se blocked only "nw and ne free";
    // with sw and se free depending on nw/ne; with sw blocked only "all others free".
    match (sw, se, nw, ne) {
        (true, false, true, true) => Some((lo(bx), hi(bz))),
        (true, false, _, _) => None,
        (true, true, true, true) => None,
        (true, true, true, false) => Some((lo(bx), lo(bz))),
        (true, true, false, true) => Some((hi(bx), lo(bz))),
        (true, true, false, false) => None,
        (false, true, true, true) => Some((hi(bx), hi(bz))),
        (false, _, _, _) => None,
    }
}

/// `GroupFormation_AddFrame` (`0x0042A760`, re/groups-formations 12.3): for each `offsets` entry (CMap iteration)
/// `p + R(dir)·(Mul(lx, sp), Mul(lz, sp))`; frame appended at the back.
pub fn add_frame(sim: &mut Sim, gid: i32, p: (i32, i32), dir: i32) {
    let Some(g) = find_mut(sim, gid) else { return };
    let sp = form(g).spacing;
    let mut frame = MfcMap::new();
    for (k, &(lx, lz)) in g.offsets.iter() {
        frame.set(k, rot(p, (mul(lx, sp), mul(lz, sp)), dir));
    }
    form_mut(g).frames.push_back(frame);
}

/// `GroupFormation_AddTurnFrames` (`0x0042AA40`, re/groups-formations 12.3): a turn of more than 90 degrees ->
/// vf4C and old direction + 180 degrees; frames at the pivot (vf3C), returns the pivot. vf4C mirrors only line,
/// column and pincer (`0x004073C0`: negate lz, swap wing ends); square and type 6 have there
/// `0x004EC990` (nothing, re/groups-formations 1.5 table `+0x4C`).
pub fn add_turn_frames(sim: &mut Sim, gid: i32, p: (i32, i32), old: i32, new: i32) -> (i32, i32) {
    let Some(g) = find_mut(sim, gid) else {
        return p;
    };
    let mut old = old;
    // `wrap_angle` returns [-180 degrees, 180 degrees): `wrapping_abs` does not overflow.
    let mut d = wrap_angle(new.wrapping_sub(old));
    if d.wrapping_abs() > 90 << 12 {
        if matches!(g.kind, 1 | 3 | 4) {
            mirror(&mut g.offsets);
            let f = form_mut(g);
            (f.end_max, f.end_min) = (f.end_min, f.end_max);
        }
        old = wrap_angle(old.wrapping_add(180 << 12));
        d = wrap_angle(new.wrapping_sub(old));
    }
    let f = form(g);
    let v = pivot(
        g.kind,
        &g.offsets,
        (f.end_max, f.end_min),
        f.spacing,
        (p.0, p.1, old),
        d,
    );
    add_frame(sim, gid, v, new);
    v
}

/// Facing direction `GO+0x1C0`. The addition `+0x168` at status `0x5000000` belongs to SiegeEngine
/// and is missing, as in `walk::direction`.
fn facing(sim: &Sim, id: ObjectId) -> i32 {
    crate::module::walk::direction(sim, id)
}

/// `GameObject_SetGridPosition(p, 1)`: keep the height or fit it to the terrain.
fn set_grid_xz(sim: &mut Sim, id: ObjectId, p: (i32, i32)) {
    set_grid_position(sim, id, FxVec3::new(Fx(p.0), Fx::ZERO, Fx(p.1)), true);
}

/// vf14 `GroupFormation_OnLeaderFindTo` (`0x0042A270`, re/groups-formations 12.9): leader to the reference point
/// for planning, layer of the formation. Does not check for rank members: the leader of the
/// rear group also runs through it, gets `offsets` (0, 0) and overwrites the march reference (K-257).
pub fn on_leader_find_to(sim: &mut Sim, gid: i32, id: ObjectId) {
    if !find(sim, gid).is_some_and(|g| g.marching) {
        return;
    }
    let Some(start) = none::grid_pos(sim, id) else {
        return;
    };
    let d = facing(sim, id);
    let Some(g) = find_mut(sim, gid) else { return };
    // `CMap::operator[]`: a missing entry is inserted with (0, 0).
    let o = match g.offsets.get(id.0) {
        Some(&o) => o,
        None => {
            g.offsets.set(id.0, (0, 0));
            (0, 0)
        }
    };
    // First rotate, then spread.
    let v = rot((0, 0), o, d);
    let sp = form(g).spacing;
    let march_ref = (
        start.0.wrapping_sub(mul(v.0, sp)),
        start.1.wrapping_sub(mul(v.1, sp)),
        d,
    );
    let f = form_mut(g);
    f.leader_start = start;
    f.march_ref = march_ref;
    let layer = send(sim, id, &Message::new(0x1400, &[]));
    let Some(g) = find_mut(sim, gid) else { return };
    let f = form_mut(g);
    f.leader_layer = layer;
    let g_layer = f.layer;
    set_grid_xz(sim, id, (march_ref.0, march_ref.1));
    send(sim, id, &Message::new(0xF00, &[g_layer]));
}

/// vf18 `GroupFormation_OnLeaderPath` (`0x0042ABB0`, re/groups-formations 3.5 step 2, 12.3): frames from the path
/// of the leader, then `0x2A00` to all members.
pub fn on_leader_path(sim: &mut Sim, gid: i32, id: ObjectId) -> bool {
    let Some(g) = find(sim, gid) else {
        return false;
    };
    let f = form(g);
    if f.rear_members.contains(&id) {
        // Rank-0 member: to the rear group; its result is discarded, return value always 1
        // (`0x0042ABE1`-`0x0042ABF8`: `rear->vf18(id)` via VTable `+0x18`, then `AL = 1`).
        let rear = f.rear;
        super::on_leader_path(sim, rear, id);
        return true;
    }
    if !g.marching {
        return none::on_leader_path(sim, gid, id);
    }
    let (start, leader_layer) = (f.leader_start, f.leader_layer);
    if sim.object(id).is_none() {
        return false;
    }
    set_grid_xz(sim, id, start);
    send(sim, id, &Message::new(0xF00, &[leader_layer]));
    let Some(path) = walk(sim, id).map(|w| w.high_path.clone()) else {
        return false;
    };
    let Some(&tail) = path.back() else {
        return false;
    };
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    let f = form_mut(g);
    f.frames.clear();
    let (march_ref, layer, m) = (
        f.march_ref,
        corner_layer(f.layer),
        mul(f.corner_margin, f.spacing),
    );
    // Point list: reference point, each waypoint except the last (shifted at corners), path end.
    let mut points = vec![(march_ref.0, march_ref.1)];
    for h in path.iter().take(path.len() - 1) {
        let p = (h.x, h.z);
        points.push(corner_adjust(&sim.state.path, layer, m, p).unwrap_or(p));
    }
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    g.anchor = (tail.x, tail.z, g.anchor.2);
    points.push((tail.x, tail.z));
    // No frame arises for the start point itself; equal points are skipped.
    let (mut a, mut prev) = (points[0], march_ref.2);
    for &b in &points[1..] {
        if a == b {
            continue;
        }
        let new = atan2_deg(b.1.wrapping_sub(a.1), b.0.wrapping_sub(a.0));
        add_turn_frames(sim, gid, a, prev, new);
        add_frame(sim, gid, b, new);
        (a, prev) = (b, new);
    }
    let Some(g) = find(sim, gid) else {
        return false;
    };
    // Final turn to the anchor direction; the pivot becomes the new anchor.
    let mut anchor = g.anchor;
    if anchor.2 != NO_DIRECTION {
        (anchor.0, anchor.1) = add_turn_frames(sim, gid, a, prev, anchor.2);
    } else {
        anchor.2 = prev;
    }
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    g.anchor = anchor;
    // `0x2A00` to all members (order `members`), posted with abort.
    let members = g.members.clone();
    for m in members {
        if sim.object(m).is_some() {
            post_message(sim, m, Message::new(0x2A00, &[]), true);
        }
    }
    if let Some(g) = find_mut(sim, gid) {
        let f = form_mut(g);
        f.reported = 0;
        f.arrived = 0;
    }
    true
}

/// `GroupFormation_OnMemberReport` (`0x0042A440`, re/groups-formations 12.7, K-219): only when all have reported,
/// `0x1200` with its frame places synchronously for each member; one buffer for all (leftovers of the
/// predecessor stay). The frames are re-read per place: a `0x1200` can already advance during the
/// distribution (`advance_frame`).
pub fn on_member_report(sim: &mut Sim, gid: i32, _id: ObjectId) {
    let Some(g) = find_mut(sim, gid) else { return };
    let members = g.members.clone();
    let (flags, dir) = (g.walk_flags | 0x10, g.anchor.2);
    let f = form_mut(g);
    f.reported = f.reported.wrapping_add(1);
    if (f.reported as usize) < members.len() {
        return;
    }
    let n = f.frames.len() as i64;
    let points = 2 * n - 2;
    let mut args = vec![0i32; (7 + points.max(0)) as usize];
    args[3] = flags;
    args[5] = dir;
    // Place of `m` in frame `i`: `None` = frame missing, `Some(None)` = member missing in the frame.
    let slot = |sim: &Sim, i: usize, m: ObjectId| -> Option<Option<(i32, i32)>> {
        let fr = form(find(sim, gid)?).frames.get(i)?;
        Some(fr.get(m.0).copied())
    };
    for m in members {
        // Frames 0 ... F - 2 as waypoints until one does not contain the member; then the
        // next frame must contain it (path end), otherwise the distribution ends for all (K-219).
        let mut i = 0usize;
        let next = loop {
            if 2 * (i as i64) >= points {
                break i;
            }
            let Some(p) = slot(sim, i, m) else { return };
            let Some((x, z)) = p else { break i + 1 };
            args[7 + 2 * i] = x;
            args[8 + 2 * i] = z;
            i += 1;
        };
        let Some(Some((xe, ze))) = slot(sim, next, m) else {
            return;
        };
        args[0] = xe;
        args[1] = ze;
        if let Some(g) = find_mut(sim, gid) {
            g.targets.set(m.0, (xe, ze, dir));
        }
        if sim.object(m).is_some() {
            send(sim, m, &Message::new(0x1200, &args));
        }
    }
}

/// `GroupFormation_OnMemberArrived` (`0x0042A640`, re/groups-formations 12.7): exactly on the place in the front
/// frame (if the entry is missing: (0, 0)) -> arrived, into `ready`, advance.
pub fn on_member_arrived(sim: &mut Sim, gid: i32, id: ObjectId) -> bool {
    let Some(pos) = none::grid_pos(sim, id) else {
        return false;
    };
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    let f = form_mut(g);
    let Some(front) = f.frames.front() else {
        return false;
    };
    if front.get(id.0).copied().unwrap_or((0, 0)) != pos {
        return false;
    }
    f.arrived = f.arrived.wrapping_add(1);
    if !f.ready.contains(&id) {
        f.ready.push(id);
    }
    advance_frame(sim, gid);
    true
}

/// `GroupFormation_AdvanceFrame` (`0x0042A080`, re/groups-formations 12.7, K-180): all in `ready` arrived ->
/// discard the front frame, `0x3A00` synchronously to every existing object in `ready`. The frame
/// change is thus triggered by the member that arrives last, in its tick.
pub fn advance_frame(sim: &mut Sim, gid: i32) {
    let Some(g) = find_mut(sim, gid) else { return };
    let f = form_mut(g);
    if !(f.arrived > 0 && f.arrived as usize == f.ready.len()) {
        return;
    }
    f.frames.pop_front();
    f.arrived = 0;
    let ready = f.ready.clone();
    for m in ready {
        if sim.object(m).is_some() {
            send(sim, m, &Message::new(0x3A00, &[]));
        }
    }
}

/// `GroupFormation_Rejoin` (`0x0042A030`): member and not yet in `ready` -> append at the back.
pub fn rejoin(sim: &mut Sim, gid: i32, id: ObjectId) -> bool {
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    let member = g.members.contains(&id);
    let f = form_mut(g);
    if member && !f.ready.contains(&id) {
        f.ready.push(id);
        return true;
    }
    false
}

/// `GroupFormation_Leave` (`0x0042A730`): take out of `ready`, then advance if needed.
pub fn leave(sim: &mut Sim, gid: i32, id: ObjectId) {
    let Some(g) = find_mut(sim, gid) else { return };
    let f = form_mut(g);
    if let Some(i) = f.ready.iter().position(|&m| m == id) {
        f.ready.remove(i);
        advance_frame(sim, gid);
    }
}

/// `GroupFormation_GetChaseSlot` (`0x004299C0`, re/groups-formations 3.6, 12.7): place of `id` relative to the
/// reference (`ready` head) at its trajectory point `n` (`0x2300 {n}`).
pub fn chase_slot(sim: &mut Sim, gid: i32, id: ObjectId, n: i32) -> Option<(i32, i32)> {
    let g = find(sim, gid)?;
    let f = form(g);
    let &reference = f.ready.first()?;
    let oi = g.offsets.get(id.0).copied().unwrap_or((0, 0));
    let or = g.offsets.get(reference.0).copied().unwrap_or((0, 0));
    let sp = f.spacing;
    sim.object(reference)?;
    let mut msg = Message::new(0x2300, &[n, 0, 0]);
    if send_mut(sim, reference, &mut msg) == 0 {
        return None;
    }
    let v = (
        mul(oi.0.wrapping_sub(or.0), sp),
        mul(oi.1.wrapping_sub(or.1), sp),
    );
    Some(rot((msg.arg(0), msg.arg(1)), v, msg.arg(2)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::execute;
    use crate::group::testkit::*;
    use crate::group::{Group, add_member, geometry, key};
    use crate::message::{Envelope, Posted};
    use crate::messaging::dispatch_command;
    use crate::module::walk::{find_to_msg, walk_mut};
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::step;

    /// 64 x 64 free, individual cells blocked in the land 1x1 layer.
    fn grid_with(blocked: &[(i32, i32)]) -> (SimState, Rules) {
        let rules = rules();
        let mut state = state(&rules);
        for &(x, z) in blocked {
            state.path.mark_cell(1, x, z, true);
        }
        (state, rules)
    }

    /// Formation group by hand: infantrymen from tile (10, 10), `offsets` set, wing ends
    /// via the places, `spacing = 0x2000`.
    fn formation_group(sim: &mut Sim, kind: u8, offsets: &[(i32, i32)]) -> i32 {
        let gid = sim.state.groups.next_id;
        sim.state.groups.next_id += 1;
        sim.state.groups.map.set(
            key(gid),
            Group::new(gid, kind, (-1, -1, NO_DIRECTION), false),
        );
        for (i, &o) in offsets.iter().enumerate() {
            let u = spawn(sim, INFANTRY, (10 + i as i32, 10));
            assert!(add_member(sim, gid, u));
            find_mut(sim, gid).unwrap().offsets.set(u.0, o);
        }
        let g = find_mut(sim, gid).unwrap();
        let (max, min) = geometry::find_ends(&g.offsets).unwrap();
        let f = form_mut(g);
        (f.end_max, f.end_min, f.spacing) = (max, min, 0x2000);
        gid
    }

    fn member(sim: &Sim, gid: i32, i: usize) -> ObjectId {
        find(sim, gid).unwrap().members[i]
    }

    fn ends(sim: &Sim, gid: i32) -> (ObjectId, ObjectId) {
        let f = form(find(sim, gid).unwrap());
        (f.end_max, f.end_min)
    }

    /// March on: reference point, `leader_start` = position of the leader, anchor without position or direction.
    fn marching(sim: &mut Sim, gid: i32, march_ref: (i32, i32, i32)) {
        let leader = member(sim, gid, 0);
        let start = none::grid_pos(sim, leader).unwrap();
        let g = find_mut(sim, gid).unwrap();
        g.marching = true;
        g.anchor = (-1, -1, NO_DIRECTION);
        let f = form_mut(g);
        f.march_ref = march_ref;
        f.leader_start = start;
    }

    /// Fill the HighPath of the object with fresh waypoint IDs.
    fn set_path(sim: &mut Sim, id: ObjectId, pts: &[(i32, i32)]) {
        let w = walk_mut(sim, id).unwrap();
        w.high_path.clear();
        for &(x, z) in pts {
            let p = w.point(x, z);
            w.high_path.push_back(p);
        }
    }

    // QUIRK K-254: kept
    #[test]
    fn corner_adjustment_moves_away_from_the_single_blocked_diagonal() {
        // re/groups-formations 12.2, m = Mul(corner_margin, spacing) = 1000, Q = 0x400.
        let t = |c: i32| c << 12;
        let (s, _) = grid_with(&[(11, 11)]); // only NE blocked
        assert_eq!(
            corner_adjust(&s.path, 0, 1000, (t(10) + 7, t(10) + 9)),
            Some((t(10) + 1024 - 1000, t(10) + 1024 - 1000))
        );
        let (s, _) = grid_with(&[(9, 9)]); // only SW blocked
        assert_eq!(
            corner_adjust(&s.path, 0, 1000, (t(10), t(10))),
            Some((t(10) + 3072 + 1000, t(10) + 3072 + 1000))
        );
        let (s, _) = grid_with(&[(11, 9)]); // only SE blocked
        assert_eq!(
            corner_adjust(&s.path, 0, 1000, (t(10), t(10))),
            Some((t(10) + 1024 - 1000, t(10) + 3072 + 1000))
        );
        let (s, _) = grid_with(&[(9, 11)]); // only NW blocked
        assert_eq!(
            corner_adjust(&s.path, 0, 1000, (t(10), t(10))),
            Some((t(10) + 3072 + 1000, t(10) + 1024 - 1000))
        );
        // All free, two diagonals blocked, cell itself blocked -> no shift (K-254).
        let (s, _) = grid_with(&[]);
        assert_eq!(corner_adjust(&s.path, 0, 1000, (t(10), t(10))), None);
        let (s, _) = grid_with(&[(9, 9), (11, 11)]);
        assert_eq!(corner_adjust(&s.path, 0, 1000, (t(10), t(10))), None);
        let (s, _) = grid_with(&[(10, 10)]);
        assert_eq!(corner_adjust(&s.path, 0, 1000, (t(10), t(10))), None);
        // West and NW blocked -> the cell moves south (oz = 9), there NW (= west neighbor)
        // is blocked.
        let (s, _) = grid_with(&[(9, 10), (9, 11)]);
        assert_eq!(
            corner_adjust(&s.path, 0, 1000, (t(10), t(10))),
            Some((t(10) + 3072 + 1000, t(9) + 1024 - 1000))
        );
    }

    // QUIRK K-254: kept
    #[test]
    fn corner_adjustment_at_the_map_edge() {
        // Outside = 0x3FFFFFFF != 0: no panic, no shift.
        let (s, _) = grid_with(&[]);
        assert_eq!(corner_adjust(&s.path, 0, 1000, (0, 0)), None);
        assert_eq!(
            corner_adjust(&s.path, 0, 1000, ((63 << 12) + 4095, (63 << 12) + 4095)),
            None
        );
    }

    #[test]
    fn corner_layer_is_the_1x1_layer_of_the_class() {
        // Layer 3 (0x39) stays; layer 5 (land 2x2, 0x0A) -> 0x09 = layer 0; invalid -> land.
        assert_eq!(corner_layer(0), 0);
        assert_eq!(corner_layer(3), 3);
        assert_eq!(corner_layer(5), 0);
        assert_eq!(corner_layer(-1), 0);
        assert_eq!(corner_layer(99), 0);
    }

    #[test]
    fn frames_rotate_the_offsets_and_turns_over_90_degrees_mirror() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 1024)]); // line of two
        add_frame(&mut sim, gid, (0, 0), 90 << 12);
        let f = &find(&sim, gid).unwrap().formation.as_ref().unwrap().frames[0];
        // lz = 1024 * sp 2 = 2048 to the left; at 90 degrees left lies at -x.
        assert_eq!(f.get(member(&sim, gid, 1).0), Some(&(-2048, 0)));
        // 180 degrees turn: mirror (lz negated, ends swapped), then pivot with d = 0 -> the reference
        // itself.
        let (end_max, end_min) = ends(&sim, gid);
        let v = add_turn_frames(&mut sim, gid, (100, 200), 0, 180 << 12);
        assert_eq!(v, (100, 200));
        assert_eq!(ends(&sim, gid), (end_min, end_max));
        let g = find(&sim, gid).unwrap();
        assert_eq!(g.offsets.get(member(&sim, gid, 1).0), Some(&(0, -1024)));
        let frames = &g.formation.as_ref().unwrap().frames;
        assert_eq!(frames.len(), 2);
        // Mirrored and turned by 180 degrees: the member stays on its world side (+z).
        assert_eq!(
            frames[1].get(member(&sim, gid, 1).0),
            Some(&(100, 200 + 2048))
        );
    }

    #[test]
    fn a_square_turning_over_90_degrees_is_not_mirrored() {
        // vf4C is `0x004EC990` for square and type 6 (nothing, re/groups-formations 1.5 table `+0x4C`); only
        // `old += 180°` applies (AddTurnFrames itself, re/groups-formations 12.3). Pivot = center.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 2, &[(0, -1024), (0, 1024)]);
        let offsets = find(&sim, gid).unwrap().offsets.clone();
        let before = ends(&sim, gid);
        let v = add_turn_frames(&mut sim, gid, (100, 200), 0, 180 << 12);
        assert_eq!(v, (100, 200));
        assert_eq!(ends(&sim, gid), before);
        let g = find(&sim, gid).unwrap();
        assert_eq!(g.offsets, offsets);
        // Unmirrored turned by 180 degrees: the member changes its world side (-z).
        let frames = &g.formation.as_ref().unwrap().frames;
        assert_eq!(
            frames[0].get(member(&sim, gid, 1).0),
            Some(&(100, 200 - 2048))
        );
        // Type 6 likewise.
        let gid = formation_group(&mut sim, 6, &[(0, -1024), (0, 1024)]);
        let offsets = find(&sim, gid).unwrap().offsets.clone();
        add_turn_frames(&mut sim, gid, (100, 200), 0, 180 << 12);
        assert_eq!(find(&sim, gid).unwrap().offsets, offsets);
    }

    #[test]
    fn a_second_move_builds_frames_from_the_leaders_path() {
        let rules = rules();
        let mut state = state(&rules);
        let u: Vec<ObjectId> = {
            let mut sim = Sim::new(&mut state, &rules);
            let u: Vec<_> = (0..4)
                .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
                .collect();
            execute(&mut sim, &mv(&u, (12, 20), 1, NO_DIRECTION), true);
            u
        };
        for _ in 0..200 {
            step(&mut state, &rules, &[]);
        }
        // Second command to the same selection -> realign (`0x0042A190`): only the leader (head)
        // gets a FINDTO, targets cleared, `ready` = members.
        {
            let mut sim = Sim::new(&mut state, &rules);
            execute(&mut sim, &mv(&u, (40, 20), 1, NO_DIRECTION), true);
            let g = find(&sim, 0).unwrap();
            assert!(g.marching);
            let leader = g.members[0];
            assert_eq!(leader, u[3], "head = chosen last");
            assert!(g.targets.is_empty());
            assert_eq!(g.formation.as_ref().unwrap().ready, g.members);
            let (x, z, flags) = posted_find_to(&sim, leader).expect("leader FINDTO");
            assert_eq!((x, z, flags & 8), (tile_center(40), tile_center(20), 8));
            assert!(u[..3].iter().all(|&m| posted_find_to(&sim, m).is_none()));
        }
        step(&mut state, &rules, &[]);
        step(&mut state, &rules, &[]);
        let sim = Sim::new(&mut state, &rules);
        let g = find(&sim, 0).unwrap();
        let f = g.formation.as_ref().unwrap();
        assert!(!f.frames.is_empty(), "frames from the leader path");
        assert!(f.frames.iter().all(|fr| u.iter().all(|m| fr.contains(m.0))));
        assert_eq!(g.anchor.0 >> 12, 40);
    }

    // QUIRK K-257: kept
    #[test]
    fn rear_group_members_run_through_the_main_find_to_hook() {
        // K-257 (re/groups-formations 12.9): during the march, the FINDTO (flag 8) of the rear group leader
        // runs through vf14 of the main group: `offsets` gets (0, 0) for it although it is no rank member.
        let rules = rules();
        let mut state = state(&rules);
        let u: Vec<ObjectId> = {
            let mut sim = Sim::new(&mut state, &rules);
            let mut u: Vec<_> = (0..4)
                .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
                .collect();
            u.extend((0..2).map(|i| spawn(&mut sim, WARRIOR, (14 + i, 10))));
            execute(&mut sim, &mv(&u, (12, 20), 1, NO_DIRECTION), true);
            u
        };
        let rear = {
            let sim = Sim::new(&mut state, &rules);
            let g = find(&sim, 0).unwrap();
            let f = g.formation.as_ref().unwrap();
            assert_eq!((g.members.len(), f.rear_members.len()), (4, 2));
            // At the first command the main group does not march: no entry.
            assert!(u[4..].iter().all(|w| g.offsets.get(w.0).is_none()));
            f.rear
        };
        for _ in 0..200 {
            step(&mut state, &rules, &[]);
        }
        let last_rear_leader = {
            let mut sim = Sim::new(&mut state, &rules);
            execute(&mut sim, &mv(&u, (40, 20), 1, NO_DIRECTION), true);
            let leaders = find(&sim, rear).expect("rear group").packs.keys();
            assert_eq!(leaders.len(), 1, "both warriors in one pack");
            let w = ObjectId(leaders[0]);
            (
                w,
                none::grid_pos(&sim, w).unwrap(),
                walk(&sim, w).unwrap().layer,
            )
        };
        step(&mut state, &rules, &[]);
        let sim = Sim::new(&mut state, &rules);
        let r = find(&sim, rear).expect("rear group");
        let leaders = r.packs.keys();
        assert!(!leaders.is_empty());
        let g = find(&sim, 0).unwrap();
        for l in leaders {
            assert!(u[4..].contains(&ObjectId(l)));
            assert!(!g.members.contains(&ObjectId(l)));
            assert_eq!(g.offsets.get(l), Some(&(0, 0)), "K-257");
        }
        // The warrior overwrote the march reference: offset (0, 0) -> `march_ref` = its position.
        // (In this scene the main leader does not plan until the next tick and then sets it anew.)
        let (w, pos, layer) = last_rear_leader;
        let f = form(g);
        assert_eq!(f.leader_start, pos);
        assert_eq!((f.march_ref.0, f.march_ref.1), pos);
        assert_eq!(f.leader_layer, layer as i32);
        assert!(u[4..].contains(&w));
    }

    // QUIRK K-262: kept
    #[test]
    fn a_leader_whose_find_to_ends_without_a_path_stays_on_the_march_reference() {
        // K-262 (re/groups-formations 12.9): `PropertyWalk_OnMsgFindTo` (`0x00538590`) calls vf14 before
        // `ClampPosToMap`, the target radius test and planning. If the leader, after being moved onto `march_ref`,
        // already lies within the target radius (command to its own point), the FINDTO ends without a PATH: it
        // stays on `march_ref` and in the layer of the formation, no frames arise.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -256), (0, 256)]);
        let leader = member(&sim, gid, 0);
        marching(&mut sim, gid, (0, 0, 0));
        let start = none::grid_pos(&sim, leader).unwrap();
        form_mut(find_mut(&mut sim, gid).unwrap()).layer = 3;
        assert_ne!(walk(&sim, leader).unwrap().layer, 3);
        let env = Envelope {
            msg: find_to_msg(start.0, start.1, -1, NO_DIRECTION, 0, false, 8),
            success: None,
            fault: None,
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        dispatch_command(&mut sim, leader, &env);
        let f = form(find(&sim, gid).unwrap());
        let march_ref = (f.march_ref.0, f.march_ref.1);
        assert_ne!(march_ref, start, "the shift moves the leader");
        assert_eq!(f.leader_start, start);
        assert_eq!(none::grid_pos(&sim, leader), Some(march_ref));
        assert_eq!(
            walk(&sim, leader).unwrap().layer,
            3,
            "layer of the formation"
        );
        assert!(walk(&sim, leader).unwrap().high_path.is_empty(), "no path");
        assert!(f.frames.is_empty(), "no PATH, no frames");
    }

    #[test]
    fn equal_points_build_no_frame() {
        // Leader path whose first waypoint equals the reference point: no frame for it (re/groups-formations 12.3).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 1024)]);
        let leader = member(&sim, gid, 0);
        marching(&mut sim, gid, (tile_center(10), tile_center(10), 0));
        set_path(
            &mut sim,
            leader,
            &[
                (tile_center(10), tile_center(10)),
                (tile_center(20), tile_center(10)),
            ],
        );
        assert!(on_leader_path(&mut sim, gid, leader));
        // march_ref = (10, 10) -> first waypoint equal -> skipped. For the path end: turn frame
        // at the old point (also at 0 degrees) and frame at the path end = 2. Without skipping it would be 4.
        // Anchor without direction -> no final turn, anchor direction = last walking direction (0).
        let g = find(&sim, gid).unwrap();
        assert_eq!(g.formation.as_ref().unwrap().frames.len(), 2);
        assert_eq!(g.anchor, (tile_center(20), tile_center(10), 0));
    }

    /// Posted messages `code` in the FIFO of the object (with abort flag).
    fn posted(sim: &Sim, id: ObjectId, code: u32) -> Vec<bool> {
        sim.object(id).map_or(Vec::new(), |o| {
            o.fifo
                .iter()
                .filter_map(|p| match p {
                    Posted::Message { msg, abort } if msg.code == code => Some(*abort),
                    _ => None,
                })
                .collect()
        })
    }

    // QUIRK K-261: kept
    #[test]
    fn the_leader_path_posts_0x2a00_to_every_living_member_and_resets_the_counts() {
        // re/groups-formations 3.5 step 2, 12.3: `0x2A00` posted with abort to every existing member
        // (order `members`), then `reported = arrived = 0`.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 0), (0, 1024)]);
        let leader = member(&sim, gid, 0);
        let gone = member(&sim, gid, 2);
        marching(&mut sim, gid, (tile_center(10), tile_center(10), 0));
        set_path(&mut sim, leader, &[(tile_center(20), tile_center(10))]);
        crate::world::destroy_object(&mut sim, gone);
        {
            let f = form_mut(find_mut(&mut sim, gid).unwrap());
            (f.reported, f.arrived) = (5, 2);
        }
        assert!(on_leader_path(&mut sim, gid, leader));
        let g = find(&sim, gid).unwrap();
        assert!(
            g.members.contains(&gone),
            "the group member stays registered"
        );
        for i in 0..2 {
            assert_eq!(posted(&sim, member(&sim, gid, i), 0x2A00), vec![true]);
        }
        assert!(sim.object(gone).is_none());
        let f = form(g);
        assert_eq!((f.reported, f.arrived), (0, 0));
        // K-261: the state (destroyed member without leaving, e.g. a sunken canoe) is reachable;
        // it loads and continues bit-identically. The two remaining ones report, `reported`
        // never reaches `members.len()` = 3: the group stands still.
        let bytes = crate::save::testkit::save_bytes(&state, &rules);
        let mut loaded = crate::save::SaveGame::from_bytes(&bytes)
            .expect("loading")
            .state;
        for i in 0..200 {
            step(&mut state, &rules, &[]);
            step(&mut loaded, &rules, &[]);
            assert_eq!(
                crate::save::checksum(&state),
                crate::save::checksum(&loaded),
                "tick {i} after loading"
            );
        }
        let sim = Sim::new(&mut state, &rules);
        let g = find(&sim, gid).unwrap();
        assert!(g.members.contains(&gone));
        assert_eq!(form(g).reported, 2, "only the living report");
    }

    #[test]
    fn a_march_with_a_target_direction_turns_at_the_end_around_the_inner_wing() {
        // re/groups-formations 12.3: anchor with direction -> after the loop
        // `AddTurnFrames(path end, prev, anchor.dir)`; the pivot (line: inner wing end) becomes the new anchor.
        // Without the branch: 2 frames, anchor = path end with direction 0.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 1024)]);
        let (a, b) = (member(&sim, gid, 0), member(&sim, gid, 1));
        marching(&mut sim, gid, (tile_center(10), tile_center(10), 0));
        find_mut(&mut sim, gid).unwrap().anchor.2 = 90 << 12;
        let end = (tile_center(20), tile_center(10));
        set_path(&mut sim, a, &[end]);
        assert!(on_leader_path(&mut sim, gid, a));
        let g = find(&sim, gid).unwrap();
        // d = +90 degrees -> pivot around `end_max` (b, lz = 1024 * sp 2 = 2048 to the left): w = (end.x, end.z +
        // 2048), result rot(w, end - w, 90 degrees) = (end.x + 2048, end.z + 2048).
        assert_eq!(form(g).end_max, b);
        assert_eq!(g.anchor, (end.0 + 2048, end.1 + 2048, 90 << 12));
        let frames = &form(g).frames;
        assert_eq!(frames.len(), 3);
        // Final frame: b (inner wing end) stays put, a swings by 4096 toward +x, +z.
        assert_eq!(frames[2].get(b.0), Some(&(end.0, end.1 + 2048)));
        assert_eq!(frames[1].get(b.0), Some(&(end.0, end.1 + 2048)));
        assert_eq!(frames[2].get(a.0), Some(&(end.0 + 4096, end.1 + 2048)));
    }

    #[test]
    fn a_march_waypoint_at_a_corner_is_shifted_away_from_the_blocked_diagonal() {
        // re/groups-formations 12.2/12.3: waypoints except the last go through the corner shift; the path
        // end does not. One member with place (0, 0): frame = points. Cell (20, 10) free, only NE (21, 11)
        // blocked -> (lo(20), lo(10)) with m = Mul(500, sp 2) = 1000.
        let (mut state, rules) = grid_with(&[(21, 11)]);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, 0)]);
        let a = member(&sim, gid, 0);
        form_mut(find_mut(&mut sim, gid).unwrap()).corner_margin = 500;
        marching(&mut sim, gid, (tile_center(10), tile_center(10), 0));
        let corner = (tile_center(20), tile_center(10));
        let end = (tile_center(20), tile_center(20));
        set_path(&mut sim, a, &[corner, end]);
        assert!(on_leader_path(&mut sim, gid, a));
        let shifted = ((20 << 12) + 1024 - 1000, (10 << 12) + 1024 - 1000);
        let spots: Vec<_> = form(find(&sim, gid).unwrap())
            .frames
            .iter()
            .map(|f| *f.get(a.0).unwrap())
            .collect();
        // Frames: turn frame at the reference, corner, turn frame at the corner, path end.
        assert_eq!(spots.len(), 4);
        assert_eq!(spots[1], shifted, "waypoint at the corner shifted");
        assert!(!spots.contains(&corner));
        assert_eq!(spots[3], end, "path end without shift");
    }

    // QUIRK K-180: kept
    #[test]
    fn arriving_on_the_spot_waits_and_the_last_one_advances_the_frame() {
        // re/groups-formations 3.5 step 4, 12.7: exactly on the place in the front frame -> state 10, `arrived`;
        // next to it -> state 0xB, bit 0x10 off, 1.5x. From 0xB back onto the place: cruising speed,
        // arrived; all in `ready` there -> frame gone, `0x3A00` to all (state 5, 0x800 on).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 1024)]);
        let (a, b) = (member(&sim, gid, 0), member(&sim, gid, 1));
        let (pa, pb) = (
            none::grid_pos(&sim, a).unwrap(),
            none::grid_pos(&sim, b).unwrap(),
        );
        {
            let g = find_mut(&mut sim, gid).unwrap();
            let f = form_mut(g);
            f.ready = vec![a, b];
            let mut front = MfcMap::new();
            front.set(a.0, pa);
            front.set(b.0, (pb.0 + 1, pb.1));
            f.frames.push_back(front);
            f.frames.push_back(MfcMap::new());
        }
        for m in [a, b] {
            let w = walk_mut(&mut sim, m).unwrap();
            w.state = 5;
            let c = w.speed.cruising;
            w.speed.set_cur_speed(c);
        }
        let reached = crate::module::walk::formation::on_formation_high_point_reached;
        let st = |sim: &Sim, m: ObjectId| sim.object(m).unwrap().status;
        reached(&mut sim, a);
        assert_eq!(walk(&sim, a).unwrap().state, 10);
        assert_ne!(st(&sim, a) & 0x10, 0);
        assert_eq!(form(find(&sim, gid).unwrap()).arrived, 1);
        reached(&mut sim, b);
        let w = walk(&sim, b).unwrap();
        assert_eq!(w.state, 0xB);
        assert_eq!(w.speed.cur, w.speed.default * 3 / 2);
        assert_eq!(st(&sim, b) & 0x10, 0);
        let f = form(find(&sim, gid).unwrap());
        assert_eq!((f.arrived, f.frames.len()), (1, 2));
        // Correct the place: b arrives from 0xB, the frame advances.
        form_mut(find_mut(&mut sim, gid).unwrap()).frames[0].set(b.0, pb);
        reached(&mut sim, b);
        let f = form(find(&sim, gid).unwrap());
        assert_eq!((f.arrived, f.frames.len()), (0, 1));
        for m in [a, b] {
            let w = walk(&sim, m).unwrap();
            assert_eq!(w.state, 5, "{m:?}");
            assert_eq!((st(&sim, m) & 0x10, st(&sim, m) & 0x800), (0, 0x800));
        }
        let w = walk(&sim, b).unwrap();
        assert_eq!(w.speed.cur, w.speed.cruising, "cruising speed from 0xB");
    }

    #[test]
    fn entering_formation_walk_leaves_ready_and_may_advance_the_frame() {
        // re/groups-formations 3.6: `0x700` in state 5 -> `EnterFormationWalk`: out of `ready` (advance if
        // needed), 1.5x, state 0xB, counter 0. Here b is already waiting (arrived 1): without a, all are there.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 1024)]);
        let (a, b) = (member(&sim, gid, 0), member(&sim, gid, 1));
        {
            let f = form_mut(find_mut(&mut sim, gid).unwrap());
            (f.ready, f.arrived) = (vec![a, b], 1);
            f.frames.push_back(MfcMap::new());
        }
        walk_mut(&mut sim, a).unwrap().state = 5;
        walk_mut(&mut sim, b).unwrap().state = 10;
        let mut m = Message::new(0x700, &[-1]);
        crate::messaging::send_mut(&mut sim, a, &mut m);
        let w = walk(&sim, a).unwrap();
        assert_eq!((w.state, w.ticks_since_chase), (0xB, 0));
        assert_eq!(w.speed.cur, w.speed.default * 3 / 2);
        let f = form(find(&sim, gid).unwrap());
        assert_eq!(
            (f.ready.clone(), f.arrived, f.frames.len()),
            (vec![b], 0, 0)
        );
        assert_eq!(walk(&sim, b).unwrap().state, 5, "0x3A00 to b");
        // Catching up: a is a member and not in `ready` -> appended at the back again; not a second time.
        assert!(rejoin(&mut sim, gid, a));
        assert!(!rejoin(&mut sim, gid, a));
        assert_eq!(form(find(&sim, gid).unwrap()).ready, vec![b, a]);
    }

    #[test]
    fn the_chase_slot_is_the_spot_next_to_the_reference() {
        // re/groups-formations 12.7 `ChaseSlot`: ref = `ready` head; without a trajectory its position and
        // direction (0x2300 {0}); place = rot(ref, ((o_id - o_ref) * sp), dir). b lies 2048 * sp 2 = 4096 left of a.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1024), (0, 1024)]);
        let (a, b) = (member(&sim, gid, 0), member(&sim, gid, 1));
        assert_eq!(chase_slot(&mut sim, gid, b, 0), None, "ready empty");
        form_mut(find_mut(&mut sim, gid).unwrap()).ready = vec![a];
        let pa = none::grid_pos(&sim, a).unwrap();
        let dir = crate::module::walk::direction(&sim, a);
        assert_eq!(
            chase_slot(&mut sim, gid, b, 0),
            Some(rot(pa, (0, 4096), dir))
        );
        assert_eq!(chase_slot(&mut sim, gid, a, 1), Some(pa));
    }

    // QUIRK K-219: kept
    #[test]
    fn the_distribution_reuses_one_buffer_and_stops_at_the_first_member_without_an_end() {
        // K-219, re/groups-formations 12.7 `OnMemberReport`: F = 3 -> two waypoints, path end from frame 2. b is
        // missing in frame 1: its second point stays that of a (one buffer for all), end = next frame. c is
        // missing at the path end: the distribution ends, c and d get nothing.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gid = formation_group(&mut sim, 1, &[(0, -1536), (0, -512), (0, 512), (0, 1536)]);
        let m: Vec<ObjectId> = (0..4).map(|i| member(&sim, gid, i)).collect();
        let spot = |i: usize, k: i32| (tile_center(20 + i as i32), tile_center(20 + 5 * k));
        {
            let f = form_mut(find_mut(&mut sim, gid).unwrap());
            for k in 0..3 {
                let mut fr = MfcMap::new();
                for (i, id) in m.iter().enumerate() {
                    if !(k == 1 && i == 1 || k == 2 && i == 2) {
                        fr.set(id.0, spot(i, k));
                    }
                }
                f.frames.push_back(fr);
            }
            f.reported = 3;
        }
        on_member_report(&mut sim, gid, m[0]);
        let g = find(&sim, gid).unwrap();
        assert_eq!(form(g).reported, 4);
        let hp = |sim: &Sim, id: ObjectId| -> Vec<(i32, i32)> {
            walk(sim, id)
                .unwrap()
                .high_path
                .iter()
                .map(|h| (h.x, h.z))
                .collect()
        };
        assert_eq!(hp(&sim, m[0]), vec![spot(0, 0), spot(0, 1), spot(0, 2)]);
        assert_eq!(hp(&sim, m[1]), vec![spot(1, 0), spot(0, 1), spot(1, 2)]);
        for (i, &id) in m[..2].iter().enumerate() {
            let (x, z) = spot(i, 2);
            assert_eq!(g.targets.get(id.0), Some(&(x, z, NO_DIRECTION)));
            assert_eq!(walk(&sim, id).unwrap().state, 5);
        }
        for &id in &m[2..] {
            assert_eq!(g.targets.get(id.0), None);
            assert!(hp(&sim, id).is_empty());
            assert_eq!(walk(&sim, id).unwrap().state, 1);
        }
    }
}
