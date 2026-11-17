//! Particle emitter like `ParticleEmitter_Update` (`0x005952D0`), `ParticleEmitter_SpawnParticle`
//! (`0x005946A0`), `ParticleEmitter_UpdateParticle` (`0x00594E00`) and `ParticleEmitter_Render`
//! (`0x00595470`), pseudocode in re/effects. Times in seconds. With a target
//! (`Effect_SetTarget` `0x0057BBC0`) the particles fly ballistically to the target point
//! (re/effects, section "Implementation of the targeting mode").

use std::sync::Arc;

use glam::{Mat4, Vec3};
use nml_formats::pef::ParticleDesc;
use nml_world::math::{game_rotation_y, game_rotation_z};
use nml_world::rand::MsvcRand;

use crate::output::{
    EffectBlend, EffectFrame, EffectSpace, EffectVertex, QUAD_CORNERS, QUAD_UVS, d3d_color,
};

/// Maximum count of simultaneous particles per emitter (pool of `0x1A0`-byte entries).
pub const MAX_PARTICLES: usize = 100;

/// `Rand_SignedHundredths` (`0x005A0630`): `1 - (rand() % 200) / 100` in [-0.99; 1].
pub fn rand_signed(rng: &mut MsvcRand) -> f32 {
    1.0 - (rng.rand() % 200) as f32 / 100.0
}

/// `Rand_UnitHundredths` (`0x005A0660`): `(rand() % 100) / 100` in [0; 0.99].
pub fn rand_unit(rng: &mut MsvcRand) -> f32 {
    (rng.rand() % 100) as f32 / 100.0
}

/// Degrees -> radians with the constant of the game.
pub const DEG_TO_RAD: f32 = 0.017_453_292;

/// Position of an effect for emitters and billboards: position (world) and rotation about y.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub position: Vec3,
    pub yaw: f32,
}

/// `p * RotY(phi)` of the game (row vector).
pub fn rotate_y(p: Vec3, yaw: f32) -> Vec3 {
    game_rotation_y(yaw) * p
}

/// Piecewise linear curve of a particle (color, alpha or size): segment and value.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Track<const N: usize> {
    value: [f32; N],
    segment: usize,
}

/// Control times, values and slopes of a curve (`ParticleEmitterDesc_Compute*Slopes`).
#[derive(Debug, Clone, PartialEq)]
struct Keys<const N: usize> {
    times: [f32; 4],
    values: [[f32; N]; 4],
    slopes: [[f32; N]; 3],
}

impl<const N: usize> Keys<N> {
    fn new(points: [(f32, [f32; N]); 4]) -> Self {
        let times = points.map(|p| p.0);
        let values = points.map(|p| p.1);
        let slopes = std::array::from_fn(|i| {
            std::array::from_fn(|k| (values[i + 1][k] - values[i][k]) / (times[i + 1] - times[i]))
        });
        Self {
            times,
            values,
            slopes,
        }
    }

    fn start(&self) -> Track<N> {
        Track {
            value: self.values[0],
            segment: 0,
        }
    }

    /// `Particle_UpdateColor/Alpha/Size` (`0x005930C0`/`0x00593220`/`0x005932D0`): segment by
    /// age, on a change the control value, then `+ dt * slope`, limited to 0...255.
    fn update(&self, track: &mut Track<N>, age: f32, dt: f32) {
        let segment = if age < self.times[1] {
            0
        } else if age < self.times[2] {
            1
        } else {
            2
        };
        if segment != track.segment {
            track.segment = segment;
            track.value = self.values[segment];
        }
        for (v, s) in track.value.iter_mut().zip(self.slopes[segment]) {
            *v = (*v + dt * s).clamp(0.0, 255.0);
        }
    }
}

/// The three curves of an emitter (color, alpha, size).
#[derive(Debug, Clone, PartialEq)]
struct Curves {
    colors: Keys<3>,
    alphas: Keys<1>,
    sizes: Keys<1>,
}

impl Curves {
    fn of(d: &ParticleDesc) -> Self {
        Self {
            colors: Keys::new(d.colors.map(|c| (c[0], [c[1], c[2], c[3]]))),
            alphas: Keys::new(d.alphas.map(|a| (a[0], [a[1]]))),
            sizes: Keys::new(d.sizes.map(|s| (s[0], [s[1]]))),
        }
    }
}

/// Target flight of a particle (`Particle+0x150` acceleration, `+0x15C` start velocity,
/// `+0x168` start position), set in `SpawnParticle` (`0x005946A0`) if the effect has a target.
#[derive(Debug, Clone, PartialEq)]
struct Flight {
    p0: Vec3,
    v0: Vec3,
    accel: Vec3,
    /// Curves at the start: the original copies control points and slopes into the particle
    /// (`0x005946A0`, read by `Particle_UpdateColor` `0x005930C0`), so a later
    /// `SetLifeForTarget` stretches only new particles. Reproduced only in targeting mode; without a
    /// target the particles read the curves of the emitter as before.
    curves: Arc<Curves>,
}

