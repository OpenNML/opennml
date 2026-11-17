//! Terrain areas (re/random-map Z.6.7): step 8 of the generator, `create_TerrainArea` `FUN_005466F0` with the
//! candidate test `FUN_005430A0`.

use anyhow::bail;
use nml_formats::rnd::RandomMapScript;
use nml_sim::fx::{Fx, fixed_div, fixed_hypot};

use crate::PlayerArea;
use crate::area::Area;
use crate::grid::Grid;
use crate::growth::{grow_clump, roll, rough_border};
use crate::paint::{paint_mask, texture_number};
use crate::rng::GenRand;

/// Upper limit for `numClumps`. Real scripts stay far below it (even with 8 players and map factor 4);
/// above it, even a block without a hit would draw `305 · numClumps` random numbers, and the original would
/// run practically forever with `numClumps` near `i32::MAX` (never a hang, as with `connect_Players`).
const MAX_CLUMPS: i32 = 1 << 16;

/// Limit of the clump attempts (`CMP 0x64` at `0x005467F9` and `0x0054681A`, D-334).
const CLUMP_TRIES: i32 = 100;

/// Player distance `ToInt(Fixed_Hypot(FromInt(dx), FromInt(dz)))` (`FUN_005430A0`).
///
/// `Fixed_Hypot` divides by `s = Div(hi, 1.0)` or `s = Div(1.0, hi)`, with `hi` the larger magnitude.
/// If `hi << 10` in 32 bits ends up at a magnitude below 4096 (|d| a multiple of 1024, or both
/// fixed-point values `i32::MIN`), `s = 0`: in the original a division by zero (crash), in
/// [`fixed_hypot`] a panic. The guard computes exactly this `s` beforehand and reports the case;
/// in all other cases - even far outside the map - it computes like [`fixed_hypot`].
/// Shared with the `AvoidPlayerAreas` check of the object step (`FUN_00543410`,
/// `crate::checks::far_from_players`).
pub(crate) fn player_distance(dx: i32, dz: i32) -> anyhow::Result<i32> {
    let (p, q) = (Fx::from_int(dx).0, Fx::from_int(dz).0);
    let hi = p.wrapping_abs().max(q.wrapping_abs());
    let divisor = match hi {
        0 => 1,
        0x1000.. => fixed_div(hi, 0x1000),
        _ => fixed_div(0x1000, hi),
    };
    if divisor == 0 {
        bail!(
            "random map: player distance ({dx}, {dz}) divides by zero in Fixed_Hypot \
             (candidate or player center far outside the map)"
        );
    }
    Ok(Fx(fixed_hypot(p, q)).to_int())
}

