//! Module 48 `ROBBING` (`CPropertyRobbing`, constructor `PropertyRobbing_Construct` `0x005236F0`,
//! vtable `0x00625640`, `0x7C` bytes, re/headhunter-robbing 1.3): rob corpses.
//!
//! The module holds the state and `Init` and implements the flow (corpse search, arrival,
//! robbing, abort, re/headhunter-robbing 3.8, 3.9; command `0x4C` in [`crate::command::ability`]).
//! The module has no further state: the running work sits in the timers (group `0x80`), the
//! lock `0x800000` and the walk order. The tick is only `Module_UpdateEvents` (slot 13
//! `0x005119A0`).
//!
//! Re-entrancy: `dispatch_command` in [`seek_corpse`] can return `0x230`/`0x430` synchronously
//! (corpse already within the target radius); the module has no state that would have
//! to be written before, and holds no borrow across the call.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_mut};
use crate::module::events::update_events;
use crate::module::special::{self, FINDTO, post, release_animation};
use crate::module::stand::parse_int_list;
use crate::module::{ModuleKind, dying, init_flags};
use crate::object::status;
use crate::sim::Sim;
use crate::world::region_id_at;

#[cfg(test)]
mod tests;

/// Module index robbing.
pub const INDEX: usize = 0x30;
/// Group of the timers (`0x330`; the abort clears them, re/headhunter-robbing 3.9); the same as for the
/// special attacks ([`special::post`]).
pub const GROUP: u8 = special::GROUP;
/// Attack module: out of the tick list at the start (`0x130`, re/headhunter-robbing 2.2).
pub const ATTACK: usize = special::ATTACK;
/// Search filter `MM_RETURN_DIED | MM_ANDALL | MM_CHECKHANDLETYPE` (`0x4000012`, `0x00523810`).
const SEEK_MODE: u32 = 0x400_0012;
/// Built-in position check (layer and region, re/match-filter, section "Appendix"), only with a walk layer.
const SEEK_IN_REGION: u32 = 0x20;
/// Walk: query layer (`m_PPHandleNxN`; -1 without walk).
const WALK_LAYER: u32 = 0x1400;
/// Corpse: pick up and empty the robbing gold (`0x610 {0}` -> gold in `arg(0)`, re/combat-death 2.5).
const TAKE_GOLD: u32 = 0x610;
/// Attribute sight (config 18, quarter tiles).
const SIGHT: u32 = 18;

/// Messages to module 48 (query `0x00523BC0`, re/headhunter-robbing 2.2).
pub mod msg {
    /// Start: attack module off, corpse search (`0x00523A70`).
    pub const START: u32 = 0x130;
    /// `{corpse}`: walk success, animation and timer `0x330` (`0x00523A90`).
    pub const ARRIVED: u32 = 0x230;
    /// `{corpse}`: robbing (`0x00523B40`).
    pub const ROB: u32 = 0x330;
    /// `{corpse}`: walk failure, keep searching (`0x00523810`).
    pub const WALK_FAULT: u32 = 0x430;
}

/// `CPropertyRobbing`, field of the savegame (slot 16 `0x00523C50`, re/headhunter-robbing 1.3, 5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Robbing {
    /// `m_RobAnim` (`+0x70`, 490002): robbing animation (index 0 is played).
    pub rob_anim: Vec<i32>,
}

pub fn robbing<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Robbing> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Robbing(r) => Some(r),
        _ => None,
    }
}

pub fn robbing_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Robbing> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Robbing(r) => Some(r),
        _ => None,
    }
}

/// `Init` (`0x00523C10`, slot 9, re/headhunter-robbing 1.3): `m_RobAnim` = `IntList` from 490002,
/// module flags (`m_MatchInt`) = 490001.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let text = desc.values.get(&490_002).cloned().unwrap_or_default();
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Robbing(r) = &mut m.kind {
        r.rob_anim = parse_int_list(&text, &[]);
    }
}

/// `Query` (slot 7, `0x00523BC0`, re/headhunter-robbing 2.2): `0x130` start, `0x230 {corpse}`
/// arrival, `0x330 {corpse}` robbing, `0x430 {corpse}` walk failure; unknown codes 0 (not
/// -1).
pub fn query(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let corpse = ObjectId::from_i32(m.arg(0));
    match m.code {
        msg::START => on_start(sim, id),
        msg::ARRIVED => on_arrived(sim, id, corpse),
        msg::ROB => on_rob(sim, id, corpse),
        msg::WALK_FAULT => seek_corpse(sim, id, Some(corpse)),
        _ => 0,
    }
}

/// `0x130` start (`0x00523A70`, re/headhunter-robbing 3.9): attack module `0x11` out of the tick via the pending list
/// (without `HasModule` check), then [`seek_corpse`] without an excluded corpse.
fn on_start(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.queue_module_tick_change(ATTACK, false);
    }
    seek_corpse(sim, id, None)
}

