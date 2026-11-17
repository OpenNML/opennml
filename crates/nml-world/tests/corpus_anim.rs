//! Evaluates every animation of every loadable model in the archives (all `.rdo`/`.rdw`, as the
//! model browser offers them) at several points in time: all bone and mesh matrices finite,
//! morph frames matching the mesh. Also counts the meshes where one animation has a node and
//! morph track and another only the morph track (crossfade from the hidden old morph track,
//! `AnimationPlayer::pose`).

use nml_formats::rdo::MeshKind;
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};
use nml_world::anim::{AnimationPlayer, MeshPose, Pose, ticks_at};
use nml_world::assets::ModelCache;

#[test]
fn every_animation_of_every_model_evaluates() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".rdo") || p.ends_with(".rdw"))
        .collect();
    paths.sort();
    paths.dedup();
    let mut cache = ModelCache::new();
    let (mut models, mut evaluated, mut morphs) = (0, 0, 0);
    let (mut unloadable, mut problems) = (Vec::new(), Vec::new());
    let (mut hidden_morph_meshes, mut hidden_morph_pairs, mut node_and_morph) = (Vec::new(), 0, 0);
    for path in &paths {
        let Some(model) = cache.get(&vfs, path) else {
            unloadable.push(path.clone());
            continue;
        };
        models += 1;
        // Meshes with (node + morph) in one animation and only morph in another animation.
        let has_bones = !model.rdo.bones.is_empty();
        for (m, mesh) in model.rdo.meshes.iter().enumerate() {
            if mesh.kind == MeshKind::Helper || (mesh.kind == MeshKind::Skin && has_bones) {
                continue;
            }
            let tracks = |i: usize| {
                model.animations[i].as_deref().map(|a| {
                    (
                        a.node_tracks.iter().any(|t| t.name == mesh.name),
                        a.morph_tracks.iter().any(|t| t.name == mesh.name),
                    )
                })
            };
            let both: Vec<usize> = (0..model.animations.len())
                .filter(|&i| tracks(i) == Some((true, true)))
                .collect();
            let morph_only: Vec<usize> = (0..model.animations.len())
                .filter(|&i| tracks(i) == Some((false, true)))
                .collect();
            node_and_morph += both.len();
            if both.is_empty() || morph_only.is_empty() {
                continue;
            }
            hidden_morph_meshes.push(format!("{}: {}", model.path, mesh.name));
            for &old in &both {
                for &new in &morph_only {
                    hidden_morph_pairs += 1;
                    let mut player = AnimationPlayer::new(Some(old), 0.0);
                    assert!(player.play(Some(new), 500.0));
                    if let MeshPose::Morph { positions, .. } = &player.pose(&model, 600.0).meshes[m]
                    {
                        assert_eq!(positions.len(), mesh.positions.len(), "{}", model.path);
                    }
                }
            }
        }
        for (i, anim) in model.animations.iter().enumerate() {
            let Some(anim) = anim else { continue };
            for ms in [0.0, 333.0, 1e4] {
                let pose = Pose::evaluate(&model, anim, ticks_at(anim, ms));
                let finite = (0..model.rdo.bones.len())
                    .all(|b| pose.bone_matrix(&model, b).is_finite())
                    && (0..model.rdo.meshes.len()).all(|m| pose.mesh_matrix(&model, m).is_finite());
                if !finite {
                    problems.push(format!("{} animation {i} at {ms} ms", model.path));
                }
                for (mesh, state) in model.rdo.meshes.iter().zip(&pose.meshes) {
                    if let MeshPose::Morph { positions, .. } = state {
                        morphs += 1;
                        assert_eq!(positions.len(), mesh.positions.len(), "{}", model.path);
                    }
                }
                evaluated += 1;
            }
        }
    }
    eprintln!(
        "{} models, {models} loaded, {} not loadable ({unloadable:?}), {evaluated} evaluations, \
         {morphs} morph poses",
        paths.len(),
        unloadable.len()
    );
    eprintln!(
        "Node+morph in one animation: {node_and_morph} (mesh, animation); -> only morph: {} meshes, {hidden_morph_pairs} animation pairs: {hidden_morph_meshes:#?}",
        hidden_morph_meshes.len()
    );
    assert!(problems.is_empty(), "{problems:#?}");
    // Load all models; `kanone_spa_stehen.rdo` (`.cfg` without OBJECT block) without animations.
    assert!(unloadable.is_empty(), "not loadable: {unloadable:#?}");
    assert!(paths.len() > 1000, "only {} models", paths.len());
    assert!(evaluated > 3000, "only {evaluated} evaluations");
    assert!(morphs > 0, "no morph track evaluated");
}
