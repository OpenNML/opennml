//! Module 28 `CPropertyAssassin` (re/masking): the state of digging in and assassinating (animations, stab damage,
//! target), `Init` (1.3), the queries `0x11C` ... `0x71C` (2.2, 3.6-3.9) with search chain and stab and the tick (only
//! `Module_UpdateEvents`, slot 13 `0x005119A0`).
//!
//! Effects and `NotifyUiIfSelected` are handled by the host: the simulation reports the effects as
//! [`SimEvent::EffectOnObject`], the command bar reads the status. Slot 10 (`m_IsBusy`) stays false and the abort (slot
//! 11) does nothing (K-309).

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::id::ObjectId;
use crate::message::Message;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::ModuleKind;
use super::events::update_events;
use super::gather::{anim_label_time, set_animation};
use super::masking::BLOCKING_STATUS;

pub const INDEX: usize = 28;
/// Distance between two target searches without a hit in ms (`0x41C`, `0x31C`; re/masking 3.7).
pub const SEARCH_MS: i32 = 500;
/// Pause after a stab in ms (`0x71C`; re/masking 3.8).
pub const REST_MS: i32 = 5000;
/// Search radius in fixed point 20.12 (4096 = 1 tile, `DAT_00672494`); the hard radius is added.
pub const RANGE: i32 = 4096;
/// Timer group of all assassin timers (re/masking 3.10).
const TIMER_GROUP: u8 = 0x80;
/// Attack module: dug in, it is out of the tick list (`QueueModuleTickChange(0x11, …)`).
const ATTACK_MODULE: usize = 0x11;
/// Hit points (attribute 14).
const HITPOINTS: u32 = 14;
/// Render bit "shadow" (`ClearRenderBits(2)` or `OrRenderBits(2)`).
const SHADOW: u32 = 2;
/// Dug in: `MASKED | WAITING4KILLING` (`0x500000`).
const DUG_IN: u32 = status::MASKED | status::WAITING4KILLING;
/// Filter of the target search (`0x004FFE20`): `MM_PROPERTY | MM_ANDALL | MM_HL_ENEMYLIST`.
const SEARCH_MODE: u32 = 0x414;
/// Effects on the object (`0x00622274`, `0x00622298`).
pub const EFFECT_GO_DOWN: &str = "data/effects/meuchel_eingraben.pef";
pub const EFFECT_GO_UP: &str = "data/effects/meuchel_ausgraben.pef";

/// Messages of the assassin module (query `0x004FFFF0`, re/masking 2.2).
pub mod msg {
    /// `PMSG_ASSASSIN_GODOWN`: dig in (command `0x20`).
    pub const GO_DOWN: u32 = 0x11C;
    /// `PMSG_ASSASSIN_GOUP`: dig out (command `0x21`).
    pub const GO_UP: u32 = 0x21C;
    /// `PMSG_ASSASSIN_CHECKTARGET`: target search (own timer).
    pub const CHECK_TARGET: u32 = 0x31C;
    /// `PMSG_ASSASSIN_GODOWNREADY`: dig-in animation finished (own timer).
    pub const GO_DOWN_READY: u32 = 0x41C;
    /// `PMSG_ASSASSIN_GOUPREADY`: dig-out animation finished (own timer).
    pub const GO_UP_READY: u32 = 0x51C;
    /// `PMSG_ASSASSIN_ATTACK`: stab, damage to the target (own timer).
    pub const ATTACK: u32 = 0x61C;
    /// `PMSG_ASSASSIN_ATTACKFINISHED`: stab animation finished (own timer).
    pub const ATTACK_FINISHED: u32 = 0x71C;
}

/// `CPropertyAssassin` (`0x88` bytes); savegame in the order `m_AttackPoints`, `m_GoDownAnim`, `m_GoUpAnim`,
/// `m_IsBusy`, `m_IsDown`, `m_KillAnim`, `m_TargetHID` (re/masking 1.3, 5). The message filter (`GameObject+0x730`) is
/// object state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assassin {
    /// `m_IsBusy` (`+0x70`): never set in the original (slot 10, K-309); always `false`.
    pub is_busy: bool,
    /// `m_IsDown` (`+0x71`): dug in (from the end of the dig-in animation).
    pub is_down: bool,
    /// `m_GoDownAnim` (`+0x74`, 290001).
    pub go_down_anim: i32,
    /// `m_GoUpAnim` (`+0x78`, 290002).
    pub go_up_anim: i32,
    /// `m_KillAnim` (`+0x7C`, 290003): stab animation.
    pub kill_anim: i32,
    /// `m_AttackPoints` (`+0x80`, 290004): damage of a stab (zombie 0).
    pub attack_points: i32,
    /// `m_TargetHID` (`+0x84`): target of the running stab; may point to a removed object. The original leaves the
    /// field uninitialized, the reimplementation starts with `NONE` (K-309).
    pub target: ObjectId,
}

// `ObjectId` has no `Default`; the start is `NONE`.
impl Default for Assassin {
    fn default() -> Self {
        Self {
            is_busy: false,
            is_down: false,
            go_down_anim: 0,
            go_up_anim: 0,
            kill_anim: 0,
            attack_points: 0,
            target: ObjectId::NONE,
        }
    }
}

pub fn assassin<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Assassin> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Assassin(a) => Some(a),
        _ => None,
    }
}

pub fn assassin_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Assassin> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Assassin(a) => Some(a),
        _ => None,
    }
}

