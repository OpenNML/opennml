//! Models on the GPU and drawing of the objects like `ModelInstance_Draw` (`0x0058B5E0`): per
//! instance first the opaque material groups (state `Objects`, alpha test), then the transparent
//! ones (blending on, depth writing off). With a pose (`nml_world::anim::Pose`), skin meshes are
//! deformed in the vertex shader (`ModelInstance_DrawSkinnedGroup` `0x00588170`), rigid meshes
//! are moved along their node track and morph frames are drawn as CPU vertex data
//! (`ModelInstance_DrawCpuVertexGroup` `0x00583480`); without a pose, skin meshes appear in the
//! bind pose.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Affine3A, Mat3, Mat4, Vec4};
use nml_formats::rdo::{MeshKind, RdoMaterial, RdoMesh};
use nml_rda::Vfs;
use nml_world::anim::{MeshPose, Pose, skin_weights};
use nml_world::assets::{ModelAsset, load_texture};
use nml_world::math::mesh_node_matrix;
use wgpu::util::DeviceExt;

use crate::shared::{ALPHA_BLEND, DRAW_STRIDE, PipelineDesc, Shared, pipeline, shader};
use crate::texture::{GpuTexture, upload_rgba};

/// Vertex of rigid meshes (FVF `0x112`: position, normal, UV; stride `0x20`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ObjectVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Vertex of the skin meshes: like [`ObjectVertex`] plus 4 bones and 4 weights
/// (`nml_world::anim::skin_weights`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// `Draw` in `shaders/object.wgsl` (176 bytes, every 256 bytes in the buffer).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DrawUniforms {
    pub world: [[f32; 4]; 4],
    /// [`normal_matrix`] of `world` (WGSL `mat3x3`: three columns of 16 bytes each).
    pub normal: [[f32; 4]; 3],
    pub diffuse: [f32; 4],
    pub ambient: [f32; 4],
    /// x: first bone of the instance in the bone buffer.
    pub bones: [u32; 4],
    /// Cut ([`Cut`]): x world height, y 0 = none, 1 = keep lower part, 2 = keep upper part.
    pub cut: [f32; 4],
}

/// Vertex indices of the `.rdo` per GPU vertex: per triangle `i1, i2, i0` like
/// `Model_FillStaticVertices` (`0x0057F550`).
pub fn mesh_corners(mesh: &RdoMesh) -> Vec<u32> {
    mesh.faces
        .iter()
        .flat_map(|f| [f.vertices[1], f.vertices[2], f.vertices[0]])
        .collect()
}

/// UVs of the triangle corners in the same order, `(u, −v)`; without UVs `(0, 0)`.
fn corner_uvs(mesh: &RdoMesh) -> Vec<[f32; 2]> {
    let mut uvs = Vec::with_capacity(mesh.faces.len() * 3);
    for f in 0..mesh.faces.len() {
        for k in [1, 2, 0] {
            uvs.push(match &mesh.uv_faces {
                Some(uv_faces) => {
                    let [u, v] = mesh.uvs[uv_faces[f][k] as usize];
                    [u, -v]
                }
                None => [0.0, 0.0],
            });
        }
    }
    uvs
}

/// Deviation from the original: non-finite normals (NaN/inf in corrupted meshes) would ruin the
/// whole triangle in the shader; they become the zero vector, so the object receives only the
/// ambient light there. The same applies to missing normals of a morph frame.
fn finite_or_zero(normal: Option<&[f32; 3]>) -> [f32; 3] {
    match normal {
        Some(n) if n.iter().all(|c| c.is_finite()) => *n,
        _ => [0.0; 3],
    }
}

/// Vertices of a mesh from positions and normals (of the `.rdo` or of a morph frame) in the
/// corner order of [`mesh_corners`].
pub fn vertices_from(
    mesh: &RdoMesh,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
) -> Vec<ObjectVertex> {
    mesh_corners(mesh)
        .into_iter()
        .zip(corner_uvs(mesh))
        .map(|(v, uv)| ObjectVertex {
            position: positions[v as usize],
            normal: finite_or_zero(normals.get(v as usize)),
            uv,
        })
        .collect()
}

/// Vertices of a mesh like `Model_FillStaticVertices` (`0x0057F550`): three vertices per triangle
/// in the order `i1, i2, i0`, vertex normal, UV `(u, −v)` (without UVs `(0, 0)`).
pub fn mesh_vertices(mesh: &RdoMesh) -> Vec<ObjectVertex> {
    vertices_from(mesh, &mesh.positions, &mesh.vertex_normals)
}

/// Vertices of a skin mesh like [`mesh_vertices`] with bones and weights per `.rdo` vertex
/// (`skin_weights`).
pub fn skinned_vertices(mesh: &RdoMesh, weights: &[([u16; 4], [f32; 4])]) -> Vec<SkinnedVertex> {
    mesh_corners(mesh)
        .into_iter()
        .zip(mesh_vertices(mesh))
        .map(|(v, base)| SkinnedVertex {
            position: base.position,
            normal: base.normal,
            uv: base.uv,
            joints: weights[v as usize].0,
            weights: weights[v as usize].1,
        })
        .collect()
}

/// Material like `Material_FromRdoMaterial` or `Material_MakeDefaultWhite` (`0x00571960`,
/// `0x00571A00`): colors of the `.rdo` only for `EF-MATERIAL`, otherwise white; alpha `1 − transparency`.
/// Return value `(diffuse, ambient, transparent)`.
///
/// Counterpart of `nml_export::model::material_json` (glTF material of the same rule): the selection
/// `EF-MATERIAL` and the alpha must match in both. Here the colors stay in the sRGB value of the game
/// (GPU uniform); only the export converts them to linear.
pub fn material_colors(material: &RdoMaterial) -> (Vec4, Vec4, bool) {
    let alpha = 1.0 - material.transparency;
    let (diffuse, ambient) = if material.name.eq_ignore_ascii_case("EF-MATERIAL") {
        (
            Vec4::from((glam::Vec3::from(material.diffuse), alpha)),
            Vec4::from((glam::Vec3::from(material.ambient), 1.0)),
        )
    } else {
        (Vec4::new(1.0, 1.0, 1.0, alpha), Vec4::ONE)
    };
    (diffuse, ambient, material.transparency != 0.0)
}

/// Index of the material that gets the player color: the **first** `EF-MATERIAL` of the model
/// (`FUN_00580B10`: name comparison, then `return`; the names are uppercased at load time, here
/// compared ignoring case). Further `EF-MATERIAL`s keep the `.rdo` color (D-220).
pub fn player_colored_material(materials: &[RdoMaterial]) -> Option<usize> {
    materials
        .iter()
        .position(|m| m.name.eq_ignore_ascii_case("EF-MATERIAL"))
}

