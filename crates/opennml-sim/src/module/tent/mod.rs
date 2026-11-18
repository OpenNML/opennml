//! Module 42 `TENT` (`PropertyTent_Construct` `0x0052DC50`, vtable `0x006262A8`, `0x88` bytes,
//! re/tents 1.2): the tent of the Prairie Indians, which can be packed into the travois. The state
//! is `m_TravoisGuid` (430001), `m_TentStatus` (0 at rest, 1 fade-out, 2 fade-in),
//! `m_TentCounter` (remaining time of the crossfade in ms), `m_TentDelta` (change of the crossfade
//! factor per ms, as `f32` bits), `m_BackupShowStatus` and `m_TravoisHID`. Module 35 (travois) is in
//! [`transport`]. The file implements packing (`0x12A`, re/tents 3.1), the crossfade (re/tents 3.2), the completion
//! of packing (re/tents 3.3), unpacking (`0x22A`, re/tents 3.4) and its completion (re/tents 3.5); the error
//! cases (re/tents 3.6) have tests.

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::placement::check_placement_at_own_pos;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{ObjectDesc, create_object, desc_flags, queue_destroy, set_grid_position};

use super::{ModuleKind, building, init_flags, quarter};

pub mod transport;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unit_tests;

/// Module index of Tent.
pub const INDEX: usize = 0x2A;
/// `VALUEID` of the travois GUID (`0x68AB1`).
pub const VALUE_TRAVOIS: u32 = 430001;
/// Duration of fading out and in (`0x9C4`, re/tents 1.4).
pub const FADE_MS: i32 = 2500;
/// `m_TentDelta` when fading out: `−1/2500` per ms as `f32` bits (re/tents 9.1).
pub const DELTA_OUT: u32 = 0xB9D1_B717;
/// `m_TentDelta` when fading in: `+1/2500` per ms as `f32` bits (re/tents 9.1).
pub const DELTA_IN: u32 = 0x39D1_B717;

/// Messages of the tent (query `0x0052E120`, re/tents 2.1).
pub mod msg {
    /// Packing (`PropertyTent_Pack` `0x0052E010`); response 0.
    pub const PACK: u32 = 0x12A;
    /// `{travois, x, z}`: begin unpacking (`PropertyTent_Unpack` `0x0052DE90`); response 0/-1.
    pub const UNPACK: u32 = 0x22A;
}

/// Values of `m_TentStatus`.
pub mod status {
    /// At rest.
    pub const REST: i32 = 0;
    /// Fade-out (packing).
    pub const FADE_OUT: i32 = 1;
    /// Fade-in (unpacking).
    pub const FADE_IN: i32 = 2;
}

/// `CPropertyTent`; in the savegame the six fields of slot 16 (`0x0052E150`, re/tents 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tent {
    /// `m_TravoisGuid` (`+0x70`, 430001; init, shipped data 1717).
    pub travois_guid: u32,
    /// `m_TentStatus` (`+0x74`, [`status`]).
    pub status: i32,
    /// `m_TentCounter` (`+0x78`): remaining time of the crossfade in ms.
    pub counter: i32,
    /// `m_TentDelta` (`+0x7C`) as `f32` bits ([`DELTA_OUT`], [`DELTA_IN`]); the tick computes with
    /// the bits, never with libm.
    pub delta_bits: u32,
    /// `m_BackupShowStatus` (`+0x80`): render bits for the end of the crossfade.
    pub backup_show: u32,
    /// `m_TravoisHID` (`+0x84`): most recently created travois, never reset. May point to a
    /// removed object (the original resolves the ID on each access).
    pub travois: ObjectId,
}

// `ObjectId` has no `Default`.
impl Default for Tent {
    fn default() -> Self {
        Self {
            travois_guid: 0,
            status: status::REST,
            counter: 0,
            delta_bits: 0,
            backup_show: 0,
            travois: ObjectId::NONE,
        }
    }
}

/// `PropertyTent_Init` (`0x0052DC90`): flags (0), then `m_TravoisGuid = GetInt(430001)`.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let guid = desc.value_int(VALUE_TRAVOIS).unwrap_or(0) as u32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        if let ModuleKind::Tent(t) = &mut m.kind {
            t.travois_guid = guid;
        }
    }
}

