//! Growth of the areas (re/random-map Z.6.1): rough border `FUN_0053FE30`, edge list of the land and
//! player areas `FUN_00541190`, clumps of the terrain areas `FUN_00545A40` and the distance test
//! `FUN_00540020`. Masks: 0 free, `0xFF` border, otherwise ID low byte or 1 (clumps).
//!
//! Tiles `(x, z)` lie in mask and ID grid at `DIM_X·z + x` (32-bit pointer arithmetic in the
//! original). The reimplementation computes the position in `i64`: within the mask it applies as in the
//! original (also for `x` outside `0..DIM_X`, then in the neighboring row), outside it the original ran
//! into foreign memory - here an error message (D-335).

use anyhow::bail;

use crate::area::{Area, EdgeEntry};
use crate::grid::Grid;
use crate::rng::GenRand;

/// Position `DIM_X·z + x`, if it lies in `0..len`.
fn cell(x: i32, z: i32, dim_x: i32, len: usize) -> Option<usize> {
    let index = i64::from(dim_x) * i64::from(z) + i64::from(x);
    usize::try_from(index).ok().filter(|&i| i < len)
}

/// `rand() % (value + 1)` with the divisor from the script; divisor 0 divided by zero in the original
/// (`IDIV`), here an error message. `rand() ≥ 0`, so never `i32::MIN % −1`.
pub(crate) fn roll(rng: &mut GenRand, value: i32, key: &str) -> anyhow::Result<i32> {
    let divisor = value.wrapping_add(1);
    let draw = rng.rand();
    match draw.checked_rem(divisor) {
        Some(r) => Ok(r),
        None => bail!("random map: {key} {value} divides by zero"),
    }
}

/// Rough border `FUN_0053FE30` (`0x0053FE30`): four loops in fixed order, one
/// `r = rand() % (BorderRoughness + 1)` per row or column (also at 0), so `2·DIM_Z + 2·DIM_X` draws.
/// With `x_min + r > DIM_X` the first loop writes into the following row (D-335).
pub(crate) fn rough_border(
    mask: &mut [u8],
    area: &Area,
    dim_x: i32,
    dim_z: i32,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let roughness = area.def.border_roughness;
    let fill = |mask: &mut [u8], x: i32, z: i32| -> anyhow::Result<()> {
        let Some(i) = cell(x, z, dim_x, mask.len()) else {
            bail!(
                "random map: rough border writes outside the map (tile {x}, {z}; \
                 borders {}/{}/{}/{}, BorderRoughness {roughness})",
                area.x_min,
                area.x_max,
                area.z_min,
                area.z_max
            );
        };
        mask[i] = 0xFF;
        Ok(())
    };
    for z in 0..dim_z {
        let w = roll(rng, roughness, "BorderRoughness")?.wrapping_add(area.x_min);
        for x in 0..w {
            fill(mask, x, z)?;
        }
    }
    for z in 0..dim_z {
        let s = area
            .x_max
            .wrapping_sub(roll(rng, roughness, "BorderRoughness")?);
        for x in s..dim_x {
            fill(mask, x, z)?;
        }
    }
    for x in 0..dim_x {
        let s = area
            .z_max
            .wrapping_sub(roll(rng, roughness, "BorderRoughness")?);
        for z in s..dim_z {
            fill(mask, x, z)?;
        }
    }
    for x in 0..dim_x {
        let h = roll(rng, roughness, "BorderRoughness")?.wrapping_add(area.z_min);
        for z in 0..h {
            fill(mask, x, z)?;
        }
    }
    Ok(())
}

