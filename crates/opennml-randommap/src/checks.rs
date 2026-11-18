//! Checks per placement attempt of the object step (re/random-map Z.7.3, ZK-b.4; quirks D-337, D-340,
//! D-357, D-358, D-360).
//!
//! Strict order with short-circuiting like in the placers `FUN_005451E0`/`FUN_00544B40`
//! (`0x0054563A…0x005456A0`): snapping (1) -> tile (2) -> player areas (3) -> `PutOnto` (4) ->
//! [`PutOnFlat` (5)] -> border (6) -> occupancy (7) -> [group distance (8)]. No check draws randomness.
//! `Fill` (`FUN_00545760`) takes only individual building blocks ((3)/(4), snapping, occupancy).
//!
//! Floating point in `f32` like the x87 flow under 24-bit precision (each operation rounded once),
//! fixed point via `opennml_sim::fx`.

#[cfg(test)]
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use opennml_formats::prj::Project;
use opennml_formats::rnd::{PlaceObjectDef, RandomMapScript};
use opennml_sim::combat::hit_zone::vector3_length;
use opennml_sim::footprint::generator_world_matrix;
use opennml_sim::fx::ftol;

use crate::PlayerArea;
use crate::geometry::{ModelGeometry, ground_rect, snap_hull};
use crate::grid::Grid;
use crate::objects::{MapObject, set_object};
use crate::paint::texture_number;
use crate::relief;
use crate::rng::GenRand;
use crate::terrain_areas::player_distance;

/// Hook for placing a passed attempt (`FUN_0053FD00`): GUID, attempt, owner, randomness (R7). The default
/// is [`objects::set_object`](crate::objects::set_object); tests set their own hooks. The placer
/// paints `GroundTexture` itself afterwards (like the original).
pub(crate) type SetObject = for<'c> fn(&mut ObjectCtx<'c>, u32, &Attempt, i32, &mut GenRand);

/// State of the object step (`FUN_005468D0` and the placers): what the checks read, plus project,
/// file source, geometry cache, rotation table, object list and slots.
pub(crate) struct ObjectCtx<'a> {
    /// Working grid (texture numbers for `PutOnto`; `GroundTexture` writes into it).
    pub grid: &'a mut Grid,
    /// Map script (texture number from the name, `FUN_00541710`; `#Object` list, `FUN_005417D0`).
    pub script: &'a RandomMapScript,
    /// Player areas in list order (`Generator+0x40`).
    pub players: &'a [PlayerArea],
    /// Project of the script (`Project_FindObject` `0x004FB670`).
    pub project: &'a Project,
    /// File source for the models (`None` = file missing).
    pub read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    /// The object step's own blocking map (D-337).
    pub block: BlockMap,
    /// Face normals per tile for `PutOnFlat`.
    pub normals: FaceNormals,
    /// Earlier snapped group centers of the current block (`FUN_005410E0`, list per block, with
    /// `ForAllPlayers` across all areas). The placer clears it per block and, after a passed check (8),
    /// appends [`Attempt::position`] (`FUN_0048FBA0` `0x00541175`).
    pub centres: Vec<[f32; 3]>,
    /// Center for the group members (`[ESP+0x30…0x38]` in `FUN_005451E0`): the **unsnapped**
    /// position of the most recently **drawn** center attempt, including a failed one (D-354); `None`
    /// until a center attempt was drawn in the block (undetermined stack values in the original).
    pub centre: Option<[f32; 2]>,
    /// Model geometry per normalized model path across all objects (model cache
    /// `FUN_0057DB80`/`FUN_0057D3C0`); `None` = model not loadable (D-355).
    pub geometries: BTreeMap<String, Option<Rc<ModelGeometry>>>,
    /// `OBJ_YROT` per GUID, as the random rotation overwrites it in the project object (D-336);
    /// if the GUID is missing, `default_rotation[1]` of the project applies.
    pub rotations: BTreeMap<u32, f32>,
    /// Placed objects in placement order (`Level+0x30`).
    pub objects: Vec<MapObject>,
    /// Occupied slots 0...7 (`Level+0x50`, `FUN_005473B0`).
    pub slots_used: [bool; 8],
    /// Placing a passed attempt (hook).
    pub set_object: SetObject,
    /// Edge length of the map in tiles (`DIM_X` = `DIM_Z`).
    pub dim: i32,
    /// Tile size `SX` = `SZ` (128).
    pub tile_size: f32,
    /// Test hook: how often each check ran (order as in [`Check`]); test build only.
    #[cfg(test)]
    pub checks_run: Cell<[u32; 8]>,
    /// Test hook: attempts as a single object or group center (0) and as a member (1).
    #[cfg(test)]
    pub attempts_run: [u32; 2],
}

