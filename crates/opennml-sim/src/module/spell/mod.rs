//! Module 38 `CPropertySpell` (vtable `0x00625E28`, re/temp-upgrades 15): animation and effect lists
//! (390002, 390003) and the fields of the spell (kind, button, cost, animation, index, reserve,
//! target point, corpse).
//! Implements kind 4 (execute a temporary upgrade via module 44): init, tick with its own
//! regeneration of the extra energy, cast `0x126`, effect start `0x226`, effect `0x326`, end `0x426`
//! and abort. Also kinds 1 (explosion) and 2 (storm): cast with turning to the target point,
//! deduction not until the effect (re/spells 3.1-3.3), kind 3
//! (reincarnation, [`reincarnate`]: corpse search at the cast, war wolf, reserve and death callbacks
//! at the effect, re/spells 3.4/3.5) and kind 5 (drain mana via the command `0x4E`: envelope with
//! the success message `0xB7F {target}`, [`on_message`]; turning to the target position, deduction
//! and drain at the effect, re/spells 2.3, 3.1, 3.3).
//! The sounds (`0x1E`/`0x1F`) go to the host as events; the effect on the object (390003) is
//! presentation.

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::combat::hit_zone::world_pos;
use crate::combat::projectile::{add_explosion, add_storm};
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_success};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::events::update_events;
use super::gather::list_at;
use super::stand::parse_int_list;
use super::{ModuleKind, exec_temp, init_flags};

pub mod reincarnate;

/// Module index of Spell.
pub const INDEX: usize = 0x26;
/// Kind "explosion" (world effect `0x004F0D50`, re/spells 1.3).
pub const ART_EXPLOSION: i32 = 1;
/// Kind "storm" (world effect `0x004F42C0`, re/spells 1.4).
pub const ART_STORM: i32 = 2;
/// Kind "reincarnation" (re/spells 3.4, 3.5).
pub const ART_REINCARNATE: i32 = 3;
/// Kind "execute a temporary upgrade via module 44" (re/temp-upgrades 15 no. 3).
pub const ART_EXEC_TEMP: i32 = 4;
/// Kind "drain mana" (command `0x4E`, re/spells 2.3).
pub const ART_DRAIN: i32 = 5;
/// Extra energy (mana), attribute 15; the cost is in config 15 of the button GUID.
pub const MANA: u32 = 15;
/// Attack module, switched off during the spell animation.
const ATTACK: usize = 0x11;
/// Group of the timers `0x326`/`0x426` (the abort deletes them).
const GROUP: u8 = 0x80;
/// Sound "Cast" (30) at the cast with a value set (re/spells 7).
const SOUND_CAST: i32 = 0x1E;
/// Sound "Hit" (31) at the effect `0x326` (re/spells 7).
const SOUND_HIT: i32 = 0x1F;
/// Flags of the spell sounds (`Sound3d(..., 0x42, ...)`).
const SOUND_FLAGS: u32 = 0x42;
/// Walk `PMSG_PTTOPT_FINDTO` (argc 7: target, goal radius, look-at point, tolerance, flags).
const FINDTO: u32 = 0x200;

/// Messages to module 38 (`PropertySpell_HandleMessage` `0x0052CD60`).
pub mod msg {
    /// `{kind, anim, idx, button}`: start the spell (`PropertySpell_Cast` `0x0052C040`).
    pub const CAST: u32 = 0x126;
    /// Effect start: animation, lock, timers (`0x0052C470`).
    pub const START: u32 = 0x226;
    /// Effect (`0x0052CA80`).
    pub const EFFECT: u32 = 0x326;
    /// End of the animation (`0x0052BE50`).
    pub const END: u32 = 0x426;
    /// `{..., n}`: lower the reserve of kind 3 by `n`.
    pub const RESERVE: u32 = 0x526;
}

