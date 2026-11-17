//! Shadows on the terrain like `ObjectRenderer_DrawShadowOnGround` (`0x005485A0`) and the
//! shadow textures like `ShadowTexture_RenderOccluder` (`0x0054E7B0`), see
//! re/shadows. Instead of individual render targets, the shadow textures live in an atlas
//! (`R8Unorm`, cells of 128 x 128 or quarter cells of 64 x 64). The geometry is computed by `nml_world::shadows`.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Affine3A, Mat4, Vec3};
use nml_rda::Vfs;
use nml_world::assets::{ModelCache, load_texture};
use nml_world::classes::{DecalKind, ObjectShadow, ShadowKind};
use nml_world::shadows::{
    Aabb, DecalColor, DecalHull, DecalVertex, ReceiverVertex, SHADOW_DECAL_COLOR, ShadowFrame,
    TileMask, ground_shadow_in_view, instance_box, mesh_box, object_in_view, push_decal,
    push_receiver, receives_shadow, shadow_alpha, special_decal_color,
};
use nml_world::terrain::{FogFactors, Terrain};
use nml_world::visibility::Frustum;

use crate::objects::{
    Instance, OBJECT_ATTRIBUTES, ObjectRenderer, ObjectVertex, SKINNED_ATTRIBUTES, SkinnedVertex,
};
use crate::scene::ModelEntry;
use crate::shared::{ALPHA_BLEND, PipelineDesc, Shared, pipeline, shader};
use crate::texture::upload_rgba;

/// Edge length of an atlas cell (largest shadow texture).
pub const CELL: u32 = 128;
/// Largest edge length of the atlas (4,096 cells of 128 x 128).
pub const MAX_ATLAS_SIZE: u32 = 8192;
/// Format of the atlas: only the alpha of the silhouette counts (RGB is always 0 in the original).
pub const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// A cell in the atlas (texels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Life {
    /// Kind 2: stays occupied as long as the object is visible.
    Persistent,
    /// Kind 3: valid for one frame.
    Frame,
}

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    full: Option<Life>,
    quarters: [Option<Life>; 4],
}

impl Slot {
    fn is_empty(&self) -> bool {
        self.full.is_none() && self.quarters.iter().all(Option::is_none)
    }
}

/// Occupancy of the atlas: 128-texel cells; 64-texel textures share a cell, four to a cell
/// (`GameEngine_AcquireFrameShadowTexture` `0x00432FA0` and `…PersistentShadowTexture`
/// `0x00433190` allocate a separate render target per texture in the original).
#[derive(Debug, Clone)]
pub struct AtlasAlloc {
    columns: u32,
    slots: Vec<Slot>,
}

impl AtlasAlloc {
    /// Atlas with `columns × columns` cells.
    pub fn new(columns: u32) -> Self {
        Self {
            columns,
            slots: vec![Slot::default(); (columns * columns) as usize],
        }
    }

    /// Occupies a cell for a texture of edge `size` (64 or 128); `None` if the atlas is
    /// full. 64-texel textures fill partially used cells first.
    pub fn alloc(&mut self, size: u32, persistent: bool) -> Option<Cell> {
        let life = if persistent {
            Life::Persistent
        } else {
            Life::Frame
        };
        let cell = |index: usize, quarter: Option<usize>| {
            let (x, y) = (index as u32 % self.columns, index as u32 / self.columns);
            let (qx, qy) = quarter.map_or((0, 0), |q| (q as u32 % 2, q as u32 / 2));
            Cell {
                x: x * CELL + qx * CELL / 2,
                y: y * CELL + qy * CELL / 2,
                size: if quarter.is_some() { CELL / 2 } else { CELL },
            }
        };
        if size > CELL / 2 {
            let index = self.slots.iter().position(Slot::is_empty)?;
            self.slots[index].full = Some(life);
            return Some(cell(index, None));
        }
        let partial = self.slots.iter().position(|s| {
            s.full.is_none() && !s.is_empty() && s.quarters.iter().any(Option::is_none)
        });
        let index = partial.or_else(|| self.slots.iter().position(Slot::is_empty))?;
        let quarter = self.slots[index]
            .quarters
            .iter()
            .position(Option::is_none)?;
        self.slots[index].quarters[quarter] = Some(life);
        Some(cell(index, Some(quarter)))
    }

    /// Frees a cell.
    pub fn free(&mut self, cell: Cell) {
        let index = ((cell.y / CELL) * self.columns + cell.x / CELL) as usize;
        let slot = &mut self.slots[index];
        if cell.size >= CELL {
            slot.full = None;
        } else {
            let q = ((cell.y % CELL) / (CELL / 2) * 2 + (cell.x % CELL) / (CELL / 2)) as usize;
            slot.quarters[q] = None;
        }
    }

