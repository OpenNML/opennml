//! Exports all models (`.rdo`/`.rdw`) of the archives and the level kamp1-1 and checks the files
//! with the glTF validator of the `gltf` crate.

use nml_export::{LevelExportOptions, level_to_glb, model_to_glb};
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};
use nml_world::assets::ModelCache;
use nml_world::level::load_level;

/// Checks the data contents: all normals finite with length ~ 1, all animation times finite and
/// strictly increasing. Returns the number of animations.
fn check_data(name: &str, glb: &[u8]) -> usize {
    let gltf = gltf::Gltf::from_slice(glb).unwrap_or_else(|e| panic!("{name}: {e}"));
    // Without geometry (e.g. pure helper nodes) there is no binary block.
    let Some(blob) = gltf.blob.as_deref() else {
        assert_eq!(
            gltf.meshes().count(),
            0,
            "{name}: meshes without binary block"
        );
        assert_eq!(
            gltf.animations().count(),
            0,
            "{name}: animations without binary block"
        );
        return 0;
    };
    let buffers = |_: gltf::Buffer| Some(blob);
    for mesh in gltf.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(buffers);
            let Some(normals) = reader.read_normals() else {
                continue;
            };
            for (i, n) in normals.enumerate() {
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!(
                    len.is_finite() && (len - 1.0).abs() < 1e-3,
                    "{name}: normal {i} in mesh {:?} is {n:?} (length {len})",
                    mesh.name()
                );
            }
        }
    }
    for animation in gltf.animations() {
        for channel in animation.channels() {
            let reader = channel.reader(buffers);
            let Some(times) = reader.read_inputs() else {
                panic!("{name}: animation {:?} without times", animation.name());
            };
            let mut previous = f32::NEG_INFINITY;
            for (i, t) in times.enumerate() {
                assert!(
                    t.is_finite() && t > previous,
                    "{name}: animation {:?}: time {i} = {t} not strictly increasing (previous {previous})",
                    animation.name()
                );
                previous = t;
            }
        }
    }
    gltf.animations().count()
}

#[test]
fn all_models_and_kamp1_1_export_valid_gltf() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/kampagne1/kamp1-1/kamp1-1.lvl").unwrap();
    let mut models: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".rdo") || p.ends_with(".rdw"))
        .collect();
    models.sort();
    models.dedup();
    let mut cache = ModelCache::new();
    let (mut animations, mut exported, mut unreadable) = (0, 0, Vec::new());
    for path in &models {
        let Some(model) = cache.get(&vfs, path) else {
            unreadable.push(path.as_str());
            continue;
        };
        let glb = model_to_glb(&vfs, &model).unwrap_or_else(|e| panic!("{path}: {e:#}"));
        animations += check_data(path, &glb);
        exported += 1;
    }
    eprintln!(
        "{} model files, {exported} exported, {animations} animations, not loadable: {unreadable:?}",
        models.len()
    );
    // All models load (including `kanone_spa_stehen.rdo` with a `.cfg` without OBJECT block).
    assert!(unreadable.is_empty(), "not loadable: {unreadable:?}");
    assert!(exported > 500, "only {exported} models exported");
    assert!(animations > 1000, "only {animations} animations exported");
    let options = LevelExportOptions {
        pixels_per_tile: 4,
        ..LevelExportOptions::default()
    };
    let glb = level_to_glb(&vfs, &scene, &options).unwrap();
    check_data("kamp1-1.lvl", &glb);
    let gltf = gltf::Gltf::from_slice(&glb).unwrap();
    assert!(
        gltf.nodes().count() > 1000,
        "{} nodes",
        gltf.nodes().count()
    );
}
