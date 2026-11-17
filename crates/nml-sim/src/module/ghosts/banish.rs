//! Module 31 `CPropertyBanish` (`PropertyBanish_Construct` `0x00502BB0`, vtable `0x006225E0`,
//! re/ghosts-banish 1.3): target filter, banish animation, node prefix of the banishing spell effects and
//! button of the last `0x11F`.
//!
//! The effect ID list `m_EffectID` and the fixed effect name are presentation: the host shows
//! the banishing spell as long as a timer `0x41F` runs (deviation from the original). `Init` loads
//! the flags (320003), they are never evaluated (re/ghosts-banish 8).

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::collision::query_radius;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::object::status;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};

use crate::module::gather::{anim_label_time, set_animation};
use crate::module::spell::{MANA, button_cost, mana_percent};
use crate::module::{ModuleKind, attack, init_flags};

/// Module index Banish.
pub const INDEX: usize = 0x1F;
/// Delay `0x11F` -> `0x21F` (banishment takes effect) in ms (`0x00503074`).
pub const DELAY_MS: i32 = 1500;
/// Radius effect of the banishing (fixed, `0x00622640`; the original does not read 320006).
pub const RADIUS_EFFECT: &str = "data/effects/radius_bannen.hef";
/// Banishing spell effect at the nodes (`m_EffectName`, fixed).
pub const SPELL_EFFECT: &str = "data/effects/bannspruch.pef";

/// Timer group of `0x21F`/`0x31F`/`0x41F` (`Module_PostDelayedMessage`, `0x00503069`).
const TIMER_GROUP: u8 = 0x80;
/// Sight (attribute 18, `MAX_ONLY`): banish radius = sight * 4096 >> 3 (re/ghosts-banish 8 no. 11).
const SIGHT: u32 = 18;
/// Sound "banishing begins" (`0x00502FF4`) or "banishment hits" (`0x00502CA0`) at the banisher (host).
const SOUND_START: i32 = 0x1E;
const SOUND_START_FLAGS: u32 = 0x43;
const SOUND_HIT: i32 = 0x1F;
const SOUND_HIT_FLAGS: u32 = 0x42;
/// Riding `{4}` (`0x50D`, `PropertyRiding_GetAnim`): banish animation in the saddle (list 140010).
const RIDING_ANIM: u32 = 0x50D;
const RIDING_BANISH: i32 = 4;
/// `0x27F {pct, attacker}` with -100 % of the max HP (re/module-runtime 4).
const ADD_HP_PERCENT: u32 = 0x27F;
const BAN_PERCENT: i32 = -100;

/// Messages to module 31 (query `0x00503470`, re/ghosts-banish 2.3).
pub mod msg {
    /// `{button}`: start banishing (command `0x25`, `0x00502F40`).
    pub const START: u32 = 0x11F;
    /// Banishment takes effect (own timer, `0x00502CA0`).
    pub const NOW: u32 = 0x21F;
    /// Banish animation finished (own timer).
    pub const END: u32 = 0x31F;
    /// Remove the banishing spell effects (own timer, marker 1 - 1000 ms).
    pub const EFFECTS_OFF: u32 = 0x41F;
}

/// `CPropertyBanish`, fields of the savegame (serialization `0x005034D0`, re/ghosts-banish 1.3, 5,
/// 10.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banish {
    /// `m_MMTarget` (`+0x70`, 320001): `BUTTONMSG` of the target filter in `targets.ini` (shipped data 440).
    pub mm_target: i32,
    /// `m_BanishAnim` (`+0x74`, 320004): banish animation.
    pub anim: i32,
    /// `m_DummyName` (`+0x98`, 320005): node prefix of the banishing spell effects (`%s%02d`).
    pub dummy: String,
    /// `m_SpellGuid` (`+0x9C`): button GUID of the last `0x11F`.
    pub spell_guid: i32,
}

pub fn banish<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Banish> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Banish(b) => Some(b),
        _ => None,
    }
}

