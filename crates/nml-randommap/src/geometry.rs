//! Model geometry of the object step (re/random-map Z.7.3, ZK-b.4, D-356): the hulls with which the
//! generator snaps an object (`FUN_00542CF0`) and determines its footprint rectangle
//! (`GameObject_GetHelperBoxTileRect` `0x00446C80`, `ModelBounds_GetTileRect` `0x00446B10`), plus the
//! sphere radius of the border check.
//!
//! Everything in `f32` in the order of the original (no glam arithmetic): generator world matrix
//! [`nml_sim::footprint::generator_world_matrix`], corners via
//! [`nml_sim::footprint::transform_coord`] (`Vector3_TransformCoord` `0x00575250`).

use std::collections::BTreeMap;
use std::rc::Rc;

use nml_formats::object_cfg::{ObjectConfig, object_cfg_path, parse_object_cfg};
use nml_formats::rdo::{RdoFile, RdoMesh, parse_rdo};
use nml_formats::text::decode_cp1252;
use nml_rda::normalize_path;
use nml_sim::footprint::{Matrix, transform_coord};
use nml_sim::fx::ftol;
use nml_world::anim::MAX_SUB_DEPTH;

/// Point or vector `(x, y, z)`.
pub(crate) type Vec3 = [f32; 3];
/// Eight corners of a box.
pub(crate) type Corners = [Vec3; 8];
/// Axis-aligned box `(min, max)`.
pub(crate) type Aabb = (Vec3, Vec3);

/// Mesh type of the helper nodes (3005).
const HELPER_TYPE: i32 = 0xBBD;
/// Name of the helper node whose hull determines the snapping and the rectangle.
const BLOCKING01: &str = "blocking01";

/// Geometry of a model as the object step needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelGeometry {
    /// The 8 corners of the helper node `blocking01` in object space
    /// (`ModelInstance_GetHelperNodeBox` `0x0058E790`), `None` without the node.
    pub blocking01: Option<Corners>,
    /// Instance hull `ModelInstance_ComputeBoundingBox(·, 0)` (`0x0058AC40`) of a fresh,
    /// unanimated instance including sub-objects, as `(min, max)`.
    pub instance_box: Aabb,
    /// The 8 stored corners of the file hull `RdoBounds` (`Model+0x9FC`, `ModelBounds_GetTileRect`).
    pub file_box: Corners,
    /// Sphere radius `Model+0xA5C` = `RdoBounds.radius` of the file (not computed).
    pub radius: f32,
}

/// Loads the geometry of the model `model` via `read` (`.rdo`, object `.cfg` next to it,
/// sub-objects as in [`nml_world::assets::ModelCache::load`]). `None` if the model itself is
/// not loadable (D-355); unloadable sub-objects are dropped as in the original.
pub(crate) fn load_geometry(
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    model: &str,
) -> Option<ModelGeometry> {
    let mut models = Models {
        read,
        cache: BTreeMap::new(),
    };
    let root = models.get(model)?;
    Some(ModelGeometry {
        blocking01: blocking01_corners(&root.rdo),
        instance_box: instance_box(&root, 0, &mut models),
        file_box: root.rdo.bounds.corners,
        radius: root.rdo.bounds.radius,
    })
}

/// Hull for snapping (Z.7.3 (1), `FUN_00542CF0` `0x00542DA0…0x00542E10`): the 8 corners of
/// `blocking01`, otherwise of the instance hull (`Box_ToCorners8` `0x005796A0`), each through
/// `m` with `Vector3_TransformCoord`, then `Vector3x8_GetMinMax` (`0x004467C0`); not
/// shrunk.
pub(crate) fn snap_hull(geo: &ModelGeometry, m: &Matrix) -> Aabb {
    let corners = geo
        .blocking01
        .unwrap_or_else(|| to_corners8(&geo.instance_box));
    min_max(&corners.map(|c| transform_coord(c, m)))
}

