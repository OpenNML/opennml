//! Collision manager (`GameWorld+0x76C`, re/collision part A): bucket grid of 8 x 8 tiles with three
//! lists per bucket (all objects except trees, movers, trees), the cost functions for terrain
//! footprint and overlap of units, and the search for the nearest matching object.

pub mod records;

use opennml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::fx::{fixed_div, fixed_hypot};
use crate::id::{ObjectId, TREE_SLOT};
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::motion::step::{COST_BLOCKED, COST_COLLISION, COST_OUTSIDE, StepWorld};
use crate::object::{GameObject, status};
use crate::path::LAYER_COUNT;
use crate::path::grid::{COST_FREE, COST_OUTSIDE as GRID_OUTSIDE};
use crate::sim::Sim;

/// Tiles per bucket and side.
pub const BUCKET_TILES: i32 = 8;

/// Bucket grid (`CollisionMgr_Init` `0x004B6490`): `rows = DIM_Z / 8`, `cols = DIM_X / 8`, per bucket
/// three lists in insertion order (`CPtrList::AddTail`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollisionGrid {
    pub rows: i32,
    pub cols: i32,
    /// List A (`+0x2C`): all objects of the slots != 9.
    pub all: Vec<Vec<ObjectId>>,
    /// List C (`+0x30`): movers (module 0 walk, 0x21 train or 0x33 multipart object).
    pub movers: Vec<Vec<ObjectId>>,
    /// List B (`+0x34`): slot 9 (trees).
    pub trees: Vec<Vec<ObjectId>>,
}

impl CollisionGrid {
    pub fn new(dim_x: i32, dim_z: i32) -> Self {
        let (rows, cols) = (dim_z / BUCKET_TILES, dim_x / BUCKET_TILES);
        let n = (rows.max(0) * cols.max(0)) as usize;
        Self {
            rows,
            cols,
            all: vec![Vec::new(); n],
            movers: vec![Vec::new(); n],
            trees: vec![Vec::new(); n],
        }
    }

    /// Bucket index `cols * bz + bx`, `None` outside.
    pub fn index(&self, bx: i32, bz: i32) -> Option<usize> {
        ((0..self.cols).contains(&bx) && (0..self.rows).contains(&bz))
            .then(|| (self.cols * bz + bx) as usize)
    }

    /// Change of owner: the original moves nothing (same pointer), the new ID takes the place of
    /// the old one.
    pub fn replace(&mut self, old: ObjectId, new: ObjectId) {
        for list in self
            .all
            .iter_mut()
            .chain(&mut self.movers)
            .chain(&mut self.trees)
        {
            for id in list.iter_mut().filter(|id| **id == old) {
                *id = new;
            }
        }
    }

    /// Savegame check: dimensions and list count fit the path grid (`dim_x`, `dim_z` are its sides).
    pub fn fits(&self, dim_x: i32, dim_z: i32) -> bool {
        if self.rows != dim_z / BUCKET_TILES || self.cols != dim_x / BUCKET_TILES {
            return false;
        }
        // i64: the product of corrupted rows and columns would not fit into i32.
        let n = i64::from(self.rows) * i64::from(self.cols);
        [self.all.len(), self.movers.len(), self.trees.len()]
            .iter()
            .all(|&len| len as i64 == n)
    }
}

/// Bucket of a fixed-point coordinate: `(v >> 12) / 8` (tile -1 ... -7 lands in bucket 0, quirk).
fn bucket(v: i32) -> i32 {
    (v >> 12) / BUCKET_TILES
}

fn remove_first(list: &mut Vec<ObjectId>, id: ObjectId) -> bool {
    match list.iter().position(|&x| x == id) {
        Some(p) => {
            list.remove(p);
            true
        }
        None => false,
    }
}

/// `CollisionMgr_InsertObjectAt` (`0x004B6A10`) at the grid position `pos`.
fn insert_at(sim: &mut Sim, id: ObjectId, pos: (i32, i32)) {
    let Some(o) = sim.object(id) else { return };
    let mover = o.has_module(0, true) || o.has_module(0x33, true) || o.has_module(0x21, true);
    let grid = &mut sim.state.collision;
    let Some(i) = grid.index(bucket(pos.0), bucket(pos.1)) else {
        return;
    };
    if id.slot() != TREE_SLOT {
        remove_first(&mut grid.all[i], id);
        remove_first(&mut grid.movers[i], id);
        grid.all[i].push(id);
        if mover {
            grid.movers[i].push(id);
        }
    } else {
        grid.trees[i].push(id);
    }
}

/// `CollisionMgr_InsertObject` (`0x004B7590`): at the unclamped grid position. Afterwards it
/// notifies the area queries (`notify_area_queries_insert` `0x004B62A0`,
/// [`crate::area::notify_insert`], re/collision A1.5). The reimplementation also notifies if the
/// bucket lies outside the grid and `insert_at` silently discards (the original returns there before
/// the notification): such an object can then be in a query list without being in the
/// grid. Reachable only with a position far outside the map; `remove` clears the
/// lists independent of the bucket.
pub fn insert(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let pos = (o.grid.x.0, o.grid.z.0);
    insert_at(sim, id, pos);
    crate::area::notify_insert(sim, id);
}

