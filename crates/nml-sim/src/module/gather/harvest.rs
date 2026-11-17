//! Module 12 `CPropertyHarvesting` (vtable `0x00624368`, re/gathering): harvesting.
//!
//! Flow: `0x10C {farm}` -> **ask** for one of the farm's five work slots (`0x603`, round-robin
//! via the farm's pointer, without claiming it) and walk to its node `farmerNN` (FINDTO with
//! coordinates); without a free slot simply walk to the farm and ask once more there - free -> state 6
//! (next tick to the farm again), otherwise **abort** (no waiting). At the node the slot is
//! claimed (`0x803`), the field animation is set and the **first** unit is taken only after a wait
//! (`0xA03`). Full -> free from the obstacle (walk `0x900`), then on to the
//! food store; the slot stays claimed during the delivery trips, only the abort releases it.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::{edge_distance_to_object, find_nearest_matching};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, post_message, send, send_mut};
use crate::module::building;
use crate::module::events::update_events;
use crate::module::stand::{list_modulo, parse_int_list};
use crate::module::walk::find_to_msg;
use crate::module::{ModuleKind, abort as abort_module};
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;

use super::mining::clear_quartered_at_source;
use super::{
    Gather, HARVESTING, gather, gather_mut, load, own_load, position, set_animation,
    set_carried_load, status,
};

/// Module index.
pub const INDEX: usize = 12;
/// Food rate (attribute 31, units per minute; maximum with upgrades).
pub const RATE: u32 = 31;
/// Object state bit "harvesting" (`HARVESTING`, `+0x90`).
pub const HARVESTING_BIT: u32 = 0x400;
/// Sight (attribute 18, quarter tiles): search radius of `FindFarmOrStart`.
pub const SIGHT: u32 = 18;
/// Object state bit "dying": the farm accepts no more work.
const DYING: u32 = 0x4_0000;

/// Messages of the harvest.
pub mod msg {
    /// `{farm}`: harvest (`PMSG_HARVESTING`).
    pub const START: u32 = 0x10C;
    /// `{sel}` -> stand animation with food.
    pub const GET_STAND_ANIM: u32 = 0x20C;
    /// `{sel}` -> walk animation with food.
    pub const GET_WALK_ANIM: u32 = 0x30C;
    /// Walk: destination reached.
    pub const DEST_REACHED: u32 = 0x40C;
    /// Walk: aborted.
    pub const ABORT: u32 = 0x50C;
    /// `{store}`: deliver (success of the freeing, `{-1}`).
    pub const DELIVER: u32 = 0x60C;
    /// Timer: harvest step.
    pub const HARVEST_STEP: u32 = 0x70C;
    /// Timer: delivery finished.
    pub const DELIVERED: u32 = 0x80C;
    /// `{-1, 0}` -> `{distance, farm}` or `{d, farm}` -> start work (`0x77F`).
    pub const FIND_FARM: u32 = 0x90C;
    /// `{farm}`: set the harvest target (no sender known).
    pub const SET_FARM: u32 = 0xA0C;
}

/// Work state (`m_Status`) of the harvest.
pub mod state {
    /// Harvesting at the node.
    pub const HARVESTING: i32 = 100;
}

/// State of `CPropertyHarvesting` (`PropertyHarvesting_Construct` `0x00516E90`), saved
/// like `PropertyHarvesting_SerializeFields` (`0x00517A90`) after the base fields. `Init` does not
/// read the flags (130004) (`m_MatchInt` stays 0).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harvest {
    pub gather: Gather,
    /// `m_HarvestWalkAnim` (`+0x9C`, 130001): walk animations with food.
    pub walk_anim: Vec<i32>,
    /// `m_HarvestStandAnim` (`+0xA8`, 130002): stand animations with food.
    pub stand_anim: Vec<i32>,
    /// `m_HarvestingPos` (`+0xB4`): farm slot 0...4, -1 = none (constructor 0, start -1).
    pub harvesting_pos: i32,
    /// `m_HarvestAnim` (`+0xB8`, 130006): field animation (set only on arrival).
    pub harvest_anim: i32,
    /// `m_SearchRadius` (`+0xBC`): never written (`Init` does not read 130005).
    pub search_radius: i32,
    /// `m_AnimCount` (`+0xC0`): empty steps since the last full load.
    pub anim_count: i32,
    /// `m_lastTimeLeak` (`+0xC4`): time remainder, 0 on every arrival.
    pub last_time_leak: i32,
}

