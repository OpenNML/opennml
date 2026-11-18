//! Output of the effects for the renderer: vertex lists in layout FVF `0x1C2` (stride 28) and
//! draw commands in the drawing order of the original.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Affine3A, Vec3};

/// Vertex FVF `0x1C2`: position, `DIFFUSE` (D3DCOLOR `0xAARRGGBB`), `SPECULAR` (unused, 0),
/// UV; 28 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct EffectVertex {
    pub position: [f32; 3],
    pub color: u32,
    pub specular: u32,
    pub uv: [f32; 2],
}

/// Blend mode of the state blocks `eff_billboard` (`BLENDALPHA` 0: `SRCALPHA`/`ONE`) and
/// `eff_billboard_alpha` (`SRCALPHA`/`INVSRCALPHA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectBlend {
    Additive,
    Alpha,
}

impl EffectBlend {
    pub fn from_blend_alpha(blend_alpha: bool) -> Self {
        if blend_alpha {
            Self::Alpha
        } else {
            Self::Additive
        }
    }
}

/// Coordinate space of the vertices: view space (world = view = identity, only the projection applies;
/// particles and billboards) or world (flat layers with the camera view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSpace {
    View,
    World,
}

/// Triangles with one texture and one state.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadBatch {
    /// Texture path (lowercase, as in the `.pef`); `None` without a texture.
    pub texture: Option<String>,
    pub blend: EffectBlend,
    pub space: EffectSpace,
    /// Vertices in [`EffectFrame::vertices`] (6 per quad).
    pub vertices: Range<u32>,
}

/// A model in the effect pass (model effects, surf, fish, eagle).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDraw {
    /// Model path (normalized).
    pub model: String,
    /// World matrix (original coordinates).
    pub world: Affine3A,
    /// Running animation: index and run time in ms (speed 128 = normal); `None` = bind pose.
    pub animation: Option<(usize, f32)>,
    /// `Material_MakeTinted(s, s, s)`: diffuse = ambient = `(s, s, s)` instead of the material colors.
    pub tint: Option<f32>,
    /// `Some(a)`: all groups in the transparent pass with material alpha `a`
    /// (`Material_SetAlpha`); `None`: like an object (opaque groups first).
    pub alpha: Option<f32>,
    /// `D3DBLEND` values source/destination of the transparent pass; `None` = `SRCALPHA`/
    /// `INVSRCALPHA`.
    pub blend: Option<(i32, i32)>,
    /// `CULLMODE` CCW (discard back faces); `false` = NONE.
    pub cull: bool,
    /// Fog like the objects (`m_EnableFog`); `false` = `FOGENABLE` 0.
    pub fog: bool,
    /// Only this mesh of the model, without its node position (flying arrow from the shooter model);
    /// `None` = the whole model.
    pub mesh: Option<String>,
    /// Player color of the first `EF-MATERIAL` (`FUN_005884C0`, like `InstanceLook::player_color` of the
    /// renderer); `None` = the `.rdo` color as with all effects (D-221).
    /// Set only by the rally point flag (`PropertyBuilding_RenderMeetingFlag` `0x00507BB0`).
    pub player_color: Option<[f32; 3]>,
}

/// A draw command.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    Quads(QuadBatch),
    Model(ModelDraw),
}

/// Everything that the effects of a frame draw, in drawing order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectFrame {
    pub vertices: Vec<EffectVertex>,
    pub commands: Vec<DrawCommand>,
}

impl EffectFrame {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.commands.clear();
    }

    /// Appends quads (6 vertices each); empty lists create no command.
    pub fn push_quads(
        &mut self,
        texture: Option<String>,
        blend: EffectBlend,
        space: EffectSpace,
        vertices: impl IntoIterator<Item = EffectVertex>,
    ) {
        let first = self.vertices.len() as u32;
        self.vertices.extend(vertices);
        let last = self.vertices.len() as u32;
        if last > first {
            self.commands.push(DrawCommand::Quads(QuadBatch {
                texture,
                blend,
                space,
                vertices: first..last,
            }));
        }
    }

    /// Number of drawn quads.
    pub fn quad_count(&self) -> usize {
        self.vertices.len() / 6
    }

    /// Number of drawn models.
    pub fn model_count(&self) -> usize {
        self.commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Model(_)))
            .count()
    }
}