/// `PropertyAssassin_Init` (`0x004FF9A0`, re/masking 1.3): the four values 290001-290004; the flags stay 0, `is_busy`
/// and `is_down` off. Init does not set the target.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = 0;
    if let ModuleKind::Assassin(a) = &mut m.kind {
        a.is_busy = false;
        a.is_down = false;
        a.go_down_anim = value(290_001);
        a.go_up_anim = value(290_002);
        a.kill_anim = value(290_003);
        a.attack_points = value(290_004);
    }
}

/// `PropertyAssassin_Query` (`0x004FFFF0`, re/masking 2.2): `0x11C` dig in, `0x21C` dig out, `0x31C` target search,
/// `0x41C`/`0x51C` end of the dig-in/dig-out animation, `0x61C` stab, `0x71C` end of the stab (all 0), otherwise -1. A
/// switched-off module receives no message (`send` returns -1, K-307): the timers rest, `WAITING4KILLING` and the
/// filter stay.
pub fn query(sim: &mut Sim, id: ObjectId, message: &mut Message) -> i32 {
    if assassin(sim, id).is_none() {
        return -1;
    }
    match message.code {
        msg::GO_DOWN => go_down(sim, id),
        msg::GO_UP => go_up(sim, id),
        msg::CHECK_TARGET => check_target(sim, id),
        msg::GO_DOWN_READY => go_down_ready(sim, id),
        msg::GO_UP_READY => go_up_ready(sim, id),
        msg::ATTACK => attack(sim, id),
        msg::ATTACK_FINISHED => attack_finished(sim, id),
        _ => -1,
    }
}

/// Tick (slot 13 `0x005119A0`): only the timers.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

/// Timer `code` after `delay_ms` (group `0x80`, `Module_PostDelayedMessage`).
fn post_delayed(sim: &mut Sim, id: ObjectId, delay_ms: i32, code: u32) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(delay_ms, Message::new(code, &[]), false, TIMER_GROUP);
    }
}

/// Effect on the object (`GameObject_AddOwnedEffect`, host).
fn effect(sim: &mut Sim, id: ObjectId, path: &str) {
    sim.state.events.push(SimEvent::EffectOnObject {
        object: id,
        path: path.to_string(),
    });
}

/// `PropertyAssassin_GoDown` (`0x004FFA00`, re/masking 3.6): nothing if already dug in (`m_IsDown`) or the unit is
/// riding, sitting in a boat, swimming or diving (`0x3A000`). Otherwise dig-in animation (without restart), `0x41C`
/// after marker 1, immediately `MASKED | WAITING4KILLING`, effect, `0x21C` and `0x110` **appended** to the message
/// filter (without clearing, K-308; two `0x11C` in the same tick give duplicate codes, C-237), mode 0 and the attack
/// module out of the tick list. `m_IsDown` is not set until `0x41C`: until then dig-out is ignored.
fn go_down(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(a) = assassin(sim, id) else { return 0 };
    let (is_down, anim) = (a.is_down, a.go_down_anim);
    let Some(st) = sim.object(id).map(|o| o.status) else {
        return 0;
    };
    if is_down || st & BLOCKING_STATUS != 0 {
        return 0;
    }
    set_animation(sim, id, anim);
    let delay = anim_label_time(sim, id, anim, 1);
    post_delayed(sim, id, delay, msg::GO_DOWN_READY);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(DUG_IN, true);
    }
    effect(sim, id, EFFECT_GO_DOWN);
    if let Some(o) = sim.object_mut(id) {
        o.filter.codes.push(msg::GO_UP);
        o.filter.codes.push(0x110);
        o.filter.mode = 0;
        o.queue_module_tick_change(ATTACK_MODULE, false);
    }
    0
}

/// `PropertyAssassin_GoDownReady` (`0x004FFCD0`): `m_IsDown`, no ground shadow, first search after 500 ms.
fn go_down_ready(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(a) = assassin_mut(sim, id) {
        a.is_down = true;
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits &= !SHADOW;
    }
    post_delayed(sim, id, SEARCH_MS, msg::CHECK_TARGET);
    0
}

/// `PropertyAssassin_CheckTarget` (`0x004FFE20`, re/masking 3.7, C-235): without `m_IsDown` the chain ends. Otherwise
/// the nearest object with walk (even switched off), enemy of the owner, not dead, not quartered - **without a fog
/// check** (camouflaged ones count) - within an edge distance <= 4096 + `GO+0x1B0`. No hit: next search after 500 ms.
/// Hit: `m_TargetHID`, stab animation (restart), `0x61C` after marker 2, `0x71C` after marker 1, `MASKED` off
/// (`WAITING4KILLING` stays, C-239).
fn check_target(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(a) = assassin(sim, id) else { return 0 };
    let (is_down, kill) = (a.is_down, a.kill_anim);
    if !is_down {
        return 0;
    }
    let Some(o) = sim.object(id) else { return 0 };
    let (pos, owner, hard) = (
        (o.grid.x.0, o.grid.z.0),
        i32::from(o.owner()),
        o.hard_radius,
    );
    let f = MatchFilter {
        mode: SEARCH_MODE,
        prop_id: 0,
        ..MatchFilter::constructed()
    };
    let Some(t) = find_nearest_matching(sim, pos, &f, RANGE, owner, hard) else {
        post_delayed(sim, id, SEARCH_MS, msg::CHECK_TARGET);
        return 0;
    };
    if let Some(a) = assassin_mut(sim, id) {
        a.target = t;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(kill, 0, true, false, false);
    }
    let (hit, end) = (
        anim_label_time(sim, id, kill, 2),
        anim_label_time(sim, id, kill, 1),
    );
    post_delayed(sim, id, hit, msg::ATTACK);
    post_delayed(sim, id, end, msg::ATTACK_FINISHED);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::MASKED, false);
    }
    0
}

