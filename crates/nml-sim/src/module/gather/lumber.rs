//! Module 8 `CPropertyLumbering` (vtable `0x00624AC8`, re/gathering): lumbering.
//!
//! Flow: `0x108 {tree}` only records the tree as the **search center** (`m_TargetTreePos`);
//! `GotoTree` always searches anew for the nearest searchable tree of the same path region around this
//! point (if the lumberjack stands on a blocked tile, the region is determined via an own lumberyard),
//! registers at the tree and walks there by FINDTOTREE. From the arrival on, one unit of wood comes per
//! chop interval (`60000 / rate` ms with leak, a timer fires after `d/100 + 1` object ticks); "full" is
//! recognized only in the following step. Then it goes to the nearest finished own lumberyard (first
//! one tile out from the tree), after the delivery back.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::{
    edge_distance_to_object, edge_distance_to_point, find_nearest_matching, query_radius,
};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, post_message, send, send_mut};
use crate::module::events::update_events;
use crate::module::stand::{list_modulo, parse_int_list};
use crate::module::tree;
use crate::module::walk::find_to_msg;
use crate::module::{ModuleKind, abort as abort_module};
use crate::motion::NO_DIRECTION;
use crate::path::BLOCKED;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::region_id_at;

use super::{
    Gather, LUMBERING, clear_events, gather, gather_mut, load, own_load, position, post_delayed,
    set_animation, set_carried_load, status, walk_layer,
};

/// Module index.
pub const INDEX: usize = 8;
/// Wood rate (attribute 32, units per minute; maximum with upgrades).
pub const RATE: u32 = 32;
/// Object state bit "chops wood" (`LUMBERING`, `+0x90`).
pub const LUMBERING_BIT: u32 = 0x80;
/// Object state bit of the collision exemption zone.
pub const EXEMPT_BIT: u32 = 0x1000;
/// Chop sound (`SoundManager_PlayObjectSound3D(0x69, 0x11, ...)`, number in the sound list of the type).
pub const SFX_CHOP: i32 = 0x69;
/// Margin of the exemption zone around the tree beyond the hard radius.
const EXEMPT_MARGIN: i32 = 0xB51;
/// One tile in fixed point (`[0x006729A8]`).
const TILE: i32 = 0x1000;

/// Messages of lumbering (names from the table `0x0061AD74...`, from `0x708` on without names).
pub mod msg {
    /// `{tree}`: fell wood (`PMSG_LUMBERING`).
    pub const START: u32 = 0x108;
    /// `{sel}` -> stand animation with wood (`PMSG_LUMBERING_GET_STAND_ANIM`).
    pub const GET_STAND_ANIM: u32 = 0x208;
    /// -> walk animation with wood (`PMSG_LUMBERING_GET_WALK_ANIM`).
    pub const GET_WALK_ANIM: u32 = 0x308;
    /// Walk: destination reached (`PMSG_LUMBERING_DEST_REACHED`).
    pub const DEST_REACHED: u32 = 0x408;
    /// Walk: aborted (`PMSG_LUMBERING_ABORT`).
    pub const ABORT: u32 = 0x508;
    /// `{store}`: deliver (`PMSG_LUMBERING_DELIVER`).
    pub const DELIVER: u32 = 0x608;
    /// Timer: chop sound.
    pub const CHOP_SOUND: u32 = 0x708;
    /// Timer: restart the chop animation.
    pub const CHOP_ANIM: u32 = 0x808;
    /// Timer: delivery finished.
    pub const DELIVERED: u32 = 0x908;
    /// `{-1, 0}` -> `{distance, tree}` or `{d, tree}` -> start work (`0x77F`).
    pub const FIND_TREE: u32 = 0xA08;
    /// Timer: chop step.
    pub const CHOP_STEP: u32 = 0xB08;
}

/// Work states (`m_Status`) of lumbering.
pub mod state {
    /// Chopping.
    pub const CHOPPING: i32 = 200;
    /// Waiting (`m_WaitCounter`; no code sets this state).
    pub const WAITING: i32 = 201;
}

/// State of `CPropertyLumbering` (`PropertyLumbering_Construct` `0x0051C360`), saved like
/// `PropertyLumbering_SerializeFields` (`0x0051D500`) after the base fields. The hit list
/// `+0x9C` of `ResolveRegionViaLumberyard` is cleared before every use and is local here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lumber {
    pub gather: Gather,
    /// `m_WaitCounter` (`+0xB8`), only in the dead state 201.
    pub wait_counter: i32,
    /// `m_LumberWalkAnim` (`+0xBC`, 90001).
    pub walk_anim: i32,
    /// `m_LumberStandAnim` (`+0xC0`, 90002).
    pub stand_anim: Vec<i32>,
    /// `m_LumberAnim` (`+0xCC`, 90006): chop animation.
    pub lumber_anim: i32,
    /// `m_SearchRadius` (`+0xD0`): neither written nor read (`Init` does not read 90005).
    pub search_radius: i32,
    /// `m_TargetTreePos` (`+0xD4`, x, z, y): search center, set only by `0x108`.
    pub target_tree_pos: (i32, i32, i32),
    /// `m_lastTimeLeak` (`+0xE4`): time remainder of the chop intervals, never reset.
    pub last_time_leak: i32,
}

pub(crate) fn lumber<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Lumber> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Lumber(l) => Some(l),
        _ => None,
    }
}

pub(crate) fn lumber_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Lumber> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Lumber(l) => Some(l),
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

fn has_status(sim: &Sim, id: ObjectId, bits: u32) -> bool {
    sim.object(id).is_some_and(|o| o.status & bits != 0)
}

fn set_status_flags(sim: &mut Sim, id: ObjectId, bits: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(bits, on);
    }
}

/// `0x404 {own ID}` to the tree: release the registration.
fn unregister_at(sim: &mut Sim, id: ObjectId, tree: ObjectId) {
    send(
        sim,
        tree,
        &Message::new(tree::msg::UNREGISTER, &[id.as_i32()]),
    );
}

/// Give up timers and registration, `m_ResourceHID = 0` (**quirk:** 0, not -1) and
/// state 6 ("search anew").
fn give_up_tree(sim: &mut Sim, id: ObjectId) {
    clear_events(sim, id, INDEX);
    set_resource(sim, id, ObjectId(0));
    set_status_of(sim, id, status::SEARCH);
}

