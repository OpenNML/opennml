//! Module 11 `CPropertyMultiHead` (front part) and 51 `CPropertyMultiPart` (rear part) of the
//! multipart objects (re/train-multipart 1.2, 1.3): carts and stagecoach. This part holds
//! the states (without a load check: `fixed_mul` also terminates for the drawbar `i32::MIN`, K-324)
//! and the wiring in `module::mod`; also the creation of the rear part
//! 100 ms after the init (3.7), the following with bend limit +/-90 degrees (3.8) in the tick of the
//! front part, death, removal and change of owner. The frame path of the original (`FrameUpdate`,
//! re/train-multipart 3.8, K-247) is missing in the simulation; the host computes it with [`follow_angle`].

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::fx::{Fx, FxVec3, fixed_mul};
use crate::id::ObjectId;
use crate::math::{atan2_deg, cos_deg, sin_deg};
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::object::GameObject;
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::{
    ObjectDesc, create_object, desc_flags, grid_to_world, queue_owner_change, set_direction,
    set_grid_position, wrap_angle,
};

use super::events::update_events;
use super::{ModuleKind, init_flags};

/// Module index MultiHead (front part).
pub const HEAD: usize = 0x0B;
/// Module index MultiPart (rear part).
pub const PART: usize = 0x33;

/// `CPropertyMultiHead` (`0x88` bytes, constructor `0x0051DD30`); in the savegame `m_LinkedHID` and
/// `m_LastPos` (`0x0051E5D0`), plus the type of the rear part, which the original reads again from the
/// parameters (re/train-multipart 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiHead {
    /// `m_LinkedHID` (`+0x70`): the rear part.
    pub linked: ObjectId,
    /// Type of the rear part (`+0x74`, 120001).
    pub part_guid: i32,
    /// `m_LastPos` (`+0x78`): last reported position (x, z, direction; fixed point).
    pub last_pos: (i32, i32, i32),
}

impl Default for MultiHead {
    fn default() -> Self {
        Self {
            linked: ObjectId::NONE,
            part_guid: 0,
            last_pos: (0, 0, 0),
        }
    }
}

/// `CPropertyMultiPart` (`0x98` bytes, constructor `0x0051E610`), fields of the savegame
/// (`0x0051EF80`, re/train-multipart 1.3, 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiPart {
    /// `m_PreviousHID` (`+0x70`): the front part.
    pub prev: ObjectId,
    /// `m_NextHID` (`+0x74`): further rear part (only with 520001 > 0; never in the shipped data).
    pub next: ObjectId,
    /// `m_CentreOffset` (`+0x78`, 520006): drawbar length (fixed point).
    pub offset: i32,
    /// `m_CentreOfRotation` (`+0x7C`): followed drawbar point (x, z, direction).
    pub centre: (i32, i32, i32),
    /// `m_WalkAnim`, `m_IdleAnim`, `m_DieAnim` (520004, 520003, 520005).
    pub walk_anim: i32,
    pub idle_anim: i32,
    pub die_anim: i32,
}

impl Default for MultiPart {
    fn default() -> Self {
        Self {
            prev: ObjectId::NONE,
            next: ObjectId::NONE,
            offset: 0,
            centre: (0, 0, 0),
            walk_anim: 0,
            idle_anim: 0,
            die_anim: 0,
        }
    }
}

/// Creation flags of the rear part (`0x8D3`: type, position, rotation y, slot, color, ready).
const TAIL_DESC_FLAGS: u32 = desc_flags::GUID
    | desc_flags::POSITION
    | desc_flags::ROT_Y
    | desc_flags::PLAYER
    | desc_flags::COLOR
    | desc_flags::READY;
/// Creation flags of the further link of a chain (`0x8C3`, without rotation).
const LINK_DESC_FLAGS: u32 = desc_flags::START_OBJECT;
/// Delay of the `0x40B` after the init in ms (`0x0051DE00`).
const CREATE_DELAY_MS: i32 = 100;
/// Event group of the delayed `0x40B`.
const EVENT_GROUP: u8 = 0x80;
/// Walk spin threshold of the front parts in degrees (`0x2F00 {180}`, re/train-multipart 4.3).
const SPIN_THRESHOLD_DEG: i32 = 180;
/// Largest bend of the rear part to the head direction (statically `Fixed_FromInt(0x5A)`).
const MAX_BEND: i32 = 90 << 12;
/// Upper limit of the steps of `0x633` along the predecessors (protection against rings that only a
/// corrupted savegame contains; the load check allows them).
const MAX_CHAIN: usize = 64;

/// Player color of the slot (`GameWorld_ChangeObjectOwner`/`OnOwnerChanged` use the same
/// table; without a player: 8).
fn player_color(sim: &Sim, slot: u8) -> i32 {
    sim.state
        .players
        .get(usize::from(slot))
        .map_or(8, |p| p.color)
}

/// Front part state of the object.
pub fn head<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s MultiHead> {
    match &sim.module(id, HEAD)?.kind {
        ModuleKind::MultiHead(h) => Some(h),
        _ => None,
    }
}

pub fn head_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut MultiHead> {
    match &mut sim.module_mut(id, HEAD)?.kind {
        ModuleKind::MultiHead(h) => Some(h),
        _ => None,
    }
}

/// Rear part state of the object.
pub fn part<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s MultiPart> {
    match &sim.module(id, PART)?.kind {
        ModuleKind::MultiPart(p) => Some(p),
        _ => None,
    }
}

pub fn part_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut MultiPart> {
    match &mut sim.module_mut(id, PART)?.kind {
        ModuleKind::MultiPart(p) => Some(p),
        _ => None,
    }
}

