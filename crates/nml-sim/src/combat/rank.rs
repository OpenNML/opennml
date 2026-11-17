//! Experience and ranks (`GameObject_AddExperience` `0x0046D7F0`, `…SetRankResetExperience`
//! `0x0046D7B0`, `…ApplyRank` `0x0046D680`; re/combat-damage 4). Rank bonuses come directly
//! from `default.dat` (upgrade `4399 + r`, config `id + 100`).

use crate::id::ObjectId;
use crate::sim::Sim;
use crate::state::SimEvent;

pub const EXPERIENCE_UPGRADE_EFFECT: &str = "data/effects/experience_upgrade.pef";

/// `GameObject_AddExperience`: rank 5 accepts nothing more; a full experience fraction raises the
/// rank (the excess is forfeited). Max(30) = 0 (no rank init) does not change the fraction.
pub fn add_experience(sim: &mut Sim, id: ObjectId, xp: i32) {
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 30);
    let Some(o) = sim.object_mut(id) else { return };
    if o.rank == 5 {
        return;
    }
    o.attributes.add_scaled_to_current(30, xp, 10, p);
    if o.attributes.is_full(30) {
        let r = i32::from(o.rank) + 1;
        set_rank_reset_experience(sim, id, r);
    }
}

/// `GameObject_SetRankResetExperience`: base value of attribute 30 = `(r + 1) · 100`, fraction 0, then
/// `ApplyRank(r)`.
pub fn set_rank_reset_experience(sim: &mut Sim, id: ObjectId, r: i32) {
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 30);
    let Some(o) = sim.object_mut(id) else { return };
    o.attributes
        .set_modifier(30, r.wrapping_add(1).wrapping_mul(100), false, 0);
    o.attributes.set_current_value(30, 0, p);
    apply_rank(sim, id, r);
}

