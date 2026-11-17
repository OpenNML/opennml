//! Module 23 `CPropertyQuarter` (re/garrison-and-commands): units move into and out of a
//! container (buildings and vehicles). Capacity = attribute `0x17`, each
//! unit takes one place. Quartered units carry `QUARTERED` (`0x100`), follow the carrier
//! in the object tick (`+0x6E0`), are exempt from collision, invisible to filters and
//! are not drawn; they stay in their tick group. Everything runs via `Query`, there is no
//! `OnMessage` (base: envelopes fizzle with `true`).
//!
//! Mounted units and canoe occupants dismount via riding before moving in ([`accept_unit`]).
//! Mobile containers (containers with a walk module, ships) lead the meeting point `0x1E17`/`0x1F17`/
//! `0x2017` in [`super::boarding`] (re/ships 3.1-3.4); their fields
//! (`m_WaitingList`, `m_Status`) are saved, the meeting point is in the savegame, deviating
//! from the original (W-197).

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::{find_nearest_matching, fixed_pos_distance, query_cost_at};
use crate::filter::matches_class;
use crate::footprint::helper_node_map_position;
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, notify_attacked_by, post_message, send, send_mut};
use crate::object::status;
use crate::path::grid::COST_FREE;
use crate::path::{LAYER_MASKS, layer_from_move_mask};
use crate::sim::Sim;
use crate::world::set_grid_position;

use super::ModuleKind;
use super::boarding;
use super::events::update_events;

pub const INDEX: usize = 23;
/// Group of the quarters timers.
const GROUP: u8 = 0x80;
/// Intake capacity (attribute `0x17`, config 23).
const CAPACITY: u32 = 0x17;
/// Space needed per unit (`0x004EC9A0`: `MOV EAX,1`).
const UNIT_SIZE: i32 = 1;
/// Start value of the counter for killing enemy occupants (`m_EnemyDies`, ms).
pub const ENEMY_DIES_START: i32 = 5000;
/// Base radius of the spiral search around a non-building container (`[0x0067134C]`, one tile).
const SPIRAL_BASE_RADIUS: i32 = 4096;
/// Search radius in front of a building (`Fixed_FromInt(10)`, ten tiles).
const SPIRAL_BUILDING_RADIUS: i32 = 10 << 12;
/// Cost limit of a free point (`[0x006719EC]` = `Fixed_FromFloat(0.01)` = 40).
pub const FREE_COST_LIMIT: i32 = 40;

/// Module indices that the quarters query.
mod modules {
    pub const WALK: usize = 0;
    pub const BUILDING: usize = 3;
    pub const FIGHT_FAR: usize = 0x13;
    pub const HEALING: usize = 0x1D;
    pub const BOND_SLAVE: usize = 0x2D;
    pub const SIEGE_ENGINE: usize = 0x2E;
}

/// Quarters flags (`MID_PROP_QUARTER_*`, `m_MatchInt`) with the module index in the low byte.
pub mod flags {
    /// `FREEFORQUARTER`: places free.
    pub const FREE_FOR_QUARTER: u32 = 0x117;
    /// `QUARTERED`: has occupants.
    pub const QUARTERED: u32 = 0x217;
    /// `USE_SECRET_PASSAGE`: letting out via another building (command `0x76`, dead in the shipped data).
    pub const USE_SECRET_PASSAGE: u32 = 0x417;
    /// Class "finished building" (`CONSTRUCTED_FINISHED` of the building module).
    pub(super) const FINISHED_BUILDING: u32 = 0x403;
}

/// Queries of the quarters (`PropertyQuarter_Query` `0x00521010`).
pub mod msg {
    /// `{unit}` (or `{unit, delay}`): move in.
    pub const ENTER: u32 = 0x1617;
    /// `{unit}` (or `{unit, 1}` second phase): let out one unit.
    pub const UNQUARTER_ONE: u32 = 0x1717;
    /// `{→ids…}`: occupants in order of entry, `argc` = count.
    pub const LIST_OCCUPANTS: u32 = 0x1817;
    /// Let out all.
    pub const UNQUARTER_ALL: u32 = 0x1917;
    /// End of construction of the building: places free.
    pub const BUILDING_FINISHED: u32 = 0x1A17;
    /// `{target}`: the garrison attacks.
    pub const GARRISON_ATTACK: u32 = 0x1B17;
    /// -> garrison not empty (`targets.ini` `CALLMETHOD 2`).
    pub const HAS_GARRISON: u32 = 0x1C17;
    /// `{mode}`: behavior of the garrison.
    pub const SET_GARRISON_BEHAVIOR: u32 = 0x1D17;
    /// Container is destroyed.
    pub const CONTAINER_DESTROYED: u32 = 0x2117;
    /// -> occupied places.
    pub const OCCUPANT_COUNT: u32 = 0x2217;
    /// `{hid}`: collision partner boards (walk `PropertyWalk_IsQuarterPartner`).
    pub const TOUCH_PARTNER: u32 = 0x2317;
    /// `{old, new}`: replace the ID of an occupant (change of owner of the occupant).
    pub const REPLACE_OCCUPANT: u32 = 0x2417;
    /// `{x, z, ids…}`: set the meeting point of mobile containers, `ids` to the waiting list (re/ships 2.1).
    pub const SET_MEETING_POINT: u32 = 0x1E17;
    /// Container has reached the meeting point: take in the waiting (re/ships 2.1).
    pub const CONTAINER_ARRIVED: u32 = 0x1F17;
    /// `{→x, →z}`: read the meeting point, 1 only with status 1 (re/ships 2.1).
    pub const GET_MEETING_POINT: u32 = 0x2017;
}

/// State of `CPropertyQuarter` (`0xFC` bytes, `PropertyQuarter_Construct` `0x0051FD60`), fields
/// in the order of the serialization (`PropertyQuarter_SerializeFields` `0x00521130`). The
/// meeting point of mobile containers (`+0xD4/+0xD8`) is neither saved nor set in the
/// constructor in the original; the reimplementation keeps it in the savegame (W-197).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarter {
    /// `m_MaxQuarterCount` (`+0x70`): capacity.
    pub max: i32,
    /// `m_CurQuarterCount` (`+0x74`): occupied places.
    pub cur: i32,
    /// `m_QuarterList` (`+0x78`): own occupants in order of entry.
    pub occupants: Vec<ObjectId>,
    /// `m_WaitingList` (`+0xB0`): units waiting at the meeting point (mobile container).
    pub waiting: Vec<ObjectId>,
    /// `m_Garison` (`+0x94`): occupants with ranged combat (module `0x13`), only in buildings.
    pub garrison: Vec<ObjectId>,
    /// `m_Status` (`+0xCC`): mobile container 0 free, 1 drives to the meeting point, 2 arrived.
    pub status: i32,
    /// `m_EnemyQuarterList` (`+0xDC`): enemy occupants (never filled in the shipped data).
    pub enemies: Vec<ObjectId>,
    /// `m_EnemyDies` (`+0xF8`): ms until the next enemy occupant is killed.
    pub enemy_dies: i32,
    /// Meeting point (`+0xD4/+0xD8`, grid fixed-point x, z): destination of the mobile container while
    /// `status` is 1 (re/ships 1.3, 5). Appended at the end (savegame format 10).
    pub meeting_point: Option<(i32, i32)>,
}

