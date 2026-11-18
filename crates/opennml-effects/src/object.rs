//! Model effect (`OBJECTEFFECT`, component `0x24` bytes) like `ObjectEffect_Start`
//! (`0x0059B510`), `ObjectEffect_Update` (`0x0059B580`) and `ObjectEffect_Render`
//! (`0x0059B760`): a model with animation, faded in and out, drawn with the
//! blend factors of the file in the transparent pass.

use glam::{Affine3A, Vec3};
use opennml_formats::object_cfg::ObjectConfig;
use opennml_formats::pef::ObjectEffectDesc;
use opennml_rda::normalize_path;
use opennml_world::math::game_rotation_y;

use crate::output::{DrawCommand, EffectFrame, ModelDraw};
use crate::particles::Placement;

/// Fade factor without fading (`0x3F7FFFEF` ~ 0.99999899): never exactly 1, so all
/// groups land in the transparent pass.
pub const FULL_FACTOR: f32 = f32::from_bits(0x3F7F_FFEF);

/// Animation index for `ANIMGUID` (`0x005883E0`): the animation with this `GUID`; without
/// `GUID` entries the index itself; `None` if it does not exist.
pub fn animation_for_guid(config: &ObjectConfig, guid: i32) -> Option<usize> {
    if guid < 0 {
        return None;
    }
    if let Some(i) = config.animations.iter().position(|a| a.guid == Some(guid)) {
        return Some(i);
    }
    let index = guid as usize;
    (config.animations.iter().all(|a| a.guid.is_none()) && index < config.animations.len())
        .then_some(index)
}

/// A model effect.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectEffect {
    desc: ObjectEffectDesc,
    /// Normalized model path.
    pub model: String,
    /// Animation (from `ANIMGUID`), determined by the caller via the `.cfg` of the model.
    animation: Option<usize>,
    active: bool,
    /// Component time (s) and last effect time.
    time: f32,
    last: f32,
    /// Time at which the animation started (component time, s).
    animation_start: f32,
    factor: f32,
}

impl ObjectEffect {
    pub fn new(desc: &ObjectEffectDesc, animation: Option<usize>) -> Self {
        Self {
            desc: desc.clone(),
            model: normalize_path(&desc.object),
            animation,
            active: false,
            time: 0.0,
            last: 0.0,
            animation_start: 0.0,
            factor: FULL_FACTOR,
        }
    }

