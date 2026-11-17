//! Module 27 `CPropertyGhost` (`PropertyGhost_Construct` `0x00515AB0`, vtable `0x00623F90`,
//! re/ghosts-banish 1.4): state of the ghost (calm, appearing, fading out, suck strike due), the
//! fade-out alpha, the suck target and the appear animation.
//!
//! `m_FadePos`/`m_FadeStep` are never read, the effect names and IDs are presentation; both
//! are absent from the state. `alpha` starts at 1.0 instead of uninitialized - it is not read until after
//! `0x21B`, which sets it to 1.0 (deviation from the original). The alpha is the only `f32` value in the
//! simulation.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::combat::damage::damage_message;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{send, send_fault};
use crate::module::events::update_events;
use crate::module::gather::set_animation;
use crate::module::{ModuleKind, init_flags};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::queue_destroy;

/// Module index Ghost.
pub const INDEX: usize = 0x1B;
/// Alpha deduction per tick while fading out (`0x3CF5C28F`, `0x005161E8`).
pub const ALPHA_STEP: f32 = 0.03;
/// Suck damage to the target and to the ghost per strike (`0x005160F3`, `0x0051610B`).
pub const SUCK_DAMAGE: i32 = 5;
/// Timer group of `0x41B` (`Module_PostDelayedMessage(..., 0x80)`, `0x00515D80`).
const TIMER_GROUP: u8 = 0x80;
/// Hit points (attribute 14).
const HP: u32 = 14;
/// Building module (`GameObject_HasModule(3, 0)` in `0x00515C00`).
const BUILDING: usize = 3;
/// Dying (`PMSG_DIE`, dying module) or `PMSG_ATTACK_DONE` to the own attack.
const DIE: u32 = 0x110;
const ATTACK_DONE: u32 = 0x511;
/// `m_AppearEffectName` (`+0x88`) and `m_DisappearEffectName` (`+0x8C`).
pub const APPEAR_EFFECT: &str = "data/effects/Geist_erscheinen.pef";
pub const DISAPPEAR_EFFECT: &str = "data/effects/Geist_verschwinden.pef";

/// Values of `m_Status` (`+0x78`, re/ghosts-banish 1.4).
pub mod state {
    /// Calm.
    pub const IDLE: u8 = 0;
    /// Appearing (until `0x41B`).
    pub const APPEARING: u8 = 1;
    /// Fading out (`0x21B`).
    pub const FADING: u8 = 2;
    /// Suck strike due (`0x31B`).
    pub const SUCKING: u8 = 3;
}

/// Messages to module 27 (query `0x00516020`, re/ghosts-banish 2.2).
pub mod msg {
    /// Appearing (from module 26 after creation, `0x00515D80`).
    pub const APPEAR: u32 = 0x11B;
    /// Fade out (dying module, banishment).
    pub const FADE: u32 = 0x21B;
    /// `{target}`: suck energy (FightFar `StartShot`, `0x00515C00`).
    pub const SUCK: u32 = 0x31B;
    /// Appearing finished (own timer).
    pub const APPEARED: u32 = 0x41B;
    /// End sucking (FightFar).
    pub const STOP_SUCK: u32 = 0x71B;
}

/// `CPropertyGhost`, fields of the savegame (serialization `0x00516280`, re/ghosts-banish 1.4, 5,
/// 10.1). Without `Eq` because of the `f32` fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ghost {
    /// `m_Status` (`+0x78`): 0 calm, 1 appearing, 2 fading out, 3 suck strike due.
    pub status: u8,
    /// `m_Alpha` (`+0x80`): opacity while fading out.
    pub alpha: f32,
    /// `m_AlphaStep` (`+0x7C`): deduction per tick.
    pub alpha_step: f32,
    /// `m_TargetHID` (`+0x84`): suck target; may be stale.
    pub target: ObjectId,
    /// `m_AppearAnim` (`+0xA4`, 280001): animation **and** delay of `0x41B` in ms (C-262).
    pub appear_anim: i32,
}

