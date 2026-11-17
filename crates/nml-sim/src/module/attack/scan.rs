//! Target search of the attack module (`PropertyAttack_ScanForTarget` `0x00501990`,
//! re/combat-targeting 1): priorities from `priorities.ini`, base filter, reach filter,
//! selection by priority and gap, switch rule.

use nml_formats::targets::MatchFilter;

use crate::filter::{matches, matches_target};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::object::status;
use crate::sim::Sim;

use super::{BUSY, attack, get_stubbornness};

/// `PropertyAttack_BuildPriorityFilters` (`0x00500D20`): TARGET filters of all entries whose
/// MY filter the attacker satisfies, in file order; reference = attacker, fog player = its
/// owner.
pub fn priority_filters(sim: &mut Sim, id: ObjectId) -> Vec<MatchFilter> {
    let entries = sim.rules.priorities.clone();
    let mut out = Vec::new();
    for e in entries {
        if matches(sim, id, &e.my, -1) {
            let mut f = e.target;
            f.ref_object = id.as_i32();
            f.fog_player = i32::from(id.slot());
            out.push(f);
        }
    }
    out
}

/// `Priorities_FindMatchIndex` (`0x00500BF0`): index of the first matching filter, otherwise -1.
pub fn find_match_index(
    sim: &mut Sim,
    obj: Option<ObjectId>,
    prio: &[MatchFilter],
    owner: i32,
) -> i32 {
    let Some(o) = obj else { return -1 };
    for (i, f) in prio.iter().enumerate() {
        if matches_target(sim, o, f, owner, f.fog_player) {
            return i as i32;
        }
    }
    -1
}

/// `PropertyAttack_BuildEnemyFilter` (`0x00501760`) together with the filter chain `f1` to `f3`
/// (re/combat-targeting 1.5): not diving, visible to the owner, not in a camouflaged container; no ghost, no
/// mount; enemy without headhunter module, with `m_AttackAnimals` or aggressive animal (slot 8). The
/// chain is built locally on each call instead of the global filters of the original (C-35).
pub fn enemy_filter(sim: &mut Sim, id: ObjectId) -> MatchFilter {
    let owner = i32::from(id.slot());
    let animals = attack(sim, id).is_some_and(|a| a.attack_animals);
    // `MM_PROPID -1`: only active animal modules count (with 0 or 0x11 switched-off ones too).
    let f3 = MatchFilter {
        mode: 0x212,
        mode2: 4,
        match_bits: 0x114,
        prop_id: u32::MAX,
        ..MatchFilter::constructed()
    };
    let f2 = MatchFilter {
        mode: if animals { 0x2000_8410 } else { 0x8410 },
        mode2: 4,
        prop_id: 0x22,
        next: animals.then(|| Box::new(f3)),
        ..MatchFilter::constructed()
    };
    let f1 = MatchFilter {
        mode: 0x1000_C010,
        mode2: 4,
        match_bits: 0x8000,
        prop_id: 0x1B,
        next: Some(Box::new(f2)),
        ..MatchFilter::constructed()
    };
    let mut head = MatchFilter {
        mode: 0x1200_2010,
        mode2: 6,
        status: status::DIVING,
        fog_player: owner,
        method: 5,
        next: Some(Box::new(f1)),
        ..MatchFilter::constructed()
    };
    if owner == 8 {
        head.mode &= !0x200_0000;
        send(sim, id, &Message::new(0xB00, &[])); // C-35: result discarded
        if send(sim, id, &Message::new(0xA14, &[])) == 1 {
            head.mode &= !0x2000;
            head.mode2 = 0x80;
        }
    }
    head
}

/// Near and far band (each `(min, max)`, -1 without the ability) together with own radius and search radius.
pub(crate) struct Bands {
    pub near: (i32, i32),
    pub far: (i32, i32),
    pub radius: i32,
}

/// Range of the combat module (`0xA12` near, `0xB13` far; re/combat-targeting 1.7).
fn query_range(sim: &mut Sim, id: ObjectId, code: u32) -> (i32, i32) {
    let mut m = Message::new(code, &[-1, -1]);
    send_mut(sim, id, &mut m);
    (m.arg(0), m.arg(1))
}

