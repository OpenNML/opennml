//! Module 26 `CPropertyEvokeGhost` (`PropertyEvokeGhost_Construct` `0x00510FA0`, vtable
//! `0x00623628`, re/ghosts-banish 1.2): search radius, corpse position and ID, most recently created
//! ghost, evocation animation, cost and button of the last `0x11A`.
//!
//! Effect names and IDs (`m_HandEffectName` ... `m_HandEffectID`) are presentation (the host derives the
//! hand and staff effects from `EVOKING`, deviation from the original) and absent from the state; the name
//! counter `+0xB0` is kept in [`EvokeGhost::name_counter`]. No change of owner
//! (slot 5 empty), never busy (slot 10).

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{ObjectDesc, create_object, desc_flags, grid_to_world, set_object_event_bit};

use super::ghost;
use crate::module::gather::{anim_label_time, set_animation};
use crate::module::spell::{MANA, button_cost, mana_percent};
use crate::module::{ModuleKind, attack, init_flags, walk};

/// Module index EvokeGhost.
pub const INDEX: usize = 0x1A;
/// GUID of the evoked ghost (`0x585`, `0x005114D0`).
pub const GHOST_GUID: u32 = 1413;
/// Corpse search radius in tiles (`[0x006727C8] * 10`, `0x00511450`).
pub const RADIUS_TILES: i32 = 10;
/// Delay `0x41A` -> `0x21A` (create ghost) in ms (`0x0051120A`).
pub const CREATE_DELAY_MS: i32 = 200;
/// Fixed-point one (`[0x006727C8]`, initializer `0x00611200`).
const FIXED_ONE: i32 = 4096;
/// Timer group of `0x21A`/`0x31A` (`Module_PostDelayedMessage`, re/ghosts-banish 3.1).
const TIMER_GROUP: u8 = 0x80;
/// Corpse filter (`0x00510D90`, re/ghosts-banish 1.6): `RETURN_DIED | 0x1000000 | CHECKNOTHANDLETYPE
/// | ANDALL | CHECKPROPERTY | CHECKSTATUS`; `MatchesFilter` does not evaluate `0x1000000`.
const CORPSE_MODE: u32 = 0x500_4015;
/// Death flag `MID_PROP_DIE_GHOST` at the dying module (`0x100`, low byte = module 16; C-154).
const CORPSE_BITS: u32 = 0x110;
/// Player slot of the corpse search (`0x00510D90`; corpses belong to slot 8, without player check).
const CORPSE_PLAYER: i32 = 8;
/// Event bit of the scripts (`GameWorld_SetObjectEventBit(hid, 4, ...)`: off at `0x41A`, on at
/// `0x31A`; evaluated by the scripts).
const EVENT_BIT: u32 = 4;
/// Walk `FINDTO` (argc 7: target, radius, look point, tolerance, flags; re/walk 13).
const FINDTO: u32 = 0x200;
/// Dying module: `PMSG_DIE_SETGHOSTFLAG {f}` and `PMSG_DIE_SETDEATHLISTENER` (re/ghosts-banish 2.4).
const SET_GHOST_FLAG: u32 = 0x510;
const SET_DEATH_LISTENER: u32 = 0x310;
/// Sound "evocation begins" at the evoker or "ghost appears" at the ghost (host).
const SOUND_START: i32 = 0x1E;
const SOUND_GHOST: i32 = 0x1F;
const SOUND_FLAGS: u32 = 0x42;

/// Messages to module 26 (query `0x00511700`, re/ghosts-banish 2.1).
pub mod msg {
    /// `{button}`: start evoking (command `0x1F`, `0x00510D90`).
    pub const EVOKE: u32 = 0x11A;
    /// Create ghost (own timer, `0x005114D0`).
    pub const CREATE: u32 = 0x21A;
    /// Evocation animation finished (own timer).
    pub const STOP_ANIM: u32 = 0x31A;
    /// Arrival at the corpse (walk success, `0x00511170`).
    pub const START: u32 = 0x41A;
    /// Path to the corpse failed (walk fault).
    pub const ABORT: u32 = 0x51A;
    /// Death callback of the ghost (dying module).
    pub const GHOST_DIED: u32 = 0x61A;
}

