//! Module 25 `CPropertyMasking` (re/masking): the camouflage state (camouflage animations,
//! camouflage time, camouflage and busy flags, `EVERYTIMETARNED`), `Init` (3.1), the queries `0x119` ...
//! `0x619` (2.1, 3.2, 3.3) and the tick with the energy state machine and the enemy scouting (3.4).
//!
//! The slot of the local player (`m_DummyAktPlayer` `+0x88`, K-13) is missing on purpose: it only selects
//! animation and opacity and is presentation of the host (re/masking 10.4). Therefore the
//! simulation always sets `anim[0]` on the building (alarm post), and the switch block of the tick is omitted.
//! Opacity and `NotifyUiIfSelected` are likewise host (the viewer reads the status).

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::fog::FogView;
use crate::id::ObjectId;
use crate::message::Message;
use crate::object::status;
use crate::sim::Sim;

use super::events::update_events;
use super::gather::{anim_label_time, list_at, set_animation};
use super::stand::{list_modulo, parse_int_list};
use super::{ModuleKind, init_flags};

pub const INDEX: usize = 25;
/// `MID_PROP_MASKING_EVERYTIMETARNED` (260002 = `0x100`): always camouflaged (re/masking 0, 3.4).
pub const EVERYTIMETARNED: u32 = 0x100;
/// Shortest camouflage time in ms (`m_MaskTime = max(label, 1000)`, re/masking 3.1).
pub const MIN_MASK_TIME: i32 = 1000;
/// Attribute "extra energy" (config 15), energy of camouflaging.
const ENERGY: u32 = 15;
/// Label 1 of an animation (end of the camouflage animation).
const LABEL_END: i32 = 1;
/// Timer group of `0x319`/`0x419` (re/masking 3.10).
const TIMER_GROUP: u8 = 0x80;
/// `0x0051D850`: mounted, in a boat, swimming, diving - then `0x119` does not camouflage (`0x3A000`).
pub const BLOCKING_STATUS: u32 =
    status::ISMOUNTED | status::ISMOUNTEDONBOAT | status::SWIMMING | status::DIVING;
/// Consumption per tick in the camouflaged state (fraction, without `EVERYTIMETARNED`).
const DRAIN: i32 = 55;
/// Recharge per tick (fraction).
const RECHARGE: i32 = 83;
/// Deduction per tick on a tile that an enemy of the owner scouts (C-233).
const CLEAR_UP_DRAIN: i32 = 2000;
/// `EVERYTIMETARNED` camouflages only above 10 % energy (fraction > 10,000).
const AUTO_MASK_ABOVE: i32 = 10_000;
/// Building module (`HasModule(3, 0)`): alarm posts camouflage without animation.
const BUILDING: usize = 3;

/// Messages of the camouflage module (query `0x0051DB30`, re/masking 2.1).
pub mod msg {
    /// `PMSG_MASKING_STARTMASKING`: begin camouflaging (command `0x1D`).
    pub const START: u32 = 0x119;
    /// `PMSG_MASKING_ENDMASKING`: begin uncamouflaging (command `0x1E`).
    pub const END: u32 = 0x219;
    /// `PMSG_MASKING_MASKING_FINISHED`: camouflage animation finished (own timer).
    pub const FINISHED: u32 = 0x319;
    /// `PMSG_MASKING_DEMASKING_FINISHED`: uncamouflage animation finished (own timer).
    pub const DEMASK_FINISHED: u32 = 0x419;
    /// `PMSG_MASKING_SHOWEFFEKT`: has no effect.
    pub const SHOW_EFFECT: u32 = 0x519;
    /// `PMSG_MASKING_DEMASK_IMMEDIATELY`: uncamouflage at once (swimming, mounting).
    pub const DEMASK_NOW: u32 = 0x619;
}

/// `CPropertyMasking` (`0x8C` bytes); savegame in the order `m_MaskAnim`, `m_IsMasked`,
/// `m_MaskTime`, `m_IsBusy`, `m_NeverUnmasked` (re/masking 1.2, 5). The flags (260002) are in the
/// [`ModuleBase`](super::base::ModuleBase).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Masking {
    /// `m_MaskAnim` (`+0x70`, 260001): `[0]` camouflage, `[1 % n]` uncamouflage animation.
    pub anim: Vec<i32>,
    /// `m_IsMasked` (`+0x7C`): camouflage process **begun** (already before `MASKED`, not just once it is set).
    pub is_masked: bool,
    /// `m_MaskTime` (`+0x80`): `max(label 1 of anim[0], 1000)` ms, for camouflaging **and** uncamouflaging.
    pub mask_time: i32,
    /// `m_IsBusy` (`+0x84`): the animation is running (`0x319`/`0x419` is pending).
    pub is_busy: bool,
    /// `m_NeverUnmasked` (`+0x85`): `EVERYTIMETARNED` (`Module_HasFlags(0x119)`).
    pub never_unmasked: bool,
}

pub fn masking<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Masking> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Masking(m) => Some(m),
        _ => None,
    }
}

pub fn masking_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Masking> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Masking(m) => Some(m),
        _ => None,
    }
}

