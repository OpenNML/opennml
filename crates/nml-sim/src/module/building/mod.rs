//! Module 3 `CPropertyBuilding` (re/buildings; footprint and occupancy re/occupancy 5): rectangle
//! from the model hull, byte mask with holes at the helper nodes `frei01…`, remove nature and trees
//! on the area, occupancy rectangle (with blocking border `0x3303`) and path grid in all nine
//! layers; build time and cost per build step, docking places ([`parking`]), rally point, construction
//! start, timers in the building tick, deposit of mine and field ([`deposit`]), farm work slots
//! ([`farm`]), build step, wall group, construction site flag and burning down ([`construction`]),
//! production, research and trade ([`production`]), spawning of new units and rally point ([`rally`]),
//! destroying, selling, collapse and demolition ([`destroy`]), gate init and doors ([`gate`]), hiding
//! the tents ([`tent`]), enemy watch, neutralizing and capturing the wall group ([`watch`]), counters
//! in the destructor and change of owner.

pub mod construction;
pub mod deposit;
pub mod destroy;
pub mod farm;
#[cfg(test)]
pub(crate) mod fort_testkit;
#[cfg(test)]
mod fort_tests;
pub mod gate;
pub mod parking;
pub mod production;
pub mod rally;
pub(crate) mod tent;
pub mod track;
pub mod watch;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::fixed_vec::FixedVec;
use crate::footprint::{
    footprint_tile_rect, helper_node_map_position, local_offset_to_grid, mark_tile_for_layers,
    world_matrix,
};
use crate::fx::{Fx, FxVec3, fixed_div, fixed_mul};
use crate::id::{NATURE_SLOT, ObjectId, TREE_SLOT};
use crate::message::Message;
use crate::messaging::send;
use crate::object::status;
use crate::sim::Sim;

use self::parking::ParkingLot;
use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 3;
/// Side length of the mask (`PropertyBuilding+0x10E`, 20 x 20 with border 1).
pub const MASK_SIDE: i32 = 20;
/// Fields of the mask (20 * 20).
pub const MASK_LEN: usize = (MASK_SIDE * MASK_SIDE) as usize;
/// `PRI_Alarmposten`: Rechteck = eigene Kachel.
const ALARM_POST: u32 = 1610;
/// Group of the building timers.
const GROUP: u8 = 0x80;
/// At most this many waiting production orders (`PropertyBuilding_EnqueueProduction` `0x00508670`).
pub const MAX_QUEUE: usize = 26;
/// Slots of the farm workers (`m_RegisteredHandles`, helper nodes `farmer01…05`).
pub const FARM_SLOTS: usize = 5;

/// Building flags (`+0x0C`, names from `targetdefines.ini`) with the module index in the low byte, as
/// the original passes them to `Module_HasFlags`/`SetFlags`/`ClearFlags` (the low byte does not count).
pub mod flags {
    pub const GOLDMINE: u32 = 0x103;
    pub const FARM: u32 = 0x203;
    pub const CONSTRUCTED_FINISHED: u32 = 0x403;
    pub const TURM: u32 = 0x1_0003;
    pub const TOR: u32 = 0x2_0003;
    pub const ISANIMATED: u32 = 0x4_0003;
    pub const BURNDOWN: u32 = 0x8_0003;
    pub const PALISADE: u32 = 0x10_0003;
    pub const PALISADECORNER: u32 = 0x20_0003;
    pub const TOROPEN: u32 = 0x40_0003;
    /// `TORCLOSE` (re/arming-gates 1.D): gate closed; run-time bit like `TOROPEN`.
    pub const TORCLOSE: u32 = 0x80_0003;
    pub const BUILDINGEXPLODE: u32 = 0x100_0003;
    /// `TRACK` (tracks): without footprint in the path grid.
    pub const TRACK: u32 = 0x200_0003;
    /// Run time: production queue active.
    pub const MAKINGGUID: u32 = 0x400_0003;
    /// `MID_PROP_BUILDING_CASTLEBUILDING`: castle building (powder house explosion hits everyone,
    /// re/combat-death 4).
    pub const CASTLEBUILDING: u32 = 0x1000_0003;
    /// Run-time bits that `ReadParams` leaves alone (FINISHED, TOROPEN, TORCLOSE, MAKINGGUID).
    pub const KEPT_BY_READ_PARAMS: u32 = 0x04C0_0403;
    /// `StartBuild`: clear everything except TRACK, PALISADECORNER and PALISADE.
    pub const CLEARED_BY_START_BUILD: u32 = 0xFDCF_FFFC;
}

/// Messages of the building (`PropertyBuilding_Query` `0x0050BDE0`), as far as implemented.
pub mod msg {
    /// `PMSG_BUILDING_STARTBUILD`: become a construction site again (`StartBuild`).
    pub const START_BUILD: u32 = 0x103;
    /// `{speed, _}` -> build step: 1 continue, 0 stop, 2 next construction site in `args[1]`.
    pub const BUILD_TICK: u32 = 0x203;
    /// `{n}` -> gold from the deposit of the mine.
    pub const GET_GOLD: u32 = 0x303;
    /// `{k}` -> helper node `farmer{k+1}` in `args[1..4]`.
    pub const FARMER_POS: u32 = 0x503;
    /// `{hid, idx}` -> farm work slot free? (-1 round-robin, -2 first free, otherwise this one).
    pub const FARM_QUERY_SLOT: u32 = 0x603;
    /// `{hid, idx}` -> occupy farm work slot.
    pub const FARM_CLAIM_SLOT: u32 = 0x803;
    /// `{hid}` -> release farm work slot.
    pub const FARM_RELEASE_SLOT: u32 = 0x903;
    /// `{n}` -> food from the deposit of the field.
    pub const GET_FOOD: u32 = 0xA03;
    /// `{guid}` -> queue an order (1) or reject it (0).
    pub const ENQUEUE: u32 = 0xE03;
    /// -> running order and progress (`f32` bits) in `args[0..2]`.
    pub const PROGRESS: u32 = 0xF03;
    /// -> waiting orders in the arguments (`argc` = number).
    pub const LIST: u32 = 0x1003;
    /// `{n}`: cancel the `n`-th waiting (0 = running) order.
    pub const CANCEL: u32 = 0x1103;
    /// `{x, z}` or `{−1, object}`: set rally point.
    pub const SET_MEETING_POINT: u32 = 0x1203;
    /// `{id}`: send the unit to the rally point.
    pub const SEND_TO_MEETING_POINT: u32 = 0x4103;
    /// `{x, z, hid, layer}` -> nearest free docking place (`GetNearestDockPos`).
    pub const NEAREST_DOCK: u32 = 0xB03;
    /// `{index, hid}` -> register docking place.
    pub const REGISTER_DOCK: u32 = 0xC03;
    /// `{hid}` -> unregister docking place.
    pub const UNREGISTER_DOCK: u32 = 0xD03;
    /// -> rally point x, z.
    pub const MEETING_POINT: u32 = 0x1303;
    /// `OnMsgRaiseLowHp` (after every HP change `0x27F`): dead branch, answer 0.
    pub const RAISE_LOW_HP: u32 = 0x1403;
    /// `{k, ids[k]}`: set the members of the wall group (to the master).
    pub const SET_SLAVE_LIST: u32 = 0x1B03;
    /// `{id}`: take a member out of the group.
    pub const REMOVE_SLAVE: u32 = 0x1C03;
    /// `{id}`: set master, forget own members.
    pub const SET_MASTER: u32 = 0x1D03;
    /// Demolish finally (also in the wall group): 1 = group still lives.
    pub const FINALLY_DEATH: u32 = 0x1E03;
    /// Put to sleep (palisade under a tower).
    pub const SET_SLEEPING: u32 = 0x1F03;
    /// Wake (palisade under a collapsed tower).
    pub const WAKE: u32 = 0x2003;
    /// `{id}`: append a member.
    pub const ADD_SLAVE: u32 = 0x2103;
    /// -> master of the group (without one: own ID) in `args[0]`.
    pub const GET_MASTER: u32 = 0x2203;
    /// -> exit x, y, z.
    pub const EXIT_POS: u32 = 0x2503;
    /// `{layer, r0, r1, …}` -> list index of the first docking place region in the list.
    pub const DOCK_REGION: u32 = 0x2C03;
    /// `{guid, slot}` -> spawn a unit (`0x0050BA00` -> `!PropertyBuilding_SpawnProducedUnit`):
    /// 0 created, 1 failed (headhunter, re/headhunter-robbing 2.3).
    pub const SPAWN_UNIT: u32 = 0x2D03;
    /// Pack tent: hide the building (`PropertyBuilding_OnMsgHide` `0x00508E00`, re/tents 2.3).
    pub const HIDE: u32 = 0x2E03;
    /// Tent placed (`0x0050A5F0`, re/tents 3.5).
    pub const TENT_PLACED: u32 = 0x2F03;
    /// `{flush}`: release or block the path grid.
    pub const UNBLOCK_FOOTPRINT: u32 = 0x3003;
    pub const BLOCK_FOOTPRINT: u32 = 0x3103;
    /// `{n}`: enlarge the occupancy rectangle by `n` per side.
    pub const EXPAND_OCCUPANCY: u32 = 0x3303;
    /// `{hid}` -> occupied docking place of this unit.
    pub const DOCK_OF: u32 = 0x3403;
    /// `{mode}`: set behavior (attack mode) and pass it on to the occupants.
    pub const SET_ATTACK_MODE: u32 = 0x3503;
    /// -> attack mode of new units.
    pub const GET_ATTACK_MODE: u32 = 0x3603;
    /// Destroy (`GameObject_CheckDeath`); return value 1.
    pub const DESTROY: u32 = 0x3703;
    /// `{x, z}` -> point on the edge of the footprint.
    pub const PROJECT_TO_EDGE: u32 = 0x3803;
    /// Sell (command `0x67`).
    pub const SELL: u32 = 0x3903;
    /// Search for a deposit (field, gold mine).
    pub const FIND_DEPOSIT: u32 = 0x3A03;
    /// `{a, b}`: sinking (timer after destroying, presentation only).
    pub const START_SINK: u32 = 0x3B03;
    /// Fire out (timer after destroying, presentation only).
    pub const CLEAR_FIRES: u32 = 0x3C03;
    /// Open gate (`PropertyBuilding_OpenDoor` `0x00506AA0`, re/arming-gates 3.C2).
    pub const OPEN_DOOR: u32 = 0x2303;
    /// Close gate (re/arming-gates 3.C2).
    pub const CLOSE_DOOR: u32 = 0x2403;
    /// Hand the group to slot 8 (`OnMsgAbandonToNeutral` `0x00506C60`, re/arming-gates 3.D2).
    pub const ABANDON_TO_NEUTRAL: u32 = 0x2703;
    /// Check the area for a capturing player (`OnMsgCheckCaptureArea` `0x00507CD0`, re/arming-gates 3.D3).
    pub const CHECK_CAPTURE_AREA: u32 = 0x2903;
    /// Register the capture watch (`OnMsgRegisterCaptureWatch` `0x00508CF0`, re/arming-gates 3.D2).
    pub const REGISTER_CAPTURE_WATCH: u32 = 0x2A03;
    /// Register the enemy watch (`RegisterEnemyAreaWatch` `0x00507DA0`, re/arming-gates 3.D1).
    pub const REGISTER_ENEMY_WATCH: u32 = 0x2B03;
    /// Collapse (timer 4.5 s after destroying).
    pub const COLLAPSE: u32 = 0x3D03;
    /// -> larger side of the footprint.
    pub const MAX_SIDE: u32 = 0x3E03;
    /// `{old, new}`: replace the ID in the farm work slots.
    pub const REPLACE_HANDLE: u32 = 0x3F03;
    /// Explosion (timer 700 ms after destroying a `BUILDINGEXPLODE` building).
    pub const EXPLODE: u32 = 0x4203;
    /// -> resting (`m_IsSleeping`): no attack target.
    pub const IS_SLEEPING: u32 = 0x4303;
}

