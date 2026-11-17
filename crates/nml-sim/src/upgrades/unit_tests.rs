//! Tests of the type table, of apply/remove/revoke and of the variant switch (re/upgrades 3.2-3.4, 4.4, 9).
//! The scenes with `UPGRADE_GOLDEN` follow in `tests.rs`.

use super::testkit::*;
use super::*;
use crate::message::Message;
use crate::messaging::send;
use crate::module::ModuleKind;
use crate::object::status;

/// Percentage of attribute 32 (wood rate) for colonists of the slot.
fn wood_percent(sim: &Sim, slot: u8) -> i32 {
    sim.type_upgrade_percent(slot, COLONIST, 32)
}

fn count(sim: &Sim, slot: usize, guid: u32) -> i32 {
    sim.state.guid_counts[slot].get(&guid).copied().unwrap_or(0)
}

fn prop_bits(sim: &Sim, id: ObjectId, index: usize) -> u32 {
    sim.module(id, index)
        .expect("module present")
        .base
        .prop_bits
}

fn enabled(sim: &Sim, id: ObjectId, index: usize) -> bool {
    sim.module(id, index).expect("module present").base.enabled
}

/// Module indices of the colonist: walk, construction, stand, lumbering, shot, dying, attack,
/// melee, ranged combat.
const WALK: usize = 0;
const CONSTRUCT: usize = 1;
const STAND: usize = 2;
const LUMBER: usize = 8;
const SHOOT: usize = 10;
const DYING: usize = 16;
const ATTACK: usize = 17;
const FIGHT_NEAR: usize = 18;
const FIGHT_FAR: usize = 19;

#[test]
fn percentages_add_per_type_and_overwrite_per_guid() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    apply_upgrade(&mut sim, 0, 2800, false);
    assert_eq!(wood_percent(&sim, 0), 25);
    apply_upgrade(&mut sim, 0, 2801, false);
    assert_eq!(wood_percent(&sim, 0), 55, "additive per type");
    // K-69: the map overwrites per GUID, counter and list take duplicates.
    apply_upgrade(&mut sim, 0, 2800, false);
    assert_eq!(wood_percent(&sim, 0), 55);
    assert_eq!(count(&sim, 0, 2800), 2);
    assert_eq!(sim.state.applied_upgrades[0], vec![2800, 2801, 2800]);
    // Only the slot and only the type of the upgrade.
    assert_eq!(wood_percent(&sim, 1), 0);
    assert_eq!(sim.type_upgrade_percent(0, PIKEMAN, 32), 0);
    assert_eq!(sim.type_upgrade_percent(0, COLONIST, 14), 0);
    // K-281: remove deletes the entry despite the duplicate, the list loses the first
    // occurrence.
    remove_upgrade(&mut sim, 0, 2800);
    assert_eq!(wood_percent(&sim, 0), 30);
    assert_eq!(count(&sim, 0, 2800), 1);
    assert_eq!(sim.state.applied_upgrades[0], vec![2801, 2800]);
}

#[test]
fn slot_8_writes_the_table_but_reads_zero() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let counts = sim.state.guid_counts.clone();
    apply_upgrade(&mut sim, 8, 2800, false);
    // W-213: entry in the table of slot 8, read only for 0-7.
    let table = &sim.state.type_upgrades[8][&COLONIST];
    assert_eq!(table.sum_percent(32), 25);
    assert_eq!(wood_percent(&sim, 8), 0);
    assert_eq!(sim.state.guid_counts, counts, "counter only for 0-7");
    assert_eq!(sim.state.applied_upgrades[8], vec![2800]);
    // Slots 9/10: list yes, table never; from 11 and negative nothing (A7).
    apply_upgrade(&mut sim, 10, 2800, false);
    assert_eq!(sim.state.applied_upgrades[10], vec![2800]);
    let before = sim.state.clone();
    apply_upgrade(&mut sim, 11, 2800, false);
    apply_upgrade(&mut sim, -1, 2800, false);
    remove_upgrade(&mut sim, 8, 2800);
    assert_eq!(*sim.state, before);
}