/// `CollisionMgr_RemoveObject` (`0x004B65C0`): from the bucket of the tile; not found -> search all
/// buckets of list A (not list B and C, quirk).
///
/// The area queries lose the object first (`notify_area_queries_remove` `0x004B62F0`,
/// [`crate::area::notify_remove`]; the original does it at the end of every path, the result is the same).
pub fn remove(sim: &mut Sim, id: ObjectId) {
    crate::area::notify_remove(sim, id);
    let Some(o) = sim.object(id) else { return };
    let (tx, tz) = o.tile;
    let grid = &mut sim.state.collision;
    if let Some(i) = grid.index(tx / BUCKET_TILES, tz / BUCKET_TILES) {
        let found = if id.slot() != TREE_SLOT {
            remove_first(&mut grid.movers[i], id);
            remove_first(&mut grid.all[i], id)
        } else {
            remove_first(&mut grid.trees[i], id)
        };
        if found {
            return;
        }
    }
    for list in &mut grid.all {
        list.retain(|&x| x != id);
    }
}

/// `CollisionMgr_GetTileCost` (`0x004B5C60`): cell of the fixed-point position in the layer (index):
/// layer outside 0...8 -> 0, outside the map `0x3FFFFFFF`, blocked `0x3FFFFFFE`.
pub fn tile_cost(sim: &Sim, x: i32, z: i32, layer: usize) -> i32 {
    if layer >= LAYER_COUNT {
        return 0;
    }
    match sim.state.path.cell_cost(x >> 12, z >> 12, layer) {
        COST_FREE => 0,
        GRID_OUTSIDE => COST_OUTSIDE,
        _ => COST_BLOCKED,
    }
}

/// `GameObject_IsCollisionExempt(pos, use_pos)` (`0x0046BA80`): on the wall, in a boat, mounted
/// or quartered; otherwise only with the exemption zone `0x1000` (object or circle).
pub fn is_collision_exempt(
    sim: &Sim,
    o: &GameObject,
    pos: Option<(i32, i32)>,
    use_pos: bool,
) -> bool {
    if o.status & 0x0200_A100 != 0 {
        return true;
    }
    if o.status & 0x1000 == 0 {
        return false;
    }
    let e = &o.exempt;
    let own = (o.grid.x.0, o.grid.z.0);
    if e.mode == 0 {
        if e.object.is_none() {
            return true;
        }
        let Some(other) = sim.object(e.object) else {
            return false;
        };
        let p = if use_pos { pos.unwrap_or(own) } else { own };
        let d = fixed_hypot(
            other.grid.x.0.wrapping_sub(p.0),
            other.grid.z.0.wrapping_sub(p.1),
        );
        return d < o.soft_radius.wrapping_add(other.soft_radius);
    }
    let p = pos.unwrap_or(own);
    fixed_hypot(p.0.wrapping_sub(e.center.0), p.1.wrapping_sub(e.center.1)) < e.radius
}

/// `CollisionMgr_GetFootprintTerrainCost` (`0x004B5E10`): center, then the four corners +-hard
/// radius in the order (+,+), (-,+), (+,-), (-,-); the first value above the threshold wins
/// (threshold 0, for exempt objects `0x3FFFFFFE`).
pub fn footprint_terrain_cost(sim: &Sim, x: i32, z: i32, id: ObjectId, layer: usize) -> i32 {
    let Some(o) = sim.object(id) else {
        return tile_cost(sim, x, z, layer);
    };
    let r = o.hard_radius;
    let threshold = if is_collision_exempt(sim, o, Some((x, z)), false) {
        COST_BLOCKED
    } else {
        0
    };
    for (px, pz) in [
        (x, z),
        (x.wrapping_add(r), z.wrapping_add(r)),
        (x.wrapping_sub(r), z.wrapping_add(r)),
        (x.wrapping_add(r), z.wrapping_sub(r)),
        (x.wrapping_sub(r), z.wrapping_sub(r)),
    ] {
        let c = tile_cost(sim, px, pz, layer);
        if c > threshold {
            return c;
        }
    }
    0
}

/// `GameObject_AreOwnersFriends` (`0x00476220`): friendship of the slots (with `CARRIING` that of the
/// carried object), only slots 0-7.
fn owners_are_friends(sim: &Sim, a: &GameObject, b: &GameObject) -> bool {
    let slot = |o: &GameObject| {
        let t = o.target;
        if o.status & status::CARRIING != 0 && !t.is_none() && t.0 != 0 {
            sim.object(t).map_or(o.owner(), GameObject::owner)
        } else {
            o.owner()
        }
    };
    sim.state
        .diplomacy
        .is_friend(i32::from(slot(a)), i32::from(slot(b)))
}

