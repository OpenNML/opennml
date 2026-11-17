//! Effects on the GPU (re/effects, section "Embedding in the frame"): quads of the particles and
//! billboards from `nml_effects::output::EffectFrame` with the states `eff_billboard` (additive)
//! and `eff_billboard_alpha`, in view space or in world coordinates, plus models (model effects,
//! surf, fish, eagles) through the object renderer. Drawing follows the order of the commands,
//! without sorting.

use std::collections::HashMap;
use std::sync::Arc;

use nml_effects::output::{
    DrawCommand, EffectBlend, EffectFrame, EffectSpace, EffectVertex, ModelDraw,
};
use nml_rda::{Vfs, normalize_path};
use nml_world::anim::AnimationPlayer;
use nml_world::assets::{ModelCache, load_texture};

use crate::objects::{
    Instance, InstanceLook, MeshFilter, ModelLibrary, OBJECT_ATTRIBUTES, ObjectRenderer,
    ObjectVertex, SKINNED_ATTRIBUTES, SkinnedVertex,
};
use crate::shared::{ALPHA_BLEND, PipelineDesc, Shared, pipeline, shader};
use crate::texture::{GpuTexture, upload_rgba};

/// `D3DBLEND` -> wgpu. `DESTALPHA`/`INVDESTALPHA` act like 1 and 0 respectively on the game's
/// framebuffer without alpha (R5G6B5 or X8R8G8B8); `BOTHSRCALPHA`/`BOTHINVSRCALPHA` act like
/// `SRCALPHA`/`INVSRCALPHA` (only in the unused `object_test.pef`).
pub fn d3d_blend(value: i32) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as F;
    match value {
        1 => F::Zero,
        2 | 7 => F::One,
        3 => F::Src,
        4 => F::OneMinusSrc,
        5 | 12 => F::SrcAlpha,
        6 | 13 => F::OneMinusSrcAlpha,
        8 => F::Zero,
        9 => F::Dst,
        10 => F::OneMinusDst,
        11 => F::SrcAlphaSaturated,
        _ => F::One,
    }
}

/// Pipeline of a model in the effect pass: skin, blend factors (`None` = opaque with depth
/// writing), culling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ModelKey {
    skinned: bool,
    blend: Option<(wgpu::BlendFactor, wgpu::BlendFactor)>,
    cull: bool,
}

/// Pipeline key of a run: like an object (`Objects`), otherwise all groups with the blend factors
/// of the command.
fn model_key(draw: &ModelDraw, skinned: bool, transparent: bool) -> ModelKey {
    let blend = match (draw.alpha, draw.blend) {
        (None, _) if !transparent => None,
        (_, Some((src, dst))) => Some((d3d_blend(src), d3d_blend(dst))),
        _ => Some((
            wgpu::BlendFactor::SrcAlpha,
            wgpu::BlendFactor::OneMinusSrcAlpha,
        )),
    };
    ModelKey {
        skinned,
        blend,
        cull: draw.cull,
    }
}

/// Look of a model command: tint, opacity and fog of the command, brightness 1 (without fog
/// brightness `k`). The player color comes from the command: `None` for the effects, i.e. the
/// `.rdo` color instead of the player color of another object that was last written into the
/// shared material table (D-221); for the rally point flag its flag color (`0x005884C0`, D-259).
fn model_look(draw: &ModelDraw) -> InstanceLook {
    InstanceLook {
        tint: draw.tint.map(|s| [s, s, s]),
        alpha: draw.alpha,
        fog: draw.fog,
        // State `eff_objects` of the model effects: no alpha test.
        alpha_test: draw.blend.is_none(),
        cut: None,
        brightness: 1.0,
        player_color: draw.player_color,
        default_white: false,
    }
}

/// `SRCALPHA`/`ONE` for color and alpha (`eff_billboard`).
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Position, `DIFFUSE` (as four bytes B, G, R, A), UV; `SPECULAR` stays unread.
const ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Unorm8x4,
        offset: 12,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 20,
        shader_location: 2,
    },
];

/// Pipelines and textures of the effects, shared by all passes.
pub struct EffectPipelines {
    /// `[space][blend]`: view space/world x additive/alpha.
    pipelines: [[wgpu::RenderPipeline; 2]; 2],
    textures: HashMap<String, Option<(Arc<GpuTexture>, wgpu::BindGroup)>>,
    white: wgpu::BindGroup,
    object_module: wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    models: HashMap<ModelKey, wgpu::RenderPipeline>,
}

fn space_index(space: EffectSpace) -> usize {
    match space {
        EffectSpace::View => 0,
        EffectSpace::World => 1,
    }
}

