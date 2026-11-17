//! Loads all levels and all models of the project file from the original installation.

use nml_formats::hlm::parse_hlm;
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};
use nml_world::assets::{ModelCache, load_texture};
use nml_world::chunks::build_chunk_textures;
use nml_world::classes::{ObjectTypes, ShadowKind, in_no_shadow_area, object_shadow};
use nml_world::environment::{WeatherKind, level_start_environment, load_light_presets};
use nml_world::level::{level_paths, load_level};
use nml_world::shadows::{Aabb, DecalHull, mesh_box};

fn open() -> Option<Vfs> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    Some(Vfs::open_game_dir(&dir).unwrap())
}

#[test]
fn all_levels_load_with_game_normals_and_object_heights() {
    let Some(vfs) = open() else { return };
    let levels = level_paths(&vfs);
    assert_eq!(levels.len(), 55);
    let (mut on_ground, mut normals_checked) = (0, 0);
    for lvl in &levels {
        let scene = load_level(&vfs, lvl).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
        // The stored normals are exactly the game formula (docs/formats/hlm.md: all 910,807 points).
        let base = lvl.strip_suffix(".lvl").unwrap();
        let t = &scene.world.terrain;
        let hlm = parse_hlm(&vfs.read(&format!("{base}.hlm")).unwrap(), t.dim_x, t.dim_z).unwrap();
        for (vertex, point) in scene.terrain.vertices.iter().zip(&hlm.points) {
            let file = glam::Vec3::from(point.normal);
            assert!(
                (vertex.normal - file).length() < 1e-4,
                "{lvl}: {:?} != {file:?}",
                vertex.normal
            );
            normals_checked += 1;
        }
        for o in &scene.world.objects {
            let [x, y, z] = o.position;
            if (scene.terrain.height_at(x, z) - y).abs() < 0.5 {
                on_ground += 1;
            }
        }
    }
    eprintln!(
        "{} levels, {normals_checked} normals checked, {on_ground} objects on the terrain",
        levels.len()
    );
    assert_eq!(normals_checked, 910_807);
    // docs/formats/wld.md: 123,573 of 126,085 objects stand on the terrain (|dy| < 0.5).
    assert_eq!(on_ground, 123_573);
}

#[test]
fn start_lights_exist_and_relighting_with_the_level_light_changes_nothing() {
    let Some(vfs) = open() else { return };
    let presets = load_light_presets(&vfs).unwrap();
    assert_eq!(presets.len(), 32);
    let (mut lights, mut snow, mut rain) = (0, 0, 0);
    for lvl in level_paths(&vfs) {
        let start = level_start_environment(&lvl);
        if let Some(name) = start.light {
            assert!(
                nml_formats::lightdata::find_preset(&presets, name).is_some(),
                "{lvl}: preset {name} missing"
            );
            lights += 1;
        }
        match start.weather {
            Some(WeatherKind::Snow) => snow += 1,
            Some(WeatherKind::Rain) => rain += 1,
            None => {}
        }
        let mut scene = load_level(&vfs, &lvl).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
        let loaded: Vec<[u8; 3]> = scene.terrain.vertices.iter().map(|v| v.lit_color).collect();
        scene.apply_light(Some(&presets[1]));
        scene.apply_light(None);
        assert!(
            scene
                .terrain
                .vertices
                .iter()
                .map(|v| v.lit_color)
                .eq(loaded),
            "{lvl}: recomputation with the level light differs"
        );
    }
    // Every entry of the tables matches an existing level.
    assert_eq!((lights, snow, rain), (11, 5, 2));
}

#[test]
fn kamp1_1_builds_all_chunk_textures() {
    let Some(vfs) = open() else { return };
    let scene = load_level(&vfs, "data/levels/kampagne1/kamp1-1/kamp1-1.lvl").unwrap();
    assert!(
        scene.objects.len() > 1000,
        "{} objects",
        scene.objects.len()
    );
    let images = scene.ground_textures(&vfs);
    eprintln!(
        "kamp1-1: {} objects, {} ground textures, {} blocks",
        scene.objects.len(),
        images.len(),
        scene.terrain.chunk_count()
    );
    assert!(!images.is_empty());
    for chunk in 0..scene.terrain.chunk_count() {
        let textures = build_chunk_textures(&scene.terrain, &scene.project, &images, chunk);
        assert!(
            textures.base.pixels().any(|p| p.0[..3] != [0, 0, 0]),
            "block {chunk} black"
        );
        assert!(!textures.layers.is_empty());
    }
}

