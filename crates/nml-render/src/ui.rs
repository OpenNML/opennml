//! 2D pass of the UI: implements the `DrawCmd` lists of `nml-ui` (Direct3D 8 state blocks
//! `Font`/`3dicons-*`, re/ui-rendering 3.4; quad calculation re/menu-system 8.4.1).
//!
//! - Blending of the color `SRCALPHA/INVSRCALPHA` (D3D8 default of the blocks), color = texture * diffuse; the
//!   alpha component blends with `One/OneMinusSrcAlpha` (the D3D8 back buffer alpha is never displayed, over
//!   an opaque background alpha stays 255). Color fills (`texture: None`) go through a white 1x1 texture.
//! - [`Blend::Additive`] (state block `billboard`, beam of the loading screen, O-194) through a second pipeline
//!   with `SRCALPHA/ONE`; the target alpha stays as it is.
//! - **D3D8 raster rule:** under D3D8 the pixel center is on whole coordinates,
//!   under wgpu at `+0.5`. All corners are therefore shifted by `+0.5` px: same coverage (top-left
//!   rule) and same sampling point. The corners themselves carry no -0.5 offset, as in the original.
//! - Alpha test of the original (ALPHAREF not determined according to re/menu-system 8.4, section "Open"):
//!   equivalent through the blending, no `discard` in the shader.

use std::collections::BTreeMap;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use nml_ui::assets::UiAssets;
use nml_ui::draw::{Blend, DrawCmd, Filter, TextureId};

use crate::texture::{GpuTexture, update_rgba, upload_rgba, white};

/// Quad vertex: position in NDC, UV, diffuse as RGBA 0...1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct UiVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

const UI_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];

/// Binding of a batch: texture (`None` = white fill) and filter (`false` = POINT, `true` = LINEAR).
type BindKey = (Option<TextureId>, bool);

/// Key of a batch: binding and blend kind (`true` = additive).
type BatchKey = (BindKey, bool);

fn batch_key(texture: Option<TextureId>, filter: Filter, blend: Blend) -> BatchKey {
    (
        (texture, filter == Filter::Linear),
        blend == Blend::Additive,
    )
}

/// An uploaded texture with its state: revision and size of the image it was created from.
struct CachedTexture {
    revision: u32,
    size: (u32, u32),
    texture: GpuTexture,
}

/// ARGB (`0xAARRGGBB`) to RGBA 0...1.
fn argb_to_rgba(color: u32) -> [f32; 4] {
    let channel = |shift: u32| ((color >> shift) & 0xFF) as f32 / 255.0;
    [channel(16), channel(8), channel(0), channel(24)]
}

/// Merges consecutive commands with the same texture, filter and blend kind. The
/// order of the commands is preserved (no reordering across textures): equal keys separated by
/// others form separate batches.
fn batches(cmds: &[DrawCmd]) -> Vec<(BatchKey, Range<usize>)> {
    let mut out: Vec<(BatchKey, Range<usize>)> = Vec::new();
    for (i, cmd) in cmds.iter().enumerate() {
        let key = match *cmd {
            DrawCmd::Quad {
                texture,
                filter,
                blend,
                ..
            } => batch_key(texture, filter, blend),
            DrawCmd::Quad4 {
                texture, filter, ..
            } => batch_key(texture, filter, Blend::Alpha),
        };
        match out.last_mut() {
            Some((last, range)) if *last == key => range.end = i + 1,
            _ => out.push((key, i..i + 1)),
        }
    }
    out
}