/// `PropertyMultiHead_Init` (`0x0051DE00`, slot 9, re/train-multipart 3.7): flags, `ReadParams`,
/// then the delayed `0x40B` (100 ms, once, group `0x80`) that creates the rear part. Like
/// every timer it counts from the next `Module_UpdateEvents` and fires only when "greater"
/// (K-275): the rear part arises in the third object tick.
pub fn init_head(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(HEAD, desc);
    if let Some(m) = sim.module_mut(id, HEAD) {
        m.base.flags = flags;
    }
    read_params_head(sim, id, desc);
    if let Some(m) = sim.module_mut(id, HEAD) {
        m.base.post_delayed_message(
            CREATE_DELAY_MS,
            Message::new(0x40B, &[]),
            false,
            EVENT_GROUP,
        );
    }
}

/// `PropertyMultiPart_Init` (`0x0051E6F0`, slot 9, re/train-multipart 3.8): flags, `ReadParams`,
/// render bits (`= 3`, re/object-classes), not selectable, idle animation.
pub fn init_part(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(PART, desc);
    if let Some(m) = sim.module_mut(id, PART) {
        m.base.flags = flags;
    }
    read_params_part(sim, id, desc);
    let idle = part(sim, id).map_or(0, |p| p.idle_anim);
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = 3;
        o.selectable = false;
        o.set_animation(idle, 0, false, false, false);
    }
}

/// `PropertyMultiHead_ReadParams` (`0x0051DE50`, slot 4, re/train-multipart 3.7; also on a
/// variant switch): last reported position = own position, type of the rear part (120001), and the
/// spin threshold 180 degrees to the walk (`0x2F00`): carts turn via the turning radius.
pub fn read_params_head(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let Some(o) = sim.object(id) else { return };
    let pos = (o.grid.x.0, o.grid.z.0, o.direction.0);
    let guid = desc.value_int(120001).unwrap_or(0) as i32;
    if let Some(h) = head_mut(sim, id) {
        h.last_pos = pos;
        h.part_guid = guid;
    }
    send(sim, id, &Message::new(0x2F00, &[SPIN_THRESHOLD_DEG]));
}

/// `PropertyMultiPart_ReadParams` (`0x0051ED60`, slot 4, re/train-multipart 3.8): drawbar length
/// (520006), followed drawbar point `position − direction · length`, hard and soft radius
/// (520002), status `0x1000` with itself as exempt object (K-249), animations (520004
/// walking, 520003 resting, 520005 dying). 520001 > 0 creates a further link (chain, never in the
/// shipped data).
pub fn read_params_part(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |k: u32| desc.value_int(k).unwrap_or(0) as i32;
    let offset = value(520006);
    let Some(o) = sim.object(id) else { return };
    let (grid, d, guid) = (o.grid, o.direction.0, o.guid);
    let centre = (
        grid.x.0.wrapping_sub(fixed_mul(cos_deg(d), offset)),
        grid.z.0.wrapping_sub(fixed_mul(sin_deg(d), offset)),
        d,
    );
    if let Some(p) = part_mut(sim, id) {
        p.offset = offset;
        p.centre = centre;
        p.walk_anim = value(520004);
        p.idle_anim = value(520003);
        p.die_anim = value(520005);
    }
    if let Some(o) = sim.object_mut(id) {
        o.hard_radius = value(520002);
        o.soft_radius = value(520002);
        o.set_status_flags(0x1000, true);
        o.set_collision_exempt_object(id);
    }
    let next_guid = value(520001);
    if next_guid > 0 {
        // A chain with the own type would create without end (data, never in the shipped data).
        if next_guid as u32 == guid {
            log::warn!("Rear part {id:?}: type {guid} names itself as a further link");
            return;
        }
        create_link(sim, id, next_guid);
    }
}

/// Further link behind the rear part `id` (`0x0051ED60`): flags `0x8C3`, position shifted by twice the
/// hard radius to the rear (x); if the creation fails, `next` stays -1.
fn create_link(sim: &mut Sim, id: ObjectId, guid: i32) {
    let Some(o) = sim.object(id) else { return };
    let (grid, hard, slot) = (o.grid, o.hard_radius, id.slot());
    let cell = sim.rules.map.cell;
    let at = FxVec3::new(
        Fx(grid.x.0.wrapping_add(hard.wrapping_mul(2))),
        grid.y,
        grid.z,
    );
    let mut desc = ObjectDesc {
        flags: LINK_DESC_FLAGS,
        guid: guid as u32,
        player: i32::from(slot),
        position: grid_to_world(at, cell),
        scale: [1.0; 3],
        rotation: [0.0; 3],
        color: player_color(sim, slot),
    };
    let next = create_object(sim, &mut desc).unwrap_or(ObjectId::NONE);
    if let Some(p) = part_mut(sim, id) {
        p.next = next;
    }
    if !next.is_none() {
        send(sim, next, &Message::new(0x433, &[id.as_i32()]));
    }
}

/// `MultiHead_CreatePart` (`0x0051E1C0`, re/train-multipart 3.7): create the rear part `guid` at the position
/// of the front part (flags `0x8D3`, same slot, player color, rotation like
/// `set_direction`), then set `linked`, transfer the script name `<name>tail`,
/// query the hard radius `r` of the rear part
/// (`0x133`) and add it to the front part (hard `+ r`, soft `+ 2r`), send `0x433 {front part}` and
/// the last reported position (`0x233`). The radii are only on the object; the
/// collision lists store none, so no re-following is needed. `guid ≤ 0` does nothing; if
/// the creation fails, `linked` = -1.
pub fn create_part(sim: &mut Sim, head_id: ObjectId, guid: i32) {
    if guid <= 0 {
        return;
    }
    let Some(o) = sim.object(head_id) else { return };
    let (grid, dir, slot) = (o.grid, o.direction.0, head_id.slot());
    let cell = sim.rules.map.cell;
    let mut desc = ObjectDesc {
        flags: TAIL_DESC_FLAGS,
        guid: guid as u32,
        player: i32::from(slot),
        position: grid_to_world(grid, cell),
        scale: [1.0; 3],
        // Like `world::set_direction`: `rad(dir + 90°)`.
        rotation: [
            0.0,
            dir.wrapping_add(90 << 12) as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35),
            0.0,
        ],
        color: player_color(sim, slot),
    };
    let created = create_object(sim, &mut desc);
    let Some(new) = created else {
        if let Some(h) = head_mut(sim, head_id) {
            h.linked = ObjectId::NONE;
        }
        return;
    };
    let Some(h) = head_mut(sim, head_id) else {
        return;
    };
    h.linked = new;
    let last = h.last_pos;
    // `0x0051E2DE`: script name `<name>tail` (`FUN_0043E310`, E-146 corrected).
    crate::names::transfer(&mut sim.state.names, head_id, new, "tail");
    let r = send(sim, new, &Message::new(0x133, &[]));
    if let Some(o) = sim.object_mut(head_id) {
        o.hard_radius = o.hard_radius.wrapping_add(r);
        o.soft_radius = o.soft_radius.wrapping_add(r.wrapping_mul(2));
    }
    send(sim, new, &Message::new(0x433, &[head_id.as_i32()]));
    send(sim, new, &Message::new(0x233, &[last.0, last.1, last.2]));
}

