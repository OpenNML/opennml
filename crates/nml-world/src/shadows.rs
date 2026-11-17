//! Geometry of shadows on the terrain as in the original (re/shadows): decals from the shadow
//! model or ground plate (`ShadowCaster_DrawShadowDecal` `0x0054D300`,
//! `ShadowCaster_DrawGroundplaneDecal` `0x0054DBE0`), light view and projection of the shadow
//! textures (`ShadowTexture_RenderOccluder` `0x0054E7B0`, `Matrix_ShadowProjection` `0x00575B60`)
//! and their receivers (`ShadowTexture_ProjectOnReceiver` `0x0054F0C0`). Pure CPU computation;
//! drawing happens in `nml-render`.

use glam::{Affine3A, Mat4, Vec3, Vec4};
use nml_formats::rdo::{Bounds, MeshKind};
use nml_formats::wld::World;

use crate::anim::{Playback, Pose};
use crate::assets::ModelAsset;
use crate::camera::look_at_lh;
use crate::classes::ShadowKind;
use crate::math::{game_rotation_y, mesh_node_matrix};
use crate::terrain::{FogFactors, Terrain};
use crate::visibility::{Containment, Frustum, TileRect};

/// Distance of the light eye point from the hull box center (`0x005762E0` with 10,000).
pub const LIGHT_DISTANCE: f32 = 10_000.0;
/// Largest tile span `|x0 - x1|` or `|z0 - z1|` of a texture shadow; above it, it is not
/// drawn (`0x0054F4DC`).
pub const MAX_TILE_SPAN: i64 = 20;
/// Shadow decal: color `0x80808080` (`ObjectRenderer_DrawShadowReceiverPass`), only RGB is
/// effective (`tex * 128/255`).
pub const SHADOW_DECAL_COLOR: [u8; 4] = [128, 128, 128, 128];
/// Color of the special-class decal (gold deposit, fertile soil) in the object pass:
/// `0xFF000000 | k'*0x010101` with `k' = trunc(k * 255)` (`0x00549B97`, re/shadows, shadow
/// decal), `k` = brightness factor of the object in the fog of war (1 = no fog).
pub fn special_decal_color(k: f32) -> [u8; 4] {
    let k = (k * 255.0) as u8;
    [k, k, k, 255]
}

/// Visibility of the tiles in a frame (`TerrainTile+0x00`, set by the quadtree).
#[derive(Debug, Clone, PartialEq)]
pub struct TileMask {
    dim_x: usize,
    dim_z: usize,
    visible: Vec<bool>,
}

impl TileMask {
    /// All tiles visible.
    pub fn all(dim_x: usize, dim_z: usize) -> Self {
        Self {
            dim_x,
            dim_z,
            visible: vec![true; dim_x * dim_z],
        }
    }

    /// Tiles of the rectangles from `TerrainQuadtree::visible_rects`.
    pub fn from_rects(rects: &[TileRect], dim_x: usize, dim_z: usize) -> Self {
        let mut mask = Self {
            dim_x,
            dim_z,
            visible: vec![false; dim_x * dim_z],
        };
        for rect in rects {
            for (x, z) in rect.tiles(dim_x, dim_z) {
                mask.visible[z * dim_x + x] = true;
            }
        }
        mask
    }

    /// Tile `(x, z)` lies in the map and is visible.
    pub fn contains(&self, x: i64, z: i64) -> bool {
        (0..self.dim_x as i64).contains(&x)
            && (0..self.dim_z as i64).contains(&z)
            && self.visible[z as usize * self.dim_x + x as usize]
    }

    /// Number of visible tiles.
    pub fn count(&self) -> usize {
        self.visible.iter().filter(|v| **v).count()
    }
}

/// Inclusive tile range `x0..=x1`, `z0..=z1` (`_ftol`, truncated toward zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileBox {
    pub x0: i64,
    pub x1: i64,
    pub z0: i64,
    pub z1: i64,
}

impl TileBox {
    /// Tiles of a rectangle in world coordinates (`min`, `max` in x/z).
    pub fn from_world(min: (f32, f32), max: (f32, f32), size: (f32, f32)) -> Self {
        Self {
            x0: (min.0 / size.0) as i64,
            x1: (max.0 / size.0) as i64,
            z0: (min.1 / size.1) as i64,
            z1: (max.1 / size.1) as i64,
        }
    }

    /// Tiles in drawing order (x outer, z inner as in `0x0054F4DC`).
    pub fn tiles(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        (self.x0..=self.x1).flat_map(move |x| (self.z0..=self.z1).map(move |z| (x, z)))
    }

    /// Like [`TileBox::tiles`], but only tiles in `0..dim_x` x `0..dim_z`. [`TileMask::contains`]
    /// rejects tiles outside the map anyway; this way the loop does not run over billions of
    /// tiles even for huge (saturated) ranges.
    pub fn tiles_in(&self, dim_x: usize, dim_z: usize) -> impl Iterator<Item = (i64, i64)> + '_ {
        let (max_x, max_z) = (dim_x as i64 - 1, dim_z as i64 - 1);
        (self.x0.max(0)..=self.x1.min(max_x))
            .flat_map(move |x| (self.z0.max(0)..=self.z1.min(max_z)).map(move |z| (x, z)))
    }

    /// Span `|x1 - x0|` and `|z1 - z0|` at most [`MAX_TILE_SPAN`] (without overflow for
    /// saturated ranges).
    pub fn span_within_limit(&self) -> bool {
        let limit = MAX_TILE_SPAN as u64;
        self.x1.abs_diff(self.x0) <= limit && self.z1.abs_diff(self.z0) <= limit
    }
}

/// The four grid points of a tile as two triangles in the same split as the terrain:
/// `(x,z), (x,z+1), (x+1,z)` and `(x,z+1), (x+1,z+1), (x+1,z)` (triangle strip with 4
/// vertices, `Renderer_DrawUpTriStripColorTex` `0x00571B50`).
pub fn tile_corners(x: usize, z: usize) -> [(usize, usize); 6] {
    [
        (x, z),
        (x, z + 1),
        (x + 1, z),
        (x, z + 1),
        (x + 1, z + 1),
        (x + 1, z),
    ]
}

