//! Module 35 `TRANSPORTTENT` (`CPropertyTransportTent`, `0x00531980`, vtable `0x00626898`, `0x74`
//! bytes, re/tents 1.3): the travois `PRI_ZeltTravois` (1717), which carries a packed tent. The
//! flags (bit `0x100` = `TRANSPORTINGTENT`) are in the [`ModuleBase`](crate::module::base::ModuleBase),
//! the state is only `m_TentHID`. Query `0x00531B10` (re/tents 2.2): `0x123` (remember tent) and
//! `0x423` (query tent), `0x223` (arrived, unpack), `0x323` (walk failure) and the change of owner
//! ([`on_owner_changed`]). No tick (vtable slot 13 empty).

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::message::Message;
use crate::messaging::send;
use crate::module::{ModuleKind, init_flags, set_match_bits};
use crate::sim::Sim;
use crate::world::queue_destroy;

/// Module index of TransportTent.
pub const INDEX: usize = 0x23;
/// Flag `TRANSPORTINGTENT` (`MID_PROP_TRANSPORTTENT_TRANSPORTINGTENT`, `targetdefines.ini` 291).
pub const TRANSPORTING_TENT: u32 = 0x100;
/// [`TRANSPORTING_TENT`] with the module index in the low byte, as the original passes it to
/// `SetFlags`/`ClearFlags` (`0x123`).
const TRANSPORTING_TENT_MID: u32 = TRANSPORTING_TENT | INDEX as u32;

/// Messages of the travois (query `0x00531B10`, re/tents 2.2).
pub mod msg {
    /// `{tent}`: remember the tent and set `TRANSPORTINGTENT` (`0x00531A90`); sender is packing.
    pub const SET_TENT: u32 = 0x123;
    /// `{x, z}`: walk success of the command `0x2E` (`0x005319D0`).
    pub const ARRIVED: u32 = 0x223;
    /// Walk failure of the command `0x2E` (`0x00531A60`): log only, 0.
    pub const WALK_FAILED: u32 = 0x323;
    /// -> `m_TentHID` (`0x00531A80`).
    pub const GET_TENT: u32 = 0x423;
}

/// `CPropertyTransportTent`; in the savegame `m_TentHID` (`0x00531B90`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportTent {
    /// `m_TentHID` (`+0x70`): the packed tent; may point to a removed object.
    pub tent: ObjectId,
}

// `ObjectId` has no `Default`.
impl Default for TransportTent {
    fn default() -> Self {
        Self {
            tent: ObjectId::NONE,
        }
    }
}

/// `PropertyTransportTent_Init` (`0x005319B0`): the flags from 360001 (shipped data 0).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
}

fn transport<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s TransportTent> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::TransportTent(t) => Some(t),
        _ => None,
    }
}

fn transport_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut TransportTent> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::TransportTent(t) => Some(t),
        _ => None,
    }
}

/// `CPropertyTransportTent::Query` (`0x00531B10`, re/tents 2.2); unknown codes -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::SET_TENT => {
            // `0x00531A90`: `m_TentHID = tent`, `SetFlags(0x123)`.
            let tent = ObjectId::from_i32(m.arg(0));
            let Some(t) = transport_mut(sim, id) else {
                return -1;
            };
            t.tent = tent;
            set_match_bits(sim, id, TRANSPORTING_TENT_MID, true);
            0
        }
        msg::ARRIVED => on_arrived(sim, id, m),
        msg::WALK_FAILED => {
            // `0x00531A60` (re/tents 3.6): target unreachable or run aborted; travois and
            // tent stay, a new command is possible.
            log::debug!("Error, could not reach position");
            0
        }
        msg::GET_TENT => transport(sim, id).map_or(-1, |t| t.tent.as_i32()),
        _ => -1,
    }
}

/// `0x005319D0` (`0x223 {x, z}`, re/tents 3.4): walk success of the command `0x2E`. If the tent no
/// longer exists, 0 without effect (the travois keeps the flag, W-261). Otherwise
/// `0x22A {travois, x, z}` to the tent; if that fails (< 0), -1 and the travois keeps tent and
/// flag. If it succeeds, the travois becomes unselectable, loses `TRANSPORTINGTENT` and forgets the
/// tent; it is destroyed not until the end of the fade-in (`finish_unpack`). If it arrives during
/// the fade-out, `0x22A` answers 0 without effect and the link is released anyway (W-250).
pub fn on_arrived(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let tent = transport(sim, id).map_or(ObjectId::NONE, |t| t.tent);
    if sim.object(tent).is_none() {
        return 0;
    }
    let unpack = Message::new(super::msg::UNPACK, &[id.as_i32(), m.arg(0), m.arg(1)]);
    if send(sim, tent, &unpack) < 0 {
        return -1;
    }
    if let Some(o) = sim.object_mut(id) {
        o.selectable = false;
    }
    set_match_bits(sim, id, TRANSPORTING_TENT_MID, false);
    if let Some(t) = transport_mut(sim, id) {
        t.tent = ObjectId::NONE;
    }
    0
}

/// Change of owner (slot 5, `0x00531AB0`, re/tents 2.2; `id` is the new ID): if the new
/// slot is 8 - death queues `QueueOwnerChange(travois, 8)` - and the travois carries a tent,
/// the tent goes into the destroy list (removed in the next phase-0 step, without
/// death flow, W-260) and `m_TentHID` is forgotten. The flag `TRANSPORTINGTENT` stays. A
/// change to another player leaves the tent with the old owner.
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId) {
    if id.slot() != NEUTRAL_SLOT {
        return;
    }
    let Some(t) = transport_mut(sim, id) else {
        return;
    };
    let tent = std::mem::replace(&mut t.tent, ObjectId::NONE);
    if !tent.is_none() {
        queue_destroy(sim, tent);
    }
}