/// `PropertyAssassin_Attack` (`0x004FFD50`, re/masking 3.8, C-234): `GameWorld_GetObjectById(target, 0)` (also a
/// carcass) loses `m_AttackPoints` directly from its hit points - without armor, attacker, combat points or experience;
/// the death is checked by the tick of the victim. If the target is missing (destroyed, stale ID), nothing happens.
fn attack(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(a) = assassin(sim, id) else { return 0 };
    let (t, points) = (a.target, a.attack_points);
    let Some(o) = sim.object(t) else { return 0 };
    let pct = sim.type_upgrade_percent(o.owner(), o.guid, HITPOINTS);
    if let Some(o) = sim.object_mut(t) {
        o.attributes
            .add_to_current_value(HITPOINTS, points.wrapping_neg(), pct);
    }
    0
}

/// `PropertyAssassin_AttackFinished` (`0x004FFD90`): `MASKED` on - even if dug out in the meantime (C-236) - and the
/// next search after 5000 ms.
fn attack_finished(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::MASKED, true);
    }
    post_delayed(sim, id, REST_MS, msg::CHECK_TARGET);
    0
}

/// `PropertyAssassin_GoUp` (`0x004FFB80`, re/masking 3.9): only with `m_IsDown`. Dig-out animation (without restart),
/// shadow again, `WAITING4KILLING` and `MASKED` off, lock bit `0x800000` on, `0x51C` after marker 1, `m_IsDown` off,
/// effect, filter cleared with mode 1 and the attack module back into the tick list. Pending timers stay: `0x31C` ends
/// at `!is_down`, `0x61C` still damages, `0x71C` camouflages again.
fn go_up(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(a) = assassin(sim, id) else { return 0 };
    let (is_down, anim) = (a.is_down, a.go_up_anim);
    if !is_down {
        return 0;
    }
    set_animation(sim, id, anim);
    if let Some(o) = sim.object_mut(id) {
        o.render_bits |= SHADOW;
        o.set_status_flags(status::WAITING4KILLING, false);
        o.set_status_flags(status::MASKED, false);
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
    let delay = anim_label_time(sim, id, anim, 1);
    post_delayed(sim, id, delay, msg::GO_UP_READY);
    if let Some(a) = assassin_mut(sim, id) {
        a.is_down = false;
    }
    effect(sim, id, EFFECT_GO_UP);
    if let Some(o) = sim.object_mut(id) {
        o.filter.codes.clear();
        o.filter.mode = 1;
        o.queue_module_tick_change(ATTACK_MODULE, true);
    }
    0
}

/// `PropertyAssassin_GoUpReady` (`0x004FFD20`): `m_IsDown` off, lock bit `0x800000` off.
fn go_up_ready(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(a) = assassin_mut(sim, id) {
        a.is_down = false;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, CommandKind, NO_SPREAD};
    use crate::filter::matches_target;
    use crate::fx::{Fx, FxVec3};
    use crate::message::{Envelope, Message, MessageFilter};
    use crate::messaging::{post_command, post_message, send};
    use crate::module::ability_testkit as tk;
    use crate::module::building::fort_testkit::move_to;
    use crate::module::walk::find_to_msg;
    use crate::object::status;
    use crate::rules::Rules;
    use crate::save::testkit::save_and_load;
    use crate::sim::Sim;
    use crate::state::{SimEvent, SimState};
    use crate::world::{destroy_object, set_grid_position};
    use nml_formats::targets::MatchFilter;

    /// Dug in: `MASKED | WAITING4KILLING` (`0x500000`).
    const DUG: u32 = status::MASKED | status::WAITING4KILLING;
    /// Attack module (tick list).
    const ATTACK_INDEX: u8 = 17;

    #[test]
    fn init_reads_the_four_values() {
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Without upgrade 2922 module 28 is off, `Init` ran anyway.
        let id = tk::spawn(&mut sim, tk::ASSASSIN, 0, (10, 10));
        assert!(!sim.object(id).unwrap().has_module(INDEX, false));
        let a = assassin(&sim, id).expect("module 28");
        assert_eq!(
            (a.go_down_anim, a.go_up_anim, a.kill_anim, a.attack_points),
            (8, 0, 1, 250)
        );
        // `m_TargetHID` without Init: start `NONE`; nothing has begun.
        assert_eq!(a.target, ObjectId::NONE);
        assert!(!a.is_busy && !a.is_down);
        assert_eq!(sim.module(id, INDEX).unwrap().base.flags, 0);
    }

    /// World with 2922 unlocked (assassins are created with module 28 switched on).
    fn unlocked_state(rules: &Rules) -> SimState {
        let mut state = tk::state(rules);
        tk::unlock(&mut state, 0, tk::DIG_IN_UPGRADE);
        state
    }

    fn say(sim: &mut Sim, id: ObjectId, code: u32) -> i32 {
        send(sim, id, &Message::new(code, &[]))
    }

    fn st(sim: &Sim, id: ObjectId) -> u32 {
        sim.object(id).unwrap().status
    }

    fn hp(sim: &Sim, id: ObjectId) -> i32 {
        sim.attribute(id, 14)
    }

    /// Fraction of the hit points unclamped (negative after the stab).
    fn raw_hp(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().attributes.raw_fraction(14)
    }

    /// Fraction of a colonist (160 HP) after `n` stabs of 250: `AddToCurrentValue` reads the value clamped to 0, so
    /// every further stab sets it to -250.
    fn colonist_after(n: usize) -> i32 {
        match n {
            0 => 100_000,
            1 => (160 - 250) * 100_000 / 160,
            _ => -250 * 100_000 / 160,
        }
    }

    /// Carcasses of type `guid` on slot 8 (`StartDying` changes the owner, C-142).
    fn corpses(sim: &Sim, guid: u32) -> Vec<ObjectId> {
        sim.state
            .objects()
            .filter(|o| o.guid == guid && o.owner() == 8 && o.status & status::DIED != 0)
            .map(|o| o.id)
            .collect()
    }

    fn corpse(sim: &Sim, guid: u32) -> Option<ObjectId> {
        corpses(sim, guid).first().copied()
    }

    /// [`raw_hp`] of the object or - if it has already changed owner after its death (depending on the order of the
    /// ticks in the same object tick) - of its carcass.
    fn raw_hp_or_corpse(sim: &Sim, id: ObjectId, guid: u32) -> i32 {
        match sim.object(id) {
            Some(_) => raw_hp(sim, id),
            None => raw_hp(sim, corpse(sim, guid).expect("carcass")),
        }
    }

    fn is_down(sim: &Sim, id: ObjectId) -> bool {
        assassin(sim, id).unwrap().is_down
    }

    fn target(sim: &Sim, id: ObjectId) -> ObjectId {
        assassin(sim, id).unwrap().target
    }

    fn anim(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().anim.current
    }

    fn filter(sim: &Sim, id: ObjectId) -> MessageFilter {
        sim.object(id).unwrap().filter.clone()
    }

    /// Pending timers `code` of the module (list and `m_TimersWS`).
    fn timers(sim: &Sim, id: ObjectId, code: u32) -> usize {
        let b = &sim.module(id, INDEX).unwrap().base;
        b.events
            .iter()
            .chain(b.timers_ws.iter())
            .filter(|e| e.msg.code == code)
            .count()
    }

    /// `MM_CHECKFOG` at the head filter for slot 1 (TARGET caller).
    fn seen_by_1(sim: &mut Sim, id: ObjectId) -> bool {
        let fog = MatchFilter {
            mode: 0x200_0000,
            ..MatchFilter::constructed()
        };
        matches_target(sim, id, &fog, 1, 1)
    }

    /// Has one of the steps since clearing the event list reported the effect `path` on the object?
    fn effect(sim: &Sim, id: ObjectId, path: &str) -> bool {
        sim.state.events.iter().any(|e| {
            matches!(e, SimEvent::EffectOnObject { object, path: p } if *object == id && p == path)
        })
    }

    /// Place object `id` at the grid point `from + (dx, dz)` (fixed point, 4096 = 1 tile).
    fn place_at(sim: &mut Sim, id: ObjectId, from: ObjectId, (dx, dz): (i32, i32)) {
        let g = sim.object(from).unwrap().grid;
        let pos = FxVec3::new(Fx(g.x.0 + dx), g.y, Fx(g.z.0 + dz));
        set_grid_position(sim, id, pos, true);
    }

    /// Place object `id` to the left of `from` on the first grid point with an edge distance >= `edge` and return it
    /// (`GameObject_GetEdgeDistanceToPoint` via `Fixed_Hypot` with table: the distance is not exactly center - radius
    /// and jumps in steps).
    fn place_at_edge(sim: &mut Sim, id: ObjectId, from: ObjectId, edge: i32) -> i32 {
        let g = sim.object(from).unwrap().grid;
        for dx in 4096..8192 {
            place_at(sim, id, from, (-dx, 0));
            let d = crate::collision::edge_distance_to_point(sim, id, (g.x.0, g.z.0));
            if d >= edge {
                return d;
            }
        }
        panic!("edge distance {edge} not reachable");
    }

    /// Read the savegame and check it against the rules (like the viewer when loading).
    fn assert_loads(state: &SimState, rules: &Rules) {
        let bytes = crate::save::testkit::save_bytes(state, rules);
        crate::save::SaveGame::from_bytes(&bytes)
            .expect("savegame readable")
            .check(rules)
            .expect("savegame valid");
    }

    /// Assassin of slot 0 on `tile`, dug in via `0x11C` before tick N, then `n` ticks (tick N is the first).
    fn dug_in(sim: &mut Sim, tile: (i32, i32), n: usize) -> ObjectId {
        let a = tk::spawn(sim, tk::ASSASSIN, 0, tile);
        assert_eq!(say(sim, a, msg::GO_DOWN), 0);
        tk::run_ticks(sim, n);
        a
    }

    // QUIRK K-308, W-167, C-234, C-239: kept
    #[test]
    fn a_dug_in_assassin_stabs_once_and_rests() {
        // re/masking 9 V6. Timers from `send` before tick N fall in N + floor(d/100) + 1, those from a handler in
        // `update_events` of tick M in M + floor(d/100) + 1 (`events.rs`, step D).
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ASSASSIN, 0, (20, 20));
        // Colonist one tile next to it: edge distance 4096 - 512 = 3584 <= 4608.
        let c = tk::spawn(&mut sim, tk::COLONIST, 1, (21, 20));
        tk::run_ticks(&mut sim, 1);
        assert!(sim.object(a).unwrap().tick_list.contains(&ATTACK_INDEX));
        assert_ne!(sim.object(a).unwrap().render_bits & 2, 0, "shadow");
        assert!(seen_by_1(&mut sim, a), "visible before digging in");
        sim.state.events.clear();
        assert_eq!(say(&mut sim, a, msg::GO_DOWN), 0);
        assert_eq!(st(&sim, a) & DUG, DUG, "immediately");
        assert_eq!(
            filter(&sim, a),
            MessageFilter {
                mode: 0,
                codes: vec![0x21C, 0x110]
            }
        );
        assert!(effect(&sim, a, EFFECT_GO_DOWN));
        assert!(!is_down(&sim, a));
        tk::run_ticks(&mut sim, 1); // tick N
        assert!(
            !sim.object(a).unwrap().tick_list.contains(&ATTACK_INDEX),
            "attack out of the tick list after the tick"
        );
        assert_eq!(anim(&sim, a), 8, "dig-in animation");
        assert!(
            !seen_by_1(&mut sim, a),
            "dug in: invisible without scouting"
        );
        // `0x41C` after 1843 ms: N + 19.
        tk::run_ticks(&mut sim, 18); // N + 1 ... N + 18
        assert!(!is_down(&sim, a));
        tk::run_ticks(&mut sim, 1); // N + 19
        assert!(is_down(&sim, a));
        assert_eq!(sim.object(a).unwrap().render_bits & 2, 0, "shadow off");
        // `0x31C` after 500 ms: N + 25.
        tk::run_ticks(&mut sim, 5); // N + 20 ... N + 24
        assert_eq!(target(&sim, a), ObjectId::NONE);
        assert_eq!(st(&sim, a) & DUG, DUG);
        tk::run_ticks(&mut sim, 1); // N + 25: search
        assert_eq!(target(&sim, a), c);
        assert_eq!(anim(&sim, a), 1, "stab animation");
        assert_eq!(st(&sim, a) & DUG, status::WAITING4KILLING, "MASKED off");
        // C-239: during the stab a target for slot 1 on a visible tile.
        assert!(seen_by_1(&mut sim, a));
        assert_eq!(hp(&sim, c), 160);
        // `0x61C` after marker 2 (640 ms): N + 32, -250 directly onto the hit points.
        tk::run_ticks(&mut sim, 6); // N + 26 ... N + 31
        assert_eq!(hp(&sim, c), 160);
        tk::run_ticks(&mut sim, 1); // N + 32
        // The victim ticks after the assassin: `CheckDeath` in the same tick, carcass on slot 8.
        assert!(sim.object(c).is_none());
        let dead = corpse(&sim, tk::COLONIST).expect("dead via CheckDeath");
        assert_eq!(raw_hp(&sim, dead), colonist_after(1), "160 - 250");
        assert!(
            sim.object(dead).unwrap().last_attacker.is_none(),
            "no attacker"
        );
        // `0x71C` after marker 1 (1180 ms): N + 37, camouflaged again.
        tk::run_ticks(&mut sim, 4); // N + 33 ... N + 36
        assert_eq!(st(&sim, a) & DUG, status::WAITING4KILLING);
        assert!(seen_by_1(&mut sim, a));
        tk::run_ticks(&mut sim, 1); // N + 37
        assert_eq!(st(&sim, a) & DUG, DUG);
        assert!(!seen_by_1(&mut sim, a), "invisible again after the stab");
        // Next search after 5000 ms: N + 88. A new enemy from N + 40 is found only then.
        tk::run_ticks(&mut sim, 3); // N + 38 ... N + 40
        let c2 = tk::spawn(&mut sim, tk::COLONIST, 1, (20, 21));
        tk::run_ticks(&mut sim, 47); // N + 41 ... N + 87
        assert_eq!((target(&sim, a), st(&sim, a) & DUG), (c, DUG));
        assert_eq!(timers(&sim, a, msg::CHECK_TARGET), 1, "one search chain");
        tk::run_ticks(&mut sim, 1); // N + 88
        assert_eq!(target(&sim, a), c2);
        assert_eq!(st(&sim, a) & DUG, status::WAITING4KILLING);
    }

    // QUIRK C-235: kept
    #[test]
    fn the_search_ignores_fog_and_buildings() {
        // C-235, re/masking 3.7: filter `0x414` (walk, enemy, not dead, not quartered) without `MM_CHECKFOG`; edge
        // distance <= 4096 + 512.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::ASSASSIN, 0, (20, 20));
        // Enemy building right next to it (nearer than any unit), without walk.
        let house = tk::spawn(&mut sim, tk::MONASTERY, 1, (20, 18));
        // Enemy ranger camouflages itself (`EVERYTIMETARNED`, `0x319` after 19 ticks).
        let ranger = tk::spawn(&mut sim, tk::RANGER, 1, (21, 20));
        // Colonist just outside: the first edge distance above 4608.
        let far = tk::spawn(&mut sim, tk::COLONIST, 1, (30, 30));
        let out = place_at_edge(&mut sim, far, a, 4609);
        assert!((4609..4700).contains(&out), "{out}");
        let at = sim.object(a).map(|o| (o.grid.x.0, o.grid.z.0)).unwrap();
        let edge = |sim: &mut Sim, id| crate::collision::edge_distance_to_point(sim, id, at);
        let (to_house, to_ranger) = (edge(&mut sim, house), edge(&mut sim, ranger));
        assert!(to_house < to_ranger, "{to_house} < {to_ranger}");
        tk::run_ticks(&mut sim, 20);
        assert_ne!(st(&sim, ranger) & status::MASKED, 0, "camouflaged");
        let probe = MatchFilter {
            mode: 0x200_0000,
            ..MatchFilter::constructed()
        };
        assert!(
            !matches_target(&mut sim, ranger, &probe, 0, 0),
            "invisible for slot 0"
        );
        let (house_hp, far_hp) = (hp(&sim, house), hp(&sim, far));
        assert_eq!(say(&mut sim, a, msg::GO_DOWN), 0);
        tk::run_ticks(&mut sim, 25); // N ... N + 24
        assert_eq!(target(&sim, a), ObjectId::NONE);
        tk::run_ticks(&mut sim, 1); // N + 25
        assert_eq!(
            target(&sim, a),
            ranger,
            "camouflaged enemy, not the building"
        );
        tk::run_ticks(&mut sim, 7); // N + 32
        assert_eq!(hp(&sim, ranger), 1800 - 250);
        // Ranger out of range: the search from N + 88 finds nothing (building, > 4608) and repeats every 500 ms.
        move_to(&mut sim, ranger, (40, 40));
        tk::run_ticks(&mut sim, 100);
        assert_eq!(target(&sim, a), ranger);
        assert_eq!(st(&sim, a) & DUG, DUG);
        assert_eq!(timers(&sim, a, msg::CHECK_TARGET), 1);
        assert_eq!((hp(&sim, house), hp(&sim, far)), (house_hp, far_hp));
        // Edge distance of exactly 4608 still counts (`d > radius` excludes).
        assert_eq!(place_at_edge(&mut sim, far, a, 4608), 4608);
        tk::run_ticks(&mut sim, 6);
        assert_eq!(target(&sim, a), far);
        assert_eq!(hp(&sim, house), house_hp, "never the building");
    }

    // QUIRK K-308: kept
    #[test]
    fn posted_orders_are_dropped_while_dug_in() {
        // re/masking 9 V7, K-308: mode 0 with `[0x21C, 0x110]` lets only dig-out and dying through.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = dug_in(&mut sim, (20, 20), 20);
        assert!(is_down(&sim, a));
        let walk = Envelope::bare(find_to_msg(
            30 << 12,
            20 << 12,
            0,
            crate::motion::NO_DIRECTION,
            0,
            false,
            0x201,
        ));
        assert!(!post_command(&mut sim, a, walk.clone(), true), "dropped");
        assert!(sim.object(a).unwrap().fifo.is_empty());
        // Also the move command `0x04` (own unit, accepted) stays without effect.
        let go = Command {
            player: 0,
            kind: CommandKind::Move {
                units: vec![a],
                x: (30 << 12) + 0x800,
                z: (20 << 12) + 0x800,
                target: -1,
                formation: 0,
                direction: crate::motion::NO_DIRECTION,
                spread: NO_SPREAD,
                flags: 0,
            },
        };
        crate::tick::step(sim.state, sim.rules, &[go]);
        tk::run_ticks(&mut sim, 20);
        assert_eq!(sim.object(a).unwrap().tile, (20, 20), "stays");
        // `0x21` dig out (posted, with abort): animation 0, status off, `0x800000`, filter default, attack on.
        assert!(post_message(
            &mut sim,
            a,
            Message::new(msg::GO_UP, &[]),
            true
        ));
        let events: Vec<SimEvent> =
            (0..2) // tick M
                .flat_map(|_| crate::tick::step(sim.state, sim.rules, &[]))
                .collect();
        assert!(events.iter().any(|e| matches!(
            e,
            SimEvent::EffectOnObject { object, path } if *object == a && path == EFFECT_GO_UP
        )));
        let o = sim.object(a).unwrap();
        assert_eq!(o.anim.current, 0, "dig-out animation");
        assert_eq!(o.status & DUG, 0);
        assert_ne!(o.status & status::ANIM_LOCKED, 0);
        assert_ne!(o.render_bits & 2, 0, "shadow again");
        assert_eq!(o.filter, MessageFilter::default());
        assert!(o.tick_list.contains(&ATTACK_INDEX), "attack on");
        assert!(!is_down(&sim, a));
        // `0x51C` after 773 ms: M + 8.
        tk::run_ticks(&mut sim, 7);
        assert_ne!(st(&sim, a) & status::ANIM_LOCKED, 0);
        tk::run_ticks(&mut sim, 1);
        assert_eq!(st(&sim, a) & status::ANIM_LOCKED, 0);
        // Now it walks.
        assert!(post_command(&mut sim, a, walk, true));
        tk::run_ticks(&mut sim, 40);
        assert_ne!(sim.object(a).unwrap().tile, (20, 20));
    }

    // QUIRK W-167, C-236: kept
    #[test]
    fn digging_out_during_the_stab_masks_again() {
        // C-236, re/masking 8 no. 13: `0x71C` camouflages the dug-out one as well (without `WAITING4KILLING`).
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = tk::spawn(&mut sim, tk::COLONIST, 1, (21, 20));
        let a = dug_in(&mut sim, (20, 20), 33); // until N + 32: stab landed
        assert_eq!(raw_hp_or_corpse(&sim, c, tk::COLONIST), colonist_after(1));
        assert_eq!(say(&mut sim, a, msg::GO_UP), 0);
        assert_eq!(st(&sim, a) & DUG, 0);
        tk::run_ticks(&mut sim, 5); // N + 33 ... N + 37: `0x71C`
        assert_eq!(
            st(&sim, a) & DUG,
            status::MASKED,
            "camouflaged without WAITING4KILLING"
        );
        assert!(
            !seen_by_1(&mut sim, a),
            "never visible to foreigners (W-167)"
        );
        // The search in N + 88 ends at `!is_down`: a new enemy stays untouched.
        let c2 = tk::spawn(&mut sim, tk::COLONIST, 1, (20, 21));
        tk::run_ticks(&mut sim, 60);
        assert_eq!(hp(&sim, c2), 160);
        assert_eq!(timers(&sim, a, msg::CHECK_TARGET), 0, "chain ended");
        assert_eq!(st(&sim, a) & DUG, status::MASKED, "stays camouflaged");
    }

    // QUIRK K-308, C-237: kept
    #[test]
    fn two_search_chains_stab_twice() {
        // C-237, re/masking 8 no. 14: two `0x11C` posted before the filter, both run in the same tick (`m_IsDown` not
        // until `0x41C`) -> two `0x41C`, two search chains. Both chains run simultaneously and find the same nearest
        // target (one `m_TargetHID`): every stab lands twice, first against the near colonist, 5 s later against the
        // second.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let (a, c, c2) = {
            let mut sim = Sim::new(&mut state, &rules);
            let a = tk::spawn(&mut sim, tk::ASSASSIN, 0, (20, 20));
            let c = tk::spawn(&mut sim, tk::COLONIST, 1, (21, 20));
            // Second colonist further away: edge distance 4600 - 512 = 4088.
            let c2 = tk::spawn(&mut sim, tk::COLONIST, 1, (30, 30));
            place_at(&mut sim, c2, a, (0, 4600));
            for _ in 0..2 {
                assert!(post_message(
                    &mut sim,
                    a,
                    Message::new(msg::GO_DOWN, &[]),
                    true
                ));
            }
            tk::run_ticks(&mut sim, 1); // tick N
            assert_eq!(
                filter(&sim, a),
                MessageFilter {
                    mode: 0,
                    codes: vec![0x21C, 0x110, 0x21C, 0x110]
                },
                "appended without clearing (K-308)"
            );
            assert_eq!(timers(&sim, a, msg::GO_DOWN_READY), 2);
            // The doubled filter is reachable and loads.
            assert_loads(sim.state, &rules);
            (a, c, c2)
        };
        let mut loaded = save_and_load(&state, &rules);
        let run = |state: &mut SimState, n: usize| -> Vec<u64> {
            let mut sim = Sim::new(state, &rules);
            (0..n)
                .map(|_| {
                    tk::run_ticks(&mut sim, 1);
                    crate::save::checksum(sim.state)
                })
                .collect::<Vec<u64>>()
        };
        assert_eq!(
            run(&mut state, 24),
            run(&mut loaded, 24),
            "sequence the same after loading"
        );
        let mut sim = Sim::new(&mut state, &rules); // after N + 24
        assert_eq!(timers(&sim, a, msg::CHECK_TARGET), 2, "two search chains");
        tk::run_ticks(&mut sim, 1); // N + 25
        assert_eq!(target(&sim, a), c);
        assert_eq!(timers(&sim, a, msg::ATTACK), 2);
        tk::run_ticks(&mut sim, 7); // N + 32
        let first = corpse(&sim, tk::COLONIST).expect("carcass");
        assert_eq!(raw_hp(&sim, first), colonist_after(2), "two stabs");
        tk::run_ticks(&mut sim, 5); // N + 37
        assert_eq!(timers(&sim, a, msg::CHECK_TARGET), 2);
        assert_eq!(hp(&sim, c2), 160);
        tk::run_ticks(&mut sim, 51); // N + 88: both searches find the second
        assert_eq!(target(&sim, a), c2);
        tk::run_ticks(&mut sim, 7); // N + 95
        let second = corpses(&sim, tk::COLONIST);
        let second = *second
            .iter()
            .find(|&&k| k != first)
            .expect("second carcass");
        assert_eq!(raw_hp(&sim, second), colonist_after(2));
    }

    #[test]
    fn a_stab_on_a_destroyed_target_does_nothing() {
        // `0x61C` fetches the target via the ID (`GetObjectByID`, also dead); if it is missing, nothing happens.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = tk::spawn(&mut sim, tk::COLONIST, 1, (21, 20));
        let a = dug_in(&mut sim, (20, 20), 26); // until N + 25: search
        assert_eq!(target(&sim, a), c);
        destroy_object(&mut sim, c);
        assert!(sim.object(c).is_none());
        let c2 = tk::spawn(&mut sim, tk::COLONIST, 1, (20, 22)); // edge distance 7680: too far
        let hps = (hp(&sim, a), hp(&sim, c2));
        assert_loads(sim.state, &rules);
        tk::run_ticks(&mut sim, 7); // N + 32: stab into the void
        assert_eq!((hp(&sim, a), hp(&sim, c2)), hps);
        tk::run_ticks(&mut sim, 5); // N + 37: the chain continues
        assert_eq!(st(&sim, a) & DUG, DUG);
        move_to(&mut sim, c2, (20, 21));
        tk::run_ticks(&mut sim, 51); // N + 88
        assert_eq!(target(&sim, a), c2);
        tk::run_ticks(&mut sim, 7); // N + 95
        assert_eq!(raw_hp_or_corpse(&sim, c2, tk::COLONIST), colonist_after(1));
    }

    // QUIRK C-234: kept
    #[test]
    fn a_zombie_stabs_for_nothing() {
        // re/masking 9 V11, C-234: zombie (slot 8, enemy of all) with damage 0. Animation 5 has only marker 1 (33 ms):
        // `0x41C` in N + 1, search N + 7, `0x61C` (marker 2 missing -> 0 ms) and `0x71C` both in N + 8.
        let rules = tk::rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let z = tk::spawn(&mut sim, tk::ZOMBIE, 8, (20, 20));
        let c = tk::spawn(&mut sim, tk::COLONIST, 0, (21, 20));
        assert!(sim.object(z).unwrap().has_module(INDEX, false));
        assert_eq!(say(&mut sim, z, msg::GO_DOWN), 0);
        tk::run_ticks(&mut sim, 1); // N
        assert_eq!(anim(&sim, z), 5);
        tk::run_ticks(&mut sim, 1); // N + 1
        assert!(is_down(&sim, z));
        tk::run_ticks(&mut sim, 5); // N + 2 ... N + 6
        let starts = sim.object(z).unwrap().anim.starts;
        tk::run_ticks(&mut sim, 1); // N + 7: search, stab animation 5 restarted
        assert_eq!(target(&sim, z), c);
        assert_eq!(anim(&sim, z), 5);
        assert_ne!(sim.object(z).unwrap().anim.starts, starts, "restart");
        assert_eq!(st(&sim, z) & DUG, status::WAITING4KILLING);
        tk::run_ticks(&mut sim, 1); // N + 8
        assert_eq!(st(&sim, z) & DUG, DUG);
        assert_eq!(hp(&sim, c), 160, "damage 0");
        tk::run_ticks(&mut sim, 60);
        assert_eq!(hp(&sim, c), 160);
        assert_eq!(st(&sim, c) & status::DIED, 0);
    }

    // QUIRK C-238: kept
    #[test]
    fn death_keeps_the_dug_in_marks() {
        // C-238, re/masking 4 "Dying": `StartDying` clears only `MASKED`.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = dug_in(&mut sim, (20, 20), 20);
        assert!(is_down(&sim, a));
        let dug_filter = filter(&sim, a);
        assert_eq!(send(&mut sim, a, &Message::new(0x110, &[])), 0);
        tk::run_ticks(&mut sim, 3);
        let body = corpse(&sim, tk::ASSASSIN).expect("carcass on slot 8");
        assert_eq!(st(&sim, body) & DUG, status::WAITING4KILLING);
        assert_eq!(filter(&sim, body), dug_filter);
        assert_loads(sim.state, &rules);
        let loaded = save_and_load(sim.state, &rules);
        let o = loaded.object(body, false).expect("carcass");
        assert_eq!(
            (o.status & DUG, &o.filter),
            (status::WAITING4KILLING, &dug_filter)
        );
    }

    // QUIRK K-307: kept
    #[test]
    fn a_disabled_dig_in_variant_freezes_the_marks() {
        // K-307, dig-in part: module 28 off (variant loss, K-300) -> `WAITING4KILLING`, `MASKED` and the filter stay,
        // `0x21C` returns -1, timers rest.
        let rules = tk::rules();
        let mut state = unlocked_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = dug_in(&mut sim, (20, 20), 20);
        assert!(is_down(&sim, a));
        let dug_filter = filter(&sim, a);
        assert_eq!(st(&sim, a) & DUG, DUG);
        // Variant 9 (upgrade 2922) drops out: the module is switched off.
        crate::variants::update_variant_mask(&mut sim, a, !(1 << 9), 0);
        assert!(
            !sim.object(a).unwrap().has_module(INDEX, false),
            "module off"
        );
        assert_eq!(say(&mut sim, a, msg::GO_UP), -1);
        assert_eq!(say(&mut sim, a, msg::GO_DOWN), -1);
        // Even a posted dig-out (the filter lets `0x21C` through) has no effect.
        post_message(&mut sim, a, Message::new(msg::GO_UP, &[]), true);
        tk::run_ticks(&mut sim, 30);
        assert_eq!(st(&sim, a) & DUG, DUG, "marks stay");
        assert_eq!(filter(&sim, a), dug_filter, "filter stays");
        assert!(is_down(&sim, a));
        assert_eq!(anim(&sim, a), 8, "dig-in animation stays");
        assert_loads(sim.state, &rules);
        // The same via merely switching the module off.
        let b = dug_in(&mut sim, (30, 30), 20);
        sim.object_mut(b).unwrap().set_module_enabled(INDEX, false);
        assert_eq!(say(&mut sim, b, msg::GO_UP), -1);
        tk::run_ticks(&mut sim, 10);
        assert_eq!(st(&sim, b) & DUG, DUG);
        assert_eq!(filter(&sim, b), dug_filter);
    }
}
