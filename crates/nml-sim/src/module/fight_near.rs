//! Module 18 `CPropertyFightNear` (re/combat-melee): one swing per order `0x112`,
//! animation choice with the table sequence, hits via timers (group `0x80`) at the time marker,
//! damage `attack(melee) * End / 2000`, end -> success `0x511` to the attack module.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{send, send_fault, send_success};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 0x12;
/// `[m_MinDist, m_MaxDist]` = `[0, Fixed(1) >> 1]` (`0x006113C0`).
pub const RANGE: (i32, i32) = (0, 2048);
const GROUP: u8 = 0x80;

/// `CPropertyFightNear` (`0x88` bytes); the savegame holds target and list (feedback messages and timers
/// are in `ModuleBase`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FightNear {
    /// `m_FightAnim` (190002).
    pub fight_anim: Vec<i32>,
    /// `m_TargetHID` (constructor 0, not -1).
    pub target: ObjectId,
}

impl Default for FightNear {
    /// `PropertyFightNear_Construct` (`0x00513DB0`).
    fn default() -> Self {
        Self {
            fight_anim: Vec::new(),
            target: ObjectId(0),
        }
    }
}

fn fight_near_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut FightNear> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::FightNear(f) => Some(f),
        _ => None,
    }
}

fn fight_near<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s FightNear> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::FightNear(f) => Some(f),
        _ => None,
    }
}

/// `PropertyFightNear_Init` (`0x00514390`); ReadParams is empty (variants do not change the list).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let text = desc.values.get(&190002).cloned().unwrap_or_default();
    let flags = desc.value_int(190001).unwrap_or(0) as u32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        m.kind = ModuleKind::FightNear(Box::new(FightNear {
            fight_anim: super::stand::parse_int_list(&text, &[]),
            target: ObjectId(0),
        }));
    }
}

fn post(sim: &mut Sim, id: ObjectId, delay: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(delay, msg, false, GROUP);
    }
}

/// `PropertyFightNear_StartSwing` (`0x00513E70`).
fn start_swing(sim: &mut Sim, id: ObjectId) {
    let r = sim.state.rng.rand(); // C-61: always first, 0...32767
    let list = fight_near(sim, id)
        .map(|f| f.fight_anim.clone())
        .unwrap_or_default();
    if list.is_empty() {
        log::warn!(
            "Melee {id:?}: empty fight animation list - abort instead of division by 0 (A7, C-70)"
        );
        abort(sim, id);
        return;
    }
    let mut anim = list[(r as usize) % list.len()];
    let st = sim.object(id).map_or(0, |o| o.status);
    if st & status::ISMOUNTED != 0 {
        anim = send(sim, id, &Message::new(0x50D, &[9, r])); // list 140006 (re/riding 2.3)
    } else if st & status::ISMOUNTEDONBOAT != 0 {
        anim = send(sim, id, &Message::new(0x50E, &[9, r]));
    }
    let guid = sim.object(id).map_or(0, |o| o.guid);
    let label = |l| sim.rules.anim_label_time_at(guid, anim, l);
    let (end, hit1, hit2) = (label(1), label(2), label(0x16));
    if end < 1 || hit1 < 1 || hit1 >= end || hit2 >= end {
        log::debug!("Melee {id:?}: faulty time labels: End {end}, Hit1 {hit1}, Hit2 {hit2}");
        abort(sim, id); // C-63
        return;
    }
    post(sim, id, end, Message::new(0x912, &[]));
    let atk = sim.attribute(id, 19);
    let d = atk.wrapping_mul(end) / 2000; // C-62
    if hit2 > 0 {
        let a = d / 2;
        post(sim, id, hit1, Message::new(0x712, &[a]));
        post(sim, id, hit2, Message::new(0x812, &[d - a])); // C-65: Hit2 < Hit1 allowed
    } else {
        post(sim, id, hit1, Message::new(0x712, &[d]));
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, true, false, false); // C-67: crossfade request 0 ignored
    }
    // C-72: mount or boat animation. `0x50D {10}` always goes to the **own** riding module
    // (13), also in the boat (there -1 if it is missing); the target `+0x2CC` sets it without a restart.
    if let Some(t) = super::walk::mount_of(sim, id) {
        let a = send(sim, id, &Message::new(0x50D, &[10]));
        if let Some(m) = sim.object_mut(t) {
            m.set_animation(a, 0, false, false, false);
        }
    }
}

