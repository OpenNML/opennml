//! Damage message `PMSG_ENERGY_ADDHITPOINTS` (`0x17F`, `GameObject_HandleOwnMessage`
//! `0x0046D840`, disassembly `0x0046D9E5…0x0046DE5E`; re/combat-damage 1, 3).

use nml_formats::bonus::{
    ART_CANNON_BUILDING, ART_FIRE_BUILDING, ART_MELEE_ARTILLERY, ART_MELEE_MELEEARTILLERY,
    ART_MOUNTY_SWORD, ART_PEEK_MOUNTY, ART_SWORD_PEEK, ART_UNIT_BUILDING,
};

use crate::filter::matches_class;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{notify_attacked_by, send};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

/// `DamageMsg_Construct(source, 0x14)` + `DamageMsg_ToMessage`: default amount `−current(20)`, here
/// simply replaced by `amount` (all callers overwrite it).
/// Arguments (re/combat-damage 1.1): amount, source, type, status, height `+0x1A4`, 0, position (0, dead),
/// fire arrow source `Q(0x4000A)`, always 1, plague source `Q(0x810)`.
pub fn damage_message(sim: &mut Sim, source: ObjectId, amount: i32) -> Message {
    let (guid, st, y) = sim
        .object(source)
        .map_or((0, 0, 0), |o| (o.guid, o.status, o.grid.y.0));
    let fire = i32::from(matches_class(sim, source, 0x4_000A));
    let pest = i32::from(matches_class(sim, source, 0x810));
    Message::new(
        0x17F,
        &[
            amount,
            source.as_i32(),
            guid as i32,
            st as i32,
            y,
            0,
            0,
            0,
            0,
            fire,
            1,
            pest,
        ],
    )
}

/// Explicit damage message `DamageMsg` from `0x004F03D0` (dynamite effect, re/special-attacks 3.3,
/// 10.2): `0x17F` with the 12 arguments in the order of [`damage_message`], but only amount,
/// source and GUID set - status 0, height 0, no fire arrow, no plague, word 10 = 1. No
/// source object is read; if it is missing at the hit, [`on_damage`] computes without the attacker part (no bonus,
/// no building deduction, no experience; `NotifyAttackedBy` nonetheless, C-129).
pub fn explicit_damage_message(amount: i32, source: ObjectId, guid: u32) -> Message {
    Message::new(
        0x17F,
        &[
            amount,
            source.as_i32(),
            guid as i32,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            0,
        ],
    )
}

/// `GameObject_AcceptsAttackBonus` (`0x0046C950`, re/combat-damage 1.4).
pub fn accepts_attack_bonus(sim: &mut Sim, id: ObjectId, art: u32) -> bool {
    let (st, walk) = sim
        .object(id)
        .map_or((0, false), |o| (o.status, o.has_module(0, false)));
    match art {
        ART_SWORD_PEEK => matches_class(sim, id, 0x20FF),
        ART_PEEK_MOUNTY => st & status::ISMOUNTED != 0,
        ART_MOUNTY_SWORD => {
            walk && st & status::ISMOUNTED == 0
                && !matches_class(sim, id, 0x2_0000)
                && !matches_class(sim, id, 0x1_0011)
        }
        ART_CANNON_BUILDING | ART_FIRE_BUILDING => {
            matches_class(sim, id, 0x1_00FF)
                || sim.object(id).is_some_and(|o| o.has_module(3, false))
        }
        _ => false,
    }
}

fn enabled(sim: &Sim, id: ObjectId, m: usize) -> bool {
    sim.object(id).is_some_and(|o| o.has_module(m, false))
}

/// Branch `0x17F`; return value always 0. Arguments: `[0]` amount (HP, negative = damage), `[1]`
/// source, `[11]` plague source; only argc 12 is read (`DamageMsg_FromMessage`).
pub fn on_damage(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    let Some(v) = sim.object(id) else { return 0 };
    if v.status & status::ISMOUNTEDONBOAT != 0 {
        let boat = v.target;
        if !boat.is_none() && boat.0 != 0 {
            send(sim, boat, msg);
        }
        return 0;
    }
    if msg.args.len() != 12 {
        return 0; // Quirk: fields uninitialized; no existing caller produces another argc
    }
    let source = ObjectId::from_i32(msg.arg(1));
    let mut d = msg.arg(0).wrapping_mul(1000);
    if msg.arg(1) != 0 {
        if source == id {
            d /= 1000; // Quirk: self-damage / 1000 (C-120)
        } else {
            notify_attacked_by(sim, id, source); // always, also without an object (C-129)
            if sim.object(source).is_some() {
                d = attacker_part(sim, id, source, d);
            }
        }
    }
    if msg.arg(11) != 0 {
        send(sim, id, &Message::new(0x910, &[1]));
    }
    let (v_slot, guid) = sim.object(id).map_or((8, 0), |o| (o.owner(), o.guid));
    d = sim.state.combat.hit_factor(v_slot).wrapping_mul(d) / 100;
    let p = sim.type_upgrade_percent(v_slot, guid, 14);
    if let Some(o) = sim.object_mut(id) {
        o.attributes.add_scaled_to_current(14, d, 1000, p);
    }
    for (module, code) in [(3, 0x1403), (0, 0x1100)] {
        if sim.module(id, module).is_some() {
            send(sim, id, &Message::new(code, &[]));
        }
    }
    report_under_attack(sim, id);
    0
}

