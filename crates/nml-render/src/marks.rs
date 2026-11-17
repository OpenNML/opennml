//! Ground marks of layer 4 (`GameEngine_RenderWorld` step 7, `EffectManager_RenderLayer(4)`
//! `0x004F21F0`, re/renderer): footprints and craters (`FootprintEffect_Render` `0x004F1B90`)
//! as rotated terrain decals (`TerrainDecal_DrawSquare` `0x0054C1D0`, white * alpha) and wheel
//! treads (`0x004F5560`) as black strips without a texture, both in state `Footprint`
//! (`shaders/marks.wgsl`) after the terrain and **before** the shadows. The geometry is rebuilt
//! every frame from [`GroundMarks`].
//!
//! Deviation: the original draws the wheel tread with the normal projection (only `FootprintEffect`
//! sets `g_matGameProjectionDepthBias`) and relies on `ZBIAS` 16 of the state. `ZBIAS` is not
//! reproduced in the reimplementation (as with state `Shadow`); as a substitute, the wheel tread also
//! takes the projection with depth offset, otherwise the strip would fight the terrain over depth.

use std::ops::Range;

use nml_effects::output::{GroundMarks, TreadQuad};
use nml_rda::Vfs;
use nml_world::markers::push_ring;
use nml_world::shadows::{DecalVertex, TileMask};
use nml_world::terrain::Terrain;

use crate::markers::DecalTextures;
use crate::shadows::{DECAL_ATTRIBUTES, DecalVertexGpu, upload};
use crate::shared::{ALPHA_BLEND, PipelineDesc, Shared, pipeline, shader};

/// Counts of the ground marks of a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroundMarkStats {
    /// Drawn decals (marks with a loadable texture).
    pub decals: usize,
    /// Drawn tread quads.
    pub tread_quads: usize,
}

/// Pipelines, textures and geometry of the ground marks.
pub struct GroundMarkGpu {
    footprint_pipeline: wgpu::RenderPipeline,
    tread_pipeline: wgpu::RenderPipeline,
    textures: DecalTextures,
    /// Bind group per texture path of the mark table (at most eight paths, looked up every frame
    /// without allocation; `None` = not loadable).
    texture_of: Vec<(&'static str, Option<usize>)>,
    decal_buffer: Option<wgpu::Buffer>,
    /// Draw ranges: for each sequence of consecutive decals with the same texture, texture and
    /// corners (the blend order stays that of the decals).
    decal_draws: Vec<(usize, Range<u32>)>,
    /// Corners of the decals and working list of one decal, reused across frames.
    decal_vertices: Vec<DecalVertexGpu>,
    scratch: Vec<DecalVertex>,
    tread_buffer: Option<wgpu::Buffer>,
    tread_vertices: u32,
    /// All tiles of the map (`TerrainDecal_Draw` checks only the map boundaries).
    all: Option<((usize, usize), TileMask)>,
    stats: GroundMarkStats,
}

impl GroundMarkGpu {
    pub fn new(device: &wgpu::Device, shared: &Shared, color_format: wgpu::TextureFormat) -> Self {
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DecalVertexGpu>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DECAL_ATTRIBUTES,
        };
        let module = shader(device, "Ground marks", include_str!("shaders/marks.wgsl"));
        let with_texture = [&shared.frame_layout, &shared.textures_layout];
        let without_texture = [&shared.frame_layout];
        let state = |label, layouts, fragment| PipelineDesc {
            label,
            layouts,
            module: &module,
            vertex: "vs_mark",
            fragment,
            buffers: std::slice::from_ref(&layout),
            blend: Some(ALPHA_BLEND),
            depth_write: false,
            cull: true,
            color_format,
        };
        let footprint_pipeline = pipeline(
            device,
            &state("Ground mark (Footprint)", &with_texture, "fs_footprint"),
        );
        let tread_pipeline = pipeline(
            device,
            &state("Tread (Footprint)", &without_texture, "fs_tread"),
        );
        Self {
            footprint_pipeline,
            tread_pipeline,
            textures: DecalTextures::new("Track texture"),
            texture_of: Vec::new(),
            decal_buffer: None,
            decal_draws: Vec::new(),
            decal_vertices: Vec::new(),
            scratch: Vec::new(),
            tread_buffer: None,
            tread_vertices: 0,
            all: None,
            stats: GroundMarkStats::default(),
        }
    }

