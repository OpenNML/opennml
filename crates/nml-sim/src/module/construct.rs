//! Module 1 `CPropertyConstructBuilding` (re/construction): the construction worker. The command
//! `0x101 {building}` checks the own position (walk `0x900`), walks via FINDTO `{building, -1, 1}`
//! to a docking place of the building, checks again there and then sends in **every** object tick
//! a build step `0x203 {speed, 0}` to the building. Its answer decides: 1 continue, 0
//! done (field and gold mine hand over to harvesting `0x10C` or gold mining `0x107`, otherwise the
//! success message), 2 continue at the next construction site (`args[1]`). The same path repairs finished
//! buildings. The hammer sound is a timer that only produces an event.
//!
//! States (`m_Status`): 0 free, 1 started (check passed), 2 building, 3 arrived, 4 check
//! or walk running, 5 repairing, 6 target is finished and undamaged.
//!
//! Event bit 5 for the scripts (E-187) lies on the construction worker: off at the start of construction
//! ([`on_start_building`]), on at the end of a build step (`finish`). The branches for
//! siege engines (modules `0x2E`/`0x32`, without a factory class) and throwing (`ISPIONEER`, never set
//! in the shipped data) exist as in the original, but are unreachable.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::{edge_distance_to_object, find_nearest_matching};
use crate::filter::matches_class;
use crate::fx::FxVec3;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, post_command, send, send_fault, send_mut, send_success};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::ModuleKind;
use super::building::flags as building_flags;
use super::events::update_events;

/// Module index.
pub const INDEX: usize = 1;
/// Group of the timers (hammer sound, throw).
const GROUP: u8 = 0x80;
/// Module flag `ISPIONEER` (with the module index in the low byte; never set in the shipped data).
pub const ISPIONEER: u32 = 0x101;
/// State bit `CONSTRUCT` (`GameObject+0x1AC`): the worker is on its way with a construction order.
pub const CONSTRUCTING: u32 = 0x20;
/// Hammer sound (sound list `0x67`, group `0x12`).
pub const SFX_HAMMER: i32 = 0x67;
/// Construction speed without a speed upgrade (`m_ConstructSpeed`).
pub const DEFAULT_SPEED: i32 = 100;

/// Messages of the module (`PropertyConstructBuilding_OnCommand` `0x0050DC40`, `..._Query`
/// `0x0050D800`).
pub mod msg {
    /// `PMSG_CONSTRUCT_BUILDING {building}`: accept the order.
    pub const CONSTRUCT: u32 = 0x101;
    /// `PMSG_CONSTRUCT_BUILDING_START`: arrived at the building and checked, begin construction.
    pub const START: u32 = 0x201;
    /// `PMSG_CONSTRUCT_BUILDING_ABORT`: abort (`Stop`).
    pub const ABORT: u32 = 0x301;
    /// `PMSG_CONSTRUCT_BUILDING_CHECKPOS`: walk has reached the destination.
    pub const ARRIVED: u32 = 0x401;
    /// `PMSG_CONSTRUCT_BUILDING_GOTO`: check passed, start walking.
    pub const GOTO: u32 = 0x501;
    /// `{v}`: set the construction speed (speed upgrade 4451: 300 or 100).
    pub const SET_SPEED: u32 = 0x601;
    /// `{-1, _}` -> next own construction site `{edge distance, ID}`; otherwise `{_, ID}` as an order.
    pub const FIND_NEAREST_SITE: u32 = 0x701;
    /// Throw step (pioneers, dead).
    pub const THROW_STEP: u32 = 0x801;
    /// Timer of the hammer sound.
    pub const HAMMER_SOUND: u32 = 0x901;
}

/// Values of `m_Status`.
pub mod state {
    pub const IDLE: i32 = 0;
    pub const STARTED: i32 = 1;
    pub const BUILDING: i32 = 2;
    pub const ARRIVED: i32 = 3;
    pub const WALKING: i32 = 4;
    pub const REPAIRING: i32 = 5;
    pub const TARGET_DONE: i32 = 6;
}

/// Messages to other modules and objects.
mod other {
    /// Walk: check the position (`PMSG_PTTOPT_CHECKPOS`).
    pub const WALK_CHECKPOS: u32 = 0x900;
    /// Walk: walk to an object (`PMSG_PTTOPT_FINDTO`).
    pub const WALK_FINDTO: u32 = 0x200;
    /// Walk: walk to a unit (siege engine, dead).
    pub const WALK_FINDTO_UNIT: u32 = 0x400;
    /// Walk: stop (without an argument: abort).
    pub const WALK_STOP: u32 = 0xC00;
    /// Building: build step `{speed, answer}`.
    pub const BUILD_TICK: u32 = 0x203;
    /// Siege engine (module `0x2E`, without a class): build step, parking lot, parking lot free.
    pub const SIEGE_BUILD: u32 = 0x1A2E;
    pub const SIEGE_PARKING: u32 = 0x1B2E;
    pub const SIEGE_RELEASE: u32 = 0x1C2E;
    /// Module `0x32` (without a class): build step.
    pub const MULTIPART_BUILD: u32 = 0x1432;
    /// Follow-up work after construction: harvesting (module 12) or gold mining (module 7).
    pub const HARVEST: u32 = 0x10C;
    pub const MINE_GOLD: u32 = 0x107;
    /// Report an idle worker (feedback message of the commands).
    pub const IDLE_WORKER: u32 = 0x87F;
}

