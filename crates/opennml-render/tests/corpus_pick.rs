//! Ray test on real models (`opennml_render::pick`): the SID town hall with helper node `hitbox`
//! and a deciduous tree behind it. Skipped if the installation is missing.
//!
//! The ranks are in the game host (`crates/opennml-client/src/game/selection.rs`): for the right-click
//! (`Input_PickTargetAction` `0x00480E80`) rank 4 tests the town hall (module 3), rank 5 the tree;
//! the click target `0x0047A210` (left click, hover) never takes trees.

use glam::{Affine3A, Vec3};
use opennml_formats::rdo::MeshKind;
use opennml_rda::testutil::corpus_game_dir;
use opennml_rda::{Vfs, normalize_path};
use opennml_render::pick::{PickDistances, PickFlags, PickPart, PickRay, pick_object, ray_hitbox};
use opennml_world::anim::{Playback, idle_animation};
use opennml_world::assets::ModelCache;
use opennml_world::shadows::Aabb;
use opennml_world::shadows::{instance_box, mesh_box};

const RATHAUS: &str = "data/r3d/sid/buildings/summer/rathaus/rathaus.rdo";
const BAUM: &str = "data/r3d/nature/summer/trees/laubbaum01.rdo";

const POLYGONS: PickFlags = PickFlags {
    triangles: true,
    only_hitbox: false,
};

#[test]
fn a_right_click_ray_above_the_town_hall_roof_hits_the_tree_behind_it() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut models = ModelCache::new();
    let hall = models.get(&vfs, RATHAUS).unwrap();
    let tree = models.get(&vfs, BAUM).unwrap();
    // 74 models have a helper node `hitbox`, the town hall is one of them.
    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".rdo"))
        .collect();
    paths.sort();
    paths.dedup();
    let with_hitbox = paths
        .iter()
        .filter_map(|p| models.get(&vfs, p))
        .filter(|m| {
            m.rdo
                .meshes
                .iter()
                .any(|mesh| mesh.kind == MeshKind::Helper && mesh.name == "hitbox")
        })
        .count();
    assert_eq!(with_hitbox, 74);

    // Town hall at the origin, tree 700 behind it; camera obliquely from the front (45 degrees).
    let hall_world = Affine3A::IDENTITY;
    let tree_world = Affine3A::from_translation(Vec3::new(0.0, 0.0, 700.0));
    let forward = Vec3::new(0.0, -1.0, 1.0).normalize();
    let ray_to = |target: Vec3| {
        let origin = target - forward * 3000.0;
        PickRay {
            origin,
            direction: (target - origin).normalize(),
            forward,
        }
    };
    let pick = |asset, world: &Affine3A, ray: &PickRay, d: &mut PickDistances| {
        let hull = mesh_box(asset, None).unwrap();
        pick_object(
            &[PickPart { asset, pose: None }],
            world,
            &hull,
            || hull.center(),
            ray,
            POLYGONS,
            d,
        )
    };
    // Aimed at the tree: the ray runs over the roof, through the hull sphere of the town hall,
    // but hits neither its triangles nor the hitbox (rank 4 without a hit, `d1`/`d2`
    // unchanged) - the tree in rank 5 wins.
    let b = &tree.rdo.bounds;
    let aim = tree_world.transform_point3(Vec3::from(b.center));
    let ray = ray_to(aim);
    let hb = &hall.rdo.bounds;
    let (c, r) = (Vec3::from(hb.center), hb.radius);
    let closest = c - ray.origin - ray.direction * (c - ray.origin).dot(ray.direction);
    assert!(
        closest.length() < r,
        "ray misses the hull sphere: {closest:?}"
    );
    assert_eq!(ray_hitbox(&hall, &hall_world, &ray), Some(None));
    let mut d = PickDistances::default();
    assert!(!pick(&hall, &hall_world, &ray, &mut d));
    assert!(pick(&tree, &tree_world, &ray, &mut d));
    // Aimed at the center of the town hall: the hitbox hits, without a distance comparison.
    let ray = ray_to(c);
    let mut d = PickDistances { d1: 1.0, d2: 0.0 };
    assert!(matches!(
        ray_hitbox(&hall, &hall_world, &ray),
        Some(Some(_))
    ));
    assert!(pick(&hall, &hall_world, &ray, &mut d));
    assert!(d.d1 > 1.0);
}

