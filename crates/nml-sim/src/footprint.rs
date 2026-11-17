//! Occupancy by objects (re/occupancy): block or release tiles in the path grid
//! (`GameEngine_MarkTileForLayers`), set occupancy bits and snap objects to the tile grid
//! (`GameObject_SnapAndBlockFootprint`).

use crate::fx::{Fx, FxVec3, fixed_mul, ftol};
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_cos, sin_deg};
use crate::path::terrain::reclassify_tile;
use crate::sim::Sim;
use crate::world::{grid_to_world, snap_height_to_terrain, wrap_angle};

/// 4 x 4 matrix as in the original (row `i`, column `j`; row vectors, translation in row 3).
pub type Matrix = [[f32; 4]; 4];

fn identity() -> Matrix {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// `Matrix_Multiply(out, a, b)` (`0x00575720`) = `b · a`, each entry in the summation order of the
/// original; the addition onto the zeroed target turns `−0` into `+0`.
fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = (((b[i][0] * a[0][j] + b[i][1] * a[1][j]) + b[i][3] * a[3][j])
                + b[i][2] * a[2][j])
                + 0.0;
        }
    }
    r
}

/// Rotation matrix about the axis `axis` (0 = x, 1 = y, 2 = z) like `0x00575530`/`0x005755A0`/`0x00575610`
/// (`sin`/`cos` like x87). X: m11 = c, m12 = -s, m21 = s, m22 = c; Y: m00 = c, m02 = s, m20 = -s,
/// m22 = c; Z: m00 = c, m01 = -s, m10 = s, m11 = c.
fn rotation(axis: usize, angle: f32) -> Matrix {
    let (s, c) = sin_cos(angle);
    let mut m = identity();
    let (p, q) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    m[p][p] = c;
    m[q][q] = c;
    m[p][q] = -s;
    m[q][p] = s;
    m
}

/// `GameObject_SetWorldMatrix` (`0x00466370`): `S · Ry · Rz · Rx · T`, `sin`/`cos` like x87; scaling
/// happens only if `scale.x ≠ 1` (quirk: y and z are not checked).
pub fn world_matrix(position: [f32; 3], rotation_xyz: [f32; 3], scale: [f32; 3]) -> Matrix {
    let (rx, ry, rz) = (
        rotation(0, rotation_xyz[0]),
        rotation(1, rotation_xyz[1]),
        rotation(2, rotation_xyz[2]),
    );
    let mut m = multiply(&multiply(&rx, &rz), &ry);
    let mut t = identity();
    t[3][..3].copy_from_slice(&position);
    m = multiply(&t, &m);
    if scale[0] != 1.0 {
        let mut s = identity();
        for (i, v) in scale.iter().enumerate() {
            s[i][i] = *v;
        }
        m = multiply(&m, &s);
    }
    m
}

/// World matrix of the snapping in the random map generator (`FUN_00542CF0` `0x00542D06…0x00542D85`,
/// the same in `RandomMapObject_Place` `0x004EC72C…0x004EC798`): `A = Matrix_Multiply(Rx, Ry)`,
/// `B = Matrix_Multiply(T, A)`, `M = Matrix_Multiply(B, Rz)`; with `Matrix_Multiply(a, b) = b · a`
/// thus **`Rz · ((Ry · Rx) · T)`** (row vectors: first rotate about z, then y, then x, then
/// translate). The bracketing determines the rounding; without scaling.
pub fn generator_world_matrix(position: [f32; 3], rotation_xyz: [f32; 3]) -> Matrix {
    let (rx, ry, rz) = (
        rotation(0, rotation_xyz[0]),
        rotation(1, rotation_xyz[1]),
        rotation(2, rotation_xyz[2]),
    );
    let mut t = identity();
    t[3][..3].copy_from_slice(&position);
    multiply(&multiply(&t, &multiply(&rx, &ry)), &rz)
}

/// `Vector3_TransformCoord` (`0x00575250`): affine, without division by w.
pub fn transform_coord(v: [f32; 3], m: &Matrix) -> [f32; 3] {
    [
        ((m[2][0] * v[2] + m[1][0] * v[1]) + v[0] * m[0][0]) + m[3][0],
        ((m[2][1] * v[2] + m[0][1] * v[0]) + m[1][1] * v[1]) + m[3][1],
        ((m[2][2] * v[2] + m[0][2] * v[0]) + m[1][2] * v[1]) + m[3][2],
    ]
}