/// `0x3303`: enlarge the occupancy rectangle by `n` per side.
pub const MSG_EXPAND_OCCUPANCY: u32 = msg::EXPAND_OCCUPANCY;

/// State of `CPropertyBuilding` (`0x3E8` bytes; fields and initial values after
/// `PropertyBuilding_Construct` `0x005075F0` and `PropertyBuilding_SerializeAndRestore`
/// `0x0050C4C0`). Fields that serve only the presentation (sinking, flag model) are missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Building {
    /// Footprint `[x0, z0, x1, z1]` inclusive (`+0x2C0`).
    pub rect: [i32; 4],
    /// Occupancy rectangle (`+0x2D0`).
    pub occupancy_rect: [i32; 4],
    /// Byte mask 20 x 20 (row z, column x, border 1): != 0 = blocked.
    pub mask: FixedVec<u8, MASK_LEN>,
    /// Path grid currently blocked (`+0x38C`).
    pub blocked: bool,
    /// Occupancy tiles that were free on setting, for the rollback (`+0x3CC`).
    pub freed: Vec<(i32, i32)>,
    /// `m_LastHorse` (`+0x70`): horse color rotation 0/1/2.
    pub last_horse: i32,
    /// `+0x74`: message "repair too expensive" shown. As in the original not in the savegame (and
    /// thus not in the checksum): `false` after loading, the message may appear again.
    #[serde(skip)]
    pub repair_msg_shown: bool,
    /// `m_NoResourcenAnnounced` (`+0x75`): message "deposit empty" shown.
    pub no_resources_announced: bool,
    /// `m_IsBuildBuilding` (`+0x76`): construction site counts in the counter "in progress".
    pub is_build_building: bool,
    /// `m_IsLoading` (`+0xA0`): init argument "finished".
    pub is_loading: bool,
    /// `m_BackupShowStatus` (`+0x9C`): render bits before hiding.
    pub render_bits_backup: u32,
    /// `m_IsExplode` (`+0x10C`) = Flag `BUILDINGEXPLODE`.
    pub is_explode: bool,
    /// `m_IsMsgSent` (`+0x10D`): "funds missing" already reported.
    pub msg_sent: bool,
    /// `m_UndergroundBuilding` (`+0x2A0`): GUID of the palisade under a tower (0 = none).
    pub underground_building: u32,
    /// `m_IsSleeping` (`+0x2A4`): production and construction rest.
    pub is_sleeping: bool,
    /// `m_IsDying` (`+0x2A5`).
    pub is_dying: bool,
    /// `m_HitpointsPerTick` (`+0x2A8`): `100000 / BuildTime + 1`.
    pub hp_per_tick: i32,
    /// `m_CostPerTick` (`+0x2AC`): costs `<< 12`, the first three divided by the build time.
    pub cost_per_tick: [i32; 5],
    /// `m_BuildCounter` (`+0x2E0`): remaining work.
    pub build_counter: i32,
    /// `m_BuildSecondCounter` (`+0x2E4`).
    pub build_second_counter: i32,
    /// `m_BuildTime` (`+0x2E8`): `max(1, Cfg13 · 100)`, never 0 (divisor).
    pub build_time: i32,
    /// `m_Target` (`+0x2EC`): deposit of a mine or a field (initial value ID 0).
    pub target: ObjectId,
    /// `m_RequestPos` (`+0x2F0`): pointer of the farm slots assigned round-robin (-1 ... 4).
    pub request_pos: i32,
    /// `m_RegisteredHandles` (`+0x2F4`): farm workers per slot.
    pub registered_handles: [ObjectId; FARM_SLOTS],
    /// `m_growed` (`+0x308`): field animation.
    pub growed: bool,
    /// `m_ParkingLots` (`+0x30C`): docking places.
    pub parking_lots: Vec<ParkingLot>,
    /// `m_MakeQueue` (`+0x320`): waiting GUIDs.
    pub make_queue: VecDeque<u32>,
    /// `m_MakeStartTime` (`+0x33C`): total duration in ms (0 = not yet paid).
    pub make_start: i32,
    /// `m_MakeTime` (`+0x340`): remaining duration in ms.
    pub make_time: i32,
    /// `m_MakeGuid` (`+0x344`): running entry (0 = none).
    pub make_guid: u32,
    /// `m_MatchIntBackup` (`+0x348`): flags after `Init`, written back on completion.
    pub flags_backup: u32,
    /// `m_MeetingPoint` (`+0x34C`): rally point (fixed-point grid).
    pub meeting_point: FxVec3,
    /// `m_MeetingTarget` (`+0x35C`): rally target; saved, but never tracked (in the original only
    /// when drawing the flag, deviation from the original).
    pub meeting_target: ObjectId,
    /// `m_IsDead` (`+0x360`).
    pub is_dead: bool,
    /// `m_MasterBuilding` (`+0x36C`): master of the wall group (ID 0 = none).
    pub master: ObjectId,
    /// `m_SlaveBuildings` (`+0x370`): members of the wall group.
    pub slaves: Vec<ObjectId>,
    /// `m_Anim` (`+0x390`, VALUEID 40004).
    pub anim: Vec<i32>,
    /// `m_curAnim` (`+0x39C`).
    pub cur_anim: i32,
    /// `m_BurnDown` (`+0x3A0`) = Flag `BURNDOWN`.
    pub burn_down: bool,
    /// `m_HeightmapInserted` (`+0x3A8`): entered in the `BuildingTileMap` (the map itself is in
    /// `tile_map.rs`).
    pub heightmap_inserted: bool,
    /// `m_AttackMode` (`+0x3AC`): attack mode of new units.
    pub attack_mode: i32,
    /// `m_LastTrackHID` (`+0x3A4`): last built track segment of the station (re/train-multipart
    /// 3.6); the ID may point to a removed track. Appended at the end (format 10).
    pub last_track: ObjectId,
    /// Rectangle of the area watch `{x0, z0, x1, z1}` in tiles (`+0xA4`, re/arming-gates 1.D; default
    /// 0). Appended at the end (format 13).
    pub check_area_rect: [i32; 4],
    /// Key of the area query of this building (`+0xB4`, -1 = none). Appended at the end
    /// (format 13).
    pub check_area_id: i32,
    /// Filter mode of the area query (`+0xB8`, `area::ENEMY_WATCH` or `area::CAPTURE_WATCH`; the
    /// reimplementation stores only the mode, not the filter). Appended at the end (format 13).
    pub check_area_mode: u32,
}