fn tent<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Tent> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Tent(t) => Some(t),
        _ => None,
    }
}

fn tent_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Tent> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Tent(t) => Some(t),
        _ => None,
    }
}

/// `PropertyTent_Query` (`0x0052E120`, re/tents 2.1); unknown codes -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::PACK => pack(sim, id),
        msg::UNPACK => unpack(sim, id, m),
        _ => -1,
    }
}

/// `PropertyTent_Pack` (`0x0052E010`, `0x12A`, re/tents 3.1). Silently 0 during a crossfade.
/// Otherwise create the travois with flags `0xC3` (GUID, position, slot, color - **without**
/// "finished": no type counter +1, W-248) at the origin of the tent; it appears in the still locked
/// footprint and walk init moves it to the nearest free tile (W-249). Then in order: remember the
/// travois, `0x1917` (all occupants out, -1 without quarters), `0x123 {tent}` to the travois,
/// `0x2E03` to the building, fade-out. The construction state is not checked (W-247).
pub fn pack(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((st, guid)) = tent(sim, id).map(|t| (t.status, t.travois_guid)) else {
        return -1;
    };
    if st != status::REST {
        return 0;
    }
    let Some((pos, color)) = sim.object(id).map(|o| (o.world_pos, o.color)) else {
        return -1;
    };
    let mut desc = ObjectDesc {
        flags: desc_flags::GUID | desc_flags::POSITION | desc_flags::PLAYER | desc_flags::COLOR,
        color,
        ..ObjectDesc::new(guid, i32::from(id.slot()), pos)
    };
    let Some(travois) = create_object(sim, &mut desc) else {
        log::debug!("error, couldn´t create guid: {guid}");
        return 0;
    };
    if let Some(t) = tent_mut(sim, id) {
        t.travois = travois;
    }
    send(sim, id, &Message::new(quarter::msg::UNQUARTER_ALL, &[]));
    send(
        sim,
        travois,
        &Message::new(transport::msg::SET_TENT, &[id.as_i32()]),
    );
    send(sim, id, &Message::new(building::msg::HIDE, &[]));
    start_fade(sim, id, true);
    0
}

/// `PropertyTent_Unpack` (`0x0052DE90`, `0x22A {travois, x, z}`, re/tents 3.4), sender is the
/// arrived travois. In this order:
///
/// 1. During a crossfade silently 0, no effect (W-250).
/// 2. Foreign travois ID: take it over and still return -1 (W-259).
/// 3. Grid position `(x, 0, z)` keeping the height; on a tile change in the collision manager
///    again, otherwise not (W-89). The reimplementation does not store the world matrix
///    (`GameObject_SetWorldMatrix` `0x00466370`, rotation stays), it computes it from `world_pos`
///    when needed.
/// 4. Place check at the own position ([`check_placement_at_own_pos`]); "built on" -> -1, the tent
///    stays hidden at the new position (W-251).
/// 5. Set the occupancy bits of the model rectangle directly, bypassing the freed list (W-252).
/// 6. Travois unselectable, fade-in; 0. The render list `+0x164` belongs to the host.
pub fn unpack(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let Some((st, hid)) = tent(sim, id).map(|t| (t.status, t.travois)) else {
        return -1;
    };
    if st != status::REST {
        return 0;
    }
    if m.arg(0) != hid.as_i32() {
        log::debug!("be careful, wrong TravoisHID!!!");
        if let Some(t) = tent_mut(sim, id) {
            t.travois = ObjectId::from_i32(m.arg(0));
        }
        return -1;
    }
    let pos = FxVec3::new(Fx(m.arg(1)), Fx(0), Fx(m.arg(2)));
    set_grid_position(sim, id, pos, true);
    if !check_placement_at_own_pos(sim, id) {
        log::debug!("No longer possible, the site is built on!");
        return -1;
    }
    let rect = building::footprint_rect(sim, id);
    sim.state.occupancy.set_rect(rect, true);
    if let Some(o) = sim.object_mut(hid) {
        o.selectable = false;
    }
    start_fade(sim, id, false);
    0
}

