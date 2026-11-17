//! Module 37 `CPropertyKiller` (`PropertyKiller_Construct` `0x0051B280`, vtable `0x00624998`,
//! `0x88` bytes, re/special-attacks 1.3): killer. Near (flag `0x100`) it stabs the target with the
//! knife, far (`0x200`) it shoots from cover.
//!
//! This file holds the state, `Init`, and the near and far attack ([`query`], [`abort`],
//! re/special-attacks 3.4, 3.5, 3.9). Never busy (slot 10 `0x00532A90`), change of owner empty,
//! tick only `Module_UpdateEvents`.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::collision::edge_distance_to_object;
use crate::combat::damage::damage_message;
use crate::combat::hit_zone::{attach_blood, world_pos};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::spell::pay_mana;
use crate::module::{ModuleKind, init_flags};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::{FINDTO, FINDTO_UNIT, GROUP, lock_animation, order_cost, post, release_animation};

/// Module index Killer.
pub const INDEX: usize = 0x25;
/// Arrival at the victim: distance <= 2 tiles (`0x00672988`, re/special-attacks 1.6).
pub const REACH: i32 = 2 * 4096;
/// Walk maximum radius (`0x0067298C`).
pub const WALK_MAX: i32 = 2048;
/// Muzzle flash when sniping (`0x624A04`; rendering).
const MUZZLE: &str = "data/effects/normaler_schuss.pef";
/// Muzzle node of the shooter (`0x624A28`).
const MUZZLE_NODE: &str = "gun01";

/// Messages to module 37 (query `0x0051BD30`, re/special-attacks 2.2).
pub mod msg {
    /// `{target, button}`: near attack order (`0x0051B610`).
    pub const KILL: u32 = 0x125;
    /// Arrival at the victim (`0x0051B680`).
    pub const KILL_AT: u32 = 0x225;
    /// Knife hit, time marker 2 of the animation (`0x0051B310`).
    pub const KILL_HIT: u32 = 0x325;
    /// `{target, button}`: far attack order, sniping (`0x0051B7D0`).
    pub const SNIPE: u32 = 0x425;
    /// Shot, time marker 3 (`0x0051B9A0`).
    pub const SNIPE_SHOT: u32 = 0x525;
    /// End of sniping (`0x0051B5B0`).
    pub const SNIPE_END: u32 = 0x625;
    /// Turn to the victim completed (`0x0051B500`).
    pub const SNIPE_TURNED: u32 = 0x725;
}

/// `CPropertyKiller`, fields of the savegame (`SerializeFields` `0x0051BDB0`, re/special-attacks 1.3,
/// 5, 10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Killer {
    /// `m_KillNearAnim` (`+0x70`, 380002): knife animation.
    pub near_anim: i32,
    /// `m_KillFarAnim` (`+0x74`, 380003): fire animation.
    pub far_anim: i32,
    /// `m_TargetHID` (`+0x78`): target; may be stale. The original leaves it uninitialized until the
    /// first order (memory garbage in the savegame, D-240); Rust `NONE`.
    pub target: ObjectId,
    /// `m_ExtraEnergy` (`+0x7C`): mana cost of the order; original uninitialized (D-240), Rust 0.
    pub extra_energy: i32,
    /// `m_KillNearDamage` (`+0x80`, 380004): near damage (HP).
    pub near_damage: i32,
    /// `m_KillFarDamage` (`+0x84`, 380005): far damage (HP).
    pub far_damage: i32,
}

impl Default for Killer {
    /// Deviation D-240: target `NONE`, cost 0 instead of memory garbage.
    fn default() -> Self {
        Self {
            near_anim: 0,
            far_anim: 0,
            target: ObjectId::NONE,
            extra_energy: 0,
            near_damage: 0,
            far_damage: 0,
        }
    }
}

pub fn killer<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Killer> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Killer(k) => Some(k),
        _ => None,
    }
}

pub fn killer_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Killer> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Killer(k) => Some(k),
        _ => None,
    }
}

/// `PropertyKiller_Init` (`0x0051B2B0`, slot 9, re/special-attacks 1.3): flags 380001, near animation
/// 380002, far animation 380003, near damage 380004, far damage 380005.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let (near_anim, far_anim) = (value(380_002), value(380_003));
    let (near_damage, far_damage) = (value(380_004), value(380_005));
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Killer(k) = &mut m.kind {
        k.near_anim = near_anim;
        k.far_anim = far_anim;
        k.near_damage = near_damage;
        k.far_damage = far_damage;
    }
}