/// `CPropertyEvokeGhost`, fields of the savegame (serialization `0x00511850`, re/ghosts-banish 1.2,
/// 5, 10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvokeGhost {
    /// `m_EvokeRadius` (`+0x70`): search radius in fixed point (Init 40,960).
    pub radius: i32,
    /// `m_DeadBodyPos` (`+0x78`): corpse position, order of `FxVec3` (x, y, z).
    pub dead_body_pos: [i32; 3],
    /// `m_DeadBodyHID` (`+0x88`): corpse; may be stale.
    pub dead_body: ObjectId,
    /// `m_GhostHID` (`+0x8C`): most recently created ghost (never reset); may be stale.
    pub ghost: ObjectId,
    /// `m_EvokeAnim` (`+0x90`, 270001): evocation animation.
    pub anim: i32,
    /// `m_ExtraEnergy` (`+0xAC`): cost of the last `0x11A` (config 15 of the button).
    pub extra_energy: i32,
    /// `m_SpellGuid` (`+0xB4`): button GUID of the last `0x11A` (sound argument).
    pub spell_guid: i32,
    /// Name counter `+0xB0`: suffix `"%02d"` of the script name of every created ghost,
    /// value before incrementing, start 0.
    pub name_counter: i32,
}

// `ObjectId` has no `Default`; the IDs start as "no object".
impl Default for EvokeGhost {
    fn default() -> Self {
        Self {
            radius: 0,
            dead_body_pos: [0; 3],
            dead_body: ObjectId::NONE,
            ghost: ObjectId::NONE,
            anim: 0,
            extra_energy: 0,
            spell_guid: 0,
            name_counter: 0,
        }
    }
}

pub fn evoke_ghost<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s EvokeGhost> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::EvokeGhost(e) => Some(e),
        _ => None,
    }
}

pub fn evoke_ghost_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut EvokeGhost> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::EvokeGhost(e) => Some(e),
        _ => None,
    }
}

/// `PropertyEvokeGhost_Init` (`0x00511450`, slot 9, re/ghosts-banish 1.2): radius `Fixed(1) * 10`,
/// animation 270001; the flags are 0 (no data value).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let anim = desc.value_int(270_001).unwrap_or(0) as i32;
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::EvokeGhost(e) = &mut m.kind {
        e.radius = RADIUS_TILES * FIXED_ONE;
        e.anim = anim;
    }
}

/// `PropertyEvokeGhost_Query` (`0x00511700`, slot 7, re/ghosts-banish 2.1): `0x11A` starts (-1 too
/// little mana, otherwise 0), `0x41A` starts at the corpse (1 or 0), `0x21A` creates the ghost,
/// `0x31A` ends the animation, `0x51A` aborts, `0x61A` is the death callback of the ghost (all
/// 1); otherwise -1. A switched-off module receives no message (`send`).
pub fn query(sim: &mut Sim, id: ObjectId, message: &mut Message) -> i32 {
    match message.code {
        msg::EVOKE => evoke(sim, id, message.arg(0)),
        msg::START => start_evoking(sim, id),
        msg::CREATE => {
            if sim
                .object(id)
                .is_some_and(|o| o.status & status::EVOKING != 0)
            {
                create_ghost(sim, id);
            }
            1
        }
        msg::STOP_ANIM => {
            stop_anim(sim, id);
            1
        }
        msg::ABORT => {
            abort(sim, id);
            1
        }
        msg::GHOST_DIED => {
            let extra = evoke_ghost(sim, id).map_or(0, |e| e.extra_energy);
            release_mana(sim, id, extra);
            1
        }
        _ => -1,
    }
}

/// Timer `code` after `delay_ms` (group `0x80`, once; `Module_PostDelayedMessage`).
fn post_delayed(sim: &mut Sim, id: ObjectId, delay_ms: i32, code: u32) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(delay_ms, Message::new(code, &[]), false, TIMER_GROUP);
    }
}

fn current_mana(sim: &Sim, id: ObjectId) -> i32 {
    let p = mana_percent(sim, id);
    sim.object(id)
        .map_or(0, |o| o.attributes.current_value(MANA, p))
}

/// Change the mana base value (key-0 absolute value, `AttributeSet_GetModifierAbsolute` `0x0048F7C0`) by
/// `delta`, then set the current value `current` (`SetModifier(15, ..., 0, 0)`,
/// `SetCurrentValue(15, ...)`; re/ghosts-banish 3.1-3.3, C-257).
fn shift_mana_base(sim: &mut Sim, id: ObjectId, delta: i32, current: i32) {
    let p = mana_percent(sim, id);
    if let Some(o) = sim.object_mut(id) {
        let base = o.attributes.base(MANA);
        o.attributes
            .set_modifier(MANA, base.wrapping_add(delta), false, 0);
        o.attributes.set_current_value(MANA, current, p);
    }
}