// Constructor `0x00515AB0`: state 0, target -1; alpha 1.0 (deviation from the original), step like `Init`.
impl Default for Ghost {
    fn default() -> Self {
        Self {
            status: 0,
            alpha: 1.0,
            alpha_step: ALPHA_STEP,
            target: ObjectId::NONE,
            appear_anim: 0,
        }
    }
}

pub fn ghost<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Ghost> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Ghost(g) => Some(g),
        _ => None,
    }
}

pub fn ghost_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Ghost> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Ghost(g) => Some(g),
        _ => None,
    }
}

/// `PropertyGhost_Init` (`0x005161D0`, slot 9, re/ghosts-banish 1.4): alpha step 0.03 and
/// appear animation 280001; the flags are 0 (no data value).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let appear_anim = desc.value_int(280_001).unwrap_or(0) as i32;
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Ghost(g) = &mut m.kind {
        g.alpha_step = ALPHA_STEP;
        g.appear_anim = appear_anim;
    }
}

/// `PropertyGhost_Query` (`0x00516020`, slot 7, re/ghosts-banish 2.2): `0x11B` appearing, `0x21B`
/// fading out, `0x41B` appearing finished, `0x71B` end sucking (all 1); otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, message: &mut Message) -> i32 {
    match message.code {
        msg::APPEAR => appear(sim, id),
        msg::FADE => start_fade_out(sim, id),
        msg::APPEARED => appeared(sim, id),
        msg::STOP_SUCK => stop_suck(sim, id),
        _ => return -1,
    }
    1
}

/// `PropertyGhost_OnMessage` (`0x00515FE0`, slot 12, re/ghosts-banish 2.2): **first** take over success and
/// fault from every envelope (C-266), then `Ghost_StartSuck` only for `0x31B`; otherwise
/// `false`.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_success(env.success.as_ref(), env.success_target);
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
    }
    if env.msg.code != msg::SUCK {
        return false;
    }
    start_suck(sim, id, ObjectId::from_i32(env.msg.arg(0)))
}

/// `PropertyGhost_Tick` (`0x00516190`, slot 13, re/ghosts-banish 3.4): state 2 fades out, state
/// 3 performs the suck strike; then the timers.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    match ghost(sim, id).map(|g| g.status) {
        Some(state::FADING) => fade_step(sim, id),
        Some(state::SUCKING) => suck(sim, id),
        _ => {}
    }
    update_events(sim, id, INDEX, dt);
}

/// `Ghost_Appear` (`0x00515D80`, `0x11B`): animation `m_AppearAnim`, lock bit `0x800000` on,
/// clear timer group `0x80`, state 1, effect "appear" at the world position, `0x41B` after
/// `m_AppearAnim` ms (C-262: the same value is animation and delay). Only `0x41B` changes the
/// selectability.
fn appear(sim: &mut Sim, id: ObjectId) {
    let Some(anim) = ghost(sim, id).map(|g| g.appear_anim) else {
        return;
    };
    set_animation(sim, id, anim);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(TIMER_GROUP);
    }
    if let Some(g) = ghost_mut(sim, id) {
        g.status = state::APPEARING;
    }
    sim.state.events.push(SimEvent::EffectOnObject {
        object: id,
        path: APPEAR_EFFECT.into(),
    });
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(anim, Message::new(msg::APPEARED, &[]), false, TIMER_GROUP);
    }
}

/// `0x005159C0` (`0x41B`): state 0, selectable, lock bit off - also after `0x21B` (C-265).
fn appeared(sim: &mut Sim, id: ObjectId) {
    if let Some(g) = ghost_mut(sim, id) {
        g.status = state::IDLE;
    }
    if let Some(o) = sim.object_mut(id) {
        o.selectable = true;
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
}

/// `Ghost_StopSuck` (`0x00515A60`, `0x71B`): no suck target, state 0 (also interrupts the fade-out
/// of a living ghost, C-265). The host derives the beam from `target`.
fn stop_suck(sim: &mut Sim, id: ObjectId) {
    if let Some(g) = ghost_mut(sim, id) {
        g.target = ObjectId::NONE;
        g.status = state::IDLE;
    }
}

/// `Ghost_StartFadeOut` (`0x00515EC0`, `0x21B`, re/ghosts-banish 3.4): lock bit off, not selectable,
/// state 2, alpha 1.0, effect "disappear" at the world position. The end of the appear effect in
/// state 1 is presentation (host); the suction beam stays (only `0x71B` ends it).
fn start_fade_out(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
        o.selectable = false;
    }
    if let Some(g) = ghost_mut(sim, id) {
        g.status = state::FADING;
        g.alpha = 1.0;
    }
    sim.state.events.push(SimEvent::EffectOnObject {
        object: id,
        path: DISAPPEAR_EFFECT.into(),
    });
}

