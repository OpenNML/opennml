//! Snow and rain as in the original (re/weather-and-light, section "Weather"): one particle per
//! tile (`Terrain_InitWeatherParticles` `0x0055F490`), moved and drawn only in visible tiles
//! (`Terrain_DrawWeatherParticlesInRect` `0x00559DE0`).

use std::f32::consts::TAU;

use crate::environment::WeatherKind;
use crate::rand::{MsvcRand, RAND_UNIT};
use crate::terrain::Terrain;
use crate::visibility::TileRect;

/// Seed of the random generator for the weather.
pub const WEATHER_SEED: u32 = 1;

/// Reference frame time of the snow phase in ms.
///
/// Deviation from the original: In the game the phase grows by `0.3 * v` per frame (at 144 FPS
/// almost five times as fast as at 30 FPS); here by `0.3 * v * delta / 33 ms`.
pub const SNOW_PHASE_REFERENCE_MS: f32 = 33.0;

/// Values of `Terrain_SetWeatherType` (`0x0055F6C0`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct KindParams {
    /// Share of active particles.
    density: f32,
    /// Height a particle bounces to after impact.
    top: f32,
    /// Fall speed in units/ms.
    speed_min: f32,
    speed_max: f32,
    /// Largest sway amplitude.
    amplitude: f32,
}

fn kind_params(kind: WeatherKind) -> KindParams {
    match kind {
        WeatherKind::Snow => KindParams {
            density: 1.0,
            top: 2000.0,
            speed_min: 0.05,
            speed_max: 0.08,
            amplitude: 100.0,
        },
        WeatherKind::Rain => KindParams {
            density: 0.5,
            top: 3000.0,
            speed_min: 1.2,
            speed_max: 2.0,
            amplitude: 0.0,
        },
    }
}

/// An active particle (`TerrainTile+0x44`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherParticle {
    /// `+0x04`: height after impact.
    pub top: f32,
    /// `+0x08`: ground height, not clamped to `WATER_LEVEL` (rain falls to the sea floor).
    pub ground: f32,
    /// `+0x0C`: current height.
    pub y: f32,
    /// `+0x10`: fall speed (units/ms).
    pub speed: f32,
    /// `+0x14`, `+0x1C`: tile center.
    pub cx: f32,
    pub cz: f32,
    /// `+0x20`: sway amplitude (snow).
    pub amplitude: f32,
    /// `+0x24`: sway phase.
    pub phase: f32,
}

/// Weather of a level.
#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    kind: WeatherKind,
    dim_x: usize,
    dim_z: usize,
    /// Index `DIM_X * z + x`; `None` = off forever (until the next switch-on).
    particles: Vec<Option<WeatherParticle>>,
}

impl Weather {
    /// Creates the particles like `Terrain_InitWeatherParticles`: tiles x outer, z inner; per
    /// tile a random value for "active", then height, speed, amplitude and phase.
    pub fn new(kind: WeatherKind, terrain: &Terrain, rng: &mut MsvcRand) -> Self {
        let k = kind_params(kind);
        let (dim_x, dim_z) = (terrain.dim_x(), terrain.dim_z());
        let (size_x, size_z) = (terrain.params.size_x as i32, terrain.params.size_z as i32);
        let water = terrain.params.water_level;
        let mut particles = vec![None; dim_x * dim_z];
        for x in 0..dim_x {
            for z in 0..dim_z {
                if k.density <= rng.unit() {
                    continue;
                }
                let cx = (size_x * x as i32 + size_x / 2) as f32;
                let cz = (size_z * z as i32 + size_z / 2) as f32;
                let ground = terrain.ground_height(cx, cz);
                let floor = ground.max(water);
                let y = (k.top - floor) * rng.rand() as f32 * RAND_UNIT + floor;
                let speed =
                    (k.speed_max - k.speed_min) * rng.rand() as f32 * RAND_UNIT + k.speed_min;
                let amplitude = rng.rand() as f32 * k.amplitude * RAND_UNIT;
                let phase = rng.rand() as f32 * 0.000_191_753_45;
                particles[dim_x * z + x] = Some(WeatherParticle {
                    top: k.top,
                    ground,
                    y,
                    speed,
                    cx,
                    cz,
                    amplitude,
                    phase,
                });
            }
        }
        Self {
            kind,
            dim_x,
            dim_z,
            particles,
        }
    }

