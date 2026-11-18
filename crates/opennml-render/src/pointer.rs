//! 3D mouse pointer (`HardwareCursor 0`) like `Cursor3D_Render` (`0x004CCC50`),
//! re/pointer-3d MZ.5 - MZ.7: own UI camera with aspect ratio 1 over the whole frame, directional
//! light of triple strength from the front without ambient light, tint instead of material color,
//! own depth; the pass draws onto the finished frame (color stays, depth is cleared).
//!
//! The pure functions (camera, origin, look) need no GPU; [`PointerRenderer`] draws a
//! [`PointerScene`] with its own [`Shared`], its own [`ObjectRenderer`] and its own
//! [`ModelLibrary`].

use std::sync::Arc;

use glam::{Affine3A, Mat4, Vec3, Vec4};
use opennml_rda::Vfs;
use opennml_world::anim::Pose;
use opennml_world::assets::ModelAsset;
use opennml_world::camera::look_at_lh;

use crate::objects::{Instance, InstanceLook, MeshFilter, ModelLibrary, ObjectRenderer};
use crate::shared::{DEPTH_FORMAT, FrameUniforms, Shared};
use crate::texture::white;

/// Scale of the origin `20100 · tan 0.05` (= 1005.8383...): half frame width or height in model
/// units on the plane z = 100, 20100 units in front of the eye (re/pointer-3d MZ.5).
pub const POINTER_ORIGIN_SCALE: f32 = 1005.838;

/// `_11 = _22 = 1 / tan(fov/2)` with fov 0.1 (`1 / tan 0.05` = 19.9833305...); as a number, so
/// that the run-time path computes no trigonometric function.
const POINTER_FOCAL: f32 = 19.983_33;
/// Eye of the UI camera (`FUN_00572F10`): (0, 0, -20000), target 0, up (0, 1, 0).
const POINTER_EYE_Z: f32 = -20000.0;
/// World position of the model origin in z (`Cursor3D_Render` `0x004CCC50`).
const POINTER_Z: f32 = 100.0;
/// `_33 = zf / (zf − zn)` with `zn = 10100`, `zf = −9900` (`0x0042BF14…0x0042C025`).
const POINTER_Q: f32 = 0.495;
/// `_43 = −Q · zn`.
const POINTER_Q_NEAR: f32 = -4999.5;

/// View of the UI camera (`FUN_00572F10`, `D3DXMatrixLookAtLH`): eye (0, 0, -20000),
/// target 0, up (0, 1, 0) - a pure translation by +20000 in z.
pub fn pointer_view() -> Mat4 {
    look_at_lh(Vec3::new(0.0, 0.0, POINTER_EYE_Z), Vec3::ZERO, Vec3::Y)
}

/// Projection of the UI camera (`FUN_005727E0`, `PerspectiveFovLH` with fov 0.1,
/// aspect ratio 1, `zn = 10100`, `zf = −9900`): `_11 = _22 = 19.98333`, `_33 = 0.495`,
/// `_34 = 1`, `_43 = −4999.5` (row-vector notation like
/// [`opennml_world::camera::game_projection`]). Aspect ratio 1 over the whole frame: stretched
/// horizontally by `W/H` on any non-square screen (re/pointer-3d MZ.5).
pub fn pointer_projection() -> Mat4 {
    Mat4::from_cols(
        Vec4::new(POINTER_FOCAL, 0.0, 0.0, 0.0),
        Vec4::new(0.0, POINTER_FOCAL, 0.0, 0.0),
        Vec4::new(0.0, 0.0, POINTER_Q, 1.0),
        Vec4::new(0.0, 0.0, POINTER_Q_NEAR, 0.0),
    )
}

/// World position of the model origin for the mouse `(mx, my)` in a frame of `width × height`
/// (`Cursor3D_Render` `0x004CCC50`): `P = (K · (2·mx/W − 1), K · (1 − 2·my/H), 100)` with
/// `K = 20100 · tan 0.05`. The original thus hits the point `(mx, my)`, which in the D3D8 raster
/// is the pixel center; in the wgpu raster the center lies at `mx + 0.5`, hence the half
/// pixel. No hotspot (O-297).
pub fn pointer_origin(mx: i32, my: i32, width: u32, height: u32) -> Vec3 {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    Vec3::new(
        POINTER_ORIGIN_SCALE * (2.0 * (mx as f32 + 0.5) / w - 1.0),
        POINTER_ORIGIN_SCALE * (1.0 - 2.0 * (my as f32 + 0.5) / h),
        POINTER_Z,
    )
}