/// A footprint or crater mark of layer 4 (`FootprintEffect_Render` `0x004F1B90`): a
/// terrain decal `TerrainDecal_DrawSquare(position, yaw, (alpha << 24) | 0xFFFFFF, r, 0)` (`0x0054C1D0`),
/// square `+-r` around the position, rotated by the yaw, in state `Footprint`.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundDecal {
    /// Texture path in original spelling (`Data/R3D/Nature/Summer/Footsteps/...`) from the fixed
    /// table [`crate::marks::Footprint::texture`].
    pub texture: &'static str,
    pub position: Vec3,
    /// Rotation of the mark (yaw of the unit or crater, radians).
    pub yaw: f32,
    /// Half edge length `r`.
    pub radius: f32,
    /// Vertex alpha (color white).
    pub alpha: u8,
}

/// A quad of a tread mark (`0x004F5560`, triangle strip of four corners): `corners` in
/// strip order A (previous position + side), B (previous position - side), C (position + side), D (position -
/// side), corners at terrain height; black, without texture. `alpha[0]` applies to A and B, `alpha[1]`
/// to C and D.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreadQuad {
    pub corners: [Vec3; 4],
    pub alpha: [u8; 2],
}

/// Everything that layer 4 (ground marks, `GameEngine_RenderWorld` step 7) draws in a frame:
/// footprints and craters as decals, tread marks as strips.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroundMarks {
    pub decals: Vec<GroundDecal>,
    pub treads: Vec<TreadQuad>,
}

impl GroundMarks {
    pub fn is_empty(&self) -> bool {
        self.decals.is_empty() && self.treads.is_empty()
    }
}

/// `D3DCOLOR` like `(int)a << 24 | (int)r << 16 | (int)g << 8 | (int)b` with truncation
/// (`_ftol`) and without masking: values above 255 run into the next byte as in the original.
pub fn d3d_color(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let t = |v: f32| v as i32 as u32;
    (t(a) << 24) | (t(r) << 16) | (t(g) << 8) | t(b)
}

/// Corners of a quad with half width `w` and height `h` as two triangles clockwise
/// (`(-w, h) (w, h) (w, -h) | (-w, -h) (-w, h) (w, -h)`) and their UVs.
pub const QUAD_CORNERS: [[f32; 2]; 6] = [
    [-1.0, 1.0],
    [1.0, 1.0],
    [1.0, -1.0],
    [-1.0, -1.0],
    [-1.0, 1.0],
    [1.0, -1.0],
];
pub const QUAD_UVS: [[f32; 2]; 6] = [
    [0.0, 0.0],
    [1.0, 0.0],
    [1.0, 1.0],
    [0.0, 1.0],
    [0.0, 0.0],
    [1.0, 1.0],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_truncated_and_not_masked() {
        assert_eq!(d3d_color(255.0, 254.9, 1.5, 0.0), 0xFFFE_0100);
        // 256 in red runs into the alpha byte (original without mask).
        assert_eq!(d3d_color(0.0, 256.0, 0.0, 0.0), 0x0100_0000);
        assert_eq!(std::mem::size_of::<EffectVertex>(), 28);
    }

    #[test]
    fn empty_quad_lists_make_no_command() {
        let mut frame = EffectFrame::default();
        frame.push_quads(None, EffectBlend::Alpha, EffectSpace::View, []);
        assert!(frame.commands.is_empty());
        let v = EffectVertex {
            position: [0.0; 3],
            color: 0,
            specular: 0,
            uv: [0.0; 2],
        };
        frame.push_quads(
            Some("a.tga".into()),
            EffectBlend::Additive,
            EffectSpace::World,
            [v; 6],
        );
        assert_eq!(frame.quad_count(), 1);
        assert_eq!(
            frame.commands,
            [DrawCommand::Quads(QuadBatch {
                texture: Some("a.tga".into()),
                blend: EffectBlend::Additive,
                space: EffectSpace::World,
                vertices: 0..6,
            })]
        );
    }
}
