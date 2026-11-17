//! Module 32 `CPropertyEvokeAnimal` (`PropertyEvokeAnimal_Construct` `0x00510240`, vtable
//! `0x00623530`, re/evoke-aura 1.2): animal evocation with animation, animal kind, spawn position, cost,
//! button and the reserve (sum of the costs of living animals).
//!
//! State, `Init`, cast, reserve, position search ([`spawn`]), abort and tick, and the appearance
//! ([`evoke`]) with the death callbacks. Effect name, node name and effect ID are presentation and
//! absent from the state. The name counter `+0x98` is kept in [`EvokeAnimal::name_counter`]. No change
//! of owner (slot 5 empty), never busy (slot 10 `0x00532A90`).

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::events::update_events;
use crate::module::gather::{anim_label_time, set_animation};
use crate::module::spell::{button_cost, pay_mana, regenerate_mana};
use crate::module::{ModuleKind, attack, init_flags};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{
    ObjectDesc, create_object, desc_flags, grid_to_world, set_grid_position, set_object_event_bit,
};

pub(crate) mod spawn;
#[cfg(test)]
mod tests;

/// Module index EvokeAnimal.
pub const INDEX: usize = 0x20;
/// Animal GUIDs by kind (`m_AnimType`): 0 wolf, 1 bear, 2 cat (re/evoke-aura 1.2).
pub const ANIMAL_GUIDS: [u32; 3] = [3401, 3402, 3400];
/// Extra energy (mana), attribute 15.
const MANA: u32 = 15;
/// Hit points, attribute 14.
const HITPOINTS: u32 = 14;
/// Timer group of `0x220` (`Module_PostDelayedMessage` `0x004FD690`, re/evoke-aura 3.1).
const TIMER_GROUP: u8 = 0x80;
/// Event bit of the scripts (`GameWorld_SetObjectEventBit(hid, 4, …)`: off on the cast, on on
/// appearing; evaluated in the scripts).
const EVENT_BIT: u32 = 4;
/// Sound "wolf/bear/cat" (`0x14 + kind`, ID 3, type GUID) and "evocation begins" (`0x1E`,
/// ID `0x42`, button GUID) for the host.
const SOUND_ANIMAL: i32 = 0x14;
const SOUND_ANIMAL_FLAGS: u32 = 3;
const SOUND_START: i32 = 0x1E;
const SOUND_FLAGS: u32 = 0x42;

/// Messages to module 32 (query `0x00510B30`, re/evoke-aura 2.1).
pub mod msg {
    /// `{kind, button}`: begin the evocation (`0x005103B0`).
    pub const CAST: u32 = 0x120;
    /// Let the animal appear (own timer, `0x005107D0`).
    pub const EVOKE: u32 = 0x220;
    /// `{…, cost}`: lower the reserve by the cost of a dead animal (death callback).
    pub const RELEASE: u32 = 0x320;
}

/// `CPropertyEvokeAnimal`, fields of the savegame (`SerializeFields` `0x00510B80`, re/evoke-aura 1.2,
/// 5, 10.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvokeAnimal {
    /// `m_EvokeAnim` (`+0x70`, 330001): animation index.
    pub anim: i32,
    /// `m_AnimType` (`+0x74`): kind 0 wolf, 1 bear, 2 cat (from `0x120`).
    pub art: i32,
    /// `m_SpawnPos` (`+0x78`): position at the start of the effect, order of `FxVec3` (x, y, z).
    pub spawn: [i32; 3],
    /// `m_ExtraEnergy` (`+0x94`): cost of the running evocation.
    pub cost: i32,
    /// `m_SpellGuid` (`+0x9C`): button GUID (sound).
    pub button: i32,
    /// `m_UsedExtraEnergy` (`+0xA0`): reserve, sum of the costs of living animals (+ running ones).
    pub reserve: i32,
    /// Name counter `+0x98`: suffix `"%02d"` of the script name of every created animal,
    /// value before incrementing, start 0.
    pub name_counter: i32,
}

pub fn evoke_animal<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s EvokeAnimal> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::EvokeAnimal(e) => Some(e),
        _ => None,
    }
}

pub fn evoke_animal_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut EvokeAnimal> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::EvokeAnimal(e) => Some(e),
        _ => None,
    }
}

/// `PropertyEvokeAnimal_Init` (`0x00510770`, slot 9, re/evoke-aura 1.2): animation 330001,
/// `AttributeSet_SetRegenRate(15, 0)` (the general mana regeneration rests, the tick of the module
/// regenerates itself) and reserve 0; 330002 is not read, the flags are 0.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let anim = desc.value_int(330_001).unwrap_or(0) as i32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        if let ModuleKind::EvokeAnimal(e) = &mut m.kind {
            e.anim = anim;
            e.reserve = 0;
        }
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_regen_rate(MANA, 0);
    }
}

