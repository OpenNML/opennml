//! Ground markers of the selection, the target marker and the object under the mouse
//! (`GameEngine_RenderWorld` step 8: `FUN_0047A490` with render bit `0x8000000`, after the
//! shadows and before the objects, re/renderer). For each marker, `ObjectRenderer_Draw`
//! (`0x00549AC0`) draws a ring, a flat square or a frame ([`nml_world::markers`]) with world matrix =
//! identity, projection with depth offset and state `Shadow` (color = texture * vertex color,
//! alpha = texture, Z test without writing, fog); types without a selection texture get a line
//! rectangle (state `alpharect2`). The geometry is rebuilt every frame; ray test and entries are
//! left untouched.

use std::collections::HashMap;
use std::ops::Range;

use glam::{Affine3A, Vec3};
use nml_formats::prj::SelectionMarker;
use nml_rda::Vfs;
use nml_world::assets::load_texture;
use nml_world::markers::{
    BLOCKING_NODE, MarkerShape, helper_box_tile_rect, helper_node_corners, line_rect, marker_shape,
    push_bounds_frame, push_flat_square, push_ring,
};
use nml_world::math::game_rotation_y;
use nml_world::shadows::{DecalHull, DecalVertex, TileMask, instance_box};
use nml_world::terrain::Terrain;

use crate::scene::ModelEntry;
use crate::shadows::{DECAL_ATTRIBUTES, DecalVertexGpu, upload};
use crate::shared::{ALPHA_BLEND, PipelineDesc, Shared, pipeline, pipeline_with_topology, shader};
use crate::texture::upload_rgba;

/// A marker of the frame (data of the object as `ObjectRenderer_Draw` reads it).
#[derive(Debug, Clone, PartialEq)]
pub struct GroundMarker {
    /// Entry of the renderer ([`crate::SceneRenderer::entries`]): main model and world matrix.
    pub entry: usize,
    /// Position `GameObject+0x08…+0x10` and yaw `+0x158`.
    pub position: Vec3,
    pub yaw: f32,
    /// `ObjectRenderer+0x98` (player color, [`nml_world::markers::marker_color`]) as RGBA.
    pub color: [u8; 4],
    /// `OBJ_SELECTION_*` of the type (project object `ObjectRenderer+0x90`).
    pub selection: SelectionMarker,
    /// State `0x2000000` (on the wall): flat square at object height.
    pub on_wall: bool,
    /// `OBJ_ONWATER` of the type or state `0x10000` (floating): ring raised to water height.
    pub raise_to_water: bool,
}

/// A decal without an object: target point `data/icons/zielpunkt.tga` of the command bar
/// (O-237, `0x004574B0`) and defense ring `defenceradius.tga` (`GameEngine_RenderDefenceRadiusMarker`
/// `0x0042F750`). The original draws both in state `Shadow` with world matrix = identity.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldDecal {
    /// Path of the texture.
    pub texture: String,
    /// Center; for `flat` the height of the square.
    pub position: Vec3,
    /// Half side `r` (square `±r`).
    pub radius: f32,
    /// Color as RGBA (D3DCOLOR `0xFFFFFFFF` or `0x800064C8`).
    pub color: [u8; 4],
    /// Flat square at height `y` (`0x0054BFA0`) instead of the terrain decal (`TerrainDecal_DrawSquare`
    /// `0x0054C1D0` with yaw 0 and flag 1).
    pub flat: bool,
}

enum MarkerDraw {
    Decal {
        texture: usize,
        vertices: Range<u32>,
    },
    Lines {
        vertices: Range<u32>,
    },
}

/// Counts of the markers of a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarkerStats {
    /// Drawn rings/squares/frames or line rectangles.
    pub decals: usize,
    pub lines: usize,
}

/// Textures of the terrain decals (CLAMP, linear, without mip levels like the decal) per path
/// (lowercase); those that cannot be loaded are reported once and stay `None`. Shared by the
/// ground markers and the ground marks of layer 4 ([`crate::marks`]).
pub(crate) struct DecalTextures {
    /// Word for the warning ("Selection texture", "Track texture").
    label: &'static str,
    textures: HashMap<String, Option<usize>>,
    bind_groups: Vec<wgpu::BindGroup>,
}

impl DecalTextures {
    pub(crate) fn new(label: &'static str) -> Self {
        Self {
            label,
            textures: HashMap::new(),
            bind_groups: Vec::new(),
        }
    }

    /// Index of the bind group of the texture `path` (loaded on first use).
    pub(crate) fn get(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        path: &str,
    ) -> Option<usize> {
        let key = path.to_ascii_lowercase();
        if let Some(t) = self.textures.get(&key) {
            return *t;
        }
        let t = match load_texture(vfs, path) {
            Ok(img) => {
                let gpu = upload_rgba(device, queue, &img, false, path);
                self.bind_groups
                    .push(shared.textures(device, &gpu.view, &gpu.view));
                Some(self.bind_groups.len() - 1)
            }
            Err(e) => {
                log::warn!("{} {path}: {e:#}", self.label);
                None
            }
        };
        self.textures.insert(key, t);
        t
    }