pub fn banish_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Banish> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Banish(b) => Some(b),
        _ => None,
    }
}

/// `PropertyBanish_Init` (`0x00502E80`, slot 9, re/ghosts-banish 1.3): flags 320003, target filter
/// 320001, animation 320004, node prefix 320005.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let (mm_target, anim) = (value(320_001), value(320_004));
    let dummy = desc.values.get(&320_005).cloned().unwrap_or_default();
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Banish(b) = &mut m.kind {
        b.mm_target = mm_target;
        b.anim = anim;
        b.dummy = dummy;
    }
}

/// `PropertyBanish_Query` (`0x00503470`, slot 7, re/ghosts-banish 2.3): `0x11F {button}` starts,
/// `0x21F` banishes (both 0), `0x31F` ends the animation, `0x41F` removes the banishing spell effects
/// (both 1); otherwise -1. A switched-off module receives no message (`send`).
pub fn query(sim: &mut Sim, id: ObjectId, message: &mut Message) -> i32 {
    match message.code {
        msg::START => {
            start(sim, id, message.arg(0));
            0
        }
        msg::NOW => {
            banish_now(sim, id);
            0
        }
        msg::END => {
            unlock(sim, id);
            1
        }
        // `Banish_RemoveEffects` (`0x00503370`): effects only; the host derives them from the
        // timer `0x41F` (deviation from the original).
        msg::EFFECTS_OFF => 1,
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

/// Attack on, if present - also switched off (`GameObject_HasModule(0x11, 1)`), then
/// `ANIM_LOCKED` off (`0x31F` `0x00502B80`, abort `0x00503420`).
fn unlock(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id) {
        if o.has_module(attack::INDEX, true) {
            o.queue_module_tick_change(attack::INDEX, true);
        }
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
}

/// `Banish_Start` (`0x00502F40`, re/ghosts-banish 3.6): with `ANIM_LOCKED` set (banishing,
/// healing trance, ghost appearing, script lock; C-270, C-254) nothing. Otherwise remember the button; cost
/// = config 15 of its value set (without a set or at 0 free), too little mana -> nothing, otherwise
/// paid **immediately** - also without a ghost in the radius (C-268). Sound, `ANIM_LOCKED` on, attack off
/// (without module check), banish animation (when mounted the answer of riding `0x50D {4}`, without
/// `RIDINGBANISH` check, C-269), `0x21F` after 1,500 ms, `0x31F` after marker 1 and `0x41F`
/// 1,000 ms before it (all group `0x80`). The host derives the radius effect and banishing spell at the
/// nodes `dummy01...` from the timer `0x41F` (deviation from the original).
fn start(sim: &mut Sim, id: ObjectId, button: i32) {
    let Some(o) = sim.object(id) else { return };
    if o.status & status::ANIM_LOCKED != 0 {
        return;
    }
    let (slot, pos) = (o.owner(), o.world_pos);
    let Some(b) = banish_mut(sim, id) else { return };
    b.spell_guid = button;
    let anim = b.anim;
    if let Some(c) = button_cost(sim, slot, button)
        && c != 0
    {
        let p = mana_percent(sim, id);
        let Some(o) = sim.object_mut(id) else { return };
        if o.attributes.current_value(MANA, p) < c {
            return;
        }
        o.attributes.add_to_current_value(MANA, c.wrapping_neg(), p);
    }
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: SOUND_START,
        flags: SOUND_START_FLAGS,
        pos,
        guid: button as u32,
    });
    let mounted = sim.object_mut(id).is_some_and(|o| {
        o.set_status_flags(status::ANIM_LOCKED, true);
        o.queue_module_tick_change(attack::INDEX, false);
        o.status & status::ISMOUNTED != 0
    });
    let anim = if mounted {
        send(sim, id, &Message::new(RIDING_ANIM, &[RIDING_BANISH]))
    } else {
        anim
    };
    set_animation(sim, id, anim);
    post_delayed(sim, id, DELAY_MS, msg::NOW);
    let t = anim_label_time(sim, id, anim, 1);
    post_delayed(sim, id, t, msg::END);
    post_delayed(sim, id, t.wrapping_sub(1000), msg::EFFECTS_OFF);
}

