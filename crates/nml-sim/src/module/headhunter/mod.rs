//! Module 34 `HEADHUNTER` (`CPropertyHeadHunter`, constructor `PropertyHeadHunter_Construct`
//! `0x00517B40`, VTable `0x006245A0`, `0x98` bytes, re/headhunter-robbing 1.2): the
//! headhunter that a headquarters hires. For each kind (4100 SPA, 4101 PRI, 4102 SID) there is
//! one entry worldwide in `GameWorld+0x4DC` ([`crate::state::SimState::headhunters`], 1.4).
//!
//! This module holds the state, `Init`, destructor, the takeover `0x122` and the query `0x222`
//! (command `0x2C` in [`crate::command::headhunter`]); the flow is in [`hunt`]: target search,
//! approach, way home and tick, melee kill and throw. Slot 4 (ReadParams), 5 (change of owner),
//! 10 ("busy") and 11 (abort -> `true`) are empty in the original or the base.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::fog::SEE;
use crate::fog::sources::{VisionSource, remove_vision_source, set_vision_source};
use crate::id::ObjectId;
use crate::message::Message;
use crate::module::ModuleKind;
use crate::module::events::update_events;
use crate::sim::Sim;
use crate::state::SimState;

pub mod hunt;
#[cfg(test)]
mod tests;

/// Module index of the headhunter.
pub const INDEX: usize = 0x22;
/// Group of the timers of the flow (`0x622` clears it, re/headhunter-robbing 2.1).
pub const GROUP: u8 = 0x80;
/// Type GUIDs of the three kinds (`0x1004`-`0x1006`): SPA, PRI, SID.
pub const GUIDS: [u32; 3] = [4100, 4101, 4102];
/// Price without a value set and after a success (constructor `+0x7C`/`+0x84`, C-315).
pub const DEFAULT_PRICE: i32 = 500;
/// Surcharge per hire (C-309).
pub const PRICE_STEP: i32 = 50;
/// Approach to a target unit: 1 tile (`0x00672928`, re/headhunter-robbing 1.4).
pub const NEAR: i32 = 4096;
/// Approach to a target building: 4 tiles.
pub const FAR: i32 = 4 * 4096;
/// Sight range of the sight source of the headhunter in quarter tiles (`GameWorld_SetVisionSource(-1, hid, 8,
/// ...)`, 1.2).
pub const VISION_SIZE: i32 = 8;

/// Messages to module 34 (query `0x00519210`, re/headhunter-robbing 2.1).
pub mod msg {
    /// `{target_player, target_guid, customer_slot}`: takeover/hiring (`0x00517FB0`, 3.2).
    pub const HIRE: u32 = 0x122;
    /// Query price, target player, target GUID, customer (inline).
    pub const PRICE: u32 = 0x222;
    /// "Think": target search or reset (`0x00518760`, 3.3).
    pub const THINK: u32 = 0x322;
    /// `{target}`: melee kill (`0x005190B0`, 3.4).
    pub const KILL: u32 = 0x422;
    /// Vanishing (`0x00517D90`, 3.6).
    pub const VANISH: u32 = 0x522;
    /// Clear group `0x80`, `0x322` after 1000 ms (`0x00517DB0`).
    pub const RESTART: u32 = 0x622;
    /// `{target}`: start the melee kill (`0x00517CE0`, 3.4).
    pub const START_KILL: u32 = 0x722;
    /// Walk success (`0x00518800`, 3.3).
    pub const WALK_OK: u32 = 0x822;
    /// Walk fault (`0x005186D0`, 3.3).
    pub const WALK_FAULT: u32 = 0x922;
    /// Throw dynamite at a unit (`0x00518C60`, 3.5).
    pub const THROW_UNIT: u32 = 0xA22;
    /// Throw dynamite at a building (`0x005188C0`, 3.5).
    pub const THROW_BUILDING: u32 = 0xB22;
    /// Throw animation finished (`0x00518030`).
    pub const THROW_END: u32 = 0xC22;
    /// Impact at a unit (`0x00518050`).
    pub const IMPACT_UNIT: u32 = 0xD22;
    /// Impact at a building (`0x00518190`).
    pub const IMPACT_BUILDING: u32 = 0xE22;
}

