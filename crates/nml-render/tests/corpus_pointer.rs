//! Drawing pass of the 3D mouse pointer (`nml_render::pointer`, `Cursor3D_Render` `0x004CCC50`) on the
//! real `mouse.rdo`: outlines against the extent table re/pointer-3d MZ.5, tint
//! and opacity (re/pointer-3d MZ.6), own depth per image size (re/pointer-3d MZ.7). The pose comes directly from
//! `Playback` (animation = pointer ID, start at 0 ms). Skipped without installation or
//! GPU.

use std::sync::Arc;

use glam::Vec3;
use image::RgbaImage;
use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_render::pointer::{
    PointerPiece, PointerRenderer, PointerScene, pointer_origin, pointer_projection, pointer_view,
};
use nml_render::{BackendChoice, CAPTURE_FORMAT, headless_device, render_to_image};
use nml_world::anim::Playback;
use nml_world::assets::{ModelAsset, ModelCache};

const MOUSE: &str = "data/r3d/misc/mousecursor/mouse.rdo";
const MOUSE_AT: (i32, i32) = (300, 300);
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// Tints of the pointer choice.
const WHITE: [f32; 4] = [1.0; 4];
const BUILD_VALID: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const TARGET_VALID: [f32; 4] = [0.5, 1.0, 0.5, 0.8];
const SMALL: (u32, u32) = (1024, 768);
const WIDE: (u32, u32) = (1920, 1080);

struct Bench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vfs: Vfs,
    models: ModelCache,
    carrier: Arc<ModelAsset>,
    renderer: PointerRenderer,
}

impl Bench {
    fn new() -> Option<Self> {
        let Some(dir) = corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return None;
        };
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return None;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let mut models = ModelCache::new();
        let carrier = models.get(&vfs, MOUSE).expect("loading mouse.rdo");
        let renderer = PointerRenderer::new(&device, &queue, CAPTURE_FORMAT);
        Some(Self {
            device,
            queue,
            vfs,
            models,
            carrier,
            renderer,
        })
    }

    /// Pointer `id` at `e_ms` milliseconds of the loop with the mouse at `mouse`: carrier and the
    /// sub-objects of the running animation.
    fn scene_at(
        &mut self,
        id: usize,
        e_ms: f32,
        tint: [f32; 4],
        size: (u32, u32),
        mouse: (i32, i32),
    ) -> PointerScene {
        let (vfs, models) = (&self.vfs, &mut self.models);
        let carrier = self.carrier.clone();
        let playback = Playback::new(&carrier, |file| models.get(vfs, file), Some(id), 0.0);
        let mut pieces = vec![PointerPiece {
            pose: Some(playback.player().pose(&carrier, e_ms)),
            asset: carrier.clone(),
        }];
        pieces.extend(playback.active_subs(true).map(|sub| PointerPiece {
            asset: sub.asset.clone(),
            pose: Some(sub.player.pose(&sub.asset, e_ms)),
        }));
        assert!(pieces.len() >= 2, "pointer {id}: no sub-object");
        PointerScene {
            pieces,
            origin: pointer_origin(mouse.0, mouse.1, size.0, size.1),
            tint,
            mouse,
            depth_zero: Vec::new(),
        }
    }

    /// Like [`Self::scene_at`] with the mouse at [`MOUSE_AT`].
    fn scene(&mut self, id: usize, e_ms: f32, tint: [f32; 4], size: (u32, u32)) -> PointerScene {
        self.scene_at(id, e_ms, tint, size, MOUSE_AT)
    }

    /// Draws `scene` onto a single-color background.
    fn render(&mut self, scene: &PointerScene, size: (u32, u32), ground: [u8; 4]) -> RgbaImage {
        let Self {
            device,
            queue,
            vfs,
            renderer,
            ..
        } = self;
        render_to_image(device, queue, size, CAPTURE_FORMAT, |encoder, view| {
            let colour = |c: u8| f64::from(c) / 255.0;
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Background"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: colour(ground[0]),
                            g: colour(ground[1]),
                            b: colour(ground[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.draw(device, queue, encoder, view, size, vfs, scene);
        })
        .expect("reading image")
    }
}

/// Outline of the pixels that do not show the background: edges (left, top, right, bottom) in
/// pixels (right/bottom = last pixel + 1).
fn outline(image: &RgbaImage, ground: [u8; 4]) -> Option<[f32; 4]> {
    let mut edges: Option<[u32; 4]> = None;
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel.0 == ground {
            continue;
        }
        let e = edges.get_or_insert([x, y, x + 1, y + 1]);
        *e = [e[0].min(x), e[1].min(y), e[2].max(x + 1), e[3].max(y + 1)];
    }
    edges.map(|e| e.map(|v| v as f32))
}

