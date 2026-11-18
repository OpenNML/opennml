//! Docking places of a building (`m_ParkingLots` `+0x30C`, re/buildings): ring points around
//! every blocked mask tile, choice of the nearest free place for a unit (same region, place
//! offset outward by its radius), registering and unregistering. When walking to the building
//! (`PropertyWalk_ResolveGoalPosition` `0x005379C0`) the unit first asks `0x3403`, then `0xB03`, and
//! registers with `0xC03`; `StopMoving` unregisters it with `0xD03`.

use serde::{Deserialize, Serialize};

use crate::collision::{fixed_pos_distance, query_cost_at};
use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::Message;
use crate::messaging::send;
use crate::motion::step::COST_COLLISION;
use crate::path::BLOCKED;
use crate::sim::Sim;

use super::{Building, MASK_SIDE, building, building_mut};

/// A docking place (16 bytes in the original).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkingLot {
    /// Fixed-point grid position.
    pub x: i32,
    pub z: i32,
    /// Registered unit (`NONE` = free).
    pub hid: ObjectId,
    /// Facing direction toward the building in whole degrees.
    pub dir: i32,
}

/// Half tile (`[0x006726EC]`), quarter tile (`[0x006726F0]`) and eighth tile in fixed point.
const HALF: i32 = 0x800;
const QUARTER: i32 = 0x400;
const EIGHTH: i32 = QUARTER / 2;

/// `PropertyBuilding_BuildParkingLots` (`0x00509980`): for every blocked mask tile (row z
/// outside, column x inside, each 1 ... 18) with center `(c << 12) + bx`, `(r << 12) + bz`, up to
/// twelve places are appended depending on the free neighbors - four corners (`±0.625` tile), four
/// side centers and four side quarters. A rectangle w x h without holes has `4 + 4 · (w + h)`.
pub fn build_parking_lots(mask: &[u8], bx: i32, bz: i32) -> Vec<ParkingLot> {
    let set = |r: i32, c: i32| {
        mask.get((r * MASK_SIDE + c) as usize)
            .is_some_and(|&v| v != 0)
    };
    let far = HALF + EIGHTH;
    let mut lots = Vec::new();
    for r in 1..MASK_SIDE - 1 {
        for c in 1..MASK_SIDE - 1 {
            if !set(r, c) {
                continue;
            }
            let (cx, cz) = ((c << 12) + bx, (r << 12) + bz);
            let free = |dr: i32, dc: i32| !set(r + dr, c + dc);
            let (nw, n, ne) = (free(-1, -1), free(-1, 0), free(-1, 1));
            let (w, e) = (free(0, -1), free(0, 1));
            let (sw, s, se) = (free(1, -1), free(1, 0), free(1, 1));
            let mut push = |x: i32, z: i32, dir: i32| {
                lots.push(ParkingLot {
                    x,
                    z,
                    hid: ObjectId::NONE,
                    dir,
                });
            };
            if n && nw && w {
                push(cx - far, cz - far, 45);
            }
            if n && ne && e {
                push(cx + far, cz - far, 135);
            }
            if w && sw && s {
                push(cx - far, cz + far, -45);
            }
            if e && s && se {
                push(cx + far, cz + far, -135);
            }
            if w {
                push(cx - far, cz - QUARTER, 0);
            }
            if n {
                push(cx + QUARTER, cz - far, 90);
            }
            if e {
                push(cx + far, cz + QUARTER, -180);
            }
            if s {
                push(cx - QUARTER, cz + far, -90);
            }
            if nw && n {
                push(cx - QUARTER, cz - far, 90);
            }
            if ne && e {
                push(cx + far, cz - QUARTER, -180);
            }
            if se && s {
                push(cx + QUARTER, cz + far, -90);
            }
            if sw && w {
                push(cx - far, cz + QUARTER, 0);
            }
        }
    }
    lots
}

/// `PropertyBuilding_OffsetDockPosByRadius` (`0x005048F0`): place offset by the hard radius of the
/// unit against the facing direction, i.e. away from the building: `x − cos(dir)·r`, `z − sin(dir)·r`
/// (table sine of whole degrees, `Fixed_Mul(trig, r)`).
pub fn offset_dock_pos_by_radius(lot: &ParkingLot, radius: i32) -> (i32, i32) {
    let a = lot.dir.wrapping_shl(12);
    (
        lot.x.wrapping_sub(fixed_mul(cos_deg(a), radius)),
        lot.z.wrapping_sub(fixed_mul(sin_deg(a), radius)),
    )
}

