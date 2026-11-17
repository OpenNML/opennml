//! Module 16 `CPropertyDying` (re/combat-death): dying (`0x110`) with a carcass on slot 8,
//! death callbacks, robbing gold, decay (`0x210`) and sinking until removal; release the rider-mount link of
//! rider/horse/canoe (2.8), sink canoes with the paddler (2.9) and release the mount on
//! dismounting (`0x810`).

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::fixed_vec::FixedVec;
use crate::fx::{Fx, FxVec3, fixed_mul};
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::{Envelope, Message};
use crate::messaging::{abort_all_modules, send};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{ObjectDesc, create_object, desc_flags, queue_destroy, set_grid_position};

use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 0x10;
const GROUP: u8 = 0x80;
/// `CANROBBED` (`0x200`, `MID_PROP_DIE_CANROBBED 528` = module `0x10`, bit `0x200`): corpse
/// robbable; filter bit of the corpse filter of robbing (re/headhunter-robbing 3.8).
pub const CANROBBED: u32 = 0x200;
const GHOST: u32 = 0x100;
/// `DIES_WITH_PEST_CLOUD` (`0x800`, with low byte `0x810`).
const PEST_CLOUD: u32 = 0x800;
const ISINFECTED: u32 = 0x1000;
/// Status bit "exemption zone active" (`0x1000`).
const EXEMPT: u32 = 0x1000;
/// Canoes (`PropertyDying_SinkCanoe` `0x0050EBA0`): no owner change.
const CANOES: [u32; 2] = [1408, 1712];
/// Status bits of the siege crew (module `0x2E`; meaning open, re/combat-death 2.8).
const SIEGE_CREW_A: u32 = 0x100_0000;
const SIEGE_CREW_B: u32 = 0x400_0000;
/// Offset of the canoe occupant in the dead branch of `sink_canoe` (`DAT_0067274C`, 1/2 tile).
const HALF_TILE: i32 = 2048;

/// Bucket count of the callback map.
pub const LISTENER_BUCKETS: usize = 17;

/// Callback map (`CMap` HID -> message, 17 buckets, hash `(hid as u32) >> 4`, insertion at the bucket head,
/// traversal buckets ascending; re/combat-death 2.3). The type fixes the bucket count; an entry in the wrong
/// bucket (only from a corrupted savegame) is merely not replaced or removed, `iter` calls it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Listeners(pub FixedVec<Vec<(ObjectId, Message)>, LISTENER_BUCKETS>);

impl Listeners {
    fn bucket(h: ObjectId) -> usize {
        ((h.0 >> 4) % LISTENER_BUCKETS as u32) as usize
    }

    /// `SetAt`: the same key replaces in place, otherwise at the bucket head.
    pub fn insert(&mut self, h: ObjectId, msg: Message) {
        let b = &mut self.0[Self::bucket(h)];
        if let Some(e) = b.iter_mut().find(|e| e.0 == h) {
            e.1 = msg;
        } else {
            b.insert(0, (h, msg));
        }
    }

    /// `RemoveKey`.
    pub fn remove(&mut self, h: ObjectId) {
        self.0[Self::bucket(h)].retain(|e| e.0 != h);
    }

    /// MFC order: buckets ascending, within each bucket head -> end.
    pub fn iter(&self) -> impl Iterator<Item = &(ObjectId, Message)> {
        self.0.iter().flatten()
    }
}

/// `CPropertyDying` (`0xAC` bytes), savegame order `0x0050FAF0`; uninitialized fields
/// of the original are 0 (C-152).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dying {
    /// `m_DieAnim` (170002).
    pub die_anim: i32,
    /// `m_DieCounter`: remaining time of the sinking (ms).
    pub die_counter: i32,
    /// `m_HideSpeed`: sinking per tick (world units); the rendering derives the depth.
    pub hide_speed: i32,
    /// `m_Listeners`: death callbacks.
    pub listeners: Listeners,
    /// `m_RobEffectID`: stored (savegame/fidelity to the original), never used in the original (C-151).
    pub rob_effect_id: i32,
    /// `m_RobGold`: gold when robbing (25 or 0).
    pub rob_gold: i32,
    /// 0 calm, 1 died (callbacks pending), 2 sinking.
    pub status: u8,
    /// `m_TimeLabel`: time marker 1 of the chosen dying animation (ms).
    pub time_label: i32,
    /// `m_PestParticleID`: stored (savegame/fidelity to the original); the plague is dropped (shipped data,
    /// C-156), guard test `no_type_carries_the_plague`.
    pub pest_effect: i32,
}

impl Default for Dying {
    /// `PropertyDying_Construct` (`0x0050E6D0`): `+0x98 = 0`, `+0xA8 = -1`, the rest 0 (C-152).
    fn default() -> Self {
        Self {
            die_anim: 0,
            die_counter: 0,
            hide_speed: 0,
            listeners: Listeners::default(),
            rob_effect_id: 0,
            rob_gold: 0,
            status: 0,
            time_label: 0,
            pest_effect: -1,
        }
    }
}

fn dying_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Dying> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Dying(d) => Some(d),
        _ => None,
    }
}

fn dying<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Dying> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Dying(d) => Some(d),
        _ => None,
    }
}