/// Normal matrix of rigid and morph meshes: inverse transpose of the linear part of the world
/// matrix, **without** normalization - this is how the D3D fixed-function pipeline
/// (`ModelInstance_DrawStaticGroup`, `NORMALIZENORMALS` stays off) transforms normals; with
/// scaling `s` the normal becomes `1/s` shorter. Non-invertible matrices (scaling 0) yield the
/// zero matrix (ambient light only) instead of NaN - the mesh is flat then anyway.
pub fn normal_matrix(world: &Affine3A) -> Mat3 {
    let linear = Mat3::from(world.matrix3);
    let det = linear.determinant();
    if det == 0.0 || !det.is_finite() {
        return Mat3::ZERO;
    }
    let normal = linear.inverse().transpose();
    if normal.is_finite() {
        normal
    } else {
        Mat3::ZERO
    }
}

struct Run {
    first_vertex: u32,
    vertex_count: u32,
    material: usize,
}

struct MeshGpu {
    /// Index in `RdoFile::meshes`.
    rdo_index: usize,
    /// Vertices in the bind pose ([`ObjectVertex`]).
    buffer: wgpu::Buffer,
    /// Skin meshes of a model with bones: vertices with bones and weights
    /// ([`SkinnedVertex`]).
    skinned: Option<wgpu::Buffer>,
    /// The mesh has a morph track in an animation of the model (vertices rebuilt every frame).
    morphs: bool,
    /// Node matrix (rigid) or identity matrix (skin, object space).
    node: Affine3A,
    runs: Vec<Run>,
}

struct MaterialGpu {
    bind_group: wgpu::BindGroup,
    diffuse: Vec4,
    ambient: Vec4,
    transparent: bool,
    /// The first `EF-MATERIAL` of the model ([`player_colored_material`]).
    player_colored: bool,
}

/// A model on the GPU.
pub struct ModelGpu {
    /// The model data (for poses: bones, meshes, animations).
    pub asset: Arc<ModelAsset>,
    meshes: Vec<MeshGpu>,
    materials: Vec<MaterialGpu>,
    /// Bounding sphere in object space (`RdoBounds` of the header): center, radius.
    pub bounds: (glam::Vec3, f32),
}

/// Loads models and textures once per path.
pub struct ModelLibrary {
    textures: HashMap<String, Option<Arc<GpuTexture>>>,
    models: HashMap<String, Arc<ModelGpu>>,
    white: Arc<GpuTexture>,
}

impl ModelLibrary {
    pub fn new(white: Arc<GpuTexture>) -> Self {
        Self {
            textures: HashMap::new(),
            models: HashMap::new(),
            white,
        }
    }

    fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        path: &str,
    ) -> Arc<GpuTexture> {
        let entry = self.textures.entry(path.to_string()).or_insert_with(|| {
            match load_texture(vfs, path) {
                Ok(img) => Some(Arc::new(upload_rgba(device, queue, &img, false, path))),
                Err(e) => {
                    log::warn!("{e:#}");
                    None
                }
            }
        });
        entry.clone().unwrap_or_else(|| self.white.clone())
    }

    /// GPU data of a model (from memory or newly created).
    pub fn model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        model: &Arc<ModelAsset>,
    ) -> Arc<ModelGpu> {
        if let Some(gpu) = self.models.get(&model.path) {
            return gpu.clone();
        }
        let colored = player_colored_material(&model.rdo.materials);
        let materials = model
            .rdo
            .materials
            .iter()
            .zip(&model.material_textures)
            .enumerate()
            .map(|(index, (material, texture))| {
                let texture = match texture {
                    Some(path) => self.texture(device, queue, vfs, path),
                    None => self.white.clone(),
                };
                let (diffuse, ambient, transparent) = material_colors(material);
                MaterialGpu {
                    bind_group: shared.textures(device, &texture.view, &self.white.view),
                    diffuse,
                    ambient,
                    transparent,
                    player_colored: colored == Some(index),
                }
            })
            .collect();
        let morph_names: HashSet<&str> = model
            .animations
            .iter()
            .flatten()
            .flat_map(|a| a.morph_tracks.iter().map(|t| t.name.as_str()))
            .collect();
        let buffer = |label: &str, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let meshes = model
            .rdo
            .meshes
            .iter()
            .enumerate()
            .filter(|(_, m)| m.kind != MeshKind::Helper && !m.faces.is_empty())
            .map(|(index, mesh)| {
                let skinned =
                    (mesh.kind == MeshKind::Skin && !model.rdo.bones.is_empty()).then(|| {
                        let vertices = skinned_vertices(mesh, &skin_weights(&model.rdo, index));
                        buffer(&mesh.name, bytemuck::cast_slice(&vertices))
                    });
                MeshGpu {
                    rdo_index: index,
                    buffer: buffer(&mesh.name, bytemuck::cast_slice(&mesh_vertices(mesh))),
                    skinned,
                    morphs: morph_names.contains(mesh.name.as_str()),
                    node: match mesh.kind {
                        MeshKind::Rigid => mesh_node_matrix(mesh),
                        _ => Affine3A::IDENTITY,
                    },
                    runs: mesh
                        .material_runs()
                        .into_iter()
                        .map(|(first, count, material)| Run {
                            first_vertex: first as u32 * 3,
                            vertex_count: count as u32 * 3,
                            material: material as usize,
                        })
                        .collect(),
                }
            })
            .collect();
        let gpu = Arc::new(ModelGpu {
            asset: model.clone(),
            meshes,
            materials,
            bounds: (
                glam::Vec3::from(model.rdo.bounds.center),
                model.rdo.bounds.radius,
            ),
        });
        self.models.insert(model.path.clone(), gpu.clone());
        gpu
    }
}

/// Cut of a model at a horizontal plane (construction phase and fire, `ObjectRenderer_Draw`
/// `0x00549AC0`): the original splits the triangles on the CPU (`0x00584A50` keeps the part below,
/// `0x00585CE0` the part above the plane); here the fragment shader discards the other side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    /// Height of the plane in world space.
    pub height: f32,
    /// `true`: keep the part above the plane, otherwise the part below it.
    pub keep_above: bool,
}