/// `PropertyFightNear_ApplyHit` (`0x00514190`).
fn apply_hit(sim: &mut Sim, id: ObjectId, part: i32) {
    if sim
        .object(id)
        .is_none_or(|o| o.status & status::FIGHTING == 0)
    {
        log::debug!("Melee {id:?}: Bug?  Hitting although HS_FIGHT is not set."); // C-66
        return;
    }
    let t = fight_near(sim, id).map_or(ObjectId::NONE, |f| f.target);
    if sim.state.object(t, true).is_none() {
        return; // C-60: only dead counts as missing
    }
    let msg = crate::combat::damage::damage_message(sim, id, part.wrapping_neg());
    send(sim, t, &msg);
    let building = crate::filter::matches_class(sim, t, 0x2_0000)
        || sim.object(t).is_some_and(|o| o.has_module(3, false));
    let Some((pos, guid)) = sim.object(id).map(|o| (o.world_pos, o.guid)) else {
        return;
    };
    // C-69, C-116: sound at the position of the attacker, without a fog check.
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: if building { 7 } else { 6 },
        flags: 0x13,
        pos,
        guid,
    });
}

/// `PropertyFightNear_HandleMessage` (`0x005142F0`).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let dead = sim.object(id).is_none_or(|o| o.status & status::DIED != 0);
    if dead && msg.code != 0xB12 {
        return -1;
    }
    match msg.code {
        0x212 => {
            let t = fight_near(sim, id).map_or(ObjectId::NONE, |f| f.target);
            if msg.args.len() == 1 && msg.arg(0) == t.as_i32() {
                sim.object(id).map_or(-1, |o| {
                    let p = sim.type_upgrade_percent(o.owner(), o.guid, 19);
                    o.attributes.max_value(19, p)
                })
            } else {
                -1
            }
        }
        0x712 | 0x812 => {
            apply_hit(sim, id, msg.arg(0));
            0
        }
        0x912 => {
            send_success(sim, id, INDEX);
            0
        }
        // Writes both arguments without looking at the counter (`0x0051437B...0x00514384`).
        0xA12 => {
            msg.set_arg(0, RANGE.0);
            msg.set_arg(1, RANGE.1);
            0
        }
        0xB12 => {
            if let Some(m) = sim.module_mut(id, INDEX) {
                m.base.clear_event_group(GROUP);
            }
            0
        }
        _ => -1,
    }
}

/// `PropertyFightNear_OnMessage` (`0x00514130`): only `0x112` (C-64: no protection against a second one).
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let dead = sim.object(id).is_none_or(|o| o.status & status::DIED != 0);
    if dead && env.msg.code != 0xB12 {
        return false;
    }
    if env.msg.code == 0x112 {
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.set_success(env.success.as_ref(), env.success_target);
            m.base.set_fault(env.fault.as_ref(), env.fault_target);
        }
        if let Some(f) = fight_near_mut(sim, id) {
            f.target = ObjectId::from_i32(env.msg.arg(0));
        }
        start_swing(sim, id);
    }
    true
}

/// `PropertyFightNear_Abort` (`0x005142D0`): delete timers, fault message (C-20).
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    send_fault(sim, id, INDEX);
    true
}

