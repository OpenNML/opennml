//! Billboard effect like `BillboardEffect_Update` (`0x00598AA0`), `BillboardLayer_Update`
//! (`0x005979D0`), `BillboardLayer_Render` (`0x00598E50`) and
//! `BillboardLayer_RenderDirectional` (`0x00597B10`), pseudocode in re/effects.

use glam::{Mat3, Mat4, Vec3};
use nml_formats::pef::{BillboardDesc, BillboardLayerDesc, CURVE_POINTS, Curve};
use nml_world::math::{game_rotation_x, game_rotation_y, game_rotation_z};

use crate::output::{
    EffectBlend, EffectFrame, EffectSpace, EffectVertex, QUAD_CORNERS, QUAD_UVS, d3d_color,
};
use crate::particles::{Placement, rotate_y};

/// State of a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerState {
    time: f32,
    last: f32,
    visible: bool,
    alive: bool,
    i0: usize,
    i1: usize,
    frac: f32,
    frame: usize,
}

/// One layer: description with scalable size and state.
#[derive(Debug, Clone, PartialEq)]
pub struct BillboardLayer {
    desc: BillboardLayerDesc,
    /// `SIZE` x/y (world units), multiplied by the effect scaling.
    size: [f32; 2],
    state: LayerState,
}

impl BillboardLayer {
    pub fn visible(&self) -> bool {
        self.state.visible
    }

    pub fn desc(&self) -> &BillboardLayerDesc {
        &self.desc
    }

    pub fn size(&self) -> [f32; 2] {
        self.size
    }

    /// Curve value, linear between the control points `i0` and `i1`.
    fn curve(&self, curve: &Curve) -> f32 {
        let s = &self.state;
        let a = curve.values[s.i0.min(CURVE_POINTS - 1)];
        let b = curve.values[s.i1.min(CURVE_POINTS - 1)];
        a + (b - a) * s.frac
    }

    /// `BillboardLayer_Update` (`0x005979D0`); `true` = layer is alive.
    fn update(&mut self, t: f32) -> bool {
        let d = &self.desc;
        let s = &mut self.state;
        if s.last == 0.0 {
            s.last = t;
        }
        s.time += t - s.last;
        s.last = t;
        if s.time < d.start {
            s.visible = false;
            return true;
        }
        let u = s.time - d.start;
        if u < d.duration {
            let f = u * 99.0 / d.duration;
            let (mut i0, mut i1, mut frac) = (f.floor() as usize, f.ceil() as usize, 0.0);
            if i1 > 99 {
                (i0, i1) = (0, 1);
            } else {
                frac = f - i0 as f32;
            }
            s.i0 = i0;
            s.i1 = i1;
            s.frac = frac;
            s.frame = (u * d.textures.len() as f32 / d.duration) as usize;
            s.visible = true;
            true
        } else if u - d.duration < d.pause {
            s.visible = false;
            true
        } else if d.looping && s.time >= d.start + d.duration + d.pause {
            // The frame of the last call stays.
            s.time = d.start;
            true
        } else {
            s.visible = false;
            false
        }
    }