/// Candidate test `FUN_005430A0` (`0x005430A0`) of the terrain areas:
/// 1. with `AvoidPlayerAreas`, for each entry of the player area list (`G+0x3C`, list order)
///    `ToInt(Hypot(x − cx, z − cz)) < minSize` -> no, **without** a draw;
/// 2. `d = max(0, AvoidOther − rand() % (AvoidRoughness + 1))`, exactly one `rand()`;
/// 3. every tile in `[x−d, x+d] × [z−d, z+d]` inside the map must carry the target number: `PutOnto`,
///    otherwise `Replace`, otherwise -1 (set = not empty; unknown name -> -1).
///
/// The original computes the bounds in 32 bits: if `x + d` overflows, the loop does not run and the
/// test passes. The reimplementation clips the square to the map (like `avoid_ok`).
pub(crate) fn candidate_ok(
    x: i32,
    z: i32,
    area: &Area,
    players: &[PlayerArea],
    grid: &Grid,
    script: &RandomMapScript,
    rng: &mut GenRand,
) -> anyhow::Result<bool> {
    let def = &area.def;
    if def.avoid_player_areas {
        for p in players {
            let dist = player_distance(x.wrapping_sub(p.cx), z.wrapping_sub(p.cz))?;
            if dist < p.min_size {
                return Ok(false);
            }
        }
    }
    let r = roll(rng, def.avoid_roughness, "AvoidRoughness")?;
    let d = def.avoid_other.wrapping_sub(r).max(0);
    let target = if !def.put_onto.is_empty() {
        texture_number(script, &def.put_onto)
    } else if !def.replace.is_empty() {
        texture_number(script, &def.replace)
    } else {
        -1
    };
    let (x_lo, x_hi) = (x.wrapping_sub(d), x.wrapping_add(d));
    let (z_lo, z_hi) = (z.wrapping_sub(d), z.wrapping_add(d));
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    for xx in x_lo.max(0)..=x_hi.min(dim_x - 1) {
        for zz in z_lo.max(0)..=z_hi.min(dim_z - 1) {
            if grid.tile(xx as usize, zz as usize) != target {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Case A (`0x0054674E`): zero the mask, rough border, then `FUN_005459A0` - each tile (x outer,
/// z inner) with mask 0 and a passed test gets 1 - and only then paint: the test sees the tiles from
/// before the block (no cascade).
fn replace_block(
    mask: &mut [u8],
    area: &Area,
    grid: &mut Grid,
    script: &RandomMapScript,
    players: &[PlayerArea],
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    mask.fill(0);
    rough_border(mask, area, dim_x, dim_z, rng)?;
    for x in 0..grid.dim_x {
        for z in 0..grid.dim_z {
            let i = grid.dim_x * z + x;
            if mask[i] == 0 && candidate_ok(x as i32, z as i32, area, players, grid, script, rng)? {
                mask[i] = 1;
            }
        }
    }
    paint_mask(grid, mask, texture_number(script, &area.def.texture));
    Ok(())
}

/// One clump in case B (`0x005467B0`): position attempts `cx = rand() % (x_max − x_min) + x_min`, then
/// `cz = rand() % (z_max − z_min) + z_min` (center in the record), `alt = tries; tries += 1`, stop if
/// `alt > 100`, otherwise test. Only with `tries < 100` (hit at `alt ≤ 98`, D-334): zero the mask,
/// rough border, growth `FUN_00545A40`, paint immediately if `grown ≥ minSize` (later clumps see it),
/// clear the edge list (`FUN_005409A0`).
///
/// Empty frame: the original divides by zero (crash, `IDIV` after the respective `rand()`). The
/// reimplementation reports `x_max == x_min` before the first draw and `z_max == z_min` after the
/// x draw (D-335).
fn clump(
    mask: &mut [u8],
    area: &mut Area,
    grid: &mut Grid,
    script: &RandomMapScript,
    players: &[PlayerArea],
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let mut tries = 0i32;
    loop {
        let x_span = area.x_max.wrapping_sub(area.x_min);
        if x_span == 0 {
            bail!(
                "random map: create_TerrainArea with empty frame in x (x_min = x_max = {}) \
                 divides by zero",
                area.x_min
            );
        }
        // `rand() ≥ 0` and divisor `≠ 0`: the C remainder never overflows.
        area.cx = (rng.rand() % x_span).wrapping_add(area.x_min);
        let z_span = area.z_max.wrapping_sub(area.z_min);
        if z_span == 0 {
            bail!(
                "random map: create_TerrainArea with empty frame in z (z_min = z_max = {}) \
                 divides by zero",
                area.z_min
            );
        }
        area.cz = (rng.rand() % z_span).wrapping_add(area.z_min);
        let alt = tries;
        tries += 1;
        if alt > CLUMP_TRIES {
            break;
        }
        if candidate_ok(area.cx, area.cz, area, players, grid, script, rng)? {
            break;
        }
    }
    if tries >= CLUMP_TRIES {
        return Ok(());
    }
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    mask.fill(0);
    rough_border(mask, area, dim_x, dim_z, rng)?;
    let (cx, cz) = (area.cx, area.cz);
    let test =
        |x, z, area: &Area, rng: &mut GenRand| candidate_ok(x, z, area, players, grid, script, rng);
    let grown = grow_clump(mask, cx, cz, area, grid, rng, test)?;
    if area.def.min_size <= grown {
        paint_mask(grid, mask, texture_number(script, &area.def.texture));
    }
    area.edges.clear();
    area.cursor = None;
    Ok(())
}

/// `create_TerrainArea` `FUN_005466F0` (`0x005466F0`), step 8: only with a non-empty list, one shared
/// mask; each area in script order with `Replace` (non-empty) case A, otherwise case B with
/// `numClumps` clumps (none with `numClumps ≤ 0`: `do … while` behind `0 < numClumps`).
/// The ID grid stays untouched.
pub(crate) fn terrain_areas(
    areas: &mut [Area],
    grid: &mut Grid,
    script: &RandomMapScript,
    players: &[PlayerArea],
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    if areas.is_empty() {
        return Ok(());
    }
    let mut mask = vec![0u8; grid.dim_x * grid.dim_z];
    for area in areas.iter_mut() {
        if area.def.replace.is_empty() {
            if area.def.num_clumps > MAX_CLUMPS {
                bail!(
                    "random map: create_TerrainArea with numClumps {} (at most {MAX_CLUMPS})",
                    area.def.num_clumps
                );
            }
            for _ in 0..area.def.num_clumps {
                clump(&mut mask, area, grid, script, players, rng)?;
            }
        } else {
            replace_block(&mut mask, area, grid, script, players, rng)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::area::finish_area;
    use crate::paint::tests::{texture, tile_picture};
    use crate::rng::Phase;
    use crate::spec::scale_factor;
    use nml_formats::rnd::AreaDef;
    use nml_world::rand::MsvcRand;

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

    fn script(names: &[&str]) -> RandomMapScript {
        RandomMapScript {
            textures: names
                .iter()
                .enumerate()
                .map(|(i, name)| texture(i as i32 + 1, name))
                .collect(),
            ..RandomMapScript::default()
        }
    }

    /// Grid with all tiles set to `number`.
    fn filled(dim_x: usize, dim_z: usize, number: i32) -> Grid {
        let mut grid = Grid::new(dim_x, dim_z);
        grid.tiles.fill(number);
        grid
    }

    fn terrain_rng(seed: u32) -> GenRand {
        let mut rng = GenRand::new(seed);
        rng.set_phase(Phase::TerrainAreas);
        rng
    }

    /// Terrain areas with extreme values return `Ok` or an error message, never a panic (debug build,
    /// with overflow checks) and never a hang.
    #[test]
    fn extreme_terrain_values_never_panic() {
        let base = AreaDef {
            texture: "B".into(),
            put_onto: "A".into(),
            num_clumps: 3,
            ..AreaDef::default()
        };
        let variants = [
            AreaDef {
                border_roughness: 9999,
                ..base.clone()
            },
            AreaDef {
                border_roughness: i32::MAX,
                avoid_roughness: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                avoid_roughness: i32::MAX,
                avoid_other: 3,
                ..base.clone()
            },
            AreaDef {
                num_tiles: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                num_clumps: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                left: -50,
                right: -50,
                top: -50,
                bottom: -50,
                num_tiles: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                left: 60,
                right: 60,
                top: 60,
                bottom: 60,
                border_roughness: 9999,
                ..base.clone()
            },
            AreaDef {
                min_size: 500,
                num_tiles: 1000,
                ..base.clone()
            },
            AreaDef {
                min_size: i32::MIN,
                num_tiles: i32::MIN,
                ..base.clone()
            },
            AreaDef {
                avoid_other: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                replace: "A".into(),
                border_roughness: 9999,
                avoid_roughness: i32::MAX,
                avoid_other: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                replace: "A".into(),
                left: -50,
                right: 60,
                top: 60,
                bottom: -50,
                avoid_other: i32::MIN,
                ..base.clone()
            },
        ];
        let names = script(&["A", "B"]);
        for (i, def) in variants.iter().enumerate() {
            let finished = finish_area(def, 16, 16, 2, scale_factor(0));
            let mut grid = filled(16, 16, 1);
            let mut rng = terrain_rng(i as u32);
            if let Err(error) = terrain_areas(&mut [finished], &mut grid, &names, &[], &mut rng) {
                let text = format!("{error:#}");
                assert!(text.contains("random map"), "variant {i}: {text}");
            }
        }
    }

    #[test]
    fn replace_paints_after_the_whole_scan() {
        // Three columns of A (1) on the left, three columns of B (2) on the right.
        let mut grid = filled(6, 3, 2);
        for z in 0..3 {
            for x in 0..3 {
                grid.set_tile(x, z, 1);
            }
        }
        let def = AreaDef {
            replace: "A".into(),
            texture: "B".into(),
            avoid_other: 1,
            ..AreaDef::default()
        };
        let mut areas = [area(def, (0, 6, 0, 3))];
        let mut rng = terrain_rng(1);
        terrain_areas(&mut areas, &mut grid, &script(&["A", "B"]), &[], &mut rng).unwrap();
        // Columns 0 and 1 had only A in their 3x3 square before the block, column 2 borders B.
        // With a cascade (paint immediately, x outer) column 1 would already see the new B in column 0
        // and stay A.
        assert_eq!(tile_picture(&grid), ["221222", "221222", "221222"]);
        // Rough border `2·DIM_Z + 2·DIM_X`, then one test with one draw per free tile.
        assert_eq!(rng.counts().terrain_areas, 2 * 3 + 2 * 6 + 18);
        // The ID grid stays.
        assert_eq!(grid.id_grid, vec![0; 18]);
    }

    /// First hit `alt` (0-based) of the clump search on a 4x4 field with exactly one target tile
    /// `(2, 3)`, by hand with the MSVC sequence: per attempt x, z, then with `alt ≤ 100` the test with
    /// one draw (`AvoidRoughness 0`).
    fn first_hit(seed: u32) -> Option<i32> {
        let mut plain = MsvcRand::new(seed);
        for alt in 0..=101 {
            let x = plain.rand() % 4;
            let z = plain.rand() % 4;
            if alt > 100 {
                return None;
            }
            plain.rand();
            if (x, z) == (2, 3) {
                return Some(alt);
            }
        }
        None
    }

    /// One clump `PutOnto SEA`, `Texture ROCK` on LAND with one SEA tile `(2, 3)`.
    fn single_target_run(seed: u32) -> (Grid, Area, u32) {
        let mut grid = filled(4, 4, 1);
        grid.set_tile(2, 3, 2);
        let def = AreaDef {
            put_onto: "SEA".into(),
            texture: "ROCK".into(),
            num_tiles: 1,
            ..AreaDef::default()
        };
        let mut areas = [area(def, (0, 4, 0, 4))];
        let mut rng = terrain_rng(seed);
        let names = script(&["LAND", "SEA", "ROCK"]);
        terrain_areas(&mut areas, &mut grid, &names, &[], &mut rng).unwrap();
        let [area] = areas;
        (grid, area, rng.counts().terrain_areas)
    }

    #[test]
    fn clump_hit_at_try_99_is_discarded() {
        // D-334: `alt = tries; tries += 1; if alt > 100 break; if Test break;` and afterwards grow only
        // with `tries < 100`. A hit at `alt = 99` (`tries = 100`) is rejected.
        assert_eq!(first_hit(45_495), Some(99));
        let (grid, _, draws) = single_target_run(45_495);
        assert_eq!(grid.tile(2, 3), 2);
        assert_eq!(grid.tiles.iter().filter(|&&n| n == 3).count(), 0);
        // 100 attempts of three draws each, no border, no growth.
        assert_eq!(draws, 100 * 3);

        // Likewise at `alt = 100`, the last attempt with a test.
        assert_eq!(first_hit(21_248), Some(100));
        let (grid, _, draws) = single_target_run(21_248);
        assert_eq!(grid.tile(2, 3), 2);
        assert_eq!(draws, 101 * 3);

        // Without a hit: 102 position draws, but only 101 tests.
        assert_eq!(first_hit(882), None);
        let (grid, _, draws) = single_target_run(882);
        assert_eq!(grid.tile(2, 3), 2);
        assert_eq!(draws, 102 * 2 + 101);

        // A hit at `alt = 98` counts: rough border 4*4, start tile with two draws, painted
        // (`grown 1 ≥ minSize 0`), edge list cleared (`FUN_005409A0`).
        assert_eq!(first_hit(9_179), Some(98));
        let (grid, area, draws) = single_target_run(9_179);
        assert_eq!(draws, 99 * 3 + 4 * 4 + 2);
        assert_eq!(grid.tile(2, 3), 3);
        assert_eq!(grid.tiles.iter().filter(|&&n| n == 3).count(), 1);
        assert_eq!((area.cx, area.cz, area.grown), (2, 3, 1));
        assert!(area.edges.is_empty());
        assert_eq!(area.cursor, None);
    }

    #[test]
    fn block_without_replace_or_putonto_only_draws() {
        // After the base texture no tile carries -1: every test fails, 102 * 2 + 101 draws per clump,
        // the tiles stay (D-334).
        let def = AreaDef {
            texture: "ROCK".into(),
            num_tiles: 10,
            num_clumps: 3,
            ..AreaDef::default()
        };
        let names = script(&["LAND", "ROCK"]);
        let mut grid = filled(8, 8, 1);
        let mut areas = [area(def.clone(), (0, 8, 0, 8))];
        let mut rng = terrain_rng(5);
        terrain_areas(&mut areas, &mut grid, &names, &[], &mut rng).unwrap();
        assert_eq!(grid.tiles, vec![1; 64]);
        assert_eq!(rng.counts().terrain_areas, 3 * (102 * 2 + 101));

        // On unpainted tiles (-1) the same block takes effect immediately: target -1.
        let mut grid = Grid::new(8, 8);
        let mut areas = [area(def, (0, 8, 0, 8))];
        terrain_areas(&mut areas, &mut grid, &names, &[], &mut terrain_rng(5)).unwrap();
        assert!(grid.tiles.contains(&2));
    }

    #[test]
    fn avoid_player_areas_skips_the_draw() {
        let grid = filled(8, 8, 1);
        let names = script(&["A"]);
        let def = AreaDef {
            put_onto: "A".into(),
            avoid_player_areas: true,
            ..AreaDef::default()
        };
        let a = area(def, (0, 8, 0, 8));
        let near = PlayerArea {
            id: 1,
            cx: 2,
            cz: 2,
            min_size: 3,
        };
        let far = PlayerArea {
            id: 2,
            cx: 7,
            cz: 7,
            min_size: 1,
        };
        let mut rng = terrain_rng(1);
        // `ToInt(Hypot(1, 1)) = 1 < 3`: no, without a draw - even if the player is not the first one.
        assert!(!candidate_ok(3, 3, &a, &[near], &grid, &names, &mut rng).unwrap());
        assert!(!candidate_ok(3, 3, &a, &[far, near], &grid, &names, &mut rng).unwrap());
        assert_eq!(rng.counts().terrain_areas, 0);
        // Distance 3 is not less than 3: continues with exactly one draw.
        assert!(candidate_ok(5, 2, &a, &[near], &grid, &names, &mut rng).unwrap());
        assert_eq!(rng.counts().terrain_areas, 1);
        // Without `AvoidPlayerAreas` the distance does not count.
        let mut free = a.clone();
        free.def.avoid_player_areas = false;
        assert!(candidate_ok(3, 3, &free, &[near], &grid, &names, &mut rng).unwrap());
        assert_eq!(rng.counts().terrain_areas, 2);
        // Center exactly on the player: `Hypot(0, 0) = 0`.
        assert!(!candidate_ok(2, 2, &a, &[near], &grid, &names, &mut rng).unwrap());
        assert_eq!(rng.counts().terrain_areas, 2);
    }

    #[test]
    fn candidate_square_needs_the_target_texture() {
        // A (1) everywhere, B (2) at (4, 4).
        let mut grid = filled(6, 6, 1);
        grid.set_tile(4, 4, 2);
        let names = script(&["A", "B"]);
        let check = |def: AreaDef, x, z| {
            let a = area(def, (0, 6, 0, 6));
            candidate_ok(x, z, &a, &[], &grid, &names, &mut terrain_rng(1)).unwrap()
        };
        let with = |put_onto: &str, replace: &str, avoid_other| AreaDef {
            put_onto: put_onto.into(),
            replace: replace.into(),
            avoid_other,
            ..AreaDef::default()
        };
        // `PutOnto` before `Replace`.
        assert!(check(with("A", "B", 0), 2, 2));
        assert!(!check(with("B", "A", 0), 2, 2));
        assert!(check(with("", "A", 0), 2, 2));
        // `(2d+1)²`: d = 1 sees B from (3, 3), d = 0 does not.
        assert!(!check(with("A", "", 1), 3, 3));
        assert!(check(with("A", "", 0), 3, 3));
        // At the edge only the tiles inside the map.
        assert!(check(with("A", "", 1), 0, 0));
        // Unknown name -> -1, no tile carries -1.
        assert!(!check(with("C", "", 0), 0, 0));
        // `x + d` overflows: the square is empty, the test passes (like x86).
        assert!(check(with("B", "", i32::MAX), 3, 3));
        // `AvoidRoughness −1` divided by zero.
        let def = AreaDef {
            avoid_roughness: -1,
            ..with("A", "", 0)
        };
        let error = candidate_ok(
            2,
            2,
            &area(def, (0, 6, 0, 6)),
            &[],
            &grid,
            &names,
            &mut terrain_rng(1),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("AvoidRoughness"), "{error:#}");
    }

    #[test]
    fn player_distance_division_by_zero_is_an_error() {
        // `Fixed_Hypot` divides by zero for distances whose fixed-point value `·1024` comes to zero in
        // 32 bits (|d| = 1024, 2048, ...). Below that the reimplementation computes like `fixed_hypot`.
        let grid = filled(4, 4, 1);
        let names = script(&["A"]);
        let def = AreaDef {
            put_onto: "A".into(),
            avoid_player_areas: true,
            ..AreaDef::default()
        };
        let a = area(def, (0, 4, 0, 4));
        let player = |cx, cz| PlayerArea {
            id: 1,
            cx,
            cz,
            min_size: 1,
        };
        for (cx, cz) in [(1024, 0), (0, -1024), (-3072, 1), (1 << 19, 1 << 19)] {
            let mut rng = terrain_rng(1);
            let error =
                candidate_ok(0, 0, &a, &[player(cx, cz)], &grid, &names, &mut rng).unwrap_err();
            assert!(
                format!("{error:#}").contains("player distance"),
                "{error:#}"
            );
            assert_eq!(rng.counts().terrain_areas, 0);
        }
        // Up to |d| = 1023 (far beyond any map) the guard never reports and computes like
        // `fixed_hypot`.
        for dx in -1023..=1023 {
            for dz in (-1023..=1023).step_by(3) {
                let expected = Fx(fixed_hypot(Fx::from_int(dx).0, Fx::from_int(dz).0)).to_int();
                assert_eq!(player_distance(dx, dz).unwrap(), expected, "({dx}, {dz})");
            }
        }
        assert_eq!(player_distance(3, 4).unwrap(), 5);
        // Far, but without a division by zero: keeps computing without a panic.
        for (cx, cz) in [(1023, 0), (600, 900), (i32::MAX, i32::MIN)] {
            candidate_ok(
                0,
                0,
                &a,
                &[player(cx, cz)],
                &grid,
                &names,
                &mut terrain_rng(1),
            )
            .unwrap();
        }
    }

    #[test]
    fn empty_frame_is_an_error() {
        let grid = filled(4, 4, 1);
        let names = script(&["A"]);
        let run = |frame, num_clumps| {
            let def = AreaDef {
                num_clumps,
                ..AreaDef::default()
            };
            let mut grid = grid.clone();
            let mut rng = terrain_rng(1);
            let result = terrain_areas(&mut [area(def, frame)], &mut grid, &names, &[], &mut rng);
            (result, rng.counts().terrain_areas)
        };
        // `x_max == x_min` before the first draw, `z_max == z_min` after the x draw.
        let (result, draws) = run((3, 3, 0, 4), 1);
        assert!(format!("{:#}", result.unwrap_err()).contains("frame"));
        assert_eq!(draws, 0);
        let (result, draws) = run((0, 4, 2, 2), 1);
        assert!(format!("{:#}", result.unwrap_err()).contains("frame"));
        assert_eq!(draws, 1);
        // Without clumps (`numClumps ≤ 0`) the loop does not run: no error, no randomness.
        for num_clumps in [0, -1, i32::MIN] {
            let (result, draws) = run((3, 3, 2, 2), num_clumps);
            result.unwrap();
            assert_eq!(draws, 0);
        }
        // Absurdly many clumps: a message before the first draw instead of a hang.
        let (result, draws) = run((0, 4, 0, 4), MAX_CLUMPS + 1);
        assert!(format!("{:#}", result.unwrap_err()).contains("numClumps"));
        assert_eq!(draws, 0);
        // A reversed frame does not divide by zero: `rand() % −4` is the C remainder (0 ... 3), so with
        // `x_min = 4` x = 4 ... 7. Seed 1: `41 % −4 + 4 = 5`, `18467 % 4 = 3`; the test sees no tile of
        // the map and passes, the start tile (5, 3) would lie beyond the mask -> message after three
        // draws and the rough border (D-335).
        let (result, draws) = run((4, 0, 0, 4), 1);
        assert!(format!("{:#}", result.unwrap_err()).contains("outside"));
        assert_eq!(draws, 3 + 4 * 4);
    }

    #[test]
    fn later_clumps_see_earlier_clumps() {
        // Two LAND tiles, two clumps of one `ROCK` tile each on `LAND`: the first paints immediately, the
        // second finds only the other tile - with every seed both end up as ROCK.
        let def = AreaDef {
            put_onto: "LAND".into(),
            texture: "ROCK".into(),
            num_tiles: 2,
            num_clumps: 2,
            ..AreaDef::default()
        };
        for seed in 1..40 {
            let mut grid = filled(2, 1, 1);
            let mut areas = [area(def.clone(), (0, 2, 0, 1))];
            let names = script(&["LAND", "ROCK"]);
            terrain_areas(&mut areas, &mut grid, &names, &[], &mut terrain_rng(seed)).unwrap();
            assert_eq!(grid.tiles, [2, 2], "seed {seed}");
        }
    }

    #[test]
    fn no_terrain_areas_draw_nothing() {
        let mut grid = filled(4, 4, 1);
        let mut rng = terrain_rng(1);
        terrain_areas(&mut [], &mut grid, &script(&[]), &[], &mut rng).unwrap();
        assert_eq!(rng.counts().terrain_areas, 0);
        assert_eq!(grid.tiles, vec![1; 16]);
    }
}
