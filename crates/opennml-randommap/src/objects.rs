//! Placing an object and the object step in the generator (re/random-map Z.7.4, ZK-b.4; quirks D-336,
//! D-337, D-349, D-355, D-361, D-362).
//!
//! - [`set_object`] = `FUN_0053FD00`: `RandomMapObject_Place` (`0x004EC660`) and `FUN_005473B0`,
//!   then with `OBJ_RANDOM_ROT ≠ 0` the random rotation R7 (re/random-map ZK-b.3) into the project
//!   object (here the rotation table [`ObjectCtx::rotations`]).
//! - [`ground_texture`] = `FUN_00543260`: the three placers call it themselves after placing, only
//!   with a non-empty `GroundTexture` of the block, with the rotation that applies **afterwards** (D-362).
//! - The object step `FUN_005468D0` (step 13) is called by `Generator::object_step` with its own
//!   blocking map.

use opennml_formats::prj::ProjectObject;
use opennml_formats::rnd::PlaceObjectDef;
use opennml_sim::footprint::generator_world_matrix;
use opennml_sim::fx::{Fx, fixed_div, fixed_mul};

use crate::checks::{Attempt, ObjectCtx, tiles_in_map};
use crate::geometry::{ModelGeometry, ground_rect};
use crate::paint::texture_number;
use crate::rng::GenRand;

/// `PI` of the game in fixed point (`[0x00672E6C]` = `Fixed_FromFloat(3.14159274f)` = 12867,
/// truncated; static initializer `0x00612A60`, D-361).
pub(crate) const PI: i32 = 12867;

/// Owner "neutral" (`RandomMapObject_Place`: owner not in 0...7 -> 8); also the owner of the objects of the
/// general placer (`PUSH 0x8` `0x005456CC`) and of `Fill` (`place.rs`).
pub(crate) const NEUTRAL: i32 = 8;

/// A placed object (`RandomMapObject`, `FUN_005474B0`): position with y = 0 (only the world sets the
/// height), rotation about the vertical axis at placement time (`+0x54`, the project rotation
/// **before** the random rotation, D-336), owner 0...7 or 8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapObject {
    pub guid: u32,
    pub position: [f32; 3],
    pub rotation_y: f32,
    pub owner: i32,
}

/// Random rotation from `deg = rand() % 361` (`0x0053FD9D…0x0053FDF0`):
/// `ToFloat(Mul(Div(FromInt(deg), FromInt(360)), Mul(PI, FromInt(2))))`, argument order of
/// `Fixed_Mul`/`Fixed_Div` as in the original (D-361: 360 degrees -> 6.28271484375).
pub(crate) fn random_rotation(deg: i32) -> f32 {
    let two_pi = fixed_mul(PI, Fx::from_int(2).0);
    let part = fixed_div(Fx::from_int(deg).0, Fx::from_int(360).0);
    Fx(fixed_mul(part, two_pi)).to_f32()
}

/// Placing a passed attempt (`FUN_0053FD00`, hook [`ObjectCtx::set_object`]):
///
/// 1. `RandomMapObject_Place` (`0x004EC660`): looks up the project object (if it is missing, placing
///    fails without any effect - unreachable in map generation, the placer just found it),
///    owner not in 0...7 -> 8, footprint rectangle **unconditionally** into the blocking map (`0x004EC842`).
/// 2. `FUN_005473B0`: owner < 8 occupies the slot (`RM+0x50`), object to the end of the list with the
///    rotation at placement time.
/// 3. `OBJ_RANDOM_ROT ≠ 0` (`0x0053FD96`): R7 `deg = rand() % 361`, the new rotation applies from the
///    **next** object of this type (D-336).
///
/// A model that cannot be loaded does not reach placing: the placer rejects it already at the
/// cache instance (`FUN_0057DB80`, D-355).
pub(crate) fn set_object(
    ctx: &mut ObjectCtx<'_>,
    guid: u32,
    attempt: &Attempt,
    owner: i32,
    rng: &mut GenRand,
) {
    let project = ctx.project;
    let Some(po) = project.objects.get(&guid) else {
        return;
    };
    let owner = if (0..8).contains(&owner) {
        owner
    } else {
        NEUTRAL
    };
    ctx.block.set(attempt.rect);
    if owner < 8 {
        ctx.slots_used[owner as usize] = true;
    }
    let rotation_y = ctx.rotation(po)[1];
    ctx.objects.push(MapObject {
        guid,
        position: attempt.position,
        rotation_y,
        owner,
    });
    if po.random_rot {
        // `CDQ; IDIV 0x169`: `rand()` is not negative.
        let deg = rng.rand() % 361;
        ctx.rotations.insert(guid, random_rotation(deg));
    }
}

/// `GroundTexture` (`FUN_00543260`, from the placers `0x00545715`, `0x00545156`, `0x00545944`
/// after placing, only with a non-empty name `+0x228`): footprint rectangle from the world matrix
/// at the position of the attempt with the rotation **now** in the project object (after R7, D-362),
/// `blocking01` otherwise file hull of the cache instance; all tiles of it in the map get the
/// texture number of the name (`FUN_00541710`, unknown -> -1), immediately visible to later
/// `PutOnto` checks.
pub(crate) fn ground_texture(
    ctx: &mut ObjectCtx<'_>,
    def: &PlaceObjectDef,
    po: &ProjectObject,
    geo: &ModelGeometry,
    attempt: &Attempt,
) {
    if def.ground_texture.is_empty() {
        return;
    }
    let rot = ctx.rotation(po);
    let rect = ground_rect(
        geo,
        &generator_world_matrix(attempt.position, rot),
        ctx.tile_size as i32,
    );
    let number = texture_number(ctx.script, &def.ground_texture);
    let (dim_x, dim_z) = (ctx.grid.dim_x, ctx.grid.dim_z);
    for (x, z) in tiles_in_map(rect, dim_x, dim_z) {
        ctx.grid.set_tile(x, z, number);
    }
}

#[cfg(test)]
mod tests;