impl Default for Quarter {
    fn default() -> Self {
        Self {
            max: -1,
            cur: 0,
            occupants: Vec::new(),
            waiting: Vec::new(),
            garrison: Vec::new(),
            status: 0,
            enemies: Vec::new(),
            enemy_dies: ENEMY_DIES_START,
            meeting_point: None,
        }
    }
}

pub(super) fn quarter<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Quarter> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Quarter(q) => Some(&**q),
        _ => None,
    }
}

pub(super) fn quarter_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Quarter> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Quarter(q) => Some(&mut **q),
        _ => None,
    }
}

/// `Module_HasFlags` of the quarters.
fn has_flags(sim: &Sim, id: ObjectId, x: u32) -> bool {
    sim.module(id, INDEX).is_some_and(|m| m.base.has_flags(x))
}

/// `Module_SetFlags` (`0x004FD240`): without the low byte.
fn set_flags(sim: &mut Sim, id: ObjectId, x: u32) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags |= x & 0xFFFF_FF00;
    }
}

/// `Module_ClearFlags` (`0x004FD260`): without the low byte.
fn clear_flags(sim: &mut Sim, id: ObjectId, x: u32) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags &= !(x & 0xFFFF_FF00);
    }
}

fn has_module(sim: &Sim, id: ObjectId, index: usize) -> bool {
    sim.object(id).is_some_and(|o| o.has_module(index, false))
}

fn set_status(sim: &mut Sim, id: ObjectId, flags: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(flags, on);
    }
}

/// `GameObject_GetTargetObject` (`0x00466910`): linked object `+0x2CC ∉ {−1, 0}` (mount).
fn target_object(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let t = sim.object(id)?.target;
    (!t.is_none() && t.0 != 0 && sim.object(t).is_some()).then_some(t)
}

fn remove_first(list: &mut Vec<ObjectId>, id: ObjectId) -> bool {
    match list.iter().position(|&x| x == id) {
        Some(p) => {
            list.remove(p);
            true
        }
        None => false,
    }
}

fn post_delayed(sim: &mut Sim, id: ObjectId, delay: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(delay, msg, false, GROUP);
    }
}

/// `0x0042D390` -> `0x004E5580`: attack mode message `0x611 {a, mode, b, flag}` to the
/// attack module; `a`/`b` by the mode (0 -> 1/4, 1/3/4 -> 4/1, 2 -> 2/1, otherwise 0/0).
pub fn attack_mode_msg(mode: i32, flag: bool) -> Message {
    let (a, b) = match mode {
        0 => (1, 4),
        1 | 3 | 4 => (4, 1),
        2 => (2, 1),
        _ => (0, 0),
    };
    Message::new(0x611, &[a, mode, b, i32::from(flag)])
}

/// `0x0042D330` -> `BehaviourMsg_Init` (`0x004E5660`): walk behavior `0x2100 {follow, mobile,
/// tether, abort, backup}` by the mode (0 -> `{1,0,−1,1}`, 1 -> `{1,1,0,1}`, 2 -> `{1,1,−2,1}`,
/// 3 -> `{1,0,−1,0}`, 4 -> `{1,1,−3,1}`, otherwise 0), `backup` = `flag`. Walk evaluates the message with
/// the behavior modes (`walk::behaviour::apply_behaviour_msg`).
pub fn walk_behavior_msg(mode: i32, flag: bool) -> Message {
    let w = match mode {
        0 => [1, 0, -1, 1],
        1 => [1, 1, 0, 1],
        2 => [1, 1, -2, 1],
        3 => [1, 0, -1, 0],
        4 => [1, 1, -3, 1],
        _ => [0; 4],
    };
    Message::new(0x2100, &[w[0], w[1], w[2], w[3], i32::from(flag)])
}

/// `FUN_004E5610`: attack mode message of the AI `0x611 {a, a == 1 ? 0 : 3, b, 0}` - aggressiveness
/// `a`, stance 0 at `a == 1`, otherwise 3, persistence `b` (re/ki-queries 7.3, E-101). The
/// network command checks the values (exception A7); the message itself does not.
pub fn ai_attack_mode_msg(a: i32, b: i32) -> Message {
    Message::new(0x611, &[a, if a == 1 { 0 } else { 3 }, b, 0])
}

/// `FUN_004E5760`: walk behavior of the AI `0x2100 {follow, mobile, tether, abort, 0}` with the
/// tether only at a mobile home and value >= -2 (otherwise -1), re/ki-queries 7.3, E-102. The upper limit
/// of the tether is checked by the network command ([`crate::command::ai::MAX_AI_TETHER`]).
pub fn ai_walk_flags_msg(mobile_home: bool, tether: i32, follow: bool, abort: bool) -> Message {
    let tether = if mobile_home && tether >= -2 {
        tether
    } else {
        -1
    };
    Message::new(
        0x2100,
        &[
            i32::from(follow),
            i32::from(mobile_home),
            tether,
            i32::from(abort),
            0,
        ],
    )
}

/// `PropertyQuarter_ReadParams` (`0x0051FCF0`, slot 4; reads no `VALUEID`): fills the capacity fraction
/// to full, then like [`on_player_data_changed`].
pub fn read_params(sim: &mut Sim, id: ObjectId, _desc: &ModuleDesc) {
    if let Some(o) = sim.object_mut(id) {
        o.attributes.fill_current(CAPACITY);
    }
    on_player_data_changed(sim, id);
}

/// `PropertyQuarter_OnPlayerDataChanged` (`0x0051FC90`, slot 8): capacity = current value of
/// attribute `0x17` (with upgrade percent); with free places set `FREEFORQUARTER` - a building
/// only when finished. Quirk: never cleared, even if the capacity drops below the occupancy.
pub fn on_player_data_changed(sim: &mut Sim, id: ObjectId) {
    let max = sim.attribute(id, CAPACITY);
    let Some(q) = quarter_mut(sim, id) else {
        return;
    };
    q.max = max;
    if q.cur < max
        && (!has_module(sim, id, modules::BUILDING)
            || matches_class(sim, id, flags::FINISHED_BUILDING))
    {
        set_flags(sim, id, flags::FREE_FOR_QUARTER);
    }
}

/// `PropertyQuarter_Init` (`0x0051FEB0`, slot 9): only `ReadParams`. The branch for a
/// "busy" quarters is dead (`0x00532A90` always returns `false`). Unfinished buildings
/// do not get `FREEFORQUARTER` until the end of construction (`0x1A17`).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    read_params(sim, id, desc);
}

/// `PropertyQuarter_Tick` (`0x00520940`, slot 13): the branch of the enemy occupants (capture at
/// 20 % hit points or ten enemies, otherwise kill one every 5000 ms) is dead in the shipped data -
/// `AcceptUnit` always rejects foreign units, the list stays empty. What remains are the timers
/// (`Module_UpdateEvents`), through which delayed moving in and letting out run.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

/// `PropertyQuarter_Abort` (`0x00520130`, slot 11): a mobile container forgets the waiting;
/// the occupants stay inside.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(q) = quarter_mut(sim, id) {
        q.status = 0;
        q.waiting.clear();
    }
    true
}