/// State of `CPropertyConstructBuilding` (`0x9C` bytes; fields in the order of
/// `PropertyConstructBuilding_SerializeFields` `0x0050DD30`, initial values from the constructor
/// `0x0050CF00`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Construct {
    /// `m_BuildingHID` (`+0x78`): target; initial value ID 0 (not -1, quirk), after `Stop` -1.
    pub building: ObjectId,
    /// `m_ConstructAnim` (`+0x70`, VALUEID 20001).
    pub construct_anim: i32,
    /// `m_ConstructSpeed` (`+0x7C`): construction speed, `0x601`.
    pub speed: i32,
    /// `m_Status` (`+0x74`).
    pub status: i32,
    /// `m_ThrowAnim` (`+0x80`, VALUEID 20003).
    pub throw_anim: i32,
    /// `m_BuildingPos` (`+0x88`): grid position **of the building** at the start of construction; if the
    /// building moves, construction ends (with success, quirk).
    pub building_pos: FxVec3,
    /// `m_SiegeengineParkinglot` (`+0x84`): parking lot at a siege engine (-1 = none).
    pub parking: i32,
    /// `m_TotalAnimTime` (`+0x98`): interval of the hammer sounds (time marker 1 of the construction animation, ms).
    /// Not initialized in the original (memory content, but saved); 0 here (deviation from the original).
    pub total_anim_time: i32,
}

impl Default for Construct {
    /// `PropertyConstructBuilding_Construct` (`0x0050CF00`).
    fn default() -> Self {
        Self {
            building: ObjectId(0),
            construct_anim: 0,
            speed: DEFAULT_SPEED,
            status: state::IDLE,
            throw_anim: 0,
            building_pos: FxVec3::ZERO,
            parking: -1,
            total_anim_time: 0,
        }
    }
}

pub(crate) fn construct<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Construct> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Construct(c) => Some(&**c),
        _ => None,
    }
}

pub(crate) fn construct_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Construct> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Construct(c) => Some(&mut **c),
        _ => None,
    }
}

fn set_state(sim: &mut Sim, id: ObjectId, s: i32) {
    if let Some(c) = construct_mut(sim, id) {
        c.status = s;
    }
}

fn set_constructing(sim: &mut Sim, id: ObjectId, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(CONSTRUCTING, on);
    }
}

fn has_flags(sim: &Sim, id: ObjectId, x: u32) -> bool {
    sim.module(id, INDEX).is_some_and(|m| m.base.has_flags(x))
}

/// Envelope to the own object with fault and success message (target -1 = own object), as
/// the original builds it on the stack.
fn envelope(msg: Message, fault: Option<Message>, success: Option<Message>) -> Envelope {
    Envelope {
        msg,
        success,
        fault,
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    }
}

/// `GameObject_SetAnimation(anim, 128.0, 0, 0, 0)` (`0x00466780`).
fn set_animation(sim: &mut Sim, id: ObjectId, anim: i32) {
    if let Some(o) = sim.object_mut(id) {
        o.set_animation_speed(anim, 0, 128.0, false, false, false);
    }
}

/// `GameObject_GetAnimLabelTime(anim, label)` (`0x004669D0`) of the worker.
fn anim_label_time(sim: &Sim, id: ObjectId, anim: i32, label: i32) -> i32 {
    sim.object(id)
        .map_or(0, |o| sim.rules.anim_label_time_at(o.guid, anim, label))
}

fn post_delayed(sim: &mut Sim, id: ObjectId, delay: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(delay, msg, false, GROUP);
    }
}

/// `PropertyConstructBuilding_Init` (`0x0050CFF0`, slot 9): flags (20002), construction and throw animation
/// (20001, 20003). `ReadParams` (slot 4) is empty: a variant switch changes nothing.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = value(20002) as u32;
    if let ModuleKind::Construct(c) = &mut m.kind {
        c.construct_anim = value(20001);
        c.throw_anim = value(20003);
    }
}

/// `PropertyConstructBuilding_Query` (`0x0050D800`, slot 7): `0x901` is the hammer sound timer
/// (return value 0); everything else goes without feedback messages to [`on_command`], `true` -> 1, `false` -> -1.
/// Answers (`0x701`) are then in the arguments.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    if m.code == msg::HAMMER_SOUND {
        return on_hammer_sound_timer(sim, id, m);
    }
    let mut env = Envelope::bare(std::mem::replace(m, Message::new(0, &[])));
    let ok = on_command(sim, id, &mut env);
    *m = env.msg;
    if ok { 1 } else { -1 }
}

/// `OnMessage` (slot 12) = [`on_command`].
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let mut env = env.clone();
    on_command(sim, id, &mut env)
}

/// `PropertyConstructBuilding_OnCommand` (`0x0050DC40`, slot 12); unknown codes `false`.
pub fn on_command(sim: &mut Sim, id: ObjectId, env: &mut Envelope) -> bool {
    match env.msg.code {
        msg::CONSTRUCT => on_start_building(sim, id, env),
        msg::START => on_msg_start_building(sim, id),
        msg::ABORT => abort(sim, id),
        msg::ARRIVED => {
            if construct(sim, id).is_some_and(|c| c.status != state::IDLE) {
                set_state(sim, id, state::ARRIVED);
            }
            true
        }
        msg::GOTO => {
            set_state(sim, id, state::STARTED);
            set_constructing(sim, id, true);
            true
        }
        msg::SET_SPEED => {
            let v = env.msg.arg(0);
            if let Some(c) = construct_mut(sim, id) {
                c.speed = v;
            }
            true
        }
        msg::FIND_NEAREST_SITE => on_find_nearest_site(sim, id, &mut env.msg),
        msg::THROW_STEP => on_throw_step(sim, id),
        _ => false,
    }
}

