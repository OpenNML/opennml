//! Building placement check (re/construction): tile rectangle of a building site
//! (`GameWorld_ComputePlacementTileRect`), the check with all branches (`GameEngine_CheckBuildingPlacement`),
//! the search for a free spot in rings (`GameEngine_FindBuildingPlacementNear`) and a facade for commands and
//! viewer.
//!
//! Floating point as in the original: every intermediate quantity is an `f32` (x87 with a 24-bit mantissa and
//! `FSTP dword`). The check changes `pos`/`rot` like the original, even if it fails afterwards (field/gold mine:
//! position of the deposit, gate: rotation of the wall).

use opennml_formats::targets::MatchFilter;

use crate::collision::find_nearest_matching;
use crate::footprint::{transform_coord, world_matrix};
use crate::fx::{Fx, ftol};
use crate::id::{NATURE_SLOT, ObjectId};
use crate::message::Message;
use crate::messaging::send;
use crate::rules::{MapInfo, Rules};
use crate::sim::Sim;
use crate::world::{region_id_at, set_grid_position, world_to_grid};

/// Building flags (value 40003 of module 3) that the check evaluates.
pub mod flags {
    /// Tower: snaps to an own wall piece within one tile.
    pub const TOWER: u32 = 0x1_0000;
    /// Field/farm: needs fertile soil within 5 tiles.
    pub const FIELD: u32 = 0x200;
    /// Gate: needs an own wall piece under every tile.
    pub const GATE: u32 = 0x2_0000;
    /// Gold mine: needs a gold deposit within 5 tiles.
    pub const GOLD_MINE: u32 = 0x100;
    /// Harbor/shipyard: water and land in proportion.
    pub const HARBOR: u32 = 0x8000;
    /// Wall piece: only the first tile counts.
    pub const WALL_SEGMENT: u32 = 0x10_0000;
    /// Wall onto which a gate fits (the straight pieces; corners do not have it).
    pub const GATE_WALL: u32 = 0x20_0000;
}

/// `DAT_0067137C`: search radius one tile (fixed point).
const ONE_TILE: i32 = 0x1000;
/// `DAT_00671380`: search radius half a tile (fixed point).
const HALF_TILE: i32 = 0x800;
/// Search radius for deposits (`Fixed_FromInt(5)`).
const DEPOSIT_RADIUS: i32 = 5 << 12;
/// Filter "own building with all bits": `ANDALL | MYLIST | CHECKHANDLETYPE`.
const MODE_OWN_BUILDING: u32 = 0x112;
/// Filter "type bits only" (`CHECKHANDLETYPE`).
const MODE_TYPE_BITS: u32 = 0x2;
/// Filter "tree list" (`TREELIST`).
const MODE_TREE_LIST: u32 = 0x1000;
/// Type bits of module 3 with the wall piece bit.
const BITS_WALL_SEGMENT: u32 = 0x10_0003;
/// Type bits of module 3 with the gate wall bit.
const BITS_GATE_WALL: u32 = 0x20_0003;
/// Type bits of module 6 (resource): fertile soil or gold.
const BITS_FERTILE: u32 = 0x206;
const BITS_GOLD: u32 = 0x106;
/// Module indices of building and tree.
const BUILDING: u32 = 3;
const TREE: usize = 4;
/// Rings of the placement search up to below `20 · cell`.
const SEARCH_RINGS: i32 = 20;

/// Hull of a building model in the default pose: `mn = center − half extent`, `mx = center + half extent`
/// (`Model_GetBoundsCenter`/`Model_GetBoundsHalfExtent` = header hull of the `.rdo`,
/// `Vector3_Subtract`/`Vector3_Add` per axis in `f32`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementBox {
    pub mn: [f32; 3],
    pub mx: [f32; 3],
}

impl PlacementBox {
    /// Hull of a building type from [`Rules::model_boxes`]; none without a loadable model.
    pub fn of(rules: &Rules, guid: u32) -> Option<Self> {
        let b = rules.model_boxes.get(&guid)?;
        Some(Self {
            mn: std::array::from_fn(|i| b.center[i] - b.half[i]),
            mx: std::array::from_fn(|i| b.center[i] + b.half[i]),
        })
    }
}

/// Remaining inputs of the check: building flags (40003), player slot and path layer/region of the first worker
/// (layer < 0 = without region check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementArgs {
    pub flags: u32,
    pub player: i32,
    pub layer: i32,
    pub region: i32,
}

/// `GameWorld_ComputePlacementTileRect` (`0x0047A700`): the hull corners `mn`/`mx` transformed with `RotY(rot)·T(pos)`,
/// sorted per axis, moved inward by `cell/8`, truncated and divided by the cell: `[x0, z0, x1, z1]` inclusive. The
/// same calculation as the footprint ([`crate::footprint::footprint_tile_rect`] with `rot_x = rot_z = 0`, scale 1),
/// but swapping happens only for an ordered `a > b` (`0x0047A7A5`: `FCOMP`, `TEST AH,0x41`, `JNZ` skips the swap for
/// NaN too). Invalid if the lower corner is negative after moving inward (NaN passes) or `x1`/`z1` are not on the
/// map; `x0 > x1` (empty rectangle) is not checked (quirk).
pub fn placement_rect(
    mn: [f32; 3],
    mx: [f32; 3],
    pos: [f32; 3],
    rot: f32,
    map: &MapInfo,
) -> Option<[i32; 4]> {
    let m = world_matrix(pos, [0.0, rot, 0.0], [1.0; 3]);
    let mut a = transform_coord(mn, &m);
    let mut b = transform_coord(mx, &m);
    for k in [0, 2] {
        // Swap only for `a > b` (not for NaN).
        if a[k] > b[k] {
            std::mem::swap(&mut a[k], &mut b[k]);
        }
    }
    let (sx, sz) = (map.cell[0], map.cell[2]);
    let (qx, qz) = ((sx / 8) as f32, (sz / 8) as f32);
    let (x1, z1) = (b[0] - qx, b[2] - qz);
    let (x0, z0) = (qx + a[0], qz + a[2]);
    if x0 < 0.0 || z0 < 0.0 {
        return None;
    }
    let r = [ftol(x0) / sx, ftol(z0) / sz, ftol(x1) / sx, ftol(z1) / sz];
    (r[2] < map.dim_x && r[3] < map.dim_z).then_some(r)
}

/// `GameWorld_SnapBoxMinCornerToGrid` (`0x0047A930`): the corner `mn` transformed with `RotY(rot)·T(pos)` (without
/// sorting) goes onto a tile boundary; y stays. Like [`crate::setup::snap_start_object`]: the fraction in x is stored
/// as `f32`, the one in z stays in the x87 register - equivalent at 24-bit precision, because `FIMUL SZ`, `FSUB` and
/// `FSTP` each round to 24 bits (`0x0047AA1A` onward): `pos.z − f32(fr_z · SZ)`.
pub fn snap_box_min_corner_to_grid(mn: [f32; 3], pos: &mut [f32; 3], rot: f32, map: &MapInfo) {
    let m = world_matrix(*pos, [0.0, rot, 0.0], [1.0; 3]);
    let a = transform_coord(mn, &m);
    let (sx, sz) = (map.cell[0] as f32, map.cell[2] as f32);
    let qx: f32 = a[0] / sx;
    let qz: f32 = a[2] / sz;
    let fr_x: f32 = qx - qx.floor();
    let fr_z: f32 = qz - qz.floor();
    pos[0] -= sx * fr_x;
    pos[2] -= fr_z * sz;
}