/// `PropertyQuarter_OnOwnerChanged(old, new)` (`0x00520240`, slot 5): each occupant gets the new
/// carrier ID (`0xA7F {new}`); the occupants keep their owner.
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId) {
    let list = quarter(sim, id).map_or_else(Vec::new, |q| q.occupants.clone());
    for u in list {
        if sim.object(u).is_some() {
            send(sim, u, &Message::new(0xA7F, &[id.as_i32()]));
        }
    }
}

/// Part of `PropertyWalk_OnOwnerChanged(old, new)` (`0x00532C20`): a quartered occupant with a
/// new ID reports `0x2417 {old, new}` to its carrier (`0x00468110`). The group deregistration
/// is in `walk::formation::on_owner_changed` (before it); the UI message is missing.
pub fn notify_carrier_of_new_id(sim: &mut Sim, new: ObjectId, old: ObjectId) {
    let Some(o) = sim.object(new) else { return };
    if o.status & status::QUARTERED == 0 {
        return;
    }
    let carrier = o.carrier;
    if sim.object(carrier).is_some() {
        send(
            sim,
            carrier,
            &Message::new(msg::REPLACE_OCCUPANT, &[old.as_i32(), new.as_i32()]),
        );
    }
}

/// `PropertyQuarter_Query` (`0x00521010`, slot 7). The meeting-point messages of mobile containers
/// (`0x1E17` set, `0x1F17` arrived, `0x2017` read) are answered by [`super::boarding`].
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::ENTER => on_enter(sim, id, m),
        msg::UNQUARTER_ONE => unquarter_one(sim, id, m),
        msg::LIST_OCCUPANTS => {
            // `PropertyQuarter_OnMsgListOccupants` (`0x0051FEF0`): at most `argc` IDs,
            // then `argc` = count.
            let list = quarter(sim, id).map_or_else(Vec::new, |q| q.occupants.clone());
            let n = list.len().min(m.args.len());
            for (i, u) in list.iter().take(n).enumerate() {
                m.args[i] = u.as_i32();
            }
            m.args.truncate(n);
            0
        }
        msg::UNQUARTER_ALL => unquarter_all(sim, id),
        msg::BUILDING_FINISHED => {
            // `PropertyQuarter_OnMsgBuildingFinished` (`0x0051FC40`).
            if quarter(sim, id).is_some_and(|q| q.max > 0) {
                set_flags(sim, id, flags::FREE_FOR_QUARTER);
            }
            0
        }
        msg::GARRISON_ATTACK => garrison_attack(sim, id, m.arg(0)),
        msg::HAS_GARRISON => quarter(sim, id).map_or(0, |q| i32::from(!q.garrison.is_empty())),
        msg::SET_GARRISON_BEHAVIOR => set_garrison_behavior(sim, id, m.arg(0)),
        msg::CONTAINER_DESTROYED => on_container_destroyed(sim, id),
        msg::OCCUPANT_COUNT => quarter(sim, id).map_or(0, |q| q.cur),
        msg::TOUCH_PARTNER => touch_partner(sim, id, ObjectId::from_i32(m.arg(0))),
        msg::SET_MEETING_POINT => boarding::on_set_meeting_point(sim, id, m),
        msg::CONTAINER_ARRIVED => boarding::on_container_arrived(sim, id),
        msg::GET_MEETING_POINT => boarding::get_meeting_point(sim, id, m),
        msg::REPLACE_OCCUPANT => {
            // `PropertyQuarter_OnMsgReplaceOccupantId` (`0x005202A0`): first hit in the
            // occupants, then in the garrison.
            let (old, new) = (ObjectId::from_i32(m.arg(0)), ObjectId::from_i32(m.arg(1)));
            let Some(q) = quarter_mut(sim, id) else {
                return 0;
            };
            let Some(p) = q.occupants.iter().position(|&x| x == old) else {
                return 0;
            };
            q.occupants[p] = new;
            if let Some(p) = q.garrison.iter().position(|&x| x == old) {
                q.garrison[p] = new;
            }
            1
        }
        _ => -1,
    }
}

/// `PropertyQuarter_OnMsgEnter` (`0x00520E20`): with two arguments (only from the dead
/// collection building branch) delays itself (`argc` -> 1); a mobile container that has not yet
/// arrived (`m_Status ≠ 2`, walk module present) parks the unit (`0x2D00`) instead of taking it
/// in - that is the path of the meeting point (re/ships 3.3, [`super::boarding`]); otherwise
/// [`accept_unit`]. Return value 0.
fn on_enter(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let u = ObjectId::from_i32(m.arg(0));
    if sim.object(u).is_none() {
        return 0;
    }
    if m.args.len() == 2 {
        let delay = m.arg(1);
        m.args.truncate(1);
        post_delayed(sim, id, delay, m.clone());
        return 0;
    }
    let arrived = quarter(sim, id).is_some_and(|q| q.status == 2);
    if !arrived && has_module(sim, id, modules::WALK) {
        send(sim, u, &Message::new(0x2D00, &[]));
        return 0;
    }
    accept_unit(sim, id, u)
}

/// `PropertyQuarter_AcceptUnit` (`0x00520AF0`): 0 = moved in (or already inside), 1 = rejected.
/// Mounted on a boat (or mounted in front of a building) the unit must first dismount
/// synchronously: `0x20D` goes to **module 13** (re/riding 4.5, W-204) - pure boat types without
/// riding are rejected, types with both modules get out via riding and keep `0x8000` until
/// step 11 of the next object tick (the canoe is already free via `0x810`); horses stay
/// outside. Siege engines never; foreign units always rejected (the check `0x004527C0` always
/// returns `false`, the branch of the enemy occupants is dead). Full: an already quartered unit
/// comes out at the node `ausgang`. Otherwise appended to the occupants, ranged fighters in the
/// building also into the garrison, `EnterContainer`, set `QUARTERED` and `FREEFORQUARTER`
/// after the occupancy.
pub fn accept_unit(sim: &mut Sim, id: ObjectId, u: ObjectId) -> i32 {
    let Some(st) = sim.object(u).map(|o| o.status) else {
        return 1;
    };
    let building = has_module(sim, id, modules::BUILDING);
    if (st & status::ISMOUNTEDONBOAT != 0 || (st & status::ISMOUNTED != 0 && building))
        && dispatch_command(sim, u, &Envelope::bare(Message::new(0x20D, &[]))) == 0
    {
        return 1;
    }
    if has_module(sim, u, modules::SIEGE_ENGINE) {
        return 1;
    }
    if u.slot() != id.slot() {
        return 1;
    }
    let Some(q) = quarter(sim, id) else { return 1 };
    if q.cur.wrapping_add(UNIT_SIZE) > q.max {
        if st & status::QUARTERED != 0 {
            set_status(sim, u, status::QUARTERED, false);
            if let Some(p) = helper_node_map_position(sim, id, "ausgang") {
                set_grid_position(sim, u, p, true);
            }
        }
        return 1;
    }
    if q.occupants.contains(&u) {
        return 0;
    }
    let garrison = has_module(sim, u, modules::FIGHT_FAR) && building;
    if let Some(q) = quarter_mut(sim, id) {
        q.occupants.push(u);
        q.cur = q.cur.wrapping_add(UNIT_SIZE);
        if garrison {
            q.garrison.push(u);
        }
    }
    enter_container(sim, u, id);
    set_flags(sim, id, flags::QUARTERED);
    if quarter(sim, id).is_some_and(|q| q.cur < q.max) {
        set_flags(sim, id, flags::FREE_FOR_QUARTER);
    } else {
        clear_flags(sim, id, flags::FREE_FOR_QUARTER);
    }
    0
}

