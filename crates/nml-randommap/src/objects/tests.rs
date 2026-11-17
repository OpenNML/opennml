use nml_sim::footprint::generator_world_matrix;
use nml_world::rand::MsvcRand;

use super::*;
use crate::checks::tiles_in_map;
use crate::geometry::ground_rect;
use crate::place::tests::{DIM, MODEL, S, World, block, with_textures};

/// A passed attempt at the tile corner `(tx, tz)` with rectangle `rect`.
fn attempt_at(tx: i32, tz: i32, rect: [i32; 4]) -> Attempt {
    Attempt {
        position: [tx as f32 * S, 0.0, tz as f32 * S],
        tile: (tx, tz),
        rect,
    }
}

// QUIRK D-361: kept
#[test]
// The check values are exact multiples of 1/4096 and exactly representable as `f32`.
#[allow(clippy::excessive_precision)]
fn random_rotation_uses_pi_12867() {
    // D-361: `PI` = 12867, `2·PI` = 25734; fixed point with 12 bits.
    assert_eq!(PI, 12867);
    assert_eq!(random_rotation(90), 6433.0 / 4096.0);
    assert_eq!(random_rotation(0), 0.0);
    assert_eq!(random_rotation(1), 0.01220703125);
    assert_eq!(random_rotation(90), 1.570556640625);
    assert_eq!(random_rotation(180), 3.141357421875);
    assert_eq!(random_rotation(360), 6.28271484375);
}

// QUIRK D-336: kept
#[test]
fn rotation_applies_to_the_next_object_of_the_type() {
    // D-336: the first object of a type carries the project rotation, every further one the rotation
    // drawn for its predecessor (R7 after placing); a type without `OBJ_RANDOM_ROT` draws nothing.
    let mut w = World::new();
    let po = w.project.objects.get_mut(&MODEL).unwrap();
    po.random_rot = true;
    po.default_rotation = [0.25, 0.5, 0.75];
    let other = *w.project.objects.keys().find(|&&g| g != MODEL).unwrap();
    let run = w.run_with(7, None, |ctx, rng| {
        set_object(ctx, MODEL, &attempt_at(1, 1, [1, 1, 1, 1]), 0, rng);
        assert_eq!(ctx.rotation(&ctx.project.objects[&MODEL])[0], 0.25);
        set_object(ctx, other, &attempt_at(2, 2, [2, 2, 2, 2]), 9, rng);
        set_object(ctx, MODEL, &attempt_at(3, 3, [3, 3, 3, 3]), -1, rng);
        set_object(ctx, MODEL, &attempt_at(4, 4, [4, 4, 4, 4]), 7, rng);
        // The table keeps the most recently drawn rotation for the next object; X/Z stay.
        let rot = ctx.rotation(&ctx.project.objects[&MODEL]);
        let mut replay = MsvcRand::new(7);
        let degs: Vec<i32> = (0..3).map(|_| replay.rand() as i32 % 361).collect();
        assert_eq!(rot, [0.25, random_rotation(degs[2]), 0.75]);
        Ok(())
    });
    run.result.unwrap();
    let mut replay = MsvcRand::new(7);
    let degs: Vec<i32> = (0..3).map(|_| replay.rand() as i32 % 361).collect();
    let rotations: Vec<f32> = run.objects.iter().map(|o| o.rotation_y).collect();
    assert_eq!(
        rotations,
        [0.5, 0.0, random_rotation(degs[0]), random_rotation(degs[1])]
    );
    // R7 per object of the rotated type, none for the other.
    assert_eq!(run.draws, 3);
    // Owner outside 0-7 -> 8; slots only for 0-7.
    let owners: Vec<i32> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, [0, 8, 8, 7]);
    let mut slots = [false; 8];
    slots[0] = true;
    slots[7] = true;
    assert_eq!(run.slots_used, slots);
}

// QUIRK D-337: kept
#[test]
fn set_object_blocks_the_rect_unconditionally() {
    // `RandomMapObject_Place` `0x004EC842`: `SetRect` also over already blocked tiles; the tiles
    // outside the map are omitted. Unknown GUID: `Place` fails without effect.
    let mut w = World::new().only_free(&[(0, 0), (1, 0)]);
    let run = w.run_with(1, None, |ctx, rng| {
        set_object(ctx, MODEL, &attempt_at(0, 0, [-1, -1, 1, 1]), 8, rng);
        set_object(ctx, u32::MAX - 1, &attempt_at(5, 5, [5, 5, 5, 5]), 0, rng);
        assert!(!ctx.block.is_free([0, 0, 0, 0]));
        assert!(!ctx.block.is_free([1, 0, 1, 0]));
        assert!(ctx.block.cells().iter().all(|&c| c == 1));
        Ok(())
    });
    run.result.unwrap();
    assert_eq!(run.objects.len(), 1);
    assert_eq!(run.slots_used, [false; 8]);
    assert_eq!(run.draws, 0);
}

