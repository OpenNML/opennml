//! Land and player areas (re/random-map Z.6.3, Z.6.4): steps 4 and 5 of the generator,
//! `create_LandArea` `FUN_00542930` and `create_PlayerAreas` `FUN_00542A60`.

use anyhow::bail;
use nml_formats::rnd::RandomMapScript;
use nml_sim::fx::{Fx, fixed_mul};
use nml_sim::math::{cos_deg, sin_deg};

use crate::area::Area;
use crate::grid::Grid;
use crate::growth::{grow_area, rough_border};
use crate::paint::{disc, paint_mask, texture_number};
use crate::rng::GenRand;

/// `rand() % n` with the number of players `G+0x120` (`SharePlayerLands`). The menu enforces 2 ... 8;
/// a divisor <= 0 occurs only on a direct call -> error message instead of division by zero.
fn roll_player(rng: &mut GenRand, players: i32) -> anyhow::Result<i32> {
    if players <= 0 {
        bail!("random map: SharePlayerLands with {players} players divides by zero");
    }
    Ok(rng.rand() % players)
}

/// The mask carries the low byte of the ID (`FUN_00541190`). If it is 0, the placed tile stays free in
/// the mask and is placed again and again: growth ends only after `numTiles` steps with an endlessly
/// growing edge list (a hang in the original). The reimplementation rejects the area before growing.
/// A low byte of `0xFF` (border mark) does not hang - the tile counts as occupied afterwards - and
/// stays as in the original (grows, but is not painted).
fn check_mask_id(area: &Area) -> anyhow::Result<()> {
    let id = area.def.id;
    if id & 0xFF == 0 {
        bail!(
            "random map: area with ID {id} cannot be created - the low byte 0 would never end the \
             growth"
        );
    }
    Ok(())
}

/// Growth until `sum ≥ numTiles` or no progress; the first step always places the start tile
/// (`do … while`).
fn grow_land(
    mask: &mut [u8],
    area: &mut Area,
    grid: &mut Grid,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let mut sum = 0i32;
    loop {
        let k = i32::from(grow_area(mask, area, grid, rng)?);
        sum = sum.wrapping_add(k);
        if area.def.num_tiles <= sum || k == 0 {
            return Ok(());
        }
    }
}

/// `create_LandArea` `FUN_00542930` (`0x00542930`), step 4: per area in script order center
/// `((z_min + z_max) / 2, (x_min + x_max) / 2)` (C division), ID `rand() % n + 1` with
/// `SharePlayerLands`, otherwise `ID == 0` -> 1 (the ID of player 1, D-330); fresh mask, rough border,
/// growth, paint the mask with `Texture`. The original does not free the edge lists here (no consequence).
pub(crate) fn land_areas(
    areas: &mut [Area],
    grid: &mut Grid,
    script: &RandomMapScript,
    players: i32,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    for area in areas.iter_mut() {
        area.cz = area.z_min.wrapping_add(area.z_max) / 2;
        area.cx = area.x_min.wrapping_add(area.x_max) / 2;
        if area.def.share_player_lands {
            area.def.id = roll_player(rng, players)?.wrapping_add(1);
        } else if area.def.id == 0 {
            area.def.id = 1;
        }
        check_mask_id(area)?;
        let mut mask = vec![0u8; grid.dim_x * grid.dim_z];
        rough_border(&mut mask, area, dim_x, dim_z, rng)?;
        grow_land(&mut mask, area, grid, rng)?;
        paint_mask(grid, &mask, texture_number(script, &area.def.texture));
    }
    Ok(())
}

/// `ToInt(Fixed_Mul(FromInt(h), t))` - first `FromInt(h)`, then sin/cos (`0x00542A60`).
fn offset(h: i32, t: i32) -> i32 {
    Fx(fixed_mul(Fx::from_int(h).0, t)).to_int()
}