/// `CPropertySpell`, fields of the savegame (re/temp-upgrades 15 no. 1-7, re/spells 1.2, 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spell {
    /// Animations (`+0x70`, 390002).
    pub anims: Vec<i32>,
    /// Effect indices (`+0x7C`, 390003).
    pub effects: Vec<i32>,
    /// Kind of the running spell (`+0x98`).
    pub art: i32,
    /// Button (`+0x94`).
    pub button: i32,
    /// Cost in extra energy (`+0xA4`, config 15).
    pub cost: i32,
    /// Index into [`Spell::anims`] (`+0xB0`).
    pub anim: i32,
    /// Index into the execution list of module 44 (`+0xAC`).
    pub idx: i32,
    /// Reserve of kind 3 (`+0xA8`).
    pub reserve: i32,
    /// `m_Position` (world): target point of kinds 1 and 2 (from the command as `f32` bits, re/spells 1.2);
    /// for kind 5 deviating: the target position at the cast (C-305).
    pub target: [f32; 3],
    /// `m_deadBodyHID`: corpse of the reincarnation (kind 3); may be stale.
    pub corpse: ObjectId,
}

impl Default for Spell {
    /// Default of the constructor: everything 0/empty, target point zero, no corpse (`NONE`).
    fn default() -> Self {
        Self {
            anims: Vec::new(),
            effects: Vec::new(),
            art: 0,
            button: 0,
            cost: 0,
            anim: 0,
            idx: 0,
            reserve: 0,
            target: [0.0; 3],
            corpse: ObjectId::NONE,
        }
    }
}

/// `PropertySpell_Init` (`0x0052C3D0`, re/temp-upgrades 15 no. 1): flags (390001), lists
/// 390002/390003 (`IntList_ParseFromString`), `AttributeSet_SetRegenRate(15, 0)` (the general
/// regeneration of the extra energy rests, the tick of the module regenerates itself) and reserve 0.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let (anims, effects) = (text(390002), text(390003));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        if let ModuleKind::Spell(s) = &mut m.kind {
            s.anims = parse_int_list(&anims, &[]);
            s.effects = parse_int_list(&effects, &[]);
            s.reserve = 0;
        }
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_regen_rate(MANA, 0);
    }
}

fn spell<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Spell> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Spell(s) => Some(s),
        _ => None,
    }
}

fn spell_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Spell> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Spell(s) => Some(s),
        _ => None,
    }
}

/// Upgrade percentage of the owner for the extra energy of the object.
pub(crate) fn mana_percent(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id)
        .map_or(0, |o| sim.type_upgrade_percent(o.owner(), o.guid, MANA))
}

/// Mana cost of a button for the slot `slot` (`GameWorld_GetTypeValueSet(slot, button)`
/// `0x0047C7F0`, then `ValueSet_GetInt(15)`): `None` if the value set is missing - then the
/// button costs nothing and is not checked. Shared by the cast and the command `0x5E`.
pub fn button_cost(sim: &Sim, slot: u8, button: i32) -> Option<i32> {
    sim.rules
        .type_value_set(i32::from(slot), button as u32)
        .map(|set| set.get(&MANA).copied().unwrap_or(0))
}

/// Deduct extra energy (`AttributeSet_AddToCurrentValue(15, -cost)`).
pub fn pay_mana(sim: &mut Sim, id: ObjectId, cost: i32) {
    let p = mana_percent(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.attributes
            .add_to_current_value(MANA, cost.wrapping_neg(), p);
    }
}

/// `PropertySpell_RegenerateExtraEnergy` (`0x0052BD80`, slot 13, re/temp-upgrades 15 no. 2):
/// `Module_UpdateEvents`, then the regeneration with the reserve ([`regenerate_mana`]).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    let Some(reserve) = spell(sim, id).map(|s| s.reserve) else {
        return;
    };
    regenerate_mana(sim, id, reserve);
}