/// Release bound mana: base value `+ extra`, current value unchanged (`0x61A` and abort,
/// re/ghosts-banish 3.2, 3.3; quirk with the **current** `m_ExtraEnergy`, C-257).
fn release_mana(sim: &mut Sim, id: ObjectId, extra: i32) {
    let cur = current_mana(sim, id);
    shift_mana_base(sim, id, extra, cur);
}

/// Attack (module 17) back into the tick list if present - also switched off
/// (`GameObject_HasModule(0x11, 1)`, `0x00511745`).
fn resume_attack(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id)
        && o.has_module(attack::INDEX, true)
    {
        o.queue_module_tick_change(attack::INDEX, true);
    }
}

/// `EvokeGhost_Start` (`0x00510D90`, re/ghosts-banish 3.1): remember the button (before any check), cost
/// = config 15 of its value set (without a set free and unchecked, C-281 remainder); too little mana ->
/// -1. Then the next unused corpse - every dead walk object without death flag `GHOST`, without
/// player check (C-256, C-154) - at edge distance up to 10 tiles + own hard radius; none ->
/// 0 without effect. Otherwise remember corpse and grid position and walk `FINDTO` with the **own** position as
/// target and the corpse as look point (argc 7, C-255): success `0x41A`, fault `0x51A`. Returns 0.
fn evoke(sim: &mut Sim, id: ObjectId, button: i32) -> i32 {
    let Some(o) = sim.object(id) else { return 0 };
    let (slot, grid, hard) = (o.owner(), o.grid, o.hard_radius);
    let cost = button_cost(sim, slot, button);
    let cur = current_mana(sim, id);
    let Some(e) = evoke_ghost_mut(sim, id) else {
        return 0;
    };
    e.spell_guid = button;
    match cost {
        None => e.extra_energy = 0,
        Some(c) => {
            e.extra_energy = c;
            if cur < c {
                return -1;
            }
        }
    }
    let radius = e.radius;
    let f = MatchFilter {
        mode: CORPSE_MODE,
        mode2: 0,
        status: status::DIED,
        match_bits: CORPSE_BITS,
        prop_id: walk::INDEX as u32,
        ..MatchFilter::constructed()
    };
    let Some(body) =
        find_nearest_matching(sim, (grid.x.0, grid.z.0), &f, radius, CORPSE_PLAYER, hard)
    else {
        return 0;
    };
    let Some(c) = sim.object(body).map(|o| o.grid) else {
        return 0;
    };
    if let Some(e) = evoke_ghost_mut(sim, id) {
        e.dead_body = body;
        e.dead_body_pos = [c.x.0, c.y.0, c.z.0];
    }
    let env = Envelope {
        msg: Message::new(FINDTO, &[grid.x.0, grid.z.0, -1, c.x.0, c.z.0, -1, 1]),
        success: Some(Message::new(msg::START, &[])),
        fault: Some(Message::new(msg::ABORT, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
    0
}

/// `EvokeGhost_StartEvoking` (`0x41A`, `0x00511170`, re/ghosts-banish 3.1): check the mana again, the
/// corpse must still be there (`GameWorld_GetObjectById(..., 0)`, also dying) - otherwise 0 without
/// effect. Then event bit 4 off, sound, `0x21A` after 200 ms, **base value** and current value lowered by
/// the cost (C-257), `EVOKING` on, attack out of the tick list, evocation animation,
/// `0x31A` after its marker 1 and `0x510 {1}` to the corpse (consumed; only sender of
/// `0x510`, C-145). The host derives the hand and staff effect from `EVOKING` (deviation from the original).
/// Returns 1.
fn start_evoking(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(e) = evoke_ghost(sim, id) else {
        return 0;
    };
    let (extra, body, anim, guid) = (e.extra_energy, e.dead_body, e.anim, e.spell_guid);
    let cur = current_mana(sim, id);
    if cur < extra || sim.object(body).is_none() {
        return 0;
    }
    set_object_event_bit(sim, id, EVENT_BIT, false);
    let Some(pos) = sim.object(id).map(|o| o.world_pos) else {
        return 0;
    };
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: SOUND_START,
        flags: SOUND_FLAGS,
        pos,
        guid: guid as u32,
    });
    post_delayed(sim, id, CREATE_DELAY_MS, msg::CREATE);
    shift_mana_base(sim, id, extra.wrapping_neg(), cur.wrapping_sub(extra));
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::EVOKING, true);
        o.queue_module_tick_change(attack::INDEX, false);
    }
    set_animation(sim, id, anim);
    let t = anim_label_time(sim, id, anim, 1);
    post_delayed(sim, id, t, msg::STOP_ANIM);
    send(sim, body, &Message::new(SET_GHOST_FLAG, &[1]));
    1
}