/// Look of the pointer for the tint `(r, g, b, a)` (re/pointer-3d MZ.6): white like
/// `Material_MakeDefaultWhite` (`0x00571A00`) or tinted like `Material_MakeTinted` (`0x005832F0`):
/// diffuse = ambient = `(r, g, b)` for all groups, the `.rdo` color never applies (O-454). With `a < 1`
/// (targeting mode 0.8) all groups are in the transparent pass without Z writing, unsorted
/// (`ModelInstance_DrawOpaqueGroup` `0x0058A4F0` aborts at `alpha ≠ 1`, O-456). Fog off (`FOGENABLE 0`),
/// alpha test like the state block `Objects`.
pub fn pointer_look(tint: [f32; 4]) -> InstanceLook {
    let [r, g, b, a] = tint;
    InstanceLook {
        tint: Some([r, g, b]),
        alpha: (a < 1.0).then_some(a),
        fog: false,
        alpha_test: true,
        ..InstanceLook::default()
    }
}

/// Frame uniforms of the pointer pass: camera like [`pointer_view`]/[`pointer_projection`],
/// light 0 directional from (0, 0, 1) with diffuse (3, 3, 3, 3), ambient and specular 0
/// (`Cursor3D_Render` `0x004CCD86…0x004CCE7F`, `Renderer_SetDirectionalLight` `0x00571300`;
/// the game never sets `D3DRS_AMBIENT`). `light_dir` is the direction **toward** the light, so
/// (0, 0, -1). This gives `color = texture × clamp(3 · max(0, −N_z) · tint)` (O-454, O-455);
/// the normals enter unnormalized as everywhere (O-458). Without fog.
pub fn pointer_uniforms() -> FrameUniforms {
    let mut uniforms = FrameUniforms::new(pointer_view(), pointer_projection());
    uniforms.light_dir = [0.0, 0.0, -1.0, 0.0];
    uniforms.light_diffuse = [3.0; 4];
    uniforms.light_ambient = [0.0; 4];
    uniforms
}

/// A piece of the pointer: the carrier `mouse.rdo` or a sub-object of the running animation
/// (`mouse <name>.rdo`) with its pose (`None` = bind pose).
#[derive(Debug, Clone)]
pub struct PointerPiece {
    pub asset: Arc<ModelAsset>,
    pub pose: Option<Pose>,
}

/// What the pointer pass draws in a frame.
#[derive(Debug, Clone)]
pub struct PointerScene {
    /// Carrier and sub-objects; all with the world matrix `Translation(origin)` (the sub-objects
    /// take over the instance matrix of the carrier, `0x0058B530`).
    pub pieces: Vec<PointerPiece>,
    /// Model origin in the world of the pointer camera ([`pointer_origin`]).
    pub origin: Vec3,
    /// Tint `(r, g, b, a)`.
    pub tint: [f32; 4],
    /// Mouse position in pixels.
    pub mouse: (i32, i32),
    /// Rectangles `(x0, y0, x1, y1)` in pixels (right and bottom exclusive) in which depth 0
    /// stands in front of the pointer: the minimap (O-457, `GameSession::minimap_depth_rects`).
    /// Empty = nowhere, the pointer is not clipped.
    pub depth_zero: Vec<[i32; 4]>,
}

/// Half edge of the square that `Renderer_ClearZRects` (`0x005715F0`, from
/// `0x004CCC74…0x004CCCA8`) clears around the mouse to depth 1.0: `(mx − 32, my − 32, mx + 32,
/// my + 32)`, fixed in pixels (re/pointer-3d MZ.7).
pub const POINTER_CLEAR_HALF: i32 = 32;