#[test]
fn incremental_counts_and_marks_without_list_or_variants() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let c = spawn(&mut sim, COLONIST, 1, (10, 10));
    let other = spawn(&mut sim, COLONIST, 0, (12, 10));
    let bits: Vec<u32> = (0..20)
        .filter_map(|n| sim.module(c, n).map(|m| m.base.prop_bits))
        .collect();
    // W-214: counter +1, no list, flag on the object, no variants.
    apply_upgrade(&mut sim, 1, 3374, true);
    assert_eq!(count(&sim, 1, 3374), 1);
    assert!(sim.state.applied_upgrades[1].is_empty());
    assert_eq!(sim.object(c).unwrap().show_counter_upgrade, 1);
    assert_eq!(sim.object(other).unwrap().show_counter_upgrade, 0);
    let after: Vec<u32> = (0..20)
        .filter_map(|n| sim.module(c, n).map(|m| m.base.prop_bits))
        .collect();
    assert_eq!(after, bits);
    revoke_upgrade_all_players(&mut sim, 3374);
    assert_eq!(count(&sim, 1, 3374), 0);
    assert_eq!(sim.object(c).unwrap().show_counter_upgrade, 0);
    // Second revocation: counter 0 stays 0, the flag does not fall below 0.
    revoke_upgrade_all_players(&mut sim, 3374);
    assert_eq!(count(&sim, 1, 3374), 0);
    assert_eq!(sim.object(c).unwrap().show_counter_upgrade, 0);
}

#[test]
fn revoking_removes_the_percentages_of_every_player() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // 3375 weakness against slots 0 and 1 (HP -40% for colonist, pikeman, archer).
    for p in [0, 1] {
        apply_upgrade(&mut sim, p, 3375, true);
    }
    for ty in [COLONIST, PIKEMAN, ARCHER] {
        assert_eq!(sim.type_upgrade_percent(1, ty, 14), -40);
    }
    revoke_upgrade_all_players(&mut sim, 3375);
    for p in [0u8, 1] {
        assert_eq!(count(&sim, usize::from(p), 3375), 0);
        for ty in [COLONIST, PIKEMAN, ARCHER] {
            assert_eq!(sim.type_upgrade_percent(p, ty, 14), 0);
        }
    }
}

#[test]
fn removing_without_apply_makes_the_count_negative() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // W-215: `0x3A` without a previous apply.
    remove_upgrade(&mut sim, 0, 3374);
    assert_eq!(count(&sim, 0, 3374), -1);
    assert_eq!(crate::economy::guid_count(&sim, 0, 3374), -1);
}

#[test]
fn an_upgrade_switches_sites_passengers_and_dying_objects_of_the_slot() {
    use crate::module::quarter::{accept_unit, flags::FREE_FOR_QUARTER};
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tower = spawn(&mut sim, TOWER, 0, (10, 10));
    let tower_site = site(&mut sim, TOWER, 0, (30, 30));
    let outside = spawn(&mut sim, COLONIST, 0, (12, 14));
    let inside = spawn(&mut sim, COLONIST, 0, (14, 10));
    let dying = spawn(&mut sim, COLONIST, 0, (16, 10));
    let pikeman = spawn(&mut sim, PIKEMAN, 0, (20, 10));
    let foreign = spawn(&mut sim, COLONIST, 1, (18, 10));
    assert_eq!(accept_unit(&mut sim, tower, inside), 0);
    assert!(sim.object(inside).unwrap().status & status::QUARTERED != 0);
    // All colonists armed so that bit 7 (2806 and 486) is unambiguous.
    for id in [outside, inside, dying, foreign] {
        set_armed(&mut sim, id, true);
    }
    // Dying (`0x110`): `DIED`, all modules except 0/0x10/0x15/0x2F off (pending list, taken over directly
    // here); the object stays in the table of slot 0 until the change of owner.
    for id in [dying, pikeman] {
        send(&mut sim, id, &Message::new(0x110, &[]));
        sim.object_mut(id)
            .unwrap()
            .apply_module_activation_changes();
        assert!(sim.object(id).unwrap().status & status::DIED != 0);
        assert_eq!(id.slot(), 0);
        assert!(!enabled(&sim, id, STAND), "stand off when dying");
    }
    apply_upgrade(&mut sim, 0, 2806, false);
    // Outside: variant 7 (armed with 2806).
    for n in [WALK, STAND, DYING, ATTACK, FIGHT_FAR, SHOOT] {
        assert_eq!(
            prop_bits(&sim, outside, n),
            1 << 7,
            "module {n} in variant 7"
        );
        assert!(enabled(&sim, outside, n), "module {n} switched on");
    }
    for n in [CONSTRUCT, LUMBER, FIGHT_NEAR] {
        assert!(
            !enabled(&sim, outside, n),
            "module {n} off without variant 7"
        );
    }
    // Quartered and dying ones are included in the calculation - but their filter terms 485/486 fail at
    // the pre-check of `GameObject_MatchesFilter` (QUARTERED without `DONOTIGNOREQUARTEREDUNITS`, DIED
    // without `RETURN_DIED`, re/match-filter): neither bit 6 nor bit 7, all modules with variants go off,
    // also walk (a new quirk).
    for id in [inside, dying] {
        let mask = crate::variants::player_variant_mask(&mut sim, id);
        assert_eq!(mask & (1 << 6 | 1 << 7), 0);
        for n in [
            WALK, CONSTRUCT, STAND, LUMBER, SHOOT, DYING, ATTACK, FIGHT_NEAR, FIGHT_FAR,
        ] {
            assert!(
                !enabled(&sim, id, n),
                "module {n} off without a matching variant"
            );
        }
    }
    // K-282: modules with a matching variant are switched on, also on the dying one - the pikeman
    // (modules without a mask) gets its stand back with 2810, also in the tick list.
    apply_upgrade(&mut sim, 0, 2810, false);
    assert!(enabled(&sim, pikeman, STAND));
    sim.object_mut(pikeman)
        .unwrap()
        .apply_module_activation_changes();
    assert!(
        sim.object(pikeman)
            .unwrap()
            .tick_list
            .contains(&(STAND as u8))
    );
    // Construction site: vf+0x20 of the quarters recomputes the capacity (2817), `FREEFORQUARTER` only on
    // the finished tower.
    apply_upgrade(&mut sim, 0, 2817, false);
    for id in [tower, tower_site] {
        match &sim.module(id, 23).unwrap().kind {
            ModuleKind::Quarter(q) => assert_eq!(q.max, 4),
            _ => panic!("module 23 is not quarters"),
        }
    }
    let free =
        |sim: &Sim, id| sim.module(id, 23).unwrap().base.flags & (FREE_FOR_QUARTER & !0xFF) != 0;
    assert!(free(&sim, tower) && !free(&sim, tower_site));
    // Foreign slot: unchanged although armed.
    for n in [WALK, STAND, ATTACK] {
        assert_eq!(prop_bits(&sim, foreign, n), 1 << 6);
    }
    assert!(!enabled(&sim, foreign, FIGHT_FAR) && enabled(&sim, foreign, FIGHT_NEAR));
}