/// Query (slot 7, `0x00510B30`, re/evoke-aura 2.1): `0x120 {kind, button}` cast ([`cast`]), `0x220`
/// appearing ([`evoke`]), `0x320 {…, cost}` lowers the reserve (`+0xA0 −= msg[3]`, return value 1);
/// otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, message: &Message) -> i32 {
    match message.code {
        msg::CAST => cast(sim, id, message.arg(0), message.arg(1)),
        msg::EVOKE => evoke(sim, id),
        msg::RELEASE => {
            let cost = message.arg(1);
            if let Some(e) = evoke_animal_mut(sim, id) {
                e.reserve = e.reserve.wrapping_sub(cost);
            }
            1
        }
        _ => -1,
    }
}

/// OnMessage (slot 12, `0x00510730`): only the envelope `0x120` - take over the failure and success
/// message (`Module_SetFaultMessage`/`SetSuccessMessage`; both are never sent), cast,
/// `true`; otherwise `false`.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if env.msg.code != msg::CAST {
        return false;
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    query(sim, id, &env.msg);
    true
}

/// `EvokeAnimal_Cast` (`0x005103B0`, re/evoke-aura 3.1): first abort a running evocation
/// (slot 11, then continue). Remember the button, cost = config 15 of its value set (without a set 0 and
/// unchecked, C-281), set **before** the check; `current(15) < cost` -> 0. Then event bit 4
/// off and the time marker 1 of the evocation animation; `< 1` -> 0 without mana (C-293). Remember the kind; for
/// kind 0...2 deduct mana immediately (`AttributeSet_AddToCurrentValue`, without lower limit), raise the reserve by
/// the cost and sound `0x14 + kind`; other kinds neither mana nor reserve (C-282).
/// `EVOKING` on, attack switched **on** via the pending list (spell switches it off, C-283), animation,
/// `0x220` after the marker (group `0x80`), remember the position, sound `0x1E`. The host derives the beam from
/// `EVOKING` and [`EvokeAnimal::spawn`] (deviation from the original). Return value 1 (the FIFO discards it).
///
/// Deviation (exception A7, K-312): the position ([`spawn::find_spawn_pos`]) is searched **before** the
/// deduction instead of after the timer; it depends on nothing that the cast changes in between
/// (same inputs, same position). Without a position the cast fails with 0 before mana flows.
fn cast(sim: &mut Sim, id: ObjectId, art: i32, button: i32) -> i32 {
    let Some(o) = sim.object(id) else { return 0 };
    if o.status & status::EVOKING != 0 {
        abort(sim, id);
    }
    let Some(slot) = sim.object(id).map(|o| o.owner()) else {
        return 0;
    };
    let cost = button_cost(sim, slot, button);
    let current = sim.attribute(id, MANA);
    let Some(e) = evoke_animal_mut(sim, id) else {
        return 0;
    };
    e.button = button;
    e.cost = cost.unwrap_or(0);
    if cost.is_some_and(|c| current < c) {
        return 0;
    }
    let (anim, cost) = (e.anim, e.cost);
    set_object_event_bit(sim, id, EVENT_BIT, false);
    let t = anim_label_time(sim, id, anim, 1);
    if t < 1 {
        return 0;
    }
    let Some(spawn) = spawn::find_spawn_pos(sim, id) else {
        return 0;
    };
    let Some(e) = evoke_animal_mut(sim, id) else {
        return 0;
    };
    e.art = art;
    if (0..=2).contains(&art) {
        e.reserve = e.reserve.wrapping_add(cost);
        pay_mana(sim, id, cost);
        sound(sim, id, SOUND_ANIMAL + art, SOUND_ANIMAL_FLAGS, None);
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::EVOKING, true);
        o.queue_module_tick_change(attack::INDEX, true);
    }
    set_animation(sim, id, anim);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(t, Message::new(msg::EVOKE, &[]), false, TIMER_GROUP);
        if let ModuleKind::EvokeAnimal(e) = &mut m.kind {
            e.spawn = spawn;
        }
    }
    sound(sim, id, SOUND_START, SOUND_FLAGS, Some(button));
    1
}

