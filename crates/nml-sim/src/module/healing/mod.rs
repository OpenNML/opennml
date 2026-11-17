//! Module 29 `CPropertyHealing` (re/healing): the healing state (animations, healing amount,
//! HEF name, readiness, targets), `ReadParams`/`Init` (1.3), the healing filter, the queries (2.1)
//! with the healing trance (3.2-3.5) and the tick (3.1). The healing flows themselves are in
//! [`heal`] (area 3.6, single 3.7).
//!
//! The target effect maps, `m_EffectID`, `m_UseTargetMap1` and the hit list are presentation
//! (host) and are missing from the state (re/healing 5); likewise the healing circle
//! (`healing_circle.hef`, the host reads `ready` and `hef`) and `NotifyUiIfSelected`. The healing
//! filter is derived from the flag word ([`match_filter`]), not stored. No `OnMessage`, no abort
//! (slot 11 only `true`), `m_IsBusy` (slot 10) stays false.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::filter::matches_class;
use crate::id::ObjectId;
use crate::message::Message;
use crate::object::status;
use crate::sim::Sim;

use super::events::update_events;
use super::gather::{anim_label_time, list_at, set_animation};
use super::stand::{list_modulo, parse_int_list};
use super::{ModuleKind, init_flags};

pub mod heal;

pub const INDEX: usize = 29;
/// Attribute "extra energy" (config 15), the mana of the healers.
const MANA: u32 = 15;
/// Timer group of `0x31D`/`0x41D` (`Module_PostDelayedMessage`, re/healing 2.2).
const TIMER_GROUP: u8 = 0x80;
/// Attack and walk module: out of the tick list in the trance (re/healing 3.2, 3.5).
const ATTACK_MODULE: usize = 0x11;
const WALK_MODULE: usize = 0;
/// Building module and class "finished building" (`MatchesClass(0x403)`, re/healing 3.1).
const BUILDING_MODULE: usize = 3;
const FINISHED_BUILDING: u32 = 0x403;

/// Flags of the module (`targetdefines.ini` 143-148, low byte `0x1D`; re/healing 1.1).
pub mod flags {
    /// `NOHEALINGANIM`: without healing animation (evaluated by nobody).
    pub const NOHEALINGANIM: u32 = 0x100;
    /// `HEALINGONLYGUIDS`: only objects of the GUID list in `remove_anim` (shipyards).
    pub const ONLYGUIDS: u32 = 0x200;
    /// `HEALPTTOPT`: includes humans (walk bit `PTTOPT_NOTHUMAN`).
    pub const PTTOPT: u32 = 0x400;
    /// `HEALBUILDING`: heals buildings (no shipped type).
    pub const BUILDING: u32 = 0x800;
    /// `HEALMACHINES`: heals machines.
    pub const MACHINES: u32 = 0x1000;
    /// `SINGLEHEALING`: one target per tick (the healer units), otherwise area healing.
    pub const SINGLE: u32 = 0x2000;
}

/// Messages of the healing module (query `0x0051A0C0`, re/healing 2.1).
pub mod msg {
    /// Begin the trance (command `0x34`).
    pub const BEGIN: u32 = 0x11D;
    /// Initiate ending the trance (command `0x35`).
    pub const END_REQUEST: u32 = 0x21D;
    /// Setup finished, ready (own timer).
    pub const READY: u32 = 0x31D;
    /// Teardown finished, trance off (own timer).
    pub const END: u32 = 0x41D;
    /// `{sel}`: stand animation in the trance (stand).
    pub const STAND: u32 = 0x51D;
    /// Dying animation (dying).
    pub const DIE: u32 = 0x61D;
    /// `{1}`/`{0}`: building with occupants ready/not ready (quarters).
    pub const CONTAINER: u32 = 0x71D;
    /// `{target}`: requested target (command `0x78`).
    pub const NEW_TARGET: u32 = 0x81D;
}

