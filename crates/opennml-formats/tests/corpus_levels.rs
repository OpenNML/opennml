//! Reads all 55 levels (lvl, project file, wld, hlm, tlm) and checks the cross references between them.

use std::collections::{BTreeMap, BTreeSet};

use opennml_formats::hlm::parse_hlm;
use opennml_formats::lvl::parse_level;
use opennml_formats::prj::{Project, parse_project};
use opennml_formats::text::decode_cp1252;
use opennml_formats::tlm::parse_tlm;
use opennml_formats::wld::parse_world;
use opennml_rda::testutil::corpus_game_dir;
use opennml_rda::{Vfs, normalize_path};

#[test]
fn all_levels_parse_with_consistent_files() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let text =
        |path: &str| decode_cp1252(&vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}")));
    let levels: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".lvl"))
        .collect();
    assert_eq!(levels.len(), 55);

    let mut projects: BTreeMap<String, Project> = BTreeMap::new();
    let (mut objects, mut areas, mut cameras) = (0, 0, 0);
    let mut problems = Vec::new();
    for lvl in &levels {
        let level = parse_level(&text(lvl)).unwrap_or_else(|e| panic!("{lvl}: {e}"));
        let project_path = normalize_path(&level.project_file);
        let project = projects
            .entry(project_path.clone())
            .or_insert_with(|| parse_project(&text(&project_path)).unwrap());
        let colors: BTreeSet<i32> = level.players.iter().map(|p| p.color).collect();
        if colors.len() != level.players.len() || colors.iter().any(|c| !(0..=7).contains(c)) {
            problems.push(format!("{lvl}: player colors {colors:?}"));
        }
        let base = lvl.strip_suffix(".lvl").unwrap();
        let world = parse_world(&text(&format!("{base}.wld")))
            .unwrap_or_else(|e| panic!("{base}.wld: {e}"));
        let t = &world.terrain;
        let hlm = parse_hlm(&vfs.read(&format!("{base}.hlm")).unwrap(), t.dim_x, t.dim_z)
            .unwrap_or_else(|e| panic!("{base}.hlm: {e}"));
        let tlm = parse_tlm(&vfs.read(&format!("{base}.tlm")).unwrap(), t.dim_x, t.dim_z)
            .unwrap_or_else(|e| panic!("{base}.tlm: {e}"));
        for (i, p) in hlm.points.iter().enumerate() {
            let (x, z) = (i % hlm.verts_x, i / hlm.verts_x);
            if p.position[0] != x as f32 * t.size_x || p.position[2] != z as f32 * t.size_z {
                problems.push(format!("{base}.hlm: point {i} not on the grid"));
                break;
            }
            if p.color_guid
                .is_some_and(|g| !project.colors.contains_key(&g))
            {
                problems.push(format!("{base}.hlm: unknown color GUID {:?}", p.color_guid));
                break;
            }
        }
        if let Some(g) = tlm
            .guids
            .iter()
            .find(|g| !project.textures.contains_key(*g))
        {
            problems.push(format!("{base}.tlm: unknown texture GUID {g}"));
        }
        for o in &world.objects {
            if !project.objects.contains_key(&o.guid) {
                problems.push(format!("{base}.wld: unknown object type {}", o.guid));
            }
        }
        objects += world.objects.len();
        areas += world.areas.len();
        cameras += world.cameras.len();
    }
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!((objects, areas, cameras), (126_085, 2_948, 415));
    assert_eq!(projects.len(), 1);
    let project = projects.values().next().unwrap();
    assert_eq!(
        (
            project.objects.len(),
            project.textures.len(),
            project.colors.len()
        ),
        (512, 48, 24)
    );
    // Model rule: all 512 types have an existing model.
    let missing: Vec<_> = project
        .objects
        .values()
        .filter(|o| !vfs.exists(&o.model))
        .collect();
    assert!(missing.is_empty(), "{missing:#?}");
}
