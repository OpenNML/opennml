use std::collections::BTreeMap;

use nml_formats::rdo::{Bounds, MeshKind, RdoFile, RdoLayout, RdoMesh};
use nml_formats::testutil::{bounds, sample_rdo, write_rdo};
use nml_sim::footprint::generator_world_matrix;

use super::*;

/// Mesh of a test model without geometry (for the hulls only the type and the hull count).
fn mesh(type_id: i32, name: &str, b: Bounds) -> RdoMesh {
    let template = sample_rdo(RdoLayout::Standard).meshes[2].clone();
    RdoMesh {
        kind: MeshKind::from_type_id(type_id).unwrap(),
        type_id,
        name: name.into(),
        bounds: b,
        ..template
    }
}

fn rdo(file_bounds: Bounds, meshes: Vec<RdoMesh>) -> Vec<u8> {
    let sample = sample_rdo(RdoLayout::Standard);
    write_rdo(&RdoFile {
        layout: RdoLayout::Standard,
        bounds: file_bounds,
        meshes,
        bones: Vec::new(),
        materials: sample.materials,
    })
}

fn cfg(subs: &[&str]) -> Vec<u8> {
    let mut text = String::from("OBJECT\r\n{\r\n");
    for s in subs {
        text.push_str(&format!(
            "\tSUBOBJECT\r\n\t{{\r\n\t\tFILE \"{s}\"\r\n\t}}\r\n"
        ));
    }
    text.push_str("}\r\n");
    text.into_bytes()
}

fn load(files: &BTreeMap<String, Vec<u8>>, model: &str) -> Option<ModelGeometry> {
    load_geometry(&mut |p| files.get(p).cloned(), model)
}

fn bits(b: &Aabb) -> [[u32; 3]; 2] {
    [b.0.map(f32::to_bits), b.1.map(f32::to_bits)]
}

/// Geometry from a hull (-h ... h) for the rectangle and snapping tests.
fn box_geometry(h: f32, blocking01: bool) -> ModelGeometry {
    let corners = to_corners8(&([-h, 0.0, -h], [h, 10.0, h]));
    ModelGeometry {
        blocking01: blocking01.then_some(corners),
        instance_box: ([-1.0; 3], [1.0; 3]),
        file_box: corners,
        radius: h,
    }
}

// QUIRK D-356: kept
#[test]
fn ground_rect_shrinks_by_an_eighth() {
    let m = generator_world_matrix([640.0, 0.0, 384.0], [0.0; 3]);
    // x: 576 ... 704 -> 592 ... 688 -> 4 ... 5; z: 320 ... 448 -> 336 ... 432 -> 2 ... 3.
    for with in [false, true] {
        assert_eq!(
            ground_rect(&box_geometry(64.0, with), &m, 128),
            [4, 2, 5, 3]
        );
    }
}

// QUIRK D-356: kept
#[test]
fn negative_rect_rounds_down_only_with_blocking01() {
    // x = 32: -32 ... 96 -> -16 ... 80; ftol(-16)/128 = 0, with `blocking01` -1 (D-356).
    let m = generator_world_matrix([32.0, 0.0, 384.0], [0.0; 3]);
    assert_eq!(
        ground_rect(&box_geometry(64.0, true), &m, 128),
        [-1, 2, 0, 3]
    );
    assert_eq!(
        ground_rect(&box_geometry(64.0, false), &m, 128),
        [0, 2, 0, 3]
    );
    // Decremented, not set to -1: x = -120 -> x0 = -168 -> -1 -> -2; x1 = -72 -> 0 -> -1.
    let m = generator_world_matrix([-120.0, 0.0, 384.0], [0.0; 3]);
    assert_eq!(
        ground_rect(&box_geometry(64.0, true), &m, 128),
        [-2, 2, -1, 3]
    );
    assert_eq!(
        ground_rect(&box_geometry(64.0, false), &m, 128),
        [-1, 2, 0, 3]
    );
    // Exactly -128 before `ftol`: -1, with `blocking01` -2 (also for exact multiples).
    let m = generator_world_matrix([-80.0, 0.0, 384.0], [0.0; 3]);
    assert_eq!(ground_rect(&box_geometry(64.0, true), &m, 128)[0], -2);
    assert_eq!(ground_rect(&box_geometry(64.0, false), &m, 128)[0], -1);
}

