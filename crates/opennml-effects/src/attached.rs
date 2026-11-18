//! AttachedEffect (`0x9C` bytes, vtable `0x00620E68`): the wrapper around an effect instance with
//! lifetime and attachment to a point, an object or a helper node
//! (`AttachedEffect_Tick` `0x004F3A00`, `AttachedEffect_FrameUpdate` `0x004F3A50`).

use glam::Vec3;
use opennml_world::math::{game_rotation_x, game_rotation_y, object_world_matrix};

use crate::effect::EffectInstance;
use crate::particles::Placement;

/// Unlimited lifetime (`0x7FFFFFFF`).
pub const UNLIMITED_MS: i32 = i32::MAX;
/// Game tick of the objects and world effects: 100 ms game time.
pub const TICK_MS: u32 = 100;

/// Position of a carrier object in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Carrier {
    pub position: Vec3,
    /// `OBJ_XROT/YROT/ZROT` (radians).
    pub rotation: Vec3,
    /// Helper node in object space, if the effect hangs on one and it exists.
    pub node: Option<Vec3>,
}

/// Attachment.
#[derive(Debug, Clone, PartialEq)]
pub enum Anchor {
    /// Fixed (`SetPositionAndRotation`, world effects).
    Fixed(Placement),
    /// On the object with offset (`AttachToObject`); `with_rotation` = `+0x98`.
    Object { offset: Vec3, with_rotation: bool },
    /// On the helper node (`AttachToNode`, follows the animation).
    Node(String),
}

/// Target of the particles of an effect.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectTarget {
    /// Fixed point (`AttachedEffect_SetTarget` `0x004F3660`, only user: animal evoker):
    /// `Effect_SetTarget` once when set.
    Point(Vec3),
    /// Helper node of a target object (`AttachedEffect_SetTargetNode` `0x004F3DE0`: `+0x30` node,
    /// `+0x35` "follow target"; only user: ghost with `follow = true`). The target object (`+0x28`)
    /// is held by [`crate::world::EffectWorld`]; `AttachedEffect_FrameUpdate` (`0x004F3A50`) sets the
    /// target each frame from the node position (without a node, the object position).
    Node { node: String, follow: bool },
}

/// Effect with lifetime and attachment.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedEffect {
    pub effect: EffectInstance,
    /// `+0x20`: remaining lifetime (ms).
    remaining_ms: i32,
    pub anchor: Anchor,
    /// `+0x88`: additional rotation about y.
    pub yaw: f32,
    /// Target of the particles ([`AttachedEffect::set_target`]).
    pub target: Option<EffectTarget>,
}

impl AttachedEffect {
    /// `effect` has already been started (`Effect_Start(0)`).
    pub fn new(effect: EffectInstance, lifetime_ms: i32, anchor: Anchor, yaw: f32) -> Self {
        Self {
            effect,
            remaining_ms: lifetime_ms,
            anchor,
            yaw,
            target: None,
        }
    }

    /// Sets the target: a fixed point goes to `Effect_SetTarget` immediately (measured against the
    /// current effect position, so before the first position against the map origin; the first position measures
    /// again before a particle is created, `Effect_SetPosition` `0x0057C4E0`); a
    /// node is not evaluated until the next frame.
    pub fn set_target(&mut self, target: EffectTarget) {
        if let EffectTarget::Point(z) = &target {
            self.effect.set_target(*z);
        }
        self.target = Some(target);
    }

    /// Node name of the attachment.
    pub fn node(&self) -> Option<&str> {
        match &self.anchor {
            Anchor::Node(name) => Some(name),
            _ => None,
        }
    }

    /// `AttachedEffect_Tick`: lifetime -100 ms; `false` = delete (expired, or no
    /// component active any more and no particle any more; then all emitters are stopped).
    pub fn tick(&mut self) -> bool {
        self.remaining_ms = self.remaining_ms.saturating_sub(TICK_MS as i32);
        if self.remaining_ms <= 0 {
            return false;
        }
        self.effect.is_active() || self.effect.stop_and_count() > 0
    }

    /// `AttachedEffect_Stop` (vtable `+0x0C`): all components inactive immediately.
    pub fn stop(&mut self) {
        self.effect.stop_and_count();
    }