/// Ranges with own radius; search radius `max(sight*1024, near_max, far_max) + 4096`
/// (+ container size when quartered in a building); the stub addend is 0 (C-33).
pub(crate) fn search_bands(sim: &mut Sim, id: ObjectId) -> Bands {
    let r = sim.object(id).map_or(0, |o| o.hard_radius);
    let shift = |(a0, a1): (i32, i32)| {
        (
            if a0 > 0 { r + a0 } else { a0 },
            if a1 >= 0 { r + a1 } else { a1 },
        )
    };
    let far = if super::can_fight_far(sim, id) {
        shift(query_range(sim, id, 0xB13))
    } else {
        (-1, -1)
    };
    let near = if super::can_fight_near(sim, id) {
        shift(query_range(sim, id, 0xA12))
    } else {
        (-1, -1)
    };
    let sight = sim.attribute(id, 18) << 10;
    let x = near.1.max(far.1);
    let mut radius = if x > sight { x } else { sight };
    radius = radius.wrapping_add(4096);
    if let Some(c) = crate::collision::quartering_container(sim, id)
        && sim.object(c).is_some_and(|o| o.has_module(3, false))
    {
        radius = radius.wrapping_add(send(sim, c, &Message::new(0x3E03, &[])) << 12);
    }
    Bands { near, far, radius }
}

/// `PropertyAttack_BuildReachFilter` (`0x00500DF0`, re/combat-targeting 3.1).
pub(crate) fn reach_filter(sim: &mut Sim, id: ObjectId) -> MatchFilter {
    let mut f = MatchFilter {
        mode: 0x30,
        ref_object: id.as_i32(),
        ..MatchFilter::constructed()
    };
    // `ApplyShootTolerance` (`0x00500590`): only with `HASSHOOTTOLERANCE` (never in the shipped data).
    if sim
        .module(id, super::INDEX)
        .is_some_and(|m| m.base.has_flags(0x8_0000))
    {
        f.mode2 |= 8;
        f.angle_tolerance = attack(sim, id).map_or(0, |a| a.shoot_tolerance);
    }
    f.line_of_sight = crate::filter::matches_class(sim, id, 0x10_0011);
    let layer = send(sim, id, &Message::new(0x1400, &[]));
    if !(0..=8).contains(&layer) {
        f.mode = u32::MAX; // C-38: the filter always fails
        return f;
    }
    let mut l = layer;
    if sim.object(id).is_some_and(|o| o.has_module(0xF, false)) {
        if sim
            .object(id)
            .is_some_and(|o| o.status & status::SWIMMING == 0)
        {
            // `layer_from_move_mask` returns layer 0 without a match: the range check of the
            // original afterwards never takes effect.
            l = crate::path::layer_from_move_mask(crate::path::LAYER_MASKS[layer as usize] & !0x20)
                as i32;
        } else {
            f.mode |= 0x2002;
            f.match_bits = 0x800;
            f.status = status::SWIMMING;
        }
    }
    f.layer = l;
    let tether = send(sim, id, &Message::new(0x2200, &[]));
    if tether >= 0 {
        let mut h = Message::new(0x3300, &[-1, -1]);
        send_mut(sim, id, &mut h);
        f.home = (h.arg(0), h.arg(1));
        f.tether = tether;
    }
    f
}

/// `PropertyAttack_GetRangeGap` (`0x00500380`): 0 in the band, otherwise distance to the band; `MAX` if
/// the reach filter does not match or there is no ability.
pub(crate) fn range_gap(
    sim: &mut Sim,
    id: ObjectId,
    dist: i32,
    mut min: i32,
    mut max: i32,
    cand: Option<ObjectId>,
    reach: &mut MatchFilter,
) -> i32 {
    let Some(c) = cand else { return i32::MAX };
    if max < 0 {
        return i32::MAX;
    }
    let rc = sim.object(c).map_or(0, |o| o.hard_radius);
    reach.range_max = rc + max;
    reach.range_min = if min > 0 { rc + min } else { min };
    if !matches(sim, c, reach, i32::from(id.slot())) {
        return i32::MAX;
    }
    let (st, r) = sim.object(id).map_or((0, 0), |o| (o.status, o.hard_radius));
    if st & status::QUARTERED != 0 {
        if sim.object(id).is_some_and(|o| o.has_module(0x13, false)) {
            min = (min - r).max(0);
            max -= r;
        } else {
            min = 0;
            max = 0;
        }
    }
    let mut gap = i32::MAX;
    if dist < min {
        gap = min - dist;
    }
    if max < dist && dist - max < gap {
        gap = dist - max;
    }
    if gap == i32::MAX { 0 } else { gap }
}