/// Angle of the rear part to the head (`MultiPart_FollowHead` `0x0051E860`, re/train-multipart 3.8):
/// the angle from `centre` to the head `(x, z, dir)` (`FixedPos_AngleTo`, 0 degrees = +x), bent at most
/// +/-90 degrees against the head direction: if the bend is `> 90°`, `dir + 90°` applies, if it is
/// `< −90°`, `dir − 90°` applies (folded). Pure, without state; the host uses it for
/// the frame display.
pub fn follow_angle(centre: (i32, i32), head: (i32, i32, i32)) -> i32 {
    let a = atan2_deg(head.1.wrapping_sub(centre.1), head.0.wrapping_sub(centre.0));
    let k = wrap_angle(a.wrapping_sub(head.2));
    if k > MAX_BEND {
        wrap_angle(head.2.wrapping_add(MAX_BEND))
    } else if k < -MAX_BEND {
        wrap_angle(head.2.wrapping_sub(MAX_BEND))
    } else {
        a
    }
}

/// New drawbar point (x, z, direction): `head − (cos a, sin a) · length` with the angle `a` from
/// [`follow_angle`].
pub fn follow_centre(centre: (i32, i32), offset: i32, head: (i32, i32, i32)) -> (i32, i32, i32) {
    let a = follow_angle(centre, head);
    (
        head.0.wrapping_sub(fixed_mul(cos_deg(a), offset)),
        head.1.wrapping_sub(fixed_mul(sin_deg(a), offset)),
        a,
    )
}

/// `MultiPart_FollowHead(pos, sim = 1)` (`0x0051E860`, re/train-multipart 3.8) from the tick of the
/// front part (`0x233`): follow the drawbar point, set the rear part to the position of the
/// front part (`SetGridPosition(x, z, y = 0, keepY)`), turn in the direction `a` and request the
/// walking animation. The frame path of the original (`FrameUpdate`, `sim = 0`, writes
/// the same state fields depending on the frame rate) is missing in the simulation (K-247);
/// the host computes it with [`follow_angle`] without state.
pub fn follow_head(sim: &mut Sim, part_id: ObjectId, head: (i32, i32, i32)) {
    let Some(p) = part(sim, part_id) else { return };
    let (centre, offset, walk) = (p.centre, p.offset, p.walk_anim);
    let new = follow_centre((centre.0, centre.1), offset, head);
    if let Some(p) = part_mut(sim, part_id) {
        p.centre = new;
    }
    set_grid_position(
        sim,
        part_id,
        FxVec3::new(Fx(head.0), Fx(0), Fx(head.1)),
        true,
    );
    set_direction(sim, part_id, new.2);
    if let Some(o) = sim.object_mut(part_id) {
        o.set_animation(walk, 0, false, false, false);
    }
}

/// Rear part state of an object.
fn part_of(o: &GameObject) -> Option<&MultiPart> {
    match &o.module(PART)?.kind {
        ModuleKind::MultiPart(p) => Some(p),
        _ => None,
    }
}

/// `true` if the chain of references `step` (`next` or `prev`) runs in a cycle from the rear part `start`
/// (also a self-reference, also a ring that the chain enters only after some links). The chain
/// ends at a missing ID or an object without module 51. Only a corrupted or
/// manipulated state has rings; `0x133` only passes on without a ring.
pub fn chain_loops_in(
    state: &SimState,
    start: ObjectId,
    step: impl Fn(&MultiPart) -> ObjectId,
) -> bool {
    let mut seen = vec![start];
    let mut cur = state
        .object(start, false)
        .and_then(part_of)
        .map_or(ObjectId::NONE, &step);
    loop {
        if cur.is_none() {
            return false;
        }
        let Some(o) = state.object(cur, false) else {
            return false;
        };
        if seen.contains(&cur) {
            return true;
        }
        seen.push(cur);
        let Some(p) = part_of(o) else {
            return false;
        };
        cur = step(p);
    }
}

/// `PropertyMultiHead_Query` (`0x0051E4E0`, slot 7, re/train-multipart 2.3); unknown codes
/// return -1 (`Stub_ReturnMinusOne`).
pub fn query_head(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        // Create the rear part, only without an existing link (sender: siege engine).
        0x10B => {
            if let Some(h) = head(sim, id)
                && h.linked.is_none()
            {
                let guid = h.part_guid;
                create_part(sim, id, guid);
            }
            1
        }
        // Queue the rear part for removal (`0x0051DEE0`), break the link.
        0x20B => {
            if let Some(h) = head_mut(sim, id) {
                let linked = std::mem::replace(&mut h.linked, ObjectId::NONE);
                crate::world::queue_destroy(sim, linked);
            }
            1
        }
        // Death of the front part (`0x0051DF00`): the rear part dies with it (`0x110`).
        0x30B => {
            if let Some(h) = head_mut(sim, id) {
                let linked = std::mem::replace(&mut h.linked, ObjectId::NONE);
                send(sim, linked, &Message::new(0x110, &[]));
            }
            1
        }
        // Own delayed creation: without siege module `0x2E` and without a -1 check (K-248).
        0x40B => {
            if sim.object(id).is_some_and(|o| !o.has_module(0x2E, false))
                && let Some(guid) = head(sim, id).map(|h| h.part_guid)
            {
                create_part(sim, id, guid);
            }
            1
        }
        // Interpolated position of the walk (`0x3600`): the sender is only the rear part in the frame path
        // (re/train-multipart 3.8), in the simulation the code has no sender.
        0x50B => {
            msg.code = 0x3600;
            send_mut(sim, id, msg);
            1
        }
        0x60B => 1,
        0x70B => head(sim, id).map_or(-1, |h| h.linked.as_i32()),
        0x80B => {
            let hid = ObjectId::from_i32(msg.arg(0));
            if let Some(h) = head_mut(sim, id) {
                h.linked = hid;
            }
            1
        }
        _ => -1,
    }
}

