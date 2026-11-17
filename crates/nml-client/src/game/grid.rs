//! Grid and screen layer of the game modules: fixed-point grid position of a world point, section of the path
//! overlay and projection of a world point to pixels. Independent of other modules of the viewer.

use glam::{Mat4, Vec3};
use nml_sim::fx::Fx;

/// Half the edge length of the displayed section in tiles.
pub const OVERLAY_RADIUS: i32 = 24;

/// Fixed-point grid position of a world point (`fx(x) / cell` as in `ApplyPlacement`).
pub fn world_to_grid(p: Vec3, cell: [i32; 3]) -> (i32, i32) {
    (Fx::from_f32(p.x).0 / cell[0], Fx::from_f32(p.z).0 / cell[2])
}

/// Pixel of a world point in physical pixels (top left is 0, 0); `None` behind the camera.
pub fn project(view_proj: Mat4, point: Vec3, size: (u32, u32)) -> Option<[f32; 2]> {
    let clip = view_proj * point.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some([
        (ndc.x + 1.0) * 0.5 * size.0 as f32,
        (1.0 - ndc.y) * 0.5 * size.1 as f32,
    ])
}