impl<'a> ObjectCtx<'a> {
    /// Start of the object step: blocking map = copy of `steep`, normals from the heights, empty
    /// caches and lists, fresh rotation table, placing via [`set_object`].
    pub(crate) fn new(
        grid: &'a mut Grid,
        script: &'a RandomMapScript,
        players: &'a [PlayerArea],
        steep: &[u8],
        project: &'a Project,
        read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
        tile_size: f32,
    ) -> Self {
        debug_assert_eq!(grid.dim_x, grid.dim_z);
        let block = BlockMap::from_steep(steep, grid.dim_x);
        let normals = FaceNormals::from_grid(grid, tile_size);
        let dim = grid.dim_x as i32;
        ObjectCtx {
            grid,
            script,
            players,
            project,
            read,
            block,
            normals,
            centres: Vec::new(),
            centre: None,
            geometries: BTreeMap::new(),
            rotations: BTreeMap::new(),
            objects: Vec::new(),
            slots_used: [false; 8],
            set_object,
            dim,
            tile_size,
            #[cfg(test)]
            checks_run: Cell::new([0; 8]),
            #[cfg(test)]
            attempts_run: [0; 2],
        }
    }

    /// In the test build, counts that check `check` ran; otherwise has no effect.
    #[inline(always)]
    fn note(&self, check: Check) {
        #[cfg(test)]
        {
            let mut run = self.checks_run.get();
            run[check as usize] += 1;
            self.checks_run.set(run);
        }
        #[cfg(not(test))]
        let _ = check;
    }
}

/// The checks of an attempt in flow order (index of the test hook).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    Snap,
    Tile,
    PlayerAreas,
    PutOnto,
    PutOnFlat,
    Border,
    Block,
    GroupDistance,
}

/// The two face normals per tile (`Tile+0x0C` triangle A, `+0x18` triangle B), index
/// `dim_x·z + x`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaceNormals {
    pub a: Vec<[f32; 3]>,
    pub b: Vec<[f32; 3]>,
    dim_x: usize,
    dim_z: usize,
}

impl FaceNormals {
    /// From the heights like `Terrain_ComputeTileFaceNormals` (`0x0055AED0`), with exactly the same
    /// points and calls as [`relief::steep_flags`] ([`relief::face_normals`]). Between steepness
    /// and object step nobody changes the heights (`FUN_00547110` only sets light).
    pub(crate) fn from_grid(grid: &Grid, tile_size: f32) -> Self {
        let (a, b) = relief::face_normals(grid, tile_size)
            .into_iter()
            .map(|(a, b)| (a.to_array(), b.to_array()))
            .unzip();
        FaceNormals {
            a,
            b,
            dim_x: grid.dim_x,
            dim_z: grid.dim_z,
        }
    }
}

/// Blocking map of the object step (D-337, D-349): steepness plus footprints of objects already
/// placed; in the original the byte field `Terrain+0x10`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockMap {
    dim: usize,
    cells: Vec<u8>,
}

impl BlockMap {
    /// Copy of the steepness at the start of the object step (`Terrain_ComputeSteepTileFlags`
    /// `0x0055BE90` had overwritten the field completely beforehand).
    pub(crate) fn from_steep(steep: &[u8], dim: usize) -> Self {
        debug_assert_eq!(steep.len(), dim * dim);
        BlockMap {
            dim,
            cells: steep.to_vec(),
        }
    }

    /// `FUN_00558270` ("rectangle free?", only caller `FUN_00543560`): `x` outer, `z` inner,
    /// `x0 > x1` -> free; only tiles in the map count, any byte != 0 blocks.
    pub(crate) fn is_free(&self, rect: [i32; 4]) -> bool {
        tiles_in_map(rect, self.dim, self.dim).all(|(x, z)| self.cells[self.dim * z + x] == 0)
    }

    /// `ByteGrid_SetRect` (`0x005581B0`, from `RandomMapObject_Place` `0x004EC842`): same
    /// bounds, writes 1 into the tiles of the map.
    pub(crate) fn set(&mut self, rect: [i32; 4]) {
        let dim = self.dim;
        for (x, z) in tiles_in_map(rect, dim, dim) {
            self.cells[dim * z + x] = 1;
        }
    }