/// `PropertyMultiPart_Query` (`0x0051EB60`, slot 7, re/train-multipart 2.4); unknown codes
/// return -1.
pub fn query_part(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        // Hard radius; pass on to a further link whose answer is discarded. A
        // ring in the state (only manipulated) passes nothing on instead of recursing endlessly.
        0x133 => {
            let next = part(sim, id).map_or(ObjectId::NONE, |p| p.next);
            if !next.is_none() && !chain_loops_in(sim.state, id, |p| p.next) {
                send(sim, next, &Message::new(0x133, &[]));
            }
            sim.object(id).map_or(0, |o| o.hard_radius)
        }
        // Position of the front part from its tick.
        0x233 => {
            follow_head(sim, id, (msg.arg(0), msg.arg(1), msg.arg(2)));
            0
        }
        // Front part stands (turning on the spot is not reported, K-248): idle animation.
        0x333 => {
            let idle = part(sim, id).map_or(0, |p| p.idle_anim);
            if let Some(o) = sim.object_mut(id) {
                o.set_animation(idle, 0, false, false, false);
            }
            0
        }
        // Front part (`0x433`) or new ID of the front part after the change of owner (`0x733`).
        0x433 | 0x733 => {
            let prev = ObjectId::from_i32(msg.arg(0));
            if let Some(p) = part_mut(sim, id) {
                p.prev = prev;
            }
            0
        }
        // Head of the chain (`0x0051E7E0`): a predecessor with module 51 passes it on, otherwise the
        // predecessor is the head; if it is missing, -1. Iterative and bounded (rings only in the manipulated
        // state).
        0x633 => {
            let mut cur = id;
            for _ in 0..MAX_CHAIN {
                let prev = part(sim, cur).map_or(ObjectId::NONE, |p| p.prev);
                let Some(o) = sim.object(prev) else {
                    return -1;
                };
                if !o.has_module(PART, false) {
                    return prev.as_i32();
                }
                cur = prev;
            }
            -1
        }
        _ => -1,
    }
}

/// `PropertyMultiHead_Tick` (`0x0051DFE0`, slot 13, re/train-multipart 3.7): timers, then
/// (after the walk tick, module 0 < 11) report the position: if (x, z) has
/// changed since the last report, `0x233 {x, z, direction}` to the rear part, otherwise `0x333`. Only the
/// direction does not count (K-248). The rear part has no tick (`0x004FF840`, empty).
pub fn tick_head(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, HEAD, dt);
    let Some(h) = head(sim, id) else { return };
    let (linked, last) = (h.linked, h.last_pos);
    // The original fetches the rear part with `GameWorld_GetObjectById(m_LinkedHID, 0)` (`0x0051DFE0`),
    // dying objects included. `0x233`/`0x333` therefore also go to a dying rear part;
    // `sim.object(linked)` models that because it looks up without a `DIED` filter.
    if sim.object(linked).is_none() {
        return;
    }
    let Some(o) = sim.object(id) else { return };
    let cur = (o.grid.x.0, o.grid.z.0, o.direction.0);
    if (cur.0, cur.1) != (last.0, last.1) {
        send(sim, linked, &Message::new(0x233, &[cur.0, cur.1, cur.2]));
        if let Some(h) = head_mut(sim, id) {
            h.last_pos = cur;
        }
    } else {
        send(sim, linked, &Message::new(0x333, &[]));
    }
}

/// Change of owner of the front part (`0x0051DF50`, slot 5, re/train-multipart 3.7; `id` is the
/// new ID): an existing rear part gets `0x733 {new}` and is queued into the slot of the
/// new owner (`QueueOwnerChange`, it changes in the following pass).
pub fn on_owner_changed_head(sim: &mut Sim, id: ObjectId, old: ObjectId) {
    let _ = old;
    let Some(linked) = head(sim, id).map(|h| h.linked) else {
        return;
    };
    if sim.object(linked).is_none() {
        return;
    }
    send(sim, linked, &Message::new(0x733, &[id.as_i32()]));
    queue_owner_change(sim, linked, id.slot());
}