fn blend_index(blend: EffectBlend) -> usize {
    match blend {
        EffectBlend::Additive => 0,
        EffectBlend::Alpha => 1,
    }
}

impl EffectPipelines {
    pub fn new(
        device: &wgpu::Device,
        shared: &Shared,
        color_format: wgpu::TextureFormat,
        white: &GpuTexture,
    ) -> Self {
        let module = shader(device, "Effects", include_str!("shaders/effects.wgsl"));
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<EffectVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        };
        let make = |label, vertex, blend| {
            pipeline(
                device,
                &PipelineDesc {
                    label,
                    layouts: &[&shared.frame_layout, &shared.textures_layout],
                    module: &module,
                    vertex,
                    fragment: "fs_effect",
                    buffers: std::slice::from_ref(&layout),
                    blend: Some(blend),
                    depth_write: false,
                    cull: false,
                    color_format,
                },
            )
        };
        Self {
            pipelines: [
                [
                    make("eff_billboard view space", "vs_view", ADDITIVE),
                    make("eff_billboard_alpha view space", "vs_view", ALPHA_BLEND),
                ],
                [
                    make("eff_billboard world", "vs_world", ADDITIVE),
                    make("eff_billboard_alpha world", "vs_world", ALPHA_BLEND),
                ],
            ],
            textures: HashMap::new(),
            white: shared.textures(device, &white.view, &white.view),
            object_module: shader(device, "Effect models", include_str!("shaders/object.wgsl")),
            color_format,
            models: HashMap::new(),
        }
    }

    /// Creates the pipeline of a model run (object shader, without depth writing when
    /// blending).
    fn model_pipeline(&mut self, device: &wgpu::Device, shared: &Shared, key: ModelKey) {
        if self.models.contains_key(&key) {
            return;
        }
        let (stride, attributes, vertex): (u64, &[wgpu::VertexAttribute], &str) = if key.skinned {
            (
                std::mem::size_of::<SkinnedVertex>() as u64,
                &SKINNED_ATTRIBUTES,
                "vs_skinned",
            )
        } else {
            (
                std::mem::size_of::<ObjectVertex>() as u64,
                &OBJECT_ATTRIBUTES,
                "vs_object",
            )
        };
        let layout = wgpu::VertexBufferLayout {
            array_stride: stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes,
        };
        let blend = key.blend.map(|(src, dst)| {
            let component = wgpu::BlendComponent {
                src_factor: src,
                dst_factor: dst,
                operation: wgpu::BlendOperation::Add,
            };
            wgpu::BlendState {
                color: component,
                alpha: component,
            }
        });
        let made = pipeline(
            device,
            &PipelineDesc {
                label: "Effect model",
                layouts: &[
                    &shared.frame_layout,
                    &shared.textures_layout,
                    &shared.draw_layout,
                ],
                module: &self.object_module,
                vertex,
                fragment: "fs_object",
                buffers: std::slice::from_ref(&layout),
                blend,
                depth_write: blend.is_none(),
                cull: key.cull,
                color_format: self.color_format,
            },
        );
        self.models.insert(key, made);
    }

    /// Loads the texture of a command once per normalized path (without a mip chain); if it is
    /// missing, white is drawn (logged once).
    fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        path: &str,
    ) {
        let key = normalize_path(path);
        self.textures
            .entry(key)
            .or_insert_with_key(|key| match load_texture(vfs, key) {
                // `Renderer_LoadTexture` with one level: no mip chain.
                Ok(img) => {
                    let texture = Arc::new(upload_rgba(device, queue, &img, false, key));
                    let bind = shared.textures(device, &texture.view, &texture.view);
                    Some((texture, bind))
                }
                Err(e) => {
                    log::warn!("Effect texture: {e:#}");
                    None
                }
            });
    }

    fn bind_group(&self, path: Option<&str>) -> &wgpu::BindGroup {
        path.and_then(|p| self.textures.get(&normalize_path(p)))
            .and_then(Option::as_ref)
            .map_or(&self.white, |(_, bind)| bind)
    }
}

/// One pass of the effects (e.g. above the water): vertex buffer, models and commands of a
/// frame.
pub struct EffectPass {
    buffer: Option<wgpu::Buffer>,
    commands: Vec<DrawCommand>,
    quads: usize,
    /// Models of the commands (one instance per `DrawCommand::Model`, in command order).
    objects: ObjectRenderer,
    /// Drawn models (loadable) of the last frame.
    models: usize,
    /// Instance per command (`None`: quads or a model that cannot be loaded).
    instance_of: Vec<Option<usize>>,
}