impl Default for Building {
    /// `PropertyBuilding_Construct` (`0x005075F0`).
    fn default() -> Self {
        Self {
            rect: [0; 4],
            occupancy_rect: [0; 4],
            mask: FixedVec::default(),
            blocked: false,
            freed: Vec::new(),
            last_horse: 0,
            repair_msg_shown: false,
            no_resources_announced: false,
            is_build_building: false,
            is_loading: false,
            render_bits_backup: 0,
            is_explode: false,
            msg_sent: false,
            underground_building: 0,
            is_sleeping: false,
            is_dying: false,
            hp_per_tick: 0,
            cost_per_tick: [0; 5],
            build_counter: 0,
            build_second_counter: 0,
            build_time: 1,
            target: ObjectId(0),
            request_pos: -1,
            registered_handles: [ObjectId::NONE; FARM_SLOTS],
            growed: false,
            parking_lots: Vec::new(),
            make_queue: VecDeque::new(),
            make_start: 0,
            make_time: 0,
            make_guid: 0,
            flags_backup: 0,
            meeting_point: FxVec3::ZERO,
            meeting_target: ObjectId::NONE,
            is_dead: false,
            master: ObjectId(0),
            slaves: Vec::new(),
            anim: Vec::new(),
            cur_anim: 0,
            burn_down: true,
            heightmap_inserted: false,
            attack_mode: 3,
            last_track: ObjectId::NONE,
            check_area_rect: [0; 4],
            check_area_id: -1,
            check_area_mode: 0,
        }
    }
}

pub(crate) fn building_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Building> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Building(b) => Some(&mut **b),
        _ => None,
    }
}

pub(crate) fn building<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Building> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Building(b) => Some(&**b),
        _ => None,
    }
}

/// `Module_HasFlags` of the building module.
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

fn module_flags(sim: &Sim, id: ObjectId) -> u32 {
    sim.module(id, INDEX).map_or(0, |m| m.base.flags)
}

/// Mask index of the tile `(x, z)` relative to the rectangle; none outside the 20 x 20 (in the original
/// a rectangle of more than 18 tiles would run beyond the field, exception A7; at most 8 in the
/// shipped data). Column and row are checked individually: a column that is too wide must not overflow
/// into the next mask row, the rectangle ends at the mask.
fn mask_index(rect: [i32; 4], x: i32, z: i32) -> Option<usize> {
    let (c, r) = (x - rect[0] + 1, z - rect[1] + 1);
    ((0..MASK_SIDE).contains(&c) && (0..MASK_SIDE).contains(&r))
        .then(|| (r * MASK_SIDE + c) as usize)
}

/// One-time warning that a building rectangle is larger than the mask (exception A7).
fn warn_if_larger_than_mask(rect: [i32; 4]) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let too_large = rect[2] - rect[0] + 1 > MASK_SIDE - 1 || rect[3] - rect[1] + 1 > MASK_SIDE - 1;
    if too_large && !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "Building rectangle {rect:?} larger than the mask ({MASK_SIDE} x {MASK_SIDE}): the mask ends at its edge"
        );
    }
}

/// `PropertyBuilding_ReadParams` (`0x00508590`, slot 4): flags 40003 over the run-time bits
/// (read twice, also into the backup `+0x348`), `m_BurnDown`, animation list 40004; an animated
/// **finished** building draws one table random number for its animation (on `Init` the flags are
/// 0 beforehand, so only on a variant change - quirk).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = desc.value_int(40003).unwrap_or(0) as u32;
    let text = desc.values.get(&40004).cloned().unwrap_or_default();
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = (m.base.flags & flags::KEPT_BY_READ_PARAMS) | value;
    }
    let burn_down = has_flags(sim, id, flags::BURNDOWN);
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    b.flags_backup = (b.flags_backup & flags::KEPT_BY_READ_PARAMS) | value;
    b.burn_down = burn_down;
    b.anim = super::stand::parse_int_list(&text, &b.anim);
    if has_flags(sim, id, flags::ISANIMATED) && has_flags(sim, id, flags::CONSTRUCTED_FINISHED) {
        let r = sim.state.rng.rand();
        let anim = building(sim, id).map_or(-1, |b| super::stand::list_modulo(&b.anim, r));
        if let Some(b) = building_mut(sim, id) {
            b.cur_anim = anim;
        }
        set_building_animation(sim, id, anim);
    }
}

/// `GameObject_SetAnimation(anim, 128.0, 0, 0, 0)` (`0x00466780`): wanted, without restart and
/// without immediate handover.
fn set_building_animation(sim: &mut Sim, id: ObjectId, anim: i32) {
    if let Some(o) = sim.object_mut(id) {
        o.set_animation_speed(anim, 0, 128.0, false, false, false);
    }
}

/// `GameObject_GetFootprintTileRect` (`0x004664E0`, re/occupancy B1): the tile rectangle of the
/// model hull ([`Rules::building_shapes`](crate::rules::Rules)) at the current position and rotation
/// of the object (world matrix from `world_pos`, `rotation`, `scale`). Without the exception of the
/// alarm post 1610, which only `Init` knows: placing a tent (`0x2F03`, re/tents 3.5) uses the model
/// rectangle for it as well (W-254). Without an object `[0; 4]`.
pub(crate) fn footprint_rect(sim: &Sim, id: ObjectId) -> [i32; 4] {
    let Some(o) = sim.object(id) else {
        return [0; 4];
    };
    let m = world_matrix(o.world_pos, o.rotation, o.scale);
    let shape = sim.rules.building_shapes.get(&o.guid);
    let (center, half) = shape.map_or(([0.0; 3], [0.0; 3]), |s| (s.center, s.half));
    footprint_tile_rect(center, half, &m, sim.rules.map.cell)
}

/// `PropertyBuilding_Init` (`0x0050B150`) with `slot` (player slot of the creation) and `ready` =
/// pre-placed or created finished (`ObjDesc.flags & 0x900`), in the order of the original: flags via
/// `ReadParams`, grid part (rectangle, mask, clear the building site), build time and cost per build
/// step, docking places, rally point, tower over palisade, gate, occupancy, flag backup, then finished
/// (path grid) or construction site (`StartBuild`), last the deposit timer.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, slot: u8, ready: bool) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = 0;
    }
    if let Some(b) = building_mut(sim, id) {
        b.is_loading = ready;
        b.heightmap_inserted = true;
        b.flags_backup = 0;
    }
    read_params(sim, id, desc);
    let rules = sim.rules;
    let cell = rules.map.cell;
    let Some(o) = sim.object_mut(id) else { return };
    o.render_bits |= 3;
    // Tilt back; the world matrix from the raw position (before snapping the height).
    o.rotation[0] = 0.0;
    o.rotation[2] = 0.0;
    let (guid, tile, grid, direction) = (o.guid, o.tile, o.grid, o.direction);
    let rect = if guid == ALARM_POST {
        [tile.0, tile.1, tile.0, tile.1]
    } else {
        footprint_rect(sim, id)
    };
    warn_if_larger_than_mask(rect);
    let mut mask = FixedVec::<u8, MASK_LEN>::default();
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            if let Some(i) = mask_index(rect, x, z) {
                mask[i] = 1;
            }
        }
    }
    // `PropertyBuilding_CarveFreeNodesFromMask`: helper nodes `frei01…` open their tile.
    for v in rules
        .building_shapes
        .get(&guid)
        .map_or(&[][..], |s| &s.free_nodes)
    {
        let (px, pz) = local_offset_to_grid(*v, grid, direction, cell);
        let (c, r) = ((px >> 12) + 1 - rect[0], (pz >> 12) + 1 - rect[1]);
        if 0 < c && c < 19 && 0 < r && r < 19 {
            mask[(r * MASK_SIDE + c) as usize] = 0;
        }
    }
    if let Some(b) = building_mut(sim, id) {
        b.rect = rect;
        b.occupancy_rect = rect;
        b.mask = mask;
    }
    destroy_objects_on_footprint(sim, id);
    init_build_costs(sim, id, slot, guid);
    set_flags(sim, id, flags::CONSTRUCTED_FINISHED);
    if let Some(b) = building_mut(sim, id) {
        b.parking_lots =
            parking::build_parking_lots(&b.mask, (rect[0] << 12) - 0x800, (rect[1] << 12) - 0x800);
    }
    let meeting = match get_exit_pos(sim, id, None) {
        (true, p) => p,
        (false, _) => {
            let g = sim.object(id).map_or(FxVec3::ZERO, |o| o.grid);
            FxVec3::new(g.x, g.y, Fx(g.z.0.wrapping_sub(3 << 12)))
        }
    };
    if let Some(b) = building_mut(sim, id) {
        b.meeting_point = meeting;
        b.meeting_target = ObjectId::NONE;
        b.build_second_counter = 0;
    }
    if has_flags(sim, id, flags::TURM) {
        hide_palisade_under_tower(sim, id);
    }
    if has_flags(sim, id, flags::TOR) {
        // `PropertyBuilding_InitGate` (`0x00506820`, re/arming-gates 3.C1), also for the construction
        // site; afterwards `SetFlags(TOROPEN)` (before the flag backup, so that completion restores
        // `TOROPEN` without `OpenDoor`) and the timer `0x3303 {0}` (set the occupancy again).
        gate::init_gate(sim, id);
        set_flags(sim, id, flags::TOROPEN);
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.post_delayed_message(
                100,
                Message::new(msg::EXPAND_OCCUPANCY, &[0]),
                false,
                GROUP,
            );
        }
    }
    mark_occupancy_rect(sim, id, true);
    if has_flags(sim, id, flags::PALISADE)
        && has_flags(sim, id, flags::PALISADECORNER)
        && let Some(o) = sim.object_mut(id)
    {
        o.attributes.set_modifier(18, 0, false, 0);
    }
    let backup = module_flags(sim, id);
    if let Some(b) = building_mut(sim, id) {
        b.flags_backup = backup;
    }
    if ready {
        if has_flags(sim, id, flags::ISANIMATED) {
            let anim = building(sim, id).map_or(0, |b| b.cur_anim);
            set_building_animation(sim, id, anim);
        }
        mark_footprint(sim, id, true, true);
    } else {
        if has_flags(sim, id, flags::FARM) || has_flags(sim, id, flags::GOLDMINE) {
            mark_footprint(sim, id, true, true);
        }
        start_build(sim, id);
    }
    let explode = has_flags(sim, id, flags::BUILDINGEXPLODE);
    if let Some(b) = building_mut(sim, id) {
        b.is_explode = explode;
    }
    clear_flags(sim, id, flags::MAKINGGUID);
    if (has_flags(sim, id, flags::FARM) || has_flags(sim, id, flags::GOLDMINE))
        && let Some(m) = sim.module_mut(id, INDEX)
    {
        m.base
            .post_delayed_message(100, Message::new(msg::FIND_DEPOSIT, &[]), false, GROUP);
    }
}

