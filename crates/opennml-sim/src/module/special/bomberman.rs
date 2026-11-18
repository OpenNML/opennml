//! Module 36 `CPropertyBomberman` (`PropertyBomberman_Construct` `0x00503EB0`, vtable `0x00622738`,
//! `0xA4` bytes, re/special-attacks 1.2): demolition expert. It places a dynamite charge on a building
//! (flag `0x100`) or ship (`0x200`); the charge is a world effect of the projectile list
//! (`ProjectileKind::Dynamite`, re/special-attacks 1.5).
//!
//! This file holds the state, `Init`, and the order, placing and igniting ([`query`], [`abort`],
//! re/special-attacks 3.1-3.3, 3.9). The siege engine `m_SiegeengineParkinglot` (dead branch, C-335)
//! is missing in the state and in the behavior. Never busy in the sense of `is_busy` (slot 10
//! `0x00532A90`), change of owner empty.

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::collision::edge_distance_to_object;
use crate::combat::hit_zone::world_pos;
use crate::combat::projectile::add_dynamite;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::spell::pay_mana;
use crate::module::{ModuleKind, init_flags};
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::sim::Sim;

use super::{FINDTO, GROUP, lock_animation, order_cost, post, release_animation};

/// Module index Bomberman.
pub const INDEX: usize = 0x24;
/// Arrival at the building: distance <= 4 tiles (`Fixed(1)` `0x006726A8` * 4, re/special-attacks 1.6).
pub const REACH: i32 = 4 * 4096;
/// Ship when igniting: distance <= 1 tile (`0x006726A8`).
pub const SHIP_REACH: i32 = 4096;
/// Fuse time of the charge in ms (`m_TimeOut`, `DynamiteEffect_Construct` `0x004F04D0`).
pub const FUSE_MS: i32 = 10_000;
/// Amount of the damage message of the explosion (`0x17F`, re/special-attacks 3.3).
pub const DAMAGE: i32 = -4000;
/// Walk FINDTO, running, if the walk has the flag `0x4000` (way back, re/special-attacks 4.2).
const FINDTO_RUN: u32 = 0x100;

/// Messages to module 36 (query `0x00503FD0`, re/special-attacks 2.1).
pub mod msg {
    /// `{target, button}`: order building (`0x00503B70`).
    pub const ORDER_BUILDING: u32 = 0x124;
    /// Arrival at the building: place charge (`0x00503560`).
    pub const AT_BUILDING: u32 = 0x224;
    /// End of the placing animation at the building: ignite charge (`0x00503860`).
    pub const FIRE_BUILDING: u32 = 0x324;
    /// `{target, button}`: order ship (`0x00503C90`).
    pub const ORDER_SHIP: u32 = 0x424;
    /// Arrival at the ship: place charge (`0x00503670`).
    pub const AT_SHIP: u32 = 0x524;
    /// End of the placing animation at the ship: ignite charge (`0x005039D0`).
    pub const FIRE_SHIP: u32 = 0x624;
}

/// `CPropertyBomberman`, fields of the savegame (`SerializeFields` `0x00504160`, re/special-attacks
/// 1.2, 5, 10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bomberman {
    /// `m_DynamiteObject` (`+0x70`, 370002): model of the charge (rendering).
    pub model: String,
    /// `m_DynamiteBuildingAnim` (`+0x74`, 370004): animation of the charge at the building (rendering).
    pub building_anim: i32,
    /// `m_DynamiteShipAnim` (`+0x78`, 370006): the same at the ship.
    pub ship_anim: i32,
    /// `m_PutDynamiteBuildingAnim` (`+0x7C`, 370003): own placing animation at the building.
    pub put_building_anim: i32,
    /// `m_PutDynamiteShipAnim` (`+0x80`, 370005): own placing animation at the ship.
    pub put_ship_anim: i32,
    /// `m_TargetHID` (`+0x84`): target; may be stale.
    pub target: ObjectId,
    /// `m_ExtraEnergy` (`+0x88`): mana cost of the order. The original leaves it uninitialized until the
    /// first order; Rust 0.
    pub extra_energy: i32,
    /// `m_ReturnPoint` (`+0x8C`): return point = own position at the order, in the order
    /// of `FxVec3` (grid x, y, z).
    pub return_point: [i32; 3],
    /// `m_IsBusy` (`+0xA0`): currently placing (between arrival and timer `0x324`/`0x624`).
    pub busy: bool,
}