/// A particle.
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    /// Position (world; with `MOVEPARTICLESWITHOBJECT` without a target relative to the effect position).
    pub position: Vec3,
    velocity: Vec3,
    gravity: Vec3,
    age: f32,
    life: f32,
    color: Track<3>,
    alpha: Track<1>,
    size: Track<1>,
    rotation: f32,
    rotation_rate: f32,
    orbit_angle: f32,
    orbit_rate: f32,
    orbit_offset: Vec3,
    center: Vec3,
    /// Target flight (only with a target at the start).
    flight: Option<Flight>,
}

impl Particle {
    pub fn size(&self) -> f32 {
        self.size.value[0]
    }

    pub fn alpha(&self) -> f32 {
        self.alpha.value[0]
    }

    pub fn color(&self) -> [f32; 3] {
        self.color.value
    }

    pub fn rotation(&self) -> f32 {
        self.rotation
    }
}

/// Particle emitter: description (scalable copy), state and pool.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleEmitter {
    /// Copy of the description; `SPREAD`, `GRAVITY`, `SPEED`, `ATTRACT` and the `SIZE` values
    /// are multiplied by the effect scaling (`ParticleEmitter_SetScale` `0x00593A20`),
    /// `LIFE` and the control times by the flight time (`ParticleEmitter_SetLifeForTarget`
    /// `0x00593EF0`).
    desc: ParticleDesc,
    curves: Arc<Curves>,
    active: bool,
    emitter_time: f32,
    last: f32,
    last_burst: f32,
    carry: f32,
    first: bool,
    /// The effect has a target (`Effect+0x15C`, sticky): `draw` then does not add the effect position
    /// even with `MOVEPARTICLESWITHOBJECT` (`0x00595470`).
    has_target: bool,
    /// Active particles, newest first.
    particles: Vec<Particle>,
}

impl ParticleEmitter {
    pub fn new(desc: &ParticleDesc) -> Self {
        Self {
            desc: desc.clone(),
            curves: Arc::new(Curves::of(desc)),
            active: false,
            emitter_time: 0.0,
            last: 0.0,
            last_burst: 0.0,
            carry: 0.0,
            first: true,
            has_target: false,
            particles: Vec::new(),
        }
    }

    fn rebuild_keys(&mut self) {
        self.curves = Arc::new(Curves::of(&self.desc));
    }

    pub fn desc(&self) -> &ParticleDesc {
        &self.desc
    }

