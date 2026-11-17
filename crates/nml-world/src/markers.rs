//! Ground markers of the selection as in the original (`ObjectRenderer_Draw` `0x00549AC0`, render bit
//! `0x8000000`, re/renderer ground markers section): ring decal on the terrain tiles
//! (`TerrainDecal_DrawSquare` `0x0054C1D0`), flat square at object height (`0x0054BFA0`), frame
//! over the model hull (`FUN_0054C2D0`) and the line rectangle of the types without a selection texture. Pure
//! CPU computation; drawing happens in `nml-render`.

use glam::{Affine3A, Vec3};

use crate::assets::ModelAsset;
use crate::math::{game_rotation_y, helper_node_matrix};
use crate::shadows::{DecalColor, DecalHull, DecalVertex, TileMask, push_decal, tile_corners};
use crate::terrain::Terrain;
use nml_formats::rdo::MeshKind;

/// Player colors `g_PlayerColors` (`0x0061ABB0`, 12 x D3DCOLOR, same as `0x006189D0` of the text colors)
/// as RGBA. `GameObject_SetPlayerColor` (`0x004680B0`) passes `table[GameObject+0x2D0]` to
/// `ObjectRenderer+0x98`; slot 8 (without player) and 9 (flags of nature) are light gray.
pub const PLAYER_MARKER_COLORS: [[u8; 4]; 12] = [
    [0xFF, 0xCE, 0x0D, 0xFF],
    [0x00, 0x73, 0xE6, 0xFF],
    [0x17, 0xA7, 0x10, 0xFF],
    [0xC4, 0x18, 0x00, 0xFF],
    [0x82, 0x01, 0xD8, 0xFF],
    [0xB9, 0x00, 0x66, 0xFF],
    [0xFF, 0x9F, 0x14, 0xFF],
    [0x00, 0xA8, 0xA8, 0xFF],
    [0xAC, 0xAC, 0xAC, 0xFF],
    [0xAC, 0xAC, 0xAC, 0xFF],
    [0xAC, 0xAC, 0xAC, 0xFF],
    [0xAC, 0xAC, 0xAC, 0xFF],
];

/// Marker color for the color index `GameObject+0x2D0`. Deviation: the original reads the table
/// unchecked; indices outside 0..11 (they occur via script action 7, E-244) yield light gray here.
pub fn marker_color(index: i32) -> [u8; 4] {
    usize::try_from(index)
        .ok()
        .and_then(|i| PLAYER_MARKER_COLORS.get(i))
        .copied()
        .unwrap_or(PLAYER_MARKER_COLORS[8])
}

/// [`marker_color`] as D3DCOLOR (`0xAARRGGBB`), as the tables `0x0061C268` (minimap dots,
/// `0x0047D4C0`) and `0x006189D0` (`[CR;COLOR %X]` of the tooltips) hold them; the common source of the
/// UI for player colors (minimap, pings, tooltips). Indices outside 0..11 yield light gray.
pub fn marker_argb(index: i32) -> u32 {
    let [r, g, b, a] = marker_color(index);
    u32::from_be_bytes([a, r, g, b])
}

/// Player color of the models for the color index `GameObject+0x2D0` (RGB from [`marker_color`]):
/// `GameObject_SetPlayerColor` (`0x004680B0`) passes the same table to the renderer, which uses it to
/// color the first `EF-MATERIAL` (re/renderer, objects pass section).
pub fn player_rgb(index: i32) -> [u8; 3] {
    let [r, g, b, _] = marker_color(index);
    [r, g, b]
}

/// Drawing kind of a marker (`ObjectRenderer_Draw`, `0x0054A454...0x0054A82D`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerShape {
    /// Without a selection texture (`ObjectRenderer+0x8C == 0`): [`line_rect`].
    Lines,
    /// `OBJ_SELECTION_USE_BBOX`: [`push_bounds_frame`].
    Frame,
    /// State `0x2000000` (on the wall): [`push_flat_square`].
    FlatSquare,
    /// Normal case: [`push_ring`].
    Ring,
}

/// Drawing kind by project data and state: first the texture, then `USE_BBOX` (the frame
/// does not know the wall state), then state `0x2000000`.
pub fn marker_shape(has_texture: bool, use_bbox: bool, on_wall: bool) -> MarkerShape {
    match (has_texture, use_bbox, on_wall) {
        (false, _, _) => MarkerShape::Lines,
        (true, true, _) => MarkerShape::Frame,
        (true, false, true) => MarkerShape::FlatSquare,
        (true, false, false) => MarkerShape::Ring,
    }
}