/// `CollisionMgr_GetPairOverlapCost` (`0x004B5F60`): overlap of the bucket object `a` with `b`
/// (positions optionally replaced): hard `max(0x1000, Fixed_Div(ov, s) << 9)`, plus soft
/// `Fixed_Div(ov', s')`. `moving` skips moving units, `friends` befriended ones. The
/// seventh argument of the original has no effect.
///
/// Deviation (K-326): if the sum of the soft radii is 0, the soft part is omitted. The original divides by
/// zero there as soon as the distance is negative (`fixed_hypot` for positions 512 ... 1024 tiles apart) and
/// crashes; all other inputs compute bit-identically. The hard divisor is never 0: both hard radii are >= 1
/// here, so their sum lies in 2 ... 2^32 - 2.
///
/// QUIRK K-326: deviation
pub fn pair_overlap_cost(
    sim: &mut Sim,
    a: ObjectId,
    b: ObjectId,
    pos_b: Option<(i32, i32)>,
    pos_a: Option<(i32, i32)>,
    moving: bool,
    friends: bool,
) -> i32 {
    let (Some(oa), Some(_)) = (sim.object(a), sim.object(b)) else {
        return 0;
    };
    if is_collision_exempt(sim, oa, None, false) || oa.hard_radius <= 0 {
        return 0;
    }
    let (a_status, a_target) = (oa.status, oa.target);
    if moving
        && a_status & status::MOVE != 0
        && a_status & 0x10 == 0
        && send(sim, a, &Message::new(0x1D00, &[])) == 0
    {
        return 0;
    }
    let (Some(oa), Some(ob)) = (sim.object(a), sim.object(b)) else {
        return 0;
    };
    let friendly = owners_are_friends(sim, oa, ob);
    if is_collision_exempt(sim, ob, None, false) || ob.hard_radius <= 0 {
        return 0;
    }
    if a_status & 0xE000 != 0 && ob.id == a_target {
        return 0;
    }
    if friends && friendly {
        return 0;
    }
    if (a_status ^ ob.status) & 0x2_0000 != 0 {
        return 0;
    }
    let (ra, rb, sa, sb) = (
        oa.hard_radius,
        ob.hard_radius,
        oa.soft_radius,
        ob.soft_radius,
    );
    let pb = pos_b.unwrap_or((ob.grid.x.0, ob.grid.z.0));
    let pa = pos_a.unwrap_or((oa.grid.x.0, oa.grid.z.0));
    // Underwater animal against deep-sea unit (both directions).
    for (x, y) in [(a, b), (b, a)] {
        if send(sim, x, &Message::new(0xA14, &[])) == 1
            && send(sim, y, &Message::new(0x3700, &[])) == 1
        {
            return 0;
        }
    }
    let d = fixed_hypot(pa.0.wrapping_sub(pb.0), pa.1.wrapping_sub(pb.1));
    let mut r = 0;
    let s = rb.wrapping_add(ra);
    if d < s {
        let ov = rb.wrapping_sub(d).wrapping_add(ra);
        r = fixed_div(ov, s).wrapping_shl(9).max(COST_COLLISION);
    }
    let ss = sb.wrapping_add(sa);
    if d < ss && ss != 0 {
        r = r.wrapping_add(fixed_div(sb.wrapping_sub(d).wrapping_add(sa), ss));
    }
    r
}

/// `CollisionMgr_GetCostAt` (`0x004B6730`): first the terrain (with a unit the footprint, otherwise
/// the cell; > 0 aborts), then the maximum of the overlaps with all objects of list A in
/// the 3 x 3 buckets around the position (z outer, x inner), excluding the unit itself.
pub fn cost_at(
    sim: &mut Sim,
    p: (i32, i32),
    me: Option<ObjectId>,
    layer: usize,
    moving: bool,
    friends: bool,
) -> i32 {
    let mut c = match me {
        Some(id) => footprint_terrain_cost(sim, p.0, p.1, id, layer),
        None => tile_cost(sim, p.0, p.1, layer),
    };
    if c > 0 {
        return c;
    }
    let (rows, cols) = (sim.state.collision.rows, sim.state.collision.cols);
    let (bx, bz) = (bucket(p.0), bucket(p.1));
    let clamp = |v: i32, n: i32| v.max(0).min(n - 1);
    let (z0, z1) = (clamp(bz - 1, rows), clamp(bz + 1, rows));
    let (x0, x1) = (clamp(bx - 1, cols), clamp(bx + 1, cols));
    let Some(me) = me else {
        // Without a querying unit the original crashes in the friend check; no caller does that.
        return c;
    };
    for z in z0..=z1 {
        for x in x0..=x1 {
            let i = (cols * z + x) as usize;
            let list = sim.state.collision.all.get(i).cloned().unwrap_or_default();
            for o in list {
                if o == me {
                    continue;
                }
                let v = pair_overlap_cost(sim, o, me, Some(p), None, moving, friends);
                if v > c {
                    c = v;
                }
            }
        }
    }
    c
}

/// `CollisionMgr_QueryCostAt` (`0x004B7310`).
pub fn query_cost_at(
    sim: &mut Sim,
    x: i32,
    z: i32,
    me: Option<ObjectId>,
    layer: usize,
    moving: bool,
    friends: bool,
) -> i32 {
    cost_at(sim, (x, z), me, layer, moving, friends)
}

/// Step of the ring search `DAT_006719E0` (static `Fixed_FromInt(1) >> 1 >> 1`): a
/// quarter tile.
pub const SPOT_STEP: i32 = 0x400;
/// Reach of the ring search (`Fixed_FromInt(5)`): rings at distance `0, 1, … < 5 · 4096` in
/// steps of [`SPOT_STEP`], i.e. 20 rings up to 4.75 tiles.
pub const SPOT_REACH: i32 = 5 << 12;

/// `CollisionMgr_FindLowestCostSpotNear(pos, unit)` (`0x004B73B0`, building exit
/// `PropertyBuilding_SendUnitToMeetingPoint`): square rings around `pos` (ring `k` with half
/// side `k ·` [`SPOT_STEP`]); per ring first the two rows (x ascending, per x first the
/// row with the smaller z), then the left and right column (z ascending, corners twice). Evaluated
/// is `QueryCostAt(x, z, unit, layer from 0xB00, skip moving, count friends)`; a
/// point replaces `pos` only with **strictly** smaller cost (the first wins). Return value: a
/// point was evaluated (cost below `i32::MAX`). The height stays.
///
/// Deviation (exception A7): the loop variables compute in `i64` - the original loops in circles with a
/// point near the `i32` limit (loop overflow), the queried points themselves
/// are formed with overflow as in the original.
pub fn find_lowest_cost_spot_near(sim: &mut Sim, pos: &mut (i32, i32), unit: ObjectId) -> bool {
    if sim.object(unit).is_none() {
        return false;
    }
    let layer = send(sim, unit, &Message::new(0xB00, &[]));
    // Layer -1 (without walk) lies outside 0...8: the terrain then costs nothing (`GetTileCost`).
    let layer = usize::try_from(layer).unwrap_or(usize::MAX);
    let (x0, z0) = (i64::from(pos.0), i64::from(pos.1));
    let off = z0 - x0;
    let step = i64::from(SPOT_STEP);
    let mut best = i32::MAX;
    let mut probe = |sim: &mut Sim, x: i64, z: i64, pos: &mut (i32, i32)| {
        let (x, z) = (x as i32, z as i32);
        let c = query_cost_at(sim, x, z, Some(unit), layer, true, false);
        if c < best {
            *pos = (x, z);
            best = c;
        }
    };
    let (mut lo, mut hi, mut ring) = (x0, x0, 0i64);
    while ring < i64::from(SPOT_REACH) {
        let mut x = lo;
        while x <= hi {
            probe(sim, x, lo + off, pos);
            probe(sim, x, hi + off, pos);
            x += step;
        }
        let mut z = lo + off;
        while z <= hi + off {
            probe(sim, lo, z, pos);
            probe(sim, hi, z, pos);
            z += step;
        }
        ring += step;
        lo -= step;
        hi += step;
    }
    best < i32::MAX
}