/// `CPropertyHealing` (`0x1B4` bytes); parameters as `ReadParams` sets them (re/healing 1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Healing {
    /// `m_EstablishAnim` (`+0xD8`, 300003): setup animation of the trance.
    pub establish_anim: i32,
    /// `m_RemoveAnim` (`+0xDC`, 300001): teardown animation `[0]` **and** GUID list with `ONLYGUIDS`.
    pub remove_anim: Vec<i32>,
    /// `m_DieAnim` (`+0xE8`, 300004): answer to `0x61D`.
    pub die_anim: i32,
    /// `m_StandAnim` (`+0xEC`, 300005): stand animation in the trance.
    pub stand_anim: Vec<i32>,
    /// `m_HealAnim` (`+0xF8`, 300006).
    pub heal_anim: i32,
    /// `m_Healingpoints` (`+0x1A8`, 300007): hit points per minute.
    pub healing_points: i32,
    /// `m_HEFFilename` (`+0x1AC`, 300008): healing circle effect; `"0"` and missing are empty.
    pub hef: String,
    /// `m_ReadyToHeal` (`+0xFC`): heals in this tick.
    pub ready: bool,
    /// `m_SickPersonHID` (`+0xD0`): target of single healing (`ObjectId(0)` = none); may point to a
    /// removed object.
    pub sick: ObjectId,
    /// `m_NewTarget` (`+0x1B0`): requested target (`0x81D`; `ObjectId(0)` = none).
    pub new_target: ObjectId,
}

// `ObjectId` has no `Default`; the IDs start with 0 (constructor `0x0051A2B0`).
impl Default for Healing {
    fn default() -> Self {
        Self {
            establish_anim: 0,
            remove_anim: Vec::new(),
            die_anim: 0,
            stand_anim: Vec::new(),
            heal_anim: 0,
            healing_points: 0,
            hef: String::new(),
            ready: false,
            sick: ObjectId(0),
            new_target: ObjectId(0),
        }
    }
}

pub fn healing<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Healing> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Healing(h) => Some(h),
        _ => None,
    }
}

pub fn healing_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Healing> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Healing(h) => Some(h),
        _ => None,
    }
}

/// `Module_HasFlags` (`0x004FD280`) for a flag word: only the upper three bytes count.
fn has(word: u32, x: u32) -> bool {
    word & x & 0xFFFF_FF00 != 0
}

/// `m_HealingMatch` (and with `PTTOPT && BUILDING` the chain `m_InnerMatch`) after `ReadParams`
/// (`0x0051A460`, re/healing 1.3): own (`MYLIST`), injured (`ENERGYLOST`) objects with walk
/// (property 0) or buildings (3); without `MACHINES && PTTOPT` additionally the walk bit
/// `0x20000` and "machines only" (`0x2`) or "no machines" (`0x4000`). All other words
/// come from [`MatchFilter::constructed`] (in the original stack and heap leftovers, without consequence;
/// C-253).
pub fn match_filter(word: u32) -> MatchFilter {
    let mut m = MatchFilter::constructed();
    let mut inner = MatchFilter::constructed();
    let chained = has(word, flags::PTTOPT) && has(word, flags::BUILDING);
    if chained {
        // ORNEXT | ENERGYLOST | MYLIST | ANDALL | CHECKPROPERTY, follow-up filter: any building.
        m.mode = 0x2080_0114;
        inner.mode = 4;
        inner.prop_id = 3;
        m.prop_id = 0;
    } else {
        // ENERGYLOST | MYLIST | ANDALL | CHECKPROPERTY: walk (0) or building (3).
        m.mode = 0x0080_0114;
        m.prop_id = if has(word, flags::BUILDING) { 3 } else { 0 };
    }
    if !(has(word, flags::MACHINES) && has(word, flags::PTTOPT)) {
        let bits = if has(word, flags::MACHINES) {
            0x2
        } else {
            0x4000
        };
        m.match_bits = 0x2_0000;
        inner.match_bits = 0x2_0000;
        m.mode |= bits;
        inner.mode |= bits;
    }
    if chained {
        m.next = Some(Box::new(inner));
    }
    m
}