fn union(a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> Option<[f32; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Scale in pixels per model unit (horizontal, vertical) at the origin, from camera and
/// projection of the pointer.
fn scale(size: (u32, u32)) -> (f32, f32) {
    let pixel = |p: Vec3| {
        let clip = pointer_projection() * pointer_view() * p.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        (
            (x + 1.0) / 2.0 * size.0 as f32,
            (1.0 - y) / 2.0 * size.1 as f32,
        )
    };
    let origin = pointer_origin(MOUSE_AT.0, MOUSE_AT.1, size.0, size.1);
    let at = pixel(origin);
    let right = pixel(origin + Vec3::X);
    let up = pixel(origin + Vec3::Y);
    (right.0 - at.0, at.1 - up.1)
}

/// Expected edges for the extent `x` (left ... right) and `y` (bottom ... top, y up)
/// in model units around the mouse point `mouse + 0.5`, snapped to the pixel grid: the boundaries of the
/// pixels whose center lies inside the extent (wgpu raster rule like D3D8: a pixel
/// belongs to the triangle if its center is inside). Without this step up to half a pixel more
/// is missing at pointed ends: the sword tip of pointer 6 reaches 534.08 at 1024 x 768, the
/// last pixel with its center in the triangle is 532 (edge 533).
fn expected(size: (u32, u32), mouse: (i32, i32), x: (f32, f32), y: (f32, f32)) -> [f32; 4] {
    let (sx, sy) = scale(size);
    let (cx, cy) = (mouse.0 as f32 + 0.5, mouse.1 as f32 + 0.5);
    let first = |v: f32| (v - 0.5).ceil();
    let past = |v: f32| (v - 0.5).floor() + 1.0;
    [
        first(cx + x.0 * sx),
        first(cy - y.1 * sy),
        past(cx + x.1 * sx),
        past(cy - y.0 * sy),
    ]
}

fn assert_edges(got: [f32; 4], want: [f32; 4], what: &str) {
    for (g, w) in got.iter().zip(&want) {
        assert!(
            (g - w).abs() <= 1.0,
            "{what}: outline {got:?}, expected {want:?} (+/- 1 px)"
        );
    }
}

/// re/pointer-3d MZ.11: pointer 7 at e = 250 ms and 1024 x 768 (scale 0.5090 / 0.3818 px per unit) reaches
/// x -0.86 ... 32.97 and y -0.95 ... 39.02 units (downward) around the mouse point.
#[test]
fn pointer_seven_outline_at_250_ms() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    let (sx, sy) = scale(SMALL);
    assert!(
        (sx - 0.5090).abs() < 1e-4 && (sy - 0.3818).abs() < 1e-4,
        "scale {sx} / {sy}"
    );
    let scene = bench.scene(7, 250.0, WHITE, SMALL);
    let image = bench.render(&scene, SMALL, MAGENTA);
    let got = outline(&image, MAGENTA).expect("pointer 7 visible");
    let want = expected(SMALL, MOUSE_AT, (-0.86, 32.97), (-39.02, 0.95));
    eprintln!("pointer 7 at 250 ms: {got:?}, expected {want:?}");
    assert_edges(got, want, "pointer 7 at 250 ms");
}

/// re/pointer-3d MZ.5: union of the outlines over the loop (e = 0, 50, ..., 950 ms) against the
/// extent table at 1024 x 768 and 1920 x 1080 (scale 0.9544 / 0.5369).
///
/// The mouse is in the center of the image here: the table projects the
/// vertices as if the origin lay on the view axis. Away from the center the
/// perspective shifts the parts with their own depth (z -62 ... +44 units) against each other - with the mouse
/// at (300, 300) and 1920 x 1080 by up to 0.8 px (pointer 7, right edge 334.7 instead of 335.5;
/// the original shows it the same way, to the right of the center up to mx + 35, re/pointer-3d MZ.13).
// QUIRK O-458: kept
#[test]
fn pointer_outlines_match_the_extent_table() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    let (sx, sy) = scale(WIDE);
    assert!(
        (sx - 0.9544).abs() < 1e-4 && (sy - 0.5369).abs() < 1e-4,
        "scale {sx} / {sy}"
    );
    // ID, x left ... right, y bottom ... top (model units, table re/pointer-3d MZ.5), loop D in ms.
    let table = [
        (6, (-19.5, 42.4), (-62.5, 22.0), 1000),
        (7, (-0.9, 36.7), (-39.0, 0.9), 1000),
        (9, (-28.4, 30.1), (-30.3, 28.4), 1000),
    ];
    for (id, x, y, loop_ms) in table {
        for size in [SMALL, WIDE] {
            let mouse = (size.0 as i32 / 2, size.1 as i32 / 2);
            let mut all = None;
            for e in (0..loop_ms).step_by(50) {
                let scene = bench.scene_at(id, e as f32, WHITE, size, mouse);
                let image = bench.render(&scene, size, MAGENTA);
                all = union(all, outline(&image, MAGENTA));
            }
            let got = all.expect("pointer visible");
            let want = expected(size, mouse, x, y);
            eprintln!("pointer {id} at {size:?}: {got:?}, expected {want:?}");
            assert_edges(got, want, &format!("pointer {id} at {size:?}"));
        }
    }
}

