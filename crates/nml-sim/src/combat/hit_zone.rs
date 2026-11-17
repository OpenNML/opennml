//! Hit zones (re/combat-projectiles 15.2, 15.3): picking a zone of the nearer half in rotation
//! (`GameObject_PickHitZoneTowards` `0x0046F2E0`) and the world position of a zone
//! (`GameObject_GetHitZoneWorldPos` `0x0046F1C0`). `f32` as in the original: rotation via
//! [`crate::footprint::world_matrix`] (x87 `sin`/`cos`), distance in the order of
//! `Vector3_Length`. Pattern and tower zones (`0x229`/`0xB32`) are dead in the original (re/special-movement 4).

use crate::combat::projectile::BLOOD;
use crate::footprint::{Matrix, transform_coord, world_matrix};
use crate::id::ObjectId;
use crate::sim::Sim;
use crate::state::SimEvent;

/// `GameObject_GetWorldPosition` (`0x004669E0`): grid * cell / 4096 (exact).
pub fn world_pos(sim: &Sim, id: ObjectId) -> Option<[f32; 3]> {
    let o = sim.object(id)?;
    Some(crate::world::grid_to_world(o.grid, sim.rules.map.cell))
}

/// `Vector3_Length` (`0x00575130`) under 24-bit x87: `√((x² + y²) + z²)` in `f32`.
pub fn vector3_length(v: [f32; 3]) -> f32 {
    ((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt()
}

/// `Matrix_RotationY(+0x158)`.
fn rotation_y(yaw: f32) -> Matrix {
    world_matrix([0.0; 3], [0.0, yaw, 0.0], [1.0; 3])
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn model_center(sim: &Sim, guid: u32) -> [f32; 3] {
    sim.rules
        .model_centers
        .get(&guid)
        .copied()
        .unwrap_or([0.0; 3])
}

/// `GameObject_PickHitZoneTowards(from, &out)` (`0x0046F2E0`): without zones the full model center
/// (index -1), a single zone directly (index 0), otherwise among the `h = max(n/2, 1)` nearest zones to the
/// rotated shooter position the one with the old counter value; the counter keeps turning (C-91). Distance
/// without zone height (C-209); equally distant: the one found first comes first. An empty entry (distance >=
/// 100,000) or a counter outside `0…h−1` (only when manipulated) yields the model center or
/// counter value 0 (A7, C-210). Returns the index and the world position.
pub fn pick_hit_zone_towards(
    sim: &mut Sim,
    target: ObjectId,
    from: [f32; 3],
) -> Option<(i32, [f32; 3])> {
    let (guid, yaw, counter) = sim
        .object(target)
        .map(|o| (o.guid, o.rotation[1], o.hit_zone_counter))?;
    let w = world_pos(sim, target)?;
    let m = rotation_y(yaw);
    let zones = sim.rules.hit_zones.get(&guid).cloned().unwrap_or_default();
    let n = zones.len();
    let (index, local) = if n < 1 {
        (-1, model_center(sim, guid))
    } else if n == 1 {
        (0, zones[0])
    } else {
        let h = (n >> 1).max(1);
        let rel = [from[0] - w[0], from[1] - w[1], from[2] - w[2]];
        let p = transform_coord(rel, &m);
        let c = usize::try_from(counter)
            .ok()
            .filter(|&c| c < h)
            .unwrap_or(0);
        let next = if c + 1 >= h { 0 } else { c + 1 };
        if let Some(o) = sim.object_mut(target) {
            o.hit_zone_counter = next as i32;
        }
        let mut dist = vec![100_000.0f32; h];
        let mut pick: Vec<Option<usize>> = vec![None; h];
        for (i, z) in zones.iter().enumerate() {
            let f = vector3_length([z[0] - p[0], 0.0 - p[1], z[2] - p[2]]);
            let mut j = h - 1;
            loop {
                if dist[j] <= f {
                    break;
                }
                if j < h - 1 {
                    dist[j + 1] = dist[j];
                    pick[j + 1] = pick[j];
                }
                dist[j] = f;
                pick[j] = Some(i);
                if j == 0 {
                    break;
                }
                j -= 1;
            }
        }
        match pick[c] {
            Some(i) => (i as i32, zones[i]),
            None => (-1, model_center(sim, guid)),
        }
    };
    Some((index, add(transform_coord(local, &m), w)))
}

/// Blood at the target at the zone in the direction of `from` (ranged hit `ArrowProjectile_OnArrive`
/// `0x004ED990`; sniping `0x0051BBA5…0x0051BC32`, re/special-attacks 3.5):
/// [`pick_hit_zone_towards`] exactly once (the counter turns, C-91), then `EffectAttached` with the
/// offset zone - world position of the target. Presentation only; if the target is missing, nothing.
pub(crate) fn attach_blood(sim: &mut Sim, target: ObjectId, from: [f32; 3]) {
    let Some(tz) = world_pos(sim, target) else {
        return;
    };
    let hit = pick_hit_zone_towards(sim, target, from).map_or(tz, |z| z.1);
    sim.state.events.push(SimEvent::EffectAttached {
        object: target,
        path: BLOOD.into(),
        node: String::new(),
        offset: [hit[0] - tz[0], hit[1] - tz[1], hit[2] - tz[2]],
    });
}

/// `GameObject_GetHitZoneWorldPos(i, &out)` (`0x0046F1C0`): zone `i` (`0 ≤ i < n`), otherwise the
/// model center **with its height only** (`x = z = 0`); rotated and added to the world position.
pub fn hit_zone_world_pos(sim: &Sim, target: ObjectId, index: i32) -> Option<[f32; 3]> {
    let o = sim.object(target)?;
    let w = world_pos(sim, target)?;
    let zone = usize::try_from(index)
        .ok()
        .and_then(|i| sim.rules.hit_zones.get(&o.guid)?.get(i).copied());
    let local = zone.unwrap_or_else(|| [0.0, model_center(sim, o.guid)[1], 0.0]);
    Some(add(transform_coord(local, &rotation_y(o.rotation[1])), w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::{COLONIST, HOUSE, rules, spawn, state};

    /// House with four zones (+-100 in x and z, height 0) and center (5, 50, 7).
    fn zoned() -> crate::rules::Rules {
        let mut r = rules();
        r.hit_zones.insert(
            HOUSE,
            vec![
                [100.0, 0.0, 0.0],
                [-100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0],
                [0.0, 0.0, -100.0],
            ],
        );
        r.model_centers.insert(HOUSE, [5.0, 50.0, 7.0]);
        r
    }

    // QUIRK C-91, C-209: kept
    // QUIRK C-210: deviation
    #[test]
    fn zones_rotate_over_the_nearer_half_and_keep_the_first_of_equals() {
        let rules = zoned();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HOUSE, 1, (10, 10));
        sim.object_mut(h).unwrap().rotation[1] = 0.0;
        let w = world_pos(&sim, h).unwrap();
        let from = [w[0] + 1000.0, w[1], w[2]];
        // Distances 900, 1100, 1005, 1005; h = 2 -> [zone 0, zone 2] (zone 3 equally far, later, C-209).
        let picks: Vec<i32> = (0..4)
            .map(|_| pick_hit_zone_towards(&mut sim, h, from).unwrap().0)
            .collect();
        assert_eq!(picks, vec![0, 2, 0, 2]);
        let (i, p) = pick_hit_zone_towards(&mut sim, h, from).unwrap();
        assert_eq!((i, p), (0, [w[0] + 100.0, w[1], w[2]]));
        // Counter outside 0...h-1 (only when manipulated) counts as 0 (exception A7).
        sim.object_mut(h).unwrap().hit_zone_counter = 7;
        assert_eq!(pick_hit_zone_towards(&mut sim, h, from).unwrap().0, 0);
        assert_eq!(sim.object(h).unwrap().hit_zone_counter, 1);
        // Empty entry (C-210): all zones farther than 100,000 from the shooter position -> model center
        // (index -1) instead of an uninitialized index; the counter still turns (1 -> 0).
        let far = [1.0e7, 0.0, 0.0];
        assert_eq!(
            pick_hit_zone_towards(&mut sim, h, far),
            Some((-1, [w[0] + 5.0, w[1] + 50.0, w[2] + 7.0]))
        );
        assert_eq!(sim.object(h).unwrap().hit_zone_counter, 0);
    }

    #[test]
    fn without_zones_the_model_centre_counts_and_single_zones_do_not_turn() {
        let rules = zoned();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 1, (12, 10));
        let w = world_pos(&sim, c).unwrap();
        // Colonist without zones and without center: index -1, position = world position; the counter stays.
        assert_eq!(pick_hit_zone_towards(&mut sim, c, [0.0; 3]), Some((-1, w)));
        assert_eq!(sim.object(c).unwrap().hit_zone_counter, 0);
        // `GetHitZoneWorldPos`: zone i or the center with its height only.
        let h = spawn(&mut sim, HOUSE, 1, (20, 10));
        sim.object_mut(h).unwrap().rotation[1] = 0.0;
        let hw = world_pos(&sim, h).unwrap();
        assert_eq!(
            hit_zone_world_pos(&sim, h, 1),
            Some([hw[0] - 100.0, hw[1], hw[2]])
        );
        assert_eq!(
            hit_zone_world_pos(&sim, h, 7),
            Some([hw[0], hw[1] + 50.0, hw[2]])
        );
        assert_eq!(
            hit_zone_world_pos(&sim, h, -1),
            Some([hw[0], hw[1] + 50.0, hw[2]])
        );
        assert_eq!(vector3_length([3.0, 4.0, 12.0]), 13.0);
    }

    #[test]
    fn one_zone_keeps_the_counter_and_no_zone_uses_the_full_centre() {
        let mut rules = zoned();
        rules.hit_zones.insert(HOUSE, vec![[0.0, 10.0, 0.0]]);
        let mut state1 = state(&rules);
        let mut sim = Sim::new(&mut state1, &rules);
        let h = spawn(&mut sim, HOUSE, 1, (10, 10));
        sim.object_mut(h).unwrap().rotation[1] = 0.0;
        let w = world_pos(&sim, h).unwrap();
        assert_eq!(
            pick_hit_zone_towards(&mut sim, h, [0.0; 3]),
            Some((0, [w[0], w[1] + 10.0, w[2]]))
        );
        assert_eq!(sim.object(h).unwrap().hit_zone_counter, 0);
        let mut rules2 = zoned();
        rules2.hit_zones.remove(&HOUSE);
        let mut state2 = state(&rules2);
        let mut sim2 = Sim::new(&mut state2, &rules2);
        let h2 = spawn(&mut sim2, HOUSE, 1, (10, 10));
        sim2.object_mut(h2).unwrap().rotation[1] = 0.0;
        let w2 = world_pos(&sim2, h2).unwrap();
        // `PickHitZoneTowards` without zones: full model center (x and z stay), index -1.
        assert_eq!(
            pick_hit_zone_towards(&mut sim2, h2, [0.0; 3]),
            Some((-1, [w2[0] + 5.0, w2[1] + 50.0, w2[2] + 7.0]))
        );
    }
}