/// Hull of a decal model: center and half extent of the `.rdo` header
/// (`Model_GetBoundsCenter`, `Model_GetBoundsHalfExtent`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalHull {
    pub center: Vec3,
    pub half: Vec3,
}

impl DecalHull {
    pub fn from_bounds(bounds: &Bounds) -> Self {
        let (min, max) = (Vec3::from(bounds.min), Vec3::from(bounds.max));
        Self {
            center: Vec3::from(bounds.center),
            half: (max - min) / 2.0,
        }
    }
}

/// Vertex of a decal (FVF `0x1C2` without specular): world position, texture coordinate, color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

/// Color of a decal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecalColor {
    /// Fixed color (shadow decal, special class).
    Fixed([u8; 4]),
    /// Terrain color per grid point (`TerrainVertex+0x28`, ground plate).
    Terrain,
}

/// Appends the tiles of a decal to `out` (`ShadowCaster_DrawShadowDecal`): footprint from the
/// four corners `p + RotY(theta)*(l|m)`, tiles in the map and visible, actual terrain heights.
/// Texture coordinates with the original's texture matrix: `(u, v) = (u0, v0, 1) * M` with
/// `u0 = (X - p.x)/dx`, `v0 = -(Z - p.z)/dz`, `M` from rotation theta and offset `a = -l.x/dx`,
/// `b = 1 + l.z/dz`. The terrain color of the ground plate carries the fog-of-war factor
/// (`fog`, `None` = factor 1; the original reads the fog-tinted `TerrainVertex+0x28`). Returns
/// the number of tiles.
#[allow(clippy::too_many_arguments)]
pub fn push_decal(
    terrain: &Terrain,
    visible: &TileMask,
    hull: &DecalHull,
    position: Vec3,
    yaw: f32,
    color: DecalColor,
    fog: Option<&FogFactors>,
    out: &mut Vec<DecalVertex>,
) -> usize {
    let l = hull.center - hull.half;
    let m = hull.center + hull.half;
    let (dx, dz) = (2.0 * hull.half.x, 2.0 * hull.half.z);
    if dx <= 0.0 || dz <= 0.0 {
        return 0;
    }
    let rotation = game_rotation_y(yaw);
    let corners = [(l.x, l.z), (m.x, l.z), (l.x, m.z), (m.x, m.z)]
        .map(|(x, z)| position + rotation * Vec3::new(x, 0.0, z));
    let min = corners
        .iter()
        .fold((f32::MAX, f32::MAX), |a, c| (a.0.min(c.x), a.1.min(c.z)));
    let max = corners
        .iter()
        .fold((f32::MIN, f32::MIN), |a, c| (a.0.max(c.x), a.1.max(c.z)));
    let p = &terrain.params;
    let tiles = TileBox::from_world(min, max, (p.size_x, p.size_z));
    let (s, c) = yaw.sin_cos();
    let (a, b) = (-l.x / dx, 1.0 + l.z / dz);
    let mut count = 0;
    for (x, z) in tiles.tiles_in(visible.dim_x, visible.dim_z) {
        if !visible.contains(x, z) {
            continue;
        }
        count += 1;
        for (gx, gz) in tile_corners(x as usize, z as usize) {
            let v = terrain.vertex(gx, gz);
            let u0 = (v.position.x - position.x) / dx;
            let v0 = -(v.position.z - position.z) / dz;
            let rgba = match color {
                DecalColor::Fixed(rgba) => rgba,
                DecalColor::Terrain => {
                    let [r, g, b] = fog.map_or(v.lit_color, |f| f.apply(gx, gz, v.lit_color));
                    [r, g, b, 0]
                }
            };
            out.push(DecalVertex {
                position: v.position.to_array(),
                uv: [u0 * c - v0 * s + a, u0 * s + v0 * c + b],
                color: rgba,
            });
        }
    }
    count
}

/// Direction **toward** the shadow light: normalized `[SHADOW] SHADOW_LIGHT_POSITION` (the
/// parser normalizes while reading, `0x00442C77`).
///
/// Deviation: A zero vector (or non-finite value) is degenerate in the original; here, as with
/// the vertical light (see [`light_view`]), the direction `(0, 1, 0)` applies.
pub fn shadow_light_dir(position: [f32; 3]) -> Vec3 {
    Vec3::from(position).try_normalize().unwrap_or(Vec3::Y)
}

/// Shadow light of a map: direction to the light and `SHADOW_INTENSITY`. The end of the
/// `[LIGHT]` block presets both (`0x00442BB9`: `LIGHT_POSITION`, `LIGHT_INTENSITY`), a
/// `[SHADOW]` block overrides them. Without either block (does not occur) `(0, 1, 0)` and 0.
///
/// Deviation: In the original the preset direction stays unnormalized (eye point farther away,
/// projection almost the same); here it is normalized.
pub fn shadow_light(world: &World) -> (Vec3, i32) {
    match (&world.shadow, &world.light) {
        (Some(shadow), _) => (shadow_light_dir(shadow.light_position), shadow.intensity),
        (None, Some(light)) => (shadow_light_dir(light.position), light.intensity),
        (None, None) => (Vec3::Y, 0),
    }
}

/// Alpha of a texture shadow: `(trunc(I * k) & 0xFF) / 255` (`0x005487C0`: only the low byte
/// ends up in the color's alpha), `k` = brightness factor of the object in the fog of war
/// (1 = no fog).
pub fn shadow_alpha(intensity: i32, k: f32) -> f32 {
    (((intensity as f32 * k) as i64) & 0xFF) as f32 / 255.0
}

/// Status bits `GameObject+0x1AC` that block the receiver pass: `ONWALL` (bit 25) always.
pub const NO_RECEIVER_STATUS: u32 = 0x0200_0000;
/// In addition for kind != 3: `ISMOUNTED` (13), `ISMOUNTEDONBOAT` (15), `SWIMMING` (16), `DIED` (18).
pub const NO_RECEIVER_STATUS_UNLESS_PER_FRAME: u32 = 0x2000 | 0x8000 | 0x1_0000 | 0x4_0000;