    /// Position from the attachment (`AttachedEffect_FrameUpdate`): on the node `node * world`
    /// (`Ry * Rz * Rx * T`, without scaling), otherwise object position + offset (with object rotation
    /// `* RotX * RotY`); rotation = additional rotation + object rotation.
    pub fn placement(&self, carrier: Option<&Carrier>) -> Placement {
        let (Some(c), anchor) = (carrier, &self.anchor) else {
            return self.effect.placement();
        };
        match anchor {
            Anchor::Fixed(placement) => *placement,
            Anchor::Node(_) => Placement {
                position: match c.node {
                    Some(local) => object_world_matrix(c.position, c.rotation, Vec3::ONE)
                        .transform_point3(local),
                    None => c.position,
                },
                yaw: self.yaw + c.rotation.y,
            },
            Anchor::Object {
                offset,
                with_rotation,
            } => {
                if *with_rotation {
                    Placement {
                        position: c.position
                            + game_rotation_y(c.rotation.y)
                                * (game_rotation_x(c.rotation.x) * *offset),
                        yaw: self.yaw + c.rotation.y,
                    }
                } else {
                    Placement {
                        position: c.position + *offset,
                        yaw: self.yaw,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::NoAssets;
    use opennml_formats::pef::{ParticleDesc, PefComponent, PefFile};

    /// One-shot emitter: 10/s for 1 s, life 0.5 s.
    fn effect() -> EffectInstance {
        let pef = PefFile {
            movement_time: 1.0,
            components: vec![PefComponent::Particles(ParticleDesc {
                emit: 10,
                duration: 1.0,
                life: 0.5,
                accel: 1.0,
                ..ParticleDesc::default()
            })],
        };
        let mut e = EffectInstance::new("x.pef", &pef, true, &mut NoAssets);
        e.start(0, 0);
        e
    }

    #[test]
    fn lifetime_and_finished_effects_end_the_effect() {
        let mut short = AttachedEffect::new(
            effect(),
            250,
            Anchor::Fixed(Placement {
                position: Vec3::ZERO,
                yaw: 0.0,
            }),
            0.0,
        );
        assert!(short.tick() && short.tick());
        assert!(!short.tick());
        // Unlimited: ends as soon as nothing is active any more and no particle lives.
        let mut once = AttachedEffect::new(effect(), UNLIMITED_MS, Anchor::Node("a01".into()), 0.0);
        let mut rng = opennml_world::rand::MsvcRand::new(1);
        let here = once.effect.placement();
        for now in (50..=1000).step_by(50) {
            once.effect.advance(now, here, true, &mut rng);
        }
        assert!(once.tick());
        for now in (1050..=1700).step_by(50) {
            once.effect.advance(now, here, true, &mut rng);
        }
        assert!(!once.tick());
        assert_eq!(once.node(), Some("a01"));
    }

    #[test]
    fn placements_follow_nodes_offsets_and_object_rotation() {
        let half = std::f32::consts::FRAC_PI_2;
        let carrier = Carrier {
            position: Vec3::new(100.0, 0.0, 0.0),
            rotation: Vec3::new(0.0, half, 0.0),
            node: Some(Vec3::new(0.0, 10.0, 5.0)),
        };
        let node = AttachedEffect::new(effect(), UNLIMITED_MS, Anchor::Node("n01".into()), 0.25);
        let p = node.placement(Some(&carrier));
        // (0, 10, 5) * RotY(90 degrees) = (-5, 10, 0).
        assert!((p.position - Vec3::new(95.0, 10.0, 0.0)).length() < 1e-4);
        assert_eq!(p.yaw, 0.25 + half);
        let missing = Carrier {
            node: None,
            ..carrier
        };
        assert_eq!(node.placement(Some(&missing)).position, carrier.position);
        let offset = Anchor::Object {
            offset: Vec3::new(0.0, 0.0, 10.0),
            with_rotation: true,
        };
        let fire = AttachedEffect::new(effect(), UNLIMITED_MS, offset, 0.0);
        assert!(
            (fire.placement(Some(&carrier)).position - Vec3::new(90.0, 0.0, 0.0)).length() < 1e-4
        );
        let plain = AttachedEffect::new(
            effect(),
            UNLIMITED_MS,
            Anchor::Object {
                offset: Vec3::new(0.0, 0.0, 10.0),
                with_rotation: false,
            },
            0.0,
        );
        let p = plain.placement(Some(&carrier));
        assert_eq!((p.position, p.yaw), (Vec3::new(100.0, 0.0, 10.0), 0.0));
    }
}