pub(crate) fn harvest<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Harvest> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Harvest(h) => Some(h),
        _ => None,
    }
}

pub(crate) fn harvest_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Harvest> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Harvest(h) => Some(h),
        _ => None,
    }
}

fn status_of(sim: &Sim, id: ObjectId) -> i32 {
    super::state(sim, id, INDEX)
}

fn set_status_of(sim: &mut Sim, id: ObjectId, s: i32) {
    super::set_state(sim, id, INDEX, s);
}

fn resource(sim: &Sim, id: ObjectId) -> ObjectId {
    gather(sim, id, INDEX).map_or(ObjectId::NONE, |g| g.resource_hid)
}

fn set_resource(sim: &mut Sim, id: ObjectId, r: ObjectId) {
    if let Some(g) = gather_mut(sim, id, INDEX) {
        g.resource_hid = r;
    }
}

fn pos_of(sim: &Sim, id: ObjectId) -> i32 {
    harvest(sim, id).map_or(-1, |h| h.harvesting_pos)
}

fn set_pos(sim: &mut Sim, id: ObjectId, p: i32) {
    if let Some(h) = harvest_mut(sim, id) {
        h.harvesting_pos = p;
    }
}

fn set_status_flags(sim: &mut Sim, id: ObjectId, bits: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(bits, on);
    }
}

/// `PropertyHarvesting_Init` (`0x00517930`): stand animations 130002, walk animations 130001
/// (lists), field animation 130006, delivery animation 130003. The original does not read 130004 (flags),
/// 130005 and 130007.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let (stand, walk) = (text(130002), text(130001));
    let (anim, deliver) = (value(130006), value(130003));
    if let Some(h) = harvest_mut(sim, id) {
        h.stand_anim = parse_int_list(&stand, &h.stand_anim);
        h.walk_anim = parse_int_list(&walk, &h.walk_anim);
        h.harvest_anim = anim;
        h.gather.deliver_anim = deliver;
    }
}

/// `PropertyHarvesting_Query` (`0x005179A0`, slot 7); all other codes go through the
/// base query to [`on_message`].
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        // `IntList_GetModulo`: empty list (division by 0 in the original) -> -1 (A7).
        msg::GET_STAND_ANIM => {
            harvest(sim, id).map_or(-1, |h| list_modulo(&h.stand_anim, m.arg(0)))
        }
        msg::GET_WALK_ANIM => harvest(sim, id).map_or(-1, |h| list_modulo(&h.walk_anim, m.arg(0))),
        msg::HARVEST_STEP => {
            harvest_step(sim, id);
            // Quirk: afterwards the base query additionally with the same message
            // (no effect, -1).
            super::base_query(sim, id, INDEX, m)
        }
        msg::FIND_FARM => find_farm_or_start(sim, id, m),
        msg::SET_FARM => {
            set_resource(sim, id, ObjectId::from_i32(m.arg(0)));
            1
        }
        _ => super::base_query(sim, id, INDEX, m),
    }
}

/// `PropertyHarvesting_OnMessage` (`0x00517240`, slot 12): `0x10C` starts, everything else goes to the
/// base (`0x40C` destination reached, `0x50C` abort, `0x60C` deliver, `0x80C` delivery finished).
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if env.msg.code == msg::START {
        on_start_harvesting(sim, id, env);
        return true;
    }
    super::on_command(sim, id, INDEX, env)
}

