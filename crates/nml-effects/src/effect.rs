//! Effect instance (`0x170` bytes, `Effect_Construct` `0x0057B950`): a loaded `.pef` with
//! position, rotation about y, scaling, effect time and components, advanced like
//! `Effect_SetTransformAndAdvance` (`0x0057C550`) and `Effect_Render` (`0x0057BD70`); target flight
//! of the particles via `Effect_SetTarget` (`0x0057BBC0`).

use glam::{Mat4, Vec3};
use nml_formats::pef::{PefComponent, PefFile};
use nml_world::rand::MsvcRand;

use crate::billboard::BillboardEffect;
use crate::object::ObjectEffect;
use crate::output::EffectFrame;
use crate::particles::{ParticleEmitter, Placement};

/// Effects that have not been drawn for longer than this many milliseconds freeze (except
/// with "run invisibly", `Effect+0x148`).
pub const FREEZE_AFTER_MS: u32 = 200;

/// What an instance looks up when it is created: pixel sizes of textures (billboard layers without
/// `SIZE`) and animation indices of the model effects (`ANIMGUID` in the `.cfg` of the model).
pub trait EffectAssets {
    fn texture_size(&mut self, path: &str) -> Option<[u32; 2]>;
    fn animation(&mut self, model: &str, guid: i32) -> Option<usize>;
}

/// Without files: no texture sizes, no animations.
pub struct NoAssets;

impl EffectAssets for NoAssets {
    fn texture_size(&mut self, _path: &str) -> Option<[u32; 2]> {
        None
    }

    fn animation(&mut self, _model: &str, _guid: i32) -> Option<usize> {
        None
    }
}

/// One component of the `.pef`.
#[derive(Debug, Clone, PartialEq)]
pub enum Component {
    Particles(Box<ParticleEmitter>),
    Billboard(BillboardEffect),
    Object(ObjectEffect),
}

impl Component {
    fn start(&mut self, ms: u32) {
        match self {
            Self::Particles(p) => p.start(ms),
            Self::Billboard(b) => b.start(ms),
            Self::Object(o) => o.start(ms),
        }
    }

    fn is_active(&self) -> bool {
        match self {
            Self::Particles(p) => p.is_active(),
            Self::Billboard(b) => b.is_active(),
            Self::Object(o) => o.is_active(),
        }
    }

    /// Model effects do not scale.
    fn scale(&mut self, factor: f32) {
        match self {
            Self::Particles(p) => p.scale(factor),
            Self::Billboard(b) => b.scale(factor),
            Self::Object(_) => {}
        }
    }
}

/// A running effect instance.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInstance {
    /// Path of the `.pef` (key of the library).
    pub path: String,
    components: Vec<Component>,
    placement: Placement,
    /// `+0x144`.
    scale: f32,
    /// `+0x148`: keeps running even when not drawn.
    run_invisible: bool,
    /// `+0x150`: effect time (ms).
    time_ms: u32,
    /// `+0x14C`: last drawn.
    drawn_ms: u32,
    /// `+0x158`: last advanced.
    advanced_ms: u32,
    /// `MOVEMENTTIME` of the template (`Template+0x104`): seconds per 128 units of target distance.
    movement_time: f32,
    /// `+0x15C`/`+0x160`: target point; once set, never cleared.
    target: Option<Vec3>,
    /// `+0x16C`: flight time of the particles to the target (s).
    flight_s: f32,
}

impl EffectInstance {
    /// Instance of a template (`EffectLibrary_InstantiateCached` `0x0057C120`).
    pub fn new(
        path: &str,
        pef: &PefFile,
        run_invisible: bool,
        assets: &mut dyn EffectAssets,
    ) -> Self {
        let components = pef
            .components
            .iter()
            .map(|c| match c {
                PefComponent::Particles(d) => {
                    Component::Particles(Box::new(ParticleEmitter::new(d)))
                }
                PefComponent::Billboard(d) => {
                    Component::Billboard(BillboardEffect::new(d, &mut |texture| {
                        assets.texture_size(texture)
                    }))
                }
                PefComponent::Object(d) => {
                    let animation = assets.animation(&d.object, d.anim_guid);
                    Component::Object(ObjectEffect::new(d, animation))
                }
            })
            .collect();
        Self {
            path: path.to_string(),
            components,
            placement: Placement {
                position: Vec3::ZERO,
                yaw: 0.0,
            },
            scale: 1.0,
            run_invisible,
            time_ms: 0,
            drawn_ms: 0,
            advanced_ms: 0,
            movement_time: pef.movement_time,
            target: None,
            flight_s: 1.0,
        }
    }