/// Units are hit via the hull of the running animation (`ModelInstance_ComputeBoundingBox`
/// with "animated", skin meshes via `MAXBOUNDINGBOX`): the SID female settler in `frau_stehen01` stands
/// narrower (x +/-26) than her T-pose header hull (x +/-52). A ray at x = 40 therefore hits only without a
/// running animation.
#[test]
fn units_are_hit_through_the_hull_of_their_running_animation() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut models = ModelCache::new();
    let unit = models
        .get(&vfs, "data/r3d/sid/units/siedlerin/siedlerin.rdo")
        .unwrap();
    let idle = idle_animation(&unit);
    let playback = Playback::new(&unit, |f| models.get(&vfs, f), idle, 0.0);
    let pose = playback.player().pose(&unit, 0.0);
    let rest = instance_box(&unit, &playback, None, 0.0);
    let running = instance_box(&unit, &playback, Some(&pose), 0.0);
    assert!(rest.min.x < -50.0 && rest.max.x > 50.0, "{rest:?}");
    assert!(running.min.x > -27.0 && running.max.x < 27.0, "{running:?}");
    // Rotated by 90 degrees (position x = 1000): the hull center enters the unit rule unrotated.
    let world = Affine3A::from_rotation_translation(
        glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        Vec3::new(1000.0, 0.0, 0.0),
    );
    let flags = PickFlags::default();
    let ray = |x: f32| PickRay {
        origin: Vec3::new(1000.0 + x, 60.0, -2000.0),
        direction: Vec3::Z,
        forward: Vec3::Z,
    };
    let parts = [PickPart {
        asset: &unit,
        pose: Some(&pose),
    }];
    // Rotated, the header hull lies along z; across the x axis the T-pose at x = 40 is no
    // longer wide enough - hence an unrotated unit for the width comparison.
    let straight = Affine3A::from_translation(Vec3::new(1000.0, 0.0, 0.0));
    let hit = |hull: &Aabb, world: &Affine3A, x: f32, d: &mut PickDistances| {
        pick_object(&parts, world, hull, || rest.center(), &ray(x), flags, d)
    };
    assert!(hit(&rest, &straight, 40.0, &mut PickDistances::default()));
    assert!(!hit(
        &running,
        &straight,
        40.0,
        &mut PickDistances::default()
    ));
    let mut d = PickDistances::default();
    assert!(hit(&running, &straight, 5.0, &mut d));
    let c = running.center();
    // e = distance in the plane z = center: (5 - cx, 60 - cy).
    let e = |x: f32| (x - c.x).hypot(60.0 - c.y);
    assert!((d.d2 - e(5.0)).abs() < 1e-3, "{d:?} {c:?}");
    let mut d = PickDistances::default();
    assert!(hit(&running, &world, 0.0, &mut d));
    // Rotated, cz (-7.5) would otherwise count as lateral offset.
    assert!((d.d2 - e(0.0)).abs() < 1e-3, "{d:?} {c:?}");
}

/// All 74 helper nodes `hitbox` store their corners as a cube; the shape of the cuboid only arises
/// through the node scale of the `.rdo` (`RdoMesh::scale`), which `ModelInstance_GetHelperNodeBox`
/// `0x0058E790` passes to `Vector3_ApplyNodeTransform` `0x00578180`. Checked on the SPA finca (flat,
/// scale y 0.082) and the SID town hall (wide, scale x 2.09) against the same models with
/// scale 1, as the reimplementation computed before D-367.
// QUIRK D-367: kept
#[test]
fn hitboxes_take_the_node_scale_of_the_rdo() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut models = ModelCache::new();
    let unscaled = |asset: &opennml_world::assets::ModelAsset| {
        let mut rdo = asset.rdo.clone();
        for mesh in &mut rdo.meshes {
            if mesh.kind == MeshKind::Helper && mesh.name == "hitbox" {
                mesh.scale = [1.0; 3];
            }
        }
        // For the hitbox test only the `.rdo` counts.
        opennml_world::assets::ModelAsset {
            path: asset.path.clone(),
            rdo,
            config: Default::default(),
            animations: Vec::new(),
            material_textures: Vec::new(),
            bone_track_cache: Default::default(),
        }
    };
    let node = |asset: &opennml_world::assets::ModelAsset| {
        asset
            .rdo
            .meshes
            .iter()
            .find(|m| m.kind == MeshKind::Helper && m.name == "hitbox")
            .cloned()
            .unwrap()
    };
    // Horizontal ray from the front (camera at z = -3000) at height `y`, lateral position `x`.
    let ray = |x: f32, y: f32| PickRay {
        origin: Vec3::new(x, y, -3000.0),
        direction: Vec3::Z,
        forward: Vec3::Z,
    };
    let world = Affine3A::IDENTITY;

    // Finca: cube +/-364.6 around y = 30.9, scale (1, 0.082, 1) -> height 30.9 +/- 29.9.
    let finca = models
        .get(&vfs, "data/r3d/spa/buildings/summer/finca/finca_spa.rdo")
        .unwrap();
    let n = node(&finca);
    assert_eq!(n.scale, [1.0, 0.08196721, 1.0]);
    assert_eq!(n.bounds.max, [364.59653; 3]);
    let top = n.position[1] + n.bounds.max[1] * n.scale[1];
    assert!((top - 60.8).abs() < 0.1, "{top}");
    let flat = unscaled(&finca);
    // 100 above the ground: above the finca, inside the unscaled cube.
    assert_eq!(ray_hitbox(&finca, &world, &ray(0.0, 100.0)), Some(None));
    assert!(matches!(
        ray_hitbox(&flat, &world, &ray(0.0, 100.0)),
        Some(Some(_))
    ));
    // At half height both hit; the front face is the same (scale z = 1).
    let (Some(Some((t, _))), Some(Some((t1, _)))) = (
        ray_hitbox(&finca, &world, &ray(0.0, 30.0)),
        ray_hitbox(&flat, &world, &ray(0.0, 30.0)),
    ) else {
        panic!("finca not hit at half height");
    };
    assert!((t - t1).abs() < 1e-2, "{t} {t1}");

    // Town hall: cube +/-163.8 around x = 2.0, scale (2.09, 1.30, 1) -> x 2.0 +/- 341.6.
    let hall = models.get(&vfs, RATHAUS).unwrap();
    let n = node(&hall);
    assert_eq!(n.scale, [2.0857143, 1.298, 1.0]);
    let narrow = unscaled(&hall);
    let y = n.position[1];
    // 250 to the side: inside the scaled cuboid, next to the unscaled cube.
    assert!(matches!(
        ray_hitbox(&hall, &world, &ray(250.0, y)),
        Some(Some(_))
    ));
    assert_eq!(ray_hitbox(&narrow, &world, &ray(250.0, y)), Some(None));
    assert_eq!(ray_hitbox(&hall, &world, &ray(360.0, y)), Some(None));
}