/// Ring of the selection (`TerrainDecal_DrawSquare` -> `TerrainDecal_Draw` `0x0054B600` with
/// `r1 = r2 = OBJ_SELECTION_RADIUS`): square `+-r` around `position`, rotated by `yaw`, on the
/// terrain tiles. The texture matrix of the original (rotation, offset `(0.5, 0.5)` with a square
/// hull) is the same as with the shadow decal ([`push_decal`] with a hull around the origin).
/// `TerrainDecal_Draw` only checks the map boundaries, not the visibility of the tile: `all` is
/// [`TileMask::all`]. With `raise_to_water` (`OBJ_ONWATER` or state `0x10000`) every
/// vertex is raised to the water height (`y <= WATER_LEVEL` -> `WATER_LEVEL`). Returns the number of
/// tiles.
#[allow(clippy::too_many_arguments)]
pub fn push_ring(
    terrain: &Terrain,
    all: &TileMask,
    radius: f32,
    position: Vec3,
    yaw: f32,
    color: [u8; 4],
    raise_to_water: bool,
    out: &mut Vec<DecalVertex>,
) -> usize {
    let start = out.len();
    let hull = DecalHull {
        center: Vec3::ZERO,
        half: Vec3::new(radius, 0.0, radius),
    };
    let tiles = push_decal(
        terrain,
        all,
        &hull,
        position,
        yaw,
        DecalColor::Fixed(color),
        None,
        out,
    );
    if raise_to_water {
        let water = terrain.params.water_level;
        for v in &mut out[start..] {
            if v.position[1] <= water {
                v.position[1] = water;
            }
        }
    }
    tiles
}

/// Flat square `+-r` at object height without rotation and without texture matrix (`0x0054BFA0`, state
/// `0x2000000` = on the wall): corners `(-r, -r)` uv (0, 0), `(-r, r)` (0, 1), `(r, -r)` (1, 0),
/// `(r, r)` (1, 1) as a strip, here two triangles like [`tile_corners`].
pub fn push_flat_square(position: Vec3, radius: f32, color: [u8; 4], out: &mut Vec<DecalVertex>) {
    let corner = |sx: f32, sz: f32| DecalVertex {
        position: (position + Vec3::new(sx * radius, 0.0, sz * radius)).to_array(),
        uv: [(sx + 1.0) / 2.0, (sz + 1.0) / 2.0],
        color,
    };
    let strip = [
        corner(-1.0, -1.0),
        corner(-1.0, 1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
    ];
    out.extend([strip[0], strip[1], strip[2], strip[1], strip[3], strip[2]]);
}

/// Name of the helper node whose tile rectangle determines the frame.
pub const BLOCKING_NODE: &str = "blocking01";

/// The 8 corners of the hull of the helper node `name` (mesh type 3005) of the main model, with the
/// rest matrix of the node including the node scale ([`helper_node_matrix`], `0x00578180`, D-367) and
/// then `world` (`ModelInstance_GetHelperNodeBox` `0x0058E790`); `None` without a node. In the shipped data
/// the only frame object with `blocking01` (GUID 1610, alarm post, scan over
/// `nml-wiese.prj`) has the scale `(1, 1, 1)`; so the frame looks the same.
pub fn helper_node_corners(asset: &ModelAsset, name: &str, world: &Affine3A) -> Option<[Vec3; 8]> {
    let mesh = asset
        .rdo
        .meshes
        .iter()
        .find(|m| m.kind == MeshKind::Helper && m.name.eq_ignore_ascii_case(name))?;
    let m = *world * helper_node_matrix(mesh);
    let (min, max) = (Vec3::from(mesh.bounds.min), Vec3::from(mesh.bounds.max));
    Some(std::array::from_fn(|i| {
        let pick = |bit: usize, k: usize| if i & bit == 0 { min[k] } else { max[k] };
        m.transform_point3(Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2)))
    }))
}

