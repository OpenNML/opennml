//! Weather on the GPU like `Terrain_RenderWeatherPass` (`0x00557210`): particle centers as
//! instances, quad in the vertex shader; state block `Weather` (additive `SRCALPHA`/`ONE`, depth
//! read without writing, no culling, no fog). The simulation lives in
//! `opennml_world::weather`.

use opennml_world::environment::WeatherKind;

use crate::shared::{PipelineDesc, Shared, pipeline, shader};
use crate::texture::GpuTexture;

/// Textures (both 32 x 32), loaded like `Terrain_InitWeatherParticles` (`0x0055F490`).
pub const SNOW_TEXTURE: &str = "data/r3d/nature/summer/snow/snow.tga";
pub const RAIN_TEXTURE: &str = "data/r3d/nature/summer/rain/rain.tga";

/// `SRCALPHA`/`ONE` for color and alpha.
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

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];

/// Pipelines, textures and instance buffer of the weather.
pub struct WeatherGpu {
    snow_pipeline: wgpu::RenderPipeline,
    rain_pipeline: wgpu::RenderPipeline,
    snow: Option<wgpu::BindGroup>,
    rain: Option<wgpu::BindGroup>,
    instances: Option<wgpu::Buffer>,
    count: u32,
    kind: Option<WeatherKind>,
}

impl WeatherGpu {
    /// `snow`/`rain`: the weather textures; if one is missing, this kind draws nothing.
    pub fn new(
        device: &wgpu::Device,
        shared: &Shared,
        color_format: wgpu::TextureFormat,
        snow: Option<&GpuTexture>,
        rain: Option<&GpuTexture>,
    ) -> Self {
        let module = shader(device, "Weather", include_str!("shaders/weather.wgsl"));
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<[f32; 3]>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        };
        let make = |label, vertex| {
            pipeline(
                device,
                &PipelineDesc {
                    label,
                    layouts: &[&shared.frame_layout, &shared.textures_layout],
                    module: &module,
                    vertex,
                    fragment: "fs_weather",
                    buffers: std::slice::from_ref(&layout),
                    blend: Some(ADDITIVE),
                    depth_write: false,
                    cull: false,
                    color_format,
                },
            )
        };
        Self {
            snow_pipeline: make("Weather Snow", "vs_snow"),
            rain_pipeline: make("Weather Rain", "vs_rain"),
            snow: snow.map(|t| shared.textures(device, &t.view, &t.view)),
            rain: rain.map(|t| shared.textures(device, &t.view, &t.view)),
            instances: None,
            count: 0,
            kind: None,
        }
    }

    /// Particles of this frame (`None` = no weather); the buffer grows as needed.
    pub fn set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        kind: Option<WeatherKind>,
        centers: &[[f32; 3]],
    ) {
        self.kind = kind;
        // Without a texture of the kind nothing is drawn, so nothing is counted either.
        let textured = match kind {
            Some(WeatherKind::Snow) => self.snow.is_some(),
            Some(WeatherKind::Rain) => self.rain.is_some(),
            None => false,
        };
        self.count = if textured { centers.len() as u32 } else { 0 };
        if self.count == 0 {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(centers);
        let fits = self
            .instances
            .as_ref()
            .is_some_and(|b| b.size() >= bytes.len() as u64);
        if !fits {
            self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Weather particles"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.instances {
            queue.write_buffer(buffer, 0, bytes);
        }
    }

    /// Number of drawn particles.
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared) {
        let (Some(kind), Some(buffer)) = (self.kind, &self.instances) else {
            return;
        };
        let (pipeline, textures) = match kind {
            WeatherKind::Snow => (&self.snow_pipeline, &self.snow),
            WeatherKind::Rain => (&self.rain_pipeline, &self.rain),
        };
        let Some(textures) = textures else { return };
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        pass.set_bind_group(1, textures, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}