/// Bonuses, malus, experience and attack factor (`0x0046DA63…`), with an existing source `a`.
fn attacker_part(sim: &mut Sim, v: ObjectId, a: ObjectId, mut d: i32) -> i32 {
    let ship_vs_ship = matches_class(sim, a, 0x2_0000)
        && matches_class(sim, v, 0x2_0000)
        && matches_class(sim, a, 0x2000)
        && matches_class(sim, v, 0x2000);
    let art = send(sim, a, &Message::new(0x811, &[0]));
    let mut bonus = 0;
    if art > 0 && !ship_vs_ship {
        let art = art as u32;
        bonus = sim.rules.bonus.attack_bonus(art);
        let accepts = accepts_attack_bonus(sim, v, art);
        if (bonus > 0 && !accepts) || (bonus < 0 && accepts) {
            bonus = 0;
        }
    }
    if enabled(sim, a, 0x12) {
        let mut x = 0;
        if enabled(sim, v, 0x13) {
            let art = if enabled(sim, v, 0x12) {
                ART_MELEE_MELEEARTILLERY
            } else {
                ART_MELEE_ARTILLERY
            };
            x = sim.rules.bonus.attack_bonus(art);
        }
        bonus = bonus.wrapping_add(x);
    }
    d = bonus.wrapping_add(100).wrapping_mul(d) / 100;
    if !matches_class(sim, a, 0x2_0000) && enabled(sim, v, 3) {
        let b = sim.rules.bonus.attack_bonus(ART_UNIT_BUILDING);
        d = b.wrapping_add(100).wrapping_mul(d) / 100;
    }
    let unit_or_occupied =
        enabled(sim, v, 0) || (enabled(sim, v, 3) && matches_class(sim, v, 0x217));
    let (a_slot, v_slot) = (a.slot(), v.slot());
    if unit_or_occupied
        && !sim
            .state
            .diplomacy
            .is_friend(i32::from(a_slot), i32::from(v_slot))
    {
        let div = if matches_class(sim, a, 0x113) {
            10_000
        } else {
            1_000
        };
        crate::combat::rank::add_experience(sim, a, (d / div).wrapping_neg());
    }
    sim.state.combat.attack_factor(a_slot).wrapping_mul(d) / 100
}