    /// Builds the ground marks of a frame: for each decal a square `±r` around the position, rotated
    /// by the yaw (texture matrix `[[c, s], [−s, c]]`, [`push_ring`]), on all terrain tiles with
    /// color `(255, 255, 255, α)`; for each tread quad two triangles `(A, B, C)`, `(C, B, D)` like the
    /// triangle strip of the original, black with the alpha of its corners. Decals with a texture
    /// that cannot be loaded are dropped (reported once). Consecutive decals with the same texture
    /// share one draw call; since their corners are contiguous, the blend order stays the same.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        terrain: &Terrain,
        marks: &GroundMarks,
    ) {
        self.decal_draws.clear();
        self.tread_vertices = 0;
        self.stats = GroundMarkStats::default();
        if marks.is_empty() {
            return;
        }
        let (dim_x, dim_z) = (terrain.dim_x(), terrain.dim_z());
        let all = match self.all.take() {
            Some((dims, mask)) if dims == (dim_x, dim_z) => mask,
            _ => TileMask::all(dim_x, dim_z),
        };
        let mut decals = std::mem::take(&mut self.decal_vertices);
        let mut scratch = std::mem::take(&mut self.scratch);
        decals.clear();
        let mut drawn = 0;
        // Texture of the previous decal: sequences of the same texture query the table only once.
        let mut previous: Option<(&'static str, Option<usize>)> = None;
        for d in &marks.decals {
            let texture = match previous {
                Some((path, t)) if path == d.texture => t,
                _ => {
                    let t = self.texture_index(device, queue, shared, vfs, d.texture);
                    previous = Some((d.texture, t));
                    t
                }
            };
            let Some(texture) = texture else {
                continue;
            };
            scratch.clear();
            push_ring(
                terrain,
                &all,
                d.radius,
                d.position,
                d.yaw,
                [255, 255, 255, d.alpha],
                false,
                &mut scratch,
            );
            if scratch.is_empty() {
                continue;
            }
            drawn += 1;
            let start = decals.len() as u32;
            decals.extend(scratch.iter().map(|&v| DecalVertexGpu::from(v)));
            push_draw(&mut self.decal_draws, texture, start..decals.len() as u32);
        }
        self.all = Some(((dim_x, dim_z), all));
        let treads: Vec<DecalVertexGpu> = marks.treads.iter().flat_map(tread_triangles).collect();
        self.tread_vertices = treads.len() as u32;
        self.stats = GroundMarkStats {
            decals: drawn,
            tread_quads: marks.treads.len(),
        };
        upload(
            device,
            queue,
            &mut self.decal_buffer,
            "Ground marks",
            bytemuck::cast_slice(&decals),
        );
        self.decal_vertices = decals;
        self.scratch = scratch;
        upload(
            device,
            queue,
            &mut self.tread_buffer,
            "Treads",
            bytemuck::cast_slice(&treads),
        );
    }

    /// Draws the prepared ground marks (after the terrain, before the shadows): first the
    /// treads, then the decals.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared) {
        if self.decal_draws.is_empty() && self.tread_vertices == 0 {
            return;
        }
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        if let Some(buffer) = self
            .tread_buffer
            .as_ref()
            .filter(|_| self.tread_vertices > 0)
        {
            pass.set_pipeline(&self.tread_pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..self.tread_vertices, 0..1);
        }
        if let Some(buffer) = self
            .decal_buffer
            .as_ref()
            .filter(|_| !self.decal_draws.is_empty())
        {
            pass.set_pipeline(&self.footprint_pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            for (texture, vertices) in &self.decal_draws {
                pass.set_bind_group(1, self.textures.bind_group(*texture), &[]);
                pass.draw(vertices.clone(), 0..1);
            }
        }
    }

    /// Bind group of the mark texture `path` (loaded on first use via [`DecalTextures::get`]).
    fn texture_index(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        path: &'static str,
    ) -> Option<usize> {
        if let Some(&(_, t)) = self.texture_of.iter().find(|(p, _)| *p == path) {
            return t;
        }
        let t = self.textures.get(device, queue, shared, vfs, path);
        self.texture_of.push((path, t));
        t
    }

    /// Counts of the last [`GroundMarkGpu::prepare`].
    pub fn stats(&self) -> GroundMarkStats {
        self.stats
    }
}

/// Appends the corners `vertices` of a decal with the texture `texture`: to the last range if it
/// has the same texture and ends directly before it, otherwise as a new range.
fn push_draw(draws: &mut Vec<(usize, Range<u32>)>, texture: usize, vertices: Range<u32>) {
    match draws.last_mut() {
        Some((t, range)) if *t == texture && range.end == vertices.start => {
            range.end = vertices.end;
        }
        _ => draws.push((texture, vertices)),
    }
}

/// Two triangles of a tread quad in the order of the triangle strip `A B C D`.
fn tread_triangles(q: &TreadQuad) -> [DecalVertexGpu; 6] {
    let v = |i: usize| DecalVertexGpu {
        position: q.corners[i].to_array(),
        uv: [0.0; 2],
        color: [0, 0, 0, q.alpha[i / 2]],
    };
    [v(0), v(1), v(2), v(2), v(1), v(3)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn consecutive_decals_with_one_texture_share_a_draw() {
        let mut draws = Vec::new();
        push_draw(&mut draws, 0, 0..6);
        push_draw(&mut draws, 0, 6..18);
        push_draw(&mut draws, 1, 18..24);
        push_draw(&mut draws, 0, 24..30);
        push_draw(&mut draws, 0, 30..36);
        // The blend order stays: texture 0 comes again as its own range after texture 1.
        assert_eq!(draws, [(0, 0..18), (1, 18..24), (0, 24..36)]);
    }

    #[test]
    fn tread_quads_become_two_triangles_in_strip_order() {
        let q = TreadQuad {
            corners: [Vec3::X, -Vec3::X, Vec3::X + Vec3::Z, Vec3::Z - Vec3::X],
            alpha: [0x60, 0x20],
        };
        let t = tread_triangles(&q);
        let corner = |v: &DecalVertexGpu| q.corners.iter().position(|c| c.to_array() == v.position);
        assert_eq!(
            t.iter().map(|v| corner(v).unwrap()).collect::<Vec<_>>(),
            [0, 1, 2, 2, 1, 3]
        );
        assert_eq!(
            t.iter().map(|v| v.color).collect::<Vec<_>>(),
            [
                [0, 0, 0, 0x60],
                [0, 0, 0, 0x60],
                [0, 0, 0, 0x20],
                [0, 0, 0, 0x20],
                [0, 0, 0, 0x60],
                [0, 0, 0, 0x20],
            ]
        );
    }
}
