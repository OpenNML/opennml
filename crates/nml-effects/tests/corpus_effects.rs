//! Runs every `.pef` of the original installation for 4 s in steps of 33 ms: all
//! files yield an instance, almost all draw something, and two runs yield the same
//! vertices. Skipped when the installation is missing.

use glam::Vec3;
use nml_effects::library::EffectLibrary;
use nml_effects::output::EffectFrame;
use nml_effects::particles::Placement;
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};
use nml_world::assets::ModelCache;
use nml_world::camera::look_at_lh;
use nml_world::rand::MsvcRand;

/// Vertices and drawn quads (over all frames) of a run.
fn simulate(vfs: &Vfs, library: &mut EffectLibrary, path: &str) -> (EffectFrame, usize) {
    let mut effect = library
        .instantiate(vfs, &mut ModelCache::new(), path, false)
        .unwrap();
    effect.start(0, 0);
    let mut rng = MsvcRand::new(nml_effects::EFFECTS_SEED);
    let placement = Placement {
        position: Vec3::new(1000.0, 50.0, 1000.0),
        yaw: 0.5,
    };
    let view = look_at_lh(Vec3::new(1000.0, 800.0, 400.0), placement.position, Vec3::Y);
    let mut quads = 0;
    let mut frame = EffectFrame::default();
    for step in 1..=120 {
        effect.advance(step * 33, placement, true, &mut rng);
        frame.clear();
        effect.draw(view, &mut frame);
        quads += frame.quad_count();
    }
    (frame, quads)
}

#[test]
fn every_pef_runs_deterministically() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".pef"))
        .collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 127);
    let mut library = EffectLibrary::new();
    let mut silent = Vec::new();
    for path in &paths {
        let (last, quads) = simulate(&vfs, &mut library, path);
        assert_eq!(simulate(&vfs, &mut library, path), (last, quads), "{path}");
        if quads == 0 {
            silent.push(path.as_str());
        }
    }
    assert_eq!(library.loaded(), 127);
    // Only the two pure model effects draw no quads (models follow in the
    // model effects).
    assert_eq!(silent, ["data/effects/object_test.pef"], "{silent:?}");
}