/// `create_PlayerAreas` `FUN_00542A60` (`0x00542A60`), step 5, only with a non-empty list.
///
/// Start positions (Z.6.4, D-329): `a = FromInt(rand() % 361)` (0 degrees and 360 degrees twice), step
/// `FromInt(360 / n)`; per area of the list (all blocks one after another, the angle and the
/// ID counter continue across the blocks) an ellipse in the border frame with semi-axes
/// `(x_max − x_min) / 2 − minSize` and `(z_max − z_min) / 2 − minSize`, angle 0 = `+z`; ID
/// `rand() % n + counter` with `SharePlayerLands` (counter stays), otherwise `ID == 0` -> counter,
/// counter + 1.
///
/// Growth: one mask, rough border of the **last** area, rounds (each round one step per area) until
/// `sum ≥ numTiles` of the last area or a round without progress. Afterwards per area a disc `minSize`
/// with its `Texture` and empty the edge list (`FUN_005409A0`), finally paint the mask with the
/// `Texture` of the last area.
pub(crate) fn player_areas(
    areas: &mut [Area],
    grid: &mut Grid,
    script: &RandomMapScript,
    players: i32,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    if areas.is_empty() {
        return Ok(());
    }
    // `0x168 / n`: the list has `n` copies per block, so `n ≥ 1` as long as it is not empty.
    if players <= 0 {
        bail!("random map: player areas with {players} players");
    }
    let mut angle = Fx::from_int(rng.rand() % 361).0;
    let step = Fx::from_int(360 / players).0;
    let mut counter = 1i32;
    for area in areas.iter_mut() {
        let min_size = area.def.min_size;
        let hx = (area.x_max.wrapping_sub(area.x_min) / 2).wrapping_sub(min_size);
        let hz = (area.z_max.wrapping_sub(area.z_min) / 2).wrapping_sub(min_size);
        area.cx = offset(hx, sin_deg(angle)).wrapping_add(area.x_min.wrapping_add(area.x_max) / 2);
        area.cz = offset(hz, cos_deg(angle)).wrapping_add(area.z_max.wrapping_add(area.z_min) / 2);
        if area.def.share_player_lands {
            area.def.id = roll_player(rng, players)?.wrapping_add(counter);
        } else if area.def.id == 0 {
            area.def.id = counter;
            counter = counter.wrapping_add(1);
        }
        angle = angle.wrapping_add(step);
    }
    for area in areas.iter() {
        check_mask_id(area)?;
    }
    let (dim_x, dim_z) = (grid.dim_x as i32, grid.dim_z as i32);
    let mut mask = vec![0u8; grid.dim_x * grid.dim_z];
    let last = areas.len() - 1;
    rough_border(&mut mask, &areas[last], dim_x, dim_z, rng)?;
    let mut sum = 0i32;
    loop {
        let mut round = 0i32;
        for area in areas.iter_mut() {
            round = round.wrapping_add(i32::from(grow_area(&mut mask, area, grid, rng)?));
        }
        sum = sum.wrapping_add(round);
        if !(sum < areas[last].def.num_tiles && round != 0) {
            break;
        }
    }
    for area in areas.iter_mut() {
        disc(
            grid,
            area.cx,
            area.cz,
            area.def.min_size,
            texture_number(script, &area.def.texture),
        );
        area.edges.clear();
        area.cursor = None;
    }
    paint_mask(
        grid,
        &mask,
        texture_number(script, &areas[last].def.texture),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::area::finish_area;
    use crate::paint::tests::texture;
    use crate::rng::Phase;
    use crate::spec::scale_factor;
    use nml_formats::rnd::AreaDef;
    use nml_world::rand::MsvcRand;

    fn area(def: AreaDef, frame: (i32, i32, i32, i32)) -> Area {
        Area {
            x_min: frame.0,
            x_max: frame.1,
            z_min: frame.2,
            z_max: frame.3,
            ..finish_area(&def, 0, 0, 1, scale_factor(0))
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

    fn centers(areas: &[Area]) -> Vec<(i32, i32)> {
        areas.iter().map(|a| (a.cx, a.cz)).collect()
    }

    fn ids(areas: &[Area]) -> Vec<i32> {
        areas.iter().map(|a| a.def.id).collect()
    }

    /// First draw of the seed.
    fn first_draw(seed: u32) -> i32 {
        MsvcRand::new(seed).rand() as i32
    }

    #[test]
    fn start_angle_draws_mod_361_and_steps_360_by_n() {
        // Seed 209 draws 721 -> `% 361` = 360 degrees, seed 99 draws 361 -> 0 degrees: the same position (D-329).
        assert_eq!(first_draw(209) % 361, 360);
        assert_eq!(first_draw(99) % 361, 0);
        let def = AreaDef {
            min_size: 2,
            ..AreaDef::default()
        };
        let run = |seed| {
            let mut areas = vec![area(def.clone(), (0, 64, 0, 64)); 3];
            let mut grid = Grid::new(64, 64);
            let mut rng = GenRand::new(seed);
            rng.set_phase(Phase::PlayerAreas);
            player_areas(&mut areas, &mut grid, &script(&[]), 3, &mut rng).unwrap();
            (centers(&areas), ids(&areas), rng.counts().player_areas)
        };
        let (at_360, ids_360, draws) = run(209);
        let (at_0, ids_0, _) = run(99);
        // Semi-axis `64/2 − 2 = 30`, step `360 / 3 = 120`: 0/360 degrees -> (32 + 0, 32 + 30);
        // 120 degrees -> sin 3547, cos -2048; 240 degrees = -120 degrees -> sin -3547 (rounded down -26).
        assert_eq!(at_360, [(32, 62), (57, 17), (6, 17)]);
        assert_eq!(at_0, at_360);
        // IDs 1, 2, 3 from the counter.
        assert_eq!(ids_360, [1, 2, 3]);
        assert_eq!(ids_0, ids_360);
        // `numTiles 0`: one round, three start tiles: 1 (angle) + 4*64 (border) + 3·2.
        assert_eq!(draws, 1 + 4 * 64 + 3 * 2);
    }

    #[test]
    fn player_tiles_count_against_the_last_area() {
        let first = AreaDef {
            min_size: 1,
            num_tiles: 100,
            texture: "A".into(),
            ..AreaDef::default()
        };
        let last = AreaDef {
            num_tiles: 10,
            texture: "B".into(),
            ..first.clone()
        };
        let mut areas = vec![area(first, (0, 16, 0, 16)), area(last, (0, 16, 0, 16))];
        let mut grid = Grid::new(16, 16);
        let mut rng = GenRand::new(5);
        player_areas(&mut areas, &mut grid, &script(&["A", "B"]), 2, &mut rng).unwrap();
        // Per round each player grows by one tile; ends at sum >= 10 of the last area:
        // 10 together, not 10 each and not 100.
        assert_eq!(areas.iter().map(|a| a.grown).collect::<Vec<_>>(), [5, 5]);
        for id in [1, 2] {
            assert_eq!(grid.id_grid.iter().filter(|&&v| v == id).count(), 5);
        }
        // Finally the texture of the last area paints the whole mask, also over player 1.
        for (i, &id) in grid.id_grid.iter().enumerate() {
            if id != 0 {
                assert_eq!(grid.tiles[i], 2, "tile {i}");
            }
        }
        // The edge lists are emptied (`FUN_005409A0`).
        assert!(
            areas
                .iter()
                .all(|a| a.edges.is_empty() && a.cursor.is_none())
        );
    }

    #[test]
    fn share_player_lands_keeps_the_counter() {
        // Two blocks of two copies each: first `SharePlayerLands`, then without an ID.
        let shared = AreaDef {
            share_player_lands: true,
            ..AreaDef::default()
        };
        let mut areas = vec![
            area(shared.clone(), (0, 16, 0, 16)),
            area(shared, (0, 16, 0, 16)),
            area(AreaDef::default(), (0, 16, 0, 16)),
            area(AreaDef::default(), (0, 16, 0, 16)),
        ];
        let mut grid = Grid::new(16, 16);
        let mut rng = GenRand::new(11);
        player_areas(&mut areas, &mut grid, &script(&[]), 2, &mut rng).unwrap();
        // Angle, then `rand() % 2 + 1` per shared area; the counter stays at 1.
        let mut plain = MsvcRand::new(11);
        plain.rand();
        let s0 = plain.rand() as i32 % 2 + 1;
        let s1 = plain.rand() as i32 % 2 + 1;
        assert_eq!(ids(&areas), [s0, s1, 1, 2]);
        // The angle continues across both blocks: area 2 lies at `a + 360°` like area 0.
        assert_eq!(centers(&areas)[2], centers(&areas)[0]);
        assert_eq!(centers(&areas)[3], centers(&areas)[1]);
        // An ID in the block applies to all copies, the counter does not run.
        let fixed = AreaDef {
            id: 7,
            ..AreaDef::default()
        };
        let mut areas = vec![area(fixed, (0, 16, 0, 16)); 2];
        player_areas(&mut areas, &mut grid, &script(&[]), 2, &mut rng).unwrap();
        assert_eq!(ids(&areas), [7, 7]);
    }

    #[test]
    fn land_area_without_id_gets_one() {
        // Land area without an ID on the left half of a 16x16 map.
        let land = AreaDef {
            num_tiles: 1000,
            texture: "LAND".into(),
            ..AreaDef::default()
        };
        let mut lands = vec![area(land, (0, 8, 0, 16))];
        let mut grid = Grid::new(16, 16);
        let names = script(&["LAND", "PLAYER"]);
        let mut rng = GenRand::new(3);
        rng.set_phase(Phase::LandAreas);
        land_areas(&mut lands, &mut grid, &names, 2, &mut rng).unwrap();
        assert_eq!(lands[0].def.id, 1);
        assert_eq!((lands[0].cx, lands[0].cz), (4, 8));
        assert_eq!(lands[0].grown, 128);
        for z in 0..16 {
            for x in 0..16 {
                let left = x < 8;
                assert_eq!(grid.id(x, z), if left { 1 } else { 0 });
                assert_eq!(grid.tile(x, z), if left { 1 } else { -1 });
            }
        }
        // Seed 71 draws 270 degrees: player 1 starts on the left (1, 8), player 2 on the right (15, 8).
        assert_eq!(first_draw(71) % 361, 270);
        let player = AreaDef {
            min_size: 1,
            num_tiles: 1000,
            texture: "PLAYER".into(),
            ..AreaDef::default()
        };
        let mut players = vec![area(player, (0, 16, 0, 16)); 2];
        let mut rng = GenRand::new(71);
        player_areas(&mut players, &mut grid, &names, 2, &mut rng).unwrap();
        assert_eq!(centers(&players), [(1, 8), (15, 8)]);
        // Player 1 (ID 1) grows into the whole land area (D-330) ...
        for z in 0..16 {
            for x in 0..8 {
                assert_eq!((grid.id(x, z), grid.tile(x, z)), (1, 2), "tile {x}, {z}");
            }
        }
        // ... player 2 avoids it.
        let mut own = 0;
        for z in 0..16 {
            for x in 0..16 {
                if grid.id(x, z) == 2 {
                    assert!(x >= 8, "player 2 on tile {x}, {z}");
                    own += 1;
                }
            }
        }
        assert!(own > 0);
    }

    #[test]
    fn land_area_center_and_shared_id() {
        let def = AreaDef {
            share_player_lands: true,
            ..AreaDef::default()
        };
        // Center `(x_min + x_max) / 2` with C division (toward 0): (-3 + 4) / 2 = 0, (1 + 8) / 2 = 4.
        let mut lands = vec![area(def, (-3, 4, 1, 8))];
        let mut grid = Grid::new(8, 8);
        let mut rng = GenRand::new(2);
        land_areas(&mut lands, &mut grid, &script(&[]), 3, &mut rng).unwrap();
        assert_eq!((lands[0].cx, lands[0].cz), (0, 4));
        // `SharePlayerLands`: `rand() % n + 1` before the border.
        assert_eq!(lands[0].def.id, first_draw(2) % 3 + 1);
        // `numTiles 0`: only the start tile.
        assert_eq!(lands[0].grown, 1);
    }

    #[test]
    fn area_id_with_low_byte_zero_is_rejected() {
        // Mask = ID low byte = 0: the tile would stay free and be placed again endlessly.
        for id in [256, -256, 0x1_0000] {
            let def = AreaDef {
                id,
                num_tiles: i32::MAX,
                ..AreaDef::default()
            };
            let mut grid = Grid::new(8, 8);
            let mut rng = GenRand::new(1);
            let error = land_areas(
                &mut [area(def.clone(), (0, 8, 0, 8))],
                &mut grid,
                &script(&[]),
                2,
                &mut rng,
            )
            .unwrap_err();
            assert!(
                format!("{error:#}").contains(&format!("ID {id}")),
                "{error:#}"
            );
            assert_eq!(grid.id_grid, vec![0; 64]);
            let error = player_areas(
                &mut vec![area(def, (0, 8, 0, 8)); 2],
                &mut grid,
                &script(&[]),
                2,
                &mut rng,
            )
            .unwrap_err();
            assert!(
                format!("{error:#}").contains(&format!("ID {id}")),
                "{error:#}"
            );
            assert_eq!(grid.id_grid, vec![0; 64]);
        }
    }

    #[test]
    fn area_id_with_low_byte_ff_grows_but_is_not_painted() {
        // Mask = `0xFF` = border mark: every tile is placed only once (no hang), but the paint routine
        // skips `0xFF` - the area stands in the ID grid without a texture (as in the original).
        let def = AreaDef {
            id: 255,
            num_tiles: i32::MAX,
            texture: "A".into(),
            ..AreaDef::default()
        };
        let mut lands = vec![area(def, (0, 4, 0, 4))];
        let mut grid = Grid::new(4, 4);
        land_areas(
            &mut lands,
            &mut grid,
            &script(&["A"]),
            2,
            &mut GenRand::new(1),
        )
        .unwrap();
        assert_eq!(lands[0].grown, 16);
        assert_eq!(grid.id_grid, vec![255; 16]);
        assert_eq!(grid.tiles, vec![-1; 16]);
    }

    #[test]
    fn extreme_area_values_never_panic() {
        let base = AreaDef {
            texture: "A".into(),
            ..AreaDef::default()
        };
        let variants = [
            AreaDef {
                num_tiles: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                min_size: 500,
                ..base.clone()
            },
            AreaDef {
                min_size: i32::MIN,
                num_tiles: i32::MIN,
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
                ..base.clone()
            },
            AreaDef {
                left: 99,
                border_roughness: 0,
                num_tiles: 1000,
                ..base.clone()
            },
            AreaDef {
                left: 99,
                top: 99,
                min_size: 500,
                share_player_lands: true,
                num_tiles: i32::MAX,
                ..base.clone()
            },
            AreaDef {
                avoid_other: i32::MAX,
                num_tiles: i32::MAX,
                ..base.clone()
            },
        ];
        let names = script(&["A"]);
        for (i, def) in variants.iter().enumerate() {
            for players in [2, 8] {
                let check = |result: anyhow::Result<()>| {
                    if let Err(error) = result {
                        let text = format!("{error:#}");
                        assert!(text.contains("random map"), "variant {i}: {text}");
                    }
                };
                let finished = finish_area(def, 64, 64, players, scale_factor(0));
                let mut grid = Grid::new(64, 64);
                let mut rng = GenRand::new(i as u32);
                check(land_areas(
                    &mut [finished.clone()],
                    &mut grid,
                    &names,
                    players,
                    &mut rng,
                ));
                let mut copies = vec![finished; players as usize];
                check(player_areas(
                    &mut copies,
                    &mut grid,
                    &names,
                    players,
                    &mut rng,
                ));
            }
        }
    }
}
