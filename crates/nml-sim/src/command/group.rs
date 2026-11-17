//! Group commands: walking with several units (`GameWorld_CmdMoveSelection` `0x0047CA10`,
//! re/groups-formations 3.1), formation (`0x44`) and scatter (`0x75`).

use super::{NO_SPREAD, RESPONSE_EXECUTE, move_unit, move_walk_flags, owns, respond, script_path};
use crate::fx::{fixed_hypot, fixed_mul};
use crate::group::{self, GroupOrder};
use crate::id::ObjectId;
use crate::math::{atan2_deg, cos_deg, sin_deg};
use crate::message::{Envelope, Message};
use crate::messaging::post_command;
use crate::module::walk::find_to_msg;
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;

/// `0x04` Walk. `units` in selection order (`units[0]` selected first). Only own, selectable
/// units count (A8); n is determined afterwards.
///
/// **Script path** ([`script_path`], E-215, re/commands 9): the original takes the count field
/// of the command as n, without an ownership or selectability filter - dead IDs count for the choice "one
/// unit or group". n = 1: the object must exist (even dying) and have module 0;
/// n > 1: empty the queue for each existing unit, the group is formed by `CreateGroup`
/// (only existing, non-dying units) or `Group_HasSameMembers`. Acknowledgment and emptying apply to
/// `units[0]` (= `ids[n−1]`), only if the object exists. [`owns`] counts for the verdict
/// (`Accepted`, `Partial` or `Rejected`), but filters nothing here.
pub(super) fn move_selection(
    sim: &mut Sim,
    player: u8,
    units: &[ObjectId],
    (x, z, target, formation, direction, spread, flags): (i32, i32, i32, i32, i32, i32, i32),
    direct: bool,
) {
    let own: Vec<ObjectId> = if script_path(sim) {
        for &u in units {
            owns(sim, player, u);
        }
        units.to_vec()
    } else {
        units
            .iter()
            .copied()
            .filter(|&u| owns(sim, player, u))
            .collect()
    };
    let Some(&first) = own.first() else { return };
    if direct {
        respond(sim, first, RESPONSE_EXECUTE);
    }
    if own.len() == 1 {
        // n = 1: the object must exist (script path: a stale ID does nothing).
        if sim.object(first).is_none() {
            return;
        }
        // n = 1: take it out of the old group, then walk alone (re/groups-formations 1.8, re/walk 15.1).
        let old = group::group_of(sim, first);
        group::remove_unit(sim, old, first, first, false);
        move_unit(sim, first, (x, z, target, direction, flags), direct);
        return;
    }
    if direct {
        for &u in &own {
            if let Some(o) = sim.object_mut(u) {
                o.commands.clear();
            }
        }
    }
    let mut order = GroupOrder {
        units: own.iter().rev().copied().collect(),
        x,
        z,
        formation,
        direction,
        spread,
        run: flags & 1 != 0,
    };
    let old = group::group_of(sim, first);
    if group::find(sim, old).is_some() && group::has_same_members(sim, old, &mut order) {
        group::realign(sim, old, (x, z, order.direction), spread, order.run);
        return;
    }
    let walk_flags = move_walk_flags(flags);
    let idle = Message::new(0x97F, &[]);
    if let Some(g) = group::create_group(sim, &order)
        && !group::move_in_formation(sim, g, walk_flags, Some(idle.clone()), Some(idle))
    {
        for &u in &order.units {
            group::remove_unit(sim, g, u, u, false);
        }
    }
}

/// `0x44` Formation (`GameWorld_CmdSetFormation` `0x0047E710`, re/groups-formations 2.2): only with n > 1
/// `MoveCommand(n, −1, −1, −1, formation, Fixed(−200), −1, 0)` to `CmdMoveSelection`. Same
/// selection -> realign with anchor -1 (moves nobody, K-222), otherwise re-forming around the
/// centroid; formation 0 dissolves the group (re/groups-formations 12.10).
pub(super) fn set_formation(
    sim: &mut Sim,
    player: u8,
    units: &[ObjectId],
    formation: i32,
    direct: bool,
) {
    let own: Vec<ObjectId> = units
        .iter()
        .copied()
        .filter(|&u| owns(sim, player, u))
        .collect();
    if own.len() > 1 {
        let order = (-1, -1, -1, formation, NO_DIRECTION, NO_SPREAD, 0);
        move_selection(sim, player, &own, order, direct);
    }
}

/// `0x75` Scatter (`GameWorld_CmdScatterSelection` `0x0047F530`, re/groups-formations 3.9): radially from the
/// centroid to 1.5 times the distance; with `direct` response sound and emptying the queue for each
/// unit (K-220); then leaving the group. Units exactly at the centroid stay in place
/// and in their group. Order = selection order (the command bar builds the list directly).
pub(super) fn scatter(sim: &mut Sim, player: u8, units: &[ObjectId], direct: bool) {
    let own: Vec<(ObjectId, (i32, i32))> = units
        .iter()
        .filter(|&&u| owns(sim, player, u))
        .filter_map(|&u| group::none::grid_pos(sim, u).map(|p| (u, p)))
        .collect();
    let k = own.len() as i32;
    if k <= 1 {
        return;
    }
    let (sx, sz) = own.iter().fold((0i32, 0i32), |a, e| {
        (a.0.wrapping_add(e.1.0), a.1.wrapping_add(e.1.1))
    });
    let c = (sx / k, sz / k);
    for (u, p) in own {
        let (dx, dz) = (p.0.wrapping_sub(c.0), p.1.wrapping_sub(c.1));
        let d = fixed_hypot(dx, dz);
        if d <= 0 {
            continue;
        }
        let phi = atan2_deg(dz, dx);
        let r = fixed_mul(d, 0x1800);
        let goal = (
            c.0.wrapping_add(fixed_mul(cos_deg(phi), r)),
            c.1.wrapping_add(fixed_mul(sin_deg(phi), r)),
        );
        if direct {
            respond(sim, u, RESPONSE_EXECUTE);
            if let Some(o) = sim.object_mut(u) {
                o.commands.clear();
            }
        }
        let idle = Message::new(0x97F, &[]);
        let env = Envelope {
            msg: find_to_msg(goal.0, goal.1, -1, NO_DIRECTION, 0, false, 0x201),
            success: Some(idle.clone()),
            fault: Some(idle),
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        post_command(sim, u, env, true);
        let gid = group::group_of(sim, u);
        group::remove_unit(sim, gid, u, u, false);
    }
}
