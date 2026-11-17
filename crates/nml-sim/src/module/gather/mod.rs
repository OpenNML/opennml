//! Gatherer base `CPropertyGetResource` (vtable `0x00623E08`, re/gathering): the
//! shared state and flow of gold mining (module 7), lumbering (8), harvesting (12) and fishing
//! (55) - message dispatcher, abort, base tick, path to the delivery building, delivery with
//! credit, time formula of the gathering steps and load check.
//!
//! The derived modules embed [`Gather`] and supply their constants (`+0x80...+0x90`, set by the
//! constructor in the original and not saved) via the table [`GatherKind`]. The base reaches their
//! "virtual" functions through the dispatchers [`on_message`] (slot 12), [`on_dest_reached`]
//! (slot 18), [`goto_source`] (slot 19) and [`goto_delivery`] (slot 20); the abort (slot 11) runs
//! through [`crate::module::abort`].

pub mod fishing;
pub mod harvest;
pub mod hunting;
pub mod lumber;
pub mod mining;

use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::filter::matches_class;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_fault};
use crate::path::BLOCKED;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::region_id_at;

use super::ModuleKind;

/// Event group of all gathering timers (`Module_PostDelayedMessage(..., 0x80)`).
pub const GROUP: u8 = 0x80;
/// Load capacity (attribute 17, `AttributeSet_GetMaxValue`).
pub const CAPACITY: u32 = 17;
/// Query to walk: path layer `m_PPHandleNxN` (valid 0...8).
const WALK_LAYER: u32 = 0x1400;
/// Query to a building: release the docking place of the ID.
const UNREGISTER_DOCK: u32 = 0xD03;

/// States `m_Status` (`+0x78`) that all gatherers share; each derived module keeps its own work
/// states (gold 300, wood 200/201, harvest 100, fishing 400, hunt 100...102).
pub mod status {
    /// Free.
    pub const IDLE: i32 = 0;
    /// On the way to the source.
    pub const TO_SOURCE: i32 = 1;
    /// On the way to the delivery building.
    pub const TO_DELIVERY: i32 = 2;
    /// Delivery running (timer "delivery finished").
    pub const DELIVERING: i32 = 3;
    /// Search the source anew (next base tick).
    pub const SEARCH: i32 = 6;
}

/// Load kinds (`GameObject+0x2AC`) = resource index of the account.
pub mod load {
    pub const GOLD: i32 = 0;
    pub const WOOD: i32 = 1;
    pub const FOOD: i32 = 2;
    /// Empty (`GameObject_SetCarriedLoad(0, -1)`).
    pub const NONE: i32 = -1;
}

/// Constants of a gatherer derivation (constructor of the original, not saved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatherKind {
    /// Module index.
    pub index: usize,
    /// `+0x80`: success code of the walk orders (DEST_REACHED).
    pub success: u32,
    /// `+0x84`: fault code of the walk orders (ABORT).
    pub fault: u32,
    /// `+0x88`: code "delivery finished" (timer of [`deliver_load`]).
    pub done: u32,
    /// `+0x8C`: class of the delivery target (low byte module 3, above it the building flags).
    pub deliver_class: u32,
    /// `+0x90`: object state bit (`GameObject+0x1AC`) of gathering.
    pub status_bit: u32,
    /// Own load kind ([`load`]).
    pub resource: i32,
}

/// Gold mining `CPropertyGoldMining` (ctor `0x00516510`).
pub const MINING: GatherKind = GatherKind {
    index: 7,
    success: 0x207,
    fault: 0x307,
    done: 0x807,
    deliver_class: 0xC03,
    status_bit: 0x40,
    resource: load::GOLD,
};

/// Lumbering `CPropertyLumbering` (ctor `0x0051C360`): delivery to buildings with **all** bits
/// `0x1400` (lumberyard `0x1000` and finished `0x400`).
pub const LUMBERING: GatherKind = GatherKind {
    index: 8,
    success: 0x408,
    fault: 0x508,
    done: 0x908,
    deliver_class: 0x1403,
    status_bit: 0x80,
    resource: load::WOOD,
};

/// Harvesting `CPropertyHarvesting` (ctor `0x00516E90`).
pub const HARVESTING: GatherKind = GatherKind {
    index: 12,
    success: 0x40C,
    fault: 0x50C,
    done: 0x80C,
    deliver_class: 0x2403,
    status_bit: 0x400,
    resource: load::FOOD,
};

/// Fishing `CPropertyFishing` (ctor `0x005149E0`); **quirk:** state bit `0x40` like gold
/// mining.
pub const FISHING: GatherKind = GatherKind {
    index: 55,
    success: 0x237,
    fault: 0x337,
    done: 0x837,
    deliver_class: 0x8403,
    status_bit: 0x40,
    resource: load::FOOD,
};

/// Hunting `CPropertyHunting` (ctor `0x0051A860`): delivery to buildings with all bits `0x2003`
/// (food store; without the "finished" bit, but construction sites carry no accept bit).
pub const HUNTING: GatherKind = GatherKind {
    index: 22,
    success: 0x816,
    fault: 0x916,
    done: 0xB16,
    deliver_class: 0x2003,
    status_bit: 0x200,
    resource: load::FOOD,
};