/// `PropertyHealing_ReadParams` (`0x0051A460`, re/healing 1.3): sets the mana to 0 (C-241,
/// `SetCurrentValue(0xF, 0)`), reads 300001-300008 and the flags (300002); `"0"` as HEF name
/// means none. `ReadParams` runs during creation after `InitAttributes`, so a freshly created
/// healer starts with 0 mana instead of full. The filter is supplied by [`match_filter`].
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    if let Some(o) = sim.object(id) {
        let percent = sim.type_upgrade_percent(o.owner(), o.guid, MANA);
        if let Some(o) = sim.object_mut(id) {
            o.attributes.set_current_value(MANA, 0, percent);
        }
    }
    let text = |v: u32| desc.values.get(&v).map_or("", String::as_str);
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = init_flags(INDEX, desc);
    if let ModuleKind::Healing(h) = &mut m.kind {
        h.establish_anim = value(300_003);
        h.remove_anim = parse_int_list(text(300_001), &h.remove_anim);
        h.die_anim = value(300_004);
        h.stand_anim = parse_int_list(text(300_005), &h.stand_anim);
        h.heal_anim = value(300_006);
        h.hef = match text(300_008) {
            "0" => String::new(),
            hef => hef.to_string(),
        };
        h.healing_points = value(300_007);
    }
}

/// `PropertyHealing_Init` (`0x00519510`, slot 9): `ReadParams`, and an unfinished object
/// is not ready. The healing circle effect on the finished object is presentation (host).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, finished: bool) {
    read_params(sim, id, desc);
    if !finished && let Some(h) = healing_mut(sim, id) {
        h.ready = false;
    }
}

/// `PropertyHealing_Query` (`0x0051A0C0`, slot 7, re/healing 2.1): `0x11D`/`0x21D`/`0x31D`/`0x41D`
/// take effect (3.2-3.5) and answer -1; `0x51D {sel}` -> `m_StandAnim[sel % n]` (stand, W-152);
/// `0x61D` -> not ready, `m_DieAnim` (dying); `0x71D {1}` ready, otherwise not ready, answer 1
/// (quarters, without status and filter, C-250); `0x81D {target}` -> `m_NewTarget` without any check,
/// answer 1 (C-251); otherwise -1. A switched-off module receives no message (`send`).
pub fn query(sim: &mut Sim, id: ObjectId, message: &mut Message) -> i32 {
    let Some(h) = healing(sim, id) else {
        return -1;
    };
    match message.code {
        msg::BEGIN => begin(sim, id),
        msg::END_REQUEST => end_request(sim, id),
        msg::READY => set_ready(sim, id),
        msg::END => end(sim, id),
        msg::STAND => return list_modulo(&h.stand_anim, message.arg(0)),
        msg::DIE => {
            let die = h.die_anim;
            if let Some(h) = healing_mut(sim, id) {
                h.ready = false;
            }
            return die;
        }
        msg::CONTAINER => {
            let on = message.arg(0) == 1;
            if let Some(h) = healing_mut(sim, id) {
                h.ready = on;
            }
            return 1;
        }
        msg::NEW_TARGET => {
            let target = ObjectId::from_i32(message.arg(0));
            if let Some(h) = healing_mut(sim, id) {
                h.new_target = target;
            }
            return 1;
        }
        _ => {}
    }
    -1
}

/// `PropertyHealing_Tick` (`0x0051A1F0`, slot 13, re/healing 3.1): dead (`DIED`) -> return immediately, even
/// without timers (`0x31D`/`0x41D` of a dying object never fire). Ready -> heal ([`heal`]); otherwise
/// a finished building with a HEF name becomes ready (C-249, and again after `0x71D {0}`, C-250).
/// Then `Module_UpdateEvents` (`0x004FD7E0`): the healing tick runs before the timers, a healer that became
/// ready in this tick does not heal until the next one (re/healing 9.4).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    let Some(o) = sim.object(id) else { return };
    if o.status & status::DIED != 0 {
        return;
    }
    let building = o.has_module(BUILDING_MODULE, false);
    let Some(single) = sim
        .module(id, INDEX)
        .map(|m| m.base.has_flags(flags::SINGLE))
    else {
        return;
    };
    let Some(h) = healing(sim, id) else { return };
    if h.ready {
        // `SINGLEHEALING` -> single healing (`0x00519C00`), otherwise area (`0x00519830`).
        if single {
            heal::single_heal(sim, id);
        } else {
            heal::heal_area(sim, id);
        }
    } else if building && !h.hef.is_empty() && matches_class(sim, id, FINISHED_BUILDING) {
        // The healing circle of the local player is presentation (host).
        if let Some(h) = healing_mut(sim, id) {
            h.ready = true;
        }
    }
    update_events(sim, id, INDEX, dt);
}