#[test]
fn all_project_models_load() {
    let Some(vfs) = open() else { return };
    let scene = load_level(&vfs, "data/levels/kampagne1/kamp1-1/kamp1-1.lvl").unwrap();
    let mut cache = ModelCache::new();
    let (mut missing_animations, mut missing_textures) = (Vec::new(), Vec::new());
    for object in scene.project.objects.values() {
        let model = cache.get(&vfs, &object.model).unwrap_or_else(|| {
            panic!(
                "model {} of type {} not loadable",
                object.model, object.guid
            )
        });
        for (entry, anim) in model.config.animations.iter().zip(&model.animations) {
            if anim.is_none() {
                missing_animations.push(format!("{}: {:?}", model.path, entry.file));
            }
        }
        for texture in model.material_textures.iter().flatten() {
            if load_texture(&vfs, texture).is_err() {
                missing_textures.push(format!("{}: {texture}", model.path));
            }
        }
    }
    eprintln!(
        "{} project types, all models loaded, {} missing animations",
        scene.project.objects.len(),
        missing_animations.len()
    );
    assert!(missing_animations.is_empty(), "{missing_animations:#?}");
    // The project types alone are complete; missing textures do not turn up until all models are checked.
    missing_textures.sort();
    assert!(missing_textures.is_empty(), "{missing_textures:#?}");
}

#[test]
fn all_model_files_load_and_missing_textures_do_not_abort() {
    let Some(vfs) = open() else { return };
    let mut cache = ModelCache::new();
    let (mut models, mut unreadable, mut missing_textures) = (0, Vec::new(), Vec::new());
    let paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".rdo") || p.ends_with(".rdw"))
        .collect();
    for path in &paths {
        let Some(model) = cache.get(&vfs, path) else {
            unreadable.push(path.clone());
            continue;
        };
        models += 1;
        for texture in model.material_textures.iter().flatten() {
            if !vfs.exists(texture) {
                assert!(load_texture(&vfs, texture).is_err());
                missing_textures.push(format!("{path}: {texture}"));
            }
        }
    }
    missing_textures.sort();
    missing_textures.dedup();
    eprintln!(
        "{} model files, {models} loaded, {} not loadable, missing material textures: {missing_textures:#?}",
        paths.len(),
        unreadable.len()
    );
    // Load all models, also `kanone_spa_stehen.rdo` (`.cfg` without OBJECT block -> empty
    // configuration as in the game).
    assert!(unreadable.is_empty(), "not loadable: {unreadable:#?}");
    // Named but missing material textures (model: resolved path); the game draws these
    // materials untextured, loading does not abort.
    assert_eq!(
        missing_textures,
        [
            "data/r3d/eng/units/infanterist/wache_gewehr.rdo: data/r3d/eng/units/infanterist/winchester.tga",
            "data/r3d/pri/units/travois/travois2_pri.rdw: data/r3d/pri/units/travois/horse.tga",
            "data/r3d/pri/units/travois/travois_pri.rdw: data/r3d/pri/units/travois/horse2.tga",
        ]
    );
}