/// Deviating appearance of an instance (models in the effect pass, construction phase, fire).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceLook {
    /// `Material_MakeTinted(r, g, b)` (`0x005832F0`) for all material groups, also `EF-MATERIAL`:
    /// diffuse = ambient = `(r, g, b)`, diffuse alpha stays (fire `[0.2; 0.2; 0.2]`, construction
    /// preview green `[0, 1, 0]` or red `[1, 0, 0]`).
    pub tint: Option<[f32; 3]>,
    /// `Material_SetAlpha(a)`: diffuse alpha `a`, all groups in the transparent pass.
    pub alpha: Option<f32>,
    /// Fog like the objects; `false` = `FOGENABLE` 0.
    pub fog: bool,
    /// Alpha test `GREATER` 0; `false` = `ALPHATESTENABLE` 0 (state `eff_objects`).
    pub alpha_test: bool,
    /// Draw only the part on one side of a horizontal plane.
    pub cut: Option<Cut>,
    /// Brightness factor of the fog (`ObjectRenderer+0x4C`): multiplies diffuse and ambient color
    /// after the tint like `ObjectRenderer_Draw` `0x00549C2D`; 1 = unchanged.
    pub brightness: f32,
    /// Player color `ObjectRenderer+0x9C…+0xA4` (`g_PlayerColors[GameObject+0x2D0] / 255`):
    /// `ObjectRenderer_Draw` (`0x00549AC0`) writes it before every draw via `FUN_005884C0`
    /// -> `FUN_00580B10` as diffuse and ambient into the first `EF-MATERIAL` ([`player_colored_material`])
    /// of the model and the sub-objects. `None` = `.rdo` color (model browser, scaffolding, effects).
    pub player_color: Option<[f32; 3]>,
    /// Untinted like the silhouette (`DrawTransparentGroup` `0x0058A850` with flags 1, re/world-display
    /// W.2.1, D-264): the first `EF-MATERIAL` in player color * brightness, further `EF-MATERIAL`s
    /// with their `.rdo` color (`Material_FromRdoMaterial`), all other groups white
    /// (`Material_MakeDefaultWhite`) - without tint and without brightness factor; the texture stays.
    pub default_white: bool,
}

impl Default for InstanceLook {
    fn default() -> Self {
        Self {
            tint: None,
            alpha: None,
            fog: true,
            alpha_test: true,
            cut: None,
            brightness: 1.0,
            player_color: None,
            default_white: false,
        }
    }
}

/// Material or tint color of an instance with brightness factor (`ObjectRenderer_Draw`
/// `0x00549AC0`: base color and tint * `+0x4C`). `player_colored`: the material is the first
/// `EF-MATERIAL` of the model; it then takes the player color of the instance (before tint and `k`).
pub(crate) fn instance_colors(
    diffuse: Vec4,
    ambient: Vec4,
    player_colored: bool,
    look: &InstanceLook,
) -> (Vec4, Vec4) {
    let (mut diffuse, mut ambient) = (diffuse, ambient);
    // Silhouette (`DrawTransparentGroup` `0x0058A850` with flags 1): per group `Material_FromRdoMaterial`
    // for `EF-MATERIAL`, otherwise `Material_MakeDefaultWhite`. The base colors from `material_colors`
    // already follow this rule (`.rdo` color only for `EF-MATERIAL`, otherwise white); the first
    // `EF-MATERIAL` carries the player color set beforehand * k (`0x005884C0` with `r.color * r.k`).
    // No tint, no k on the remaining colors.
    if look.default_white {
        if player_colored && let Some([r, g, b]) = look.player_color {
            let k = look.brightness;
            diffuse = Vec4::new(r * k, g * k, b * k, diffuse.w);
            ambient = Vec4::new(r * k, g * k, b * k, 1.0);
        }
        return (diffuse, ambient);
    }
    // `FUN_00580B10`: diffuse = ambient = player color; diffuse alpha stays `1 − transparency`.
    if player_colored && let Some([r, g, b]) = look.player_color {
        diffuse = Vec4::new(r, g, b, diffuse.w);
        ambient = Vec4::new(r, g, b, 1.0);
    }
    if let Some([r, g, b]) = look.tint {
        diffuse = Vec4::new(r, g, b, diffuse.w);
        ambient = Vec4::new(r, g, b, 1.0);
    }
    let k = look.brightness;
    (
        Vec4::new(diffuse.x * k, diffuse.y * k, diffuse.z * k, diffuse.w),
        Vec4::new(ambient.x * k, ambient.y * k, ambient.z * k, ambient.w),
    )
}

/// Diffuse and ambient color of a material group in the draw buffer: [`instance_colors`] with the
/// alpha of the group in **both** colors. `ModelInstance_DrawTransparentGroup` (`0x0058A850`)
/// sets it with `Material_SetAlpha` (`0x00583390`, fade factor: diffuse, ambient and specular
/// alpha) or `Material_ApplyRdoAlpha` (`0x005833B0`: ambient alpha from material `+0x10`, diffuse
/// alpha from `+0x20`; `Model_LoadMaterials` `0x00581D10` fills both with `1 − transparency`, so the
/// reimplementation keeps only the one value). The fixed-function pipeline reads only the diffuse
/// alpha, the skin shader `blendingN.vsh` also the ambient alpha (D-287, `vs_skinned`).
pub(crate) fn draw_colors(
    diffuse: Vec4,
    ambient: Vec4,
    player_colored: bool,
    look: &InstanceLook,
) -> (Vec4, Vec4) {
    let (mut diffuse, mut ambient) = instance_colors(diffuse, ambient, player_colored, look);
    let alpha = look.alpha.unwrap_or(diffuse.w);
    diffuse.w = alpha;
    ambient.w = alpha;
    (diffuse, ambient)
}

/// Which meshes of an instance are drawn: the hide mask of the original
/// (`ModelInstance_SetMeshVisibleByName` `0x0058BCB0`, name lowercase, exact) or a single mesh
/// (arrow from the shooter model, re/combat-projectiles 15.6).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MeshFilter {
    #[default]
    All,
    /// Do not draw these meshes.
    Hide(Arc<[String]>),
    /// Only this mesh, without its node position in the model (the world matrix of the instance is its position).
    Only(Arc<str>),
}

impl MeshFilter {
    pub fn draws(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Hide(names) => !names.iter().any(|n| n.eq_ignore_ascii_case(name)),
            Self::Only(n) => n.eq_ignore_ascii_case(name),
        }
    }
}

/// A model instance to be drawn.
pub struct Instance {
    pub model: Arc<ModelGpu>,
    /// World matrix of the instance (original coordinates).
    pub world: Affine3A,
    /// Pose (`AnimationPlayer::pose` of the same model); `None` = bind pose.
    pub pose: Option<Pose>,
    pub look: InstanceLook,
    /// Hide mask or single mesh.
    pub meshes: MeshFilter,
}

/// Where the vertices of a mesh come from in this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    /// `MeshGpu::buffer` with `vs_object` (bind pose, rigid meshes, node track).
    Static,
    /// `MeshGpu::skinned` with `vs_skinned` (bones of the pose).
    Skinned,
    /// Morph buffer `k` of the frame with `vs_object`.
    Morph(usize),
}