// QUIRK D-356: kept
#[test]
fn narrow_rect_swaps_its_bounds() {
    // Hull -8 ... 8: x0 = 16 + 632 = 648, x1 = 648 - 16 = 632 -> swapped: 632 ... 648 -> 4 ... 5.
    let m = generator_world_matrix([640.0, 0.0, 384.0], [0.0; 3]);
    assert_eq!(
        ground_rect(&box_geometry(8.0, false), &m, 128),
        [4, 2, 5, 3]
    );
}

// QUIRK D-356: kept
#[test]
fn snap_hull_takes_the_instance_box_without_blocking01() {
    let m = generator_world_matrix([640.0, 0.0, 384.0], [0.0; 3]);
    let geo = ModelGeometry {
        blocking01: None,
        instance_box: ([-10.0, 0.0, -20.0], [30.0, 5.0, 40.0]),
        file_box: to_corners8(&([-64.0; 3], [64.0; 3])),
        radius: 1.0,
    };
    // Instance hull, not the file hull, not shrunk.
    assert_eq!(
        snap_hull(&geo, &m),
        ([630.0, 0.0, 364.0], [670.0, 5.0, 424.0])
    );
    // With `blocking01` its corners apply.
    let with = ModelGeometry {
        blocking01: Some(to_corners8(&([-64.0, 0.0, -64.0], [64.0, 10.0, 64.0]))),
        ..geo
    };
    assert_eq!(
        snap_hull(&with, &m),
        ([576.0, 0.0, 320.0], [704.0, 10.0, 448.0])
    );
    // Rotation about y = pi/2: the corners are rotated, then min/max (x <-> z).
    let m = generator_world_matrix([640.0, 0.0, 384.0], [0.0, std::f32::consts::FRAC_PI_2, 0.0]);
    let (mn, mx) = snap_hull(&geo, &m);
    for (got, want) in mn
        .iter()
        .chain(&mx)
        .zip([600.0, 0.0, 374.0, 660.0, 5.0, 414.0])
    {
        assert!((got - want).abs() < 1e-3, "{mn:?} {mx:?}");
    }
}

// QUIRK D-356: kept
#[test]
fn radius_is_the_file_value() {
    let mut file = bounds([-1.0; 3], [1.0; 3]);
    file.radius = 77.5;
    file.corners[3] = [9.0, -9.0, 9.0];
    let files = BTreeMap::from([(
        "a.rdo".to_string(),
        rdo(
            file.clone(),
            vec![mesh(3002, "kiste", bounds([0.0; 3], [2.0; 3]))],
        ),
    )]);
    let geo = load(&files, "a.rdo").unwrap();
    assert_eq!(geo.radius, 77.5);
    // The file hull is the 8 stored corners (even one that is not a min/max combination).
    assert_eq!(geo.file_box, file.corners);
    assert_eq!(geo.blocking01, None);
}