fn has_flags(sim: &Sim, id: ObjectId, x: u32) -> bool {
    sim.module(id, INDEX).is_some_and(|m| m.base.has_flags(x))
}

fn has_module(sim: &Sim, id: ObjectId, index: usize) -> bool {
    sim.object(id).is_some_and(|o| o.has_module(index, false))
}

/// `GameObject_GetTargetObject` (`0x00466910`): `+0x2CC` not in {-1, 0} and present.
pub(crate) fn target_object(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let t = sim.object(id)?.target;
    (!t.is_none() && t.0 != 0 && sim.object(t).is_some()).then_some(t)
}

fn post(sim: &mut Sim, id: ObjectId, delay: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(delay, msg, false, GROUP);
    }
}

/// `Module_InitViaReadParams` (`0x00521920`): create the state, then [`read_params`].
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.kind = ModuleKind::Dying(Box::default());
    }
    read_params(sim, id, desc);
}

/// `PropertyDying_ReadParams` (`0x0050E7C0`): dying animation and flags; plague carriers without
/// infection are infected ([`set_infected`]).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let anim = desc.value_int(170002).unwrap_or(0) as i32;
    let flags = desc.value_int(170001).unwrap_or(0) as u32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    if let Some(d) = dying_mut(sim, id) {
        d.die_anim = anim;
    }
    if has_flags(sim, id, 0x810) && !has_flags(sim, id, 0x1010) {
        set_infected(sim, id, true);
    }
}

/// `PropertyDying_SetInfected` (`0x0050E5B0`): only the flag `ISINFECTED`; the plague particles
/// (`m_PestParticleID`) are dropped (shipped data, C-156, re/ghosts-banish 3.7), guard test
/// `no_type_carries_the_plague`. Nothing with `NOTHUMAN`.
fn set_infected(sim: &mut Sim, id: ObjectId, on: bool) {
    if crate::filter::matches_class(sim, id, 0x2_0000) {
        return;
    }
    let infected = has_flags(sim, id, ISINFECTED);
    if on && !infected {
        // Effect `0x004F30B0(world, go, 10000)` -> `m_PestParticleID`: dropped (shipped data), guard test
        // `no_type_carries_the_plague`.
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.flags |= ISINFECTED;
        }
    } else if !on && infected && !has_flags(sim, id, PEST_CLOUD) {
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.flags &= !ISINFECTED;
        }
        if let Some(d) = dying_mut(sim, id) {
            d.pest_effect = -1;
        }
    }
}