/// Does an object with status word `status` get its shadow on the ground in the receiver pass
/// (mask 2) (`ObjectRenderer_Draw` `0x00549AE4`...`0x00549B47`)? A dead object loses decal,
/// shadow texture and ground plate from `DIED` on for kinds 1 and 2; only kind 3 keeps drawing
/// (shadow texture per frame in the current pose, without the carcass's opacity).
pub fn receives_shadow(kind: ShadowKind, status: u32) -> bool {
    status & NO_RECEIVER_STATUS == 0
        && (kind == ShadowKind::PerFrame || status & NO_RECEIVER_STATUS_UNLESS_PER_FRAME == 0)
}

/// Axis-aligned box in object space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// Box for "no mesh" (zeros). The original (`0x0058AC40`) starts the union with the box of
    /// the first mesh; zeros apply only if the instance has no mesh of its own.
    pub const ZERO: Self = Self {
        min: Vec3::ZERO,
        max: Vec3::ZERO,
    };

    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        points.into_iter().fold(None, |acc: Option<Self>, p| {
            Some(match acc {
                Some(b) => Self {
                    min: b.min.min(p),
                    max: b.max.max(p),
                },
                None => Self { min: p, max: p },
            })
        })
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Corners as in `0x005796A0`: corner `i` = (`i & 1` -> max.x, `i & 2` -> max.y, `i & 4` -> max.z).
    pub fn corners(&self) -> [Vec3; 8] {
        std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 != 0 { self.max.x } else { self.min.x },
                if i & 2 != 0 { self.max.y } else { self.min.y },
                if i & 4 != 0 { self.max.z } else { self.min.z },
            )
        })
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) / 2.0
    }

    pub fn half_extent(&self) -> Vec3 {
        (self.max - self.min) / 2.0
    }

    fn of_bounds(bounds: &Bounds) -> Self {
        Self {
            min: Vec3::from(bounds.min),
            max: Vec3::from(bounds.max),
        }
    }

    fn of_max_bounding_box(b: Option<[f32; 6]>) -> Self {
        // Without `MAXBOUNDINGBOX` (9 animations in the shipped data) the model's memory stays
        // unwritten; zeros are assumed.
        let b = b.unwrap_or([0.0; 6]);
        Self {
            min: Vec3::new(b[0], b[1], b[2]),
            max: Vec3::new(b[3], b[4], b[5]),
        }
    }
}

/// Mesh part of `ModelInstance_ComputeBoundingBox` (`0x0058AC40`). Without `animated` (flag 0)
/// the union of the file's mesh hulls (helper nodes excluded). With
/// `animated = Some((animation, crossfade source, pose))` (flag 1, animation running):
/// skin meshes and meshes with a morph track in the animation via their `MAXBOUNDINGBOX`
/// (united with that of the source), other meshes via their hull in the current pose
/// (`ModelInstance_GetMeshBoundingCorners` `0x005887E0`). `None` without meshes.
pub fn mesh_box(
    model: &ModelAsset,
    animated: Option<(usize, Option<usize>, &Pose)>,
) -> Option<Aabb> {
    let max_box = |index: usize| {
        Aabb::of_max_bounding_box(
            model
                .config
                .animations
                .get(index)
                .and_then(|a| a.max_bounding_box),
        )
    };
    let mut result: Option<Aabb> = None;
    for (i, mesh) in model.rdo.meshes.iter().enumerate() {
        if mesh.kind == MeshKind::Helper {
            continue;
        }
        let part = match animated {
            Some((current, source, pose)) => {
                let morph = model
                    .animations
                    .get(current)
                    .and_then(Option::as_ref)
                    .is_some_and(|a| a.morph_tracks.iter().any(|t| t.name == mesh.name));
                if mesh.kind == MeshKind::Skin || morph {
                    let b = max_box(current);
                    source.map_or(b, |s| b.union(&max_box(s)))
                } else {
                    let to_object = match mesh.kind {
                        MeshKind::Rigid => {
                            pose.mesh_matrix(model, i) * mesh_node_matrix(mesh).inverse()
                        }
                        _ => Affine3A::IDENTITY,
                    };
                    let corners = Aabb::of_bounds(&mesh.bounds).corners();
                    Aabb::from_points(corners.map(|c| to_object.transform_point3(c)))
                        .unwrap_or(Aabb::ZERO)
                }
            }
            None => Aabb::of_bounds(&mesh.bounds),
        };
        result = Some(result.map_or(part, |r| r.union(&part)));
    }
    result
}

/// Hull of an instance with sub-objects (`ModelInstance_ComputeBoundingBox`): `pose` = pose of
/// the running animation (flag 1), `None` = rest hull (flag 0). Sub-objects of the list
/// ([`Playback::bounding_subs`]) with their own playback, without link transform (as in the
/// original). The caller takes care of the monotonic growth at flag 1 (`Instance+0x110`).
pub fn instance_box(
    model: &ModelAsset,
    playback: &Playback,
    pose: Option<&Pose>,
    now_ms: f32,
) -> Aabb {
    let player = playback.player();
    let animated = pose.and_then(|pose| {
        player
            .current
            .map(|current| (current, player.crossfade_source(now_ms), pose))
    });
    let flag = animated.is_some();
    let mut result = mesh_box(model, animated);
    for sub in playback.bounding_subs(flag) {
        let sub_pose =
            (flag && sub.player.current.is_some()).then(|| sub.player.pose(&sub.asset, now_ms));
        let sub_animated = sub_pose.as_ref().and_then(|pose| {
            sub.player
                .current
                .map(|current| (current, sub.player.crossfade_source(now_ms), pose))
        });
        let part = mesh_box(&sub.asset, sub_animated).unwrap_or(Aabb::ZERO);
        result = Some(result.unwrap_or(Aabb::ZERO).union(&part));
    }
    result.unwrap_or(Aabb::ZERO)
}