/// `GameObject_EnterContainer(u, c)` (`0x004712A0`): a mount (`ISMOUNTED`) comes along - only in
/// mobile containers (ships), in front of buildings [`accept_unit`] has already dismounted the rider;
/// `QUARTERED`, position of the container (own height), carrier = container. Ranged fighters in the
/// building fetch the attack mode of the building (`0x3603`); attack mode `0x611` (module 17, with
/// backup) and walk behavior `0x2100 {1,0,−1,1,1}` go to the unit (the behavior backs up the
/// settings and aborts all modules of the unit with `[3] = 1`). A healer in the building
/// switches on healing in the building (`0x71D {1}` -> `healing::query`, re/healing 4) and its
/// healing module off the tick list; no effect in the shipped data (no healing building has module 23, C-250).
///
/// The selection (`GameWorld_RemoveFromSelection` `0x004801B0`) is not carried by the simulation: the
/// viewer takes objects with `QUARTERED` out of its selection, makes them unclickable and does not draw
/// them (`GameObject_RenderPass` `0x0046B8CE`); `selectable` stays untouched as in the original.
pub fn enter_container(sim: &mut Sim, u: ObjectId, c: ObjectId) {
    let (Some(uo), Some(co)) = (sim.object(u), sim.object(c)) else {
        return;
    };
    let (st, cpos) = (uo.status, co.grid);
    if st & status::ISMOUNTED != 0
        && let Some(mount) = target_object(sim, u)
    {
        set_status(sim, mount, status::QUARTERED, true);
        set_grid_position(sim, mount, cpos, true);
    }
    set_status(sim, u, status::QUARTERED, true);
    set_grid_position(sim, u, cpos, true);
    if let Some(o) = sim.object_mut(u) {
        o.carrier = c;
    }
    let building = has_module(sim, c, modules::BUILDING);
    let mut mode = 0;
    if has_module(sim, u, modules::FIGHT_FAR) && building {
        mode = send(sim, c, &Message::new(0x3603, &[]));
    }
    send(sim, u, &attack_mode_msg(mode, true));
    send(sim, u, &walk_behavior_msg(0, true));
    if has_module(sim, u, modules::HEALING) && building {
        send(sim, c, &Message::new(0x71D, &[1]));
        if let Some(o) = sim.object_mut(u) {
            o.queue_module_tick_change(modules::HEALING, false);
        }
    }
}

/// `GameObject_LeaveContainer(u)` (`0x00471420`): `QUARTERED` removed first. Mounted on a boat ->
/// `false` without further effect (quirk W-128/C-19: the bit is already cleared; reachable only
/// in the same tick as W-204, as long as a PRI type still carries `0x8000`). With a carrier: the start point
/// in front of a building is its exit (`0x2503`, search radius ten tiles, healing in the building off via
/// `0x71D {0}`), otherwise the position of the container (radius one tile + own hard + soft radius
/// of the container); the unit is placed there and searches in a spiral for a free place
/// ([`find_free_spot_spiral`]). Without a place it stays at the start point and the result is `false`.
/// Mounted (only from mobile containers) the mount comes along - quirk: `QUARTERED` is cleared on the
/// **container**, not on the rider. Finally `0x1611` (attack module), `0x3D00` (walk restores the backed-up
/// behavior) and the healing module back into the tick list.
pub fn leave_container(sim: &mut Sim, u: ObjectId) -> bool {
    let mut ok = true;
    set_status(sim, u, status::QUARTERED, false);
    let Some(uo) = sim.object(u) else { return true };
    if uo.status & status::ISMOUNTEDONBOAT != 0 {
        return false;
    }
    let (carrier, own_radius, has_healing) = (
        uo.carrier,
        uo.hard_radius,
        uo.module(modules::HEALING).is_some(),
    );
    if let Some(co) = sim.object(carrier) {
        let (cpos, warn_radius) = (co.grid, co.soft_radius);
        let mut radius = SPIRAL_BASE_RADIUS.wrapping_add(own_radius);
        let start = if has_module(sim, carrier, modules::BUILDING) {
            let mut r = Message::new(0x2503, &[0, 0, 0]);
            send_mut(sim, carrier, &mut r);
            radius = SPIRAL_BUILDING_RADIUS;
            if has_healing {
                send(sim, carrier, &Message::new(0x71D, &[0]));
            }
            FxVec3::new(Fx(r.arg(0)), Fx(r.arg(1)), Fx(r.arg(2)))
        } else {
            radius = radius.wrapping_add(warn_radius);
            cpos
        };
        set_grid_position(sim, u, start, true);
        match find_free_spot_spiral(sim, u, radius) {
            Some(out) => {
                set_grid_position(sim, u, out, true);
                let mounted = sim
                    .object(u)
                    .is_some_and(|o| o.status & status::ISMOUNTED != 0);
                if mounted {
                    set_status(sim, carrier, status::QUARTERED, false);
                    if let Some(mount) = target_object(sim, u) {
                        set_grid_position(sim, mount, out, true);
                        set_status(sim, mount, status::QUARTERED, false);
                    }
                }
            }
            None => ok = false,
        }
    }
    send(sim, u, &Message::new(0x1611, &[]));
    send(sim, u, &Message::new(0x3D00, &[]));
    if let Some(o) = sim.object_mut(u) {
        o.queue_module_tick_change(modules::HEALING, true);
    }
    ok
}

/// Point sequence of `GameObject_FindFreeSpotSpiral`: square spiral around `(x0, z0)` with
/// step width `step`, side lengths 1, 1, 2, 2, 3, 3, ..., first toward -x, then +z, +x, -z. The
/// center itself never occurs; it aborts only in direction -x, as soon as `x0 − x > radius`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spiral {
    x0: i32,
    x: i32,
    z: i32,
    k: i32,
    len: i32,
    dir: u8,
    step: i32,
}

impl Spiral {
    pub fn new(x0: i32, z0: i32, step: i32) -> Self {
        Self {
            x0,
            x: x0,
            z: z0,
            k: 0,
            len: 1,
            dir: 0,
            step,
        }
    }

    /// Next point; `None` at the abort.
    pub fn next_point(&mut self, radius: i32) -> Option<(i32, i32)> {
        self.k = (self.k + 1) % self.len;
        let turn = self.k == 0;
        match self.dir {
            0 => {
                self.x = self.x.wrapping_sub(self.step);
                if self.x0.wrapping_sub(self.x) > radius {
                    return None;
                }
                if turn {
                    self.dir = 1;
                }
            }
            1 => {
                self.z = self.z.wrapping_add(self.step);
                if turn {
                    self.dir = 2;
                    self.len += 1;
                }
            }
            2 => {
                self.x = self.x.wrapping_add(self.step);
                if turn {
                    self.dir = 3;
                }
            }
            _ => {
                self.z = self.z.wrapping_sub(self.step);
                if turn {
                    self.dir = 0;
                    self.len += 1;
                }
            }
        }
        Some((self.x, self.z))
    }
}