/// Regeneration of the extra energy after `Module_UpdateEvents` (rest of `0x0052BD80`; the
/// animal evocation ticks the same way, `0x0050FE10`, re/evoke-aura 3.4): `m = Max(15) -
/// reserve`; `current(15) < m` -> `AttributeSet_AddScaledToCurrent(15, 1, 10)`, otherwise
/// `SetCurrentValue(15, m)`.
pub(crate) fn regenerate_mana(sim: &mut Sim, id: ObjectId, reserve: i32) {
    let p = mana_percent(sim, id);
    let Some(o) = sim.object_mut(id) else { return };
    let m = o.attributes.max_value(MANA, p).wrapping_sub(reserve);
    if o.attributes.current_value(MANA, p) < m {
        o.attributes.add_scaled_to_current(MANA, 1, 10, p);
    } else {
        o.attributes.set_current_value(MANA, m, p);
    }
}

/// `Query` (slot 7, `PropertySpell_HandleMessage` `0x0052CD60`): cast, effect start, effect, end
/// and reserve; otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    match m.code {
        msg::CAST => cast(sim, id, m),
        msg::START => start(sim, id),
        msg::EFFECT => effect(sim, id),
        msg::END => {
            end(sim, id);
            0
        }
        msg::RESERVE => {
            let n = m.arg(1);
            if let Some(s) = spell_mut(sim, id) {
                s.reserve = s.reserve.wrapping_sub(n);
            }
            0
        }
        _ => -1,
    }
}

/// `OnMessage` (slot 12, `PropertySpell_OnMessage` `0x0052BEB0`): take over the fault and
/// success message of the envelope (`Module_SetFaultMessage` `0x004FD140`, then
/// `Module_SetSuccessMessage` `0x004FD1C0`), then `Query(msg) != -1` - without looking at the code.
/// The only sender is the command `0x4E` (success `0xB7F {target}`, [`effect`] kind 5 sends it);
/// the way via the FIFO (`0x36`) does not set the feedback messages.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    query(sim, id, &env.msg) != -1
}

/// Spell sound `action` (flags `0x42`, object GUID = button) at the position `pos` (presentation).
fn sound(sim: &mut Sim, id: ObjectId, action: i32, pos: [f32; 3], button: i32) {
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action,
        flags: SOUND_FLAGS,
        pos,
        guid: button as u32,
    });
}