/// Six vertices per command (two triangles). Screen pixels -> NDC with the D3D8 raster shift `+0.5`.
fn build_vertices(cmds: &[DrawCmd], size: (u32, u32)) -> Vec<UiVertex> {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let ndc = |x: f32, y: f32| [(x + 0.5) / w * 2.0 - 1.0, 1.0 - (y + 0.5) / h * 2.0];
    let mut vertices = Vec::with_capacity(cmds.len() * 6);
    for cmd in cmds {
        let (dst, uv, color) = match *cmd {
            DrawCmd::Quad { dst, uv, color, .. } => (dst, uv, color),
            // Free quad (corners clockwise from top left): triangles 0-1-3 and 1-2-3 like with `Quad`, with
            // the same shift `+0.5`. No clipping against a rectangle (the minimap extends past the edge).
            DrawCmd::Quad4 {
                corners, uv, color, ..
            } => {
                let color = argb_to_rgba(color);
                let v = |k: usize| UiVertex {
                    position: ndc(corners[k][0], corners[k][1]),
                    uv: uv[k],
                    color,
                };
                vertices.extend([v(0), v(1), v(3), v(1), v(2), v(3)]);
                continue;
            }
        };
        let color = argb_to_rgba(color);
        let v = |x: i32, y: i32, u: f32, vv: f32| UiVertex {
            position: ndc(x as f32, y as f32),
            uv: [u, vv],
            color,
        };
        let [x0, y0, x1, y1] = dst;
        let [u0, v0, u1, v1] = uv;
        vertices.extend([
            v(x0, y0, u0, v0),
            v(x1, y0, u1, v0),
            v(x0, y1, u0, v1),
            v(x1, y0, u1, v0),
            v(x1, y1, u1, v1),
            v(x0, y1, u0, v1),
        ]);
    }
    vertices
}

/// Draws quad commands of the UI in screen pixels (one pass without depth, over the existing
/// image).
pub struct UiRenderer {
    pipeline: wgpu::RenderPipeline,
    /// Additive blending `SRCALPHA/ONE` ([`Blend::Additive`]).
    additive: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    point: wgpu::Sampler,
    linear: wgpu::Sampler,
    /// White 1x1 texture for color fills; created on the first draw (`new` gets no queue).
    white: Option<wgpu::TextureView>,
    /// Loaded textures by ID. The ID applies to the `UiAssets` used for drawing; an image replaced
    /// later (`insert_image` on an existing path, other `UiAssets::revision`) is reloaded on the
    /// next draw: into the existing texture if the size is the same, otherwise as a new texture.
    textures: BTreeMap<TextureId, CachedTexture>,
    bind_groups: BTreeMap<BindKey, wgpu::BindGroup>,
}