/// Corners of the depth pre-pass (re/pointer-3d MZ.7, O-457) as a triangle list in NDC including
/// depth: first every rectangle from `depth_zero` with depth 0 (what the minimap leaves behind
/// in the state block `Map`), then the square [`POINTER_CLEAR_HALF`] around `mouse` with depth
/// 1.0 (`Renderer_ClearZRects` `0x005715F0`). Rectangles are clamped to the frame `size`, empty
/// ones skipped. If nothing remains of `depth_zero`, the list is empty: then 1.0 stands
/// everywhere as after clearing, and the square would change nothing.
pub fn depth_pass_vertices(
    depth_zero: &[[i32; 4]],
    mouse: (i32, i32),
    size: (u32, u32),
) -> Vec<[f32; 3]> {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let (wi, hi) = (
        i32::try_from(w).unwrap_or(i32::MAX),
        i32::try_from(h).unwrap_or(i32::MAX),
    );
    let clamp = |[x0, y0, x1, y1]: [i32; 4]| {
        let r = [
            x0.clamp(0, wi),
            y0.clamp(0, hi),
            x1.clamp(0, wi),
            y1.clamp(0, hi),
        ];
        (r[0] < r[2] && r[1] < r[3]).then_some(r)
    };
    let (wf, hf) = (w as f32, h as f32);
    let mut out = Vec::new();
    let mut push = |[x0, y0, x1, y1]: [i32; 4], z: f32| {
        let nx = |x: i32| x as f32 / wf * 2.0 - 1.0;
        let ny = |y: i32| 1.0 - y as f32 / hf * 2.0;
        let (l, t, r, b) = (nx(x0), ny(y0), nx(x1), ny(y1));
        out.extend([
            [l, t, z],
            [r, t, z],
            [l, b, z],
            [r, t, z],
            [r, b, z],
            [l, b, z],
        ]);
    };
    let mut any = false;
    for rect in depth_zero.iter().copied().filter_map(clamp) {
        push(rect, 0.0);
        any = true;
    }
    if !any {
        return Vec::new();
    }
    let (mx, my) = mouse;
    let square = [
        mx.saturating_sub(POINTER_CLEAR_HALF),
        my.saturating_sub(POINTER_CLEAR_HALF),
        mx.saturating_add(POINTER_CLEAR_HALF),
        my.saturating_add(POINTER_CLEAR_HALF),
    ];
    if let Some(square) = clamp(square) {
        push(square, 1.0);
    }
    out
}

/// Pipeline and corner buffer of the depth pre-pass: comparison `Always`, Z writing, no color.
struct DepthPass {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    capacity: u64,
}

impl DepthPass {
    fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Pointer depth pre-pass"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/pointer_depth.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pointer depth pre-pass"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Pointer depth pre-pass"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_depth"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_depth"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let capacity = 64 * std::mem::size_of::<[f32; 3]>() as u64;
        let buffer = Self::buffer(device, capacity);
        Self {
            pipeline,
            buffer,
            capacity,
        }
    }

    fn buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Pointer depth pre-pass"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads `vertices` (the buffer grows as needed) and returns their count; `None` without corners.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[[f32; 3]],
    ) -> Option<u32> {
        if vertices.is_empty() {
            return None;
        }
        let bytes: &[u8] = bytemuck::cast_slice(vertices);
        let needed = bytes.len() as u64;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = Self::buffer(device, self.capacity);
        }
        queue.write_buffer(&self.buffer, 0, bytes);
        u32::try_from(vertices.len()).ok()
    }
}

/// Depth buffer of the pointer in frame size.
struct PointerDepth {
    size: (u32, u32),
    view: wgpu::TextureView,
}

/// Draw pass of the 3D pointer (`Cursor3D_Render` `0x004CCC50`) onto a finished frame.
pub struct PointerRenderer {
    shared: Shared,
    objects: ObjectRenderer,
    library: ModelLibrary,
    depth: Option<PointerDepth>,
    depth_pass: DepthPass,
}