/// Footprint rectangle `[x0, z0, x1, z1]` (inclusive) for `PutOnFlat`, occupancy, `SetRect`,
/// `GroundTexture` and `ClearRect` (Z.7.3 (7)): with `blocking01`
/// `GameObject_GetHelperBoxTileRect` (`0x00446C80`), otherwise the file hull via
/// `ModelBounds_GetTileRect` (`0x00446B10`, without the correction of negative values; D-356).
pub(crate) fn ground_rect(geo: &ModelGeometry, m: &Matrix, s: i32) -> [i32; 4] {
    match &geo.blocking01 {
        Some(corners) => tile_rect(&corners.map(|c| transform_coord(c, m)), s, true),
        None => tile_rect(&geo.file_box.map(|c| transform_coord(c, m)), s, false),
    }
}

/// Shared core of `0x00446C80` (`0x00446D1E…0x00446E79`) and `0x00446B10`
/// (`0x00446B7B…0x00446C4E`): min/max like `Vector3x8_GetMinMax`, `S8 = S / 8` (toward 0),
/// `x0 = S8 + min.x`, `x1 = max.x − S8` (f32), swapped when ordered `x0 > x1` (NaN does not swap),
/// each value `ftol(·) / S` (`IDIV`, toward 0). With `minus_one` (only `blocking01`) every value
/// whose `f32` before `ftol` is less than 0 is **decreased** by 1 (-16 -> 0 - 1 = -1, -200 -> -1 - 1 = -2;
/// `TEST AH,5; JP; DEC`).
fn tile_rect(corners: &Corners, s: i32, minus_one: bool) -> [i32; 4] {
    let (mn, mx) = min_max(corners);
    let s8 = s.wrapping_div(8) as f32;
    let (mut x0, mut x1) = (s8 + mn[0], mx[0] - s8);
    let (mut z0, mut z1) = (s8 + mn[2], mx[2] - s8);
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
    }
    if z0 > z1 {
        std::mem::swap(&mut z0, &mut z1);
    }
    [x0, z0, x1, z1].map(|v| {
        let tile = ftol(v).wrapping_div(s);
        if minus_one && v < 0.0 {
            tile.wrapping_sub(1)
        } else {
            tile
        }
    })
}

/// `Vector3x8_GetMinMax` (`0x004467C0`) or `Box_FromCorners8` (`0x00577BC0`): start with corner 0,
/// then per corner strictly `<` (min) or `>` (max); NaN and equal values (also `−0`/`+0`) change
/// nothing.
fn min_max(corners: &Corners) -> Aabb {
    let (mut mn, mut mx) = (corners[0], corners[0]);
    for c in corners {
        for k in 0..3 {
            if c[k] < mn[k] {
                mn[k] = c[k];
            }
        }
        for k in 0..3 {
            if c[k] > mx[k] {
                mx[k] = c[k];
            }
        }
    }
    (mn, mx)
}

/// `Box_ToCorners8` (`0x005796A0`): Ecke `i` = (`i & 1` → max.x, `i & 2` → max.y, `i & 4` → max.z).
fn to_corners8(b: &Aabb) -> Corners {
    let (mn, mx) = b;
    std::array::from_fn(|i| {
        [
            if i & 1 != 0 { mx[0] } else { mn[0] },
            if i & 2 != 0 { mx[1] } else { mn[1] },
            if i & 4 != 0 { mx[2] } else { mn[2] },
        ]
    })
}

/// `Box_Union(a, b, out)` (`0x005760E0`): copy of `a`, then per component `b.min < a.min` or
/// `b.max > a.max` (strict, NaN takes nothing). The order of the arguments matters for `−0`/`+0`.
fn union(a: &Aabb, b: &Aabb) -> Aabb {
    let (mut mn, mut mx) = *a;
    for (m, &v) in mn.iter_mut().zip(&b.0) {
        if v < *m {
            *m = v;
        }
    }
    for (m, &v) in mx.iter_mut().zip(&b.1) {
        if v > *m {
            *m = v;
        }
    }
    (mn, mx)
}