    /// Position, size, color, rotation of the layer for the corners.
    fn draw(&self, view: Mat4, base: Vec3, placement: Placement, out: &mut EffectFrame) {
        if !self.state.visible {
            return;
        }
        let d = &self.desc;
        let texture = d
            .textures
            .get(self.state.frame.min(d.textures.len().saturating_sub(1)))
            .cloned()
            .flatten();
        let w = self.curve(&d.x_scale) * self.size[0];
        let h = self.curve(&d.y_scale) * self.size[1];
        let [r, g, b] = d.color;
        let color = d3d_color(self.curve(&d.fading), r, g, b);
        let rotation = self.curve(&d.rotation);
        let offset = Vec3::from(d.offset);
        let phi = placement.yaw;
        let mut p = base + offset;
        let blend = EffectBlend::from_blend_alpha(d.blend_alpha);
        if d.directional {
            if phi != 0.0 {
                p = rotate_y(p, phi);
            }
            p += placement.position;
            let movement = Vec3::new(
                self.curve(&d.x_move),
                self.curve(&d.move_ud),
                self.curve(&d.y_move),
            );
            p += if phi != 0.0 {
                rotate_y(movement, phi)
            } else {
                movement
            };
            // Corner * RotY(ROTATION) * RotY(phi) (with ROTOFF about OFFSET), afterwards - as in the original
            // in a loop of its own - with tilt angles * RotZ * RotX.
            let m: Mat3 = game_rotation_y(phi) * game_rotation_y(rotation);
            let tilt = (d.tilt_x != 0.0 || d.tilt_z != 0.0)
                .then(|| game_rotation_x(d.tilt_x) * game_rotation_z(d.tilt_z));
            let vertices = QUAD_CORNERS.iter().zip(QUAD_UVS).map(|(c, uv)| {
                let corner = Vec3::new(c[0] * w, 0.0, c[1] * h);
                let mut corner = if d.rot_off {
                    m * (corner + offset) - offset
                } else {
                    m * corner
                };
                if let Some(tilt) = tilt {
                    corner = tilt * corner;
                }
                EffectVertex {
                    position: (p + corner).to_array(),
                    color,
                    specular: 0,
                    uv,
                }
            });
            out.push_quads(texture, blend, EffectSpace::World, vertices);
        } else {
            if phi != 0.0 && p != Vec3::ZERO {
                p = rotate_y(p, phi);
            }
            p += placement.position;
            let m = view.transform_point3(p)
                + Vec3::new(self.curve(&d.x_move), self.curve(&d.y_move), 0.0);
            let rz = game_rotation_z(rotation);
            let vertices = QUAD_CORNERS.iter().zip(QUAD_UVS).map(|(c, uv)| {
                let corner = Vec3::new(c[0] * w, c[1] * h, 0.0);
                let corner = if d.rot_off {
                    rz * (corner + offset) - offset
                } else {
                    rz * corner
                };
                EffectVertex {
                    position: (m + corner).to_array(),
                    color,
                    specular: 0,
                    uv,
                }
            });
            out.push_quads(texture, blend, EffectSpace::View, vertices);
        }
    }
}

/// Billboard component: origin and layers.
#[derive(Debug, Clone, PartialEq)]
pub struct BillboardEffect {
    position: Vec3,
    layers: Vec<BillboardLayer>,
    active: bool,
}

impl BillboardEffect {
    /// `texture_size` returns the pixel size of a texture (for layers without `SIZE`; without a
    /// loadable texture 64 x 64 like `BillboardLayerDesc_SetTextureCount` `0x00598CC0`).
    pub fn new(
        desc: &BillboardDesc,
        texture_size: &mut dyn FnMut(&str) -> Option<[u32; 2]>,
    ) -> Self {
        let layers = desc
            .layers
            .iter()
            .map(|l| {
                let size = l.size.map(|s| s.map(|v| v as f32)).unwrap_or_else(|| {
                    l.textures
                        .first()
                        .cloned()
                        .flatten()
                        .and_then(|t| texture_size(&t))
                        .map_or([64.0; 2], |s| s.map(|v| v as f32))
                });
                BillboardLayer {
                    desc: l.clone(),
                    size,
                    state: LayerState {
                        time: 0.0,
                        last: 0.0,
                        visible: false,
                        alive: true,
                        i0: 0,
                        i1: 0,
                        frac: 0.0,
                        frame: 0,
                    },
                }
            })
            .collect();
        Self {
            position: Vec3::from(desc.position),
            layers,
            active: false,
        }
    }

    pub fn layers(&self) -> &[BillboardLayer] {
        &self.layers
    }

