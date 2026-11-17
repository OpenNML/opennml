//! Mounting and boarding (re/riding 3.1, 4.6): approach via walk `FINDTO_UNIT`,
//! change of owner of foreign mounts (`0x47F`, K-224), rider-mount link (`DoMount`), completion
//! (`FinishMount`) with the mounty bonus, substitute search after a failed attempt (`0xA0D`, K-226).
//!
//! The link is accessed only through IDs with an existence check; after `0x47F` the old ID of the
//! mount is no longer valid, the callback brings the new one.

use nml_formats::targets::MatchFilter;

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{abort_all_modules, dispatch_command, send, send_mut};
use crate::module::gather::list_at;
use crate::object::status;
use crate::sim::Sim;
use crate::world::{set_direction, set_grid_position};

use super::{HAS_LANCE, RIDING, abort, list, riding, riding_mut};

/// Key of the mounty modifier (`0x7E`, re/riding 4.6).
pub(crate) const MOUNTY_KEY: i32 = 0x7E;

/// Maximum distance of the approach (`[0x00672BC4]` = 2048, re/riding 3.1).
const APPROACH_DISTANCE: i32 = 2048;
/// Exemption zone "collision with the target exempted" (status `0x1000`).
const EXEMPT_ZONE: u32 = 0x1000;
/// Timer group of all riding timers (re/riding 3.7).
const TIMER_GROUP: u8 = 0x80;
/// Module PatternWalking (`0x2F`): `FinishMount` takes it out of the tick.
const PATTERN_WALKING: usize = 0x2F;