/// `PropertyLumbering_Init` (`0x0051D2D0`): stand animations (90002, list), walk (90001), chop
/// (90006) and delivery animation (90007). The original does not read 90003, 90004 and 90005; the
/// module flags stay 0.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let text = desc.values.get(&90002).cloned().unwrap_or_default();
    let (walk, chop, deliver) = (value(90001), value(90006), value(90007));
    if let Some(l) = lumber_mut(sim, id) {
        l.stand_anim = parse_int_list(&text, &l.stand_anim);
        l.walk_anim = walk;
        l.lumber_anim = chop;
        l.gather.deliver_anim = deliver;
    }
}

/// `PropertyLumbering_Query` (`0x0051D390`, slot 7); all other codes go through the
/// base query to [`on_message`].
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        // `IntList_GetModulo`: empty list (division by 0 in the original) -> -1 (A7).
        msg::GET_STAND_ANIM => lumber(sim, id).map_or(-1, |l| list_modulo(&l.stand_anim, m.arg(0))),
        msg::GET_WALK_ANIM => lumber(sim, id).map_or(-1, |l| l.walk_anim),
        msg::CHOP_SOUND => {
            play_chop_sound(sim, id);
            1
        }
        msg::CHOP_ANIM => {
            start_chop_anim(sim, id);
            1
        }
        msg::FIND_TREE => find_tree(sim, id, m),
        msg::CHOP_STEP => {
            chop_step(sim, id);
            1
        }
        _ => super::base_query(sim, id, INDEX, m),
    }
}

/// `PropertyLumbering_OnMessage` (`0x0051D340`, slot 12): `0x108` starts, everything else the base
/// (`0x408` destination reached, `0x508` abort, `0x608` deliver, `0x908` delivery finished).
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if env.msg.code == msg::START {
        return on_start_lumbering(sim, id, env);
    }
    super::on_command(sim, id, INDEX, env)
}

/// `PropertyLumbering_Tick` (`0x0051BE40`, slot 13): the state logic **first**, the timers
/// **then**. Chopping without bit `0x80` -> state 6 (**quirk:** without unregistering at the tree; the
/// chop timers are not cleared until `GotoTree`); state 201 counts `m_WaitCounter` down (dead); otherwise
/// the base tick (state 6 -> `GotoTree`).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    match status_of(sim, id) {
        state::CHOPPING => {
            if !has_status(sim, id, LUMBERING_BIT) {
                set_status_of(sim, id, status::SEARCH);
            }
        }
        state::WAITING => {
            let done = lumber_mut(sim, id).is_some_and(|l| {
                l.wait_counter = l.wait_counter.wrapping_sub(1);
                l.wait_counter < 1
            });
            if done {
                goto_tree(sim, id);
            }
        }
        _ => super::tick_base(sim, id, INDEX),
    }
    update_events(sim, id, INDEX, dt);
}

/// `PropertyLumbering_Abort` (`0x0051C080`, slot 11): exemption zone off, but only if bit `0x80`
/// is still set (**quirk:** if gold mining at slot 7 clears bit `0x80` beforehand while
/// `GameObject_AbortAllModules` aborts, `0x1000` stays), unregister at the tree in **every** state,
/// clear timers, then the base abort.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if has_status(sim, id, LUMBERING_BIT) {
        set_status_flags(sim, id, EXEMPT_BIT, false);
    }
    let t = resource(sim, id);
    if sim.object(t).is_some() {
        unregister_at(sim, id, t);
    }
    clear_events(sim, id, INDEX);
    super::stop(sim, id, INDEX)
}

/// `PropertyLumbering_OnDestReached` (`0x0051C060`, slot 18): in state 1 arrived at the tree,
/// otherwise the base (state 2 -> deliver).
pub fn on_dest_reached(sim: &mut Sim, id: ObjectId, _env: &Envelope) -> bool {
    if status_of(sim, id) != status::TO_SOURCE {
        return super::base_on_dest_reached(sim, id, INDEX);
    }
    on_arrived_at_tree(sim, id);
    true
}

/// `PropertyLumbering_OnStartLumbering` (`0x0051CF80`, command `0x108 {tree}`): take over the
/// feedback messages, bit `0x80`, `m_ResourceHID` and the search center `m_TargetTreePos` (position
/// of the tree, if it is missing: the own one), then `GotoTree`.
fn on_start_lumbering(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    set_status_flags(sim, id, LUMBERING_BIT, true);
    let t = ObjectId::from_i32(env.msg.arg(0));
    let grid = sim
        .object(t)
        .or_else(|| sim.object(id))
        .map_or((0, 0, 0), |o| (o.grid.x.0, o.grid.z.0, o.grid.y.0));
    if let Some(l) = lumber_mut(sim, id) {
        l.gather.resource_hid = t;
        l.target_tree_pos = grid;
    }
    goto_tree(sim, id);
    true
}

/// Search filter of the trees (`GotoTree`, `FindTree`): tree list `0x1000`, built-in check `0x20`,
/// AND, module 4 present with bit `0x100` ("slot free"), without reference object; layer and region of
/// the own tile - if it is blocked (`0xFFFF`), the region via a lumberyard
/// ([`resolve_region_via_lumberyard`]).
fn tree_filter(sim: &mut Sim, id: ObjectId, layer: i32) -> MatchFilter {
    let mut region = region_id_at(sim, id, layer, -1, -1);
    if region == BLOCKED {
        let r = resource(sim, id);
        region = resolve_region_via_lumberyard(sim, id, layer, r);
    }
    MatchFilter {
        mode: 0x1036,
        mode2: 0,
        match_bits: 0x104,
        prop_id: 4,
        ref_object: 0,
        layer,
        region: i32::from(region),
        ..MatchFilter::constructed()
    }
}