/// `PropertyDying_StartDying` (`0x0050EE30`, `0x110`) in the order of the original
/// (re/combat-death 2); return value 0 (in a boat the answer of the boat).
fn start_dying(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    if o.status & status::DIED != 0 {
        return 0;
    }
    // Event bit 2 first (`0x0050EE73`, E-148): on the current, old ID - the owner change to slot 8
    // is queued and does not transfer the event word; the boat occupant that passes dying on
    // gets it, too.
    crate::world::set_object_event_bit(sim, id, 2, true);
    let Some(o) = sim.object(id) else {
        return 0;
    };
    let (guid, owner, attacker) = (o.guid, o.owner(), o.last_attacker);
    // Statistic "unit lost" at the owner (`0x0050EE8C`), then "killed" at the slot of the last
    // attacker (`0x0050EEB8`, bits 11...15 of the ID, without an enemy check - O-168).
    crate::accounting::add_statistic(
        sim,
        i32::from(owner),
        crate::accounting::kind::UNIT_LOST,
        guid as i32,
    );
    if !attacker.is_none() {
        crate::accounting::add_statistic(
            sim,
            i32::from(attacker.slot()),
            crate::accounting::kind::UNIT_KILLED,
            guid as i32,
        );
    }
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    o.set_status_flags(status::MASKED, false);
    let (tile, pos) = (o.tile, o.world_pos);
    crate::fog::set_vision_source(
        &mut sim.state.fog,
        crate::fog::VisionSource {
            x: tile.0,
            z_or_hid: tile.1,
            size: 8,
            player: i32::from(owner),
            remaining_ms: 10_000,
            flags: crate::fog::EXPLORE | crate::fog::SEE,
        },
    );
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: 0x65,
        flags: 0x43,
        pos,
        guid,
    });
    let Some(o) = sim.object_mut(id) else {
        return 0;
    };
    o.selectable = false;
    let st = o.status;
    // C-148: the occupant of a boat passes dying on to the boat (return value of the boat);
    // the dying boat sets it down via `unlink` and makes it selectable again.
    if st & status::ISMOUNTEDONBOAT != 0
        && let Some(boat) = target_object(sim, id)
    {
        return send(sim, boat, msg);
    }
    let mut anim = dying(sim, id).map_or(0, |d| d.die_anim);
    if st & status::ISMOUNTED != 0 {
        anim = send(sim, id, &Message::new(0x50D, &[8]));
    } else if st & status::ISMOUNTEDONBOAT != 0 {
        anim = send(sim, id, &Message::new(0x50E, &[8]));
    } else if st & status::SWIMMING != 0 {
        anim = send(sim, id, &Message::new(0x30F, &[]));
    } else if st & status::HEAL != 0 {
        // Healing trance (re/healing 4): `0x61D` returns `m_DieAnim` and clears `m_ReadyToHeal`
        // (`healing::query`); `HEAL` and the message filter stay on the carcass, `0x31D`/`0x41D`
        // never fire (the healing tick ends at `DIED`).
        anim = send(sim, id, &Message::new(0x61D, &[]));
    }
    // Sapping (module 0x34: `0x534`, dust cloud) has no type in the shipped data. Wall (`ONWALL`: `0xE2F`,
    // `0x102F` after 3/4 of the time marker) is dead in the original (re/special-movement 4).
    abort_all_modules(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, false, false, false);
    }
    // "Slot running" is `GameWorld+0x62C[owner slot]` (`FUN_00478080`, false outside 0...10)
    // - not `Player::slot_active` (`Player+0x218`).
    let slot_active = sim
        .state
        .slot_running
        .get(usize::from(id.slot()))
        .copied()
        .unwrap_or(false);
    // Animals with an animal module and `NOTHUMAN` (walk) are not robbable (C-149).
    let rob_gold = if !slot_active
        || has_module(sim, id, 0x14)
        || crate::filter::matches_class(sim, id, 0x2_0000)
    {
        0
    } else {
        25
    };
    if let Some(d) = dying_mut(sim, id) {
        d.rob_gold = rob_gold;
    }
    // C-147: hunted animals (module 0x15) keep their hard radius.
    if (!has_module(sim, id, 0x15) || crate::filter::matches_class(sim, id, 0x214))
        && let Some(o) = sim.object_mut(id)
    {
        o.set_status_flags(EXEMPT, true);
        let (x, z, r) = (o.grid.x.0, o.grid.z.0, o.soft_radius);
        o.set_collision_exempt_circle(x, z, r);
    }
    // Own effects go to the world (`0x004692D0`): rendering.
    // Ships and canoes (walk `WATER|NOTHUMAN`) and water animals: shadow off immediately.
    let ships = MatchFilter {
        mode: 0x54,
        match_bits: 0x2_2000,
        prop_id: 0,
        ..MatchFilter::constructed()
    };
    let no_shadow = crate::filter::matches(sim, id, &ships, -1)
        || (crate::filter::matches_class(sim, id, 0x2000) && has_module(sim, id, 0x14));
    if let Some(o) = sim.object_mut(id) {
        if no_shadow {
            o.render_bits &= !2;
        }
        o.set_status_flags(status::DIED, true);
    }
    // Let the counter-upgrades of this source run out (`0x00412650`, re/temp-upgrades 3.6): remainder 0,
    // expiry in the next phase 0 step (W-218).
    crate::upgrades::counter::expire_by_source(sim.state, id);
    if has_module(sim, id, 0x1B) {
        // Ghost: `0x21B` (fade-out over 34 ticks, `crate::module::ghosts::ghost`),
        // kill counter (`0x0050F2FE`, E-189), all modules except 0x10 and 0x1B off, callbacks immediately;
        // no owner change, status stays 0.
        send(sim, id, &Message::new(0x21B, &[]));
        count_kill(sim, id);
        if let Some(o) = sim.object_mut(id) {
            for i in 0..crate::object::MODULE_SLOTS {
                if i != INDEX && i != 0x1B {
                    o.queue_module_tick_change(i, false);
                }
            }
        }
        notify_listeners(sim, id);
        return 0;
    }
    // C-146: The branch "animal with `IS_EVOKED` -> `0x114`" comes after `DIED` and checks without
    // `RETURN_DIED` - always false, not reproduced. Evoked animals (`animal::query` `0x314`,
    // re/evoke-aura 3.2a) therefore die like every object; their callback `{0x114}` hangs on the old
    // ID and is lost after the owner change (C-278).
    if has_module(sim, id, 0x2E) {
        send(sim, id, &Message::new(0x172E, &[]));
    }
    if has_module(sim, id, 0x0B) {
        send(sim, id, &Message::new(0x30B, &[]));
    }
    // `DIES_WITH_PEST_CLOUD`: infection off and plague cloud - dropped (shipped data), guard test
    // `no_type_carries_the_plague`.
    send(sim, id, &Message::new(0xC00, &[]));
    send(sim, id, &Message::new(0x411, &[]));
    send(sim, id, &Message::new(0x2117, &[]));
    // Break the rider-mount link, then sink the canoe (re/riding 13.3: the canoe then has no target any more,
    // K-264).
    if let Some(t) = target_object(sim, id) {
        unlink(sim, id, t);
    }
    // Sink the canoe, kill counter per GUID (`0x0050F606`, E-189), then ownership to slot 8 (C-142) -
    // the owner change does not apply to the canoe.
    let canoe = CANOES.contains(&guid);
    if canoe {
        sink_canoe(sim, id);
    }
    count_kill(sim, id);
    if !canoe {
        crate::world::queue_owner_change(sim, id, 8);
    }
    let label = sim.rules.anim_label_time_at(guid, anim, 1);
    if let Some(d) = dying_mut(sim, id) {
        d.status = 1;
        d.time_label = label;
    }
    let Some(o) = sim.object(id) else {
        return 0;
    };
    let (st, carrier) = (o.status, o.carrier);
    if st & status::QUARTERED != 0 && !carrier.is_none() && sim.object(carrier).is_some() {
        send(sim, carrier, &Message::new(0x1717, &[id.as_i32()]));
    }
    // C-150: effective at the end of T+1 (pending list).
    if let Some(o) = sim.object_mut(id) {
        for i in 0..crate::object::MODULE_SLOTS {
            if ![INDEX, 0x15, 0, 0x2F].contains(&i) {
                o.queue_module_tick_change(i, false);
            }
        }
    }
    0
}