/// `PropertyRiding_MountStart` (`0x00521C80`, `0x10D`/`0x10E`): remember the target (even a missing one),
/// exemption zone on the target, walk `FINDTO_UNIT {target, 0, 2048, 0, -1, 1, 0, 0}` with failure
/// `0x30D`/`0x30E` and success `0x40D`/`0x40E {target}`; the approach runs if the walk accepts.
///
/// K-267 (order as in the original): if the approach ends synchronously (within range and facing
/// the target, the walk reports success still inside `dispatch_command`), `DoMount` has already set
/// `going_to_horse = false` and `0x1000` off - after that this call sets
/// `going_to_horse = true`. The flag then stays without an exemption zone; only a later
/// abort reads it (clears `0x1000` and the exemption object again, without effect).
pub(crate) fn mount_start(sim: &mut Sim, id: ObjectId, index: usize, target: ObjectId) {
    let Some(r) = riding_mut(sim, id, index) else {
        return;
    };
    r.mount_target = target;
    let (fault, success) = (r.fault_msg(), r.success_msg());
    // A missing target stays without feedback (the original checks before the rest).
    if sim.object(target).is_none() {
        return;
    }
    if let Some(r) = riding_mut(sim, id, index) {
        r.next_mount_anim = -1;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_collision_exempt_object(target);
        o.set_status_flags(EXEMPT_ZONE, true);
    }
    let t = target.as_i32();
    let env = Envelope {
        msg: Message::new(0x400, &[t, 0, APPROACH_DISTANCE, 0, -1, 1, 0, 0]),
        fault: Some(Message::new(fault, &[])),
        success: Some(Message::new(success, &[t])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    if dispatch_command(sim, id, &env) != 0
        && let Some(r) = riding_mut(sim, id, index)
    {
        r.going_to_horse = true;
    }
}

/// `PropertyRiding_MountDestReached` (`0x00522620`, `0x40D` query or `0x40E` via
/// `OnMessage`): target missing -> abort; same owner slot -> `DoMount`; otherwise `0x47F
/// {slot(R), R, success code}` to the mount (K-224): the change of owner in phase 0 calls
/// `0x40D`/`0x40E {new ID}` back.
pub(crate) fn mount_dest_reached(sim: &mut Sim, id: ObjectId, index: usize, mount: ObjectId) {
    let Some(r) = riding_mut(sim, id, index) else {
        return;
    };
    r.mount_target = mount;
    let code = r.success_msg();
    if sim.object(mount).is_none() {
        abort(sim, id, index);
        return;
    }
    if id.slot() == mount.slot() {
        do_mount(sim, id, index);
        return;
    }
    let msg = Message::new(0x47F, &[i32::from(id.slot()), id.as_i32(), code as i32]);
    send(sim, mount, &msg);
}

/// `PropertyRiding_DoMount` (`0x00522370`): returns 1, -1 or the value of [`finish_mount`].
///
/// Does not check `DIED` (K-266). A failure (target missing, already mounted, target carries; in the
/// reimplementation also: target is the rider itself or itself a rider or occupant, K-269) sets a timer `0xA0D`
/// (100 ms) in the **own** module - the low byte `0x0D` sends it to module 13 when it fires,
/// pure boat types do not have it (K-226). After `AbortAllModules`,
/// `GameObject_ClearField04` (`0x00467FA0`) clears the whole AI mark `ai_marks` of the mount (E-57,
/// re/ki-queries 5.9). The selection change is done by the viewer
/// (deviation from the original); `NotifyUiIfSelected` is omitted (deviation from the original).
pub(crate) fn do_mount(sim: &mut Sim, id: ObjectId, index: usize) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(EXEMPT_ZONE, false);
        o.exempt.object = ObjectId(0); // K-225: 0 instead of -1, only `+0x2C4`
    }
    let Some(r) = riding_mut(sim, id, index) else {
        return -1;
    };
    r.going_to_horse = false;
    let (mount, bit) = (r.mount_target, r.status_bit());
    let own = sim.object(id).map_or(0, |o| o.status);
    // K-269 (exception A7, re/riding 3.1, 8.18): a rider with itself as target (reachable only via
    // `0x10D`/`0x10E` with its own ID) would carry itself, `set_grid_position`
    // would follow the chain without end. If the target is itself a rider or occupant, both would refer
    // to each other (two-element ring, walk forwarding without end). In both cases it fails here
    // as with a target that already carries.
    let free = mount != id
        && sim.object(mount).is_some_and(|m| {
            m.status & (status::CARRIING | status::ISMOUNTED | status::ISMOUNTEDONBOAT) == 0
        });
    if !free || own & bit != 0 {
        if let Some(m) = sim.module_mut(id, index) {
            m.base
                .post_delayed_message(100, Message::new(0xA0D, &[]), false, TIMER_GROUP);
        }
        return -1;
    }
    abort_all_modules(sim, mount);
    if let Some(m) = sim.object_mut(mount) {
        m.ai_marks = 0;
    }
    // Home and tether of the still unmounted rider (own walk): `0x3300` always returns 1.
    let mut home = Message::new(0x3300, &[-1, -1]);
    let has_home = send_mut(sim, id, &mut home) != 0 && home.arg(0) != -1;
    let tether = send(sim, id, &Message::new(0x2200, &[]));
    let behaviour = [
        1,
        i32::from(has_home),
        if has_home { tether } else { -1 },
        0,
        0,
    ];
    send(sim, mount, &Message::new(0x2100, &behaviour));
    if let Some(o) = sim.object_mut(id) {
        o.target = mount;
        o.set_status_flags(bit, true);
    }
    if let Some(m) = sim.object_mut(mount) {
        m.target = id;
        m.set_status_flags(status::CARRIING, true);
    }
    // Cancel camouflage: Masking `0x0051D800` (module 25), answer 1; pending `0x319`/`0x419`
    // stay (K-302).
    if sim
        .object(id)
        .is_some_and(|o| o.status & status::MASKED != 0)
    {
        send(sim, id, &Message::new(0x619, &[]));
    }
    let Some(r) = riding(sim, id, index) else {
        return -1;
    };
    let place = match r.next_mount_anim {
        0 => list::MOUNT_LEFT,
        1 => list::MOUNT_RIGHT,
        _ => return finish_mount(sim, id, index),
    };
    let anim = list_at(&r.anims[place], 0);
    if anim == 0 {
        return finish_mount(sim, id, index);
    }
    // Dead branch (K-223, `next_mount_anim` is always -1 in the game): mounting animation, then
    // `0x70D` after time marker 1.
    let Some((grid, dir)) = sim.object(mount).map(|m| (m.grid, m.direction.0)) else {
        return -1;
    };
    set_grid_position(sim, id, grid, true);
    set_direction(sim, id, dir);
    let guid = sim.object(id).map_or(0, |o| o.guid);
    let t = sim.rules.anim_label_time(guid, anim);
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, true, true, false);
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    if let Some(m) = sim.module_mut(id, index) {
        m.base
            .post_delayed_message(t, Message::new(0x70D, &[]), false, TIMER_GROUP);
    }
    if let Some(r) = riding_mut(sim, id, index) {
        r.busy = true;
    }
    1
}

