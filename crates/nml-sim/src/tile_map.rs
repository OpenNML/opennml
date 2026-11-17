//! `BuildingTileMap` (`GameWorld+0x41C`, re/combat-targeting 4): per tile the height of a stamped
//! building (`m_MapHandles`) and its ID (`m_MapHIDs`); the terrain grid `m_MapTerrain` lives in
//! [`crate::rules::TileTerrain`]. For stamping, removal and line of sight see [`add_building`] and
//! [`has_line_of_sight`].
//!
//! Savegame (deviation C-48): saved, but **thin** - only occupied tiles as `(index, height, ID)` in
//! ascending order. Reading rejects corrupted data.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::footprint::helper_node_map_position;
use crate::fx::{Fx, fixed_div, fixed_hypot, fixed_mul};
use crate::id::ObjectId;
use crate::math::{atan2_deg, cos_deg, sin_deg};
use crate::object::{GameObject, status};
use crate::sim::Sim;

/// Largest map side (`GameWorld+0x49C/+0x4A0`, re/combat-targeting 4.1).
pub const MAX_SIDE: i32 = 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildingTileMap {
    width: i32,
    height: i32,
    handles: Vec<i32>,
    hids: Vec<i32>,
}

impl BuildingTileMap {
    /// Empty map `w x h` (`BuildingTileMap_Init`: heights and IDs 0).
    pub fn new(width: i32, height: i32) -> Self {
        let n = (width.max(0) * height.max(0)) as usize;
        Self {
            width,
            height,
            handles: vec![0; n],
            hids: vec![0; n],
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        ((0..self.width).contains(&x) && (0..self.height).contains(&z))
            .then(|| (z * self.width + x) as usize)
    }

    /// Building height of the tile; 0 outside.
    pub fn handle(&self, x: i32, z: i32) -> i32 {
        self.index(x, z).map_or(0, |i| self.handles[i])
    }

    /// Building ID of the tile (`BuildingTileMap_GetHid` `0x00490A70`); 0 outside.
    pub fn hid(&self, x: i32, z: i32) -> i32 {
        self.index(x, z).map_or(0, |i| self.hids[i])
    }

    /// Occupied tiles `(index, height, ID)` ascending; occupied means ID != 0 (stamping and
    /// removal write both fields together).
    pub fn occupied(&self) -> impl Iterator<Item = (u32, i32, i32)> + '_ {
        self.hids
            .iter()
            .enumerate()
            .filter(|(_, h)| **h != 0)
            .map(|(i, &h)| (i as u32, self.handles[i], h))
    }

    /// `BuildingTileMap_SetCell` (`0x004909E0`): the higher height wins; the same ID or
    /// `force` always overwrites; has no effect outside.
    pub fn set_cell(&mut self, x: i32, z: i32, handle: i32, hid: i32, force: bool) {
        if let Some(i) = self.index(x, z)
            && (self.handles[i] < handle || self.hids[i] == hid || force)
        {
            self.handles[i] = handle;
            self.hids[i] = hid;
        }
    }

    /// `BuildingTileMap_RemoveObjectId` (`0x00490B50`): clear tiles with the ID.
    pub fn remove_id(&mut self, hid: i32) {
        for i in 0..self.hids.len() {
            if self.hids[i] == hid {
                self.hids[i] = 0;
                self.handles[i] = 0;
            }
        }
    }

    /// `BuildingTileMap_ReplaceObjectId` (`0x00490AF0`, change of owner): rewrite the ID.
    pub fn replace_id(&mut self, old: i32, new: i32) {
        for h in &mut self.hids {
            if *h == old {
                *h = new;
            }
        }
    }
}

impl Serialize for BuildingTileMap {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let cells: Vec<(u32, i32, i32)> = self.occupied().collect();
        (self.width, self.height, cells).serialize(s)
    }
}