/// All gatherer derivations.
pub const KINDS: [GatherKind; 5] = [MINING, LUMBERING, HARVESTING, FISHING, HUNTING];

/// Constants of the derived module at module slot `index`.
pub fn kind(index: usize) -> Option<&'static GatherKind> {
    KINDS.iter().find(|k| k.index == index)
}

/// Base fields (`PropertyGetResource_ConstructBase` `0x00515080`), saved in this order
/// (`PropertyGetResource_Serialize` `0x00515930`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gather {
    /// `m_ResourceHID` (`+0x70`): source (tree, mine, farm, basket); `ObjectId(0)` after an empty tree
    /// (quirk of lumbering), otherwise -1 = none.
    pub resource_hid: ObjectId,
    /// `m_HQHID` (`+0x74`): delivery building.
    pub hq_hid: ObjectId,
    /// `m_Status` (`+0x78`).
    pub status: i32,
    /// `m_DeliverAnim` (`+0x7C`): delivery animation (-1 everywhere in the shipped data).
    pub deliver_anim: i32,
    /// `m_TimedStatus` (`+0x94`): delayed state transition; no function sets it (only -1),
    /// only a savegame can carry it.
    pub timed_status: i32,
    /// `m_Timer` (`+0x98`).
    pub timer: i32,
}

impl Default for Gather {
    fn default() -> Self {
        Self {
            resource_hid: ObjectId::NONE,
            hq_hid: ObjectId::NONE,
            status: status::IDLE,
            deliver_anim: -1,
            timed_status: -1,
            timer: -1,
        }
    }
}

impl ModuleKind {
    /// Base fields of a gathering module.
    pub fn gather(&self) -> Option<&Gather> {
        match self {
            ModuleKind::Lumber(l) => Some(&l.gather),
            ModuleKind::Mining(m) => Some(&m.gather),
            ModuleKind::Harvest(h) => Some(&h.gather),
            ModuleKind::Fishing(f) => Some(&f.gather),
            ModuleKind::Hunting(h) => Some(&h.gather),
            _ => None,
        }
    }

    pub fn gather_mut(&mut self) -> Option<&mut Gather> {
        match self {
            ModuleKind::Lumber(l) => Some(&mut l.gather),
            ModuleKind::Mining(m) => Some(&mut m.gather),
            ModuleKind::Harvest(h) => Some(&mut h.gather),
            ModuleKind::Fishing(f) => Some(&mut f.gather),
            ModuleKind::Hunting(h) => Some(&mut h.gather),
            _ => None,
        }
    }
}

/// Base fields of the gathering module at slot `index`.
pub fn gather<'s>(sim: &'s Sim, id: ObjectId, index: usize) -> Option<&'s Gather> {
    sim.module(id, index)?.kind.gather()
}

pub fn gather_mut<'s>(sim: &'s mut Sim, id: ObjectId, index: usize) -> Option<&'s mut Gather> {
    sim.module_mut(id, index)?.kind.gather_mut()
}

/// State `m_Status` (-1 without a gathering module).
pub fn state(sim: &Sim, id: ObjectId, index: usize) -> i32 {
    gather(sim, id, index).map_or(-1, |g| g.status)
}

pub fn set_state(sim: &mut Sim, id: ObjectId, index: usize, s: i32) {
    if let Some(g) = gather_mut(sim, id, index) {
        g.status = s;
    }
}

fn set_status_flags(sim: &mut Sim, id: ObjectId, bits: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(bits, on);
    }
}

/// Grid position `(x, z)`.
pub(crate) fn position(sim: &Sim, id: ObjectId) -> (i32, i32) {
    sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0))
}

/// Walk layer `m_PPHandleNxN` (query `0x1400`; -1 without walk).
pub(crate) fn walk_layer(sim: &mut Sim, id: ObjectId) -> i32 {
    send(sim, id, &Message::new(WALK_LAYER, &[]))
}

/// `Module_PostDelayedMessage(d, msg, once, 0x80)`.
pub fn post_delayed(sim: &mut Sim, id: ObjectId, index: usize, delay: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, index) {
        m.base.post_delayed_message(delay, msg, false, GROUP);
    }
}

/// `Module_ClearEventGroup(0x80)`.
pub fn clear_events(sim: &mut Sim, id: ObjectId, index: usize) {
    if let Some(m) = sim.module_mut(id, index) {
        m.base.clear_event_group(GROUP);
    }
}

/// `GameObject_GetAnimLabelTime(anim, label)` (`0x004669D0`) of the own model.
pub fn anim_label_time(sim: &Sim, id: ObjectId, anim: i32, label: i32) -> i32 {
    sim.object(id)
        .map_or(0, |o| sim.rules.anim_label_time_at(o.guid, anim, label))
}

/// `IntList_GetAt(i)` (`0x00405840`): entry with bounds check, otherwise 0.
pub fn list_at(list: &[i32], i: usize) -> i32 {
    list.get(i).copied().unwrap_or(0)
}

/// `GameObject_SetAnimation(anim, 128.0, 0, 0, 0)` (`0x00466780`): wanted, without restart and
/// without immediate handover.
pub fn set_animation(sim: &mut Sim, id: ObjectId, anim: i32) {
    if let Some(o) = sim.object_mut(id) {
        o.set_animation_speed(anim, 0, 128.0, false, false, false);
    }
}