// QUIRK D-336, D-362: kept
#[test]
fn ground_texture_uses_the_new_rotation() {
    // D-362/D-336: `GroundTexture` computes with the new rotation after R7. Seed 1 draws 41 -> 41 degrees:
    // the rotated crate reaches into the neighboring tile, the rectangle of the old rotation does not.
    let mut w = with_textures(World::new());
    w.project.objects.get_mut(&MODEL).unwrap().random_rot = true;
    let run = w.run_with(1, None, |ctx, rng| {
        let po = ctx.project.objects[&MODEL].clone();
        let geo = ctx.geometry(&po.model).unwrap();
        let position = [3.0 * S, 0.0, 3.0 * S];
        let old_rect = ground_rect(&geo, &generator_world_matrix(position, [0.0; 3]), 128);
        let a = Attempt {
            position,
            tile: (3, 3),
            rect: old_rect,
        };
        set_object(ctx, MODEL, &a, 8, rng);
        let def = PlaceObjectDef {
            ground_texture: "F".into(),
            ..block("A", 1, 1)
        };
        ground_texture(ctx, &def, &po, &geo, &a);
        let new_rot = ctx.rotation(&po);
        assert_eq!(new_rot[1], random_rotation(41));
        let new_rect = ground_rect(&geo, &generator_world_matrix(position, new_rot), 128);
        assert_ne!(new_rect, old_rect);
        let painted: Vec<(usize, usize)> = (0..DIM)
            .flat_map(|x| (0..DIM).map(move |z| (x, z)))
            .filter(|&(x, z)| ctx.grid.tile(x, z) == 2)
            .collect();
        assert_eq!(
            painted,
            tiles_in_map(new_rect, DIM, DIM).collect::<Vec<_>>()
        );
        // The block map carries the rectangle of the old rotation (time of placement).
        assert!(!ctx.block.is_free(old_rect));
        Ok(())
    });
    run.result.unwrap();
    assert_eq!(run.draws, 1);
    assert_eq!(MsvcRand::new(1).rand() % 361, 41);
}

// QUIRK D-358: kept
#[test]
fn ground_texture_without_name_paints_nothing_and_unknown_name_paints_minus_one() {
    // `0x005456E7…0x005456F5`: empty name -> no call; unknown name -> `FUN_00541710` -1.
    let mut w = with_textures(World::new());
    let run = w.run_with(1, None, |ctx, _| {
        let po = ctx.project.objects[&MODEL].clone();
        let geo = ctx.geometry(&po.model).unwrap();
        let a = attempt_at(2, 2, [2, 2, 2, 2]);
        ground_texture(ctx, &block("A", 1, 1), &po, &geo, &a);
        assert!(ctx.grid.tiles.iter().all(|&t| t == 1));
        let def = PlaceObjectDef {
            ground_texture: "UNKNOWN".into(),
            ..block("A", 1, 1)
        };
        ground_texture(ctx, &def, &po, &geo, &a);
        assert_eq!(ctx.grid.tile(2, 2), -1);
        assert_eq!(ctx.grid.tiles.iter().filter(|&&t| t == -1).count(), 1);
        Ok(())
    });
    run.result.unwrap();
}

// QUIRK D-362: kept
#[test]
fn placers_paint_ground_texture_after_setting() {
    // General placer with `GroundTexture F`: every placed object has its tile painted
    // (crate 0 ... 100 without rotation: rectangle = tile of the position).
    let mut w = with_textures(World::new());
    let def = PlaceObjectDef {
        ground_texture: "F".into(),
        ..block("A", 3, 1)
    };
    let run = w.place(&def, 1, None);
    run.result.unwrap();
    assert_eq!(run.objects.len(), 3);
    let mut painted: Vec<usize> = run
        .objects
        .iter()
        .map(|o| DIM * (o.position[2] / S) as usize + (o.position[0] / S) as usize)
        .collect();
    painted.sort_unstable();
    let tiles: Vec<usize> = (0..DIM * DIM).filter(|&i| run.tiles[i] == 2).collect();
    assert_eq!(tiles, painted);
}