struct DrawItem {
    instance: usize,
    mesh: usize,
    run: usize,
    transparent: bool,
    source: Source,
    slot: u32,
}

/// Draws model instances.
pub struct ObjectRenderer {
    opaque: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    skinned_opaque: wgpu::RenderPipeline,
    skinned_transparent: wgpu::RenderPipeline,
    draw_buffer: wgpu::Buffer,
    /// Bone matrices of all skin instances of the frame (64 bytes each).
    bone_buffer: wgpu::Buffer,
    draw_bind_group: wgpu::BindGroup,
    /// Entries that `draw_buffer` and `bone_buffer` hold.
    draw_capacity: u64,
    bone_capacity: u64,
    /// Vertex buffers of the morph frames, reused across frames.
    morph_buffers: Vec<wgpu::Buffer>,
    instances: Vec<Instance>,
    items: Vec<DrawItem>,
}

pub(crate) const OBJECT_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
pub(crate) const SKINNED_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Uint16x4, 4 => Float32x4
];
/// Size of a bone matrix in the bone buffer.
const BONE_SIZE: u64 = std::mem::size_of::<[[f32; 4]; 4]>() as u64;

impl ObjectRenderer {
    pub fn new(device: &wgpu::Device, shared: &Shared, color_format: wgpu::TextureFormat) -> Self {
        let module = shader(device, "Objects", include_str!("shaders/object.wgsl"));
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
        let layouts = [
            &shared.frame_layout,
            &shared.textures_layout,
            &shared.draw_layout,
        ];
        let make = |label, vertex, layout: &wgpu::VertexBufferLayout, blend, depth_write| {
            pipeline(
                device,
                &PipelineDesc {
                    label,
                    layouts: &layouts,
                    module: &module,
                    vertex,
                    fragment: "fs_object",
                    buffers: std::slice::from_ref(layout),
                    blend,
                    depth_write,
                    cull: true,
                    color_format,
                },
            )
        };
        let opaque = make("Objects", "vs_object", &static_layout, None, true);
        let transparent = make(
            "Objects transparent",
            "vs_object",
            &static_layout,
            Some(ALPHA_BLEND),
            false,
        );
        let skinned_opaque = make("Objects Skin", "vs_skinned", &skinned_layout, None, true);
        let skinned_transparent = make(
            "Objects Skin transparent",
            "vs_skinned",
            &skinned_layout,
            Some(ALPHA_BLEND),
            false,
        );
        let draw_buffer = Self::buffer(
            device,
            "Draw uniforms",
            DRAW_STRIDE,
            wgpu::BufferUsages::UNIFORM,
        );
        let bone_buffer = Self::buffer(device, "Bones", BONE_SIZE, wgpu::BufferUsages::STORAGE);
        let draw_bind_group = Self::bind_group(device, shared, &draw_buffer, &bone_buffer);
        Self {
            opaque,
            transparent,
            skinned_opaque,
            skinned_transparent,
            draw_buffer,
            bone_buffer,
            draw_bind_group,
            draw_capacity: 1,
            bone_capacity: 1,
            morph_buffers: Vec::new(),
            instances: Vec::new(),
            items: Vec::new(),
        }
    }

