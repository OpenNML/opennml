//! Model ambient effects of the map (re/effects, section "Map ambient objects"):
//! surf (`SurfEffect` `0x004F4C00`, GUID 4104-4106), fish (`FishEffect` `0x004F1330`,
//! GUID 3411, layer 2; also at fishing baskets 3505) and eagle (`EagleEffect` `0x004F0B60`,
//! GUID 3408). They are created instead of a game object (`World_PlaceObject` `0x00440720`), only
//! from `ParticlesQuality` 1, check their visibility in the game tick and only keep running while visible.

use std::f64::consts::{FRAC_PI_2, PI};

use glam::{Affine3A, Mat3, Vec3};
use nml_world::math::game_rotation_y;
use nml_world::rand::MsvcRand;
use nml_world::visibility::{Containment, Frustum};

use crate::output::{DrawCommand, EffectFrame, ModelDraw};

/// GUIDs of the surf (`World_IsSurfGuid` `0x004F45D0`).
pub const SURF_GUIDS: [u32; 3] = [4104, 4105, 4106];
/// GUID of the fish (`World_IsFishGuid` `0x004F10D0`).
pub const FISH_GUID: u32 = 3411;
/// GUID of the eagle (`World_IsEagleGuid` `0x004F0960`).
pub const EAGLE_GUID: u32 = 3408;
/// GUID of the fishing baskets: invisible game object with its own fish effect.
pub const FISH_BASKET_GUID: u32 = 3505;

/// `rand() * (1/32767)` as a double like the game (`0x006213A8`).
fn rand_unit_f64(rng: &mut MsvcRand) -> f64 {
    f64::from(rng.rand()) * (1.0 / 32767.0)
}

/// One wave of the surf.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wave {
    /// `m_rad` (type 0: alpha/life phase, type 1: phase `p`).
    a: f32,
    /// `m_rad2` (type 0: run phase, type 1: `2p`).
    b: f32,
    position: Vec3,
    scale: Vec3,
    alpha: f32,
}

/// Surf on the coast.
#[derive(Debug, Clone, PartialEq)]
pub struct Surf {
    pub guid: u32,
    /// `OBJ_POSITION` of the level object (location of the surf sound).
    pub position: Vec3,
    /// `OBJ_FILENAME` of the type (`brandung01...03.rdo`).
    pub model: String,
    /// Type 0 (4104/4105, two waves, tint 2) or 1 (4106, one wave, tint 1).
    kind: u8,
    yaw: f32,
    /// `m_Position`: base `OBJ_POSITION - 1.2 * D`.
    pub base: Vec3,
    /// `m_Direction`: `(0, 0, 320) * Ry(OBJ_YROT)`.
    direction: Vec3,
    /// Bounding sphere of the visibility test (center `B - D/2`, radius `|D| + 1.5 * R`).
    sphere: (Vec3, f32),
    waves: Vec<Wave>,
}

impl Surf {
    /// `SurfEffect_Construct` (`0x004F4C00`): one `rand()` per wave, phase
    /// `rand/32767 * 0.2 + i`. `model_radius` = hull radius of the `.rdo`.
    pub fn new(
        guid: u32,
        model: &str,
        position: Vec3,
        yaw: f32,
        model_radius: f32,
        rng: &mut MsvcRand,
    ) -> Self {
        let (kind, count) = if guid == 4106 { (1, 1) } else { (0, 2) };
        let mut waves = Vec::with_capacity(count);
        let mut acc = 0.0f32;
        for _ in 0..count {
            let phase = (rand_unit_f64(rng) * f64::from(0.2f32) + f64::from(acc)) as f32;
            waves.push(Wave {
                a: phase,
                b: phase,
                position,
                scale: Vec3::ONE,
                alpha: 0.0,
            });
            acc += 1.0;
        }
        let direction = game_rotation_y(yaw) * Vec3::new(0.0, 0.0, 320.0);
        let base = position - 1.2 * direction;
        let sphere = (
            base - direction / 2.0,
            direction.length() + model_radius * 1.5,
        );
        Self {
            guid,
            position,
            model: model.to_string(),
            kind,
            yaw,
            base,
            direction,
            sphere,
            waves,
        }
    }