/// `Terrain_GetTileMinCornerHeight` (`0x0055AB70`): smallest of the four corner heights of the tile, starting at the
/// corner `(x, z)`; a NaN is skipped (or stays if it is the first corner).
fn tile_min_corner_height(rules: &Rules, x: usize, z: usize) -> f32 {
    let t = &rules.terrain;
    let mut h = t.vertex(x, z).position.y;
    for (cx, cz) in [(x + 1, z), (x, z + 1), (x + 1, z + 1)] {
        let c = t.vertex(cx, cz).position.y;
        if h > c {
            h = c;
        }
    }
    h
}

/// `GameEngine_IsTileUnderWater` (`0x004301A0`): tile on the map and its smallest corner height strictly below the
/// water level.
pub fn is_tile_under_water(rules: &Rules, x: i32, z: i32) -> bool {
    let map = rules.map;
    if !(0..map.dim_x).contains(&x) || !(0..map.dim_z).contains(&z) {
        return false;
    }
    tile_min_corner_height(rules, x as usize, z as usize) < map.water_level
}

/// Filter of the check: words 0-3 set, the rest like `MatchFilter_Construct`. The word `MM_PROPID` remains stack
/// content in the original; here 0 (it only decides whether switched-off modules count - building and deposit
/// modules are never switched off).
fn filter(mode: u32, match_bits: u32) -> MatchFilter {
    MatchFilter {
        mode,
        match_bits,
        ..MatchFilter::constructed()
    }
}

/// `PathPlanner_GetRegionId(layer, x, z)` equal to the worker's region (16-bit ID against the 32-bit argument: -1
/// never matches).
fn in_region(sim: &Sim, layer: i32, region: i32, x: i32, z: i32) -> bool {
    let id = sim.state.path.region_id(layer as usize, x, z);
    u32::from(id) == region as u32
}

/// Limit per A7/A8 (not a check of the original): position and rotation finite, `x` and `z` on the map
/// (`0 ≤ x < dim_x · SX`, `0 ≤ z < dim_z · SZ`; never for a cell size <= 0). Without it, `ftol` for NaN, +-infinity or
/// large values would give `i32::MIN` and thus a "valid" rectangle, and the wall piece branch would report the first
/// tile outside the map as free.
pub fn pose_on_map(map: &MapInfo, pos: [f32; 3], rot: f32) -> bool {
    let inside = |v: f32, dim: i32, cell: i32| {
        let end = f64::from(dim) * f64::from(cell);
        (0.0..end).contains(&f64::from(v))
    };
    pos.iter().all(|v| v.is_finite())
        && rot.is_finite()
        && inside(pos[0], map.dim_x, map.cell[0])
        && inside(pos[2], map.dim_z, map.cell[2])
}

/// `GameEngine_CheckBuildingPlacement` (`0x00483580`): branches in the order of the original.
/// Before that, [`pose_on_map`] rejects non-finite and far-off-map positions (A7/A8).
///
/// - **Tower** (`0x10000`): own wall piece within one tile around `pos/cell` -> position and rotation of the wall
///   piece, valid **without** further checks (quirk); otherwise continue.
/// - Rectangle ([`placement_rect`]) invalid -> invalid. Search point `P = pos · (1/SX) + 0.5` - z also with `1/SX`
///   (quirk).
/// - **Field** (`0x200`): fertile soil <= 5 tiles around `P`, snap the position (the rotation stays), rectangle
///   anew.
/// - **Gate** (`0x20000`): own wall <= 1 tile, adopt its rotation, rectangle anew, every tile needs a wall <= 1/2 tile
///   around its center; otherwise no check.
/// - **Gold mine** (`0x100`): `P` anew from the current position, gold deposit <= 5 tiles, snap, rectangle anew.
/// - **Harbor** (`0x8000`): tiles `x0..x1`, `z0..z1` with **exclusive** upper limits (quirk); land tiles must be free
///   in layer 0; water and land each at least one fifth (`total/water ≥ 1.25`, `total/land ≥ 1.25` in `f32`); region.
/// - **Wall piece** (`0x100000`): only the first tile `(x0, z0)`: free (also outside the map, quirk) or a tree <= 1
///   tile around `P` -> valid.
/// - **Field/gold mine** (`0x300`): every tile free in layer 0 (`GameEngine_IsCellBlocked`), region.
/// - **Default**: rectangle on the map (`x0`/`z0` only checked for >= 0), no occupancy bit, region; an empty
///   rectangle is valid without a region check (quirk). The subsequent check of the helper nodes `ffb00…` is omitted:
///   no model in the shipped data has such nodes, and the rules do not load them.
///
/// Region: with `layer ≥ 0` at least one checked tile must lie in `region`.
pub fn check_building_placement(
    sim: &mut Sim,
    hull: &PlacementBox,
    pos: &mut [f32; 3],
    rot: &mut f32,
    args: &PlacementArgs,
) -> bool {
    let map = sim.rules.map;
    if !pose_on_map(&map, *pos, *rot) {
        return false;
    }
    let fl = args.flags & 0xFFFF_FF00;
    let (player, layer, region) = (args.player, args.layer, args.region);
    let (sx, sz) = (map.cell[0] as f32, map.cell[2] as f32);
    let pose = |sim: &Sim, id: ObjectId| sim.object(id).map(|o| (o.world_pos, o.rotation[1]));
    // A. Tower: search point `pos/cell` (quotients in `f32`, without +0.5).
    if fl & flags::TOWER != 0 {
        let p = (Fx::from_f32(pos[0] / sx).0, Fx::from_f32(pos[2] / sz).0);
        let f = filter(MODE_OWN_BUILDING, BITS_WALL_SEGMENT);
        if let Some(w) = find_nearest_matching(sim, p, &f, ONE_TILE, player, -1)
            && let Some((wp, wr)) = pose(sim, w)
        {
            *pos = wp;
            *rot = wr;
            return true;
        }
    }
    let Some(mut r) = placement_rect(hull.mn, hull.mx, *pos, *rot, &map) else {
        return false;
    };
    // Search point: quirk - z also with 1/SX.
    let inv = 1.0f32 / sx;
    let search_point = |pos: &[f32; 3]| {
        (
            Fx::from_f32(pos[0] * inv + 0.5).0,
            Fx::from_f32(pos[2] * inv + 0.5).0,
        )
    };
    let mut p = search_point(pos);
    // B. Field: fertile soil (resource list, player 10), snap; the rotation stays.
    if fl & flags::FIELD != 0 {
        let f = filter(MODE_TYPE_BITS, BITS_FERTILE);
        let found = find_nearest_matching(sim, p, &f, DEPOSIT_RADIUS, i32::from(NATURE_SLOT), -1);
        let Some((gp, _)) = found.and_then(|g| pose(sim, g)) else {
            return false;
        };
        *pos = gp;
        snap_box_min_corner_to_grid(hull.mn, pos, *rot, &map);
        let Some(nr) = placement_rect(hull.mn, hull.mx, *pos, *rot, &map) else {
            return false;
        };
        r = nr;
    }
    // C. Gate: rotation of the wall; every tile carries a wall.
    if fl & flags::GATE != 0 {
        let f = filter(MODE_OWN_BUILDING, BITS_GATE_WALL);
        let found = find_nearest_matching(sim, p, &f, ONE_TILE, player, -1);
        let Some((_, wr)) = found.and_then(|w| pose(sim, w)) else {
            return false;
        };
        *rot = wr;
        let Some(r) = placement_rect(hull.mn, hull.mx, *pos, *rot, &map) else {
            return false;
        };
        for z in r[1]..=r[3] {
            for x in r[0]..=r[2] {
                let c = (
                    Fx::from_f32(x as f32 + 0.5).0,
                    Fx::from_f32(z as f32 + 0.5).0,
                );
                if find_nearest_matching(sim, c, &f, HALF_TILE, player, -1).is_none() {
                    return false;
                }
            }
        }
        return true;
    }
    // D. Gold mine: search point anew (`FixedPos_SetFromFloats`, y + 0.5 is irrelevant for the search).
    if fl & flags::GOLD_MINE != 0 {
        p = search_point(pos);
        let f = filter(MODE_TYPE_BITS, BITS_GOLD);
        let found = find_nearest_matching(sim, p, &f, DEPOSIT_RADIUS, i32::from(NATURE_SLOT), -1);
        let Some((gp, _)) = found.and_then(|g| pose(sim, g)) else {
            return false;
        };
        *pos = gp;
        snap_box_min_corner_to_grid(hull.mn, pos, *rot, &map);
        let Some(nr) = placement_rect(hull.mn, hull.mx, *pos, *rot, &map) else {
            return false;
        };
        r = nr;
    }
    // E. Harbor: upper limits exclusive (quirk).
    if fl & flags::HARBOR != 0 {
        if r[1] >= r[3] {
            return false;
        }
        let (mut water, mut land, mut total, mut hit) = (0.0f32, 0.0f32, 0.0f32, false);
        for z in r[1]..r[3] {
            for x in r[0]..r[2] {
                if layer >= 0 && in_region(sim, layer, region, x, z) {
                    hit = true;
                }
                total += 1.0;
                if is_tile_under_water(sim.rules, x, z) {
                    water += 1.0;
                } else {
                    if sim.state.path.cell_cost(x, z, 0) != 0 {
                        return false;
                    }
                    land += 1.0;
                }
            }
        }
        if water == 0.0 || land == 0.0 {
            return false;
        }
        if !(1.25 <= total / water && 1.25 <= total / land) {
            return false;
        }
        return layer < 0 || hit;
    }
    // F. Wall piece: only the first tile.
    if fl & flags::WALL_SEGMENT != 0 {
        if !sim.state.occupancy.test(r[0], r[1]) {
            return true;
        }
        let f = filter(MODE_TREE_LIST, 0);
        return find_nearest_matching(sim, p, &f, ONE_TILE, player, -1)
            .and_then(|t| sim.object(t))
            .is_some_and(|t| t.has_module(TREE, false));
    }
    // G. Field/gold mine: path grid layer 0, inclusive limits.
    if fl & (flags::FIELD | flags::GOLD_MINE) != 0 {
        let mut hit = false;
        for z in r[1]..=r[3] {
            for x in r[0]..=r[2] {
                if layer >= 0 && in_region(sim, layer, region, x, z) {
                    hit = true;
                }
                if sim.state.path.cell_cost(x, z, 0) != 0 {
                    return false;
                }
            }
        }
        return layer < 0 || hit;
    }
    // H. Default: occupancy bits.
    if r[1] < 0 || r[0] < 0 || r[3] >= map.dim_z || r[2] >= map.dim_x {
        return false;
    }
    let mut hit = false;
    for z in r[1]..=r[3] {
        for x in r[0]..=r[2] {
            if layer >= 0 && in_region(sim, layer, region, x, z) {
                hit = true;
            }
            if sim.state.occupancy.test(x, z) {
                return false;
            }
        }
    }
    layer < 0 || hit
}

