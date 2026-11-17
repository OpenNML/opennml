//! Ray test of an object like `ObjectRenderer_PickScreenPoint` (`0x0054AAF0`, called via
//! `FUN_0046CEA0` from the click selection `0x0047A210` and `Input_PickTargetAction` `0x00480E80`).
//! Without a GPU (testable).
//!
//! Flow per object with the shared distances `d1`/`d2` of the caller (start 1e7):
//! 1. Helper node `hitbox` (`ModelInstance_GetHelperNodeBox` `0x0058E790`): if the ray hits the
//!    box, the object counts as hit **without a distance comparison** (`d1` = world distance).
//!    Without a hit, the test ends at `MID_PROP_GLOBAL_ONLY_HITBOX` (category `0x100000`).
//! 2. Hull sphere and hull box (`ModelInstance_PickRaySphere` `0x0058C040`, `…PickRayBox`
//!    `0x0058C440`) in model space, both from the overall hull of the instance
//!    (`ModelInstance_ComputeBoundingBox` `0x0058AC40` with "animated" = animation running,
//!    `Instance+0x2C`; the 9th argument `0` at `0x0054ACE5` never selects the per-mesh branch): with
//!    a running animation, skin meshes via `MAXBOUNDINGBOX` of the animation. For non-units they are
//!    only a pre-filter, for units **the** hit surface.
//! 3. Non-units and `MID_PROP_GLOBAL_POLYGONCHECK` (category `0x80000`): triangles
//!    (`ModelInstance_PickRayTriangles` `0x0058C760`); accepted only if `t < d1`, then `d2 = 0`.
//!    Quirks: the **first** hit triangle in mesh/triangle order counts (not the nearest), `t` is
//!    measured in the space of the mesh (ray converted back via the inverse world and node matrix
//!    and normalized there, so in model units with `OBJ_SCALE`), front faces only, the texture
//!    alpha does not count (gaps in foliage hit), `t` is not checked for `≥ 0`
//!    (`Ray_IntersectTriangle` `0x00577440`, determinant from `1e-4`).
//! 4. Units (hit surface = sphere and box from 2.): intersection `P` of the ray with the plane
//!    through the hull center `C` perpendicular to the view direction (`Ray_IntersectPlane`
//!    `0x00576F60`), `e = |C − P|`, `t = |camera − P|`; with `d2 > 0` the smaller `e` wins, with
//!    `d2 ≤ 0` (the last hit was a triangle) a `t ≤ d1`. `C` = object position + center of the overall hull
//!    (`ObjectRenderer_GetBoundsCenter(…, 1)`, `0x0054AE75…0x0054AE8F`), added **unrotated and
//!    unscaled**.
//!
//! Simplifications: the monotonic growth of the animated hull until the next animation start
//! (`Instance+0x110`) is missing; the `hitbox` box sits at the rest position of the node.

use glam::{Affine3A, Vec3};
use nml_formats::rdo::{MeshKind, RdoMesh};
use nml_world::anim::{MeshPose, Pose};
use nml_world::assets::ModelAsset;
use nml_world::camera::ray_sphere;
use nml_world::math::{helper_node_matrix, mesh_node_matrix};
use nml_world::shadows::Aabb;

/// Start value of both distances (`0x4B189680` = 1e7).
pub const PICK_START: f32 = 1e7;

/// Name of the helper node with the hit box.
const HITBOX: &str = "hitbox";

/// View ray: camera position, normalized direction through the screen point and view direction
/// of the camera (for the unit rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickRay {
    pub origin: Vec3,
    pub direction: Vec3,
    pub forward: Vec3,
}

/// Check levels of an object (`FUN_0046CEA0`): bit 0 triangles (class `0x800FF` or no module 0),
/// bit 2 only `hitbox` (class `0x1000FF`). The game never sets bit 1 (without box).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PickFlags {
    pub triangles: bool,
    pub only_hitbox: bool,
}

/// Shared distances of the click selection (`local_14`/`local_18` in `0x0047A210`). They are not
/// reset across the ranks; this is not observable because they change only on a hit and a hit
/// ends the rank loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickDistances {
    pub d1: f32,
    pub d2: f32,
}

impl Default for PickDistances {
    fn default() -> Self {
        Self {
            d1: PICK_START,
            d2: PICK_START,
        }
    }
}