/// `GameObject_GetHelperBoxTileRect` (`0x00446C80`, re/occupancy 2.2): tile rectangle
/// `[x0, z0, x1, z1]` (inclusive) of the world corners `corners`, shrunk per side by `tile/8`;
/// bounds that are too narrow are swapped. First truncate, then divide as integers; for negative
/// bounds afterward -1 (also for exact multiples, quirk).
pub fn helper_box_tile_rect(corners: &[Vec3; 8], size_x: i32, size_z: i32) -> [i32; 4] {
    let (mut mn, mut mx) = (corners[0], corners[0]);
    for c in &corners[1..] {
        // `Vector3x8_GetMinMax` `0x004467C0`: strictly < or >.
        for k in 0..3 {
            if c[k] < mn[k] {
                mn[k] = c[k];
            }
            if c[k] > mx[k] {
                mx[k] = c[k];
            }
        }
    }
    let (sx8, sz8) = ((size_x / 8) as f32, (size_z / 8) as f32);
    let (mut x0, mut x1) = (mn.x + sx8, mx.x - sx8);
    let (mut z0, mut z1) = (mn.z + sz8, mx.z - sz8);
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
    }
    if z0 > z1 {
        std::mem::swap(&mut z0, &mut z1);
    }
    let tile = |v: f32, size: i32| (v as i32) / size - i32::from(v < 0.0);
    [
        tile(x0, size_x),
        tile(z0, size_z),
        tile(x1, size_x),
        tile(z1, size_z),
    ]
}

/// Texture coordinates of a frame tile (`FUN_0054C2D0`, `0x0054C823...0x0054CEDC`):
/// `(u0, u1, v0, v1)` with `(x, z)` -> `(u0, v0)`, `(x, z+1)` -> `(u0, v1)`, `(x+1, z)` -> `(u1, v0)`,
/// `(x+1, z+1)` -> `(u1, v1)`. Like a 9-slice: corners from the texture quarters (0..0.5 or 0.5..1),
/// edges from the middle strip 0.25..0.75 across; inner tiles are not drawn (`None`).
/// A row or column of tiles uses the whole texture across (0..1).
pub fn frame_uv(x: i64, z: i64, x0: i64, x1: i64, z0: i64, z1: i64) -> Option<[f32; 4]> {
    // Half axis: at the start 0..0.5, at the end 0.5..1, whole (one tile wide) 0..1.
    let edge = |v: i64, a: i64, b: i64| -> Option<[f32; 2]> {
        if a == b {
            Some([0.0, 1.0])
        } else if v == a {
            Some([0.0, 0.5])
        } else if v == b {
            Some([0.5, 1.0])
        } else {
            None
        }
    };
    const MIDDLE: [f32; 2] = [0.25, 0.75];
    let (u, v) = match (edge(x, x0, x1), edge(z, z0, z1)) {
        (Some(u), Some(v)) => (u, v),
        (Some(u), None) => (u, MIDDLE),
        (None, Some(v)) => (MIDDLE, v),
        (None, None) => return None,
    };
    Some([u[0], u[1], v[0], v[1]])
}