/// `GameObject_ApplyRank`: effect, `+500` to the hit points with the **old** maximum (C-130),
/// remove old rank bonuses, set new ones from the owner's configuration.
pub fn apply_rank(sim: &mut Sim, id: ObjectId, r: i32) {
    let Some(o) = sim.object(id) else { return };
    if r == i32::from(o.rank) {
        return;
    }
    let (old, slot, guid) = (o.rank, o.owner(), o.guid);
    // The old hit point maximum with the upgrade percentages (C-130): `+500` on it.
    let p = sim.type_upgrade_percent(slot, guid, 14);
    sim.state.events.push(SimEvent::EffectOnObject {
        object: id,
        path: EXPERIENCE_UPGRADE_EFFECT.to_string(),
    });
    let values: Vec<(u32, i32)> = (13..=34)
        .map(|a| (a, sim.rules.config_value(slot, (4399 + r) as u32, a + 100)))
        .collect();
    let o = sim.object_mut(id).expect("present");
    o.attributes.add_to_current_value(14, 500, p);
    if old > 0 {
        for a in 13..=34 {
            o.attributes.remove_modifier(a, 4399 + i32::from(old));
        }
    }
    for (a, v) in values {
        if v != 0 {
            o.attributes.set_modifier(a, v, true, 4399 + r);
        }
    }
    o.rank = r.clamp(0, 5) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;

    // QUIRK C-130: kept
    #[test]
    fn rank_up_heals_500_with_the_old_max_and_replaces_the_bonus() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let p = spawn(&mut sim, PIKEMAN, 0, (4, 4));
        set_rank_reset_experience(&mut sim, p, 0);
        sim.object_mut(p)
            .unwrap()
            .attributes
            .set_current_fraction(14, 37_037); // 100/270 HP
        sim.state.events.clear();
        add_experience(&mut sim, p, 1000);
        let o = sim.object(p).unwrap();
        assert_eq!(o.rank, 1);
        assert_eq!(o.attributes.max_value(14, 0), 283);
        assert_eq!(
            o.attributes.current_value(14, 0),
            283,
            "min(600, 270) = full, then +5 %"
        );
        assert_eq!(o.attributes.max_value(19, 0), 27);
        assert_eq!(o.attributes.max_value(30, 0), 200);
        assert_eq!(o.attributes.current_fraction(30), 0, "excess is forfeited");
        let effects: Vec<_> = sim
            .state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::EffectOnObject { object, path } => Some((*object, path.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(effects, vec![(p, EXPERIENCE_UPGRADE_EFFECT.to_string())]);
        add_experience(&mut sim, p, 2000);
        let o = sim.object(p).unwrap();
        assert_eq!(
            (
                o.rank,
                o.attributes.max_value(14, 0),
                o.attributes.max_value(19, 0)
            ),
            (2, 297, 28),
            "bonus replaced, not added"
        );
        assert_eq!(o.attributes.max_value(18, 0), 33);
        // Shadow warrior 1200 HP with 300 HP: 300 + 500 = 800 -> fraction 66,666 -> max 1260 -> 840.
        let s = spawn(&mut sim, SHADOW_WARRIOR, 0, (8, 8));
        set_rank_reset_experience(&mut sim, s, 0);
        sim.object_mut(s)
            .unwrap()
            .attributes
            .set_current_fraction(14, 25_000);
        add_experience(&mut sim, s, 1000);
        assert_eq!(sim.object(s).unwrap().attributes.current_value(14, 0), 840);
    }

    // QUIRK C-126: kept
    #[test]
    fn rank_five_takes_no_experience_and_max_zero_never_ranks() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let p = spawn(&mut sim, PIKEMAN, 0, (4, 4));
        set_rank_reset_experience(&mut sim, p, 0);
        // 5 XP per hit: 200 / 400 / 603 / 800 / 1000 hits per rank (re/combat-damage 7.2).
        let mut hits = Vec::new();
        let mut n = 0;
        while sim.object(p).unwrap().rank < 5 {
            add_experience(&mut sim, p, 5);
            n += 1;
            if sim.object(p).unwrap().rank as usize > hits.len() {
                hits.push(n);
                n = 0;
            }
        }
        assert_eq!(hits, vec![200, 400, 603, 800, 1000]);
        assert_eq!(sim.object(p).unwrap().attributes.max_value(14, 0), 405);
        add_experience(&mut sim, p, 100_000);
        assert_eq!(sim.object(p).unwrap().rank, 5);
        // Slot 8: no rank init (attack::init), Max(30) stays 0 -> no progress, no rank.
        let q = spawn(&mut sim, PIKEMAN, 8, (8, 8));
        assert_eq!(sim.object(q).unwrap().attributes.max_value(30, 0), 0);
        add_experience(&mut sim, q, 100_000);
        assert_eq!(sim.object(q).unwrap().rank, 0);
    }

    #[test]
    fn promotion_adds_500_with_the_upgraded_old_maximum() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Pikeman with +10 % armor (2810): max 297; 100/297 HP + 500 -> full.
        sim.state.type_upgrades[0]
            .entry(PIKEMAN)
            .or_default()
            .set_percent(2810, 14, 10);
        let p = spawn(&mut sim, PIKEMAN, 0, (4, 4));
        set_rank_reset_experience(&mut sim, p, 0);
        assert_eq!(sim.attribute(p, 14), 297);
        sim.object_mut(p)
            .unwrap()
            .attributes
            .set_current_value(14, 100, 10);
        add_experience(&mut sim, p, 1000);
        assert_eq!(sim.object(p).unwrap().rank, 1);
        assert_eq!(sim.attribute(p, 14), 310, "full, max 270 * 10 % + 283");
        // Shadow warrior 1200 HP with +50 % (max 1800) at 25 % = 450: 450 + 500 = 950 of 1800
        // (fraction 52,777); then rank +5 %: max 1860 -> 982 (without the percentage it would be 800/1200 -> 1240).
        sim.state.type_upgrades[0]
            .entry(SHADOW_WARRIOR)
            .or_default()
            .set_percent(9999, 14, 50);
        let s = spawn(&mut sim, SHADOW_WARRIOR, 0, (8, 8));
        set_rank_reset_experience(&mut sim, s, 0);
        sim.object_mut(s)
            .unwrap()
            .attributes
            .set_current_fraction(14, 25_000);
        assert_eq!(sim.attribute(s, 14), 450);
        add_experience(&mut sim, s, 1000);
        let a = &sim.object(s).unwrap().attributes;
        assert_eq!(a.current_fraction(14), 52_777);
        assert_eq!(sim.attribute(s, 14), 982);
        assert_eq!(a.max_value(14, 50), 1860);
    }
}