/// A part of an object: the main model (first) or a drawn sub-object, in the space of the
/// carrier, with its pose (`None` = bind pose).
#[derive(Clone, Copy)]
pub struct PickPart<'a> {
    pub asset: &'a ModelAsset,
    pub pose: Option<&'a Pose>,
}

/// Ray into the space of `m` (`Matrix_InverseAffine`, direction normalized there); `None` for a
/// non-invertible matrix.
fn local_ray(m: &Affine3A, origin: Vec3, direction: Vec3) -> Option<(Vec3, Vec3)> {
    if m.matrix3.determinant() == 0.0 {
        return None;
    }
    let inv = m.inverse();
    let o = inv.transform_point3(origin);
    let d = inv.transform_vector3(direction).normalize_or_zero();
    (d != Vec3::ZERO && o.is_finite() && d.is_finite()).then_some((o, d))
}

/// `Ray_IntersectTriangle` (`0x00577440`, as in the DirectX sample "Pick"): distance `t` along
/// `d`; determinant at least `1e-4` in absolute value, `t` unchecked (also negative).
fn ray_triangle(o: Vec3, d: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let p = d.cross(e2);
    let mut det = e1.dot(p);
    let tvec = if det > 0.0 {
        o - v0
    } else {
        det = -det;
        v0 - o
    };
    // `0.0001 <= det` (NaN falls through).
    if det.partial_cmp(&1e-4).is_none_or(|o| o.is_lt()) {
        return None;
    }
    let u = tvec.dot(p);
    if !(0.0..=det).contains(&u) {
        return None;
    }
    let q = tvec.cross(e1);
    let v = d.dot(q);
    if !(v >= 0.0 && u + v <= det) {
        return None;
    }
    Some(e2.dot(q) / det)
}

/// Node matrix of a mesh for the triangle test: skin meshes lie in object space (bind pose,
/// `0xBB9`), rigid ones with their node track or rest matrix.
fn mesh_matrix(asset: &ModelAsset, pose: Option<&Pose>, index: usize, mesh: &RdoMesh) -> Affine3A {
    if mesh.kind == MeshKind::Skin {
        return Affine3A::IDENTITY;
    }
    match pose.and_then(|p| p.meshes.get(index).map(|m| (p, m))) {
        Some((p, MeshPose::Node { .. })) => p.mesh_matrix(asset, index),
        _ => mesh_node_matrix(mesh),
    }
}

/// `ModelInstance_PickRayTriangles` (`0x0058C760`) over all parts in order, ray in model space:
/// the **first** hit front-facing triangle (`dot(−normal, direction) ≥ 0`) with its `t` in the
/// space of its mesh. Helper nodes are skipped.
pub fn ray_triangles(parts: &[PickPart], origin: Vec3, direction: Vec3) -> Option<f32> {
    for part in parts {
        let asset = part.asset;
        for (index, mesh) in asset.rdo.meshes.iter().enumerate() {
            if mesh.kind == MeshKind::Helper || mesh.faces.is_empty() {
                continue;
            }
            let m = mesh_matrix(asset, part.pose, index, mesh);
            let Some((o, d)) = (if m == Affine3A::IDENTITY {
                Some((origin, direction))
            } else {
                local_ray(&m, origin, direction)
            }) else {
                continue;
            };
            for (f, face) in mesh.faces.iter().enumerate() {
                let Some(n) = mesh.face_normals.get(f) else {
                    continue;
                };
                // `0 <= dot(−normal, direction)`: front faces only (NaN falls through).
                let facing = (-Vec3::from(*n)).dot(d);
                if facing.partial_cmp(&0.0).is_none_or(|o| o.is_lt()) {
                    continue;
                }
                let vertex = |k: usize| {
                    mesh.positions
                        .get(face.vertices[k] as usize)
                        .map(|&p| Vec3::from(p))
                };
                let (Some(v0), Some(v1), Some(v2)) = (vertex(0), vertex(1), vertex(2)) else {
                    continue;
                };
                if let Some(t) = ray_triangle(o, d, v0, v1, v2) {
                    return Some(t);
                }
            }
        }
    }
    None
}