/// `CPropertyHeadHunter`, fields of the savegame in the order of the original (slot 16
/// `0x00519130`, re/headhunter-robbing 1.2, 5, 10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadHunter {
    /// `m_TargetGUID` (`+0x70`): target GUID (command field 3); -1 after success.
    pub target_guid: i32,
    /// `m_TargetHID` (`+0x74`): last found object (before the switch to the container).
    pub target_hid: ObjectId,
    /// `m_TargetPlayerID` (`+0x78`): target player (command field 2); -1 after success.
    pub target_player: i32,
    /// `m_CurReward` (`+0x7C`): current price (Init config 12, +50 per hire, 500 after success).
    pub cur_reward: i32,
    /// `m_CustomerHL` (`+0x80`): slot of the customer, -1 = none.
    pub customer: i32,
    /// `m_CustomerCost` (`+0x84`): what the customer has paid (refund).
    pub customer_cost: i32,
    /// `m_KillAnimNear` (`+0x88`, 350002).
    pub kill_anim_near: i32,
    /// `m_KillAnimFar` (`+0x8C`, 350003).
    pub kill_anim_far: i32,
    /// `m_LastTargetStatus` (`+0x90`): 0 search, 1 to the unit, 2 to the building (3-5 never set).
    pub state: i32,
    /// `m_LastTarget` (`+0x94`): current target (unit or building/container).
    pub last_target: ObjectId,
}

impl Default for HeadHunter {
    /// Constructor values (`0x00517B40`): IDs "none", price and customer price 500, no customer.
    fn default() -> Self {
        Self {
            target_guid: 0,
            target_hid: ObjectId::NONE,
            target_player: 0,
            cur_reward: DEFAULT_PRICE,
            customer: -1,
            customer_cost: DEFAULT_PRICE,
            kill_anim_near: 0,
            kill_anim_far: 0,
            state: 0,
            last_target: ObjectId::NONE,
        }
    }
}

/// Kind (0 SPA, 1 PRI, 2 SID) of the type GUID; other types have none (`Init`: "no valid
/// headhunter").
pub fn art_of(guid: u32) -> Option<usize> {
    GUIDS.iter().position(|&g| g == guid)
}

pub fn headhunter<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s HeadHunter> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::HeadHunter(h) => Some(h),
        _ => None,
    }
}

pub fn headhunter_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut HeadHunter> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::HeadHunter(h) => Some(h),
        _ => None,
    }
}

/// `PropertyHeadHunter_Init` (`0x00517C20`, slot 9, re/headhunter-robbing 1.2): price and
/// customer price = config 12 from the value set of the slot (`GameWorld_GetTypeValueSet`; without a set
/// the constructor value 500 stays), animations 350002/350003; type `0x1004`-`0x1006` -> entry
/// `GameWorld+0x4DC[kind]` (overwrites, `FUN_0047B580`), otherwise only "no valid headhunter"; then
/// for **every** type the object-bound sight source (size 8, all players, unlimited, "seen"
/// only, C-310). The original does not read 350001: the module flags stay 0.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, slot: u8) {
    let Some(guid) = sim.object(id).map(|o| o.guid) else {
        return;
    };
    let price = sim
        .rules
        .type_value_set(i32::from(slot), guid)
        .map(|set| set.get(&12).copied().unwrap_or(0));
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let (near, far) = (value(350_002), value(350_003));
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = 0;
    if let ModuleKind::HeadHunter(h) = &mut m.kind {
        if let Some(price) = price {
            h.cur_reward = price;
            h.customer_cost = price;
        }
        h.kill_anim_near = near;
        h.kill_anim_far = far;
    }
    match art_of(guid) {
        Some(art) => sim.state.headhunters[art] = Some(id),
        None => log::debug!("Object {}: no valid headhunter (type {guid})", id.0),
    }
    set_vision_source(
        &mut sim.state.fog,
        VisionSource {
            x: -1,
            z_or_hid: id.as_i32(),
            size: VISION_SIZE,
            player: -1,
            remaining_ms: -1,
            flags: SEE,
        },
    );
}