/// `GameObject_SetCarriedLoad(n, kind)` (`0x0046B960`): amount and kind (`+0x2A8/+0x2AC`) and the
/// load capacity display (`AttributeSet_SetCurrentValue(17, n)`). The notification of the selection
/// at the transition empty <-> loaded and `GameWorld_RebuildTargetActions` are UI.
pub fn set_carried_load(sim: &mut Sim, id: ObjectId, amount: i32, kind: i32) {
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, CAPACITY);
    let Some(o) = sim.object_mut(id) else { return };
    o.load_amount = amount;
    o.load_kind = kind;
    o.attributes.set_current_value(CAPACITY, amount, p);
}

/// Load check of all gatherers (`GameObject_GetCarriedLoad` `0x00466880`): the amount if the kind
/// is the own one, otherwise 0 (the foreign load is overwritten at the next [`set_carried_load`]).
pub fn own_load(sim: &Sim, id: ObjectId, kind: i32) -> i32 {
    sim.object(id).map_or(0, |o| {
        if o.load_kind == kind {
            o.load_amount
        } else {
            0
        }
    })
}

/// Load capacity (`AttributeSet_GetMaxValue(17)`, maximum with upgrades, not the current value).
pub fn capacity(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).map_or(0, |o| {
        let p = sim.type_upgrade_percent(o.owner(), o.guid, CAPACITY);
        o.attributes.max_value(CAPACITY, p)
    })
}

/// Time formula of the gathering steps (`0x005167F0`, `0x00517140`, `0x00514C50`, `0x0051AB80` and the
/// chop interval `0x0051BF9F...`/`0x0051C263...`): rate (units per minute, < 1 -> 1), `t = 60000 / r
/// + leak` (both `IDIV`, toward 0), delay `floor(t/100) * 100`, new leak `t % 100`. The leak only
/// compensates the rounding down to 100 ms, not the additional tick per timer (a
/// timer with `d` fires after `d/100 + 1` object ticks); the remainder of `60000 / r` is lost.
pub fn unit_delay(rate: i32, leak: i32) -> (i32, i32) {
    let r = if rate < 1 { 1 } else { rate };
    let t = (60_000 / r).wrapping_add(leak);
    ((t / 100).wrapping_mul(100), t % 100)
}

/// `schedule_next_unit`: rate from the current value of `rate_attr` (gold 33, wood 32, otherwise 31;
/// without "current value" = maximum with upgrades), schedule the next step `msg`; returns the new
/// leak.
pub fn schedule_next_unit(
    sim: &mut Sim,
    id: ObjectId,
    index: usize,
    rate_attr: u32,
    leak: i32,
    msg: Message,
) -> i32 {
    let (d, leak) = unit_delay(sim.attribute(id, rate_attr), leak);
    post_delayed(sim, id, index, d, msg);
    leak
}

// --- Dispatchers of the "virtual" functions of the derived modules ---------------------------

/// Slot 12 `OnMessage` of the derived module (`vf+0x30`).
pub fn on_message(sim: &mut Sim, id: ObjectId, index: usize, env: &Envelope) -> bool {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Lumber(_)) => lumber::on_message(sim, id, env),
        Some(ModuleKind::Mining(_)) => mining::on_message(sim, id, env),
        Some(ModuleKind::Harvest(_)) => harvest::on_message(sim, id, env),
        Some(ModuleKind::Fishing(_)) => fishing::on_message(sim, id, env),
        _ => on_command(sim, id, index, env),
    }
}

/// Slot 18 `OnDestReached` of the derived module (`vf+0x48`).
pub fn on_dest_reached(sim: &mut Sim, id: ObjectId, index: usize, env: &Envelope) -> bool {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Lumber(_)) => lumber::on_dest_reached(sim, id, env),
        Some(ModuleKind::Mining(_)) => mining::on_dest_reached(sim, id, env),
        Some(ModuleKind::Harvest(_)) => harvest::on_dest_reached(sim, id, env),
        Some(ModuleKind::Fishing(_)) => fishing::on_dest_reached(sim, id, env),
        _ => base_on_dest_reached(sim, id, index),
    }
}

/// Slot 19 "to the source" of the derived module (`vf+0x4C`; wood `PropertyLumbering_GotoTree`, gold
/// `PropertyGoldMining_GotoMine`, harvest `PropertyHarvesting_GotoFarm`, fishing
/// `PropertyFishing_GotoBaskets`, hunt `PropertyHunting_GotoCarcass`).
pub fn goto_source(sim: &mut Sim, id: ObjectId, index: usize) {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Lumber(_)) => lumber::goto_tree(sim, id),
        Some(ModuleKind::Mining(_)) => mining::goto_mine(sim, id),
        Some(ModuleKind::Harvest(_)) => harvest::goto_farm(sim, id),
        Some(ModuleKind::Fishing(_)) => fishing::goto_baskets(sim, id),
        Some(ModuleKind::Hunting(_)) => hunting::goto_carcass(sim, id),
        _ => {}
    }
}