/// Change of owner of the rear part (`0x0051E740`, slot 5, re/train-multipart 3.7; `id` is the
/// new ID): the front part learns it (`0x80B {new}`), and the exempt object of the
/// collision is the new own ID - without a front part only the second applies.
pub fn on_owner_changed_part(sim: &mut Sim, id: ObjectId, old: ObjectId) {
    let _ = old;
    let prev = part(sim, id).map_or(ObjectId::NONE, |p| p.prev);
    if sim.object(prev).is_some() {
        send(sim, prev, &Message::new(0x80B, &[id.as_i32()]));
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_collision_exempt_object(id);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::fx::{Fx, FxVec3};
    use crate::module::test_hook;
    use crate::module::train::testkit::*;
    use crate::module::walk::walk;
    use crate::object::status;
    use crate::rules::Rules;
    use crate::state::{SimEvent, SimState};
    use crate::tick::step;
    use crate::world::{
        change_owner, destroy_object, process_owner_changes, set_direction, set_grid_position,
        wrap_angle,
    };

    /// Center of tile 20 in fixed point.
    const MID: i32 = 20 * 4096 + 2048;

    /// World with a front part `guid` (slot 0) on tile (20, 20), still without a tick.
    fn world_with(guid: u32) -> (Rules, SimState, ObjectId) {
        let rules = rules();
        let mut state = state(&rules);
        let head = {
            let mut sim = Sim::new(&mut state, &rules);
            spawn(&mut sim, guid, 0, (20, 20))
        };
        (rules, state, head)
    }

    /// `n` object ticks (two `step` calls each).
    fn ticks(state: &mut SimState, rules: &Rules, n: usize) {
        for _ in 0..2 * n {
            step(state, rules, &[]);
        }
    }

    /// Front part with rear part: after three object ticks (K-275: the 100 ms timer fires in the
    /// third call of `update_events`).
    fn with_tail(guid: u32) -> (Rules, SimState, ObjectId, ObjectId) {
        let (rules, mut state, cart) = world_with(guid);
        ticks(&mut state, &rules, 3);
        let tail = head(&Sim::new(&mut state, &rules), cart).unwrap().linked;
        assert!(!tail.is_none(), "rear part created");
        (rules, state, cart, tail)
    }

    /// Shift the head by `(dx, dz)` and optionally rotate.
    fn move_head(sim: &mut Sim, cart: ObjectId, dx: i32, dz: i32, dir: Option<i32>) {
        let g = sim.object(cart).unwrap().grid;
        set_grid_position(
            sim,
            cart,
            FxVec3::new(Fx(g.x.0 + dx), g.y, Fx(g.z.0 + dz)),
            true,
        );
        if let Some(d) = dir {
            set_direction(sim, cart, d << 12);
        }
    }

    fn centre_of(sim: &Sim, tail: ObjectId) -> (i32, i32, i32) {
        part(sim, tail).unwrap().centre
    }

    // QUIRK K-249, K-279: kept
    #[test]
    fn the_head_creates_its_tail_after_100_ms_and_grows_its_radii() {
        // Create the stagecoach 2611 directly: no rear part before the first tick; the 0x40B
        // (100 ms) fires like every timer only in the third object tick (K-275: taken over in the
        // first, 100 not > 100 in the second, 200 in the third).
        let (rules, mut state, cart) = world_with(CART);
        {
            let sim = Sim::new(&mut state, &rules);
            let h = head(&sim, cart).unwrap();
            assert_eq!(h.linked, ObjectId::NONE);
            assert_eq!(h.part_guid, i32::try_from(CART_TAIL).unwrap());
            assert_eq!(h.last_pos, (MID, MID, 0), "ReadParams: own position");
            // Walk special path 0x2F00 {180}: spin threshold 180 degrees.
            assert_eq!(walk(&sim, cart).unwrap().speed.spin_threshold, 180 << 12);
            let o = sim.object(cart).unwrap();
            assert_eq!((o.hard_radius, o.soft_radius), (1900, 4096));
            let timer = sim
                .module(cart, HEAD)
                .unwrap()
                .base
                .timers_ws
                .front()
                .unwrap();
            assert_eq!((timer.msg.code, timer.b, timer.group()), (0x40B, 100, 0x80));
        }
        for n in 1..=2 {
            ticks(&mut state, &rules, 1);
            let sim = Sim::new(&mut state, &rules);
            assert!(head(&sim, cart).unwrap().linked.is_none(), "tick {n}");
        }
        ticks(&mut state, &rules, 1);
        let sim = Sim::new(&mut state, &rules);
        let tail = head(&sim, cart).unwrap().linked;
        assert!(!tail.is_none(), "third tick");
        let t = sim.object(tail).unwrap();
        assert_eq!(t.guid, CART_TAIL);
        // 0x8D3: type, position, rotation, slot, color, ready (counts toward the object counter).
        assert_eq!(tail.slot(), 0);
        assert_eq!(sim.state.guid_counts[0][&CART_TAIL], 1);
        assert_eq!(t.color, sim.object(cart).unwrap().color);
        // Front part 1900 + 1900 / 4096 + 2 * 1900; rear part hard = soft = 1900.
        let o = sim.object(cart).unwrap();
        assert_eq!((o.hard_radius, o.soft_radius), (3800, 7896));
        assert_eq!((t.hard_radius, t.soft_radius), (1900, 1900));
        // Status 0x1000, exempt object = itself (K-249), not selectable, render bits 3.
        assert_ne!(t.status & 0x1000, 0);
        assert_eq!((t.exempt.object, t.exempt.mode), (tail, 0));
        assert!(!t.selectable);
        assert_eq!(t.render_bits, 3);
        // Idle animation 0 (the head stood still: 0x333), drawbar 5300.
        assert_eq!(t.anim.wanted, 0);
        let p = part(&sim, tail).unwrap();
        assert_eq!((p.prev, p.next, p.offset), (cart, ObjectId::NONE, 5300));
        assert_eq!((p.walk_anim, p.idle_anim, p.die_anim), (1, 0, 2));
        assert_eq!(p.centre, (MID - 5300, MID, 0));
        // The rear part stands at the position of the front part.
        assert_eq!((t.grid.x.0, t.grid.z.0), (MID, MID));
        // The timer is used up: no second rear part.
        let before = sim.state.guid_counts[0][&CART_TAIL];
        ticks(&mut state, &rules, 5);
        assert_eq!(
            Sim::new(&mut state, &rules).state.guid_counts[0][&CART_TAIL],
            before
        );
    }

    // QUIRK E-146: kept
    #[test]
    fn multipart_tail_gets_name_tail() {
        // Named stagecoach (like `.wld` `OBJ_NAME`, entered on creation): the rear part
        // arises only with the delayed 0x40B and is called `<name>tail` (`FUN_0051E1C0` ->
        // `FUN_0043E310`, E-146 corrected).
        let rules = rules();
        let mut state = state(&rules);
        let cart = {
            let mut sim = Sim::new(&mut state, &rules);
            let mut desc = crate::world::ObjectDesc::new(CART, 0, [20.0 * 128.0 + 64.0; 3]);
            desc.position[1] = 0.0;
            crate::world::create_object_named(&mut sim, &mut desc, Some("LilaOchs")).unwrap()
        };
        assert_eq!(state.names.get("lilaochstail"), None, "before the 0x40B");
        ticks(&mut state, &rules, 3);
        let tail = head(&Sim::new(&mut state, &rules), cart).unwrap().linked;
        assert!(!tail.is_none());
        assert_eq!(
            state.names.get("lilaochstail"),
            Some(&*tail.as_i32().to_string())
        );
        assert_eq!(
            state.names.get("lilaochs"),
            Some(&*cart.as_i32().to_string())
        );
    }

    // QUIRK K-247: deviation
    #[test]
    fn the_tail_follows_straight_and_bends_at_most_90_degrees() {
        let (rules, mut state, cart, tail) = with_tail(CART);
        // Straight travel direction 0 with step s: centre = (x - 5300, z), direction 0, position of the
        // rear part = head position, walking animation 1.
        let s = 1000;
        {
            let mut sim = Sim::new(&mut state, &rules);
            move_head(&mut sim, cart, s, 0, None);
        }
        ticks(&mut state, &rules, 1);
        {
            let sim = Sim::new(&mut state, &rules);
            let (x, z) = (MID + s, MID);
            assert_eq!(centre_of(&sim, tail), (x - 5300, z, 0));
            let t = sim.object(tail).unwrap();
            assert_eq!((t.grid.x.0, t.grid.z.0, t.direction.0), (x, z, 0));
            assert_eq!(t.anim.wanted, 1);
            assert_eq!(head(&sim, cart).unwrap().last_pos, (x, z, 0));
        }
        // Head drives sideways (direction 90 degrees, position +s in z): angle atan2_deg(s, 5300).
        {
            let mut sim = Sim::new(&mut state, &rules);
            move_head(&mut sim, cart, 0, 2000, Some(90));
        }
        ticks(&mut state, &rules, 1);
        let a = {
            let sim = Sim::new(&mut state, &rules);
            let a = atan2_deg(2000, 5300);
            assert!((20 << 12..21 << 12).contains(&a), "~ 20.7 degrees: {a}");
            let (x, z) = (MID + s, MID + 2000);
            assert_eq!(
                centre_of(&sim, tail),
                (
                    x - fixed_mul(cos_deg(a), 5300),
                    z - fixed_mul(sin_deg(a), 5300),
                    a
                )
            );
            let t = sim.object(tail).unwrap();
            assert_eq!(
                (t.grid.x.0, t.grid.z.0, t.direction.0),
                (x, z, wrap_angle(a))
            );
            a
        };
        // Head turns by 180 degrees (direction 180 degrees, but keeps driving in +x): bend > 90 degrees ->
        // rear part direction = dir - 90 degrees.
        {
            let mut sim = Sim::new(&mut state, &rules);
            move_head(&mut sim, cart, 1000, 0, Some(180));
        }
        ticks(&mut state, &rules, 1);
        let sim = Sim::new(&mut state, &rules);
        assert!(a < 90 << 12);
        assert_eq!(centre_of(&sim, tail).2, 90 << 12);
        assert_eq!(sim.object(tail).unwrap().direction.0, 90 << 12);
        let (x, z) = (MID + s + 1000, MID + 2000);
        assert_eq!(
            centre_of(&sim, tail),
            (
                x - fixed_mul(cos_deg(90 << 12), 5300),
                z - fixed_mul(sin_deg(90 << 12), 5300),
                90 << 12
            )
        );
    }

    // QUIRK K-247: deviation
    #[test]
    fn follow_angle_clamps_the_bend_at_90_degrees() {
        // Pure function (the host uses it for the frame display): angle centre -> head,
        // bend to the head direction at most +/-90 degrees (strictly greater clamps).
        let c = (0, 0);
        let h = |dir: i32| (5300, 0, dir << 12);
        assert_eq!(follow_angle(c, h(0)), 0);
        assert_eq!(
            follow_angle(c, h(-90)),
            0,
            "exactly 90 degrees: not clamped"
        );
        assert_eq!(
            follow_angle(c, h(-100)),
            -10 << 12,
            "k = 100 degrees -> dir + 90 degrees"
        );
        assert_eq!(
            follow_angle(c, h(100)),
            10 << 12,
            "k = -100 degrees -> dir - 90 degrees"
        );
        assert_eq!(follow_angle(c, h(180)), 90 << 12);
        assert_eq!(
            follow_angle(c, h(-180)),
            90 << 12,
            "-180 degrees - 90 degrees folded"
        );
        // Unclamped: the angle of the vector.
        assert_eq!(
            follow_angle((0, 0), (5300, 2000, 90 << 12)),
            atan2_deg(2000, 5300)
        );
        assert_eq!(
            follow_angle((100, 50), (100, 50 + 5300, 90 << 12)),
            90 << 12
        );
    }

    // QUIRK K-248: kept
    #[test]
    fn turning_on_the_spot_is_not_reported() {
        // K-248: only changing the direction -> 0x333 (idle animation), centre unchanged.
        let (rules, mut state, cart, tail) = with_tail(CART);
        {
            let mut sim = Sim::new(&mut state, &rules);
            move_head(&mut sim, cart, 500, 0, None);
        }
        ticks(&mut state, &rules, 1);
        let before = {
            let sim = Sim::new(&mut state, &rules);
            assert_eq!(sim.object(tail).unwrap().anim.wanted, 1);
            (
                centre_of(&sim, tail),
                head(&sim, cart).unwrap().last_pos,
                sim.object(tail).unwrap().direction.0,
            )
        };
        {
            let mut sim = Sim::new(&mut state, &rules);
            set_direction(&mut sim, cart, 90 << 12);
        }
        ticks(&mut state, &rules, 2);
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(sim.object(cart).unwrap().direction.0, 90 << 12);
        assert_eq!(centre_of(&sim, tail), before.0, "centre unchanged");
        assert_eq!(head(&sim, cart).unwrap().last_pos, before.1);
        assert_eq!(sim.object(tail).unwrap().direction.0, before.2);
        assert_eq!(sim.object(tail).unwrap().anim.wanted, 0, "idle animation");
    }

    // QUIRK K-248: kept
    #[test]
    fn the_tail_dies_with_its_head_and_survives_its_removal() {
        // Death of the front part -> 0x30B -> 0x110 to the rear part (dies with animation 2).
        let (rules, mut state, cart, tail) = with_tail(CART_MORTAL);
        {
            let mut sim = Sim::new(&mut state, &rules);
            crate::messaging::send(&mut sim, cart, &Message::new(0x110, &[]));
            assert_ne!(sim.object(cart).unwrap().status & status::DIED, 0);
            let t = sim
                .object(tail)
                .expect("dies with animation, stays standing");
            assert_ne!(t.status & status::DIED, 0);
            assert_eq!(t.anim.wanted, 2, "dying animation 170002");
            assert_eq!(head(&sim, cart).unwrap().linked, ObjectId::NONE);
        }
        // Removal without death leaves the rear part orphaned (K-248).
        let (rules, mut state, cart, tail) = with_tail(CART);
        {
            let mut sim = Sim::new(&mut state, &rules);
            destroy_object(&mut sim, cart);
        }
        ticks(&mut state, &rules, 5);
        let mut sim = Sim::new(&mut state, &rules);
        let t = sim.object(tail).expect("rear part stands");
        assert_eq!(t.status & status::DIED, 0);
        assert_eq!(part(&sim, tail).unwrap().prev, cart, "reference stale");
        // No head of the chain any more.
        assert_eq!(
            crate::messaging::send(&mut sim, tail, &Message::new(0x633, &[])),
            -1
        );
        // Death of the rear part only: the front part notices it only later, via the ID (the tick skips it).
        let (rules, mut state, cart, tail) = with_tail(CART);
        {
            let mut sim = Sim::new(&mut state, &rules);
            destroy_object(&mut sim, tail);
        }
        ticks(&mut state, &rules, 3);
        let sim = Sim::new(&mut state, &rules);
        assert!(sim.object(cart).is_some());
        assert_eq!(head(&sim, cart).unwrap().linked, tail, "stays standing");
    }

    // QUIRK K-249: kept
    // QUIRK K-278: deviation
    #[test]
    fn an_owner_change_keeps_head_and_tail_together() {
        // Change of owner of the front part: 0x733 {new} to the rear part and its change of owner;
        // for the rear part 0x80B {new} to the front part, exempt object = own new ID.
        let (rules, mut state, cart, tail) = with_tail(CART);
        let mut sim = Sim::new(&mut state, &rules);
        let new_cart = change_owner(&mut sim, cart, 2).expect("change of owner");
        assert_eq!(new_cart.slot(), 2);
        assert_eq!(part(&sim, tail).unwrap().prev, new_cart, "0x733");
        assert_eq!(sim.state.owner_changes, [(tail, 2)], "rear part queued");
        // The rear part changes in the following pass of the queue.
        process_owner_changes(&mut sim);
        assert!(sim.object(tail).is_none());
        let new_tail = head(&sim, new_cart).unwrap().linked;
        assert_eq!(
            (new_tail.slot(), sim.object(new_tail).unwrap().guid),
            (2, CART_TAIL)
        );
        assert_eq!(part(&sim, new_tail).unwrap().prev, new_cart);
        let t = sim.object(new_tail).unwrap();
        assert_eq!((t.exempt.object, t.exempt.mode), (new_tail, 0));
        assert!(sim.state.events.iter().any(
            |e| matches!(e, SimEvent::OwnerChanged { old, new } if *old == tail && *new == new_tail)
        ));
        // Both continue to follow: a step of the head reaches the new rear part.
        {
            let mut sim = Sim::new(&mut state, &rules);
            move_head(&mut sim, new_cart, 700, 0, None);
        }
        ticks(&mut state, &rules, 1);
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(sim.object(new_tail).unwrap().grid.x.0, MID + 700);

        // Only the rear part changes: the front part learns the new ID (0x80B).
        let (rules, mut state, cart, tail) = with_tail(CART);
        let mut sim = Sim::new(&mut state, &rules);
        let new_tail = change_owner(&mut sim, tail, 3).unwrap();
        assert_eq!(head(&sim, cart).unwrap().linked, new_tail);
        assert_eq!(sim.object(new_tail).unwrap().exempt.object, new_tail);
        assert!(sim.state.owner_changes.is_empty(), "the front part stays");

        // Without a rear part no follow-up queueing.
        let (rules, mut state, cart) = world_with(CART);
        let mut sim = Sim::new(&mut state, &rules);
        let new_cart = change_owner(&mut sim, cart, 2).unwrap();
        assert!(sim.state.owner_changes.is_empty());
        assert!(head(&sim, new_cart).unwrap().linked.is_none());
    }

    thread_local! {
        static SEEN: RefCell<Vec<(ObjectId, usize, u32)>> = const { RefCell::new(Vec::new()) };
    }

    fn record(_: &mut Sim, id: ObjectId, index: usize, msg: &Message) {
        SEEN.with(|s| s.borrow_mut().push((id, index, msg.code)));
    }

    #[test]
    fn the_head_answers_its_queries() {
        use crate::messaging::send;
        let (rules, mut state, cart, tail) = with_tail(CART);
        let mut sim = Sim::new(&mut state, &rules);
        let q = |sim: &mut Sim, code: u32, args: &[i32]| send(sim, cart, &Message::new(code, args));
        assert_eq!(q(&mut sim, 0x60B, &[]), 1);
        assert_eq!(q(&mut sim, 0x70B, &[]), tail.as_i32());
        assert_eq!(q(&mut sim, 0x1B0B, &[]), -1, "unknown");
        // 0x10B creates only without a rear part, 0x40B without a check (K-248, a bug without consequences).
        assert_eq!(q(&mut sim, 0x10B, &[]), 1);
        assert_eq!(sim.state.guid_counts[0][&CART_TAIL], 1);
        assert_eq!(head(&sim, cart).unwrap().linked, tail);
        // 0x80B sets the ID.
        assert_eq!(q(&mut sim, 0x80B, &[77]), 1);
        assert_eq!(head(&sim, cart).unwrap().linked, ObjectId::from_i32(77));
        assert_eq!(q(&mut sim, 0x80B, &[tail.as_i32()]), 1);
        // 0x20B queues the rear part for removal.
        assert_eq!(q(&mut sim, 0x20B, &[]), 1);
        assert_eq!(sim.state.destroy_queue, [tail]);
        assert!(head(&sim, cart).unwrap().linked.is_none());
        // Now 0x10B creates a new rear part; 0x40B creates yet another one without a check.
        assert_eq!(q(&mut sim, 0x10B, &[]), 1);
        let second = head(&sim, cart).unwrap().linked;
        assert!(!second.is_none() && second != tail);
        assert_eq!(q(&mut sim, 0x40B, &[]), 1);
        let third = head(&sim, cart).unwrap().linked;
        assert!(third != second, "0x40B does not check linked");
        assert_eq!(sim.state.guid_counts[0][&CART_TAIL], 3);
        // Radii: 0x133 adds per creation.
        let o = sim.object(cart).unwrap();
        assert_eq!(o.hard_radius, 1900 + 3 * 1900);
        // With module 0x2E (siege engine) 0x40B creates nothing.
        sim.object_mut(cart).unwrap().modules[0x2E] = Some(crate::module::Module::new(0x2E));
        sim.object_mut(cart)
            .unwrap()
            .queue_module_tick_change(0x2E, true);
        sim.object_mut(cart)
            .unwrap()
            .apply_module_activation_changes();
        assert_eq!(q(&mut sim, 0x40B, &[]), 1);
        assert_eq!(head(&sim, cart).unwrap().linked, third);
        // 0x50B is sent as 0x3600 to the own object (walk: interpolated position).
        SEEN.with(|s| s.borrow_mut().clear());
        test_hook::observe(record, || {
            let mut m = Message::new(0x50B, &[0, 0, 0]);
            assert_eq!(crate::messaging::send_mut(&mut sim, cart, &mut m), 1);
        });
        let seen = SEEN.with(|s| s.borrow().clone());
        assert!(seen.contains(&(cart, 0, 0x3600)), "{seen:?}");
    }

    #[test]
    fn the_part_answers_its_queries() {
        use crate::messaging::send;
        let (rules, mut state, cart, tail) = with_tail(CART);
        let mut sim = Sim::new(&mut state, &rules);
        let q = |sim: &mut Sim, code: u32, args: &[i32]| send(sim, tail, &Message::new(code, args));
        assert_eq!(q(&mut sim, 0x133, &[]), 1900, "own hard radius");
        assert_eq!(q(&mut sim, 0x1B33, &[]), -1);
        assert_eq!(q(&mut sim, 0x333, &[]), 0);
        assert_eq!(sim.object(tail).unwrap().anim.wanted, 0);
        // 0x233 sets position, direction and the walking animation.
        assert_eq!(q(&mut sim, 0x233, &[MID + 300, MID, 0]), 0);
        let t = sim.object(tail).unwrap();
        assert_eq!((t.grid.x.0, t.anim.wanted), (MID + 300, 1));
        // 0x633: the predecessor without module 51 is the head of the chain.
        assert_eq!(q(&mut sim, 0x633, &[]), cart.as_i32());
        // 0x433 and 0x733 set the predecessor.
        assert_eq!(q(&mut sim, 0x433, &[5]), 0);
        assert_eq!(part(&sim, tail).unwrap().prev, ObjectId::from_i32(5));
        assert_eq!(q(&mut sim, 0x733, &[cart.as_i32()]), 0);
        assert_eq!(part(&sim, tail).unwrap().prev, cart);
    }

    // QUIRK K-280: deviation
    #[test]
    fn a_chain_of_parts_is_created_forwarded_and_survives_a_loop() {
        // 520001 > 0 (never in the shipped data): the further link arises in ReadParams, 0x133 is
        // passed on (result discarded), 0x633 finds the head of the chain.
        let (rules, mut state, cart) = world_with(CART);
        let mut sim = Sim::new(&mut state, &rules);
        let first = spawn(&mut sim, TAIL_CHAIN, 0, (22, 22));
        let p = part(&sim, first).unwrap();
        let second = p.next;
        assert!(!second.is_none());
        assert_eq!(sim.object(second).unwrap().guid, CART_TAIL);
        assert_eq!(part(&sim, second).unwrap().prev, first);
        // x + 2 * hard radius (1300) behind the first.
        let g = sim.object(first).unwrap().grid;
        assert_eq!(sim.object(second).unwrap().grid.x.0, g.x.0 + 2 * 1300);
        // 0x133 at the first: own radius, the second learns of it.
        SEEN.with(|s| s.borrow_mut().clear());
        let r = test_hook::observe(record, || {
            crate::messaging::send(&mut sim, first, &Message::new(0x133, &[]))
        });
        assert_eq!(r, 1300);
        let seen = SEEN.with(|s| s.borrow().clone());
        assert!(seen.contains(&(second, PART, 0x133)), "{seen:?}");
        // Head of the chain: first link -> front part.
        part_mut(&mut sim, first).unwrap().prev = cart;
        assert_eq!(
            crate::messaging::send(&mut sim, second, &Message::new(0x633, &[])),
            cart.as_i32()
        );
        // Rings (only through manipulated state): neither 0x133 nor 0x633 run endlessly.
        part_mut(&mut sim, second).unwrap().next = first;
        part_mut(&mut sim, first).unwrap().prev = second;
        assert_eq!(
            crate::messaging::send(&mut sim, first, &Message::new(0x133, &[])),
            1300
        );
        assert_eq!(
            crate::messaging::send(&mut sim, first, &Message::new(0x633, &[])),
            -1
        );
        part_mut(&mut sim, first).unwrap().next = first;
        assert_eq!(
            crate::messaging::send(&mut sim, first, &Message::new(0x133, &[])),
            1300,
            "self-reference"
        );
    }
}