/// `PropertyAttack_PickTargetByPriority` (`0x005013C0`): candidates per group in reverse
/// order of discovery (`AddHead`, C-32); target in a container -> the container; return value (target, near, group).
pub(crate) fn pick_target(
    sim: &mut Sim,
    id: ObjectId,
    cands: &[ObjectId],
    prio: &[MatchFilter],
    b: &Bands,
) -> (Option<ObjectId>, bool, i32) {
    if prio.is_empty() {
        return (None, false, -1);
    }
    let owner = i32::from(id.slot());
    let mut groups: Vec<Vec<ObjectId>> = vec![Vec::new(); prio.len()];
    for &c in cands {
        let i = find_match_index(sim, Some(c), prio, owner);
        if i >= 0 {
            groups[i as usize].insert(0, c);
        }
    }
    let mut reach = reach_filter(sim, id);
    let my_box = crate::collision::quartering_container(sim, id);
    let me = sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
    // `near_gap` is not reset per candidate (no effect, re/combat-targeting 1.8).
    let (mut best, mut best_obj, mut near_gap, mut near) = (i32::MAX, None, 0, false);
    for (g, list) in groups.iter().enumerate() {
        for &c0 in list {
            let c = crate::collision::quartering_container(sim, c0).unwrap_or(c0);
            let dist = match my_box {
                Some(bx) => crate::collision::edge_distance_to_object(sim, c, bx),
                None => crate::collision::edge_distance_to_point(sim, c, me),
            };
            let far_gap = range_gap(sim, id, dist, b.far.0, b.far.1, Some(c), &mut reach);
            if send(sim, c, &Message::new(0x3700, &[])) == 1 {
                near_gap = i32::MAX; // target is a large seagoing ship
            } else if far_gap > 0 {
                near_gap = range_gap(sim, id, dist, b.near.0, b.near.1, Some(c), &mut reach);
            }
            let m = near_gap.min(far_gap);
            if m < best {
                best = m;
                best_obj = Some(c);
                near = far_gap > near_gap;
                if m == 0 {
                    break;
                }
            }
        }
        if best_obj.is_some() {
            return (best_obj, near, g as i32);
        }
    }
    (None, near, -1)
}

