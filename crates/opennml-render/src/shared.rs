//! Shared GPU building blocks of the world passes: frame uniforms, binding layouts, samplers and
//! the creation of the pipelines (one pipeline per state block of the original).

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

/// Depth format (original: D16; 32-bit floating point is at least as precise).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// `Frame` in `shaders/common.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub projection: [[f32; 4]; 4],
    pub light_dir: [f32; 4],
    pub light_diffuse: [f32; 4],
    pub light_ambient: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
    pub water: [f32; 4],
    pub water2: [f32; 4],
    pub view_proj_bias: [[f32; 4]; 4],
}

/// Projection with depth bias like `Matrix_PerspectiveFovLHDepthBias` (`0x00575B00`):
/// `m22 · (1 − Q · 10⁻⁶)` with `Q = m22` (`f/(f − n)`), otherwise unchanged.
pub fn depth_bias_projection(projection: Mat4) -> Mat4 {
    let mut biased = projection;
    let q = projection.z_axis.z;
    biased.z_axis.z = q * (1.0 - q * 1e-6);
    biased
}

impl FrameUniforms {
    pub fn new(view: Mat4, projection: Mat4) -> Self {
        Self {
            view_proj_bias: (depth_bias_projection(projection) * view).to_cols_array_2d(),
            view_proj: (projection * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            projection: projection.to_cols_array_2d(),
            light_dir: [0.0, 0.0, 1.0, 0.0],
            light_diffuse: [1.0; 4],
            light_ambient: [0.0, 0.0, 0.0, 1.0],
            fog_color: [0.0, 0.0, 0.0, 1.0],
            fog: [0.0; 4],
            water: [0.0; 4],
            water2: [0.0; 4],
        }
    }
}

/// Binding layouts and samplers that all world passes share.
pub struct Shared {
    pub frame_layout: wgpu::BindGroupLayout,
    pub textures_layout: wgpu::BindGroupLayout,
    pub draw_layout: wgpu::BindGroupLayout,
    pub frame_buffer: wgpu::Buffer,
    pub frame_bind_group: wgpu::BindGroup,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// Size of an entry in the buffer of the draw uniforms (alignment of dynamic offsets).
pub const DRAW_STRIDE: u64 = 256;

impl Shared {
    pub fn new(device: &wgpu::Device) -> Self {
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                sampler_entry(1),
                sampler_entry(2),
                sampler_entry(3),
            ],
        });
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture pair"),
            entries: &[texture_entry(0), texture_entry(1)],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Draw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<
                            crate::objects::DrawUniforms,
                        >() as u64),
                    },
                    count: None,
                },
                // Bone matrices of all skin instances of a frame (object space).
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                },
            ],
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Frame uniforms"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |label: &str, address: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let wrap = sampler("WRAP", wgpu::AddressMode::Repeat);
        let mirror = sampler("MIRROR", wgpu::AddressMode::MirrorRepeat);
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("CLAMP"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Frame"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&wrap),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&mirror),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&clamp),
                },
            ],
        });
        Self {
            frame_layout,
            textures_layout,
            draw_layout,
            frame_buffer,
            frame_bind_group,
        }
    }

    /// Bind group for two textures (`tex_a`, `tex_b`).
    pub fn textures(
        &self,
        device: &wgpu::Device,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Texture pair"),
            layout: &self.textures_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
            ],
        })
    }
}

/// Description of a pipeline (corresponds to a state block of the original).
pub struct PipelineDesc<'a> {
    pub label: &'a str,
    pub layouts: &'a [&'a wgpu::BindGroupLayout],
    pub module: &'a wgpu::ShaderModule,
    pub vertex: &'a str,
    pub fragment: &'a str,
    pub buffers: &'a [wgpu::VertexBufferLayout<'a>],
    pub blend: Option<wgpu::BlendState>,
    pub depth_write: bool,
    /// `CULLMODE` CCW -> discard back faces (front face clockwise); NONE -> `false`.
    pub cull: bool,
    pub color_format: wgpu::TextureFormat,
}

/// SRCALPHA/INVSRCALPHA.
pub const ALPHA_BLEND: wgpu::BlendState = wgpu::BlendState::ALPHA_BLENDING;

pub fn pipeline(device: &wgpu::Device, desc: &PipelineDesc) -> wgpu::RenderPipeline {
    pipeline_with_topology(device, desc, wgpu::PrimitiveTopology::TriangleList)
}

/// Like [`pipeline`] with its own primitive type (e.g. line list).
pub fn pipeline_with_topology(
    device: &wgpu::Device,
    desc: &PipelineDesc,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> =
        desc.layouts.iter().map(|l| Some(*l)).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(desc.label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    let buffers: Vec<Option<wgpu::VertexBufferLayout>> =
        desc.buffers.iter().cloned().map(Some).collect();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(desc.label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: desc.module,
            entry_point: Some(desc.vertex),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology,
            // Direct3D: front faces appear clockwise; wgpu has the same screen orientation.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: desc.cull.then_some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(desc.depth_write),
            // D3D default ZFUNC LESSEQUAL.
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: desc.module,
            entry_point: Some(desc.fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: desc.color_format,
                blend: desc.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Shader module from `common.wgsl` and one further file.
pub fn shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let code = format!("{}\n{}", include_str!("shaders/common.wgsl"), source);
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(code.into()),
    })
}