impl UiRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UI"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ui.wgsl").into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UI"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UI"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        // Color: SRCALPHA/INVSRCALPHA (state blocks Font/3dicons-*, re/ui-rendering 3.4). The alpha
        // component blends with `One/OneMinusSrcAlpha`: the D3D8 back buffer alpha is never displayed,
        // and over an opaque background the target alpha should stay 255 (screenshots); fidelity to the
        // original concerns only RGB.
        let src_alpha = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let alpha_keep = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = |blend: wgpu::BlendState, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_ui"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<UiVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &UI_ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_ui"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Additive (`billboard`): color `SRCALPHA/ONE`, target alpha unchanged.
        let additive_color = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let alpha_unchanged = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let additive = pipeline(
            wgpu::BlendState {
                color: additive_color,
                alpha: alpha_unchanged,
            },
            "UI additive",
        );
        let pipeline = pipeline(
            wgpu::BlendState {
                color: src_alpha,
                alpha: alpha_keep,
            },
            "UI",
        );
        // CLAMP in both directions, no mip filter (textures without mip levels).
        let sampler = |filter: wgpu::FilterMode, label: &str| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };
        Self {
            pipeline,
            additive,
            bind_layout,
            point: sampler(wgpu::FilterMode::Nearest, "UI POINT"),
            linear: sampler(wgpu::FilterMode::Linear, "UI LINEAR"),
            white: None,
            textures: BTreeMap::new(),
            bind_groups: BTreeMap::new(),
        }
    }

    /// Draws the commands in list order over the existing image (`LoadOp::Load`). Textures are loaded
    /// from `assets` as needed (without mip levels). `size` is the target size in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        assets: &UiAssets,
        cmds: &[DrawCmd],
    ) {
        if cmds.is_empty() {
            return;
        }
        let vertices = build_vertices(cmds, size);
        let stacks = batches(cmds);
        for &((bind, _), _) in &stacks {
            self.prepare(device, queue, assets, bind);
        }
        // Own buffer per call: several calls in the same encoder (overlay and dialog) do not overwrite
        // each other because `write_buffer` takes effect before the encoder is submitted.
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI"),
            size: (vertices.len() * std::mem::size_of::<UiVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("UI"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        for ((bind, additive), range) in stacks {
            pass.set_pipeline(if additive {
                &self.additive
            } else {
                &self.pipeline
            });
            pass.set_bind_group(0, &self.bind_groups[&bind], &[]);
            pass.draw(range.start as u32 * 6..range.end as u32 * 6, 0..1);
        }
    }

    /// Brings the texture of an ID up to the state of the image collection. New: upload. Other revision: with
    /// the same size write to the existing texture (bind groups stay valid, the view is
    /// the same), otherwise create anew and discard the bind groups of this ID.
    fn refresh_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &UiAssets,
        id: TextureId,
    ) {
        let revision = assets.revision(id);
        let image = assets.image(id);
        let size = image.dimensions();
        match self.textures.get_mut(&id) {
            Some(cached) if cached.revision == revision => {}
            Some(cached) if cached.size == size => {
                update_rgba(queue, &cached.texture, image);
                cached.revision = revision;
            }
            stale => {
                let texture = upload_rgba(device, queue, image, false, assets.path(id));
                let entry = CachedTexture {
                    revision,
                    size,
                    texture,
                };
                if stale.is_some() {
                    self.bind_groups
                        .retain(|&(texture, _), _| texture != Some(id));
                }
                self.textures.insert(id, entry);
            }
        }
    }

    /// Loads the texture and creates the bind group for a batch key if it is missing.
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &UiAssets,
        key: BindKey,
    ) {
        let (texture, linear) = key;
        if let Some(id) = texture {
            self.refresh_texture(device, queue, assets, id);
        }
        if self.bind_groups.contains_key(&key) {
            return;
        }
        let view = match texture {
            None => self.white.get_or_insert_with(|| white(device, queue).view),
            Some(id) => &self.textures[&id].texture.view,
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(if linear {
                        &self.linear
                    } else {
                        &self.point
                    }),
                },
            ],
        });
        self.bind_groups.insert(key, bind_group);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CAPTURE_FORMAT, render_to_image};
    use crate::gpu::{BackendChoice, headless_device};
    use image::{Rgba, RgbaImage};
    use nml_ui::draw::{Filter, TextureId};

    /// Gets a GPU; without a device `None` (the test skips itself with a note).
    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        match headless_device(BackendChoice::Auto) {
            Ok((device, queue, _)) => Some((device, queue)),
            Err(_) => {
                eprintln!("no GPU, test skipped");
                None
            }
        }
    }

    fn render(
        gpu: &(wgpu::Device, wgpu::Queue),
        size: (u32, u32),
        assets: &UiAssets,
        cmds: &[DrawCmd],
    ) -> RgbaImage {
        let (device, queue) = gpu;
        let mut ui = UiRenderer::new(device, CAPTURE_FORMAT);
        render_to_image(device, queue, size, CAPTURE_FORMAT, |encoder, view| {
            ui.draw(device, queue, encoder, view, size, assets, cmds);
        })
        .unwrap()
    }

    fn fill(color: u32, dst: [i32; 4]) -> DrawCmd {
        DrawCmd::Quad {
            texture: None,
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color,
            filter: Filter::Point,
            blend: Blend::Alpha,
        }
    }

    /// The same command, additively blended.
    fn additive(cmd: DrawCmd) -> DrawCmd {
        match cmd {
            DrawCmd::Quad {
                texture,
                dst,
                uv,
                color,
                filter,
                ..
            } => DrawCmd::Quad {
                texture,
                dst,
                uv,
                color,
                filter,
                blend: Blend::Additive,
            },
            other => other,
        }
    }

    fn textured(texture: TextureId, dst: [i32; 4], color: u32, filter: Filter) -> DrawCmd {
        DrawCmd::Quad {
            texture: Some(texture),
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color,
            filter,
            blend: Blend::Alpha,
        }
    }

    #[test]
    fn quad4_two_triangles() {
        // Two triangles over corners 0-1-3 and 1-2-3 (corners clockwise from top left), each corner with its UV
        // and the D3D8 shift +0.5 px like `Quad`.
        let corners = [[10.0, 2.5], [18.0, 10.0], [10.0, 17.5], [2.0, 10.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let cmd = DrawCmd::Quad4 {
            texture: None,
            corners,
            uv,
            color: 0x80FF_0000,
            filter: Filter::Point,
        };
        let size = (20, 20);
        let vertices = build_vertices(&[cmd], size);
        assert_eq!(vertices.len(), 6);
        let ndc = |[x, y]: [f32; 2]| [(x + 0.5) / 20.0 * 2.0 - 1.0, 1.0 - (y + 0.5) / 20.0 * 2.0];
        for (vertex, k) in vertices.iter().zip([0, 1, 3, 1, 2, 3]) {
            assert_eq!(vertex.position, ndc(corners[k]), "corner {k}");
            assert_eq!(vertex.uv, uv[k], "corner {k}");
            assert_eq!(vertex.color, argb_to_rgba(0x80FF_0000));
        }
        // Same batch as a `Quad` with the same texture and the same filter.
        let quad = fill(0xFFFF_FFFF, [0, 0, 1, 1]);
        assert_eq!(batches(&[quad, cmd]), vec![(((None, false), false), 0..2)]);
    }

    #[test]
    fn draws_quads_offscreen() {
        let Some(gpu) = gpu() else { return };
        let mut assets = UiAssets::new();
        let white = assets.insert_image("data/w.tga", RgbaImage::from_pixel(1, 1, Rgba([255; 4])));
        let cmds = [textured(white, [4, 4, 12, 12], 0xFFFF_0000, Filter::Point)];
        let img = render(&gpu, (32, 32), &assets, &cmds);
        assert_eq!(img.get_pixel(8, 8).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(20, 20).0[3], 0);
        assert_eq!(img.get_pixel(12, 8).0[3], 0);
        assert_eq!(img.get_pixel(3, 8).0[3], 0);
        assert_eq!(img.get_pixel(4, 4).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(11, 11).0, [255, 0, 0, 255]);
    }

    #[test]
    fn point_filter_keeps_texels() {
        let Some(gpu) = gpu() else { return };
        let mut assets = UiAssets::new();
        let checker = assets.insert_image(
            "data/c.tga",
            RgbaImage::from_fn(2, 2, |x, y| {
                let v = if (x + y) % 2 == 0 { 0 } else { 255 };
                Rgba([v, v, v, 255])
            }),
        );
        let cmds = [textured(checker, [0, 0, 8, 8], 0xFFFF_FFFF, Filter::Point)];
        let img = render(&gpu, (8, 8), &assets, &cmds);
        for (x, y, p) in img.enumerate_pixels() {
            assert!(
                p.0[0] == 0 || p.0[0] == 255,
                "pixel ({x},{y}) = {:?} is not pure black/white",
                p.0
            );
        }
        // In the middle of the texel fields (avoid edges at u = 0.5): checkerboard.
        assert_eq!(img.get_pixel(1, 1).0[0], 0);
        assert_eq!(img.get_pixel(6, 1).0[0], 255);
        assert_eq!(img.get_pixel(1, 6).0[0], 255);
        assert_eq!(img.get_pixel(6, 6).0[0], 0);
    }

    #[test]
    fn alpha_blend_is_srcalpha() {
        let Some(gpu) = gpu() else { return };
        let assets = UiAssets::new();
        let cmds = [
            fill(0xFFFF_FFFF, [0, 0, 8, 8]),
            fill(0x80FF_0000, [0, 0, 8, 8]),
        ];
        let img = render(&gpu, (8, 8), &assets, &cmds);
        let p = img.get_pixel(4, 4).0;
        assert_eq!(p[0], 255);
        assert!(p[1].abs_diff(127) <= 1, "G = {}", p[1]);
        assert!(p[2].abs_diff(127) <= 1, "B = {}", p[2]);
    }

    // QUIRK O-194: kept
    #[test]
    fn additive_blend_adds_srcalpha_times_colour() {
        // State block `billboard` (O-194): target + source * source alpha, the target is kept.
        let Some(gpu) = gpu() else { return };
        let assets = UiAssets::new();
        let cmds = [
            fill(0xFF40_4040, [0, 0, 8, 8]),
            additive(fill(0x80FF_0000, [0, 0, 8, 8])),
        ];
        let p = render(&gpu, (8, 8), &assets, &cmds).get_pixel(4, 4).0;
        // R: 64 + 255 * 0.5 ~ 192; G and B stay 64 (blended normally they would be ~ 32); alpha stays opaque.
        assert!(p[0].abs_diff(192) <= 1, "R = {}", p[0]);
        assert_eq!((p[1], p[2], p[3]), (64, 64, 255));
        // Saturated at 255.
        let cmds = [
            fill(0xFFC0_C0C0, [0, 0, 8, 8]),
            additive(fill(0xFFFF_FFFF, [0, 0, 8, 8])),
        ];
        assert_eq!(
            render(&gpu, (8, 8), &assets, &cmds).get_pixel(4, 4).0,
            [255, 255, 255, 255]
        );
    }

    // QUIRK O-63: kept
    #[test]
    fn destination_alpha_stays_opaque_over_opaque_ground() {
        // The alpha component of the blend is `One/OneMinusSrcAlpha`: the D3D8 back buffer alpha is never
        // displayed, RGB stays `SRCALPHA/INVSRCALPHA` (re/ui-rendering 4.3).
        let Some(gpu) = gpu() else { return };
        let assets = UiAssets::new();
        let over_white = [
            fill(0xFFFF_FFFF, [0, 0, 8, 8]),
            fill(0x80FF_0000, [0, 0, 8, 8]),
        ];
        let p = render(&gpu, (8, 8), &assets, &over_white).get_pixel(4, 4).0;
        assert_eq!(p[0], 255);
        assert!(p[1].abs_diff(127) <= 1, "G = {}", p[1]);
        assert!(p[2].abs_diff(127) <= 1, "B = {}", p[2]);
        assert_eq!(p[3], 255, "target alpha over an opaque background");
        // Over a transparent background the source alpha stays (~ 128).
        let over_clear = [fill(0x80FF_0000, [0, 0, 8, 8])];
        let q = render(&gpu, (8, 8), &assets, &over_clear).get_pixel(4, 4).0;
        assert!(q[3].abs_diff(128) <= 1, "A = {}", q[3]);
    }

    // QUIRK O-63: kept
    #[test]
    fn gray_atlas_darkens_twice() {
        // O-63: atlas pixel RGB = A = 128; SRCALPHA acts in addition to the texture color -> 0.5 * 0.5.
        let Some(gpu) = gpu() else { return };
        let mut assets = UiAssets::new();
        let atlas = assets.insert_image(
            "data/a.tga",
            RgbaImage::from_pixel(1, 1, Rgba([128, 128, 128, 128])),
        );
        let cmds = [
            fill(0xFF00_0000, [0, 0, 8, 8]),
            textured(atlas, [0, 0, 8, 8], 0xFFFF_FFFF, Filter::Point),
        ];
        let img = render(&gpu, (8, 8), &assets, &cmds);
        let p = img.get_pixel(4, 4).0;
        for c in &p[..3] {
            assert!(c.abs_diff(64) <= 1, "value {c}");
        }
    }

    #[test]
    fn d3d8_samples_at_pixel_corners() {
        let Some(gpu) = gpu() else { return };
        let mut assets = UiAssets::new();
        let tex = assets.insert_image(
            "data/g.tga",
            RgbaImage::from_fn(2, 1, |x, _| {
                let v = if x == 0 { 0 } else { 255 };
                Rgba([v, v, v, 255])
            }),
        );
        let cmds = [textured(tex, [0, 0, 4, 1], 0xFFFF_FFFF, Filter::Linear)];
        let img = render(&gpu, (4, 1), &assets, &cmds);
        let row: Vec<u8> = (0..4).map(|x| img.get_pixel(x, 0).0[0]).collect();
        assert_eq!(row[0], 0, "{row:?}");
        assert_eq!(row[1], 0, "{row:?}");
        assert!(row[2].abs_diff(127) <= 1, "{row:?}");
        assert_eq!(row[3], 255, "{row:?}");
    }

    /// Video quad (deviation): with the UV from `nml_ui::video::video_uv`, LINEAR at 1:1 hits the texel centers, the
    /// image stays texel-exact (without the shift, LINEAR averages 2 x 2 neighbors each, see above).
    // QUIRK O-66: deviation
    #[test]
    fn video_uv_keeps_texels_at_one_to_one() {
        let Some(gpu) = gpu() else { return };
        let mut assets = UiAssets::new();
        let texels = [0u8, 255, 40, 200];
        let tex = assets.insert_image(
            "video:frame",
            RgbaImage::from_fn(4, 2, |x, y| {
                let v = texels[x as usize] ^ (y as u8 * 0x0F);
                Rgba([v, 255 - v, v / 2, 255])
            }),
        );
        let dst = [0, 0, 4, 2];
        let cmd = DrawCmd::Quad {
            texture: Some(tex),
            dst,
            uv: nml_ui::video::video_uv(dst),
            color: 0xFFFF_FFFF,
            filter: Filter::Linear,
            blend: Blend::Alpha,
        };
        let img = render(&gpu, (4, 2), &assets, &[cmd]);
        for y in 0..2 {
            for x in 0..4 {
                let v = texels[x as usize] ^ (y as u8 * 0x0F);
                assert_eq!(
                    img.get_pixel(x, y).0,
                    [v, 255 - v, v / 2, 255],
                    "({x}, {y})"
                );
            }
        }
    }

    /// Like [`render`], but with a drawer that persists between calls (texture cache).
    fn render_with(
        gpu: &(wgpu::Device, wgpu::Queue),
        ui: &mut UiRenderer,
        size: (u32, u32),
        assets: &UiAssets,
        cmds: &[DrawCmd],
    ) -> RgbaImage {
        let (device, queue) = gpu;
        render_to_image(device, queue, size, CAPTURE_FORMAT, |encoder, view| {
            ui.draw(device, queue, encoder, view, size, assets, cmds);
        })
        .unwrap()
    }

    #[test]
    fn replaced_image_is_uploaded_again() {
        let Some(gpu) = gpu() else { return };
        let mut ui = UiRenderer::new(&gpu.0, CAPTURE_FORMAT);
        let mut assets = UiAssets::new();
        let solid =
            |w, h, rgb: [u8; 3]| RgbaImage::from_pixel(w, h, Rgba([rgb[0], rgb[1], rgb[2], 255]));
        let id = assets.insert_image("video:frame", solid(4, 4, [255, 0, 0]));
        let cmds = [textured(id, [0, 0, 8, 8], 0xFFFF_FFFF, Filter::Point)];

        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &cmds);
        assert_eq!(img.get_pixel(4, 4).0, [255, 0, 0, 255], "first image red");

        // Same size: the existing texture is written to.
        assets.insert_image("video:frame", solid(4, 4, [0, 255, 0]));
        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &cmds);
        assert_eq!(img.get_pixel(4, 4).0, [0, 255, 0, 255], "replaced: green");

        // Continue without change: unchanged green.
        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &cmds);
        assert_eq!(img.get_pixel(4, 4).0, [0, 255, 0, 255]);

        // Size change 4 x 4 -> 8 x 2: new texture, new bind group (point and linear filter).
        assets.insert_image("video:frame", solid(8, 2, [0, 0, 255]));
        let linear = [textured(id, [0, 0, 8, 8], 0xFFFF_FFFF, Filter::Linear)];
        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &cmds);
        assert_eq!(img.get_pixel(4, 4).0, [0, 0, 255, 255], "size 8 x 2: blue");
        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &linear);
        assert_eq!(img.get_pixel(4, 4).0, [0, 0, 255, 255]);

        // And back to the same size 8 x 2 with new content.
        assets.insert_image("video:frame", solid(8, 2, [255, 255, 0]));
        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &linear);
        assert_eq!(img.get_pixel(4, 4).0, [255, 255, 0, 255]);
        let img = render_with(&gpu, &mut ui, (8, 8), &assets, &cmds);
        assert_eq!(img.get_pixel(4, 4).0, [255, 255, 0, 255]);
    }

    #[test]
    fn argb_maps_to_rgba_floats() {
        assert_eq!(argb_to_rgba(0x80FF_0000), [1.0, 0.0, 0.0, 128.0 / 255.0]);
        assert_eq!(argb_to_rgba(0x0000_00FF), [0.0, 0.0, 1.0, 0.0]);
    }

    // QUIRK O-194: kept
    #[test]
    fn batches_merge_only_adjacent_equal_keys() {
        let a = TextureId(0);
        let b = TextureId(1);
        let q = |t: Option<TextureId>, f: Filter| DrawCmd::Quad {
            texture: t,
            dst: [0, 0, 1, 1],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: 0xFFFF_FFFF,
            filter: f,
            blend: Blend::Alpha,
        };
        let cmds = [
            q(Some(a), Filter::Point),
            q(Some(a), Filter::Point),
            q(Some(a), Filter::Linear),
            q(Some(b), Filter::Linear),
            q(Some(a), Filter::Point),
            q(None, Filter::Point),
        ];
        let ranges: Vec<Range<usize>> = batches(&cmds).into_iter().map(|(_, r)| r).collect();
        assert_eq!(ranges, [0..2, 2..3, 3..4, 4..5, 5..6]);
        assert!(batches(&[]).is_empty());
        // The blend kind separates batches of the same texture and the same filter.
        let add = additive(q(Some(a), Filter::Point));
        let keys: Vec<BatchKey> = batches(&[q(Some(a), Filter::Point), add, add])
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, [((Some(a), false), false), ((Some(a), false), true)]);
    }

    #[test]
    fn later_commands_cover_earlier_ones() {
        let Some(gpu) = gpu() else { return };
        let assets = UiAssets::new();
        let cmds = [
            fill(0xFFFF_0000, [0, 0, 8, 8]),
            fill(0xFF00_FF00, [2, 2, 6, 6]),
        ];
        let img = render(&gpu, (8, 8), &assets, &cmds);
        assert_eq!(img.get_pixel(4, 4).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(1, 1).0, [255, 0, 0, 255]);
    }

    #[test]
    fn no_commands_leave_the_target_untouched() {
        let Some(gpu) = gpu() else { return };
        let img = render(&gpu, (4, 4), &UiAssets::new(), &[]);
        assert!(img.pixels().all(|p| p.0 == [0, 0, 0, 0]));
    }
}