/// Shadow kinds of all generated objects in the 34 campaign and tutorial `.wld` (riders split
/// into both parts) against the count in re/object-classes (section "Check against the data"):
/// `[kind 2, kind 3, decal, none]` per `ShadowQuality`, without `noshadow` areas.
#[test]
fn shadow_kinds_of_the_campaign_placements_match_the_analysis() {
    let Some(vfs) = open() else { return };
    let types = ObjectTypes::load(&vfs);
    let project = nml_formats::prj::parse_project(&nml_formats::text::decode_cp1252(
        &vfs.read("data/projects/nml-wiese.prj").unwrap(),
    ))
    .unwrap();
    let mut worlds: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".wld") && (p.contains("/kampagne") || p.contains("/tutorial")))
        .collect();
    worlds.sort();
    worlds.dedup();
    assert_eq!(worlds.len(), 34);
    let mut counts = [[0usize; 4]; 4];
    let mut no_shadow_objects = 0;
    for path in &worlds {
        let text = nml_formats::text::decode_cp1252(&vfs.read(path).unwrap());
        let world = nml_formats::wld::parse_world(&text).unwrap();
        for o in &world.objects {
            let tile = (
                (o.position[0] / world.terrain.size_x).floor() as i64,
                (o.position[2] / world.terrain.size_z).floor() as i64,
            );
            let parts = types.parts(o.guid);
            if !parts.is_empty() && in_no_shadow_area(&world.areas, tile) {
                no_shadow_objects += 1;
            }
            for part in parts {
                for (quality, count) in counts.iter_mut().enumerate() {
                    let shadow = object_shadow(&types, &project, &[part], quality as u8, false);
                    let slot = match shadow.kind {
                        ShadowKind::Persistent => 0,
                        ShadowKind::PerFrame => 1,
                        _ if !shadow.decals.is_empty() => 2,
                        _ => 3,
                    };
                    count[slot] += 1;
                }
            }
        }
    }
    eprintln!("{counts:?}, {no_shadow_objects} objects in noshadow areas");
    // The ten noshadow areas all lie in kamp0-1.
    assert_eq!(no_shadow_objects, 1_131);
    assert_eq!(counts[0], [0, 0, 97, 85_110]);
    assert_eq!(counts[1], [0, 0, 81_472, 3_735]);
    assert_eq!(counts[2], [4_431, 629, 77_296, 2_851]);
    assert_eq!(counts[3], [38_448, 46_523, 101, 135]);
}

/// Loads all shadow and ground plate models of the project file; each with a flat hull and an
/// existing texture in material 0 (re/shadows, section "Data").
#[test]
fn shadow_and_groundplane_models_have_a_hull_and_a_texture() {
    let Some(vfs) = open() else { return };
    let project = nml_formats::prj::parse_project(&nml_formats::text::decode_cp1252(
        &vfs.read("data/projects/nml-wiese.prj").unwrap(),
    ))
    .unwrap();
    let mut paths: Vec<(String, bool)> = project
        .objects
        .values()
        .flat_map(|o| {
            let shadow = o.shadow_model.iter().map(|p| (normalize_path(p), false));
            let ground = o
                .groundplane_model
                .iter()
                .map(|p| (normalize_path(p), true));
            shadow.chain(ground).collect::<Vec<_>>()
        })
        .collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 273);
    let mut models = ModelCache::new();
    let mut same_hull = 0;
    for (path, ground) in &paths {
        let model = models.get(&vfs, path).unwrap_or_else(|| panic!("{path}"));
        let hull = DecalHull::from_bounds(&model.rdo.bounds);
        assert!(
            hull.half.x > 0.0 && hull.half.z > 0.0 && hull.half.y <= 14.5,
            "{path}"
        );
        let texture = model.material_textures[0].as_deref().unwrap();
        assert!(load_texture(&vfs, texture).is_ok(), "{path}: {texture}");
        // Ground plates use the mesh hulls (`ComputeBoundingBox(..., 0)`), shadow decals the
        // header: in the shipped data the same area in x/z.
        if *ground {
            let meshes: Aabb = mesh_box(&model, None).unwrap();
            let head = Aabb {
                min: hull.center - hull.half,
                max: hull.center + hull.half,
            };
            if (meshes.min - head.min).abs().max_element() < 0.5
                && (meshes.max - head.max).abs().max_element() < 0.5
            {
                same_hull += 1;
            }
        }
    }
    eprintln!("{same_hull} of 75 ground plates with equal header and mesh hull");
    assert_eq!(paths.iter().filter(|(_, g)| *g).count(), 75);
    assert_eq!(same_hull, 75);
}