/// Costs and movement check from the view of a unit (`BStarPlanner_GetNodeCost` `0x00407FD0`,
/// `Motion_IsStepBlocked` `0x00446170`): terrain footprint in the layer `layer`, units via
/// `QueryCostAt`; without a unit only the cell.
pub struct UnitView<'a, 'b> {
    pub sim: &'a mut Sim<'b>,
    pub unit: Option<ObjectId>,
    pub layer: usize,
}

impl StepWorld for UnitView<'_, '_> {
    fn footprint_cost(&mut self, x: i32, z: i32) -> i32 {
        match self.unit {
            Some(u) => footprint_terrain_cost(self.sim, x, z, u, self.layer),
            None => tile_cost(self.sim, x, z, self.layer),
        }
    }

    fn query_cost(&mut self, x: i32, z: i32, friends: bool) -> i32 {
        query_cost_at(self.sim, x, z, self.unit, self.layer, false, friends)
    }
}

/// `CollisionMgr_FindNearestMatching` (`0x004B7620`, re/collision A3.1): own bucket,
/// then rings of buckets; within a bucket the first entered wins on a tie, across buckets only
/// a strictly smaller distance; return in the first ring with a hit inside the radius.
/// List B (trees) with filter mode `0x1000` or player 9, otherwise list A. `edge_ofs ≥ 0`: distance
/// = edge distance of the candidate to the point ([`edge_distance_to_point`]), the radius grows by
/// `edge_ofs`; `edge_ofs = −1`: center distance. Radius < 0 = whole map.
pub fn find_nearest_matching(
    sim: &mut Sim,
    pos: (i32, i32),
    filter: &MatchFilter,
    radius: i32,
    player: i32,
    edge_ofs: i32,
) -> Option<ObjectId> {
    find_nearest_matching_seen(sim, pos, filter, radius, player, edge_ofs, None)
}

/// [`find_nearest_matching`] with a sight check per candidate (extension of the reimplementation for
/// `player::Sight::Fog`; the original AI sees everything, E-25): with `seen_by = Some(p)`
/// a candidate only counts if `p` sees it (`crate::filter::seen_by`); an unseen
/// candidate behaves like a missing one (it also does not raise `range_max` in edge mode). The
/// check lies **in** the search: a hidden nearer hit does not hide a visible farther
/// one. `None` computes exactly like [`find_nearest_matching`].
pub fn find_nearest_matching_seen(
    sim: &mut Sim,
    pos: (i32, i32),
    filter: &MatchFilter,
    radius: i32,
    player: i32,
    edge_ofs: i32,
    seen_by: Option<u8>,
) -> Option<ObjectId> {
    let (rows, cols) = (sim.state.collision.rows, sim.state.collision.cols);
    let (bx, bz) = (bucket(pos.0), bucket(pos.1));
    let edge = edge_ofs >= 0;
    let m = cols.max(rows);
    let (rings, radius) = if radius < 0 {
        (m, i32::MAX)
    } else {
        let radius = if edge {
            radius.wrapping_add(edge_ofs)
        } else {
            radius
        };
        let k = bucket(radius) + 1;
        (k.min(m), radius)
    };
    let search = |sim: &mut Sim, x: i32, z: i32| {
        nearest_in_bucket(
            sim,
            (x, z),
            pos,
            filter,
            player,
            radius,
            edge,
            seen_by,
            None,
        )
    };
    let (mut best, mut bd) = match search(sim, bx, bz) {
        Some((id, d)) => (Some(id), d),
        None => (None, i32::MAX),
    };
    for k in 1..=rings {
        for i in 0..2 * k {
            for (ex, ez) in [
                (bx - k + i, bz - k),
                (bx - k + 1 + i, bz + k),
                (bx - k, bz - k + 1 + i),
                (bx + k, bz - k + i),
            ] {
                if let Some((id, d)) = search(sim, ex, ez)
                    && d < bd
                {
                    best = Some(id);
                    bd = d;
                }
            }
        }
        if best.is_some() && bd <= radius {
            return best;
        }
    }
    best.filter(|_| bd <= radius)
}