    pub fn components(&self) -> &[Component] {
        &self.components
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    pub fn time_ms(&self) -> u32 {
        self.time_ms
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn runs_invisible(&self) -> bool {
        self.run_invisible
    }

    /// Target point (`+0x160`), once it has been set.
    pub fn target(&self) -> Option<Vec3> {
        self.target
    }

    /// Flight time to the target in s (`+0x16C`, default 1).
    pub fn flight_s(&self) -> f32 {
        self.flight_s
    }

    /// `Effect_SetTarget(z)` (`0x0057BBC0`): remember the target; distance to the current effect position
    /// (`+0x138`), 0 counts as 128; flight time `distance * MOVEMENTTIME * 1/128`; each emitter
    /// `SetLifeForTarget(flight time)`. With `MOVEMENTTIME 0` everything becomes NaN as in the original (in
    /// the shipped data all `.pef` have 1). Every new position then measures again
    /// ([`EffectInstance::set_placement`], [`EffectInstance::advance`]).
    pub fn set_target(&mut self, z: Vec3) {
        self.target = Some(z);
        let mut d = (z - self.placement.position).length();
        if d == 0.0 {
            d = 128.0;
        }
        self.flight_s = d * self.movement_time * 0.0078125;
        for component in &mut self.components {
            if let Component::Particles(p) = component {
                p.set_life_for_target(self.flight_s);
            }
        }
    }

    /// `Effect_Start(ms)` (`0x0057BAD0`) at clock time `now_ms`: effect time = `ms`, timestamps =
    /// now, `Start(ms)` per component.
    pub fn start(&mut self, ms: u32, now_ms: u32) {
        self.time_ms = ms;
        self.drawn_ms = now_ms;
        self.advanced_ms = now_ms;
        for component in &mut self.components {
            component.start(ms);
        }
    }

    /// Sets the position without advancing (`Effect_SetPosition` `0x0057C4E0`, `Effect_SetRotationY`):
    /// with a target set, measured again immediately, so against the new position.
    pub fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
        self.remeasure();
    }

    /// End of `Effect_SetPosition` (`0x0057C4E0`): `if +0x15C { Effect_SetTarget(+0x160) }`.
    fn remeasure(&mut self) {
        if let Some(z) = self.target {
            self.set_target(z);
        }
    }

    /// `Effect_SetScale(s)` (`0x0057BC90`): per component with the factor `s / s_old`; `s <= 0`
    /// is ignored.
    pub fn set_scale(&mut self, s: f32) {
        if s <= 0.0 {
            return;
        }
        let factor = s / self.scale;
        for component in &mut self.components {
            component.scale(factor);
        }
        self.scale = s;
    }

    /// Frozen: not drawn for longer than 200 ms and without "run invisibly".
    pub fn frozen(&self, now_ms: u32) -> bool {
        !self.run_invisible && now_ms.wrapping_sub(self.drawn_ms) > FREEZE_AFTER_MS
    }

    /// `Effect_UpdateComponents` (`0x0057BD00`): `Update(effect time / 1000)` per component.
    fn update_components(&mut self, rng: &mut MsvcRand) {
        let t = self.time_ms as f32 / 1000.0;
        let placement = self.placement;
        let target = self.target;
        for component in &mut self.components {
            match component {
                Component::Particles(p) => p.update(t, placement, target, rng),
                Component::Billboard(b) => b.update(t),
                Component::Object(o) => o.update(t),
            }
        }
    }

    /// One frame at clock time `now_ms`: `Effect_SetTransformAndAdvance` with the new position, then,
    /// if the effect is drawn in this frame (`drawn`), the advance of `Effect_Render` (components
    /// to the effect time, timestamp "drawn").
    ///
    /// If the effect was drawn within the last 200 ms, the effect time advances by the time that has
    /// passed since the last call; otherwise only with "run invisibly" (then immediately
    /// with advancing the components), without it it stands still. Where the position is set,
    /// `Effect_SetPosition` measures the flight time against it again before a particle is created; when frozen,
    /// both stay as they are.
    pub fn advance(&mut self, now_ms: u32, placement: Placement, drawn: bool, rng: &mut MsvcRand) {
        let dt = now_ms.wrapping_sub(self.advanced_ms);
        if now_ms.wrapping_sub(self.drawn_ms) <= FREEZE_AFTER_MS {
            self.time_ms = self.time_ms.wrapping_add(dt);
            self.set_placement(placement);
        } else if self.run_invisible {
            self.time_ms = self.time_ms.wrapping_add(dt);
            self.set_placement(placement);
            self.update_components(rng);
        }
        self.advanced_ms = now_ms;
        if drawn {
            self.update_components(rng);
            self.drawn_ms = now_ms;
        }
    }

    /// Draws the components in file order (`view` = view matrix of the camera).
    pub fn draw(&self, view: Mat4, out: &mut EffectFrame) {
        for component in &self.components {
            match component {
                Component::Particles(p) => p.draw(view, self.placement, out),
                Component::Billboard(b) => b.draw(view, self.placement, out),
                Component::Object(o) => o.draw(self.placement, out),
            }
        }
    }

    /// `Effect_IsAnyComponentActive` (`0x0057BB80`).
    pub fn is_active(&self) -> bool {
        self.components.iter().any(Component::is_active)
    }

    /// `Effect_StopEmittersAndIsDead` (`0x0057BB30`): stop all emitters, number of
    /// particles still alive.
    pub fn stop_and_count(&mut self) -> usize {
        self.components
            .iter_mut()
            .map(|c| match c {
                Component::Particles(p) => p.stop_and_count(),
                Component::Billboard(b) => {
                    b.stop();
                    0
                }
                Component::Object(o) => {
                    o.stop();
                    0
                }
            })
            .sum()
    }

    /// Living particles of all emitters.
    pub fn particle_count(&self) -> usize {
        self.components
            .iter()
            .map(|c| match c {
                Component::Particles(p) => p.particles().len(),
                Component::Billboard(_) | Component::Object(_) => 0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::pef::{BillboardDesc, ParticleDesc};

    /// One emitter (10/s, life 1 s, `LOOP`) and an empty billboard effect.
    fn pef() -> PefFile {
        PefFile {
            movement_time: 1.0,
            components: vec![
                PefComponent::Particles(ParticleDesc {
                    texture: "p.tga".into(),
                    looping: true,
                    emit: 10,
                    life: 1.0,
                    accel: 1.0,
                    speed: 5.0,
                    sizes: [[0.0, 4.0], [0.3, 4.0], [0.6, 4.0], [1.0, 4.0]],
                    alphas: [[0.0, 255.0], [0.3, 255.0], [0.6, 255.0], [1.0, 255.0]],
                    colors: [[0.0, 255.0, 255.0, 255.0]; 4],
                    ..ParticleDesc::default()
                }),
                PefComponent::Billboard(BillboardDesc::default()),
            ],
        }
    }

    fn instance(run_invisible: bool) -> EffectInstance {
        let mut e = EffectInstance::new("x.pef", &pef(), run_invisible, &mut NoAssets);
        e.start(0, 1000);
        e
    }

    const HERE: Placement = Placement {
        position: Vec3::new(10.0, 0.0, 0.0),
        yaw: 0.0,
    };

    #[test]
    fn effects_freeze_200_ms_after_the_last_drawing_unless_they_run_invisible() {
        let mut rng = MsvcRand::new(1);
        let mut e = instance(false);
        e.advance(1050, HERE, true, &mut rng);
        assert_eq!((e.time_ms(), e.placement()), (50, HERE));
        // No longer drawn: keeps running for 200 ms, after that everything stands still.
        for now in [1100, 1150, 1200, 1250] {
            e.advance(now, HERE, false, &mut rng);
        }
        assert_eq!(e.time_ms(), 250);
        assert!(e.frozen(1251));
        let moved = Placement {
            position: Vec3::ONE,
            yaw: 1.0,
        };
        e.advance(1300, moved, false, &mut rng);
        assert_eq!((e.time_ms(), e.placement()), (250, HERE));
        // Drawn again: only this frame takes the stamp, then the time keeps running
        // (without the missed time).
        e.advance(1350, moved, true, &mut rng);
        assert_eq!(e.time_ms(), 250);
        e.advance(1400, moved, true, &mut rng);
        assert_eq!((e.time_ms(), e.placement()), (300, moved));
        // With "run invisibly": time and particles keep running undrawn.
        let mut e = instance(true);
        for now in (1050..=1600).step_by(50) {
            e.advance(now, HERE, false, &mut rng);
        }
        assert_eq!(e.time_ms(), 600);
        assert!(!e.frozen(1600));
        assert!(e.particle_count() > 0);
    }

    #[test]
    fn scaling_applies_the_ratio_to_the_components() {
        let mut e = instance(false);
        e.set_scale(2.0);
        e.set_scale(3.0);
        e.set_scale(0.0);
        assert_eq!(e.scale(), 3.0);
        let Component::Particles(p) = &e.components()[0] else {
            panic!("emitter expected");
        };
        assert!((p.desc().speed - 15.0).abs() < 1e-4);
        assert!((p.desc().sizes[0][1] - 12.0).abs() < 1e-4);
    }

    #[test]
    fn stopping_counts_the_remaining_particles() {
        let mut rng = MsvcRand::new(1);
        let mut e = instance(false);
        for now in (1050..=1500).step_by(50) {
            e.advance(now, HERE, true, &mut rng);
        }
        assert!(e.is_active());
        let left = e.stop_and_count();
        assert_eq!(left, e.particle_count());
        assert!(left > 0 && !e.is_active());
    }

    #[test]
    fn split_steps_give_the_same_vertices() {
        let run = |chunks: &[u32]| {
            let mut rng = MsvcRand::new(3);
            let mut e = instance(false);
            let mut now = 1000;
            for &n in chunks {
                for _ in 0..n {
                    now += 16;
                    e.advance(now, HERE, true, &mut rng);
                }
            }
            let mut frame = EffectFrame::default();
            e.draw(Mat4::IDENTITY, &mut frame);
            frame
        };
        let whole = run(&[30]);
        assert!(whole.quad_count() > 0);
        assert_eq!(whole, run(&[10, 20]));
    }

    /// `Effect_SetPosition` (`0x0057C4E0`) measures again immediately when a target is set: when setting the
    /// position and in both branches of `advance` that set it; not when frozen.
    #[test]
    fn every_new_placement_remeasures_the_flight() {
        let mut rng = MsvcRand::new(1);
        let z = Vec3::new(0.0, 0.0, 256.0);
        let flight = |p: Vec3| (z - p).length() / 128.0;
        let mut e = instance(false);
        // Before the first position against the origin (intermediate value).
        e.set_target(z);
        assert_eq!(e.flight_s(), 2.0);
        e.set_placement(HERE);
        assert_eq!(e.flight_s(), flight(HERE.position));
        let Component::Particles(p) = &e.components()[0] else {
            panic!("emitter expected");
        };
        assert_eq!(p.desc().life, flight(HERE.position));
        // Drawn: new position, new measurement.
        let moved = Placement {
            position: Vec3::new(0.0, 0.0, 64.0),
            yaw: 0.5,
        };
        e.advance(1050, moved, true, &mut rng);
        assert_eq!(e.flight_s(), 1.5);
        // Frozen (undrawn for over 200 ms): position and flight time stay.
        assert!(e.frozen(1300));
        e.advance(1300, HERE, false, &mut rng);
        assert_eq!((e.placement(), e.flight_s()), (moved, 1.5));
        // "Run invisibly" (second branch, undrawn for over 200 ms): also measured again.
        let mut e = instance(true);
        e.set_target(z);
        assert!(!e.frozen(1300));
        // Position (0, 0, 64) instead of HERE: distance 192 -> 1.5 s, clearly separated from the intermediate value 2.0
        // (HERE is almost perpendicular to the target and would give only 2.0015 s).
        assert_eq!(e.flight_s(), 2.0);
        e.advance(1300, moved, false, &mut rng);
        assert_ne!(e.flight_s(), 2.0);
        assert_eq!(e.flight_s(), 1.5);
    }

    /// Without a target bit-exact as before the targeting mode was added (via the instance: 60 frames, moving
    /// position).
    #[test]
    fn instances_without_target_are_unchanged() {
        let mut rng = MsvcRand::new(5);
        let mut e = instance(false);
        for i in 1..=60u32 {
            let placement = Placement {
                position: Vec3::new(i as f32, 0.0, 2.0 * i as f32),
                yaw: 0.01 * i as f32,
            };
            e.advance(1000 + 16 * i, placement, true, &mut rng);
        }
        let mut frame = EffectFrame::default();
        e.draw(
            Mat4::from_translation(Vec3::new(0.0, 0.0, 100.0)),
            &mut frame,
        );
        let bits: Vec<[u32; 3]> = frame
            .vertices
            .iter()
            .step_by(37)
            .map(|v| v.position.map(f32::to_bits))
            .collect();
        // Measured before the targeting mode was added.
        assert_eq!(
            (frame.quad_count(), bits),
            (
                10,
                vec![
                    [1_113_040_438, 1_082_130_432, 1_129_849_411],
                    [1_102_768_632, 1_082_130_432, 1_125_065_437]
                ]
            )
        );
    }
}