/// re/pointer-3d MZ.6: the valid build cursor (0, 1, 0, 1) replaces the material color; only the
/// green channel of the texture remains, opaque.
// QUIRK O-454: kept
#[test]
fn build_tint_keeps_only_green() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    let scene = bench.scene(7, 250.0, BUILD_VALID, SMALL);
    let image = bench.render(&scene, SMALL, MAGENTA);
    let drawn: Vec<_> = image.pixels().filter(|p| p.0 != MAGENTA).collect();
    assert!(drawn.len() > 50, "only {} pixels drawn", drawn.len());
    for p in &drawn {
        assert_eq!((p.0[0], p.0[2]), (0, 0), "pixel {:?}", p.0);
    }
    assert!(drawn.iter().any(|p| p.0[1] > 64), "no green");
}

/// re/pointer-3d MZ.6, O-456: in targeting mode (0.5; 1; 0.5; 0.8) the pointer blends with opacity 0.8 without
/// Z writing; the same pointer on magenta and on black differs per drawn
/// pixel by 0.2 * magenta = (51, 0, 51). Where front faces of the pointer overlap, it blends
/// twice (O-456): there 0.2^2 * magenta = (10, 0, 10) remains.
// QUIRK O-455, O-456: kept
#[test]
fn target_tint_blends_with_alpha_0_8() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    // Pointer 9 (target disc) as in targeting mode.
    let scene = bench.scene(9, 0.0, TARGET_VALID, SMALL);
    let on_magenta = bench.render(&scene, SMALL, MAGENTA);
    let on_black = bench.render(&scene, SMALL, BLACK);
    let (mut single, mut double) = (0, 0);
    for (m, b) in on_magenta.pixels().zip(on_black.pixels()) {
        if m.0 == MAGENTA {
            continue;
        }
        let diff = [0, 1, 2].map(|c| i32::from(m.0[c]) - i32::from(b.0[c]));
        let near = |want: [i32; 3]| diff.iter().zip(want).all(|(d, w)| (d - w).abs() <= 2);
        if near([51, 0, 51]) {
            single += 1;
        } else if near([10, 0, 10]) {
            double += 1;
        } else {
            panic!(
                "pixel {:?} on magenta, {:?} on black: difference {diff:?}",
                m.0, b.0
            );
        }
    }
    eprintln!("targeting mode: {single} pixels blended once, {double} blended twice");
    assert!(single > 50, "only {single} pixels blended once");
    assert!(double > 0, "no pixels blended twice (O-456)");
    assert!(
        double * 4 < single,
        "{double} blended twice with {single} blended once"
    );
}

