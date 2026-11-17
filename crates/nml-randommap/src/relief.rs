//! Relief of the generator (re/random-map Z.6.8-Z.6.10): heights from the textures `FUN_00540FD0`,
//! smoothing `FUN_00540AD0` and the steepness field `Terrain_ComputeSteepTileFlags` (`0x0055BE90`).
//! No randomness. Light and normals are computed later by `nml_world::terrain::Terrain::new` from
//! the same positions.

use anyhow::bail;
use glam::Vec3;
use nml_formats::rnd::RandomMapScript;
use nml_world::terrain::tile_face_normals;

use crate::grid::Grid;

/// Heights from the textures, `FUN_00540FD0` (`0x00540FD0`), step 9: for `x` outer, `z` inner the
/// definition for the texture number of the tile (`FUN_00540970`; -1 or unknown: skipped); all
/// four corner points get `HEIGHT`. So a grid point carries the height of the last visited
/// neighboring tile.
pub(crate) fn heights_from_textures(grid: &mut Grid, script: &RandomMapScript) {
    for x in 0..grid.dim_x {
        for z in 0..grid.dim_z {
            let number = grid.tile(x, z);
            let Some(texture) = script.textures.iter().find(|t| t.number == number) else {
                continue;
            };
            for (vx, vz) in [(x, z), (x, z + 1), (x + 1, z), (x + 1, z + 1)] {
                grid.set_height(vx, vz, texture.height);
            }
        }
    }
}

/// Upper limit of the smoothing passes that still change something. `SmoothFactor` is a raw
/// script `i32` (up to over a billion): `FUN_00540AD0` runs arbitrarily long in the original. Smoothing
/// converges to a fixed point (changes only where |h - avg| > 0.2, the grid becomes smoother);
/// a pass without a change makes all further ones have no effect. Real scripts need few passes; if
/// something still changes after this number, generation ends with an error message (never a hang;
/// quirks D-347).
const MAX_SMOOTH_PASSES: i32 = 10_000;

/// Smoothing, `FUN_00540AD0` (`0x00540AD0`), step 10: `times` times (only > 0), in place, `x` outer,
/// `z` inner over all grid points. Neighbors `d = (x−1, z)`, `c = (x+1, z)`, `b = (x, z+1)`,
/// `a = (x, z−1)` only within the grid, otherwise the height `h` itself. The x87 flow under PC24 is an
/// `f32` computation in fixed order: `avg = ((((a + b) + c) + d) + h) · 0.2f`; `avg` is taken
/// over if `|h − avg| > 0.2`, compared as `f64` (Z.6.9).
///
/// A pass without a change ends the loop (fixed point, the result stays bit-identical). If something
/// still changes after [`MAX_SMOOTH_PASSES`] passes and further ones would be required, that is an
/// error (D-347).
pub(crate) fn smooth(grid: &mut Grid, times: i32) -> anyhow::Result<()> {
    smooth_limited(grid, times, MAX_SMOOTH_PASSES)
}

fn smooth_limited(grid: &mut Grid, times: i32, max_passes: i32) -> anyhow::Result<()> {
    for pass in 0..times {
        if !smooth_pass(grid) {
            return Ok(());
        }
        if pass + 1 >= max_passes && pass + 1 < times {
            bail!(
                "random map: SmoothFactor {times}: the smoothing still changes heights after {max_passes} passes"
            );
        }
    }
    Ok(())
}

/// One pass of `FUN_00540AD0`; `true` if at least one height value was changed.
fn smooth_pass(grid: &mut Grid) -> bool {
    let (verts_x, verts_z) = (grid.dim_x + 1, grid.dim_z + 1);
    let mut changed = false;
    for x in 0..verts_x {
        for z in 0..verts_z {
            let h = grid.height(x, z);
            let d = if x > 0 { grid.height(x - 1, z) } else { h };
            let c = if x + 1 < verts_x {
                grid.height(x + 1, z)
            } else {
                h
            };
            let b = if z + 1 < verts_z {
                grid.height(x, z + 1)
            } else {
                h
            };
            let a = if z > 0 { grid.height(x, z - 1) } else { h };
            let avg = ((((a + b) + c) + d) + h) * 0.2f32;
            if ((h - avg).abs() as f64) > 0.2f64 {
                grid.set_height(x, z, avg);
                changed = true;
            }
        }
    }
    changed
}