/// `0x005159F0` (tick in state 2, re/ghosts-banish 3.4, 9.2): `alpha - alpha_step` in `f32` (rounded
/// per step); at `<= 0` alpha 0 and state 0, **then** `0x110` and
/// `QueueObjectDestroy`. Afterwards nothing more is written: for a living ghost `0x110` sends
/// `0x21B` again via the ghost branch (alpha 1.0, state 2) - it is fully visible again
/// until removal (C-264). `set_alpha` (`0x00468090`) is presentation (the host reads `alpha`).
fn fade_step(sim: &mut Sim, id: ObjectId) {
    let Some(g) = ghost_mut(sim, id) else { return };
    g.alpha -= g.alpha_step;
    if g.alpha <= 0.0 {
        g.alpha = 0.0;
        g.status = state::IDLE;
        send(sim, id, &Message::new(DIE, &[]));
        queue_destroy(sim, id);
    }
}

/// `Ghost_StartSuck` (`0x00515C00`, re/ghosts-banish 3.5, C-263): own HP < 1 -> `false`; target
/// missing (`GetObjectById(target, 0)`, dying ones count) -> `false`; both without a message. Building ->
/// fault message (`0x411` to attack), `false`. Otherwise remember the target, state 3, `true` - also for
/// a dead target that is still present; the host derives the beam.
fn start_suck(sim: &mut Sim, id: ObjectId, target: ObjectId) -> bool {
    if sim.attribute(id, HP) < 1 {
        return false;
    }
    let Some(t) = sim.object(target) else {
        return false;
    };
    if t.has_module(BUILDING, false) {
        send_fault(sim, id, INDEX);
        return false;
    }
    if let Some(g) = ghost_mut(sim, id) {
        g.target = target;
        g.status = state::SUCKING;
    }
    true
}

/// `0x00516080` (tick in state 3, re/ghosts-banish 3.5, 9.3): state 0; target present and HP > 0
/// -> `DamageMsg(ghost, 0x14)` with -5 via the damage path (`0x17F`) to the target, -5 HP at the ghost
/// directly, at own HP <= 1 end sucking and fade out. Always `0x511` to the own attack.
fn suck(sim: &mut Sim, id: ObjectId) {
    let Some(g) = ghost_mut(sim, id) else { return };
    g.status = state::IDLE;
    let target = g.target;
    if sim.object(target).is_some() && sim.attribute(target, HP) > 0 {
        let dm = damage_message(sim, id, -SUCK_DAMAGE);
        send(sim, target, &dm);
        if let Some((slot, guid)) = sim.object(id).map(|o| (o.owner(), o.guid)) {
            let p = sim.type_upgrade_percent(slot, guid, HP);
            if let Some(o) = sim.object_mut(id) {
                o.attributes.add_to_current_value(HP, -SUCK_DAMAGE, p);
            }
        }
        if sim.attribute(id, HP) <= 1 {
            stop_suck(sim, id);
            start_fade_out(sim, id);
        }
    }
    send(sim, id, &Message::new(ATTACK_DONE, &[]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ability_testkit as tk;

    #[test]
    fn init_reads_the_appear_anim() {
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = tk::spawn(&mut sim, tk::GHOST, 0, (10, 10));
        let ghost = ghost(&sim, g).expect("module 27");
        assert_eq!(ghost.appear_anim, 0);
        assert_eq!((ghost.alpha, ghost.alpha_step), (1.0, 0.03));
        assert_eq!((ghost.status, ghost.target), (0, ObjectId::NONE));
        assert_eq!(sim.module(g, INDEX).unwrap().base.flags, 0);
    }
}