/// `ModelInstance_ComputeBoundingBox(·, 0)` (`0x0058AC40`) of a fresh instance of `model` at
/// sub-object depth `depth`:
///
/// ```text
/// acc = zero box; first = no
/// for each mesh i (file order), type != 0xBBD (skin 0xBB9 without animation like rigid):
///     tmp = Box_FromCorners8(GetMeshBoundingCorners(i, 0))   // 8 stored mesh corners (+0x170)
///     acc = first ? Box_Union(acc, tmp) : tmp; first = yes
/// if sub-objects (+0x24) are present: for each link of the object level (0x00583610),
///     sub-instance != 0 and IsSubObjectLinkEnabled (0x0058ABD0; fresh, without filter: yes):
///     tmp = Box_FromCorners8(ComputeBoundingBox(sub-instance, 0))   // recursive, 8 corners
///     acc = Box_Union(tmp, acc)                                    // argument order swapped
/// Result: Box_ToCorners8(acc)
/// ```
///
/// Without its own mesh the zero box stays in the union (D-356). Sub-objects of sub-objects
/// count too (up to [`MAX_SUB_DEPTH`] as in `nml_world::anim`; the original recurses without
/// limit).
fn instance_box(model: &Model, depth: usize, models: &mut Models<'_>) -> Aabb {
    let mut acc: Aabb = ([0.0; 3], [0.0; 3]);
    let mut first = true;
    for mesh in model.rdo.meshes.iter().filter(|m| m.type_id != HELPER_TYPE) {
        let tmp = min_max(&mesh.bounds.corners);
        acc = if first { tmp } else { union(&acc, &tmp) };
        first = false;
    }
    if depth < MAX_SUB_DEPTH {
        for link in &model.sub_objects {
            let Some(sub) = models.get(link) else {
                continue;
            };
            let tmp = min_max(&to_corners8(&instance_box(&sub, depth + 1, models)));
            acc = union(&tmp, &acc);
        }
    }
    acc
}

/// Corners of the first helper node (`0xBBD`) named `blocking01` (`ModelInstance_GetHelperNodeBox`
/// `0x0058E790`: search name lowercased, then `strcmp` against the stored name):
/// `D = GetHelperNodePosition − node(mittelpunkt)` (fresh instance: both `node(mittelpunkt)`,
/// so `D = +0`), corner `i` = `node(ecke_i) + D` in file order (`0x0058EAE1…0x0058EB0E`).
fn blocking01_corners(rdo: &RdoFile) -> Option<Corners> {
    let mesh = rdo
        .meshes
        .iter()
        .find(|m| m.type_id == HELPER_TYPE && m.name == BLOCKING01)?;
    // `ModelInstance_GetHelperNodePosition` (`0x0058DAB0`, unanimated) and `0x0058E9DF` both compute
    // `node(mittelpunkt)`; `D` is therefore `+0` (only NaN would stay NaN).
    let p = node_transform(mesh.bounds.center, mesh);
    let c = node_transform(mesh.bounds.center, mesh);
    let d: Vec3 = std::array::from_fn(|k| p[k] - c[k]);
    Some(mesh.bounds.corners.map(|corner| {
        let v = node_transform(corner, mesh);
        std::array::from_fn(|k| v[k] + d[k])
    }))
}

/// `Vector3_ApplyNodeTransform(v, t, q, s)` (`0x00578180`) with `t` = node position (`+0x110`),
/// `q` = rotation `(w, x, y, z)` (`+0x11C`), `s` = node scale (`+0x12C`, file value
/// [`RdoMesh::scale`]; the loader does not overwrite it, D-365): `v ∘ s` (`0x005A0910`),
/// vector part of `q⁻¹ · (0, v) · q` (`0x00576960`, twice `Quaternion_MultiplyOut`
/// `0x00577880`), then `+ t` (`0x005A0890`). With the identity rotation `(±1, 0, 0, 0)` (all
/// `blocking01` in the shipped data, D-356) this is exactly `v ∘ s + t`. The scale is != 1 in 79 of 110
/// models with `blocking01` (e.g. palme04...06 20/29) and, as in the original, affects
/// snapping ([`snap_hull`]) and footprint rectangle ([`ground_rect`]) equally.
fn node_transform(v: Vec3, mesh: &RdoMesh) -> Vec3 {
    let s = mesh.scale;
    let q = mesh.rotation_wxyz;
    let v: Vec3 = std::array::from_fn(|k| v[k] * s[k]);
    let p = [0.0, v[0], v[1], v[2]];
    let r = quat_mul(&quat_mul(&quat_inverse(&q), &p), &q);
    let t = mesh.position;
    std::array::from_fn(|k| r[k + 1] + t[k])
}