impl EffectPass {
    pub fn new(device: &wgpu::Device, shared: &Shared, color_format: wgpu::TextureFormat) -> Self {
        Self {
            buffer: None,
            commands: Vec::new(),
            quads: 0,
            objects: ObjectRenderer::new(device, shared, color_format),
            models: 0,
            instance_of: Vec::new(),
        }
    }

    /// Takes over the effects of a frame; the buffer grows as needed. Models come from `models`
    /// (those that cannot be loaded are dropped) and are drawn in their animation at the given
    /// time.
    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        vfs: &Vfs,
        pipelines: &mut EffectPipelines,
        library: &mut ModelLibrary,
        models: &mut ModelCache,
        frame: &EffectFrame,
    ) {
        let mut instances = Vec::new();
        self.instance_of.clear();
        for command in &frame.commands {
            match command {
                DrawCommand::Quads(batch) => {
                    if let Some(texture) = &batch.texture {
                        pipelines.texture(device, queue, shared, vfs, texture);
                    }
                    self.instance_of.push(None);
                }
                DrawCommand::Model(draw) => {
                    let Some(asset) = models.get(vfs, &draw.model) else {
                        self.instance_of.push(None);
                        continue;
                    };
                    for skinned in [false, true] {
                        for transparent in [false, true] {
                            pipelines.model_pipeline(
                                device,
                                shared,
                                model_key(draw, skinned, transparent),
                            );
                        }
                    }
                    let pose = draw
                        .animation
                        .map(|(index, ms)| AnimationPlayer::new(Some(index), 0.0).pose(&asset, ms));
                    self.instance_of.push(Some(instances.len()));
                    instances.push(Instance {
                        model: library.model(device, queue, shared, vfs, &asset),
                        world: draw.world,
                        pose,
                        look: model_look(draw),
                        meshes: draw
                            .mesh
                            .as_deref()
                            .map_or(MeshFilter::All, |m| MeshFilter::Only(m.into())),
                    });
                }
            }
        }
        self.models = instances.len();
        self.objects.set_instances(device, queue, shared, instances);
        self.commands = frame.commands.clone();
        self.quads = frame.quad_count();
        if frame.vertices.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&frame.vertices);
        if self
            .buffer
            .as_ref()
            .is_none_or(|b| b.size() < bytes.len() as u64)
        {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Effect vertices"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.buffer {
            queue.write_buffer(buffer, 0, bytes);
        }
    }

    /// Drawn quads of the last frame.
    pub fn quad_count(&self) -> usize {
        self.quads
    }

    /// Drawn models of the last frame.
    pub fn model_count(&self) -> usize {
        self.models
    }

    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        shared: &Shared,
        pipelines: &EffectPipelines,
    ) {
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        for (command, instance) in self.commands.iter().zip(&self.instance_of) {
            match command {
                DrawCommand::Quads(batch) => {
                    let Some(buffer) = &self.buffer else { continue };
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.set_pipeline(
                        &pipelines.pipelines[space_index(batch.space)][blend_index(batch.blend)],
                    );
                    pass.set_bind_group(1, pipelines.bind_group(batch.texture.as_deref()), &[]);
                    pass.draw(batch.vertices.clone(), 0..1);
                }
                DrawCommand::Model(draw) => {
                    let Some(i) = *instance else { continue };
                    self.objects
                        .draw_instances(pass, i..i + 1, |_, skinned, transparent| {
                            pipelines.models.get(&model_key(draw, skinned, transparent))
                        });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::{Affine3A, Vec3};

    use super::*;

    /// The rally point flag (`PropertyBuilding_RenderMeetingFlag` `0x00507BB0`) draws with
    /// `ModelInstance_Render(1.0, ...)`: without fog brightness `k`, with its player color in the
    /// first `EF-MATERIAL` (D-259); the other effect models keep the `.rdo` color (D-221).
    #[test]
    fn flag_ignores_fog_brightness() {
        let draw = ModelDraw {
            model: "data/r3d/misc/tools/sammelpunkt/flag.rdo".into(),
            world: Affine3A::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            animation: Some((0, 250.0)),
            tint: None,
            alpha: None,
            blend: None,
            cull: true,
            fog: true,
            mesh: None,
            player_color: Some([1.0, 0.5, 0.25]),
        };
        let look = model_look(&draw);
        assert_eq!(look.brightness, 1.0);
        assert_eq!(look.player_color, Some([1.0, 0.5, 0.25]));
        assert!(look.fog && look.alpha_test && look.tint.is_none() && look.alpha.is_none());
        let plain = model_look(&ModelDraw {
            player_color: None,
            ..draw
        });
        assert_eq!(plain.player_color, None);
    }
}