    /// `SurfEffect_FrameUpdate(dt)` (`0x004F46B0`), `dt` in whole game-time milliseconds.
    fn frame_update(&mut self, dt: u32) {
        let sin = |x: f32| f64::from(x).sin() as f32;
        if self.kind == 0 {
            let d = dt as f32 * 0.0005;
            for w in &mut self.waves {
                w.scale = Vec3::new(sin(w.b) + 0.5, 1.0, 0.5 + sin(w.a));
                w.alpha = sin(w.a);
                w.position = self.base + self.direction * sin(w.b);
                w.b += d;
                if w.b > std::f32::consts::FRAC_PI_2 {
                    w.b = std::f32::consts::FRAC_PI_2;
                }
                w.a += d;
                if w.a >= std::f32::consts::PI {
                    w.a = 0.0;
                    w.b = 0.0;
                }
            }
        } else {
            let d = dt as f32 * 0.0004;
            for w in &mut self.waves {
                w.scale = Vec3::new(sin(w.a), 1.0, sin(w.a));
                w.alpha = sin(w.b);
                w.position = self.base + self.direction * sin(w.a);
                w.a += d;
                if w.a >= std::f32::consts::FRAC_PI_2 {
                    w.a = 0.0;
                }
                w.b = 2.0 * w.a;
            }
        }
    }

    /// `SurfEffect_Render` (`0x004F4880`): per wave `S * Ry * T`, tinted (2.0 or 1.0), alpha
    /// of the wave, own states (blend `SRCALPHA`/`INVSRCALPHA`, no depth writing,
    /// CCW, no fog).
    fn draw(&self, out: &mut EffectFrame) {
        let tint = if self.kind == 0 { 2.0 } else { 1.0 };
        let rotation = game_rotation_y(self.yaw);
        for w in &self.waves {
            let world = Affine3A::from_mat3_translation(
                rotation * Mat3::from_diagonal(w.scale),
                w.position,
            );
            out.commands.push(DrawCommand::Model(ModelDraw {
                model: self.model.clone(),
                world,
                animation: None,
                tint: Some(tint),
                alpha: Some(w.alpha),
                blend: None,
                cull: true,
                fog: false,
                mesh: None,
                player_color: None,
            }));
        }
    }
}

/// One fish.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fish {
    position: Vec3,
    yaw: f32,
    x_phase: f32,
    z_phase: f32,
    radius: i32,
}

/// Five fish below the water surface.
#[derive(Debug, Clone, PartialEq)]
pub struct Fishes {
    /// `OBJ_SHADOW_FILENAME` of the type (`fish.rdo` or `fisch nahrung.rdo`).
    pub model: String,
    /// Center `OBJ_POSITION - (0, 20, 0)`.
    pub center: Vec3,
    sphere_radius: f32,
    fish: Vec<Fish>,
}

impl Fishes {
    /// `FishEffect_Construct` (`0x004F1330`): 5 fish, radii 60...140, two `rand()` per fish
    /// (phase x, then z, in `[0, pi]`).
    pub fn new(model: &str, position: Vec3, model_radius: f32, rng: &mut MsvcRand) -> Self {
        let center = position - Vec3::new(0.0, 20.0, 0.0);
        let fish: Vec<Fish> = (0..5)
            .map(|i| {
                let x_phase = (rand_unit_f64(rng) * PI) as f32;
                let z_phase = (rand_unit_f64(rng) * PI) as f32;
                Fish {
                    position: center,
                    yaw: 0.0,
                    x_phase,
                    z_phase,
                    radius: 60 + 20 * i,
                }
            })
            .collect();
        Self {
            model: model.to_string(),
            center,
            sphere_radius: model_radius + 140.0,
            fish,
        }
    }

    /// `FishEffect_Update(dt)` (`0x004F1180`): Lissajous path, yaw from the movement.
    fn frame_update(&mut self, dt: u32) {
        let dx = f64::from(dt as f32 * 0.0002);
        let dz = dt as f64 * f64::from(0.0003f32);
        let c = self.center;
        for f in &mut self.fish {
            let px = f64::from(f.x_phase) + dx;
            f.x_phase = px as f32;
            let pz = f64::from(f.z_phase) + dz;
            f.z_phase = pz as f32;
            let old = f.position;
            let r = f64::from(f.radius);
            f.position = Vec3::new(
                (f64::from(c.x) + px.sin() * r) as f32,
                c.y,
                (f64::from(c.z) + pz.cos() * r) as f32,
            );
            f.yaw = (f64::from(old.z - f.position.z).atan2(f64::from(old.x - f.position.x))
                - FRAC_PI_2) as f32;
        }
    }