/// Entry into the box `[min, max]` (ray in the space of the box): `t ≥ 0` of the first front
/// face. Same as `FUN_005776B0` for boxes **in front of** the camera; the original intersects the
/// plane of the first entry side without a sign check (`0x00576F60`) and takes `|P − camera|`,
/// so it also hits boxes behind or around the camera - without effect with the game camera.
fn ray_box_entry(o: Vec3, d: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut enter = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;
    for k in 0..3 {
        if d[k] == 0.0 {
            if o[k] < min[k] || o[k] > max[k] {
                return None;
            }
            continue;
        }
        let (a, b) = ((min[k] - o[k]) / d[k], (max[k] - o[k]) / d[k]);
        enter = enter.max(a.min(b));
        exit = exit.min(a.max(b));
    }
    (enter <= exit && enter >= 0.0).then_some(enter)
}

/// Hull sphere in model space: center and half diagonal of the overall hull (`0x0058C1C0…`).
fn hull_sphere(hull: &Aabb, o: Vec3, d: Vec3) -> bool {
    ray_sphere(o, d, hull.center(), hull.half_extent().length()).is_some()
}

/// Does the main model have a helper node `hitbox`?
pub fn has_hitbox(asset: &ModelAsset) -> bool {
    asset
        .rdo
        .meshes
        .iter()
        .any(|m| m.kind == MeshKind::Helper && m.name == HITBOX)
}

/// Box of the helper node `hitbox` of the main model (`ModelInstance_GetHelperNodeBox`): the
/// stored hull of the node with node scale, rotation and position (`Vector3_ApplyNodeTransform`
/// `0x00578180` with `mesh+0x12C`, [`helper_node_matrix`], D-367) and then the world matrix of the
/// object (`Vector3_TransformCoord` per corner, `0x0054AB76…`). The stored corners are a cube for
/// all 74 models; only the scale turns it into the flat or wide box of the building. Result:
/// `None` without a node, otherwise the hit (world distance, hit point).
pub fn ray_hitbox(
    asset: &ModelAsset,
    world: &Affine3A,
    ray: &PickRay,
) -> Option<Option<(f32, Vec3)>> {
    let mesh = asset
        .rdo
        .meshes
        .iter()
        .find(|m| m.kind == MeshKind::Helper && m.name == HITBOX)?;
    let m = *world * helper_node_matrix(mesh);
    let hit = local_ray(&m, ray.origin, ray.direction).and_then(|(o, d)| {
        let t = ray_box_entry(
            o,
            d,
            Vec3::from(mesh.bounds.min),
            Vec3::from(mesh.bounds.max),
        )?;
        let p = m.transform_point3(o + d * t);
        Some(((p - ray.origin).length(), p))
    });
    Some(hit)
}