    pub(crate) fn cells(&self) -> &[u8] {
        &self.cells
    }
}

/// The tiles of the inclusive rectangle `[x0, z0, x1, z1]` that lie in the map, `x` outer,
/// `z` inner - like the loops `for (x = x0; x <= x1; x++) for (z = z0; z <= z1; z++) if (in
/// map)` of the original (`0x00558270`, `0x005581B0`, `0x0055BFA0`). The range is clipped to the
/// map beforehand; the result of each check is the same, only without loops over tiles
/// outside (e.g. `ftol(NaN)` = `i32::MIN`).
pub(crate) fn tiles_in_map(
    rect: [i32; 4],
    dim_x: usize,
    dim_z: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let [x0, z0, x1, z1] = rect.map(i64::from);
    let (x0, x1) = (x0.max(0), x1.min(dim_x as i64 - 1));
    let (z0, z1) = (z0.max(0), z1.min(dim_z as i64 - 1));
    (x0..=x1).flat_map(move |x| (z0..=z1).map(move |z| (x as usize, z as usize)))
}

/// Snapping of one axis, `FUN_00542CF0` (Z.7.3 (1), D-357): `w = max − min` (f32); `w < s`
/// (ordered, `0x00542E29 … 0x00542E30`) -> `x = f32(ftol(x/s)·s + s/2)` (`IMUL` integer, `s/2`
/// toward 0, `FIADD`; origin of the object onto the tile center, positions in `(−s, 0)` land on
/// `s/2`); otherwise (`w ≥ s` or NaN, `TEST AH,5; JP`) `t = f32(min/s)`,
/// `x = f32(x − s·f32(t − ftol(floor(t))))` (`0x00542E69 … 0x00542EA8`, left hull edge onto the
/// tile boundary).
pub(crate) fn snap_to_grid(pos: f32, min: f32, max: f32, s: f32) -> f32 {
    let s_int = s as i32;
    let w = max - min;
    if w < s {
        let cell = ftol(pos / s).wrapping_mul(s_int);
        // `FILD` + `FIADD` exact, one rounding on storing.
        (f64::from(cell) + f64::from(s_int / 2)) as f32
    } else {
        let t = min / s;
        // `floor` (`0x005D4980`) on the `double` of `t`: integral, hence representable in `f32`.
        let fl = ftol(t.floor());
        // `FILD fl; FSUBR t`: one rounding.
        let frac = (f64::from(t) - f64::from(fl)) as f32;
        pos - s * frac
    }
}

/// Tile of the attempt (Z.7.3 (2)): `(ftol(x/s), ftol(z/s))`, toward 0.
pub(crate) fn tile_of(x: f32, z: f32, s: f32) -> (i32, i32) {
    (ftol(x / s), ftol(z / s))
}