/// Breaks the rider-mount link in `StartDying` (re/combat-death 2.8, re/riding 4.4, 13.3); `t` is the
/// existing target (`+0x2CC`).
/// - Siege bits `0x1000000 | 0x4000000` (module `0x2E`): `0x22E {id}` to the target,
///   own reference -1.
/// - I am the rider (the target carries, `CARRIING`): release the mount via [`release_mount`], own
///   rider bits off.
/// - Otherwise I carry the target (horse/canoe dies): `CARRIING` off; a canoe occupant (`0x8000`)
///   gets `0x70E {1}` (dying animation, RidingBoat) and becomes selectable again; the rider bits
///   of the target go off, the own reference becomes -1. The target keeps its reference (carcass or
///   destroyed canoe, C-153) and its mounty bonus (re/riding 8.6).
fn unlink(sim: &mut Sim, id: ObjectId, t: ObjectId) {
    let Some(st) = sim.object(id).map(|o| o.status) else {
        return;
    };
    let t_st = sim.object(t).map_or(0, |o| o.status);
    if st & (SIEGE_CREW_A | SIEGE_CREW_B) != 0 {
        send(sim, t, &Message::new(0x22E, &[id.as_i32()]));
        if let Some(o) = sim.object_mut(id) {
            o.target = ObjectId::NONE;
        }
    } else if t_st & status::CARRIING != 0 {
        release_mount(sim, id);
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(status::ISMOUNTED, false);
            o.set_status_flags(status::ISMOUNTEDONBOAT, false);
        }
    } else {
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(status::CARRIING, false);
        }
        if t_st & status::ISMOUNTEDONBOAT != 0 {
            send(sim, t, &Message::new(0x70E, &[1]));
            if let Some(o) = sim.object_mut(t) {
                o.selectable = true;
            }
        }
        if let Some(o) = sim.object_mut(t) {
            o.set_status_flags(status::ISMOUNTED, false);
            o.set_status_flags(status::ISMOUNTEDONBOAT, false);
        }
        if let Some(o) = sim.object_mut(id) {
            o.target = ObjectId::NONE;
        }
    }
}

/// `PropertyDying_SinkCanoe` (`0x0050EBA0`, re/combat-death 2.9, re/riding 13.3): a sinking
/// canoe (1408/1712).
///
/// If the canoe still has a target, it gets `0x302` (stand: `ONWATER`), becomes selectable and is
/// shifted by half a tile (`DAT_0067274C` = 2048) against the canoe direction (`keepY`). From `StartDying`
/// this is dead code because [`unlink`] deletes the reference beforehand (K-264); the direction bonus
/// `+0x168` at status `0x5000000` is dropped (re/riding 13.5).
///
/// Then always a paddler (K-265): GUID 1400 (for 1408) or 1700, flags `0x893`, owner of the
/// canoe, color of the owner slot (`GameWorld_GetPlayerColor` `0x004789E0`, call
/// `0x0050ED43` - not the color of the canoe), world position and yaw of the canoe; on success `0x302`,
/// grid position of the canoe (`keepY`) and `0x502 {dying animation of the canoe}` (one-shot animation).
/// Finally the canoe is destroyed (`QueueObjectDestroy`), without an owner change.
pub(crate) fn sink_canoe(sim: &mut Sim, id: ObjectId) {
    let Some((guid, grid, dir, world_pos, yaw)) = sim
        .object(id)
        .map(|o| (o.guid, o.grid, o.direction.0, o.world_pos, o.rotation[1]))
    else {
        return;
    };
    if let Some(t) = target_object(sim, id) {
        send(sim, t, &Message::new(0x302, &[]));
        if let Some(o) = sim.object_mut(t) {
            o.selectable = true;
        }
        if let Some(tg) = sim.object(t).map(|o| o.grid) {
            let x = tg.x.0.wrapping_sub(fixed_mul(HALF_TILE, cos_deg(dir)));
            let z = tg.z.0.wrapping_sub(fixed_mul(HALF_TILE, sin_deg(dir)));
            set_grid_position(sim, t, FxVec3::new(Fx(x), Fx(0), Fx(z)), true);
        }
    }
    let slot = i32::from(id.slot());
    let mut d = ObjectDesc::new(if guid == CANOES[0] { 1400 } else { 1700 }, slot, world_pos);
    // `0x893`.
    d.flags = desc_flags::READY
        | desc_flags::COLOR
        | desc_flags::ROT_Y
        | desc_flags::POSITION
        | desc_flags::GUID;
    d.color = crate::setup::player_color(sim, slot);
    d.rotation = [0.0, yaw, 0.0];
    if let Some(p) = create_object(sim, &mut d) {
        send(sim, p, &Message::new(0x302, &[]));
        set_grid_position(sim, p, grid, true);
        let anim = dying(sim, id).map_or(0, |d| d.die_anim);
        send(sim, p, &Message::new(0x502, &[anim]));
    }
    queue_destroy(sim, id);
}