#[test]
fn types_without_obj_upgrades_keep_their_variants() {
    // K-70: type 1102 with stand in the variants 9 (with 2800) and 10 (without), but not in
    // `OBJ_UPGRADES` of 2800.
    let types = format!(
        "{TYPES}GUID 1102\nNAME SPA_Kolonistin\nVALUEID 100 6 1 0\n\
         PROPID 2 9\nVALUEID 30001 1 1 1\nPROPID 2 10\nVALUEID 30001 1 1 0\n"
    );
    let mut rules = rules_from(&types);
    rules
        .variant_rules
        .extend(nml_formats::gifcon::parse_variant_rules(
            "BIT 9 = 2800\nBIT 10 = -2800\n",
        ));
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let woman = spawn(&mut sim, 1102, 0, (10, 10));
    assert_eq!(prop_bits(&sim, woman, STAND), 1 << 10);
    apply_upgrade(&mut sim, 0, 2800, false);
    assert_eq!(prop_bits(&sim, woman, STAND), 1 << 10, "no switch (K-70)");
    // Not until a variant switch of its own does it take the new mask.
    crate::variants::update_variant_mask(&mut sim, woman, u32::MAX, 0);
    assert_eq!(prop_bits(&sim, woman, STAND), 1 << 9);
}

#[test]
fn an_owner_change_does_not_recompute_variants() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let c = spawn(&mut sim, COLONIST, 1, (10, 10));
    set_armed(&mut sim, c, true);
    apply_upgrade(&mut sim, 1, 2806, false);
    assert_eq!(prop_bits(&sim, c, FIGHT_FAR), 1 << 7);
    // K-283: handover to slot 0 (without 2806) keeps the variants of the old owner.
    let new = crate::world::change_owner(&mut sim, c, 0).unwrap();
    for n in [WALK, ATTACK, FIGHT_FAR] {
        assert_eq!(prop_bits(&sim, new, n), 1 << 7);
    }
    assert!(enabled(&sim, new, FIGHT_FAR));
    let mask = crate::variants::player_variant_mask(&mut sim, new);
    assert_eq!(mask & (1 << 7), 0, "recomputed it would be variant 6");
}