    /// `Start(ms)` (`0x005939E0`).
    pub fn start(&mut self, ms: u32) {
        self.active = true;
        self.emitter_time = ms as f32 / 1000.0;
        self.last = 0.0;
        self.last_burst = 0.0;
        self.carry = 0.0;
        self.first = true;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// `StopAndCount` (`0x00593A10`): deactivate, number of living particles.
    pub fn stop_and_count(&mut self) -> usize {
        self.active = false;
        self.particles.len()
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// `ParticleEmitter_SetScale` (`0x00593A20`) with the factor new/old.
    pub fn scale(&mut self, factor: f32) {
        let d = &mut self.desc;
        d.spread *= factor;
        d.gravity = d.gravity.map(|g| g * factor);
        d.speed *= factor;
        d.attract *= factor;
        for size in &mut d.sizes {
            size[1] = (size[1] * factor).min(255.0);
        }
        self.rebuild_keys();
    }

    /// `ParticleEmitter_SetLifeForTarget` (`0x00593EF0`): factor `t / LIFE` (current life,
    /// not the template: repeated calls continue relative to it as in the original); all four
    /// control times of each curve (index 0 included) times the factor, `LIFE = t`, slopes anew.
    /// The rotation rate still arises at the start from `SPIN / LIFE` (`0x005938F0`).
    pub fn set_life_for_target(&mut self, t: f32) {
        let d = &mut self.desc;
        let factor = t / d.life;
        for c in &mut d.colors {
            c[0] *= factor;
        }
        for a in &mut d.alphas {
            a[0] *= factor;
        }
        for s in &mut d.sizes {
            s[0] *= factor;
        }
        d.life = t;
        self.rebuild_keys();
    }

    /// `Update(t)`: emission and movement up to the effect time `t` (seconds); `target` = target point
    /// of the effect (`Effect+0x160`, as long as `+0x15C` is set).
    pub fn update(
        &mut self,
        t: f32,
        placement: Placement,
        target: Option<Vec3>,
        rng: &mut MsvcRand,
    ) {
        self.has_target |= target.is_some();
        if self.last == 0.0 {
            self.last = t;
        }
        if self.last_burst == 0.0 {
            self.last_burst = t;
        }
        let dt = t - self.last;
        self.emitter_time += dt;
        let d = &self.desc;
        if !d.looping && self.emitter_time >= d.start + d.duration {
            self.active = false;
        }
        let mut n = 0;
        if self.active && self.emitter_time >= d.start {
            if d.burst {
                if t - self.last_burst > d.burst_delay || self.first {
                    n = d.emit.max(0) as usize;
                    self.last_burst = t;
                    self.first = false;
                }
            } else {
                let a = d.emit as f32 * dt + self.carry;
                let whole = a as i32;
                self.carry = a - whole as f32;
                n = whole.max(0) as usize;
                if n == 0 && self.first {
                    n = 1;
                }
                self.first = false;
            }
        }
        let mut particles = std::mem::take(&mut self.particles);
        particles.retain_mut(|p| self.update_particle(p, dt, placement));
        let free = MAX_PARTICLES.saturating_sub(particles.len());
        for _ in 0..n.min(free) {
            let p = self.spawn(placement, target, rng);
            particles.insert(0, p);
        }
        self.particles = particles;
        self.last = t;
    }

    /// `SpawnParticle` (`0x005946A0`).
    fn spawn(&self, placement: Placement, target: Option<Vec3>, rng: &mut MsvcRand) -> Particle {
        let d = &self.desc;
        let phi = placement.yaw;
        let r1 = rand_signed(rng);
        let r2 = rand_signed(rng);
        let q = Vec3::new(r2, 0.0, r1);
        let mut yaw = d.yaw + rand_signed(rng) * d.yaw_var;
        let pitch = d.pitch + rand_signed(rng) * d.pitch_var;
        let mut gravity = Vec3::from(d.gravity);
        let mut p = Vec3::from(d.pos) + q * d.spread;
        let mut center = Vec3::from(d.pos);
        if phi != 0.0 {
            yaw += phi;
            gravity = rotate_y(gravity, phi);
            if p != Vec3::ZERO {
                p = rotate_y(p, phi);
            }
            center = rotate_y(center, phi);
        }
        p += placement.position;
        center += placement.position;
        if d.emitter_type == 2 {
            let a = (rng.rand() % 360) as f32 * DEG_TO_RAD + yaw;
            yaw = a;
            p.x += a.cos() * d.emitter_radius;
            p.z += a.sin() * d.emitter_radius;
        }
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let mut velocity = Vec3::new(-sy * cp, sp, cy * cp) * d.speed;
        if d.burst {
            velocity *= (rand_unit(rng) + 1.0) * 0.5;
        }
        let mut rotation_rate = d.spin / d.life;
        if rotation_rate < 0.0 {
            rotation_rate *= r2;
        }
        // Target term `0x00594C33...0x00594CF0`: `a = (2,2,2) (*) ((target + q*SPREAD) - p0 - v0*T) *
        // (1 / T^2)` (component-wise) with `q` unrotated (only the start offset is rotated by phi) and `p0` after
        // rotation, effect position and ring; `T` = life of the particle.
        let flight = target.map(|z| {
            let life = d.life;
            let life_sq = life * life;
            let reach = z + q * d.spread - p - velocity * life;
            Flight {
                p0: p,
                v0: velocity,
                accel: (Vec3::splat(2.0) * reach) * (1.0 / life_sq),
                curves: Arc::clone(&self.curves),
            }
        });
        if d.move_with_object && target.is_none() {
            p -= placement.position;
            center -= placement.position;
        }
        Particle {
            position: p,
            velocity,
            gravity,
            age: 0.0,
            life: d.life,
            color: self.curves.colors.start(),
            alpha: self.curves.alphas.start(),
            size: self.curves.sizes.start(),
            rotation: 0.0,
            rotation_rate,
            orbit_angle: yaw,
            orbit_rate: d.rot_y / d.life,
            orbit_offset: Vec3::ZERO,
            center,
            flight,
        }
    }

    /// `UpdateParticle` (`0x00594E00`); `false` = died.
    fn update_particle(&self, p: &mut Particle, dt: f32, placement: Placement) -> bool {
        if dt == 0.0 {
            return true;
        }
        p.age += dt;
        if p.age > p.life {
            return false;
        }
        let d = &self.desc;
        let h = dt * d.accel;
        let curves: &Curves = match &p.flight {
            // With a target (`0x00594E84...0x00594F20`): `(p0 + v0*age) + ((1/2 (*) a)*age)*age`;
            // `GRAVITY`, `ACCEL`, `ATTRACT` and `ROTY` have no effect.
            Some(f) => {
                let age = p.age;
                p.position = (f.p0 + f.v0 * age) + ((f.accel * 0.5) * age) * age;
                &f.curves
            }
            None => {
                p.position -= p.orbit_offset;
                p.position += p.velocity * h;
                let gh = p.gravity * (0.5 * h);
                if d.attract != 0.0 {
                    // Original: aims at the absolute point even with MOVEPARTICLESWITHOBJECT.
                    let mut dir = placement.position + Vec3::from(d.pos) - p.position;
                    for (axis, on) in d.attract_axes.iter().enumerate() {
                        if *on == 0 {
                            dir[axis] = 0.0;
                        }
                    }
                    if dir.length() > 0.0 {
                        p.velocity += dir.normalize() * d.attract * h;
                    }
                }
                if p.orbit_rate != 0.0 {
                    p.center += gh;
                    p.orbit_angle += p.orbit_rate * h;
                    let mut r = p.position - p.center;
                    r.y = 0.0;
                    let length = r.length();
                    let (s, c) = p.orbit_angle.sin_cos();
                    p.orbit_offset = Vec3::new(c * length, 0.0, s * length) - r;
                }
                p.velocity += gh;
                p.position += p.orbit_offset;
                &self.curves
            }
        };
        curves.colors.update(&mut p.color, p.age, dt);
        curves.alphas.update(&mut p.alpha, p.age, dt);
        curves.sizes.update(&mut p.size, p.age, dt);
        // Rotation with the RAW `dt` (`0x00595178...0x00595188`: `+0x148 += dt * +0x14C`; `h = dt*ACCEL`
        // lives in a different stack slot), in both modes.
        p.rotation += dt * p.rotation_rate;
        true
    }

    /// `ParticleEmitter_Render` (`0x00595470`): per particle (newest first) a quad in
    /// view space (`view` = view matrix of the camera, world = identity). The effect position is added with
    /// `MOVEPARTICLESWITHOBJECT` only without a target.
    pub fn draw(&self, view: Mat4, placement: Placement, out: &mut EffectFrame) {
        let d = &self.desc;
        let texture = (!d.texture.is_empty()).then(|| d.texture.clone());
        let relative = d.move_with_object && !self.has_target;
        let vertices = self.particles.iter().flat_map(|p| {
            let world = if relative {
                p.position + placement.position
            } else {
                p.position
            };
            let c = view.transform_point3(world);
            let [r, g, b] = p.color.value;
            let color = d3d_color(p.alpha.value[0], r, g, b);
            let s = p.size.value[0];
            let rotation = (p.rotation != 0.0).then(|| game_rotation_z(p.rotation));
            QUAD_CORNERS.iter().zip(QUAD_UVS).map(move |(corner, uv)| {
                let mut corner = Vec3::new(corner[0] * s, corner[1] * s, 0.0);
                if let Some(m) = rotation {
                    corner = m * corner;
                }
                EffectVertex {
                    position: (c + corner).to_array(),
                    color,
                    specular: 0,
                    uv,
                }
            })
        });
        out.push_quads(
            texture,
            EffectBlend::from_blend_alpha(d.blend_alpha),
            EffectSpace::View,
            vertices,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emitter: 10 particles/s from 0 s, 1 s long, life 2 s, without movement and random spread.
    fn desc() -> ParticleDesc {
        ParticleDesc {
            texture: "p.tga".into(),
            duration: 1.0,
            emit: 10,
            life: 2.0,
            accel: 1.0,
            colors: [
                [0.0, 0.0, 0.0, 0.0],
                [1.0, 100.0, 50.0, 0.0],
                [1.5, 100.0, 50.0, 0.0],
                [2.0, 0.0, 0.0, 255.0],
            ],
            alphas: [[0.0, 255.0], [1.0, 255.0], [1.5, 255.0], [2.0, 0.0]],
            sizes: [[0.0, 10.0], [1.0, 20.0], [1.5, 20.0], [2.0, 20.0]],
            ..ParticleDesc::default()
        }
    }

    const AT_ORIGIN: Placement = Placement {
        position: Vec3::ZERO,
        yaw: 0.0,
    };

    /// Steps of `dt` seconds from effect time 0.
    fn run(emitter: &mut ParticleEmitter, steps: usize, dt: f32, rng: &mut MsvcRand) {
        for i in 1..=steps {
            emitter.update(i as f32 * dt, AT_ORIGIN, None, rng);
        }
    }

    #[test]
    fn continuous_emission_carries_fractions_and_stops_after_the_duration() {
        let mut emitter = ParticleEmitter::new(&desc());
        emitter.start(0);
        let mut rng = MsvcRand::new(1);
        // First call: last = t, dt = 0 -> no fraction, but the first particle.
        emitter.update(0.05, AT_ORIGIN, None, &mut rng);
        assert_eq!(emitter.particles().len(), 1);
        // 0.05 s * 10/s = 0.5 per step: a particle every second time.
        emitter.update(0.10, AT_ORIGIN, None, &mut rng);
        assert_eq!(emitter.particles().len(), 1);
        emitter.update(0.15, AT_ORIGIN, None, &mut rng);
        assert_eq!(emitter.particles().len(), 2);
        run(&mut emitter, 30, 0.05, &mut rng);
        // Up to 1.5 s: emission ends at START + DURATION = 1 s (LOOP 0).
        assert!(!emitter.is_active());
        let count = emitter.particles().len();
        assert!((9..=11).contains(&count), "{count}");
    }

    #[test]
    fn bursts_emit_all_at_once_after_the_delay() {
        let mut emitter = ParticleEmitter::new(&ParticleDesc {
            burst: true,
            burst_delay: 0.25,
            looping: true,
            ..desc()
        });
        emitter.start(0);
        let mut rng = MsvcRand::new(1);
        emitter.update(0.1, AT_ORIGIN, None, &mut rng);
        assert_eq!(emitter.particles().len(), 10);
        emitter.update(0.3, AT_ORIGIN, None, &mut rng);
        assert_eq!(emitter.particles().len(), 10);
        emitter.update(0.36, AT_ORIGIN, None, &mut rng);
        assert_eq!(emitter.particles().len(), 20);
        // Pool: at most 100.
        for i in 0..20 {
            emitter.update(0.4 + i as f32 * 0.26, AT_ORIGIN, None, &mut rng);
        }
        assert!(emitter.particles().len() <= MAX_PARTICLES);
    }

    #[test]
    fn curves_follow_their_segments_and_clamp() {
        let mut emitter = ParticleEmitter::new(&desc());
        emitter.start(0);
        let mut rng = MsvcRand::new(1);
        emitter.update(0.01, AT_ORIGIN, None, &mut rng);
        // New particle: control value 0, not moved in this frame.
        let p = &emitter.particles()[0];
        assert_eq!((p.color(), p.alpha(), p.size()), ([0.0; 3], 255.0, 10.0));
        emitter.update(0.51, AT_ORIGIN, None, &mut rng);
        // Age 0.5: segment 0, red 0 + 0.5 * 100.
        let p = emitter.particles().last().unwrap();
        assert!((p.color()[0] - 50.0).abs() < 1e-3, "{:?}", p.color());
        assert!((p.size() - 15.0).abs() < 1e-3);
        emitter.update(1.26, AT_ORIGIN, None, &mut rng);
        // Age 1.25: segment 1 sets control value 1 and runs with slope 0.
        let p = emitter.particles().last().unwrap();
        assert_eq!(p.color(), [100.0, 50.0, 0.0]);
        emitter.update(1.76, AT_ORIGIN, None, &mut rng);
        // Age 1.75: segment 2, blue 0 + 0.5 * 510 = 255 (limited), alpha 255 - 0.5 * 510.
        let p = emitter.particles().last().unwrap();
        assert_eq!(p.color()[2], 255.0);
        assert_eq!(p.alpha(), 0.0);
        // Age > life: died.
        emitter.update(2.2, AT_ORIGIN, None, &mut rng);
        assert!(emitter.particles().iter().all(|p| p.age <= p.life));
    }

    #[test]
    fn particles_move_with_half_gravity_and_accel_as_time_factor() {
        let mut emitter = ParticleEmitter::new(&ParticleDesc {
            emit: 1,
            speed: 10.0,
            pitch: std::f32::consts::FRAC_PI_2,
            gravity: [0.0, -20.0, 0.0],
            accel: 2.0,
            looping: true,
            ..desc()
        });
        emitter.start(0);
        let mut rng = MsvcRand::new(1);
        emitter.update(0.5, AT_ORIGIN, None, &mut rng);
        let start = emitter.particles()[0].position;
        emitter.update(1.0, AT_ORIGIN, None, &mut rng);
        let p = emitter.particles().last().unwrap();
        // h = 0.5 * 2 = 1: position + v * h, then v += 0.5 * g * h.
        assert!(
            (p.position.y - start.y - 10.0).abs() < 1e-3,
            "{:?}",
            p.position
        );
        assert!((p.velocity.y - 0.0).abs() < 1e-3);
    }

    #[test]
    fn rotated_effects_rotate_offsets_and_directions() {
        let d = ParticleDesc {
            emit: 1,
            pos: [10.0, 0.0, 0.0],
            speed: 1.0,
            yaw: 0.0,
            ..desc()
        };
        let mut emitter = ParticleEmitter::new(&d);
        emitter.start(0);
        let placement = Placement {
            position: Vec3::new(100.0, 0.0, 0.0),
            yaw: std::f32::consts::FRAC_PI_2,
        };
        emitter.update(0.1, placement, None, &mut MsvcRand::new(1));
        let p = &emitter.particles()[0];
        // (10, 0, 0) * RotY(90 degrees) = (0, 0, 10); direction yaw = 90 degrees: (-1, 0, 0).
        assert!((p.position - Vec3::new(100.0, 0.0, 10.0)).length() < 1e-3);
        assert!((p.velocity - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn scaling_multiplies_sizes_up_to_255_and_spread() {
        let mut emitter = ParticleEmitter::new(&ParticleDesc {
            spread: 4.0,
            ..desc()
        });
        emitter.scale(20.0);
        assert_eq!(emitter.desc().spread, 80.0);
        assert_eq!(emitter.desc().sizes[0][1], 200.0);
        assert_eq!(emitter.desc().sizes[1][1], 255.0);
    }

    #[test]
    fn particles_draw_as_view_space_quads() {
        let mut emitter = ParticleEmitter::new(&desc());
        emitter.start(0);
        emitter.update(0.1, AT_ORIGIN, None, &mut MsvcRand::new(1));
        let mut frame = EffectFrame::default();
        let view = Mat4::from_translation(Vec3::new(0.0, 0.0, 50.0));
        emitter.draw(view, AT_ORIGIN, &mut frame);
        assert_eq!(frame.quad_count(), 1);
        let v = &frame.vertices;
        // Size 10: corners +/-10 around the center in view space.
        let c = view.transform_point3(emitter.particles()[0].position);
        assert_eq!(v[0].position, [c.x - 10.0, c.y + 10.0, c.z]);
        assert_eq!(v[2].position, [c.x + 10.0, c.y - 10.0, c.z]);
        assert_eq!(v[0].color, 0xFF00_0000);
        assert_eq!((v[0].uv, v[3].uv), ([0.0, 0.0], [0.0, 1.0]));
    }

    #[test]
    fn same_seed_and_steps_give_the_same_particles() {
        let d = ParticleDesc {
            emit: 30,
            spread: 5.0,
            yaw_var: 1.0,
            pitch_var: 1.0,
            speed: 20.0,
            spin: -3.0,
            rot_y: 2.0,
            emitter_type: 2,
            emitter_radius: 7.0,
            looping: true,
            ..desc()
        };
        let simulate = || {
            let mut emitter = ParticleEmitter::new(&d);
            emitter.start(0);
            let mut rng = MsvcRand::new(9);
            run(&mut emitter, 60, 1.0 / 30.0, &mut rng);
            emitter
        };
        assert_eq!(simulate(), simulate());
    }

    /// Effect with one emitter `d` and `MOVEMENTTIME mt`.
    fn instance(d: ParticleDesc, mt: f32) -> crate::effect::EffectInstance {
        let pef = nml_formats::pef::PefFile {
            movement_time: mt,
            components: vec![nml_formats::pef::PefComponent::Particles(d)],
        };
        crate::effect::EffectInstance::new("x.pef", &pef, true, &mut crate::effect::NoAssets)
    }

    fn emitter_of(e: &crate::effect::EffectInstance) -> &ParticleEmitter {
        match &e.components()[0] {
            crate::effect::Component::Particles(p) => p,
            _ => panic!("emitter expected"),
        }
    }

    // QUIRK D-276: kept
    #[test]
    fn set_target_stretches_life_and_key_times() {
        let mut e = instance(desc(), 1.0);
        assert_eq!((e.target(), e.flight_s()), (None, 1.0));
        // Distance 256 at MOVEMENTTIME 1: T = 2 = LIFE, factor 1.
        e.set_target(Vec3::new(256.0, 0.0, 0.0));
        assert_eq!(e.flight_s(), 2.0);
        assert_eq!(e.target(), Some(Vec3::new(256.0, 0.0, 0.0)));
        let p = emitter_of(&e);
        assert_eq!(p.desc().life, 2.0);
        assert_eq!(p.curves.colors.times, [0.0, 1.0, 1.5, 2.0]);
        // Distance 512: T = 4, all control times * 2 (relative to the current life), slopes halved.
        e.set_target(Vec3::new(0.0, 0.0, -512.0));
        assert_eq!(e.flight_s(), 4.0);
        let p = emitter_of(&e);
        assert_eq!(p.desc().life, 4.0);
        assert_eq!(p.desc().colors.map(|c| c[0]), [0.0, 2.0, 3.0, 4.0]);
        assert_eq!(p.desc().alphas.map(|a| a[0]), [0.0, 2.0, 3.0, 4.0]);
        assert_eq!(p.desc().sizes.map(|s| s[0]), [0.0, 2.0, 3.0, 4.0]);
        assert_eq!(p.curves.colors.slopes[0], [50.0, 25.0, 0.0]);
        assert_eq!(p.curves.sizes.slopes[0], [5.0]);
        assert_eq!(p.curves.alphas.slopes[2], [-255.0]);
    }

    // QUIRK D-276: kept
    #[test]
    fn zero_distance_counts_as_128() {
        let mut e = instance(desc(), 1.5);
        e.set_target(Vec3::ZERO);
        assert_eq!(e.flight_s(), 1.5);
        assert_eq!(emitter_of(&e).desc().life, 1.5);
    }

    /// Like `licht03` in `geist_angriff.pef`: straight up, ring 9, without spread.
    fn light() -> ParticleDesc {
        ParticleDesc {
            emit: 1,
            duration: 0.1,
            speed: 67.0,
            pitch: std::f32::consts::FRAC_PI_2,
            emitter_type: 2,
            emitter_radius: 9.0,
            ..desc()
        }
    }

    const ASIDE: Placement = Placement {
        position: Vec3::new(10.0, 5.0, -20.0),
        yaw: 0.4,
    };

    // QUIRK D-276: kept
    #[test]
    fn particles_land_on_the_target_after_the_flight_time() {
        let mut emitter = ParticleEmitter::new(&light());
        emitter.start(0);
        emitter.set_life_for_target(2.0);
        let z = Vec3::new(256.0, 0.0, 0.0);
        let mut rng = MsvcRand::new(1);
        // Steps of 0.25 s (exact in f32): the first particle is 2 s old after 8 steps.
        for i in 1..=9 {
            emitter.update(i as f32 * 0.25, ASIDE, Some(z), &mut rng);
        }
        let p = emitter.particles().last().unwrap();
        assert_eq!(p.age, 2.0);
        assert!((p.position - z).length() < 0.01, "{:?}", p.position);
        emitter.update(2.5, ASIDE, Some(z), &mut rng);
        assert!(emitter.particles().iter().all(|p| p.age <= 2.0));
    }

    // QUIRK D-276: kept
    #[test]
    fn spread_lands_unrotated_around_the_target() {
        // `q * SPREAD` unrotated in the target term, the start position rotated.
        let mut emitter = ParticleEmitter::new(&ParticleDesc {
            spread: 6.0,
            ..light()
        });
        emitter.start(0);
        emitter.set_life_for_target(1.0);
        let z = Vec3::new(-40.0, 30.0, 80.0);
        let mut rng = MsvcRand::new(7);
        let mut replay = rng.clone();
        let r1 = rand_signed(&mut replay);
        let r2 = rand_signed(&mut replay);
        for i in 1..=5 {
            emitter.update(i as f32 * 0.25, ASIDE, Some(z), &mut rng);
        }
        let p = emitter.particles().last().unwrap();
        assert_eq!(p.age, 1.0);
        let landing = z + Vec3::new(r2, 0.0, r1) * 6.0;
        assert!((p.position - landing).length() < 0.01, "{:?}", p.position);
    }

    // QUIRK D-276: kept
    #[test]
    fn target_mode_ignores_gravity_accel_attract_and_roty() {
        let plain = light();
        let busy = ParticleDesc {
            gravity: [0.0, -50.0, 10.0],
            accel: 3.0,
            attract: 20.0,
            attract_axes: [1, 1, 1],
            rot_y: 4.0,
            move_with_object: true,
            ..light()
        };
        let z = Vec3::new(100.0, 0.0, 50.0);
        let fly = |d: &ParticleDesc| {
            let mut emitter = ParticleEmitter::new(d);
            emitter.start(0);
            emitter.set_life_for_target(1.5);
            let mut rng = MsvcRand::new(3);
            for i in 1..=5 {
                emitter.update(i as f32 * 0.25, ASIDE, Some(z), &mut rng);
            }
            let mut frame = EffectFrame::default();
            emitter.draw(Mat4::IDENTITY, ASIDE, &mut frame);
            let positions: Vec<Vec3> = emitter.particles().iter().map(|p| p.position).collect();
            (positions, frame)
        };
        let (a, frame_a) = fly(&plain);
        let (b, frame_b) = fly(&busy);
        assert!(!a.is_empty());
        assert_eq!(a, b);
        // Drawn without effect position although MOVEPARTICLESWITHOBJECT is set (`0x00595470`).
        assert_eq!(frame_a, frame_b);
    }

    #[test]
    fn spin_unchanged_by_target_mode() {
        // The rotation runs the same in both modes (`dt * rotation rate`, rotation rate = SPIN / LIFE).
        let d = ParticleDesc {
            spin: 3.0,
            accel: 2.0,
            ..light()
        };
        let spin = |target: Option<Vec3>| {
            let mut emitter = ParticleEmitter::new(&d);
            emitter.start(0);
            if target.is_some() {
                emitter.set_life_for_target(2.0);
            }
            let mut rng = MsvcRand::new(3);
            for i in 1..=6 {
                emitter.update(i as f32 * 0.25, ASIDE, target, &mut rng);
            }
            emitter
                .particles()
                .iter()
                .map(|p| p.rotation())
                .collect::<Vec<_>>()
        };
        let free = spin(None);
        assert!(free.iter().any(|&r| r != 0.0));
        assert_eq!(free, spin(Some(Vec3::new(256.0, 0.0, 0.0))));
    }

    // QUIRK D-95: kept
    #[test]
    fn spin_ignores_accel() {
        // The rotation takes the raw `dt` (`0x00595178`), not `dt * ACCEL`: same rotation with
        // ACCEL 1 and 2 after the same `dt`, with and without a target.
        let spin = |accel: f32, target: Option<Vec3>| {
            let d = ParticleDesc {
                spin: 3.0,
                accel,
                ..light()
            };
            let mut emitter = ParticleEmitter::new(&d);
            emitter.start(0);
            if target.is_some() {
                emitter.set_life_for_target(2.0);
            }
            let mut rng = MsvcRand::new(3);
            for i in 1..=6 {
                emitter.update(i as f32 * 0.25, ASIDE, target, &mut rng);
            }
            emitter
                .particles()
                .iter()
                .map(|p| p.rotation())
                .collect::<Vec<_>>()
        };
        let slow = spin(1.0, None);
        assert!(slow.iter().any(|&r| r != 0.0));
        assert_eq!(slow, spin(2.0, None));
        let target = Some(Vec3::new(256.0, 0.0, 0.0));
        let flying = spin(1.0, target);
        assert!(flying.iter().any(|&r| r != 0.0));
        assert_eq!(flying, spin(2.0, target));
    }

    // QUIRK D-276: kept
    #[test]
    fn started_particles_keep_their_target() {
        let d = ParticleDesc {
            emit: 4,
            duration: 2.0,
            ..desc()
        };
        let a = Vec3::new(256.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 0.0, 512.0);
        // Two equal runs; the second switches target and flight time after 1 s.
        let run = |switch: bool| {
            let mut emitter = ParticleEmitter::new(&d);
            emitter.start(0);
            emitter.set_life_for_target(2.0);
            let mut rng = MsvcRand::new(11);
            let mut oldest = Vec::new();
            for i in 1..=9 {
                let target = if switch && i > 4 {
                    if i == 5 {
                        emitter.set_life_for_target(4.0);
                    }
                    b
                } else {
                    a
                };
                emitter.update(i as f32 * 0.25, ASIDE, Some(target), &mut rng);
                let p = emitter.particles().last().unwrap().clone();
                oldest.push((p.position, p.color(), p.alpha(), p.size()));
            }
            (oldest, emitter)
        };
        let (kept, _) = run(false);
        let (switched, emitter) = run(true);
        // The first particle keeps flying with the position and curves of its start and lands at `a`.
        assert_eq!(kept, switched);
        assert!((switched.last().unwrap().0 - a).length() < 0.01);
        // New particles fly to `b` and live 4 s.
        assert_eq!(emitter.particles()[0].life, 4.0);
    }

    /// FNV-1a (64 bit) over position, color and UV of all corners of a frame.
    fn fingerprint(frame: &EffectFrame) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |x: u32| {
            for b in x.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for v in &frame.vertices {
            v.position.iter().for_each(|c| eat(c.to_bits()));
            eat(v.color);
            v.uv.iter().for_each(|c| eat(c.to_bits()));
        }
        h
    }

    /// Without a target bit-exact as before the targeting mode was added: 60 frames of an emitter with all
    /// kinds of movement (spread, ring, gravity, attraction, orbit, rotation, moving along,
    /// rotation of the effect), rescaled after 30 frames (running fire).
    #[test]
    fn effects_without_target_are_unchanged() {
        let d = ParticleDesc {
            emit: 20,
            looping: true,
            spread: 5.0,
            yaw_var: 1.0,
            pitch: 0.7,
            pitch_var: 0.4,
            speed: 20.0,
            gravity: [0.0, -15.0, 3.0],
            attract: 4.0,
            attract_axes: [1, 0, 1],
            accel: 2.0,
            spin: -3.0,
            rot_y: 2.0,
            emitter_type: 2,
            emitter_radius: 7.0,
            move_with_object: true,
            pos: [3.0, 1.0, -2.0],
            ..desc()
        };
        let mut emitter = ParticleEmitter::new(&d);
        emitter.start(0);
        let mut rng = MsvcRand::new(9);
        let mut placement = Placement {
            position: Vec3::new(50.0, 4.0, -20.0),
            yaw: 0.3,
        };
        for i in 1..=60 {
            if i == 30 {
                emitter.scale(1.5);
            }
            placement.position.x += 0.5;
            emitter.update(i as f32 / 30.0, placement, None, &mut rng);
        }
        let mut frame = EffectFrame::default();
        let view = Mat4::from_translation(Vec3::new(1.0, -2.0, 300.0));
        emitter.draw(view, placement, &mut frame);
        let first = frame.vertices[0].position.map(f32::to_bits);
        let last = frame.vertices.last().unwrap().position.map(f32::to_bits);
        assert_eq!(
            (frame.quad_count(), fingerprint(&frame), first, last),
            (
                40,
                1_430_885_959_722_315_570,
                [1_115_754_130, 1_099_956_224, 1_133_396_424],
                [1_120_664_661, 3_262_672_226, 1_132_884_268]
            ),
            // Before the rotation fix (old rotation with `dt*ACCEL`): 14_150_386_435_453_179_243 /
            // [1_119_910_186, 3_230_483_912, ...]. This example (SPIN -3,
            // ACCEL 2) now rotates with the raw `dt` (`0x00595178`); with SPIN 0 old and new values are
            // bit-identical, only the rotation of the quads has changed.
            "measured with the rotation by `dt`"
        );
    }
}