/// Visibility test of an object like `0x00468C60`: sphere (center and half diagonal of the
/// rest hull) and, if it lies partially inside, the axis-aligned rest box, both only shifted by
/// the object position - rotation and scaling of the object are not taken into account.
pub fn object_in_view(frustum: &Frustum, rest: &Aabb, position: Vec3) -> bool {
    let radius = rest.half_extent().length();
    match frustum.classify_sphere(rest.center() + position, radius) {
        Containment::Outside => false,
        Containment::Inside => true,
        Containment::Partial => {
            frustum.classify_box(rest.min + position, rest.max + position) != Containment::Outside
        }
    }
}

/// Ground shadow in the view frustum (`ShadowTexture_UpdateVisibility` `0x0054F8B0`, kind 2/3
/// only): center `C = W * center(rest hull)`, `H = C + half extent` (unrotated), eye point
/// `C + 10000 * L`; the rays through `C` and `H` intersect the plane `y = (W * corner 0).y`;
/// sphere around the first intersection point with the distance between the two as radius.
pub fn ground_shadow_in_view(
    frustum: &Frustum,
    rest: &Aabb,
    world: &Affine3A,
    light: Vec3,
) -> bool {
    let y_min = world.transform_point3(rest.corners()[0]).y;
    let center = world.transform_point3(rest.center());
    let edge = center + rest.half_extent();
    let eye = center + light * LIGHT_DISTANCE;
    let hit = |target: Vec3| {
        let direction = (target - eye).normalize_or_zero();
        (direction.y < 0.0).then(|| eye + direction * ((y_min - eye.y) / direction.y))
    };
    let (Some(a), Some(b)) = (hit(center), hit(edge)) else {
        return false;
    };
    frustum.classify_sphere(a, (a - b).length()) != Containment::Outside
}

/// `Matrix_ShadowProjection(zn = -100, zf = 100, x_max, y_max, B, H)` (`0x00575B60`) as a
/// glam matrix: `m00 = (0.98 - 1/B)/x_max`, `m11 = (0.98 - 1/H)/y_max`, `m22 = 0.5`, `m23 = 1`,
/// `m32 = 50`.
pub fn shadow_projection(x_max: f32, y_max: f32, size: u32) -> Mat4 {
    let s = size as f32;
    Mat4::from_cols(
        Vec4::new((0.98 - 1.0 / s) / x_max, 0.0, 0.0, 0.0),
        Vec4::new(0.0, (0.98 - 1.0 / s) / y_max, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.5, 1.0),
        Vec4::new(0.0, 0.0, 50.0, 0.0),
    )
}

/// Light view `Matrix_LookAtLH(eye, center, up = (0, 1, 0))`.
///
/// Deviation: If the light is vertical (`SHADOW_LIGHT_POSITION 0 1 0`, two maps), the original
/// is degenerate (normalizing a zero vector); then `up = (0, 0, 1)` applies.
pub fn light_view(eye: Vec3, center: Vec3) -> Mat4 {
    let forward = (center - eye).normalize_or_zero();
    let up = if forward.cross(Vec3::Y).length_squared() < 1e-12 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    look_at_lh(eye, center, up)
}

/// Light view, projection and receiver range of a texture shadow (state after
/// `ShadowTexture_RenderOccluder` and the tile range of `ShadowTexture_ProjectOnReceiver`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowFrame {
    /// `L_v` (`ShadowCaster+0x128`).
    pub view: Mat4,
    /// `P_s` (`ShadowCaster+0x1A8`).
    pub projection: Mat4,
    pub eye: Vec3,
    /// y of hull box corner 0 in world (`ShadowCaster+0x44`), height of the projection plane.
    pub y_min: f32,
    /// Receiver tiles; `None` if the span is above [`MAX_TILE_SPAN`] (then the original draws
    /// nothing).
    pub tiles: Option<TileBox>,
}

impl ShadowFrame {
    /// Computes light view, projection and tile range for an occluder: `corners` = eight hull
    /// box corners in object space (corner `i`: `i & 1` -> x max, `i & 2` -> y max, `i & 4` ->
    /// z max), `world` = world matrix, `light` = direction to the light, `size` = texture size.
    pub fn new(
        corners: &[Vec3; 8],
        world: &Affine3A,
        light: Vec3,
        size: u32,
        tile_size: (f32, f32),
    ) -> Self {
        // Zero light like vertical light (otherwise the eye point falls onto the center: NaN view).
        let light = if light.is_finite() && light.length_squared() > 1e-12 {
            light
        } else {
            Vec3::Y
        };
        let center = world.transform_point3((corners[0] + corners[7]) / 2.0);
        let eye = center + light * LIGHT_DISTANCE;
        let view = light_view(eye, center);
        let world_corners = corners.map(|c| world.transform_point3(c));
        let (mut x_max, mut y_max) = (0.0f32, 0.0f32);
        for corner in &world_corners {
            let q = view.transform_point3(*corner);
            x_max = x_max.max((q.x / q.z).abs());
            y_max = y_max.max((q.y / q.z).abs());
        }
        let y_min = world_corners[0].y;
        // Rays from the eye point through the corners onto the plane y = y_min (`0x00576F60`).
        let mut min = (f32::MAX, f32::MAX);
        let mut max = (f32::MIN, f32::MIN);
        let mut hit = None;
        for corner in &world_corners {
            let direction = (*corner - eye).normalize_or_zero();
            if direction.y < 0.0 {
                hit = Some(eye + direction * ((y_min - eye.y) / direction.y));
            }
            if let Some(p) = hit {
                min = (min.0.min(p.x), min.1.min(p.z));
                max = (max.0.max(p.x), max.1.max(p.z));
            }
        }
        let tiles = hit
            .map(|_| TileBox::from_world(min, max, tile_size))
            .filter(TileBox::span_within_limit);
        Self {
            view,
            projection: shadow_projection(x_max.max(1e-6), y_max.max(1e-6), size),
            eye,
            y_min,
            tiles,
        }
    }

    /// Texture coordinates of a receiver point (without the D3D8 half texel): the point is pushed
    /// vertically onto `y = y_min` and projected from the light.
    pub fn texture_coord(&self, p: Vec3) -> [f32; 2] {
        let q = self.light_coord(p);
        let (m00, m11) = (self.projection.x_axis.x, self.projection.y_axis.y);
        [0.5 + 0.5 * m00 * q[0] / q[2], 0.5 - 0.5 * m11 * q[1] / q[2]]
    }