/// `FUN_00481680(slot, guid, 1)` from `StartDying`: slot of the last attacker (`+0x1B8`, without an
/// attacker 31 -> discarded), GUID of the victim (`+0x54`) at the time of counting.
fn count_kill(sim: &mut Sim, id: ObjectId) {
    let Some((attacker, guid)) = sim.object(id).map(|o| (o.last_attacker, o.guid)) else {
        return;
    };
    crate::world::add_kill_count(sim, crate::world::kill_slot(attacker), guid);
}

/// `PropertyDying_NotifyListeners` (`0x0050E820`): per entry in MFC order, if the target
/// exists (also dying): argument 0 = current ID, send synchronously; then empty.
fn notify_listeners(sim: &mut Sim, id: ObjectId) {
    let entries: Vec<(ObjectId, Message)> =
        dying(sim, id).map_or_else(Vec::new, |d| d.listeners.iter().cloned().collect());
    for (h, mut msg) in entries {
        if sim.object(h).is_some() {
            if let Some(a) = msg.args.first_mut() {
                *a = id.as_i32();
            }
            send(sim, h, &msg);
        }
    }
    if let Some(d) = dying_mut(sim, id) {
        d.listeners = Listeners::default();
    }
}

/// `PropertyDying_ScheduleCorpseTimers` (`0x0050E320`): hunted animals with module 21 **switched on**
/// get `0x215`; the check for `IS_EVOKED` is always false after `DIED` (C-146), module 21
/// turns it into the carcass (`module::edible`). For evoked animals `0x314` switched module 21
/// off (re/evoke-aura 3.2a, 4): they become ordinary corpses (C-146, C-278). Otherwise
/// decay after 30 s and, with robbing gold, "robbable" at the time marker.
fn schedule_corpse_timers(sim: &mut Sim, id: ObjectId) {
    let (gold, label) = dying(sim, id).map_or((0, 0), |d| (d.rob_gold, d.time_label));
    if has_module(sim, id, 0x15) && !crate::filter::matches_class(sim, id, 0x214) {
        post(sim, id, label.max(1500), Message::new(0x215, &[]));
        return;
    }
    post(sim, id, 30_000, Message::new(0x210, &[]));
    if gold != 0 {
        post(sim, id, label, Message::new(0x710, &[]));
    }
}

/// `PropertyDying_Tick` (`0x0050E950`): timers; status 1 -> callbacks and carcass timers
/// (swimmers sink immediately); status 2 -> sink counter -(dt+1) per tick (C-144), then remove.
/// The host derives the sinking of the rendering from `die_counter`/`hide_speed`
/// (re/combat-death 5).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    match dying(sim, id).map_or(0, |d| d.status) {
        1 => {
            notify_listeners(sim, id);
            let swimming = sim
                .object(id)
                .is_some_and(|o| o.status & status::SWIMMING != 0);
            if swimming {
                if let Some(d) = dying_mut(sim, id) {
                    d.status = 2;
                    d.die_counter = 7500;
                    d.hide_speed = 3;
                }
            } else {
                schedule_corpse_timers(sim, id);
                if let Some(d) = dying_mut(sim, id) {
                    d.status = 0;
                }
            }
        }
        2 => {
            let Some(d) = dying_mut(sim, id) else {
                return;
            };
            d.die_counter = d.die_counter.wrapping_add((-1i32).wrapping_sub(dt));
            if d.die_counter <= 0 {
                d.status = 0;
                queue_destroy(sim, id);
            }
        }
        _ => {}
    }
}

/// `PropertyDying_ClearRobbable` (`0x0050E410`): only with `CANROBBED` gold 0 and flag off.
fn clear_robbable(sim: &mut Sim, id: ObjectId) {
    if !has_flags(sim, id, CANROBBED) {
        return;
    }
    if let Some(d) = dying_mut(sim, id) {
        d.rob_gold = 0;
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags &= !CANROBBED;
    }
}

/// `PropertyDying_HandleMessage` (`0x0050F8A0`, query); unknown codes -1.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        // Return value of `StartDying` (re/riding 13.4, `0x0050F907...0x0050F910`): 0, for the
        // canoe occupant the answer of the canoe.
        0x110 => {
            let m = msg.clone();
            start_dying(sim, id, &m)
        }
        // Decay: sink, shadow off, no longer robbable.
        0x210 => {
            if let Some(d) = dying_mut(sim, id) {
                d.status = 2;
                d.die_counter = 3000;
                d.hide_speed = 1;
            }
            if let Some(o) = sim.object_mut(id) {
                o.render_bits &= !2;
            }
            clear_robbable(sim, id);
            0
        }
        // Unregister the death callback `{hid}`.
        0x410 => {
            let h = ObjectId::from_i32(msg.arg(0));
            if let Some(d) = dying_mut(sim, id) {
                d.listeners.remove(h);
            }
            0
        }
        0x510 => {
            if let Some(m) = sim.module_mut(id, INDEX) {
                if msg.arg(0) == 0 {
                    m.base.flags &= GHOST; // C-145
                } else {
                    m.base.flags |= GHOST;
                }
            }
            1
        }
        // Collect robbing gold.
        0x610 => {
            msg.set_arg(0, dying(sim, id).map_or(0, |d| d.rob_gold));
            clear_robbable(sim, id);
            0
        }
        // Robbable from now on.
        0x710 => {
            if dying(sim, id).is_some_and(|d| d.rob_gold != 0)
                && let Some(m) = sim.module_mut(id, INDEX)
            {
                m.base.flags |= CANROBBED;
            }
            0
        }
        // Release the mount (`0x0050E4B0`, re/combat-death 2.8; sender `FinishDismount`).
        0x810 => release_mount(sim, id),
        // Infection on/off (only the flag; the effect is dropped (shipped data), guard test
        // `no_type_carries_the_plague`).
        0x910 => {
            set_infected(sim, id, msg.arg(0) == 1);
            0
        }
        _ => -1,
    }
}

