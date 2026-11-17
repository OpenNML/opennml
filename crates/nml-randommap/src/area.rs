//! Areas of the generator (`create_LandArea`/`create_PlayerAreas`/`create_TerrainArea`, `0x374`
//! bytes, `FUN_005406D0`): the raw record of the reader with borders in tiles (`FUN_0053FBF0`),
//! scaled quantities (`FUN_00540810`) and the run-time fields. See re/random-map Z.5.5.

use nml_formats::rnd::AreaDef;
use nml_sim::fx::{Fx, fixed_div, fixed_mul};

/// Entry of the edge list (`+0x358`, `FUN_00541190`): tile, direction and tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EdgeEntry {
    pub(crate) x: i32,
    pub(crate) z: i32,
    pub(crate) dir: i32,
    pub(crate) tries: i32,
}

/// A prepared area. `def` carries the scaled quantities (`min_size`, `num_tiles`,
/// `num_clumps`); the percentage borders stay raw in it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Area {
    pub(crate) def: AreaDef,
    /// `+0x300`, `+0x308`, `+0x30C`, `+0x304`: frame in tiles.
    pub(crate) x_min: i32,
    pub(crate) x_max: i32,
    pub(crate) z_min: i32,
    pub(crate) z_max: i32,
    /// `+0x34C`/`+0x350`: center (set by the terrain steps).
    pub(crate) cx: i32,
    pub(crate) cz: i32,
    /// `+0x340`: grown tiles.
    pub(crate) grown: i32,
    /// `+0x358`: edge list.
    pub(crate) edges: Vec<EdgeEntry>,
    /// `+0x354`: pointer into the edge list.
    pub(crate) cursor: Option<usize>,
}

/// End of an area block at `}` (end of `FUN_00543CA0`): borders, then quantities.
/// `players` is `G+0x120`, `scale` the map factor `G+0x268`.
pub(crate) fn finish_area(def: &AreaDef, dim_x: i32, dim_z: i32, players: i32, scale: Fx) -> Area {
    let mut area = Area {
        def: def.clone(),
        x_min: 0,
        x_max: dim_x,
        z_min: 0,
        z_max: dim_z,
        cx: 0,
        cz: 0,
        grown: 0,
        edges: Vec::new(),
        cursor: None,
    };
    apply_borders(&mut area, dim_x, dim_z);
    apply_quantities(&mut area, players, scale);
    area
}

/// `trunc(DIM · P · 0.01)` like `FILD DIM; FIMUL P; FMUL double 0.01; _ftol` (`FUN_0053FBF0`). For
/// `|DIM · P| < 2²⁴` the x87 computes exactly (even with 24 bits of precision), and because `0.01` as a double is
/// minimally larger, the result equals integer division toward 0. Above that, PC24 and integer round
/// differently (only absurd borders); outside `i32`, `_ftol` returns `0x80000000`.
fn border_cells(dim: i32, percent: i32) -> i32 {
    i32::try_from(i64::from(dim) * i64::from(percent) / 100).unwrap_or(i32::MIN)
}

/// Borders `FUN_0053FBF0` (`0x0053FBF0`): `L + R` and `T + B` as a signed 32-bit sum against
/// 100 (`ADD; CMP 0x64; JGE`); both smaller -> `x_min = DIM_X·L/100`, `z_min = DIM_Z·B/100`,
/// `x_max = DIM_X − DIM_X·R/100`, `z_max = DIM_Z − DIM_Z·T/100`, otherwise the whole map.
fn apply_borders(area: &mut Area, dim_x: i32, dim_z: i32) {
    let d = &area.def;
    if d.left.wrapping_add(d.right) >= 100 || d.top.wrapping_add(d.bottom) >= 100 {
        return;
    }
    area.x_min = border_cells(dim_x, d.left);
    area.z_min = border_cells(dim_z, d.bottom);
    area.x_max = dim_x.wrapping_sub(border_cells(dim_x, d.right));
    area.z_max = dim_z.wrapping_sub(border_cells(dim_z, d.top));
}

