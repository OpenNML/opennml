//! Module 49 `SPECIALMOVE` (no class name; constructor `0x0052B380`, vtable `0x00625D18`,
//! `0x84` bytes, re/special-attacks 1.4): smith with Whirlwind and "Damage building"
//! (hammering).
//!
//! This file holds the state, `Init` and both attacks
//! ([`query`], [`abort`], re/special-attacks 3.6, 3.7, 3.9). The own effect on the object
//! (`m_EffectID`, `schmied_rundumschlag.pef`/`schmied_gebäude.pef` at the node `schmiedhammer01`) is
//! rendering and missing in the state; the host derives it from `busy` and `mode` (deviation from
//! the original).
//! The module is never busy in the sense of `is_busy`, change of owner empty, tick only
//! `Module_UpdateEvents`.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::{edge_distance_to_point, query_radius};
use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::gather::list_at;
use crate::module::spell::{MANA, pay_mana};
use crate::module::stand::parse_int_list;
use crate::module::{ModuleKind, init_flags};
use crate::sim::Sim;
use crate::state::SimEvent;

use super::{FINDTO, GROUP, lock_animation, post, release_animation};

/// Module index SpecialMove.
pub const INDEX: usize = 0x31;
/// Radius of the whirlwind: 2 tiles (re/special-attacks 3.6).
pub const WHIRL_RADIUS: i32 = 2 * 4096;
/// Whirlwind: damage in percent of the maximum HP of the target.
pub const WHIRL_PERCENT: i32 = 60;
/// Hammering: damage in percent of the maximum HP of the building.
pub const HAMMER_PERCENT: i32 = 40;
/// Walk tolerance to the building (`0x006123E0`, from `0x00672CA0` = 4096): argc 3: tolerance,
/// outside 0...180 -> 5 degrees; radius from the kind of the target.
pub const WALK_TOLERANCE: i32 = 2048;

/// Messages to module 49 (query `0x0052BD30`, re/special-attacks 2.3).
pub mod msg {
    /// `{kind, ...}`: execute attack, kind 0 whirlwind, 1 hammering (`0x0052BCB0`).
    pub const EXECUTE: u32 = 0x131;
    /// Order "Damage building": walk to the building (`0x0052B440`).
    pub const GOTO_BUILDING: u32 = 0x231;
    /// Hit, time marker of the animation (`0x0052B5F0`).
    pub const HIT: u32 = 0x331;
    /// End of the animation (`0x0052B4E0`).
    pub const END: u32 = 0x431;
}

/// `SPECIALMOVE`, fields of the savegame (`SerializeFields` `0x0052BCE0`, re/special-attacks 1.4, 5,
/// 10.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialMove {
    /// `m_SpecialMoveAnims` (`+0x70`, 500002): animation [0] whirlwind, [1] hammering.
    pub anims: Vec<i32>,
    /// `m_IsTurnAround` (`+0x7C`): animation running (between start and `0x431`).
    pub busy: bool,
    /// Kind of the running attack for the host: 0 whirlwind, 1 hammering.
    pub mode: i32,
}

pub fn special_move<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s SpecialMove> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::SpecialMove(s) => Some(s),
        _ => None,
    }
}

pub fn special_move_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut SpecialMove> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::SpecialMove(s) => Some(s),
        _ => None,
    }
}

/// `Init` (`0x0052B690`, slot 9, re/special-attacks 1.4): flags 500001 (unused), animation list
/// 500002 (`IntList_ParseFromString`).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let text = desc.values.get(&500_002).cloned().unwrap_or_default();
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::SpecialMove(s) = &mut m.kind {
        s.anims = parse_int_list(&text, &[]);
    }
}

/// Probe distance in front of the smith: 1 tile (re/special-attacks 3.6).
const PROBE: i32 = 4096;
/// Start value of the search "smallest edge distance": 10 tiles (`Fixed(10)`, re/special-attacks 3.6).
const PROBE_MAX: i32 = 10 * 4096;
/// Sector width of the whirlwind in degrees.
const SECTOR_DEG: i32 = 45;
/// Damage message in percent of the maximum HP (`GameObject_HandleOwnMessage` `0x0046D840`).
const PERCENT_DAMAGE: u32 = 0x27F;

/// `Query` (slot 7, `0x0052BD30`, re/special-attacks 2.3): the four codes of the flow, unknown
/// codes -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    match m.code {
        msg::EXECUTE => execute(sim, id, m),
        msg::GOTO_BUILDING => goto_building(sim, id, m),
        msg::HIT => percent_hit(sim, id, m),
        msg::END => end(sim, id),
        _ => -1,
    }
}