/// `EvokeAnimal_Evoke` (`0x005107D0`, re/evoke-aura 3.2): without `EVOKING` 0. GUID by kind from
/// [`ANIMAL_GUIDS`] (C-177), otherwise abort (refund) and 0. The animal is created as a start object
/// (`0x8C3`, `GameWorld_CreateObject` `0x00485F70`) with slot and color of the evoker at
/// `grid_to_world(spawn)`; if that fails, abort and 0. Then the script name `<name>NN` with the
/// name counter before incrementing (`FUN_0043E310`), exactly at `spawn`
/// (`SetGridPosition` `0x00470F00`), two death callbacks at the animal (`0x310`: `{0x114}` to the animal
/// itself - never delivered under the old ID, C-278 -, `{0x320, 0, cost}` to the
/// evoker, C-276), `{0x314}` to the animal (`animal::query`, re/evoke-aura 3.2a), attack of the
/// evoker into the tick list (`HasModule(0x11, 1)`), `EVOKING` off, event bit 4 on, `{0x414}`
/// (fade in), sound `0x1E` (ID `0x42`, button) at the animal. Finally
/// `AttributeSet_GetModifierAbsolute(0)` (`0x0048F7C0`) -> `SetModifier(14, 2 · v, 0, 0)`
/// (`0x00510A89…0x00510AAA`) doubles the base value of the hit points; the full fraction stays (C-284,
/// corrected values: wolf 160, bear 240, cat 160). Beam ends are presentation
/// (deviation from the original). Return value 1.
fn evoke(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(o) = sim.object(id) else { return 0 };
    if o.status & status::EVOKING == 0 {
        return 0;
    }
    let slot = i32::from(o.owner());
    let Some(e) = evoke_animal(sim, id) else {
        return 0;
    };
    let (art, spawn, cost, button) = (e.art, e.spawn, e.cost, e.button);
    let Some(&guid) = usize::try_from(art).ok().and_then(|a| ANIMAL_GUIDS.get(a)) else {
        abort(sim, id);
        return 0;
    };
    let pos = FxVec3::new(Fx(spawn[0]), Fx(spawn[1]), Fx(spawn[2]));
    let mut desc = ObjectDesc::new(guid, slot, grid_to_world(pos, sim.rules.map.cell));
    desc.flags = desc_flags::START_OBJECT;
    desc.color = crate::setup::player_color(sim, slot);
    let Some(animal) = create_object(sim, &mut desc) else {
        abort(sim, id);
        return 0;
    };
    // `0x005108E2`...`0x0051093A`: increment the counter after creating, pass the old value as the suffix
    // to `FUN_0043E310` (also without a name of the evoker).
    let counter = evoke_animal_mut(sim, id).map_or(0, |e| {
        let c = e.name_counter;
        e.name_counter = c.wrapping_add(1);
        c
    });
    crate::names::transfer(&mut sim.state.names, id, animal, &format!("{counter:02}"));
    set_grid_position(sim, animal, pos, true);
    let listen = |success: Message, target: ObjectId| Envelope {
        msg: Message::new(0x310, &[]),
        success: Some(success),
        fault: None,
        success_target: target,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, animal, &listen(Message::new(0x114, &[]), animal));
    dispatch_command(
        sim,
        animal,
        &listen(Message::new(msg::RELEASE, &[0, cost]), id),
    );
    send(sim, animal, &Message::new(0x314, &[0]));
    if let Some(o) = sim.object_mut(id) {
        if o.has_module(attack::INDEX, true) {
            o.queue_module_tick_change(attack::INDEX, true);
        }
        o.set_status_flags(status::EVOKING, false);
    }
    set_object_event_bit(sim, id, EVENT_BIT, true);
    send(sim, animal, &Message::new(0x414, &[]));
    sound(sim, animal, SOUND_START, SOUND_FLAGS, Some(button));
    if let Some(o) = sim.object_mut(animal) {
        let v = o.attributes.base(HITPOINTS);
        o.attributes
            .set_modifier(HITPOINTS, v.wrapping_mul(2), false, 0);
    }
    1
}

/// `Sound3d` at the world position of the object `id` with the ID `guid` (without: the type GUID).
fn sound(sim: &mut Sim, id: ObjectId, action: i32, flags: u32, guid: Option<i32>) {
    let Some(o) = sim.object(id) else { return };
    let (pos, own) = (o.world_pos, o.guid);
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action,
        flags,
        pos,
        guid: guid.map_or(own, |g| g as u32),
    });
}

/// Abort (slot 11, `0x0050FE80`, re/evoke-aura 3.3): clear group `0x80` (`0x220` gone), attack
/// back into the tick list if present - also switched off (`GameObject_HasModule(0x11, 1)`);
/// during `EVOKING` refund the cost in full (C-277) and take it out of the reserve.
/// `EVOKING` off, `true`. The host ends the beam with `EVOKING` (deviation from the original).
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(TIMER_GROUP);
    }
    let Some(o) = sim.object_mut(id) else {
        return true;
    };
    if o.has_module(attack::INDEX, true) {
        o.queue_module_tick_change(attack::INDEX, true);
    }
    let evoking = o.status & status::EVOKING != 0;
    if evoking && let Some(cost) = evoke_animal(sim, id).map(|e| e.cost) {
        pay_mana(sim, id, cost.wrapping_neg());
        if let Some(e) = evoke_animal_mut(sim, id) {
            e.reserve = e.reserve.wrapping_sub(cost);
        }
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::EVOKING, false);
    }
    true
}

/// Tick (slot 13, `0x0050FE10`, re/evoke-aura 3.4): `Module_UpdateEvents`, then the regeneration like
/// spell with the reserve (cap `Max(15) − reserve`, also downward).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    let Some(reserve) = evoke_animal(sim, id).map(|e| e.reserve) else {
        return;
    };
    regenerate_mana(sim, id, reserve);
}