/// Tick `0x005119A0`: only `UpdateEvents`.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::state::SimEvent;

    /// Attacker and target center to center 3072 (edge distance 2048), both face each other.
    fn duel(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
        a: u32,
        t: u32,
    ) -> (ObjectId, ObjectId) {
        let mut sim = Sim::new(state, rules);
        let x = (5 << 12) + 0x800;
        let mk = |sim: &mut Sim, guid, slot, fx: i32| {
            crate::world::create_object(
                sim,
                &mut crate::world::ObjectDesc::new(
                    guid,
                    slot,
                    [fx as f32 / 32.0, 0.0, x as f32 / 32.0],
                ),
            )
            .unwrap()
        };
        let att = mk(&mut sim, a, 0, x);
        let tgt = mk(&mut sim, t, 1, x + 3072);
        sim.object_mut(att).unwrap().direction = crate::fx::Fx(0);
        sim.object_mut(tgt).unwrap().direction = crate::fx::Fx(180 << 12);
        // Attack like command 0x11: 0x111 into the FIFO (with abort).
        crate::messaging::post_message(
            &mut sim,
            att,
            Message::new(0x111, &[tgt.as_i32(), 0]),
            true,
        );
        (att, tgt)
    }

    /// (tick, fraction) at every change of the HP of the target and the tick S of the first swing.
    fn record(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
        att: ObjectId,
        tgt: ObjectId,
        steps: usize,
    ) -> (u32, Vec<(u32, i32)>) {
        let mut s = None;
        let mut hits = Vec::new();
        let mut last = 100_000;
        for _ in 0..steps {
            crate::tick::step(state, rules, &[]);
            let sim = Sim::new(state, rules);
            if s.is_none()
                && sim
                    .object(att)
                    .is_some_and(|o| o.status & crate::object::status::FIGHTING != 0)
            {
                s = Some(sim.state.tick);
            }
            let Some(o) = sim.state.object(tgt, false) else {
                break;
            };
            let f = o.attributes.current_fraction(14);
            if f != last {
                hits.push((sim.state.tick, f - last));
                last = f;
            }
        }
        (s.expect("swing begun"), hits)
    }

    // QUIRK C-14: kept
    #[test]
    fn sabre_against_warrior_hits_at_s_plus_4_then_every_13_ticks() {
        // re/combat-melee 8.1/8.2: saber End 1200, Hit1 320 -> D = 18*1200/2000 = 10; period 13.
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules, SABRE, WARRIOR);
        let (s, hits) = record(&mut state, &rules, a, t, 2 * 120);
        assert!(hits.len() >= 8);
        assert_eq!(hits[0], (s + 4, -6250), "10 HP of 160");
        for w in hits.windows(2) {
            assert_eq!(w[1].0 - w[0].0, 13, "period (floor(End/100) + 1)");
            assert_eq!(w[1].1, -6250);
        }
    }

    // QUIRK C-62: kept
    #[test]
    fn warrior_against_pikeman_splits_hits_with_the_sword_bonus() {
        // Warrior End 1283, Hit1 560, Hit2 1096 -> D = 5: 2 at S+6, 3 at S+11; +40 % SWORD_PEEK.
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules, WARRIOR, PIKEMAN);
        let (s, hits) = record(&mut state, &rules, a, t, 2 * 40);
        assert_eq!(
            &hits[..4],
            &[
                (s + 6, -1037),
                (s + 11, -1555),
                (s + 19, -1037),
                (s + 24, -1555)
            ]
        );
    }

    // QUIRK C-61, C-63: kept
    #[test]
    fn every_swing_draws_once_from_the_table_and_bad_labels_abort() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules, CONTINENTAL, WARRIOR);
        let before = state.rng.index();
        for _ in 0..4 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        assert_ne!(
            state.rng.index(),
            before,
            "draw even with invalid time markers (C-61)"
        );
        let sim = Sim::new(&mut state, &rules);
        // No Hit1 -> abort -> 0x411 -> attack ended; never damage (C-63).
        assert_eq!(
            sim.object(t).unwrap().attributes.current_fraction(14),
            100_000
        );
        let target = crate::module::attack::attack(&sim, a).map(|x| x.target);
        assert!(target.is_some_and(|x| x.is_none() || x == t));
    }

    // QUIRK C-60: kept
    #[test]
    fn a_target_dying_mid_swing_takes_no_more_damage_and_the_attack_ends() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules, WARRIOR, PIKEMAN);
        // Run until after Hit1 (S + 6), then let the target die (DIED set).
        let (_, hits) = record(&mut state, &rules, a, t, 2 * 8);
        assert_eq!(hits.len(), 1);
        Sim::new(&mut state, &rules).object_mut(t).unwrap().status |= crate::object::status::DIED;
        let f = state
            .object(t, false)
            .unwrap()
            .attributes
            .current_fraction(14);
        for _ in 0..2 * 15 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        assert_eq!(
            state
                .object(t, false)
                .unwrap()
                .attributes
                .current_fraction(14),
            f,
            "Hit2 does not hit the dead (C-60)"
        );
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(
            crate::module::attack::attack(&sim, a).unwrap().target,
            ObjectId::NONE
        );
    }

    // QUIRK C-69: kept
    #[test]
    fn hits_play_sound_6_or_7_at_the_attacker_with_flags_0x13() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, _t) = duel(&mut state, &rules, SABRE, WARRIOR);
        let mut sounds = Vec::new();
        for _ in 0..2 * 8 {
            for e in crate::tick::step(&mut state, &rules, &[]) {
                if let SimEvent::Sound3d {
                    object,
                    action,
                    flags,
                    guid,
                    ..
                } = e
                {
                    sounds.push((object, action, flags, guid));
                }
            }
        }
        assert!(sounds.contains(&(a, 6, 0x13, SABRE)), "{sounds:?}");
    }

    #[test]
    fn range_answer_and_cancel() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, SABRE, 0, (4, 4));
        let mut m = Message::new(0xA12, &[-1, -1]);
        assert_eq!(crate::messaging::send_mut(&mut sim, a, &mut m), 0);
        assert_eq!((m.arg(0), m.arg(1)), RANGE);
        assert_eq!(
            crate::messaging::send(&mut sim, a, &Message::new(0x512, &[])),
            -1,
            "GETFIGHTANIM dead (C-71)"
        );
        assert_eq!(
            crate::messaging::send(&mut sim, a, &Message::new(0xB12, &[])),
            0
        );
    }
}