/// Start the crossfade (`0x0052DCB0`, re/tents 3.2): counter 2500 ms, direction and status
/// (out: 1, factor 1.0; in: 2, factor 0.0), save the current render bits for the end, then
/// only render bit 4. The original computes the direction as `f32(±1 / 2500)` (x87, one rounding);
/// that gives exactly [`DELTA_OUT`] or [`DELTA_IN`] (re/tents 9.1).
pub fn start_fade(sim: &mut Sim, id: ObjectId, out: bool) {
    let Some(t) = tent_mut(sim, id) else {
        return;
    };
    t.counter = FADE_MS;
    let progress = if out {
        t.delta_bits = DELTA_OUT;
        t.status = status::FADE_OUT;
        1.0
    } else {
        t.delta_bits = DELTA_IN;
        t.status = status::FADE_IN;
        0.0
    };
    let Some(bits) = sim.object_mut(id).map(|o| {
        o.progress_scale = progress;
        o.render_bits
    }) else {
        return;
    };
    if let Some(t) = tent_mut(sim, id) {
        t.backup_show = bits;
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = 4;
    }
}

/// One step of the crossfade factor (`0x0052DE2F…0x0052DE3C`: `FILD dt; FMUL f32; FADD f32;
/// FSTP f32`). The original computes on x87 with a 24-bit mantissa (D3D without `FPU_PRESERVE`,
/// re/renderer, re/sim-core): `FMUL` and `FADD` each round to `f32`. Hence two separate
/// `f32` operations, product before sum, without FMA (re/tents 3.2, 9.1). `FILD` loads
/// `dt` exactly; `dt as f32` is just as exact for `|dt| ≤ 2²⁴` (the tick is 100 ms).
pub(crate) fn fade_step(progress: f32, delta_bits: u32, dt: i32) -> f32 {
    let product = dt as f32 * f32::from_bits(delta_bits);
    progress + product
}

/// `PropertyTent_Tick` (`0x0052DE20`, slot 13, re/tents 3.2): advance the factor only in status 1
/// or 2 (also the construction progress of a construction site, D-228); when the counter has
/// expired, set the saved render bits, run the completion and rest.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    let Some((st, delta)) = tent(sim, id).map(|t| (t.status, t.delta_bits)) else {
        return;
    };
    if !(status::FADE_OUT..=status::FADE_IN).contains(&st) {
        return;
    }
    if let Some(o) = sim.object_mut(id) {
        o.progress_scale = fade_step(o.progress_scale, delta, dt);
    }
    let Some(t) = tent_mut(sim, id) else {
        return;
    };
    t.counter = t.counter.wrapping_sub(dt);
    if t.counter > 0 {
        return;
    }
    let backup = t.backup_show;
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = backup;
    }
    if st == status::FADE_OUT {
        finish_pack(sim, id);
    } else {
        finish_unpack(sim, id);
    }
    if let Some(t) = tent_mut(sim, id) {
        t.status = status::REST;
    }
}

/// Completion of packing (`0x0052DD20`, re/tents 3.3): if the tent was selected, the selection
/// passes to the travois. The selection belongs to the host; the simulation always reports the handover.
fn finish_pack(sim: &mut Sim, id: ObjectId) {
    let Some(travois) = tent(sim, id).map(|t| t.travois) else {
        return;
    };
    sim.state.events.push(SimEvent::SelectionHandover {
        from: id,
        to: travois,
        only_if_empty: false,
    });
}

/// `PropertyTent_FinishBuildingSetup` (`0x0052DD60`, re/tents 3.5): `0x2F03` to the building. If
/// the travois still exists, the selection passes from it to the tent (only if it is empty
/// afterwards) and the travois goes into the destroy list; `m_TravoisHID` stays. If the travois
/// has changed owner in the meantime (death), the old ID finds nothing (re/tents 3.6).
fn finish_unpack(sim: &mut Sim, id: ObjectId) {
    send(sim, id, &Message::new(building::msg::TENT_PLACED, &[]));
    let Some(travois) = tent(sim, id).map(|t| t.travois) else {
        return;
    };
    if sim.object(travois).is_none() {
        return;
    }
    sim.state.events.push(SimEvent::SelectionHandover {
        from: travois,
        to: id,
        only_if_empty: true,
    });
    queue_destroy(sim, travois);
}