/// `PropertyConstructBuilding_OnStartBuilding` (`0x0050D0F0`, `0x101 {building}`): take over the feedback
/// messages from the envelope (delete missing ones), remember the target, state 4 and synchronously the
/// position check of walk (`0x900`, fault `0x301`, success `0x501`). A pioneer at a
/// building that is not a siege target (stub `FUN_004527C0` always returns `false`) aborts -
/// there are no pioneers in the shipped data.
fn on_start_building(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    let target = ObjectId::from_i32(env.msg.arg(0));
    if let Some(c) = construct_mut(sim, id) {
        c.building = target;
    }
    if sim.object(target).is_some() {
        let siege_target = false;
        if siege_target != has_flags(sim, id, ISPIONEER) {
            log::debug!("Construction worker {id:?}: pioneer at the wrong target {target:?}");
            abort(sim, id);
            return false;
        }
    }
    // Event bit 5 off (`0x0050D158`), even if the building is missing.
    crate::world::set_object_event_bit(sim, id, 5, false);
    set_state(sim, id, state::WALKING);
    let check = envelope(
        Message::new(other::WALK_CHECKPOS, &[]),
        Some(Message::new(msg::ABORT, &[])),
        Some(Message::new(msg::GOTO, &[])),
    );
    dispatch_command(sim, id, &check);
    true
}

/// `PropertyConstructBuilding_GotoBuilding` (`0x0050D1F0`, state 1): walk to the building - with a
/// building (or `0x32`) module FINDTO `{building, -1, 1}` (walk chooses a docking place), otherwise
/// the dead branch of a siege engine asks for a parking lot. State 4, walk reports
/// arrival `0x401` or fault `0x301`. If the building is missing: `Stop`.
fn goto_building(sim: &mut Sim, id: ObjectId) {
    let Some(target) = construct(sim, id).map(|c| c.building) else {
        return;
    };
    let Some(b) = sim.object(target) else {
        log::debug!("Construction worker {id:?}: building {target:?} missing");
        abort(sim, id);
        return;
    };
    let m = if b.has_module(3, false) || b.has_module(0x32, false) {
        Message::new(other::WALK_FINDTO, &[target.as_i32(), -1, 1])
    } else {
        let (gx, gz) = sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
        let mut q = Message::new(other::SIEGE_PARKING, &[1, gx, gz]);
        let r = send_mut(sim, target, &mut q);
        if r < 1 {
            Message::new(msg::ABORT, &[])
        } else {
            if let Some(c) = construct_mut(sim, id) {
                c.parking = r;
            }
            Message::new(
                other::WALK_FINDTO_UNIT,
                &[target.as_i32(), 0, -7, 0, -1, 1, q.arg(0), q.arg(1)],
            )
        }
    };
    set_state(sim, id, state::WALKING);
    let go = envelope(
        m,
        Some(Message::new(msg::ABORT, &[])),
        Some(Message::new(msg::ARRIVED, &[])),
    );
    dispatch_command(sim, id, &go);
}

/// `PropertyConstructBuilding_OnMsgStartBuilding` (`0x0050D850`, `0x201`): only in state 1. A
/// finished target is repaired (5), otherwise built (2); remember the building position, construction animation, first
/// hammer sound after time marker 2, then at the interval of time marker 1. The throw branch of the pioneers
/// (target with `TRACK`) is dead.
fn on_msg_start_building(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(c) = construct(sim, id) else {
        return true;
    };
    if c.status != state::STARTED {
        log::debug!("Construction worker {id:?}: OnMsgStartBuilding(): unexpected command");
        return true;
    }
    let (target, construct_anim, throw_anim) = (c.building, c.construct_anim, c.throw_anim);
    let exists = sim.object(target).is_some();
    if has_flags(sim, id, ISPIONEER)
        && sim.object(target).is_some_and(|b| {
            b.has_module(3, false)
                && b.module(3)
                    .is_some_and(|m| m.base.has_flags(building_flags::TRACK))
        })
    {
        set_state(sim, id, state::WALKING);
        set_animation(sim, id, throw_anim);
        let delay = anim_label_time(sim, id, throw_anim, 1);
        post_delayed(sim, id, delay, Message::new(msg::THROW_STEP, &[]));
        return true;
    }
    let repair = exists && matches_class(sim, target, building_flags::CONSTRUCTED_FINISHED);
    set_state(
        sim,
        id,
        if repair {
            state::REPAIRING
        } else {
            state::BUILDING
        },
    );
    if let Some(grid) = sim.object(target).map(|b| b.grid)
        && let Some(c) = construct_mut(sim, id)
    {
        c.building_pos = grid;
    }
    set_animation(sim, id, construct_anim);
    let first = anim_label_time(sim, id, construct_anim, 2);
    let total = anim_label_time(sim, id, construct_anim, 1);
    if let Some(c) = construct_mut(sim, id) {
        c.total_anim_time = total;
    }
    post_delayed(sim, id, first, Message::new(msg::HAMMER_SOUND, &[]));
    true
}

/// `PropertyConstructBuilding_OnHammerSoundTimer` (`0x0050D790`, `0x901`): if the worker stands
/// (no `MOVE`), hammer sound as an event; if the construction order is over (state bit `0x20` off),
/// the chain ends. Otherwise the same message again after `m_TotalAnimTime`. Return value 0.
fn on_hammer_sound_timer(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let st = sim.object(id).map_or(0, |o| o.status);
    if st & status::MOVE == 0 {
        sim.state.events.push(SimEvent::Sound {
            object: id,
            sound: SFX_HAMMER,
        });
        if st & CONSTRUCTING == 0 {
            return 0;
        }
    }
    let delay = construct(sim, id).map_or(0, |c| c.total_anim_time);
    post_delayed(sim, id, delay, m.clone());
    0
}