impl Default for Bomberman {
    /// Default of the constructor: no target (`NONE`), otherwise 0/empty.
    fn default() -> Self {
        Self {
            model: String::new(),
            building_anim: 0,
            ship_anim: 0,
            put_building_anim: 0,
            put_ship_anim: 0,
            target: ObjectId::NONE,
            extra_energy: 0,
            return_point: [0; 3],
            busy: false,
        }
    }
}

pub fn bomberman<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Bomberman> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Bomberman(b) => Some(b),
        _ => None,
    }
}

pub fn bomberman_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Bomberman> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Bomberman(b) => Some(b),
        _ => None,
    }
}

/// `PropertyBomberman_Init` (`0x00504060`, slot 9, re/special-attacks 1.2): flags 370001, model
/// 370002, placing animation building 370003, charge animation building 370004, placing animation ship
/// 370005, charge animation ship 370006.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let model = desc.values.get(&370_002).cloned().unwrap_or_default();
    let (put_building, building) = (value(370_003), value(370_004));
    let (put_ship, ship) = (value(370_005), value(370_006));
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Bomberman(b) = &mut m.kind {
        b.model = model;
        b.put_building_anim = put_building;
        b.building_anim = building;
        b.put_ship_anim = put_ship;
        b.ship_anim = ship;
    }
}

/// `Query` (slot 7, `0x00503FD0`, re/special-attacks 2.1): the six codes of the flow, each with
/// return value 0; unknown codes also return **0** instead of -1 (K-321).
pub fn query(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    match m.code {
        msg::ORDER_BUILDING => order(sim, id, m, false),
        msg::ORDER_SHIP => order(sim, id, m, true),
        msg::AT_BUILDING => arrived(sim, id, false),
        msg::AT_SHIP => arrived(sim, id, true),
        msg::FIRE_BUILDING => fire(sim, id, false),
        msg::FIRE_SHIP => fire(sim, id, true),
        _ => 0,
    }
}