impl<'de> Deserialize<'de> for BuildingTileMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let (width, height, cells): (i32, i32, Vec<(u32, i32, i32)>) = Deserialize::deserialize(d)?;
        if !(0..=MAX_SIDE).contains(&width) || !(0..=MAX_SIDE).contains(&height) {
            return Err(D::Error::custom(format!(
                "building map {width} x {height} outside 0..{MAX_SIDE}"
            )));
        }
        let mut m = Self::new(width, height);
        let n = (width * height) as u32;
        let mut last: Option<u32> = None;
        for (i, handle, hid) in cells {
            if i >= n || last.is_some_and(|l| i <= l) || hid == 0 {
                return Err(D::Error::custom(format!(
                    "building map: tile {i} invalid, duplicate or without ID"
                )));
            }
            m.handles[i as usize] = handle;
            m.hids[i as usize] = hid;
            last = Some(i);
        }
        Ok(m)
    }
}

/// Height of the tile (`BuildingTileMap_GetHeight` `0x00490990`): terrain + building; 0 outside.
pub fn height(sim: &Sim, x: i32, z: i32) -> i32 {
    let m = &sim.state.tile_map;
    if m.index(x, z).is_none() {
        return 0;
    }
    sim.rules.tile_terrain.at(x, z).wrapping_add(m.handle(x, z))
}

/// `BuildingTileMap_AddBuilding(go, skip_free)` (`0x00492250`) -> `StampBuilding`
/// (`0x00491F10`): half header hull (`vf+0x30(0)` = `BuildingShape::half`) via
/// `GridPos_FromWorldFloat` in fixed point; height `2*y` (dead: 0); width and depth swap in the
/// window `d ∈ [45, 115] ∪ [205, 315]`, `d = (degrees + 450) % 360` (C-46); rectangle around the position
/// offset by `+2048`, half-open `[x0, x1) x [z0, z1)`; `frei%02d` tiles with height 0. Pattern and
/// towers (`0x29`, `0x32`) do not stamp. The SiegeEngine offset (`0x500_0000`) does not exist for buildings
/// (`footprint.rs`).
pub fn add_building(sim: &mut Sim, id: ObjectId, skip_free_nodes: bool) {
    let Some(o) = sim.object(id) else { return };
    if o.has_module(0x29, false) || o.has_module(0x32, false) {
        return;
    }
    let cell = sim.rules.map.cell;
    let half = sim
        .rules
        .building_shapes
        .get(&o.guid)
        .map_or([0.0; 3], |s| s.half);
    let ext = |i: usize| Fx::from_f32(half[i]).0 / cell[i];
    let (mut hx, hy, mut hz) = (ext(0), ext(1), ext(2));
    let h = if o.status & status::DIED != 0 {
        0
    } else {
        hy.wrapping_mul(2)
    };
    let d = ((o.direction.0 >> 12) + 450) % 360;
    if (45..=315).contains(&d) && (d <= 115 || d >= 205) {
        std::mem::swap(&mut hx, &mut hz);
    }
    let (gx, gz) = (o.grid.x.0, o.grid.z.0);
    let (mut l, mut r) = (
        gx.wrapping_add(hx).wrapping_add(2048),
        gx.wrapping_sub(hx).wrapping_add(2048),
    );
    let (mut t, mut b) = (
        gz.wrapping_add(hz).wrapping_add(2048),
        gz.wrapping_sub(hz).wrapping_add(2048),
    );
    if r < l {
        std::mem::swap(&mut l, &mut r);
    }
    if b < t {
        std::mem::swap(&mut t, &mut b);
    }
    let mut cells = BTreeMap::new();
    for z in (t >> 12)..(b >> 12) {
        for x in (l >> 12)..(r >> 12) {
            cells.insert((z, x), h);
        }
    }
    if skip_free_nodes {
        for i in 1..100 {
            let Some(p) = helper_node_map_position(sim, id, &format!("frei{i:02}")) else {
                break;
            };
            if let Some(v) = cells.get_mut(&(p.z.0 >> 12, p.x.0 >> 12)) {
                *v = 0;
            }
        }
    }
    let hid = id.as_i32();
    for ((z, x), v) in cells {
        sim.state.tile_map.set_cell(x, z, v, hid, false);
    }
}

/// Speed of the sight ray (`Trajectory_InitSpeed5` `0x00562EB0`: `Fixed(5)`).
const RAY_SPEED: i32 = 5 << 12;

/// Straight ray (`Trajectory_Init` `0x00562DE0`, without gravity): start `[x, z, height]`,
/// horizontal length `D`, time `T = D / v`, slope angle `theta`.
struct Ray {
    p0: [i32; 3],
    d: i32,
    t: i32,
    theta: i32,
}