/// `GameObject_GetFootprintTileRect` (`0x004664E0`): hull center `center` (mesh hulls without
/// animation) ± half extent `half` (header hull), only these two points transformed, sorted per
/// axis, per side inward by `cell/8`, truncated and divided by the cell as integers:
/// `[x0, z0, x1, z1]` inclusive (for narrow hulls also empty, `x0 > x1`).
pub fn footprint_tile_rect(
    center: [f32; 3],
    half: [f32; 3],
    m: &Matrix,
    cell: [i32; 3],
) -> [i32; 4] {
    let mut a = transform_coord(std::array::from_fn(|i| center[i] - half[i]), m);
    let mut b = transform_coord(std::array::from_fn(|i| center[i] + half[i]), m);
    for k in [0, 2] {
        // Swap if not `a ≤ b` (also for NaN).
        if a[k] > b[k] || a[k].is_nan() || b[k].is_nan() {
            std::mem::swap(&mut a[k], &mut b[k]);
        }
    }
    let (sx8, sz8) = ((cell[0] / 8) as f32, (cell[2] / 8) as f32);
    let (x1, z1) = (b[0] - sx8, b[2] - sz8);
    let (x0, z0) = (sx8 + a[0], sz8 + a[2]);
    [
        ftol(x0) / cell[0],
        ftol(z0) / cell[2],
        ftol(x1) / cell[0],
        ftol(z1) / cell[2],
    ]
}

/// `GameWorld_LocalOffsetToGridPos` (`0x00479490`): point in object space (truncated to whole units)
/// rotated with table `sin`/`cos` of whole degrees by `−wrap(direction + 90°)` and added to the
/// fixed-point grid position; returns (x, z) in fixed point.
pub fn local_offset_to_grid(
    v: [f32; 3],
    grid: FxVec3,
    direction: Fx,
    cell: [i32; 3],
) -> (i32, i32) {
    let a = wrap_angle((90 << 12) + direction.0).wrapping_neg();
    let lx = ftol(v[0]).wrapping_shl(12) / cell[0];
    let lz = ftol(v[2]).wrapping_shl(12) / cell[2];
    let (s, c) = (sin_deg(a), cos_deg(a));
    let x = fixed_mul(lz, s)
        .wrapping_add(fixed_mul(lx, c))
        .wrapping_add(grid.x.0);
    let z = fixed_mul(lx, s.wrapping_neg())
        .wrapping_add(fixed_mul(lz, c))
        .wrapping_add(grid.z.0);
    (x, z)
}

/// `GameObject_GetHelperNodeMapPosition` (`0x0046D400` -> `0x0046D190`, re/occupancy):
/// grid position (fixed point) of the helper node `name` from the building model of the type - x and z
/// via [`local_offset_to_grid`], the height `(ftol(v.y) << 12) / SY` plus the own height. `None`
/// without a node. Computed with the rest pose (deviation from the original: with a running animation with a
/// track of this node the original would read the animated position - for `frei…` there is no track in the
/// shipped data, for `ausgang`/`farmer…` unchecked); the direction offset at state
/// `0x0500_0000` (SiegeEngine) does not occur for buildings.
pub fn helper_node_map_position(sim: &Sim, id: ObjectId, name: &str) -> Option<FxVec3> {
    let o = sim.object(id)?;
    let v = sim.rules.building_shapes.get(&o.guid)?.helper_node(name)?;
    let cell = sim.rules.map.cell;
    let (x, z) = local_offset_to_grid(v, o.grid, o.direction, cell);
    let y = (ftol(v[1]).wrapping_shl(12) / cell[1]).wrapping_add(o.grid.y.0);
    Some(FxVec3::new(Fx(x), Fx(y), Fx(z)))
}

/// `GameEngine_MarkTileForLayers` (`0x00432100`): set the cell in the layers of the mask; when
/// releasing, afterwards classify the tile anew according to the terrain (all base layers, without
/// occupancy bit, without clearing).
pub fn mark_tile_for_layers(sim: &mut Sim, mask: u32, x: i32, z: i32, blocked: bool) {
    sim.state.path.mark_cell(mask, x, z, blocked);
    if !blocked {
        reclassify_tile(&mut sim.state.path, &sim.rules.tile_classes, x, z);
    }
}

/// `GameObject_SyncGridFromWorldPos` (`0x00471240`) with `GameObject_SetGridPosition`
/// (`0x00470F00`): fixed-point position from the world position, tile clamped to the map,
/// world position back from the fixed point.
fn sync_grid_from_world_pos(sim: &mut Sim, id: ObjectId) {
    let map = sim.rules.map;
    let Some(o) = sim.object_mut(id) else { return };
    o.grid.x = Fx(Fx::from_f32(o.world_pos[0]).0 / map.cell[0]);
    o.grid.z = Fx(Fx::from_f32(o.world_pos[2]).0 / map.cell[2]);
    let (tx, tz) = o.grid.tile();
    o.tile = (tx.clamp(0, map.dim_x - 1), tz.clamp(0, map.dim_z - 1));
    let w = grid_to_world(o.grid, map.cell);
    o.world_pos[0] = w[0];
    o.world_pos[2] = w[2];
}