/// Flags (value 40003) of the **first** `PROPID 3` description of a type (like
/// `GameEngine_FindBuildingPlacementNear`; the variants in the shipped data differ only in bit `0x80000`, which no
/// check reads). If the value is missing, 0 (stack content in the original if the value set is missing); no building
/// -> `None`.
pub fn building_flags(rules: &Rules, guid: u32) -> Option<u32> {
    let desc = rules.types.get(&guid)?.module(BUILDING)?;
    Some(desc.value_int(40003).unwrap_or(0) as u32)
}

/// Ring search of `GameEngine_FindBuildingPlacementNear` (`0x00434050`): first `(x, z)`, then rings
/// `r = 1 … 19` cells; per ring an x pass (`xa = x − r, …` while `xa < x + r`, rows
/// `zr − r` and `zr + r`) and a z pass (`za = z − r, …` while `za < z + r`, columns
/// `xa_end − r` and `xa_end + r`). **Quirks:** the columns lie at the end value of the x pass (i.e.
/// at `x` and `x + 2r` instead of `x ∓ r`), and the rows `zr` of the x pass use from the second
/// ring on the end value of the previous z pass (the same stack variable), not `z`. `attempt` returns
/// `true` on success; the return value says whether an attempt succeeded.
fn spiral_search(
    x: f32,
    z: f32,
    cell: [i32; 3],
    mut attempt: impl FnMut(f32, f32) -> bool,
) -> bool {
    if attempt(x, z) {
        return true;
    }
    let (sx, sz) = (cell[0] as f32, cell[2] as f32);
    let limit = cell[0].wrapping_mul(SEARCH_RINGS) as f32;
    let mut r = sx;
    // `[ESP+0x10]`: first z, then the loop variable of the z pass.
    let mut row_z = z;
    while limit > r {
        let mut xa = x - r;
        if xa < r + x {
            let low = row_z - r;
            while xa < r + x {
                if attempt(xa, low) || attempt(xa, r + row_z) {
                    return true;
                }
                xa += sx;
            }
        }
        row_z = z - r;
        if r + z > row_z {
            let left = xa - r;
            while r + z > row_z {
                if attempt(left, row_z) || attempt(r + xa, row_z) {
                    return true;
                }
                row_z += sz;
            }
        }
        r += sx;
    }
    false
}

/// Limit of the placement search (A7/A8): cell sizes > 0, start point finite and at most `3 · 20` cells beside the
/// map. The rings reach (with the shifted row) to about `2 · 20` cells from the start; further out no attempt would
/// lie on the map, [`pose_on_map`] would reject every one - the result `false` thus stays the same, only without the
/// loop that no longer advances from `|x| ≥ 2³¹` on (`x + SX == x` in `f32`).
fn search_start_near_map(map: &MapInfo, x: f32, z: f32) -> bool {
    // The rings grow by SX, the rows of the z pass by SZ: margin with the larger cell.
    let margin = 3.0 * f64::from(SEARCH_RINGS) * f64::from(map.cell[0].max(map.cell[2]));
    let near = |v: f32, dim: i32, cell: i32| {
        let end = f64::from(dim) * f64::from(cell);
        (-margin..=end + margin).contains(&f64::from(v))
    };
    map.cell[0] > 0
        && map.cell[2] > 0
        && x.is_finite()
        && z.is_finite()
        && near(x, map.dim_x, map.cell[0])
        && near(z, map.dim_z, map.cell[2])
}

/// `GameEngine_FindBuildingPlacementNear` (`0x00434050`): checks `(x, 0, z)` with rotation 0 and then rings around the
/// point ([`spiral_search`]); success -> `x`/`z` of the (possibly snapped) position, a snapped rotation is lost.
/// Height and rotation are not reset between attempts (side effects of the check stay in effect). No building or no
/// model -> `false`. A7/A8: non-finite or far-off-map start values and cell size <= 0 -> immediately `false`
/// ([`search_start_near_map`]; otherwise the ring search would run endlessly from `|x| ≥ 2³¹` on).
pub fn find_building_placement_near(
    sim: &mut Sim,
    x: &mut f32,
    z: &mut f32,
    guid: u32,
    player: i32,
    layer: i32,
    region: i32,
) -> bool {
    let rules = sim.rules;
    if !search_start_near_map(&rules.map, *x, *z) {
        return false;
    }
    let (Some(flags), Some(hull)) = (building_flags(rules, guid), PlacementBox::of(rules, guid))
    else {
        return false;
    };
    let args = PlacementArgs {
        flags,
        player,
        layer,
        region,
    };
    let mut pos = [*x, 0.0, *z];
    let mut rot = 0.0f32;
    let found = spiral_search(*x, *z, rules.map.cell, |px, pz| {
        pos[0] = px;
        pos[2] = pz;
        check_building_placement(sim, &hull, &mut pos, &mut rot, &args)
    });
    if found {
        *x = pos[0];
        *z = pos[2];
    }
    found
}