    pub(crate) fn bind_group(&self, index: usize) -> &wgpu::BindGroup {
        &self.bind_groups[index]
    }
}

/// Pipelines, textures and geometry of the ground markers.
pub struct MarkerGpu {
    decal_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    textures: DecalTextures,
    decal_buffer: Option<wgpu::Buffer>,
    line_buffer: Option<wgpu::Buffer>,
    draws: Vec<MarkerDraw>,
    /// All tiles of the map (`TerrainDecal_Draw` checks only the map boundaries).
    all: Option<((usize, usize), TileMask)>,
    stats: MarkerStats,
    /// Decals without an object ([`MarkerGpu::prepare_world`]): buffer and, per decal, texture and corners.
    world_buffer: Option<wgpu::Buffer>,
    world_draws: Vec<(usize, Range<u32>)>,
}

impl MarkerGpu {
    pub fn new(device: &wgpu::Device, shared: &Shared, color_format: wgpu::TextureFormat) -> Self {
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DecalVertexGpu>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DECAL_ATTRIBUTES,
        };
        // State `Shadow` like the shadow decal (`shaders/shadows.wgsl`).
        let decal_module = shader(device, "Marker", include_str!("shaders/shadows.wgsl"));
        let decal_pipeline = pipeline(
            device,
            &PipelineDesc {
                label: "Marker (Shadow)",
                layouts: &[&shared.frame_layout, &shared.textures_layout],
                module: &decal_module,
                vertex: "vs_decal",
                fragment: "fs_decal",
                buffers: std::slice::from_ref(&layout),
                blend: Some(ALPHA_BLEND),
                depth_write: false,
                cull: true,
                color_format,
            },
        );
        let line_module = shader(device, "Marker lines", include_str!("shaders/markers.wgsl"));
        let line_pipeline = pipeline_with_topology(
            device,
            &PipelineDesc {
                label: "Marker (alpharect2)",
                layouts: &[&shared.frame_layout],
                module: &line_module,
                vertex: "vs_line",
                fragment: "fs_line",
                buffers: std::slice::from_ref(&layout),
                blend: Some(ALPHA_BLEND),
                depth_write: false,
                cull: false,
                color_format,
            },
            wgpu::PrimitiveTopology::LineList,
        );
        Self {
            decal_pipeline,
            line_pipeline,
            textures: DecalTextures::new("Selection texture"),
            decal_buffer: None,
            line_buffer: None,
            draws: Vec::new(),
            all: None,
            stats: MarkerStats::default(),
            world_buffer: None,
            world_draws: Vec::new(),
        }
    }

    /// Builds the markers of a frame in the order of `markers` (as `FUN_0047A490` draws them).
    /// Markers without an entry (no main model) are dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        terrain: &Terrain,
        entries: &[ModelEntry],
        markers: &[GroundMarker],
    ) {
        self.draws.clear();
        self.stats = MarkerStats::default();
        if markers.is_empty() {
            return;
        }
        let (dim_x, dim_z) = (terrain.dim_x(), terrain.dim_z());
        let all = match self.all.take() {
            Some((dims, mask)) if dims == (dim_x, dim_z) => mask,
            _ => TileMask::all(dim_x, dim_z),
        };
        let mut decals: Vec<DecalVertexGpu> = Vec::new();
        let mut lines: Vec<DecalVertexGpu> = Vec::new();
        let mut scratch: Vec<DecalVertex> = Vec::new();
        let size = (terrain.params.size_x as i32, terrain.params.size_z as i32);
        for m in markers {
            let Some(entry) = entries.get(m.entry) else {
                continue;
            };
            let shape = marker_shape(
                m.selection.texture.is_some(),
                m.selection.use_bbox,
                m.on_wall,
            );
            if shape == MarkerShape::Lines {
                let center = instance_box(&entry.asset, &entry.playback, None, 0.0).center();
                let half = DecalHull::from_bounds(&entry.asset.rdo.bounds).half;
                let start = lines.len() as u32;
                lines.extend(
                    line_rect(center, half, &entry.world).map(|p| DecalVertexGpu {
                        position: p.to_array(),
                        uv: [0.0; 2],
                        color: m.color,
                    }),
                );
                self.draws.push(MarkerDraw::Lines {
                    vertices: start..lines.len() as u32,
                });
                self.stats.lines += 1;
                continue;
            }
            let Some(path) = m.selection.texture.as_deref() else {
                continue;
            };
            let Some(texture) = self.textures.get(device, queue, shared, vfs, path) else {
                continue;
            };
            scratch.clear();
            match shape {
                MarkerShape::Ring => {
                    push_ring(
                        terrain,
                        &all,
                        m.selection.radius,
                        m.position,
                        m.yaw,
                        m.color,
                        m.raise_to_water,
                        &mut scratch,
                    );
                }
                MarkerShape::FlatSquare => {
                    push_flat_square(m.position, m.selection.radius, m.color, &mut scratch);
                }
                MarkerShape::Frame => {
                    // World matrix for the helper node: only rotation and position (`0x0054C51A…`).
                    let world = Affine3A::from_translation(m.position)
                        * Affine3A::from_mat3(game_rotation_y(m.yaw));
                    let helper = helper_node_corners(&entry.asset, BLOCKING_NODE, &world)
                        .map(|c| helper_box_tile_rect(&c, size.0, size.1));
                    let hull = DecalHull::from_bounds(&entry.asset.rdo.bounds);
                    push_bounds_frame(
                        terrain,
                        &hull,
                        helper,
                        m.position,
                        m.yaw,
                        m.color,
                        &mut scratch,
                    );
                }
                MarkerShape::Lines => unreachable!(),
            }
            if scratch.is_empty() {
                continue;
            }
            let start = decals.len() as u32;
            decals.extend(scratch.iter().map(|&v| DecalVertexGpu::from(v)));
            self.draws.push(MarkerDraw::Decal {
                texture,
                vertices: start..decals.len() as u32,
            });
            self.stats.decals += 1;
        }
        self.all = Some(((dim_x, dim_z), all));
        upload(
            device,
            queue,
            &mut self.decal_buffer,
            "Markers",
            bytemuck::cast_slice(&decals),
        );
        upload(
            device,
            queue,
            &mut self.line_buffer,
            "Marker lines",
            bytemuck::cast_slice(&lines),
        );
    }

    /// Builds the decals without an object of a frame ([`WorldDecal`]); empty = none. Terrain decals over all tiles of
    /// the map (`TerrainDecal_Draw` checks only the map boundaries), flat squares at their height. Decals with a
    /// texture that cannot be loaded are dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_world(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        terrain: &Terrain,
        decals: &[WorldDecal],
    ) {
        self.world_draws.clear();
        if decals.is_empty() {
            return;
        }
        let all = TileMask::all(terrain.dim_x(), terrain.dim_z());
        let mut vertices: Vec<DecalVertexGpu> = Vec::new();
        let mut scratch: Vec<DecalVertex> = Vec::new();
        for d in decals {
            let Some(texture) = self.textures.get(device, queue, shared, vfs, &d.texture) else {
                continue;
            };
            scratch.clear();
            if d.flat {
                push_flat_square(d.position, d.radius, d.color, &mut scratch);
            } else {
                push_ring(
                    terrain,
                    &all,
                    d.radius,
                    d.position,
                    0.0,
                    d.color,
                    false,
                    &mut scratch,
                );
            }
            if scratch.is_empty() {
                continue;
            }
            let start = vertices.len() as u32;
            vertices.extend(scratch.iter().map(|&v| DecalVertexGpu::from(v)));
            self.world_draws
                .push((texture, start..vertices.len() as u32));
        }
        upload(
            device,
            queue,
            &mut self.world_buffer,
            "Decals without object",
            bytemuck::cast_slice(&vertices),
        );
    }

    /// Number of decals without an object of the last [`MarkerGpu::prepare_world`].
    pub fn world_decals(&self) -> usize {
        self.world_draws.len()
    }

    /// Draws the prepared markers (after the shadows, before the objects), then the decals without an object.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared) {
        if self.draws.is_empty() && self.world_draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        for draw in &self.draws {
            match draw {
                MarkerDraw::Decal { texture, vertices } => {
                    let Some(buffer) = &self.decal_buffer else {
                        continue;
                    };
                    pass.set_pipeline(&self.decal_pipeline);
                    pass.set_bind_group(1, self.textures.bind_group(*texture), &[]);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(vertices.clone(), 0..1);
                }
                MarkerDraw::Lines { vertices } => {
                    let Some(buffer) = &self.line_buffer else {
                        continue;
                    };
                    pass.set_pipeline(&self.line_pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(vertices.clone(), 0..1);
                }
            }
        }
        if let Some(buffer) = self
            .world_buffer
            .as_ref()
            .filter(|_| !self.world_draws.is_empty())
        {
            pass.set_pipeline(&self.decal_pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            for (texture, vertices) in &self.world_draws {
                pass.set_bind_group(1, self.textures.bind_group(*texture), &[]);
                pass.draw(vertices.clone(), 0..1);
            }
        }
    }

    /// Counts of the last [`MarkerGpu::prepare`].
    pub fn stats(&self) -> MarkerStats {
        self.stats
    }
}