/// `GameObject_ReportUnderAttack` (`0x0046CB40`): event only; the game host shows the message for
/// the active player (GUID first, otherwise 100/101).
pub fn report_under_attack(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let ev = SimEvent::UnderAttack {
        object: id,
        owner: o.owner(),
        guid: o.guid,
        building: o.has_module(3, false),
        tile: o.tile,
    };
    sim.state.events.push(ev);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::rank::set_rank_reset_experience;
    use crate::combat::testkit::*;

    struct Case {
        name: &'static str,
        attacker: u32,
        victim: u32,
        amount: i32,
        before: i32,
        factors: (i32, i32),
        after: i32,
        hp: i32,
        xp: i32,
    }

    /// re/combat-damage 7.2; fraction x 100,000; XP as experience fraction (Max(30) = 100 -> x100).
    #[test]
    fn damage_follows_the_cases_of_combat_damage_7_2() {
        let case = |name, attacker, victim, amount, before, factors, after, hp, xp| Case {
            name,
            attacker,
            victim,
            amount,
            before,
            factors,
            after,
            hp,
            xp,
        };
        let cases = [
            case(
                "1 Warrior->Pikeman",
                WARRIOR,
                PIKEMAN,
                -4,
                100_000,
                (100, 100),
                97_926,
                264,
                5,
            ),
            case(
                "1b half",
                WARRIOR,
                PIKEMAN,
                -4,
                50_000,
                (100, 100),
                47_926,
                129,
                5,
            ),
            case(
                "6 Saber->Archer",
                SABRE,
                ARCHER,
                -9,
                100_000,
                (100, 100),
                91_291,
                142,
                13,
            ),
            case(
                "6b Saber->ENG inf.",
                SABRE,
                ENG_INFANTRY,
                -9,
                100_000,
                (100, 100),
                95_910,
                264,
                11,
            ),
            case(
                "7 Saber->HQ",
                SABRE,
                HQ,
                -9,
                100_000,
                (100, 100),
                99_820,
                3993,
                0,
            ),
            case(
                "8 Handicap A",
                WARRIOR,
                PIKEMAN,
                -4,
                100_000,
                (80, 100),
                98_341,
                266,
                5,
            ),
            case(
                "8b Handicap both",
                WARRIOR,
                PIKEMAN,
                -4,
                100_000,
                (80, 80),
                98_673,
                266,
                5,
            ),
            case(
                "9 ISBOUNTY",
                WARSHIP,
                SPA_INFANTRY,
                -20,
                100_000,
                (100, 100),
                89_474,
                170,
                2,
            ),
        ];
        let rules = rules();
        for c in cases {
            let mut state = state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let a = spawn(&mut sim, c.attacker, 0, (4, 4));
            let v = spawn(&mut sim, c.victim, 1, (8, 8));
            set_rank_reset_experience(&mut sim, a, 0);
            sim.state.combat.attack_factor[0] = c.factors.0;
            sim.state.combat.hit_factor[1] = c.factors.1;
            sim.object_mut(v)
                .unwrap()
                .attributes
                .set_current_fraction(14, c.before);
            let msg = damage_message(&mut sim, a, c.amount);
            assert_eq!(msg.args.len(), 12, "{}", c.name);
            assert_eq!(crate::messaging::send(&mut sim, v, &msg), 0, "{}", c.name);
            let o = sim.object(v).unwrap();
            assert_eq!(o.attributes.current_fraction(14), c.after, "{}", c.name);
            assert_eq!(o.attributes.current_value(14, 0), c.hp, "{}", c.name);
            assert_eq!(o.last_attacker, a, "{}: NotifyAttackedBy", c.name);
            let xp = sim.object(a).unwrap().attributes.current_fraction(30) / 100;
            assert_eq!(xp, c.xp, "{}: experience", c.name);
        }
    }

    /// `DamageMsg` from `0x004F03D0` (re/special-attacks 3.3, 10.2): only amount, source and GUID; without a
    /// source object `on_damage` computes without the attacker part (no building deduction, no experience).
    #[test]
    fn the_explicit_damage_message_carries_only_source_and_guid() {
        use crate::module::ability_testkit as ak;
        let gone = ObjectId::new(999, 0, 1);
        let msg = explicit_damage_message(-4000, gone, ak::PATRIOT);
        assert_eq!(msg.code, 0x17F);
        assert_eq!(
            msg.args,
            [-4000, gone.as_i32(), 2315, 0, 0, 0, 0, 0, 0, 0, 1, 0]
        );
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let monastery = ak::spawn(&mut sim, ak::MONASTERY, 1, (20, 20));
        let bomber = ak::spawn(&mut sim, ak::PATRIOT, 0, (10, 10));
        let hp = |sim: &Sim| {
            let o = sim.object(monastery).unwrap();
            o.attributes
                .current_value(14, sim.type_upgrade_percent(1, ak::MONASTERY, 14))
        };
        assert!(sim.object(gone).is_none());
        crate::messaging::send(&mut sim, monastery, &msg);
        assert_eq!(hp(&sim), 10_000 - 4000, "exactly the amount");
        // Counter-check: with an existing source the building deduction -20% applies (-3200, re/special-attacks
        // 3.3).
        let msg = damage_message(&mut sim, bomber, -4000);
        crate::messaging::send(&mut sim, monastery, &msg);
        assert_eq!(hp(&sim), 10_000 - 4000 - 3200);
    }

    #[test]
    fn self_damage_is_divided_by_1000_and_boat_riders_forward_everything() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Case 10: self-damage -4 -> -4 milli-HP -> fraction 99,999 (quirk).
        let p = spawn(&mut sim, PIKEMAN, 0, (4, 4));
        let msg = damage_message(&mut sim, p, -4);
        crate::messaging::send(&mut sim, p, &msg);
        assert_eq!(
            sim.object(p).unwrap().attributes.current_fraction(14),
            99_999
        );
        assert!(
            sim.object(p).unwrap().last_attacker.is_none(),
            "no NotifyAttackedBy"
        );
        // Rider in a boat (0x8000): the original message goes to the connected object, nothing else.
        let rider = spawn(&mut sim, PIKEMAN, 0, (6, 6));
        let boat = spawn(&mut sim, SABRE, 0, (7, 7));
        let a = spawn(&mut sim, WARRIOR, 1, (8, 8));
        let o = sim.object_mut(rider).unwrap();
        o.status |= crate::object::status::ISMOUNTEDONBOAT;
        o.target = boat;
        sim.state.events.clear();
        let msg = damage_message(&mut sim, a, -9);
        crate::messaging::send(&mut sim, rider, &msg);
        assert_eq!(
            sim.object(rider).unwrap().attributes.current_fraction(14),
            100_000
        );
        assert!(sim.object(rider).unwrap().last_attacker.is_none());
        assert!(sim.object(boat).unwrap().attributes.current_fraction(14) < 100_000);
        let reports: Vec<_> = sim
            .state
            .events
            .iter()
            .filter(|e| matches!(e, SimEvent::UnderAttack { .. }))
            .collect();
        assert_eq!(reports.len(), 1, "only the boat reports the attack");
    }

    #[test]
    fn experience_counts_against_neutrals_only_for_0x17f() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, SABRE, 0, (4, 4));
        let neutral = spawn(&mut sim, PIKEMAN, 8, (8, 8));
        set_rank_reset_experience(&mut sim, a, 0);
        let msg = damage_message(&mut sim, a, -9);
        crate::messaging::send(&mut sim, neutral, &msg);
        assert_eq!(
            sim.object(a).unwrap().attributes.current_fraction(30),
            900,
            "9 XP against slot 8"
        );
        // 0x27F {-10 %, A}: HasEnemyRelation(0, 8) is false -> no experience.
        crate::messaging::send(&mut sim, neutral, &Message::new(0x27F, &[-10, a.as_i32()]));
        assert_eq!(sim.object(a).unwrap().attributes.current_fraction(30), 900);
        // Against an enemy 0x27F gives -d experience (27 HP), afterwards NotifyAttackedBy.
        let enemy = spawn(&mut sim, PIKEMAN, 1, (10, 10));
        sim.state.events.clear();
        crate::messaging::send(&mut sim, enemy, &Message::new(0x27F, &[-10, a.as_i32()]));
        assert_eq!(
            sim.object(a).unwrap().attributes.current_fraction(30),
            900 + 2700
        );
        assert_eq!(sim.object(enemy).unwrap().last_attacker, a);
        // Healing also reports the "attack" (ReportUnderAttack after every 0x27F).
        crate::messaging::send(&mut sim, enemy, &Message::new(0x27F, &[5]));
        let n = sim
            .state
            .events
            .iter()
            .filter(|e| matches!(e, SimEvent::UnderAttack { .. }))
            .count();
        assert_eq!(n, 2);
    }

    /// Raw fraction of the pikeman after a hit by the warrior (SWORD_PEEK +40%, factors 100,
    /// pikeman 270 HP, fraction before 100,000), recomputed with the formula from re/combat-damage 1.2
    /// in `i32` with overflow: `d = a·1000`, `d = 140·d / 100`, `Δ = d·100 000 / (270·1000)`.
    #[test]
    fn damage_wraps_like_the_original_and_never_panics() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, WARRIOR, 0, (4, 4));
        let v = spawn(&mut sim, PIKEMAN, 1, (8, 8));
        set_rank_reset_experience(&mut sim, a, 0);
        // (amount, expected raw fraction). The overflows lie at +-15,340 (140 * 1000 * a).
        let vectors = [
            // a * 1000 overflows and yields 0: no damage.
            (i32::MIN, 100_000),
            // 140 * (-15,340,000) overflows and becomes positive (+21,473,672): healing, capped at the top.
            (-15_340, 100_000),
            // Just no overflow yet: d = -21,474,600 -> delta = -7,953,555, fraction stays negative.
            (-15_339, -7_853_555),
            (-1, 99_482),
            (0, 100_000),
            (1, 100_000),
            // i32::MAX * 1000 = -1000 (overflow) -> d = -1400 -> delta = -518.
            (i32::MAX, 99_482),
        ];
        for (amount, raw) in vectors {
            sim.object_mut(v)
                .unwrap()
                .attributes
                .set_current_fraction(14, 100_000);
            let msg = damage_message(&mut sim, a, amount);
            crate::messaging::send(&mut sim, v, &msg);
            let o = sim.object(v).unwrap();
            assert_eq!(o.attributes.raw_fraction(14), raw, "amount {amount}");
            assert_eq!(
                o.attributes.current_fraction(14),
                raw.max(0),
                "amount {amount}: reading clamps to 0"
            );
        }
        // Max 0 (module-less observer without HP) does not divide by 0 and stays unchanged.
        let o = spawn(&mut sim, OBSERVER, 1, (12, 12));
        let before = sim.object(o).unwrap().attributes.raw_fraction(14);
        let msg = damage_message(&mut sim, a, -9);
        crate::messaging::send(&mut sim, o, &msg);
        assert_eq!(sim.object(o).unwrap().attributes.raw_fraction(14), before);
    }
}