/// `PropertyRiding_FinishMount` (`0x005219B0`, also `0x70D`): the rider jumps to the position of the
/// mount (`keepY`), takes over presentation position and direction, gets the mounty bonus, loses
/// the load (W-205); stand crossfade off and idle anew (`0x202 {1}` draws `rand()`),
/// PatternWalking out of the tick. Returns -1 without a mount, otherwise 1.
pub(crate) fn finish_mount(sim: &mut Sim, id: ObjectId, index: usize) -> i32 {
    let Some(r) = riding_mut(sim, id, index) else {
        return -1;
    };
    r.busy = false;
    let mount = r.mount_target;
    let Some((grid, world_pos, dir)) = sim
        .object(mount)
        .map(|m| (m.grid, m.world_pos, m.direction.0))
    else {
        return -1;
    };
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    set_grid_position(sim, id, grid, true);
    if let Some(o) = sim.object_mut(id) {
        o.world_pos = world_pos;
    }
    set_direction(sim, id, dir);
    apply_mounty_bonus(sim, id, index);
    crate::module::gather::set_carried_load(sim, id, 0, -1);
    send(sim, id, &Message::new(0x802, &[]));
    send(sim, id, &Message::new(0x202, &[1]));
    if let Some(o) = sim.object_mut(id) {
        o.queue_module_tick_change(PATTERN_WALKING, false);
    }
    1
}

/// `PropertyRiding_SearchMount` (`0x005229B0`, `0xA0D`, always module 13): nearest free
/// SADDLE animal within the radius `sight << 10` (filter `NOTSTATUS|ANDALL|HANDLETYPE`, status `CARRIING`,
/// walk flag `0x8000`; any owner) -> new approach; otherwise abort.
pub(crate) fn search_mount(sim: &mut Sim, id: ObjectId) {
    let Some(pos) = sim.object(id).map(|o| (o.grid.x.0, o.grid.z.0)) else {
        return;
    };
    let radius = sim.attribute(id, 18) << 10;
    let filter = MatchFilter {
        mode: 0x2012,
        status: status::CARRIING,
        match_bits: 0x8000,
        ..MatchFilter::constructed()
    };
    let player = i32::from(id.slot());
    match crate::collision::find_nearest_matching(sim, pos, &filter, radius, player, -1) {
        Some(m) => mount_start(sim, id, RIDING, m),
        None => {
            abort(sim, id, RIDING);
        }
    }
}

/// `PropertyRiding_ApplyMountyBonus` (`0x00522890`, vf+0x4C, both modules; C-136, C-218): for
/// each `(attr, pct)` from `MOUNTY_BONUS` (by attribute), if present, the
/// percentage modifier with key `0x7E`; with `HAS_LANCE` of the module `UpdateVariantMask(-1, 0)`.
pub(crate) fn apply_mounty_bonus(sim: &mut Sim, id: ObjectId, index: usize) {
    let rules = sim.rules;
    if let Some(o) = sim.object_mut(id) {
        for (&attr, &pct) in &rules.bonus.mounty {
            let attr = attr as u32;
            if o.attributes.is_present(attr) {
                o.attributes.set_modifier(attr, pct, true, MOUNTY_KEY);
            }
        }
    }
    if sim
        .module(id, index)
        .is_some_and(|m| m.base.has_flags(HAS_LANCE))
    {
        crate::variants::update_variant_mask(sim, id, u32::MAX, 0);
    }
}

/// `PropertyRiding_RemoveMountyBonus` (`0x00522930`, vf+0x48, from `FinishDismount`): remove the
/// modifier `0x7E` of each attribute from `MOUNTY_BONUS`.
pub(crate) fn remove_mounty_bonus(sim: &mut Sim, id: ObjectId) {
    let rules = sim.rules;
    if let Some(o) = sim.object_mut(id) {
        for &attr in rules.bonus.mounty.keys() {
            o.attributes.remove_modifier(attr as u32, MOUNTY_KEY);
        }
    }
}