/// `PropertyHarvesting_Tick` (`0x00517110`, slot 13): except while harvesting (state 100) the
/// base tick (state 6 -> `GotoFarm`), then the timers.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    if status_of(sim, id) != state::HARVESTING {
        super::tick_base(sim, id, INDEX);
    }
    update_events(sim, id, INDEX, dt);
}

/// `PropertyHarvesting_Abort` (`0x00517090`, slot 11): farm present -> release the slot
/// (`0x903 {self}`), `m_HarvestingPos = -1`, `m_ResourceHID = -1`; if the farm is missing, both
/// stay. Then the base abort.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    let farm = resource(sim, id);
    if sim.object(farm).is_some() {
        send(
            sim,
            farm,
            &Message::new(building::msg::FARM_RELEASE_SLOT, &[id.as_i32()]),
        );
        log::debug!("Farmer {id:?}: removed from the farm");
        set_pos(sim, id, -1);
        set_resource(sim, id, ObjectId::NONE);
    }
    super::stop(sim, id, INDEX)
}

/// `PropertyHarvesting_OnOwnerChanged` (`0x005171B0`, slot 5): farm present -> `0x3F03 {old,
/// new}` to the farm (the slot entry follows the new ID).
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId, old: ObjectId) {
    let farm = resource(sim, id);
    if sim.object(farm).is_some() {
        send(
            sim,
            farm,
            &Message::new(building::msg::REPLACE_HANDLE, &[old.as_i32(), id.as_i32()]),
        );
    }
}

/// `PropertyHarvesting_OnDestReached` (`0x005176C0`, slot 18): except in state 1 the base
/// (state 2 -> deliver). Arrived at the farm **without** a slot: ask once more - free ->
/// state 6 (next tick `GotoFarm`), otherwise abort. At the node -> [`harvest_step`].
pub fn on_dest_reached(sim: &mut Sim, id: ObjectId, _env: &Envelope) -> bool {
    if status_of(sim, id) != status::TO_SOURCE {
        return super::base_on_dest_reached(sim, id, INDEX);
    }
    if pos_of(sim, id) == -1 {
        let farm = resource(sim, id);
        if claim_farm_slot(sim, id, farm) {
            set_status_of(sim, id, status::SEARCH);
        } else {
            abort_module(sim, id, INDEX);
        }
        return true;
    }
    harvest_step(sim, id);
    true
}

/// `PropertyHarvesting_OnStartHarvesting` (`0x00516FB0`, command `0x10C {farm}`): take over the
/// feedback messages, bit `0x400`, `m_ResourceHID`, `m_HarvestingPos = -1`, then `GotoFarm`.
fn on_start_harvesting(sim: &mut Sim, id: ObjectId, env: &Envelope) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    set_status_flags(sim, id, HARVESTING_BIT, true);
    set_resource(sim, id, ObjectId::from_i32(env.msg.arg(0)));
    set_pos(sim, id, -1);
    goto_farm(sim, id);
}

/// `PropertyHarvesting_ClaimFarmSlot` (`0x00517010`, name misleading: only a **query**): without a farm
/// `m_HarvestingPos = -1`; otherwise `0x603 {self, m_HarvestingPos}` (-1 = round-robin) and remember the
/// result. Returns "slot found".
fn claim_farm_slot(sim: &mut Sim, id: ObjectId, farm: ObjectId) -> bool {
    if sim.object(farm).is_none() {
        set_pos(sim, id, -1);
        return false;
    }
    let cur = pos_of(sim, id);
    let p = send(
        sim,
        farm,
        &Message::new(building::msg::FARM_QUERY_SLOT, &[id.as_i32(), cur]),
    );
    set_pos(sim, id, p);
    p != -1
}

