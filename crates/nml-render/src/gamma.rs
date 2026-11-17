//! Gamma ramp of the original as a post-processing step (re/renderer, section "Gamma ramp").
//!
//! The original uses `Renderer_SetGammaRamp` (`0x00572060`) to set a gamma ramp of the device
//! (`IDirect3DDevice8::SetGammaRamp`); wgpu has no counterpart for it. The reimplementation computes the same
//! ramp ([`gamma_ramp`]) and applies it as the last pass before presenting ([`GammaPass`]). Screenshots
//! bypass this pass and, as in the original, do not contain the ramp.

/// Factor `65535.0` (`0x00632A78`, `float` `0x477FFF00`).
const RAMP_SCALE: f32 = 65535.0;
/// Factor `1/255` (`0x00618468`, `float` `0x3B808081`).
const INV_255: f32 = f32::from_bits(0x3B80_8081);

/// Ramp of one channel like `Renderer_SetGammaRamp` (`0x0057208B`...`0x005721CB`): for `i = 0…255`
/// `ftol(clamp(((f32)i · (G · 65535)) · (1/255), 0, 65535))`. Every step is rounded to `f32` (the x87 unit
/// runs at single precision, O-99). The clamping follows the comparisons of the original: less than 0 -> 0,
/// from 65535 on and unordered (NaN, e.g. `0 · ∞`) -> 65535. `ftol` truncates toward 0.
///
/// The ramp is linear: `G = 1` maps unchanged (at 8 bits), larger values brighten and clip at the top.
pub fn gamma_ramp(gamma: f32) -> [u16; 256] {
    let scaled = gamma * RAMP_SCALE;
    std::array::from_fn(|i| {
        let x = (i as f32 * scaled) * INV_255;
        let clamped = if x < 0.0 {
            0.0
        } else if x < RAMP_SCALE {
            x
        } else {
            // `x ≥ 65535` and NaN
            RAMP_SCALE
        };
        clamped as u16
    })
}

/// Value of a ramp entry on an 8-bit output: `entry / 65535` with the rounding of the `Unorm8` target.
pub fn ramp_to_8bit(entry: u16) -> u8 {
    ((u32::from(entry) * 255 + 32767) / 65535) as u8
}

/// Whether the ramp changes nothing on an 8-bit output (then the pass is omitted, e.g. at `G = 1`).
pub fn ramp_is_identity(ramp: &[u16; 256]) -> bool {
    ramp.iter()
        .enumerate()
        .all(|(i, &entry)| usize::from(ramp_to_8bit(entry)) == i)
}

/// Values for the shader: entry / 65535, four each in a `vec4`.
fn ramp_uniform(ramp: &[u16; 256]) -> [f32; 256] {
    std::array::from_fn(|i| f32::from(ramp[i]) / RAMP_SCALE)
}

