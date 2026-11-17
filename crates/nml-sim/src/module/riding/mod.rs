//! Module 13 `CPropertyRiding` and 14 `CPropertyRidingBoat` (re/riding): rider <-> horse
//! and occupant <-> canoe. Both modules are the same class with different constants (status bit,
//! failure and success message). This file holds the state, parameters, animation queries
//! (`0x50D`/`0x50E`), message dispatch, tick (throwing off 3377), abort and the load check
//! of the rider-mount link; mounting is in [`mount`], dismounting and letting the occupant off a
//! sinking canoe are in [`dismount`]. Releasing the link on dying is in the dying module
//! (`module::dying`, `unlink`, `sink_canoe`).

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{post_message, send_fault};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimState;

use super::events::update_events;
use super::stand::{list_modulo, parse_int_list};
use super::{Module, ModuleKind};

pub(crate) mod dismount;
pub(crate) mod mount;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;

/// Module index Riding (rider on horse).
pub const RIDING: usize = 0x0D;
/// Module index RidingBoat (occupant in canoe).
pub const RIDING_BOAT: usize = 0x0E;

/// Module flag `HAS_LANCE` (VALUEID 140001, only SPA_Konquistador 1103).
pub const HAS_LANCE: u32 = 0x100;
/// Module flag "lance ready"; set by `ReadParams` when `HAS_LANCE` is set (re/riding 1.1).
pub const LANCE_AVAILABLE: u32 = 0x200;

/// Places of the 14 animation lists, in the order of the savegame (re/riding 5). The place
/// also gives the VALUEID: `140002 + place` (Riding).
pub mod list {
    pub const WALK: usize = 0;
    pub const RUN: usize = 1;
    pub const STAND: usize = 2;
    pub const DIE: usize = 3;
    pub const FIGHT_NEAR: usize = 4;
    pub const FIGHT_FAR: usize = 5;
    pub const PRESENT: usize = 6;
    pub const RELOAD: usize = 7;
    pub const BANISH: usize = 8;
    pub const HORSE_FIGHT: usize = 9;
    pub const MOUNT_LEFT: usize = 10;
    pub const MOUNT_RIGHT: usize = 11;
    pub const UNMOUNT: usize = 12;
    pub const STAND_UP: usize = 13;
}

/// First VALUEID of the lists of Riding (140002 = `WALK`).
const LIST_VALUE_BASE: u32 = 140_002;
/// VALUEIDs of the lists of RidingBoat (`PropertyRidingBoat_Init`) with their place; the boat has no
/// running and none of the lists from `HORSE_FIGHT` on.
const BOAT_LISTS: [(u32, usize); 8] = [
    (150_002, list::WALK),
    (150_003, list::STAND),
    (150_004, list::DIE),
    (150_005, list::FIGHT_NEAR),
    (150_006, list::FIGHT_FAR),
    (150_007, list::PRESENT),
    (150_008, list::RELOAD),
    (150_009, list::BANISH),
];

/// `CPropertyRiding` (`0x138` bytes); the constants `+0x74...+0x80` follow from `boat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Riding {
    /// RidingBoat (module 14): status bit, messages and identifiers.
    pub boat: bool,
    /// `m_mountHID` (`+0x70`): target or mount, also during the approach.
    pub mount_target: ObjectId,
    /// The 14 animation lists (`+0x84...+0x120`), places in [`list`].
    pub anims: [Vec<i32>; 14],
    /// `m_GoingToHorse` (`+0x12C`): approach to the mount in progress.
    pub going_to_horse: bool,
    /// `m_isBusy` (`+0x12D`).
    pub busy: bool,
    /// `m_nextMountAnim` (`+0x130`): 0 = left, 1 = right, otherwise no mounting animation.
    pub next_mount_anim: i32,
    /// `m_lanceAvailable` (`+0x134`).
    pub lance_available: bool,
}

impl Riding {
    /// Constructor `0x00521680` (boat `0x00523360`).
    pub fn new(boat: bool) -> Self {
        Self {
            boat,
            mount_target: ObjectId::NONE,
            anims: Default::default(),
            going_to_horse: false,
            busy: false,
            next_mount_anim: -1,
            lance_available: true,
        }
    }

    /// Status bit "sits on" (`+0x74`): `ISMOUNTED` or `ISMOUNTEDONBOAT`.
    pub fn status_bit(&self) -> u32 {
        if self.boat {
            status::ISMOUNTEDONBOAT
        } else {
            status::ISMOUNTED
        }
    }

    /// Failure message of the approach (`+0x78`).
    pub fn fault_msg(&self) -> u32 {
        if self.boat { 0x30E } else { 0x30D }
    }

    /// Success message of the approach (`+0x7C`).
    pub fn success_msg(&self) -> u32 {
        if self.boat { 0x40E } else { 0x40D }
    }

