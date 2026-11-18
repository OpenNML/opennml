//! Paint routines of the generator (re/random-map Z.6.2): texture number for a name `FUN_00541710`,
//! paint mask `FUN_00541A60` and disc `FUN_005418A0`. What is painted is the texture number
//! (`Tile+0x08`); the GUID is set only by step 14.

use anyhow::bail;
use opennml_formats::rnd::RandomMapScript;
use opennml_sim::fx::{Fx, fixed_div, fixed_mul};
use opennml_sim::math::{cos_deg, sin_deg};

use crate::grid::Grid;

/// `FUN_00541710` (`0x00541710`): number of the **first** `#Texture` with the same name (`_mbscmp`,
/// case matters), otherwise -1.
pub(crate) fn texture_number(script: &RandomMapScript, name: &str) -> i32 {
    script
        .textures
        .iter()
        .find(|t| t.name == name)
        .map_or(-1, |t| t.number)
}

/// `FUN_00541A60` (`0x00541A60`): every tile with mask not in {0, `0xFF`} gets `number` (also -1 for
/// an unknown name). No randomness.
pub(crate) fn paint_mask(grid: &mut Grid, mask: &[u8], number: i32) {
    for (tile, &m) in grid.tiles.iter_mut().zip(mask) {
        if m != 0 && m != 0xFF {
            *tile = number;
        }
    }
}

/// `ToInt(Fixed_Mul(FromInt(r), t)) + c` - first `FromInt(r)`, then the table value (`0x005418A0`).
fn scaled_offset(r: i32, t: i32, c: i32) -> i32 {
    Fx(fixed_mul(Fx::from_int(r).0, t)).to_int().wrapping_add(c)
}

/// Shared form of `FUN_005418A0` and `FUN_00540180`: for `k = 0 … 180` (181 rows)
/// `xa = r·sin(k + 180°) + cx`, `xb = r·sin k + cx`, `zz = r·cos k + cz` in fixed point, all three
/// clamped to the map, then `f(x, zz)` for `x = xa … xb`. Large radii skip rows (the table values
/// jump), columns and rows outside land on the edge tile. No randomness.
fn disc_cells(
    grid: &mut Grid,
    cx: i32,
    cz: i32,
    r: i32,
    mut f: impl FnMut(&mut Grid, usize, usize),
) {
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    let clamp = |v: i32, dim: i32| v.max(0).min(dim - 1);
    for k in 0..=180 {
        let xa = scaled_offset(r, sin_deg(Fx::from_int(k + 180).0), cx);
        let xb = scaled_offset(r, sin_deg(Fx::from_int(k).0), cx);
        let zz = scaled_offset(r, cos_deg(Fx::from_int(k).0), cz);
        let (xa, xb, zz) = (clamp(xa, dim_x), clamp(xb, dim_x), clamp(zz, dim_z));
        for x in xa..=xb {
            f(grid, x as usize, zz as usize);
        }
    }
}

/// Disc `FUN_005418A0` (`0x005418A0`): paints the shape of [`disc_cells`] with `number`.
pub(crate) fn disc(grid: &mut Grid, cx: i32, cz: i32, r: i32, number: i32) {
    disc_cells(grid, cx, cz, r, |grid, x, z| grid.set_tile(x, z, number));
}

/// Conditional disc `FUN_00540180` (`0x00540180`): the same shape as [`disc`], but only tiles with
/// the number `old` get `new` (`old` = -1 hits the unpainted tiles).
pub(crate) fn disc_if(grid: &mut Grid, cx: i32, cz: i32, r: i32, new: i32, old: i32) {
    disc_cells(grid, cx, cz, r, |grid, x, z| {
        if grid.tile(x, z) == old {
            grid.set_tile(x, z, new);
        }
    });
}

/// `BaseTexture` `FUN_00541B20` (`0x00541B20`), step 6: every tile with number -1 gets `number`
/// (the number of `BaseTexture`, -1 for an unknown name). No randomness.
pub(crate) fn base_texture(grid: &mut Grid, number: i32) {
    for tile in grid.tiles.iter_mut().filter(|tile| **tile == -1) {
        *tile = number;
    }
}

/// Upper limit of the steps of a connection segment. Real maps are far below it (at most about
/// twice the edge length); above it a center would lie far outside the map, and the
/// original would run practically forever (never a hang).
const MAX_CONNECT_STEPS: i32 = 1 << 16;