/// Intermediate target of one size with a bind group.
struct Target {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// "Gamma" pass: the frame is drawn into an intermediate target ([`GammaPass::source`]) and written through the ramp
/// into the actual target with [`GammaPass::apply`].
pub struct GammaPass {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    ramp: wgpu::Buffer,
    target: Option<Target>,
}

impl GammaPass {
    /// Pass for targets in format `format` (intermediate target in the same format).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Gamma"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/gamma.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Gamma"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Gamma"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Gamma"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_gamma"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_gamma"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let ramp = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Gamma ramp"),
            size: std::mem::size_of::<[f32; 256]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            format,
            pipeline,
            layout,
            ramp,
            target: None,
        }
    }

    /// Loads the ramp (effective from the next [`GammaPass::apply`] that is submitted after it).
    pub fn set_ramp(&self, queue: &wgpu::Queue, ramp: &[u16; 256]) {
        queue.write_buffer(&self.ramp, 0, bytemuck::cast_slice(&ramp_uniform(ramp)));
    }

    /// Intermediate target of size `size`, recreated if the size differs.
    pub fn source(&mut self, device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
        let size = (size.0.max(1), size.1.max(1));
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Frame before gamma"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Gamma"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.ramp.as_entire_binding(),
                    },
                ],
            });
            self.target = Some(Target {
                size,
                view,
                bind_group,
            });
        }
        self.target
            .as_ref()
            .map(|t| t.view.clone())
            .expect("intermediate target created")
    }

    /// Writes the intermediate target through the ramp to `target` (same size as at [`GammaPass::source`]).
    pub fn apply(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let Some(source) = &self.target else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Gamma"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &source.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendChoice, CAPTURE_FORMAT, headless_device, render_to_image};

    // QUIRK D-368: kept
    #[test]
    fn constants_are_the_floats_of_the_original() {
        assert_eq!(RAMP_SCALE.to_bits(), 0x477F_FF00);
        assert_eq!(INV_255.to_bits(), 0x3B80_8081);
    }

    // QUIRK D-368: kept
    #[test]
    fn neutral_gamma_keeps_eight_bit_values() {
        let ramp = gamma_ramp(1.0);
        assert!(ramp_is_identity(&ramp));
        // Also exactly i * 257 in 16 bits (recomputed for all 256 entries).
        assert!(
            ramp.iter()
                .enumerate()
                .all(|(i, &e)| u32::from(e) == 257 * i as u32)
        );
    }

    // QUIRK D-368: kept
    #[test]
    fn ramp_is_linear_and_clipped_like_the_original() {
        // Slider 50 -> G = 1.5 (O-99): factor 1.5, clipped at the top from input 170.
        let bright = gamma_ramp(1.5);
        assert!(!ramp_is_identity(&bright));
        assert_eq!(bright[100], 38550);
        assert_eq!(ramp_to_8bit(bright[100]), 150);
        assert_eq!(bright[169], 65149);
        assert_eq!(ramp_to_8bit(bright[169]), 253);
        assert_eq!(bright[170], 65535);
        assert_eq!(bright[255], 65535);
        // Slider 0 -> G = 0.5: half brightness.
        let dark = gamma_ramp(0.5);
        assert_eq!(dark[200], 25700);
        assert_eq!(ramp_to_8bit(dark[200]), 100);
        assert_eq!(dark[255], 32767);
        assert_eq!(ramp_to_8bit(dark[255]), 127);
        // Monotonically increasing.
        assert!(bright.windows(2).all(|w| w[0] <= w[1]));
        assert!(dark.windows(2).all(|w| w[0] <= w[1]));
    }

    // QUIRK D-368: kept
    #[test]
    fn clamping_follows_the_comparisons_of_the_original() {
        // Negative (e.g. `Gamma -1` in Engine.ini): everything 0, black.
        assert!(gamma_ramp(-1.0).iter().all(|&e| e == 0));
        // NaN: unordered -> 65535.
        assert!(gamma_ramp(f32::NAN).iter().all(|&e| e == 65535));
        // inf: entry 0 is 0 * inf = NaN -> 65535, the remaining inf -> 65535.
        assert!(gamma_ramp(f32::INFINITY).iter().all(|&e| e == 65535));
    }

    // QUIRK D-368: kept
    #[test]
    fn pass_applies_the_ramp_per_channel() {
        // Skip if there is no graphics card.
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        for format in [CAPTURE_FORMAT, wgpu::TextureFormat::Bgra8Unorm] {
            let mut pass = GammaPass::new(&device, format);
            let ramp = gamma_ramp(1.5);
            pass.set_ramp(&queue, &ramp);
            let img = render_to_image(&device, &queue, (4, 2), format, |encoder, target| {
                let source = pass.source(&device, (4, 2));
                {
                    let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Test image"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &source,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 100.0 / 255.0,
                                    g: 200.0 / 255.0,
                                    b: 0.0,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                }
                pass.apply(encoder, target);
            })
            .unwrap();
            let expected = [ramp_to_8bit(ramp[100]), ramp_to_8bit(ramp[200]), 0, 255];
            assert_eq!(expected, [150, 255, 0, 255]);
            assert_eq!(img.get_pixel(0, 0).0, expected, "format {format:?}");
            assert_eq!(img.get_pixel(3, 1).0, expected, "format {format:?}");
        }
    }
}