/// `AvoidPlayerAreas` in `FUN_00543410` (`0x00543440 … 0x00543485`, Z.7.3 (3)): per area
/// `ToInt(Fixed_Hypot(FromInt(tx − cx), FromInt(tz − cz))) < minSize` -> no. If `Fixed_Hypot`
/// divided by zero (|distance| a multiple of 1024 tiles; a crash in the original, D-335), the shared
/// guard [`player_distance`] reports an error instead of a panic. Reachable because (3) runs before
/// the border (6) (members with a large `maxGroupRadius`, `ForAllPlayers` with a large
/// `maxDistToPlayers`).
pub(crate) fn far_from_players(tx: i32, tz: i32, areas: &[PlayerArea]) -> anyhow::Result<bool> {
    for a in areas {
        if player_distance(tx.wrapping_sub(a.cx), tz.wrapping_sub(a.cz))? < a.min_size {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `PutOnto` in `FUN_00543410` (`0x005434AA … 0x0054353E`, Z.7.3 (4), D-340, D-358): all tiles
/// `[tx − r, tx + r] × [tz − r, tz + r]` **in the map** carry `number`; tiles outside count
/// as matching. `r = avoid_other < 0` does not run the loop (`0x005434CE` `JG`): yes.
pub(crate) fn put_onto_ok(grid: &Grid, tx: i32, tz: i32, number: i32, avoid_other: i32) -> bool {
    let r = avoid_other;
    let rect = [
        tx.wrapping_sub(r),
        tz.wrapping_sub(r),
        tx.wrapping_add(r),
        tz.wrapping_add(r),
    ];
    tiles_in_map(rect, grid.dim_x, grid.dim_z).all(|(x, z)| grid.tile(x, z) == number)
}

/// `PutOnFlat` (`FUN_005436B0` -> `FUN_0055BFA0`, Z.7.3 (5), D-340): every tile of the
/// footprint rectangle in the map has both normals **exactly** `(0, −1, 0)` (`FUN_00574FB0`:
/// per component ordered equal, NaN unequal, `−0 == +0`). Exactly flat means: all three
/// corner heights of the triangle equal (x/z of the normal are products of height difference and
/// `S`), so the result does not depend on the rounding order of the normalization (`0x0055AED0`).
pub(crate) fn flat_ok(normals: &FaceNormals, rect: [i32; 4]) -> bool {
    const DOWN: [f32; 3] = [0.0, -1.0, 0.0];
    let flat = |n: &[f32; 3]| n[0] == DOWN[0] && n[1] == DOWN[1] && n[2] == DOWN[2];
    tiles_in_map(rect, normals.dim_x, normals.dim_z).all(|(x, z)| {
        let i = normals.dim_x * z + x;
        flat(&normals.a[i]) && flat(&normals.b[i])
    })
}

/// Border (`FUN_005409E0` -> `FUN_00557620`, Z.7.3 (6)): `(x, z)`, `(x + r, z + r)` and
/// `(x − r, z − r)` (sums `f32`) each strictly in `(0, S·W) × (0, S·H)`; NaN -> no. The limit is
/// the integer product (`IMUL`), loaded **unrounded** via `FILD` and compared against the point -
/// not `f32(S·W)`; the reimplementation therefore compares in `f64` (exact for every `i32`). `r` is
/// the sphere radius of the file, the rotation does not count.
pub(crate) fn inside_map(x: f32, z: f32, r: f32, w: i32, h: i32, s: f32) -> bool {
    let s_int = s as i32;
    let (bx, bz) = (
        f64::from(s_int.wrapping_mul(w)),
        f64::from(s_int.wrapping_mul(h)),
    );
    let inside = |px: f32, pz: f32| {
        let (px, pz) = (f64::from(px), f64::from(pz));
        px > 0.0 && bx > px && pz > 0.0 && bz > pz
    };
    inside(x, z) && inside(x + r, z + r) && inside(x - r, z - r)
}

/// Group distance (`FUN_005410E0`, Z.7.3 (8), D-360): with `minGroupDistance > 0`,
/// `Vector3_Length(c − e)` (3D, f32) to every earlier center `e` must be at least
/// `f32(SX · minGroupDistance)` (also for z `SX`); `len < L` -> no, NaN counts as far enough
/// (`TEST AH,5; JNP`). Appending to the list is done by the placer.
pub(crate) fn group_distance_ok(
    c: [f32; 3],
    earlier: &[[f32; 3]],
    min_group_distance: i32,
    s: f32,
) -> bool {
    if min_group_distance <= 0 {
        return true;
    }
    let limit = (s as i32).wrapping_mul(min_group_distance) as f32;
    earlier.iter().all(|e| {
        let len = vector3_length([c[0] - e[0], c[1] - e[1], c[2] - e[2]]);
        len >= limit || len.is_nan()
    })
}

/// A passed attempt: snapped position (y = 0), tile and footprint rectangle
/// `[x0, z0, x1, z1]` (for `SetRect` and `GroundTexture` at the same position).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Attempt {
    pub position: [f32; 3],
    pub tile: (i32, i32),
    pub rect: [i32; 4],
}

/// Snapping (1) without `NoGrid`, `FUN_00542CF0`: one hull from the world matrix at the position
/// before snapping, for both axes.
fn snap(
    ctx: &ObjectCtx,
    def: &PlaceObjectDef,
    geo: &ModelGeometry,
    rot: [f32; 3],
    x: f32,
    z: f32,
) -> (f32, f32) {
    if def.no_grid {
        return (x, z);
    }
    ctx.note(Check::Snap);
    let s = ctx.tile_size;
    let (mn, mx) = snap_hull(geo, &generator_world_matrix([x, 0.0, z], rot));
    (
        snap_to_grid(x, mn[0], mx[0], s),
        snap_to_grid(z, mn[2], mx[2], s),
    )
}

/// `FUN_00543410(tx, tz, def)`: (3) with `AvoidPlayerAreas`, then (4) with a non-empty `PutOnto`,
/// at tile `(tx, tz)`. `Err` only as with [`far_from_players`].
pub(crate) fn tile_ok(
    ctx: &ObjectCtx,
    def: &PlaceObjectDef,
    tx: i32,
    tz: i32,
) -> anyhow::Result<bool> {
    if def.avoid_player_areas {
        ctx.note(Check::PlayerAreas);
        if !far_from_players(tx, tz, ctx.players)? {
            return Ok(false);
        }
    }
    if !def.put_onto.is_empty() {
        ctx.note(Check::PutOnto);
        let number = texture_number(ctx.script, &def.put_onto);
        if !put_onto_ok(ctx.grid, tx, tz, number, def.avoid_other) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Rest of a `Fill` attempt (`FUN_00545760` `0x0054586E…0x005458F1`), after tile
/// `tile` passed [`tile_ok`] and the GUID was drawn: position `(x, 0, z)` = `(x·SX, z·SZ)`,
/// snapping (without `NoGrid`), then **only** occupancy (7) at the snapped position. No
/// `PutOnFlat` (5), no border (6), no new tile: (3)/(4) applied to the loop tile, even if
/// snapping shifts the position. [`Attempt::tile`] is therefore the loop tile.
pub(crate) fn fill_attempt(
    ctx: &ObjectCtx,
    def: &PlaceObjectDef,
    geo: &ModelGeometry,
    rot: [f32; 3],
    x: f32,
    z: f32,
    tile: (i32, i32),
) -> Option<Attempt> {
    let (x, z) = snap(ctx, def, geo, rot, x, z);
    let position = [x, 0.0, z];
    let rect = ground_rect(
        geo,
        &generator_world_matrix(position, rot),
        ctx.tile_size as i32,
    );
    ctx.note(Check::Block);
    ctx.block.is_free(rect).then_some(Attempt {
        position,
        tile,
        rect,
    })
}

/// An attempt at `(x, 0, z)` with the project rotation `rot` (`OBJ_XROT/YROT/ZROT`) for
/// `FUN_005451E0`/`FUN_00544B40` (`0x0054563A…0x005456A0`): snapping (without `NoGrid`, world
/// matrix at the position before snapping) -> tile -> (3) with `AvoidPlayerAreas` -> (4) with a
/// non-empty `PutOnto` -> (5) with `PutOnFlat` -> (6) -> (7) -> (8) only for a group center
/// (`numGroups > 1`, `j == 0`). The first "no" ends the attempt (`Ok(None)`). The rectangles of (5)
/// and (7) come from the world matrix at the snapped position (`FUN_005436B0`, `FUN_00543560`).
/// `Err` only if (3) divided by zero in the original (D-335, [`far_from_players`]).
pub(crate) fn check_attempt(
    ctx: &ObjectCtx,
    def: &PlaceObjectDef,
    geo: &ModelGeometry,
    rot: [f32; 3],
    x: f32,
    z: f32,
    group_centre: bool,
) -> anyhow::Result<Option<Attempt>> {
    let s = ctx.tile_size;
    // (1) Snapping.
    let (x, z) = snap(ctx, def, geo, rot, x, z);
    // (2) Tile.
    ctx.note(Check::Tile);
    let (tx, tz) = tile_of(x, z, s);
    // (3) and (4): `FUN_00543410`.
    if !tile_ok(ctx, def, tx, tz)? {
        return Ok(None);
    }
    let position = [x, 0.0, z];
    let rect = ground_rect(geo, &generator_world_matrix(position, rot), s as i32);
    // (5) `PutOnFlat`.
    if def.put_on_flat {
        ctx.note(Check::PutOnFlat);
        if !flat_ok(&ctx.normals, rect) {
            return Ok(None);
        }
    }
    // (6) Border.
    ctx.note(Check::Border);
    if !inside_map(x, z, geo.radius, ctx.dim, ctx.dim, s) {
        return Ok(None);
    }
    // (7) Occupancy.
    ctx.note(Check::Block);
    if !ctx.block.is_free(rect) {
        return Ok(None);
    }
    // (8) Group distance.
    if group_centre {
        ctx.note(Check::GroupDistance);
        if !group_distance_ok(position, &ctx.centres, def.min_group_distance, s) {
            return Ok(None);
        }
    }
    Ok(Some(Attempt {
        position,
        tile: (tx, tz),
        rect,
    }))
}

#[cfg(test)]
mod tests;