// QUIRK D-356: kept
#[test]
fn instance_box_follows_0x0058ac40() {
    let mut files = BTreeMap::new();
    // A helper node does not count; rigid and skin mesh (unanimated) via their stored corners.
    files.insert(
        "a.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![
                mesh(3005, "zauber01", bounds([-1000.0; 3], [1000.0; 3])),
                mesh(
                    3002,
                    "kiste",
                    bounds([-10.0, 0.0, -20.0], [30.0, 5.0, 40.0]),
                ),
                mesh(
                    3001,
                    "koerper",
                    bounds([-50.0, -1.0, 0.0], [0.0, 50.0, 10.0]),
                ),
            ],
        ),
    );
    // Sub-objects of the object level, recursive; one that cannot be loaded is omitted.
    files.insert("a.cfg".to_string(), cfg(&["sub.rdo", "missing.rdo"]));
    files.insert(
        "sub.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![mesh(3002, "part", bounds([0.0; 3], [100.0, 1.0, 1.0]))],
        ),
    );
    files.insert("sub.cfg".to_string(), cfg(&["subsub.rdo"]));
    files.insert(
        "subsub.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![mesh(3002, "tip", bounds([-5.0, -200.0, 0.0], [0.0; 3]))],
        ),
    );
    let geo = load(&files, "a.rdo").unwrap();
    assert_eq!(
        geo.instance_box,
        ([-50.0, -200.0, -20.0], [100.0, 50.0, 40.0])
    );

    // Without a mesh of its own the zero box stays in the union (D-356).
    files.insert(
        "empty.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![mesh(3005, "zauber01", bounds([-1.0; 3], [1.0; 3]))],
        ),
    );
    files.insert("empty.cfg".to_string(), cfg(&["far.rdo"]));
    files.insert(
        "far.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![mesh(3002, "kiste", bounds([5.0; 3], [10.0; 3]))],
        ),
    );
    assert_eq!(
        load(&files, "empty.rdo").unwrap().instance_box,
        ([0.0; 3], [10.0; 3])
    );
    // Without a mesh and without a sub-object: the zero box.
    files.insert(
        "nothing.rdo".to_string(),
        rdo(bounds([-1.0; 3], [1.0; 3]), Vec::new()),
    );
    assert_eq!(
        load(&files, "nothing.rdo").unwrap().instance_box,
        ([0.0; 3], [0.0; 3])
    );

    // Argument order `Box_Union(sub-object, acc)`: with -0 (own mesh) against +0 (sub-object) the
    // sub-object wins; meshes among themselves `Box_Union(acc, mesh)`: the first one stays.
    files.insert(
        "zero.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![
                mesh(3002, "a", bounds([-0.0, 0.0, 0.0], [1.0; 3])),
                mesh(3002, "b", bounds([0.0, -0.0, 0.0], [1.0; 3])),
            ],
        ),
    );
    files.insert("zero.cfg".to_string(), cfg(&["plus.rdo"]));
    files.insert(
        "plus.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![mesh(3002, "c", bounds([0.0; 3], [0.5; 3]))],
        ),
    );
    let b = load(&files, "zero.rdo").unwrap().instance_box;
    assert_eq!(
        bits(&b)[0],
        [0.0_f32.to_bits(), 0.0_f32.to_bits(), 0.0_f32.to_bits()]
    );
    // Without a sub-object the -0 of the first mesh stays, the +0 of the second changes nothing.
    files.remove("zero.cfg");
    let b = load(&files, "zero.rdo").unwrap().instance_box;
    assert_eq!(bits(&b)[0], [0x8000_0000, 0, 0]);

    // `Box_ToCorners8` -> `Box_FromCorners8` of a sub-object with x = -0 ... +0 gives -0 ... -0
    // (strict comparison keeps corner 0); united with the zero box, -0 stays.
    files.insert(
        "flat.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![mesh(3002, "f", bounds([-0.0, 1.0, 1.0], [0.0, 2.0, 2.0]))],
        ),
    );
    files.insert(
        "carrier.rdo".to_string(),
        rdo(bounds([-1.0; 3], [1.0; 3]), Vec::new()),
    );
    files.insert("carrier.cfg".to_string(), cfg(&["flat.rdo"]));
    let b = load(&files, "carrier.rdo").unwrap().instance_box;
    assert_eq!(b.0[0].to_bits(), 0x8000_0000);
    assert_eq!(b.1[0].to_bits(), 0x8000_0000);
    assert_eq!((b.0[1], b.1[1]), (0.0, 2.0));
}

#[test]
fn unloadable_model_has_no_geometry() {
    let files = BTreeMap::from([("broken.rdo".to_string(), vec![1, 2, 3])]);
    assert_eq!(load(&files, "broken.rdo"), None);
    assert_eq!(load(&files, "missing.rdo"), None);
}

// QUIRK D-356: kept
#[test]
fn blocking01_corners_follow_0x0058e790() {
    // Identity rotation as w = -1 (shipped data): corners = stored corner + node position, in
    // file order, -0 becomes +0.
    let mut b = bounds([-64.0, -0.0, -32.0], [64.0, 10.0, 32.0]);
    b.corners.swap(1, 6);
    let mut node = mesh(3005, BLOCKING01, b.clone());
    node.position = [5.0, 0.0, -3.0];
    node.rotation_wxyz = [-1.0, 0.0, 0.0, 0.0];
    let files = BTreeMap::from([(
        "h.rdo".to_string(),
        rdo(
            bounds([-1.0; 3], [1.0; 3]),
            vec![
                mesh(3005, "zauber01", bounds([0.0; 3], [1.0; 3])),
                node.clone(),
            ],
        ),
    )]);
    let corners = load(&files, "h.rdo").unwrap().blocking01.unwrap();
    let want = b
        .corners
        .map(|c| [c[0] + 5.0, c[1] + 0.0 + 0.0, c[2] - 3.0]);
    assert_eq!(
        corners.map(|c| c.map(f32::to_bits)),
        want.map(|c| c.map(f32::to_bits))
    );
    assert_eq!(corners[0][1].to_bits(), 0);

    // Real rotation (90 degrees about y): quaternion sandwich `q⁻¹ · v · q`, close to the node matrix of
    // `nml_world` (glam, `v · R + t`).
    let h = std::f32::consts::FRAC_1_SQRT_2;
    node.rotation_wxyz = [h, 0.0, h, 0.0];
    let glam = nml_world::math::mesh_node_matrix(&node);
    for c in b.corners {
        let got = node_transform(c, &node);
        let want = glam.transform_point3(glam::Vec3::from(c));
        assert!(
            (glam::Vec3::from(got) - want).length() < 1e-3,
            "{got:?} {want:?}"
        );
    }
    // Rotation sense of the original: (64, y, 32) -> (-32, y, 64) + t, so x -> +z, z -> -x.
    let got = node_transform([64.0, 0.0, 32.0], &node);
    assert!(
        (got[0] + 27.0).abs() < 1e-3 && (got[2] - 61.0).abs() < 1e-3,
        "{got:?}"
    );
}