/// Path layer and region of a worker like `BuildCursor_UpdatePlacement` (`0x0044FDB0`): layer via the query `0x1400`,
/// region at its tile; region `0xFFFF` -> layer -1.
pub fn worker_layer_region(sim: &mut Sim, worker: ObjectId) -> (i32, i32) {
    let layer = send(sim, worker, &Message::new(0x1400, &[]));
    let region = region_id_at(sim, worker, layer, -1, -1);
    let layer = if region == crate::path::BLOCKED {
        -1
    } else {
        layer
    };
    (layer, i32::from(region))
}

/// Facade for commands and viewer: building site for the type `guid` with flags 40003, player slot `player` and
/// layer/region of the first worker (-1 without a worker). Valid -> the position and rotation adjusted by the check.
/// Non-finite or far-off-map positions are invalid before the check runs ([`pose_on_map`], A7/A8).
pub fn check_placement_for(
    sim: &mut Sim,
    guid: u32,
    pos: [f32; 3],
    rot: f32,
    player: i32,
    worker: Option<ObjectId>,
) -> Option<([f32; 3], f32)> {
    if !pose_on_map(&sim.rules.map, pos, rot) {
        return None;
    }
    let flags = building_flags(sim.rules, guid)?;
    let hull = PlacementBox::of(sim.rules, guid)?;
    let (layer, region) = worker.map_or((-1, -1), |w| worker_layer_region(sim, w));
    let args = PlacementArgs {
        flags,
        player,
        layer,
        region,
    };
    let (mut pos, mut rot) = (pos, rot);
    check_building_placement(sim, &hull, &mut pos, &mut rot, &args).then_some((pos, rot))
}

/// `GameObject_CheckPlacementAtOwnPos` (`0x00473F10`, re/tents 3.4): placement check of an existing building at its
/// own position, for unpacking a tent (`0x22A`). Inputs as in the original: the **current** flags of module 3 (not
/// the data values), the model hull of the type, `world_pos`, the rotation `rotation[1]`, the owner's player slot and
/// layer and region -1 (without reachability; only the cursor checks that). The check writes the rotation straight
/// back into the object (`GO+0x158`, also on failure; only tower and gate can change it). Valid ->
/// `SetGridPosition(world_to_grid(pos), keep_y)` (`0x00473B20`), for tents (default branch, `pos` unchanged) a round
/// trip grid -> world -> grid. Without an object or model hull invalid.
pub fn check_placement_at_own_pos(sim: &mut Sim, id: ObjectId) -> bool {
    let flags = sim
        .module(id, BUILDING as usize)
        .map_or(0, |m| m.base.flags);
    let Some((guid, mut pos, mut rot)) =
        sim.object(id).map(|o| (o.guid, o.world_pos, o.rotation[1]))
    else {
        return false;
    };
    let Some(hull) = PlacementBox::of(sim.rules, guid) else {
        return false;
    };
    let args = PlacementArgs {
        flags,
        player: i32::from(id.slot()),
        layer: -1,
        region: -1,
    };
    let ok = check_building_placement(sim, &hull, &mut pos, &mut rot, &args);
    if let Some(o) = sim.object_mut(id) {
        o.rotation[1] = rot;
    }
    if ok {
        let grid = world_to_grid(pos, sim.rules.map.cell);
        set_grid_position(sim, id, grid, true);
    }
    ok
}

/// Building flags (40003) for which the build cursor never rotates (`BuildCursor_UpdatePlacement` `0x0044FDB0`:
/// `(+0x44 & 0x10000003) == 0` -> rotation from dragging, otherwise 0). The cursor's constructor (`FUN_0044FC80`,
/// `0x0044FD03 AND EAX, 0xFFFFFF00` -> `[ESI+0x44]`) stores the flags without the low byte, so only `0x10000000` is
/// effective. What is locked is determined by the flags, not by the kind of check: in the shipped data tower, gate,
/// field and gold mine of all nations, the Spanish shipyard 1008 (a harbor) and 4250-4253; no building type carries
/// bits 0/1.
pub const ROTATION_LOCK: u32 = 0x1000_0000;

/// The build cursor never rotates the type ([`ROTATION_LOCK`]); no building -> `false`.
pub fn rotation_locked(rules: &Rules, guid: u32) -> bool {
    building_flags(rules, guid).is_some_and(|f| f & ROTATION_LOCK != 0)
}

/// Wall and corner type of a fort command `0x18` for `guid` (wall or corner): for the wall buttons 1011, 1311, 1912,
/// 2212 and 2512 the build cursor starts the fort cursor (`FUN_00453530`), which keeps the corner as **wall + 1**
/// (`FUN_00451260`: `+0x68` wall, `+0x64` corner) and builds the wall ring from both (`FUN_00450BE0`). Here from the
/// data instead of from the list: the wall has the wall piece and gate wall bit, the corner = wall + 1 only the wall
/// piece bit. In the shipped data this gives exactly the five pairs 1011/1012, 1311/1312, 1912/1913, 2212/2213,
/// 2512/2513 (corpus test). Returns `(wall, corner)`; no pair -> `None`.
pub fn wall_corner_pair(rules: &Rules, guid: u32) -> Option<(u32, u32)> {
    let is_wall = |g: u32| {
        building_flags(rules, g).is_some_and(|f| {
            f & (flags::WALL_SEGMENT | flags::GATE_WALL) == flags::WALL_SEGMENT | flags::GATE_WALL
        })
    };
    let is_corner = |g: u32| {
        building_flags(rules, g)
            .is_some_and(|f| f & (flags::WALL_SEGMENT | flags::GATE_WALL) == flags::WALL_SEGMENT)
    };
    let pair = |wall: u32| {
        let corner = wall.checked_add(1)?;
        (is_wall(wall) && is_corner(corner)).then_some((wall, corner))
    };
    pair(guid).or_else(|| pair(guid.checked_sub(1)?))
}

/// Building site of the build cursor: position, rotation and whether it is valid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPlacement {
    pub pos: [f32; 3],
    pub rot: f32,
    pub valid: bool,
}

/// Snapping like the build cursor (`BuildCursor_GetSnappedPosition` `0x004CD240`, then
/// `BuildCursor_UpdatePlacement` `0x0044FDB0`): locked types ([`rotation_locked`]) with rotation 0, the hull minimum
/// corner onto the tile boundary ([`snap_box_min_corner_to_grid`]), then the check ([`check_placement_for`]), which
/// snaps tower, gate, field and gold mine further. Valid -> exactly the position and rotation that a command `0x01`
/// must carry ([`snapped_command_pose`]); invalid -> the position after the tile snapping. For viewer, control channel
/// and tests that command buildings with free coordinates. `None` without a model hull.
pub fn cursor_placement(
    sim: &mut Sim,
    guid: u32,
    ground: [f32; 3],
    rot: f32,
    player: i32,
    worker: Option<ObjectId>,
) -> Option<CursorPlacement> {
    let hull = PlacementBox::of(sim.rules, guid)?;
    let rot = if rotation_locked(sim.rules, guid) {
        0.0
    } else {
        rot
    };
    let mut pos = ground;
    snap_box_min_corner_to_grid(hull.mn, &mut pos, rot, &sim.rules.map);
    Some(
        match check_placement_for(sim, guid, pos, rot, player, worker) {
            Some((pos, rot)) => CursorPlacement {
                pos,
                rot,
                valid: true,
            },
            None => CursorPlacement {
                pos,
                rot,
                valid: false,
            },
        },
    )
}