/// `Fixed_ToInt(Fixed_Mul(S, Fixed_FromInt(n)))` (argument order as at `0x005408BF`).
pub(crate) fn scaled(scale: Fx, n: i32) -> i32 {
    Fx(fixed_mul(scale.0, Fx::from_int(n).0)).to_int()
}

/// Quantities `FUN_00540810` (`0x00540810`, D-325), in this order:
/// 1. `ScaleToPlayers`: `minSize`, `numClumps` times `n`, with `Percent == 0` also `numTiles`.
/// 2. `Percent > 0`: `numTiles` from the frame area (`FUN_00540100`).
/// 3. `ScaleToMapSize`: `minSize`, `numClumps`, with `Percent == 0` `numTiles` times the map factor;
///    **otherwise** `ScaleClumpsToMapSize`: only `numClumps`.
fn apply_quantities(area: &mut Area, players: i32, scale: Fx) {
    let (x_min, x_max, z_min, z_max) = (area.x_min, area.x_max, area.z_min, area.z_max);
    let d = &mut area.def;
    if d.scale_to_players {
        d.min_size = d.min_size.wrapping_mul(players);
        d.num_clumps = d.num_clumps.wrapping_mul(players);
        if d.percent == 0 {
            d.num_tiles = d.num_tiles.wrapping_mul(players);
        }
    }
    if d.percent > 0 {
        d.num_tiles = percent_tiles(x_min, x_max, z_min, z_max, d.percent);
    }
    if d.scale_to_map_size {
        d.min_size = scaled(scale, d.min_size);
        d.num_clumps = scaled(scale, d.num_clumps);
        if d.percent == 0 {
            d.num_tiles = scaled(scale, d.num_tiles);
        }
    } else if d.scale_clumps_to_map_size {
        d.num_clumps = scaled(scale, d.num_clumps);
    }
}