    /// `PropertyRiding_GetAnim` (`0x00522060`, re/riding 2.3): `args = {kind, sel}`; missing `sel`
    /// -> 0. An unknown kind returns 0 (not -1), an empty list -1 (deviation from K-228:
    /// the original divides by 0, exception A7).
    pub fn get_anim(&self, args: &[i32]) -> i32 {
        let kind = args.first().copied().unwrap_or(0);
        let sel = args.get(1).copied().unwrap_or(0);
        let place = match kind {
            1 => list::FIGHT_FAR,
            2 => list::PRESENT,
            3 => list::RELOAD,
            4 => list::BANISH,
            5 => list::WALK,
            6 => list::RUN,
            7 => list::STAND,
            8 => list::DIE,
            9 => list::FIGHT_NEAR,
            10 => list::HORSE_FIGHT,
            0x222EC => list::MOUNT_LEFT,
            0x222ED => list::MOUNT_RIGHT,
            0x222EE => list::UNMOUNT,
            0x222EF => list::STAND_UP,
            _ => return 0,
        };
        list_modulo(&self.anims[place], sel)
    }
}

pub fn riding<'s>(sim: &'s Sim, id: ObjectId, index: usize) -> Option<&'s Riding> {
    match &sim.module(id, index)?.kind {
        ModuleKind::Riding(r) => Some(r),
        _ => None,
    }
}

pub fn riding_mut<'s>(sim: &'s mut Sim, id: ObjectId, index: usize) -> Option<&'s mut Riding> {
    match &mut sim.module_mut(id, index)?.kind {
        ModuleKind::Riding(r) => Some(r),
        _ => None,
    }
}

/// Init (slot 9): Riding via `ReadParams` (`Module_InitViaReadParams` `0x00521920`), RidingBoat
/// via `PropertyRidingBoat_Init` (`0x005235B0`, re/riding 1.1).
pub fn init(sim: &mut Sim, id: ObjectId, index: usize, desc: &ModuleDesc) {
    if index == RIDING_BOAT {
        init_boat(sim, id, index, desc);
    } else {
        read_params(sim, id, index, desc);
    }
}

/// `PropertyRidingBoat_Init`: 150002 -> walking, 150003 -> standing, 150004 -> dying, 150005 ->
/// melee, 150006 -> ranged combat, 150007 -> presenting, 150008 -> reloading, 150009 -> banishing; running
/// and the remaining lists stay empty, the flags 0 (150001 is not read).
fn init_boat(sim: &mut Sim, id: ObjectId, index: usize, desc: &ModuleDesc) {
    let Some(r) = riding_mut(sim, id, index) else {
        return;
    };
    for (value, place) in BOAT_LISTS {
        let text = desc.values.get(&value).map_or("", String::as_str);
        r.anims[place] = parse_int_list(text, &r.anims[place]);
    }
}

/// `PropertyRiding_ReadParams` (`0x00522BE0`, re/riding 1.1, 13.6): flags 140001, the 14 lists
/// 140002...140015 (empty text leaves a list as it is), then the lance rule. Runs as init of
/// module 13 and as variant change (slot 4) of **both** modules (K-71): a boat descriptor does not
/// have the values, the flags become 0 and the lists stay.
pub fn read_params(sim: &mut Sim, id: ObjectId, index: usize, desc: &ModuleDesc) {
    let Some(Module { base, kind }) = sim.module_mut(id, index) else {
        return;
    };
    let ModuleKind::Riding(r) = kind else {
        return;
    };
    base.flags = desc.value_int(140_001).unwrap_or(0) as u32;
    for (place, list) in r.anims.iter_mut().enumerate() {
        let text = desc
            .values
            .get(&(LIST_VALUE_BASE + place as u32))
            .map_or("", String::as_str);
        *list = parse_int_list(text, list);
    }
    if r.lance_available && base.has_flags(HAS_LANCE) && !base.has_flags(LANCE_AVAILABLE) {
        base.flags |= LANCE_AVAILABLE;
    }
}

/// `PropertyRiding_Query` (`0x00522AC0`) and `PropertyRidingBoat_Query` (`0x005233A0`,
/// re/riding 2.1, 2.2): abort `0x30D`, arrival `0x40D`, GETANIM, mounting animation finished
/// `0x70D`, dismounting animation finished `0x80D {x, z}`, search horse `0xA0D` (return 1); otherwise via
/// `OnMessage` with return 0. The codes with low byte `0x0D` reach only module 13; the
/// boat module gets `0x30E`/`0x40E` via `OnMessage` and itself answers only the sinking
/// of the canoe (`0x70E`/`0x80E`, re/riding 3.3).
pub fn query(sim: &mut Sim, id: ObjectId, index: usize, msg: &mut Message) -> i32 {
    if index == RIDING {
        match msg.code {
            0x30D => return i32::from(abort(sim, id, index)),
            0x40D => {
                mount::mount_dest_reached(sim, id, index, ObjectId::from_i32(msg.arg(0)));
                return 1;
            }
            0x70D => {
                mount::finish_mount(sim, id, index);
                return 1;
            }
            0x80D => {
                dismount::finish_dismount(sim, id, index, false, msg.arg(0), msg.arg(1));
                return 1;
            }
            0xA0D => {
                mount::search_mount(sim, id);
                return 1;
            }
            _ => {}
        }
    } else {
        match msg.code {
            0x70E => return dismount::boat_sunk(sim, id, index),
            0x80E => return dismount::boat_sunk_done(sim, id, index),
            _ => {}
        }
    }
    let anim_code = if index == RIDING_BOAT { 0x50E } else { 0x50D };
    if msg.code == anim_code {
        return riding(sim, id, index).map_or(-1, |r| r.get_anim(&msg.args));
    }
    on_message(sim, id, index, &Envelope::bare(msg.clone()));
    0
}