/// `EvokeGhost_CreateGhost` (`0x21A`, `0x005114D0`, re/ghosts-banish 3.2; only with `EVOKING`): ghost
/// 1413 as start object (`0x8C3`) with slot and color of the evoker at the corpse position
/// remembered at search time (grid * cell via `f32`, even if the corpse has gone missing in the meantime).
/// If creation fails, the mana stays bound (C-261). Otherwise remember `ghost`, `0x11B`
/// (appearing), death callback `0x61A` via the `0x310` envelope to the dying module of the ghost and
/// the sound at the ghost. Before remembering, the ghost gets the script name `<name>NN` with the
/// name counter before incrementing (`0x005115A7`...`0x005115FF`); dummy effects are host.
fn create_ghost(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let slot = i32::from(o.owner());
    let Some(e) = evoke_ghost(sim, id) else {
        return;
    };
    let (p, guid) = (e.dead_body_pos, e.spell_guid);
    let grid = FxVec3::new(Fx(p[0]), Fx(p[1]), Fx(p[2]));
    let mut desc = ObjectDesc::new(GHOST_GUID, slot, grid_to_world(grid, sim.rules.map.cell));
    desc.flags = desc_flags::START_OBJECT;
    desc.color = crate::setup::player_color(sim, slot);
    let Some(g) = create_object(sim, &mut desc) else {
        return;
    };
    let counter = evoke_ghost_mut(sim, id).map_or(0, |e| {
        let c = e.name_counter;
        e.name_counter = c.wrapping_add(1);
        c
    });
    crate::names::transfer(&mut sim.state.names, id, g, &format!("{counter:02}"));
    if let Some(e) = evoke_ghost_mut(sim, id) {
        e.ghost = g;
    }
    send(sim, g, &Message::new(ghost::msg::APPEAR, &[]));
    let env = Envelope {
        msg: Message::new(SET_DEATH_LISTENER, &[]),
        success: Some(Message::new(msg::GHOST_DIED, &[])),
        fault: None,
        success_target: id,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, g, &env);
    if let Some(pos) = sim.object(g).map(|o| o.world_pos) {
        sim.state.events.push(SimEvent::Sound3d {
            object: g,
            action: SOUND_GHOST,
            flags: SOUND_FLAGS,
            pos,
            guid: guid as u32,
        });
    }
}

/// `0x31A` (timer at marker 1 of the evocation animation, re/ghosts-banish 2.1): `EVOKING` off,
/// attack back on (if present), event bit 4 on.
fn stop_anim(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::EVOKING, false);
    }
    resume_attack(sim, id);
    set_object_event_bit(sim, id, EVENT_BIT, true);
}

/// `PropertyEvokeGhost_Abort` (`0x00510C70`, slot 11, re/ghosts-banish 3.3): with `EVOKING` the
/// most recently created ghost gets `0x21B` (fade-out) - before `0x21A` that is the **previous** one (C-259) -,
/// the base value rises again by the cost (between `0x21A` and `0x31A` the death of the ghost returns
/// it a second time, C-258), `EVOKING` goes off. Attack back on if present (also without
/// `EVOKING`). The timers `0x21A`/`0x31A` stay (no `ClearEventGroup`, C-260); the
/// effects are ended by the host, `+0x74` is not in the state. Returns `true`.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    let evoking = sim
        .object(id)
        .is_some_and(|o| o.status & status::EVOKING != 0);
    if evoking && let Some((g, extra)) = evoke_ghost(sim, id).map(|e| (e.ghost, e.extra_energy)) {
        if sim.object(g).is_some() {
            send(sim, g, &Message::new(ghost::msg::FADE, &[]));
        }
        release_mana(sim, id, extra);
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(status::EVOKING, false);
        }
    }
    resume_attack(sim, id);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ability_testkit as tk;

    #[test]
    fn init_sets_the_radius_and_the_anim() {
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let evoker = tk::spawn(&mut sim, tk::GHOST_EVOKER, 0, (10, 10));
        let e = evoke_ghost(&sim, evoker).expect("module 26");
        assert_eq!(e.radius, 40_960);
        assert_eq!(e.anim, 0);
        assert_eq!((e.dead_body, e.ghost), (ObjectId::NONE, ObjectId::NONE));
        assert_eq!(
            (e.dead_body_pos, e.extra_energy, e.spell_guid),
            ([0; 3], 0, 0)
        );
        assert_eq!(sim.module(evoker, INDEX).unwrap().base.flags, 0);
    }
}