    /// Frees all cells of the frame textures (kind 3) (counter per frame set to 0, `0x0042F010`).
    pub fn end_frame(&mut self) {
        for slot in &mut self.slots {
            if slot.full == Some(Life::Frame) {
                slot.full = None;
            }
            for quarter in &mut slot.quarters {
                if *quarter == Some(Life::Frame) {
                    *quarter = None;
                }
            }
        }
    }

    /// Occupied cells in units of 128 x 128 (quarters count 1/4).
    pub fn used(&self) -> f32 {
        self.slots
            .iter()
            .map(|s| {
                if s.full.is_some() {
                    1.0
                } else {
                    s.quarters.iter().filter(|q| q.is_some()).count() as f32 / 4.0
                }
            })
            .sum()
    }

    /// Number of cells.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
}

/// Vertex of the decals and the ground surface (position, UV, color; FVF `0x1C2` without specular).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DecalVertexGpu {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

impl From<DecalVertex> for DecalVertexGpu {
    fn from(v: DecalVertex) -> Self {
        Self {
            position: v.position,
            uv: v.uv,
            color: v.color,
        }
    }
}

/// Vertex of the texture shadow receivers: position, `q`, `(m00, m11, alpha)`, atlas cell.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ReceiverVertexGpu {
    pub position: [f32; 3],
    pub light: [f32; 3],
    pub scale: [f32; 3],
    pub cell: [f32; 4],
}

pub(crate) const DECAL_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4];
const RECEIVER_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x4
];

/// Shadow counters of a frame (state snapshot, overlay).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShadowStats {
    /// Objects per shadow kind 0-3 (all entries, regardless of visibility).
    pub kinds: [usize; 4],
    /// Drawn shadow decals (kind 1).
    pub decals: usize,
    /// Drawn ground planes.
    pub groundplanes: usize,
    /// Drawn decals of the special class (gold deposit; objects and fog memories).
    pub specials: usize,
    /// Drawn texture shadows of kind 2 and kind 3.
    pub persistent: usize,
    pub per_frame: usize,
    /// Shadow textures rendered in this frame (occluder passes).
    pub rendered: usize,
    /// Occupied atlas cells (128 x 128; quarters count 1/4) and number of cells.
    pub atlas_used: f32,
    pub atlas_capacity: usize,
    /// Texture shadows without a free atlas cell (not drawn).
    pub atlas_full: usize,
}

/// Shadow of a scene entry (same order as the model entries).
#[derive(Debug, Clone, PartialEq)]
pub struct EntryShadow {
    pub setup: ObjectShadow,
    /// Position of the object (`GameObject+0x08`) and yaw (`+0x158`, `OBJ_YROT`).
    pub position: Vec3,
    pub yaw: f32,
    /// Moving object (modules 0, 0x21, 0x33): after the static ones in the receiver pass.
    pub moving: bool,
}

/// Hull and texture of a decal model.
struct DecalAsset {
    hull: DecalHull,
    bind_group: wgpu::BindGroup,
}

/// State of a shadow kept across frames.
struct ShadowEntry {
    shadow: EntryShadow,
    decals: Vec<(DecalKind, Arc<DecalAsset>)>,
    world: Affine3A,
    /// Rest hull with sub-objects (`ComputeBoundingBox(…, 0)`).
    rest: Aabb,
    /// Kind 2: occupied cell; re-render when `dirty`.
    cell: Option<Cell>,
    dirty: bool,
    /// Light view of the last rendered texture.
    frame: Option<ShadowFrame>,
    /// Hull that grows while an animation runs (`Instance+0x110`), valid for (model, animation,
    /// start). In the original it belongs to the model instance and therefore survives [`ShadowGpu::set`].
    grown: Option<(GrownKey, Aabb)>,
}

/// Model (address of the asset), animation and its start (bits of the start time).
type GrownKey = (usize, Option<usize>, u32);

/// Decal of the special class of an active fog memory (gold deposit, fertile soil):
/// model, remembered position and yaw, brightness factor `k` of the image.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryDecal {
    /// Normalized path of the `.rdo` (hull and material-0 texture).
    pub model: String,
    pub position: Vec3,
    pub yaw: f32,
    pub brightness: f32,
}

enum ShadowDraw {
    Decal {
        asset: Arc<DecalAsset>,
        vertices: Range<u32>,
    },
    Receiver {
        vertices: Range<u32>,
    },
}