/// `Query` (slot 7, `0x0051BD30`, re/special-attacks 2.2): the seven codes of the flow with return value
/// 0, unknown codes -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    match m.code {
        msg::KILL => kill(sim, id, m),
        msg::KILL_AT => kill_at(sim, id),
        msg::KILL_HIT => kill_hit(sim, id),
        msg::SNIPE => snipe(sim, id, m),
        msg::SNIPE_TURNED => snipe_turned(sim, id),
        msg::SNIPE_SHOT => snipe_shot(sim, id),
        msg::SNIPE_END => snipe_end(sim, id),
        _ => -1,
    }
}

/// Walk order synchronously via `GameObject_DispatchPostedCommand` (`0x00473690`) with success **and**
/// failure `back` to the own object.
fn walk_order(sim: &mut Sim, id: ObjectId, walk: Message, back: u32) {
    let env = Envelope {
        msg: walk,
        success: Some(Message::new(back, &[])),
        fault: Some(Message::new(back, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// Timer `code` without arguments after `t` ms ([`post`]).
fn post_code(sim: &mut Sim, id: ObjectId, t: i32, code: u32) {
    post(sim, id, INDEX, t, Message::new(code, &[]));
}

/// Near order `{target, button}` (`0x125` `0x0051B610`, re/special-attacks 3.4): cost = config 15 of the
/// button (if the set is missing -> 0 without a check); if the mana is not enough, silently 0 (deduction
/// not until the hit, K-320). Then remember the target and, if it exists (`GameWorld_GetObjectById(target, 0)`,
/// also dead objects), walk FINDTO_UNIT `{target, 0, 2048, 0, -1, 1, 0, 0}` (`0x0051B450`) with success
/// **and** failure `0x225`.
fn kill(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let (target, button) = (ObjectId::from_i32(m.arg(0)), m.arg(1));
    let (cost, affordable) = order_cost(sim, id, button);
    let Some(k) = killer_mut(sim, id) else {
        return 0;
    };
    k.extra_energy = cost;
    if !affordable {
        return 0;
    }
    k.target = target;
    if sim.object(target).is_some() {
        let args = [target.as_i32(), 0, WALK_MAX, 0, -1, 1, 0, 0];
        walk_order(sim, id, Message::new(FINDTO_UNIT, &args), msg::KILL_AT);
    }
    0
}

/// Arrival at the victim (`0x225` `0x0051B680`, re/special-attacks 3.4; also as failure of the run): target
/// missing -> 0; edge distance `GameObject_GetEdgeDistanceToObject(go, target)` (`0x0046A7F0`) >
/// [`REACH`] -> "victim ran away", 0; if the killer swims -> 0. Otherwise knife animation (speed 128),
/// `ANIM_LOCKED` on, attack module off, sound 6/`0x42` at the own world position (`0x00553080`,
/// rendering) and timer `0x325` after time marker 1 of the knife animation.
fn kill_at(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((target, anim)) = killer(sim, id).map(|k| (k.target, k.near_anim)) else {
        return 0;
    };
    if sim.object(target).is_none() {
        return 0;
    }
    if edge_distance_to_object(sim, id, target) > REACH {
        log::debug!("Killer {id:?}: victim ran away");
        return 0;
    }
    let Some(pos) = world_pos(sim, id) else {
        return 0;
    };
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    if o.status & status::SWIMMING != 0 {
        return 0;
    }
    o.set_animation(anim, 0, false, false, false);
    lock_animation(o);
    let guid = o.guid;
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: 6,
        flags: 0x42,
        pos,
        guid,
    });
    let t = sim.rules.anim_label_time_at(guid, anim, 1);
    post_code(sim, id, t, msg::KILL_HIT);
    0
}

/// Knife hit (`0x325` `0x0051B310`, re/special-attacks 3.4): `ANIM_LOCKED` off, attack module on
/// (if present); then the hit with -`m_KillNearDamage` without a distance check (C-331, [`hit`]).
fn kill_hit(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
    }
    let Some((target, damage, cost)) =
        killer(sim, id).map(|k| (k.target, k.near_damage, k.extra_energy))
    else {
        return 0;
    };
    if sim.object(target).is_none() {
        log::debug!("Killer {id:?}: victim died...");
        return 0;
    }
    hit(sim, id, target, damage, cost);
    0
}

/// Hit: `0x17F` with -`damage` from the own object (`DamageMsg_Construct(go, 0x14)`
/// `0x004F28E0`, amount replaced; normal hit with bonuses, experience and `NotifyAttackedBy`),
/// then deduct the mana without a check (K-320).
fn hit(sim: &mut Sim, id: ObjectId, target: ObjectId, damage: i32, cost: i32) {
    let m = damage_message(sim, id, damage.wrapping_neg());
    send(sim, target, &m);
    pay_mana(sim, id, cost);
}

/// Far order `{target, button}` (`0x425` `0x0051B7D0`, re/special-attacks 3.5): if the killer swims
/// -> 0 (before everything else); remember the target; if it is missing -> 0; cost like [`kill`]. Then the
/// face order FINDTO argc 7 on the spot: target = own grid position, look-at point = grid position of the
/// target, goal radius and tolerance -1, flags 1; success **and** failure `0x725`. The module does
/// not check range, sight and fog (C-332).
fn snipe(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let (target, button) = (ObjectId::from_i32(m.arg(0)), m.arg(1));
    let Some(own) = sim
        .object(id)
        .filter(|o| o.status & status::SWIMMING == 0)
        .map(|o| o.grid)
    else {
        return 0;
    };
    let Some(k) = killer_mut(sim, id) else {
        return 0;
    };
    k.target = target;
    let Some(at) = sim.object(target).map(|t| t.grid) else {
        return 0;
    };
    let (cost, affordable) = order_cost(sim, id, button);
    let Some(k) = killer_mut(sim, id) else {
        return 0;
    };
    k.extra_energy = cost;
    if !affordable {
        return 0;
    }
    let args = [own.x.0, own.z.0, -1, at.x.0, at.z.0, -1, 1];
    walk_order(sim, id, Message::new(FINDTO, &args), msg::SNIPE_TURNED);
    0
}

/// Turn finished (`0x725` `0x0051B500`, re/special-attacks 3.5): `ANIM_LOCKED` on, attack module
/// off, fire animation (speed 128), timer `0x525` after time marker 3 (shot) and `0x625` after
/// time marker 1 (end) of the fire animation.
fn snipe_turned(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(anim) = killer(sim, id).map(|k| k.far_anim) else {
        return 0;
    };
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    lock_animation(o);
    o.set_animation(anim, 0, false, false, false);
    let guid = o.guid;
    let shot = sim.rules.anim_label_time_at(guid, anim, 3);
    let end = sim.rules.anim_label_time_at(guid, anim, 1);
    post_code(sim, id, shot, msg::SNIPE_SHOT);
    post_code(sim, id, end, msg::SNIPE_END);
    0
}

/// Shot (`0x525` `0x0051B9A0`, re/special-attacks 3.5): if the target is missing (dead targets count as
/// present): "target is dead", [`abort`], 0. Otherwise sound 1/3 at the muzzle, muzzle flash at the
/// node `gun01`, blood on the target at the zone toward the muzzle ([`attach_blood`]: hit zone exactly
/// once, the counter turns, C-91; `blood.pef`, the host applies the blood switch as for arrow
/// and rifle (`0x0051BBAA`, O-392); no building/vehicle branch),
/// then the hit with -`m_KillFarDamage` ([`hit`]). Immediate hit without projectile, without
/// range, sight and line-of-sight check (C-332). Deviation: muzzle = own world position
/// (the node `gun01` is known only to the host).
fn snipe_shot(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((target, damage, cost)) =
        killer(sim, id).map(|k| (k.target, k.far_damage, k.extra_energy))
    else {
        return 0;
    };
    if sim.object(target).is_none() {
        log::debug!("Killer {id:?}: target is dead");
        abort(sim, id);
        return 0;
    }
    let (Some(muzzle), Some(guid)) = (world_pos(sim, id), sim.object(id).map(|o| o.guid)) else {
        return 0;
    };
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: 1,
        flags: 3,
        pos: muzzle,
        guid,
    });
    sim.state.events.push(SimEvent::EffectAttached {
        object: id,
        path: MUZZLE.into(),
        node: MUZZLE_NODE.into(),
        offset: [0.0; 3],
    });
    attach_blood(sim, target, muzzle);
    hit(sim, id, target, damage, cost);
    0
}

/// End of sniping (`0x625` `0x0051B5B0`): `ANIM_LOCKED` off, attack module on (if present).
fn snipe_end(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
    }
    0
}

/// `Abort` (slot 11, `0x0051B410`, re/special-attacks 3.9): at any time - `ANIM_LOCKED` off,
/// attack module on (if present), clear timer group `0x80` (pending hit or
/// shot and end are dropped, the mana stays); `m_TargetHID` stays; `true`.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    true
}