/// `PropertyMasking_Init` (`0x0051DBA0`, re/masking 3.1): flags (260002), camouflage animations (260001),
/// camouflage time from label 1 of the camouflage animation (at least 1000 ms); with `EVERYTIMETARNED`
/// (`Module_HasFlags(0x119)`, only `0x100` counts, K-305) the energy is full. The general
/// regeneration of the extra energy stays off afterwards, the module recharges itself - even for a
/// disabled variant, for which `Init` runs anyway.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let text = desc.values.get(&260_001).map_or("", String::as_str);
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = init_flags(INDEX, desc);
    let never_unmasked = m.base.has_flags(EVERYTIMETARNED | INDEX as u32);
    let anim = parse_int_list(text, &[]);
    let mask_time = anim_label_time(sim, id, list_at(&anim, 0), LABEL_END).max(MIN_MASK_TIME);
    if let Some(ModuleKind::Masking(mk)) = sim.module_mut(id, INDEX).map(|m| &mut m.kind) {
        mk.anim = anim;
        mk.mask_time = mask_time;
        mk.is_masked = false;
        mk.is_busy = false;
        mk.never_unmasked = never_unmasked;
    }
    if let Some(o) = sim.object_mut(id) {
        if never_unmasked {
            o.attributes.fill_current(ENERGY);
        }
        o.attributes.set_regen_rate(ENERGY, 0);
    }
}

/// `PropertyMasking_Query` (`0x0051DB30`, re/masking 2.1): `0x119` camouflage, `0x219` uncamouflage, `0x319`
/// and `0x419` end of the animation, `0x519` no effect (all 0), `0x619` uncamouflage at once (1),
/// otherwise -1. A disabled module receives no message (`send` returns -1, K-307).
pub fn query(sim: &mut Sim, id: ObjectId, message: &mut Message) -> i32 {
    if masking(sim, id).is_none() {
        return -1;
    }
    match message.code {
        msg::START => start_masking(sim, id),
        msg::END => end_masking(sim, id),
        msg::FINISHED => finish(sim, id, true),
        msg::DEMASK_FINISHED => finish(sim, id, false),
        msg::SHOW_EFFECT => 0,
        msg::DEMASK_NOW => demask_immediately(sim, id),
        _ => -1,
    }
}

/// Building (alarm post)? `HasModule(3, 0)`.
fn is_building(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.has_module(BUILDING, false))
}

/// `PropertyMasking_StartMasking` (`0x0051D890`, re/masking 3.2). Building: `MASKED` at once with
/// `anim[0]` (restart, passed on at once; instead of the choice by the local player, D-235),
/// `m_IsBusy` off. Unit: nothing if the process is already running (`m_IsMasked`) or it rides, sits in a
/// boat, swims or dives; otherwise camouflage animation `anim[0]`, `0x319` after `m_MaskTime`
/// (group `0x80`) and the lock bit `0x800000` without lock time. The energy is not
/// checked (K-301). Return value 0.
fn start_masking(sim: &mut Sim, id: ObjectId) -> i32 {
    let building = is_building(sim, id);
    let Some(st) = sim.object(id).map(|o| o.status) else {
        return 0;
    };
    let Some(m) = masking(sim, id) else { return 0 };
    let (anim, is_masked, mask_time) = (list_modulo(&m.anim, 0), m.is_masked, m.mask_time);
    if building {
        if let Some(o) = sim.object_mut(id) {
            o.set_animation(anim, 0, true, true, false);
        }
        if let Some(m) = masking_mut(sim, id) {
            m.is_busy = false;
            m.is_masked = true;
        }
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(status::MASKED, true);
        }
        return 0;
    }
    if is_masked || st & BLOCKING_STATUS != 0 {
        return 0;
    }
    set_animation(sim, id, anim);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(
            mask_time,
            Message::new(msg::FINISHED, &[]),
            false,
            TIMER_GROUP,
        );
    }
    if let Some(m) = masking_mut(sim, id) {
        m.is_busy = true;
        m.is_masked = true;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    0
}

/// `PropertyMasking_EndMasking` (`0x0051D670`, re/masking 3.3). Building: `anim[0]` (restart, at once),
/// `MASKED` off, both flags off. Unit: only with `m_IsMasked` (also in the middle of the
/// camouflage animation, K-303) uncamouflage animation `anim[1 % n]`, `0x419` after `m_MaskTime` (K-304),
/// `m_IsMasked` off, `m_IsBusy` and `0x800000` on; `MASKED` stays until `0x419`. Return value 0.
fn end_masking(sim: &mut Sim, id: ObjectId) -> i32 {
    let building = is_building(sim, id);
    let Some(m) = masking(sim, id) else { return 0 };
    let (first, second) = (list_modulo(&m.anim, 0), list_modulo(&m.anim, 1));
    let (is_masked, mask_time) = (m.is_masked, m.mask_time);
    if building {
        if let Some(o) = sim.object_mut(id) {
            o.set_animation(first, 0, true, true, false);
            o.set_status_flags(status::MASKED, false);
        }
        if let Some(m) = masking_mut(sim, id) {
            m.is_masked = false;
            m.is_busy = false;
        }
        return 0;
    }
    if !is_masked {
        return 0;
    }
    set_animation(sim, id, second);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(
            mask_time,
            Message::new(msg::DEMASK_FINISHED, &[]),
            false,
            TIMER_GROUP,
        );
    }
    if let Some(m) = masking_mut(sim, id) {
        m.is_masked = false;
        m.is_busy = true;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    0
}