/// `GameObject_FindFreeSpotSpiral(u, _, radius, out)` (`0x0046F9A0`): spiral ([`Spiral`]) around the
/// **current** position of the unit with step width = double hard radius in the path layer of its
/// movement kind (`0xB00`, mask without water bit `0x20`). A point fits if `QueryCostAt` there is
/// below 40 and **one of the four neighbor cells** in the path grid is free (the target cell
/// itself need not be). Result with `y = 0` (the caller sets its own height).
///
/// Deviation: with a step width <= 0 (hard radius 0) the spiral would run forever in the original
/// if the start point does not fit; here `None` (not reachable in the shipped data, exception A7).
pub fn find_free_spot_spiral(sim: &mut Sim, u: ObjectId, radius: i32) -> Option<FxVec3> {
    let t = send(sim, u, &Message::new(0xB00, &[]));
    let mask = *LAYER_MASKS.get(usize::try_from(t).ok()?)?;
    let layer = layer_from_move_mask(mask & !0x20);
    let o = sim.object(u)?;
    let (x0, z0, step) = (o.grid.x.0, o.grid.z.0, o.hard_radius.wrapping_mul(2));
    if step <= 0 {
        log::warn!("Spiral search for {u:?} with step width {step}: aborted (exception A7)");
        return None;
    }
    let mut spiral = Spiral::new(x0, z0, step);
    while let Some((x, z)) = spiral.next_point(radius) {
        if query_cost_at(sim, x, z, Some(u), layer, false, false) >= FREE_COST_LIMIT {
            continue;
        }
        let (tx, tz) = (x >> 12, z >> 12);
        let free = |dx: i32, dz: i32| {
            sim.state
                .path
                .cell_cost(tx.wrapping_add(dx), tz.wrapping_add(dz), layer)
                == COST_FREE
        };
        if free(1, 0) || free(-1, 0) || free(0, 1) || free(0, -1) {
            return Some(FxVec3::new(Fx(x), Fx(0), Fx(z)));
        }
    }
    None
}

/// `PropertyQuarter_OnMsgUnquarterOne` (`0x00520310`). Second phase (`argc 2`, after the
/// secret passage): let out, otherwise again in one second. First phase: occupants only; a slave
/// (module `0x2D`) loses its collision exemption. Secret passage (`USE_SECRET_PASSAGE`): the
/// nearest own finished building with a nature module takes the unit in and lets it out after
/// `distance / 4` ms - quirk: no type has module 3 and 5, the branch never hits (and the flag
/// is set only by command `0x76`, whose buttons are missing). Otherwise `LeaveContainer`; without a place the
/// unit moves in again. A vanished unit is removed (`cur − 1`).
fn unquarter_one(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let uid = ObjectId::from_i32(m.arg(0));
    let exists = sim.object(uid).is_some();
    if m.args.len() == 2 {
        if exists && !leave_container(sim, uid) {
            post_delayed(sim, id, 1000, m.clone());
        }
        return 0;
    }
    if !quarter(sim, id).is_some_and(|q| q.occupants.contains(&uid)) {
        return 0;
    }
    if exists && has_module(sim, uid, modules::BOND_SLAVE) {
        if let Some(o) = sim.object_mut(uid) {
            o.exempt.object = ObjectId(0);
        }
        set_status(sim, uid, 0x1000, false);
    }
    let mut moved = false;
    if exists && has_flags(sim, id, flags::USE_SECRET_PASSAGE) {
        moved = secret_passage(sim, id, uid);
    }
    let out = !exists || moved || leave_container(sim, uid);
    if out {
        if let Some(q) = quarter_mut(sim, id) {
            remove_first(&mut q.occupants, uid);
            // Vanished: `DEC` instead of the unit size - both 1.
            q.cur = q.cur.wrapping_sub(UNIT_SIZE);
            remove_first(&mut q.garrison, uid);
        }
    } else {
        enter_container(sim, uid, id);
    }
    if quarter(sim, id).is_some_and(|q| q.cur < 1) {
        clear_flags(sim, id, flags::QUARTERED);
    }
    if quarter(sim, id).is_some_and(|q| q.cur < q.max) {
        set_flags(sim, id, flags::FREE_FOR_QUARTER);
    }
    0
}

/// Secret passage of `UnquarterOne`: filter `{mode 0x116, mode2 0x10, matchbits 0x403, PROPID 5}`
/// over the whole map for the owner; the unit switches into the building found and is let out
/// after `distance / 4` ms (truncated) with `0x1717 {u, 1}`.
fn secret_passage(sim: &mut Sim, id: ObjectId, u: ObjectId) -> bool {
    let filter = MatchFilter {
        mode: 0x116,
        mode2: 0x10,
        match_bits: flags::FINISHED_BUILDING,
        prop_id: 5,
        ..MatchFilter::constructed()
    };
    let Some(pos) = sim.object(id).map(|o| o.grid) else {
        return false;
    };
    let owner = i32::from(id.slot());
    let Some(b) = find_nearest_matching(sim, (pos.x.0, pos.z.0), &filter, -1, owner, -1) else {
        return false;
    };
    let Some(bpos) = sim.object(b).map(|o| o.grid) else {
        return false;
    };
    let d = fixed_pos_distance((bpos.x.0, bpos.z.0), (pos.x.0, pos.z.0));
    set_grid_position(sim, u, bpos, true);
    if let Some(o) = sim.object_mut(u) {
        o.carrier = b;
    }
    let msg = Message::new(msg::UNQUARTER_ONE, &[u.as_i32(), 1]);
    post_delayed(sim, id, d / 4, msg);
    true
}

/// `PropertyQuarter_UnquarterAll` (`0x005205C0`): per occupant in order of entry; vanished ones
/// stay in the list (quirk), with a secret passage via `UnquarterOne`, otherwise `LeaveContainer` -
/// without a place the unit stays inside (`QUARTERED` set again). Afterwards occupancy = list length.
fn unquarter_all(sim: &mut Sim, id: ObjectId) -> i32 {
    let list = quarter(sim, id).map_or_else(Vec::new, |q| q.occupants.clone());
    for u in list {
        if sim.object(u).is_none() {
            continue;
        }
        if has_flags(sim, id, flags::USE_SECRET_PASSAGE) {
            unquarter_one(sim, id, &Message::new(msg::UNQUARTER_ONE, &[u.as_i32()]));
            continue;
        }
        if !leave_container(sim, u) {
            set_status(sim, u, status::QUARTERED, true);
        } else if let Some(q) = quarter_mut(sim, id) {
            remove_first(&mut q.occupants, u);
            remove_first(&mut q.garrison, u);
        }
    }
    let Some(q) = quarter_mut(sim, id) else {
        return 0;
    };
    q.cur = q.occupants.len() as i32;
    let (cur, max) = (q.cur, q.max);
    if cur < max {
        set_flags(sim, id, flags::FREE_FOR_QUARTER);
    }
    if cur == 0 {
        clear_flags(sim, id, flags::QUARTERED);
    }
    0
}