/// `PropertyConstructBuilding_OnFindNearestSite` (`0x0050DA30`, `0x701`; without a
/// sender in the shipped data). `args[0] = -1`: nearest own unfinished building (filter `0x4114`, class
/// `0x403` negated, module 3) around the grid position within the radius sight/4 tiles (attribute 18,
/// division toward 0), edge distance via the hard radius -> `args = {edge distance, ID}`, `true`.
/// Otherwise enqueue the order `0x101 {args[1]}` with feedback messages `0x87F` (with abort).
fn on_find_nearest_site(sim: &mut Sim, id: ObjectId, m: &mut Message) -> bool {
    if m.arg(0) != -1 {
        let order = envelope(
            Message::new(msg::CONSTRUCT, &[m.arg(1)]),
            Some(Message::new(other::IDLE_WORKER, &[])),
            Some(Message::new(other::IDLE_WORKER, &[])),
        );
        return post_command(sim, id, order, true);
    }
    let Some(o) = sim.object(id) else {
        return false;
    };
    let (pos, hard) = ((o.grid.x.0, o.grid.z.0), o.hard_radius);
    let radius = (sim.attribute(id, 18) / 4).wrapping_shl(12);
    let filter = site_filter();
    let player = i32::from(id.slot());
    let Some(site) = find_nearest_matching(sim, pos, &filter, radius, player, hard) else {
        return false;
    };
    let d = edge_distance_to_object(sim, site, id);
    m.set_arg(0, d);
    m.set_arg(1, site.as_i32());
    true
}

/// Filter "own construction site" (`0x4114` = class negated, own list, AND, module): module 3 without
/// `CONSTRUCTED_FINISHED`.
pub(crate) fn site_filter() -> MatchFilter {
    MatchFilter {
        mode: 0x4114,
        match_bits: building_flags::CONSTRUCTED_FINISHED,
        prop_id: 3,
        ..MatchFilter::constructed()
    }
}

/// `PropertyConstructBuilding_OnThrowStep` (`0x0050D390`, `0x801`, pioneers - dead): one
/// build step from a distance, then `Stop`.
fn on_throw_step(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(c) = construct(sim, id) else {
        return false;
    };
    let (target, speed) = (c.building, c.speed);
    if sim.object(target).is_none() {
        abort(sim, id);
        return false;
    }
    send(sim, target, &Message::new(other::BUILD_TICK, &[speed, 0]));
    abort(sim, id);
    true
}

/// `PropertyConstructBuilding_Stop` (`0x0050D030`, slot 11): remove the timers of group `0x80`;
/// if the worker is still walking to the building (state 4 with bit `0x20`), stop walk (`0xC00` without an
/// argument = abort; its fault message `0x301` calls `Stop` again, harmless); release the parking lot at the
/// siege engine; target -1, state 0, bit `0x20` off, send the fault message.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    let Some(c) = construct(sim, id) else {
        return true;
    };
    let (st, parking, target) = (c.status, c.parking, c.building);
    let walking = sim.object(id).is_some_and(|o| o.status & CONSTRUCTING != 0);
    if st == state::WALKING && walking {
        send(sim, id, &Message::new(other::WALK_STOP, &[]));
    }
    if parking != -1 {
        if sim.object(target).is_some() {
            send(sim, target, &Message::new(other::SIEGE_RELEASE, &[parking]));
        }
        if let Some(c) = construct_mut(sim, id) {
            c.parking = -1;
        }
    }
    if let Some(c) = construct_mut(sim, id) {
        c.building = ObjectId::NONE;
        c.status = state::IDLE;
    }
    set_constructing(sim, id, false);
    send_fault(sim, id, INDEX);
    true
}

/// `PropertyConstructBuilding_Tick` (`0x0050D420`, slot 13): one step of the state machine, then
/// the timers (`Module_UpdateEvents`). Quirk: if the building is missing in the building state, the
/// tick ends after `Stop` **without** `UpdateEvents`.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    let Some(c) = construct(sim, id) else {
        return;
    };
    let update = match c.status {
        state::BUILDING | state::REPAIRING | state::TARGET_DONE => build_step(sim, id),
        state::ARRIVED => {
            set_state(sim, id, state::STARTED);
            let check = envelope(
                Message::new(other::WALK_CHECKPOS, &[]),
                Some(Message::new(msg::ABORT, &[])),
                Some(Message::new(msg::START, &[])),
            );
            dispatch_command(sim, id, &check);
            true
        }
        state::STARTED => {
            goto_building(sim, id);
            true
        }
        state::WALKING => {
            check_target_while_walking(sim, id);
            true
        }
        _ => true,
    };
    if update {
        update_events(sim, id, INDEX, dt);
    }
}

/// State 4 with bit `0x20` (on the way): building missing or dead -> `Stop`; if it is finished and
/// undamaged -> state 6 (the next build step returns 0 or 2, walk continues - quirk).
fn check_target_while_walking(sim: &mut Sim, id: ObjectId) {
    if sim.object(id).is_none_or(|o| o.status & CONSTRUCTING == 0) {
        return;
    }
    let target = construct(sim, id).map_or(ObjectId::NONE, |c| c.building);
    match sim.object(target) {
        None => {
            abort(sim, id);
        }
        Some(b) if b.status & status::DIED != 0 => {
            abort(sim, id);
        }
        Some(b) => {
            if b.attributes.is_full(14)
                && matches_class(sim, target, building_flags::CONSTRUCTED_FINISHED)
            {
                set_state(sim, id, state::TARGET_DONE);
            }
        }
    }
}