/// Frame over the model hull (`FUN_0054C2D0`, `OBJ_SELECTION_USE_BBOX`, buildings and
/// gold deposits): rest hull of the main model (`.rdo` header, `hull`) inset in x/z per side by
/// `tile/8` (integer), rotated by `yaw` at `position`; with the tile rectangle `helper`
/// of the node `blocking01` ([`helper_box_tile_rect`], world matrix only rotation and position) its
/// rectangle applies. Tile range `trunc(min * 1/tile) ... trunc(max * 1/tile)` (x outer, z
/// inner), tiles outside the map are dropped, as are the inner tiles ([`frame_uv`]); per
/// tile the four grid points with their height. With a helper node each tile is enlarged by a
/// quarter tile outward (`0x0054C973...0x0054CB15`). Returns the number of tiles.
#[allow(clippy::too_many_arguments)]
pub fn push_bounds_frame(
    terrain: &Terrain,
    hull: &DecalHull,
    helper: Option<[i32; 4]>,
    position: Vec3,
    yaw: f32,
    color: [u8; 4],
    out: &mut Vec<DecalVertex>,
) -> usize {
    let p = &terrain.params;
    let (size_x, size_z) = (p.size_x as i32, p.size_z as i32);
    let (sx, sz) = (size_x as f32, size_z as f32);
    let corners: [(f32, f32); 4] = match helper {
        Some([rx0, rz0, rx1, rz1]) => [
            (rx0 as f32 * sx, rz1 as f32 * sz),
            (rx1 as f32 * sx, rz1 as f32 * sz),
            (rx0 as f32 * sx, rz0 as f32 * sz),
            (rx1 as f32 * sx, rz0 as f32 * sz),
        ],
        None => {
            let (mut mn, mut mx) = (hull.center - hull.half, hull.center + hull.half);
            let (sx8, sz8) = ((size_x / 8) as f32, (size_z / 8) as f32);
            mn.x += sx8;
            mn.z += sz8;
            mx.x -= sx8;
            mx.z -= sz8;
            let rotation = game_rotation_y(yaw);
            [(mn.x, mn.z), (mx.x, mn.z), (mn.x, mx.z), (mx.x, mx.z)].map(|(x, z)| {
                let c = position + rotation * Vec3::new(x, 0.0, z);
                (c.x, c.z)
            })
        }
    };
    let min_x = corners.iter().fold(f32::MAX, |a, c| a.min(c.0));
    let max_x = corners.iter().fold(f32::MIN, |a, c| a.max(c.0));
    let min_z = corners.iter().fold(f32::MAX, |a, c| a.min(c.1));
    let max_z = corners.iter().fold(f32::MIN, |a, c| a.max(c.1));
    let (inv_x, inv_z) = (1.0 / sx, 1.0 / sz);
    let (x0, x1) = ((min_x * inv_x) as i64, (max_x * inv_x) as i64);
    let (z0, z1) = ((min_z * inv_z) as i64, (max_z * inv_z) as i64);
    let grow = helper.map(|_| (sx * 0.25, sz * 0.25));
    let (dim_x, dim_z) = (terrain.dim_x() as i64, terrain.dim_z() as i64);
    let mut count = 0;
    for x in x0.max(0)..=x1.min(dim_x - 1) {
        for z in z0.max(0)..=z1.min(dim_z - 1) {
            let Some([u0, u1, v0, v1]) = frame_uv(x, z, x0, x1, z0, z1) else {
                continue;
            };
            count += 1;
            for (gx, gz) in tile_corners(x as usize, z as usize) {
                let (right, far) = (gx > x as usize, gz > z as usize);
                let mut pos = terrain.vertex(gx, gz).position;
                if let Some((dx, dz)) = grow {
                    pos.x += if right { dx } else { -dx };
                    pos.z += if far { dz } else { -dz };
                }
                out.push(DecalVertex {
                    position: pos.to_array(),
                    uv: [if right { u1 } else { u0 }, if far { v1 } else { v0 }],
                    color,
                });
            }
        }
    }
    count
}

