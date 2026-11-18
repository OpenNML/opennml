//! Group types 0 and 5 (`GroupNone`, re/groups-formations 3.2, 3.4, 12.8): offsets to the centroid, targets with
//! compression, packs and path distribution of the pack leader.

use super::{find, find_mut};
use crate::fx::{fixed_div, fixed_hypot, fixed_mul, fixed_sqrt_int};
use crate::id::ObjectId;
use crate::sim::Sim;

/// Grid position of an object (`GO+0x198`).
pub(crate) fn grid_pos(sim: &Sim, id: ObjectId) -> Option<(i32, i32)> {
    sim.object(id).map(|o| (o.grid.x.0, o.grid.z.0))
}

/// `GroupNone_CaptureMemberOffsets` (`0x0056EC80`, re/groups-formations 12.8).
pub fn capture(sim: &mut Sim, gid: i32) {
    let Some(g) = find(sim, gid) else { return };
    let members = g.members.clone();
    let pos: Vec<_> = members.iter().map(|&m| (m, grid_pos(sim, m))).collect();
    let Some(g) = find_mut(sim, gid) else { return };
    g.max_dist = 0;
    if members.is_empty() {
        g.centroid = (-1, -1);
        return;
    }
    let (mut sx, mut sz, mut k) = (0i32, 0i32, 0i32);
    for (_, p) in &pos {
        if let Some((x, z)) = p {
            sx = sx.wrapping_add(*x);
            sz = sz.wrapping_add(*z);
            k += 1;
        }
    }
    g.centroid = if k > 0 { (sx / k, sz / k) } else { (0, 0) };
    let c = g.centroid;
    for (m, p) in pos {
        let Some((x, z)) = p else { continue };
        let o = (x.wrapping_sub(c.0), z.wrapping_sub(c.1));
        g.offsets.set(m.0, o);
        g.max_dist = g.max_dist.max(fixed_hypot(o.0, o.1));
    }
}

/// `GroupNone_ComputeTargets` (`0x004E4C30`): anchor x == -1 -> 0 (command lapses). Does not check the
/// places (K-208).
pub fn compute_targets(sim: &mut Sim, gid: i32) -> bool {
    let Some(g) = find(sim, gid) else {
        return false;
    };
    let (members, compress, max_dist, anchor) =
        (g.members.clone(), g.compress, g.max_dist, g.anchor);
    let mut s = 0x1000;
    if compress && max_dist > 0 {
        let soft: i32 = members
            .iter()
            .filter_map(|&m| sim.object(m).map(|o| o.soft_radius))
            .fold(0, i32::wrapping_add);
        let q = fixed_div(
            soft.wrapping_mul(2),
            fixed_mul(fixed_sqrt_int(members.len()), max_dist),
        );
        s = s.min(q);
    }
    if anchor.0 == -1 {
        return false;
    }
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    for m in members {
        let o = g.offsets.get(m.0).copied().unwrap_or((0, 0));
        let t = (
            anchor.0.wrapping_add(fixed_mul(o.0, s)),
            anchor.1.wrapping_add(fixed_mul(o.1, s)),
            anchor.2,
        );
        g.targets.set(m.0, t);
    }
    true
}

/// `Group_CanJoinPack` (`0x0056DC90`, re/groups-formations 9, 12.8): member `m` runs in the pack of leader `f` if
/// both are at most four tiles apart and the `k − 1` intermediate points (step 0.5 tiles) lie on free cells of
/// the layer.
pub fn can_join(sim: &mut Sim, f: ObjectId, m: ObjectId, layer: i32) -> bool {
    let (Some(fp), Some(mp)) = (grid_pos(sim, f), grid_pos(sim, m)) else {
        return false;
    };
    let (dx, dz) = (mp.0.wrapping_sub(fp.0), mp.1.wrapping_sub(fp.1));
    let d = fixed_hypot(dx, dz);
    if d > 0x4000 {
        return false;
    }
    let k = fixed_div(d, 0x800) >> 12;
    if k == 0 {
        return true;
    }
    let (sx, sz) = (dx / k, dz / k);
    let (mut x, mut z) = fp;
    for _ in 1..k {
        x = x.wrapping_add(sx);
        z = z.wrapping_add(sz);
        // Layer outside 0-8 (also -1): `cell_cost` returns "free" like `GetCellCost`.
        let cost =
            usize::try_from(layer).map_or(0, |l| sim.state.path.cell_cost(x >> 12, z >> 12, l));
        if cost != 0 {
            return false;
        }
    }
    true
}