/// `PropertyQuarter_OnMsgGarrisonAttack` (`0x0051FF30`): per garrison member (alive)
/// `0x111 {target, 0}` to the attack module. Return value 0 for an empty garrison, otherwise 1.
fn garrison_attack(sim: &mut Sim, id: ObjectId, target: i32) -> i32 {
    let list = quarter(sim, id).map_or_else(Vec::new, |q| q.garrison.clone());
    if list.is_empty() {
        return 0;
    }
    for u in list {
        if sim.state.object(u, true).is_some() {
            send(sim, u, &Message::new(0x111, &[target, 0]));
        }
    }
    1
}

/// `PropertyQuarter_OnMsgSetGarrisonBehavior` (`0x0051FFB0`): per living occupant with ranged combat
/// attack mode `0x611` and walk behavior `0x2100` of the mode (without backup).
fn set_garrison_behavior(sim: &mut Sim, id: ObjectId, mode: i32) -> i32 {
    let attack = attack_mode_msg(mode, false);
    let behavior = walk_behavior_msg(mode, false);
    let list = quarter(sim, id).map_or_else(Vec::new, |q| q.occupants.clone());
    for u in list {
        if sim.state.object(u, true).is_some() && has_module(sim, u, modules::FIGHT_FAR) {
            send(sim, u, &attack);
            send(sim, u, &behavior);
        }
    }
    1
}

/// `PropertyQuarter_OnMsgContainerDestroyed` (`0x005206C0`, `0x2117`): a building puts its
/// occupants out ([`release_occupants`]); remaining occupants (vehicles) learn the
/// last attacker of the container and die (`0x110` posted with abort, dying module). The
/// enemy branch after that is dead in the shipped data. Return value 1.
fn on_container_destroyed(sim: &mut Sim, id: ObjectId) -> i32 {
    if has_module(sim, id, modules::BUILDING) {
        release_occupants(sim, id);
    }
    let attacker = sim.object(id).map_or(ObjectId::NONE, |o| o.last_attacker);
    let list = quarter(sim, id).map_or_else(Vec::new, |q| q.occupants.clone());
    for u in list {
        if sim.object(u).is_none() {
            continue;
        }
        if !attacker.is_none() {
            notify_attacked_by(sim, u, attacker);
        }
        post_message(sim, u, Message::new(0x110, &[]), true);
    }
    while let Some(u) = quarter_mut(sim, id).and_then(|q| {
        let first = q.enemies.first().copied();
        if first.is_some() {
            q.enemies.remove(0);
        }
        first
    }) {
        if sim.object(u).is_none() {
            continue;
        }
        if !attacker.is_none() {
            notify_attacked_by(sim, u, attacker);
        }
        leave_container(sim, u);
        post_message(sim, u, Message::new(0x110, &[]), true);
    }
    1
}

/// `PropertyQuarter_ReleaseOccupants` (`0x00520150`): per occupant in order of entry
/// `LeaveContainer`; without a place it dies (`0x110` synchronous, dying module), otherwise it draws **one**
/// table random number `r` and suffers `0x27F {−r % 50, last attacker}` (0 ... 49 % of the
/// hit points). Afterwards lists empty, occupancy reset to 0, places free (with capacity > 0), `QUARTERED`
/// cleared.
pub fn release_occupants(sim: &mut Sim, id: ObjectId) {
    let list = quarter(sim, id).map_or_else(Vec::new, |q| q.occupants.clone());
    for u in list {
        if sim.object(u).is_none() {
            continue;
        }
        if !leave_container(sim, u) {
            send(sim, u, &Message::new(0x110, &[]));
            continue;
        }
        let r = sim.state.rng.rand();
        let percent = r.wrapping_neg() % 50;
        let attacker = sim.object(id).map_or(ObjectId::NONE, |o| o.last_attacker);
        send(sim, u, &Message::new(0x27F, &[percent, attacker.as_i32()]));
    }
    let Some(q) = quarter_mut(sim, id) else {
        return;
    };
    q.occupants.clear();
    q.garrison.clear();
    q.cur = 0;
    if q.max > 0 {
        set_flags(sim, id, flags::FREE_FOR_QUARTER);
    }
    clear_flags(sim, id, flags::QUARTERED);
}