/// Check of a command position (A8; the original builds without a check at the command position,
/// `GameWorld_CmdPlaceSingleBuilding` `0x004896D0`): the check from the command position on, with rotation 0 for
/// locked types ([`rotation_locked`]) like the cursor. Valid -> the snapped position and rotation; a command from the
/// UI already carries them, so it applies only if both are **bit-identical** to the command position. The snapping of
/// the check (tower, gate: position or rotation of the wall piece; field, gold mine: deposit plus
/// [`snap_box_min_corner_to_grid`]) is idempotent - verified on all deposits and wall pieces of all 55 maps in the
/// shipped data, as a sample in the corpus test `cursor_snaps_are_idempotent_on_sample_maps`. The tile snapping of
/// the cursor is **not** repeated: it is not idempotent (in `f32` the corner sometimes ends up just below the tile
/// boundary, a second snap then jumps one tile further).
pub fn snapped_command_pose(
    sim: &mut Sim,
    guid: u32,
    pos: [f32; 3],
    rot: f32,
    player: i32,
    worker: Option<ObjectId>,
) -> Option<([f32; 3], f32)> {
    let input = if rotation_locked(sim.rules, guid) {
        0.0
    } else {
        rot
    };
    check_placement_for(sim, guid, pos, input, player, worker)
}