/// `SpatialGrid_QueryRadius` (`0x004B6CD0`, re/collision): like
/// [`find_nearest_matching`] over own bucket and rings, but in collect mode - **all** hits
/// in the radius in the order found (bucket: center, then rings; within a bucket insertion order), without
/// aborting after the first ring with a hit. Radius < 0 = whole map; unlike in the search
/// the radius does not grow in edge mode (`edge`). The original reports "list not empty".
pub fn query_radius(
    sim: &mut Sim,
    pos: (i32, i32),
    filter: &MatchFilter,
    radius: i32,
    player: i32,
    edge: bool,
) -> Vec<ObjectId> {
    let (rows, cols) = (sim.state.collision.rows, sim.state.collision.cols);
    let (bx, bz) = (bucket(pos.0), bucket(pos.1));
    let m = cols.max(rows);
    let (rings, radius) = if radius < 0 {
        (m, i32::MAX)
    } else {
        ((bucket(radius) + 1).min(m), radius)
    };
    let mut out = Vec::new();
    nearest_in_bucket(
        sim,
        (bx, bz),
        pos,
        filter,
        player,
        radius,
        edge,
        None,
        Some(&mut out),
    );
    for k in 1..=rings {
        for i in 0..2 * k {
            for (ex, ez) in [
                (bx - k + i, bz - k),
                (bx - k + 1 + i, bz + k),
                (bx - k, bz - k + 1 + i),
                (bx + k, bz - k + i),
            ] {
                nearest_in_bucket(
                    sim,
                    (ex, ez),
                    pos,
                    filter,
                    player,
                    radius,
                    edge,
                    None,
                    Some(&mut out),
                );
            }
        }
    }
    out
}

/// `CollisionMgr_FindNearestInBucket` (`0x004B6B40`), without a collect list (`out = None`: the
/// nearest hit) or with one (append all hits in the radius). The filter goes in as a copy
/// (value parameter); with edge distance and `range_max ≥ 0`, `range_max` is raised by the hard radius
/// of the candidate before every filter check - summed up across the candidates of the
/// bucket (quirk). `seen_by`: skip unseen candidates
/// ([`find_nearest_matching_seen`]).
#[allow(clippy::too_many_arguments)]
fn nearest_in_bucket(
    sim: &mut Sim,
    (x, z): (i32, i32),
    pos: (i32, i32),
    filter: &MatchFilter,
    player: i32,
    radius: i32,
    edge: bool,
    seen_by: Option<u8>,
    mut out: Option<&mut Vec<ObjectId>>,
) -> Option<(ObjectId, i32)> {
    let grid = &sim.state.collision;
    let i = grid.index(x, z)?;
    let list = if filter.mode & 0x1000 != 0 || player == i32::from(TREE_SLOT) {
        grid.trees[i].clone()
    } else {
        grid.all[i].clone()
    };
    let mut copy: Option<MatchFilter> = None;
    let mut best: Option<(ObjectId, i32)> = None;
    for id in list {
        let Some(o) = sim.object(id) else { continue };
        let (centre, hard) = ((o.grid.x.0, o.grid.z.0), o.hard_radius);
        let d = if edge {
            edge_distance_to_point(sim, id, pos)
        } else {
            fixed_pos_distance(centre, pos)
        };
        if d > radius || (out.is_none() && best.is_some_and(|(_, bd)| d >= bd)) {
            continue;
        }
        if seen_by.is_some_and(|p| !crate::filter::seen_by(sim, id, p)) {
            continue;
        }
        if edge && copy.as_ref().unwrap_or(filter).range_max >= 0 {
            let f = copy.get_or_insert_with(|| filter.clone());
            f.range_max = f.range_max.wrapping_add(hard);
        }
        if !crate::filter::matches(sim, id, copy.as_ref().unwrap_or(filter), player) {
            continue;
        }
        match out.as_deref_mut() {
            Some(list) => list.push(id),
            None => best = Some((id, d)),
        }
    }
    best
}

/// Distance of two grid points (`FixedPos_Distance` `0x00401200` = `Fixed_Hypot` of the difference).
pub fn fixed_pos_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    fixed_hypot(a.0.wrapping_sub(b.0), a.1.wrapping_sub(b.1))
}

/// Quartered (`status & 0x100`): the container `+0x6E0` (`GameObject_GetQuarteringContainer`
/// `0x00466BB0`, also dying ones).
pub fn quartering_container(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let o = sim.object(id)?;
    if o.status & status::QUARTERED == 0 {
        return None;
    }
    sim.object(o.carrier).map(|c| c.id)
}

/// Greatest depth of nested containers (protection against corrupted savegames; the original follows
/// the chain without a limit).
const MAX_CONTAINER_DEPTH: usize = 16;

/// Container with an active building module in which `id` is quartered.
fn building_container(sim: &Sim, id: ObjectId, depth: usize) -> Option<ObjectId> {
    quartering_container(sim, id).filter(|&c| {
        depth < MAX_CONTAINER_DEPTH && sim.object(c).is_some_and(|c| c.has_module(3, false))
    })
}

/// `GameObject_GetEdgeDistanceToPoint` (`0x00468930`, re/walk 16.2): building (module 3
/// active) -> distance to the edge point that the building writes into the arguments on `0x3803 {x, z}`;
/// quartered in a building -> its edge distance; otherwise center distance minus hard radius
/// (only if > 0). Never negative. The square branch of the modules `0x29`/`0x32` is dead (both without a
/// factory class) and missing.
pub fn edge_distance_to_point(sim: &mut Sim, id: ObjectId, p: (i32, i32)) -> i32 {
    edge_distance_to_point_at(sim, id, p, 0)
}

fn edge_distance_to_point_at(sim: &mut Sim, id: ObjectId, p: (i32, i32), depth: usize) -> i32 {
    let Some(o) = sim.object(id) else { return 0 };
    let (centre, hard, building) = (
        (o.grid.x.0, o.grid.z.0),
        o.hard_radius,
        o.has_module(3, false),
    );
    let d = if building {
        let mut m = Message::new(0x3803, &[p.0, p.1]);
        send_mut(sim, id, &mut m);
        fixed_pos_distance(p, (m.arg(0), m.arg(1)))
    } else if let Some(c) = building_container(sim, id, depth) {
        return edge_distance_to_point_at(sim, c, p, depth + 1);
    } else {
        let d = fixed_pos_distance(p, centre);
        if hard > 0 { d.wrapping_sub(hard) } else { d }
    };
    d.max(0)
}