/// `ScanForTarget` (re/combat-targeting 1.2): gate checks (aggressiveness 4, free - for a rider also
/// the carrier -, stubbornness; at 3 only after a fresh hit by someone other than the target,
/// then like 2), priorities, spatial query within the search radius, selection and switch rule.
pub fn scan_for_target(sim: &mut Sim, id: ObjectId) -> bool {
    let Some(a) = attack(sim, id).filter(|a| a.aggressiveness == 4).copied() else {
        return false;
    };
    let Some(o) = sim.object(id) else {
        return false;
    };
    let (st, carrier) = (o.status, o.target);
    if st & BUSY != 0 {
        return false;
    }
    if st & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0
        && sim.object(carrier).is_some_and(|c| c.status & BUSY != 0)
    {
        return false;
    }
    let mut stub = get_stubbornness(sim, id);
    if stub > 3 {
        return false;
    }
    if stub == 3 {
        // `GameObject_GetRecentAttackerId` (`0x00467D00`): attacker of the last 125 ticks.
        let recent = sim
            .object(id)
            .filter(|o| o.ticks_since_hit < 125)
            .map_or(ObjectId::NONE, |o| o.last_attacker);
        if recent.is_none() || recent == a.target {
            return false;
        }
        stub = 2;
    }
    let prio = priority_filters(sim, id);
    let cur = sim.state.object(a.target, true).map(|_| a.target);
    if !a.target.is_none() && cur.is_none() {
        super::abort(sim, id);
        return false;
    }
    let owner = i32::from(id.slot());
    let cur_idx = find_match_index(sim, cur, &prio, owner);
    if stub == 2 && cur_idx == 0 {
        return false;
    }
    if send(sim, id, &Message::new(0x122E, &[0])) == 0 {
        return false; // siege engine (module 0x2E): -1 in the shipped data
    }
    let enemy = enemy_filter(sim, id);
    let b = search_bands(sim, id);
    let mut started = false;
    if b.near.1.max(b.far.1) >= 1 {
        let me = sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
        let list = crate::collision::query_radius(sim, me, &enemy, b.radius, owner, true);
        let (pick, near, idx) = pick_target(sim, id, &list, &prio, &b);
        if let Some(t) = pick {
            let take = match cur {
                None => true,
                Some(c) if t == c => false,
                Some(c) => {
                    if cur_idx < idx {
                        false // new group worse, also at cur_idx = -1 (C-31)
                    } else if cur_idx != idx {
                        true
                    } else if stub == 2 {
                        false
                    } else {
                        let dn = crate::collision::edge_distance_to_object(sim, t, id);
                        let dc = crate::collision::edge_distance_to_object(sim, c, id);
                        dn <= dc - 4096
                    }
                }
            };
            if take {
                let old_auto = attack(sim, id).is_none_or(|s| s.automatic);
                if let Some(s) = super::attack_mut(sim, id) {
                    s.automatic = true;
                    s.target = t;
                }
                started = super::start_attack(sim, id, 0, near);
                if !started && let Some(c) = cur {
                    if let Some(s) = super::attack_mut(sim, id) {
                        s.automatic = old_auto;
                        s.target = c;
                    }
                    started = super::start_attack(sim, id, 0, false);
                }
            }
        }
    }
    let quartered = sim
        .object(id)
        .is_some_and(|o| o.status & status::QUARTERED != 0);
    if attack(sim, id).is_some_and(|s| s.target.is_none()) && !started && !quartered {
        // C-30: a failed start without an old target leaves the new target in place.
        super::restore_aggressiveness(sim, id);
    }
    started
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;

    #[test]
    fn the_test_world_has_five_priority_groups() {
        assert_eq!(rules().priorities.len(), 5);
    }

    // QUIRK C-33: kept
    #[test]
    fn search_radius_and_bands_follow_combat_targeting_1_13() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let archer = spawn(&mut sim, ARCHER, 0, (4, 4));
        let inf = spawn(&mut sim, ENG_INFANTRY, 0, (8, 4));
        let warrior = spawn(&mut sim, WARRIOR, 0, (12, 4));
        let b = search_bands(&mut sim, archer);
        assert_eq!((b.near, b.far, b.radius), ((-1, -1), (4608, 29184), 45056));
        let b = search_bands(&mut sim, inf);
        assert_eq!((b.near, b.far, b.radius), ((0, 2560), (4608, 29184), 36864));
        let b = search_bands(&mut sim, warrior);
        assert_eq!((b.near, b.far, b.radius), ((0, 2560), (-1, -1), 45056));
    }

    // QUIRK C-38: kept
    #[test]
    fn range_gaps_follow_combat_targeting_1_13() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let at = |sim: &mut Sim, guid, slot, fx: (i32, i32)| {
            crate::world::create_object(
                sim,
                &mut crate::world::ObjectDesc::new(
                    guid,
                    slot,
                    [fx.0 as f32 / 32.0, 0.0, fx.1 as f32 / 32.0],
                ),
            )
            .unwrap()
        };
        let base = (20 << 12) + 0x800;
        // Archer, target center 9 tiles: dist = 36,352, far_gap = 7168, near impossible.
        let archer = at(&mut sim, ARCHER, 0, (base, base));
        let t = at(&mut sim, SABRE, 1, (base + 36864, base));
        let b = search_bands(&mut sim, archer);
        let mut reach = reach_filter(&mut sim, archer);
        let dist = crate::collision::edge_distance_to_point(&mut sim, t, (base, base));
        assert_eq!(dist, 36352);
        assert_eq!(
            range_gap(
                &mut sim,
                archer,
                dist,
                b.far.0,
                b.far.1,
                Some(t),
                &mut reach
            ),
            7168
        );
        assert_eq!(
            range_gap(
                &mut sim,
                archer,
                dist,
                b.near.0,
                b.near.1,
                Some(t),
                &mut reach
            ),
            i32::MAX
        );
        // Warrior, target center 3 tiles: near_gap = 11,776 - 2560 = 9216.
        let w = at(&mut sim, WARRIOR, 0, (base, base + 8 * 4096));
        let t2 = at(&mut sim, SABRE, 1, (base + 3 * 4096, base + 8 * 4096));
        let b = search_bands(&mut sim, w);
        let mut reach = reach_filter(&mut sim, w);
        let dist = crate::collision::edge_distance_to_point(&mut sim, t2, (base, base + 8 * 4096));
        assert_eq!(
            range_gap(&mut sim, w, dist, b.near.0, b.near.1, Some(t2), &mut reach),
            9216
        );
        // C-38: without walk layer (`0x1400` -> -1) the reach filter always fails.
        let tree = at(
            &mut sim,
            crate::command::testkit::TREE,
            9,
            (base, base - 4 * 4096),
        );
        let mut reach = reach_filter(&mut sim, tree);
        assert_eq!(reach.mode, u32::MAX);
        assert_eq!(
            range_gap(&mut sim, tree, 0, 0, 2560, Some(t2), &mut reach),
            i32::MAX
        );
    }

    // QUIRK W-170, C-32: kept
    #[test]
    fn priorities_pick_the_highest_group_and_the_last_found_on_ties() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (10, 10));
        // Aggressiveness 4
        send(
            &mut sim,
            w,
            &crate::module::quarter::attack_mode_msg(3, false),
        );
        let house = spawn(&mut sim, HQ, 1, (16, 10));
        let f1 = spawn(&mut sim, SABRE, 1, (10, 13));
        let f2 = spawn(&mut sim, SABRE, 1, (7, 10));
        let prio = priority_filters(&mut sim, w);
        assert_eq!(prio.len(), 4);
        assert_eq!(find_match_index(&mut sim, Some(house), &prio, 0), 3);
        assert_eq!(find_match_index(&mut sim, Some(f1), &prio, 0), 1);
        let b = search_bands(&mut sim, w);
        let enemy = enemy_filter(&mut sim, w);
        let me = sim.object(w).map(|o| (o.grid.x.0, o.grid.z.0)).unwrap();
        let cands = crate::collision::query_radius(&mut sim, me, &enemy, b.radius, 0, true);
        let (pick, near, idx) = pick_target(&mut sim, w, &cands, &prio, &b);
        assert_eq!(idx, 1, "fighter ranks before building");
        assert!(near);
        // Tie (both 3 tiles, same gap): the last found one wins (C-32).
        let last = cands
            .iter()
            .rev()
            .copied()
            .find(|&c| c == f1 || c == f2)
            .unwrap();
        assert_eq!(pick, Some(last));
    }

    // QUIRK C-31: kept
    #[test]
    fn the_switch_rule_needs_a_higher_group_or_one_tile_closer() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (10, 10));
        send(
            &mut sim,
            w,
            &crate::module::quarter::attack_mode_msg(3, false),
        );
        let far = spawn(&mut sim, SABRE, 1, (10, 16)); // 6 tiles
        let near = spawn(&mut sim, SABRE, 1, (10, 5)); // 5 tiles: exactly 1 tile closer
        super::super::attack_mut(&mut sim, w).unwrap().target = far;
        assert!(scan_for_target(&mut sim, w));
        assert_eq!(
            attack(&sim, w).unwrap().target,
            near,
            "<= d_old - 4096 switches"
        );
        // Target without priority (cur_idx = -1): never switch (C-31).
        let mut state = crate::command::testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (10, 10));
        send(
            &mut sim,
            w,
            &crate::module::quarter::attack_mode_msg(3, false),
        );
        let tree = spawn(&mut sim, crate::command::testkit::TREE, 9, (10, 14));
        let _enemy = spawn(&mut sim, SABRE, 1, (10, 12));
        super::super::attack_mut(&mut sim, w).unwrap().target = tree;
        assert!(!scan_for_target(&mut sim, w));
        assert_eq!(attack(&sim, w).unwrap().target, tree);
    }

    #[test]
    fn neutral_attackers_ignore_fog_and_attack_points_skip_slots_from_8() {
        let rules = rules();
        let mut state = crate::command::testkit::fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let wolf = spawn(&mut sim, SABRE, 8, (10, 10));
        super::super::attack_mut(&mut sim, wolf)
            .unwrap()
            .aggressiveness = 4;
        let player = spawn(&mut sim, SABRE, 0, (12, 10));
        super::super::attack_mut(&mut sim, player)
            .unwrap()
            .aggressiveness = 4;
        assert!(scan_for_target(&mut sim, wolf), "slot 8 without fog");
        assert!(
            !scan_for_target(&mut sim, player),
            "slot 0 sees nobody in the fog"
        );
        // Combat points: the attack on slot 0 counts, one on the neutral (slot 8) does not.
        let s = super::super::attack_mut(&mut sim, player).unwrap();
        s.target = wolf;
        s.status = 1;
        super::super::tick(&mut sim, player, 1);
        assert_eq!(sim.state.combat.attack_points, [0; 8], "slot 8 discarded");
        super::super::tick(&mut sim, wolf, 1);
        assert_eq!(sim.state.combat.attack_points[0], 190);
    }

    #[test]
    fn the_search_counter_fires_by_hid_phase_and_drops_the_rest() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (10, 10));
        let c0 = attack(&sim, w).unwrap().search_counter;
        // First search in tick k with c0 + k*100 > 1499 (re/combat-attack 11: 553 -> 10th tick).
        let k = (1500 - c0 + 99) / 100;
        for _ in 0..k - 1 {
            super::super::tick(&mut sim, w, 100);
        }
        assert_eq!(attack(&sim, w).unwrap().search_counter, c0 + (k - 1) * 100);
        super::super::tick(&mut sim, w, 100);
        assert_eq!(
            attack(&sim, w).unwrap().search_counter,
            0,
            "remainder discarded (C-12)"
        );
    }
}