    pub fn kind(&self) -> WeatherKind {
        self.kind
    }

    /// Number of active particles (snow: all tiles, rain: about half).
    pub fn active_count(&self) -> usize {
        self.particles.iter().flatten().count()
    }

    /// Particle of tile `(x, z)`.
    pub fn particle(&self, x: usize, z: usize) -> Option<&WeatherParticle> {
        self.particles.get(self.dim_x * z + x)?.as_ref()
    }

    fn visible_indices<'a>(&self, rects: &'a [TileRect]) -> impl Iterator<Item = usize> + 'a {
        let (dim_x, dim_z) = (self.dim_x, self.dim_z);
        rects
            .iter()
            .flat_map(move |r| r.tiles(dim_x, dim_z))
            .map(move |(x, z)| dim_x * z + x)
    }

    /// Centers of the active particles in the rectangles (drawing order): snow
    /// `(cx + a * sin phi, y, cz)`, rain `(cx, y, cz)`.
    pub fn positions(&self, rects: &[TileRect]) -> Vec<[f32; 3]> {
        self.visible_indices(rects)
            .filter_map(|i| self.particles[i].as_ref())
            .map(|p| match self.kind {
                WeatherKind::Snow => [p.cx + p.amplitude * p.phase.sin(), p.y, p.cz],
                WeatherKind::Rain => [p.cx, p.y, p.cz],
            })
            .collect()
    }

    /// Moves the active particles of the rectangles by `dt_ms`: `y -= delta * v`, below the ground
    /// back to the top (same x/z); snow sways (see [`SNOW_PHASE_REFERENCE_MS`]).
    /// Particles outside stand still. Result: number of moved particles.
    pub fn advance(&mut self, rects: &[TileRect], dt_ms: f32) -> usize {
        let indices: Vec<usize> = self.visible_indices(rects).collect();
        let mut moved = 0;
        for i in indices {
            let Some(p) = self.particles[i].as_mut() else {
                continue;
            };
            p.y -= dt_ms * p.speed;
            if p.y < p.ground {
                p.y = p.top;
            }
            if self.kind == WeatherKind::Snow {
                p.phase += 0.3 * p.speed * dt_ms / SNOW_PHASE_REFERENCE_MS;
                if p.phase > TAU {
                    p.phase -= TAU;
                }
            }
            moved += 1;
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rand::RAND_MAX;
    use crate::terrain::tests::test_terrain;

    fn all(dim: usize) -> Vec<TileRect> {
        vec![TileRect {
            x0: 0,
            z0: 0,
            size: dim,
        }]
    }

    #[test]
    fn particles_start_within_the_game_ranges() {
        let (terrain, _) = test_terrain(16, |_, _| 1);
        let snow = Weather::new(
            WeatherKind::Snow,
            &terrain,
            &mut MsvcRand::new(WEATHER_SEED),
        );
        assert_eq!(snow.active_count(), 256);
        let p = snow.particle(3, 5).unwrap();
        assert_eq!((p.cx, p.cz, p.top, p.ground), (448.0, 704.0, 2000.0, 0.0));
        for p in snow.particles.iter().flatten() {
            assert!((0.0..=2000.0).contains(&p.y));
            assert!((0.05..=0.08).contains(&p.speed));
            assert!((0.0..=100.0).contains(&p.amplitude));
            assert!((0.0..=TAU).contains(&p.phase));
        }
        let rain = Weather::new(
            WeatherKind::Rain,
            &terrain,
            &mut MsvcRand::new(WEATHER_SEED),
        );
        let active = rain.active_count();
        assert!((100..156).contains(&active), "{active} of 256 active");
        assert!(
            rain.particles.iter().flatten().all(|p| {
                (1.2..=2.0).contains(&p.speed) && p.amplitude == 0.0 && p.top == 3000.0
            })
        );
        // As in the original: rand() = 32767 yields exactly 1.0 in f32, and "density <= R" switches
        // the particle off - so even with snow (density 1.0) a tile occasionally stays empty.
        assert!(1.0 <= RAND_MAX as f32 * RAND_UNIT);
        // First tile with seed 1: rand() = 41 -> active (41/32767 < 0.5), height from 18467.
        let first = rain.particle(0, 0).unwrap();
        assert!((first.y - 3000.0 * 18467.0 * RAND_UNIT).abs() < 1e-2);
    }

    #[test]
    fn particles_fall_and_jump_back_to_the_top() {
        let (terrain, _) = test_terrain(4, |_, _| 1);
        let mut rain = Weather::new(WeatherKind::Rain, &terrain, &mut MsvcRand::new(3));
        let (x, z) = (0..16)
            .map(|i| (i % 4, i / 4))
            .find(|&(x, z)| rain.particle(x, z).is_some())
            .unwrap();
        let before = *rain.particle(x, z).unwrap();
        assert_eq!(rain.advance(&all(4), 10.0), rain.active_count());
        let after = *rain.particle(x, z).unwrap();
        assert!((after.y - (before.y - 10.0 * before.speed)).abs() < 1e-3);
        // Until below the ground: back to 3000.
        let steps = (before.y / (before.speed * 100.0)).ceil() as usize + 1;
        for _ in 0..steps {
            rain.advance(&all(4), 100.0);
        }
        let p = rain.particle(x, z).unwrap();
        assert!(p.y > 2500.0 && p.y <= 3000.0, "{}", p.y);
        assert_eq!((p.cx, p.cz), (before.cx, before.cz));
    }

    #[test]
    fn snow_sways_with_time_and_only_visible_particles_move() {
        let (terrain, _) = test_terrain(4, |_, _| 1);
        let mut snow = Weather::new(
            WeatherKind::Snow,
            &terrain,
            &mut MsvcRand::new(WEATHER_SEED),
        );
        let rect = [TileRect {
            x0: 0,
            z0: 0,
            size: 1,
        }];
        let start = snow.clone();
        let p0 = *snow.particle(0, 0).unwrap();
        let [px, _, _] = snow.positions(&rect)[0];
        assert!((px - (p0.cx + p0.amplitude * p0.phase.sin())).abs() < 1e-4);
        assert_eq!(snow.advance(&rect, 33.0), 1);
        let p1 = snow.particle(0, 0).unwrap();
        let expected = (p0.phase + 0.3 * p0.speed) % TAU;
        assert!((p1.phase - expected).abs() < 1e-5);
        assert_eq!(
            snow.particle(1, 0),
            start.particle(1, 0),
            "invisible: stands still"
        );
        assert_eq!(snow.positions(&rect).len(), 1);
        assert_eq!(snow.positions(&all(4)).len(), 16);
    }

    #[test]
    fn same_steps_give_the_same_particles() {
        let (terrain, _) = test_terrain(8, |_, _| 1);
        let run = |chunks: &[usize]| {
            let mut w = Weather::new(
                WeatherKind::Snow,
                &terrain,
                &mut MsvcRand::new(WEATHER_SEED),
            );
            for &n in chunks {
                for _ in 0..n {
                    w.advance(&all(8), 16.0);
                }
            }
            w
        };
        assert_eq!(run(&[30]), run(&[10, 20]));
        assert_ne!(run(&[30]), run(&[]));
    }
}