struct Occluder {
    entry: usize,
    cell: Cell,
    /// Skip transparent groups (animation running, `ModelInstance_Draw` `0x0058B6FC`).
    skip_transparent: bool,
}

/// Size of an entry in the buffer of the occluder matrices (alignment of dynamic offsets).
const CASTER_STRIDE: u64 = 256;

/// Pipelines, atlas and geometry of the shadows.
pub struct ShadowGpu {
    decal_pipeline: wgpu::RenderPipeline,
    receiver_pipeline: wgpu::RenderPipeline,
    ground_pipeline: wgpu::RenderPipeline,
    /// (skin, transparent): static/opaque, static/transparent, skin/...
    occluder_pipelines: [wgpu::RenderPipeline; 4],
    clear_pipeline: wgpu::RenderPipeline,
    caster_layout: wgpu::BindGroupLayout,
    caster_buffer: wgpu::Buffer,
    caster_bind_group: wgpu::BindGroup,
    caster_capacity: u64,
    atlas_view: wgpu::TextureView,
    atlas_bind_group: wgpu::BindGroup,
    atlas_size: u32,
    alloc: AtlasAlloc,
    decal_buffer: Option<wgpu::Buffer>,
    receiver_buffer: Option<wgpu::Buffer>,
    ground: Option<(wgpu::Buffer, u32)>,
    draws: Vec<ShadowDraw>,
    specials: Vec<ShadowDraw>,
    /// Special-class decals of the active fog memories ([`ShadowGpu::set_memory_decals`]).
    memory_decals: Vec<(Arc<DecalAsset>, MemoryDecal)>,
    occluders: Vec<Occluder>,
    assets: HashMap<(String, bool), Option<Arc<DecalAsset>>>,
    entries: Vec<ShadowEntry>,
    /// Draw order: static before moving, each in 8 x 8-tile sectors (z, then x).
    order: Vec<usize>,
    light: Vec3,
    intensity: i32,
    last_view_proj: Option<Mat4>,
    stats: ShadowStats,
}