#[test]
fn tower_capacity_and_walk_speed_follow_the_upgrade() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tower = spawn(&mut sim, TOWER, 0, (10, 10));
    let c = spawn(&mut sim, COLONIST, 0, (14, 10));
    let capacity = |sim: &Sim| match &sim.module(tower, 23).unwrap().kind {
        ModuleKind::Quarter(q) => q.max,
        _ => panic!("module 23 is not quarters"),
    };
    let default_speed = |sim: &Sim| match &sim.module(c, WALK).unwrap().kind {
        ModuleKind::Walk(w) => w.speed.default,
        _ => panic!("module 0 is not walk"),
    };
    let unit = crate::motion::speed::TIME_UNIT;
    assert_eq!(capacity(&sim), 2);
    assert_eq!(default_speed(&sim), crate::fx::fixed_div(145, unit));
    // 2817: 2 + 2*100/100 = 4 via vf+0x20 of the quarters.
    apply_upgrade(&mut sim, 0, 2817, false);
    assert_eq!(capacity(&sim), 4);
    // 3120: 145 + floor(145*30/100) = 188 as walk base speed (re/upgrades 9).
    apply_upgrade(&mut sim, 0, 3120, false);
    assert_eq!(sim.attribute(c, 16), 188);
    assert_eq!(default_speed(&sim), crate::fx::fixed_div(188, unit));
}

#[test]
fn fire_protection_switches_buildings_and_draws_one_random_value_each() {
    // Animated building (ISANIMATED 0x40000) with the variants 2 (without extinguishing) and 1 (with
    // 2813), like the 96 building types of the shipped data (re/upgrades 3.4).
    const MILL: u32 = 1002;
    let anims = [4, 5, 6, 7, 8, 9, 10];
    let types = format!(
        "{TYPES}GUID 1002\nNAME SPA_Muehle\nVALUEID 100 6 1 0\n\
         PROPID 3 2\nVALUEID 40003 6 1 262144\nVALUEID 40004 1 1 4 5 6 7 8 9 10\n\
         PROPID 3 1\nVALUEID 40003 6 1 262144\nVALUEID 40004 1 1 4 5 6 7 8 9 10\n"
    );
    let mut rules = rules_from(&types);
    add_building_shape(&mut rules, MILL, 100.0);
    rules.defaults.insert(MILL, [(13, 30), (14, 1500)].into());
    rules.upgrade_types.insert(2813, vec![MILL]);
    rules
        .variant_rules
        .extend(nml_formats::gifcon::parse_variant_rules(
            "BIT 1 = 2813\nBIT 2 = -2813\n",
        ));
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let first = spawn(&mut sim, MILL, 0, (10, 10));
    let building_site = site(&mut sim, MILL, 0, (20, 10));
    let second = spawn(&mut sim, MILL, 0, (30, 10));
    let foreign = spawn(&mut sim, MILL, 1, (40, 10));
    assert!(first.index() < building_site.index() && building_site.index() < second.index());
    for id in [first, building_site, second, foreign] {
        assert_eq!(prop_bits(&sim, id, 3), 1 << 2);
    }
    let mut expected = sim.state.rng;
    let draws = [expected.rand(), expected.rand()];
    apply_upgrade(&mut sim, 0, 2813, false);
    // Per finished building of the slot one table random value in table order; the construction site
    // switches without randomness, the foreign building not at all.
    assert_eq!(sim.state.rng, expected);
    for (id, r) in [(first, draws[0]), (second, draws[1])] {
        assert_eq!(prop_bits(&sim, id, 3), 1 << 1);
        let want = anims[r.rem_euclid(anims.len() as i32) as usize];
        assert_eq!(sim.object(id).unwrap().anim.wanted, want);
    }
    assert_eq!(prop_bits(&sim, building_site, 3), 1 << 1);
    assert_eq!(prop_bits(&sim, foreign, 3), 1 << 2);
}

#[test]
fn objects_of_a_slot_follow_the_table_order() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, COLONIST, 0, (10, 10));
    let b = spawn(&mut sim, COLONIST, 0, (12, 10));
    let c = spawn(&mut sim, COLONIST, 0, (14, 10));
    spawn(&mut sim, COLONIST, 1, (16, 10));
    crate::world::destroy_object(&mut sim, b);
    assert_eq!(objects_of_slot(sim.state, 0), vec![a, c]);
    assert!(objects_of_slot(sim.state, 11).is_empty());
}

// --- Percentages at the other readers --------------------------------------------------------

/// Percentage of the attribute `attribute` of the object for its owner.
fn percent_of(sim: &Sim, id: ObjectId, attribute: u32) -> i32 {
    let o = sim.object(id).expect("object present");
    sim.type_upgrade_percent(o.owner(), o.guid, attribute)
}

/// `(current, maximum)` of the hit points with the upgrade percentages.
fn hit_points(sim: &Sim, id: ObjectId) -> (i32, i32) {
    let p = percent_of(sim, id, 14);
    let a = &sim.object(id).expect("object present").attributes;
    (a.current_value(14, p), a.max_value(14, p))
}