/// `PropertyHeadHunter_Destruct` (`0x00517B80`): clear the entry of the kind **unconditionally**
/// (`FUN_0047B5B0`, even if it already belongs to another headhunter, re/headhunter-robbing 1.2), remove
/// the sight source (`0x0047E0F0(-1, hid)`, deviation from the original).
pub fn destruct(sim: &mut Sim, id: ObjectId) {
    if let Some(art) = sim.object(id).and_then(|o| art_of(o.guid)) {
        sim.state.headhunters[art] = None;
    }
    remove_vision_source(&mut sim.state.fog, -1, id.as_i32());
}

/// `FUN_00477100(art)`: the registered headhunter of the kind (`GameWorld+0x4DC[art]`) via
/// `GetObjectById(hid, false)` - also a dead one; an entry without an object (stale ID)
/// counts as "no headhunter".
pub fn current(state: &SimState, art: usize) -> Option<ObjectId> {
    state
        .headhunters
        .get(art)
        .copied()
        .flatten()
        .filter(|&id| state.object(id, false).is_some())
}

/// `Query` (slot 7, `0x00519210`, re/headhunter-robbing 2.1): `0x122` takeover ([`on_hire`]),
/// `0x222` query in the arguments; `0x322`, `0x522`, `0x622`, `0x822` and `0x922` in the flow
/// ([`hunt`]); melee kill `0x422`/`0x722` and throw `0xA22`-`0xE22`.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::HIRE => on_hire(sim, id, m),
        msg::PRICE => {
            let Some(h) = headhunter(sim, id) else {
                return -1;
            };
            let answer = [h.cur_reward, h.target_player, h.target_guid, h.customer];
            for (i, v) in answer.into_iter().enumerate() {
                m.set_arg(i, v);
            }
            0
        }
        msg::THINK => hunt::think(sim, id),
        msg::VANISH => hunt::vanish(sim, id),
        msg::RESTART => hunt::restart(sim, id),
        msg::WALK_OK => hunt::on_walk_success(sim, id),
        msg::WALK_FAULT => hunt::on_walk_fault(sim, id),
        msg::KILL => hunt::on_kill(sim, id, ObjectId::from_i32(m.arg(0))),
        msg::START_KILL => {
            hunt::start_near_kill(sim, id, ObjectId::from_i32(m.arg(0)));
            0
        }
        msg::THROW_UNIT => hunt::on_throw(sim, id, msg::IMPACT_UNIT),
        msg::THROW_BUILDING => hunt::on_throw(sim, id, msg::IMPACT_BUILDING),
        msg::THROW_END => hunt::on_throw_end(sim, id),
        msg::IMPACT_UNIT => hunt::on_impact(sim, id, false),
        msg::IMPACT_BUILDING => hunt::on_impact(sim, id, true),
        _ => -1,
    }
}

/// `0x122 {target_player, target_guid, customer}` (`0x00517FB0`, re/headhunter-robbing 3.2): first the
/// refund of the paid price to the previous customer (also to the same player, C-309;
/// no player -> nothing), then customer, paid price = previous price, target player, price +50,
/// target GUID, state 0; clear group `0x80` (C-318) and `0x322` after 1000 ms into the same group.
/// Returns 0.
fn on_hire(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let Some((customer, cost)) = headhunter(sim, id).map(|h| (h.customer, h.customer_cost)) else {
        return -1;
    };
    if let Some(p) = usize::try_from(customer)
        .ok()
        .and_then(|c| sim.state.players.get_mut(c))
    {
        p.add_resource(0, cost);
    }
    let Some(module) = sim.module_mut(id, INDEX) else {
        return -1;
    };
    if let ModuleKind::HeadHunter(h) = &mut module.kind {
        h.customer = m.arg(2);
        h.customer_cost = h.cur_reward;
        h.target_player = m.arg(0);
        h.cur_reward = h.cur_reward.wrapping_add(PRICE_STEP);
        h.target_guid = m.arg(1);
        h.state = 0;
    }
    module.base.clear_event_group(GROUP);
    module
        .base
        .post_delayed_message(1000, Message::new(msg::THINK, &[]), false, GROUP);
    0
}

/// Tick (slot 13, `0x00518730`, re/headhunter-robbing 3.10): `Module_UpdateEvents`, then the way home
/// without movement, lock and timer ([`hunt::after_update_events`], C-324).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    hunt::after_update_events(sim, id);
}