/// `OnMessage` (`0x005230C0`/`0x005234C0`): mounting `0x10D/0x10E` and dismounting `0x20D/0x20E`
/// remember the success and failure message of the envelope; mounting starts the approach
/// ([`mount::mount_start`]), dismounting returns the success of [`dismount::dismount_start`].
/// The boat module gets failure `0x30E` (abort) and success `0x40E` (arrival) here, because its
/// query has no branch of its own for them. The lance codes (`0xC0D` ...) fall into the
/// "otherwise" branch (deviation from the original).
pub fn on_message(sim: &mut Sim, id: ObjectId, index: usize, env: &Envelope) -> bool {
    let (mount, dismount) = if index == RIDING_BOAT {
        (0x10E, 0x20E)
    } else {
        (0x10D, 0x20D)
    };
    let code = env.msg.code;
    if (code == mount || code == dismount)
        && let Some(m) = sim.module_mut(id, index)
    {
        m.base.set_success(env.success.as_ref(), env.success_target);
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
    }
    let target = ObjectId::from_i32(env.msg.arg(0));
    match (index, code) {
        (_, c) if c == mount => mount::mount_start(sim, id, index, target),
        (_, c) if c == dismount => return dismount::dismount_start(sim, id, index),
        (RIDING_BOAT, 0x30E) => {
            abort(sim, id, index);
        }
        (RIDING_BOAT, 0x40E) => mount::mount_dest_reached(sim, id, index, target),
        _ => {}
    }
    true
}

/// GUID "Execute throw off" (counter-upgrade, re/riding 3.4).
const THROW_OFF: u32 = 3377;

/// Tick `0x00521930` (both modules; RidingBoat jumps here, `0x00523580`): `UpdateEvents`,
/// then a mounted object (`0x2000`, never `0x8000`) posts `{0x20D}` with abort as long as its player
/// counts 3377 (K-229) - so a type with both modules does it twice per tick.
pub fn tick(sim: &mut Sim, id: ObjectId, index: usize, dt: i32) {
    update_events(sim, id, index, dt);
    let mounted = sim
        .object(id)
        .is_some_and(|o| o.status & status::ISMOUNTED != 0);
    if mounted && crate::economy::guid_count(sim, i32::from(id.slot()), THROW_OFF) != 0 {
        post_message(sim, id, Message::new(0x20D, &[]), true);
    }
}

/// `Abort` (`0x00521F50`, re/riding 3.6): if the approach is running, exemption zone off and exemption object 0
/// (K-225); then the failure message. An existing link stays.
pub fn abort(sim: &mut Sim, id: ObjectId, index: usize) -> bool {
    if let Some(r) = riding_mut(sim, id, index)
        && r.going_to_horse
    {
        r.going_to_horse = false;
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(0x1000, false);
            o.exempt.object = ObjectId(0);
        }
    }
    send_fault(sim, id, index);
    true
}

/// Load check: no rider chain runs in a circle. Dying, walk and all
/// queries except `0x2000`/`0x2100` pass from an object with `ISMOUNTED` or
/// `ISMOUNTEDONBOAT` on to its target without limit as long as it exists, also to dead members
/// (`0xC00`, `walk::mount_of`); a ring would run without end or until the stack overflows. The chain
/// therefore follows the `target` of every object with a rider bit as long as the target exists, whether
/// alive or dead. Each object is visited at most once (like
/// [`crate::world::carry_chains_are_acyclic`]).
pub(crate) fn rider_chains_are_acyclic(state: &SimState) -> bool {
    let next = |id: ObjectId| {
        let o = state.object(id, false)?;
        let t = o.target;
        (o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0 && !t.is_none() && t.0 != 0)
            .then_some(t)
            .filter(|&t| state.object(t, false).is_some())
    };
    let mut known = std::collections::BTreeSet::new();
    for start in state.objects().map(|o| o.id) {
        let mut path = std::collections::BTreeSet::new();
        let mut cur = Some(start);
        while let Some(id) = cur {
            if known.contains(&id.0) {
                break;
            }
            if !path.insert(id.0) {
                return false;
            }
            cur = next(id);
        }
        known.extend(path);
    }
    true
}