/// Building states 2/5/6: per object tick **one** build step `0x203 {speed, 0}` to the building, as long as
/// it stands on the remembered grid position (otherwise answer 0, quirk). Answer 0: finished -
/// state 0, bit `0x20` off, timers removed; a field hands over to harvesting `0x10C`, a gold mine to
/// gold mining `0x107` (with the own feedback messages, which stay set), otherwise the
/// success message. Answer 2: like a new order to the construction site `args[1]` - without
/// feedback messages (they are lost, quirk). If the building is missing: `Stop`, return value `false`.
fn build_step(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(c) = construct(sim, id) else {
        return true;
    };
    let (target, speed, pos) = (c.building, c.speed, c.building_pos);
    let Some(b) = sim.object(target) else {
        log::debug!("Construction worker {id:?}: building {target:?} missing");
        abort(sim, id);
        return false;
    };
    let mut step = Message::new(other::BUILD_TICK, &[speed, 0]);
    let r = if pos == b.grid {
        if b.has_module(3, false) {
            send_mut(sim, target, &mut step)
        } else if b.has_module(0x2E, false) {
            send(sim, target, &Message::new(other::SIEGE_BUILD, &[speed]))
        } else if b.has_module(0x32, false) {
            send(
                sim,
                target,
                &Message::new(other::MULTIPART_BUILD, &[speed, 0]),
            )
        } else {
            0
        }
    } else {
        0
    };
    match r {
        0 => finish(sim, id, target),
        2 => {
            let next = Envelope::bare(Message::new(msg::CONSTRUCT, &[step.arg(1)]));
            on_start_building(sim, id, &next);
        }
        // 1 = continue building. The original knows only 0/1/2; -1 comes here from exception A7 paths
        // (message to a missing object or module) and acts like 1.
        _ => {}
    }
    true
}