/// `PropertyQuarter_OnMsgTouchPartner` (`0x00520D50`, `0x2317`): collision with a living
/// unit - if it waits at the meeting point (mobile container, re/ships 2.1), it boards and leaves the
/// waiting list (empty -> `Abort`); otherwise it boards if its walk order with success `0x1617` leads to
/// this container (`0x3800 {container}`, re/ships). Return value
/// 1 = boarded.
fn touch_partner(sim: &mut Sim, id: ObjectId, u: ObjectId) -> i32 {
    if sim.state.object(u, true).is_none() {
        return 0;
    }
    if quarter(sim, id).is_some_and(|q| q.waiting.contains(&u)) {
        let accepted = accept_unit(sim, id, u) == 0;
        let empty = quarter_mut(sim, id).is_none_or(|q| {
            remove_first(&mut q.waiting, u);
            q.waiting.is_empty()
        });
        if empty {
            abort(sim, id);
        }
        return i32::from(accepted);
    }
    let mut q = Message::new(0x3800, &[id.as_i32()]);
    if send_mut(sim, u, &mut q) != 0 {
        return i32::from(accept_unit(sim, id, u) == 0);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::building;
    use crate::rules::{BuildingShape, Rules};
    use crate::state::SimState;
    use crate::tick::step;
    use crate::world::{ObjectDesc, change_owner, create_object, desc_flags, destroy_object};

    /// Colonist (walk land 1 x 1, `CANQUARTER`, hard radius 512), ranged fighter (plus module 19),
    /// siege engine (module 46), headquarters with quarters (capacity 5) and house without.
    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1101\nNAME SPA_Schuetze\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nPROPID 19\nGUID 1102\nNAME SPA_Kanone\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nPROPID 46\nGUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nPROPID 23\n";

    /// Hull of the `SPA_Hauptquartier` (as in the building test) with an exit in front of the building.
    fn hq_shape(exit: bool) -> BuildingShape {
        let f = f32::from_bits;
        let mut s = BuildingShape {
            center: [f(0x3F82_3B80), f(0x439B_AB3F), f(0x41FF_1998)],
            half: [f(0x4399_C662), f(0x439B_401D), f(0x4345_B4F9)],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        };
        if exit {
            s.helper_nodes.insert("ausgang".into(), [0.0, 0.0, 600.0]);
        }
        s
    }

    fn rules(exit: bool) -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        r.building_shapes.insert(1000, hq_shape(exit));
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        r.defaults.insert(1100, set(&[(14, 120), (16, 183)]));
        r.defaults.insert(1101, set(&[(14, 120), (16, 183)]));
        r.defaults.insert(1102, set(&[(14, 300)]));
        r.defaults
            .insert(1000, set(&[(13, 90), (14, 4000), (23, 5)]));
        r
    }

    fn create(sim: &mut Sim, guid: u32, slot: i32, pos: (f32, f32), ready: bool) -> ObjectId {
        let mut desc = ObjectDesc::new(guid, slot, [pos.0, 0.0, pos.1]);
        if ready {
            desc.flags |= desc_flags::PLACED;
        }
        create_object(sim, &mut desc).unwrap()
    }

    fn flags_of(sim: &Sim, id: ObjectId) -> u32 {
        sim.module(id, INDEX).unwrap().base.flags
    }

    fn q(sim: &Sim, id: ObjectId) -> Quarter {
        quarter(sim, id).unwrap().clone()
    }

    fn enter(sim: &mut Sim, hq: ObjectId, u: ObjectId) -> i32 {
        send(sim, hq, &Message::new(msg::ENTER, &[u.as_i32()]))
    }

    /// Five colonists next to the headquarters (tiles 2...6 in row 3).
    fn colonists(sim: &mut Sim, n: usize) -> Vec<ObjectId> {
        (0..n)
            .map(|i| {
                let x = (2 + i) as f32 * 128.0 + 64.0;
                create(sim, 1100, 0, (x, 3.0 * 128.0 + 64.0), true)
            })
            .collect()
    }

    #[test]
    fn the_spiral_walks_square_rings_and_stops_left_of_the_radius() {
        // Step width 2048 (hard radius 1024), unit 1024.
        let mut s = Spiral::new(0, 0, 2048);
        let first: Vec<(i32, i32)> = (0..9)
            .map(|_| s.next_point(10 << 12).unwrap())
            .map(|(x, z)| (x / 1024, z / 1024))
            .collect();
        assert_eq!(
            first,
            [
                (-2, 0),
                (-2, 2),
                (0, 2),
                (2, 2),
                (2, 0),
                (2, -2),
                (0, -2),
                (-2, -2),
                (-4, -2)
            ]
        );
        // At ten tiles 1680 points until the abort; the center never occurs.
        let mut s = Spiral::new(5000, 7000, 2048);
        let mut n = 0;
        while let Some(p) = s.next_point(10 << 12) {
            assert_ne!(p, (5000, 7000));
            n += 1;
        }
        assert_eq!(n, 1680);
    }

    #[test]
    fn the_headquarters_takes_five_units_and_flags_its_state() {
        let rules = rules(false);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        // Capacity = attribute 0x17, finished -> FREEFORQUARTER.
        assert_eq!((q(&sim, hq).max, q(&sim, hq).cur), (5, 0));
        assert_eq!(flags_of(&sim, hq), 0x100);
        let units = colonists(&mut sim, 6);
        for (i, &u) in units[..5].iter().enumerate() {
            assert_eq!(enter(&mut sim, hq, u), 0);
            let o = sim.object(u).unwrap();
            assert_ne!(o.status & status::QUARTERED, 0);
            assert_eq!(
                (o.carrier, o.grid.x, o.grid.z),
                (
                    hq,
                    sim.object(hq).unwrap().grid.x,
                    sim.object(hq).unwrap().grid.z
                )
            );
            // QUARTERED at the quarters, FREEFORQUARTER until the fifth.
            let free = if i < 4 { 0x100 } else { 0 };
            assert_eq!(flags_of(&sim, hq), 0x200 | free, "after {}", i + 1);
        }
        // The sixth is rejected and stays outside; moving in twice changes nothing.
        let sixth = units[5];
        assert_eq!(enter(&mut sim, hq, sixth), 1);
        assert_eq!(accept_unit(&mut sim, hq, sixth), 1);
        assert_eq!(sim.object(sixth).unwrap().status & status::QUARTERED, 0);
        assert_eq!(q(&sim, hq).occupants, units[..5]);
        assert!(q(&sim, hq).garrison.is_empty());
        // Queries: count, list (at most argc), no garrison.
        assert_eq!(
            send(&mut sim, hq, &Message::new(msg::OCCUPANT_COUNT, &[])),
            5
        );
        let mut list = Message::new(msg::LIST_OCCUPANTS, &[0; 3]);
        assert_eq!(send_mut(&mut sim, hq, &mut list), 0);
        assert_eq!(
            list.args,
            units[..3].iter().map(|u| u.as_i32()).collect::<Vec<_>>()
        );
        assert_eq!(send(&mut sim, hq, &Message::new(msg::HAS_GARRISON, &[])), 0);
        // Filters do not see quartered units.
        assert!(!matches_class(&mut sim, units[0], 0x40000));
        assert!(matches_class(&mut sim, sixth, 0x40000));
        // Quirk: the capacity is checked before the list - an occupant who moves in again when the
        // house is full loses `QUARTERED` (without an exit he stays standing), but stays in
        // the list.
        assert_eq!(accept_unit(&mut sim, hq, units[0]), 1);
        assert_eq!(sim.object(units[0]).unwrap().status & status::QUARTERED, 0);
        assert_eq!(q(&sim, hq).occupants, units[..5]);
    }

    #[test]
    fn free_for_quarter_is_set_on_capacity_changes_but_never_cleared() {
        // `PropertyQuarter_OnPlayerDataChanged` (`0x0051FC90`): sets the flag with free places,
        // but never clears it (quirk; reachable via upgrades, 2817 tower).
        let rules = rules(false);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        for u in colonists(&mut sim, 5) {
            assert_eq!(enter(&mut sim, hq, u), 0);
        }
        let free = |sim: &Sim| flags_of(sim, hq) & flags::FREE_FOR_QUARTER != 0;
        assert!(!free(&sim));
        let capacity = |sim: &mut Sim, n: i32| {
            // Like an upgrade: surcharge on the base capacity 5 (fraction stays full).
            let a = &mut sim.object_mut(hq).unwrap().attributes;
            a.set_modifier(CAPACITY, n - 5, false, 7);
            on_player_data_changed(sim, hq);
            q(sim, hq).max
        };
        assert_eq!(capacity(&mut sim, 3), 3);
        assert!(!free(&sim));
        assert_eq!(capacity(&mut sim, 8), 8);
        assert!(free(&sim));
        // Below the occupancy again: the flag stays.
        assert_eq!(capacity(&mut sim, 3), 3);
        assert!(free(&sim));
    }

    #[test]
    fn foreign_units_siege_engines_and_construction_sites_are_refused_until_finished() {
        let rules = rules(false);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let foreign = create(&mut sim, 1100, 1, (300.0, 300.0), true);
        let cannon = create(&mut sim, 1102, 0, (600.0, 300.0), true);
        let archer = create(&mut sim, 1101, 0, (900.0, 300.0), true);
        assert_eq!(accept_unit(&mut sim, hq, foreign), 1);
        assert_eq!(accept_unit(&mut sim, hq, cannon), 1);
        // Ranged fighter in the building: garrison.
        assert_eq!(accept_unit(&mut sim, hq, archer), 0);
        assert_eq!(q(&sim, hq).garrison, [archer]);
        assert_eq!(send(&mut sim, hq, &Message::new(msg::HAS_GARRISON, &[])), 1);
        assert_eq!(
            send(
                &mut sim,
                hq,
                &Message::new(msg::GARRISON_ATTACK, &[foreign.as_i32()])
            ),
            1
        );
        // Construction site: no FREEFORQUARTER, the places are not freed until the end of construction (0x1A17).
        let site = create(&mut sim, 1000, 0, (2500.0, 2000.0), false);
        assert_eq!((q(&sim, site).max, flags_of(&sim, site)), (5, 0));
        send(&mut sim, site, &Message::new(msg::BUILDING_FINISHED, &[]));
        assert_eq!(flags_of(&sim, site), 0x100);
        // The building reports its attack mode (default 3).
        assert_eq!(send(&mut sim, hq, &Message::new(0x3603, &[])), 3);
    }

    #[test]
    fn occupants_follow_their_carrier_keep_ticking_and_vanish_with_it() {
        let rules = rules(false);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let (hq, u) = {
            let mut sim = Sim::new(&mut state, &rules);
            let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
            let u = colonists(&mut sim, 1)[0];
            assert_eq!(enter(&mut sim, hq, u), 0);
            (hq, u)
        };
        for _ in 0..6 {
            step(&mut state, &rules, &[]);
        }
        let grid = state.object(hq, false).unwrap().grid;
        let o = state.object(u, false).unwrap();
        assert_ne!(o.status & status::QUARTERED, 0);
        assert_eq!((o.grid.x, o.grid.z), (grid.x, grid.z));
        assert!(state.tick_groups.iter().any(|g| g.contains(&u)));
        // The carrier moves: the occupant follows in the next object tick.
        {
            let mut sim = Sim::new(&mut state, &rules);
            let to = FxVec3::new(Fx(grid.x.0 + 4096), grid.y, grid.z);
            set_grid_position(&mut sim, hq, to, true);
            crate::tick::object_tick(&mut sim, u, 100);
            assert_eq!(sim.object(u).unwrap().grid.x, to.x);
            // Carrier gone -> the occupant is destroyed with it.
            destroy_object(&mut sim, hq);
        }
        for _ in 0..2 {
            step(&mut state, &rules, &[]);
        }
        assert!(state.object(u, false).is_none());
    }

    #[test]
    fn units_leave_at_the_exit_onto_a_free_spot_one_by_one_or_all_together() {
        let rules = rules(true);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let units = colonists(&mut sim, 5);
        for &u in &units {
            enter(&mut sim, hq, u);
        }
        let (has_exit, exit) = building::get_exit_pos(&mut sim, hq, None);
        assert!(has_exit);
        let rect = building::building(&sim, hq).unwrap().rect;
        let (tx, tz) = (exit.x.0 >> 12, exit.z.0 >> 12);
        assert!(!(rect[0]..=rect[2]).contains(&tx) || !(rect[1]..=rect[3]).contains(&tz));
        // Not quartered: nothing.
        let stranger = create(&mut sim, 1100, 0, (300.0, 300.0), true);
        assert_eq!(
            send(
                &mut sim,
                hq,
                &Message::new(msg::UNQUARTER_ONE, &[stranger.as_i32()])
            ),
            0
        );
        assert_eq!(q(&sim, hq).cur, 5);
        // One out: first spiral point left of the exit (step width 1024).
        send(
            &mut sim,
            hq,
            &Message::new(msg::UNQUARTER_ONE, &[units[2].as_i32()]),
        );
        let o = sim.object(units[2]).unwrap();
        assert_eq!(o.status & status::QUARTERED, 0);
        assert_eq!((o.grid.x.0, o.grid.z.0), (exit.x.0 - 1024, exit.z.0));
        assert_eq!(q(&sim, hq).cur, 4);
        assert_eq!(flags_of(&sim, hq), 0x300);
        // The next: the soft radii (1024 each) overlap up to a distance of 2048, so
        // the fourth spiral point (+1, +1) is the first that fits.
        send(
            &mut sim,
            hq,
            &Message::new(msg::UNQUARTER_ONE, &[units[0].as_i32()]),
        );
        let o = sim.object(units[0]).unwrap();
        assert_eq!((o.grid.x.0, o.grid.z.0), (exit.x.0 + 1024, exit.z.0 + 1024));
        // All the others: list empty, QUARTERED cleared, places free.
        assert_eq!(
            send(&mut sim, hq, &Message::new(msg::UNQUARTER_ALL, &[])),
            0
        );
        let after = q(&sim, hq);
        assert_eq!((after.cur, after.occupants.len()), (0, 0));
        assert_eq!(flags_of(&sim, hq), 0x100);
        for &u in &units {
            assert_eq!(sim.object(u).unwrap().status & status::QUARTERED, 0);
        }
    }

    #[test]
    fn a_destroyed_building_releases_its_occupants_with_random_damage() {
        let rules = rules(true);
        let mut state = SimState::new(0, 7);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let attacker = create(&mut sim, 1100, 1, (300.0, 300.0), true);
        sim.object_mut(hq).unwrap().last_attacker = attacker;
        let units = colonists(&mut sim, 4);
        for &u in &units {
            enter(&mut sim, hq, u);
        }
        // Expected: one table random number per unit put out, damage -(r % 50) %.
        let mut rng = sim.state.rng;
        let start = rng.index();
        let expected: Vec<i32> = units
            .iter()
            .map(|_| {
                let p = rng.rand().wrapping_neg() % 50;
                assert!((-49..=0).contains(&p));
                120 + 120 * p / 100
            })
            .collect();
        assert_eq!(
            send(&mut sim, hq, &Message::new(msg::CONTAINER_DESTROYED, &[])),
            1
        );
        assert_eq!(
            usize::from(sim.state.rng.index()),
            (usize::from(start) + units.len()) % 1000
        );
        for (&u, &hp) in units.iter().zip(&expected) {
            let o = sim.object(u).unwrap();
            assert_eq!(o.status & status::QUARTERED, 0);
            assert_eq!(sim.attribute(u, 14), hp, "{u:?}");
            let o = sim.object(u).unwrap();
            assert_eq!((o.last_attacker, o.ticks_since_hit), (attacker, 0));
        }
        let after = q(&sim, hq);
        assert!(after.occupants.is_empty() && after.cur == 0);
        assert_eq!(flags_of(&sim, hq), 0x100);
    }

    #[test]
    fn owner_changes_rewrite_the_carrier_and_the_occupant_list() {
        let rules = rules(false);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let units = colonists(&mut sim, 2);
        for &u in &units {
            enter(&mut sim, hq, u);
        }
        // Occupant changes owner: walk reports 0x2417 {old, new}, the list is rewritten.
        let moved = change_owner(&mut sim, units[1], 2).unwrap();
        assert_eq!(q(&sim, hq).occupants, [units[0], moved]);
        assert_eq!(
            send(
                &mut sim,
                hq,
                &Message::new(msg::REPLACE_OCCUPANT, &[units[1].as_i32(), 5])
            ),
            0
        );
        // Container changes owner: the occupants get the new carrier ID.
        let new_hq = change_owner(&mut sim, hq, 1).unwrap();
        for u in [units[0], moved] {
            assert_eq!(sim.object(u).unwrap().carrier, new_hq);
        }
    }

    #[test]
    fn damage_in_percent_returns_the_amount_and_notes_the_attacker() {
        let rules = rules(false);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = colonists(&mut sim, 1)[0];
        let a = create(&mut sim, 1100, 1, (300.0, 300.0), true);
        // -33 % of 120 = -39; without an attacker +0x1B8 stays empty.
        assert_eq!(send(&mut sim, u, &Message::new(0x27F, &[-33])), -39);
        assert_eq!(sim.attribute(u, 14), 81);
        assert!(sim.object(u).unwrap().last_attacker.is_none());
        assert_eq!(
            send(&mut sim, u, &Message::new(0x27F, &[-1, a.as_i32()])),
            -1
        );
        assert_eq!(sim.object(u).unwrap().last_attacker, a);
    }
}