impl Ray {
    fn new(p0: [i32; 3], p1: [i32; 3]) -> Self {
        let d = fixed_hypot(p1[0].wrapping_sub(p0[0]), p1[1].wrapping_sub(p0[1]));
        // Same position: the original aborts, `θ` stays unset and is never read.
        let theta = if d == 0 {
            0
        } else {
            atan2_deg(p1[2].wrapping_sub(p0[2]), d)
        };
        Self {
            p0,
            d,
            t: fixed_div(d, RAY_SPEED),
            theta,
        }
    }

    /// `Trajectory_GetHeightAtCell` (`0x00562AA0`) for the quarter tile `p`: sample offset by
    /// half a tile (C-43); with `D = 0`, `out` stays unchanged.
    fn height_at(&self, p: (i32, i32), out: &mut i32) {
        if self.d == 0 {
            return;
        }
        let px = p.0.wrapping_shl(12) / 4 + 2048;
        let pz = p.1.wrapping_shl(12) / 4 + 2048;
        let dist = fixed_hypot(px.wrapping_sub(self.p0[0]), pz.wrapping_sub(self.p0[1]));
        let tt = fixed_div(fixed_mul(self.t, dist), self.d);
        let c = cos_deg(self.theta);
        // C-44: at theta = +-90 degrees the original divides by 0 (crash); here the ray does not rise (A7).
        let vy = if c == 0 {
            0
        } else {
            fixed_mul(sin_deg(self.theta), fixed_div(RAY_SPEED, c))
        };
        *out = self.p0[2].wrapping_add(fixed_mul(vy, tt));
    }
}

/// `BuildingTileMap_RasterizeLine` (`0x004919A0`): Bresenham variant in quarter tiles,
/// both ends included. The loop is limited to `|dx| + |dz| + 1` points (safeguard,
/// never reached in the original or here).
fn rasterize(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut z) = a;
    let (dx, sx) = ((b.0 - x).abs(), if b.0 - x >= 0 { 1 } else { -1 });
    let (dz, sz) = ((b.1 - z).abs(), if b.1 - z >= 0 { 1 } else { -1 });
    let mut err = if dz < dx {
        -((dx + 1) / 2)
    } else {
        (dz + 1) / 2
    };
    let mut out = Vec::new();
    for _ in 0..=(dx + dz) {
        if (x, z) == b {
            break;
        }
        out.push((x, z));
        if err < 0 {
            x += sx;
            err += dz;
        } else {
            z += sz;
            err -= dx;
        }
    }
    out.push((x, z));
    out
}

fn hid_at(sim: &Sim, p: [i32; 3]) -> i32 {
    sim.state.tile_map.hid(p[0] >> 12, p[1] >> 12)
}

/// `BuildingTileMap_TraceTrajectory` (`0x00491A70`): every quarter tile between the start and
/// end tile; free if the tile height is not above the ray, the tile belongs to the start or
/// target building, or it is the end point. Start value of the ray height: end tile z (C-43).
fn trace(sim: &Sim, p0: [i32; 3], p1: [i32; 3]) -> bool {
    let ray = Ray::new(p0, p1);
    let s = ((p0[0] >> 12) * 4, (p0[1] >> 12) * 4);
    let e = ((p1[0] >> 12) * 4, (p1[1] >> 12) * 4);
    let (id0, id1) = (hid_at(sim, p0), hid_at(sim, p1));
    let mut ray_h = e.1 / 4;
    for p in rasterize(s, e) {
        ray.height_at(p, &mut ray_h);
        let (tx, tz) = (p.0 / 4, p.1 / 4);
        let hid = sim.state.tile_map.hid(tx, tz);
        let free = height(sim, tx, tz) <= ray_h
            || (id0 != 0 && hid == id0)
            || (id1 != 0 && hid == id1)
            || p == e;
        if !free {
            return false;
        }
    }
    true
}