/// `Group_BuildPacks` (`0x0056EDE0`, re/groups-formations 12.8): for each member find the first matching leader
/// (CMap iteration), otherwise it becomes a new leader.
pub fn build_packs(sim: &mut Sim, gid: i32) {
    let Some(g) = find_mut(sim, gid) else { return };
    g.packs.clear();
    let members = g.members.clone();
    for m in members {
        let layer = layer_of(sim, m);
        let leaders = find(sim, gid).map_or(Vec::new(), |g| g.packs.keys());
        let mut joined = None;
        for f in leaders.into_iter().map(ObjectId) {
            if layer_of(sim, f) == layer && can_join(sim, f, m, layer) {
                joined = Some(f);
                break;
            }
        }
        let Some(g) = find_mut(sim, gid) else { return };
        match joined {
            Some(f) => {
                if let Some(pack) = g.packs.get_mut(f.0) {
                    pack.push(m);
                }
            }
            None => g.packs.set(m.0, Vec::new()),
        }
    }
}

/// `Group_GetMemberLayer` (`0x0056DE60`): answer to `0x1400`, -1 without an object.
fn layer_of(sim: &mut Sim, id: ObjectId) -> i32 {
    if sim.object(id).is_none() {
        return -1;
    }
    crate::messaging::send(sim, id, &crate::message::Message::new(0x1400, &[]))
}

/// Base vf18 `Group_OnLeaderPath` (`0x0056E790`, re/groups-formations 12.8): path of the leader to the followers.
pub fn on_leader_path(sim: &mut Sim, gid: i32, leader: ObjectId) -> bool {
    use crate::message::{Envelope, Message};
    use crate::messaging::{post_command, send};
    use crate::module::walk::walk;
    let Some(path) = walk(sim, leader).map(|w| w.high_path.clone()) else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    let Some(g) = find(sim, gid) else {
        return false;
    };
    let s2 = if g.max_dist == 0 {
        0x1000
    } else {
        0x1000.min(fixed_div(0x79E, g.max_dist))
    };
    let Some(pack) = g.packs.get(leader.0).cloned() else {
        return false;
    };
    let (run, flags, success, fault) = (
        g.run,
        g.walk_flags | 0x10,
        g.success.clone(),
        g.fault.clone(),
    );
    let k = path.len() - 1;
    let wps: Vec<(i32, i32)> = path
        .iter()
        .take(k)
        .map(|h| (((h.x >> 12) << 12) + 0x800, ((h.z >> 12) << 12) + 0x800))
        .collect();
    for m in pack {
        let Some((mx, mz)) = grid_pos(sim, m) else {
            continue;
        };
        let Some(&(tx, tz, tdir)) = find(sim, gid).and_then(|g| g.targets.get(m.0)) else {
            continue;
        };
        let layer = send(sim, m, &Message::new(0x1400, &[]));
        let (mut x, mut z) = (tx, tz);
        let region = |sim: &Sim, x: i32, z: i32| {
            usize::try_from(layer).map_or(crate::path::BLOCKED, |l| {
                sim.state.path.region_id(l, x >> 12, z >> 12)
            })
        };
        if region(sim, tx, tz) != region(sim, mx, mz) {
            let (start, goal) = ((mx >> 12, mz >> 12), (tx >> 12, tz >> 12));
            match crate::path::reach::find_nearest_reachable_cell(
                sim, layer, start, goal, -1, None, None,
            ) {
                Some((cx, cz)) => (x, z) = ((cx << 12) + 0x800, (cz << 12) + 0x800),
                None => (x, z) = (mx, mz),
            }
        }
        let o = find(sim, gid)
            .and_then(|g| g.offsets.get(m.0).copied())
            .unwrap_or((0, 0));
        let mut args = vec![x, z, 0, flags, -1, tdir, -1];
        for &(wx, wz) in &wps {
            args.push(wx.wrapping_add(fixed_mul(o.0, s2)));
            args.push(wz.wrapping_add(fixed_mul(o.1, s2)));
        }
        // `PathMsg_ConstructIndependent(…, run, …)`: 0x600 = run (`walk::on_message`).
        let code = if run { 0x600 } else { 0x500 };
        let env = Envelope {
            msg: Message { code, args },
            success: success.clone(),
            fault: fault.clone(),
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        let to = super::linked_target(sim, m);
        post_command(sim, to, env, true);
    }
    if let Some(g) = find_mut(sim, gid) {
        g.success = None;
        g.fault = None;
    }
    true
}