/// Slot 20 "to the delivery building" of the derived module (`vf+0x50`; `hint` = suggested building, -1 =
/// none).
pub fn goto_delivery(sim: &mut Sim, id: ObjectId, index: usize, hint: ObjectId) {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Lumber(_)) => lumber::goto_delivery_building(sim, id, hint),
        _ => goto_delivery_building(sim, id, index, hint),
    }
}

// --- Base functions -------------------------------------------------------------------------

/// `PropertyGetResource_Query` (`0x00515250`, slot 7 of the base): envelope `{msg, no
/// feedback messages, -1, -1}` to `OnMessage` of the derived module; 1 = handled, otherwise -1.
pub fn base_query(sim: &mut Sim, id: ObjectId, index: usize, msg: &Message) -> i32 {
    let env = Envelope::bare(msg.clone());
    if on_message(sim, id, index, &env) {
        1
    } else {
        -1
    }
}

/// `PropertyGetResource_OnCommand` (`0x00515820`, slot 12 of the base): comparison on the full
/// code. A module thus also reacts to the codes of other gathering modules if they reach it
/// (the low-byte switch of `GameObject_SendMessage` practically prevents that; the hunt codes
/// `0x816/0x916/0xA16/0xB16` go to the hunt (module 22, [`hunting`]).
pub fn on_command(sim: &mut Sim, id: ObjectId, index: usize, env: &Envelope) -> bool {
    match env.msg.code {
        0x207 | 0x235 | 0x237 | 0x408 | 0x40C | 0x816 => on_dest_reached(sim, id, index, env),
        0x307 | 0x335 | 0x337 | 0x508 | 0x50C | 0x916 => {
            crate::module::abort(sim, id, index);
            true
        }
        0x607 | 0x608 | 0x60C | 0x635 | 0x637 | 0xA16 => {
            on_deliver_command(sim, id, index, &env.msg);
            true
        }
        0x807 | 0x80C | 0x835 | 0x837 | 0x908 | 0xB16 => {
            deliver_load(sim, id, index);
            true
        }
        _ => false,
    }
}

/// `PropertyGetResource_OnDeliverCommand` (`0x00515290`): set state bit `+0x90` and go to the
/// delivery building `args[0]` (slot 20).
fn on_deliver_command(sim: &mut Sim, id: ObjectId, index: usize, msg: &Message) {
    let Some(k) = kind(index) else { return };
    set_status_flags(sim, id, k.status_bit, true);
    goto_delivery(sim, id, index, ObjectId::from_i32(msg.arg(0)));
}

/// `PropertyGetResource_OnDestReached` (`0x005157F0`, slot 18 of the base): in state 2 deliver
/// (1), otherwise only log (0).
pub fn base_on_dest_reached(sim: &mut Sim, id: ObjectId, index: usize) -> bool {
    if state(sim, id, index) != status::TO_DELIVERY {
        log::debug!("Gatherer {id:?}: unexpected state when message DEST_REACHED arrives");
        return false;
    }
    deliver_load(sim, id, index);
    true
}

/// `PropertyGetResource_Stop` (`0x00515110`, slot 11 of the base): in state 2 release the docking
/// place at the delivery building, in state 1 at the source (`0xD03`; a tree answers -1), send the
/// fault message (synchronously), state 0, clear **all** five gathering state bits `0x80, 0x40,
/// 0x400, 0x200, 0x8` (quirk: independent of the module), discard the delayed state and
/// clear the timers of group `0x80`. **Quirk:** state 3 (delivery running) does not unregister
/// - the slot stays claimed with the own ID until the same unit docks again.
pub fn stop(sim: &mut Sim, id: ObjectId, index: usize) -> bool {
    let free = Message::new(UNREGISTER_DOCK, &[id.as_i32()]);
    if let Some(g) = gather(sim, id, index).cloned()
        && g.status == status::TO_DELIVERY
        && sim.object(g.hq_hid).is_some()
    {
        send(sim, g.hq_hid, &free);
    }
    if let Some(g) = gather(sim, id, index).cloned()
        && g.status == status::TO_SOURCE
        && sim.object(g.resource_hid).is_some()
    {
        send(sim, g.resource_hid, &free);
    }
    send_fault(sim, id, index);
    set_state(sim, id, index, status::IDLE);
    for bit in [0x80, 0x40, 0x400, 0x200, 0x8] {
        set_status_flags(sim, id, bit, false);
    }
    if let Some(g) = gather_mut(sim, id, index) {
        g.timed_status = -1;
    }
    clear_events(sim, id, index);
    true
}

/// `PropertyGetResource_TickBase` (`0x00515210`, without `UpdateEvents`): the delayed
/// state transition counts down only in state 0 (the branch is dead in the game, only a
/// savegame can trigger it); in state 6 to the source (slot 19).
pub fn tick_base(sim: &mut Sim, id: ObjectId, index: usize) {
    let Some(g) = gather_mut(sim, id, index) else {
        return;
    };
    if g.timed_status >= 0 {
        if g.status == status::IDLE {
            g.timer = g.timer.wrapping_sub(1);
            if g.timer < 0 {
                g.status = g.timed_status;
                g.timed_status = -1;
            }
        } else {
            g.timed_status = -1;
        }
    }
    if g.status == status::SEARCH {
        goto_source(sim, id, index);
    }
}