    /// `BillboardEffect_Start` (`0x005989C0`): layer time = ms/1000.
    pub fn start(&mut self, ms: u32) {
        self.active = true;
        for layer in &mut self.layers {
            layer.state.time = ms as f32 / 1000.0;
            layer.state.last = 0.0;
            layer.state.alive = true;
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Scaling by the factor new/old: `SIZE` x/y of the layers.
    pub fn scale(&mut self, factor: f32) {
        for layer in &mut self.layers {
            layer.size = layer.size.map(|s| s * factor);
        }
    }

    /// `BillboardEffect_Update` (`0x00598AA0`): if no layer is alive any more, the component becomes
    /// inactive; inactive (also after `Stop`) it does nothing.
    pub fn update(&mut self, t: f32) {
        if !self.active {
            return;
        }
        let mut any = false;
        for layer in &mut self.layers {
            layer.state.alive = layer.update(t);
            any |= layer.state.alive;
        }
        if !any {
            self.active = false;
        }
    }

    /// `BillboardEffect_Render` (`0x00599760`): layers in file order; inactive nothing (a
    /// stopped ember disappears immediately).
    pub fn draw(&self, view: Mat4, placement: Placement, out: &mut EffectFrame) {
        if !self.active {
            return;
        }
        for layer in &self.layers {
            layer.draw(view, self.position, placement, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(duration: f32, pause: f32, looping: bool) -> BillboardLayerDesc {
        let mut ramp = Curve::default();
        for (i, v) in ramp.values.iter_mut().enumerate() {
            *v = i as f32;
        }
        let one = Curve {
            values: [1.0; CURVE_POINTS],
        };
        let fading = Curve {
            values: [200.0; CURVE_POINTS],
        };
        BillboardLayerDesc {
            textures: vec![Some("a.tga".into()), Some("b.tga".into())],
            start: 0.5,
            duration,
            pause,
            looping,
            color: [255.0, 128.0, 0.0],
            size: Some([10, 20]),
            x_scale: one.clone(),
            y_scale: one,
            fading,
            x_move: ramp,
            ..BillboardLayerDesc::default()
        }
    }

    fn effect(layers: Vec<BillboardLayerDesc>) -> BillboardEffect {
        let desc = BillboardDesc {
            position: [0.0, 5.0, 0.0],
            layers,
            ..BillboardDesc::default()
        };
        BillboardEffect::new(&desc, &mut |_| None)
    }

    const AT_ORIGIN: Placement = Placement {
        position: Vec3::ZERO,
        yaw: 0.0,
    };

    #[test]
    fn layers_wait_play_pause_and_end() {
        let mut bb = effect(vec![layer(1.0, 0.5, false)]);
        bb.start(0);
        bb.update(0.25);
        // First call: last = t, layer time 0 < START.
        assert!(!bb.layers()[0].visible() && bb.is_active());
        bb.update(1.0);
        // Layer time 0.75: u = 0.25, f = 24.75, frame 0 of 2.
        let s = &bb.layers()[0].state;
        assert!(s.visible && (s.i0, s.i1, s.frame) == (24, 25, 0));
        assert!((s.frac - 0.75).abs() < 1e-4);
        bb.update(1.3);
        assert_eq!(bb.layers()[0].state.frame, 1);
        bb.update(1.9);
        // Layer time 1.65, u = 1.15: pause.
        assert!(!bb.layers()[0].visible() && bb.is_active());
        bb.update(2.4);
        // After the pause without LOOP: dead, component inactive.
        assert!(!bb.is_active());
        // Stopped: draw nothing any more.
        let mut bb = effect(vec![layer(1.0, 0.0, true)]);
        bb.start(0);
        bb.update(0.1);
        bb.update(0.7);
        bb.stop();
        let mut frame = EffectFrame::default();
        bb.draw(Mat4::IDENTITY, AT_ORIGIN, &mut frame);
        assert_eq!(frame.quad_count(), 0);
    }

    #[test]
    fn looping_layers_restart_and_keep_the_last_picture() {
        let mut bb = effect(vec![layer(1.0, 0.0, true)]);
        bb.start(0);
        bb.update(0.1);
        bb.update(1.45);
        let before = bb.layers()[0].state.clone();
        assert!(before.visible);
        bb.update(1.6);
        let after = &bb.layers()[0].state;
        assert_eq!(after.time, 0.5);
        assert!(after.visible && after.i0 == before.i0);
        assert!(bb.is_active());
    }

    #[test]
    fn layers_without_size_take_the_texture_size_or_64() {
        let mut desc = layer(1.0, 0.0, false);
        desc.size = None;
        let bb = BillboardEffect::new(
            &BillboardDesc {
                layers: vec![desc.clone(), desc.clone()],
                ..BillboardDesc::default()
            },
            &mut |t| (t == "a.tga").then_some([32, 16]),
        );
        assert_eq!(bb.layers()[0].size(), [32.0, 16.0]);
        desc.textures[0] = Some("missing.tga".into());
        let bb = BillboardEffect::new(
            &BillboardDesc {
                layers: vec![desc],
                ..BillboardDesc::default()
            },
            &mut |_| None,
        );
        assert_eq!(bb.layers()[0].size(), [64.0, 64.0]);
    }

    #[test]
    fn billboards_face_the_camera_and_directional_layers_lie_flat() {
        let mut bb = effect(vec![layer(1.0, 0.0, true)]);
        bb.start(0);
        bb.update(0.1);
        bb.update(0.7);
        let view = Mat4::from_translation(Vec3::new(0.0, 0.0, 100.0));
        let mut frame = EffectFrame::default();
        bb.draw(view, AT_ORIGIN, &mut frame);
        // u = 0.1: f = 9.9 -> XMOVE 9.9; half size 10 x 20, center in view space (9.9, 5, 100).
        assert_eq!(frame.quad_count(), 1);
        let v0 = frame.vertices[0].position;
        assert!((v0[0] - (9.9 - 10.0)).abs() < 1e-3 && (v0[1] - 25.0).abs() < 1e-3);
        assert_eq!(v0[2], 100.0);
        assert_eq!(frame.vertices[0].color, 0xC8FF_8000);
        let mut flat = layer(1.0, 0.0, true);
        flat.directional = true;
        flat.x_move = Curve::default();
        let mut bb = effect(vec![flat]);
        bb.start(0);
        bb.update(0.1);
        bb.update(0.7);
        let mut frame = EffectFrame::default();
        bb.draw(view, AT_ORIGIN, &mut frame);
        // Lying flat in world coordinates: corner (-10, 0, 20) about (0, 5, 0).
        assert_eq!(frame.vertices[0].position, [-10.0, 5.0, 20.0]);
        let crate::output::DrawCommand::Quads(batch) = &frame.commands[0] else {
            panic!("quads expected");
        };
        assert_eq!(batch.space, EffectSpace::World);
    }

    // QUIRK D-98: kept
    #[test]
    fn tilt_is_applied_after_the_rotoff_step() {
        // ROTATION 0, phi 0: M = identity, the corner stays (M*(c + o) - o = c). Original: afterwards
        // the tilt T = RotX(90 degrees) on this corner, T*(x, y, z) = (x, z, -y).
        let mut tilted = layer(1.0, 0.0, true);
        tilted.directional = true;
        tilted.x_move = Curve::default();
        tilted.rot_off = true;
        tilted.offset = [0.0, 0.0, 5.0];
        tilted.tilt_x = std::f32::consts::FRAC_PI_2;
        let mut bb = effect(vec![tilted]);
        bb.start(0);
        bb.update(0.1);
        bb.update(0.7);
        let view = Mat4::from_translation(Vec3::new(0.0, 0.0, 100.0));
        let mut frame = EffectFrame::default();
        bb.draw(view, AT_ORIGIN, &mut frame);
        // Corner (-10, 0, 20) -> T -> (-10, 20, 0); p = (0, 5, 0) + OFFSET = (0, 5, 5).
        // (Tilting before subtracting OFFSET would give (-10, 30, 0).)
        let v = Vec3::from(frame.vertices[0].position);
        assert!((v - Vec3::new(-10.0, 25.0, 5.0)).length() < 1e-3, "{v:?}");
    }
}