    /// `Start(ms)`: active, time = ms/1000, animation from the start.
    pub fn start(&mut self, ms: u32) {
        self.active = true;
        self.time = ms as f32 / 1000.0;
        self.last = 0.0;
        self.animation_start = self.time;
        self.factor = FULL_FACTOR;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Fade-in/fade-out factor of the last `update`.
    pub fn factor(&self) -> f32 {
        self.factor
    }

    /// `ObjectEffect_Update(t)` (effect time in s).
    pub fn update(&mut self, t: f32) {
        if self.last == 0.0 {
            self.last = t;
        }
        self.time += t - self.last;
        self.last = t;
        let d = &self.desc;
        let fade_in = if d.fade_in {
            d.fade_in_ms as f32 / 1000.0
        } else {
            0.0
        };
        let visible_until = d.duration_ms as f32 / 1000.0 + fade_in;
        let fade_out = d.fade_out_ms as f32 / 1000.0;
        self.factor = FULL_FACTOR;
        if d.fade_in && self.time < fade_in {
            self.factor = self.time / fade_in;
        }
        if d.fade_out && self.time > visible_until {
            if self.time < visible_until + fade_out {
                self.factor = 1.0 - (self.time - visible_until) / fade_out;
            } else if d.loop_fading {
                self.time = 0.0;
                self.animation_start = 0.0;
                self.factor = 0.0;
            } else {
                self.active = false;
            }
        }
    }

    /// `ObjectEffect_Render`: world = `T(POSITION) * RotY(phi) * T(effect position)` (row vectors),
    /// blend factors and culling of the file, without fog, alpha = fade factor. The animation
    /// runs at `MOVEMENT / 128` (<= 0: normal).
    // Deviation: The animation time follows the component time (original: game time since the
    // start of the animation); both are equal as long as the effect does not freeze.
    pub fn draw(&self, placement: Placement, out: &mut EffectFrame) {
        if !self.active {
            return;
        }
        let d = &self.desc;
        let world = Affine3A::from_translation(placement.position)
            * Affine3A::from_mat3(game_rotation_y(placement.yaw))
            * Affine3A::from_translation(Vec3::from(d.position));
        let speed = if d.movement <= 0 {
            1.0
        } else {
            d.movement as f32 / 128.0
        };
        out.commands.push(DrawCommand::Model(ModelDraw {
            model: self.model.clone(),
            world,
            animation: self
                .animation
                .map(|i| (i, (self.time - self.animation_start) * 1000.0 * speed)),
            tint: None,
            alpha: Some(self.factor),
            blend: Some((d.src_blend, d.dest_blend)),
            cull: d.culling,
            fog: false,
            mesh: None,
            player_color: None,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_formats::object_cfg::AnimationEntry;

    fn desc() -> ObjectEffectDesc {
        ObjectEffectDesc {
            object: "Data\\R3D\\Strahl01.rdo".into(),
            anim_guid: 0,
            culling: false,
            src_blend: 6,
            dest_blend: 7,
            ..ObjectEffectDesc::default()
        }
    }

    #[test]
    fn animation_guids_map_to_indices() {
        let entry = |guid| AnimationEntry {
            file: Some("a.rdm".into()),
            guid,
            max_bounding_box: None,
            sub_objects: vec![],
        };
        let plain = ObjectConfig {
            animations: vec![entry(None), entry(None)],
            ..ObjectConfig::default()
        };
        assert_eq!(animation_for_guid(&plain, 1), Some(1));
        assert_eq!(animation_for_guid(&plain, 2), None);
        assert_eq!(animation_for_guid(&plain, -1), None);
        let tagged = ObjectConfig {
            animations: vec![entry(Some(7)), entry(Some(3))],
            ..ObjectConfig::default()
        };
        assert_eq!(animation_for_guid(&tagged, 3), Some(1));
        assert_eq!(animation_for_guid(&tagged, 0), None);
    }

    #[test]
    fn without_fading_the_factor_stays_just_below_one() {
        let mut e = ObjectEffect::new(&desc(), Some(0));
        e.start(0);
        e.update(0.5);
        e.update(30.0);
        assert!(e.is_active());
        assert_eq!(e.factor(), FULL_FACTOR);
        const { assert!(FULL_FACTOR < 1.0 && FULL_FACTOR > 0.99999) };
        let mut frame = EffectFrame::default();
        let placement = Placement {
            position: Vec3::new(100.0, 0.0, 0.0),
            yaw: 0.0,
        };
        e.draw(placement, &mut frame);
        let DrawCommand::Model(m) = &frame.commands[0] else {
            panic!("model expected");
        };
        assert_eq!(m.model, "data/r3d/strahl01.rdo");
        assert_eq!((m.blend, m.cull, m.fog), (Some((6, 7)), false, false));
        assert_eq!(m.world.translation, Vec3::new(100.0, 0.0, 0.0).into());
        let (index, ms) = m.animation.unwrap();
        assert_eq!(index, 0);
        assert!((ms - 29_500.0).abs() < 0.1);
    }

    #[test]
    fn fades_in_and_out_and_loops_or_ends() {
        let mut e = ObjectEffect::new(
            &ObjectEffectDesc {
                fade_in: true,
                fade_out: true,
                fade_in_ms: 1000,
                duration_ms: 2000,
                fade_out_ms: 1000,
                ..desc()
            },
            None,
        );
        e.start(0);
        e.update(0.1);
        e.update(0.6);
        assert!((e.factor() - 0.5).abs() < 1e-4);
        e.update(2.0);
        assert_eq!(e.factor(), FULL_FACTOR);
        // Visible until 2 + 1 s, then 1 s of fading out.
        e.update(3.6);
        assert!((e.factor() - 0.5).abs() < 1e-4);
        e.update(4.2);
        assert!(!e.is_active());
        let mut looping = ObjectEffect::new(
            &ObjectEffectDesc {
                fade_out: true,
                loop_fading: true,
                duration_ms: 1000,
                fade_out_ms: 1000,
                ..desc()
            },
            None,
        );
        looping.start(0);
        looping.update(0.1);
        looping.update(2.2);
        assert!(looping.is_active());
        assert_eq!(looping.factor(), 0.0);
    }
}