/// End of the animation: `0x319` (`0x0051D740`) sets `MASKED`, `0x419` (`0x0051D790`) clears it;
/// both clear `m_IsBusy` and `0x800000`. `m_IsMasked` stays untouched (K-302, K-303).
fn finish(sim: &mut Sim, id: ObjectId, masked: bool) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::MASKED, masked);
    }
    if let Some(m) = masking_mut(sim, id) {
        m.is_busy = false;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    0
}

/// `PropertyMasking_DemaskImmediately` (`0x0051D800`, `0x619` from swimming and mounting):
/// `MASKED` and `0x800000` off, both flags off. Pending `0x319`/`0x419` stay
/// (K-302): a `0x319` from a running camouflage animation camouflages again afterwards. Return value 1.
fn demask_immediately(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::MASKED, false);
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    if let Some(m) = masking_mut(sim, id) {
        m.is_masked = false;
        m.is_busy = false;
    }
    1
}

/// Tick (`0x0051D9A0`, slot 13, re/masking 3.4, 3.5): first the timers (`0x319`/`0x419`), then the
/// energy state machine by (`m_IsMasked`, `m_IsBusy`):
/// - camouflaged: -55 (without `EVERYTIMETARNED`); fraction 0 or maximum 0 (sheriff 4458, C-240) ->
///   uncamouflage directly (without `AbortAllModules`), otherwise with `EVERYTIMETARNED` +83;
/// - camouflage animation: +83 only with `EVERYTIMETARNED`;
/// - uncamouflage animation and visible: +83; visible with `EVERYTIMETARNED`, without lock status, fraction
///   > 10,000 and maximum > 0 -> camouflage directly.
///
/// Afterwards -2000 if an enemy of the owner scouts the own tile (`0x00466D60`, camouflaged or
/// not, C-233), and a negative fraction is clamped to 0. The switch block of the local
/// player is omitted.
///
/// A `0x319`/`0x419` that the tick creates itself (camouflaging or uncamouflaging directly) is pending after
/// `update_events`: it enters the list only in the next tick and falls at 1600 ms in tick
/// N + 18, not N + 17 as after a command (`events.rs`, step D).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    let Some(o) = sim.object(id) else { return };
    let pct = sim.type_upgrade_percent(o.owner(), o.guid, ENERGY);
    let Some(m) = masking(sim, id) else { return };
    let (is_masked, is_busy, never) = (m.is_masked, m.is_busy, m.never_unmasked);
    let add = |sim: &mut Sim, delta: i32| {
        if let Some(o) = sim.object_mut(id) {
            o.attributes.add_fraction(ENERGY, delta);
        }
    };
    // (fraction, maximum, status) for the checks of the state machine.
    let read = |sim: &Sim| {
        sim.object(id).map_or((0, 0, 0), |o| {
            (
                o.attributes.current_fraction(ENERGY),
                o.attributes.max_value(ENERGY, pct),
                o.status,
            )
        })
    };
    match (is_masked, is_busy) {
        (true, false) => {
            if !never {
                add(sim, -DRAIN);
            }
            let (fraction, max, _) = read(sim);
            if fraction > 0 && max > 0 {
                if never {
                    add(sim, RECHARGE);
                }
            } else {
                end_masking(sim, id);
            }
        }
        (true, true) => {
            if never {
                add(sim, RECHARGE);
            }
        }
        (false, true) => add(sim, RECHARGE),
        (false, false) => {
            add(sim, RECHARGE);
            let (fraction, max, st) = read(sim);
            if never && st & BLOCKING_STATUS == 0 && fraction > AUTO_MASK_ABOVE && max > 0 {
                start_masking(sim, id);
            }
        }
    }
    let Some(o) = sim.object(id) else { return };
    let (tile, owner) = (o.tile, i32::from(o.owner()));
    if FogView::new(&*sim.state).cleared_by_enemy_of(tile.0, tile.1, owner) {
        add(sim, -CLEAR_UP_DRAIN);
    }
    if let Some(o) = sim.object_mut(id)
        && o.attributes.current_fraction(ENERGY) <= 0
    {
        o.attributes.set_current_fraction(ENERGY, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attributes::FULL;
    use crate::command::{Command, CommandKind, NO_SPREAD};
    use crate::filter::matches_target;
    use crate::message::Message;
    use crate::messaging::{abort_all_modules, post_message, send};
    use crate::module::ability_testkit as tk;
    use crate::module::building::fort_testkit::move_to;
    use crate::module::riding::mount::do_mount;
    use crate::module::riding::riding_mut;
    use crate::object::status;
    use crate::rules::Rules;
    use crate::sim::Sim;
    use crate::state::SimState;
    use crate::world::change_owner;
    use opennml_formats::targets::MatchFilter;

    /// Module index riding.
    const RIDING: usize = 13;

    /// Type with module 25 only (animation 7) and the flag word `flags`.
    fn flag_type(guid: u32, flags: u32) -> String {
        format!(
            "GUID {guid}\nNAME TEST_Tarner\nVALUEID 100 6 1 33554432\nPROPID 25\n\
             VALUEID 260001 1 1 7\nVALUEID 260002 6 1 {flags}\n"
        )
    }

    // QUIRK K-305: kept
    #[test]
    fn init_reads_anim_time_and_everytimetarned() {
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // 1402 without upgrade 2924: module 25 is switched off, `Init` ran anyway (K-305).
        let archer = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        assert!(!sim.object(archer).unwrap().has_module(INDEX, false));
        let m = masking(&sim, archer).expect("module 25");
        assert_eq!(m.anim, [7]);
        assert_eq!(m.mask_time, 1600);
        assert!(!m.never_unmasked && !m.is_masked && !m.is_busy);
        let attrs = &sim.object(archer).unwrap().attributes;
        assert_eq!(attrs.regen_rate(15), 0, "general regeneration off");
        assert_eq!(attrs.current_fraction(15), FULL);
        // Ranger: `EVERYTIMETARNED` (260002 = 0x100), energy full, the module recharges itself.
        let ranger = tk::spawn(&mut sim, tk::RANGER, 0, (12, 10));
        let m = masking(&sim, ranger).unwrap();
        assert!(m.never_unmasked);
        assert_eq!((m.anim.as_slice(), m.mask_time), (&[9][..], 1600));
        let attrs = &sim.object(ranger).unwrap().attributes;
        assert_eq!(attrs.current_fraction(15), FULL);
        assert_eq!(attrs.regen_rate(15), 0);
        // Alarm post: label 33 ms, at least 1000 ms; animations `0 1`.
        let post = tk::spawn(&mut sim, tk::ALARM_POST, 0, (16, 10));
        let m = masking(&sim, post).unwrap();
        assert_eq!((m.anim.as_slice(), m.mask_time), (&[0, 1][..], 1000));
        assert!(m.never_unmasked);
    }

    // QUIRK K-305: kept
    #[test]
    fn only_the_high_bytes_of_the_flag_word_count() {
        // `Module_HasFlags(0x119)`: only bit `0x100` counts, not the low byte (re/masking 3.1, K-305).
        let types = tk::types() + &flag_type(9001, 0x19) + &flag_type(9002, 0x100);
        let rules = tk::rules_from(&types);
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let low = tk::spawn(&mut sim, 9001, 0, (10, 10));
        let high = tk::spawn(&mut sim, 9002, 0, (12, 10));
        assert!(!masking(&sim, low).unwrap().never_unmasked);
        assert!(masking(&sim, high).unwrap().never_unmasked);
    }

    /// Fraction of the extra energy (1/100,000 of the maximum).
    fn energy(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().attributes.current_fraction(ENERGY)
    }

    fn set_energy(sim: &mut Sim, id: ObjectId, fraction: i32) {
        sim.object_mut(id)
            .unwrap()
            .attributes
            .set_current_fraction(ENERGY, fraction);
    }

    /// (`m_IsMasked`, `m_IsBusy`).
    fn flags(sim: &Sim, id: ObjectId) -> (bool, bool) {
        let m = masking(sim, id).unwrap();
        (m.is_masked, m.is_busy)
    }

    fn has(sim: &Sim, id: ObjectId, bit: u32) -> bool {
        sim.object(id).unwrap().status & bit != 0
    }

    fn masked(sim: &Sim, id: ObjectId) -> bool {
        has(sim, id, status::MASKED)
    }

    fn say(sim: &mut Sim, id: ObjectId, code: u32) -> i32 {
        send(sim, id, &Message::new(code, &[]))
    }

    /// World with 2924 unlocked (archers arise with module 25 switched on).
    fn unlocked_state(rules: &Rules) -> SimState {
        let mut state = tk::state(rules);
        tk::unlock(&mut state, 0, tk::MASK_UPGRADE);
        state
    }

    /// Archer with 2924, camouflaged via `0x119` before tick N (`0x319` in tick N + 17, after 18
    /// ticks; re/masking 9 V1).
    fn masked_archer(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
        let a = tk::spawn(sim, tk::ARCHER, 0, tile);
        assert_eq!(say(sim, a, msg::START), 0);
        tk::run_ticks(sim, 18);
        assert!(masked(sim, a) && flags(sim, a) == (true, false));
        a
    }

    /// Ranger who has camouflaged himself (tick 1, `0x319` in tick 19: see
    /// [`a_ranger_masks_itself_and_never_drains`]).
    fn masked_ranger(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
        let r = tk::spawn(sim, tk::RANGER, 0, tile);
        tk::run_ticks(sim, 19);
        assert!(masked(sim, r) && flags(sim, r) == (true, false));
        r
    }

    /// `n` object ticks in which an enemy (slot 1) scouts the tile of `id`: the bit is set
    /// before each `step` in the foreground grid (otherwise the fog cycle copies the empty
    /// background grid over it).
    fn run_ticks_cleared_up(sim: &mut Sim, id: ObjectId, n: usize) {
        for _ in 0..2 * n {
            let (x, z) = sim.object(id).unwrap().tile;
            sim.state.fog.cleared.or(x, z, 1 << 1);
            crate::tick::step(sim.state, sim.rules, &[]);
        }
    }

    // QUIRK K-304: kept
    #[test]
    fn masking_follows_the_energy_automaton() {
        // re/masking 9 V1: 1402 with 2924, energy full, `0x119` in tick N.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        assert_eq!(energy(&sim, a), FULL);
        assert_eq!(say(&mut sim, a, msg::START), 0);
        tk::run_ticks(&mut sim, 1);
        assert_eq!(flags(&sim, a), (true, true));
        assert!(has(&sim, a, status::ANIM_LOCKED) && !masked(&sim, a));
        assert_eq!(
            sim.object(a).unwrap().anim.current,
            7,
            "camouflage animation 7"
        );
        // Camouflage animation: no consumption, `0x319` after 1600 ms -> tick N + 17 (timer rule).
        tk::run_ticks(&mut sim, 16);
        assert!(!masked(&sim, a));
        assert_eq!(energy(&sim, a), FULL);
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, a) && !has(&sim, a, status::ANIM_LOCKED));
        assert_eq!(flags(&sim, a), (true, false));
        assert_eq!(energy(&sim, a), 99_945, "already -55 in the 0x319 tick");
        // -55 per tick: after 1817 further ticks 10, in the next <= 0 -> uncamouflaging begins.
        tk::run_ticks(&mut sim, 1817);
        assert_eq!((energy(&sim, a), flags(&sim, a)), (10, (true, false)));
        tk::run_ticks(&mut sim, 1);
        assert_eq!(flags(&sim, a), (false, true));
        assert_eq!(energy(&sim, a), 0, "clamped");
        assert!(masked(&sim, a), "MASKED stays until 0x419");
        assert!(has(&sim, a, status::ANIM_LOCKED));
        // Uncamouflage animation (`m_MaskTime` here too, K-304), +83 per tick. The tick creates `0x419`
        // after its `update_events`: the timer enters the list only in the next tick
        // and counts from the one after, so `0x419` falls 18 ticks later (not 17).
        tk::run_ticks(&mut sim, 17);
        assert!(masked(&sim, a));
        assert_eq!(energy(&sim, a), 17 * 83);
        tk::run_ticks(&mut sim, 1);
        assert!(!masked(&sim, a) && !has(&sim, a, status::ANIM_LOCKED));
        assert_eq!(flags(&sim, a), (false, false));
        assert_eq!(energy(&sim, a), 18 * 83);
        tk::run_ticks(&mut sim, 1);
        assert_eq!(energy(&sim, a), 19 * 83);
    }

    #[test]
    fn a_ranger_masks_itself_and_never_drains() {
        // re/masking 9 V2: `EVERYTIMETARNED` camouflages itself in the first tick (animation 9). The tick
        // camouflages after its `update_events`: `0x319` falls in tick 1 + 18.
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let r = tk::spawn(&mut sim, tk::RANGER, 0, (10, 10));
        tk::run_ticks(&mut sim, 1);
        assert_eq!(flags(&sim, r), (true, true));
        assert_eq!(sim.object(r).unwrap().anim.current, 9);
        tk::run_ticks(&mut sim, 17);
        assert!(!masked(&sim, r));
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, r) && flags(&sim, r) == (true, false));
        tk::run_ticks(&mut sim, 200);
        assert!(masked(&sim, r));
        assert_eq!(energy(&sim, r), FULL, "camouflaged +83, capped");
    }

    // QUIRK C-233: kept
    #[test]
    fn enemy_clear_up_drains_even_the_ranger() {
        // re/masking 9 V3 (numbers from 3.4: +83 before -2000, upper limit 100,000 in the first tick).
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let r = masked_ranger(&mut sim, (10, 10));
        run_ticks_cleared_up(&mut sim, r, 1);
        assert_eq!(energy(&sim, r), 98_000);
        run_ticks_cleared_up(&mut sim, r, 51);
        assert_eq!(energy(&sim, r), 233, "after 52 ticks");
        run_ticks_cleared_up(&mut sim, r, 1);
        assert_eq!(energy(&sim, r), 0, "tick 53 clamped");
        assert_eq!(flags(&sim, r), (true, false), "still camouflaged");
        run_ticks_cleared_up(&mut sim, r, 1);
        assert_eq!(
            flags(&sim, r),
            (false, true),
            "tick 54: uncamouflaging begins"
        );
        // Scouting over: +83 per tick; camouflaged again in the first tick with fraction > 10,000 (121 * 83).
        sim.state.fog.cleared.clear();
        tk::run_ticks(&mut sim, 120);
        assert_eq!(energy(&sim, r), 120 * 83);
        assert!(
            !masked(&sim, r) && flags(&sim, r) == (false, false),
            "0x419 through"
        );
        tk::run_ticks(&mut sim, 1);
        assert_eq!(energy(&sim, r), 10_043);
        assert_eq!(flags(&sim, r), (true, true), "re-camouflaging");
    }

    // QUIRK C-233: kept
    #[test]
    fn enemy_clear_up_drains_unmasked_units_too() {
        // C-233: -2000 also without camouflage, after the +83 of the tick.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        run_ticks_cleared_up(&mut sim, a, 1);
        assert_eq!(energy(&sim, a), 98_000);
        run_ticks_cleared_up(&mut sim, a, 2);
        assert_eq!(energy(&sim, a), 98_000 - 2 * 1917);
        assert_eq!(flags(&sim, a), (false, false));
        // Real path: an enemy scout (`CANCLEARUP`) next to it scouts via the fog cycle;
        // without him the unit stays full.
        let b = tk::spawn(&mut sim, tk::ARCHER, 0, (40, 40));
        tk::run_ticks(&mut sim, 20);
        assert_eq!(energy(&sim, b), FULL);
        tk::spawn(&mut sim, tk::SCOUT, 1, (41, 40));
        tk::run_ticks(&mut sim, 60);
        assert_eq!(energy(&sim, b), 0, "scouted: -1917 per tick");
    }

    // QUIRK K-301: kept
    #[test]
    fn masking_with_empty_energy_lasts_one_animation() {
        // K-301: camouflaging does not check the energy.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        set_energy(&mut sim, a, 0);
        assert_eq!(say(&mut sim, a, msg::START), 0);
        tk::run_ticks(&mut sim, 17); // N ... N + 16
        assert_eq!((flags(&sim, a), masked(&sim, a)), ((true, true), false));
        tk::run_ticks(&mut sim, 1); // N + 17
        assert!(masked(&sim, a), "0x319 sets MASKED");
        assert_eq!(
            flags(&sim, a),
            (false, true),
            "uncamouflaging in the same tick"
        );
        assert_eq!(energy(&sim, a), 0);
        // `0x419` from the tick: 18 ticks later (see `masking_follows_the_energy_automaton`).
        tk::run_ticks(&mut sim, 17);
        assert!(masked(&sim, a));
        tk::run_ticks(&mut sim, 1);
        assert!(!masked(&sim, a), "0x419");
    }

    // QUIRK C-240: kept
    #[test]
    fn a_sheriff_upgrade_unmasks_next_tick() {
        // re/masking 9 V4, C-240: extra energy -100 % -> maximum 0 -> the tick uncamouflages.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = masked_archer(&mut sim, (10, 10));
        let r = masked_ranger(&mut sim, (14, 10));
        let before = energy(&sim, a);
        assert!(crate::upgrades::temp::apply(&mut sim, a, tk::SHERIFF));
        assert!(crate::upgrades::temp::apply(&mut sim, r, tk::SHERIFF));
        let pct = sim.type_upgrade_percent(0, tk::ARCHER, ENERGY);
        assert_eq!(sim.object(a).unwrap().attributes.max_value(ENERGY, pct), 0);
        tk::run_ticks(&mut sim, 1);
        assert_eq!(flags(&sim, a), (false, true), "next tick: uncamouflaging");
        assert_eq!(energy(&sim, a), before - 55, "the fraction is kept");
        assert_eq!(flags(&sim, r), (false, true));
        // `EVERYTIMETARNED` does not camouflage again with maximum 0.
        tk::run_ticks(&mut sim, 100);
        assert!(!masked(&sim, r) && flags(&sim, r) == (false, false));
        // The camouflage command is accepted and ends at once after the animation.
        assert!(!masked(&sim, a));
        assert_eq!(say(&mut sim, a, msg::START), 0);
        tk::run_ticks(&mut sim, 17);
        assert_eq!((flags(&sim, a), masked(&sim, a)), ((true, true), false));
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, a));
        assert_eq!(
            flags(&sim, a),
            (false, true),
            "camouflaged and uncamouflaging in the same tick"
        );
    }

    // QUIRK C-232: kept
    #[test]
    fn an_alarm_post_masks_at_once_with_anim_0() {
        // re/masking 9 V5: the building camouflages at once, the simulation always sets `anim[0]`.
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let post = tk::spawn(&mut sim, tk::ALARM_POST, 0, (20, 20));
        let guest = tk::spawn(&mut sim, tk::ARCHER, 0, (24, 20));
        assert_eq!(
            crate::module::quarter::accept_unit(&mut sim, post, guest),
            0
        );
        // Target search of slot 1: `MM_CHECKFOG` at the head filter with player 1; the occupant with
        // `MM2_DONOTIGNOREQUARTEREDUNITS | MM2_CALLMETHOD`, `MM_METHODINDEX 5`.
        let fog = MatchFilter {
            mode: 0x200_0000,
            ..MatchFilter::constructed()
        };
        let in_masked = MatchFilter {
            mode: 0x10,
            mode2: 0x6,
            method: 5,
            ..MatchFilter::constructed()
        };
        assert!(matches_target(&mut sim, post, &fog, 1, 1));
        assert!(matches_target(&mut sim, guest, &in_masked, 1, 1));
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, post));
        assert_eq!(flags(&sim, post), (true, false), "without delay");
        assert_eq!(sim.object(post).unwrap().anim.current, 0);
        assert!(!matches_target(&mut sim, post, &fog, 1, 1));
        assert!(matches_target(&mut sim, post, &fog, 0, 0), "owner");
        assert!(!matches_target(&mut sim, guest, &in_masked, 1, 1));
        // Camouflaged +83, no consumption.
        tk::run_ticks(&mut sim, 30);
        assert_eq!(energy(&sim, post), FULL);
        // `0x219`: visible at once, `anim[0]` again (restarted).
        let starts = sim.object(post).unwrap().anim.starts;
        assert_eq!(say(&mut sim, post, msg::END), 0);
        assert!(!masked(&sim, post) && flags(&sim, post) == (false, false));
        let o = sim.object(post).unwrap();
        assert_eq!((o.anim.current, o.anim.starts), (0, starts.wrapping_add(1)));
        assert!(matches_target(&mut sim, post, &fog, 1, 1));
        assert!(matches_target(&mut sim, guest, &in_masked, 1, 1));
        // `EVERYTIMETARNED`: camouflaged again in the next tick.
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, post));
    }

    #[test]
    fn swimming_in_unmasks_at_once() {
        // re/masking 9 V9: swimming in sends `0x619` (response 1), `MASKED` off at once.
        let rules = tk::lake_rules(-80.0);
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = masked_archer(&mut sim, (10, 10));
        move_to(&mut sim, a, (30, 30));
        tk::run_ticks(&mut sim, 1);
        assert!(has(&sim, a, status::SWIMMING));
        assert!(!masked(&sim, a) && flags(&sim, a) == (false, false));
        // In the water `0x119` does not camouflage (`0x3A000`).
        assert_eq!(say(&mut sim, a, msg::START), 0);
        assert_eq!(flags(&sim, a), (false, false));
        // Response of `0x619`: 1 with module 25, -1 without (2924 missing: module off).
        assert_eq!(say(&mut sim, a, msg::DEMASK_NOW), 1);
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let without = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        assert_eq!(say(&mut sim, without, msg::DEMASK_NOW), -1);
        // Riding: `DoMount` sends `0x619` with `MASKED` (re/riding 2.4).
        let scout = tk::spawn(&mut sim, tk::SCOUT, 0, (20, 20));
        let horse = tk::spawn(&mut sim, tk::HORSE, 0, (21, 20));
        assert_eq!(say(&mut sim, scout, msg::START), 0);
        tk::run_ticks(&mut sim, 23); // label 2133 ms -> `0x319` in tick N + 22
        assert!(masked(&sim, scout));
        riding_mut(&mut sim, scout, RIDING).unwrap().mount_target = horse;
        do_mount(&mut sim, scout, RIDING);
        assert!(has(&sim, scout, status::ISMOUNTED));
        assert!(!masked(&sim, scout) && flags(&sim, scout) == (false, false));
    }

    // QUIRK K-302: kept
    #[test]
    fn swimming_in_during_the_mask_animation_ends_masked() {
        // re/masking 9 V10, K-302: `0x619` does not clear the pending `0x319`. Contrary to V10
        // ("-55") the unit recharges +83 per tick afterwards: `0x619` cleared `m_IsMasked` and
        // `0x319` only sets `MASKED` (`0x0051D740`), the tick runs in the state "visible"
        // (`0x0051D9A0`, re/masking 3.4). It stays camouflaged until `0x619` comes again, it dies
        // or (on land) a new `0x119` starts the flow from the beginning.
        let rules = tk::lake_rules(-80.0);
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        set_energy(&mut sim, a, 50_000);
        assert_eq!(say(&mut sim, a, msg::START), 0);
        tk::run_ticks(&mut sim, 1); // tick N
        move_to(&mut sim, a, (30, 30));
        tk::run_ticks(&mut sim, 1); // N + 1: swam in
        assert!(has(&sim, a, status::SWIMMING));
        assert!(!masked(&sim, a) && !has(&sim, a, status::ANIM_LOCKED));
        assert_eq!(flags(&sim, a), (false, false));
        tk::run_ticks(&mut sim, 15);
        assert!(!masked(&sim, a));
        tk::run_ticks(&mut sim, 1); // N + 17: `0x319`
        assert!(
            masked(&sim, a) && has(&sim, a, status::SWIMMING),
            "camouflaged in the water"
        );
        let e = energy(&sim, a);
        tk::run_ticks(&mut sim, 10);
        assert_eq!(energy(&sim, a), e + 10 * 83);
        // `0x219` has no effect any more (`m_IsMasked` off).
        assert_eq!(say(&mut sim, a, msg::END), 0);
        tk::run_ticks(&mut sim, 30);
        assert!(masked(&sim, a));
    }

    // QUIRK K-303: kept
    #[test]
    fn crossing_mask_and_unmask_ends_as_the_original() {
        // K-303 via `send` (script path; posted commands wait during the camouflage animation, see
        // `posted_orders_wait_during_the_mask_animation`).
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Uncamouflage during the camouflage animation: `0x319` camouflages, `0x419` uncamouflages -> visible.
        let a = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        assert_eq!(say(&mut sim, a, msg::START), 0);
        tk::run_ticks(&mut sim, 5); // N ... N + 4
        assert_eq!(say(&mut sim, a, msg::END), 0);
        assert_eq!(flags(&sim, a), (false, true));
        tk::run_ticks(&mut sim, 13); // `0x319` (N + 17)
        assert!(masked(&sim, a) && flags(&sim, a) == (false, false));
        tk::run_ticks(&mut sim, 5); // `0x419` (N + 5 + 17)
        assert!(!masked(&sim, a) && flags(&sim, a) == (false, false));
        tk::run_ticks(&mut sim, 30);
        assert!(!masked(&sim, a));
        // Camouflage during the uncamouflage animation: after `0x419` visible with `m_IsMasked` and -55,
        // until `0x319` camouflages again.
        let b = masked_archer(&mut sim, (14, 10));
        set_energy(&mut sim, b, 50_000);
        assert_eq!(say(&mut sim, b, msg::END), 0);
        tk::run_ticks(&mut sim, 5); // M ... M + 4
        assert_eq!(say(&mut sim, b, msg::START), 0);
        assert_eq!(flags(&sim, b), (true, true));
        tk::run_ticks(&mut sim, 13); // `0x419` (M + 17)
        assert!(!masked(&sim, b) && flags(&sim, b) == (true, false));
        let e = energy(&sim, b);
        tk::run_ticks(&mut sim, 4);
        assert!(!masked(&sim, b));
        assert_eq!(energy(&sim, b), e - 4 * 55);
        tk::run_ticks(&mut sim, 1); // `0x319` (M + 5 + 17)
        assert!(masked(&sim, b) && flags(&sim, b) == (true, false));
        assert_eq!(energy(&sim, b), e - 5 * 55);
    }

    // QUIRK K-303: kept
    #[test]
    fn posted_orders_wait_during_the_mask_animation() {
        // Slot 10 `0x0051D7E0`: `m_IsBusy` holds the FIFO until `0x319` falls.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ARCHER, 0, (10, 10));
        assert!(post_message(
            &mut sim,
            a,
            Message::new(msg::START, &[]),
            true
        ));
        tk::run_ticks(&mut sim, 1); // tick N
        assert_eq!(flags(&sim, a), (true, true));
        assert!(post_message(&mut sim, a, Message::new(msg::END, &[]), true));
        tk::run_ticks(&mut sim, 16); // until N + 16
        assert_eq!(sim.object(a).unwrap().fifo.len(), 1, "waits");
        tk::run_ticks(&mut sim, 1); // N + 17: `0x319` in the module tick, after the FIFO
        assert!(masked(&sim, a));
        assert_eq!(sim.object(a).unwrap().fifo.len(), 1);
        tk::run_ticks(&mut sim, 1); // N + 18: now `0x219` takes effect
        assert!(sim.object(a).unwrap().fifo.is_empty());
        assert_eq!(flags(&sim, a), (false, true));
        assert!(masked(&sim, a));
        tk::run_ticks(&mut sim, 17);
        assert!(!masked(&sim, a));
    }

    // QUIRK C-232: kept
    #[test]
    fn nothing_but_the_listed_causes_unmasks() {
        // C-232: walking, hits, `AbortAllModules` and change of owner do not uncamouflage.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = masked_archer(&mut sim, (10, 10));
        let enemy = tk::spawn(&mut sim, tk::COLONIST, 1, (30, 30));
        let go = Command {
            player: 0,
            kind: CommandKind::Move {
                units: vec![a],
                x: (16 << 12) + 0x800,
                z: (10 << 12) + 0x800,
                target: -1,
                formation: 0,
                direction: crate::motion::NO_DIRECTION,
                spread: NO_SPREAD,
                flags: 0,
            },
        };
        crate::tick::step(sim.state, sim.rules, &[go]);
        tk::run_ticks(&mut sim, 20);
        assert_ne!(sim.object(a).unwrap().tile, (10, 10), "walked");
        assert!(masked(&sim, a) && flags(&sim, a) == (true, false));
        let hit = Message::new(0x17F, &[-12, enemy.as_i32(), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let hp = sim.attribute(a, 14);
        send(&mut sim, a, &hit);
        assert!(sim.attribute(a, 14) < hp, "hit");
        abort_all_modules(&mut sim, a);
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, a) && flags(&sim, a) == (true, false));
        let b = change_owner(&mut sim, a, 1).expect("new ID");
        tk::run_ticks(&mut sim, 1);
        assert!(masked(&sim, b) && flags(&sim, b) == (true, false));
    }

    // QUIRK K-307: kept
    #[test]
    fn a_disabled_variant_freezes_the_mask() {
        // K-307: module 25 off (K-300) -> no tick, messages -1, `MASKED` stays.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = masked_archer(&mut sim, (10, 10));
        sim.object_mut(a).unwrap().set_module_enabled(INDEX, false);
        tk::run_ticks(&mut sim, 1);
        let e = energy(&sim, a);
        tk::run_ticks(&mut sim, 50);
        assert_eq!(energy(&sim, a), e, "no consumption");
        assert_eq!(say(&mut sim, a, msg::END), -1);
        tk::run_ticks(&mut sim, 20);
        assert!(masked(&sim, a) && flags(&sim, a) == (true, false));
    }
}