/// Order `{target, button}` (`0x124` `0x00503B70`, `0x424` `0x00503C90`, re/special-attacks 3.1):
/// return point = own grid position; if the target is missing (`GameWorld_GetObjectById(target, 0)`, also
/// dead objects), 0. Cost = config 15 of the button in the value set of the owner (if the set is missing ->
/// 0 without a check); if the mana is not enough, silently 0 (no message; deduction not until the effect,
/// K-320). Then remember the target and walk with success `0x224` or `0x524` without failure message (a
/// failure leaves the order hanging, C-327), synchronously via `GameObject_DispatchPostedCommand`
/// (`0x00473690`): building FINDTO argc 3 on the object; ship with collision exemption
/// (exemption object = ship, status `EXEMPT_ZONE`) FINDTO argc 6 on the grid position of the ship
/// without target direction. The siege branch (`0x1B2E`, module 46) is missing (C-335).
fn order(sim: &mut Sim, id: ObjectId, m: &Message, ship: bool) -> i32 {
    let (target, button) = (ObjectId::from_i32(m.arg(0)), m.arg(1));
    let Some(grid) = sim.object(id).map(|o| o.grid) else {
        return 0;
    };
    let Some(b) = bomberman_mut(sim, id) else {
        return 0;
    };
    b.return_point = [grid.x.0, grid.y.0, grid.z.0];
    let Some(at) = sim.object(target).map(|t| t.grid) else {
        return 0;
    };
    let (cost, affordable) = order_cost(sim, id, button);
    let Some(b) = bomberman_mut(sim, id) else {
        return 0;
    };
    b.extra_energy = cost;
    if !affordable {
        return 0;
    }
    b.target = target;
    let (walk, arrived) = if ship {
        if let Some(o) = sim.object_mut(id) {
            o.set_collision_exempt_object(target);
            o.set_status_flags(status::EXEMPT_ZONE, true);
        }
        let walk = Message::new(FINDTO, &[at.x.0, at.z.0, -1, NO_DIRECTION, -1, 1]);
        (walk, msg::AT_SHIP)
    } else {
        let walk = Message::new(FINDTO, &[target.as_i32(), -1, 1]);
        (walk, msg::AT_BUILDING)
    };
    let env = Envelope {
        msg: walk,
        success: Some(Message::new(arrived, &[])),
        fault: None,
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
    0
}

/// Arrival (`0x224` `0x00503560`, `0x524` `0x00503670`, re/special-attacks 3.2): target present and
/// edge distance (`GameObject_GetEdgeDistanceToObject(target, go)`) <= [`REACH`] -> `ANIM_LOCKED`,
/// attack module out via the pending list (without `HasModule` check), placing animation with speed
/// 128 like module 38, `busy` and timer `0x324`/`0x624` after time marker 1 of the placing animation
/// (group `0x80`, without repeat). Otherwise "could not reach target": at the ship
/// [`abort`], at the building nothing - `target` stays (C-327).
fn arrived(sim: &mut Sim, id: ObjectId, ship: bool) -> i32 {
    let Some((target, anim)) = bomberman(sim, id).map(|b| {
        let anim = if ship {
            b.put_ship_anim
        } else {
            b.put_building_anim
        };
        (b.target, anim)
    }) else {
        return 0;
    };
    let ok = sim.object(target).is_some() && edge_distance_to_object(sim, target, id) <= REACH;
    if !ok {
        log::debug!("Demolition expert {id:?}: could not reach target");
        if ship {
            abort(sim, id);
        }
        return 0;
    }
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    lock_animation(o);
    o.set_animation(anim, 0, false, false, false);
    let guid = o.guid;
    let t = sim.rules.anim_label_time_at(guid, anim, 1);
    let fire = if ship {
        msg::FIRE_SHIP
    } else {
        msg::FIRE_BUILDING
    };
    if let Some(b) = bomberman_mut(sim, id) {
        b.busy = true;
    }
    post(sim, id, INDEX, t, Message::new(fire, &[]));
    0
}

/// End of the placing animation (`0x324` `0x00503860`, `0x624` `0x005039D0`, re/special-attacks 3.3):
/// remember the world position; `ANIM_LOCKED` off and attack module on again (if present, also
/// switched off), at the ship also `EXEMPT_ZONE` off, `busy` off. If the target is missing: "target has
/// vanished" - at the ship [`abort`], otherwise nothing (no mana, no way back). Otherwise deduct the mana
/// only now without a check (K-320); at the ship with edge distance > [`SHIP_REACH`] only the charge
/// gets no target (C-328; `target` stays until `abort`), at the building no distance check (C-327).
/// Place the charge at the remembered position, then synchronously to the return point (`0x100`, running
/// with walk flag `0x4000`, C-326); at the ship [`abort`] afterwards, at the building `target` stays (C-327).
fn fire(sim: &mut Sim, id: ObjectId, ship: bool) -> i32 {
    let Some(pos) = world_pos(sim, id) else {
        return 0;
    };
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    release_animation(o);
    if ship {
        o.set_status_flags(status::EXEMPT_ZONE, false);
    }
    let Some(b) = bomberman_mut(sim, id) else {
        return 0;
    };
    b.busy = false;
    let (target, cost, rp) = (b.target, b.extra_energy, b.return_point);
    let anim = if ship { b.ship_anim } else { b.building_anim };
    if sim.object(target).is_none() {
        log::debug!("Demolition expert {id:?}: target has vanished");
        if ship {
            abort(sim, id);
        }
        return 0;
    }
    pay_mana(sim, id, cost);
    let charge = if ship && edge_distance_to_object(sim, target, id) > SHIP_REACH {
        log::debug!("Demolition expert {id:?}: ship is too far away");
        ObjectId::NONE
    } else {
        target
    };
    add_dynamite(sim, id, charge, pos, anim);
    let back = Message::new(FINDTO_RUN, &[rp[0], rp[2], -1, NO_DIRECTION, -1, 1]);
    send(sim, id, &back);
    if ship {
        abort(sim, id);
    }
    0
}

/// `Abort` (slot 11, `0x00503790`, re/special-attacks 3.9): if `busy` is set, returns `false`
/// without any change - timer, lock bit and `busy` stay, detonation and way back follow
/// (K-318). Otherwise clear timer group `0x80`, attack module on again (if present),
/// `ANIM_LOCKED` off; with a target set `EXEMPT_ZONE` off - also after the building branch, which never sets
/// the bit (K-319); target `NONE`; `true`.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(m) = sim.module_mut(id, INDEX) else {
        return true;
    };
    let ModuleKind::Bomberman(b) = &mut m.kind else {
        return true;
    };
    if b.busy {
        return false;
    }
    let had_target = !b.target.is_none();
    b.target = ObjectId::NONE;
    m.base.clear_event_group(GROUP);
    if let Some(o) = sim.object_mut(id) {
        release_animation(o);
        if had_target {
            o.set_status_flags(status::EXEMPT_ZONE, false);
        }
    }
    true
}
