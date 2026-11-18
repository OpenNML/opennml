//! Dismounting and disembarking (re/riding 3.2, 8.7, 8.13, 8.15, 13.1): exit point from
//! `dummy01` of the mount (rules, rest pose) or from the ring search, completion with release via
//! the dying module (`0x810`, K-227); also letting the occupant off a sinking canoe (`0x70E`/`0x80E`, 3.3).
//!
//! The rider-mount link is accessed only through IDs with an existence check; read and copy first, then change.

use crate::fx::{Fx, FxVec3, fixed_mul};
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::Message;
use crate::messaging::{abort_all_modules, send, send_mut};
use crate::module::dying::target_object;
use crate::module::gather::list_at;
use crate::object::status;
use crate::sim::Sim;
use crate::world::set_grid_position;

use super::mount::remove_mounty_bonus;
use super::{list, riding, riding_mut};

/// Cost threshold "free" of the ring search (`[0x006719EC]` = 40).
const FREE_COST: i32 = 40;
/// Timer group of all riding timers (re/riding 3.7).
const TIMER_GROUP: u8 = 0x80;
/// Module PatternWalking (`0x2F`): `FinishDismount` puts it back into the tick.
const PATTERN_WALKING: usize = 0x2F;

/// `PropertyRiding_DismountStart` (`0x00522DF0`, `0x20D`/`0x20E` via `OnMessage`): determine the exit
/// point and dismount; return value = success.
///
/// With a target (`GameObject_GetTargetObject`, without checking the rider bits) the rider takes
/// `dummy01` of the mount (`Rules::mount_nodes`, rest pose, K-232) via `local_offset_to_grid` with
/// position and direction of the mount (without `+0x168`, 13.5); otherwise the ring search with radius
/// `hard(R) + soft(M)` on the layer of the mount (`0xB00`), starting 90 degrees after the viewing
/// direction of the rider. No space -> `false`, the link stays (K-230). The dismount list (140014) is
/// empty in the shipped data: then [`finish_dismount`] immediately without an animation (D-215); otherwise the
/// dead animation branch with `0x80D {x, z}` after time marker 1.
pub(crate) fn dismount_start(sim: &mut Sim, id: ObjectId, index: usize) -> bool {
    let Some(m) = target_object(sim, id) else {
        return false;
    };
    let Some((m_guid, m_grid, m_dir, m_soft)) = sim
        .object(m)
        .map(|o| (o.guid, o.grid, o.direction, o.soft_radius))
    else {
        return false;
    };
    let Some((r_dir, r_hard)) = sim.object(id).map(|o| (o.direction.0, o.hard_radius)) else {
        return false;
    };
    let (x, z) = match sim.rules.mount_nodes.get(&m_guid) {
        Some(&node) => {
            crate::footprint::local_offset_to_grid(node, m_grid, m_dir, sim.rules.map.cell)
        }
        None => {
            let radius = r_hard.wrapping_add(m_soft);
            // Layer -1 (mount without walk) lies outside 0 to 8: the terrain then costs nothing.
            let layer = send(sim, m, &Message::new(0xB00, &[]));
            let layer = usize::try_from(layer).unwrap_or(usize::MAX);
            let start = r_dir.wrapping_add(90 << 12);
            match find_free_spot_around(sim, id, radius, layer, start) {
                Some(p) => p,
                None => return false,
            }
        }
    };
    let anim = riding(sim, id, index).map_or(0, |r| list_at(&r.anims[list::UNMOUNT], 0));
    if anim == 0 {
        return finish_dismount(sim, id, index, true, x, z);
    }
    // Dead branch (dismount list empty in the shipped data, re/riding 8.1): the mount is aborted,
    // rider and mount play the same animation, `0x80D {x, z}` after time marker 1 completes it.
    abort_all_modules(sim, m);
    let guid = sim.object(id).map_or(0, |o| o.guid);
    let t = sim.rules.anim_label_time(guid, anim);
    if let Some(module) = sim.module_mut(id, index) {
        module
            .base
            .post_delayed_message(t, Message::new(0x80D, &[x, z]), false, TIMER_GROUP);
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, true, true, true);
    }
    if let Some(o) = sim.object_mut(m) {
        o.set_animation(anim, 0, false, false, false);
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    if let Some(r) = riding_mut(sim, id, index) {
        r.busy = true;
    }
    true
}