/// `PropertyRobbing_SeekCorpse` (`0x00523810`, re/headhunter-robbing 3.8, C-321, C-322): the nearest corpse with
/// `CANROBBED` (filter `{0x4000012, match_bits 0x210}` = module `0x10`, bit `0x200`, without
/// list filter - also own and allied corpses); with a walk layer (`0x1400` >= 0) plus the
/// built-in position check `0x20` without a reference object in layer and region of the own tile. Radius
/// sight/4 tiles (C division), edge distance with the own hard radius, player = own
/// slot. No corpse, or the same as `skip` (the unreachable one from `0x430`): end without
/// feedback (no `abort`, C-320). Otherwise FINDTO `{corpse, -1, 1}` with `0x230 {corpse}` as
/// success and `0x430 {corpse}` as failure to itself. Always returns 0.
pub fn seek_corpse(sim: &mut Sim, id: ObjectId, skip: Option<ObjectId>) -> i32 {
    let layer = send(sim, id, &Message::new(WALK_LAYER, &[]));
    let mut f = MatchFilter {
        mode: SEEK_MODE,
        mode2: 0,
        match_bits: dying::CANROBBED | dying::INDEX as u32,
        ..MatchFilter::constructed()
    };
    if layer >= 0 {
        // Original `f[8] = 0`: no reference object, the position check takes layer and region.
        f.mode |= SEEK_IN_REGION;
        f.ref_object = 0;
        f.layer = layer;
        f.region = i32::from(region_id_at(sim, id, layer, -1, -1));
    }
    let Some((pos, hard)) = sim
        .object(id)
        .map(|o| ((o.grid.x.0, o.grid.z.0), o.hard_radius))
    else {
        return 0;
    };
    let radius = Fx::from_int(sim.attribute(id, SIGHT) / 4).0;
    let player = i32::from(id.slot());
    let Some(corpse) = find_nearest_matching(sim, pos, &f, radius, player, hard) else {
        return 0;
    };
    if skip == Some(corpse) {
        return 0;
    }
    let env = Envelope {
        msg: Message::new(FINDTO, &[corpse.as_i32(), -1, 1]),
        success: Some(Message::new(msg::ARRIVED, &[corpse.as_i32()])),
        fault: Some(Message::new(msg::WALK_FAULT, &[corpse.as_i32()])),
        success_target: id,
        fault_target: id,
    };
    dispatch_command(sim, id, &env);
    0
}

/// `0x230 {corpse}` arrival (`0x00523A90`, re/headhunter-robbing 3.9, C-325): if the corpse is missing
/// (`GetObjectById(..., false)`), [`seek_corpse`]; otherwise - **without** checking `CANROBBED` -
/// `m_RobAnim[0]` (speed 128 is part of `set_animation`), `0x330 {corpse}` at time marker 1 (group `0x80`) and
/// lock `0x800000` on. Returns 0. Deviation (A7): with an empty `m_RobAnim` the original reads
/// outside the list; the reimplementation plays no animation and takes time marker 0
/// (`0x330` in the next object tick).
fn on_arrived(sim: &mut Sim, id: ObjectId, corpse: ObjectId) -> i32 {
    if sim.object(corpse).is_none() {
        return seek_corpse(sim, id, None);
    }
    let anim = robbing(sim, id).and_then(|r| r.rob_anim.first().copied());
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    let guid = o.guid;
    if let Some(anim) = anim {
        o.set_animation(anim, 0, false, false, false);
    }
    o.set_status_flags(status::ANIM_LOCKED, true);
    let t = anim.map_or(0, |anim| sim.rules.anim_label_time_at(guid, anim, 1));
    post(
        sim,
        id,
        INDEX,
        t,
        Message::new(msg::ROB, &[corpse.as_i32()]),
    );
    0
}

/// `0x330 {corpse}` robbing (`0x00523B40`, re/headhunter-robbing 3.9): lock off; if the corpse still exists,
/// `0x610 {0}` to it (robbing gold 25 or 0, not robbable afterwards) and the gold in whole units to the
/// account of the owner (`Player_AddResource`); then [`seek_corpse`] - the end calls no
/// `abort` (C-320). Returns 0.
fn on_rob(sim: &mut Sim, id: ObjectId, corpse: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    if sim.object(corpse).is_some() {
        let mut q = Message::new(TAKE_GOLD, &[0]);
        send_mut(sim, corpse, &mut q);
        if let Some(p) = sim.state.players.get_mut(usize::from(id.slot())) {
            p.add_resource(0, q.arg(0));
        }
    }
    seek_corpse(sim, id, None)
}

/// `Abort` (slot 11, `0x005237B0`, re/headhunter-robbing 3.9): clear group `0x80`, lock off and put the attack module
/// back into the tick, if present (also switched off; [`release_animation`]). Returns
/// `true`.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
    }
    true
}

/// Tick (slot 13 `0x005119A0`): only `Module_UpdateEvents`.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}