    /// `q = L_v * (P.x, y_min, P.z)` (light view space, per vertex; the division happens per pixel).
    pub fn light_coord(&self, p: Vec3) -> [f32; 3] {
        self.view
            .transform_point3(Vec3::new(p.x, self.y_min, p.z))
            .to_array()
    }
}

/// Vertex of a texture shadow receiver: world position and `q` (light view space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReceiverVertex {
    pub position: [f32; 3],
    pub light: [f32; 3],
}

/// Appends the receiver tiles of a texture shadow to `out`: tiles of the range in the map and
/// visible, actual heights, with `on_water` at least `WATER_LEVEL`. Returns the number of
/// tiles.
pub fn push_receiver(
    terrain: &Terrain,
    visible: &TileMask,
    frame: &ShadowFrame,
    on_water: bool,
    out: &mut Vec<ReceiverVertex>,
) -> usize {
    let Some(tiles) = frame.tiles else { return 0 };
    let water = terrain.params.water_level;
    let mut count = 0;
    for (x, z) in tiles.tiles_in(visible.dim_x, visible.dim_z) {
        if !visible.contains(x, z) {
            continue;
        }
        count += 1;
        for (gx, gz) in tile_corners(x as usize, z as usize) {
            let mut p = terrain.vertex(gx, gz).position;
            if on_water {
                p.y = p.y.max(water);
            }
            out.push(ReceiverVertex {
                position: p.to_array(),
                light: frame.light_coord(p),
            });
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::tests::test_terrain;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn dead_objects_keep_a_ground_shadow_only_with_a_per_frame_texture() {
        use ShadowKind::*;
        const DIED: u32 = 0x4_0000;
        let all = [None, Decal, Persistent, PerFrame];
        assert!(all.iter().all(|&k| receives_shadow(k, 0)));
        // `0x00549B10`: kind 3 skips the check of bits 13, 15, 16 and 18.
        for bit in [0x2000, 0x8000, 0x1_0000, DIED] {
            assert_eq!(
                all.map(|k| receives_shadow(k, bit | 0x2)),
                [false, false, false, true],
                "Bit {bit:#x}"
            );
        }
        // `ONWALL` blocks every kind, other bits (combat, carrying) do not.
        assert!(all.iter().all(|&k| !receives_shadow(k, 0x0200_0000)));
        assert!(all.iter().all(|&k| receives_shadow(k, 0x2 | 0x4000)));
    }

    #[test]
    fn tile_masks_follow_the_visible_rectangles() {
        let rects = [
            TileRect {
                x0: 0,
                z0: 0,
                size: 2,
            },
            TileRect {
                x0: 3,
                z0: 3,
                size: 1,
            },
        ];
        let mask = TileMask::from_rects(&rects, 4, 4);
        assert_eq!(mask.count(), 5);
        assert!(mask.contains(1, 1) && mask.contains(3, 3));
        assert!(!mask.contains(2, 2) && !mask.contains(-1, 0) && !mask.contains(4, 0));
        assert_eq!(TileMask::all(4, 4).count(), 16);
    }

    #[test]
    fn decal_uv_maps_the_hull_rectangle_in_object_space() {
        let (terrain, _) = test_terrain(8, |_, _| 1);
        // Hull x in [-64, 64], z in [-32, 96] (dx = 128, dz = 128), object at (512, 0, 512).
        let hull = DecalHull {
            center: Vec3::new(0.0, 0.0, 32.0),
            half: Vec3::new(64.0, 1.0, 64.0),
        };
        let mut out = Vec::new();
        let p = Vec3::new(512.0, 0.0, 512.0);
        let tiles = push_decal(
            &terrain,
            &TileMask::all(8, 8),
            &hull,
            p,
            0.0,
            DecalColor::Fixed(SHADOW_DECAL_COLOR),
            None,
            &mut out,
        );
        // Footprint x in [448, 576] -> tiles 3, 4; z in [480, 608] -> tiles 3, 4.
        assert_eq!(tiles, 4);
        assert_eq!(out.len(), 24);
        // u = (x_l - l.x)/dx, v = (m.z - z_l)/dz.
        let uv_at = |x: f32, z: f32| {
            out.iter()
                .find(|v| v.position[0] == x && v.position[2] == z)
                .unwrap()
                .uv
        };
        let [u, v] = uv_at(512.0, 512.0);
        assert!(close(u, 0.5) && close(v, 0.75), "{u} {v}");
        let [u, v] = uv_at(384.0, 640.0);
        assert!(close(u, -0.5) && close(v, -0.25), "{u} {v}");
        // Rotated by 90 degrees: local (x, z) lies at world p + (-z, 0, x) (like the object matrix).
        out.clear();
        push_decal(
            &terrain,
            &TileMask::all(8, 8),
            &hull,
            p,
            std::f32::consts::FRAC_PI_2,
            DecalColor::Terrain,
            None,
            &mut out,
        );
        let world = |x_l: f32, z_l: f32| {
            p + game_rotation_y(std::f32::consts::FRAC_PI_2) * Vec3::new(x_l, 0.0, z_l)
        };
        let probe = world(0.0, 128.0);
        assert!(close(probe.x, 384.0) && close(probe.z, 512.0), "{probe:?}");
        let uv_at = |x: f32, z: f32| {
            out.iter()
                .find(|v| close(v.position[0], x) && close(v.position[2], z))
                .unwrap()
        };
        let v = uv_at(512.0, 512.0);
        assert!(close(v.uv[0], 0.5) && close(v.uv[1], 0.75), "{:?}", v.uv);
        // Local (0, 128): u = (0 + 64)/128, v = (96 - 128)/128.
        let uv = uv_at(probe.x, probe.z).uv;
        assert!(close(uv[0], 0.5) && close(uv[1], -0.25), "{uv:?}");
        // Ground plate: terrain color (test_terrain: 200, 200, 200).
        assert_eq!(v.color, [200, 200, 200, 0]);
        // With fog factor 0x80 per grid point: 200 * 128/255, truncated.
        let fog = FogFactors {
            verts_x: 9,
            values: vec![0x80; 81],
        };
        out.clear();
        push_decal(
            &terrain,
            &TileMask::all(8, 8),
            &hull,
            p,
            0.0,
            DecalColor::Terrain,
            Some(&fog),
            &mut out,
        );
        assert!(out.iter().all(|v| v.color == [100, 100, 100, 0]));
    }

    #[test]
    fn special_decals_carry_the_brightness_of_the_object() {
        assert_eq!(special_decal_color(1.0), [255, 255, 255, 255]);
        assert_eq!(special_decal_color(0.5), [127, 127, 127, 255]);
        assert_eq!(special_decal_color(0.0), [0, 0, 0, 255]);
    }

    #[test]
    fn decals_skip_tiles_outside_the_map_or_out_of_view() {
        let (terrain, _) = test_terrain(4, |_, _| 1);
        let hull = DecalHull {
            center: Vec3::ZERO,
            half: Vec3::new(100.0, 1.0, 100.0),
        };
        let mut out = Vec::new();
        // At (0, 0, 0): footprint -100...100 -> tiles trunc(-0.78) = 0 ... 0 (toward zero).
        let all = TileMask::all(4, 4);
        let corner = DecalColor::Fixed(SHADOW_DECAL_COLOR);
        assert_eq!(
            push_decal(
                &terrain,
                &all,
                &hull,
                Vec3::ZERO,
                0.0,
                corner,
                None,
                &mut out
            ),
            1
        );
        // In the map center 3 x 3 tiles, one of them invisible.
        let mut mask = TileMask::all(4, 4);
        mask.visible[2 * 4 + 2] = false;
        let center = Vec3::new(256.0, 0.0, 256.0);
        assert_eq!(
            push_decal(&terrain, &mask, &hull, center, 0.0, corner, None, &mut out),
            3
        );
        let center = Vec3::new(200.0, 0.0, 200.0);
        assert_eq!(
            push_decal(&terrain, &mask, &hull, center, 0.0, corner, None, &mut out),
            8
        );
    }

    #[test]
    fn shadow_light_falls_back_to_the_object_light_and_alpha_wraps() {
        let mut world = nml_formats::wld::parse_world(
            "[TERRAIN]\n{\nTERRAIN_DIM_X 2\nTERRAIN_DIM_Z 2\nCHUNK_SIZE 2\n}\n[LIGHT]\n{\nLIGHT_POSITION 0 3 4\nLIGHT_INTENSITY 40\n}\n",
        )
        .unwrap();
        let (dir, intensity) = shadow_light(&world);
        assert!((dir - Vec3::new(0.0, 0.6, 0.8)).length() < 1e-6 && intensity == 40);
        world.shadow = Some(nml_formats::wld::ShadowParams {
            light_position: [1.0, 0.0, 0.0],
            intensity: 128,
        });
        assert_eq!(shadow_light(&world), (Vec3::X, 128));
        world.shadow = None;
        world.light = None;
        assert_eq!(shadow_light(&world), (Vec3::Y, 0));
        assert!(close(shadow_alpha(128, 1.0), 128.0 / 255.0));
        // I = 300: only the low byte (44).
        assert!(close(shadow_alpha(300, 1.0), 44.0 / 255.0));
        assert!(close(shadow_alpha(128, 0.5), 64.0 / 255.0));
    }

    #[test]
    fn boxes_union_mesh_hulls_or_follow_the_animation() {
        use crate::anim::AnimationPlayer;
        use nml_formats::object_cfg::{AnimationEntry, ObjectConfig};
        use nml_formats::rdm::RdmLayout;
        use nml_formats::rdo::RdoLayout;
        use nml_formats::testutil::{bounds, sample_rdm, sample_rdo};
        let mut rdo = sample_rdo(RdoLayout::Standard);
        rdo.meshes[0].bounds = bounds([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        rdo.meshes[1].bounds = bounds([-5.0, 0.0, -5.0], [5.0, 100.0, 5.0]);
        // A helper node far outside does not count.
        rdo.meshes[2].bounds = bounds([500.0; 3], [600.0; 3]);
        let model = ModelAsset {
            path: "t.rdo".into(),
            rdo,
            config: ObjectConfig {
                model_file: None,
                sub_objects: vec![],
                animations: vec![AnimationEntry {
                    file: Some("a.rdm".into()),
                    guid: None,
                    max_bounding_box: Some([-20.0, 0.0, -20.0, 20.0, 150.0, 20.0]),
                    sub_objects: vec![],
                }],
            },
            animations: vec![Some(std::sync::Arc::new(sample_rdm(RdmLayout::Standard)))],
            material_textures: vec![None, None],
            bone_track_cache: Default::default(),
        };
        let rest = mesh_box(&model, None).unwrap();
        assert_eq!(
            (rest.min, rest.max),
            (Vec3::new(-5.0, 0.0, -5.0), Vec3::new(10.0, 100.0, 10.0))
        );
        // With animation: skin via MAXBOUNDINGBOX, the rigid kiste via its hull in the node
        // track pose.
        let pose = AnimationPlayer::new(Some(0), 0.0).pose(&model, 900.0);
        let animated = mesh_box(&model, Some((0, None, &pose))).unwrap();
        assert_eq!(animated.min.y, 0.0);
        assert_eq!(animated.max.y.max(150.0), animated.max.y);
        assert!(animated.min.x <= -20.0 && animated.max.x >= 20.0);
        let corners = rest.corners();
        assert_eq!((corners[0], corners[7]), (rest.min, rest.max));
        assert_eq!(corners[3], Vec3::new(10.0, 100.0, -5.0));
    }

    #[test]
    fn objects_are_visible_by_their_unrotated_rest_box_or_their_ground_shadow() {
        let top_down = |x0: f32, x1: f32, z0: f32, z1: f32| {
            let center = Vec3::new((x0 + x1) / 2.0, 1000.0, (z0 + z1) / 2.0);
            let view = look_at_lh(center, center - Vec3::Y * 1000.0, Vec3::Z);
            let projection = Mat4::from_cols(
                Vec4::new(2.0 / (x1 - x0), 0.0, 0.0, 0.0),
                Vec4::new(0.0, 2.0 / (z1 - z0), 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0 / 2000.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            );
            Frustum::from_view_proj(projection * view)
        };
        let frustum = top_down(0.0, 1000.0, 0.0, 1000.0);
        let rest = Aabb {
            min: Vec3::new(0.0, 0.0, -10.0),
            max: Vec3::new(200.0, 50.0, 10.0),
        };
        assert!(object_in_view(
            &frustum,
            &rest,
            Vec3::new(500.0, 0.0, 500.0)
        ));
        // Outside on the right, even if the rotation would turn the model into view.
        assert!(!object_in_view(
            &frustum,
            &rest,
            Vec3::new(1050.0, 0.0, 500.0)
        ));
        // Partially: the box protrudes into the view from the left.
        assert!(object_in_view(
            &frustum,
            &rest,
            Vec3::new(-150.0, 0.0, 500.0)
        ));
        // Ground shadow: light flat from +x, the object stands outside on the right, the shadow
        // falls toward -x into the view.
        let tall = Aabb {
            min: Vec3::new(-10.0, 0.0, -10.0),
            max: Vec3::new(10.0, 400.0, 10.0),
        };
        let world = Affine3A::from_translation(Vec3::new(1100.0, 0.0, 500.0));
        let light = Vec3::new(1.0, 1.0, 0.0).normalize();
        assert!(!object_in_view(
            &frustum,
            &tall,
            Vec3::new(1100.0, 0.0, 500.0)
        ));
        assert!(ground_shadow_in_view(&frustum, &tall, &world, light));
        // Light from -x: the shadow falls outward.
        let away = Vec3::new(-1.0, 1.0, 0.0).normalize();
        assert!(!ground_shadow_in_view(&frustum, &tall, &world, away));
    }

    #[test]
    fn shadow_projection_matches_the_decompiled_matrix() {
        let p = shadow_projection(0.02, 0.04, 128);
        let m00 = (0.98 - 1.0 / 128.0) / 0.02;
        assert!(close(p.x_axis.x, m00) && close(p.y_axis.y, (0.98 - 1.0 / 128.0) / 0.04));
        // Row-vector notation: z' = 0.5*z + 50, w' = z -> z_ndc = 0.5 + 50/z.
        let clip = p * Vec4::new(1.0, 2.0, 10_000.0, 1.0);
        assert!(close(clip.w, 10_000.0) && close(clip.z / clip.w, 0.505));
        assert!(close(clip.x / clip.w, m00 * 1.0 / 10_000.0));
    }

    #[test]
    fn light_view_looks_from_the_light_and_handles_a_vertical_light() {
        let center = Vec3::new(100.0, 0.0, 200.0);
        let light = Vec3::new(0.0, 1.0, -1.0).normalize();
        let eye = center + light * LIGHT_DISTANCE;
        let view = light_view(eye, center);
        let q = view.transform_point3(center);
        // Values around 10,000: tolerance of the floating-point computation 0.01.
        let near = |a: f32, b: f32| (a - b).abs() < 1e-2;
        assert!(
            near(q.x, 0.0) && near(q.y, 0.0) && near(q.z, LIGHT_DISTANCE),
            "{q:?}"
        );
        // Vertical light: no NaN, view still along -y.
        let view = light_view(center + Vec3::Y * LIGHT_DISTANCE, center);
        assert!(view.is_finite());
        let q = view.transform_point3(center + Vec3::new(10.0, 0.0, 0.0));
        assert!(close(q.z, LIGHT_DISTANCE) && close(q.x.abs(), 10.0));
    }

    #[test]
    fn shadow_frame_projects_the_box_and_finds_the_receiver_tiles() {
        // Cube 100^3 on the ground at (1000, 0, 1000), light from (1, 1, 0) (45 degrees).
        let corners: [Vec3; 8] = std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 != 0 { 50.0 } else { -50.0 },
                if i & 2 != 0 { 100.0 } else { 0.0 },
                if i & 4 != 0 { 50.0 } else { -50.0 },
            )
        });
        let world = Affine3A::from_translation(Vec3::new(1000.0, 0.0, 1000.0));
        let light = shadow_light_dir([1.0, 1.0, 0.0]);
        let frame = ShadowFrame::new(&corners, &world, light, 128, (128.0, 128.0));
        assert_eq!(frame.y_min, 0.0);
        assert!(close(frame.eye.y, 50.0 + LIGHT_DISTANCE / 2f32.sqrt()));
        // The silhouette fills +-(0.98 - 1/B) of the texture: the outermost corner lies at the edge.
        let mut extreme = 0.0f32;
        for c in &corners {
            let q = frame.view.transform_point3(world.transform_point3(*c));
            extreme = extreme.max((frame.projection.x_axis.x * q.x / q.z).abs());
        }
        assert!(close(extreme, 0.98 - 1.0 / 128.0), "{extreme}");
        // Shadow falls toward -x: ground points from x ~ 850 to 1050 -> tiles 6 ... 8, z 7 ... 8.
        assert_eq!(
            frame.tiles,
            Some(TileBox {
                x0: 6,
                x1: 8,
                z0: 7,
                z1: 8
            })
        );
        // The x axis of the light view lies horizontally along +z: the foot point of the center has
        // u = 0.5; it lies 50 below the hull center, so not in the image center (v != 0.5).
        let [u, v] = frame.texture_coord(Vec3::new(1000.0, 0.0, 1000.0));
        assert!(close(u, 0.5) && v > 0.5 && v < 1.0, "{u} {v}");
        // A slope below does not change the coordinate (pushed vertically onto y_min).
        assert_eq!(
            frame.texture_coord(Vec3::new(1000.0, 77.0, 1000.0)),
            frame.texture_coord(Vec3::new(1000.0, 0.0, 1000.0))
        );
    }

    #[test]
    fn huge_decal_hulls_and_receivers_stay_inside_the_map() {
        let (terrain, _) = test_terrain(4, |_, _| 1);
        let mask = TileMask::all(4, 4);
        // Huge hull: tile range saturates; only the 16 tiles of the map count.
        let hull = DecalHull {
            center: Vec3::ZERO,
            half: Vec3::new(1e30, 1.0, 1e30),
        };
        let mut out = Vec::new();
        let tiles = push_decal(
            &terrain,
            &mask,
            &hull,
            Vec3::new(256.0, 0.0, 256.0),
            0.0,
            DecalColor::Fixed(SHADOW_DECAL_COLOR),
            None,
            &mut out,
        );
        assert_eq!((tiles, out.len()), (16, 96));
        // Receiver with saturated range: likewise (the frame itself drops out at the span).
        let frame = ShadowFrame {
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            eye: Vec3::ZERO,
            y_min: 0.0,
            tiles: Some(TileBox {
                x0: i64::MIN,
                x1: i64::MAX,
                z0: i64::MIN,
                z1: i64::MAX,
            }),
        };
        let mut receivers = Vec::new();
        assert_eq!(
            push_receiver(&terrain, &mask, &frame, false, &mut receivers),
            16
        );
        assert_eq!(receivers.len(), 96);
    }

    #[test]
    fn a_zero_light_vector_falls_back_to_the_vertical_light() {
        assert_eq!(shadow_light_dir([0.0, 0.0, 0.0]), Vec3::Y);
        assert_eq!(shadow_light_dir([f32::NAN, 0.0, 0.0]), Vec3::Y);
        let corners: [Vec3; 8] = std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 != 0 { 10.0 } else { -10.0 },
                if i & 2 != 0 { 10.0 } else { 0.0 },
                if i & 4 != 0 { 10.0 } else { -10.0 },
            )
        });
        let world = Affine3A::from_translation(Vec3::new(192.0, 0.0, 192.0));
        let zero = ShadowFrame::new(&corners, &world, Vec3::ZERO, 64, (128.0, 128.0));
        let vertical = ShadowFrame::new(&corners, &world, Vec3::Y, 64, (128.0, 128.0));
        assert_eq!(zero, vertical);
        assert!(zero.view.to_cols_array().iter().all(|v| v.is_finite()));
        assert!(
            zero.projection
                .to_cols_array()
                .iter()
                .all(|v| v.is_finite())
        );
        let [u, v] = zero.texture_coord(Vec3::new(192.0, 0.0, 192.0));
        assert!(u.is_finite() && v.is_finite());
    }

    #[test]
    fn tile_boxes_clamp_to_the_map_and_check_the_span_without_overflow() {
        let huge = TileBox {
            x0: i64::MIN,
            x1: i64::MAX,
            z0: i64::MIN,
            z1: i64::MAX,
        };
        assert_eq!(huge.tiles_in(3, 2).count(), 6);
        assert!(!huge.span_within_limit());
        let tiles: Vec<_> = TileBox {
            x0: -2,
            x1: 1,
            z0: 1,
            z1: 5,
        }
        .tiles_in(4, 3)
        .collect();
        assert_eq!(tiles, [(0, 1), (0, 2), (1, 1), (1, 2)]);
        let outside = TileBox {
            x0: 7,
            x1: 9,
            z0: 0,
            z1: 1,
        };
        assert_eq!(outside.tiles_in(4, 4).count(), 0);
        let edge = TileBox {
            x0: 0,
            x1: MAX_TILE_SPAN,
            z0: 5,
            z1: 5,
        };
        assert!(edge.span_within_limit());
        let beyond = TileBox {
            x1: MAX_TILE_SPAN + 1,
            ..edge
        };
        assert!(!beyond.span_within_limit());
        let empty = TileBox {
            x0: 0,
            x1: 0,
            z0: 0,
            z1: 0,
        };
        assert_eq!(empty.tiles_in(0, 0).count(), 0);
    }

    #[test]
    fn wide_shadows_are_dropped_and_receivers_can_float_on_water() {
        // Flat, 3000-wide box: span 23 tiles > 20 -> nothing.
        let wide: [Vec3; 8] = std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 != 0 { 1500.0 } else { -1500.0 },
                if i & 2 != 0 { 10.0 } else { 0.0 },
                if i & 4 != 0 { 10.0 } else { -10.0 },
            )
        });
        let world = Affine3A::from_translation(Vec3::new(4000.0, 0.0, 4000.0));
        let frame = ShadowFrame::new(&wide, &world, Vec3::Y, 128, (128.0, 128.0));
        assert_eq!(frame.tiles, None);
        let (mut terrain, _) = test_terrain(4, |_, _| 1);
        terrain.params.water_level = 20.0;
        let mut out = Vec::new();
        assert_eq!(
            push_receiver(&terrain, &TileMask::all(4, 4), &frame, true, &mut out),
            0
        );
        // Small box in the middle of the map, light vertical: one tile, raised to 20.
        let small: [Vec3; 8] = std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 != 0 { 10.0 } else { -10.0 },
                if i & 2 != 0 { 10.0 } else { 0.0 },
                if i & 4 != 0 { 10.0 } else { -10.0 },
            )
        });
        let world = Affine3A::from_translation(Vec3::new(192.0, 0.0, 192.0));
        let frame = ShadowFrame::new(&small, &world, Vec3::Y, 64, (128.0, 128.0));
        assert_eq!(
            frame.tiles,
            Some(TileBox {
                x0: 1,
                x1: 1,
                z0: 1,
                z1: 1
            })
        );
        assert_eq!(
            push_receiver(&terrain, &TileMask::all(4, 4), &frame, true, &mut out),
            1
        );
        assert!(out.iter().all(|v| v.position[1] == 20.0));
        out.clear();
        push_receiver(&terrain, &TileMask::all(4, 4), &frame, false, &mut out);
        assert!(out.iter().all(|v| v.position[1] == 0.0));
        // The center point lies in the texture center (light vertical).
        let [u, v] = frame.texture_coord(Vec3::new(192.0, 0.0, 192.0));
        assert!(close(u, 0.5) && close(v, 0.5));
    }
}