/// `PropertyLumbering_GotoTree` (`0x0051CC10`, slot 19): state 1, bit `0x80`, clear timers;
/// invalid walk layer -> abort. Then **always** search anew: nearest matching tree around
/// `m_TargetTreePos` (center distance, whole map; **quirk:** the clicked tree counts
/// only if it matches itself). No tree: `m_ResourceHID = -1`, with a load (also gold or
/// food) to the lumberyard, otherwise abort. Tree: remember, exemption circle `hard + 0xB51` around it with
/// bit `0x1000`, register (`0x304`, slot 1/4 tile diagonally on the tree tile; full -> to the
/// tree center) and walk FINDTOTREE `{target, radius -1, look point tree, tolerance -1, flags 1}` with
/// `0x408`/`0x508`.
pub fn goto_tree(sim: &mut Sim, id: ObjectId) {
    set_status_of(sim, id, status::TO_SOURCE);
    set_status_flags(sim, id, LUMBERING_BIT, true);
    log::debug!("Lumberjack {id:?}: going to tree");
    clear_events(sim, id, INDEX);
    let layer = walk_layer(sim, id);
    if !(0..9).contains(&layer) {
        abort_module(sim, id, INDEX);
        return;
    }
    let f = tree_filter(sim, id, layer);
    let center = lumber(sim, id).map_or((0, 0), |l| (l.target_tree_pos.0, l.target_tree_pos.1));
    let found = find_nearest_matching(sim, center, &f, -1, i32::from(id.slot()), -1);
    let Some(t) = found else {
        log::debug!("Lumberjack {id:?}: no tree found");
        set_resource(sim, id, ObjectId::NONE);
        let (amount, kind) = sim
            .object(id)
            .map_or((0, load::NONE), |o| (o.load_amount, o.load_kind));
        if amount != 0 && kind != load::NONE {
            super::goto_delivery(sim, id, INDEX, ObjectId::NONE);
        } else {
            abort_module(sim, id, INDEX);
        }
        return;
    };
    let tpos = position(sim, t);
    set_resource(sim, id, t);
    if let Some(o) = sim.object_mut(id) {
        let r = o.hard_radius.wrapping_add(EXEMPT_MARGIN);
        o.set_collision_exempt_circle(tpos.0, tpos.1, r);
        o.set_status_flags(EXEMPT_BIT, true);
    }
    let mut goal = tpos;
    let mut reg = Message::new(tree::msg::REGISTER, &[0, id.as_i32()]);
    if send_mut(sim, t, &mut reg) >= 0 {
        goal = (reg.arg(0), reg.arg(1));
    }
    let env = Envelope {
        msg: Message::new(0x300, &[goal.0, goal.1, -1, tpos.0, tpos.1, -1, 1]),
        success: Some(Message::new(msg::DEST_REACHED, &[])),
        fault: Some(Message::new(msg::ABORT, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyLumbering_OnArrivedAtTree` (`0x0051BEC0`): already chopping -> nothing. Tree present with
/// wood: state 200, chop animation, timers sound (time marker 2) and animation loop
/// (time marker 1) and the **first** chop step one full interval later (the first unit
/// does not come immediately). Otherwise clear timers, `m_ResourceHID = 0`, state 6 and unregister.
fn on_arrived_at_tree(sim: &mut Sim, id: ObjectId) {
    if status_of(sim, id) == state::CHOPPING {
        return;
    }
    let t = resource(sim, id);
    let exists = sim.object(t).is_some();
    if exists && sim.attribute(t, tree::WOOD) > 0 {
        set_status_of(sim, id, state::CHOPPING);
        log::debug!("Lumberjack {id:?}: chopping");
        let anim = lumber(sim, id).map_or(0, |l| l.lumber_anim);
        set_animation(sim, id, anim);
        let sound = super::anim_label_time(sim, id, anim, 2);
        post_delayed(sim, id, INDEX, sound, Message::new(msg::CHOP_SOUND, &[]));
        let loop_ = super::anim_label_time(sim, id, anim, 1);
        post_delayed(sim, id, INDEX, loop_, Message::new(msg::CHOP_ANIM, &[]));
        schedule_chop(sim, id);
    } else {
        give_up_tree(sim, id);
        if exists {
            unregister_at(sim, id, t);
        }
    }
}

/// Schedule the next chop step (`0x0051BF9F...`/`0x0051C263...`): interval from the wood rate (attribute
/// 32) and `m_lastTimeLeak` ([`super::unit_delay`]).
fn schedule_chop(sim: &mut Sim, id: ObjectId) {
    let leak = lumber(sim, id).map_or(0, |l| l.last_time_leak);
    let leak = super::schedule_next_unit(
        sim,
        id,
        INDEX,
        RATE,
        leak,
        Message::new(msg::CHOP_STEP, &[]),
    );
    if let Some(l) = lumber_mut(sim, id) {
        l.last_time_leak = leak;
    }
}

/// `PropertyLumbering_PlayChopSound` (`0x0051C100`, `0x708`): sound at the lumberjack (presentation).
fn play_chop_sound(sim: &mut Sim, id: ObjectId) {
    sim.state.events.push(SimEvent::Sound {
        object: id,
        sound: SFX_CHOP,
    });
}

/// `PropertyLumbering_StartChopAnim` (`0x0051C120`, `0x808`): only chopping with bit `0x80` -
/// schedule sound (time marker 2) and loop (time marker 1) again, set the chop animation. Pure
/// presentation, but in the same event list (group `0x80`); without a marker both fire in every
/// tick.
fn start_chop_anim(sim: &mut Sim, id: ObjectId) {
    if status_of(sim, id) != state::CHOPPING || !has_status(sim, id, LUMBERING_BIT) {
        return;
    }
    let anim = lumber(sim, id).map_or(0, |l| l.lumber_anim);
    let sound = super::anim_label_time(sim, id, anim, 2);
    post_delayed(sim, id, INDEX, sound, Message::new(msg::CHOP_SOUND, &[]));
    let loop_ = super::anim_label_time(sim, id, anim, 1);
    post_delayed(sim, id, INDEX, loop_, Message::new(msg::CHOP_ANIM, &[]));
    set_animation(sim, id, anim);
}

/// `PropertyLumbering_ChopStep` (`0x0051C1D0`, `0xB08`): tree gone -> state 6 (without unregistering).
/// A foreign load kind counts as 0 (is replaced by wood). Full (`>= capacity`) -> clear timers,
/// to the lumberyard (slot 20 with -1) and **afterwards** unregister. Otherwise `0x104 {1}`: got wood
/// -> load it and schedule the next step (**no** full check here - "full" is
/// recognized only in the next step); got nothing -> state 6 and unregister.
fn chop_step(sim: &mut Sim, id: ObjectId) {
    let t = resource(sim, id);
    if sim.object(t).is_none() {
        give_up_tree(sim, id);
        return;
    }
    let amount = own_load(sim, id, load::WOOD);
    if amount >= super::capacity(sim, id) {
        clear_events(sim, id, INDEX);
        super::goto_delivery(sim, id, INDEX, ObjectId::NONE);
        unregister_at(sim, id, t);
        return;
    }
    let got = send(sim, t, &Message::new(tree::msg::GET_WOOD, &[1]));
    if got > 0 {
        set_carried_load(sim, id, amount.wrapping_add(got), load::WOOD);
        schedule_chop(sim, id);
    } else {
        give_up_tree(sim, id);
        unregister_at(sim, id, t);
    }
}

/// `PropertyLumbering_FindTree` (`0x0051D030`, query `0xA08` from `GameObject_ResumeNearestWork`
/// `0x77F`). Second call (`args[0] != -1`): post `0x108 {args[1]}` with abort (1 =
/// queued). First: invalid layer -> abort, -1; nearest matching tree within **one
/// tile** (center distance) and any own finished lumberyard on the map -> `args =
/// {edge distance to the tree, tree}`, 1; otherwise 0.
fn find_tree(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    if m.arg(0) != -1 {
        return i32::from(post_message(
            sim,
            id,
            Message::new(msg::START, &[m.arg(1)]),
            true,
        ));
    }
    let layer = walk_layer(sim, id);
    if !(0..9).contains(&layer) {
        abort_module(sim, id, INDEX);
        return -1;
    }
    let mut f = tree_filter(sim, id, layer);
    let (pos, owner) = (position(sim, id), i32::from(id.slot()));
    let Some(t) = find_nearest_matching(sim, pos, &f, TILE, owner, -1) else {
        return 0;
    };
    f.mode = 0x150;
    f.mode2 = 0;
    f.match_bits = LUMBERING.deliver_class;
    if find_nearest_matching(sim, pos, &f, -1, owner, -1).is_none() {
        return 0;
    }
    let tpos = position(sim, t);
    let d = edge_distance_to_point(sim, id, tpos);
    m.set_arg(0, d);
    m.set_arg(1, t.as_i32());
    1
}

/// `0x2C03 {layer, four regions}` to a building: index of the first region that one of its
/// docking places has (0...3), otherwise -1.
fn region_message(layer: i32, regions: [u16; 4]) -> Message {
    let r = regions.map(i32::from);
    Message::new(0x2C03, &[layer, r[0], r[1], r[2], r[3]])
}

/// `PropertyLumbering_ResolveRegionViaLumberyard(layer, source)` (`0x0051C4B0`): substitute region for
/// a lumberjack on a blocked tile. Without a source object (`m_ResourceHID` 0 or -1) `0xFFFF`.
/// The regions of the four neighboring tiles of the **tree** (`z+1, z-1, x+1, x-1`) go as `0x2C03` to
/// the nearest own lumberyard (edge distance; one bit `0x1000` suffices, also unfinished ones), then to
/// all own lumberyards of the map in order of discovery (`SpatialGrid_QueryRadius`); the first
/// answer 0...3 selects the region.
fn resolve_region_via_lumberyard(sim: &mut Sim, id: ObjectId, layer: i32, res: ObjectId) -> u16 {
    let Some((x, z)) = sim.object(res).map(|o| (o.grid.x.0, o.grid.z.0)) else {
        return BLOCKED;
    };
    let regions = [
        region_id_at(sim, res, layer, x, z.wrapping_add(TILE)),
        region_id_at(sim, res, layer, x, z.wrapping_sub(TILE)),
        region_id_at(sim, res, layer, x.wrapping_add(TILE), z),
        region_id_at(sim, res, layer, x.wrapping_sub(TILE), z),
    ];
    let f = MatchFilter {
        mode: 0x112,
        match_bits: 0x1003,
        ..MatchFilter::constructed()
    };
    let m = region_message(layer, regions);
    let pick = |k: i32| {
        usize::try_from(k)
            .ok()
            .and_then(|k| regions.get(k).copied())
    };
    let (pos, owner) = (position(sim, id), i32::from(id.slot()));
    let hard = sim.object(id).map_or(0, |o| o.hard_radius);
    if let Some(b) = find_nearest_matching(sim, pos, &f, -1, owner, hard)
        && let Some(r) = pick(send(sim, b, &m))
    {
        return r;
    }
    for b in query_radius(sim, pos, &f, -1, owner, false) {
        if let Some(r) = pick(send(sim, b, &m)) {
            return r;
        }
    }
    BLOCKED
}

/// `PropertyLumbering_GotoDeliveryBuilding(hint)` (`0x0051C800`, slot 20): with a hint or on
/// a tile with a region the base. Otherwise (at the tree, blocked tile) first **one tile
/// out**: among all own finished lumberyards (order of discovery) the one with the smallest
/// edge distance (strictly smaller) that reaches one of the regions of the four neighboring tiles of the
/// lumberjack (`z+1, z-1, x+1, x-1`) (`0x2C03`); FINDTO one tile far there (flags `0x20`),
/// success `0x608 {store}` (then the base with the hint), fault `0x508`. No lumberyard or invalid
/// layer -> abort.
pub fn goto_delivery_building(sim: &mut Sim, id: ObjectId, hint: ObjectId) {
    if !hint.is_none() || region_id_at(sim, id, -1, -1, -1) != BLOCKED {
        super::goto_delivery_building(sim, id, INDEX, hint);
        return;
    }
    let f = MatchFilter {
        mode: 0x150,
        match_bits: LUMBERING.deliver_class,
        ..MatchFilter::constructed()
    };
    let (pos, owner) = (position(sim, id), i32::from(id.slot()));
    let list = query_radius(sim, pos, &f, -1, owner, false);
    if list.is_empty() {
        abort_module(sim, id, INDEX);
        return;
    }
    let layer = walk_layer(sim, id);
    if !(0..9).contains(&layer) {
        abort_module(sim, id, INDEX);
        return;
    }
    let (x, z) = pos;
    let regions = [
        region_id_at(sim, id, layer, x, z.wrapping_add(TILE)),
        region_id_at(sim, id, layer, x, z.wrapping_sub(TILE)),
        region_id_at(sim, id, layer, x.wrapping_add(TILE), z),
        region_id_at(sim, id, layer, x.wrapping_sub(TILE), z),
    ];
    let m = region_message(layer, regions);
    let mut best: Option<(ObjectId, i32)> = None;
    let mut best_d = i32::MAX;
    for b in list {
        let d = edge_distance_to_object(sim, id, b);
        if d < best_d {
            let k = send(sim, b, &m);
            if k >= 0 {
                best = Some((b, k));
                best_d = d;
            }
        }
    }
    let Some((b, k)) = best else {
        abort_module(sim, id, INDEX);
        return;
    };
    let (mut gx, mut gz) = (x, z);
    match k {
        0 => gz = gz.wrapping_add(TILE),
        1 => gz = gz.wrapping_sub(TILE),
        2 => gx = gx.wrapping_add(TILE),
        3 => gx = gx.wrapping_sub(TILE),
        _ => {}
    }
    let env = Envelope {
        msg: find_to_msg(gx, gz, -1, NO_DIRECTION, -1, false, 0x20),
        success: Some(Message::new(msg::DELIVER, &[b.as_i32()])),
        fault: Some(Message::new(LUMBERING.fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Posted;
    use crate::module::gather::tests::{
        COLONIST, LUMBERYARD, TREE, WARRIOR, rules, spawn, state as new_state,
    };
    use crate::module::tick as module_tick;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::step;

    fn load_of(sim: &Sim, id: ObjectId) -> (i32, i32) {
        sim.object(id)
            .map_or((0, 0), |o| (o.load_amount, o.load_kind))
    }

    fn wood(sim: &Sim, t: ObjectId) -> i32 {
        sim.attribute(t, tree::WOOD)
    }

    fn reg_pos(sim: &Sim, t: ObjectId) -> [ObjectId; 2] {
        match &sim.module(t, tree::INDEX).unwrap().kind {
            ModuleKind::Tree(tr) => tr.reg_pos,
            _ => unreachable!(),
        }
    }

    /// Arrival at the tree: registered, state 1 with bit `0x80`; walk reports `0x408` in tick T0 before
    /// the tick of the lumbering module (module 0 ticks first).
    fn arrive(sim: &mut Sim, unit: ObjectId, t: ObjectId) {
        send(
            sim,
            t,
            &Message::new(tree::msg::REGISTER, &[0, unit.as_i32()]),
        );
        if let Some(l) = lumber_mut(sim, unit) {
            l.gather.resource_hid = t;
            l.gather.status = status::TO_SOURCE;
        }
        set_status_flags(sim, unit, LUMBERING_BIT, true);
        assert_eq!(send(sim, unit, &Message::new(msg::DEST_REACHED, &[])), 1);
        module_tick(sim, unit, INDEX, 100);
    }

    /// Ticks after T0 in which a unit came, and the tick in which the chopping ended.
    fn chop_until_done(sim: &mut Sim, unit: ObjectId, max: i32) -> (Vec<i32>, i32) {
        let mut units = Vec::new();
        let mut last = load_of(sim, unit).0;
        for t in 1..=max {
            module_tick(sim, unit, INDEX, 100);
            let now = load_of(sim, unit).0;
            if now != last {
                units.push(t);
                last = now;
            }
            if status_of(sim, unit) != state::CHOPPING {
                return (units, t);
            }
        }
        panic!("chopping does not end");
    }

    #[test]
    fn timers_fire_delay_over_100_plus_one_ticks_after_the_posting_tick() {
        for (d, want) in [(0, 1), (100, 2), (700, 8), (800, 9), (1000, 11), (1900, 20)] {
            let rules = rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
            post_delayed(&mut sim, unit, INDEX, d, Message::new(msg::CHOP_SOUND, &[]));
            module_tick(&mut sim, unit, INDEX, 100);
            sim.state.events.clear();
            let fired = (1..=30).find(|_| {
                module_tick(&mut sim, unit, INDEX, 100);
                sim.state.events.contains(&SimEvent::Sound {
                    object: unit,
                    sound: SFX_CHOP,
                })
            });
            assert_eq!(fired, Some(want), "d = {d}");
        }
    }

    #[test]
    fn a_colonist_gets_a_unit_every_eleven_ticks_and_sees_full_one_step_later() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let t = spawn(&mut sim, TREE, 9, (12, 10));
        arrive(&mut sim, unit, t);
        assert_eq!(status_of(&sim, unit), state::CHOPPING);
        assert_eq!(sim.object(unit).unwrap().anim.wanted, 3);
        let (units, done) = chop_until_done(&mut sim, unit, 400);
        assert_eq!(units, (1..=20).map(|k| 11 * k).collect::<Vec<_>>());
        assert_eq!(done, 231);
        // Without a lumberyard: abort with a full load, unregistered at the tree, gather bits off.
        assert_eq!(status_of(&sim, unit), status::IDLE);
        assert_eq!(load_of(&sim, unit), (20, load::WOOD));
        assert_eq!(wood(&sim, t), 180);
        assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
        assert_eq!(sim.object(unit).unwrap().status & 0x80, 0);
        assert_eq!(lumber(&sim, unit).unwrap().last_time_leak, 0);
    }

    #[test]
    fn a_wid_warrior_alternates_eight_and_nine_ticks_with_a_leak() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, WARRIOR, 0, (10, 10));
        let t = spawn(&mut sim, TREE, 9, (12, 10));
        arrive(&mut sim, unit, t);
        let (units, done) = chop_until_done(&mut sim, unit, 400);
        assert_eq!(units.len(), 20);
        assert_eq!(&units[..4], &[8, 17, 25, 34]);
        assert_eq!(units[19], 170);
        assert_eq!(done, 178);
        assert_eq!(lumber(&sim, unit).unwrap().last_time_leak, 50);
    }

    #[test]
    fn a_tree_with_seven_wood_gives_seven_units_then_the_lumberjack_searches_anew() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let t = spawn(&mut sim, TREE, 9, (12, 10));
        if let Some(o) = sim.object_mut(t) {
            o.attributes.set_current_value(tree::WOOD, 7, 0);
        }
        arrive(&mut sim, unit, t);
        let (units, done) = chop_until_done(&mut sim, unit, 400);
        assert_eq!(units, (1..=7).map(|k| 11 * k).collect::<Vec<_>>());
        assert_eq!(done, 88);
        // `0x104` returns 0: state 6, `m_ResourceHID = 0` (quirk), unregistered.
        assert_eq!(status_of(&sim, unit), status::SEARCH);
        assert_eq!(resource(&sim, unit), ObjectId(0));
        assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
        assert_eq!(load_of(&sim, unit), (7, load::WOOD));
    }

    #[test]
    fn a_foreign_load_counts_as_zero_and_is_replaced_by_one_wood() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let t = spawn(&mut sim, TREE, 9, (12, 10));
        arrive(&mut sim, unit, t);
        super::super::set_carried_load(&mut sim, unit, 5, load::GOLD);
        assert_eq!(send(&mut sim, unit, &Message::new(msg::CHOP_STEP, &[])), 1);
        assert_eq!(load_of(&sim, unit), (1, load::WOOD));
        assert_eq!(wood(&sim, t), 199);
    }

    #[test]
    fn the_chop_sound_and_loop_follow_time_labels_two_and_one() {
        let mut rules = rules();
        // Chop animation 3: time marker 1 after 500 ms (loop), time marker 2 after 300 ms (sound).
        rules.set_anim_labels(COLONIST, 3, &[(1, 500), (2, 300)]);
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let t = spawn(&mut sim, TREE, 9, (12, 10));
        arrive(&mut sim, unit, t);
        let starts = sim.object(unit).unwrap().anim.starts;
        let mut sounds = Vec::new();
        for k in 1..=12 {
            sim.state.events.clear();
            module_tick(&mut sim, unit, INDEX, 100);
            if sim.state.events.contains(&SimEvent::Sound {
                object: unit,
                sound: SFX_CHOP,
            }) {
                sounds.push(k);
            }
        }
        // Sound after 300 ms (tick 4); the loop (tick 6) schedules sound (tick 10) and loop again.
        assert_eq!(sounds, [4, 10]);
        // Without an object tick the wanted animation is not passed on; wanted stays 3.
        let o = sim.object(unit).unwrap();
        assert_eq!((o.anim.wanted, o.anim.starts), (3, starts));
    }

    #[test]
    fn aborting_unregisters_and_clears_the_exempt_bit_only_while_lumbering() {
        for lumbering in [true, false] {
            let rules = rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
            let t = spawn(&mut sim, TREE, 9, (12, 10));
            arrive(&mut sim, unit, t);
            set_status_flags(&mut sim, unit, EXEMPT_BIT, true);
            // As after `Mining.Stop` at slot 7: bit 0x80 already cleared.
            set_status_flags(&mut sim, unit, LUMBERING_BIT, lumbering);
            assert!(crate::module::abort(&mut sim, unit, INDEX));
            let o = sim.object(unit).unwrap();
            assert_eq!(o.status & EXEMPT_BIT != 0, !lumbering);
            assert_eq!(o.status & LUMBERING_BIT, 0);
            assert_eq!(status_of(&sim, unit), status::IDLE);
            assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
            assert!(sim.module(unit, INDEX).unwrap().base.timers_ws.is_empty());
        }
    }

    #[test]
    fn chopping_lumberjack_keeps_the_exempt_zone_when_all_modules_abort() {
        // re/gathering 2.12: if the lumberjack is chopping (walk without fault message), slot 7
        // (`Mining.Stop`) clears bit 0x80 first in `GameObject_AbortAllModules`; slot 8
        // (`PropertyLumbering_Abort`) no longer sees it -> `0x1000` stays set (quirk).
        let rules = crate::module::gather::tests::econ_rules();
        let mut state = new_state(&rules);
        let (unit, t) = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
            assert!(
                sim.module(unit, crate::module::gather::mining::INDEX)
                    .is_some()
            );
            let t = spawn(&mut sim, TREE, 9, (12, 10));
            // The real path: `GotoTree` sets exemption zone and `0x1000`, walk reports the arrival.
            send(&mut sim, unit, &Message::new(msg::START, &[t.as_i32()]));
            (unit, t)
        };
        let mut steps = 0;
        while status_of(&Sim::new(&mut state, &rules), unit) != state::CHOPPING {
            step(&mut state, &rules, &[]);
            steps += 1;
            assert!(steps < 200, "no chopping");
        }
        let mut sim = Sim::new(&mut state, &rules);
        let st = sim.object(unit).unwrap().status;
        assert_ne!(st & EXEMPT_BIT, 0);
        assert_ne!(st & LUMBERING_BIT, 0);
        crate::messaging::abort_all_modules(&mut sim, unit);
        let o = sim.object(unit).unwrap();
        assert_eq!(o.status & LUMBERING_BIT, 0);
        assert_ne!(o.status & EXEMPT_BIT, 0, "exemption zone stays");
        assert_eq!(status_of(&sim, unit), status::IDLE);
        assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
    }

    #[test]
    fn loaded_lumberjacks_answer_the_stand_and_walk_animation_queries() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::GET_STAND_ANIM, &[7])),
            5
        );
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::GET_WALK_ANIM, &[])),
            4
        );
        // Stand asks the lumbering module with wood (list 90002), the own list without a load.
        assert_eq!(crate::module::stand::select_idle_anim(&mut sim, unit, 3), 0);
        super::super::set_carried_load(&mut sim, unit, 3, load::WOOD);
        assert_eq!(crate::module::stand::select_idle_anim(&mut sim, unit, 3), 5);
        // Unknown codes: base query -> OnMessage -> not handled (-1).
        assert_eq!(send(&mut sim, unit, &Message::new(0xC08, &[])), -1);
    }

    #[test]
    fn a_new_order_aborts_walk_first_whose_fault_aborts_lumbering() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let t = spawn(&mut sim, TREE, 9, (20, 10));
        send(&mut sim, unit, &Message::new(msg::START, &[t.as_i32()]));
        assert_eq!(status_of(&sim, unit), status::TO_SOURCE);
        assert_eq!(reg_pos(&sim, t)[0], unit);
        // `GameObject_AbortAllModules`: walk (slot 0) sends `0x508` synchronously -> abort with bit
        // 0x80 (exemption zone off); slot 8 then aborts a second time (no effect).
        crate::messaging::abort_all_modules(&mut sim, unit);
        let o = sim.object(unit).unwrap();
        assert_eq!(o.status & (EXEMPT_BIT | LUMBERING_BIT), 0);
        assert_eq!(status_of(&sim, unit), status::IDLE);
        assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
    }

    #[test]
    fn stopping_while_delivering_keeps_the_dock_claimed() {
        // State 2 releases the docking place, state 3 (delivery running) does not (quirk).
        for (s, freed) in [(status::TO_DELIVERY, true), (status::DELIVERING, false)] {
            let rules = rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
            let yard = spawn(&mut sim, LUMBERYARD, 0, (20, 10));
            send(&mut sim, yard, &Message::new(0xC03, &[0, unit.as_i32()]));
            if let Some(g) = gather_mut(&mut sim, unit, INDEX) {
                g.status = s;
                g.hq_hid = yard;
            }
            let fault = Message::new(0xA7F, &[77]);
            sim.module_mut(unit, INDEX)
                .unwrap()
                .base
                .set_fault(Some(&fault), ObjectId::NONE);
            crate::module::abort(&mut sim, unit, INDEX);
            let lots = &crate::module::building::building(&sim, yard)
                .unwrap()
                .parking_lots;
            assert_eq!(lots[0].hid.is_none(), freed, "state {s}");
            // The fault message went out synchronously.
            assert_eq!(sim.object(unit).unwrap().carrier, ObjectId(77));
            assert_eq!(status_of(&sim, unit), status::IDLE);
        }
    }

    #[test]
    fn delivering_takes_two_ticks_without_an_animation() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        super::super::set_carried_load(&mut sim, unit, 20, load::WOOD);
        set_status_of(&mut sim, unit, status::TO_DELIVERY);
        let anim = sim.object(unit).unwrap().anim.wanted;
        // Arrival in tick T0: state 3, timer 100 ms, no animation.
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::DEST_REACHED, &[])),
            1
        );
        assert_eq!(status_of(&sim, unit), status::DELIVERING);
        assert_eq!(sim.object(unit).unwrap().anim.wanted, anim);
        module_tick(&mut sim, unit, INDEX, 100);
        sim.state.events.clear();
        let credited = |sim: &Sim| {
            sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::Credited { amount: 20, .. }))
        };
        module_tick(&mut sim, unit, INDEX, 100);
        assert!(!credited(&sim));
        module_tick(&mut sim, unit, INDEX, 100);
        assert!(credited(&sim));
        // Afterwards back to the source: no tree and no load -> abort.
        assert_eq!(status_of(&sim, unit), status::IDLE);
        assert_eq!(load_of(&sim, unit), (0, load::NONE));
    }

    #[test]
    fn starting_searches_around_the_clicked_tree_registers_and_walks_to_the_corner() {
        let rules = rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let a = spawn(&mut sim, TREE, 9, (20, 10));
        let b = spawn(&mut sim, TREE, 9, (21, 10));
        let far = spawn(&mut sim, TREE, 9, (11, 12));
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::START, &[b.as_i32()])),
            1
        );
        let pb = position(&sim, b);
        let l = lumber(&sim, unit).unwrap();
        assert_eq!((l.target_tree_pos.0, l.target_tree_pos.1), pb);
        assert_eq!(
            (l.gather.resource_hid, l.gather.status),
            (b, status::TO_SOURCE)
        );
        let o = sim.object(unit).unwrap();
        assert_eq!(o.status & 0x1080, 0x1080);
        assert_eq!(o.exempt.center, pb);
        assert_eq!(o.exempt.radius, 512 + 0xB51);
        assert_eq!(o.exempt.mode, 1);
        assert_eq!(reg_pos(&sim, b)[0], unit);
        let w = crate::module::walk::walk(&sim, unit).unwrap();
        assert_eq!(
            w.find_to.as_ref().map(|m| (m.code, m.args.clone())),
            Some((
                0x300,
                vec![pb.0 - 0x400, pb.1 - 0x400, -1, pb.0, pb.1, -1, 1]
            ))
        );
        // The clicked tree is full (two others registered): the nearest around it, not the
        // nearest around the lumberjack.
        send(
            &mut sim,
            b,
            &Message::new(tree::msg::UNREGISTER, &[unit.as_i32()]),
        );
        for h in [0x7_0001, 0x7_0002] {
            send(&mut sim, b, &Message::new(tree::msg::REGISTER, &[0, h]));
        }
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::START, &[b.as_i32()])),
            1
        );
        assert_eq!(resource(&sim, unit), a);
        assert_ne!(resource(&sim, unit), far);
    }

    #[test]
    fn resuming_work_starts_lumbering_at_an_adjacent_tree_when_a_lumberyard_exists() {
        // (tree tile, lumberyard present) -> posted work.
        for (tree_tile, yard, expect) in [
            ((11, 10), true, true),
            ((11, 10), false, false),
            ((13, 10), true, false),
        ] {
            let rules = rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
            let t = spawn(&mut sim, TREE, 9, tree_tile);
            if yard {
                spawn(&mut sim, LUMBERYARD, 0, (20, 20));
            }
            // A foreign lumberyard does not count.
            spawn(&mut sim, LUMBERYARD, 1, (25, 25));
            let mut q = Message::new(msg::FIND_TREE, &[-1, 0]);
            let r = send_mut(&mut sim, unit, &mut q);
            if expect {
                // Edge distance: one tile minus hard radius.
                assert_eq!((r, q.args.clone()), (1, vec![0x1000 - 512, t.as_i32()]));
            } else {
                assert_eq!((r, q.args.clone()), (0, vec![-1, 0]));
            }
            send(&mut sim, unit, &Message::new(0x77F, &[]));
            let fifo: Vec<Posted> = sim.object(unit).unwrap().fifo.iter().cloned().collect();
            if expect {
                assert_eq!(
                    fifo,
                    vec![Posted::Message {
                        msg: Message::new(msg::START, &[t.as_i32()]),
                        abort: true
                    }]
                );
            } else {
                assert!(fifo.is_empty(), "{tree_tile:?} {yard}");
            }
        }
    }

    /// Steps until `done` (at most `max`); returns the number.
    fn run_until(
        state: &mut SimState,
        rules: &Rules,
        max: usize,
        done: impl Fn(&Sim) -> bool,
    ) -> usize {
        for n in 1..=max {
            step(state, rules, &[]);
            if done(&Sim::new(state, rules)) {
                return n;
            }
        }
        panic!("not reached after {max} steps");
    }

    fn snapshot(
        state: &mut SimState,
        rules: &Rules,
        unit: ObjectId,
    ) -> (i32, ObjectId, (i32, i32)) {
        let sim = Sim::new(state, rules);
        (
            status_of(&sim, unit),
            resource(&sim, unit),
            load_of(&sim, unit),
        )
    }

    #[test]
    fn a_lumberjack_fells_delivers_to_the_lumberyard_and_returns() {
        let rules = rules();
        let mut state = new_state(&rules);
        let (unit, t, yard) = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 16));
            let t = spawn(&mut sim, TREE, 9, (14, 16));
            spawn(&mut sim, TREE, 9, (14, 19));
            let yard = spawn(&mut sim, LUMBERYARD, 0, (5, 16));
            post_message(
                &mut sim,
                unit,
                Message::new(msg::START, &[t.as_i32()]),
                true,
            );
            (unit, t, yard)
        };
        let wood0 = state.players[0].resource_raw(1);
        // Chop until full.
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == state::CHOPPING
        });
        // At the registration slot 1/4 tile lower left of the tree center: on the blocked tree tile.
        let at = {
            let mut sim = Sim::new(&mut state, &rules);
            assert_eq!(resource(&sim, unit), t);
            let (p, pt) = (position(&sim, unit), position(&sim, t));
            assert_eq!(p, (pt.0 - 0x400, pt.1 - 0x400));
            assert_eq!(region_id_at(&mut sim, unit, -1, -1, -1), BLOCKED);
            p
        };
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) != state::CHOPPING
        });
        {
            let sim = Sim::new(&mut state, &rules);
            assert_eq!(load_of(&sim, unit), (20, load::WOOD));
            assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
            // First one tile out (region `z+1` first, success `0x608 {store}`), then the
            // base path with the lumberyard as hint: state 2, FINDTO `{store, -1, 1}`.
            assert_eq!(position(&sim, unit), (at.0, at.1 + 0x1000));
            let g = gather(&sim, unit, INDEX).unwrap();
            assert_eq!((g.status, g.hq_hid), (status::TO_DELIVERY, yard));
            let w = crate::module::walk::walk(&sim, unit).unwrap();
            assert_eq!(
                w.find_to,
                Some(Message::new(0x200, &[yard.as_i32(), -1, 1]))
            );
        }
        // Deliver: balance +20, load empty, back to the tree.
        run_until(&mut state, &rules, 2000, |sim| {
            sim.state.players[0].resource_raw(1) != wood0
        });
        assert_eq!(state.players[0].resource_raw(1), wood0 + (20 << 12));
        let (s, r, load) = snapshot(&mut state, &rules, unit);
        assert_eq!((s, r, load), (status::TO_SOURCE, t, (0, load::NONE)));
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == state::CHOPPING
        });
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(resource(&sim, unit), t);
        assert_eq!(wood(&sim, t), 180);
        let _ = yard;
    }

    #[test]
    fn without_a_lumberyard_the_lumberjack_stops_with_a_full_load() {
        let rules = rules();
        let mut state = new_state(&rules);
        let (unit, t) = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 16));
            let t = spawn(&mut sim, TREE, 9, (14, 16));
            post_message(
                &mut sim,
                unit,
                Message::new(msg::START, &[t.as_i32()]),
                true,
            );
            (unit, t)
        };
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == state::CHOPPING
        });
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) != state::CHOPPING
        });
        for _ in 0..50 {
            step(&mut state, &rules, &[]);
        }
        let (s, _, load) = snapshot(&mut state, &rules, unit);
        assert_eq!((s, load), (status::IDLE, (20, load::WOOD)));
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(wood(&sim, t), 180);
        assert_eq!(reg_pos(&sim, t), [ObjectId::NONE; 2]);
    }

    #[test]
    fn a_tree_felled_while_chopping_makes_the_lumberjack_search_anew() {
        let mut rules = rules();
        // Slow lumberjack (rate 20: 3000 ms, 31 ticks): the tree falls between two steps.
        rules.defaults.get_mut(&COLONIST).unwrap().insert(32, 20);
        let mut state = new_state(&rules);
        let (unit, a, b) = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 16));
            let a = spawn(&mut sim, TREE, 9, (14, 16));
            let b = spawn(&mut sim, TREE, 9, (16, 19));
            post_message(
                &mut sim,
                unit,
                Message::new(msg::START, &[a.as_i32()]),
                true,
            );
            (unit, a, b)
        };
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == state::CHOPPING
        });
        // Someone takes all the wood: the tree falls (16 phase-0 ticks) and disappears before
        // the next chop step.
        {
            let mut sim = Sim::new(&mut state, &rules);
            assert_eq!(
                send(&mut sim, a, &Message::new(tree::msg::GET_WOOD, &[1000])),
                200
            );
        }
        let n = run_until(&mut state, &rules, 200, |sim| sim.object(a).is_none());
        assert!(n <= 32, "{n}");
        assert_eq!(snapshot(&mut state, &rules, unit).0, state::CHOPPING);
        // Next step: tree gone -> state 6 -> in the next tick `GotoTree` to tree b.
        run_until(&mut state, &rules, 200, |sim| {
            status_of(sim, unit) != state::CHOPPING
        });
        run_until(&mut state, &rules, 10, |sim| resource(sim, unit) == b);
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == state::CHOPPING
        });
        assert_eq!(snapshot(&mut state, &rules, unit).1, b);
    }
}