/// `GameObject_GetEdgeDistanceToObject(a, b)` (`0x0046A7F0`): `a` building -> edge point toward
/// `b` (`0x3803 {b.x, b.z}`), of that the edge distance of `b`; `a` quartered in a building ->
/// from the building; otherwise edge distance of `b` to the center of `a`, minus hard radius of `a`
/// (only if >= 1). Never negative.
pub fn edge_distance_to_object(sim: &mut Sim, a: ObjectId, b: ObjectId) -> i32 {
    edge_distance_to_object_at(sim, a, b, 0)
}

fn edge_distance_to_object_at(sim: &mut Sim, a: ObjectId, b: ObjectId, depth: usize) -> i32 {
    let Some(oa) = sim.object(a) else { return 0 };
    let (centre, hard, building) = (
        (oa.grid.x.0, oa.grid.z.0),
        oa.hard_radius,
        oa.has_module(3, false),
    );
    let target = sim.object(b).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
    let d = if building {
        let mut m = Message::new(0x3803, &[target.0, target.1]);
        send_mut(sim, a, &mut m);
        edge_distance_to_point(sim, b, (m.arg(0), m.arg(1)))
    } else if let Some(c) = building_container(sim, a, depth) {
        return edge_distance_to_object_at(sim, c, b, depth + 1);
    } else {
        let d = edge_distance_to_point(sim, b, centre);
        if hard >= 1 { d.wrapping_sub(hard) } else { d }
    };
    d.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Fx, FxVec3};
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, destroy_object, set_grid_position};

    const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\n";

    fn setup() -> (Rules, SimState) {
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        (rules, state)
    }

    fn unit(sim: &mut Sim, guid: u32, slot: i32, x: f32, z: f32) -> ObjectId {
        create_object(sim, &mut ObjectDesc::new(guid, slot, [x, 0.0, z])).unwrap()
    }

    fn radii(sim: &mut Sim, id: ObjectId, hard: i32, soft: i32) {
        let o = sim.object_mut(id).unwrap();
        o.hard_radius = hard;
        o.soft_radius = soft;
    }

    #[test]
    fn objects_enter_their_buckets_and_follow_tile_changes() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        // 32 tiles -> 4 x 4 buckets; tile (1, 1) -> bucket 0, tile (9, 1) -> bucket 1.
        let a = unit(&mut sim, 1100, 0, 200.0, 200.0);
        let hq = unit(&mut sim, 1000, 0, 210.0, 200.0);
        let b = unit(&mut sim, 1100, 1, 9.0 * 128.0, 200.0);
        let tree = unit(&mut sim, 3602, 0, 300.0, 300.0);
        let g = &sim.state.collision;
        assert_eq!((g.rows, g.cols, g.all.len()), (4, 4, 16));
        assert_eq!(g.all[0], [a, hq]);
        assert_eq!(g.movers[0], [a]);
        assert_eq!(
            (g.all[1].clone(), g.trees[0].clone()),
            (vec![b], vec![tree])
        );
        // Tile change into the same bucket: re-entered at the back.
        set_grid_position(
            &mut sim,
            a,
            FxVec3::new(Fx(2 << 12), Fx(0), Fx(1 << 12)),
            false,
        );
        assert_eq!(sim.state.collision.all[0], [hq, a]);
        // Into another bucket, then destroy.
        set_grid_position(
            &mut sim,
            a,
            FxVec3::new(Fx(17 << 12), Fx(0), Fx(9 << 12)),
            false,
        );
        assert_eq!(sim.state.collision.all[2 + 4], [a]);
        assert_eq!(sim.state.collision.movers[6], [a]);
        destroy_object(&mut sim, a);
        assert!(sim.state.collision.all[6].is_empty() && sim.state.collision.movers[6].is_empty());
        // Outside the map (over the upper edge) nothing is entered.
        set_grid_position(
            &mut sim,
            b,
            FxVec3::new(Fx(40 << 12), Fx(0), Fx(1 << 12)),
            false,
        );
        assert!(sim.state.collision.all.iter().all(|l| !l.contains(&b)));
        // Tile -1 ... -7 lands in bucket 0 (rounding toward 0).
        assert_eq!(bucket(-4096), 0);
        assert_eq!(bucket(-8 * 4096), -1);
    }

    #[test]
    fn the_lowest_cost_spot_is_searched_in_quarter_tile_rings() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let me = unit(&mut sim, 1100, 0, 300.0, 300.0);
        let center = (10 << 12, 10 << 12);
        // Free map: the center (first point) costs 0 and stays.
        let mut p = center;
        assert!(find_lowest_cost_spot_near(&mut sim, &mut p, me));
        assert_eq!(p, center);
        // A standing unit in the center: ring 1 (a quarter tile, diagonal 1448)
        // still overlaps softly, ring 2 starts at the bottom left with cost 0.
        let blocker = unit(&mut sim, 1100, 1, 1280.0, 1280.0);
        radii(&mut sim, blocker, 512, 1024);
        radii(&mut sim, me, 512, 1024);
        assert_eq!(sim.object(blocker).unwrap().grid.x.0, center.0);
        let mut p = center;
        assert!(find_lowest_cost_spot_near(&mut sim, &mut p, me));
        assert_eq!(p, (center.0 - 2 * SPOT_STEP, center.1 - 2 * SPOT_STEP));
        // Without a unit: nothing searched.
        let mut p = center;
        destroy_object(&mut sim, me);
        assert!(!find_lowest_cost_spot_near(&mut sim, &mut p, me));
        assert_eq!(p, center);
    }

    #[test]
    fn pair_costs_follow_the_radii_and_the_table_hypot() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let a = unit(&mut sim, 1100, 0, 1000.0, 1000.0);
        let b = unit(&mut sim, 1100, 1, 1000.0, 1000.0);
        radii(&mut sim, a, 512, 1024);
        radii(&mut sim, b, 512, 1024);
        let pa = |sim: &Sim| {
            (
                sim.object(a).unwrap().grid.x.0,
                sim.object(a).unwrap().grid.z.0,
            )
        };
        let at = |sim: &Sim, d: i32| Some((pa(sim).0 + d, pa(sim).1));
        // re/collision A6: T1 far, T2 soft, T3 hard, T4b smallest hard overlap, T5 coincident.
        for (d, want) in [
            (4096, 0),
            (1536, 1024),
            (768, 526_848),
            (1023, 10248),
            (0, 0x0020_1000),
        ] {
            let p = at(&sim, d);
            assert_eq!(
                pair_overlap_cost(&mut sim, a, b, p, None, false, false),
                want,
                "d = {d}"
            );
        }
        // Cannon 1000/1500 against human at 1200 (T6), symmetric.
        radii(&mut sim, a, 1000, 1500);
        let p = at(&sim, 1200);
        assert_eq!(
            pair_overlap_cost(&mut sim, a, b, p, None, false, false),
            434_276
        );
        radii(&mut sim, a, 4200, 8000);
        radii(&mut sim, b, 4200, 8000);
        // Warships: minimum value 0x1000 (T4c).
        let p = at(&sim, 8396);
        assert_eq!(
            pair_overlap_cost(&mut sim, a, b, p, None, false, false),
            6040
        );
        // Hard 0 (ghost) never collides; friends only without `friends`.
        radii(&mut sim, a, 512, 1024);
        radii(&mut sim, b, 512, 1024);
        let p = at(&sim, 768);
        sim.state.diplomacy.set_friend(0, 1, true);
        assert_eq!(pair_overlap_cost(&mut sim, a, b, p, None, false, true), 0);
        assert_eq!(
            pair_overlap_cost(&mut sim, a, b, p, None, false, false),
            526_848
        );
        radii(&mut sim, a, 0, 1024);
        assert_eq!(pair_overlap_cost(&mut sim, a, b, p, None, false, false), 0);
        // Quartered: exempt.
        radii(&mut sim, a, 512, 1024);
        sim.object_mut(b).unwrap().status |= 0x100;
        assert_eq!(pair_overlap_cost(&mut sim, a, b, p, None, false, false), 0);
    }

    #[test]
    fn footprints_check_centre_and_corners_and_queries_take_the_maximum() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let me = unit(&mut sim, 1100, 0, 1000.0, 1000.0);
        radii(&mut sim, me, 512, 1024);
        // T7: tile (2, 1) blocked, corner (+r, +r) of (0x1E00, 0x1800) hits it.
        sim.state.path.mark_cell(1, 2, 1, true);
        assert_eq!(
            footprint_terrain_cost(&sim, 0x1E00, 0x1800, me, 0),
            COST_BLOCKED
        );
        assert_eq!(footprint_terrain_cost(&sim, 0x1C00, 0x1800, me, 0), 0);
        // T8b: corner (-r, +r) outside the map.
        assert_eq!(footprint_terrain_cost(&sim, 300, 300, me, 0), COST_OUTSIDE);
        // Layer beyond 8: free.
        assert_eq!(tile_cost(&sim, 0x2000, 0x1000, 9), 0);
        // T9: friend hard, enemy soft, third far away -> maximum.
        let friend = unit(&mut sim, 1100, 1, 1000.0 + 768.0 / 32.0, 1000.0);
        let enemy = unit(&mut sim, 1100, 2, 1000.0 + 1536.0 / 32.0, 1000.0);
        let far = unit(&mut sim, 1100, 2, 1000.0 + 9.0 * 128.0, 1000.0);
        for id in [friend, enemy, far] {
            radii(&mut sim, id, 512, 1024);
        }
        sim.state.diplomacy.set_friend(1, 0, true);
        let p = (
            sim.object(me).unwrap().grid.x.0,
            sim.object(me).unwrap().grid.z.0,
        );
        assert_eq!(
            query_cost_at(&mut sim, p.0, p.1, Some(me), 0, false, false),
            526_848
        );
        assert_eq!(
            query_cost_at(&mut sim, p.0, p.1, Some(me), 0, false, true),
            1024
        );
        // Blocked own tile comes before the units.
        let t = (p.0 >> 12, p.1 >> 12);
        sim.state.path.mark_cell(1, t.0, t.1, true);
        assert_eq!(
            query_cost_at(&mut sim, p.0, p.1, Some(me), 0, false, false),
            COST_BLOCKED
        );
    }

    #[test]
    fn nearest_search_measures_edges_widens_the_radius_and_sums_hard_radii() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let at = |sim: &mut Sim, x: i32, z: i32, hard: i32| {
            let id = unit(sim, 1100, 0, 10.0, 10.0);
            set_grid_position(sim, id, FxVec3::new(Fx(x), Fx(0), Fx(z)), false);
            radii(sim, id, hard, hard);
            id
        };
        // Reference r at the search position, c1 5000 away (edge 4000), c2 3500 away (edge 2500).
        let p = (40_000, 40_000);
        let r = at(&mut sim, p.0, p.1, 0);
        let c1 = at(&mut sim, 45_000, 40_000, 1000);
        let c2 = at(&mut sim, 40_000, 43_500, 1000);
        assert_eq!(sim.state.collision.all[5], [r, c1, c2]);
        assert_eq!(edge_distance_to_point(&mut sim, c1, p), 4000);
        assert_eq!(edge_distance_to_point(&mut sim, c2, p), 2500);
        assert_eq!(edge_distance_to_point(&mut sim, r, p), 0);
        // Without a filter condition (AND mode): center -> r, edge distance without r -> c2.
        let any = MatchFilter {
            mode: 0x10,
            ..MatchFilter::default()
        };
        assert_eq!(
            find_nearest_matching(&mut sim, (p.0, p.1 + 6000), &any, -1, 0, -1),
            Some(c2)
        );
        assert_eq!(
            find_nearest_matching(&mut sim, (p.0 + 5000, p.1), &any, -1, 0, -1),
            Some(c1)
        );
        // Edge distance: c1 from (40000, 44000) is 5000 - 1000 = 4000, c2 500 - 1000 -> 0.
        assert_eq!(
            find_nearest_matching(&mut sim, (p.0, 44_000), &any, -1, 0, 0),
            Some(c2)
        );
        // Radius 2000 reaches c2 (edge 2500) only with `edge_ofs` 500.
        assert_eq!(
            find_nearest_matching(&mut sim, p, &any, 2000, 0, 0),
            Some(r)
        );
        let not_r = MatchFilter {
            mode: 0x10 | 0x10000 | 0x8,
            guid: 1100,
            ..MatchFilter::default()
        };
        assert_eq!(find_nearest_matching(&mut sim, p, &not_r, 2000, 0, 0), None);
        // Distance range to r (1 ... 2000): with edge distance `range_max` grows by the hard
        // radii of all checked candidates of the bucket (r +0, c1 +1000 -> 3000 < 5000, c2 +1000 ->
        // 4000 >= 3500): quirk, c2 matches. Without edge distance it stays at 2000.
        let range = MatchFilter {
            mode: 0x20 | 0x10,
            ref_object: r.as_i32(),
            range_min: 1,
            range_max: 2000,
            tether: 0,
            ..MatchFilter::constructed()
        };
        assert_eq!(
            find_nearest_matching(&mut sim, p, &range, -1, 0, 0),
            Some(c2)
        );
        assert_eq!(find_nearest_matching(&mut sim, p, &range, -1, 0, -1), None);
        // Building queries and containers: edge distance between objects (without a building: edge of
        // b to the center of a minus R(a)).
        assert_eq!(fixed_hypot(5000, 3500), 6099);
        assert_eq!(
            edge_distance_to_object(&mut sim, c1, c2),
            6099 - 1000 - 1000
        );
    }

    #[test]
    fn radius_queries_collect_every_hit_in_bucket_and_ring_order() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let tile = |t: i32| t as f32 * 128.0 + 64.0;
        let mut at = |x: i32, z: i32| unit(&mut sim, 1100, 0, tile(x), tile(z));
        // Search point tile (12, 12) = bucket (1, 1); ring 1 in the order (0,0), (1,2), (0,1),
        // (2,0), (1,0), (2,2), (0,2), (2,1); (3,3) lies in ring 2.
        let (u1, u2) = (at(13, 13), at(9, 9));
        let u3 = at(2, 2);
        let u4 = at(20, 12);
        let u5 = at(12, 20);
        let u6 = at(28, 28);
        let p = ((12 << 12) + 0x800, (12 << 12) + 0x800);
        let any = MatchFilter {
            mode: 0x10,
            ..MatchFilter::default()
        };
        assert_eq!(
            query_radius(&mut sim, p, &any, -1, 0, false),
            [u1, u2, u3, u5, u4, u6]
        );
        // Radius 8 tiles (center): two rings, (2, 2) and (28, 28) too far.
        assert_eq!(
            query_radius(&mut sim, p, &any, 8 << 12, 0, false),
            [u1, u2, u5, u4]
        );
        // The search only returns the nearest.
        assert_eq!(
            find_nearest_matching(&mut sim, p, &any, -1, 0, -1),
            Some(u1)
        );
        let none = MatchFilter {
            mode: 0x10 | 0x8,
            guid: 4711,
            ..MatchFilter::default()
        };
        assert!(query_radius(&mut sim, p, &none, -1, 0, false).is_empty());
    }

    #[test]
    fn owner_changes_keep_the_bucket_position() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let a = unit(&mut sim, 1100, 0, 200.0, 200.0);
        let b = unit(&mut sim, 1100, 0, 210.0, 200.0);
        let new = crate::world::change_owner(&mut sim, a, 3).unwrap();
        assert_eq!(sim.state.collision.all[0], [new, b]);
        assert_eq!(sim.state.collision.movers[0], [new, b]);
    }

    #[test]
    fn grids_of_another_size_do_not_fit_and_never_overflow() {
        // Corrupted savegame: rows and columns so large that their product does not fit into i32.
        let huge = CollisionGrid {
            rows: 100_000,
            cols: 100_000,
            all: Vec::new(),
            movers: Vec::new(),
            trees: Vec::new(),
        };
        assert!(!huge.fits(800_000, 800_000));
        assert!(!huge.fits(i32::MAX, i32::MAX));
        assert!(!huge.fits(800_000, 0));
        // Matching size: dimensions and list count agree.
        let grid = CollisionGrid::new(64, 32);
        assert!(grid.fits(64, 32));
        assert!(!grid.fits(32, 64));
    }
}