    fn buffer(
        device: &wgpu::Device,
        label: &str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        shared: &Shared,
        draw: &wgpu::Buffer,
        bones: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Draw uniforms"),
            layout: &shared.draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: draw,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniforms>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bones.as_entire_binding(),
                },
            ],
        })
    }

    /// Sets the instances and writes draw uniforms, bone matrices and morph vertices.
    /// World matrix per mesh = instance matrix * node matrix (`ModelInstance_SetStaticMeshMatrix`
    /// `0x005833E0`) or * node track of the pose (`ModelInstance_UpdateMeshMatrices`
    /// `0x0058B2D0`); morph meshes lie in object space (instance matrix only).
    pub fn set_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        instances: Vec<Instance>,
    ) {
        let mut items = Vec::new();
        let mut uniforms: Vec<u8> = Vec::new();
        let mut bones: Vec<[[f32; 4]; 4]> = Vec::new();
        let mut morphs: Vec<Vec<ObjectVertex>> = Vec::new();
        for (i, instance) in instances.iter().enumerate() {
            let asset = &instance.model.asset;
            let bone_base = bones.len() as u32;
            // Bones only for models with bones; without a pose, skin meshes stay in the
            // bind pose (`vs_object`).
            let animated_skin = instance
                .pose
                .as_ref()
                .filter(|_| !asset.rdo.bones.is_empty());
            if let Some(pose) = animated_skin {
                bones.extend(
                    (0..asset.rdo.bones.len())
                        .map(|b| Mat4::from(pose.bone_matrix(asset, b)).to_cols_array_2d()),
                );
            }
            let mut sources = Vec::with_capacity(instance.model.meshes.len());
            let mut matrices = Vec::with_capacity(instance.model.meshes.len());
            let mut normals = Vec::with_capacity(instance.model.meshes.len());
            for mesh in &instance.model.meshes {
                let source = match &instance.pose {
                    Some(pose) => match &pose.meshes[mesh.rdo_index] {
                        MeshPose::Morph { positions, normals } if mesh.morphs => {
                            let rdo = &asset.rdo.meshes[mesh.rdo_index];
                            morphs.push(vertices_from(rdo, positions, normals));
                            Source::Morph(morphs.len() - 1)
                        }
                        _ if mesh.skinned.is_some() && animated_skin.is_some() => Source::Skinned,
                        _ => Source::Static,
                    },
                    None => Source::Static,
                };
                let local = if matches!(instance.meshes, MeshFilter::Only(_)) {
                    Affine3A::IDENTITY
                } else {
                    match &instance.pose {
                        Some(pose) => pose.mesh_matrix(asset, mesh.rdo_index),
                        None => mesh.node,
                    }
                };
                sources.push(source);
                let world = instance.world * local;
                let normal = normal_matrix(&world);
                matrices.push(Mat4::from(world).to_cols_array_2d());
                normals.push([
                    normal.x_axis.extend(0.0).to_array(),
                    normal.y_axis.extend(0.0).to_array(),
                    normal.z_axis.extend(0.0).to_array(),
                ]);
            }
            let look = instance.look;
            for pass_transparent in [false, true] {
                for (m, mesh) in instance.model.meshes.iter().enumerate() {
                    if !instance
                        .meshes
                        .draws(&asset.rdo.meshes[mesh.rdo_index].name)
                    {
                        continue;
                    }
                    for (r, run) in mesh.runs.iter().enumerate() {
                        let Some(material) = instance.model.materials.get(run.material) else {
                            continue;
                        };
                        let transparent = material.transparent || look.alpha.is_some();
                        if transparent != pass_transparent {
                            continue;
                        }
                        let (diffuse, ambient) = draw_colors(
                            material.diffuse,
                            material.ambient,
                            material.player_colored,
                            &look,
                        );
                        let data = DrawUniforms {
                            world: matrices[m],
                            normal: normals[m],
                            diffuse: diffuse.to_array(),
                            ambient: ambient.to_array(),
                            bones: [
                                bone_base,
                                u32::from(!look.fog),
                                u32::from(!look.alpha_test),
                                0,
                            ],
                            cut: match look.cut {
                                None => [0.0; 4],
                                Some(c) => {
                                    [c.height, if c.keep_above { 2.0 } else { 1.0 }, 0.0, 0.0]
                                }
                            },
                        };
                        uniforms.extend_from_slice(bytemuck::bytes_of(&data));
                        uniforms.resize(
                            uniforms.len() + DRAW_STRIDE as usize
                                - std::mem::size_of::<DrawUniforms>(),
                            0,
                        );
                        items.push(DrawItem {
                            instance: i,
                            mesh: m,
                            run: r,
                            transparent: pass_transparent,
                            source: sources[m],
                            slot: items.len() as u32,
                        });
                    }
                }
            }
        }
        let mut rebind = false;
        let needed = (items.len() as u64).max(1);
        if needed > self.draw_capacity {
            self.draw_capacity = needed.next_power_of_two();
            self.draw_buffer = Self::buffer(
                device,
                "Draw uniforms",
                self.draw_capacity * DRAW_STRIDE,
                wgpu::BufferUsages::UNIFORM,
            );
            rebind = true;
        }
        let needed = (bones.len() as u64).max(1);
        if needed > self.bone_capacity {
            self.bone_capacity = needed.next_power_of_two();
            self.bone_buffer = Self::buffer(
                device,
                "Bones",
                self.bone_capacity * BONE_SIZE,
                wgpu::BufferUsages::STORAGE,
            );
            rebind = true;
        }
        if rebind {
            self.draw_bind_group =
                Self::bind_group(device, shared, &self.draw_buffer, &self.bone_buffer);
        }
        if !uniforms.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, &uniforms);
        }
        if !bones.is_empty() {
            queue.write_buffer(&self.bone_buffer, 0, bytemuck::cast_slice(&bones));
        }
        for (k, vertices) in morphs.iter().enumerate() {
            let bytes: &[u8] = bytemuck::cast_slice(vertices);
            if self
                .morph_buffers
                .get(k)
                .is_none_or(|b| b.size() < bytes.len() as u64)
            {
                let buffer = Self::buffer(
                    device,
                    "Morph",
                    bytes.len() as u64,
                    wgpu::BufferUsages::VERTEX,
                );
                if k < self.morph_buffers.len() {
                    self.morph_buffers[k] = buffer;
                } else {
                    self.morph_buffers.push(buffer);
                }
            }
            queue.write_buffer(&self.morph_buffers[k], 0, bytes);
        }
        self.instances = instances;
        self.items = items;
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Draws the instances `instances` with foreign pipelines (occluders of the shadow textures,
    /// models of the effects): `pick(instance, skin, transparent)` selects the pipeline of a run
    /// or skips it (`None`). This function sets group 1 (material) and 2 (draw uniforms); the
    /// caller sets groups 0 and 3.
    pub fn draw_instances<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        instances: std::ops::Range<usize>,
        pick: impl Fn(usize, bool, bool) -> Option<&'p wgpu::RenderPipeline>,
    ) {
        for item in self
            .items
            .iter()
            .filter(|item| instances.contains(&item.instance))
        {
            let Some(pipeline) = pick(
                item.instance,
                item.source == Source::Skinned,
                item.transparent,
            ) else {
                continue;
            };
            pass.set_pipeline(pipeline);
            let model = &self.instances[item.instance].model;
            let mesh = &model.meshes[item.mesh];
            let run = &mesh.runs[item.run];
            let buffer = match item.source {
                Source::Static => &mesh.buffer,
                Source::Skinned => mesh.skinned.as_ref().unwrap_or(&mesh.buffer),
                Source::Morph(k) => &self.morph_buffers[k],
            };
            pass.set_bind_group(1, &model.materials[run.material].bind_group, &[]);
            pass.set_bind_group(2, &self.draw_bind_group, &[item.slot * DRAW_STRIDE as u32]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(run.first_vertex..run.first_vertex + run.vertex_count, 0..1);
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared) {
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        // Set the pipeline only when (skin, transparent) changes.
        let mut current: Option<(bool, bool)> = None;
        for item in &self.items {
            let skinned = item.source == Source::Skinned;
            if current != Some((skinned, item.transparent)) {
                pass.set_pipeline(match (skinned, item.transparent) {
                    (false, false) => &self.opaque,
                    (false, true) => &self.transparent,
                    (true, false) => &self.skinned_opaque,
                    (true, true) => &self.skinned_transparent,
                });
                current = Some((skinned, item.transparent));
            }
            let model = &self.instances[item.instance].model;
            let mesh = &model.meshes[item.mesh];
            let run = &mesh.runs[item.run];
            let buffer = match item.source {
                Source::Static => &mesh.buffer,
                // `Skinned` is chosen only for meshes with a skin buffer (`set_instances`).
                Source::Skinned => mesh.skinned.as_ref().unwrap_or(&mesh.buffer),
                Source::Morph(k) => &self.morph_buffers[k],
            };
            pass.set_bind_group(1, &model.materials[run.material].bind_group, &[]);
            pass.set_bind_group(2, &self.draw_bind_group, &[item.slot * DRAW_STRIDE as u32]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(run.first_vertex..run.first_vertex + run.vertex_count, 0..1);
        }
    }
}

/// Sample model for tests: `sample_rdo` with `sample_rdm` as animation 0, without textures. With
/// `morph`, the animation lacks the node track of the rigid `kiste`, so its morph track applies.
#[cfg(test)]
pub(crate) fn animated_model(path: &str, morph: bool) -> Arc<ModelAsset> {
    use nml_formats::object_cfg::{AnimationEntry, ObjectConfig};
    use nml_formats::rdm::RdmLayout;
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::{sample_rdm, sample_rdo};
    let rdo = sample_rdo(RdoLayout::Standard);
    let mut anim = sample_rdm(RdmLayout::Standard);
    if morph {
        anim.node_tracks.clear();
    }
    Arc::new(ModelAsset {
        path: path.into(),
        rdo,
        config: ObjectConfig {
            model_file: None,
            sub_objects: vec![],
            animations: vec![AnimationEntry {
                file: Some("laufen.rdm".into()),
                guid: None,
                max_bounding_box: None,
                sub_objects: vec![],
            }],
        },
        animations: vec![Some(Arc::new(anim))],
        material_textures: vec![None, None],
        bone_track_cache: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::sample_rdo;
    use nml_world::anim::AnimationPlayer;

    #[test]
    fn mesh_filters_compare_names_case_insensitively() {
        let hide = MeshFilter::Hide(vec!["pfeil".to_string()].into());
        assert!(!hide.draws("Pfeil"));
        assert!(hide.draws("bogen"));
        let only = MeshFilter::Only("pfeil".into());
        assert!(only.draws("PFEIL"));
        assert!(!only.draws("bogen"));
        assert!(MeshFilter::All.draws("x"));
    }

    #[test]
    fn vertices_follow_model_fill_static_vertices() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let v = mesh_vertices(&rdo.meshes[0]);
        assert_eq!(v.len(), 3);
        // Order i1, i2, i0; UV (u, −v).
        assert_eq!(v[0].position, [0.0, 10.0, 0.0]);
        assert_eq!(v[2].position, [0.0, 0.0, 0.0]);
        assert_eq!(v[0].uv, [0.0, -1.0]);
        assert_eq!(mesh_vertices(&rdo.meshes[1])[0].uv, [0.0, 0.0]);
        assert_eq!(mesh_corners(&rdo.meshes[1]), [1, 2, 0, 3, 2, 1]);
    }

    #[test]
    fn skinned_and_morphed_vertices_keep_the_corner_order() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let weights = skin_weights(&rdo, 1);
        let v = skinned_vertices(&rdo.meshes[1], &weights);
        assert_eq!(v.len(), 6);
        assert_eq!((v[0].joints, v[0].weights), ([0; 4], [1.0, 0.0, 0.0, 0.0]));
        assert_eq!(v[0].position, [0.0, 100.0, 0.0]);
        let moved: Vec<[f32; 3]> = rdo.meshes[0]
            .positions
            .iter()
            .map(|p| [p[0], p[1] + 5.0, p[2]])
            .collect();
        let m = vertices_from(&rdo.meshes[0], &moved, &rdo.meshes[0].vertex_normals);
        assert_eq!(m[0].position, [0.0, 15.0, 0.0]);
        // Missing or non-finite normals of a morph frame become the zero vector.
        let m = vertices_from(&rdo.meshes[0], &moved, &[[f32::NAN, 0.0, 0.0]]);
        assert_eq!((m[0].normal, m[2].normal), ([0.0; 3], [0.0; 3]));
    }

    #[test]
    fn normal_matrix_is_the_inverse_transpose_without_normalization() {
        // Uniform scaling 5 (e.g. `OBJ_SCALE`): normal * 1/5 like the D3D fixed-function pipeline
        // without NORMALIZENORMALS.
        let world = Affine3A::from_scale_rotation_translation(
            glam::Vec3::splat(5.0),
            glam::Quat::from_rotation_y(0.5),
            glam::Vec3::new(1.0, 2.0, 3.0),
        );
        let n = normal_matrix(&world) * glam::Vec3::Z;
        assert!((n.length() - 0.2).abs() < 1e-6, "{n:?}");
        assert!((n.normalize() - glam::Quat::from_rotation_y(0.5) * glam::Vec3::Z).length() < 1e-6);
        // Non-uniform: a surface in the x-z plane stays perpendicular to it.
        let squash = Affine3A::from_scale(glam::Vec3::new(1.0, 4.0, 1.0));
        let tangent = squash.transform_vector3(glam::Vec3::new(1.0, 1.0, 0.0));
        let normal = normal_matrix(&squash) * glam::Vec3::new(1.0, -1.0, 0.0);
        assert!(tangent.dot(normal).abs() < 1e-6);
        // Very small but valid scaling: normal * 1/s (not truncated).
        let tiny = Affine3A::from_scale(glam::Vec3::splat(1e-4));
        let n = normal_matrix(&tiny) * glam::Vec3::Z;
        assert!((n.length() - 1e4).abs() < 1.0, "{n:?}");
        // Not invertible (scaling 0): zero matrix instead of NaN.
        let flat = Affine3A::from_scale(glam::Vec3::new(1.0, 0.0, 1.0));
        assert_eq!(normal_matrix(&flat), glam::Mat3::ZERO);
    }

    #[test]
    fn material_colors_like_the_game() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let (diffuse, ambient, transparent) = material_colors(&rdo.materials[0]);
        assert_eq!(
            (diffuse.to_array(), ambient.to_array(), transparent),
            ([0.8, 0.6, 0.4, 1.0], [0.5, 0.5, 0.5, 1.0], false)
        );
        let (diffuse, ambient, transparent) = material_colors(&rdo.materials[1]);
        assert_eq!(
            (diffuse.to_array(), ambient.to_array(), transparent),
            ([1.0, 1.0, 1.0, 0.5], [1.0; 4], true)
        );
    }

    /// Pipelines, bone buffer, morph buffer and the drawing of all paths (static, node track,
    /// skin, morph) without validation errors; skipped without a graphics card.
    #[test]
    fn object_pipelines_validate_on_the_gpu() {
        use crate::shared::DEPTH_FORMAT;
        use crate::texture::white;
        use crate::{BackendChoice, CAPTURE_FORMAT, headless_device};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shared = Shared::new(&device);
        let mut renderer = ObjectRenderer::new(&device, &shared, CAPTURE_FORMAT);
        renderer.set_instances(&device, &queue, &shared, Vec::new());
        assert!(renderer.instances().is_empty());

        let vfs = Vfs::from_archives(Vec::new());
        let mut library = ModelLibrary::new(Arc::new(white(&device, &queue)));
        let rigid = animated_model("data/r3d/test/kiste.rdo", false);
        let morph = animated_model("data/r3d/test/morph.rdo", true);
        let mut instances: Vec<(Arc<ModelGpu>, Option<Pose>)> = Vec::new();
        for asset in [&rigid, &morph] {
            let model = library.model(&device, &queue, &shared, &vfs, asset);
            let cached = library.model(&device, &queue, &shared, &vfs, asset);
            assert!(Arc::ptr_eq(&model, &cached));
            let pose = AnimationPlayer::new(Some(0), 0.0).pose(asset, 900.0);
            instances.push((model.clone(), None));
            instances.push((model, Some(pose)));
        }
        // Set twice: the buffers are reused the second time.
        for _ in 0..2 {
            let set = instances
                .iter()
                .map(|(model, pose)| Instance {
                    model: model.clone(),
                    world: Affine3A::IDENTITY,
                    pose: pose.clone(),
                    look: InstanceLook::default(),
                    meshes: MeshFilter::All,
                })
                .collect();
            renderer.set_instances(&device, &queue, &shared, set);
        }
        assert_eq!(renderer.instances().len(), 4);
        let sources: Vec<&str> = renderer
            .items
            .iter()
            .map(|item| match item.source {
                Source::Static => "static",
                Source::Skinned => "skin",
                Source::Morph(_) => "morph",
            })
            .collect();
        // Per instance first kiste (opaque), then koerper (transparent); the node track
        // of the rigid kiste draws statically with its own world matrix.
        assert_eq!(
            sources,
            [
                "static", "static", "static", "skin", "static", "static", "morph", "skin"
            ]
        );

        let size = wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        };
        let target = |format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let color = target(CAPTURE_FORMAT);
        let depth = target(DEPTH_FORMAT);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            renderer.draw(&mut pass, &shared);
        }
        queue.submit([encoder.finish()]);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    #[test]
    fn brightness_scales_diffuse_and_ambient_after_the_tint() {
        let (d, a) = (Vec4::new(0.8, 0.6, 0.4, 0.5), Vec4::new(0.2, 0.2, 0.2, 1.0));
        let look = InstanceLook {
            brightness: 0.5,
            ..InstanceLook::default()
        };
        assert_eq!(
            instance_colors(d, a, false, &look),
            (Vec4::new(0.4, 0.3, 0.2, 0.5), Vec4::new(0.1, 0.1, 0.1, 1.0))
        );
        let tinted = InstanceLook {
            tint: Some([1.0, 0.0, 0.0]),
            brightness: 0.5,
            ..InstanceLook::default()
        };
        assert_eq!(
            instance_colors(d, a, false, &tinted),
            (Vec4::new(0.5, 0.0, 0.0, 0.5), Vec4::new(0.5, 0.0, 0.0, 1.0))
        );
        assert_eq!(InstanceLook::default().brightness, 1.0);
    }

    /// `FUN_00580B10`: only the first `EF-MATERIAL` gets the player color as diffuse and ambient
    /// (diffuse alpha stays), after that tint and `k` apply as before; a second `EF-MATERIAL`
    /// (as in `kanone_sid.rdo`) and the white material keep their color.
    #[test]
    fn the_player_colour_replaces_only_the_first_ef_material() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let mut second = rdo.materials[0].clone();
        second.name = "EF-Material".into();
        let materials = [rdo.materials[1].clone(), rdo.materials[0].clone(), second];
        assert_eq!(player_colored_material(&materials), Some(1));
        assert_eq!(player_colored_material(&materials[..1]), None);
        let colors = |look: &InstanceLook| -> Vec<(Vec4, Vec4)> {
            let colored = player_colored_material(&materials);
            materials
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let (d, a, _) = material_colors(m);
                    instance_colors(d, a, colored == Some(i), look)
                })
                .collect()
        };
        let yellow = InstanceLook {
            player_color: Some([1.0, 0.8, 0.0]),
            brightness: 0.5,
            ..InstanceLook::default()
        };
        assert_eq!(
            colors(&yellow),
            [
                // White (alpha 0.5) * k.
                (Vec4::new(0.5, 0.5, 0.5, 0.5), Vec4::new(0.5, 0.5, 0.5, 1.0)),
                // First EF-MATERIAL: player color * k.
                (Vec4::new(0.5, 0.4, 0.0, 1.0), Vec4::new(0.5, 0.4, 0.0, 1.0)),
                // Second EF-MATERIAL: `.rdo` color (0.8; 0.6; 0.4) or (0.5; ...) * k.
                (
                    Vec4::new(0.4, 0.3, 0.2, 1.0),
                    Vec4::new(0.25, 0.25, 0.25, 1.0)
                ),
            ]
        );
        // Without a player color the `.rdo` color; with a tint, the tint wins.
        let plain = colors(&InstanceLook {
            brightness: 0.5,
            ..InstanceLook::default()
        });
        assert_eq!(plain[1], plain[2]);
        let burnt = colors(&InstanceLook {
            tint: Some([0.2; 3]),
            ..yellow
        });
        assert!(
            burnt
                .iter()
                .all(|(d, a)| d.truncate() == glam::Vec3::splat(0.1)
                    && a.truncate() == glam::Vec3::splat(0.1))
        );
    }

    /// Silhouette (`DrawTransparentGroup` `0x0058A850` with flags 1, D-264): the first `EF-MATERIAL`
    /// in player color * `k` (`0x005884C0` with `r.color · r.k`), further `EF-MATERIAL`s with their
    /// `.rdo` color (`Material_FromRdoMaterial`), all others white (`Material_MakeDefaultWhite`);
    /// white and `.rdo` color without `k`, a tint has no effect.
    #[test]
    fn the_stealth_look_is_white_with_the_player_colour_times_brightness() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let mut second = rdo.materials[0].clone();
        second.name = "EF-Material".into();
        let materials = [rdo.materials[1].clone(), rdo.materials[0].clone(), second];
        let colored = player_colored_material(&materials);
        let colors = |look: &InstanceLook| -> Vec<(Vec4, Vec4)> {
            materials
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let (d, a, _) = material_colors(m);
                    instance_colors(d, a, colored == Some(i), look)
                })
                .collect()
        };
        let ghost = InstanceLook {
            alpha: Some(0.1),
            player_color: Some([1.0, 0.8, 0.0]),
            brightness: 0.5,
            default_white: true,
            ..InstanceLook::default()
        };
        assert_eq!(
            colors(&ghost),
            [
                // White without k (diffuse alpha is set only by `Material_SetAlpha` when drawing).
                (Vec4::new(1.0, 1.0, 1.0, 0.5), Vec4::ONE),
                // First EF-MATERIAL: player color * k.
                (Vec4::new(0.5, 0.4, 0.0, 1.0), Vec4::new(0.5, 0.4, 0.0, 1.0)),
                // Second EF-MATERIAL: `.rdo` color without k.
                (Vec4::new(0.8, 0.6, 0.4, 1.0), Vec4::new(0.5, 0.5, 0.5, 1.0)),
            ]
        );
        // A tint does not change the silhouette.
        let tinted = InstanceLook {
            tint: Some([0.2; 3]),
            ..ghost
        };
        assert_eq!(colors(&tinted), colors(&ghost));
        assert!(!InstanceLook::default().default_white);
    }

    /// `Material_SetAlpha` (`0x00583390`) sets diffuse and ambient alpha, `Material_ApplyRdoAlpha`
    /// (`0x005833B0`) takes both from the material (`Model_LoadMaterials` `0x00581D10`: both
    /// `1 − transparency`). So ambient alpha always equals diffuse alpha (D-287), also with player
    /// color, tint and silhouette white; opaque groups stay at 1.
    #[test]
    fn the_ambient_alpha_follows_the_diffuse_alpha() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let (ef, standard) = (&rdo.materials[0], &rdo.materials[1]);
        let looks = [
            InstanceLook::default(),
            InstanceLook {
                player_color: Some([1.0, 0.8, 0.0]),
                brightness: 0.5,
                ..InstanceLook::default()
            },
            InstanceLook {
                tint: Some([0.2; 3]),
                ..InstanceLook::default()
            },
        ];
        for look in &looks {
            for colored in [false, true] {
                // Opaque EF-MATERIAL: both alphas 1.
                let (d, a, _) = material_colors(ef);
                let (d, a) = draw_colors(d, a, colored, look);
                assert_eq!((d.w, a.w), (1.0, 1.0), "{look:?}");
                // `.rdo` transparency 0.5: both 0.5.
                let (d, a, _) = material_colors(standard);
                let (d, a) = draw_colors(d, a, colored, look);
                assert_eq!((d.w, a.w), (0.5, 0.5), "{look:?}");
            }
        }
        // Fade factor or silhouette: both equal to the factor.
        for (alpha, default_white) in [(0.1, true), (0.5, true), (0.3, false)] {
            let look = InstanceLook {
                alpha: Some(alpha),
                player_color: Some([1.0, 0.8, 0.0]),
                default_white,
                ..InstanceLook::default()
            };
            for material in [ef, standard] {
                let (d, a, _) = material_colors(material);
                let (d, a) = draw_colors(d, a, true, &look);
                assert_eq!((d.w, a.w), (alpha, alpha));
            }
        }
    }

    /// Renders the skin mesh `koerper` of the sample model (plane z = 0, normal (0, 0, −1), white
    /// material with `transparency` 0.5, bones in the rest pose) orthographically, filling the
    /// image, onto black and returns the red value of one pixel: with ambient and diffuse light 1
    /// the color is white, so the pixel is `255 · α`. `pose` `None` draws via `vs_object` (fixed
    /// function), otherwise via `vs_skinned` (`blendingN.vsh`). `None` without a graphics card.
    fn rendered_alpha(light_dir: [f32; 3], skinned: bool, look: InstanceLook) -> Option<u8> {
        use crate::shared::{DEPTH_FORMAT, FrameUniforms};
        use crate::texture::white;
        use crate::{BackendChoice, CAPTURE_FORMAT, headless_device, render_to_image};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return None;
        };
        let shared = Shared::new(&device);
        let mut renderer = ObjectRenderer::new(&device, &shared, CAPTURE_FORMAT);
        let vfs = Vfs::from_archives(Vec::new());
        let mut library = ModelLibrary::new(Arc::new(white(&device, &queue)));
        let asset = animated_model("data/r3d/test/koerper.rdo", false);
        let model = library.model(&device, &queue, &shared, &vfs, &asset);
        let pose = skinned.then(|| Pose::rest(&asset));
        renderer.set_instances(
            &device,
            &queue,
            &shared,
            vec![Instance {
                model,
                world: Affine3A::IDENTITY,
                pose,
                look,
                meshes: MeshFilter::Hide(Arc::from(["kiste".to_string()])),
            }],
        );
        let expected = if skinned {
            Source::Skinned
        } else {
            Source::Static
        };
        assert!(renderer.items.iter().all(|item| item.source == expected));
        let mut frame = FrameUniforms::new(
            Mat4::IDENTITY,
            // Orthographic: x 0...20 and y 0...100 filling the image, z -1...1 -> depth 0...1.
            Mat4::from_cols(
                Vec4::new(0.1, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 0.02, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 0.5, 0.0),
                Vec4::new(-1.0, -1.0, 0.5, 1.0),
            ),
        );
        frame.light_dir = [light_dir[0], light_dir[1], light_dir[2], 0.0];
        frame.light_diffuse = [1.0; 4];
        frame.light_ambient = [1.0; 4];
        queue.write_buffer(&shared.frame_buffer, 0, bytemuck::bytes_of(&frame));
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 8,
                    height: 8,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let image = render_to_image(&device, &queue, (8, 8), CAPTURE_FORMAT, |encoder, view| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            renderer.draw(&mut pass, &shared);
        })
        .expect("reading the image");
        Some(image.get_pixel(2, 5).0[0])
    }

    /// D-287: skin meshes with a running animation (`ModelInstance_DrawSkinnedGroup` `0x00588170`,
    /// `blendingN.vsh`) blend with `α = ambient.a · 1 + max(0, N·L) · diffuse.a · 1`, i.e.
    /// `a · (1 + max(0, N·L))` clamped to [0, 1]; the fixed-function pipeline (rigid meshes, skin
    /// meshes without a pose) takes only `diffuse.a`. Silhouette alpha 0.1 -> 0.1...0.2, own camouflage
    /// 0.5 -> 0.5...1.
    #[test]
    fn skinned_meshes_mix_with_the_light_alpha() {
        let ghost = |alpha| InstanceLook {
            alpha: Some(alpha),
            player_color: Some([1.0, 0.8, 0.0]),
            default_white: true,
            ..InstanceLook::default()
        };
        let toward = [0.0, 0.0, -1.0]; // N*L = 1
        let away = [0.0, 0.0, 1.0]; // N*L = -1
        let side = [1.0, 0.0, 0.0]; // N*L = 0
        let cases = [
            // Silhouette alpha 0.1: lit side 0.2, shadow side 0.1; rigid 0.1.
            (toward, true, ghost(0.1), 0.2),
            (side, true, ghost(0.1), 0.1),
            (away, true, ghost(0.1), 0.1),
            (toward, false, ghost(0.1), 0.1),
            // Own camouflaged units alpha 0.5: lit side opaque.
            (toward, true, ghost(0.5), 1.0),
            (away, true, ghost(0.5), 0.5),
            (toward, false, ghost(0.5), 0.5),
            // `.rdo` transparency 0.5 without fade factor (`Material_ApplyRdoAlpha`).
            (toward, true, InstanceLook::default(), 1.0),
            (away, true, InstanceLook::default(), 0.5),
            (toward, false, InstanceLook::default(), 0.5),
        ];
        for (light, skinned, look, alpha) in cases {
            let Some(red) = rendered_alpha(light, skinned, look) else {
                return;
            };
            let expected = alpha * 255.0;
            assert!(
                (f32::from(red) - expected).abs() <= 1.0,
                "light {light:?}, skin {skinned}, alpha {:?}: pixel {red}, expected {expected}",
                look.alpha
            );
        }
    }
}