/// `0x131 {mode, anim_idx, ...}` (`0x0052BCB0`): mode 0 -> whirlwind with the animation index,
/// mode 1 -> hammering with the whole message, otherwise nothing; returns 0.
fn execute(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    match m.arg(0) {
        0 => whirlwind(sim, id, m.arg(1)),
        1 => hammer(sim, id, m),
        _ => {}
    }
    0
}

/// Animation `i` of the list `m_SpecialMoveAnims` (`IntList_GetAt`, missing or negative -> 0, like
/// module 38; the index comes from a message).
fn anim_at(sim: &Sim, id: ObjectId, i: i32) -> Option<i32> {
    let s = special_move(sim, id)?;
    Some(usize::try_from(i).map_or(0, |i| list_at(&s.anims, i)))
}

/// Start of an attack: `m_IsTurnAround` and the kind for the host.
fn set_busy(sim: &mut Sim, id: ObjectId, mode: i32) {
    if let Some(s) = special_move_mut(sim, id) {
        s.busy = true;
        s.mode = mode;
    }
}

/// Whirlwind (`0x0052B6D0`, re/special-attacks 3.6, C-333): enemies with a walk module
/// (`MatchFilter` `0x414` = ANDALL | ENEMYLIST | CHECKPROPERTY, module 0) with center distance <=
/// [`WHIRL_RADIUS`] around the own grid position (`SpatialGrid_QueryRadius`, order of discovery). Empty ->
/// "Get Nearest has failed!", end (the command already deducted the mana, K-320). Otherwise
/// animation `anims[anim_idx]` (speed 128), `t2` = marker 2, `dt` = marker `0x16` - `t2`,
/// `ANIM_LOCKED` on, attack module off, `0x431` after marker 1, `busy`. Then eight 45-degree sectors from
/// the viewing direction, as long as candidates remain: probe point 1 tile ahead
/// (`Fixed_CosDeg`/`Fixed_SinDeg`, `Fixed_Mul`), the candidate with the smallest edge distance
/// (`GameObject_GetEdgeDistanceToPoint` `0x00468930`; strictly smaller, start 10 tiles, on a tie
/// the earlier one) leaves the list and gets `0x331 {ID, 60}` after `t`; `t` grows per sector by
/// `dt / 8` (direction 0). The timers of a handler lie in reverse in `m_Events`: equal ticks hit in
/// reverse sector order. Deviation: viewing direction without the direction offset `+0x168` of the siege
/// engine.
fn whirlwind(sim: &mut Sim, id: ObjectId, anim_idx: i32) {
    let Some((x, z, player)) = sim
        .object(id)
        .map(|o| (o.grid.x.0, o.grid.z.0, i32::from(o.owner())))
    else {
        return;
    };
    let filter = MatchFilter {
        mode: 0x414,
        prop_id: 0,
        ..MatchFilter::constructed()
    };
    let mut list = query_radius(sim, (x, z), &filter, WHIRL_RADIUS, player, false);
    if list.is_empty() {
        log::debug!("Smith {id:?}: Get Nearest has failed!");
        return;
    }
    let Some(anim) = anim_at(sim, id, anim_idx) else {
        return;
    };
    let Some(o) = sim.object_mut(id) else {
        return;
    };
    o.set_animation(anim, 0, false, false, false);
    let (guid, dir) = (o.guid, o.direction.0);
    let t2 = sim.rules.anim_label_time_at(guid, anim, 2);
    let dt = sim
        .rules
        .anim_label_time_at(guid, anim, 0x16)
        .wrapping_sub(t2);
    if let Some(o) = sim.object_mut(id) {
        lock_animation(o);
    }
    let end = sim.rules.anim_label_time_at(guid, anim, 1);
    post(sim, id, INDEX, end, Message::new(msg::END, &[]));
    set_busy(sim, id, 0);
    let step = dt / 8;
    let mut t = t2;
    for k in (0..360).step_by(SECTOR_DEG as usize) {
        if list.is_empty() {
            break;
        }
        let a = dir.wrapping_add(k << 12);
        let p = (
            x.wrapping_add(fixed_mul(cos_deg(a), PROBE)),
            z.wrapping_add(fixed_mul(sin_deg(a), PROBE)),
        );
        let mut best = (PROBE_MAX, None);
        for (i, &o) in list.iter().enumerate() {
            let d = edge_distance_to_point(sim, o, p);
            if d < best.0 {
                best = (d, Some(i));
            }
        }
        if let Some(i) = best.1 {
            let o = list.remove(i);
            let hit = Message::new(msg::HIT, &[o.as_i32(), WHIRL_PERCENT]);
            post(sim, id, INDEX, t, hit);
        }
        t = t.wrapping_add(step);
    }
}