/// `Quaternion_Inverse` (`0x00576960`): `n = ((w² + x²) + y²) + z²`, conjugate (`0x00575450`),
/// then `r = 1 / n` (stored as `f32`) times each component (`0x00576400`).
fn quat_inverse(q: &[f32; 4]) -> [f32; 4] {
    let n = ((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3];
    let conj = [q[0], -q[1], -q[2], -q[3]];
    let r = 1.0 / n;
    conj.map(|c| r * c)
}

/// `FUN_00576840(out, a, b)` = `a · b` (Hamilton, `(w, x, y, z)`): `w = a.w·b.w − dot(a.v, b.v)`
/// with `dot` = `(z·z + y·y) + x·x` (`0x005750B0`), `v = (b.w·a.v + a.w·b.v) + a.v × b.v`
/// (`0x005762E0`, `0x005762B0`, cross product `0x005751F0`).
fn quat_mul(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    let (va, vb) = ([a[1], a[2], a[3]], [b[1], b[2], b[3]]);
    let dot = (va[2] * vb[2] + va[1] * vb[1]) + va[0] * vb[0];
    let w = a[0] * b[0] - dot;
    let cross = [
        vb[2] * va[1] - vb[1] * va[2],
        vb[0] * va[2] - vb[2] * va[0],
        vb[1] * va[0] - vb[0] * va[1],
    ];
    let v: Vec3 = std::array::from_fn(|k| (b[0] * va[k] + a[0] * vb[k]) + cross[k]);
    [w, v[0], v[1], v[2]]
}

/// A loaded model: `.rdo` and the links of the object level of its `.cfg`
/// (normalized paths, in file order, with repetitions).
struct Model {
    rdo: RdoFile,
    sub_objects: Vec<String>,
}

/// Models by normalized path, each read once (failures included).
struct Models<'a> {
    read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    cache: BTreeMap<String, Option<Rc<Model>>>,
}

impl Models<'_> {
    fn get(&mut self, path: &str) -> Option<Rc<Model>> {
        let key = normalize_path(path);
        if let Some(entry) = self.cache.get(&key) {
            return entry.clone();
        }
        let entry = match self.load(&key) {
            Ok(model) => Some(Rc::new(model)),
            Err(e) => {
                log::warn!("Model {key} cannot be loaded: {e:#}");
                None
            }
        };
        self.cache.insert(key, entry.clone());
        entry
    }

    /// Like `ModelCache::load`: `.rdo`, then the object `.cfg` next to it, if present (a
    /// faulty `.cfg` makes the model unloadable).
    fn load(&mut self, path: &str) -> anyhow::Result<Model> {
        use anyhow::Context;
        let bytes = (self.read)(path).with_context(|| format!("{path} is missing"))?;
        let rdo = parse_rdo(&bytes).with_context(|| format!("reading {path}"))?;
        let mut config = ObjectConfig::default();
        if let Some(cfg) = object_cfg_path(path)
            && let Some(text) = (self.read)(&cfg)
        {
            config = parse_object_cfg(&decode_cp1252(&text))
                .with_context(|| format!("reading {cfg}"))?;
        }
        Ok(Model {
            rdo,
            sub_objects: config
                .sub_objects
                .iter()
                .map(|s| normalize_path(&s.file))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests;