/// Build time, cost and hit points per build step (part of `PropertyBuilding_Init`):
/// `BuildTime = max(1, PlayerConfig_GetValue(slot, guid, 13) · 100)`; the costs
/// (`Player_GetObjectCostsFixed` of the owner, `<< 12`) stay 0 without an owner or value set,
/// only the first three are divided (quirk: population and value 0 stay undivided);
/// `HitpointsPerTick = 100000 / BuildTime + 1`.
fn init_build_costs(sim: &mut Sim, id: ObjectId, slot: u8, guid: u32) {
    let build_time = sim
        .rules
        .config_value(slot, guid, 13)
        .wrapping_mul(100)
        .max(1);
    let owner = id.slot();
    let fixed = sim
        .state
        .players
        .get(usize::from(owner))
        .and_then(|_| crate::economy::object_costs_fixed(sim, i32::from(owner), guid));
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    b.build_time = build_time;
    if let Some(c) = fixed {
        b.cost_per_tick = c;
    }
    for v in b.cost_per_tick.iter_mut().take(3) {
        *v = v.wrapping_div(build_time);
    }
    b.hp_per_tick = 100_000 / build_time + 1;
}

/// `PropertyBuilding_StartBuild` (`0x00504840`): begin a construction site - tilt back, position set
/// again, selectable, render bits 4, construction progress 0.1, not dead, 1 hit point, counter "in
/// progress" +1, remaining work = build time, flags cleared except `TRACK`/`PALISADE`/`PALISADECORNER`.
pub fn start_build(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object_mut(id) else { return };
    o.rotation[0] = 0.0;
    let (grid, guid) = (o.grid, o.guid);
    crate::world::set_grid_position(sim, id, grid, true);
    let p = sim
        .object(id)
        .map_or(0, |o| sim.type_upgrade_percent(o.owner(), o.guid, 14));
    let Some(o) = sim.object_mut(id) else { return };
    o.selectable = true;
    o.render_bits = 4;
    o.progress_scale = f32::from_bits(0x3DCC_CCCD);
    // `GameObject_UpdateDamageFires(0)`: presentation only.
    o.set_status_flags(status::DIED, false);
    o.attributes.set_current_value(14, 1, p);
    crate::economy::add_owner_pending_guid_count(sim.state, id, guid, 1);
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    b.is_build_building = true;
    b.build_counter = b.build_time;
    clear_flags(sim, id, flags::CLEARED_BY_START_BUILD);
    if let Some(b) = building_mut(sim, id) {
        b.is_dead = false;
    }
}

/// `PropertyBuilding_HidePalisadeUnderTower` (`0x00508B70`): an own palisade within half a tile
/// around the tower center is put to sleep (`0x1F03`); the tower remembers its GUID.
fn hide_palisade_under_tower(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let pos = (o.grid.x.0, o.grid.z.0);
    let filter = MatchFilter {
        mode: 0x112,
        match_bits: flags::PALISADE,
        ..MatchFilter::constructed()
    };
    let owner = i32::from(id.slot());
    let Some(palisade) = find_nearest_matching(sim, pos, &filter, 0x800, owner, -1) else {
        return;
    };
    if palisade == id {
        log::debug!("Tower {id:?}: finds itself as a palisade");
        return;
    }
    send(sim, palisade, &Message::new(msg::SET_SLEEPING, &[]));
    let guid = sim.object(palisade).map_or(0, |o| o.guid);
    if let Some(b) = building_mut(sim, id) {
        b.underground_building = guid;
    }
}

/// `PropertyBuilding_OnMsgSetSleeping` (`0x00504D80`, `0x1F03`): sleep, construction site again,
/// path grid free, render bits saved and 0, not selectable, state dead.
fn on_set_sleeping(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(b) = building_mut(sim, id) {
        b.is_sleeping = true;
    }
    start_build(sim, id);
    mark_footprint(sim, id, false, true);
    let bits = sim.object(id).map_or(0, |o| o.render_bits);
    if let Some(b) = building_mut(sim, id) {
        b.render_bits_backup = bits;
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = 0;
        o.selectable = false;
        o.set_status_flags(status::DIED, true);
    }
    0
}

/// `PropertyBuilding_GetExitPos(out, unit)` (`0x00509F60`): helper node `ausgang`; if it is missing,
/// the building center and `false`. If the node lies outside the path grid, `false` with the node
/// position; with a unit (walk layer via `0xB00`, != -1) it counts whether its footprint there is
/// not blocked in the terrain (`< 0x3FFFFFFE`).
pub fn get_exit_pos(sim: &mut Sim, id: ObjectId, unit: Option<ObjectId>) -> (bool, FxVec3) {
    let Some(p) = helper_node_map_position(sim, id, "ausgang") else {
        return (false, sim.object(id).map_or(FxVec3::ZERO, |o| o.grid));
    };
    if !sim.state.path.inside(p.x.0 >> 12, p.z.0 >> 12) {
        return (false, p);
    }
    if let Some(u) = unit {
        let layer = send(sim, u, &Message::new(0xB00, &[]));
        if layer != -1 {
            let l = usize::try_from(layer).unwrap_or(usize::MAX);
            let c = crate::collision::footprint_terrain_cost(sim, p.x.0, p.z.0, u, l);
            return (c < crate::motion::step::COST_BLOCKED, p);
        }
    }
    (true, p)
}

/// `PropertyBuilding_ProjectPointToRectEdge` (`0x00506EB0`): point `p` projected onto the edge of
/// the footprint (rectangle in fixed point, upper edge exclusive) from its center; a point in the
/// interior gives the center.
pub fn project_to_edge(rect: [i32; 4], p: (i32, i32)) -> (i32, i32) {
    let fx = |v: i32| v.wrapping_shl(12);
    let (x0, z0) = (fx(rect[0]), fx(rect[1]));
    let (x1, z1) = (
        fx(rect[2]).wrapping_add(0x1000),
        fx(rect[3]).wrapping_add(0x1000),
    );
    let cx = x1.wrapping_add(x0) / 2;
    let cz = z1.wrapping_add(z0) / 2;
    let (hw, hh) = (cx.wrapping_sub(x0), cz.wrapping_sub(z0));
    let (dx, dz) = (p.0.wrapping_sub(cx), p.1.wrapping_sub(cz));
    if dz.wrapping_abs() < hh && dx.wrapping_abs() < hw {
        return (cx, cz);
    }
    if dz == 0 {
        return (if dx >= 0 { x1 } else { x0 }, cz);
    }
    let t = fixed_div(fixed_mul(hh, dx), dz.wrapping_abs());
    if t.wrapping_abs() <= hw {
        return (cx.wrapping_add(t), if dz > 0 { z1 } else { z0 });
    }
    if dx == 0 {
        return (cx, cz);
    }
    let u = fixed_div(fixed_mul(hw, dz), dx.wrapping_abs());
    (if dx > 0 { x1 } else { x0 }, cz.wrapping_add(u))
}

/// `PropertyBuilding_DestroyObjectsOnFootprint` (`0x00507280`): per mask tile (z outer) remove at
/// most one object immediately - first nature of slot 10 within one tile around the tile center (also on
/// neighboring tiles outside the mask), otherwise a tree within half a tile.
fn destroy_objects_on_footprint(sim: &mut Sim, id: ObjectId) {
    let nature = MatchFilter {
        mode: 0x114,
        prop_id: 5,
        ..MatchFilter::default()
    };
    let tree = MatchFilter {
        mode: 0x1014,
        prop_id: 4,
        ..MatchFilter::default()
    };
    let Some(b) = building(sim, id) else { return };
    let (rect, mask) = (b.rect, b.mask.clone());
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            if mask_index(rect, x, z).is_none_or(|i| mask[i] == 0) {
                continue;
            }
            let center = ((x << 12) + 0x800, (z << 12) + 0x800);
            let hit =
                find_nearest_matching(sim, center, &nature, 0x1000, i32::from(NATURE_SLOT), -1)
                    .or_else(|| {
                        find_nearest_matching(sim, center, &tree, 0x800, i32::from(TREE_SLOT), -1)
                    });
            if let Some(victim) = hit {
                crate::world::destroy_object(sim, victim);
            }
        }
    }
}