/// `PropertyHarvesting_GotoFarm` (`0x00517290`, slot 19): state 1, bit `0x400`; without a farm
/// `m_ResourceHID = -1` and abort. With a queried slot to the node (`0x503 {slot}` -> position;
/// FINDTO `{x, z, -1, no direction, -1, 1}`), otherwise simply to the farm (FINDTO `{farm, -1, 0}`);
/// feedback messages `0x40C`/`0x50C`.
pub fn goto_farm(sim: &mut Sim, id: ObjectId) {
    set_status_of(sim, id, status::TO_SOURCE);
    set_status_flags(sim, id, HARVESTING_BIT, true);
    let farm = resource(sim, id);
    if sim.object(farm).is_none() {
        set_resource(sim, id, ObjectId::NONE);
        abort_module(sim, id, INDEX);
        return;
    }
    let msg = if claim_farm_slot(sim, id, farm) {
        let mut q = Message::new(building::msg::FARMER_POS, &[pos_of(sim, id), -1, -1, -1]);
        send_mut(sim, farm, &mut q);
        find_to_msg(q.arg(1), q.arg(3), -1, NO_DIRECTION, -1, false, 1)
    } else {
        Message::new(0x200, &[farm.as_i32(), -1, 0])
    };
    let env = Envelope {
        msg,
        success: Some(Message::new(HARVESTING.success, &[])),
        fault: Some(Message::new(HARVESTING.fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyHarvesting_HarvestStep` (`0x00517400`, arrival and `0x70C`).
///
/// Arrival (state != 100): claim the slot (`0x803`); if another unit holds it (and the farm is not
/// the own container), `m_HarvestingPos = -1` and ask again -> back to the farm, otherwise
/// abort. Claimed: field animation, state 100, `m_lastTimeLeak = 0` and the first step after
/// a wait - **no** withdrawal on arrival.
///
/// Step: farm gone or dying -> abort. Slot free -> `0xA03 {1}`; food -> load,
/// next step; nothing -> `m_AnimCount + 1`, up to `capacity` continue (the time marker of the
/// field animation is read and discarded), at the `capacity + 1`-th a unit is given for free
/// (quirk) and "full". Full: clear bit `0x100` if the farm is the container,
/// `m_AnimCount = 0`, exception object 0, walk `0x900` (free from the obstacle) with success
/// `0x60C {-1}` (-> to the food store) and fault `0x50C`, afterwards exception object -1. The slot
/// stays claimed.
fn harvest_step(sim: &mut Sim, id: ObjectId) {
    let farm = resource(sim, id);
    if status_of(sim, id) != state::HARVESTING {
        if sim.object(farm).is_none() {
            abort_module(sim, id, INDEX);
            return;
        }
        let p = pos_of(sim, id);
        let claim = Message::new(building::msg::FARM_CLAIM_SLOT, &[id.as_i32(), p]);
        let inside = crate::collision::quartering_container(sim, id) == Some(farm);
        if send(sim, farm, &claim) == -1 && !inside {
            set_pos(sim, id, -1);
            if claim_farm_slot(sim, id, farm) {
                goto_farm(sim, id);
            } else {
                abort_module(sim, id, INDEX);
            }
            return;
        }
        let anim = harvest(sim, id).map_or(0, |h| h.harvest_anim);
        set_animation(sim, id, anim);
        set_status_of(sim, id, state::HARVESTING);
        if let Some(h) = harvest_mut(sim, id) {
            h.last_time_leak = 0;
        }
        schedule_step(sim, id);
        return;
    }
    let Some(f) = sim.object(farm) else {
        abort_module(sim, id, INDEX);
        return;
    };
    if f.status & DYING != 0 {
        abort_module(sim, id, INDEX);
        return;
    }
    let amount = own_load(sim, id, load::FOOD);
    let cap = super::capacity(sim, id);
    if cap.wrapping_sub(amount) > 0 {
        let got = send(sim, farm, &Message::new(building::msg::GET_FOOD, &[1]));
        if got > 0 {
            set_carried_load(sim, id, amount.wrapping_add(got), load::FOOD);
            schedule_step(sim, id);
            return;
        }
        let count = harvest_mut(sim, id).map_or(0, |h| {
            h.anim_count = h.anim_count.wrapping_add(1);
            h.anim_count
        });
        if count <= cap {
            // `GetAnimLabelTime(m_HarvestAnim, 1)`: result discarded.
            schedule_step(sim, id);
            return;
        }
        set_carried_load(sim, id, amount.wrapping_add(1), load::FOOD);
    }
    // Full (`0x005175D2`).
    clear_quartered_at_source(sim, id, farm);
    if let Some(h) = harvest_mut(sim, id) {
        h.anim_count = 0;
    }
    if let Some(o) = sim.object_mut(id) {
        o.exempt.object = ObjectId(0);
    }
    let env = Envelope {
        msg: Message::new(0x900, &[]),
        success: Some(Message::new(msg::DELIVER, &[-1])),
        fault: Some(Message::new(HARVESTING.fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
    if let Some(o) = sim.object_mut(id) {
        o.set_collision_exempt_object(ObjectId::NONE);
    }
}

/// Schedule the next harvest step (`PropertyHarvesting_ScheduleNextUnit` `0x00517140`, rate 31).
fn schedule_step(sim: &mut Sim, id: ObjectId) {
    let leak = harvest(sim, id).map_or(0, |h| h.last_time_leak);
    let leak = super::schedule_next_unit(
        sim,
        id,
        INDEX,
        RATE,
        leak,
        Message::new(msg::HARVEST_STEP, &[]),
    );
    if let Some(h) = harvest_mut(sim, id) {
        h.last_time_leak = leak;
    }
}

/// `PropertyHarvesting_FindFarmOrStart` (`0x00517730`, query `0x90C` from
/// `GameObject_ResumeNearestWork` `0x77F`). Second call (`args[0] != -1`): post `0x10C {args[1]}`
/// with abort. First: nearest own farm (module 3 with `FARM`) within `sight / 4` tiles
/// (attribute 18, division toward 0; center distance) with a
/// slot for itself (`0x603 {self, -1}` - **quirk:** the query moves the round-robin pointer
/// of the farm) -> `args = {edge distance farm->farmer, farm}`, 1; otherwise 0.
fn find_farm_or_start(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    if m.arg(0) != -1 {
        return i32::from(post_message(
            sim,
            id,
            Message::new(msg::START, &[m.arg(1)]),
            true,
        ));
    }
    let f = MatchFilter {
        mode: 0x112,
        match_bits: building::flags::FARM,
        ..MatchFilter::constructed()
    };
    let radius = (sim.attribute(id, SIGHT) / 4).wrapping_shl(12);
    let (pos, owner) = (position(sim, id), i32::from(id.slot()));
    let Some(farm) = find_nearest_matching(sim, pos, &f, radius, owner, -1) else {
        return 0;
    };
    let ask = Message::new(building::msg::FARM_QUERY_SLOT, &[id.as_i32(), -1]);
    if send(sim, farm, &ask) == -1 {
        return 0;
    }
    let d = edge_distance_to_object(sim, farm, id);
    m.set_arg(0, d);
    m.set_arg(1, farm.as_i32());
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Posted;
    use crate::module::gather::tests::{
        COLONIST, FARM, FOOD_STORE, GOLD_STORE, LUMBERYARD, MINE, SOIL, TREE, econ_rules, spawn,
        spawn_deposit, state as new_state,
    };
    use crate::module::resource;
    use crate::module::tick as module_tick;

    fn load_of(sim: &Sim, id: ObjectId) -> (i32, i32) {
        sim.object(id)
            .map_or((0, 0), |o| (o.load_amount, o.load_kind))
    }

    fn slots(sim: &Sim, farm: ObjectId) -> [ObjectId; 5] {
        building::building(sim, farm).unwrap().registered_handles
    }

    fn arrive(sim: &mut Sim, unit: ObjectId) {
        assert_eq!(send(sim, unit, &Message::new(msg::DEST_REACHED, &[])), 1);
    }

    // QUIRK W-23: kept
    #[test]
    fn a_farmer_claims_his_slot_on_arrival_and_takes_the_first_unit_after_one_wait() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 20));
        let farm = spawn(&mut sim, FARM, 0, (10, 10));
        let soil = spawn_deposit(&mut sim, SOIL, 10000, (10, 10));
        spawn(&mut sim, FOOD_STORE, 0, (30, 20));
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::START, &[farm.as_i32()])),
            1
        );
        // Slot 0 queried (not claimed), path to the node `farmer01` with coordinates.
        assert_eq!(pos_of(&sim, unit), 0);
        assert_eq!(slots(&sim, farm), [ObjectId::NONE; 5]);
        let mut node = Message::new(building::msg::FARMER_POS, &[0, -1, -1, -1]);
        send_mut(&mut sim, farm, &mut node);
        let w = crate::module::walk::walk(&sim, unit).unwrap();
        assert_eq!(
            w.find_to,
            Some(Message::new(
                0x200,
                &[node.arg(1), node.arg(3), -1, NO_DIRECTION, -1, 1]
            ))
        );
        assert_eq!(
            sim.object(unit).unwrap().status & HARVESTING_BIT,
            HARVESTING_BIT
        );
        // Arrival in tick T0: claimed, field animation, no withdrawal.
        arrive(&mut sim, unit);
        module_tick(&mut sim, unit, INDEX, 100);
        assert_eq!(slots(&sim, farm)[0], unit);
        assert!(building::building(&sim, farm).unwrap().growed);
        assert_eq!(status_of(&sim, unit), state::HARVESTING);
        assert_eq!(sim.object(unit).unwrap().anim.wanted, 8);
        assert_eq!(load_of(&sim, unit).0, 0);
        let mut units = Vec::new();
        let mut done = None;
        for t in 1..=400 {
            let before = load_of(&sim, unit).0;
            module_tick(&mut sim, unit, INDEX, 100);
            if load_of(&sim, unit).0 != before {
                units.push(t);
            }
            if status_of(&sim, unit) != state::HARVESTING {
                done = Some(t);
                break;
            }
        }
        assert_eq!(units, (1..=20).map(|k| 11 * k).collect::<Vec<_>>());
        assert_eq!(done, Some(231));
        // Full: freed (free -> success `0x60C {-1}`), to the food store; the slot stays.
        let g = gather(&sim, unit, INDEX).unwrap();
        assert_eq!(g.status, status::TO_DELIVERY);
        assert_eq!(slots(&sim, farm)[0], unit);
        assert_eq!(pos_of(&sim, unit), 0);
        assert_eq!(load_of(&sim, unit), (20, load::FOOD));
        assert_eq!(sim.attribute(soil, resource::FOOD_AMOUNT), 9980);
        let o = sim.object(unit).unwrap();
        assert_eq!((o.exempt.object, o.exempt.mode), (ObjectId::NONE, 0));
        // The abort releases the slot and forgets farm and slot.
        assert!(crate::module::abort(&mut sim, unit, INDEX));
        assert_eq!(slots(&sim, farm), [ObjectId::NONE; 5]);
        assert_eq!(
            (pos_of(&sim, unit), resource(&sim, unit)),
            (-1, ObjectId::NONE)
        );
    }

    // QUIRK W-52: kept
    #[test]
    fn the_sixth_farmer_at_a_full_farm_gives_up() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let farm = spawn(&mut sim, FARM, 0, (20, 10));
        let units: Vec<ObjectId> = (0..6)
            .map(|k| spawn(&mut sim, COLONIST, 0, (14 + 2 * k, 20)))
            .collect();
        for &u in &units {
            send(&mut sim, u, &Message::new(msg::START, &[farm.as_i32()]));
        }
        // Only queried: round-robin 0...4, then 0 again.
        let asked: Vec<i32> = units.iter().map(|&u| pos_of(&sim, u)).collect();
        assert_eq!(asked, [0, 1, 2, 3, 4, 0]);
        for &u in &units[..5] {
            arrive(&mut sim, u);
            assert_eq!(status_of(&sim, u), state::HARVESTING);
        }
        assert_eq!(slots(&sim, farm)[..], units[..5]);
        // The sixth finds slot 0 claimed, asks again (all full) and aborts.
        arrive(&mut sim, units[5]);
        assert_eq!(status_of(&sim, units[5]), status::IDLE);
        assert_eq!(
            (pos_of(&sim, units[5]), resource(&sim, units[5])),
            (-1, ObjectId::NONE)
        );
        assert_eq!(sim.object(units[5]).unwrap().status & HARVESTING_BIT, 0);
        assert_eq!(slots(&sim, farm)[..], units[..5]);
    }

    // QUIRK W-52: kept
    #[test]
    fn without_a_slot_the_farmer_walks_to_the_farm_and_asks_once_more() {
        for freed in [false, true] {
            let rules = econ_rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let farm = spawn(&mut sim, FARM, 0, (20, 10));
            for k in 0..5 {
                send(
                    &mut sim,
                    farm,
                    &Message::new(building::msg::FARM_CLAIM_SLOT, &[0x7_0001 + k, k]),
                );
            }
            let unit = spawn(&mut sim, COLONIST, 0, (20, 20));
            send(&mut sim, unit, &Message::new(msg::START, &[farm.as_i32()]));
            assert_eq!(pos_of(&sim, unit), -1);
            let w = crate::module::walk::walk(&sim, unit).unwrap();
            assert_eq!(
                w.find_to,
                Some(Message::new(0x200, &[farm.as_i32(), -1, 0]))
            );
            if freed {
                send(
                    &mut sim,
                    farm,
                    &Message::new(building::msg::FARM_RELEASE_SLOT, &[0x7_0003]),
                );
            }
            arrive(&mut sim, unit);
            if freed {
                // Slot 2 free: state 6, the next tick walks to the farm again (to the node).
                assert_eq!(
                    (status_of(&sim, unit), pos_of(&sim, unit)),
                    (status::SEARCH, 2)
                );
                module_tick(&mut sim, unit, INDEX, 100);
                assert_eq!(status_of(&sim, unit), status::TO_SOURCE);
                let w = crate::module::walk::walk(&sim, unit).unwrap();
                assert_eq!(w.find_to.as_ref().map(|m| m.args.len()), Some(6));
            } else {
                assert_eq!(status_of(&sim, unit), status::IDLE);
                assert_eq!(resource(&sim, unit), ObjectId::NONE);
            }
        }
    }

    // QUIRK W-56: kept
    #[test]
    fn harvesters_answer_the_animation_queries_and_the_step_returns_minus_one() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::GET_STAND_ANIM, &[4])),
            10
        );
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::GET_WALK_ANIM, &[4])),
            9
        );
        // The stand idle asks with food `0x20C` (answer from 130002).
        assert_eq!(crate::module::stand::select_idle_anim(&mut sim, unit, 3), 0);
        set_carried_load(&mut sim, unit, 3, load::FOOD);
        assert_eq!(
            crate::module::stand::select_idle_anim(&mut sim, unit, 3),
            10
        );
        let h = harvest(&sim, unit).unwrap();
        assert_eq!(
            (
                h.harvest_anim,
                h.gather.deliver_anim,
                h.harvesting_pos,
                h.search_radius
            ),
            (8, -1, 0, 0)
        );
        // Without a farm: the step aborts, the base query afterwards reports -1.
        set_status_of(&mut sim, unit, state::HARVESTING);
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::HARVEST_STEP, &[])),
            -1
        );
        assert_eq!(status_of(&sim, unit), status::IDLE);
        // `0xA0C` only sets the target.
        assert_eq!(send(&mut sim, unit, &Message::new(msg::SET_FARM, &[77])), 1);
        assert_eq!(resource(&sim, unit), ObjectId(77));
    }

    #[test]
    fn an_owner_change_rewrites_the_farm_slot() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let farm = spawn(&mut sim, FARM, 1, (20, 10));
        let unit = spawn(&mut sim, COLONIST, 0, (20, 20));
        send(&mut sim, unit, &Message::new(msg::START, &[farm.as_i32()]));
        arrive(&mut sim, unit);
        assert_eq!(slots(&sim, farm)[0], unit);
        let new = crate::world::change_owner(&mut sim, unit, 1).expect("owner changed");
        assert_eq!(slots(&sim, farm)[0], new);
    }

    // QUIRK W-62: kept
    #[test]
    fn resuming_work_picks_the_nearest_job_and_prefers_wood_then_harvest_then_gold() {
        // Jobs per case: (tree, farm, mine) as a tile or none -> expected start command.
        type Case = (
            Option<(i32, i32)>,
            Option<(i32, i32)>,
            Option<(i32, i32)>,
            u32,
        );
        let cases: [Case; 5] = [
            (None, Some((16, 20)), None, msg::START),
            (
                None,
                Some((16, 20)),
                Some((22, 20)),
                super::super::mining::msg::START,
            ),
            (
                None,
                Some((26, 20)),
                Some((22, 20)),
                super::super::mining::msg::START,
            ),
            (None, Some((18, 20)), Some((22, 20)), msg::START),
            (Some((21, 20)), Some((16, 20)), Some((23, 20)), 0x108),
        ];
        for (tree, farm, mine, want) in cases {
            let rules = econ_rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (20, 20));
            spawn(&mut sim, GOLD_STORE, 0, (50, 50));
            spawn(&mut sim, LUMBERYARD, 0, (50, 40));
            let t = tree.map(|p| spawn(&mut sim, TREE, 9, p));
            let f = farm.map(|p| spawn(&mut sim, FARM, 0, p));
            let m = mine.map(|p| spawn(&mut sim, MINE, 0, p));
            if farm == Some((18, 20)) {
                // Tie: farm and mine lie mirror-symmetrically two tiles away.
                let ask = |sim: &mut Sim, code: u32| {
                    let mut q = Message::new(code, &[-1, 0]);
                    send_mut(sim, unit, &mut q);
                    q.arg(0)
                };
                let (h, g) = (ask(&mut sim, msg::FIND_FARM), ask(&mut sim, 0x907));
                assert!(h > 0 && h == g, "{h} {g}");
            }
            send(&mut sim, unit, &Message::new(0x77F, &[]));
            let fifo: Vec<Posted> = sim.object(unit).unwrap().fifo.iter().cloned().collect();
            let target = match want {
                msg::START => f,
                0x108 => t,
                _ => m,
            }
            .unwrap();
            assert_eq!(
                fifo,
                vec![Posted::Message {
                    msg: Message::new(want, &[target.as_i32()]),
                    abort: true
                }],
                "{tree:?} {farm:?} {mine:?}"
            );
        }
    }

    // QUIRK W-27, W-50: kept
    #[test]
    fn find_farm_searches_a_quarter_of_the_sight_and_moves_the_cursor() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (20, 20));
        // Sight 32 -> 8 tiles: the farm 9 tiles away does not count.
        let far = spawn(&mut sim, FARM, 0, (29, 20));
        let mut q = Message::new(msg::FIND_FARM, &[-1, 0]);
        assert_eq!(send_mut(&mut sim, unit, &mut q), 0);
        assert_eq!(building::building(&sim, far).unwrap().request_pos, -1);
        let near = spawn(&mut sim, FARM, 0, (28, 20));
        let mut q = Message::new(msg::FIND_FARM, &[-1, 0]);
        assert_eq!(send_mut(&mut sim, unit, &mut q), 1);
        let d = edge_distance_to_object(&mut sim, near, unit);
        assert_eq!(q.args, [d, near.as_i32()]);
        assert_eq!(building::building(&sim, near).unwrap().request_pos, 0);
        let mut q = Message::new(msg::FIND_FARM, &[-1, 0]);
        send_mut(&mut sim, unit, &mut q);
        assert_eq!(building::building(&sim, near).unwrap().request_pos, 1);
    }
}