/// Steepness field, `Terrain_ComputeSteepTileFlags` (`0x0055BE90`), step 11: grid points at
/// `(x·size, y, z·size)` (`Terrain_AllocateGrid` `0x0055C140`), per tile the two face normals,
/// `v = (a + b) / 2` (`Vector3_DivScalarInPlace`), `w = v · (0, −1, 0)`. The tile is steep (`1`)
/// if `w ≤ path_normal` or the comparison is unordered (`FCOMP` with `TEST AH,0x41`: NaN
/// on one side counts like `≤`), otherwise `0`. Index `dim_x·z + x`.
pub(crate) fn steep_flags(grid: &Grid, size: f32, path_normal: f32) -> Vec<u8> {
    let up = Vec3::new(0.0, -1.0, 0.0);
    face_normals(grid, size)
        .into_iter()
        .map(|(a, b)| {
            let w = ((a + b) / 2.0).dot(up);
            // Unordered (NaN on either side) sets C0|C3 like `<=`.
            let steep = w.is_nan() || path_normal.is_nan() || w <= path_normal;
            u8::from(steep)
        })
        .collect()
}

/// The two face normals per tile (`Terrain_ComputeTileFaceNormals` `0x0055AED0`, tile
/// `+0x0C` and `+0x18`) from the grid points `(x·size, height, z·size)` (`Terrain_AllocateGrid`
/// `0x0055C140`), index `dim_x·z + x`. Shared source of [`steep_flags`] and the
/// `PutOnFlat` check (`crate::checks::FaceNormals`).
pub(crate) fn face_normals(grid: &Grid, size: f32) -> Vec<(Vec3, Vec3)> {
    let point =
        |vx: usize, vz: usize| Vec3::new(vx as f32 * size, grid.height(vx, vz), vz as f32 * size);
    let mut normals = Vec::with_capacity(grid.dim_x * grid.dim_z);
    for z in 0..grid.dim_z {
        for x in 0..grid.dim_x {
            normals.push(tile_face_normals(
                point(x, z),
                point(x, z + 1),
                point(x + 1, z),
                point(x + 1, z + 1),
            ));
        }
    }
    normals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::tests::texture;

    fn script_with_heights(heights: &[f32]) -> RandomMapScript {
        let mut script = RandomMapScript::default();
        for (i, &height) in heights.iter().enumerate() {
            let mut def = texture(i as i32 + 1, &format!("T{i}"));
            def.height = height;
            script.textures.push(def);
        }
        script
    }

    #[test]
    fn vertex_takes_the_height_of_the_last_visited_tile() {
        // Texture 1 = 0, texture 2 = 600. Only tile (1, 1) is 0, the other three 600: the point (1, 1)
        // belongs to all four tiles, the last one (x outer, z inner) is (1, 1).
        let script = script_with_heights(&[0.0, 600.0]);
        let mut grid = Grid::new(2, 2);
        for (x, z, number) in [(0, 0, 2), (0, 1, 2), (1, 0, 2), (1, 1, 1)] {
            grid.set_tile(x, z, number);
        }
        heights_from_textures(&mut grid, &script);
        assert_eq!(grid.height(1, 1), 0.0);
        // Point (0, 0) belongs only to tile (0, 0); (2, 2) only to tile (1, 1).
        assert_eq!(grid.height(0, 0), 600.0);
        assert_eq!(grid.height(2, 2), 0.0);
        // (1, 0): tiles (0, 0) and (1, 0), both 600.
        assert_eq!(grid.height(1, 0), 600.0);
        // (2, 1): tiles (1, 0) = 600 and (1, 1) = 0; the later (1, 1) wins.
        assert_eq!(grid.height(2, 1), 0.0);
    }

    #[test]
    fn unknown_texture_number_is_skipped() {
        let script = script_with_heights(&[600.0]);
        let mut grid = Grid::new(2, 1);
        grid.set_tile(0, 0, 1);
        grid.set_tile(1, 0, -1);
        for vx in 0..=2 {
            for vz in 0..=1 {
                grid.set_height(vx, vz, 7.0);
            }
        }
        heights_from_textures(&mut grid, &script);
        assert_eq!(grid.height(0, 0), 600.0);
        assert_eq!(grid.height(1, 1), 600.0);
        // Tile (1, 0) without a definition does not touch its points; (1, *) also belongs to tile (0, 0).
        assert_eq!(grid.height(2, 0), 7.0);
        assert_eq!(grid.height(2, 1), 7.0);
    }

    #[test]
    fn smoothing_uses_the_fixed_f32_order() {
        // 3x3 grid points, a 600 spike in the middle, visiting order x outer, z inner.
        let mut grid = Grid::new(2, 2);
        grid.set_height(1, 1, 600.0);
        smooth(&mut grid, 1).unwrap();
        let k = 0.2f32;
        // (0, 0): all neighbors 0 -> stays.
        // (0, 1): a = (0, 0) = 0, b = (0, 2) = 0, c = (1, 1) = 600, d = h = 0, h = 0.
        let p01 = ((((0.0f32 + 0.0) + 600.0) + 0.0) + 0.0) * k;
        // (0, 2): a = (0, 1) = p01, b = h = 0, c = (1, 2) = 0, d = h = 0, h = 0.
        let p02 = ((((p01 + 0.0f32) + 0.0) + 0.0) + 0.0) * k;
        // (1, 0): a = h = 0, b = (1, 1) = 600, c = (2, 0) = 0, d = (0, 0) = 0, h = 0.
        let p10 = ((((0.0f32 + 600.0) + 0.0) + 0.0) + 0.0) * k;
        // (1, 1): a = (1, 0) = p10, b = (1, 2) = 0, c = (2, 1) = 0, d = (0, 1) = p01, h = 600.
        let p11 = ((((p10 + 0.0f32) + 0.0) + p01) + 600.0) * k;
        // (1, 2): a = (1, 1) = p11, b = h = 0, c = (2, 2) = 0, d = (0, 2) = p02, h = 0.
        let p12 = ((((p11 + 0.0f32) + 0.0) + p02) + 0.0) * k;
        // (2, 0): a = h = 0, b = (2, 1) = 0, c = h = 0, d = (1, 0) = p10, h = 0.
        let p20 = ((((0.0f32 + 0.0) + 0.0) + p10) + 0.0) * k;
        // (2, 1): a = (2, 0) = p20, b = (2, 2) = 0, c = h = 0, d = (1, 1) = p11, h = 0.
        let p21 = ((((p20 + 0.0f32) + 0.0) + p11) + 0.0) * k;
        // (2, 2): a = (2, 1) = p21, b = h = 0, c = h = 0, d = (1, 2) = p12, h = 0.
        let p22 = ((((p21 + 0.0f32) + 0.0) + p12) + 0.0) * k;
        // `avg` is taken over only at |h - avg| > 0.2; in this setup that holds everywhere except (0, 0).
        for (name, h, avg) in [
            ("p01", 0.0f32, p01),
            ("p02", 0.0, p02),
            ("p10", 0.0, p10),
            ("p11", 600.0, p11),
            ("p12", 0.0, p12),
            ("p20", 0.0, p20),
            ("p21", 0.0, p21),
            ("p22", 0.0, p22),
        ] {
            assert!(((h - avg).abs() as f64) > 0.2, "{name}");
        }
        let expected = [
            (0, 0, 0.0f32),
            (0, 1, p01),
            (0, 2, p02),
            (1, 0, p10),
            (1, 1, p11),
            (1, 2, p12),
            (2, 0, p20),
            (2, 1, p21),
            (2, 2, p22),
        ];
        for (x, z, value) in expected {
            assert_eq!(
                grid.height(x, z).to_bits(),
                value.to_bits(),
                "point ({x}, {z}): {} instead of {value}",
                grid.height(x, z)
            );
        }
    }

    #[test]
    fn small_differences_keep_the_height() {
        // |h - avg| = 0.08 <= 0.2: nothing changes, also with several passes.
        let mut grid = Grid::new(2, 2);
        grid.set_height(1, 1, 0.1);
        let before = grid.clone();
        smooth(&mut grid, 3).unwrap();
        assert_eq!(grid, before);
    }

    #[test]
    fn non_positive_smooth_count_does_nothing() {
        let mut grid = Grid::new(2, 2);
        grid.set_height(1, 1, 600.0);
        let before = grid.clone();
        smooth(&mut grid, 0).unwrap();
        smooth(&mut grid, -3).unwrap();
        assert_eq!(grid, before);
    }

    #[test]
    fn smoothing_stops_at_a_fixed_point() {
        let spike = || {
            let mut grid = Grid::new(4, 4);
            grid.set_height(2, 2, 600.0);
            grid
        };
        // After enough passes a further one changes nothing ...
        let mut settled = spike();
        smooth(&mut settled, 400).unwrap();
        assert!(!smooth_pass(&mut settled.clone()));
        // ... so a huge value ends immediately and yields the same grid.
        let mut huge = spike();
        smooth(&mut huge, 1_000_000).unwrap();
        assert_eq!(
            huge.heights.iter().map(|h| h.to_bits()).collect::<Vec<_>>(),
            settled
                .heights
                .iter()
                .map(|h| h.to_bits())
                .collect::<Vec<_>>()
        );
        // A grid without a change reports that without an error, also with i32::MAX.
        let mut flat = Grid::new(2, 2);
        smooth(&mut flat, i32::MAX).unwrap();
        assert_eq!(flat, Grid::new(2, 2));
    }

    #[test]
    fn smoothing_that_never_settles_is_an_error() {
        // An oscillating grid cannot be built: the averaging makes the grid smoother and ends in the
        // fixed point. The abort limit is therefore tested with a small `max_passes`: a grid that needs
        // more than one pass aborts with an error message at `times` > `max_passes`, at
        // `times` <= `max_passes` it runs exactly `times` times like the original.
        let spike = || {
            let mut grid = Grid::new(4, 4);
            grid.set_height(2, 2, 600.0);
            grid
        };
        let error = smooth_limited(&mut spike(), 1_000, 2).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("SmoothFactor 1000"), "{text}");
        assert!(text.contains("after 2 passes"), "{text}");
        let mut two = spike();
        smooth_limited(&mut two, 2, 2).unwrap();
        let mut reference = spike();
        smooth(&mut reference, 2).unwrap();
        assert_eq!(two, reference);
    }

    #[test]
    fn flat_tiles_are_not_steep() {
        let grid = Grid::new(3, 2);
        assert_eq!(steep_flags(&grid, 128.0, 0.8), vec![0; 6]);
        // w = 1.0 <= 1.0: steep.
        assert_eq!(steep_flags(&grid, 128.0, 1.0), vec![1; 6]);
        // Unordered comparison (NaN) counts like <=.
        assert_eq!(steep_flags(&grid, 128.0, f32::NAN), vec![1; 6]);
    }

    #[test]
    fn slope_tile_is_steep_below_path_normal() {
        // Ramp in x at 45 degrees (rise 128 per tile): w = cos 45 degrees ~ 0.7071 in tile (0, z);
        // tile (1, z) stays level (w = 1).
        let mut grid = Grid::new(2, 1);
        for vz in 0..=1 {
            grid.set_height(1, vz, 128.0);
            grid.set_height(2, vz, 128.0);
        }
        assert_eq!(steep_flags(&grid, 128.0, 0.7), vec![0, 0]);
        assert_eq!(steep_flags(&grid, 128.0, 0.8), vec![1, 0]);
        assert_eq!(steep_flags(&grid, 128.0, 0.0), vec![0, 0]);
    }

    #[test]
    fn steep_index_is_row_major() {
        // Ramp in x only over the column x = 1 (points (2, *) at 600): tiles (1, 0) and (1, 1)
        // are steep, index `2·z + x` = 1 and 3.
        let mut grid = Grid::new(2, 2);
        for vz in 0..=2 {
            grid.set_height(2, vz, 600.0);
        }
        assert_eq!(steep_flags(&grid, 128.0, 0.9), vec![0, 1, 0, 1]);
    }
}
