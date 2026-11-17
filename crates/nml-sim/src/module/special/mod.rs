//! Special attacks (re/special-attacks): demolition expert (module 36), killer (module 37) and
//! whirlwind/damage building (module 49). The shared steps (cost of the order,
//! animation lock with attack tick) are here; states, `Init` and behavior are in the submodules.

use crate::id::ObjectId;
use crate::message::Message;
use crate::module::spell::{MANA, button_cost};
use crate::object::{GameObject, status};
use crate::sim::Sim;

pub mod bomberman;
pub mod killer;
pub mod special_move;
#[cfg(test)]
mod tests;

/// Attack module: out of the tick during placing, knife, sniping, whirlwind and hammering
/// (re/special-attacks 4.3).
pub(crate) const ATTACK: usize = 0x11;
/// Group of the timers of the flows (the abort clears them, re/special-attacks 3.9).
pub(crate) const GROUP: u8 = 0x80;
/// Walk `PMSG_PTTOPT_FINDTO` (re/walk 2.3): argc 3 `{object, tolerance, flags}` (demolition expert at the
/// building, smith), argc 6 `{x, z, goal_radius, target_direction, tolerance, flags}` (demolition expert at the
/// ship), argc 7 `{x, z, goal_radius, look_x, look_z, tolerance, flags}` (sniping).
pub(crate) const FINDTO: u32 = 0x200;
/// Walk `PMSG_PTTOPT_FINDTO_UNIT` (`[target, min, max, offset, tolerance, flags, x, z]`, re/walk 2.4):
/// killer at the victim, headhunter at the target (re/headhunter-robbing 3.3).
pub(crate) const FINDTO_UNIT: u32 = 0x400;

/// Timer `msg` of the module `index` after `t` ms (`Module_PostDelayedMessage`, group `0x80`,
/// without repeat).
pub(crate) fn post(sim: &mut Sim, id: ObjectId, index: usize, t: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, index) {
        m.base.post_delayed_message(t, msg, false, GROUP);
    }
}

/// Cost of an order `{target, button}` (re/special-attacks 3.1, 3.4): config 15 of the button in the
/// value set of the owner (`GameWorld_GetTypeValueSet`); if the set is missing, it costs 0 without a check.
/// Returns: `m_ExtraEnergy` and whether the current mana is enough (cost <= mana).
pub(crate) fn order_cost(sim: &Sim, id: ObjectId, button: i32) -> (i32, bool) {
    let Some(owner) = sim.object(id).map(|o| o.owner()) else {
        return (0, false);
    };
    let cost = button_cost(sim, owner, button);
    let mana = sim.attribute(id, MANA);
    (cost.unwrap_or(0), cost.is_none_or(|c| c <= mana))
}

/// Start of a flow: `ANIM_LOCKED` on, attack module out of the tick via the pending list
/// (`GameObject_QueueModuleTickChange(0x11, 0)`, without `HasModule` check, re/special-attacks 4.3).
pub(crate) fn lock_animation(o: &mut GameObject) {
    o.set_status_flags(status::ANIM_LOCKED, true);
    o.queue_module_tick_change(ATTACK, false);
}

/// End of a flow: `ANIM_LOCKED` off, attack module back into the tick, if present (also
/// switched off: `GameObject_HasModule(0x11, 1)`, re/special-attacks 3.9, 4.3).
pub(crate) fn release_animation(o: &mut GameObject) {
    o.set_status_flags(status::ANIM_LOCKED, false);
    if o.has_module(ATTACK, true) {
        o.queue_module_tick_change(ATTACK, true);
    }
}