/// `ObjectRenderer_PickScreenPoint` (`0x0054AAF0`) for an object with world matrix `world`, its
/// parts (`parts[0]` = main model) and its overall hull `hull` in model space
/// (`ModelInstance_ComputeBoundingBox` with "animated" = animation running, see
/// `nml_world::shadows::instance_box`); `rest_center` supplies the center of the rest hull on
/// demand (`GetBoundsCenter(…, 0)`, only after a hitbox hit). `true` = hit, then `d` is
/// updated.
pub fn pick_object(
    parts: &[PickPart],
    world: &Affine3A,
    hull: &Aabb,
    rest_center: impl FnOnce() -> Vec3,
    ray: &PickRay,
    flags: PickFlags,
    d: &mut PickDistances,
) -> bool {
    let Some(main) = parts.first() else {
        return false;
    };
    let position = Vec3::from(world.translation);
    if let Some(hitbox) = ray_hitbox(main.asset, world, ray) {
        if let Some((t, p)) = hitbox {
            // `0x0054AC38…0x0054AC7A`: `d2 = |(hit − position) + GetBoundsCenter(…, 0)|`.
            d.d1 = t;
            d.d2 = ((p - position) + rest_center()).length();
            return true;
        }
        if flags.only_hitbox {
            return false;
        }
    }
    let Some((o, dir)) = local_ray(world, ray.origin, ray.direction) else {
        return false;
    };
    if !hull_sphere(hull, o, dir) || ray_box_entry(o, dir, hull.min, hull.max).is_none() {
        return false;
    }
    if flags.triangles {
        let Some(t) = ray_triangles(parts, o, dir) else {
            return false;
        };
        if d.d1 <= t {
            return false;
        }
        d.d1 = t;
        d.d2 = 0.0;
        return true;
    }
    // Unit: plane through the hull center (position + center, unrotated, unscaled)
    // perpendicular to the view direction.
    let center = position + hull.center();
    let denom = ray.direction.dot(ray.forward);
    if denom == 0.0 {
        // Unreachable for screen rays (direction and view direction point forward). In the
        // original, `0x00576F60` fails only for `dot < 0`; then the old values would apply, `e = 0`
        // and `t` = distance from the sphere/box test.
        return false;
    }
    let s = (center - ray.origin).dot(ray.forward) / denom;
    let p = ray.origin + ray.direction * s;
    let e = (center - p).length();
    let t = (p - ray.origin).length();
    if d.d2 <= 0.0 {
        if d.d1 < t {
            return false;
        }
    } else if d.d2 <= e {
        return false;
    }
    d.d2 = e;
    d.d1 = t;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::rdo::{Face, RdoFile, RdoLayout};
    use nml_formats::testutil::bounds;

    /// Box `[min, max]` of twelve triangles with outward normals (rigid mesh, node at the
    /// origin).
    fn cube_mesh(name: &str, min: Vec3, max: Vec3) -> RdoMesh {
        let c = |i: usize| {
            Vec3::new(
                if i & 1 != 0 { max.x } else { min.x },
                if i & 2 != 0 { max.y } else { min.y },
                if i & 4 != 0 { max.z } else { min.z },
            )
        };
        let positions: Vec<[f32; 3]> = (0..8).map(|i| c(i).to_array()).collect();
        // Sides as (corners, outward normal).
        let sides: [([usize; 4], Vec3); 6] = [
            ([0, 2, 6, 4], -Vec3::X),
            ([1, 5, 7, 3], Vec3::X),
            ([0, 4, 5, 1], -Vec3::Y),
            ([2, 3, 7, 6], Vec3::Y),
            ([0, 1, 3, 2], -Vec3::Z),
            ([4, 6, 7, 5], Vec3::Z),
        ];
        let mut faces = Vec::new();
        let mut normals = Vec::new();
        for (q, n) in sides {
            for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                faces.push(Face {
                    vertices: tri.map(|v| v as u32),
                    material: 0,
                });
                normals.push(n.to_array());
            }
        }
        RdoMesh {
            kind: MeshKind::Rigid,
            type_id: 3002,
            name: name.into(),
            pivot: [0.0; 3],
            position: [0.0; 3],
            rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
            scale: [1.0; 3],
            positions,
            faces,
            uvs: vec![],
            uv_faces: None,
            face_normals: normals,
            vertex_normals: vec![[0.0; 3]; 8],
            bounds: bounds(min.to_array(), max.to_array()),
        }
    }

    fn model(meshes: Vec<RdoMesh>, min: Vec3, max: Vec3) -> ModelAsset {
        ModelAsset {
            path: "x.rdo".into(),
            rdo: RdoFile {
                layout: RdoLayout::Standard,
                bounds: bounds(min.to_array(), max.to_array()),
                meshes,
                bones: vec![],
                materials: vec![],
            },
            config: Default::default(),
            animations: Vec::new(),
            material_textures: Vec::new(),
            bone_track_cache: Default::default(),
        }
    }

    /// House: flat body 100 x 40 x 100 around the origin, hull up to y = 200 (like a building
    /// whose hull sphere encloses the airspace above the roof).
    fn house() -> ModelAsset {
        let body = cube_mesh(
            "koerper",
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(50.0, 40.0, 50.0),
        );
        model(
            vec![body],
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(50.0, 200.0, 50.0),
        )
    }

    /// Tree: trunk box 20 x 150 x 20.
    fn tree() -> ModelAsset {
        let trunk = cube_mesh(
            "trunk",
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 150.0, 10.0),
        );
        model(
            vec![trunk],
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 150.0, 10.0),
        )
    }

    /// Ray test with the rest hull of the main model (header hull of the `.rdo`) as the overall hull.
    fn pick(
        parts: &[PickPart],
        world: &Affine3A,
        ray: &PickRay,
        flags: PickFlags,
        d: &mut PickDistances,
    ) -> bool {
        let b = &parts[0].asset.rdo.bounds;
        let hull = Aabb {
            min: Vec3::from(b.min),
            max: Vec3::from(b.max),
        };
        pick_object(parts, world, &hull, || hull.center(), ray, flags, d)
    }

    /// Camera at z = -1000 looking toward +z.
    fn ray_at(x: f32, y: f32) -> PickRay {
        PickRay {
            origin: Vec3::new(x, y, -1000.0),
            direction: Vec3::Z,
            forward: Vec3::Z,
        }
    }

    const BUILDING: PickFlags = PickFlags {
        triangles: true,
        only_hitbox: false,
    };
    const UNIT: PickFlags = PickFlags {
        triangles: false,
        only_hitbox: false,
    };

    #[test]
    fn triangles_decide_between_a_roof_and_a_tree_behind_it() {
        let (house, tree) = (house(), tree());
        let at = |z: f32| Affine3A::from_translation(Vec3::new(0.0, 0.0, z));
        let objects = [(&house, at(0.0)), (&tree, at(200.0))];
        // Rank loop: in list order, shared distances, the last hit wins.
        let pick = |ray: PickRay| {
            let mut d = PickDistances::default();
            let mut hit = None;
            for (k, (asset, world)) in objects.iter().enumerate() {
                let parts = [PickPart { asset, pose: None }];
                if pick(&parts, world, &ray, BUILDING, &mut d) {
                    hit = Some(k);
                }
            }
            (hit, d)
        };
        // Above the roof (y = 100): the hull sphere of the house is hit, its triangles are not
        // - the tree behind it wins.
        let (hit, d) = pick(ray_at(0.0, 100.0));
        assert_eq!(hit, Some(1));
        assert!((d.d1 - 1190.0).abs() < 1e-3 && d.d2 == 0.0, "{d:?}");
        // Onto the roof (y = 20): the house is nearer, the tree behind it no longer.
        let (hit, d) = pick(ray_at(0.0, 20.0));
        assert_eq!(hit, Some(0));
        assert!((d.d1 - 950.0).abs() < 1e-3, "{d:?}");
        // Beside it: nothing.
        assert_eq!(pick(ray_at(300.0, 20.0)).0, None);
    }

    #[test]
    fn triangle_distances_are_measured_in_model_space_and_only_front_faces_count() {
        let house = house();
        let parts = [PickPart {
            asset: &house,
            pose: None,
        }];
        // OBJ_SCALE 2: world distance of the front face 900, in model space 450.
        let world = Affine3A::from_scale(Vec3::splat(2.0));
        let mut d = PickDistances::default();
        assert!(pick(&parts, &world, &ray_at(0.0, 20.0), BUILDING, &mut d));
        assert!((d.d1 - 450.0).abs() < 1e-3, "{d:?}");
        // Not nearer than the previous hit: rejected, the distances stay.
        let mut near = PickDistances { d1: 400.0, d2: 0.0 };
        assert!(!pick(
            &parts,
            &world,
            &ray_at(0.0, 20.0),
            BUILDING,
            &mut near
        ));
        assert_eq!(near, PickDistances { d1: 400.0, d2: 0.0 });
        // Camera inside the house: in front of it only back faces; the front face behind it hits
        // with a negative `t` (unchecked, like `Ray_IntersectTriangle`).
        let t = ray_triangles(&parts, Vec3::new(0.0, 20.0, 0.0), Vec3::Z).unwrap();
        assert!((t + 50.0).abs() < 1e-3, "{t}");
    }

    #[test]
    fn the_first_hit_triangle_counts_not_the_nearest() {
        // Two walls one behind the other, the rear one (z = 50) first in the mesh list.
        let back = cube_mesh(
            "back",
            Vec3::new(-10.0, 0.0, 50.0),
            Vec3::new(10.0, 10.0, 51.0),
        );
        let front = cube_mesh(
            "front",
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 1.0),
        );
        let asset = model(
            vec![back, front],
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 51.0),
        );
        let parts = [PickPart {
            asset: &asset,
            pose: None,
        }];
        let t = ray_triangles(&parts, Vec3::new(0.0, 5.0, -100.0), Vec3::Z).unwrap();
        assert!((t - 150.0).abs() < 1e-3, "{t}");
    }

    #[test]
    fn units_win_by_the_distance_of_their_center_to_the_ray() {
        let unit = tree();
        let parts = [PickPart {
            asset: &unit,
            pose: None,
        }];
        let at = |x: f32, z: f32| Affine3A::from_translation(Vec3::new(x, 0.0, z));
        // Center (0, 75, z); ray at y = 75.
        let mut d = PickDistances::default();
        // Near, but the ray is 8 beside the center.
        assert!(pick(
            &parts,
            &at(8.0, 0.0),
            &ray_at(0.0, 75.0),
            UNIT,
            &mut d
        ));
        assert!(
            (d.d2 - 8.0).abs() < 1e-3 && (d.d1 - 1000.0).abs() < 1e-3,
            "{d:?}"
        );
        // Further back, ray 3 beside the center: wins despite the larger distance.
        assert!(pick(
            &parts,
            &at(-3.0, 300.0),
            &ray_at(0.0, 75.0),
            UNIT,
            &mut d
        ));
        assert!(
            (d.d2 - 3.0).abs() < 1e-3 && (d.d1 - 1300.0).abs() < 1e-3,
            "{d:?}"
        );
        // Even nearer to the camera, but 5 beside the center: loses.
        assert!(!pick(
            &parts,
            &at(5.0, -500.0),
            &ray_at(0.0, 75.0),
            UNIT,
            &mut d
        ));
        // Outside sphere and box: no hit.
        assert!(!pick(
            &parts,
            &at(50.0, 0.0),
            &ray_at(0.0, 75.0),
            UNIT,
            &mut d
        ));
        // After a triangle hit (d2 = 0) the distance counts: only nearer units.
        let mut after = PickDistances { d1: 900.0, d2: 0.0 };
        assert!(!pick(
            &parts,
            &at(0.0, 0.0),
            &ray_at(0.0, 75.0),
            UNIT,
            &mut after
        ));
        let mut after = PickDistances {
            d1: 1100.0,
            d2: 0.0,
        };
        assert!(pick(
            &parts,
            &at(0.0, 0.0),
            &ray_at(0.0, 75.0),
            UNIT,
            &mut after
        ));
    }

    #[test]
    fn units_are_hit_through_the_given_hull_with_an_unrotated_center() {
        let unit = tree();
        let parts = [PickPart {
            asset: &unit,
            pose: None,
        }];
        let hull = |half_x: f32, z: f32| Aabb {
            min: Vec3::new(-half_x, 0.0, z - 10.0),
            max: Vec3::new(half_x, 150.0, z + 10.0),
        };
        let at = Affine3A::from_translation(Vec3::new(0.0, 0.0, 0.0));
        let hit = |hull: Aabb, world: &Affine3A, x: f32| {
            let mut d = PickDistances::default();
            let ok = pick_object(
                &parts,
                world,
                &hull,
                || hull.center(),
                &ray_at(x, 75.0),
                UNIT,
                &mut d,
            );
            ok.then_some(d)
        };
        // Rest hull +-50 hits at x = 40, the narrower hull of the running animation (+-25) does not.
        assert!(hit(hull(50.0, 0.0), &at, 40.0).is_some());
        assert!(hit(hull(25.0, 0.0), &at, 40.0).is_none());
        // Rotated by 90 degrees with center offset z = -8: the center enters unrotated (position + (0, 75, -8)),
        // so the lateral distance to the ray at x = -3 is 3; rotated, it would lie at x = -8 (distance 5).
        let turned = Affine3A::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let d = hit(hull(25.0, -8.0), &turned, -3.0).unwrap();
        assert!((d.d2 - 3.0).abs() < 1e-3, "{d:?}");
    }

    #[test]
    fn hitboxes_win_without_a_distance_check() {
        // House with helper node `hitbox` (cube +-20 around (0, 20, 0)).
        let mut house = house();
        let mut node = cube_mesh(
            "hitbox",
            Vec3::new(-20.0, -20.0, -20.0),
            Vec3::new(20.0, 20.0, 20.0),
        );
        node.kind = MeshKind::Helper;
        node.type_id = 3005;
        node.faces.clear();
        node.position = [0.0, 20.0, 0.0];
        house.rdo.meshes.push(node);
        let parts = [PickPart {
            asset: &house,
            pose: None,
        }];
        let world = Affine3A::from_translation(Vec3::new(0.0, 0.0, 500.0));
        // A nearer hit (d1 = 10) does not prevent the hitbox hit.
        let mut d = PickDistances { d1: 10.0, d2: 0.0 };
        assert!(pick(&parts, &world, &ray_at(0.0, 20.0), BUILDING, &mut d));
        assert!((d.d1 - 1480.0).abs() < 1e-3, "{d:?}");
        // d2 = |(hit point (0, 20, 480) - position (0, 0, 500)) + rest hull center (0, 100, 0)|.
        assert!((d.d2 - 120.0f32.hypot(20.0)).abs() < 1e-2, "{d:?}");
        // Beside the hitbox (y = 35) the triangles decide ...
        let mut d = PickDistances::default();
        assert!(pick(&parts, &world, &ray_at(45.0, 35.0), BUILDING, &mut d));
        assert!((d.d1 - 1450.0).abs() < 1e-3 && d.d2 == 0.0, "{d:?}");
        // ... except with ONLY_HITBOX.
        let only = PickFlags {
            only_hitbox: true,
            ..BUILDING
        };
        let mut d = PickDistances::default();
        assert!(!pick(&parts, &world, &ray_at(45.0, 35.0), only, &mut d));
        assert_eq!(d, PickDistances::default());
        assert!(pick(&parts, &world, &ray_at(0.0, 20.0), only, &mut d));
        // Without a hitbox node, ONLY_HITBOX has no effect.
        let plain = self::house();
        let parts = [PickPart {
            asset: &plain,
            pose: None,
        }];
        let mut d = PickDistances::default();
        assert!(pick(&parts, &world, &ray_at(45.0, 35.0), only, &mut d));
    }

    #[test]
    fn hitboxes_apply_the_node_scale() {
        // Helper node `hitbox`: stored as a cube +-20 around (0, 20, 0), scale (2, 0.5, 1.5)
        // (`0x00578180`, as for all buildings in the shipped data) -> box x +-40, y 10 ... 30, z +-30.
        let mut house = house();
        let mut node = cube_mesh(
            "hitbox",
            Vec3::new(-20.0, -20.0, -20.0),
            Vec3::new(20.0, 20.0, 20.0),
        );
        node.kind = MeshKind::Helper;
        node.type_id = 3005;
        node.faces.clear();
        node.position = [0.0, 20.0, 0.0];
        node.scale = [2.0, 0.5, 1.5];
        house.rdo.meshes.push(node);
        let world = Affine3A::from_translation(Vec3::new(0.0, 0.0, 500.0));
        let hit = |x: f32, y: f32| ray_hitbox(&house, &world, &ray_at(x, y)).unwrap();
        // Wider than the stored cube (x = 30 > 20) ...
        let (t, p) = hit(30.0, 20.0).expect("x = 30 lies in the scaled box");
        // ... and deeper: front face at z = 500 - 30.
        assert!((t - 1470.0).abs() < 1e-3, "{t}");
        assert!((p - Vec3::new(30.0, 20.0, 470.0)).length() < 1e-3, "{p:?}");
        // Flatter: y = 35 was inside the cube (up to 40), but lies above the scaled box (up to 30).
        assert_eq!(hit(0.0, 35.0), None);
        assert_eq!(hit(45.0, 20.0), None);
    }

    #[test]
    fn ray_triangle_follows_the_directx_pick_sample() {
        let (v0, v1, v2) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        let t = ray_triangle(Vec3::new(0.2, 0.2, -5.0), Vec3::Z, v0, v1, v2).unwrap();
        assert!((t - 5.0).abs() < 1e-6);
        // Both winding directions; behind the origin negative (unchecked).
        let t = ray_triangle(Vec3::new(0.2, 0.2, 5.0), Vec3::Z, v0, v2, v1).unwrap();
        assert!((t + 5.0).abs() < 1e-6);
        assert!(ray_triangle(Vec3::new(0.8, 0.8, -5.0), Vec3::Z, v0, v1, v2).is_none());
        // Tiny triangles (determinant < 1e-4) never hit.
        let s = 0.005;
        assert!(ray_triangle(Vec3::new(0.001, 0.001, -5.0), Vec3::Z, v0, v1 * s, v2 * s).is_none());
    }
}