#[test]
fn hitpoints_follow_armour_upgrades_proportionally() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = spawn(&mut sim, PIKEMAN, 0, (10, 10));
    assert_eq!(hit_points(&sim, p), (270, 270));
    apply_upgrade(&mut sim, 0, 2810, false);
    assert_eq!(
        hit_points(&sim, p),
        (297, 297),
        "new objects start full, +10%"
    );
    apply_upgrade(&mut sim, 0, 2811, false);
    assert_eq!(hit_points(&sim, p), (337, 337), "+25%");
    apply_upgrade(&mut sim, 0, 2812, false);
    assert_eq!(hit_points(&sim, p), (405, 405), "+50%");
    // Injured: the fraction stays, the hit points grow with the maximum (pikeman 200/270).
    let q = spawn(&mut sim, PIKEMAN, 1, (12, 10));
    sim.object_mut(q)
        .unwrap()
        .attributes
        .set_current_value(14, 200, 0);
    assert_eq!(
        sim.object(q).unwrap().attributes.current_fraction(14),
        74_074
    );
    assert_eq!(hit_points(&sim, q), (200, 270));
    apply_upgrade(&mut sim, 1, 2810, false);
    assert_eq!(hit_points(&sim, q), (220, 297), "fraction 74,074 of 297");
    assert_eq!(sim.attribute(q, 14), 220);
}

#[test]
fn rank_and_type_percent_truncate_separately() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Rank 1 (own modifier 4400: +5% -> 7) and type percentage 2910 (+10% -> 15): 155 + 7 + 15.
    let a = spawn(&mut sim, ARCHER, 0, (10, 10));
    crate::combat::rank::apply_rank(&mut sim, a, 1);
    assert_eq!(sim.object(a).unwrap().rank, 1);
    apply_upgrade(&mut sim, 0, 2910, false);
    assert_eq!(
        hit_points(&sim, a).1,
        177,
        "computed together (155 * 15 / 100 = 23) it would be 178"
    );
    // Without rank: 3375 (-40%: 155 * -40 / 100 = -62) -> 93; together with 2910 P = -30 (-46) -> 109.
    let b = spawn(&mut sim, ARCHER, 1, (12, 10));
    apply_upgrade(&mut sim, 1, 3375, false);
    assert_eq!(percent_of(&sim, b, 14), -40);
    assert_eq!(hit_points(&sim, b), (93, 93));
    apply_upgrade(&mut sim, 1, 2910, false);
    assert_eq!(percent_of(&sim, b, 14), -30);
    assert_eq!(hit_points(&sim, b), (109, 109));
}

#[test]
fn regeneration_uses_the_type_percent() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Without upgrade: rate 1 (scale 10) on max 100 -> 100 per object tick, full after 1000 ticks.
    let plain = spawn(&mut sim, PIKEMAN, 1, (12, 10));
    // With 3023 (115: +50%) -> max 150, 66 per object tick (re/upgrades 9), full after 1516 ticks.
    apply_upgrade(&mut sim, 0, 3023, false);
    let boosted = spawn(&mut sim, PIKEMAN, 0, (10, 10));
    for id in [plain, boosted] {
        let a = &mut sim.object_mut(id).unwrap().attributes;
        a.set_regen_rate(15, 1);
        a.set_current_fraction(15, 0);
    }
    assert_eq!(percent_of(&sim, boosted, 15), 50);
    assert_eq!(
        sim.object(boosted).unwrap().attributes.max_value(15, 50),
        150
    );
    crate::tick::object_tick(&mut sim, plain, 100);
    crate::tick::object_tick(&mut sim, boosted, 100);
    let fraction = |sim: &Sim, id| sim.object(id).unwrap().attributes.raw_fraction(15);
    assert_eq!(fraction(&sim, plain), 100);
    assert_eq!(fraction(&sim, boosted), 66, "100,000 / (150 * 10 / 1)");
    for _ in 1..1000 {
        crate::tick::object_tick(&mut sim, plain, 100);
    }
    assert_eq!(fraction(&sim, plain), 100_000, "full after 1000 ticks");
    for _ in 1..1515 {
        crate::tick::object_tick(&mut sim, boosted, 100);
    }
    assert_eq!(
        fraction(&sim, boosted),
        99_990,
        "not yet full after 1515 ticks"
    );
    crate::tick::object_tick(&mut sim, boosted, 100);
    assert_eq!(fraction(&sim, boosted), 100_000, "full after 1516 ticks");
}