/// `FUN_00540100` (`0x00540100`): `Fixed_ToInt(Fixed_Mul(Fixed_FromInt((z_max − z_min)·(x_max −
/// x_min)), Fixed_Div(Fixed_FromInt(P), Fixed_FromInt(100))))` with the loss of `Fixed_Div`
/// (64-tile map, borders 8, 60% -> 1748 instead of 1749.6). The divisor is the constant 100 * 4096.
fn percent_tiles(x_min: i32, x_max: i32, z_min: i32, z_max: i32, percent: i32) -> i32 {
    let tiles = z_max
        .wrapping_sub(z_min)
        .wrapping_mul(x_max.wrapping_sub(x_min));
    let ratio = fixed_div(Fx::from_int(percent).0, Fx::from_int(100).0);
    Fx(fixed_mul(Fx::from_int(tiles).0, ratio)).to_int()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::scale_factor;

    fn bordered(left: i32, right: i32, top: i32, bottom: i32) -> AreaDef {
        AreaDef {
            left,
            right,
            top,
            bottom,
            ..AreaDef::default()
        }
    }

    fn frame(area: &Area) -> (i32, i32, i32, i32) {
        (area.x_min, area.x_max, area.z_min, area.z_max)
    }

    #[test]
    fn percent_loses_precision_like_fixed_div() {
        let def = AreaDef {
            percent: 60,
            ..bordered(8, 8, 8, 8)
        };
        let area = finish_area(&def, 64, 64, 2, scale_factor(0));
        assert_eq!(frame(&area), (5, 59, 5, 59));
        // 54 * 54 * 0.6 = 1749.6; `Fixed_Div(60, 100)` = 2456/4096 costs almost two tiles.
        assert_eq!(area.def.num_tiles, 1748);
    }

    #[test]
    fn borders_follow_integer_percent() {
        let s = scale_factor(1);
        let area = finish_area(&bordered(10, 20, 30, 5), 128, 128, 2, s);
        // x: 12.8 -> 12, 128 - 25.6 -> 103; z: 6.4 -> 6, `TopBorder` sets z_max: 128 - 38.4 -> 90.
        assert_eq!(frame(&area), (12, 103, 6, 90));
        // Separate edge lengths in x and z.
        let area = finish_area(&bordered(50, 0, 0, 50), 64, 128, 2, s);
        assert_eq!(frame(&area), (32, 64, 64, 128));
        // `L + R ≥ 100` or `T + B ≥ 100` -> whole map.
        for def in [bordered(60, 40, 10, 10), bordered(10, 10, 99, 1)] {
            assert_eq!(frame(&finish_area(&def, 64, 64, 2, s)), (0, 64, 0, 64));
        }
        // Negative borders truncate toward 0: 64 * -7 / 100 = -4.48 -> -4.
        let area = finish_area(&bordered(-7, -50, 0, 0), 64, 64, 2, s);
        assert_eq!(frame(&area), (-4, 96, 0, 64));
        // The sum overflows in 32 bits (`ADD; CMP 0x64`), outside `i32` `_ftol` returns
        // `0x80000000`.
        let area = finish_area(&bordered(i32::MAX, 1, 0, 0), 256, 64, 2, s);
        assert_eq!(frame(&area), (i32::MIN, 254, 0, 64));
        let area = finish_area(&bordered(1, i32::MAX, 0, 1), 256, 64, 2, s);
        assert_eq!(frame(&area), (2, 256i32.wrapping_sub(i32::MIN), 0, 64));
        // Without overflow a huge sum stays >= 100: whole map.
        let area = finish_area(&bordered(0, i32::MAX, 0, 1), 256, 64, 2, s);
        assert_eq!(frame(&area), (0, 256, 0, 64));
    }

    #[test]
    fn quantities_apply_players_then_percent_then_map_size() {
        let base = AreaDef {
            min_size: 3,
            num_tiles: 10,
            num_clumps: 2,
            scale_to_players: true,
            scale_to_map_size: true,
            scale_clumps_to_map_size: true,
            ..AreaDef::default()
        };
        // Players, then map factor 2; `ScaleClumpsToMapSize` does not apply alongside `ScaleToMapSize`.
        let area = finish_area(&base, 128, 128, 4, scale_factor(1));
        let d = &area.def;
        assert_eq!((d.min_size, d.num_tiles, d.num_clumps), (24, 80, 16));
        // Without `ScaleToMapSize`, `ScaleClumpsToMapSize` scales only `numClumps`.
        let def = AreaDef {
            scale_to_map_size: false,
            ..base.clone()
        };
        let d = finish_area(&def, 128, 128, 4, scale_factor(1)).def;
        assert_eq!((d.min_size, d.num_tiles, d.num_clumps), (12, 40, 16));
        // `Percent > 0`: `numTiles` only from the frame area, neither with players nor with the size.
        let def = AreaDef {
            percent: 50,
            ..base.clone()
        };
        let d = finish_area(&def, 64, 64, 4, scale_factor(1)).def;
        assert_eq!((d.min_size, d.num_tiles, d.num_clumps), (24, 2048, 16));
        // Negative `Percent`: `numTiles` stays raw (checks `== 0` and `> 0`).
        let def = AreaDef {
            percent: -5,
            ..base.clone()
        };
        let d = finish_area(&def, 64, 64, 4, scale_factor(1)).def;
        assert_eq!((d.min_size, d.num_tiles, d.num_clumps), (24, 10, 16));
        // Without flags the quantities stay raw.
        let def = AreaDef {
            min_size: 3,
            num_tiles: 10,
            ..AreaDef::default()
        };
        let d = finish_area(&def, 64, 64, 4, scale_factor(2)).def;
        assert_eq!((d.min_size, d.num_tiles, d.num_clumps), (3, 10, 1));
    }

    #[test]
    fn extreme_quantities_never_panic() {
        for value in [i32::MIN, -1, 0x8_0000, i32::MAX] {
            let def = AreaDef {
                min_size: value,
                num_tiles: value,
                num_clumps: value,
                percent: value,
                left: value,
                right: value,
                top: value,
                bottom: value,
                scale_to_players: true,
                scale_to_map_size: true,
                ..AreaDef::default()
            };
            for index in 0..3 {
                finish_area(&def, 256, 256, 8, scale_factor(index));
            }
        }
    }
}