    /// `FishEffect_Render` (`0x004F1260`): state `Objects`, per fish `Ry * T`.
    fn draw(&self, out: &mut EffectFrame) {
        for f in &self.fish {
            out.commands.push(DrawCommand::Model(ModelDraw {
                model: self.model.clone(),
                world: Affine3A::from_mat3_translation(game_rotation_y(f.yaw), f.position),
                animation: None,
                tint: None,
                alpha: None,
                blend: None,
                cull: true,
                fog: true,
                mesh: None,
                player_color: None,
            }));
        }
    }
}

/// Circling eagle.
#[derive(Debug, Clone, PartialEq)]
pub struct Eagle {
    pub model: String,
    /// Center `OBJ_POSITION + (0, 1000, 0)`.
    pub center: Vec3,
    pub position: Vec3,
    model_radius: f32,
    yaw: f32,
    x_phase: f32,
    z_phase: f32,
    /// Start time of the flight animation (game time ms); it keeps running time-based.
    started_ms: u32,
}

impl Eagle {
    /// `EagleEffect_Construct` (`0x004F0B60`): animation 0 at speed 128, no randomness.
    pub fn new(model: &str, position: Vec3, model_radius: f32, now_ms: u32) -> Self {
        let center = position + Vec3::new(0.0, 1000.0, 0.0);
        Self {
            model: model.to_string(),
            center,
            position: center,
            model_radius,
            yaw: 0.0,
            x_phase: 0.0,
            z_phase: 0.0,
            started_ms: now_ms,
        }
    }

    /// `EagleEffect_Update(dt)` (`0x004F0A00`): circle with radius 600, 0.4 rad/s.
    fn frame_update(&mut self, dt: u32) {
        let d = dt as f64 * f64::from(0.0004f32);
        let px = f64::from(self.x_phase) + d;
        self.x_phase = px as f32;
        let pz = f64::from(self.z_phase) + d;
        self.z_phase = pz as f32;
        let old = self.position;
        let c = self.center;
        self.position = Vec3::new(
            (f64::from(c.x) + px.sin() * 600.0) as f32,
            c.y,
            (f64::from(c.z) + f64::from(self.z_phase).cos() * 600.0) as f32,
        );
        self.yaw = (f64::from(old.z - self.position.z).atan2(f64::from(old.x - self.position.x))
            - FRAC_PI_2) as f32;
    }

    /// `EagleEffect_Render` (`0x004F0AD0`): state `Objects`, `Ry * T`, animation 0.
    fn draw(&self, now_ms: u32, out: &mut EffectFrame) {
        out.commands.push(DrawCommand::Model(ModelDraw {
            model: self.model.clone(),
            world: Affine3A::from_mat3_translation(game_rotation_y(self.yaw), self.position),
            animation: Some((0, now_ms.wrapping_sub(self.started_ms) as f32)),
            tint: None,
            alpha: None,
            blend: None,
            cull: true,
            fog: true,
            mesh: None,
            player_color: None,
        }));
    }
}

/// An ambient effect.
#[derive(Debug, Clone, PartialEq)]
pub enum AmbientKind {
    Surf(Surf),
    Fishes(Fishes),
    Eagle(Eagle),
}

/// Ambient effect with visibility (`Visible`, set in the game tick).
#[derive(Debug, Clone, PartialEq)]
pub struct Ambient {
    pub kind: AmbientKind,
    pub visible: bool,
}

impl Ambient {
    pub fn new(kind: AmbientKind) -> Self {
        // Surf starts visible (`+0x0C` = 1), fish and eagle do not.
        let visible = matches!(kind, AmbientKind::Surf(_));
        Self { kind, visible }
    }

    /// Effect layer: fish 2 (drawn before the water), otherwise 1.
    pub fn layer(&self) -> u8 {
        match self.kind {
            AmbientKind::Fishes(_) => 2,
            _ => 1,
        }
    }

    /// Game tick (`*_Tick`): bounding sphere against the view frustum, then fog of war -
    /// surf (`SurfEffect_UpdateVisibility` `0x004F45F0`) needs `> 1` (seen), fish
    /// (`0x004F10E0`) and eagle (`0x004F0970`) `> 0` (explored). `fog` returns
    /// `QueryFogOfWarArea` of the viewer at a world position together with the view switch (without
    /// simulation 7). As in the original, the query uses the surf base `+0x2C` or the center
    /// `+0x1C` of fish and eagle (for the eagle not the sphere center, which is the flight position).
    /// Returns whether the effect is advanced and drawn until the next tick; the
    /// surf also reports its sound with it.
    pub fn tick(&mut self, frustum: &Frustum, fog: &dyn Fn(Vec3) -> u8) -> bool {
        let ((center, radius), at, need) = match &self.kind {
            AmbientKind::Surf(s) => (s.sphere, s.base, 2),
            AmbientKind::Fishes(f) => ((f.center, f.sphere_radius), f.center, 1),
            AmbientKind::Eagle(e) => ((e.position, e.model_radius), e.center, 1),
        };
        self.visible =
            frustum.classify_sphere(center, radius) != Containment::Outside && fog(at) >= need;
        self.visible
    }