/// Hammering (`0x131 {1, anim_idx, target, cost}`, `0x0052BB10`, re/special-attacks 3.7, C-334):
/// if the mana is not enough (`current(15) < cost`), silently 0 - the run was in vain (K-320); otherwise
/// deduct, `ANIM_LOCKED` on, attack module off, `busy`, animation `anims[anim_idx]` (speed 128),
/// `0x331 {target, 40}` after marker 2 (missing for animation 5 -> 0 -> next tick) and `0x431` after
/// marker 1. No distance and no target check (the target is not checked until `0x331`).
fn hammer(sim: &mut Sim, id: ObjectId, m: &Message) {
    let cost = m.arg(3);
    if sim.object(id).is_none() || sim.attribute(id, MANA) < cost {
        return;
    }
    pay_mana(sim, id, cost);
    let Some(anim) = anim_at(sim, id, m.arg(1)) else {
        return;
    };
    let Some(o) = sim.object_mut(id) else {
        return;
    };
    lock_animation(o);
    o.set_animation(anim, 0, false, false, false);
    let guid = o.guid;
    set_busy(sim, id, 1);
    let hit = sim.rules.anim_label_time_at(guid, anim, 2);
    let end = sim.rules.anim_label_time_at(guid, anim, 1);
    post(
        sim,
        id,
        INDEX,
        hit,
        Message::new(msg::HIT, &[m.arg(2), HAMMER_PERCENT]),
    );
    post(sim, id, INDEX, end, Message::new(msg::END, &[]));
}

/// Order "Damage building" `0x231 {1, anim_idx, target, cost}` (`0x0052B440`,
/// re/special-attacks 3.7): walk FINDTO argc 3 `{target, 2048, 1}` (tolerance argument) synchronously via
/// `GameObject_DispatchPostedCommand` (`0x00473690`), success `0x131` with the same four arguments,
/// without failure message; returns 0.
fn goto_building(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let args = [m.arg(0), m.arg(1), m.arg(2), m.arg(3)];
    let env = Envelope {
        msg: Message::new(FINDTO, &[m.arg(2), WALK_TOLERANCE, 1]),
        success: Some(Message::new(msg::EXECUTE, &args)),
        fault: None,
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
    0
}

/// Hit `0x331 {target, percent}` (`0x0052B5F0`, re/special-attacks 3.6): target present
/// (`GameWorld_GetObjectById`) -> synchronously `0x27F {-percent, own ID}` (-percent % of
/// max(14), experience only with enemy relation, C-125; `NotifyAttackedBy`), then the red
/// damage number (`FUN_00493030`, [`SimEvent::DamageNumber`]) with the return value; returns 0.
fn percent_hit(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let target = ObjectId::from_i32(m.arg(0));
    if sim.object(target).is_none() {
        return 0;
    }
    let damage = Message::new(PERCENT_DAMAGE, &[m.arg(1).wrapping_neg(), id.as_i32()]);
    let points = send(sim, target, &damage);
    sim.state.events.push(SimEvent::DamageNumber {
        object: target,
        points,
    });
    0
}

/// End `0x431` (`0x0052B4E0`): `ANIM_LOCKED` off, attack module on (if present),
/// `m_IsTurnAround` 0 (the effect ends at the host); returns 1.
fn end(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
    }
    if let Some(s) = special_move_mut(sim, id) {
        s.busy = false;
    }
    1
}

/// `Abort` (slot 11, `0x0052B550`, re/special-attacks 3.9): if `busy` is set, returns `false`
/// without any change - timer, lock bit and `busy` stay, hit and end follow (K-318).
/// Otherwise clear timer group `0x80`, `ANIM_LOCKED` off, attack module on (if
/// present); `true`.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(m) = sim.module_mut(id, INDEX) else {
        return true;
    };
    if let ModuleKind::SpecialMove(s) = &m.kind
        && s.busy
    {
        return false;
    }
    m.base.clear_event_group(GROUP);
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
    }
    true
}