/// Banish radius of `id` in 1/4096 tile as in `Banish_Execute` (`0x00502CA0`, re/ghosts-banish 3.6):
/// **current** sight (attribute 18, with the upgrade percentages of the owner) * 4096 >> 3,
/// i.e. half the sight range (C-267); 0 without an object. Shared by the banishment and `sim_query`.
pub fn banish_radius(state: &SimState, id: ObjectId) -> i32 {
    state.object(id, false).map_or(0, |o| {
        let p = crate::upgrades::type_upgrade_percent(state, o.owner(), o.guid, SIGHT);
        o.attributes.current_value(SIGHT, p).wrapping_mul(4096) >> 3
    })
}

/// `Banish_Execute` (`0x00502CA0`, re/ghosts-banish 3.6): TARGET filter of the entry `mm_target`
/// (shipped data 440: hostile, seen in the fog, module 27) with the owner as fog player; radius
/// [`banish_radius`] at the time of the banishment (C-267). All hits at center distance get `0x27F {-100,
/// banisher}` (HP 0, experience for the banisher, death at the next `check_death`), the sound once
/// beforehand. Without an entry or hit nothing.
fn banish_now(sim: &mut Sim, id: ObjectId) {
    let Some(mm) = banish(sim, id).map(|b| b.mm_target) else {
        return;
    };
    let Some(mut f) = sim.rules.target_entry(mm).map(|e| e.target.clone()) else {
        return;
    };
    let Some(o) = sim.object(id) else { return };
    let (slot, grid, pos) = (i32::from(o.owner()), o.grid, o.world_pos);
    f.fog_player = slot;
    let r = banish_radius(sim.state, id);
    let hits = query_radius(sim, (grid.x.0, grid.z.0), &f, r, slot, false);
    if hits.is_empty() {
        return;
    }
    let guid = banish(sim, id).map_or(0, |b| b.spell_guid);
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: SOUND_HIT,
        flags: SOUND_HIT_FLAGS,
        pos,
        guid: guid as u32,
    });
    for h in hits {
        send(
            sim,
            h,
            &Message::new(ADD_HP_PERCENT, &[BAN_PERCENT, id.as_i32()]),
        );
    }
}

/// `PropertyBanish_Abort` (`0x00503420`, slot 11, re/ghosts-banish 2.3): attack on (if
/// present), `ANIM_LOCKED` off - even if a trance or script lock set it (C-254) -,
/// `ClearEventGroup(0x80)`: the pending `0x21F` is dropped, the mana stays spent (C-268).
/// The host ends the effects with the timer `0x41F`. Returns `true`.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    unlock(sim, id);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(TIMER_GROUP);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ability_testkit as tk;

    #[test]
    fn init_reads_target_anim_and_node() {
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let evoker = tk::spawn(&mut sim, tk::GHOST_EVOKER, 0, (10, 10));
        assert_eq!(
            banish(&sim, evoker),
            Some(&Banish {
                mm_target: 440,
                anim: 10,
                dummy: "geisterhand".into(),
                spell_guid: 0,
            })
        );
        assert_eq!(sim.module(evoker, INDEX).unwrap().base.flags, 0x500);
        // Missionary: animation 6, node `bannspruch`, flags `0xD00`.
        let missionary = tk::spawn(&mut sim, tk::MISSIONARY, 0, (14, 10));
        let b = banish(&sim, missionary).expect("module 31");
        assert_eq!(
            (b.mm_target, b.anim, b.dummy.as_str()),
            (440, 6, "bannspruch")
        );
        assert_eq!(sim.module(missionary, INDEX).unwrap().base.flags, 0xD00);
    }
}