/// Line rectangle of the types without a selection texture (`ObjectRenderer_Draw`, `0x0054A4BE...0x0054A6F4`,
/// state `alpharect2`): corners `(min.x, min.z)`, `(max.x, min.z)`, `(max.x, max.z)`,
/// `(min.x, max.z)` at `y = min.y + 1` in object space with `min/max = center -/+ half extent`
/// (`ObjectRenderer_GetBoundsCenter(..., 0)`, `...HalfExtent(..., 0)`), brought into the world
/// with the object matrix `world`; indices 0-1, 1-2, 2-3, 3-0 as a line list (8 points).
pub fn line_rect(center: Vec3, half: Vec3, world: &Affine3A) -> [Vec3; 8] {
    let (mn, mx) = (center - half, center + half);
    let y = mn.y + 1.0;
    let c = [
        Vec3::new(mn.x, y, mn.z),
        Vec3::new(mx.x, y, mn.z),
        Vec3::new(mx.x, y, mx.z),
        Vec3::new(mn.x, y, mx.z),
    ]
    .map(|p| world.transform_point3(p));
    [c[0], c[1], c[1], c[2], c[2], c[3], c[3], c[0]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::tests::test_terrain;

    #[test]
    fn marker_argb_is_the_d3dcolor_table() {
        // `0x0061C268` (bytes read): 8 player colors, then gray.
        let table = [
            0xFFFF_CE0D,
            0xFF00_73E6,
            0xFF17_A710,
            0xFFC4_1800,
            0xFF82_01D8,
            0xFFB9_0066,
            0xFFFF_9F14,
            0xFF00_A8A8,
            0xFFAC_ACAC,
        ];
        for (i, &argb) in (0..).zip(&table) {
            assert_eq!(marker_argb(i), argb, "Index {i}");
        }
        assert_eq!(marker_argb(11), 0xFFAC_ACAC);
        assert_eq!(marker_argb(-1), 0xFFAC_ACAC);
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    #[test]
    fn colors_are_the_table_of_the_original_and_neutral_is_light_grey() {
        assert_eq!(marker_color(0), [255, 206, 13, 255]);
        assert_eq!(marker_color(3), [196, 24, 0, 255]);
        assert_eq!(marker_color(7), [0, 168, 168, 255]);
        for neutral in [8, 9, 11, 12, -1] {
            assert_eq!(marker_color(neutral), [0xAC, 0xAC, 0xAC, 0xFF]);
        }
    }

    #[test]
    fn shape_follows_texture_bbox_and_wall() {
        assert_eq!(marker_shape(false, true, true), MarkerShape::Lines);
        assert_eq!(marker_shape(true, true, true), MarkerShape::Frame);
        assert_eq!(marker_shape(true, false, true), MarkerShape::FlatSquare);
        assert_eq!(marker_shape(true, false, false), MarkerShape::Ring);
    }

    #[test]
    fn ring_uv_runs_over_the_square_and_turns_with_the_object() {
        // Tile 128, ring r = 64 around the tile center (192, 192): 128 ... 256 -> tiles 1 ... 2
        // (`trunc(256/128) = 2`, inclusive).
        let (terrain, _) = test_terrain(4, |_, _| 1);
        let all = TileMask::all(4, 4);
        let p = Vec3::new(192.0, 0.0, 192.0);
        let mut out = Vec::new();
        let white = [255; 4];
        assert_eq!(
            push_ring(&terrain, &all, 64.0, p, 0.0, white, false, &mut out),
            4
        );
        let uv_at = |out: &[DecalVertex], x: f32, z: f32| {
            out.iter()
                .find(|v| v.position[0] == x && v.position[2] == z)
                .unwrap()
                .uv
        };
        // Like the shadow decal: u toward +x, v toward -z (v0 = -(Z - p.z)/dz, offset 0.5).
        assert!(close(uv_at(&out, 128.0, 128.0), [0.0, 1.0]));
        assert!(close(uv_at(&out, 256.0, 128.0), [1.0, 1.0]));
        assert!(close(uv_at(&out, 128.0, 256.0), [0.0, 0.0]));
        assert!(close(uv_at(&out, 256.0, 256.0), [1.0, 0.0]));
        assert!(out.iter().all(|v| v.color == white));
        // Quarter turn: the corner (-r, -r) gets the UV of the corner (-r, +r) without rotation.
        out.clear();
        let yaw = std::f32::consts::FRAC_PI_2;
        push_ring(&terrain, &all, 64.0, p, yaw, white, false, &mut out);
        assert!(close(uv_at(&out, 128.0, 128.0), [0.0, 0.0]));
        assert!(close(uv_at(&out, 256.0, 256.0), [1.0, 1.0]));
    }

    #[test]
    fn ring_ignores_tile_visibility_and_can_rise_to_the_water() {
        let (mut terrain, _) = test_terrain(4, |_, _| 1);
        terrain.params.water_level = 20.0;
        let all = TileMask::all(4, 4);
        let mut out = Vec::new();
        let p = Vec3::new(256.0, 0.0, 256.0);
        // +-100 -> tiles 1..2 in x and z.
        assert_eq!(
            push_ring(&terrain, &all, 100.0, p, 0.0, [1; 4], true, &mut out),
            4
        );
        assert!(out.iter().all(|v| v.position[1] == 20.0));
        out.clear();
        push_ring(&terrain, &all, 100.0, p, 0.0, [1; 4], false, &mut out);
        assert!(out.iter().all(|v| v.position[1] == 0.0));
    }

    #[test]
    fn flat_square_lies_at_object_height_without_rotation() {
        let mut out = Vec::new();
        push_flat_square(Vec3::new(10.0, 50.0, 20.0), 5.0, [9; 4], &mut out);
        assert_eq!(out.len(), 6);
        assert_eq!(out[0].position, [5.0, 50.0, 15.0]);
        assert_eq!(out[0].uv, [0.0, 0.0]);
        assert_eq!(out[4].position, [15.0, 50.0, 25.0]);
        assert_eq!(out[4].uv, [1.0, 1.0]);
    }

    #[test]
    fn frame_uv_is_a_nine_slice_without_the_inner_tiles() {
        // 4 x 3 tiles 0..3 x 0..2.
        let uv = |x, z| frame_uv(x, z, 0, 3, 0, 2);
        assert_eq!(uv(0, 0), Some([0.0, 0.5, 0.0, 0.5]));
        assert_eq!(uv(3, 0), Some([0.5, 1.0, 0.0, 0.5]));
        assert_eq!(uv(0, 2), Some([0.0, 0.5, 0.5, 1.0]));
        assert_eq!(uv(3, 2), Some([0.5, 1.0, 0.5, 1.0]));
        assert_eq!(uv(1, 0), Some([0.25, 0.75, 0.0, 0.5]));
        assert_eq!(uv(2, 2), Some([0.25, 0.75, 0.5, 1.0]));
        assert_eq!(uv(0, 1), Some([0.0, 0.5, 0.25, 0.75]));
        assert_eq!(uv(3, 1), Some([0.5, 1.0, 0.25, 0.75]));
        assert_eq!(uv(1, 1), None);
        // One tile, one row, one column.
        assert_eq!(frame_uv(5, 5, 5, 5, 5, 5), Some([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(frame_uv(6, 5, 5, 7, 5, 5), Some([0.25, 0.75, 0.0, 1.0]));
        assert_eq!(frame_uv(5, 5, 5, 7, 5, 5), Some([0.0, 0.5, 0.0, 1.0]));
        assert_eq!(frame_uv(5, 6, 5, 5, 5, 7), Some([0.0, 1.0, 0.25, 0.75]));
        assert_eq!(frame_uv(5, 7, 5, 5, 5, 7), Some([0.0, 1.0, 0.5, 1.0]));
    }

    #[test]
    fn bounds_frame_insets_by_an_eighth_tile_and_skips_the_inside() {
        // Tile 128: hull +-200 around (512, 512) -> 312+16 ... 712-16 = 328 ... 696 -> tiles 2 ... 5.
        let (terrain, _) = test_terrain(8, |_, _| 1);
        let hull = DecalHull {
            center: Vec3::ZERO,
            half: Vec3::new(200.0, 50.0, 200.0),
        };
        let mut out = Vec::new();
        let p = Vec3::new(512.0, 0.0, 512.0);
        let n = push_bounds_frame(&terrain, &hull, None, p, 0.0, [7; 4], &mut out);
        // 4 x 4 tiles, of which 2 x 2 inside.
        assert_eq!(n, 12);
        assert_eq!(out.len(), 12 * 6);
        // First tile (2, 2): corner (2, 2) gets uv (0, 0), corner (3, 3) uv (0.5, 0.5).
        assert_eq!(out[0].position, [256.0, 0.0, 256.0]);
        assert_eq!(out[0].uv, [0.0, 0.0]);
        assert_eq!(out[4].position, [384.0, 0.0, 384.0]);
        assert_eq!(out[4].uv, [0.5, 0.5]);
        // A helper node rectangle enlarges each tile by a quarter tile outward.
        out.clear();
        let n = push_bounds_frame(
            &terrain,
            &hull,
            Some([1, 1, 1, 1]),
            p,
            0.0,
            [7; 4],
            &mut out,
        );
        assert_eq!(n, 1);
        assert_eq!(out[0].position, [96.0, 0.0, 96.0]);
        assert_eq!(out[4].position, [288.0, 0.0, 288.0]);
        assert_eq!(out[4].uv, [1.0, 1.0]);
    }

    #[test]
    fn bounds_frame_turns_with_the_object() {
        // Tile 128, hull +-250 (x) x +-100 (z) around (512, 512), inset by 16:
        // without rotation x 278 ... 746 -> tiles 2 ... 5, z 428 ... 596 -> 3 ... 4;
        // swapped by a quarter turn: x 3 ... 4, z 2 ... 5.
        let (terrain, _) = test_terrain(8, |_, _| 1);
        let hull = DecalHull {
            center: Vec3::ZERO,
            half: Vec3::new(250.0, 50.0, 100.0),
        };
        let p = Vec3::new(512.0, 0.0, 512.0);
        let tiles = |yaw: f32| {
            let mut out = Vec::new();
            let n = push_bounds_frame(&terrain, &hull, None, p, yaw, [7; 4], &mut out);
            assert_eq!(out.len(), n * 6);
            let range = |k: usize| {
                let lo = out.iter().map(|v| v.position[k]).fold(f32::MAX, f32::min);
                let hi = out.iter().map(|v| v.position[k]).fold(f32::MIN, f32::max);
                ((lo / 128.0) as i32, (hi / 128.0) as i32 - 1)
            };
            (n, range(0), range(2), out)
        };
        let (n, x, z, _) = tiles(0.0);
        assert_eq!((n, x, z), (8, (2, 5), (3, 4)));
        let (n, x, z, out) = tiles(std::f32::consts::FRAC_PI_2);
        assert_eq!((n, x, z), (8, (3, 4), (2, 5)));
        // Two columns x four rows without inner tiles: first tile (3, 2) with the corner
        // (0 ... 0.5, 0 ... 0.5), the edge tile (3, 3) across in the middle strip.
        assert_eq!(out[0].position, [384.0, 0.0, 256.0]);
        assert_eq!(out[0].uv, [0.0, 0.0]);
        assert_eq!(out[4].position, [512.0, 0.0, 384.0]);
        assert_eq!(out[4].uv, [0.5, 0.5]);
        assert_eq!(out[6].position, [384.0, 0.0, 384.0]);
        assert_eq!(out[6].uv, [0.0, 0.25]);
    }

    #[test]
    fn helper_rect_matches_the_occupancy_test_vectors() {
        let boxed = |x0: f32, x1: f32| -> [Vec3; 8] {
            std::array::from_fn(|i| {
                Vec3::new(
                    if i & 1 == 0 { x0 } else { x1 },
                    0.0,
                    if i & 4 == 0 { 100.0 } else { 300.0 },
                )
            })
        };
        assert_eq!(
            helper_box_tile_rect(&boxed(100.0, 300.0), 128, 128),
            [0, 0, 2, 2]
        );
        assert_eq!(helper_box_tile_rect(&boxed(-50.0, 40.0), 128, 128)[0], -1);
        assert_eq!(helper_box_tile_rect(&boxed(-50.0, 40.0), 128, 128)[2], 0);
        // Narrower than a quarter tile: bounds swapped.
        assert_eq!(helper_box_tile_rect(&boxed(100.0, 120.0), 128, 128)[0], 0);
        assert_eq!(helper_box_tile_rect(&boxed(100.0, 120.0), 128, 128)[2], 0);
    }

    #[test]
    fn helper_node_corners_apply_the_node_scale() {
        // Cube +-10 as a helper node at (0, 20, 0), scale (3, 0.5, 1) like the `hitbox` of the
        // buildings (stored corners a cube, shape only via the scale, `0x00578180`).
        let mut rdo = nml_formats::testutil::sample_rdo(nml_formats::rdo::RdoLayout::Standard);
        let mut node = rdo.meshes[0].clone();
        node.kind = nml_formats::rdo::MeshKind::Helper;
        node.type_id = 3005;
        node.name = "Hitbox".into();
        node.position = [0.0, 20.0, 0.0];
        node.rotation_wxyz = [1.0, 0.0, 0.0, 0.0];
        node.scale = [3.0, 0.5, 1.0];
        node.bounds = nml_formats::testutil::bounds([-10.0; 3], [10.0; 3]);
        rdo.meshes.push(node);
        let asset = ModelAsset {
            path: "x.rdo".into(),
            rdo,
            config: Default::default(),
            animations: Vec::new(),
            material_textures: Vec::new(),
            bone_track_cache: Default::default(),
        };
        let world = Affine3A::from_translation(Vec3::new(100.0, 0.0, 0.0));
        let c = helper_node_corners(&asset, "hitbox", &world).unwrap();
        assert_eq!(c[0], Vec3::new(70.0, 15.0, -10.0));
        assert_eq!(c[7], Vec3::new(130.0, 25.0, 10.0));
    }

    #[test]
    fn line_rect_lies_one_unit_above_the_hull_bottom() {
        let world = Affine3A::from_translation(Vec3::new(100.0, 10.0, 0.0));
        let l = line_rect(Vec3::new(0.0, 5.0, 0.0), Vec3::new(2.0, 3.0, 4.0), &world);
        assert_eq!(l[0], Vec3::new(98.0, 13.0, -4.0));
        assert_eq!(l[1], Vec3::new(102.0, 13.0, -4.0));
        assert_eq!(l[7], l[0]);
        assert_eq!(l[5], Vec3::new(98.0, 13.0, 4.0));
    }
}