// QUIRK D-356, D-365: kept
#[test]
fn blocking01_corners_apply_the_node_scale() {
    // D-365: `Vector3_ApplyNodeTransform` (`0x00578180`) first multiplies by the node scale
    // `+0x12C` (file value, `0x005A0910`), then rotation and position - corner = stored corner * s + t.
    let s = 0.689_655_2_f32;
    let b = bounds([-36.8616, -0.0, -36.8616], [36.8616, 10.0, 36.8616]);
    let mut node = mesh(3005, BLOCKING01, b.clone());
    node.position = [5.0, 0.0, -3.0];
    node.rotation_wxyz = [-1.0, 0.0, 0.0, 0.0];
    node.scale = [s; 3];
    let files = BTreeMap::from([(
        "h.rdo".to_string(),
        rdo(bounds([-1.0; 3], [1.0; 3]), vec![node]),
    )]);
    let corners = load(&files, "h.rdo").unwrap().blocking01.unwrap();
    let want = b
        .corners
        .map(|c| [c[0] * s + 5.0, c[1] * s + 0.0 + 0.0, c[2] * s - 3.0]);
    assert_eq!(
        corners.map(|c| c.map(f32::to_bits)),
        want.map(|c| c.map(f32::to_bits))
    );
    // Half edge 36.8616 * 20/29 = 25.4218 instead of 36.8616.
    assert!(
        (corners[7][0] - 5.0 - 25.4218).abs() < 1e-3,
        "{:?}",
        corners[7]
    );
}

// QUIRK D-356, D-365: kept
#[test]
fn corpus_blocking01_applies_the_node_scale() {
    // palme05 (GUID 3638) carries the scale 20/29 on the node `blocking01`, palme01 (3634) none;
    // the stored corners are ±36.8616 for both (run-time memory dump of the original, Caribbean, D-365).
    let Some((vfs, project)) = corpus_project() else {
        return;
    };
    let mut read = |p: &str| vfs.read(p).ok();
    for (guid, name, scale, half) in [
        (3634, "palme01", 1.0_f32, 36.8616_f32),
        (3638, "palme05", 0.689_655_2, 25.4218),
    ] {
        let model = &project.objects[&guid].model;
        assert!(model.to_lowercase().contains(name), "{guid}: {model}");
        let rdo = parse_rdo(&vfs.read(model).unwrap()).unwrap();
        let node = rdo
            .meshes
            .iter()
            .find(|m| m.type_id == HELPER_TYPE && m.name == BLOCKING01)
            .unwrap();
        assert_eq!(node.scale, [scale; 3], "{name}");
        assert!((node.bounds.corners[7][0] - 36.8616).abs() < 1e-3, "{name}");
        assert_eq!(node.rotation_wxyz, [-1.0, 0.0, 0.0, 0.0], "{name}");
        let corners = load_geometry(&mut read, model).unwrap().blocking01.unwrap();
        let t = node.position;
        for (got, c) in corners.iter().zip(node.bounds.corners) {
            let want: Vec3 = std::array::from_fn(|k| c[k] * scale + 0.0 + t[k]);
            assert_eq!(got.map(f32::to_bits), want.map(f32::to_bits), "{name}");
        }
        let (mn, mx) = min_max(&corners);
        let got_half = (mx[0] - mn[0]) * 0.5;
        eprintln!("{name}: scale {scale}, half edge {got_half}");
        assert!((got_half - half).abs() < 1e-3, "{name}: {got_half}");
    }
}

/// Measured (German edition with patch): FNV-1a 64 over GUID and the bits of instance hull,
/// file hull and radius of the 15 placed GUIDs without `blocking01` (D-356).
const INSTANCE_BOX_DIGEST: u64 = 0x34DC_82BE_0716_4C79;

