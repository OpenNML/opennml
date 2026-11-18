//! Destruction smoke and craters (`Building_SpawnDestructionSmoke` `0x0046C350`, re/buildings 8.5a,
//! D-248; cannon craters from `CannonBall_Impact` `0x004EF7C2`, re/combat-projectiles 3.4, D-283).
//!
//! They are triggered by the presentation events [`SimEvent::BuildingDestroyed`] (in
//! `PropertyBuilding_Destroy` `0x00506300` before sound and explosion, also for the exploding building)
//! and [`SimEvent::Crater`] (splash branch on land). Smoke and craters live only in the host's effect
//! world, not in the savegame: after loading, a restart or a jump in a replay, those of earlier
//! destructions are missing (D-271). Position and rotation of the building craters come from the host's own
//! `MsvcRand` (D-275), never from the state; the original takes the
//! position from the CRT's `rand()` and leaves the rotation uninitialized.

use glam::{Vec2, Vec3};
use opennml_effects::EFFECTS_SEED;
use opennml_effects::attached::{Anchor, UNLIMITED_MS};
use opennml_effects::marks::Footprint;
use opennml_effects::particles::Placement;
use opennml_sim::id::ObjectId;
use opennml_sim::state::SimEvent;
use opennml_world::math::object_world_matrix;
use opennml_world::rand::MsvcRand;

use crate::game::movement_effects::MovementWorld;
use crate::game::sim_host::SimHost;

/// Smoke of a destroyed building (`"data/effects/building_smoke.pef"`, `0x0046C350`).
pub const BUILDING_SMOKE: &str = "data/effects/building_smoke.pef";
/// Crater in the center (`FootprintEffect_Construct(..., 0, 0xD0)`): `bombbuilding1`, r 300.
pub const CRATER_CENTER: u32 = 0xD0;
/// Crater for odd `trunc(x)`: `bombbuilding2`, r 300.
pub const CRATER_ODD: u32 = 0x150;
/// Crater for even `trunc(x)`: `bombbuilding3`, r 300.
pub const CRATER_EVEN: u32 = 0x250;
/// Crater of a cannonball (`0x430`): `bomb.tga`, r 96.
pub const CANNON_CRATER: u32 = 0x430;
/// Number of random craters (`iVar7 = 7`).
const RANDOM_CRATERS: usize = 7;
/// Shrinking of the hull for the random craters (`- 0x96`).
const CRATER_INSET: i32 = 150;

/// Rotation of a crater from the client's randomness: `rand() / 32768 * 2*pi`.
fn random_yaw(rng: &mut MsvcRand) -> f32 {
    rng.rand() as f32 / 32768.0 * std::f32::consts::TAU
}

/// Rectangle of the hull in the world (`0x0046C350`): only the two corners `+half` and `-half` go
/// through `RotY(yaw) * T(position)`, then sorted per axis. With an oblique yaw the rectangle thereby becomes
/// narrower than the rotated hull (kept from the original, re/buildings 8.5a). Returns `(min, max)`
/// with `x` = world x, `y` = world z.
pub fn hull_rect(pos: Vec3, yaw: f32, half: Vec3) -> (Vec2, Vec2) {
    let m = object_world_matrix(pos, Vec3::new(0.0, yaw, 0.0), Vec3::ONE);
    let a = m.transform_point3(half);
    let b = m.transform_point3(-half);
    (
        Vec2::new(a.x.min(b.x), a.z.min(b.z)),
        Vec2::new(a.x.max(b.x), a.z.max(b.z)),
    )
}

/// Positions of the smoke clouds (`0x0046C350`): `x` from `min.x` in tile steps `cell.x`
/// (`GameWorld+0x34`) up to `<= max.x`, per step one cloud at `min.z` and one at `max.z`; then
/// likewise `z` in steps `cell.y` (`+0x3C`) at `min.x` and `max.x`. Height = terrain. The corners
/// occur twice, as in the original. `x`/`y` of the `Vec2` are world x/z.
pub fn smoke_points(
    min: Vec2,
    max: Vec2,
    cell: Vec2,
    height: &dyn Fn(f32, f32) -> f32,
) -> Vec<Vec3> {
    let mut points = Vec::new();
    // Cell sizes are always positive in the world (`.wld` reader); otherwise the loop would run forever.
    if cell.x <= 0.0 || cell.y <= 0.0 {
        return points;
    }
    let mut x = min.x;
    while x <= max.x {
        points.push(Vec3::new(x, height(x, min.y), min.y));
        points.push(Vec3::new(x, height(x, max.y), max.y));
        x += cell.x;
    }
    let mut z = min.y;
    while z <= max.y {
        points.push(Vec3::new(min.x, height(min.x, z), z));
        points.push(Vec3::new(max.x, height(max.x, z), z));
        z += cell.y;
    }
    points
}