/// `PropertySpell_Cast` (`0x0052C040`, re/temp-upgrades 15 no. 3, re/spells 3.1), `{kind, a, b, button}`:
/// remember the kind, height of the target point 0; mounted (`ISMOUNTED`) -> 0. Remember the button,
/// cost = config 15 of the button in the value set of the owner (set missing -> 0 without check and
/// without sound), `current(15) < cost` -> 0, otherwise sound 30 "Cast" at the position of the caster.
/// Kind 4: deduct the mana **immediately** (C-222), remember animation and index, synchronously
/// `0x226` (its return value). Kinds 1, 2 and unknown: target point from `a`/`b` as `f32` bits,
/// animation 0; then turn to the target (`0x0052C2A7`): Walk FINDTO argc 7 to the own grid position
/// with the target point as look-at point, success and fault `0x226` (C-295), synchronously via
/// `GameObject_DispatchPostedCommand` (`0x00473690`); return value 0. Kind 3: animation 0, corpse
/// search ([`reincarnate::find_corpse`], `0x0052BEE0`); without a corpse 0 - the sound has already
/// played, no mana, no animation (C-296) -, otherwise as above with the corpse position as look-at
/// point. These kinds pay the mana not until `0x326` (C-294). Kind 5 (command `0x4E`,
/// `{5, 0, target, button}`): animation `mounted ? 1 : 0` - the riding branch is dead, mounted
/// already yielded 0 above (C-306); if the target `b` is missing
/// (`GameWorld_GetObjectById(target, 0)`), 0 - the sound has already played, no mana. Otherwise as
/// above; the look-at point **deviates**: the world position of the target at the cast at height 0
/// (C-305; the original reads fields `+0x88/+0x90` there that are never written).
fn cast(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let (art, a, b, button) = (m.arg(0), m.arg(1), m.arg(2), m.arg(3));
    let Some((st, owner, pos)) = sim.object(id).map(|o| (o.status, o.owner(), o.world_pos)) else {
        return -1;
    };
    let Some(s) = spell_mut(sim, id) else {
        return -1;
    };
    s.art = art;
    s.target[1] = 0.0;
    if st & status::ISMOUNTED != 0 {
        return 0;
    }
    s.button = button;
    let cost = button_cost(sim, owner, button);
    if let Some(s) = spell_mut(sim, id) {
        s.cost = cost.unwrap_or(0);
    }
    if let Some(c) = cost {
        if sim.attribute(id, MANA) < c {
            return 0;
        }
        sound(sim, id, SOUND_CAST, pos, button);
    }
    match art {
        ART_EXEC_TEMP => {
            pay_mana(sim, id, cost.unwrap_or(0));
            if let Some(s) = spell_mut(sim, id) {
                s.anim = a;
                s.idx = b;
            }
            return start(sim, id);
        }
        ART_REINCARNATE => {
            if let Some(s) = spell_mut(sim, id) {
                s.anim = 0;
            }
            if reincarnate::find_corpse(sim, id).is_none() {
                return 0;
            }
        }
        ART_DRAIN => {
            // Dead branch (C-306): mounted, the cast already ended with 0 above.
            let anim = i32::from(st & status::ISMOUNTED != 0);
            let Some(p) = world_pos(sim, ObjectId::from_i32(b)) else {
                return 0;
            };
            if let Some(s) = spell_mut(sim, id) {
                s.anim = anim;
                s.target = [p[0], 0.0, p[2]];
            }
        }
        _ => {
            if let Some(s) = spell_mut(sim, id) {
                s.target = [f32::from_bits(a as u32), 0.0, f32::from_bits(b as u32)];
                s.anim = 0;
            }
        }
    }
    face_target(sim, id);
    0
}