/// `PropertyDying_ReleaseMount` (`0x0050E4B0`, `0x810`, re/combat-death 2.8, re/riding 3.2): the
/// own reference becomes -1; an existing target loses `ANIM_LOCKED`, is aborted, leaves
/// its group (`0x1500`, `GroupManager_RemoveUnitFromGroup(g, t, t, 0, 0)`), gets the
/// walk behavior `{0, 0, -1, 0, 0}` (`0x00474FF0(0, -1, 0, 0)`) and loses `CARRIING` and its
/// reference. Return value 1, without a target 0. The rider bits stay (the caller clears them).
pub(crate) fn release_mount(sim: &mut Sim, id: ObjectId) -> i32 {
    let t = target_object(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.target = ObjectId::NONE;
    }
    let Some(t) = t else {
        return 0;
    };
    if let Some(m) = sim.object_mut(t) {
        m.set_status_flags(status::ANIM_LOCKED, false);
    }
    abort_all_modules(sim, t);
    let g = send(sim, t, &Message::new(0x1500, &[]));
    if g != -1 {
        crate::group::remove_unit(sim, g, t, t, false);
    }
    send(sim, t, &Message::new(0x2100, &[0, 0, -1, 0, 0]));
    if let Some(m) = sim.object_mut(t) {
        m.set_status_flags(status::CARRIING, false);
        m.target = ObjectId::NONE;
    }
    1
}

/// `PropertyDying_OnMessage` (`0x0050EB80`): only `0x310` (`0x0050EA40`): if the sender
/// `success_target` exists (also dying), the success message goes into the map under its ID
/// (the same key replaces). Every `0x310` reports `true` (`MOV AL, 1` on both paths),
/// other envelopes `false`.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if env.msg.code != 0x310 {
        return false;
    }
    let h = env.success_target;
    if sim.object(h).is_some()
        && let Some(success) = &env.success
        && let Some(d) = dying_mut(sim, id)
    {
        d.listeners.insert(h, success.clone());
    }
    true
}