/// `PropertyBuilding_MarkOccupancyRect` (`0x00508310`): setting remembers the previously free
/// tiles; the rollback clears only those (without the freed list the whole rectangle).
fn mark_occupancy_rect(sim: &mut Sim, id: ObjectId, set: bool) {
    let Some(b) = building(sim, id) else { return };
    let (rect, freed) = (b.occupancy_rect, b.freed.clone());
    if !set && !freed.is_empty() {
        for (x, z) in freed {
            sim.state.occupancy.set(x, z, false);
        }
        return;
    }
    let mut newly_freed = Vec::new();
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            if set && !sim.state.occupancy.test(x, z) {
                newly_freed.push((x, z));
            }
            sim.state.occupancy.set(x, z, set);
        }
    }
    if let Some(b) = building_mut(sim, id) {
        b.freed.extend(newly_freed);
    }
}

/// A footprint or occupancy rectangle lies within the bounds of the load check: at most one mask
/// width beyond the map (`map_w` x `map_h` tiles); otherwise `mark_footprint` and
/// `mark_occupancy_rect` would run over arbitrarily many tiles. Empty or reversed rectangles stay
/// valid. The load check ([`crate::module::ModuleKind::check`]) and the command `0x59`
/// ([`crate::command::ai`]) check the same way.
pub(crate) fn rect_within_map(r: &[i32; 4], map_w: i32, map_h: i32) -> bool {
    let (w, h) = (
        map_w.saturating_add(MASK_SIDE),
        map_h.saturating_add(MASK_SIDE),
    );
    (r[0] >= -MASK_SIDE && r[0] <= w)
        && (r[2] >= -MASK_SIDE && r[2] <= w)
        && (r[1] >= -MASK_SIDE && r[1] <= h)
        && (r[3] >= -MASK_SIDE && r[3] <= h)
}

/// `PropertyBuilding_MarkFootprint` (`0x00504C80`): block or release every mask tile in all layers
/// (then terrain anew); only on a change of state, never without a footprint (`TRACK`).
fn mark_footprint(sim: &mut Sim, id: ObjectId, set: bool, flush: bool) {
    if has_flags(sim, id, flags::TRACK) {
        return;
    }
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    if b.blocked == set {
        return;
    }
    b.blocked = set;
    let (rect, mask) = (b.rect, b.mask.clone());
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            if mask_index(rect, x, z).is_some_and(|i| mask[i] != 0) {
                mark_tile_for_layers(sim, u32::MAX, x, z, set);
            }
        }
    }
    if flush {
        sim.state.path.flush_dirty_rect();
    }
}

/// `PropertyBuilding_Tick` (`0x0050CD30`, slot 13): first the timers (`Module_UpdateEvents`), then
/// the deposit mirror of mine and field ([`deposit::mirror_deposit`]), then burning down,
/// construction site flag and damage image ([`construction::tick_state`]).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    deposit::mirror_deposit(sim, id);
    construction::tick_state(sim, id);
}

/// `PropertyBuilding_Query` (`0x0050BDE0`, slot 7); unknown codes -1. The envelope entry
/// (`OnMessage`, slot 12) is the base: commands posted to a building fizzle.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let hid = |i: usize| ObjectId::from_i32(m.arg(i));
    match m.code {
        msg::START_BUILD => construction::on_start_build(sim, id),
        msg::BUILD_TICK => construction::on_build_tick(sim, id, m),
        msg::SET_SLAVE_LIST => construction::on_set_slave_list(sim, id, m),
        msg::REMOVE_SLAVE => construction::on_remove_slave(sim, id, m),
        msg::SET_MASTER => construction::on_set_master(sim, id, m),
        msg::ADD_SLAVE => construction::on_add_slave(sim, id, m),
        msg::GET_MASTER => construction::on_get_master(sim, id, m),
        msg::GET_GOLD => deposit::on_get_resource(sim, id, m.arg(0), true),
        msg::GET_FOOD => deposit::on_get_resource(sim, id, m.arg(0), false),
        msg::FIND_DEPOSIT => deposit::on_find_deposit(sim, id),
        msg::FARMER_POS => farm::on_get_farmer_pos(sim, id, m),
        msg::FARM_QUERY_SLOT => {
            let (h, idx) = (hid(0), m.arg(1));
            building_mut(sim, id).map_or(-1, |b| farm::on_query_slot(b, h, idx))
        }
        msg::FARM_CLAIM_SLOT => farm::on_claim_slot(sim, id, hid(0), m.arg(1)),
        msg::FARM_RELEASE_SLOT => {
            let h = hid(0);
            building_mut(sim, id).map_or(-1, |b| farm::on_release_slot(b, h))
        }
        msg::REPLACE_HANDLE => {
            let (old, new) = (hid(0), hid(1));
            building_mut(sim, id).map_or(-1, |b| farm::on_replace_handle(b, old, new))
        }
        msg::ENQUEUE => production::enqueue(sim, id, m.arg(0) as u32),
        msg::PROGRESS => production::on_progress(sim, id, m),
        msg::LIST => production::on_list(sim, id, m),
        msg::CANCEL => production::cancel(sim, id, m.arg(0)),
        msg::SET_MEETING_POINT => rally::on_set_meeting_point(sim, id, m.arg(0), m.arg(1)),
        msg::SEND_TO_MEETING_POINT => i32::from(rally::send_unit_to_meeting_point(
            sim,
            id,
            ObjectId::from_i32(m.arg(0)),
        )),
        msg::NEAREST_DOCK => parking::get_nearest_dock_pos(sim, id, m),
        msg::REGISTER_DOCK => building_mut(sim, id).map_or(-1, |b| {
            parking::register_dock_pos(b, m.arg(0), ObjectId::from_i32(m.arg(1)))
        }),
        msg::UNREGISTER_DOCK => building_mut(sim, id).map_or(-1, |b| {
            parking::unregister_dock_pos(b, ObjectId::from_i32(m.arg(0)))
        }),
        msg::MEETING_POINT => {
            let Some(b) = building(sim, id) else {
                return -1;
            };
            let p = b.meeting_point;
            m.set_arg(0, p.x.0);
            m.set_arg(1, p.z.0);
            0
        }
        msg::SET_SLEEPING => on_set_sleeping(sim, id),
        // `PropertyBuilding_OnMsgRaiseLowHp` (`0x00504C10`): raises HP below 20 % to 20 % only if
        // `FUN_004527C0(guid)` holds - it always returns false (`XOR AL, AL`), the branch is dead;
        // answer 0.
        msg::RAISE_LOW_HP => 0,
        msg::EXIT_POS => {
            // `PropertyBuilding_OnMsgGetExitPos` (`0x0050A120`): x, y, z (height before z).
            let (_, p) = get_exit_pos(sim, id, None);
            m.set_arg(0, p.x.0);
            m.set_arg(1, p.y.0);
            m.set_arg(2, p.z.0);
            0
        }
        msg::DOCK_REGION => parking::dock_region_index(sim, id, m),
        msg::UNBLOCK_FOOTPRINT | msg::BLOCK_FOOTPRINT => {
            // `0x00504F70` / `0x00504F90`: the senders change the collision manager.
            let set = m.code == msg::BLOCK_FOOTPRINT;
            mark_footprint(sim, id, set, m.arg(0) != 0);
            0
        }
        msg::EXPAND_OCCUPANCY => {
            // `PropertyBuilding_OnMsgExpandOccupancy` (`0x00508E60`); the border is limited by the
            // command (`command::economy::MAX_BORDER`), computed with overflow like x86. Answer 0
            // (`0x00508EA0 XOR EAX, EAX`; the query jumps in with `JMP`).
            let n = m.arg(0);
            let Some(b) = building_mut(sim, id) else {
                return -1;
            };
            let r = &mut b.occupancy_rect;
            *r = [
                r[0].wrapping_sub(n),
                r[1].wrapping_sub(n),
                r[2].wrapping_add(n),
                r[3].wrapping_add(n),
            ];
            mark_occupancy_rect(sim, id, true);
            0
        }
        msg::DOCK_OF => parking::occupied_dock_pos(sim, id, m),
        msg::PROJECT_TO_EDGE => {
            // `PropertyBuilding_OnMsgProjectToEdge` (`0x00508190`).
            let Some(b) = building(sim, id) else {
                return -1;
            };
            let (x, z) = project_to_edge(b.rect, (m.arg(0), m.arg(1)));
            m.set_arg(0, x);
            m.set_arg(1, z);
            1
        }
        // `PropertyBuilding_GetAttackMode` (`0x00505010`): attack mode for entering ranged fighters
        // (`GameObject_EnterContainer`).
        msg::GET_ATTACK_MODE => building(sim, id).map_or(-1, |b| b.attack_mode),
        // `PropertyBuilding_OnMsgSetBehavior` (`0x00504FB0`): only on a change `m_AttackMode`,
        // `0x1D17 {mode}` to the own quarters (`0x00504FD8`) and the UI (`0x00469380(2)`); answer
        // 0.
        msg::SET_ATTACK_MODE => {
            let mode = m.arg(0);
            let Some(b) = building_mut(sim, id) else {
                return -1;
            };
            if b.attack_mode != mode {
                b.attack_mode = mode;
                send(
                    sim,
                    id,
                    &Message::new(super::quarter::msg::SET_GARRISON_BEHAVIOR, &[mode]),
                );
            }
            0
        }
        msg::IS_SLEEPING => building(sim, id).map_or(-1, |b| i32::from(b.is_sleeping)),
        msg::DESTROY => {
            destroy::destroy(sim, id);
            1
        }
        msg::SELL => destroy::on_sell(sim, id),
        // `PropertyBuilding_OnMsgStartSink` (`0x00507200`): start point and speed of sinking -
        // presentation only (`FrameUpdate`), the simulation does not read the fields.
        msg::START_SINK => 1,
        // `0x00505080`: `GameObject_UpdateDamageFires(0)` - presentation only.
        msg::CLEAR_FIRES => 1,
        msg::COLLAPSE => destroy::on_collapse(sim, id),
        msg::FINALLY_DEATH => destroy::on_finally_death(sim, id),
        msg::WAKE => destroy::on_wake(sim, id),
        msg::EXPLODE => destroy::on_explode(sim, id),
        msg::MAX_SIDE => building(sim, id).map_or(-1, |b| {
            // `0x00505050`.
            (b.rect[2] - b.rect[0] + 1).max(b.rect[3] - b.rect[1] + 1)
        }),
        msg::HIDE => tent::on_hide(sim, id),
        msg::TENT_PLACED => tent::on_tent_placed(sim, id),
        // `0x00506AA0`/`0x00506B80`: open/close gate from the query (argument 0 = not forced).
        msg::OPEN_DOOR => gate::open_door(sim, id, false),
        msg::CLOSE_DOOR => gate::close_door(sim, id, false),
        // Enemy watch, neutralizing and capturing of the wall group (re/arming-gates 3.D1/3.D2,
        // [`watch`]): `0x2B03` as a timer from `CaptureByPlayer`, `0x2703` from the watcher.
        msg::REGISTER_ENEMY_WATCH => watch::register_enemy_area_watch(sim, id),
        msg::ABANDON_TO_NEUTRAL => watch::on_abandon_to_neutral(sim, id),
        msg::REGISTER_CAPTURE_WATCH => watch::on_register_capture_watch(sim, id),
        msg::CHECK_CAPTURE_AREA => watch::on_check_capture_area(sim, id),
        // `0x0050BA00`: `!PropertyBuilding_SpawnProducedUnit(guid, slot)` (command `0x2C`,
        // re/headhunter-robbing 2.3); slots outside 0-8 -> 0 like `CreateObject`.
        msg::SPAWN_UNIT => {
            let slot = u8::try_from(m.arg(1)).ok().filter(|&s| s <= 8).unwrap_or(0);
            if rally::spawn_produced_unit(sim, id, m.arg(0) as u32, slot) {
                0
            } else {
                1
            }
        }
        // 2 of the 57 codes of `0x0050BDE0` deliberately answer -1 (re/economy): `0x2603` renderer.
        // `0x4003` is omitted: the building map is in the savegame (deviation from the original), in the original it
        // only stamps anew after loading.
        0x2603 | 0x4003 => {
            log::debug!(
                "Building {id:?}: query {:#x} is not implemented yet (-1)",
                m.code
            );
            -1
        }
        _ => -1,
    }
}