#[test]
fn termites_destroy_a_building_with_one_hitpoint() {
    // W-232: the counter-upgrade 3376 (termites, -60% hit points) lowers the maximum of a
    // headquarters from 4000 to 1600; the fraction of 1 HP rounds to 0, `CheckDeath` sends 0x3703.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = spawn(&mut sim, HQ, 1, (20, 20));
    sim.object_mut(hq)
        .unwrap()
        .attributes
        .set_current_value(14, 1, 0);
    assert_eq!(hit_points(&sim, hq), (1, 4000));
    crate::tick::object_tick(&mut sim, hq, 100);
    assert_eq!(
        sim.object(hq).unwrap().status & status::DIED,
        0,
        "alive with 1 HP"
    );
    apply_upgrade(&mut sim, 1, 3376, true);
    assert_eq!(hit_points(&sim, hq), (0, 1600));
    assert_eq!(sim.object(hq).unwrap().status & status::DIED, 0);
    crate::tick::object_tick(&mut sim, hq, 100);
    assert_ne!(
        sim.object(hq).unwrap().status & status::DIED,
        0,
        "0x3703 destroys the building in the next object tick"
    );
}

#[test]
fn energy_to_hitpoints_uses_both_percentages() {
    // 0xB7F: v = attribute 15 of the source with its P, HP += v with the P of the receiver.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    apply_upgrade(&mut sim, 0, 3023, false); // Source: extra energy +50% -> 150
    apply_upgrade(&mut sim, 1, 2810, false); // Receiver: hit points +10% -> 297
    let source = spawn(&mut sim, PIKEMAN, 0, (10, 10));
    let target = spawn(&mut sim, PIKEMAN, 1, (12, 10));
    sim.object_mut(target)
        .unwrap()
        .attributes
        .set_current_fraction(14, 0);
    assert_eq!(sim.attribute(source, 15), 150);
    send(&mut sim, target, &Message::new(0xB7F, &[source.as_i32()]));
    let s = &sim.object(source).unwrap().attributes;
    assert_eq!(
        s.current_fraction(15),
        0,
        "extra energy of the source emptied"
    );
    assert_eq!(
        sim.object(target).unwrap().attributes.current_fraction(14),
        50_505,
        "150 of 297 (without P it would be 150 of 270 = 55,555)"
    );
    assert_eq!(hit_points(&sim, target), (150, 297));
}

#[test]
fn queue_entries_from_slot_11_are_dropped() {
    let rules = rules();
    let mut state = state(&rules);
    // Outside `u8` not queued at all; slot 11 queued (like the original), discarded when processed
    // (A7, W-220); slot 10 acts (list yes, counter no).
    queue_upgrade(&mut state, -1, 2800, true);
    queue_upgrade(&mut state, 256, 2800, true);
    assert!(state.upgrade_queue.is_empty());
    queue_upgrade(&mut state, 11, 2800, true);
    queue_upgrade(&mut state, 11, 2800, false);
    queue_upgrade(&mut state, 10, 2801, true);
    assert_eq!(state.upgrade_queue.len(), 3);
    let counts = state.guid_counts.clone();
    let mut sim = Sim::new(&mut state, &rules);
    process_upgrade_queue(&mut sim);
    assert!(sim.state.upgrade_queue.is_empty());
    assert_eq!(sim.state.guid_counts, counts);
    assert_eq!(sim.state.applied_upgrades[10], vec![2801]);
    for p in 0..10 {
        assert!(sim.state.applied_upgrades[p].is_empty(), "slot {p}");
    }
    // FIFO: apply, apply, remove on slot 0.
    queue_upgrade(sim.state, 0, 2800, true);
    queue_upgrade(sim.state, 0, 2801, true);
    queue_upgrade(sim.state, 0, 2800, false);
    process_upgrade_queue(&mut sim);
    assert_eq!(sim.state.applied_upgrades[0], vec![2801]);
    assert_eq!((count(&sim, 0, 2800), count(&sim, 0, 2801)), (0, 1));
}

/// The six time-of-day GUIDs `4500 + t + 10k` (k inner).
fn daytime_guids(t: u32) -> Vec<u32> {
    (0..6).map(|k| 4500 + t + 10 * k).collect()
}