fn atlas(device: &wgpu::Device, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Shadow atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// `SRCALPHA`/`ONE`... for the occluder: `ONE`/`ONE` (alpha adds up, saturated).
const ADD: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Pipeline into the atlas: without depth (`SetRenderTarget(texture, NULL)`).
#[allow(clippy::too_many_arguments)]
fn atlas_pipeline(
    device: &wgpu::Device,
    label: &str,
    layouts: &[&wgpu::BindGroupLayout],
    module: &wgpu::ShaderModule,
    (vertex, fragment): (&str, &str),
    buffers: &[wgpu::VertexBufferLayout],
    blend: Option<wgpu::BlendState>,
    cull: bool,
) -> wgpu::RenderPipeline {
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    let buffers: Vec<Option<wgpu::VertexBufferLayout>> =
        buffers.iter().cloned().map(Some).collect();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: cull.then_some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: ATLAS_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Writes `bytes` into `buffer` and grows it if needed.
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut Option<wgpu::Buffer>,
    label: &str,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    if buffer
        .as_ref()
        .is_none_or(|b| b.size() < bytes.len() as u64)
    {
        *buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (bytes.len() as u64).next_power_of_two(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    if let Some(buffer) = buffer {
        queue.write_buffer(buffer, 0, bytes);
    }
}

impl ShadowGpu {
    pub fn new(device: &wgpu::Device, shared: &Shared, color_format: wgpu::TextureFormat) -> Self {
        let module = shader(device, "Shadows", include_str!("shaders/shadows.wgsl"));
        let decal_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DecalVertexGpu>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DECAL_ATTRIBUTES,
        };
        let receiver_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ReceiverVertexGpu>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &RECEIVER_ATTRIBUTES,
        };
        let layouts = [&shared.frame_layout, &shared.textures_layout];
        let make =
            |label, (vertex, fragment), layout: &wgpu::VertexBufferLayout, blend, depth_write| {
                pipeline(
                    device,
                    &PipelineDesc {
                        label,
                        layouts: &layouts,
                        module: &module,
                        vertex,
                        fragment,
                        buffers: std::slice::from_ref(layout),
                        blend,
                        depth_write,
                        cull: true,
                        color_format,
                    },
                )
            };
        let decal_pipeline = make(
            "Shadow",
            ("vs_decal", "fs_decal"),
            &decal_layout,
            Some(ALPHA_BLEND),
            false,
        );
        let receiver_pipeline = make(
            "ShadowReceiver",
            ("vs_receiver", "fs_receiver"),
            &receiver_layout,
            Some(ALPHA_BLEND),
            false,
        );
        let ground_pipeline = make(
            "Ground surface",
            ("vs_ground", "fs_ground"),
            &decal_layout,
            None,
            true,
        );
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Occluder"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let source = format!(
            "{}\n{}",
            include_str!("shaders/object.wgsl"),
            include_str!("shaders/occluder.wgsl")
        );
        let occluder_module = shader(device, "Occluder", &source);
        let static_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ObjectVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &OBJECT_ATTRIBUTES,
        };
        let skinned_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkinnedVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &SKINNED_ATTRIBUTES,
        };
        let occluder_layouts = [
            &shared.frame_layout,
            &shared.textures_layout,
            &shared.draw_layout,
            &caster_layout,
        ];
        let occluder = |label, vertex, layout: &wgpu::VertexBufferLayout, blend| {
            atlas_pipeline(
                device,
                label,
                &occluder_layouts,
                &occluder_module,
                (vertex, "fs_occluder"),
                std::slice::from_ref(layout),
                blend,
                true,
            )
        };
        let occluder_pipelines = [
            occluder("ShadowOccluder", "vs_occluder", &static_layout, None),
            occluder(
                "ShadowOccluder transparent",
                "vs_occluder",
                &static_layout,
                Some(ADD),
            ),
            occluder(
                "ShadowOccluder Skin",
                "vs_occluder_skinned",
                &skinned_layout,
                None,
            ),
            occluder(
                "ShadowOccluder Skin transparent",
                "vs_occluder_skinned",
                &skinned_layout,
                Some(ADD),
            ),
        ];
        let clear_pipeline = atlas_pipeline(
            device,
            "Clear shadow cell",
            &[&shared.frame_layout],
            &occluder_module,
            ("vs_clear", "fs_clear"),
            &[],
            None,
            false,
        );
        let caster_buffer = Self::caster_buffer(device, 1);
        let caster_bind_group = Self::caster_bind_group(device, &caster_layout, &caster_buffer);
        let atlas_size = MAX_ATLAS_SIZE.min(device.limits().max_texture_dimension_2d) / CELL * CELL;
        let texture = atlas(device, atlas_size);
        let atlas_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_bind_group = shared.textures(device, &atlas_view, &atlas_view);
        Self {
            decal_pipeline,
            receiver_pipeline,
            ground_pipeline,
            occluder_pipelines,
            clear_pipeline,
            caster_layout,
            caster_buffer,
            caster_bind_group,
            caster_capacity: 1,
            atlas_view,
            atlas_bind_group,
            atlas_size,
            alloc: AtlasAlloc::new(atlas_size / CELL),
            decal_buffer: None,
            receiver_buffer: None,
            ground: None,
            draws: Vec::new(),
            specials: Vec::new(),
            memory_decals: Vec::new(),
            occluders: Vec::new(),
            assets: HashMap::new(),
            entries: Vec::new(),
            order: Vec::new(),
            light: Vec3::Y,
            intensity: 0,
            last_view_proj: None,
            stats: ShadowStats::default(),
        }
    }

    fn caster_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Occluder matrices"),
            size: capacity * CASTER_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn caster_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Occluder"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        })
    }

    /// Hull and texture (material 0, without mip levels) of a decal model; ground planes use
    /// the mesh hulls (`ModelInstance_GetBoundsCenter(…, 0)`), shadow decals the header.
    #[allow(clippy::too_many_arguments)]
    fn asset(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        groundplane: bool,
    ) -> Option<Arc<DecalAsset>> {
        let key = (path.to_string(), groundplane);
        if let Some(asset) = self.assets.get(&key) {
            return asset.clone();
        }
        let asset = models.get(vfs, path).and_then(|model| {
            let hull = if groundplane {
                let b = mesh_box(&model, None)?;
                DecalHull {
                    center: b.center(),
                    half: b.half_extent(),
                }
            } else {
                DecalHull::from_bounds(&model.rdo.bounds)
            };
            let texture = model.material_textures.first().cloned().flatten()?;
            match load_texture(vfs, &texture) {
                Ok(img) => {
                    let gpu = upload_rgba(device, queue, &img, false, &texture);
                    Some(Arc::new(DecalAsset {
                        hull,
                        bind_group: shared.textures(device, &gpu.view, &gpu.view),
                    }))
                }
                Err(e) => {
                    log::warn!("Shadow texture {texture}: {e:#}");
                    None
                }
            }
        });
        self.assets.insert(key, asset.clone());
        asset
    }

    /// Sets the shadows of all entries (same order as `entries`) and the
    /// shadow light (direction to the light, `SHADOW_INTENSITY`).
    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        models: &mut ModelCache,
        entries: &[ModelEntry],
        shadows: Vec<EntryShadow>,
        light: (Vec3, i32),
        tile_size: (f32, f32),
    ) {
        (self.light, self.intensity) = light;
        self.alloc = AtlasAlloc::new(self.atlas_size / CELL);
        self.last_view_proj = None;
        // Queued draws refer to entries of the old list.
        self.draws.clear();
        self.specials.clear();
        self.occluders.clear();
        let grown: Vec<_> = std::mem::take(&mut self.entries)
            .into_iter()
            .map(|e| e.grown)
            .collect();
        let mut list = Vec::with_capacity(shadows.len());
        for (index, (entry, shadow)) in entries.iter().zip(shadows).enumerate() {
            let decals = shadow
                .setup
                .decals
                .iter()
                .filter_map(|d| {
                    let groundplane = d.kind == DecalKind::Groundplane;
                    let asset =
                        self.asset(device, queue, shared, vfs, models, &d.model, groundplane)?;
                    Some((d.kind, asset))
                })
                .collect();
            list.push(ShadowEntry {
                rest: instance_box(&entry.asset, &entry.playback, None, 0.0),
                world: entry.world,
                decals,
                shadow,
                cell: None,
                dirty: true,
                frame: None,
                grown: grown.get(index).copied().flatten(),
            });
        }
        let sector = |p: Vec3| {
            (
                ((p.z / tile_size.1).floor() as i64) >> 3,
                ((p.x / tile_size.0).floor() as i64) >> 3,
            )
        };
        let mut order: Vec<usize> = (0..list.len()).collect();
        order.sort_by_key(|&i| {
            let s = &list[i].shadow;
            (s.moving, sector(s.position), i)
        });
        self.entries = list;
        self.order = order;
        let mut kinds = [0; 4];
        for e in &self.entries {
            kinds[e.shadow.setup.kind.number() as usize] += 1;
        }
        self.stats = ShadowStats {
            kinds,
            atlas_capacity: self.alloc.capacity(),
            ..Default::default()
        };
    }

    /// Special-class decals of the active fog memories (empty = none). The original draws them
    /// via the object renderer of the image (`0x004158A0` -> `0x00548B30` -> `0x005485A0`) in the
    /// object pass with `k' = trunc(k · 255)`; decals without a loadable model are dropped.
    pub fn set_memory_decals(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        models: &mut ModelCache,
        decals: Vec<MemoryDecal>,
    ) {
        self.memory_decals.clear();
        for decal in decals {
            if let Some(asset) = self.asset(device, queue, shared, vfs, models, &decal.model, false)
            {
                self.memory_decals.push((asset, decal));
            }
        }
    }

    /// Ground surface (model browser): the tiles of `terrain` in their terrain color; `None`
    /// removes it.
    pub fn set_ground(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: Option<&Terrain>,
    ) {
        self.ground = terrain.map(|terrain| {
            let mut vertices = Vec::new();
            for z in 0..terrain.dim_z() {
                for x in 0..terrain.dim_x() {
                    for (gx, gz) in nml_world::shadows::tile_corners(x, z) {
                        let v = terrain.vertex(gx, gz);
                        let [r, g, b] = v.lit_color;
                        vertices.push(DecalVertexGpu {
                            position: v.position.to_array(),
                            uv: [0.0; 2],
                            color: [r, g, b, 255],
                        });
                    }
                }
            }
            let mut buffer = None;
            upload(
                device,
                queue,
                &mut buffer,
                "Ground surface",
                bytemuck::cast_slice(&vertices),
            );
            (
                buffer.expect("ground surface has tiles"),
                vertices.len() as u32,
            )
        });
    }

    /// Prepares the shadows of a frame: visible objects (`0x00468C60`), decals,
    /// ground planes, cells and light view of the shadow textures. The state of the poses in
    /// `instances` is drawn (`SceneRenderer::update`). Fog of war: the alpha of the
    /// shadow textures carries the brightness factor of the entry (`EntryLook::brightness`), the
    /// ground planes the terrain color with `fog` (`None` = factor 1).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: &Terrain,
        visible: &TileMask,
        view_proj: Mat4,
        now_ms: f32,
        entries: &[ModelEntry],
        instances: &[Instance],
        entry_instances: &[Range<usize>],
        fog: Option<&FogFactors>,
    ) {
        self.alloc.end_frame();
        self.draws.clear();
        self.specials.clear();
        self.occluders.clear();
        // Shadows belong to other entries (browser before `set`) or there has been no
        // `update` yet: draw nothing.
        if self.entries.len() != entries.len() || entry_instances.len() != entries.len() {
            self.stats = ShadowStats {
                atlas_capacity: self.alloc.capacity(),
                ..Default::default()
            };
            return;
        }
        // Camera change: deactivate all kind-2 textures and re-render them (`0x0047FCE2`).
        if self.last_view_proj != Some(view_proj) {
            for e in &mut self.entries {
                e.dirty = true;
            }
        }
        self.last_view_proj = Some(view_proj);
        let frustum = Frustum::from_view_proj(view_proj);
        let mut decal_vertices: Vec<DecalVertexGpu> = Vec::new();
        let mut receiver_vertices: Vec<ReceiverVertexGpu> = Vec::new();
        let mut casters: Vec<[[f32; 4]; 4]> = Vec::new();
        let mut stats = ShadowStats {
            kinds: self.stats.kinds,
            atlas_capacity: self.alloc.capacity(),
            ..Default::default()
        };
        let tile_size = (terrain.params.size_x, terrain.params.size_z);
        let mut scratch: Vec<DecalVertex> = Vec::new();
        let mut receivers: Vec<ReceiverVertex> = Vec::new();
        for &i in &self.order {
            let e = &mut self.entries[i];
            let kind = e.shadow.setup.kind;
            if e.decals.is_empty() && !kind.is_texture() {
                continue;
            }
            // Render bit 2 is missing (construction site, dead building, occupant), the status word blocks
            // the receiver pass (dead for kind 1/2, `ObjectRenderer_Draw` `0x00549AE4`) or nothing is
            // drawn: no shadow (`GameObject_RenderPass` with pass 2).
            if entries[i].look.bits & crate::scene::RENDER_SHADOW == 0
                || !receives_shadow(kind, entries[i].status)
                || entry_instances[i].is_empty()
            {
                if let Some(cell) = e.cell.take() {
                    self.alloc.free(cell);
                }
                continue;
            }
            let in_view = object_in_view(&frustum, &e.rest, e.shadow.position)
                || (kind.is_texture()
                    && ground_shadow_in_view(&frustum, &e.rest, &e.world, self.light));
            if !in_view {
                if let Some(cell) = e.cell.take() {
                    self.alloc.free(cell);
                }
                continue;
            }
            // `k` = brightness factor of the object (re/shadows, `ObjectRenderer+0x4C`): alpha of the
            // shadow texture and color of the special-class decal.
            let alpha = shadow_alpha(self.intensity, entries[i].look.brightness);
            for (decal, asset) in &e.decals {
                let color = match decal {
                    DecalKind::Shadow => DecalColor::Fixed(SHADOW_DECAL_COLOR),
                    DecalKind::Groundplane => DecalColor::Terrain,
                    DecalKind::Special => {
                        DecalColor::Fixed(special_decal_color(entries[i].look.brightness))
                    }
                };
                scratch.clear();
                let tiles = push_decal(
                    terrain,
                    visible,
                    &asset.hull,
                    e.shadow.position,
                    e.shadow.yaw,
                    color,
                    fog,
                    &mut scratch,
                );
                if tiles == 0 {
                    continue;
                }
                let start = decal_vertices.len() as u32;
                decal_vertices.extend(scratch.iter().map(|&v| DecalVertexGpu::from(v)));
                let draw = ShadowDraw::Decal {
                    asset: asset.clone(),
                    vertices: start..decal_vertices.len() as u32,
                };
                match decal {
                    DecalKind::Shadow => stats.decals += 1,
                    DecalKind::Groundplane => stats.groundplanes += 1,
                    DecalKind::Special => stats.specials += 1,
                }
                if *decal == DecalKind::Special {
                    self.specials.push(draw);
                } else {
                    self.draws.push(draw);
                }
            }
            if !kind.is_texture() {
                continue;
            }
            let size = e.shadow.setup.texture_size;
            let cell = match kind {
                ShadowKind::Persistent => {
                    if e.cell.is_none() {
                        e.cell = self.alloc.alloc(size, true);
                        e.dirty = true;
                    }
                    e.cell
                }
                _ => self.alloc.alloc(size, false),
            };
            let Some(cell) = cell else {
                stats.atlas_full += 1;
                continue;
            };
            if kind == ShadowKind::PerFrame || e.dirty || e.frame.is_none() {
                let entry = &entries[i];
                let range = entry_instances[i].clone();
                let player = entry.playback.player();
                let pose = instances
                    .get(range.start)
                    .and_then(|instance| instance.pose.as_ref())
                    .filter(|_| player.current.is_some());
                let mut aabb = instance_box(&entry.asset, &entry.playback, pose, now_ms);
                if pose.is_some() {
                    let asset = Arc::as_ptr(&entry.asset) as *const () as usize;
                    let key = (asset, player.current, player.started_ms.to_bits());
                    if let Some((grown_key, grown)) = &e.grown
                        && *grown_key == key
                    {
                        aabb = grown.union(&aabb);
                    }
                    e.grown = Some((key, aabb));
                }
                let frame =
                    ShadowFrame::new(&aabb.corners(), &e.world, self.light, size, tile_size);
                casters.push((frame.projection * frame.view).to_cols_array_2d());
                self.occluders.push(Occluder {
                    entry: i,
                    cell,
                    skip_transparent: pose.is_some(),
                });
                // `dirty` stays set until `render_occluders` has drawn the texture.
                e.frame = Some(frame);
                stats.rendered += 1;
            }
            let Some(frame) = e.frame else { continue };
            receivers.clear();
            if push_receiver(
                terrain,
                visible,
                &frame,
                e.shadow.setup.on_water,
                &mut receivers,
            ) == 0
            {
                continue;
            }
            let atlas = self.atlas_size as f32;
            let scale = [frame.projection.x_axis.x, frame.projection.y_axis.y, alpha];
            let cell_uv = [
                cell.x as f32 / atlas,
                cell.y as f32 / atlas,
                cell.size as f32 / atlas,
                cell.size as f32,
            ];
            let start = receiver_vertices.len() as u32;
            receiver_vertices.extend(receivers.iter().map(|v| ReceiverVertexGpu {
                position: v.position,
                light: v.light,
                scale,
                cell: cell_uv,
            }));
            self.draws.push(ShadowDraw::Receiver {
                vertices: start..receiver_vertices.len() as u32,
            });
            match kind {
                ShadowKind::Persistent => stats.persistent += 1,
                _ => stats.per_frame += 1,
            }
        }
        // Special-class decals of the fog memories after those of the objects (the image list draws
        // after the objects); without a frustum test, the tile mask limits them.
        for (asset, d) in &self.memory_decals {
            scratch.clear();
            let color = DecalColor::Fixed(special_decal_color(d.brightness));
            let tiles = push_decal(
                terrain,
                visible,
                &asset.hull,
                d.position,
                d.yaw,
                color,
                fog,
                &mut scratch,
            );
            if tiles == 0 {
                continue;
            }
            let start = decal_vertices.len() as u32;
            decal_vertices.extend(scratch.iter().map(|&v| DecalVertexGpu::from(v)));
            self.specials.push(ShadowDraw::Decal {
                asset: asset.clone(),
                vertices: start..decal_vertices.len() as u32,
            });
            stats.specials += 1;
        }
        upload(
            device,
            queue,
            &mut self.decal_buffer,
            "Decals",
            bytemuck::cast_slice(&decal_vertices),
        );
        upload(
            device,
            queue,
            &mut self.receiver_buffer,
            "Shadow receivers",
            bytemuck::cast_slice(&receiver_vertices),
        );
        if !casters.is_empty() {
            let needed = casters.len() as u64;
            if needed > self.caster_capacity {
                self.caster_capacity = needed.next_power_of_two();
                self.caster_buffer = Self::caster_buffer(device, self.caster_capacity);
                self.caster_bind_group =
                    Self::caster_bind_group(device, &self.caster_layout, &self.caster_buffer);
            }
            let mut bytes = vec![0u8; casters.len() * CASTER_STRIDE as usize];
            for (k, m) in casters.iter().enumerate() {
                let at = k * CASTER_STRIDE as usize;
                bytes[at..at + 64].copy_from_slice(bytemuck::bytes_of(m));
            }
            queue.write_buffer(&self.caster_buffer, 0, &bytes);
        }
        stats.atlas_used = self.alloc.used();
        self.stats = stats;
    }

    /// Renders the shadow textures queued in [`ShadowGpu::prepare`] into the atlas (before the
    /// frame, separate passes like `GameEngine_RenderShadowTextures` `0x0042F010`). The
    /// drawn kind-2 textures are considered current afterwards.
    pub fn render_occluders(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        shared: &Shared,
        objects: &ObjectRenderer,
        entry_instances: &[Range<usize>],
    ) {
        if self.occluders.is_empty() {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Shadow textures"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.atlas_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        for (k, occluder) in self.occluders.iter().enumerate() {
            let Cell { x, y, size } = occluder.cell;
            pass.set_viewport(x as f32, y as f32, size as f32, size as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, size, size);
            pass.set_pipeline(&self.clear_pipeline);
            pass.draw(0..3, 0..1);
            pass.set_bind_group(
                3,
                &self.caster_bind_group,
                &[(k as u64 * CASTER_STRIDE) as u32],
            );
            let pipelines = &self.occluder_pipelines;
            objects.draw_instances(
                &mut pass,
                entry_instances[occluder.entry].clone(),
                |_, skinned, transparent| {
                    if transparent && occluder.skip_transparent {
                        return None;
                    }
                    Some(&pipelines[usize::from(skinned) * 2 + usize::from(transparent)])
                },
            );
        }
        for occluder in &self.occluders {
            let e = &mut self.entries[occluder.entry];
            if e.shadow.setup.kind == ShadowKind::Persistent {
                e.dirty = false;
            }
        }
    }

    fn draw_list(&self, pass: &mut wgpu::RenderPass<'_>, list: &[ShadowDraw]) {
        let mut current: Option<bool> = None;
        for draw in list {
            match draw {
                ShadowDraw::Decal { asset, vertices } => {
                    let Some(buffer) = &self.decal_buffer else {
                        continue;
                    };
                    if current != Some(true) {
                        pass.set_pipeline(&self.decal_pipeline);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        current = Some(true);
                    }
                    pass.set_bind_group(1, &asset.bind_group, &[]);
                    pass.draw(vertices.clone(), 0..1);
                }
                ShadowDraw::Receiver { vertices } => {
                    let Some(buffer) = &self.receiver_buffer else {
                        continue;
                    };
                    if current != Some(false) {
                        pass.set_pipeline(&self.receiver_pipeline);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                        current = Some(false);
                    }
                    pass.draw(vertices.clone(), 0..1);
                }
            }
        }
    }

    /// Ground surface (browser), then all shadows and ground planes on the terrain
    /// (`ObjectManager_RenderObjects(2)`), then the special-class decals (object pass).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared) {
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        if let Some((buffer, count)) = &self.ground {
            pass.set_pipeline(&self.ground_pipeline);
            pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
        self.draw_list(pass, &self.draws);
        self.draw_list(pass, &self.specials);
    }

    /// Tracks the position of an entry (moving units); a changed position re-renders the
    /// texture shadow.
    pub fn follow(&mut self, index: usize, position: Vec3, yaw: f32, world: Affine3A) {
        if let Some(e) = self.entries.get_mut(index)
            && (e.shadow.position != position || e.shadow.yaw != yaw)
        {
            e.shadow.position = position;
            e.shadow.yaw = yaw;
            e.world = world;
            e.dirty = true;
        }
    }

    pub fn stats(&self) -> ShadowStats {
        self.stats
    }

    /// Width of the atlas in texels.
    pub fn atlas_size(&self) -> u32 {
        self.atlas_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_packs_quarters_and_frees_frame_cells() {
        let mut atlas = AtlasAlloc::new(2);
        assert_eq!(atlas.capacity(), 4);
        let big = atlas.alloc(128, true).unwrap();
        assert_eq!(
            big,
            Cell {
                x: 0,
                y: 0,
                size: 128
            }
        );
        // Four 64-texel textures share cell 1, the fifth starts cell 2.
        let small: Vec<Cell> = (0..5).map(|_| atlas.alloc(64, false).unwrap()).collect();
        assert_eq!(
            small[0],
            Cell {
                x: 128,
                y: 0,
                size: 64
            }
        );
        assert_eq!(
            small[3],
            Cell {
                x: 192,
                y: 64,
                size: 64
            }
        );
        assert_eq!(
            small[4],
            Cell {
                x: 0,
                y: 128,
                size: 64
            }
        );
        assert_eq!(atlas.used(), 1.0 + 1.25);
        // Frame cells disappear at the end of the frame, the persistent one stays.
        atlas.end_frame();
        assert_eq!(atlas.used(), 1.0);
        let a = atlas.alloc(128, false).unwrap();
        let b = atlas.alloc(128, false).unwrap();
        let c = atlas.alloc(128, true).unwrap();
        assert_eq!((a.x, a.y, b.x, b.y, c.x, c.y), (128, 0, 0, 128, 128, 128));
        assert_eq!(atlas.alloc(64, false), None);
        atlas.free(big);
        assert_eq!(
            atlas.alloc(64, true),
            Some(Cell {
                x: 0,
                y: 0,
                size: 64
            })
        );
        atlas.free(Cell {
            x: 0,
            y: 0,
            size: 64,
        });
        assert_eq!(atlas.used(), 3.0);
    }
}