/// `GameObject_SnapAndBlockFootprint` (`0x00473B90`): mode 1/1001 snaps to the tile center,
/// 2/1000 to the tile corner; 1 blocks the one tile, 2 the four tiles
/// `(tx,tz)`, `(tx−1,tz−1)`, `(tx−1,tz)`, `(tx,tz−1)`. `path` sets the path grid in `layers`,
/// `occupancy` the occupancy bit, `set` blocks or releases. Afterwards always clear and
/// recompute the height. Mode 10 (rectangle of the helper node `blocking01`) does not occur in the shipped data and
/// here only snaps to the corner (warning); all other modes do not snap.
pub fn snap_and_block_footprint(
    sim: &mut Sim,
    id: ObjectId,
    mode: i32,
    layers: u32,
    occupancy: bool,
    path: bool,
    set: bool,
) {
    let cell = sim.rules.map.cell;
    let (sx, sz) = (cell[0] as f32, cell[2] as f32);
    let Some(o) = sim.object_mut(id) else { return };
    // At exactly 128: quotient, `floor` and product are free of rounding in `f32`.
    let bx = (o.world_pos[0] / sx).floor() * sx;
    let bz = (o.world_pos[2] / sz).floor() * sz;
    let tiles: Vec<(i32, i32)> = match mode {
        1 | 1001 => {
            o.world_pos[0] = sx * 0.5 + bx;
            o.world_pos[2] = bz + sz * 0.5;
            sync_grid_from_world_pos(sim, id);
            let g = sim.object(id).map_or_else(Default::default, |o| o.grid);
            if mode == 1 {
                vec![g.tile()]
            } else {
                Vec::new()
            }
        }
        2 | 1000 | 10 => {
            o.world_pos[0] = bx;
            o.world_pos[2] = bz;
            sync_grid_from_world_pos(sim, id);
            let (tx, tz) = sim.object(id).map_or((0, 0), |o| o.grid.tile());
            match mode {
                2 => vec![(tx, tz), (tx - 1, tz - 1), (tx - 1, tz), (tx, tz - 1)],
                10 => {
                    log::warn!(
                        "Object {}: snap mode 10 (blocking01) is not supported",
                        id.0
                    );
                    Vec::new()
                }
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    };
    for (x, z) in tiles {
        if path {
            mark_tile_for_layers(sim, layers, x, z, set);
        }
        if occupancy {
            sim.state.occupancy.set(x, z, set);
        }
    }
    sim.state.path.flush_dirty_rect();
    snap_height_to_terrain(sim, id);
}

/// Occupancy part of `PropertyResource_Init` (`0x00521340`): raw flags `& 0x106` -> square with
/// radius 1, otherwise `& 0x206` -> radius 3 around the tile of the object (gold 3 x 3, fertile soil
/// 7 x 7); never cleared.
pub fn resource_occupancy(sim: &mut Sim, id: ObjectId, flags: u32) {
    let n = if flags & 0x106 != 0 {
        1
    } else if flags & 0x206 != 0 {
        3
    } else {
        return;
    };
    let Some((tx, tz)) = sim.object(id).map(|o| o.grid.tile()) else {
        return;
    };
    sim.state
        .occupancy
        .set_rect([tx - n, tz - n, tx + n, tz + n], true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxVec3;
    use crate::path::BLOCKED;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    const TYPES: &str =
        "GUID 3700\nNAME Stein\nVALUEID 100 6 1 512\nPROPID 5\nVALUEID 60001 1 1 0\n";

    #[test]
    fn modes_snap_to_centres_or_corners_and_block_their_tiles() {
        let rules = Rules::synthetic(TYPES, 16);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let stone = create_object(
            &mut sim,
            &mut ObjectDesc::new(3700, 0, [1000.0, 0.0, 300.0]),
        )
        .unwrap();
        // Mode 1: center of tile (7, 2), layers 0-4 blocked, occupancy bit set.
        snap_and_block_footprint(&mut sim, stone, 1, 0x1F, true, true, true);
        let o = sim.object(stone).unwrap();
        assert_eq!(o.world_pos, [960.0, 0.0, 320.0]);
        assert_eq!(o.grid, FxVec3::new(Fx(30720), Fx(0), Fx(10240)));
        assert_eq!(sim.state.path.layers[0].cell(7, 2), BLOCKED);
        assert_eq!(sim.state.path.layers[4].cell(7, 2), BLOCKED);
        assert!(sim.state.occupancy.test(7, 2));
        // Derived layer 5 updated (clearing).
        assert_eq!(sim.state.path.layers[5].cell(8, 3), BLOCKED);
        // Releasing restores the terrain (flat: free except layer 4 over water).
        snap_and_block_footprint(&mut sim, stone, 1, 0x1F, true, true, false);
        assert_eq!(sim.state.path.layers[0].cell(7, 2), 0);
        assert_eq!(sim.state.path.layers[4].cell(7, 2), BLOCKED);
        assert!(!sim.state.occupancy.test(7, 2));
        // Mode 2: corner, four tiles.
        snap_and_block_footprint(&mut sim, stone, 2, 0x01, false, true, true);
        assert_eq!(sim.object(stone).unwrap().world_pos[0], 896.0);
        for (x, z) in [(7, 2), (6, 1), (6, 2), (7, 1)] {
            assert_eq!(sim.state.path.layers[0].cell(x, z), BLOCKED);
        }
        // Mode 0: no tiles, no snapping.
        let before = sim.object(stone).unwrap().world_pos;
        snap_and_block_footprint(&mut sim, stone, 0, 0x1F, true, true, true);
        assert_eq!(sim.object(stone).unwrap().world_pos, before);
    }

    /// Bits of a matrix row by row (for comparisons without a rounding tolerance).
    fn bits(m: &Matrix) -> [[u32; 4]; 4] {
        m.map(|row| row.map(f32::to_bits))
    }

    #[test]
    fn generator_matrix_rotates_z_then_y_then_x() {
        // Only y = pi/2: `Rz · ((Ry · Rx) · T)` = Ry with translation in row 3 (zeros as +0).
        let m =
            generator_world_matrix([640.0, 0.0, 384.0], [0.0, std::f32::consts::FRAC_PI_2, 0.0]);
        let (s, c) = sin_cos(std::f32::consts::FRAC_PI_2);
        assert_eq!((s.to_bits(), c.to_bits()), (0x3F80_0000, 0xB33B_BD2E));
        assert_eq!(
            bits(&m),
            [
                [0xB33B_BD2E, 0x0000_0000, 0x3F80_0000, 0x0000_0000],
                [0x0000_0000, 0x3F80_0000, 0x0000_0000, 0x0000_0000],
                [0xBF80_0000, 0x0000_0000, 0xB33B_BD2E, 0x0000_0000],
                [0x4420_0000, 0x0000_0000, 0x43C0_0000, 0x3F80_0000],
            ]
        );
        // Row vectors: v * Rz * Ry * Rx + T - first rotate about z, then y, then x (recomputed in f64).
        let (ax, ay, az) = (0.3_f64, 0.5_f64, 0.7_f64);
        let rot = |v: [f64; 3], axis: usize, a: f64| -> [f64; 3] {
            // v * R with R like `rotation` (m[p][p] = c, m[p][q] = -s, m[q][p] = s).
            let (p, q) = [(1, 2), (2, 0), (0, 1)][axis];
            let (s, c) = a.sin_cos();
            let mut r = v;
            r[p] = v[p] * c + v[q] * s;
            r[q] = -v[p] * s + v[q] * c;
            r
        };
        let m = generator_world_matrix([640.0, 10.0, 384.0], [ax as f32, ay as f32, az as f32]);
        for v in [
            [1.0_f32, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [30.0, -20.0, 50.0],
        ] {
            let e = rot(rot(rot(v.map(f64::from), 2, az), 1, ay), 0, ax);
            let got = transform_coord(v, &m);
            let t = [640.0, 10.0, 384.0];
            for k in 0..3 {
                assert!(
                    (f64::from(got[k]) - (e[k] + t[k])).abs() < 1e-3,
                    "{v:?}: {got:?} {e:?}"
                );
            }
        }
        // Bracketing `Rz · ((Ry · Rx) · T)` fixed (bits, FNV-1a 64 over all 16 entries).
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        for b in bits(&m).iter().flatten().flat_map(|b| b.to_le_bytes()) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3);
        }
        assert_eq!(h, GENERATOR_MATRIX_DIGEST, "{:#x}", h);
    }

    /// Measured: bits of `generator_world_matrix([640, 10, 384], [0.3, 0.5, 0.7])`.
    const GENERATOR_MATRIX_DIGEST: u64 = 0xAFDD_56C5_C333_D1C5;

    #[test]
    fn generator_matrix_differs_from_object_matrix() {
        let (p, r) = ([640.0, 10.0, 384.0], [0.3, 0.5, 0.7]);
        let generator = generator_world_matrix(p, r);
        let object = world_matrix(p, r, [1.0; 3]);
        assert_ne!(bits(&generator), bits(&object));
        // The same rotation about only one axis: both sequences coincide.
        for axis in 0..3 {
            let mut one = [0.0; 3];
            one[axis] = 0.5;
            assert_eq!(
                bits(&generator_world_matrix(p, one)),
                bits(&world_matrix(p, one, [1.0; 3]))
            );
        }
    }
}