#[test]
fn the_level_start_applies_six_daytime_upgrades_per_slot() {
    use crate::setup::{MatchSetup, new_match};
    // Level start via `setup::new_match` (the rest of the test world is built via
    // `SimState::new` and has no time-of-day entries).
    let start = |t: u32| {
        let mut rules = rules();
        rules.level.level_resources.insert(40, Some(t as i32));
        let state = new_match(&rules, &MatchSetup::default_for(&rules)).unwrap();
        (rules, state)
    };
    for t in [0, 2] {
        let (rules, mut state) = start(t);
        assert_eq!(state.daytime, t as i32);
        let sim = Sim::new(&mut state, &rules);
        for p in 0..8 {
            for &g in &daytime_guids(t) {
                assert_eq!(count(&sim, p, g), 1, "slot {p}, GUID {g}");
            }
        }
        // Lists 0-8 (slot outer, k inner), 9 and 10 empty.
        for p in 0..=8 {
            assert_eq!(sim.state.applied_upgrades[p], daytime_guids(t), "slot {p}");
        }
        assert!(sim.state.applied_upgrades[9].is_empty());
        assert!(sim.state.applied_upgrades[10].is_empty());
    }
    // Change 2 -> 1: on 0-7 the old ones removed, the new ones created; slot 8 keeps the old six
    // (removing only 0-7) and gets the new ones in addition.
    let (rules, mut state) = start(2);
    let mut sim = Sim::new(&mut state, &rules);
    apply_daytime_upgrades(&mut sim, 1);
    assert_eq!(sim.state.daytime, 1);
    for p in 0..8 {
        for k in 0..6 {
            assert_eq!(count(&sim, p, 4502 + 10 * k), 0, "slot {p}");
            assert_eq!(count(&sim, p, 4501 + 10 * k), 1, "slot {p}");
        }
        assert_eq!(sim.state.applied_upgrades[p], daytime_guids(1), "slot {p}");
    }
    let mut both = daytime_guids(2);
    both.extend(daytime_guids(1));
    assert_eq!(sim.state.applied_upgrades[8], both);
    // Same time of day: nothing.
    let before = crate::save::checksum(sim.state);
    apply_daytime_upgrades(&mut sim, 1);
    assert_eq!(crate::save::checksum(sim.state), before);
}

#[test]
fn console_upgrades_reject_slots_from_11() {
    use crate::command::CommandKind;
    use crate::command::testkit::run;
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let before = crate::save::checksum(sim.state);
    // A7: `0x39` outside 0...10 rejected; `0x3A` outside 0...7 without effect (original).
    for slot in [11, 12, -1, i32::MAX] {
        run(&mut sim, 0, CommandKind::ApplyUpgrade { slot, guid: 2800 });
        run(&mut sim, 0, CommandKind::ApplyUpgrade { slot, guid: -1 });
    }
    for slot in [8, 10, 11, -1] {
        run(&mut sim, 0, CommandKind::RemoveUpgrade { slot, guid: 2800 });
    }
    assert_eq!(crate::save::checksum(sim.state), before);
    // Without an ownership check (console): slot 0 applies at slot 1 and removes it there again.
    run(
        &mut sim,
        0,
        CommandKind::ApplyUpgrade {
            slot: 1,
            guid: 2800,
        },
    );
    assert_eq!(count(&sim, 1, 2800), 1);
    assert_eq!(wood_percent(&sim, 1), 25);
    run(
        &mut sim,
        0,
        CommandKind::RemoveUpgrade {
            slot: 1,
            guid: 2800,
        },
    );
    assert_eq!(count(&sim, 1, 2800), 0);
    assert!(sim.state.applied_upgrades[1].is_empty());
    // `0x3A` without apply: counter -1 (W-215).
    run(
        &mut sim,
        0,
        CommandKind::RemoveUpgrade {
            slot: 0,
            guid: 3374,
        },
    );
    assert_eq!(count(&sim, 0, 3374), -1);
}

#[test]
fn console_apply_all_runs_2800_to_3399() {
    use crate::command::CommandKind;
    use crate::command::testkit::run;
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    run(&mut sim, 0, CommandKind::ApplyUpgrade { slot: 0, guid: -1 });
    let all: Vec<u32> = (2800..3400).collect();
    assert_eq!(sim.state.applied_upgrades[0], all);
    assert_eq!(sim.state.guid_counts[0].len(), 600);
    assert!(
        sim.state.guid_counts[0]
            .iter()
            .all(|(g, &n)| all.contains(g) && n == 1)
    );
    // Defined upgrades act (wood rate 2800 +25, 2801 +30, 3372 plague -80), other slots do not.
    assert_eq!(wood_percent(&sim, 0), -25);
    assert!(sim.state.applied_upgrades[1].is_empty());
}