/// Region in the path layer `layer` at a fixed-point position (`PathPlanner_GetRegionId`); an
/// invalid layer (including -1) yields `0xFFFF`.
fn region_at(sim: &Sim, layer: i32, p: (i32, i32)) -> u16 {
    usize::try_from(layer).map_or(BLOCKED, |l| {
        sim.state.path.region_id(l, p.0 >> 12, p.1 >> 12)
    })
}

/// `PropertyBuilding_GetNearestDockPos` (`0xB03 {x, z, hid, layer}`, `0x00505A70`): with unit
/// `hid`, the nearest free place to the point (center distance, tie: smaller index) whose
/// offset position lies in the region of the point and has less than `0x1000` collision cost
/// there (without a region neither of these checks applies); an own old registration is released in the
/// process (quirk). Response `x, z, dir` in the arguments, return value the index. Otherwise (also without
/// a unit) the nearest place regardless of occupancy and region, return value -1; without places the
/// building center with direction -1.
pub fn get_nearest_dock_pos(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let q = (m.arg(0), m.arg(1));
    let hid = ObjectId::from_i32(m.arg(2));
    let layer = m.arg(3);
    let me = sim.object(hid).map(|o| (o.id, o.hard_radius));
    let region = region_at(sim, layer, q);
    let Some(count) = building(sim, id).map(|b| b.parking_lots.len()) else {
        return -1;
    };
    if count == 0 {
        let g = sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
        m.set_arg(0, g.0);
        m.set_arg(1, g.1);
        m.set_arg(2, -1);
        return -1;
    }
    if let Some((unit, radius)) = me {
        let wl = send(sim, unit, &Message::new(0xB00, &[]));
        let wl = usize::try_from(wl).unwrap_or(usize::MAX);
        let mut best: Option<(usize, ParkingLot)> = None;
        let mut best_d = i32::MAX;
        for i in 0..count {
            let Some(mut e) = building(sim, id).and_then(|b| b.parking_lots.get(i).copied()) else {
                break;
            };
            let d = fixed_pos_distance(q, (e.x, e.z));
            if e.hid == hid {
                e.hid = ObjectId::NONE;
                if let Some(b) = building_mut(sim, id) {
                    b.parking_lots[i] = e;
                }
            } else if !e.hid.is_none() {
                continue;
            }
            if d < best_d {
                let p = offset_dock_pos_by_radius(&e, radius);
                if region != BLOCKED
                    && (region_at(sim, layer, p) != region
                        || query_cost_at(sim, p.0, p.1, Some(unit), wl, false, false)
                            >= COST_COLLISION)
                {
                    continue;
                }
                best = Some((i, e));
                best_d = d;
            }
        }
        if let Some((i, e)) = best {
            let p = offset_dock_pos_by_radius(&e, radius);
            m.set_arg(0, p.0);
            m.set_arg(1, p.1);
            m.set_arg(2, e.dir);
            return i as i32;
        }
    }
    let lots = building(sim, id).map_or_else(Vec::new, |b| b.parking_lots.clone());
    let mut best = lots[0];
    let mut best_d = i32::MAX;
    for e in &lots {
        let d = fixed_pos_distance(q, (e.x, e.z));
        if d < best_d {
            best = *e;
            best_d = d;
        }
    }
    let p = match me {
        Some((_, radius)) => offset_dock_pos_by_radius(&best, radius),
        None => (best.x, best.z),
    };
    m.set_arg(0, p.0);
    m.set_arg(1, p.1);
    m.set_arg(2, best.dir);
    -1
}

/// `PropertyBuilding_RegisterDockPos` (`0xC03 {index, hid}`, `0x00505F80`): index out of range -> 0;
/// release all old registrations of `hid`, then register - even over a foreign registration
/// (quirk, return value 0 then, otherwise 1).
pub fn register_dock_pos(b: &mut Building, index: i32, hid: ObjectId) -> i32 {
    let Some(i) = usize::try_from(index)
        .ok()
        .filter(|&i| i < b.parking_lots.len())
    else {
        return 0;
    };
    for e in &mut b.parking_lots {
        if e.hid == hid {
            e.hid = ObjectId::NONE;
        }
    }
    let e = &mut b.parking_lots[i];
    let was_free = e.hid.is_none();
    e.hid = hid;
    i32::from(was_free)
}