/// Distance test `FUN_00540020` (`0x00540020`), always exactly one `rand()`:
/// `d = max(0, AvoidOther − rand() % (AvoidRoughness + 1))`; any tile in
/// `[x−d, x+d] × [z−d, z+d]` in the map with ID != 0 and != own ID -> no. The original computes the
/// bounds in 32 bits: if `x + d` overflows, the loop does not run and the tile counts as free. The
/// reimplementation clips the range to the map (same result, no hang for large `d`).
pub(crate) fn avoid_ok(
    x: i32,
    z: i32,
    area: &Area,
    grid: &Grid,
    rng: &mut GenRand,
) -> anyhow::Result<bool> {
    let r = roll(rng, area.def.avoid_roughness, "AvoidRoughness")?;
    let d = area.def.avoid_other.wrapping_sub(r).max(0);
    let (x_lo, x_hi) = (x.wrapping_sub(d), x.wrapping_add(d));
    let (z_lo, z_hi) = (z.wrapping_sub(d), z.wrapping_add(d));
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    for xx in x_lo.max(0)..=x_hi.min(dim_x - 1) {
        for zz in z_lo.max(0)..=z_hi.min(dim_z - 1) {
            let id = grid.id(xx as usize, zz as usize);
            if id != 0 && id != area.def.id {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Step in direction `dir` (`0: x−1`, `1: x+1`, `2: z+1`, `3: z−1`, 32 bits); a step out of the
/// map stays on the starting tile (`FUN_00541190` `0x005411E0`).
fn step(x: i32, z: i32, dir: i32, dim_x: i32, dim_z: i32) -> (i32, i32) {
    match dir {
        0 if x.wrapping_sub(1) >= 0 => (x.wrapping_sub(1), z),
        1 if x.wrapping_add(1) < dim_x => (x.wrapping_add(1), z),
        2 if z.wrapping_add(1) < dim_z => (x, z.wrapping_add(1)),
        3 if z.wrapping_sub(1) >= 0 => (x, z.wrapping_sub(1)),
        _ => (x, z),
    }
}

fn outside(x: i32, z: i32) -> anyhow::Error {
    anyhow::anyhow!("random map: area growth reaches outside the map (tile {x}, {z})")
}

/// A visit of the entry under the pointer (shared by `FUN_00541190` and `FUN_00545A40`): the
/// remaining directions from `dir`, as long as `tries < 4`; direction and tries stay in the entry
/// (D-332). A candidate with mask 0 goes to `test`. If all fail, the entry is dropped and the
/// pointer jumps to the list head (`None` for an empty list). Return value: the candidate found.
fn visit_cursor(
    mask: &[u8],
    area: &mut Area,
    dim_x: i32,
    dim_z: i32,
    rng: &mut GenRand,
    test: &mut impl FnMut(i32, i32, &Area, &mut GenRand) -> anyhow::Result<bool>,
) -> anyhow::Result<Option<(i32, i32)>> {
    let Some(index) = area.cursor.filter(|&i| i < area.edges.len()) else {
        bail!("random map: edge list without a valid pointer");
    };
    loop {
        let mut e = area.edges[index];
        if e.tries >= 4 {
            break;
        }
        let (x, z) = step(e.x, e.z, e.dir, dim_x, dim_z);
        e.tries = e.tries.wrapping_add(1);
        e.dir = e.dir.wrapping_add(1);
        if e.dir == 4 {
            e.dir = 0;
        }
        area.edges[index] = e;
        let i = cell(x, z, dim_x, mask.len()).ok_or_else(|| outside(x, z))?;
        if mask[i] == 0 && test(x, z, area, rng)? {
            return Ok(Some((x, z)));
        }
    }
    area.edges.remove(index);
    area.cursor = if area.edges.is_empty() { None } else { Some(0) };
    Ok(None)
}

/// New edge list entry for the placed tile: direction `rand() % 4`, then pointer to entry
/// `rand() % count` (including the new entry), `grown + 1`.
fn push_edge(area: &mut Area, x: i32, z: i32, rng: &mut GenRand) {
    let dir = rng.rand() % 4;
    area.edges.push(EdgeEntry {
        x,
        z,
        dir,
        tries: 0,
    });
    let count = area.edges.len() as i32;
    area.cursor = Some((rng.rand() % count) as usize);
    area.grown = area.grown.wrapping_add(1);
}

/// One growth step of the land and player areas `FUN_00541190` (`0x00541190`). Without a grown
/// tile it places the center `(cx, cz)` without any check (D-331), otherwise the first candidate from
/// the edge list that is free and passes the distance test. Placed are mask = ID low byte and
/// ID grid = ID. Return value: whether a tile was placed.
pub(crate) fn grow_area(
    mask: &mut [u8],
    area: &mut Area,
    grid: &mut Grid,
    rng: &mut GenRand,
) -> anyhow::Result<bool> {
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    let (x, z) = if area.grown > 0 {
        loop {
            if area.edges.is_empty() {
                return Ok(false);
            }
            let mut test = |x, z, area: &Area, rng: &mut GenRand| avoid_ok(x, z, area, grid, rng);
            if let Some(found) = visit_cursor(mask, area, dim_x, dim_z, rng, &mut test)? {
                break found;
            }
        }
    } else {
        (area.cx, area.cz)
    };
    let len = mask.len().min(grid.id_grid.len());
    let i = cell(x, z, dim_x, len).ok_or_else(|| outside(x, z))?;
    mask[i] = area.def.id as u8;
    grid.id_grid[i] = area.def.id;
    push_edge(area, x, z, rng);
    Ok(true)
}

/// Clump growth of the terrain areas `FUN_00545A40` (`0x00545A40`): `grown = 0`, start tile
/// `(x, z)` without a check (D-331), then candidates from the edge list with mask 0 and `test`
/// (`FUN_005430A0`); only mask = 1 is set, the ID grid stays. After each step it ends when
/// `numTiles / numClumps ≤ grown` or the list is empty. The caller empties the edge list
/// (`FUN_005409A0`). Divisor 0 or `i32::MIN / −1` crashed in the original -> error message.
/// Return value: `grown`.
pub(crate) fn grow_clump(
    mask: &mut [u8],
    x: i32,
    z: i32,
    area: &mut Area,
    grid: &Grid,
    rng: &mut GenRand,
    mut test: impl FnMut(i32, i32, &Area, &mut GenRand) -> anyhow::Result<bool>,
) -> anyhow::Result<i32> {
    let (num_tiles, num_clumps) = (area.def.num_tiles, area.def.num_clumps);
    let Some(target) = num_tiles.checked_div(num_clumps) else {
        bail!("random map: numTiles {num_tiles} / numClumps {num_clumps} not divisible");
    };
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    area.grown = 0;
    loop {
        let found = if area.grown < 1 {
            Some((x, z))
        } else {
            visit_cursor(mask, area, dim_x, dim_z, rng, &mut test)?
        };
        if let Some((x, z)) = found {
            let i = cell(x, z, dim_x, mask.len()).ok_or_else(|| outside(x, z))?;
            mask[i] = 1;
            push_edge(area, x, z, rng);
        }
        if target <= area.grown || area.cursor.is_none() {
            return Ok(area.grown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::area::{Area, EdgeEntry};
    use crate::grid::Grid;
    use crate::rng::{GenRand, Phase};
    use nml_formats::rnd::AreaDef;
    use nml_world::rand::MsvcRand;

    /// The MSVC sequence written out: `state = state·214013 + 2531011`, `rand = (state >> 16) & 0x7FFF`.
    fn msvc_trace(seed: u32, n: usize) -> Vec<i32> {
        let mut state = seed;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
                ((state >> 16) & 0x7FFF) as i32
            })
            .collect()
    }

    /// Seed 1 (start value of the CRT): the first draws.
    const SEED1: [i32; 6] = [41, 18467, 6334, 26500, 19169, 15724];

    #[test]
    fn trace_matches_the_generator_sequence() {
        assert_eq!(msvc_trace(1, 6), SEED1);
        let mut rng = GenRand::new(18);
        let drawn: Vec<i32> = (0..4).map(|_| rng.rand()).collect();
        assert_eq!(drawn, msvc_trace(18, 4));
    }

    fn area(def: AreaDef, frame: (i32, i32, i32, i32)) -> Area {
        Area {
            def,
            x_min: frame.0,
            x_max: frame.1,
            z_min: frame.2,
            z_max: frame.3,
            cx: 0,
            cz: 0,
            grown: 0,
            edges: Vec::new(),
            cursor: None,
        }
    }

    fn with_id(id: i32) -> AreaDef {
        AreaDef {
            id,
            ..AreaDef::default()
        }
    }

    fn entry(x: i32, z: i32, dir: i32, tries: i32) -> EdgeEntry {
        EdgeEntry { x, z, dir, tries }
    }

    /// Mask as a picture, row by row (`#` = 0xFF, `.` = 0, otherwise digit).
    fn picture(mask: &[u8], dim_x: usize) -> Vec<String> {
        mask.chunks(dim_x)
            .map(|row| {
                row.iter()
                    .map(|&v| match v {
                        0xFF => '#',
                        0 => '.',
                        v => char::from_digit(u32::from(v) % 10, 10).unwrap(),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn rough_border_draws_four_times_dim() {
        let mut rng = GenRand::new(1);
        rng.set_phase(Phase::LandAreas);
        let mut mask = vec![0u8; 64];
        let a = area(AreaDef::default(), (2, 6, 1, 7));
        rough_border(&mut mask, &a, 8, 8, &mut rng).unwrap();
        // `BorderRoughness 0` still draws: `rand() % 1` per row and column, 4 * 8.
        assert_eq!(rng.counts().land_areas, 32);
        let mut plain = MsvcRand::new(1);
        (0..32).for_each(|_| {
            plain.rand();
        });
        assert_eq!(rng.state(), plain.state());
        assert_eq!(
            picture(&mask, 8),
            [
                "########", "##....##", "##....##", "##....##", "##....##", "##....##", "##....##",
                "########",
            ]
        );
    }

    #[test]
    fn rough_border_wraps_inside_and_rejects_outside() {
        // Seed 18: row draws 97, 4582, 15113, 9592 -> `% 10` = 7, 2, 3, 2; with `x_min = −2`
        // the first loop writes 5, 0, 1, 0 tiles per row.
        assert_eq!(msvc_trace(18, 4), [97, 4582, 15113, 9592]);
        let def = AreaDef {
            border_roughness: 9,
            ..AreaDef::default()
        };
        // The remaining three loops write nothing with these borders.
        let a = area(def, (-2, 13, -9, 13));
        let mut rng = GenRand::new(18);
        let mut mask = vec![0u8; 16];
        rough_border(&mut mask, &a, 4, 4, &mut rng).unwrap();
        assert_eq!(rng.counts().preprocessor, 16);
        // Row 0 writes five tiles: the fifth is the start of row 1 (D-335).
        assert_eq!(picture(&mask, 4), ["####", "#...", "#...", "...."]);

        // In the last row the write position would run past the mask -> error instead of overwriting.
        let a = area(AreaDef::default(), (5, 4, 0, 4));
        let mut mask = vec![0u8; 16];
        let error = rough_border(&mut mask, &a, 4, 4, &mut GenRand::new(1)).unwrap_err();
        assert!(format!("{error:#}").contains("rough border"), "{error:#}");
        // `BorderRoughness −1` divided by zero.
        let def = AreaDef {
            border_roughness: -1,
            ..AreaDef::default()
        };
        let error = rough_border(
            &mut [0; 16],
            &area(def, (0, 4, 0, 4)),
            4,
            4,
            &mut GenRand::new(1),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("BorderRoughness"),
            "{error:#}"
        );
        // Huge roughness writes before the mask (`z_max − r < 0`) -> error, no panic.
        let def = AreaDef {
            border_roughness: i32::MAX,
            ..AreaDef::default()
        };
        assert!(
            rough_border(
                &mut [0; 16],
                &area(def, (0, 4, 0, 4)),
                4,
                4,
                &mut GenRand::new(1)
            )
            .is_err()
        );
    }

    #[test]
    fn first_tile_is_set_without_checks() {
        let mut grid = Grid::new(4, 4);
        let mut mask = vec![0xFFu8; 16];
        // The center lies in the ragged border and on a foreign area: set anyway (D-331).
        grid.id_grid[0] = 7;
        let mut a = area(with_id(258), (1, 3, 1, 3));
        let mut rng = GenRand::new(1);
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        // Mask = ID low byte, ID grid = whole ID.
        assert_eq!(mask[0], 2);
        assert_eq!(grid.id_grid[0], 258);
        // Direction `41 % 4 = 1`, pointer `18467 % 1 = 0`.
        assert_eq!(a.edges, [entry(0, 0, 1, 0)]);
        assert_eq!(a.cursor, Some(0));
        assert_eq!(a.grown, 1);
        assert_eq!(rng.counts().preprocessor, 2);
    }

    #[test]
    fn edge_entry_keeps_direction_and_tries() {
        let mut grid = Grid::new(3, 3);
        let mut mask = vec![0u8; 9];
        // Center (1, 1) set, border on the left and right.
        mask[4] = 5;
        mask[3] = 0xFF;
        mask[5] = 0xFF;
        let mut a = area(with_id(5), (0, 3, 0, 3));
        a.grown = 1;
        a.edges = vec![entry(1, 1, 0, 0)];
        a.cursor = Some(0);
        let mut rng = GenRand::new(1);
        // Directions 0 (x-1) and 1 (x+1) fail without a draw, 2 (z+1) is free: distance test (41),
        // direction of the new entry `18467 % 4 = 3`, pointer `6334 % 2 = 0`.
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert_eq!(mask[7], 5);
        assert_eq!(a.edges, [entry(1, 1, 3, 3), entry(1, 2, 3, 0)]);
        assert_eq!(a.cursor, Some(0));
        // Next visit of the same entry: continues with direction 3 (z-1), not from the start - left
        // would now be free but stays empty (D-332).
        mask[3] = 0;
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert_eq!(mask[1], 5);
        assert_eq!(mask[3], 0);
        // Distance test (26500), direction `19169 % 4 = 1`, pointer `15724 % 3 = 1`.
        assert_eq!(
            a.edges,
            [entry(1, 1, 0, 4), entry(1, 2, 3, 0), entry(1, 0, 1, 0)]
        );
        assert_eq!(a.cursor, Some(1));
        assert_eq!(a.grown, 3);
        assert_eq!(rng.counts().preprocessor, 6);
        assert_eq!(&grid.id_grid[..], &[0, 5, 0, 0, 0, 0, 0, 5, 0]);
    }

    #[test]
    fn failed_entry_falls_back_to_the_list_head() {
        let mut grid = Grid::new(5, 5);
        let mut mask = vec![0xFFu8; 25];
        mask[5 + 2] = 0;
        let mut a = area(with_id(4), (0, 5, 0, 5));
        a.grown = 3;
        a.edges = vec![entry(1, 1, 1, 0), entry(1, 3, 0, 0), entry(3, 3, 0, 0)];
        a.cursor = Some(2);
        let mut rng = GenRand::new(1);
        // The entry under the pointer fails in all four directions (without a draw) and is dropped;
        // it continues with the head (1, 1), not with a drawn entry (D-332).
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert_eq!(mask[5 + 2], 4);
        // Distance test (41), direction `18467 % 4 = 3`, pointer `6334 % 3 = 1`.
        assert_eq!(
            a.edges,
            [entry(1, 1, 2, 1), entry(1, 3, 0, 0), entry(2, 1, 3, 0)]
        );
        assert_eq!(a.cursor, Some(1));
        assert_eq!(a.grown, 4);
        assert_eq!(rng.counts().preprocessor, 3);

        // If the last entry fails, the list is empty: no progress, pointer empty.
        let mut a = area(with_id(4), (0, 5, 0, 5));
        a.grown = 1;
        a.edges = vec![entry(3, 3, 2, 0)];
        a.cursor = Some(0);
        let mut rng = GenRand::new(1);
        assert!(!grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert!(a.edges.is_empty());
        assert_eq!(a.cursor, None);
        assert_eq!(rng.counts().preprocessor, 0);
        // With an empty list it stays that way.
        assert!(!grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
    }

    #[test]
    fn grow_draws_two_per_tile_and_one_per_free_candidate() {
        // All neighbors are free but belong to a foreign area: one distance test per candidate,
        // each fails.
        let foreign = |start: usize| {
            let mut grid = Grid::new(3, 3);
            grid.id_grid = vec![9; 9];
            grid.id_grid[start] = 0;
            grid
        };
        let mut rng = GenRand::new(7);
        rng.set_phase(Phase::LandAreas);
        let mut grid = foreign(4);
        let mut mask = vec![0u8; 9];
        let mut a = area(with_id(1), (0, 3, 0, 3));
        (a.cx, a.cz) = (1, 1);
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert_eq!(rng.counts().land_areas, 2);
        assert!(!grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert_eq!(rng.counts().land_areas, 2 + 4);

        // In the corner, two steps out of the map stay on the start tile (mask != 0, no draw), two
        // candidates draw.
        let mut grid = foreign(0);
        let mut mask = vec![0u8; 9];
        let mut a = area(with_id(1), (0, 3, 0, 3));
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert!(!grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap());
        assert_eq!(rng.counts().land_areas, 6 + 2 + 2);

        // A free 3x3 field grows completely: 9 tiles with 2 draws each, 8 candidates with 1 each.
        let mut grid = Grid::new(3, 3);
        let mut mask = vec![0u8; 9];
        let mut a = area(with_id(3), (0, 3, 0, 3));
        (a.cx, a.cz) = (2, 1);
        let mut rng = GenRand::new(7);
        rng.set_phase(Phase::LandAreas);
        while grow_area(&mut mask, &mut a, &mut grid, &mut rng).unwrap() {}
        assert_eq!(a.grown, 9);
        assert_eq!(mask, vec![3; 9]);
        assert_eq!(grid.id_grid, vec![3; 9]);
        assert_eq!(rng.counts().land_areas, 9 * 2 + 8);
        assert_eq!(rng.counts().preprocessor, 0);
    }

    #[test]
    fn avoid_ok_checks_the_square_around_the_tile() {
        let mut grid = Grid::new(5, 5);
        grid.id_grid[5 * 4 + 4] = 9;
        grid.id_grid[5 * 3 + 2] = 3;
        let def = AreaDef {
            id: 3,
            avoid_other: 1,
            ..AreaDef::default()
        };
        let a = area(def, (0, 5, 0, 5));
        let mut rng = GenRand::new(1);
        // Own ID and ID 0 do not matter, foreign ones in `[x−d, x+d] × [z−d, z+d]` do.
        assert!(avoid_ok(2, 2, &a, &grid, &mut rng).unwrap());
        assert!(!avoid_ok(3, 3, &a, &grid, &mut rng).unwrap());
        // At the map edge only the tiles in the map.
        assert!(avoid_ok(0, 4, &a, &grid, &mut rng).unwrap());
        // Roughness: `d = max(0, 1 − rand() % 6)`; seed 1 draws 26500 here -> 4 -> d = 0.
        let mut rough = a.clone();
        rough.def.avoid_roughness = 5;
        assert_eq!(msvc_trace(1, 4)[3] % 6, 4);
        assert!(avoid_ok(3, 3, &rough, &grid, &mut rng).unwrap());
        // `x + d` overflows: the loop does not run, the tile counts as free (like x86).
        let mut huge = a.clone();
        huge.def.avoid_other = i32::MAX;
        assert!(avoid_ok(3, 3, &huge, &grid, &mut rng).unwrap());
        // Exactly one draw per call.
        assert_eq!(rng.counts().preprocessor, 5);
        // `AvoidRoughness −1` divided by zero.
        let mut zero = a.clone();
        zero.def.avoid_roughness = -1;
        let error = avoid_ok(2, 2, &zero, &grid, &mut rng).unwrap_err();
        assert!(format!("{error:#}").contains("AvoidRoughness"), "{error:#}");
    }

    #[test]
    fn start_outside_the_mask_is_an_error() {
        let mut grid = Grid::new(3, 3);
        let mut mask = vec![0u8; 9];
        let mut a = area(with_id(1), (0, 3, 0, 3));
        (a.cx, a.cz) = (-1, 0);
        let error = grow_area(&mut mask, &mut a, &mut grid, &mut GenRand::new(1)).unwrap_err();
        assert!(format!("{error:#}").contains("outside"), "{error:#}");
        // Inside the mask but left of the map: position `3·1 − 1` = end of row 0 as in the
        // original.
        (a.cx, a.cz) = (-1, 1);
        assert!(grow_area(&mut mask, &mut a, &mut grid, &mut GenRand::new(1)).unwrap());
        assert_eq!(mask[2], 1);
        assert_eq!(grid.id_grid[2], 1);
    }

    #[test]
    fn clump_growth_does_not_touch_the_id_grid() {
        let mut grid = Grid::new(4, 4);
        grid.id_grid = vec![7; 16];
        let mut mask = vec![0u8; 16];
        let def = AreaDef {
            num_tiles: 10,
            num_clumps: 2,
            ..AreaDef::default()
        };
        let mut a = area(def, (0, 4, 0, 4));
        a.grown = 99;
        let mut rng = GenRand::new(3);
        let mut calls = 0;
        let grown = grow_clump(&mut mask, 1, 2, &mut a, &grid, &mut rng, |_, _, _, _| {
            calls += 1;
            Ok(true)
        })
        .unwrap();
        // Target `10 / 2 = 5`; `grown` starts at 0, the start tile counts.
        assert_eq!(grown, 5);
        assert_eq!(a.grown, 5);
        assert_eq!(mask[4 * 2 + 1], 1);
        assert_eq!(mask.iter().filter(|&&v| v == 1).count(), 5);
        assert!(mask.iter().all(|&v| v <= 1));
        assert_eq!(grid.id_grid, vec![7; 16]);
        // Start tile without test, four candidates with test; two draws per tile.
        assert_eq!(calls, 4);
        assert_eq!(rng.counts().preprocessor, 10);

        // Target 0: the start tile is placed anyway.
        let mut mask = vec![0u8; 16];
        let mut a = area(
            AreaDef {
                num_tiles: 0,
                ..AreaDef::default()
            },
            (0, 4, 0, 4),
        );
        let grown = grow_clump(&mut mask, 0, 0, &mut a, &grid, &mut rng, |_, _, _, _| {
            Ok(true)
        })
        .unwrap();
        assert_eq!(grown, 1);

        // If every candidate fails, the clump ends with an empty list.
        let mut mask = vec![0u8; 16];
        let mut a = area(
            AreaDef {
                num_tiles: 9,
                ..AreaDef::default()
            },
            (0, 4, 0, 4),
        );
        let grown = grow_clump(&mut mask, 1, 1, &mut a, &grid, &mut rng, |_, _, _, _| {
            Ok(false)
        })
        .unwrap();
        assert_eq!(grown, 1);
        assert_eq!(a.cursor, None);
    }

    #[test]
    fn clump_target_rejects_bad_divisors() {
        let grid = Grid::new(4, 4);
        for (num_tiles, num_clumps) in [(5, 0), (i32::MIN, -1)] {
            let def = AreaDef {
                num_tiles,
                num_clumps,
                ..AreaDef::default()
            };
            let mut a = area(def, (0, 4, 0, 4));
            let mut rng = GenRand::new(1);
            let error = grow_clump(&mut [0; 16], 1, 1, &mut a, &grid, &mut rng, |_, _, _, _| {
                Ok(true)
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("numClumps"), "{error:#}");
            assert_eq!(rng.counts().preprocessor, 0);
        }
        // An error of the candidate test aborts.
        let def = AreaDef {
            num_tiles: 5,
            ..AreaDef::default()
        };
        let mut a = area(def, (0, 4, 0, 4));
        let result = grow_clump(
            &mut [0; 16],
            1,
            1,
            &mut a,
            &grid,
            &mut GenRand::new(1),
            |_, _, _, _| anyhow::bail!("Test"),
        );
        assert!(result.is_err());
    }
}