#[test]
fn a_colonist_quartered_during_an_upgrade_cannot_leave_the_tower() {
    // K-300 on quartered units, behavior of the reimplementation without intervention: 2800
    // (OBJ_UPGRADES of 1100) completes while the colonist sits in the tower. The variant term
    // fails at the pre-check (QUARTERED), all modules with variants go off, also walk;
    // `prop_bits` stay (no matching descriptor). When unquartering nothing recomputes the mask
    // (Ghidra: no caller of `UpdateVariantMask` `0x00469D80` on the way). Worse:
    // `LeaveContainer` asks walk for the movement kind (`0xB00`), the switched-off module
    // answers -1 (`GameObject_SendMessage`, re/module-runtime), `GameObject_FindFreeSpotSpiral`
    // (`0x0046F9A0`) returns 0 at once for `t < 0` - `UnquarterOne` quarters the unit again.
    // The colonist no longer gets out and stays without modules (kept as in the original, K-300).
    use crate::command::{Command, CommandKind, NO_SPREAD};
    use crate::module::quarter::{accept_unit, find_free_spot_spiral};
    use crate::motion::NO_DIRECTION;
    use crate::tick::step;
    const VARIANT_MODULES: [usize; 9] = [
        WALK, CONSTRUCT, STAND, LUMBER, SHOOT, DYING, ATTACK, FIGHT_NEAR, FIGHT_FAR,
    ];
    let order = |kind| [Command { player: 0, kind }];
    let run = |with_upgrade: bool| {
        let rules = rules();
        let mut state = state(&rules);
        let (tower, c) = {
            let mut sim = Sim::new(&mut state, &rules);
            let tower = spawn(&mut sim, TOWER, 0, (10, 10));
            let c = spawn(&mut sim, COLONIST, 0, (14, 10));
            assert_eq!(accept_unit(&mut sim, tower, c), 0);
            if with_upgrade {
                apply_upgrade(&mut sim, 0, 2800, false);
            }
            (tower, c)
        };
        let bits_before: Vec<u32> = {
            let sim = Sim::new(&mut state, &rules);
            VARIANT_MODULES
                .iter()
                .map(|&n| prop_bits(&sim, c, n))
                .collect()
        };
        for _ in 0..10 {
            step(&mut state, &rules, &[]);
        }
        let inside_grid = state.object(c, false).unwrap().grid;
        step(
            &mut state,
            &rules,
            &order(CommandKind::Unquarter {
                container: tower,
                unit: c,
            }),
        );
        for _ in 0..20 {
            step(&mut state, &rules, &[]);
        }
        step(
            &mut state,
            &rules,
            &order(CommandKind::UnquarterAll { container: tower }),
        );
        for _ in 0..20 {
            step(&mut state, &rules, &[]);
        }
        let left_grid = state.object(c, false).unwrap().grid;
        step(
            &mut state,
            &rules,
            &order(CommandKind::Move {
                units: vec![c],
                x: 30 << 12,
                z: 30 << 12,
                target: -1,
                formation: 7,
                direction: NO_DIRECTION,
                spread: NO_SPREAD,
                flags: 0,
            }),
        );
        for _ in 0..200 {
            step(&mut state, &rules, &[]);
        }
        (state, rules, tower, c, bits_before, inside_grid, left_grid)
    };
    // Counter-check without upgrade: unquartered, walk on and in the tick list, the unit walks.
    let (mut state, rules, _, c, _, inside, left) = run(false);
    let sim = Sim::new(&mut state, &rules);
    let o = sim.object(c).unwrap();
    assert_eq!(o.status & status::QUARTERED, 0);
    assert!(enabled(&sim, c, WALK) && o.tick_list.contains(&(WALK as u8)));
    assert_ne!(left, inside, "placed in front of the tower");
    assert_ne!(o.grid, left, "walks after the command");
    // With 2800 in the tower: unquartering (single and all) fails, the unit stays an occupant.
    let (mut state, rules, tower, c, bits_before, inside, left) = run(true);
    let mut sim = Sim::new(&mut state, &rules);
    let o = sim.object(c).unwrap();
    assert_ne!(o.status & status::QUARTERED, 0, "quartered again");
    assert!(o.tick_list.is_empty(), "{:?}", o.tick_list);
    assert_eq!((left, o.grid), (inside, inside), "stays in the tower");
    match &sim.module(tower, 23).unwrap().kind {
        ModuleKind::Quarter(q) => assert_eq!(q.occupants, [c]),
        _ => panic!("module 23 is not quarters"),
    }
    // No recomputation: modules off, `prop_bits` as when moving in.
    for (&n, &bits) in VARIANT_MODULES.iter().zip(&bits_before) {
        assert!(!enabled(&sim, c, n), "module {n} stays off");
        assert_eq!(prop_bits(&sim, c, n), bits, "module {n}");
    }
    // Cause: walk does not answer `0xB00`, the spiral search gives up.
    assert_eq!(send(&mut sim, c, &Message::new(0xB00, &[])), -1);
    assert_eq!(find_free_spot_spiral(&mut sim, c, 1 << 14), None);
    // The state is reachable: the savegame loads.
    let bytes = crate::save::testkit::save_bytes(sim.state, &rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(&rules)
        .expect("savegame valid");
}