/// Position and rotation bit-identical (also `-0.0 ≠ 0.0` and NaN patterns).
pub fn same_pose(a: ([f32; 3], f32), b: ([f32; 3], f32)) -> bool {
    a.0.map(f32::to_bits) == b.0.map(f32::to_bits) && a.1.to_bits() == b.1.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::footprint_tile_rect;
    use crate::rules::ModelBox;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, desc_flags};
    use std::f32::consts::FRAC_PI_2;

    /// 1001 house (default), 1011 wall (wall piece), 1015 tower, 1016 gate, 1020 field,
    /// 1030 harbor (synthetic, without rotation lock), 1031 harbor with the bits 0/1 (`0xC8003`, synthetic),
    /// 1040 gold mine, 3500 gold deposit (nature, module 6 with bit 0x100), 3520
    /// fertile soil (nature, module 6 with bit 0x200), 3602 tree.
    const TYPES: &str = "GUID 1001\nNAME Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 540672\n\
GUID 1011\nNAME Mauer\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 3686400\n\
GUID 1015\nNAME Turm\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 269041664\n\
GUID 1016\nNAME Tor\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 269107200\n\
GUID 1020\nNAME Feld\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 268968448\n\
GUID 1030\nNAME Hafen\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 819200\n\
GUID 1031\nNAME Hafen03\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 819203\n\
GUID 1040\nNAME Goldmine\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 268960000\n\
GUID 3500\nNAME Gold\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 256\n\
GUID 3520\nNAME Boden\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 512\n\
GUID 3602\nNAME Baum\nVALUEID 100 6 1 256\nPROPID 4\n";

    const MAP: MapInfo = MapInfo {
        dim_x: 256,
        dim_z: 256,
        cell: [128; 3],
        water_level: -100.0,
    };

    fn rules() -> Rules {
        Rules::synthetic(TYPES, 32)
    }

    fn state(rules: &Rules) -> SimState {
        let mut s = SimState::new(0, 0);
        s.init_map(rules);
        s.path.build_regions(u32::MAX);
        s
    }

    fn args(flags: u32) -> PlacementArgs {
        PlacementArgs {
            flags,
            player: 0,
            layer: -1,
            region: -1,
        }
    }

    fn hull(w: f32) -> PlacementBox {
        PlacementBox {
            mn: [0.0; 3],
            mx: [w, 10.0, w],
        }
    }

    #[test]
    fn placement_rects_match_the_original_vectors() {
        let big = ([-192.0, 0.0, -192.0], [192.0, 300.0, 192.0]);
        let wide = ([0.0, 0.0, 0.0], [384.0, 300.0, 256.0]);
        for (b, pos, rot, want) in [
            (big, [1000.0, 0.0, 2000.0], 0.0, Some([6, 14, 9, 17])),
            (wide, [1024.0, 0.0, 1024.0], 0.0, Some([8, 8, 10, 9])),
            (wide, [1024.0, 0.0, 1024.0], FRAC_PI_2, Some([6, 8, 7, 10])),
            (
                ([-64.0, 0.0, -64.0], [64.0, 10.0, 64.0]),
                [10.0, 0.0, 10.0],
                0.0,
                None,
            ),
            (
                ([0.0; 3], [20.0, 10.0, 20.0]),
                [500.0, 0.0, 500.0],
                0.0,
                Some([4, 4, 3, 3]),
            ),
        ] {
            assert_eq!(
                placement_rect(b.0, b.1, pos, rot, &MAP),
                want,
                "{pos:?} {rot}"
            );
            // Same calculation as the footprint (center 0, half extent = corner).
            if let Some(w) = want {
                let m = world_matrix(pos, [0.0, rot, 0.0], [1.0; 3]);
                let half = b.1.map(|v| v * 0.5);
                if b.0 == [0.0; 3] {
                    assert_eq!(footprint_tile_rect(half, half, &m, MAP.cell), w);
                }
            }
        }
        // x1/z1 outside the map.
        assert_eq!(
            placement_rect(big.0, big.1, [255.0 * 128.0, 0.0, 2000.0], 0.0, &MAP),
            None
        );
        // NaN: no swap (`TEST AH,0x41; JNZ` at 0x0047A7A5 also jumps for "unordered"), the
        // lower corner passes the test `< 0`, `ftol(NaN) = i32::MIN`.
        let nan = [f32::NAN, 0.0, -192.0];
        assert_eq!(
            placement_rect(nan, big.1, [1000.0, 0.0, 2000.0], 0.0, &MAP),
            Some([i32::MIN / 128, i32::MIN / 128, 9, 17])
        );
    }

    #[test]
    fn default_branch_checks_occupancy_and_region() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = hull(384.0);
        let check = |sim: &mut Sim, pos: [f32; 3], a: PlacementArgs| {
            let (mut p, mut r) = (pos, 0.0);
            check_building_placement(sim, &h, &mut p, &mut r, &a)
        };
        let free = [640.0, 0.0, 1792.0];
        assert_eq!(
            placement_rect(h.mn, h.mx, free, 0.0, &MAP),
            Some([5, 14, 7, 16])
        );
        sim.state.occupancy.set(9, 15, true);
        assert!(check(&mut sim, free, args(0x84000)));
        let busy = [1024.0, 0.0, 1792.0];
        assert_eq!(
            placement_rect(h.mn, h.mx, busy, 0.0, &MAP),
            Some([8, 14, 10, 16])
        );
        assert!(!check(&mut sim, busy, args(0x84000)));
        // Region required: no tile in the region -> invalid; the own one -> valid.
        let own = i32::from(sim.state.path.region_id(0, 6, 15));
        let with_region = |region| PlacementArgs {
            layer: 0,
            region,
            ..args(0x84000)
        };
        assert!(!check(&mut sim, free, with_region(0xFFFE)));
        assert!(!check(&mut sim, free, with_region(-1)));
        assert!(check(&mut sim, free, with_region(own)));
        // Empty rectangle (x0 > x1) valid without a region check, not with a region.
        let tiny = hull(20.0);
        let (mut p, mut r) = ([500.0, 0.0, 500.0], 0.0);
        assert!(check_building_placement(
            &mut sim,
            &tiny,
            &mut p,
            &mut r,
            &args(0)
        ));
        assert!(!check_building_placement(
            &mut sim,
            &tiny,
            &mut p,
            &mut r,
            &with_region(own)
        ));
    }

    /// Lowers the corner heights at the grid points below the water level.
    fn flood(rules: &mut Rules, vertices: &[(usize, usize)]) {
        let vx = rules.terrain.verts_x;
        for &(i, j) in vertices {
            rules.terrain.vertices[j * vx + i].position.y = -200.0;
        }
    }

    // QUIRK W-115: kept
    #[test]
    fn harbor_counts_with_exclusive_bounds_and_needs_a_fifth_of_each() {
        let harbor = |water: &[(usize, usize)], w: f32| {
            let mut rules = rules();
            flood(&mut rules, water);
            let mut state = state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let (mut p, mut r) = ([1280.0, 0.0, 1280.0], 0.0);
            check_building_placement(&mut sim, &hull(w), &mut p, &mut r, &args(0xC8000))
        };
        assert_eq!(
            placement_rect(
                [0.0; 3],
                [640.0, 10.0, 640.0],
                [1280.0, 0.0, 1280.0],
                0.0,
                &MAP
            ),
            Some([10, 10, 14, 14])
        );
        let column = |i: usize, js: std::ops::RangeInclusive<usize>| {
            js.map(move |j| (i, j)).collect::<Vec<_>>()
        };
        // 8/16 water: columns 12 and 13.
        let eight: Vec<_> = column(13, 10..=14)
            .into_iter()
            .chain(column(14, 10..=14))
            .collect();
        assert!(harbor(&eight, 640.0));
        // 5/16: column 13 and tile (10, 10).
        let mut five = column(14, 10..=14);
        five.push((10, 10));
        assert!(harbor(&five, 640.0));
        // 3/16: 16/3 >= 1.25, but 16/13 < 1.25.
        assert!(!harbor(&column(14, 10..=12), 640.0));
        // No water.
        assert!(!harbor(&[], 640.0));
        // 2 x 2 counted (rectangle [10, 10, 12, 12]) without water.
        assert!(!harbor(&[], 384.0));
        // Water only in the last column/row of the rectangle does not count (exclusive).
        assert!(!harbor(&column(15, 10..=15), 640.0));
    }

    #[test]
    fn water_test_uses_the_lowest_corner_strictly() {
        let mut rules = rules();
        flood(&mut rules, &[(3, 4)]);
        for (x, z) in [(2, 3), (3, 3), (2, 4), (3, 4)] {
            assert!(is_tile_under_water(&rules, x, z));
        }
        assert!(!is_tile_under_water(&rules, 4, 4));
        assert!(!is_tile_under_water(&rules, -1, 0));
        assert!(!is_tile_under_water(&rules, 32, 0));
        rules.map.water_level = 0.0;
        assert!(!is_tile_under_water(&rules, 10, 10));
    }

    fn place(sim: &mut Sim, guid: u32, player: i32, pos: [f32; 3], rot_y: f32) -> ObjectId {
        let mut d = ObjectDesc::new(guid, player, pos);
        d.flags |= desc_flags::ROT_Y | desc_flags::READY;
        d.rotation = [0.0, rot_y, 0.0];
        create_object(sim, &mut d).expect("object")
    }

    // QUIRK W-116: kept
    #[test]
    fn wall_segment_checks_only_its_first_tile() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let wall = args(0x38_4000);
        let h = hull(384.0);
        let check = |sim: &mut Sim, pos: [f32; 3]| {
            let (mut p, mut r) = (pos, 0.0);
            check_building_placement(sim, &h, &mut p, &mut r, &wall)
        };
        let pos = [640.0, 0.0, 1792.0];
        // All remaining tiles occupied: irrelevant.
        sim.state.occupancy.set_rect([6, 14, 7, 16], true);
        assert!(check(&mut sim, pos));
        sim.state.occupancy.set(5, 14, true);
        assert!(!check(&mut sim, pos));
        // A tree on the first tile (it sets the occupancy bit itself): valid.
        sim.state.occupancy.set(5, 14, false);
        place(&mut sim, 3602, 9, [700.0, 0.0, 1850.0], 0.0);
        assert!(sim.state.occupancy.test(5, 14));
        assert!(check(&mut sim, pos));
        // First tile outside the map (x0 = 32): free.
        let edge = hull(20.0);
        let (mut p, mut r) = ([4085.0, 0.0, 1792.0], 0.0);
        assert_eq!(
            placement_rect(edge.mn, edge.mx, p, 0.0, &rules.map).map(|r| r[0]),
            Some(32)
        );
        assert!(check_building_placement(
            &mut sim, &edge, &mut p, &mut r, &wall
        ));
    }

    // QUIRK W-118: kept
    #[test]
    fn tower_snaps_onto_an_own_wall_segment_without_further_checks() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = place(&mut sim, 1011, 0, [1000.0, 0.0, 2000.0], 0.7);
        let (wp, wr) = {
            let o = sim.object(w).unwrap();
            (o.world_pos, o.rotation[1])
        };
        assert_eq!(wr, 0.7);
        // Everything occupied: the tower snaps anyway.
        sim.state.occupancy.set_rect([0, 0, 31, 31], true);
        let tower = args(0x1009_4000);
        let (mut p, mut r) = ([1050.0, 0.0, 2030.0], 0.0);
        assert!(check_building_placement(
            &mut sim,
            &hull(256.0),
            &mut p,
            &mut r,
            &tower
        ));
        assert_eq!((p, r), (wp, wr));
        // Foreign wall piece: default check (occupied), position unchanged.
        let (mut p, mut r) = ([1050.0, 0.0, 2030.0], 0.0);
        let other = PlacementArgs { player: 1, ..tower };
        assert!(!check_building_placement(
            &mut sim,
            &hull(256.0),
            &mut p,
            &mut r,
            &other
        ));
        assert_eq!((p, r), ([1050.0, 0.0, 2030.0], 0.0));
        // Too far away (more than one tile).
        let (mut p, mut r) = ([1200.0, 0.0, 2000.0], 0.0);
        assert!(!check_building_placement(
            &mut sim,
            &hull(256.0),
            &mut p,
            &mut r,
            &tower
        ));
    }

    #[test]
    fn a_command_pose_must_be_the_snapped_one_and_locked_types_start_unrotated() {
        let mut rules = rules();
        for guid in [1001, 1011, 1015] {
            rules.model_boxes.insert(
                guid,
                ModelBox {
                    center: [0.0; 3],
                    half: [64.0, 50.0, 64.0],
                },
            );
        }
        // Rotation lock by the flags (bit 0x10000000): tower, gate, field; house, wall and the
        // synthetic harbor 1030 without a lock bit do not (in the shipped data the Spanish shipyard 1008
        // carries the lock bit). Bits 0/1 of the mask 0x10000003 do not lock: the cursor stores the
        // flags with `& 0xFFFFFF00` (`0x0044FD03`), its test never sees them (harbor 1031).
        assert!(
            [1015, 1016, 1020]
                .iter()
                .all(|&g| rotation_locked(&rules, g))
        );
        assert!(
            ![1001, 1011, 1030, 1031, 3602]
                .iter()
                .any(|&g| rotation_locked(&rules, g))
        );
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = place(&mut sim, 1011, 0, [1000.0, 0.0, 2000.0], 0.7);
        let (wp, wr) = {
            let o = sim.object(w).unwrap();
            (o.world_pos, o.rotation[1])
        };
        // Tower: the cursor does not rotate, the check adopts the position and rotation of the wall.
        let c = cursor_placement(&mut sim, 1015, [1050.0, 0.0, 2030.0], FRAC_PI_2, 0, None);
        assert_eq!(
            c,
            Some(CursorPlacement {
                pos: wp,
                rot: wr,
                valid: true
            })
        );
        // This position snaps unchanged (idempotent), a different rotation does not.
        let snapped = snapped_command_pose(&mut sim, 1015, wp, wr, 0, None).unwrap();
        assert!(same_pose(snapped, (wp, wr)));
        let snapped = snapped_command_pose(&mut sim, 1015, wp, 0.3, 0, None).unwrap();
        assert!(!same_pose(snapped, (wp, 0.3)));
        // House: the check does not snap, every valid position stays (also rotated); the
        // cursor puts the hull minimum corner on the tile boundary.
        let free = ([3000.5, 0.0, 1000.25], 0.3);
        let snapped = snapped_command_pose(&mut sim, 1001, free.0, free.1, 0, None).unwrap();
        assert!(same_pose(snapped, free));
        let c = cursor_placement(&mut sim, 1001, [3000.5, 0.0, 1000.25], 0.0, 0, None).unwrap();
        assert_eq!((c.pos, c.rot, c.valid), ([2880.0, 0.0, 960.0], 0.0, true));
        // Bit-identical: -0.0 is not 0.0.
        assert!(!same_pose(([1.0; 3], -0.0), ([1.0; 3], 0.0)));
        // Without a model hull no cursor.
        assert_eq!(cursor_placement(&mut sim, 1020, wp, 0.0, 0, None), None);
    }

    #[test]
    fn snapping_rounds_z_like_x_in_f32() {
        // 0x0047AA1A onward: `FIMUL SZ` -> `FSUB` -> `FSTP f32` with 24-bit precision, i.e.
        // `pos.z − f32(fr_z · SZ)`. For a cell that is not a power of two, the earlier
        // calculation with an exact product (f64) differed in the last bit (963.99994 instead of 964).
        let map = MapInfo {
            cell: [100; 3],
            ..MAP
        };
        let z = f32::from_bits(0x4480_d6b8);
        let mut pos = [500.0, 0.0, z];
        snap_box_min_corner_to_grid([-64.0, 0.0, -64.0], &mut pos, 0.0, &map);
        assert_eq!(pos, [464.0, 0.0, 964.0]);
        let mut v = [500.0, 0.0, z];
        let mut rules = rules();
        rules.map = map;
        rules.model_boxes.insert(
            1001,
            ModelBox {
                center: [0.0; 3],
                half: [64.0, 50.0, 64.0],
            },
        );
        crate::setup::snap_start_object(&rules, 1001, &mut v);
        assert_eq!(v, pos);
    }

    #[test]
    fn field_snaps_onto_fertile_ground_and_keeps_its_rotation() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let field = args(0x1008_2200);
        let h = PlacementBox {
            mn: [-192.0, 0.0, -192.0],
            mx: [192.0, 300.0, 192.0],
        };
        // Invalid without ground, position unchanged.
        let (mut p, mut r) = ([1100.0, 0.0, 2100.0], 0.3);
        assert!(!check_building_placement(
            &mut sim, &h, &mut p, &mut r, &field
        ));
        assert_eq!((p, r), ([1100.0, 0.0, 2100.0], 0.3));
        let g = place(&mut sim, 3520, 10, [1000.0, 0.0, 2000.0], 0.0);
        let gp = sim.object(g).unwrap().world_pos;
        let mut want = gp;
        snap_box_min_corner_to_grid(h.mn, &mut want, 0.3, &rules.map);
        assert_ne!(want, gp);
        assert!(check_building_placement(
            &mut sim, &h, &mut p, &mut r, &field
        ));
        assert_eq!((p, r), (want, 0.3));
        // Blocked tile in the path grid (layer 0) -> invalid, the position stays snapped.
        let rect = placement_rect(h.mn, h.mx, want, 0.3, &rules.map).unwrap();
        sim.state.path.mark_cell(1, rect[2], rect[3], true);
        sim.state.path.flush_dirty_rect();
        let (mut p, mut r) = ([1100.0, 0.0, 2100.0], 0.3);
        assert!(!check_building_placement(
            &mut sim, &h, &mut p, &mut r, &field
        ));
        assert_eq!(p, want);
        // More than 5 tiles away: no ground.
        let (mut p, mut r) = ([1000.0 + 6.0 * 128.0, 0.0, 2000.0], 0.0);
        assert!(!check_building_placement(
            &mut sim, &h, &mut p, &mut r, &field
        ));
    }

    #[test]
    fn gold_mine_snaps_onto_its_deposit_and_field_and_mine_check_the_region() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mine = args(0x1008_0100);
        let field = args(0x1008_2200);
        let h = PlacementBox {
            mn: [-192.0, 0.0, -192.0],
            mx: [192.0, 300.0, 192.0],
        };
        let check = |sim: &mut Sim, a: &PlacementArgs| {
            let (mut p, mut r) = ([1100.0, 0.0, 2100.0], 0.0);
            let ok = check_building_placement(sim, &h, &mut p, &mut r, a);
            (ok, p, r)
        };
        // Branch D: invalid without a gold deposit.
        assert!(!check(&mut sim, &mine).0);
        let g = place(&mut sim, 3500, 10, [1000.0, 0.0, 2000.0], 0.0);
        let mut want = sim.object(g).unwrap().world_pos;
        snap_box_min_corner_to_grid(h.mn, &mut want, 0.0, &rules.map);
        // Search point from the current position, slot 10, position of the deposit snapped.
        assert_eq!(check(&mut sim, &mine), (true, want, 0.0));
        // A field looks for fertile soil, not gold.
        assert!(!check(&mut sim, &field).0);
        place(&mut sim, 3520, 10, [1000.0, 0.0, 2000.0], 0.0);
        assert_eq!(check(&mut sim, &field), (true, want, 0.0));
        // Branch G with region: a tile of the rectangle must lie in the worker's region.
        let rect = placement_rect(h.mn, h.mx, want, 0.0, &rules.map).unwrap();
        let own = i32::from(sim.state.path.region_id(0, rect[0], rect[1]));
        for a in [mine, field] {
            let with = |region| PlacementArgs {
                layer: 0,
                region,
                ..a
            };
            assert!(check(&mut sim, &with(own)).0);
            assert!(!check(&mut sim, &with(0xFFFE)).0);
            assert!(!check(&mut sim, &with(-1)).0);
        }
    }

    #[test]
    fn gate_needs_a_wall_under_every_tile_and_takes_its_rotation() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Wall pieces with the gate wall bit 0x200000 (type 1011 has 0x384000 = also 0x200000).
        let gate = args(0x100A_4000);
        let h = hull(256.0);
        let (mut p, mut r) = ([1024.0, 0.0, 1024.0], 0.0);
        assert!(!check_building_placement(
            &mut sim, &h, &mut p, &mut r, &gate
        ));
        place(&mut sim, 1011, 0, [1088.0, 0.0, 1088.0], 0.0);
        assert_eq!(
            placement_rect(h.mn, h.mx, p, 0.0, &rules.map),
            Some([8, 8, 9, 9])
        );
        // Only one of four tiles carries a wall.
        assert!(!check_building_placement(
            &mut sim, &h, &mut p, &mut r, &gate
        ));
        for (x, z) in [(1216.0, 1088.0), (1088.0, 1216.0), (1216.0, 1216.0)] {
            place(&mut sim, 1011, 0, [x, 0.0, z], 0.0);
        }
        sim.state.occupancy.set_rect([0, 0, 31, 31], true);
        assert!(check_building_placement(
            &mut sim, &h, &mut p, &mut r, &gate
        ));
    }

    // QUIRK W-120: kept
    #[test]
    fn spiral_skips_the_middle_columns_and_shifts_later_rows() {
        let mut tried = Vec::new();
        assert!(!spiral_search(1000.0, 2000.0, [128; 3], |x, z| {
            tried.push((x, z));
            false
        }));
        // Ring 1 as in the model of the ring search: (1128, ...) is never checked.
        assert_eq!(
            tried[..9],
            [
                (1000.0, 2000.0),
                (872.0, 1872.0),
                (872.0, 2128.0),
                (1000.0, 1872.0),
                (1000.0, 2128.0),
                (1000.0, 1872.0),
                (1256.0, 1872.0),
                (1000.0, 2000.0),
                (1256.0, 2000.0),
            ]
        );
        assert!(!tried[..9].iter().any(|&(x, _)| x == 1128.0));
        // Ring 2: rows around the end value of the z pass of ring 1 (2128 -/+ 256), not around 2000.
        assert_eq!(tried[9..11], [(744.0, 1872.0), (744.0, 2384.0)]);
        // 19 rings of 8*k attempts each and the center.
        assert_eq!(tried.len(), 1 + 8 * (1..=19).sum::<usize>());
        // The first success ends the search.
        let mut n = 0;
        assert!(spiral_search(1000.0, 2000.0, [128; 3], |_, _| {
            n += 1;
            n == 5
        }));
        assert_eq!(n, 5);
    }

    #[test]
    fn search_near_returns_the_first_valid_spot() {
        let mut rules = rules();
        rules.model_boxes.insert(
            1001,
            ModelBox {
                center: [0.0, 50.0, 0.0],
                half: [120.0, 50.0, 120.0],
            },
        );
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Center occupied, (872, 1872) too: first hit (872, 2128).
        let busy = |x: f32, z: f32| {
            placement_rect(
                [-120.0, 0.0, -120.0],
                [120.0, 100.0, 120.0],
                [x, 0.0, z],
                0.0,
                &rules.map,
            )
            .unwrap()
        };
        let (c, a) = (busy(1000.0, 2000.0), busy(872.0, 1872.0));
        sim.state.occupancy.set(c[0], c[1], true);
        sim.state.occupancy.set(a[0], a[1], true);
        let (mut x, mut z) = (1000.0, 2000.0);
        assert!(find_building_placement_near(
            &mut sim, &mut x, &mut z, 1001, 0, -1, -1
        ));
        assert_eq!((x, z), (872.0, 2128.0));
        assert_eq!(
            check_placement_for(&mut sim, 1001, [x, 0.0, z], 0.0, 0, None),
            Some(([x, 0.0, z], 0.0))
        );
        assert_eq!(
            check_placement_for(&mut sim, 1001, [1000.0, 0.0, 2000.0], 0.0, 0, None),
            None
        );
        // Without a model or without a building module no search.
        assert!(!find_building_placement_near(
            &mut sim, &mut x, &mut z, 1011, 0, -1, -1
        ));
        assert!(!find_building_placement_near(
            &mut sim, &mut x, &mut z, 3602, 0, -1, -1
        ));
        assert_eq!(building_flags(&rules, 1030), Some(0xC8000));
    }

    /// A7/A8: without the limit, a wall piece with NaN, +-infinity or 3e9 was valid (`ftol` ->
    /// `i32::MIN` gives a "valid" rectangle, the first tile lies outside the map and counts
    /// as free), and the placement search ran endlessly from `|x| ≥ 2³¹` on.
    // QUIRK W-117: deviation
    #[test]
    fn non_finite_and_off_map_poses_are_rejected_before_the_original_check() {
        let mut rules = rules();
        rules.model_boxes.insert(
            1011,
            ModelBox {
                center: [0.0, 50.0, 0.0],
                half: [64.0, 50.0, 64.0],
            },
        );
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let wall = args(0x38_4000);
        let h = hull(20.0);
        let check = |sim: &mut Sim, pos: [f32; 3], rot: f32| {
            let (mut p, mut r) = (pos, rot);
            check_building_placement(sim, &h, &mut p, &mut r, &wall)
        };
        assert!(check(&mut sim, [640.0, 0.0, 1792.0], 0.0));
        for v in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            3.0e9,
            -3.0e9,
            -10.0,
            4096.0,
        ] {
            assert!(!check(&mut sim, [v, 0.0, 1792.0], 0.0), "x = {v}");
            assert!(!check(&mut sim, [640.0, 0.0, v], 0.0), "z = {v}");
        }
        assert!(!check(&mut sim, [640.0, f32::NAN, 1792.0], 0.0));
        assert!(!check(&mut sim, [640.0, 0.0, 1792.0], f32::NAN));
        assert!(!check(&mut sim, [640.0, 0.0, 1792.0], f32::INFINITY));
        // The facade likewise (before the query to the worker).
        assert!(check_placement_for(&mut sim, 1011, [640.0, 0.0, 1792.0], 0.0, 0, None).is_some());
        for (pos, rot) in [
            ([f32::NAN, 0.0, 1792.0], 0.0),
            ([3.0e9, 0.0, 1792.0], 0.0),
            ([640.0, 0.0, 1792.0], f32::NAN),
        ] {
            assert_eq!(check_placement_for(&mut sim, 1011, pos, rot, 0, None), None);
        }
        // Placement search: not finite or far off the map -> immediately `false`, the values stay.
        for (x0, z0) in [
            (f32::NAN, 100.0),
            (100.0, f32::INFINITY),
            (3.0e9, 100.0),
            (100.0, -3.0e9),
        ] {
            let (mut x, mut z) = (x0, z0);
            assert!(!find_building_placement_near(
                &mut sim, &mut x, &mut z, 1011, 0, -1, -1
            ));
            assert_eq!((x.to_bits(), z.to_bits()), (x0.to_bits(), z0.to_bits()));
        }
        // Just beside the map the search still reaches it.
        let (mut x, mut z) = (-256.0, 640.0);
        assert!(find_building_placement_near(
            &mut sim, &mut x, &mut z, 1011, 0, -1, -1
        ));
        assert!(pose_on_map(&rules.map, [x, 0.0, z], 0.0));
        // Cell size 0: no search.
        let mut flat = rules.clone();
        flat.map.cell = [0; 3];
        let mut state = super::tests::state(&rules);
        let mut sim = Sim::new(&mut state, &flat);
        let (mut x, mut z) = (100.0, 100.0);
        assert!(!find_building_placement_near(
            &mut sim, &mut x, &mut z, 1011, 0, -1, -1
        ));
    }

    /// Corpus: start buildings occupy their spot; next to it the search finds a spot for a house,
    /// also with layer and region of a worker.
    #[test]
    fn corpus_start_buildings_block_their_spot_and_a_house_fits_nearby() {
        use crate::setup::{MatchSetup, new_match};
        use opennml_rda::Vfs;
        use opennml_rda::testutil::corpus_game_dir;
        use opennml_world::level::load_level;
        let Some(dir) = corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        assert!(rules.model_boxes.contains_key(&1001));
        let setup = MatchSetup::default_for(&rules);
        let mut state = new_match(&rules, &setup).unwrap();
        let (hq, worker) = {
            let objects: Vec<_> = state.objects().filter(|o| o.owner() < 8).collect();
            let hq = objects
                .iter()
                .find(|o| [1000, 1300, 1600, 1900, 2200, 2500].contains(&o.guid))
                .map(|o| (o.guid, o.owner(), o.world_pos))
                .expect("start building");
            let worker = objects
                .iter()
                .find(|o| o.owner() == hq.1 && o.has_module(0, false))
                .map(|o| o.id)
                .expect("worker");
            (hq, worker)
        };
        let mut sim = Sim::new(&mut state, &rules);
        let (guid, slot, pos) = hq;
        let player = i32::from(slot);
        assert_eq!(
            check_placement_for(&mut sim, guid, pos, 0.0, player, None),
            None
        );
        assert_eq!(
            check_placement_for(&mut sim, 1001, pos, 0.0, player, None),
            None
        );
        let (layer, region) = worker_layer_region(&mut sim, worker);
        assert!(layer >= 0, "Layer {layer}");
        let (mut x, mut z) = (pos[0], pos[2]);
        assert!(find_building_placement_near(
            &mut sim, &mut x, &mut z, 1001, player, layer, region
        ));
        assert_ne!((x, z), (pos[0], pos[2]));
        let spot = check_placement_for(&mut sim, 1001, [x, 0.0, z], 0.0, player, Some(worker));
        assert_eq!(spot, Some(([x, 0.0, z], 0.0)));
    }
}