/// `connect_Players` `FUN_00543800` (`0x00543800`), step 7: per block in script order
/// `new = number(Texture)`, `old = number(PutOnto)` (-1 if empty or unknown); for `i = 0 … n−1`
/// with `n` = length of `centers` and `j = (i + 1) % n` (ring, D-333) a line from center `i` to
/// center `j` in fixed point: `N = max(|dx|, |dz|)`, step `Div(FromInt(d), FromInt(N))`, `N + 1`
/// points, at each a conditional disc `radius`. No randomness.
///
/// `centers` are the centers of **all** entries of the player area list in list order: the
/// original runs over `ESI+0x3c` (list) with the count `ESI+0x48` (its length, `0x00543880`),
/// not over the number of players `ESI+0x120`; with several `create_PlayerAreas` blocks all
/// copies together form one ring. With two entries the same segment is painted twice.
///
/// Two equal centers (also a single entry, `j = i`) divided `Fixed_Div(0, 0)` by zero in the original;
/// the reimplementation reports that as an error, likewise a segment above [`MAX_CONNECT_STEPS`]
/// (also when `N` overflows, `|i32::MIN|`) (D-333/D-335). Below that, `FromInt(N)` is never zero.
pub(crate) fn connect_players(
    grid: &mut Grid,
    script: &RandomMapScript,
    centers: &[(i32, i32)],
) -> anyhow::Result<()> {
    let n = centers.len();
    for connect in &script.connections {
        let new = texture_number(script, &connect.texture);
        let old = texture_number(script, &connect.put_onto);
        for i in 0..n {
            let (ax, az) = centers[i];
            let (bx, bz) = centers[(i + 1) % n];
            let dx = bx.wrapping_sub(ax);
            let dz = bz.wrapping_sub(az);
            let steps = dx.wrapping_abs().max(dz.wrapping_abs());
            if steps == 0 {
                bail!(
                    "random map: connect_Players from ({ax}, {az}) to ({bx}, {bz}) has \
                     length 0 (equal player centers) and divides by zero"
                );
            }
            if !(1..=MAX_CONNECT_STEPS).contains(&steps) {
                bail!(
                    "random map: connect_Players from ({ax}, {az}) to ({bx}, {bz}) is too \
                     long (player centers far outside the map)"
                );
            }
            let divisor = Fx::from_int(steps).0;
            let (sx, sz) = (
                fixed_div(Fx::from_int(dx).0, divisor),
                fixed_div(Fx::from_int(dz).0, divisor),
            );
            let (mut px, mut pz) = (Fx::from_int(ax), Fx::from_int(az));
            for _ in 0..=steps {
                disc_if(grid, px.to_int(), pz.to_int(), connect.radius, new, old);
                px = Fx(px.0.wrapping_add(sx));
                pz = Fx(pz.0.wrapping_add(sz));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use opennml_formats::rnd::{ConnectDef, TextureDef};

    pub(crate) fn texture(number: i32, name: &str) -> TextureDef {
        TextureDef {
            number,
            name: name.to_string(),
            height: 0.0,
            guids: Vec::new(),
        }
    }

    /// Tiles as a picture, row `z` by row (`.` = -1, otherwise digit).
    pub(crate) fn tile_picture(grid: &Grid) -> Vec<String> {
        grid.tiles
            .chunks(grid.dim_x)
            .map(|row| {
                row.iter()
                    .map(|&v| match v {
                        -1 => '.',
                        v => char::from_digit(v.rem_euclid(10) as u32, 10).unwrap(),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn texture_number_takes_the_first_equal_name() {
        let script = RandomMapScript {
            textures: vec![texture(1, "GRASS"), texture(2, "SAND"), texture(3, "GRASS")],
            ..RandomMapScript::default()
        };
        assert_eq!(texture_number(&script, "GRASS"), 1);
        assert_eq!(texture_number(&script, "SAND"), 2);
        // Case matters, unknown and empty -> -1.
        assert_eq!(texture_number(&script, "grass"), -1);
        assert_eq!(texture_number(&script, ""), -1);
    }

    #[test]
    fn paint_mask_skips_free_and_border_cells() {
        let mut grid = Grid::new(4, 1);
        paint_mask(&mut grid, &[0, 0xFF, 1, 7], 5);
        assert_eq!(grid.tiles, [-1, -1, 5, 5]);
        // An unknown texture paints -1.
        paint_mask(&mut grid, &[0, 0, 0, 2], -1);
        assert_eq!(grid.tiles, [-1, -1, 5, -1]);
    }

    #[test]
    fn disc_paints_181_rows_clamped_to_the_map() {
        // r = 3 around (0, 0): columns left of the map clamp to x = 0, rows with z < 0 to z = 0.
        // The shape follows the fixed-point tables (rounded down), not a round circle.
        let mut grid = Grid::new(8, 8);
        disc(&mut grid, 0, 0, 3, 4);
        assert_eq!(
            tile_picture(&grid),
            [
                "4444....", "444.....", "444.....", "4.......", "........", "........", "........",
                "........",
            ]
        );
        // Completely outside: clamped to the edge tile, no panic.
        let mut grid = Grid::new(4, 4);
        disc(&mut grid, 100, -100, 2, 1);
        assert_eq!(tile_picture(&grid), ["...1", "....", "....", "...."]);
        // Large radius: the rows jump in steps of `r · 71/4096`, rows in between stay empty (row 1
        // here), like the table calculation of the original.
        let mut grid = Grid::new(4, 4);
        disc(&mut grid, 2, 2, 600, 3);
        assert_eq!(tile_picture(&grid), ["3333", "....", "3333", "3333"]);
        // `FromInt(i32::MAX)` overflows (-4096 = -1 tile): no overflow panic.
        let mut grid = Grid::new(4, 4);
        disc(&mut grid, 2, 2, i32::MAX, 3);
        assert_eq!(tile_picture(&grid), ["....", "..3.", "....", "..3."]);
    }

    #[test]
    fn base_texture_fills_only_unset_tiles() {
        let mut grid = Grid::new(4, 1);
        grid.tiles = vec![-1, 3, -1, 0];
        base_texture(&mut grid, 7);
        assert_eq!(grid.tiles, [7, 3, 7, 0]);
        // Unknown name: -1 stays -1.
        let mut grid = Grid::new(2, 2);
        base_texture(&mut grid, -1);
        assert_eq!(grid.tiles, [-1; 4]);
    }

    #[test]
    fn disc_if_replaces_only_old() {
        // The same shape as `disc` (r = 3 around (0, 0)), but only tiles with `old`.
        let mut plain = Grid::new(8, 8);
        disc(&mut plain, 0, 0, 3, 4);
        let mut grid = Grid::new(8, 8);
        grid.tiles.fill(1);
        disc_if(&mut grid, 0, 0, 3, 4, 1);
        let shape = |g: &Grid| g.tiles.iter().map(|&v| v == 4).collect::<Vec<_>>();
        assert_eq!(shape(&grid), shape(&plain));
        assert_eq!(
            grid.tiles.iter().filter(|&&v| v == 4).count(),
            4 + 3 + 3 + 1
        );
        // Tiles with another number stay, also -1.
        let mut grid = Grid::new(8, 8);
        grid.tiles.fill(2);
        grid.tiles[0] = -1;
        disc_if(&mut grid, 0, 0, 3, 4, 1);
        assert_eq!(grid.tiles[0], -1);
        assert_eq!(grid.tiles.iter().filter(|&&v| v == 4).count(), 0);
        // `old` = -1 hits the unpainted tiles.
        disc_if(&mut grid, 0, 0, 3, 4, -1);
        assert_eq!(grid.tiles[0], 4);
        assert_eq!(grid.tiles.iter().filter(|&&v| v == 4).count(), 1);
    }

    /// Script with `WATER` (1), `GRASS` (2) and a connection `GRASS` onto `WATER`, radius 0.
    fn connect_script(radius: i32) -> RandomMapScript {
        RandomMapScript {
            textures: vec![texture(1, "WATER"), texture(2, "GRASS")],
            connections: vec![ConnectDef {
                texture: "GRASS".into(),
                put_onto: "WATER".into(),
                radius,
            }],
            ..RandomMapScript::default()
        }
    }

    fn water(dim: usize) -> Grid {
        let mut grid = Grid::new(dim, dim);
        grid.tiles.fill(1);
        grid
    }

    // QUIRK D-333: kept
    #[test]
    fn connection_is_a_ring() {
        // Three centers: (1, 1) -> (6, 1) horizontal, (6, 1) -> (1, 6) diagonal, (1, 6) -> (1, 1)
        // vertical, the last closes to the first (D-333). Radius 0 paints exactly one tile per point.
        let mut grid = water(8);
        connect_players(&mut grid, &connect_script(0), &[(1, 1), (6, 1), (1, 6)]).unwrap();
        assert_eq!(
            tile_picture(&grid),
            [
                "11111111", "12222221", "12111211", "12112111", "12121111", "12211111", "12111111",
                "11111111",
            ]
        );
        // Without a third player it stays at the horizontal segment.
        let mut open = water(8);
        connect_players(&mut open, &connect_script(0), &[(1, 1), (6, 1)]).unwrap();
        assert_ne!(open.tiles, grid.tiles);
        assert_eq!(open.tiles.iter().filter(|&&v| v == 2).count(), 6);
    }

    /// Points of the segment `a → b` after `FUN_00543800`, by hand with the same fixed-point rules.
    fn segment(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
        let (dx, dz) = (b.0 - a.0, b.1 - a.1);
        let n = dx.abs().max(dz.abs());
        let div = |d: i32| fixed_div(Fx::from_int(d).0, Fx::from_int(n).0);
        let (sx, sz) = (div(dx), div(dz));
        (0..=n)
            .map(|k| {
                (
                    Fx(Fx::from_int(a.0).0 + k * sx).to_int(),
                    Fx(Fx::from_int(a.1).0 + k * sz).to_int(),
                )
            })
            .collect()
    }

    // QUIRK D-333: kept
    #[test]
    fn two_players_paint_the_same_line_twice() {
        // Horizontal: A -> B and B -> A are the same segment, the result equals one segment.
        let mut grid = water(8);
        connect_players(&mut grid, &connect_script(0), &[(1, 3), (6, 3)]).unwrap();
        let mut once = water(8);
        for (x, z) in segment((1, 3), (6, 3)) {
            disc_if(&mut once, x, z, 0, 2, 1);
        }
        assert_eq!(grid.tiles, once.tiles);
        assert_eq!(grid.tiles.iter().filter(|&&v| v == 2).count(), 6);
        // Diagonal: exactly two segments, A -> B and B -> A (the downward step `3/5` rounds down, the
        // reverse direction does not hit the same tiles), no third.
        let mut grid = water(8);
        connect_players(&mut grid, &connect_script(1), &[(1, 1), (6, 4)]).unwrap();
        let mut by_hand = water(8);
        for (a, b) in [((1, 1), (6, 4)), ((6, 4), (1, 1))] {
            for (x, z) in segment(a, b) {
                disc_if(&mut by_hand, x, z, 1, 2, 1);
            }
        }
        assert_eq!(grid.tiles, by_hand.tiles);
    }

    #[test]
    fn each_connection_block_paints_its_own_ring() {
        // The second block (ROAD onto GRASS) sees the result of the first.
        let mut script = connect_script(0);
        script.textures.push(texture(3, "ROAD"));
        script.connections.push(ConnectDef {
            texture: "ROAD".into(),
            put_onto: "GRASS".into(),
            radius: 0,
        });
        let mut grid = water(8);
        connect_players(&mut grid, &script, &[(1, 3), (6, 3)]).unwrap();
        assert_eq!(grid.tiles.iter().filter(|&&v| v == 3).count(), 6);
        assert_eq!(grid.tiles.iter().filter(|&&v| v == 2).count(), 0);
        // Without a connection block or without centers everything stays as before.
        let mut grid = water(8);
        connect_players(&mut grid, &RandomMapScript::default(), &[(1, 1), (6, 4)]).unwrap();
        connect_players(&mut grid, &script, &[]).unwrap();
        assert_eq!(grid.tiles, water(8).tiles);
        // Missing texture: `new` = -1; unknown `PutOnto` = -1 hits unpainted tiles.
        let script = RandomMapScript {
            connections: vec![ConnectDef {
                texture: "?".into(),
                put_onto: String::new(),
                radius: 0,
            }],
            ..RandomMapScript::default()
        };
        let mut grid = Grid::new(8, 8);
        grid.tiles[8 * 3 + 2] = 5;
        connect_players(&mut grid, &script, &[(1, 3), (4, 3)]).unwrap();
        assert_eq!(grid.tiles[8 * 3 + 2], 5);
        assert_eq!(grid.tiles.iter().filter(|&&v| v == -1).count(), 63);
    }

    // QUIRK D-333: kept
    // QUIRK D-335: deviation
    #[test]
    fn identical_centers_and_huge_distances_are_errors() {
        let script = connect_script(0);
        let mut grid = water(8);
        // Two equal centers and a single entry (`j = i`): `Fixed_Div(0, 0)` in the original.
        // Likewise `dx = i32::MIN, dz = 0`: `|i32::MIN|` stays negative, the maximum is 0.
        for centers in [
            &[(2, 2), (2, 2)][..],
            &[(2, 2)][..],
            &[(0, 0), (5, 5), (5, 5)][..],
            &[(0, 0), (i32::MIN, 0)][..],
        ] {
            let error = connect_players(&mut grid, &script, centers).unwrap_err();
            assert!(format!("{error:#}").contains("by zero"), "{error:#}");
        }
        // Far outside the map and `i32` overflow (both distances `i32::MIN`): error instead of
        // panic or hang.
        for centers in [
            &[(0, 0), (1 << 20, 0)][..],
            &[(0, 0), (i32::MIN, i32::MIN)][..],
        ] {
            let error = connect_players(&mut grid, &script, centers).unwrap_err();
            assert!(format!("{error:#}").contains("too long"), "{error:#}");
        }
        // Overflowing distances whose maximum turns out small run without a panic, as in the original.
        connect_players(&mut grid, &script, &[(i32::MIN, i32::MIN), (i32::MAX, 0)]).unwrap();
        // Centers just outside: the points clamp to the edge tiles.
        connect_players(&mut grid, &script, &[(-3, -3), (-1, -3)]).unwrap();
        assert_eq!(grid.tile(0, 0), 2);
    }
}