/// `PropertyBuilding_UnregisterDockPos` (`0xD03 {hid}`, `0x00506120`): release only the **first**
/// registration (quirk); 1 if there was one.
pub fn unregister_dock_pos(b: &mut Building, hid: ObjectId) -> i32 {
    match b.parking_lots.iter_mut().find(|e| e.hid == hid) {
        Some(e) => {
            e.hid = ObjectId::NONE;
            1
        }
        None => 0,
    }
}

/// `0x3403 {hid}` (`0x005061F0`): the first place with this registration - offset by the radius
/// of the (still existing) unit - as `x, z, dir`; return value 1, otherwise 0.
pub fn occupied_dock_pos(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let hid = ObjectId::from_i32(m.arg(0));
    let Some(e) =
        building(sim, id).and_then(|b| b.parking_lots.iter().find(|e| e.hid == hid).copied())
    else {
        return 0;
    };
    let Some(radius) = sim.object(hid).map(|o| o.hard_radius) else {
        return 0;
    };
    let p = offset_dock_pos_by_radius(&e, radius);
    m.set_arg(0, p.0);
    m.set_arg(1, p.1);
    m.set_arg(2, e.dir);
    1
}

/// `0x2C03 {layer, r0, r1, …}` (`0x00507F80`): for each place in list order its region
/// (`GameObject_GetRegionIdAt` with the layer, `0xFFFF` skipped); the first one that is in the list
/// `r0, r1, …` returns its list index; none -> -1.
pub fn dock_region_index(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let lots = building(sim, id).map_or_else(Vec::new, |b| b.parking_lots.clone());
    let layer = m.arg(0);
    let list = m.args.get(1..).unwrap_or(&[]).to_vec();
    for e in lots {
        let r = crate::world::region_id_at(sim, id, layer, e.x, e.z);
        if r == BLOCKED {
            continue;
        }
        if let Some(j) = list.iter().position(|&v| v == i32::from(r)) {
            return j as i32;
        }
    }
    -1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Envelope;
    use crate::messaging::{post_command, send_mut};
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::step;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// Mask of a rectangle `w × h` without holes from mask row/column 1.
    fn rect_mask(w: i32, h: i32) -> Vec<u8> {
        let mut mask = vec![0; (MASK_SIDE * MASK_SIDE) as usize];
        for r in 1..=h {
            for c in 1..=w {
                mask[(r * MASK_SIDE + c) as usize] = 1;
            }
        }
        mask
    }

    #[test]
    fn parking_lots_ring_the_masked_tiles() {
        for (side, count) in [(1, 12), (3, 28), (4, 36)] {
            assert_eq!(
                build_parking_lots(&rect_mask(side, side), 0, 0).len(),
                count
            );
        }
        assert_eq!(
            build_parking_lots(&rect_mask(6, 4), 0, 0).len(),
            4 + 4 * (6 + 4)
        );
        // 3 x 3 at tile (10, 20): base = rectangle corner - half tile.
        let lots = build_parking_lots(&rect_mask(3, 3), (10 << 12) - 0x800, (20 << 12) - 0x800);
        let first: Vec<(i32, i32, i32)> = lots[..4].iter().map(|l| (l.x, l.z, l.dir)).collect();
        assert_eq!(
            first,
            [
                (40448, 81408, 45),
                (40448, 82944, 0),
                (44032, 81408, 90),
                (41984, 81408, 90)
            ]
        );
        assert!(lots.iter().all(|l| l.hid.is_none()));
        // A hole in the middle adds places in the interior (one on each inner side).
        let mut holed = rect_mask(3, 3);
        holed[(2 * MASK_SIDE + 2) as usize] = 0;
        assert_eq!(build_parking_lots(&holed, 0, 0).len(), 28 + 4);
    }

    #[test]
    fn docks_are_moved_out_by_the_unit_radius() {
        let lot = |dir| ParkingLot {
            x: 10_000,
            z: 20_000,
            hid: ObjectId::NONE,
            dir,
        };
        // cos 45 degrees = sin 45 degrees = 2896/4096 -> 362 at radius 512.
        assert_eq!(offset_dock_pos_by_radius(&lot(45), 512), (9638, 19638));
        assert_eq!(offset_dock_pos_by_radius(&lot(-180), 512), (10_512, 20_000));
        assert_eq!(offset_dock_pos_by_radius(&lot(90), 512), (10_000, 19_488));
        assert_eq!(offset_dock_pos_by_radius(&lot(0), 0), (10_000, 20_000));
    }

    // QUIRK W-99: kept
    #[test]
    fn registering_overwrites_foreign_claims_and_unregistering_frees_only_the_first() {
        let mut b = Building {
            parking_lots: build_parking_lots(&rect_mask(1, 1), 0, 0),
            ..Building::default()
        };
        let (a, c) = (ObjectId(7), ObjectId(9));
        assert_eq!(register_dock_pos(&mut b, 12, a), 0);
        assert_eq!(register_dock_pos(&mut b, -1, a), 0);
        assert_eq!(register_dock_pos(&mut b, 3, a), 1);
        // Registering anew releases the old registration.
        assert_eq!(register_dock_pos(&mut b, 5, a), 1);
        assert!(b.parking_lots[3].hid.is_none() && b.parking_lots[5].hid == a);
        // Foreign registration overwritten, return value 0.
        assert_eq!(register_dock_pos(&mut b, 5, c), 0);
        assert_eq!(b.parking_lots[5].hid, c);
        // Double registration (reachable only through a savegame): unregistering releases only the first.
        b.parking_lots[1].hid = c;
        assert_eq!(unregister_dock_pos(&mut b, c), 1);
        assert!(b.parking_lots[1].hid.is_none() && b.parking_lots[5].hid == c);
        assert_eq!(unregister_dock_pos(&mut b, a), 0);
    }

    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 1001\nNAME Beobachter\nVALUEID 100 6 1 0\n";

    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        r.defaults.insert(1100, [(16, 183)].into_iter().collect());
        r.building_shapes
            .insert(1000, super::super::tests::spa_hq());
        r
    }

    fn at(guid: u32, tile: (i32, i32)) -> ObjectDesc {
        let world = |t: i32| t as f32 * 128.0 + 64.0;
        ObjectDesc::new(guid, 0, [world(tile.0), 0.0, world(tile.1)])
    }

    /// Finished SPA headquarters at (1000, 2000): footprint `[5, 14, 10, 17]`.
    fn hq(sim: &mut Sim) -> ObjectId {
        let mut desc = ObjectDesc::new(1000, 0, [1000.0, 0.0, 2000.0]);
        desc.flags |= desc_flags::PLACED;
        create_object(sim, &mut desc).unwrap()
    }

    fn nearest(
        sim: &mut Sim,
        b: ObjectId,
        q: (i32, i32),
        unit: ObjectId,
        layer: i32,
    ) -> (i32, Vec<i32>) {
        let mut m = Message::new(0xB03, &[q.0, q.1, unit.as_i32(), layer]);
        let r = send_mut(sim, b, &mut m);
        (r, m.args)
    }

    // QUIRK W-99: kept
    #[test]
    fn the_nearest_free_dock_wins_ties_by_index_and_releases_the_own_claim() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = hq(&mut sim);
        let u = create_object(&mut sim, &mut at(1100, (2, 2))).unwrap();
        let v = create_object(&mut sim, &mut at(1100, (3, 2))).unwrap();
        assert_eq!(building(&sim, b).unwrap().parking_lots.len(), 44);
        // A tile ring for checking: 1 x 1 places around tile (20, 20).
        building_mut(&mut sim, b).unwrap().parking_lots =
            build_parking_lots(&rect_mask(1, 1), (20 << 12) - 0x800, (20 << 12) - 0x800);
        let center = ((20 << 12) + 0x800, (20 << 12) + 0x800);
        // From the center the eight side places are equally far: the smallest index (4, B1).
        let (r, args) = nearest(&mut sim, b, center, u, -1);
        let lot = building(&sim, b).unwrap().parking_lots[4];
        let (x, z) = offset_dock_pos_by_radius(&lot, 512);
        assert_eq!((r, args), (4, vec![x, z, 0, -1]));
        assert_eq!(send(&mut sim, b, &Message::new(0xC03, &[4, u.as_i32()])), 1);
        // The second unit gets the next equally distant place.
        assert_eq!(nearest(&mut sim, b, center, v, -1).0, 5);
        // The first asks again: its registration is released in the process, it gets 4 again.
        assert_eq!(nearest(&mut sim, b, center, u, -1).0, 4);
        assert!(building(&sim, b).unwrap().parking_lots[4].hid.is_none());
        // `0x3403`: registered place, offset by the radius.
        send(&mut sim, b, &Message::new(0xC03, &[6, v.as_i32()]));
        let mut ask = Message::new(0x3403, &[v.as_i32(), -1, -1]);
        assert_eq!(send_mut(&mut sim, b, &mut ask), 1);
        let lot6 = building(&sim, b).unwrap().parking_lots[6];
        let (x6, z6) = offset_dock_pos_by_radius(&lot6, 512);
        assert_eq!(ask.args, [x6, z6, lot6.dir]);
        let mut none = Message::new(0x3403, &[u.as_i32(), -1, -1]);
        assert_eq!(send_mut(&mut sim, b, &mut none), 0);
        // Without a unit: nearest place regardless of occupancy, not offset, return value -1.
        let (r, args) = nearest(&mut sim, b, (lot6.x, lot6.z), ObjectId::NONE, -1);
        assert_eq!((r, args), (-1, vec![lot6.x, lot6.z, lot6.dir, -1]));
        // `0xD03` unregisters.
        assert_eq!(send(&mut sim, b, &Message::new(0xD03, &[v.as_i32()])), 1);
        assert!(
            building(&sim, b)
                .unwrap()
                .parking_lots
                .iter()
                .all(|l| l.hid.is_none())
        );
        // Without places: building center and direction -1.
        building_mut(&mut sim, b).unwrap().parking_lots.clear();
        let g = sim.object(b).unwrap().grid;
        let (r, args) = nearest(&mut sim, b, center, u, -1);
        assert_eq!((r, args), (-1, vec![g.x.0, g.z.0, -1, -1]));
    }

    #[test]
    fn docks_in_another_region_or_on_a_unit_are_skipped() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = hq(&mut sim);
        let u = create_object(&mut sim, &mut at(1100, (2, 2))).unwrap();
        building_mut(&mut sim, b).unwrap().parking_lots =
            build_parking_lots(&rect_mask(1, 1), (20 << 12) - 0x800, (20 << 12) - 0x800);
        let lots = building(&sim, b).unwrap().parking_lots.clone();
        let center = ((20 << 12) + 0x800, (20 << 12) + 0x800);
        // Region of the query point in layer 0; the offset place 4 (B1) lies on tile (19, 20).
        let (x4, z4) = offset_dock_pos_by_radius(&lots[4], 512);
        assert_eq!((x4 >> 12, z4 >> 12), (19, 20));
        sim.state.path.mark_cell(1, 19, 20, true);
        sim.state.path.flush_dirty_rect();
        assert_eq!(nearest(&mut sim, b, center, u, 0).0, 5);
        // Without a layer (-1) no region check.
        assert_eq!(nearest(&mut sim, b, center, u, -1).0, 4);
        // A unit on the offset place 5: collision cost >= 0x1000.
        let (x5, z5) = offset_dock_pos_by_radius(&lots[5], 512);
        let blocker = create_object(&mut sim, &mut at(1100, (2, 3))).unwrap();
        crate::world::set_grid_position(
            &mut sim,
            blocker,
            crate::fx::FxVec3::new(crate::fx::Fx(x5), crate::fx::Fx(0), crate::fx::Fx(z5)),
            true,
        );
        let (r, _) = nearest(&mut sim, b, center, u, 0);
        assert!(r != 4 && r != 5 && r >= 0, "{r}");
        // `0x2C03`: list index of the first place region (layer 0), unknown regions -1.
        let region = i32::from(sim.state.path.region_id(0, 20, 18));
        let mut m = Message::new(0x2C03, &[0, 4711, region]);
        assert_eq!(send_mut(&mut sim, b, &mut m), 1);
        let mut m = Message::new(0x2C03, &[0, 4711]);
        assert_eq!(send_mut(&mut sim, b, &mut m), -1);
    }

    /// FINDTO `{building, −1, 1}` with feedback messages to an observer (success 111, failure 222).
    fn go_to(sim: &mut Sim, unit: ObjectId, b: ObjectId, observer: ObjectId) {
        let env = Envelope {
            msg: Message::new(0x200, &[b.as_i32(), -1, 1]),
            success: Some(Message::new(0xA7F, &[111])),
            fault: Some(Message::new(0xA7F, &[222])),
            success_target: observer,
            fault_target: observer,
        };
        post_command(sim, unit, env, true);
    }

    #[test]
    fn units_walk_to_different_docks_and_report_success() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = hq(&mut sim);
        let u = create_object(&mut sim, &mut at(1100, (2, 15))).unwrap();
        let v = create_object(&mut sim, &mut at(1100, (2, 16))).unwrap();
        let ou = create_object(&mut sim, &mut at(1001, (30, 30))).unwrap();
        let ov = create_object(&mut sim, &mut at(1001, (30, 29))).unwrap();
        // Expected place of the first unit from its start point (without registration).
        let o = sim.object(u).unwrap();
        let qu = (o.grid.x.0, o.grid.z.0);
        let layer = walk_layer(&sim, u);
        let (iu, au) = nearest(&mut sim, b, qu, u, layer);
        assert!(iu >= 0);
        go_to(&mut sim, u, b, ou);
        go_to(&mut sim, v, b, ov);
        // The two tick groups run alternately: after two ticks both have asked.
        step(&mut state, &rules, &[]);
        step(&mut state, &rules, &[]);
        let lots = |state: &mut SimState| {
            let sim = Sim::new(state, &rules);
            building(&sim, b).unwrap().parking_lots.clone()
        };
        let claimed = lots(&mut state);
        let claim = |id: ObjectId| claimed.iter().position(|l| l.hid == id);
        assert_eq!(claim(u), Some(iu as usize));
        let iv = claim(v).expect("second unit registered");
        assert_ne!(iv, iu as usize);
        let reply = |state: &SimState, o: ObjectId| state.object(o, false).unwrap().carrier.0;
        let mut steps = 0;
        while reply(&state, ou) != 111 || reply(&state, ov) != 111 {
            step(&mut state, &rules, &[]);
            steps += 1;
            assert!(steps < 300, "no success");
            assert!(reply(&state, ou) != 222 && reply(&state, ov) != 222);
        }
        // Arrived at the offset place; `StopMoving` released the registrations.
        let o = state.object(u, false).unwrap();
        let d = fixed_pos_distance((o.grid.x.0, o.grid.z.0), (au[0], au[1]));
        assert!(d <= 0x400, "distance {d}");
        assert!(lots(&mut state).iter().all(|l| l.hid.is_none()));
    }

    #[test]
    fn a_unit_without_a_free_dock_waits_and_retries_without_claiming() {
        // All places occupied by others: `0xB03` returns -1, `PropertyWalk_ResolveGoalPosition`
        // ("Have no position at the building") sends no `0xC03`. `PropertyWalk_OnMsgFindTo`
        // (`0x00538590`) then reports **no** failure, but waits in state 9 (`MOVE`,
        // `0x10`) and repeats the FINDTO after 23 ticks (`0x00532B60`).
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = hq(&mut sim);
        let u = create_object(&mut sim, &mut at(1100, (2, 15))).unwrap();
        let ou = create_object(&mut sim, &mut at(1001, (30, 30))).unwrap();
        building_mut(&mut sim, b).unwrap().parking_lots =
            build_parking_lots(&rect_mask(1, 1), (20 << 12) - 0x800, (20 << 12) - 0x800);
        assert_eq!(building(&sim, b).unwrap().parking_lots.len(), 12);
        let foreign = |i: u16| ObjectId::new(200 + i, 1, 1);
        for i in 0..12u16 {
            let claim = Message::new(0xC03, &[i32::from(i), foreign(i).as_i32()]);
            assert_eq!(send(&mut sim, b, &claim), 1);
        }
        let before = building(&sim, b).unwrap().parking_lots.clone();
        go_to(&mut sim, u, b, ou);
        let reply = |state: &SimState| state.object(ou, false).unwrap().carrier.0;
        for _ in 0..10 {
            step(&mut state, &rules, &[]);
        }
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(building(&sim, b).unwrap().parking_lots, before);
        assert_eq!(crate::module::walk::walk(&sim, u).unwrap().state, 9);
        let st = sim.object(u).unwrap().status;
        let moving = crate::object::status::MOVE | 0x10;
        assert_eq!(st & moving, moving);
        assert!(!matches!(reply(&state), 111 | 222), "no feedback");
        // A place becomes free: the repeat registers the unit there.
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(
            send(&mut sim, b, &Message::new(0xD03, &[foreign(7).as_i32()])),
            1
        );
        let mut steps = 0;
        loop {
            let sim = Sim::new(&mut state, &rules);
            let lots = building(&sim, b).unwrap().parking_lots.clone();
            if let Some(i) = lots.iter().position(|l| l.hid == u) {
                assert_eq!(i, 7);
                break;
            }
            step(&mut state, &rules, &[]);
            steps += 1;
            assert!(steps < 60, "no registration after the repeat");
        }
        assert_ne!(reply(&state), 222);
    }

    /// Layer `m_PPHandleNxN` of the unit (`0x1400`).
    fn walk_layer(sim: &Sim, id: ObjectId) -> i32 {
        crate::module::walk::walk(sim, id).map_or(-1, |w| w.layer as i32)
    }
}