/// End of a construction order (answer 0 of the build step).
fn finish(sim: &mut Sim, id: ObjectId, target: ObjectId) {
    set_state(sim, id, state::IDLE);
    set_constructing(sim, id, false);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    // Event bit 5 on (`0x0050D6B9`): new building finished, repair finished, target undamaged.
    crate::world::set_object_event_bit(sim, id, 5, true);
    let follow = if matches_class(sim, target, building_flags::FARM) {
        Some(other::HARVEST)
    } else if matches_class(sim, target, building_flags::GOLDMINE) {
        Some(other::MINE_GOLD)
    } else {
        None
    };
    match follow {
        Some(code) => {
            let (fault, success) = sim.module(id, INDEX).map_or((None, None), |m| {
                (m.base.fault.clone(), m.base.success.clone())
            });
            let env = envelope(Message::new(code, &[target.as_i32()]), fault, success);
            dispatch_command(sim, id, &env);
        }
        None => send_success(sim, id, INDEX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::building::{self, building as building_of, building_mut};
    use crate::module::walk;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::{object_tick, step};
    use crate::world::{ObjectDesc, create_object, desc_flags, set_direction, set_grid_position};

    /// Colonist with walk, construction (animation 5, flags 0, throw 0) and stand; house (150 wood, 20 s,
    /// 900 HP), field (flag `FARM`), observer without modules.
    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\nPROPID 1\nVALUEID 20001 6 1 5\nVALUEID 20002 6 1 0\nVALUEID 20003 6 1 7\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 1003\nNAME SPA_Feld\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 512\nGUID 1200\nNAME Beobachter\nVALUEID 100 6 1 0\n";

    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 48);
        r.defaults
            .insert(1100, [(16, 183), (18, 32)].into_iter().collect());
        r.defaults
            .insert(1001, [(11, 150), (13, 20), (14, 900)].into_iter().collect());
        r.defaults
            .insert(1003, [(13, 20), (14, 900)].into_iter().collect());
        // Construction animation 5: time marker 1 (repetition) 900 ms, time marker 2 (first sound) 400 ms.
        r.set_anim_labels(1100, 5, &[(1, 900), (2, 400)]);
        r
    }

    fn world(t: i32) -> f32 {
        t as f32 * 128.0 + 64.0
    }

    fn worker(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
        create_object(
            sim,
            &mut ObjectDesc::new(1100, 0, [world(tile.0), 0.0, world(tile.1)]),
        )
        .unwrap()
    }

    fn site(sim: &mut Sim, guid: u32, tile: (i32, i32)) -> ObjectId {
        create_object(
            sim,
            &mut ObjectDesc::new(guid, 0, [world(tile.0), 0.0, world(tile.1)]),
        )
        .unwrap()
    }

    fn observer(sim: &mut Sim) -> ObjectId {
        create_object(
            sim,
            &mut ObjectDesc::new(1200, 0, [world(40), 0.0, world(40)]),
        )
        .unwrap()
    }

    /// Order `0x101 {building}` like the command `0x02` (with abort), feedback messages to the
    /// observer: success 111, fault 222.
    fn order(sim: &mut Sim, w: ObjectId, b: ObjectId, obs: ObjectId) {
        let env = Envelope {
            msg: Message::new(msg::CONSTRUCT, &[b.as_i32()]),
            success: Some(Message::new(0xA7F, &[111])),
            fault: Some(Message::new(0xA7F, &[222])),
            success_target: obs,
            fault_target: obs,
        };
        post_command(sim, w, env, true);
    }

    fn reply(sim: &Sim, obs: ObjectId) -> u32 {
        sim.object(obs).unwrap().carrier.0
    }

    fn status_of(sim: &Sim, w: ObjectId) -> i32 {
        construct(sim, w).unwrap().status
    }

    #[test]
    fn workers_load_their_animations_and_start_idle() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = worker(&mut sim, (5, 5));
        let c = construct(&sim, w).unwrap();
        assert_eq!((c.construct_anim, c.throw_anim, c.speed), (5, 7, 100));
        assert_eq!((c.status, c.parking, c.total_anim_time), (0, -1, 0));
        // Initial value of the target: ID 0, not -1 (quirk of the constructor).
        assert_eq!(c.building, ObjectId(0));
        assert_eq!(sim.module(w, INDEX).unwrap().base.flags, 0);
    }

    #[test]
    fn commands_switch_states_and_unknown_ones_are_refused() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = worker(&mut sim, (5, 5));
        // 0x401 in state 0 stays 0.
        assert_eq!(send(&mut sim, w, &Message::new(msg::ARRIVED, &[])), 1);
        assert_eq!(status_of(&sim, w), state::IDLE);
        // 0x501: state 1 and state bit 0x20; then 0x401 -> 3.
        send(&mut sim, w, &Message::new(msg::GOTO, &[]));
        assert_eq!(status_of(&sim, w), state::STARTED);
        assert_ne!(sim.object(w).unwrap().status & CONSTRUCTING, 0);
        send(&mut sim, w, &Message::new(msg::ARRIVED, &[]));
        assert_eq!(status_of(&sim, w), state::ARRIVED);
        // 0x201 outside state 1: only a log entry.
        send(&mut sim, w, &Message::new(msg::START, &[]));
        assert_eq!(status_of(&sim, w), state::ARRIVED);
        send(&mut sim, w, &Message::new(msg::SET_SPEED, &[300]));
        assert_eq!(construct(&sim, w).unwrap().speed, 300);
        // Unknown: query -1, OnMessage false.
        assert_eq!(send(&mut sim, w, &Message::new(0xA01, &[])), -1);
        let env = Envelope::bare(Message::new(0xA01, &[]));
        assert_eq!(dispatch_command(&mut sim, w, &env), 0);
        // 0x301 = Stop: target -1, state 0, bit off.
        assert_eq!(send(&mut sim, w, &Message::new(msg::ABORT, &[])), 1);
        let c = construct(&sim, w).unwrap();
        assert_eq!((c.status, c.building), (state::IDLE, ObjectId::NONE));
        assert_eq!(sim.object(w).unwrap().status & CONSTRUCTING, 0);
    }

    /// Puts the worker on the docking place he would get at the building, with its viewing direction.
    fn place_on_dock(sim: &mut Sim, w: ObjectId, b: ObjectId) {
        let o = sim.object(w).unwrap();
        let layer = walk::walk(sim, w).unwrap().layer as i32;
        let mut m = Message::new(0xB03, &[o.grid.x.0, o.grid.z.0, w.as_i32(), layer]);
        assert!(send_mut(sim, b, &mut m) >= 0);
        send(sim, b, &Message::new(0xD03, &[w.as_i32()]));
        let y = sim.object(w).unwrap().grid.y;
        let p = FxVec3::new(crate::fx::Fx(m.arg(0)), y, crate::fx::Fx(m.arg(1)));
        set_grid_position(sim, w, p, true);
        set_direction(sim, w, m.arg(2) << 12);
    }

    #[test]
    fn a_worker_on_the_dock_takes_the_first_step_two_ticks_after_the_order() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = site(&mut sim, 1001, (20, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        place_on_dock(&mut sim, w, b);
        order(&mut sim, w, b, obs);
        let counter = |sim: &Sim| building_of(sim, b).unwrap().build_counter;
        // T0: order, check, walk (already in the target radius) -> state 3.
        object_tick(&mut sim, w, 100);
        assert_eq!(status_of(&sim, w), state::ARRIVED);
        assert_ne!(sim.object(w).unwrap().status & CONSTRUCTING, 0);
        assert_eq!(counter(&sim), 2000);
        // T1: check -> 0x201 -> build; construction animation and first hammer sound after time marker 2.
        object_tick(&mut sim, w, 100);
        assert_eq!(status_of(&sim, w), state::BUILDING);
        assert_eq!(counter(&sim), 2000);
        let c = construct(&sim, w).unwrap();
        assert_eq!(c.building_pos, sim.object(b).unwrap().grid);
        assert_eq!(c.total_anim_time, 900);
        assert_eq!(sim.object(w).unwrap().anim.wanted, 5);
        let events = &sim.module(w, INDEX).unwrap().base.events;
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].msg.code, events[0].b), (msg::HAMMER_SOUND, 400));
        // T2: first build step.
        object_tick(&mut sim, w, 100);
        assert_eq!(counter(&sim), 1990);
        assert_eq!(reply(&sim, obs), ObjectId::NONE.0);
    }

    #[test]
    fn a_worker_walks_to_the_site_builds_it_and_reports_success() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let (b, w, obs) = {
            let mut sim = Sim::new(&mut state, &rules);
            let b = site(&mut sim, 1001, (20, 20));
            let w = worker(&mut sim, (8, 20));
            let obs = observer(&mut sim);
            order(&mut sim, w, b, obs);
            (b, w, obs)
        };
        let counter = |state: &mut SimState| {
            building_of(&Sim::new(state, &rules), b)
                .unwrap()
                .build_counter
        };
        let mut steps = 0;
        while counter(&mut state) == 2000 {
            step(&mut state, &rules, &[]);
            steps += 1;
            assert!(steps < 400, "no build step");
        }
        // From here on one step per object tick (every two engine steps): 200 steps, finished
        // in the 199th tick after the first.
        let mut ticks = 0;
        let mut sounds = 0;
        while counter(&mut state) > 0 {
            let events = step(&mut state, &rules, &[]);
            let events2 = step(&mut state, &rules, &[]);
            sounds += events
                .iter()
                .chain(&events2)
                .filter(
                    |e| matches!(e, SimEvent::Sound { object, sound: SFX_HAMMER } if *object == w),
                )
                .count();
            ticks += 1;
            assert!(ticks < 400, "not finished");
        }
        assert_eq!(ticks, 199);
        // Hammer sound every 900 ms (the first after 400 ms) during construction.
        assert!((20..=23).contains(&sounds), "{sounds} sounds");
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(reply(&sim, obs), 111);
        let c = construct(&sim, w).unwrap();
        assert_eq!(c.status, state::IDLE);
        assert_eq!(sim.object(w).unwrap().status & CONSTRUCTING, 0);
        let o = sim.object(b).unwrap();
        assert_eq!(o.attributes.current_value(14, 0), 900);
        assert!(
            sim.module(b, building::INDEX)
                .unwrap()
                .base
                .has_flags(0x403)
        );
        // Timers of group 0x80 deleted.
        let base = &sim.module(w, INDEX).unwrap().base;
        assert!(base.timers_ws.is_empty());
    }

    // QUIRK W-70: kept
    #[test]
    fn finishing_moves_on_to_the_next_site_and_loses_the_replies() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let first = site(&mut sim, 1001, (20, 20));
        let second = site(&mut sim, 1001, (26, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        place_on_dock(&mut sim, w, first);
        order(&mut sim, w, first, obs);
        object_tick(&mut sim, w, 100);
        object_tick(&mut sim, w, 100);
        // Almost finished: the next step completes it.
        building_mut(&mut sim, first).unwrap().build_counter = 10;
        object_tick(&mut sim, w, 100);
        // Answer 2: new order to the second construction site, check and walk as with 0x101.
        let c = construct(&sim, w).unwrap();
        assert_eq!(c.building, second);
        assert_eq!(c.status, state::STARTED);
        let base = &sim.module(w, INDEX).unwrap().base;
        assert!(base.success.is_none() && base.fault.is_none());
        assert_eq!(reply(&sim, obs), ObjectId::NONE.0);
    }

    // QUIRK W-71: kept
    #[test]
    fn a_finished_field_hands_over_to_harvesting_and_keeps_the_replies() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let field = site(&mut sim, 1003, (20, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        place_on_dock(&mut sim, w, field);
        order(&mut sim, w, field, obs);
        object_tick(&mut sim, w, 100);
        object_tick(&mut sim, w, 100);
        building_mut(&mut sim, field).unwrap().build_counter = 10;
        object_tick(&mut sim, w, 100);
        // Field finished: no success, harvesting 0x10C goes to the (still idle) module 12; the own
        // feedback messages stay set.
        assert_eq!(status_of(&sim, w), state::IDLE);
        assert_eq!(reply(&sim, obs), ObjectId::NONE.0);
        let base = &sim.module(w, INDEX).unwrap().base;
        assert_eq!(base.success, Some(Message::new(0xA7F, &[111])));
        assert_eq!(base.fault, Some(Message::new(0xA7F, &[222])));
    }

    // QUIRK W-112: kept
    #[test]
    fn a_moved_building_ends_the_job_with_success() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = site(&mut sim, 1001, (20, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        place_on_dock(&mut sim, w, b);
        order(&mut sim, w, b, obs);
        object_tick(&mut sim, w, 100);
        object_tick(&mut sim, w, 100);
        if let Some(c) = construct_mut(&mut sim, w) {
            c.building_pos.x.0 += 1;
        }
        object_tick(&mut sim, w, 100);
        assert_eq!(building_of(&sim, b).unwrap().build_counter, 2000);
        assert_eq!(status_of(&sim, w), state::IDLE);
        assert_eq!(reply(&sim, obs), 111);
    }

    #[test]
    fn a_vanished_site_stops_the_worker_without_updating_its_timers() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = site(&mut sim, 1001, (20, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        place_on_dock(&mut sim, w, b);
        order(&mut sim, w, b, obs);
        object_tick(&mut sim, w, 100);
        object_tick(&mut sim, w, 100);
        crate::world::destroy_object(&mut sim, b);
        crate::module::tick(&mut sim, w, INDEX, 100);
        assert_eq!(reply(&sim, obs), 222);
        assert_eq!(status_of(&sim, w), state::IDLE);
        // Stop has noted group 0x80; without UpdateEvents the deletion stays pending.
        let base = &sim.module(w, INDEX).unwrap().base;
        assert_eq!(base.clear_events, 0x80);
        assert_eq!(base.events.len(), 1);
    }

    #[test]
    fn aborting_while_walking_stops_walk_and_reports_the_fault_once() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let (b, w, obs) = {
            let mut sim = Sim::new(&mut state, &rules);
            let b = site(&mut sim, 1001, (20, 20));
            let w = worker(&mut sim, (4, 20));
            let obs = observer(&mut sim);
            order(&mut sim, w, b, obs);
            (b, w, obs)
        };
        for _ in 0..8 {
            step(&mut state, &rules, &[]);
        }
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(status_of(&sim, w), state::WALKING);
        assert_ne!(sim.object(w).unwrap().status & status::MOVE, 0);
        assert!(abort(&mut sim, w));
        assert_eq!(reply(&sim, obs), 222);
        assert_eq!(
            sim.object(w).unwrap().status & (status::MOVE | CONSTRUCTING),
            0
        );
        let c = construct(&sim, w).unwrap();
        assert_eq!((c.status, c.building), (state::IDLE, ObjectId::NONE));
        assert!(sim.module(w, INDEX).unwrap().base.fault.is_none());
        let _ = b;
    }

    // QUIRK W-113: kept
    #[test]
    fn a_finished_healthy_target_ends_the_walk_job_early() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let (w, obs) = {
            let mut sim = Sim::new(&mut state, &rules);
            let mut desc = ObjectDesc::new(1001, 0, [world(20), 0.0, world(20)]);
            desc.flags |= desc_flags::PLACED;
            let b = create_object(&mut sim, &mut desc).unwrap();
            let w = worker(&mut sim, (4, 20));
            let obs = observer(&mut sim);
            order(&mut sim, w, b, obs);
            (w, obs)
        };
        // State 6 while walking; the next step is 0 (no other construction site) ->
        // success, walk continues.
        let mut steps = 0;
        while state.object(obs, false).unwrap().carrier.0 != 111 {
            step(&mut state, &rules, &[]);
            steps += 1;
            assert!(steps < 20, "no end");
        }
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(status_of(&sim, w), state::IDLE);
        assert_ne!(sim.object(w).unwrap().status & status::MOVE, 0);
    }

    #[test]
    fn the_hammer_timer_sounds_only_standing_and_repeats_while_constructing() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = worker(&mut sim, (5, 5));
        sim.state.events.clear();
        construct_mut(&mut sim, w).unwrap().total_anim_time = 900;
        let hammer = Message::new(msg::HAMMER_SOUND, &[]);
        let sound = SimEvent::Sound {
            object: w,
            sound: SFX_HAMMER,
        };
        // Standing, construction order running: sound and again after 900 ms.
        sim.object_mut(w)
            .unwrap()
            .set_status_flags(CONSTRUCTING, true);
        assert_eq!(send(&mut sim, w, &hammer), 0);
        assert_eq!(sim.state.events, std::slice::from_ref(&sound));
        let ws = &sim.module(w, INDEX).unwrap().base.timers_ws;
        assert_eq!((ws.len(), ws[0].b, ws[0].group()), (1, 900, 0x80));
        // Walking: no sound, but again.
        sim.state.events.clear();
        sim.object_mut(w)
            .unwrap()
            .set_status_flags(status::MOVE, true);
        send(&mut sim, w, &hammer);
        assert!(sim.state.events.is_empty());
        assert_eq!(sim.module(w, INDEX).unwrap().base.timers_ws.len(), 2);
        // Standing without an order: last sound, no repetition.
        sim.object_mut(w).unwrap().status = 0;
        send(&mut sim, w, &hammer);
        assert_eq!(sim.state.events, [sound]);
        assert_eq!(sim.module(w, INDEX).unwrap().base.timers_ws.len(), 2);
    }

    #[test]
    fn find_nearest_site_answers_in_the_arguments_and_then_queues_the_order() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = worker(&mut sim, (10, 10));
        // Sight 32 -> 8 tiles: the construction site 6 tiles away, not the foreign one and not the one
        // too far away.
        let near = site(&mut sim, 1001, (16, 10));
        create_object(
            &mut sim,
            &mut ObjectDesc::new(1001, 1, [world(13), 0.0, world(10)]),
        )
        .unwrap();
        site(&mut sim, 1001, (10, 30));
        let mut m = Message::new(msg::FIND_NEAREST_SITE, &[-1, 0]);
        assert_eq!(send_mut(&mut sim, w, &mut m), 1);
        let d = edge_distance_to_object(&mut sim, near, w);
        assert_eq!(m.args, [d, near.as_i32()]);
        // Without a construction site in sight: -1, arguments unchanged.
        let far = worker(&mut sim, (40, 40));
        let mut none = Message::new(msg::FIND_NEAREST_SITE, &[-1, 0]);
        assert_eq!(send_mut(&mut sim, far, &mut none), -1);
        assert_eq!(none.args, [-1, 0]);
        // Filled: order with abort into the FIFO.
        assert_eq!(send_mut(&mut sim, w, &mut m), 1);
        let fifo = &sim.object(w).unwrap().fifo;
        assert_eq!(fifo.len(), 1);
        assert!(fifo[0].aborts());
        assert_eq!(fifo[0].code(), msg::CONSTRUCT);
    }

    #[test]
    fn the_governor_speed_finishes_a_house_in_67_steps() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = site(&mut sim, 1001, (20, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        send(&mut sim, w, &Message::new(msg::SET_SPEED, &[300]));
        place_on_dock(&mut sim, w, b);
        order(&mut sim, w, b, obs);
        object_tick(&mut sim, w, 100);
        object_tick(&mut sim, w, 100);
        let mut steps = 0;
        while reply(&sim, obs) != 111 {
            object_tick(&mut sim, w, 100);
            steps += 1;
            assert!(steps < 100);
        }
        assert_eq!(steps, 67);
    }

    // QUIRK E-187: kept
    #[test]
    fn builder_bit_5_clears_on_start_and_sets_on_finish() {
        // `OnStartBuilding` `0x0050D158` (off) and `Tick` `0x0050D6B9` (on) - on the construction worker, not
        // on the building; likewise at the end of a repair.
        use crate::world::{object_event_bits, set_object_event_bit};
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = site(&mut sim, 1001, (20, 20));
        let w = worker(&mut sim, (14, 20));
        let obs = observer(&mut sim);
        place_on_dock(&mut sim, w, b);
        set_object_event_bit(&mut sim, w, 5, true);
        order(&mut sim, w, b, obs);
        object_tick(&mut sim, w, 100);
        assert_eq!(object_event_bits(&sim, w), 0, "start: bit 5 off");
        object_tick(&mut sim, w, 100);
        building_mut(&mut sim, b).unwrap().build_counter = 10;
        object_tick(&mut sim, w, 100);
        assert_eq!(reply(&sim, obs), 111);
        assert_eq!(object_event_bits(&sim, w), 1 << 5, "finished: bit 5 on");
        assert_eq!(object_event_bits(&sim, b), 0, "not on the building");

        // Repair of a finished, damaged house: start off, end on.
        let mut desc = ObjectDesc::new(1001, 0, [world(30), 0.0, world(20)]);
        desc.flags |= desc_flags::PLACED;
        let house = create_object(&mut sim, &mut desc).unwrap();
        sim.object_mut(house)
            .unwrap()
            .attributes
            .add_to_current_value(14, -50, 0);
        assert!(!sim.object(house).unwrap().attributes.is_full(14));
        let w2 = worker(&mut sim, (24, 20));
        place_on_dock(&mut sim, w2, house);
        set_object_event_bit(&mut sim, w2, 5, true);
        let obs2 = observer(&mut sim);
        order(&mut sim, w2, house, obs2);
        object_tick(&mut sim, w2, 100);
        assert_eq!(object_event_bits(&sim, w2), 0, "repair start: bit 5 off");
        let mut ticks = 0;
        while reply(&sim, obs2) != 111 {
            assert_eq!(object_event_bits(&sim, w2), 0);
            object_tick(&mut sim, w2, 100);
            ticks += 1;
            assert!(ticks < 400, "repair not finished");
        }
        assert_eq!(status_of(&sim, w2), state::IDLE);
        assert!(sim.object(house).unwrap().attributes.is_full(14));
        assert_eq!(
            object_event_bits(&sim, w2),
            1 << 5,
            "repair finished: bit 5 on"
        );
        assert_eq!(object_event_bits(&sim, house), 0);
    }
}