/// The 15 placed GUIDs without `blocking01` (corpus `placed_object_models_load`).
const WITHOUT_BLOCKING01: [u32; 15] = [
    3411, 3500, 3503, 3505, 3908, 3909, 3916, 3917, 4000, 4001, 4002, 4011, 4012, 4013, 4200,
];

/// Installation and project of the continental map (seed `ZKA`, landscape 0) for the corpus tests;
/// `None` without an installation.
fn corpus_project() -> Option<(nml_rda::Vfs, nml_formats::prj::Project)> {
    use nml_formats::prj::parse_project;
    use nml_formats::rnd::parse_random_map_script;
    use nml_rda::Vfs;
    use nml_rda::testutil::corpus_game_dir;
    use nml_world::rand::MsvcRand;

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let types = crate::load_types(&vfs).unwrap();
    let terrain = types
        .map_scripts
        .iter()
        .position(|s| s.to_lowercase().ends_with("/continental.rnd"))
        .unwrap();
    let spec = crate::RandomMapSpec {
        seed_text: "ZKA".to_string(),
        size: 1,
        players: 4,
        terrain,
        landscape: 0,
        daytime: 0,
        game_type: 0,
    };
    let (path, defines) = crate::script_and_defines(&spec, &types).unwrap();
    let mut rng = MsvcRand::new(0);
    let script = parse_random_map_script(&path, defines, &mut |p| vfs.read(p).ok(), &mut || {
        rng.rand() as i32
    })
    .unwrap();
    let project = parse_project(&decode_cp1252(
        &vfs.read(script.project_file.as_deref().unwrap()).unwrap(),
    ))
    .unwrap();
    Some((vfs, project))
}

// QUIRK D-356: kept
#[test]
fn corpus_instance_box_against_the_glam_approximation() {
    use nml_rda::testutil::{GameVersion, game_version};
    use nml_world::anim::Playback;
    use nml_world::assets::ModelCache;
    use nml_world::shadows;

    let Some((vfs, project)) = corpus_project() else {
        return;
    };

    let mut read = |p: &str| vfs.read(p).ok();
    let mut cache = ModelCache::new();
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut fnv = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3);
        }
    };
    let mut differ = Vec::new();
    for guid in WITHOUT_BLOCKING01 {
        let model = &project.objects[&guid].model;
        let geo = load_geometry(&mut read, model).unwrap();
        assert_eq!(geo.blocking01, None, "GUID {guid}");
        // glam approximation `nml_world::shadows::instance_box` (rest hull, only the first level of
        // sub-objects).
        let asset = cache.get(&vfs, model).unwrap();
        let playback = Playback::new(&asset, |f| cache.get(&vfs, f), None, 0.0);
        let approx = shadows::instance_box(&asset, &playback, None, 0.0);
        let approx = (approx.min.to_array(), approx.max.to_array());
        if bits(&approx) != bits(&geo.instance_box) {
            differ.push((guid, geo.instance_box, approx));
        }
        eprintln!("{guid} {model}: {:?}", geo.instance_box);
        fnv(&guid.to_le_bytes());
        for v in [geo.instance_box.0, geo.instance_box.1]
            .iter()
            .chain(&geo.file_box)
            .flatten()
            .chain(std::iter::once(&geo.radius))
        {
            fnv(&v.to_bits().to_le_bytes());
        }
    }
    eprintln!("differs from the glam approximation: {differ:?}");
    eprintln!("checksum: {h:#x}");
    // In the shipped data the glam approximation hits the computation order of the original
    // bit-exactly (no sub-objects of deeper levels, no -0 cases); the reimplementation computes
    // exactly anyway.
    assert!(differ.is_empty(), "{differ:?}");
    match game_version(&vfs).unwrap() {
        GameVersion::German => assert_eq!(h, INSTANCE_BOX_DIGEST, "{h:#x}"),
        GameVersion::English => eprintln!("English edition: checksum not recorded"),
    }

    // The node name is compared exactly (`strcmp` against the lowercase search name): in the
    // shipped data this finds the same nodes as a case-insensitive comparison.
    let mut checked = 0;
    for object in project.objects.values() {
        let Some(asset) = cache.get(&vfs, &object.model) else {
            continue;
        };
        let loose = asset
            .rdo
            .meshes
            .iter()
            .any(|m| m.type_id == HELPER_TYPE && m.name.eq_ignore_ascii_case(BLOCKING01));
        assert_eq!(
            blocking01_corners(&asset.rdo).is_some(),
            loose,
            "GUID {}",
            object.guid
        );
        checked += 1;
    }
    eprintln!("node names checked: {checked} models");
}