/// `PropertyBuilding_Destruct` (`0x00508400`): first the counters of the production
/// ([`production::release_pending_counts`]); finished -> population and living space from the
/// attributes 0x16/0x1B back to the owner, otherwise type counter +1 (balances the -1 of
/// `GameWorld_DestroyObject`: unfinished was never counted; after `Destroy` and collapse
/// `CONSTRUCTED_FINISHED` is cleared, `Destroy` has already booked the object counter) and a
/// construction site "in progress" -1; then the occupancy, then release the path grid; finally
/// deregister a running area query (`check_area_id ≠ −1`) (`FUN_004B68E0`, re/arming-gates 3.D1).
/// Its watcher dies with the module.
pub fn destruct(sim: &mut Sim, id: ObjectId) {
    production::release_pending_counts(sim, id);
    let slot = id.slot();
    if has_flags(sim, id, flags::CONSTRUCTED_FINISHED) {
        let population = sim.attribute(id, 0x16);
        let living = sim.attribute(id, 0x1B);
        if let Some(p) = sim.state.players.get_mut(usize::from(slot)) {
            p.add_resource(
                crate::economy::resource::POPULATION,
                population.wrapping_neg(),
            );
            p.add_resource(
                crate::economy::resource::LIVING_SPACE,
                living.wrapping_neg(),
            );
        }
    } else if let Some(guid) = sim.object(id).map(|o| o.guid) {
        sim.state.add_guid_count(slot, guid, 1);
        if building(sim, id).is_some_and(|b| b.is_build_building) {
            crate::economy::add_owner_pending_guid_count(sim.state, id, guid, -1);
        }
    }
    mark_occupancy_rect(sim, id, false);
    mark_footprint(sim, id, false, true);
    let key = building(sim, id).map_or(-1, |b| b.check_area_id);
    if key != -1 {
        crate::area::unregister(sim, key);
    }
}