impl PointerRenderer {
    /// Creates bindings, pipelines (state block `Objects`: light, Z test `LESSEQUAL`,
    /// Z writing, culling, alpha test) and the white fallback texture and writes the fixed
    /// frame uniforms ([`pointer_uniforms`]).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let shared = Shared::new(device);
        queue.write_buffer(
            &shared.frame_buffer,
            0,
            bytemuck::bytes_of(&pointer_uniforms()),
        );
        let objects = ObjectRenderer::new(device, &shared, color_format);
        let library = ModelLibrary::new(Arc::new(white(device, queue)));
        let depth_pass = DepthPass::new(device, color_format);
        Self {
            shared,
            objects,
            library,
            depth: None,
            depth_pass,
        }
    }

    /// Depth in frame size; recreated on a size change.
    fn ensure_depth(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.depth.as_ref().is_some_and(|d| d.size == size) {
            return;
        }
        let view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Pointer depth"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
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
        self.depth = Some(PointerDepth { size, view });
    }

    /// Draws `scene` onto `target` (size `size`, color format as in [`Self::new`]): color
    /// stays (`LoadOp::Load`), the own depth is cleared to 1.0 (re/pointer-3d MZ.7: the original
    /// clears only ±32 px around the mouse, `Renderer_ClearZRects` `0x005715F0`; above the world
    /// this has the same effect). With `scene.depth_zero` first the depth pre-pass
    /// ([`depth_pass_vertices`], O-457): depth 0 into the rectangles of the minimap, then 1.0
    /// into the square around the mouse, both with comparison `Always` and without color - above
    /// the minimap the pointer stays visible only in the square. Then opaque with Z test
    /// `LESSEQUAL` and Z writing, transparent without Z writing (pipelines of the
    /// [`ObjectRenderer`]).
    /// At most one call per `queue.submit`, because the draw uniforms go through
    /// `queue.write_buffer`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        vfs: &Vfs,
        scene: &PointerScene,
    ) {
        if scene.pieces.is_empty() {
            return;
        }
        // `Cursor3D_Render`: world = Translation(P) for the carrier; the sub-objects take over
        // its instance matrix (`ModelInstance_UpdateSubObjectMatrices` `0x0058B530`).
        let look = pointer_look(scene.tint);
        let world = Affine3A::from_translation(scene.origin);
        let instances = scene
            .pieces
            .iter()
            .map(|piece| Instance {
                model: self
                    .library
                    .model(device, queue, &self.shared, vfs, &piece.asset),
                world,
                pose: piece.pose.clone(),
                look,
                meshes: MeshFilter::All,
            })
            .collect();
        self.objects
            .set_instances(device, queue, &self.shared, instances);
        self.ensure_depth(device, (size.0.max(1), size.1.max(1)));
        let prepass = depth_pass_vertices(&scene.depth_zero, scene.mouse, size);
        let prepass = self.depth_pass.upload(device, queue, &prepass);
        let Some(depth) = &self.depth else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3D pointer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if let Some(count) = prepass {
            pass.set_pipeline(&self.depth_pass.pipeline);
            pass.set_vertex_buffer(0, self.depth_pass.buffer.slice(..));
            pass.draw(0..count, 0..1);
        }
        self.objects.draw(&mut pass, &self.shared);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixel (x to the right, y downward, wgpu raster) and depth of a world point.
    fn project(p: Vec3, width: u32, height: u32) -> (f32, f32, f32) {
        let clip = pointer_projection() * pointer_view() * p.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        (
            (ndc.x + 1.0) / 2.0 * width as f32,
            (1.0 - ndc.y) / 2.0 * height as f32,
            ndc.z,
        )
    }

    /// re/pointer-3d MZ.5/MZ.7: `PerspectiveFovLH` with fov 0.1, aspect ratio 1, `zn = 10100`,
    /// `zf = −9900` (`0x0042BF14…0x0042C025`); depth of the origin `0.495 − 4999.5/20100`.
    // QUIRK O-454: kept
    #[test]
    fn pointer_matrices_like_the_original() {
        let p = pointer_projection();
        // Row-vector notation: `_11`, `_22`, `_33`, `_34`, `_43`.
        let entries = [
            (p.x_axis.x, 19.98333),
            (p.y_axis.y, 19.98333),
            (p.z_axis.z, 0.495),
            (p.z_axis.w, 1.0),
            (p.w_axis.z, -4999.5),
        ];
        for (got, want) in entries {
            assert!((got - want).abs() < 1e-3, "entry {got}, expected {want}");
        }
        for (got, want) in [
            (p.x_axis.y, 0.0),
            (p.x_axis.z, 0.0),
            (p.x_axis.w, 0.0),
            (p.w_axis.w, 0.0),
        ] {
            assert_eq!(got, want);
        }
        // View: eye (0, 0, -20000), target 0, up (0, 1, 0) -> pure translation by +20000.
        let v = pointer_view();
        assert_eq!(
            v * Vec4::new(0.0, 0.0, 100.0, 1.0),
            Vec4::new(0.0, 0.0, 20100.0, 1.0)
        );
        let (_, _, depth) = project(Vec3::new(0.0, 0.0, 100.0), 1024, 768);
        assert!((depth - 0.2463).abs() < 1e-4, "depth of the origin {depth}");
        // re/pointer-3d MZ.7 gives 0.2458 ... 0.2467 rounded to four places (exactly 0.245835 ... 0.246701).
        for dz in [-35.0, 35.0] {
            let (_, _, depth) = project(Vec3::new(0.0, 0.0, 100.0 + dz), 1024, 768);
            let rounded = (depth * 1e4).round() / 1e4;
            assert!(
                (0.2458..=0.2467).contains(&rounded),
                "depth at {dz:+} units {depth}"
            );
        }
        // Light 0: directional from (0, 0, 1), so direction toward the light (0, 0, -1); diffuse 3, ambient 0.
        let u = pointer_uniforms();
        assert_eq!(u.light_dir, [0.0, 0.0, -1.0, 0.0]);
        assert_eq!(u.light_diffuse, [3.0; 4]);
        assert_eq!(u.light_ambient, [0.0; 4]);
        assert_eq!(u.fog[2], 0.0, "without fog");
        assert_eq!(
            u.view_proj,
            (pointer_projection() * pointer_view()).to_cols_array_2d()
        );
    }

    /// re/pointer-3d MZ.5: the origin hits the center of the mouse pixel (`(mx + 0.5, my + 0.5)` in the wgpu raster).
    #[test]
    fn origin_projects_to_the_pixel_centre() {
        // re/pointer-3d MZ.11: the original places (512, 384) at (0, 0, 100); with the half pixel
        // of the wgpu raster the origin lies off by K/W or K/H.
        let origin = pointer_origin(512, 384, 1024, 768);
        let half = Vec3::new(
            POINTER_ORIGIN_SCALE / 1024.0,
            -POINTER_ORIGIN_SCALE / 768.0,
            100.0,
        );
        assert!(origin.abs_diff_eq(half, 1e-4), "{origin}");
        for (mx, my, width, height) in [
            (512, 384, 1024, 768),
            (0, 0, 1920, 1080),
            (1919, 1079, 1920, 1080),
        ] {
            let origin = pointer_origin(mx, my, width, height);
            assert_eq!(origin.z, 100.0);
            let (x, y, _) = project(origin, width, height);
            let want = (mx as f32 + 0.5, my as f32 + 0.5);
            assert!(
                (x - want.0).abs() < 1e-3 && (y - want.1).abs() < 1e-3,
                "mouse ({mx}, {my}) at {width} x {height}: pixel ({x}, {y}), expected {want:?}"
            );
        }
    }

    /// O-457: the pre-pass first lays down the rectangles with depth 0, then the square +-32 px
    /// around the mouse with 1.0; clamped to the frame, empty rectangles are dropped, without rectangles no pre-pass.
    #[test]
    fn pointer_depth_pass_order_and_clamping() {
        assert!(depth_pass_vertices(&[], (10, 10), (100, 50)).is_empty());
        assert!(
            depth_pass_vertices(&[[5, 5, 5, 9], [200, 0, 300, 10]], (10, 10), (100, 50)).is_empty()
        );
        let v = depth_pass_vertices(&[[-10, 25, 50, 80], [3, 3, 1, 9]], (90, 10), (100, 50));
        assert_eq!(v.len(), 12);
        // Rectangle (0, 25) ... (50, 50): x -1 ... 0, y 0 ... -1, depth 0.
        let xs = |r: &[[f32; 3]]| {
            r.iter()
                .map(|p| p[0])
                .fold((f32::MAX, f32::MIN), |a, x| (a.0.min(x), a.1.max(x)))
        };
        let ys = |r: &[[f32; 3]]| {
            r.iter()
                .map(|p| p[1])
                .fold((f32::MAX, f32::MIN), |a, y| (a.0.min(y), a.1.max(y)))
        };
        assert_eq!(xs(&v[..6]), (-1.0, 0.0));
        assert_eq!(ys(&v[..6]), (-1.0, 0.0));
        assert!(v[..6].iter().all(|p| p[2] == 0.0));
        // Square (58, -22) ... (122, 42) -> (58, 0) ... (100, 42), depth 1.
        assert_eq!(xs(&v[6..]), (58.0 / 100.0 * 2.0 - 1.0, 1.0));
        assert_eq!(ys(&v[6..]), (1.0 - 42.0 / 50.0 * 2.0, 1.0));
        assert!(v[6..].iter().all(|p| p[2] == 1.0));
    }

    /// re/pointer-3d MZ.6: white and build cursor opaque, targeting mode (alpha 0.8) transparent
    /// only; the tint replaces the material color (`Material_MakeTinted` `0x005832F0`), fog off.
    // QUIRK O-454, O-455: kept
    #[test]
    fn pointer_look_by_tint() {
        let white = pointer_look([1.0; 4]);
        assert_eq!(white.tint, Some([1.0; 3]));
        assert_eq!(white.alpha, None);
        assert!(!white.fog);
        assert!(white.alpha_test);
        assert_eq!(white.cut, None);
        assert_eq!(white.brightness, 1.0);
        assert_eq!(white.player_color, None);
        assert!(!white.default_white);
        let target = pointer_look([0.5, 1.0, 0.5, 0.8]);
        assert_eq!(target.tint, Some([0.5, 1.0, 0.5]));
        assert_eq!(target.alpha, Some(0.8));
        assert!(!target.fog);
        let target_invalid = pointer_look([1.0, 0.5, 0.5, 0.8]);
        assert_eq!(target_invalid.alpha, Some(0.8));
        for build in [[0.0, 1.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]] {
            let look = pointer_look(build);
            assert_eq!(look.tint, Some([build[0], build[1], build[2]]));
            assert_eq!(look.alpha, None, "build cursor opaque");
            assert!(!look.fog);
        }
    }
}