/// `PropertyGetResource_GotoDeliveryBuilding(hint)` (`0x005152E0`, slot 20 of the base): state 2.
/// The hint building is taken if it satisfies `GameObject_MatchesClass(+0x8C)` -
/// **quirk:** there **one** bit of the class suffices and the owner is not checked (any
/// finished building or a foreign unfinished lumberyard; the credit still goes to the own
/// player). Otherwise `CollisionMgr_FindNearestMatching` searches the whole map with edge distance
/// (hard radius) for an own building with **all** bits of the class (`0x150`), with a valid region
/// of the own tile additionally reachable (`mode & 0x20`, building: `0x2C03 {layer, region}`).
/// Found: `m_HQHID` and walk FINDTO `{building, -1, 1}` with the feedback messages `+0x80`/`+0x84`
/// (docking via `0x3403/0xB03/0xC03`); otherwise `m_HQHID = -1` and abort (the load stays).
/// The filter chain of class `0x4000FF` (only `CPropertyFireGuarding`, not a gatherer) is missing.
pub fn goto_delivery_building(sim: &mut Sim, id: ObjectId, index: usize, hint: ObjectId) {
    let Some(k) = kind(index) else { return };
    set_state(sim, id, index, status::TO_DELIVERY);
    log::debug!("Gatherer {id:?}: going to main house ... hint {hint:?}");
    let mut hq =
        (sim.object(hint).is_some() && matches_class(sim, hint, k.deliver_class)).then_some(hint);
    if hq.is_none() {
        let mut f = MatchFilter {
            mode: 0x150,
            match_bits: k.deliver_class,
            ref_object: 0,
            ..MatchFilter::constructed()
        };
        let layer = walk_layer(sim, id);
        let region = region_id_at(sim, id, layer, -1, -1);
        f.region = i32::from(region);
        if region != BLOCKED {
            f.mode |= 0x20;
            f.layer = layer;
        }
        let (pos, hard) = (
            position(sim, id),
            sim.object(id).map_or(0, |o| o.hard_radius),
        );
        hq = find_nearest_matching(sim, pos, &f, -1, i32::from(id.slot()), hard);
    }
    let Some(h) = hq else {
        log::debug!("Gatherer {id:?}: no main house found");
        if let Some(g) = gather_mut(sim, id, index) {
            g.hq_hid = ObjectId::NONE;
        }
        crate::module::abort(sim, id, index);
        return;
    };
    if let Some(g) = gather_mut(sim, id, index) {
        g.hq_hid = h;
    }
    let env = Envelope {
        msg: Message::new(0x200, &[h.as_i32(), -1, 1]),
        success: Some(Message::new(k.success, &[])),
        fault: Some(Message::new(k.fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyGetResource_DeliverLoad` (`0x005155E0`), two calls. First (arrival in state 2):
/// state 3, timer "delivery finished" (`+0x88`) after time marker 1 of the delivery animation,
/// at least 100 ms - **quirk:** the animation starts only if the duration is above 100 ms
/// (in the shipped data it is -1 everywhere: 100 ms, no animation, credit in the 2nd tick after
/// arrival). Second (timer, state 3): credit `factor * amount / 100` (32-bit product,
/// `IDIV`) to the owner (`Player_AddResource`: only kinds 0-4), number above the head as
/// [`SimEvent::Credited`] (the original shows it only to the local player), clear the load,
/// release the docking place (`0xD03`) and back to the source (slot 19; the state stays 3 until
/// then). In between the statistics (`0x00515791`, [`crate::accounting`]) at the owner: kinds
/// 10/9/8 for gold/wood/food with the credited amount; for another load kind the original books
/// an uninitialized kind (W-41, O-172), the reimplementation nothing.
pub fn deliver_load(sim: &mut Sim, id: ObjectId, index: usize) {
    let Some(k) = kind(index) else { return };
    let Some(g) = gather(sim, id, index).cloned() else {
        return;
    };
    if g.status != status::DELIVERING {
        log::debug!("Gatherer {id:?}: delivering load");
        set_state(sim, id, index, status::DELIVERING);
        let len = anim_label_time(sim, id, g.deliver_anim, 1);
        let d = if len < 100 {
            100
        } else {
            anim_label_time(sim, id, g.deliver_anim, 1)
        };
        post_delayed(sim, id, index, d, Message::new(k.done, &[]));
        if d > 100 {
            set_animation(sim, id, g.deliver_anim);
        }
        return;
    }
    let slot = id.slot();
    let (amount, kind) = sim
        .object(id)
        .map_or((0, load::NONE), |o| (o.load_amount, o.load_kind));
    let factor = sim.state.economy.resource_factor(i32::from(slot));
    let credited = factor.wrapping_mul(amount) / 100;
    sim.state.events.push(SimEvent::Credited {
        object: id,
        amount: credited,
        kind,
    });
    if let (Some(p), Ok(i)) = (
        sim.state.players.get_mut(usize::from(slot)),
        usize::try_from(kind),
    ) {
        p.add_resource(i, credited);
    }
    let stat = match kind {
        load::GOLD => Some(crate::accounting::kind::GOLD),
        load::WOOD => Some(crate::accounting::kind::WOOD),
        load::FOOD => Some(crate::accounting::kind::FOOD),
        _ => None,
    };
    if let Some(stat) = stat {
        crate::accounting::add_statistic(sim, i32::from(slot), stat, credited);
    }
    set_carried_load(sim, id, 0, load::NONE);
    let hq = gather(sim, id, index).map_or(ObjectId::NONE, |g| g.hq_hid);
    if sim.object(hq).is_some() {
        send(sim, hq, &Message::new(UNREGISTER_DOCK, &[id.as_i32()]));
    }
    goto_source(sim, id, index);
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::rules::{BuildingShape, Rules};
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// SPA colonist (rate 60), WID warrior (rate 80), lumberyard, a house without lumberyard bit and
    /// a deciduous tree; walk values like the colonist in the shipped data.
    pub(crate) const TYPES: &str = concat!(
        "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\n",
        "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
        "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
        "PROPID 2\nVALUEID 30001 1 1 0\n",
        "PROPID 8\nVALUEID 90001 6 1 4\nVALUEID 90002 1 1 5\nVALUEID 90005 6 1 10\n",
        "VALUEID 90006 6 1 3\nVALUEID 90007 6 1 -1\n",
        "GUID 1400\nNAME WID_Krieger\nVALUEID 100 6 1 0\n",
        "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
        "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
        "PROPID 2\nVALUEID 30001 1 1 0\n",
        "PROPID 8\nVALUEID 90001 6 1 6\nVALUEID 90002 1 1 2\nVALUEID 90006 6 1 5\n",
        "VALUEID 90007 6 1 -1\n",
        "GUID 1013\nNAME Holzlager\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 4096\n",
        "GUID 1014\nNAME Haus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
        "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nVALUEID 50003 6 1 0\n",
    );
    pub(crate) const COLONIST: u32 = 1100;
    pub(crate) const WARRIOR: u32 = 1400;
    pub(crate) const LUMBERYARD: u32 = 1013;
    pub(crate) const HOUSE: u32 = 1014;
    pub(crate) const TREE: u32 = 3602;

    /// Rules: load capacity 20, wood rate 60 or 80, deciduous tree with 200 wood, buildings about
    /// 2 x 2 tiles.
    pub(crate) fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        r.defaults.insert(
            COLONIST,
            [(16, 183), (17, 20), (32, 60)].into_iter().collect(),
        );
        r.defaults.insert(
            WARRIOR,
            [(16, 183), (17, 20), (32, 80)].into_iter().collect(),
        );
        r.defaults.insert(TREE, [(25, 200)].into_iter().collect());
        let shape = BuildingShape {
            center: [0.0; 3],
            half: [100.0, 50.0, 100.0],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        };
        r.building_shapes.insert(LUMBERYARD, shape.clone());
        r.building_shapes.insert(HOUSE, shape);
        r
    }

    pub(crate) fn state(rules: &Rules) -> SimState {
        let mut state = SimState::new(0, 0);
        state.init_map(rules);
        state
    }

    /// Finished object on the center of tile `tile`.
    pub(crate) fn spawn(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let world = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
        desc.flags |= desc_flags::PLACED;
        create_object(sim, &mut desc).expect("object created")
    }

    /// Economy types for gold, harvest and fishing: colonist with gold mining (walk animation 6, stand
    /// animation 7, mining "16 17"), lumbering and harvest (walk animation 9, stand 10, field 8), mine
    /// (`GOLDMINE`), gold store (`0x800`), farm (`FARM`), food store (`0x2000`), harbor (`0x8000`),
    /// lumberyard, gold deposit, soil and fishing basket (nature), fishing boats SPA/ENG (fishing with
    /// the values 80001...80005) and a deciduous tree.
    pub(crate) const ECON_TYPES: &str = concat!(
        "GUID 1100
NAME SPA_Kolonist
VALUEID 100 6 1 0
",
        "PROPID 0
VALUEID 10001 6 1 3
VALUEID 10003 6 1 264448
VALUEID 10004 6 1 512
",
        "VALUEID 10005 6 1 1024
VALUEID 10006 6 1 200
VALUEID 10007 6 1 1
VALUEID 10008 6 1 4
",
        "PROPID 2
VALUEID 30001 1 1 0
",
        "PROPID 7
VALUEID 80001 6 1 6
VALUEID 80004 6 1 0
VALUEID 80005 1 1 7
",
        "VALUEID 80006 6 1 -1
VALUEID 80007 1 1 16 17
",
        "PROPID 8
VALUEID 90001 6 1 4
VALUEID 90002 1 1 5
VALUEID 90006 6 1 3
",
        "VALUEID 90007 6 1 -1
",
        "PROPID 12
VALUEID 130001 1 1 9
VALUEID 130002 1 1 10
VALUEID 130003 6 1 -1
",
        "VALUEID 130006 6 1 8
",
        "GUID 1117
NAME SPA_Fischerboot
VALUEID 100 6 1 0
",
        "PROPID 0
VALUEID 10001 6 1 3
VALUEID 10003 6 1 264448
VALUEID 10004 6 1 512
",
        "VALUEID 10005 6 1 1024
VALUEID 10006 6 1 200
VALUEID 10007 6 1 1
VALUEID 10008 6 1 4
",
        "PROPID 55
VALUEID 80001 6 1 0
VALUEID 80002 1 1 2 3
VALUEID 80003 1 1 4 5
",
        "VALUEID 80004 1 1 9
VALUEID 80005 1 1 6 7
",
        "GUID 2017
NAME ENG_Fischerboot
VALUEID 100 6 1 0
",
        "PROPID 0
VALUEID 10001 6 1 3
VALUEID 10003 6 1 264448
VALUEID 10004 6 1 512
",
        "VALUEID 10005 6 1 1024
VALUEID 10006 6 1 200
VALUEID 10007 6 1 1
VALUEID 10008 6 1 4
",
        "PROPID 55
VALUEID 80001 6 1 0
VALUEID 80002 1 1 2
VALUEID 80003 1 1 4
",
        "VALUEID 80005 1 1 6
",
        "GUID 1010
NAME Mine
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 256
",
        "GUID 1011
NAME Goldlager
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 2048
",
        "GUID 1003
NAME Feld
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 512
",
        "GUID 1012
NAME Nahrungslager
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 8192
",
        "GUID 1020
NAME Hafen
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 32768
",
        "GUID 1013
NAME Holzlager
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 4096
",
        "GUID 3500
NAME Goldvorkommen
VALUEID 100 6 1 512
PROPID 6
VALUEID 70002 6 1 1280
",
        "GUID 3503
NAME Boden
VALUEID 100 6 1 512
PROPID 6
VALUEID 70002 6 1 1536
",
        "GUID 3505
NAME Fischerkorb
VALUEID 100 6 1 512
PROPID 6
VALUEID 70002 6 1 0
",
        "GUID 3602
NAME Laubbaum
VALUEID 100 6 1 33816832
PROPID 4
VALUEID 50003 6 1 0
",
    );
    pub(crate) const BOAT: u32 = 1117;
    pub(crate) const ENG_BOAT: u32 = 2017;
    pub(crate) const MINE: u32 = 1010;
    pub(crate) const GOLD_STORE: u32 = 1011;
    pub(crate) const FARM: u32 = 1003;
    pub(crate) const FOOD_STORE: u32 = 1012;
    pub(crate) const HARBOR: u32 = 1020;
    pub(crate) const GOLD_DEPOSIT: u32 = 3500;
    pub(crate) const SOIL: u32 = 3503;
    pub(crate) const BASKET: u32 = 3505;

    /// Rules of the economy types (map 64 x 64): colonist with load capacity 20, sight 32 and
    /// the rates 60/60/80 (food/wood/gold, SPA), boats with 300/80 and 500/30, buildings about
    /// 2 x 2 tiles, the farm with the nodes `farmer01...05` (one tile apart, side by side in front of
    /// the farm).
    pub(crate) fn econ_rules() -> Rules {
        let mut r = Rules::synthetic(ECON_TYPES, 64);
        r.defaults.insert(
            COLONIST,
            [(16, 183), (17, 20), (18, 32), (31, 60), (32, 60), (33, 80)]
                .into_iter()
                .collect(),
        );
        r.defaults
            .insert(BOAT, [(16, 183), (17, 300), (31, 80)].into_iter().collect());
        r.defaults.insert(
            ENG_BOAT,
            [(16, 183), (17, 500), (31, 30)].into_iter().collect(),
        );
        r.defaults.insert(TREE, [(25, 200)].into_iter().collect());
        let shape = BuildingShape {
            center: [0.0; 3],
            half: [100.0, 50.0, 100.0],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        };
        for guid in [MINE, GOLD_STORE, FOOD_STORE, HARBOR, LUMBERYARD] {
            r.building_shapes.insert(guid, shape.clone());
        }
        let mut farm = shape;
        for k in 0..5 {
            farm.helper_nodes.insert(
                format!("farmer{:02}", k + 1),
                [(k as f32 - 2.0) * 128.0, 0.0, -256.0],
            );
        }
        r.building_shapes.insert(FARM, farm);
        r
    }

    /// Deposit (nature, slot 10) with the amount `amount` from the `[CONFIG]` (26 gold, 24 otherwise).
    pub(crate) fn spawn_deposit(
        sim: &mut Sim,
        guid: u32,
        amount: i32,
        tile: (i32, i32),
    ) -> ObjectId {
        let cfg = if guid == GOLD_DEPOSIT { 26 } else { 24 };
        sim.state.object_config = [(cfg, amount)].into_iter().collect();
        let world = |t: i32| t as f32 * 128.0 + 64.0;
        let id = create_object(
            sim,
            &mut ObjectDesc::new(guid, 10, [world(tile.0), 0.0, world(tile.1)]),
        )
        .expect("deposit created");
        sim.state.object_config.clear();
        id
    }

    /// Whole simulation steps until `done` (at most `max`); returns the number.
    pub(crate) fn run_until(
        state: &mut SimState,
        rules: &Rules,
        max: usize,
        done: impl Fn(&Sim) -> bool,
    ) -> usize {
        for n in 1..=max {
            crate::tick::step(state, rules, &[]);
            if done(&Sim::new(state, rules)) {
                return n;
            }
        }
        panic!("not reached after {max} steps");
    }

    // QUIRK W-21: kept
    #[test]
    fn the_unit_delay_rounds_to_100_ms_and_carries_the_rest() {
        // Rate 60: constantly 1000 ms without leak.
        assert_eq!(unit_delay(60, 0), (1000, 0));
        // Rate 80: 700/800 alternating (leak 50/0).
        assert_eq!(unit_delay(80, 0), (700, 50));
        assert_eq!(unit_delay(80, 50), (800, 0));
        // Rate 90 and 45: sequences from the analysis.
        for (rate, want) in [
            (
                90,
                [
                    (600, 66),
                    (700, 32),
                    (600, 98),
                    (700, 64),
                    (700, 30),
                    (600, 96),
                ],
            ),
            (
                45,
                [
                    (1300, 33),
                    (1300, 66),
                    (1300, 99),
                    (1400, 32),
                    (1300, 65),
                    (1300, 98),
                ],
            ),
        ] {
            let mut leak = 0;
            for w in want {
                let got = unit_delay(rate, leak);
                assert_eq!(got, w, "rate {rate}");
                leak = got.1;
            }
        }
        // Rate 0 or negative like 1: 60000 ms (601 ticks).
        assert_eq!(unit_delay(0, 0), (60_000, 0));
        assert_eq!(unit_delay(-5, 0), (60_000, 0));
        // Rate 70: the remainder of 60000/70 (857.14) is lost.
        assert_eq!(unit_delay(70, 0), (800, 57));
        // Rates in the shipped data (re/gathering 11.2, 11.3): gold 33, food 31.
        assert_eq!(unit_delay(33, 0), (1800, 18));
        assert_eq!(unit_delay(31, 70), (2000, 5));
        let mut leak = 0;
        for want in [(1900, 35), (1900, 70), (2000, 5)] {
            let got = unit_delay(31, leak);
            assert_eq!(got, want, "rate 31");
            leak = got.1;
        }
        // Very large rate: 60000 / r = 0, no delay.
        assert_eq!(unit_delay(600_000, 0), (0, 0));
    }

    #[test]
    fn gather_kinds_carry_the_constructor_constants() {
        assert_eq!(kind(8), Some(&LUMBERING));
        assert_eq!(
            (MINING.success, MINING.deliver_class, MINING.status_bit),
            (0x207, 0xC03, 0x40)
        );
        assert_eq!(
            (HARVESTING.fault, HARVESTING.done, HARVESTING.status_bit),
            (0x50C, 0x80C, 0x400)
        );
        assert_eq!((FISHING.deliver_class, FISHING.status_bit), (0x8403, 0x40));
        assert_eq!(kind(22), Some(&HUNTING));
        assert!(kind(3).is_none());
        let g = Gather::default();
        assert_eq!(
            (
                g.resource_hid,
                g.hq_hid,
                g.status,
                g.deliver_anim,
                g.timed_status,
                g.timer
            ),
            (ObjectId::NONE, ObjectId::NONE, 0, -1, -1, -1)
        );
    }

    /// Lumberjack with `amount` wood in state 3 (delivery running) at resource factor `factor`:
    /// second call of `DeliverLoad`. Returns: balance gain (fixed point) and event.
    fn deliver(amount: i32, factor: i32) -> (i32, Option<SimEvent>) {
        let rules = rules();
        let mut state = state(&rules);
        state.economy.set_resource_factor(0, factor);
        let mut sim = Sim::new(&mut state, &rules);
        let id = spawn(&mut sim, COLONIST, 0, (5, 5));
        set_carried_load(&mut sim, id, amount, load::WOOD);
        assert_eq!(sim.attribute(id, CAPACITY), amount);
        set_state(&mut sim, id, 8, status::DELIVERING);
        let before = sim.state.players[0].resource_raw(1);
        sim.state.events.clear();
        deliver_load(&mut sim, id, 8);
        let o = sim.object(id).unwrap();
        assert_eq!((o.load_amount, o.load_kind), (0, load::NONE));
        assert_eq!(sim.attribute(id, CAPACITY), 0);
        let credited = sim
            .state
            .events
            .iter()
            .find(|e| matches!(e, SimEvent::Credited { .. }))
            .cloned();
        (sim.state.players[0].resource_raw(1) - before, credited)
    }

    #[test]
    fn delivering_credits_factor_times_load_over_100_to_the_owner() {
        for (amount, factor, want) in [
            (20, 100, 20),
            (20, 80, 16),
            (20, 120, 24),
            (1, 50, 0),
            (19, 90, 17),
        ] {
            let (raw, event) = deliver(amount, factor);
            assert_eq!(raw, want << 12, "{amount} at {factor} %");
            assert!(
                matches!(event, Some(SimEvent::Credited { amount: a, kind: 1, .. }) if a == want),
                "{event:?}"
            );
        }
    }

    // QUIRK W-39: deviation
    #[test]
    fn matches_class_needs_one_bit_and_ignores_the_owner() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let yard = spawn(&mut sim, LUMBERYARD, 1, (5, 5));
        let house = spawn(&mut sim, HOUSE, 0, (10, 5));
        let unit = spawn(&mut sim, COLONIST, 0, (15, 5));
        // One bit of 0x1400 suffices: the foreign lumberyard and the own house (only "finished").
        assert!(matches_class(&mut sim, yard, LUMBERING.deliver_class));
        assert!(matches_class(&mut sim, house, LUMBERING.deliver_class));
        assert!(!matches_class(&mut sim, unit, LUMBERING.deliver_class));
    }
}