/// `GameWorld_HasLineOfSight` (`0x00491C10`): both objects present (also dying); eye height
/// = `max(map height, water)` + 0.75 tile (building, pattern, tower: + 0).
pub fn has_line_of_sight(sim: &Sim, from: ObjectId, to: ObjectId) -> bool {
    let (Some(a), Some(b)) = (sim.object(from), sim.object(to)) else {
        return false;
    };
    let water = sim.rules.tile_terrain.water;
    let eye = |o: &GameObject| {
        let structure =
            o.has_module(3, false) || o.has_module(0x29, false) || o.has_module(0x32, false);
        let h = height(sim, o.grid.x.0 >> 12, o.grid.z.0 >> 12);
        let base = if water < h { h } else { water };
        base.wrapping_add(if structure { 0 } else { 0xC00 })
    };
    let p0 = [a.grid.x.0, a.grid.z.0, eye(a)];
    let p1 = [b.grid.x.0, b.grid.z.0, eye(b)];
    trace(sim, p0, p1)
}

/// `BuildingTileMap_HasLineOfSightPoints` (`0x00491E90`, cell search): like [`has_line_of_sight`]
/// with finished points `[x, z, height]` (C-45: the caller computes without water and without the
/// building exception).
pub fn has_line_of_sight_points(sim: &Sim, from: [i32; 3], to: [i32; 3]) -> bool {
    trace(sim, from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::{COLONIST, HOUSE, rules, spawn, state};
    use crate::rules::{BuildingShape, Rules};
    use crate::state::SimState;

    // QUIRK C-48: deviation
    #[test]
    fn the_map_serializes_only_occupied_cells_and_reads_them_back() {
        let mut m = BuildingTileMap::new(16, 8);
        m.set_cell(3, 2, 3200, 0x1_0801, true);
        m.set_cell(15, 7, 0, 0x2_0802, true); // frei tile: height 0, ID set
        let bytes = postcard::to_allocvec(&m).unwrap();
        assert!(bytes.len() < 40, "thin: {} bytes", bytes.len());
        let back: BuildingTileMap = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, m);
        assert_eq!((back.handle(3, 2), back.hid(3, 2)), (3200, 0x1_0801));
        assert_eq!((back.handle(-1, 0), back.hid(16, 0)), (0, 0));
        assert_eq!(
            back.occupied().collect::<Vec<_>>(),
            vec![(2 * 16 + 3, 3200, 0x1_0801), (7 * 16 + 15, 0, 0x2_0802)]
        );
    }

    #[test]
    fn damaged_maps_are_rejected_while_reading() {
        let bad = |w: i32, h: i32, cells: Vec<(u32, i32, i32)>| {
            let bytes = postcard::to_allocvec(&(w, h, cells)).unwrap();
            postcard::from_bytes::<BuildingTileMap>(&bytes)
                .unwrap_err()
                .to_string()
        };
        // postcard reports its own errors with `custom` text only as "Serde Deserialization Error";
        // so the only check is that every case fails.
        for err in [
            bad(4, 4, vec![(16, 1, 1)]),
            bad(4, 4, vec![(3, 1, 1), (3, 1, 1)]),
            bad(4, 4, vec![(5, 1, 1), (3, 1, 1)]),
            bad(4, 4, vec![(2, 5, 0)]),
            bad(2000, 4, vec![]),
            bad(-1, 4, vec![]),
        ] {
            assert!(!err.is_empty());
        }
        // Valid: empty and ascending.
        let ok = postcard::to_allocvec(&(4, 4, vec![(1u32, 2, 3), (15u32, 0, 9)])).unwrap();
        assert!(postcard::from_bytes::<BuildingTileMap>(&ok).is_ok());
    }

    /// Test world with a house of the hull `half` (test world: 100/50/100).
    fn world(half: [f32; 3]) -> (Rules, SimState) {
        let mut r = rules();
        let s = r.building_shapes.get_mut(&HOUSE).unwrap();
        s.half = half;
        let st = state(&r);
        (r, st)
    }

    fn stamped(sim: &Sim) -> Vec<(i32, i32, i32)> {
        let w = sim.state.tile_map.width();
        sim.state
            .tile_map
            .occupied()
            .map(|(i, h, _)| (i as i32 % w, i as i32 / w, h))
            .collect()
    }

    // QUIRK C-46: kept
    #[test]
    fn a_building_is_stamped_from_its_half_hull_and_turns_in_the_odd_window() {
        // Half hull 300/50/100 world units -> hx = 9600, hy = 1600, hz = 3200 (ftol(f*4096)/128); height 2*hy.
        let (rules, mut state) = world([300.0, 50.0, 100.0]);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HOUSE, 0, (10, 10));
        // Rotation 0 -> direction -90 degrees -> d = 0: no swap; x in [8, 13), z in [10, 11).
        let want: Vec<_> = (8..13).map(|x| (x, 10, 3200)).collect();
        assert_eq!(stamped(&sim), want);
        assert!(
            sim.state
                .tile_map
                .occupied()
                .all(|(_, _, id)| id == h.as_i32())
        );
        // Direction 0 -> d = 90 in [45, 115]: width and depth swap (C-46).
        sim.state.tile_map.remove_id(h.as_i32());
        sim.object_mut(h).unwrap().direction = Fx(0);
        add_building(&mut sim, h, true);
        let want: Vec<_> = (8..13).map(|z| (10, z, 3200)).collect();
        assert_eq!(stamped(&sim), want);
        // Direction 30 degrees -> d = 120: no swap (115 < d < 205).
        sim.state.tile_map.remove_id(h.as_i32());
        sim.object_mut(h).unwrap().direction = Fx(30 << 12);
        add_building(&mut sim, h, true);
        assert_eq!(stamped(&sim).len(), 5);
        assert!(stamped(&sim).iter().all(|&(_, z, _)| z == 10));
    }

    #[test]
    fn free_nodes_stay_open_dead_buildings_stamp_nothing_and_ids_move_on_owner_change() {
        let (mut rules, _) = world([300.0, 50.0, 100.0]);
        let s: &mut BuildingShape = rules.building_shapes.get_mut(&HOUSE).unwrap();
        s.helper_nodes.insert("frei01".into(), [0.0, 0.0, 0.0]);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HOUSE, 0, (10, 10));
        // The frei tile (10, 10) stays open on a new stamp (height 0 only writes the same ID).
        assert_eq!(sim.state.tile_map.hid(10, 10), 0);
        assert_eq!(sim.state.tile_map.handle(9, 10), 3200);
        // Same ID again: the frei tile stays empty (height 0 only overwrites the same ID, and 0 is stored there).
        add_building(&mut sim, h, true);
        assert_eq!(
            (
                sim.state.tile_map.handle(10, 10),
                sim.state.tile_map.hid(10, 10)
            ),
            (0, 0)
        );
        add_building(&mut sim, h, false);
        assert_eq!(sim.state.tile_map.handle(10, 10), 3200);
        // Change of owner: all tiles carry the new ID (`ReplaceObjectId`).
        let new = crate::world::change_owner(&mut sim, h, 2).unwrap();
        assert!(
            sim.state
                .tile_map
                .occupied()
                .all(|(_, _, id)| id == new.as_i32())
        );
        // Dead: height 0; nothing is written on free tiles.
        sim.state.tile_map.remove_id(new.as_i32());
        sim.object_mut(new).unwrap().status |= status::DIED;
        add_building(&mut sim, new, true);
        assert_eq!(sim.state.tile_map.occupied().count(), 0);
        // Destroying clears the map.
        sim.object_mut(new).unwrap().status &= !status::DIED;
        add_building(&mut sim, new, true);
        crate::world::destroy_object(&mut sim, new);
        assert_eq!(sim.state.tile_map.occupied().count(), 0);
    }

    #[test]
    fn stamps_clip_at_the_map_border() {
        let (rules, mut state) = world([300.0, 50.0, 100.0]);
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, HOUSE, 0, (0, 5));
        let want: Vec<_> = (0..3).map(|x| (x, 5, 3200)).collect();
        assert_eq!(stamped(&sim), want, "x in [-2, 3), outside discarded");
    }

    #[test]
    fn a_house_higher_than_three_quarters_of_a_tile_blocks_the_line_of_sight() {
        // re/combat-targeting 4.3 test vector: A (10,10), B (20,10), building (15,10); eyes 3072.
        for (half_y, visible) in [(50.0, false), (40.0, true)] {
            let (rules, mut state) = world([100.0, half_y, 100.0]);
            let mut sim = Sim::new(&mut state, &rules);
            let a = spawn(&mut sim, COLONIST, 0, (10, 10));
            let b = spawn(&mut sim, COLONIST, 1, (20, 10));
            spawn(&mut sim, HOUSE, 2, (15, 10));
            assert_eq!(has_line_of_sight(&sim, a, b), visible, "hull y {half_y}");
            assert_eq!(has_line_of_sight(&sim, b, a), visible);
        }
    }

    #[test]
    fn own_and_target_building_tiles_never_block() {
        let (rules, mut state) = world([300.0, 200.0, 300.0]);
        let mut sim = Sim::new(&mut state, &rules);
        let house = spawn(&mut sim, HOUSE, 0, (10, 10));
        let b = spawn(&mut sim, COLONIST, 1, (20, 10));
        // Eye height of the building without the addition; its own tiles do not count.
        assert!(has_line_of_sight(&sim, house, b));
        assert!(has_line_of_sight(&sim, b, house));
    }

    // QUIRK C-43: kept
    // QUIRK C-44: deviation
    #[test]
    fn line_of_sight_survives_vertical_and_degenerate_rays() {
        let (rules, mut state) = world([100.0, 50.0, 100.0]);
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, HOUSE, 2, (11, 10));
        let x = (10 << 12) + 2048;
        // D = 0: only the end point; the ray height stays end tile z/4 (C-43), the end point never counts.
        assert!(has_line_of_sight_points(&sim, [x, x, 0], [x, x, 99_999]));
        // theta = +-90 degrees (D = 8 against 300 tiles of height): division by 0 in the original (C-44), flat here.
        assert!(has_line_of_sight_points(
            &sim,
            [x, x, 0],
            [x + 8, x, 300 << 12]
        ));
        assert!(has_line_of_sight_points(
            &sim,
            [x, x, 300 << 12],
            [x + 8, x, 0]
        ));
        // Through the house at (11,10) at eye height 0: blocked.
        assert!(!has_line_of_sight_points(
            &sim,
            [x, x, 0],
            [x + (3 << 12), x, 0]
        ));
        // Missing objects: no sight.
        assert!(!has_line_of_sight(
            &sim,
            crate::id::ObjectId::NONE,
            crate::id::ObjectId::NONE
        ));
    }

    #[test]
    fn the_raster_walks_quarter_tiles_including_both_ends() {
        assert_eq!(
            rasterize((0, 0), (4, 0)),
            vec![(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)]
        );
        assert_eq!(rasterize((0, 0), (0, -2)), vec![(0, 0), (0, -1), (0, -2)]);
        let r = rasterize((0, 0), (8, 2));
        assert_eq!(
            (r.first(), r.last(), r.len()),
            (Some(&(0, 0)), Some(&(8, 2)), 11)
        );
        assert_eq!(rasterize((3, 3), (3, 3)), vec![(3, 3)]);
    }

    // QUIRK C-48: deviation
    #[test]
    fn stamped_maps_survive_saving_and_loading() {
        // Stamping and removal write height and ID together; "occupied" of the thin
        // serialization (ID != 0) loses nothing in the process.
        let (rules, mut state) = world([300.0, 50.0, 100.0]);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HOUSE, 0, (10, 10));
        let g = spawn(&mut sim, HOUSE, 1, (30, 20));
        // Removal clears height and ID: without the two houses the empty map remains.
        let mut cleared = sim.state.tile_map.clone();
        cleared.remove_id(h.as_i32());
        cleared.remove_id(g.as_i32());
        assert_eq!(cleared, BuildingTileMap::new(64, 64));
        sim.state.tile_map.remove_id(g.as_i32());
        assert!(
            sim.state
                .tile_map
                .occupied()
                .all(|(_, _, id)| id == h.as_i32())
        );
        assert_eq!(sim.state.tile_map.occupied().count(), 5);
        add_building(&mut sim, g, true);
        let bytes = postcard::to_allocvec(&sim.state.tile_map).unwrap();
        let back: BuildingTileMap = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, sim.state.tile_map);
        assert!(back.occupied().any(|(_, _, id)| id == h.as_i32()));
        assert!(back.occupied().any(|(_, _, id)| id == g.as_i32()));
        assert_eq!(back.occupied().count(), 10);
    }
}