/// `PropertyBuilding_OnOwnerChanged(old, new)` (`0x00506CF0`, slot 5; `id` is the new ID):
/// every existing member gets `0x1D03 {new}` (new master), the own member list is cleared (quirk:
/// the master no longer knows its members afterwards); an existing master gets `0x2103 {new}` (the
/// old ID stays in its list, quirk); every existing farm worker `0xA0C {new}`. At the end a gate
/// that falls to slot 8 (neutral) opens with a sound: `HasFlags(TOR)` and `slot == 8` ->
/// `OpenDoor(0)` ([`gate::open_door`], W-270, re/arming-gates 3.C2). Living space and type counter
/// are booked by `GameObject_OnOwnerChanged` or `GameWorld_ChangeObjectOwner` (`crate::world`).
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId) {
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    let slaves = std::mem::take(&mut b.slaves);
    let (master, handles) = (b.master, b.registered_handles);
    for s in slaves {
        if sim.object(s).is_some() {
            send(sim, s, &Message::new(msg::SET_MASTER, &[id.as_i32()]));
        }
    }
    if master != ObjectId(0) && sim.object(master).is_some() {
        send(sim, master, &Message::new(msg::ADD_SLAVE, &[id.as_i32()]));
    }
    for h in handles {
        if sim.object(h).is_some() {
            send(sim, h, &Message::new(0xA0C, &[id.as_i32()]));
        }
    }
    if has_flags(sim, id, flags::TOR) && id.slot() == crate::id::NEUTRAL_SLOT {
        gate::open_door(sim, id, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Fx, FxVec3};
    use crate::path::BLOCKED;
    use crate::rules::{BuildingShape, Rules};
    use crate::state::SimState;
    use crate::world::{ObjectDesc, RAD_TO_DEG, create_object, desc_flags, wrap_angle};

    /// Hull of the `SPA_Hauptquartier` (1000).
    pub(crate) fn spa_hq() -> BuildingShape {
        let f = f32::from_bits;
        BuildingShape {
            center: [f(0x3F82_3B80), f(0x439B_AB3F), f(0x41FF_1998)],
            half: [f(0x4399_C662), f(0x439B_401D), f(0x4345_B4F9)],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        }
    }

    #[test]
    fn footprints_follow_the_original_matrix_and_rounding() {
        let s = spa_hq();
        let rect = |rot: f32| {
            let m = world_matrix([1000.0, 50.0, 2000.0], [0.0, rot, 0.0], [1.0; 3]);
            footprint_tile_rect(s.center, s.half, &m, [128; 3])
        };
        use std::f32::consts::{FRAC_PI_2, PI};
        assert_eq!(rect(0.0), [5, 14, 10, 17]);
        assert_eq!(rect(FRAC_PI_2), [6, 13, 8, 17]);
        assert_eq!(rect(-FRAC_PI_2), [6, 13, 9, 17]);
        assert_eq!(rect(PI), [5, 13, 10, 16]);
        // World matrix at pi/2: cos is not 0.
        let m = world_matrix([1000.0, 50.0, 2000.0], [0.0, FRAC_PI_2, 0.0], [1.0; 3]);
        assert_eq!(m[0].map(f32::to_bits), [0xB33B_BD2E, 0, 0x3F80_0000, 0]);
        assert_eq!(m[2].map(f32::to_bits), [0xBF80_0000, 0, 0xB33B_BD2E, 0]);
        assert_eq!(m[3], [1000.0, 50.0, 2000.0, 1.0]);
    }

    #[test]
    fn free_nodes_turn_with_the_table_sines_of_whole_degrees() {
        // `SPA_Tor` at the start point (5000, 5000): frei01 (-67.505; 5.378), frei02 (57.336; 5.378).
        let grid = FxVec3::new(Fx(159_746), Fx(0), Fx(157_695));
        let dir = |rot: f32| Fx(wrap_angle(Fx::from_f32(rot * RAD_TO_DEG).0 - (90 << 12)));
        let tile = |v: [f32; 3], rot: f32| {
            let (x, z) = local_offset_to_grid(v, grid, dir(rot), [128; 3]);
            (x >> 12, z >> 12)
        };
        let (a, b) = ([-67.505, 0.0, 5.378], [57.336, 0.0, 5.378]);
        assert_eq!((tile(a, 0.0), tile(b, 0.0)), ((38, 38), (39, 38)));
        let quarter = std::f32::consts::FRAC_PI_2;
        assert_eq!((tile(a, quarter), tile(b, quarter)), ((38, 37), (38, 38)));
    }

    #[test]
    fn mask_indices_end_at_the_mask_instead_of_wrapping_into_the_next_row() {
        // Rectangle 25 tiles wide: columns from 19 are outside the mask (no wrap).
        let rect = [10, 10, 34, 10];
        assert_eq!(mask_index(rect, 10, 10), Some((MASK_SIDE + 1) as usize));
        assert_eq!(mask_index(rect, 28, 10), Some((MASK_SIDE + 19) as usize));
        for x in 29..=34 {
            assert_eq!(mask_index(rect, x, 10), None, "x = {x}");
        }
        // Rows beyond the mask likewise.
        assert_eq!(mask_index([0, 0, 0, 30], 0, 25), None);
        // The border -1 does not belong to the mask.
        assert_eq!(mask_index([5, 5, 6, 6], 3, 5), None);
    }

    const TYPES: &str = "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 3700\nNAME Busch\nVALUEID 100 6 1 512\nPROPID 5\nVALUEID 60001 1 1 1\nGUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\n";

    #[test]
    fn buildings_clear_their_site_block_all_layers_and_free_them_again() {
        let mut rules = Rules::synthetic(TYPES, 32);
        rules.building_shapes.insert(1000, spa_hq());
        rules.block_path.insert(3700);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let at = |guid, x: f32, z: f32| ObjectDesc::new(guid, 0, [x, 0.0, z]);
        // Bush on (6, 15) in the rectangle, bush on the neighboring tile (4, 15) to the left of it,
        // tree on (9, 16), bush far away.
        let bush = create_object(&mut sim, &mut at(3700, 6.5 * 128.0, 15.5 * 128.0)).unwrap();
        let neighbour = create_object(&mut sim, &mut at(3700, 4.5 * 128.0, 15.5 * 128.0)).unwrap();
        let tree = create_object(&mut sim, &mut at(3602, 9.5 * 128.0, 16.5 * 128.0)).unwrap();
        let far = create_object(&mut sim, &mut at(3700, 20.5 * 128.0, 20.5 * 128.0)).unwrap();
        assert_eq!(sim.state.path.layers[0].cell(6, 15), BLOCKED);
        let mut desc = at(1000, 1000.0, 2000.0);
        desc.flags |= desc_flags::PLACED;
        let hq = create_object(&mut sim, &mut desc).unwrap();
        // Bush, neighboring bush (the border tile (5, 15) has no nature of its own) and tree removed.
        for gone in [bush, neighbour, tree] {
            assert!(sim.object(gone).is_none(), "{gone:?}");
        }
        assert!(sim.object(far).is_some());
        let b = building(&sim, hq).unwrap().clone();
        assert_eq!((b.rect, b.blocked), ([5, 14, 10, 17], true));
        for l in 0..9 {
            assert_eq!(sim.state.path.layers[l].cell(5, 14), BLOCKED, "layer {l}");
            assert_eq!(sim.state.path.layers[l].cell(10, 17), BLOCKED, "layer {l}");
        }
        assert_eq!(sim.state.path.layers[0].cell(4, 15), 0);
        assert_eq!(sim.state.path.dirty, Default::default());
        // Occupancy: the rectangle; the tree had already occupied (9, 16) -> not in the freed list.
        assert!(sim.state.occupancy.test(5, 14) && sim.state.occupancy.test(10, 17));
        assert_eq!(b.freed.len(), 6 * 4 - 1);
        // Blocking border `0x3303` with n = 1; answer 0 (`0x00508EA0 XOR EAX, EAX`).
        assert_eq!(
            crate::messaging::send(&mut sim, hq, &Message::new(0x3303, &[1])),
            0
        );
        assert!(sim.state.occupancy.test(4, 13) && sim.state.occupancy.test(11, 18));
        // Demolition: occupancy only of the remembered tiles, path grid after the terrain.
        crate::world::destroy_object(&mut sim, hq);
        assert!(!sim.state.occupancy.test(4, 13) && !sim.state.occupancy.test(5, 14));
        assert!(
            sim.state.occupancy.test(9, 16),
            "quirk: the tree's bit stays"
        );
        assert_eq!(sim.state.path.layers[0].cell(5, 14), 0);
        assert_eq!(sim.state.path.layers[4].cell(5, 14), BLOCKED);
    }

    /// Building types of the init checks: flags (40003) and animation list (40004) per type.
    const ECONOMY_TYPES: &str = "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 1002\nNAME Muehle\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 262144\nVALUEID 40004 1 1 4 7 9\nGUID 1003\nNAME Feld\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 512\nGUID 1004\nNAME Tor\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 84017152\nGUID 1050\nNAME Palisade\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 1048576\nGUID 1051\nNAME Turm\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 65536\n";

    /// Rules with `default.dat` values: HQ 500 wood, 90 s, 4000 HP, population 1; house
    /// 150 wood, 20 s, 900 HP; palisade 10 s.
    fn economy_rules() -> Rules {
        let mut rules = Rules::synthetic(ECONOMY_TYPES, 32);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        rules
            .defaults
            .insert(1000, set(&[(11, 500), (13, 90), (14, 4000), (22, 1)]));
        rules
            .defaults
            .insert(1001, set(&[(11, 150), (13, 20), (14, 900)]));
        rules.defaults.insert(1050, set(&[(13, 10), (14, 100)]));
        rules
    }

    fn place(sim: &mut Sim, guid: u32, slot: i32, pos: (f32, f32), ready: bool) -> ObjectId {
        let mut desc = ObjectDesc::new(guid, slot, [pos.0, 0.0, pos.1]);
        if ready {
            desc.flags |= desc_flags::PLACED;
        }
        create_object(sim, &mut desc).unwrap()
    }

    #[test]
    fn build_time_costs_and_hitpoints_per_step_follow_the_config() {
        let rules = economy_rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let b = building(&sim, hq).unwrap();
        // 90 s * 100; wood (500 << 12) / 9000 = 227; population 1 << 12 undivided (quirk).
        assert_eq!((b.build_time, b.hp_per_tick), (9000, 12));
        assert_eq!(b.cost_per_tick, [0, 227, 0, 4096, 0]);
        let house = place(&mut sim, 1001, 1, (2000.0, 2000.0), true);
        let b = building(&sim, house).unwrap();
        assert_eq!((b.build_time, b.hp_per_tick), (2000, 51));
        assert_eq!(b.cost_per_tick, [0, 307, 0, 0, 0]);
        // Without a build time 1 (never 0), without a player (neutral) no costs.
        let mill = place(&mut sim, 1002, 0, (3000.0, 2000.0), true);
        assert_eq!(building(&sim, mill).unwrap().hp_per_tick, 100_001);
        let neutral = place(&mut sim, 1000, 8, (1000.0, 3000.0), true);
        let b = building(&sim, neutral).unwrap();
        assert_eq!((b.build_time, b.cost_per_tick), (9000, [0; 5]));
        // Finished: flags with `CONSTRUCTED_FINISHED`, backed up; initial values of the constructor.
        let b = building(&sim, hq).unwrap();
        assert!(has_flags(&sim, hq, flags::CONSTRUCTED_FINISHED));
        assert_eq!(b.flags_backup, 0x400);
        assert!(b.heightmap_inserted && b.is_loading && !b.burn_down && !b.is_build_building);
        assert_eq!((b.attack_mode, b.request_pos, b.build_counter), (3, -1, 0));
        assert_eq!(sim.state.pending_counts[0].get(&1000), None);
    }

    #[test]
    fn construction_sites_start_with_one_hitpoint_and_count_as_pending() {
        let rules = economy_rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let site = place(&mut sim, 1000, 0, (1000.0, 2000.0), false);
        let o = sim.object(site).unwrap();
        // 1 of 4000 HP = share 25; render bits 4, construction progress 0.1, selectable.
        assert_eq!(o.attributes.current_fraction(14), 25);
        assert_eq!((o.render_bits, o.selectable), (4, true));
        assert_eq!(o.progress_scale.to_bits(), 0x3DCC_CCCD);
        let b = building(&sim, site).unwrap();
        assert!(b.is_build_building && !b.is_dead && !b.blocked);
        assert_eq!(b.build_counter, 9000);
        // The backup keeps "finished" for completion, the flags do not.
        assert_eq!((module_flags(&sim, site), b.flags_backup), (0, 0x400));
        assert_eq!(crate::economy::pending_guid_count(&sim, 0, 1000), 1);
        // A field blocks its path grid already as a construction site; the deposit timer exists
        // only when finished (StartBuild has already cleared the flag, quirk).
        let field = place(&mut sim, 1003, 0, (2000.0, 2000.0), false);
        assert!(building(&sim, field).unwrap().blocked);
        assert!(sim.module(field, INDEX).unwrap().base.timers_ws.is_empty());
        let ready = place(&mut sim, 1003, 0, (3000.0, 2000.0), true);
        let timers = &sim.module(ready, INDEX).unwrap().base.timers_ws;
        assert_eq!(timers.len(), 1);
        assert_eq!(timers[0].msg, Message::new(msg::FIND_DEPOSIT, &[]));
        assert_eq!(timers[0].b, 100);
    }

    #[test]
    fn read_params_draws_a_table_random_only_for_finished_animated_buildings() {
        let rules = economy_rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let start = sim.state.rng.index();
        let mill = place(&mut sim, 1002, 0, (1000.0, 2000.0), true);
        let site = place(&mut sim, 1002, 0, (2000.0, 2000.0), false);
        let hq = place(&mut sim, 1000, 0, (3000.0, 2000.0), true);
        // `Init` draws nothing (flags 0 beforehand); finished animated: animation `m_curAnim` = 0.
        assert_eq!(sim.state.rng.index(), start);
        let b = building(&sim, mill).unwrap();
        assert_eq!((b.anim.clone(), b.cur_anim), (vec![4, 7, 9], 0));
        assert_eq!(sim.object(mill).unwrap().anim.wanted, 0);
        let desc = |guid: u32| rules.types[&guid].module(3).unwrap().clone();
        // Variant change: construction site and non-animated building draw nothing.
        crate::module::read_params(&mut sim, site, INDEX, &desc(1002));
        crate::module::read_params(&mut sim, hq, INDEX, &desc(1000));
        assert_eq!(sim.state.rng.index(), start);
        // Finished and animated: exactly one value of the table sequence.
        crate::module::read_params(&mut sim, mill, INDEX, &desc(1002));
        assert_eq!(sim.state.rng.index(), start + 1);
        let r = i32::from(crate::rng::RAND_TABLE[usize::from(start) + 1]);
        let expected = [4, 7, 9][(r % 3) as usize];
        assert_eq!(building(&sim, mill).unwrap().cur_anim, expected);
        assert_eq!(sim.object(mill).unwrap().anim.wanted, expected);
        // The run-time bits stay, flags and backup read 40003 anew.
        assert_eq!(module_flags(&sim, mill), 0x40400);
        assert!(!building(&sim, mill).unwrap().burn_down);
    }

    #[test]
    fn the_meeting_point_is_the_exit_node_or_three_tiles_in_front() {
        let mut rules = economy_rules();
        let mut shape = spa_hq();
        shape
            .helper_nodes
            .insert("ausgang".into(), [200.5, 10.0, -300.0]);
        rules.building_shapes.insert(1000, shape);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let o = sim.object(hq).unwrap();
        let (g, dir) = (o.grid, o.direction);
        let (x, z) = local_offset_to_grid([200.5, 10.0, -300.0], g, dir, [128; 3]);
        let y = (10 << 12) / 128 + g.y.0;
        assert_eq!(
            building(&sim, hq).unwrap().meeting_point,
            FxVec3::new(Fx(x), Fx(y), Fx(z))
        );
        assert!(building(&sim, hq).unwrap().meeting_target.is_none());
        let mut m = Message::new(msg::MEETING_POINT, &[]);
        assert_eq!(crate::messaging::send_mut(&mut sim, hq, &mut m), 0);
        assert_eq!(m.args, [x, z]);
        let mut m = Message::new(msg::EXIT_POS, &[]);
        assert_eq!(crate::messaging::send_mut(&mut sim, hq, &mut m), 0);
        assert_eq!(m.args, [x, y, z]);
        // Without a node: three tiles in -z; `0x2503` then returns the center.
        let house = place(&mut sim, 1001, 0, (2000.0, 2000.0), true);
        let g = sim.object(house).unwrap().grid;
        assert_eq!(
            building(&sim, house).unwrap().meeting_point,
            FxVec3::new(g.x, g.y, Fx(g.z.0 - 3 * 4096))
        );
        let mut m = Message::new(msg::EXIT_POS, &[]);
        crate::messaging::send_mut(&mut sim, house, &mut m);
        assert_eq!(m.args, [g.x.0, g.y.0, g.z.0]);
    }

    #[test]
    fn points_project_onto_the_footprint_edge() {
        // Rectangle [5, 14, 10, 17]: x 20480 ... 45056, z 57344 ... 73728, center (32768, 65536).
        let rect = [5, 14, 10, 17];
        assert_eq!(project_to_edge(rect, (33_000, 60_000)), (32_768, 65_536));
        assert_eq!(project_to_edge(rect, (100_000, 65_536)), (45_056, 65_536));
        assert_eq!(project_to_edge(rect, (0, 65_536)), (20_480, 65_536));
        assert_eq!(project_to_edge(rect, (32_768, 0)), (32_768, 57_344));
        assert_eq!(project_to_edge(rect, (32_768, 90_000)), (32_768, 73_728));
        // Over the long side: t = Fixed_Div(Fixed_Mul(8192, 67232), 4464) = 123376 > 12288,
        // so the right edge with z + Fixed_Div(Fixed_Mul(12288, 4464), 67232) = z + 812.
        assert_eq!(project_to_edge(rect, (100_000, 70_000)), (45_056, 66_348));
    }

    #[test]
    fn building_queries_answer_in_their_arguments() {
        let mut rules = economy_rules();
        rules.building_shapes.insert(1000, spa_hq());
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place(&mut sim, 1000, 0, (1000.0, 2000.0), true);
        let send_mut = crate::messaging::send_mut;
        let mut m = Message::new(msg::PROJECT_TO_EDGE, &[100_000, 70_000]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 1);
        assert_eq!(m.args, [45_056, 66_348]);
        assert_eq!(
            send_mut(&mut sim, hq, &mut Message::new(msg::MAX_SIDE, &[])),
            6
        );
        // Path grid free and blocked again (with clearing).
        send_mut(
            &mut sim,
            hq,
            &mut Message::new(msg::UNBLOCK_FOOTPRINT, &[1]),
        );
        assert!(!building(&sim, hq).unwrap().blocked);
        assert_eq!(sim.state.path.layers[0].cell(5, 14), 0);
        send_mut(&mut sim, hq, &mut Message::new(msg::BLOCK_FOOTPRINT, &[1]));
        assert!(building(&sim, hq).unwrap().blocked);
        assert_eq!(sim.state.path.layers[0].cell(5, 14), BLOCKED);
        // Unknown codes -1, envelopes fizzle (the base reports `true`).
        assert_eq!(send_mut(&mut sim, hq, &mut Message::new(0x9903, &[])), -1);
        let env = crate::message::Envelope::bare(Message::new(msg::MAX_SIDE, &[]));
        assert_eq!(crate::messaging::dispatch_command(&mut sim, hq, &env), 1);
    }

    #[test]
    fn towers_put_the_palisade_below_to_sleep_and_gates_keep_a_timer() {
        let rules = economy_rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let palisade = place(&mut sim, 1050, 0, (1000.0, 1000.0), true);
        let lone = place(&mut sim, 1051, 0, (2000.0, 2000.0), true);
        let tower = place(&mut sim, 1051, 0, (1000.0, 1000.0), true);
        assert_eq!(building(&sim, lone).unwrap().underground_building, 0);
        assert_eq!(building(&sim, tower).unwrap().underground_building, 1050);
        let o = sim.object(palisade).unwrap();
        assert_eq!((o.render_bits, o.selectable), (0, false));
        assert_ne!(o.status & status::DIED, 0);
        let b = building(&sim, palisade).unwrap();
        // Sleeping again a construction site (render bits 4 backed up), palisade bit stays.
        assert!(b.is_sleeping && b.is_build_building && !b.blocked);
        assert_eq!((b.render_bits_backup, b.build_counter), (4, 1000));
        assert_eq!(module_flags(&sim, palisade), 0x10_0000);
        assert_eq!(crate::economy::pending_guid_count(&sim, 0, 1050), 1);
        // Gate: open (`TOROPEN` from the gate init) and the occupancy timer; explosion and running
        // production from the type flags (`0x5000000`): explodes, queue bit cleared.
        let gate = place(&mut sim, 1004, 0, (3000.0, 1000.0), true);
        assert!(has_flags(&sim, gate, flags::TOROPEN) && !has_flags(&sim, gate, flags::MAKINGGUID));
        assert!(building(&sim, gate).unwrap().is_explode);
        let timers = sim.module(gate, INDEX).unwrap().base.timers_ws.clone();
        assert_eq!(timers.len(), 1);
        assert_eq!(timers[0].msg, Message::new(msg::EXPAND_OCCUPANCY, &[0]));
        // The building tick drives the timers: take over, 100 ms, it fires after 200 ms.
        for _ in 0..3 {
            crate::module::tick(&mut sim, gate, INDEX, 100);
        }
        let base = &sim.module(gate, INDEX).unwrap().base;
        assert!(base.events.is_empty() && base.timers_ws.is_empty());
        assert_eq!(base.cur_prop_time, 200);
    }

    thread_local! {
        static GARRISON: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_garrison(_: &mut Sim, _: ObjectId, _: usize, m: &Message) {
        if m.code == super::super::quarter::msg::SET_GARRISON_BEHAVIOR {
            GARRISON.with(|g| g.borrow_mut().push(m.arg(0)));
        }
    }

    /// `0x3503` (`0x00504FB0`): only a change stores the mode and goes to the own quarters as
    /// `0x1D17`; `0x4303` reports `m_IsSleeping`.
    #[test]
    fn behaviour_changes_reach_the_quarter_and_sleeping_is_answered() {
        use crate::combat::testkit as ck;
        use crate::messaging::send_mut;
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = ck::spawn(&mut sim, ck::HQ, 0, (20, 20));
        assert_eq!(building(&sim, hq).unwrap().attack_mode, 3);
        GARRISON.with(|g| g.borrow_mut().clear());
        crate::module::test_hook::observe(record_garrison, || {
            for mode in [3, 1, 1, 4] {
                let mut m = Message::new(msg::SET_ATTACK_MODE, &[mode]);
                assert_eq!(send_mut(&mut sim, hq, &mut m), 0);
            }
        });
        assert_eq!(GARRISON.with(|g| g.borrow().clone()), [1, 4]);
        assert_eq!(building(&sim, hq).unwrap().attack_mode, 4);
        let mut m = Message::new(msg::IS_SLEEPING, &[]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 0);
        building_mut(&mut sim, hq).unwrap().is_sleeping = true;
        assert_eq!(send_mut(&mut sim, hq, &mut m), 1);
    }
}