/// Timer `code` after `delay_ms` (group `0x80`, once; `Module_PostDelayedMessage`).
fn post_delayed(sim: &mut Sim, id: ObjectId, delay_ms: i32, code: u32) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(delay_ms, Message::new(code, &[]), false, TIMER_GROUP);
    }
}

/// Begin the trance (`0x11D` -> `0x00519320`, re/healing 3.2): nothing if the lock bit
/// `0x800000` is set (transition of the trance or banishing, C-254) or `HEAL` is already set. Otherwise `HEAL`,
/// setup animation (without restart), `0x31D` after its marker 1, filter cleared with only `0x21D` in
/// mode 0 (C-247, K-53), attack and walk out of the tick list, lock bit on.
fn begin(sim: &mut Sim, id: ObjectId) {
    let Some(st) = sim.object(id).map(|o| o.status) else {
        return;
    };
    if st & (status::ANIM_LOCKED | status::HEAL) != 0 {
        return;
    }
    let Some(anim) = healing(sim, id).map(|h| h.establish_anim) else {
        return;
    };
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::HEAL, true);
    }
    let delay = anim_label_time(sim, id, anim, 1);
    set_animation(sim, id, anim);
    post_delayed(sim, id, delay, msg::READY);
    if let Some(o) = sim.object_mut(id) {
        o.filter.codes.clear();
        o.filter.codes.push(msg::END_REQUEST);
        o.filter.mode = 0;
        o.queue_module_tick_change(ATTACK_MODULE, false);
        o.queue_module_tick_change(WALK_MODULE, false);
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
}

/// Ready (`0x31D` -> `0x00519580`, re/healing 3.3): ready without a status test, first stand animation,
/// lock bit off.
fn set_ready(sim: &mut Sim, id: ObjectId) {
    let Some(h) = healing_mut(sim, id) else {
        return;
    };
    h.ready = true;
    let anim = list_at(&h.stand_anim, 0);
    set_animation(sim, id, anim);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
}

/// Initiate ending the trance (`0x21D` -> `0x00519410`, re/healing 3.4): nothing if the lock bit is set
/// (a stop during the setup animation is lost, C-248) or without `HEAL`. Otherwise no longer ready,
/// teardown animation `remove_anim[0]`, `0x41D` after its marker 1, filter empty in mode 0 (until
/// `0x41D` everything is locked, K-53), lock bit off. `m_SickPersonHID` stays until `0x41D`; the host
/// deletes the target effect.
fn end_request(sim: &mut Sim, id: ObjectId) {
    let Some(st) = sim.object(id).map(|o| o.status) else {
        return;
    };
    if st & status::ANIM_LOCKED != 0 || st & status::HEAL == 0 {
        return;
    }
    let Some(h) = healing_mut(sim, id) else {
        return;
    };
    h.ready = false;
    let anim = list_at(&h.remove_anim, 0);
    let delay = anim_label_time(sim, id, anim, 1);
    set_animation(sim, id, anim);
    post_delayed(sim, id, delay, msg::END);
    if let Some(o) = sim.object_mut(id) {
        o.filter.codes.clear();
        o.filter.mode = 0;
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
}

/// Trance off (`0x41D` -> `0x00519B70`, re/healing 3.5): `HEAL` off, filter empty in mode 1 (everything
/// allowed again), attack and walk back into the tick list, no single target any more, lock bit off.
/// The host removes the healing circle and target effects.
fn end(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::HEAL, false);
        o.filter.codes.clear();
        o.filter.mode = 1;
        o.queue_module_tick_change(ATTACK_MODULE, true);
        o.queue_module_tick_change(WALK_MODULE, true);
    }
    if let Some(h) = healing_mut(sim, id) {
        h.sick = ObjectId(0);
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
}

#[cfg(test)]
mod tests;