/// Owner change (slot 5, `0x0050E460`): a carcass keeps the color of the old owner
/// (C-142).
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId, old: ObjectId) {
    let color = sim
        .state
        .players
        .get(usize::from(old.slot()))
        .map(|p| p.color);
    if let Some(o) = sim.object_mut(id)
        && o.status & status::DIED != 0
        && let Some(c) = color
    {
        o.color = c;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::state::SimEvent;

    /// HP to 0, run until death; returns (tick T, new ID on slot 8, events).
    fn kill(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
        id: ObjectId,
    ) -> (u32, ObjectId, Vec<SimEvent>) {
        Sim::new(state, rules)
            .object_mut(id)
            .unwrap()
            .attributes
            .set_current_fraction(14, 0);
        let mut events = Vec::new();
        let mut t = None;
        let mut new_id = None;
        for _ in 0..8 {
            let e = crate::tick::step(state, rules, &[]);
            if t.is_none()
                && state
                    .object(id, false)
                    .is_some_and(|o| o.status & crate::object::status::DIED != 0)
            {
                t = Some(state.tick);
            }
            for ev in &e {
                if let SimEvent::OwnerChanged { old, new } = ev
                    && *old == id
                {
                    new_id = Some(*new);
                }
            }
            events.extend(e);
        }
        (t.expect("died"), new_id.expect("owner changed"), events)
    }

    // QUIRK C-140, C-142, C-144, C-147, C-149: kept
    #[test]
    fn a_sabre_becomes_a_neutral_corpse_and_is_removed_after_332_ticks() {
        let rules = rules();
        let mut state = state(&rules);
        let a = spawn(&mut Sim::new(&mut state, &rules), SABRE, 0, (10, 10));
        let color = state.players[0].color;
        let (t, c, events) = kill(&mut state, &rules, a);
        let o = state.object(c, false).unwrap();
        assert_eq!(c.slot(), 8, "the carcass belongs to slot 8 (C-142)");
        assert_eq!(o.color, color, "old color (slot 5)");
        assert!(o.status & crate::object::status::DIED != 0 && !o.selectable);
        assert_eq!(o.anim.wanted, 3, "dying animation 170002");
        assert!(
            o.status & 0x1000 != 0 && o.exempt.mode == 1,
            "exemption circle"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            SimEvent::Sound3d {
                action: 0x65,
                flags: 0x43,
                ..
            }
        )));
        let src = state
            .fog
            .sources
            .iter()
            .find(|s| s.size == 8 && s.player == 0);
        assert!(src.is_some(), "sight source of the old owner, 10 s");
        let dying = |state: &crate::state::SimState| match &state
            .object(c, false)
            .unwrap()
            .module(INDEX)
            .unwrap()
            .kind
        {
            ModuleKind::Dying(d) => (**d).clone(),
            _ => panic!(),
        };
        assert_eq!(
            (dying(&state).rob_gold, dying(&state).time_label),
            (25, 1280)
        );
        // Robbable from T + 3 + floor(1280/100) = T + 15; decay T + 303; removal T + 332.
        let mut robbable = None;
        let mut rot = None;
        while state.object(c, false).is_some() {
            crate::tick::step(&mut state, &rules, &[]);
            let Some(o) = state.object(c, false) else {
                break;
            };
            if robbable.is_none() && o.module(INDEX).unwrap().base.flags & 0x200 != 0 {
                robbable = Some(state.tick);
            }
            if rot.is_none() && dying(&state).status == 2 {
                rot = Some(state.tick);
                assert_eq!(o.render_bits & 2, 0, "shadow off");
            }
            assert!(state.tick < t + 400, "not removed");
        }
        assert_eq!((robbable, rot), (Some(t + 15), Some(t + 303)));
        assert!(
            state.tick >= t + 332 && state.tick <= t + 333,
            "removed after T + 332: {}",
            state.tick - t
        );
    }

    #[test]
    fn listeners_are_called_in_mfc_bucket_order_with_the_new_id() {
        let mut l = Listeners::default();
        // Bucket = (hid >> 4) % 17; insertion at the bucket head; the same key replaces in place.
        let ids = [
            ObjectId(0x10),
            ObjectId(0x120),
            ObjectId(0x11),
            ObjectId(0x20),
        ];
        for (i, &h) in ids.iter().enumerate() {
            l.insert(h, Message::new(0xA7F, &[0, i as i32]));
        }
        let order: Vec<_> = l.iter().map(|(h, _)| h.0).collect();
        // 0x10 -> bucket 1, 0x11 -> 1, 0x120 -> 1 (0x12 % 17 = 1), 0x20 -> 2.
        assert_eq!(order, vec![0x11, 0x120, 0x10, 0x20]);
        l.insert(ObjectId(0x120), Message::new(0xA7F, &[0, 9]));
        assert_eq!(l.iter().map(|(h, _)| h.0).collect::<Vec<_>>(), order);
        l.remove(ObjectId(0x10));
        assert_eq!(l.iter().count(), 3);
    }

    #[test]
    fn listeners_get_the_corpse_id_one_tick_after_death() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, obs) = {
            let mut sim = Sim::new(&mut state, &rules);
            let a = spawn(&mut sim, SABRE, 0, (10, 10));
            let obs = spawn(&mut sim, HQ, 1, (20, 20));
            // 0x310 via OnMessage: success message under the ID of the sender.
            let env = crate::message::Envelope {
                msg: Message::new(0x310, &[]),
                success: Some(Message::new(0xA7F, &[0])),
                fault: None,
                success_target: obs,
                fault_target: ObjectId::NONE,
            };
            crate::messaging::dispatch_command(&mut sim, a, &env);
            (a, obs)
        };
        let (_, c, _) = kill(&mut state, &rules, a);
        assert_eq!(
            state.object(obs, false).unwrap().carrier,
            c,
            "argument 0 = current (new) ID"
        );
    }

    // QUIRK C-143: kept
    #[test]
    fn the_corpse_still_takes_damage_and_gives_experience() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, w) = {
            let mut sim = Sim::new(&mut state, &rules);
            (
                spawn(&mut sim, SABRE, 1, (10, 10)),
                spawn(&mut sim, WARRIOR, 0, (30, 30)),
            )
        };
        let (_, c, _) = kill(&mut state, &rules, a);
        let mut sim = Sim::new(&mut state, &rules);
        let xp0 = sim.object(w).unwrap().attributes.current_fraction(30);
        let msg = crate::combat::damage::damage_message(&mut sim, w, -9);
        crate::messaging::send(&mut sim, c, &msg);
        // Stored raw fraction (`current_fraction` clamps to >= 0).
        assert!(
            sim.object(c).unwrap().attributes.raw_fraction(14) < 0,
            "HP still negative (C-143)"
        );
        assert!(
            sim.object(w).unwrap().attributes.current_fraction(30) > xp0,
            "slot 8 is nobody's friend"
        );
    }

    // QUIRK C-150: kept
    #[test]
    fn dying_aborts_all_modules_and_disables_most_after_the_next_tick() {
        let rules = rules();
        let mut state = state(&rules);
        let a = spawn(&mut Sim::new(&mut state, &rules), SABRE, 0, (10, 10));
        let (_, c, _) = kill(&mut state, &rules, a);
        let o = state.object(c, false).unwrap();
        let mut list = o.tick_list.clone();
        list.sort_unstable();
        assert_eq!(
            list,
            vec![0, 0x10],
            "only walk and the dying module keep ticking (C-150)"
        );
    }

    // QUIRK C-145, C-149: kept
    #[test]
    fn rob_and_ghost_flags_follow_the_original() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, SABRE, 0, (10, 10));
        sim.module_mut(a, INDEX).unwrap().base.flags = 0x1310;
        assert_eq!(
            crate::messaging::send(&mut sim, a, &Message::new(0x510, &[0])),
            1
        );
        assert_eq!(
            sim.module(a, INDEX).unwrap().base.flags,
            0x100,
            "&= 0x100 (C-145)"
        );
        let mut m = Message::new(0x610, &[0]);
        assert_eq!(crate::messaging::send_mut(&mut sim, a, &mut m), 0);
        assert_eq!(m.arg(0), 0, "no gold yet before death");
    }

    #[test]
    fn the_attacker_dying_mid_swing_cancels_its_timers() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = {
            let mut sim = Sim::new(&mut state, &rules);
            let x = (10 << 12) + 0x800;
            let mk = |sim: &mut Sim, g, s, fx: i32| {
                crate::world::create_object(
                    sim,
                    &mut crate::world::ObjectDesc::new(
                        g,
                        s,
                        [fx as f32 / 32.0, 0.0, x as f32 / 32.0],
                    ),
                )
                .unwrap()
            };
            let a = mk(&mut sim, WARRIOR, 0, x);
            let t = mk(&mut sim, PIKEMAN, 1, x + 3072);
            sim.object_mut(a).unwrap().direction = crate::fx::Fx(0);
            crate::messaging::post_message(
                &mut sim,
                a,
                Message::new(0x111, &[t.as_i32(), 0]),
                true,
            );
            (a, t)
        };
        for _ in 0..2 * 3 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        Sim::new(&mut state, &rules)
            .object_mut(a)
            .unwrap()
            .attributes
            .set_current_fraction(14, 0);
        let f = state
            .object(t, false)
            .unwrap()
            .attributes
            .current_fraction(14);
        for _ in 0..2 * 20 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        assert_eq!(
            state
                .object(t, false)
                .unwrap()
                .attributes
                .current_fraction(14),
            f,
            "no hit after death"
        );
    }

    // QUIRK E-148: kept
    #[test]
    fn dying_sets_bit_2_on_the_old_id_before_the_owner_change() {
        // `PropertyDying_StartDying` `0x0050EE73`: bit 2 directly after the `DIED` check on the
        // current (old) ID; the owner change to slot 8 is queued and does not transfer the
        // event word (`0x00481730`). A second `0x110` (already `DIED`) changes nothing.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, SABRE, 0, (10, 10));
        send(&mut sim, a, &Message::new(0x110, &[]));
        assert_eq!(crate::world::object_event_bits(&sim, a), 1 << 2);
        assert!(sim.object(a).is_some(), "still the old ID");
        assert_eq!(sim.state.owner_changes.len(), 1, "owner change only queued");
        crate::world::process_owner_changes(&mut sim);
        let corpse = sim
            .state
            .objects()
            .find(|o| o.id.slot() == 8)
            .map(|o| o.id)
            .expect("carcass");
        assert_eq!(crate::world::object_event_bits(&sim, corpse), 0);
        assert_eq!(
            crate::world::object_event_bits(&sim, a),
            1 << 2,
            "stays on the old ID"
        );
        crate::world::set_object_event_bit(&mut sim, corpse, 4, true);
        send(&mut sim, corpse, &Message::new(0x110, &[]));
        assert_eq!(crate::world::object_event_bits(&sim, corpse), 1 << 4);
    }

    // QUIRK E-137, E-189: kept
    #[test]
    fn kill_counter_counts_for_the_last_attacker_slot() {
        // `0x0050F606`: slot = `(last attacker >> 11) & 0x1F`, GUID of the victim; before the
        // owner change. Without an attacker (31) and with a neutral attacker (slot 8) discarded.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let killer = spawn(&mut sim, SABRE, 2, (20, 10));
        let victims = [(10, 10), (12, 10), (14, 10)].map(|t| spawn(&mut sim, PIKEMAN, 0, t));
        let attackers = [killer, ObjectId::NONE, ObjectId::new(3, 8, 1)];
        for (&v, &by) in victims.iter().zip(&attackers) {
            sim.object_mut(v).unwrap().last_attacker = by;
            send(&mut sim, v, &Message::new(0x110, &[]));
        }
        assert_eq!(crate::world::kill_count(&sim, 2, PIKEMAN), 1);
        assert_eq!(sim.state.kill_counts[2].len(), 1);
        for (slot, counts) in sim.state.kill_counts.iter().enumerate() {
            if slot != 2 {
                assert!(counts.is_empty(), "slot {slot}");
            }
        }
        // Already `DIED`: not counted a second time.
        send(&mut sim, victims[0], &Message::new(0x110, &[]));
        assert_eq!(crate::world::kill_count(&sim, 2, PIKEMAN), 1);
    }

    // QUIRK E-189: kept
    #[test]
    fn ghost_kill_counts_without_owner_change() {
        // Ghost branch `0x0050F2FE`: counted after `0x21B`, no owner change.
        use crate::module::ability_testkit as tk;
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = tk::spawn(&mut sim, tk::GHOST, 0, (10, 10));
        assert!(has_module(&sim, g, 0x1B));
        sim.object_mut(g).unwrap().last_attacker = ObjectId::new(4, 3, 1);
        send(&mut sim, g, &Message::new(0x110, &[]));
        assert_eq!(crate::world::kill_count(&sim, 3, tk::GHOST), 1);
        assert_eq!(crate::world::object_event_bits(&sim, g), 1 << 2);
        assert!(sim.state.owner_changes.is_empty());
    }
}