/// Turn to the target point (`0x0052C2A7`, re/spells 3.1): `fx = Fixed_FromFloat(x / cell size)`
/// (`FILD [+0x34]; FDIVR`), target = own grid position (fixed point), goal radius and
/// tolerance -1 (defaults), flags 1. Success = fault = `0x226` to the own object.
fn face_target(sim: &mut Sim, id: ObjectId) {
    let (Some(grid), Some(target)) = (
        sim.object(id).map(|o| o.grid),
        spell(sim, id).map(|s| s.target),
    ) else {
        return;
    };
    let cell = sim.rules.map.cell;
    let fx = Fx::from_f32(target[0] / cell[0] as f32).0;
    let fz = Fx::from_f32(target[2] / cell[2] as f32).0;
    let started = || Some(Message::new(msg::START, &[]));
    let env = Envelope {
        msg: Message::new(FINDTO, &[grid.x.0, grid.z.0, -1, fx, fz, -1, 1]),
        success: started(),
        fault: started(),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `0x226` (`0x0052C470`, re/temp-upgrades 15 no. 4): `a = 390002[anim]` (`IntList_GetAt`
/// `0x00405840`: outside the list 0); `a < 0` -> immediately `0x326` (effect without animation).
/// Otherwise `ANIM_LOCKED`, attack module off via the pending list
/// (`GameObject_QueueModuleTickChange(0x11, 0)`), animation `a` at speed 128; timers (group `0x80`,
/// no repeat) `0x326` after `floor(T/2)` and `0x426` after `T` ms with `T` = time marker 1 of the
/// **first** animation of the list (`GameObject_GetAnimLabelTime(390002[0], 1)`, C-229), not of the
/// selected one.
fn start(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((a, first)) = spell(sim, id).map(|s| {
        let a = usize::try_from(s.anim).map_or(0, |i| list_at(&s.anims, i));
        (a, list_at(&s.anims, 0))
    }) else {
        return -1;
    };
    if a < 0 {
        return effect(sim, id);
    }
    let Some(o) = sim.object_mut(id) else {
        return -1;
    };
    o.set_status_flags(status::ANIM_LOCKED, true);
    o.queue_module_tick_change(ATTACK, false);
    o.set_animation(a, 0, false, false, false);
    let guid = o.guid;
    let t = sim.rules.anim_label_time_at(guid, first, 1);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(t / 2, Message::new(msg::EFFECT, &[]), false, GROUP);
        m.base
            .post_delayed_message(t, Message::new(msg::END, &[]), false, GROUP);
    }
    0
}

/// `0x326` (`0x0052CA80`, re/temp-upgrades 15 no. 5, re/spells 3.3): kind 1 creates the explosion, kind 2
/// the storm at the target point (`GameWorld_AddEffect`, appended to the projectile list) and
/// only now deducts the cost - without a check, the mana can become negative (C-294, K-62);
/// kind 3 deducts the same way, puts the cost on the reserve (lowers the upper limit of the
/// regeneration, C-299) and reincarnates ([`reincarnate::reincarnate`], `0x0052C8F0`); kind 4 sends
/// `{0x12C, [idx]}` synchronously to the own object (module 44, [`exec_temp`]); kind 5 deducts the
/// cost the same way without a check and sends the stored success message
/// (`Module_SendSuccessMessage` `0x004FD0E0`: `0xB7F {target}` from the envelope of the command
/// `0x4E` - all the mana of the target is turned into hit points; if the target is missing, `0xB7F`
/// does nothing and the cost is gone anyway, C-303; for the second of two overlapping drains it is
/// already used up, C-304). Then sound 31 "Hit" (kinds 1/2 at the target point, otherwise at the
/// position of the caster), return value 0. Unknown kind: `no valid spellid` -> -1 without sound.
fn effect(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((art, idx, cost, button, target)) =
        spell(sim, id).map(|s| (s.art, s.idx, s.cost, s.button, s.target))
    else {
        return -1;
    };
    let Some(own) = sim.object(id).map(|o| o.world_pos) else {
        return -1;
    };
    let pos = match art {
        ART_EXPLOSION => {
            add_explosion(sim, id, target);
            pay_mana(sim, id, cost);
            target
        }
        ART_STORM => {
            add_storm(sim, id, target);
            pay_mana(sim, id, cost);
            target
        }
        ART_REINCARNATE => {
            pay_mana(sim, id, cost);
            if let Some(s) = spell_mut(sim, id) {
                s.reserve = s.reserve.wrapping_add(cost);
            }
            reincarnate::reincarnate(sim, id);
            own
        }
        ART_EXEC_TEMP => {
            send(sim, id, &Message::new(exec_temp::msg::EXECUTE, &[idx]));
            own
        }
        ART_DRAIN => {
            pay_mana(sim, id, cost);
            send_success(sim, id, INDEX);
            own
        }
        _ => {
            log::debug!("Spell {id:?}: no valid spellid {art}");
            return -1;
        }
    };
    sound(sim, id, SOUND_HIT, pos, button);
    0
}

/// `0x426` (`0x0052BE50`, re/temp-upgrades 15 no. 6): clear `ANIM_LOCKED`; if the object has the
/// attack module (even switched off, `GameObject_HasModule(0x11, 1)`), switch it back on via the
/// pending list.
fn end(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
        if o.has_module(ATTACK, true) {
            o.queue_module_tick_change(ATTACK, true);
        }
    }
}

/// `Abort` (slot 11, `0x0052BDF0`): delete timer group `0x80`, then like `0x426`; return value
/// true. An abort between cast and `0x326` forfeits the already deducted mana without effect for kind 4
/// (C-230); it costs nothing for kinds 1/2 (deduction not until `0x326`, re/spells 8 no. 1).
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    end(sim, id);
    true
}

#[cfg(test)]
mod tests;
