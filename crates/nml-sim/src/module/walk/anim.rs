//! Walk and turn animation (re/motion 7.1 and 8, re/walk 16.8). The original selects it in the
//! frame update of visible objects only (`PropertyWalk_FrameUpdate`); here the same happens
//! deterministically at the end of every object tick for all units
//! (`nml_sim::module::frame_update`), the random start comes from the CRT generator of the
//! state.

use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::motion::speed::TIME_UNIT;
use crate::object::status;
use crate::sim::Sim;
use crate::world::wrap_angle;

use super::project::project_trajectory;
use super::{INDEX, WAITING, direction, mount_of, position, walk, walk_mut};

/// Stamina (attribute 28).
const STAMINA: u32 = 28;

/// Animation selection from `PropertyWalk_FrameUpdate` (`0x00538BA0`): select anew when the status
/// bits change or when turning, as long as `MOVE` is set (waiting/planning: idle via stand `0x202`).
/// Mounted or in a boat, the rider compares the status **of the mount** with `last_status` and
/// never turns (own trajectory empty, re/riding 4.1).
pub fn frame_animation(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let mount = mount_of(sim, id);
    let st = mount
        .and_then(|m| sim.object(m))
        .map_or(o.status, |m| m.status);
    let Some(w) = walk(sim, id) else { return };
    let pos = position(sim, id);
    let dir = direction(sim, id);
    let head = w.trajectory.front().copied();
    let turning = mount.is_none() && head.is_some_and(|h| h.dir != dir && (h.x, h.z) == pos);
    if (st != w.last_status || turning != w.last_turning) && st & status::MOVE != 0 {
        if st & WAITING != 0 {
            send(sim, id, &Message::new(0x202, &[]));
        } else if !turning {
            select_walk_anim(sim, id);
        } else {
            let h = head.expect("turns only with a head point");
            select_turn_anim(sim, id, wrap_angle(h.dir.wrapping_sub(dir)) > 0);
        }
    }
    if let Some(w) = walk_mut(sim, id) {
        w.last_status = st;
        w.last_turning = turning;
    }
}

/// `PropertyWalk_SelectWalkAnim` (`0x00532CB0`): walk or run, with a load the animation of the
/// gathering module; speed `Fixed_Mul(cur, 0x500)` (128 = normal). At rest (only `MOVE`) with
/// random start `rand() % (duration - 200)`. Mounted, the riding module of the rider selects
/// (`0x50D {5}`, `{6}` if the mount runs), in a boat `0x50E {5}`; the speed is that of the mount
/// (without a target its own) and there is no random start (re/riding 4.1).
pub(crate) fn select_walk_anim(sim: &mut Sim, id: ObjectId) {
    let Some(w) = walk(sim, id) else { return };
    let (own_cur, mut anim) = (
        w.speed.cur,
        if w.speed.running {
            w.anims[1]
        } else {
            w.anims[0]
        },
    );
    let mut cur = own_cur;
    let Some(o) = sim.object(id) else { return };
    let (load, kind, st) = (o.load_amount, o.load_kind, o.status);
    if o.has_module(0x2E, false) {
        send(sim, id, &Message::new(0x62E, &[]));
    }
    if st & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0 {
        let mount = mount_of(sim, id);
        if st & status::ISMOUNTED != 0 {
            let runs = mount.is_some_and(|m| send(sim, m, &Message::new(0x2600, &[])) == 1);
            let art = if runs { 6 } else { 5 };
            anim = send(sim, id, &Message::new(0x50D, &[art]));
        } else {
            anim = send(sim, id, &Message::new(0x50E, &[5]));
        }
        if let Some(m) = mount {
            cur = send(sim, m, &Message::new(0xD00, &[]));
        }
    } else if load > 0 && st & status::SWIMMING == 0 {
        let r = match kind {
            0 => send(sim, id, &Message::new(0x507, &[])),
            1 => send(sim, id, &Message::new(0x308, &[])),
            2 => {
                let roll = sim.state.crt.rand();
                let r = send(sim, id, &Message::new(0x30C, &[roll]));
                if r == -1 {
                    send(sim, id, &Message::new(0x316, &[]))
                } else {
                    r
                }
            }
            _ => anim,
        };
        if r != -1 {
            anim = r;
        }
    } else if st & 0x2_0000 != 0 {
        // Diving, swimming: response without -1 check (quirk).
        anim = send(sim, id, &Message::new(0x318, &[]));
    } else if st & status::SWIMMING != 0 {
        anim = send(sim, id, &Message::new(0x10F, &[]));
    }
    let tempo = fixed_mul(cur, TIME_UNIT) as f32;
    let quiet = sim.object(id).is_some_and(|o| o.status & 0xFFFE_F33E == 0);
    let mut start = 0;
    if quiet {
        let guid = sim.object(id).map_or(0, |o| o.guid);
        let divisor = sim.rules.anim_label_time(guid, anim) - 200;
        let roll = sim.state.crt.rand();
        // Divisor 0 would be a crash in the original (A7: no offset).
        start = if divisor == 0 {
            0
        } else {
            roll.wrapping_rem(divisor)
        };
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_animation_speed(anim, start, tempo, false, false, false);
    }
}

/// `PropertyWalk_SelectTurnAnim` (`0x00532FE0`): turn animation only with module flag `0x8000`
/// (saddle, mounts), otherwise the walk animation.
fn select_turn_anim(sim: &mut Sim, id: ObjectId, positive: bool) {
    let saddle = sim
        .module(id, INDEX)
        .is_some_and(|m| m.base.flags & 0x8000 != 0);
    if !saddle {
        select_walk_anim(sim, id);
        return;
    }
    let target = sim.object(id).map_or(ObjectId::NONE, |o| o.target);
    if !target.is_none() && target.0 != 0 && sim.object(target).is_some() {
        send(sim, target, &Message::new(0x1A00, &[]));
    }
    let anim = walk(sim, id).map_or(-1, |w| if positive { w.anims[2] } else { w.anims[3] });
    if let Some(o) = sim.object_mut(id) {
        o.set_animation_speed(anim, 0, 128.0, false, false, false);
    }
}

/// `PropertyWalk_TickStamina` (`0x00536CD0`): sprinting costs stamina; empty -> cruising speed,
/// new animation, new trajectory.
pub(crate) fn tick_stamina(sim: &mut Sim, id: ObjectId) {
    let running = walk(sim, id).is_some_and(|w| w.speed.running);
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, STAMINA);
    let Some(o) = sim.object_mut(id) else { return };
    if !running || !o.attributes.is_present(STAMINA) {
        return;
    }
    o.attributes.add_scaled_to_current(STAMINA, -5, 10, p);
    if o.attributes.current_value(STAMINA, p) >= 1 {
        return;
    }
    if let Some(w) = walk_mut(sim, id) {
        w.speed.reset_to_cruising();
    }
    select_walk_anim(sim, id);
    project_trajectory(sim, id, true, false);
}