/// re/pointer-3d MZ.7: the pointer depth follows the image size (also 1 x 1 and very wide) without validation error.
#[test]
fn depth_follows_the_frame_size() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    for size in [SMALL, (1, 1), (2560, 1080)] {
        let scope = bench.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let scene = bench.scene(7, 0.0, WHITE, size);
        let image = bench.render(&scene, size, MAGENTA);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "image {size:?}: {error:?}");
        if size.0 > 1 {
            assert!(
                outline(&image, MAGENTA).is_some(),
                "image {size:?}: no pointer"
            );
        }
    }
}

/// Cleared square `Renderer_ClearZRects` (`0x005715F0`) around the mouse: x/y `m - 32 ... m + 31`.
fn in_square(x: u32, y: u32, mouse: (i32, i32)) -> bool {
    let (x, y) = (x as i32, y as i32);
    (mouse.0 - 32..mouse.0 + 32).contains(&x) && (mouse.1 - 32..mouse.1 + 32).contains(&y)
}

/// re/pointer-3d MZ.7, O-457: if there is depth 0 everywhere under the pointer (the minimap in state block `Map`),
/// only the cleared 64 x 64 square around the mouse remains of the pointer; inside it it is unchanged.
/// Arrow 7 at 1920 x 1080 reaches mx + 35 without clipping and ends at mx + 31 with clipping
/// (reference run re/pointer-3d MZ.13). Rectangles over the image edge are clamped, empty ones skipped.
// QUIRK O-457: kept
#[test]
fn minimap_clips_outside_the_square() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    let mouse = MOUSE_AT;
    let mut beyond = 0;
    for e in (0..1000).step_by(125) {
        let free = bench.scene_at(7, e as f32, WHITE, WIDE, mouse);
        let mut clipped = free.clone();
        clipped.depth_zero = vec![[-50, -50, 4000, 4000], [10, 10, 5, 5], [7, 7, 7, 9]];
        let scope = bench.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let without = bench.render(&free, WIDE, MAGENTA);
        let with = bench.render(&clipped, WIDE, MAGENTA);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "e = {e} ms: {error:?}");
        let mut inside = 0;
        for ((x, y, a), b) in without.enumerate_pixels().zip(with.pixels()) {
            if in_square(x, y, mouse) {
                assert_eq!(
                    a.0, b.0,
                    "e = {e} ms: pixel ({x}, {y}) changed inside the square"
                );
                inside += usize::from(a.0 != MAGENTA);
            } else {
                assert_eq!(b.0, MAGENTA, "e = {e} ms: pixel ({x}, {y}) visible outside");
                beyond += usize::from(a.0 != MAGENTA);
            }
        }
        assert!(
            inside > 50,
            "e = {e} ms: only {inside} pixels in the square"
        );
        let right = outline(&with, MAGENTA).expect("pointer visible")[2];
        assert!(
            right <= (mouse.0 + 32) as f32,
            "e = {e} ms: right edge {right}"
        );
    }
    assert!(
        beyond > 0,
        "arrow 7 never extends beyond the square at 1920 x 1080"
    );
}

/// re/pointer-3d MZ.7: without `depth_zero` (or with depth 0 only away from the pointer) nothing is cut off: arrow 7
/// reaches beyond mx + 32 at 1920 x 1080, the image is the same with and without a distant rectangle.
// QUIRK O-457: kept
#[test]
fn no_depth_zero_means_no_clipping() {
    let Some(mut bench) = Bench::new() else {
        return;
    };
    let mouse = MOUSE_AT;
    let mut widest = 0.0f32;
    for e in (0..1000).step_by(125) {
        let free = bench.scene_at(7, e as f32, WHITE, WIDE, mouse);
        assert!(free.depth_zero.is_empty());
        let mut far = free.clone();
        // The minimap at the bottom right, far from the mouse.
        far.depth_zero = vec![[1744, 904, 1920, 1080]];
        let without = bench.render(&free, WIDE, MAGENTA);
        let with_far = bench.render(&far, WIDE, MAGENTA);
        assert!(
            without.pixels().zip(with_far.pixels()).all(|(a, b)| a == b),
            "e = {e} ms: distant rectangle changes the pointer"
        );
        widest = widest.max(outline(&without, MAGENTA).expect("pointer visible")[2]);
    }
    assert!(
        widest > (mouse.0 + 32) as f32,
        "right edge at most {widest}"
    );
}