/// `PropertyRiding_FindFreeSpotAround` (`0x00521F90`, re/riding 3.2): 36 attempts `a = 0, 10, ...,
/// 350` degrees, point `(fixed_mul(cos(a + start), radius) + x, fixed_mul(sin(a + start), radius) +
/// z)` around the grid position of the rider; the first one with `QueryCostAt(rider, layer) < 40` wins.
///
/// K-268: While searching, the rider still carries `0x2000`/`0x8000` and is thus exempt from collision
/// (`IsCollisionExempt`): the footprint counts only the map edge; terrain, buildings and units
/// cost nothing. On the map the first point therefore almost always wins.
pub(crate) fn find_free_spot_around(
    sim: &mut Sim,
    id: ObjectId,
    radius: i32,
    layer: usize,
    start: i32,
) -> Option<(i32, i32)> {
    let (cx, cz) = sim.object(id).map(|o| (o.grid.x.0, o.grid.z.0))?;
    (0..36_i32).find_map(|k| {
        let a = ((k * 10) << 12).wrapping_add(start);
        let x = fixed_mul(cos_deg(a), radius).wrapping_add(cx);
        let z = fixed_mul(sin_deg(a), radius).wrapping_add(cz);
        (crate::collision::query_cost_at(sim, x, z, Some(id), layer, false, false) < FREE_COST)
            .then_some((x, z))
    })
}

/// `PropertyRiding_FinishDismount` (`0x00521AD0`, also `0x80D`): the rider jumps to `(x, z)`
/// (`keepY`; `set_grid_position` sets the drawing position equivalently), PatternWalking back into
/// the tick, release of the mount via the **dying module** (`0x810`, 8.7), rider bit off,
/// home = dismount point (`0x2500`) if one is set, mounty bonus gone, `ANIM_LOCKED` off,
/// not busy, `0x802`. A stand animation (`0x102`) only with a dismount animation (8.15);
/// `NotifyUiIfSelected` is omitted (deviation from the original). Always returns `true`.
pub(crate) fn finish_dismount(
    sim: &mut Sim,
    id: ObjectId,
    index: usize,
    no_anim: bool,
    x: i32,
    z: i32,
) -> bool {
    set_grid_position(sim, id, FxVec3::new(Fx(x), Fx(0), Fx(z)), true);
    if let Some(o) = sim.object_mut(id) {
        o.queue_module_tick_change(PATTERN_WALKING, true);
    }
    send(sim, id, &Message::new(0x810, &[]));
    let bit = riding(sim, id, index).map_or(0, super::Riding::status_bit);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(bit, false);
    }
    let mut home = Message::new(0x3300, &[-1, -1]);
    if send_mut(sim, id, &mut home) != 0 && home.arg(0) != -1 {
        send(sim, id, &Message::new(0x2500, &[]));
    }
    remove_mounty_bonus(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    if let Some(r) = riding_mut(sim, id, index) {
        r.busy = false;
    }
    send(sim, id, &Message::new(0x802, &[]));
    if !no_anim {
        let a = send(sim, id, &Message::new(0x102, &[]));
        if let Some(o) = sim.object_mut(id) {
            o.set_animation(a, 0, true, true, false);
        }
    }
    true
}

/// `PropertyRidingBoat_Query` `0x70E` (`0x005233A0`, re/riding 3.3; sender: `unlink` of the
/// dying canoe, re/combat-death 2.8): the dead answer 0. Otherwise the occupant plays the
/// dying animation of the boat (150004, `DIE[0]`) without a restart, is locked (`0x800000`) and
/// busy; `0x80E` after time marker 1 releases it ([`boat_sunk_done`]). Returns 1.
pub(crate) fn boat_sunk(sim: &mut Sim, id: ObjectId, index: usize) -> i32 {
    let Some((st, guid)) = sim.object(id).map(|o| (o.status, o.guid)) else {
        return 0;
    };
    if st & status::DIED != 0 {
        return 0;
    }
    let anim = riding(sim, id, index).map_or(0, |r| list_at(&r.anims[list::DIE], 0));
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, false, false, false);
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    if let Some(r) = riding_mut(sim, id, index) {
        r.busy = true;
    }
    let t = sim.rules.anim_label_time(guid, anim);
    if let Some(m) = sim.module_mut(id, index) {
        m.base
            .post_delayed_message(t, Message::new(0x80E, &[]), false, TIMER_GROUP);
    }
    1
}

/// `0x80E`: lock (`0x800000`) off, no longer busy; returns 1.
pub(crate) fn boat_sunk_done(sim: &mut Sim, id: ObjectId, index: usize) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    if let Some(r) = riding_mut(sim, id, index) {
        r.busy = false;
    }
    1
}
