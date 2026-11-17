//! Path grid of real maps: terrain classification and regions as in the original (counts for the shipped
//! data from the reference model of the Ghidra analysis, re/path-grid 6).

use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_sim::footprint::{footprint_tile_rect, world_matrix};
use nml_sim::occupancy::OccupancyGrid;
use nml_sim::path::grid::PathGrid;
use nml_sim::path::terrain::classify_all;
use nml_sim::rules::Rules;
use nml_sim::setup::snap_start_object;
use nml_world::level::load_level;

/// (level, blocked cells L0 ... L8, regions L0 ... L8, counters L0 ... L8) - terrain only.
type MapCounts = (&'static str, [usize; 9], [usize; 9], [i32; 9]);

const MAPS: [MapCounts; 4] = [
    (
        "data/levels/kampagne1/kamp1-1/kamp1-1.lvl",
        [12265, 190, 12265, 190, 3293, 12876, 3934, 636, 4587],
        [14, 2, 14, 2, 2, 14, 1, 3, 1],
        [61, 70, 61, 70, 66, 56, 66, 70, 67],
    ),
    (
        "data/levels/multiplayer/[2players]-battlebeach.lvl",
        [745, 175, 745, 175, 3230, 1078, 3375, 483, 3515],
        [31, 5, 31, 5, 1, 8, 1, 7, 1],
        [48, 23, 48, 23, 10, 25, 10, 25, 10],
    ),
    (
        "data/levels/multiplayer/[4players]-exile.lvl",
        [2364, 191, 2364, 191, 13514, 3035, 13839, 580, 14154],
        [1, 1, 1, 1, 1, 1, 1, 1, 1],
        [73, 65, 73, 65, 28, 71, 27, 66, 26],
    ),
    (
        "data/levels/multiplayer/[8players]-tribes.lvl",
        [
            17385, 16544, 17385, 16544, 64525, 23999, 64648, 23109, 64768,
        ],
        [536, 536, 536, 536, 1, 271, 1, 271, 1],
        [1019, 1014, 1019, 1014, 10, 649, 10, 644, 10],
    ),
];

#[test]
fn terrain_grids_and_regions_match_the_original_model() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    for (lvl, blocked, regions, counters) in MAPS {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let mut grid = PathGrid::new(rules.map.dim_x, rules.map.dim_z);
        let mut occ = OccupancyGrid::new(rules.map.dim_x, rules.map.dim_z);
        classify_all(&mut grid, &mut occ, &rules.tile_classes);
        let b: Vec<usize> = (0..9).map(|l| grid.blocked_count(l)).collect();
        assert_eq!(b, blocked, "{lvl}: blocked");
        grid.build_regions(u32::MAX);
        let r: Vec<usize> = (0..9).map(|l| grid.region_count(l)).collect();
        assert_eq!(r, regions, "{lvl}: regions");
        let c: Vec<i32> = grid.layers.iter().map(|l| l.counter).collect();
        assert_eq!(c, counters, "{lvl}: counters");
    }
}

/// Headquarters of the six nations as start buildings at (5000, 0, 5000): snapped, and their
/// footprint from the real models (test vectors of the Ghidra analysis).
#[test]
fn start_headquarters_get_the_original_footprints() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    for (guid, rect) in [
        (1000, [36, 37, 40, 39]),
        (1300, [37, 36, 39, 40]),
        (1600, [37, 37, 40, 40]),
        (1900, [36, 37, 40, 39]),
        (2200, [36, 37, 41, 40]),
        (2500, [36, 37, 41, 40]),
    ] {
        let mut v = [5000.0, 0.0, 5000.0];
        snap_start_object(&rules, guid, &mut v);
        let shape = &rules.building_shapes[&guid];
        let m = world_matrix(v, [0.0; 3], [1.0; 3]);
        assert_eq!(
            footprint_tile_rect(shape.center, shape.half, &m, rules.map.cell),
            rect,
            "{guid}"
        );
    }
    assert_eq!(rules.building_shapes[&1013].free_nodes.len(), 2);
}