    /// Frame update (`vf+0x20`) by `dt` game-time ms.
    pub fn frame_update(&mut self, dt: u32) {
        match &mut self.kind {
            AmbientKind::Surf(s) => s.frame_update(dt),
            AmbientKind::Fishes(f) => f.frame_update(dt),
            AmbientKind::Eagle(e) => e.frame_update(dt),
        }
    }

    /// Drawing (`vf+0x18`) at game time `now_ms`.
    pub fn draw(&self, now_ms: u32, out: &mut EffectFrame) {
        match &self.kind {
            AmbientKind::Surf(s) => s.draw(out),
            AmbientKind::Fishes(f) => f.draw(out),
            AmbientKind::Eagle(e) => e.draw(now_ms, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2 as HALF_PI;

    fn models(frame: &EffectFrame) -> Vec<&ModelDraw> {
        frame
            .commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Model(m) => Some(m),
                DrawCommand::Quads(_) => None,
            })
            .collect()
    }

    #[test]
    fn surf_waves_run_from_behind_the_placement_and_fade() {
        let mut rng = MsvcRand::new(1);
        let mut surf = Surf::new(
            4104,
            "s.rdo",
            Vec3::new(1000.0, 0.0, 1000.0),
            0.0,
            100.0,
            &mut rng,
        );
        // D = (0, 0, 320), base 384 behind the placement, sphere around B - D/2.
        assert_eq!(surf.base, Vec3::new(1000.0, 0.0, 616.0));
        assert_eq!(surf.sphere, (Vec3::new(1000.0, 0.0, 456.0), 470.0));
        // First rand() after srand(1) = 41: phase 41/32767 * 0.2; wave 2 offset by 1 rad.
        let p0 = (41.0f64 / 32767.0 * 0.2) as f32;
        assert_eq!((surf.waves[0].a, surf.waves[0].b), (p0, p0));
        assert!((1.0..1.2).contains(&surf.waves[1].a));
        surf.frame_update(1000);
        // Before the update: wave 0 near B, alpha sin a.
        let w = surf.waves[0];
        assert!((w.position.z - (616.0 + 320.0 * p0.sin())).abs() < 1e-3);
        assert_eq!(w.alpha, p0.sin());
        assert!((surf.waves[0].a - (p0 + 0.5)).abs() < 1e-5);
        // b stays at pi/2, at a >= pi both go to 0.
        for _ in 0..3 {
            surf.frame_update(1000);
        }
        assert_eq!(surf.waves[0].b, HALF_PI);
        surf.frame_update(3000);
        assert_eq!((surf.waves[0].a, surf.waves[0].b), (0.0, 0.0));
        let mut frame = EffectFrame::default();
        surf.draw(&mut frame);
        let m = models(&frame);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].tint, m[0].fog, m[0].cull), (Some(2.0), false, true));
    }

    #[test]
    fn surf_type_1_has_one_plain_wave() {
        let mut rng = MsvcRand::new(1);
        let mut surf = Surf::new(4106, "s.rdo", Vec3::ZERO, HALF_PI, 100.0, &mut rng);
        // D = (0, 0, 320) * Ry(90 degrees) = (-320, 0, 0).
        assert!((surf.direction - Vec3::new(-320.0, 0.0, 0.0)).length() < 1e-3);
        surf.frame_update(2000);
        assert!((surf.waves[0].a - (surf.waves[0].b / 2.0)).abs() < 1e-6);
        surf.frame_update(2000);
        // p >= pi/2 -> 0.
        assert_eq!(surf.waves[0].a, 0.0);
        let mut frame = EffectFrame::default();
        surf.draw(&mut frame);
        assert_eq!(models(&frame)[0].tint, Some(1.0));
    }

    #[test]
    fn fish_swim_on_lissajous_paths_below_the_surface() {
        let mut rng = MsvcRand::new(5);
        let mut fishes = Fishes::new("f.rdo", Vec3::new(500.0, -10.0, 500.0), 20.0, &mut rng);
        assert_eq!(fishes.center, Vec3::new(500.0, -30.0, 500.0));
        assert_eq!(fishes.sphere_radius, 160.0);
        assert_eq!(
            fishes.fish.iter().map(|f| f.radius).collect::<Vec<_>>(),
            [60, 80, 100, 120, 140]
        );
        fishes.frame_update(100);
        fishes.frame_update(100);
        for f in &fishes.fish {
            let d = f.position - fishes.center;
            assert!(d.x.abs() <= f.radius as f32 + 1e-3 && d.z.abs() <= f.radius as f32 + 1e-3);
            assert_eq!(d.y, 0.0);
        }
        let mut frame = EffectFrame::default();
        fishes.draw(&mut frame);
        assert_eq!(models(&frame).len(), 5);
    }

    #[test]
    fn eagles_circle_clockwise_1000_above_their_placement() {
        let mut eagle = Eagle::new("a.rdo", Vec3::new(0.0, 50.0, 0.0), 64.0, 0);
        // Quarter turn: from +z to +x.
        let quarter = (std::f64::consts::FRAC_PI_2 / f64::from(0.0004f32)) as u32;
        eagle.frame_update(quarter - 1);
        eagle.frame_update(1);
        assert!((eagle.position - Vec3::new(600.0, 1050.0, 0.0)).length() < 0.5);
        // Model -z points in the flight direction (-z).
        let forward = game_rotation_y(eagle.yaw) * Vec3::NEG_Z;
        assert!((forward - Vec3::NEG_Z).length() < 0.01, "{forward}");
        let mut frame = EffectFrame::default();
        eagle.draw(5000, &mut frame);
        assert_eq!(models(&frame)[0].animation, Some((0, 5000.0)));
    }

    #[test]
    fn only_visible_ambient_effects_pass_the_tick() {
        use nml_world::camera::{game_projection, look_at_lh};
        let view = look_at_lh(Vec3::new(0.0, 500.0, -500.0), Vec3::ZERO, Vec3::Y);
        let projection = game_projection(0.8, 1.0, 700.0);
        let frustum = Frustum::from_view_proj(projection * view);
        let mut rng = MsvcRand::new(1);
        let mut near = Ambient::new(AmbientKind::Fishes(Fishes::new(
            "f.rdo",
            Vec3::ZERO,
            20.0,
            &mut rng,
        )));
        let mut far = Ambient::new(AmbientKind::Fishes(Fishes::new(
            "f.rdo",
            Vec3::new(0.0, 0.0, -9000.0),
            20.0,
            &mut rng,
        )));
        assert!(!near.visible);
        assert!(near.tick(&frustum, &|_| 7) && !far.tick(&frustum, &|_| 7));
        assert_eq!((near.layer(), far.layer()), (2, 2));
        let surf = Ambient::new(AmbientKind::Surf(Surf::new(
            4105,
            "s.rdo",
            Vec3::ZERO,
            0.0,
            10.0,
            &mut rng,
        )));
        assert!(surf.visible);
        assert_eq!(surf.layer(), 1);
    }

    #[test]
    fn surf_needs_seen_tiles_and_fishes_and_eagles_explored_ones() {
        use nml_world::camera::{game_projection, look_at_lh};
        let view = look_at_lh(Vec3::new(0.0, 6000.0, -6000.0), Vec3::ZERO, Vec3::Y);
        let frustum = Frustum::from_view_proj(game_projection(0.8, 1.0, 20000.0) * view);
        let mut rng = MsvcRand::new(1);
        let mut surf = Ambient::new(AmbientKind::Surf(Surf::new(
            4105,
            "s.rdo",
            Vec3::ZERO,
            0.0,
            10.0,
            &mut rng,
        )));
        let mut fishes = Ambient::new(AmbientKind::Fishes(Fishes::new(
            "f.rdo",
            Vec3::ZERO,
            20.0,
            &mut rng,
        )));
        let mut eagle = Ambient::new(AmbientKind::Eagle(Eagle::new("a.rdo", Vec3::ZERO, 64.0, 0)));
        let cases = [
            (0u8, [false, false, false]),
            (1, [false, true, true]),
            (3, [true, true, true]),
            (7, [true, true, true]),
        ];
        for (probe, expected) in cases {
            let fog = move |_: Vec3| probe;
            let got = [
                surf.tick(&frustum, &fog),
                fishes.tick(&frustum, &fog),
                eagle.tick(&frustum, &fog),
            ];
            assert_eq!(got, expected, "fog {probe}");
        }
    }
}