/// The 1 + 7 craters of a destroyed building (`0x0046C350`) as `(position, kind, yaw)`: first
/// [`CRATER_CENTER`] at the object position `center` (`GO+0x08`), then seven at
/// `(min.x + rand % max(sx, 1), center.y, min.z + rand % max(sz, 1))` with
/// `sx = max(trunc(max.x - min.x), 0) - 150` (likewise `sz`), kind [`CRATER_ODD`] for odd
/// `trunc(x)`, otherwise [`CRATER_EVEN`]. Random sequence: rotation of the center, then per crater x, z,
/// rotation.
pub fn building_craters(
    center: Vec3,
    min: Vec2,
    max: Vec2,
    rng: &mut MsvcRand,
) -> Vec<(Vec3, u32, f32)> {
    let mut craters = Vec::with_capacity(1 + RANDOM_CRATERS);
    let yaw = random_yaw(rng);
    craters.push((center, CRATER_CENTER, yaw));
    // `CRT_ftol2` truncates; negative spans count as 0.
    let span = |d: f32| (d as i32).max(0) - CRATER_INSET;
    let (sx, sz) = (span(max.x - min.x).max(1), span(max.y - min.y).max(1));
    for _ in 0..RANDOM_CRATERS {
        let x = (rng.rand() as i32 % sx) as f32 + min.x;
        let z = (rng.rand() as i32 % sz) as f32 + min.y;
        let kind = if (x as i32) % 2 != 0 {
            CRATER_ODD
        } else {
            CRATER_EVEN
        };
        let yaw = random_yaw(rng);
        craters.push((Vec3::new(x, center.y, z), kind, yaw));
    }
    craters
}

/// Smoke and craters of the destructions and cannonballs of a map.
#[derive(Debug)]
pub struct DestructionMarks {
    /// Position and rotation of the craters (in the original CRT `rand()` or uninitialized, D-275, D-283).
    rng: MsvcRand,
}

impl Default for DestructionMarks {
    fn default() -> Self {
        Self {
            rng: MsvcRand::new(EFFECTS_SEED),
        }
    }
}

impl DestructionMarks {
    /// Start over (loading, restart, change of `ParticlesQuality`): randomness back to the initial value,
    /// so that check images are repeatable. Smoke and craters lying around go with the effect world
    /// (`EffectWorld::clear` of the caller).
    pub fn reset(&mut self) {
        self.rng = MsvcRand::new(EFFECTS_SEED);
    }

    /// Evaluate the events of a frame: [`SimEvent::BuildingDestroyed`] -> smoke and 1 + 7
    /// craters, [`SimEvent::Crater`] -> cannon crater [`CANNON_CRATER`] at `pos` with a random rotation.
    /// At any `quality` (also 0, as in the original); it only determines the lifetime of the
    /// craters (`Footprint::new`).
    pub fn apply(
        &mut self,
        events: &[SimEvent],
        sim: &SimHost,
        world: &mut dyn MovementWorld,
        quality: u8,
    ) {
        for event in events {
            match event {
                SimEvent::BuildingDestroyed { object } => {
                    self.building(*object, sim, world, quality);
                }
                SimEvent::Crater { pos } => {
                    let yaw = random_yaw(&mut self.rng);
                    world.assets().0.add_footprint(Footprint::new(
                        Vec3::from(*pos),
                        yaw,
                        0,
                        CANNON_CRATER,
                        quality,
                    ));
                }
                _ => {}
            }
        }
    }

    /// `Building_SpawnDestructionSmoke` (`0x0046C350`): hull = half head hull of the `.rdo`
    /// (`ObjectRenderer` vf+0x30 = `Rules::building_shapes[guid].half`), position `GO+0x08`
    /// (`world_pos`, not yet sunk at destruction), yaw `GO+0x158`. If the object or
    /// its shape is missing, nothing is created.
    fn building(
        &mut self,
        object: ObjectId,
        sim: &SimHost,
        world: &mut dyn MovementWorld,
        quality: u8,
    ) {
        let Some(o) = sim.state.object(object, false) else {
            log::debug!("Destruction {object:?}: object missing, no smoke");
            return;
        };
        let Some(shape) = sim.rules.building_shapes.get(&o.guid) else {
            log::debug!("Destruction {object:?}: no shape for {}, no smoke", o.guid);
            return;
        };
        let center = Vec3::from(o.world_pos);
        let (min, max) = hull_rect(center, o.rotation[1], Vec3::from(shape.half));
        let cell = sim.rules.map.cell;
        let cell = Vec2::new(cell[0] as f32, cell[2] as f32);
        let points = {
            let height = |x: f32, z: f32| world.ground_height(x, z);
            smoke_points(min, max, cell, &height)
        };
        let (effects, vfs, models) = world.assets();
        for position in points {
            // `AttachedEffect_Construct(world, path, 0x7FFFFFFF, 0, 1, 0)`, `SetPosition`,
            // `GameWorld_AddEffect`: fixed world effect, yaw 0, scale 1.
            let anchor = Anchor::Fixed(Placement { position, yaw: 0.0 });
            if effects
                .add_game_effect(
                    vfs,
                    models,
                    BUILDING_SMOKE,
                    anchor,
                    None,
                    1.0,
                    0,
                    UNLIMITED_MS,
                )
                .is_none()
            {
                log::warn!("effect {BUILDING_SMOKE} not loadable");
                break;
            }
        }
        for (pos, kind, yaw) in building_craters(center, min, max, &mut self.rng) {
            effects.add_footprint(Footprint::new(pos, yaw, 0, kind, quality));
        }
    }
}

#[cfg(test)]
mod tests;
