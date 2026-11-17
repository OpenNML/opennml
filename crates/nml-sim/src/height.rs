//! Terrain height as in the original (`Terrain_GetHeightAt` `0x0055A940`, re/motion 6.4): a
//! vertical ray from `y + 1000` to `y − 1000` against the two triangles of the tile
//! (Moller-Trumbore, `Ray_IntersectTriangle` `0x00577440`), every operation in `f32` in the order
//! of the original (x87 with 24-bit precision). In rare cases the result depends on the old
//! height `y` of the object (quirk).

use crate::fx::ftol;
use crate::rules::Rules;

type V3 = [f32; 3];

/// `Vector3_Subtract` (`0x00576280`).
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `Vector3_Dot` (`0x005750B0`): `(z·z + y·y) + x·x`.
fn dot(a: V3, b: V3) -> f32 {
    (a[2] * b[2] + a[1] * b[1]) + a[0] * b[0]
}

/// `Vector3_Cross` (`0x005751F0`).
fn cross(a: V3, b: V3) -> V3 {
    [
        b[2] * a[1] - b[1] * a[2],
        b[0] * a[2] - b[2] * a[0],
        b[1] * a[0] - b[0] * a[1],
    ]
}

/// `Vector3_Normalize` (`0x00575170`).
fn normalize(v: V3) -> V3 {
    let s = (v[0] * v[0] + v[1] * v[1]) + v[2] * v[2];
    let inv = 1.0 / s.sqrt();
    [inv * v[0], inv * v[1], inv * v[2]]
}

/// `Ray_IntersectTriangle` (`0x00577440`): two-sided, without a distance limit; distance `t`.
fn intersect(origin: V3, dir: V3, v0: V3, v1: V3, v2: V3) -> Option<f32> {
    let e1 = sub(v1, v0);
    let e2 = sub(v2, v0);
    let p = cross(dir, e2);
    let mut det = dot(e1, p);
    let t = if det > 0.0 {
        sub(origin, v0)
    } else {
        det = -det;
        sub(v0, origin)
    };
    // `JNP` test: NaN counts as "no hit".
    if det.is_nan() || det < 1e-4 {
        return None;
    }
    let u = dot(t, p);
    if u < 0.0 || u > det {
        return None;
    }
    let q = cross(t, e1);
    let v = dot(dir, q);
    if v < 0.0 || v + u > det {
        return None;
    }
    let inv = 1.0 / det;
    Some(dot(e2, q) * inv)
}

/// `Terrain_GetHeightAt(x, y, z)` in world units: outside the map (also NaN) `WATER_LEVEL`,
/// otherwise the height of the first tile triangle hit, rounded down to 0.005.
pub fn terrain_height(rules: &Rules, x: f32, y: f32, z: f32) -> f32 {
    let map = rules.map;
    let (sx, sz) = (map.cell[0], map.cell[2]);
    let width = map.dim_x.wrapping_mul(sx) as f32;
    let depth = map.dim_z.wrapping_mul(sz) as f32;
    if x.is_nan() || x < 0.0 || x >= width || z.is_nan() || z < 0.0 || z >= depth {
        return map.water_level;
    }
    let oy = y + 1000.0;
    let origin = [x, oy, z];
    let dir = normalize(sub([x, y - 1000.0, z], origin));
    let (tx, tz) = ((ftol(x) / sx) as usize, (ftol(z) / sz) as usize);
    let v = |ix: usize, iz: usize| rules.terrain.vertex(ix, iz).position.to_array();
    let a = v(tx, tz + 1);
    let b = v(tx, tz);
    let c = v(tx + 1, tz);
    let e = v(tx + 1, tz + 1);
    for (p0, p1, p2) in [(a, b, c), (a, c, e)] {
        if let Some(t) = intersect(origin, dir, p0, p1, p2) {
            return ((oy - t) * 200.0).floor() * f32::from_bits(0x3BA3_D70A);
        }
    }
    map.water_level
}

/// `GameEngine_GetTerrainHeightFixed` (`0x004319A0`): height under the grid position in fixed point
/// of the y cell, `trunc(h · 4096) / cell_y`.
pub fn terrain_height_fx(rules: &Rules, world: [f32; 3]) -> i32 {
    let h = terrain_height(rules, world[0], world[1], world[2]);
    ftol(h * 4096.0).wrapping_div(rules.map.cell[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 x 4 tiles with heights `((i·37 + j·91) mod 23) · 13.37 − 40`, water -7.25.
    fn rules() -> Rules {
        let mut r = Rules::synthetic("", 4);
        r.map.water_level = -7.25;
        let vx = r.terrain.verts_x;
        for (k, v) in r.terrain.vertices.iter_mut().enumerate() {
            let (i, j) = (k % vx, k / vx);
            v.position.y = ((i * 37 + j * 91) % 23) as f32 * 13.37 - 40.0;
        }
        r
    }

    #[test]
    fn heights_match_the_ray_test_of_the_original() {
        let r = rules();
        // (x, y, z) -> bits of the height, fixed point at cell height 128 (Python reimplementation, re/motion 6.4).
        for (x, y, z, bits, fx) in [
            (10.5, 0.0, 20.25, 0x41AF_147B, 700),
            (127.9, 50.0, 3.0, 0x4312_F5C2, 4702),
            (128.0, 0.0, 128.0, 0x4305_CF5C, 4281),
            (300.3, 100.0, 450.7, 0x4232_3333, 1425),
            (300.3, -900.0, 450.7, 0x4232_3333, 1425),
            (64.0, 0.0, 64.0, 0x4348_A8F5, 6421),
            (511.0, 0.0, 511.0, 0x4225_0A3D, 1320),
            (512.0, 0.0, 10.0, 0xC0E8_0000, -232),
            (-0.5, 0.0, 10.0, 0xC0E8_0000, -232),
            (200.0, 5000.0, 333.0, 0x4232_D70A, 1430),
            (77.77, 12.5, 401.1, 0x4318_8000, 4880),
            (450.0, 20.0, 60.0, 0x4311_B70A, 4662),
        ] {
            let h = terrain_height(&r, x, y, z);
            assert_eq!(h.to_bits(), bits, "height at ({x}, {y}, {z})");
            assert_eq!(terrain_height_fx(&r, [x, y, z]), fx);
        }
        // The old height of the object shifts the ray start: different result (quirk).
        for (xb, zb, y, a, b) in [
            (0x4300_52D0, 0x4369_2CED, 250.0, 0x42F5_0A3D, 0x42F5_07AE),
            (0x43B3_BAD1, 0x41D0_F66D, 250.0, 0x432F_68F6, 0x432F_6A3D),
            (0x43F6_295D, 0x43D6_A182, 100.0, 0x4286_B852, 0x4286_B5C2),
        ] {
            let (x, z) = (f32::from_bits(xb), f32::from_bits(zb));
            assert_eq!(terrain_height(&r, x, 0.0, z).to_bits(), a);
            assert_eq!(terrain_height(&r, x, y, z).to_bits(), b);
        }
        assert_eq!(terrain_height(&r, f32::NAN, 0.0, 1.0), -7.25);
    }
}
