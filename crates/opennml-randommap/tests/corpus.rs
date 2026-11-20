//! Random maps from the map scripts of the original installation (`data/randommapscripts/*.rnd`).
//! Skipped if the installation is missing. Only checksums (FNV-1a 64), counters and positions are
//! recorded, no heights or tile excerpts.

use opennml_formats::mp_levels::RandomMapTypes;
use std::collections::BTreeMap;

use opennml_randommap::{
    Digests, RandomMap, RandomMapSpec, generate, generate_load_path, load_types, random_map_label,
    to_scene,
};
use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};

fn open() -> Option<(Vfs, RandomMapTypes)> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let types = load_types(&vfs).unwrap();
    Some((vfs, types))
}

/// Index of the map type whose script path (lowercase) ends in `<name>.rnd`.
fn terrain(types: &RandomMapTypes, name: &str) -> usize {
    types
        .map_scripts
        .iter()
        .position(|s| s.to_lowercase().ends_with(&format!("/{name}.rnd")))
        .unwrap_or_else(|| panic!("map type {name} is missing"))
}

fn game_type(types: &RandomMapTypes, gtype: &str) -> usize {
    types
        .game_types
        .iter()
        .position(|g| g.gtype == gtype)
        .unwrap()
}

/// Seed `ZKA`, 128-tile map, 4 players, meadow, day, game mode normal.
fn base_spec(types: &RandomMapTypes, terrain: usize) -> RandomMapSpec {
    RandomMapSpec {
        seed_text: "ZKA".to_string(),
        size: 1,
        players: 4,
        terrain,
        landscape: types
            .landscape_types
            .iter()
            .position(|l| l == "MEADOW")
            .unwrap(),
        daytime: types.daytime_types.iter().position(|d| d == "DAY").unwrap(),
        game_type: game_type(types, "NORMAL"),
    }
}

/// Generation like the restart (with the object step).
fn run(vfs: &Vfs, types: &RandomMapTypes, spec: &RandomMapSpec) -> RandomMap {
    generate(vfs, types, spec).unwrap_or_else(|e| panic!("{spec:?}: {e:#}"))
}

/// Generation like the load path of the original (without the object step).
fn run_load_path(vfs: &Vfs, types: &RandomMapTypes, spec: &RandomMapSpec) -> RandomMap {
    generate_load_path(vfs, types, spec).unwrap_or_else(|e| panic!("{spec:?}: {e:#}"))
}

/// Number of placed objects per GUID.
fn per_guid(map: &RandomMap) -> BTreeMap<u32, usize> {
    let mut per = BTreeMap::new();
    for o in &map.objects {
        *per.entry(o.guid).or_default() += 1;
    }
    per
}

const SCRIPTS: [&str; 6] = [
    "continental",
    "flatland",
    "highland",
    "islands",
    "lakes",
    "rivers",
];

#[test]
fn same_seed_same_map_other_seed_other_map() {
    let Some((vfs, types)) = open() else { return };
    let spec = base_spec(&types, terrain(&types, "continental"));
    let a = run(&vfs, &types, &spec);
    let b = run(&vfs, &types, &spec);
    assert_eq!(a.digests(), b.digests());
    assert_eq!(a.rand_state, b.rand_state);
    assert_eq!(a.rand_counts, b.rand_counts);
    assert_eq!(a.player_areas, b.player_areas);
    assert_eq!(a, b);
    let other = RandomMapSpec {
        seed_text: "ZKB".to_string(),
        ..spec
    };
    let c = run(&vfs, &types, &other);
    assert_ne!(c.digests().heights, a.digests().heights);
    assert_ne!(c.rand_state, a.rand_state);
}

#[test]
fn ac_and_ea_give_the_same_map() {
    let Some((vfs, types)) = open() else { return };
    // "AC" and "EA" have the same seed 65 + 2*67 = 69 + 2*65 = 199.
    let spec = |seed: &str| RandomMapSpec {
        seed_text: seed.to_string(),
        ..base_spec(&types, terrain(&types, "rivers"))
    };
    let ac = run(&vfs, &types, &spec("AC"));
    let ea = run(&vfs, &types, &spec("EA"));
    assert_eq!((ac.hash, ea.hash), (199, 199));
    assert_eq!(ac.digests(), ea.digests());
    assert_eq!(ac.rand_counts, ea.rand_counts);
    assert_eq!(ac.rand_state, ea.rand_state);
    assert_eq!(ac.player_areas, ea.player_areas);
}

// QUIRK D-320: kept
#[test]
fn centre_tile_is_flag_without_koth() {
    let Some((vfs, types)) = open() else { return };
    // D-320: without `KING_OF_THE_HILL` all six scripts create the FLAG area (borders 49 %,
    // `numTiles 1`): it starts at `(x_min + x_max)/2` = 32/64/128. `#Texture FLAG` has a GUID
    // (30), so no draw. Measured (German edition with patch, seed ZKA, 4 players, meadow,
    // day): with `NORMAL` the center stays FLAG in all six map types and all three sizes -
    // no terrain area paints over it.
    for name in SCRIPTS {
        for size in 0..=2 {
            let spec = RandomMapSpec {
                size,
                ..base_spec(&types, terrain(&types, name))
            };
            let map = run(&vfs, &types, &spec);
            let flag = map
                .script
                .textures
                .iter()
                .find(|t| t.name == "FLAG")
                .unwrap();
            assert_eq!(flag.guids, [30], "{name}");
            let c = map.dim / 2;
            assert_eq!(c, [32, 64, 128][size as usize]);
            let centre = map.dim * c + c;
            assert_eq!(map.tile_numbers[centre], flag.number, "{name} {size}");
            assert_eq!(map.tiles[centre], 30, "{name} {size}");
            // The FLAG area has no `ID`, so it grows as ID 1 (D-330); no KotH field.
            assert_eq!(map.id_grid[centre], 1, "{name} {size}");
            assert!(!map.id_grid.contains(&10), "{name} {size}");
            assert_eq!(map.script.land_areas.len(), 1, "{name}");
        }
        // With `KING_OF_THE_HILL` the KotH area (ID 10, borders 45 %) grows first around the center; the
        // FLAG area sets its start tile afterwards unchecked (D-331): measured, the center stays
        // FLAG with ID 1 even then, the KotH area lies around it.
        let spec = RandomMapSpec {
            game_type: game_type(&types, "KING_OF_THE_HILL"),
            ..base_spec(&types, terrain(&types, name))
        };
        let map = run(&vfs, &types, &spec);
        assert_eq!(map.script.land_areas.len(), 2, "{name}");
        assert_eq!(map.script.land_areas[0].id, 10, "{name}");
        let koth = map
            .script
            .textures
            .iter()
            .find(|t| t.name == map.script.land_areas[0].texture)
            .unwrap()
            .number;
        let centre = map.dim * 64 + 64;
        assert_eq!(map.tiles[centre], 30, "{name}");
        assert_eq!(map.id_grid[centre], 1, "{name}");
        assert!(map.id_grid.contains(&10), "{name}");
        assert!(map.tile_numbers.contains(&koth), "{name}");
    }
}

#[test]
fn every_size_and_player_count_generates() {
    let Some((vfs, types)) = open() else { return };
    // Also combinations that the menu does not offer (64-tile with 8 players, 256-tile with 2)
    // run through without a panic and without a hang.
    let continental = terrain(&types, "continental");
    for size in 0..=2 {
        for players in 2..=8 {
            let spec = RandomMapSpec {
                size,
                players,
                ..base_spec(&types, continental)
            };
            let map = run(&vfs, &types, &spec);
            let dim = [64, 128, 256][size as usize];
            assert_eq!(map.dim, dim);
            assert_eq!(map.player_areas.len(), players as usize);
            assert_eq!(map.tiles.len(), dim * dim);
            assert_eq!(map.heights.len(), (dim + 1) * (dim + 1));
            assert_eq!(map.steep.len(), dim * dim);
        }
    }
}

// QUIRK D-336: kept
#[test]
fn generation_has_no_hidden_state() {
    let Some((vfs, types)) = open() else { return };
    // A, B, A in the same process -> A bit-identical; two threads in parallel -> equal.
    let a_spec = base_spec(&types, terrain(&types, "islands"));
    let b_spec = RandomMapSpec {
        seed_text: "Andere Karte".to_string(),
        size: 2,
        players: 8,
        ..base_spec(&types, terrain(&types, "continental"))
    };
    let first = run(&vfs, &types, &a_spec);
    let between = run(&vfs, &types, &b_spec);
    let again = run(&vfs, &types, &a_spec);
    // With the object step: geometry cache and rotation table are fresh per generation (the project
    // is read anew per map), nothing of B lingers.
    assert!(!first.objects.is_empty() && !between.objects.is_empty());
    assert_eq!(first, again);
    assert_eq!(first.digests(), again.digests());
    assert_eq!(first.objects, again.objects);
    assert_ne!(first.digests(), between.digests());
    // The load path in between changes nothing.
    let load = run_load_path(&vfs, &types, &a_spec);
    assert!(load.objects.is_empty());
    assert_eq!(run(&vfs, &types, &a_spec), first);
    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(|| run(&vfs, &types, &a_spec));
        let right = scope.spawn(|| run(&vfs, &types, &a_spec));
        (left.join().unwrap(), right.join().unwrap())
    });
    assert_eq!(left, first);
    assert_eq!(right, first);
}

#[test]
fn scene_has_objects() {
    let Some((vfs, types)) = open() else { return };
    let map = run(&vfs, &types, &base_spec(&types, terrain(&types, "lakes")));
    let scene = to_scene(&vfs, &map).unwrap();
    assert_eq!(scene.lvl_path, random_map_label(&map.spec));
    assert_eq!(scene.lvl_path, "randommap:ZKA");
    // World objects = placed objects in placement order, y = 0, only rotation about the vertical axis,
    // owner as color (`FUN_005474B0`).
    assert!(!map.objects.is_empty());
    assert_eq!(scene.world.objects.len(), map.objects.len());
    for (w, o) in scene.world.objects.iter().zip(&map.objects) {
        assert_eq!(w.guid, o.guid);
        assert_eq!(w.position, o.position);
        assert_eq!(w.position[1], 0.0);
        assert_eq!(w.rotation, [0.0, o.rotation_y, 0.0]);
        assert_eq!(w.scale, [1.0; 3]);
        assert_eq!(w.color, Some(o.owner));
        assert!(w.name.is_none() && w.config.is_empty());
    }
    // `TOWN_CENTER` (4200) become start points per owner, the others game and ambient objects;
    // whatever lies outside the map is discarded by the scene construction like in the original.
    let towns: Vec<_> = map.objects.iter().filter(|o| o.guid == 4200).collect();
    assert_eq!(towns.len(), 4);
    assert_eq!(
        scene.start_points.keys().copied().collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    for t in towns {
        assert_eq!(scene.start_points[&t.owner].to_array(), t.position);
    }
    assert!(!scene.objects.is_empty());
    assert!(scene.objects.len() <= map.objects.len() - 4);
    assert!(scene.objects.iter().all(|p| p.position.y == 0.0));
    assert!(scene.world.areas.is_empty());
    assert!(scene.level.players.is_empty());
    assert_eq!(scene.level.project_file, "Data\\Projects\\nml-wiese.prj");
    assert_eq!((scene.terrain.dim_x(), scene.terrain.dim_z()), (128, 128));
    // Object and shadow light from `light.inc`.
    assert_eq!(scene.world.light, map.script.object_light);
    assert!(scene.object_light.is_some() && scene.world.shadow.is_some());
    // Positions like `Terrain_AllocateGrid`: `(x·128, height, z·128)`.
    let v = scene.terrain.vertex(5, 7);
    assert_eq!(
        v.position.to_array(),
        [640.0, map.heights[129 * 7 + 5], 896.0]
    );
    // All tiles carry a texture that the project knows.
    assert!(scene.terrain.tiles.iter().all(Option::is_some));
    // Meadow: water level -10. Grid points from 10 units below it on are visibly water (alpha
    // > 0; just below -10, `water_alpha` still rounds to 0), points above the level are not.
    assert_eq!(scene.terrain.params.water_level, -10.0);
    let deep: Vec<_> = scene
        .terrain
        .vertices
        .iter()
        .filter(|v| v.position.y <= -20.0)
        .collect();
    assert!(!deep.is_empty());
    assert!(deep.iter().all(|v| v.water_alpha > 0));
    assert!(
        scene
            .terrain
            .vertices
            .iter()
            .filter(|v| v.position.y > -10.0)
            .all(|v| v.water_alpha == 0)
    );
}

/// Measured values per map type in the load path: the five terrain checksums ([`Digests::terrain`]), the
/// random number consumption of the five terrain phases ([`RandCounts::terrain`]), final state of the sequence.
struct Expected {
    script: &'static str,
    digests: [u64; 5],
    counts: [u32; 5],
    state: u32,
}

const fn counts(p: u32, l: u32, s: u32, t: u32, g: u32) -> [u32; 5] {
    [p, l, s, t, g]
}

const fn digests(
    heights: u64,
    tile_numbers: u64,
    tiles: u64,
    steep: u64,
    id_grid: u64,
) -> [u64; 5] {
    [heights, tile_numbers, tiles, steep, id_grid]
}

/// German edition with patch (reference), measured with the reimplementation; the comparison with the
/// original confirms it.
const GERMAN: [Expected; 6] = [
    Expected {
        script: "continental",
        digests: digests(
            15688425031788491322,
            8597255807492593194,
            6379432108878157932,
            14814969126023810954,
            13282550426888627812,
        ),
        counts: counts(3, 514, 28493, 137322, 8557),
        state: 2063471058,
    },
    Expected {
        script: "flatland",
        digests: digests(
            3662167105441457141,
            17209614479941476526,
            15611913174067853369,
            11248824735641314085,
            7408498344658048752,
        ),
        counts: counts(1, 514, 521, 18793, 13623),
        state: 3015942063,
    },
    Expected {
        script: "highland",
        digests: digests(
            14225864972993761210,
            515898577473626798,
            218373989439485581,
            12709006856134551297,
            5738537527996227056,
        ),
        counts: counts(0, 514, 521, 78382, 12661),
        state: 1342674081,
    },
    Expected {
        script: "islands",
        digests: digests(
            6098840426300009291,
            16450632185463177608,
            562365548064506997,
            9362813678678738318,
            7943208335420579395,
        ),
        counts: counts(0, 514, 28413, 86943, 8787),
        state: 2686877082,
    },
    Expected {
        script: "lakes",
        digests: digests(
            8019185744431526737,
            15354308520263740401,
            16729185770747567393,
            11248824735641314085,
            7408498344658048752,
        ),
        counts: counts(1, 514, 521, 104776, 10470),
        state: 2764206365,
    },
    Expected {
        script: "rivers",
        digests: digests(
            13733303506249053175,
            9983625089262788196,
            13518814774388066044,
            17939386152933399658,
            1892454617749300357,
        ),
        counts: counts(1, 514, 43246, 86505, 12918),
        state: 1979058755,
    },
];

// Confirmed by the comparison with the original (reference run, load path bit-identical: heights, GUIDs,
// steepness, vertex color, water alpha). Runs via the load path without the object step (D-341).
// QUIRK D-342: kept
// QUIRK D-341: deviation
#[test]
fn digests_per_script() {
    let Some((vfs, types)) = open() else { return };
    match game_version(&vfs).unwrap() {
        GameVersion::German => {
            for expected in &GERMAN {
                let spec = base_spec(&types, terrain(&types, expected.script));
                let map = run_load_path(&vfs, &types, &spec);
                assert_eq!(
                    map.digests().terrain(),
                    expected.digests,
                    "{}",
                    expected.script
                );
                assert_eq!(
                    map.rand_counts.terrain(),
                    expected.counts,
                    "{}",
                    expected.script
                );
                assert_eq!(map.rand_state, expected.state, "{}", expected.script);
                assert_eq!(map.rand_counts.objects, 0, "{}", expected.script);
                assert!(map.objects.is_empty() && map.object_block.is_empty());
                assert_eq!(map.slots_used, [false; 8]);
            }
        }
        // No measurements exist for the English edition (only the German installation is
        // available); there only structurally: every script runs, `Ok`, the same on repetition.
        GameVersion::English => {
            for name in SCRIPTS {
                let spec = base_spec(&types, terrain(&types, name));
                let map = run(&vfs, &types, &spec);
                assert_eq!(map.dim, 128);
                assert_eq!(run(&vfs, &types, &spec).digests(), map.digests(), "{name}");
            }
        }
    }
}

/// Measured (German edition with patch): GUIDs of the placed objects whose model has **no**
/// helper node `blocking01` (basis for D-356).
const WITHOUT_BLOCKING01: &[u32] = &[
    3411, 3500, 3503, 3505, 3908, 3909, 3916, 3917, 4000, 4001, 4002, 4011, 4012, 4013, 4200,
];

// QUIRK D-355, D-356: kept
#[test]
fn placed_object_models_load() {
    use opennml_formats::prj::parse_project;
    use opennml_formats::rdo::MeshKind;
    use opennml_formats::rnd::parse_random_map_script;
    use opennml_formats::text::decode_cp1252;
    use opennml_randommap::{script_and_defines, seed_hash};
    use opennml_world::assets::ModelCache;
    use opennml_world::rand::MsvcRand;
    use std::collections::BTreeSet;

    let Some((vfs, types)) = open() else { return };
    let mut cache = ModelCache::new();
    let mut guids: BTreeSet<u32> = BTreeSet::new();
    let mut with_key: BTreeSet<u32> = BTreeSet::new();
    let mut checked_scripts = 0;
    for name in SCRIPTS {
        for landscape in 0..types.landscape_types.len() {
            let spec = RandomMapSpec {
                landscape,
                ..base_spec(&types, terrain(&types, name))
            };
            let (path, defines) = script_and_defines(&spec, &types).unwrap();
            let mut rng = MsvcRand::new(seed_hash(&spec.seed_text));
            let script =
                parse_random_map_script(&path, defines, &mut |p| vfs.read(p).ok(), &mut || {
                    rng.rand() as i32
                })
                .unwrap_or_else(|e| panic!("{name}/{landscape}: {e:#}"));
            let project = parse_project(&decode_cp1252(
                &vfs.read(script.project_file.as_deref().unwrap()).unwrap(),
            ))
            .unwrap();
            for placement in &script.placements {
                let def = script
                    .objects
                    .iter()
                    .find(|o| o.name == placement.name)
                    .unwrap_or_else(|| {
                        panic!("{name}/{landscape}: #Object {} is missing", placement.name)
                    });
                for &guid in &def.guids {
                    let object = project.objects.get(&guid).unwrap_or_else(|| {
                        panic!("{name}/{landscape}: GUID {guid} is missing in the project")
                    });
                    assert!(
                        cache.get(&vfs, &object.model).is_some(),
                        "{name}/{landscape}: model {} (GUID {guid}) not loadable",
                        object.model
                    );
                    guids.insert(guid);
                    if object.random_rot {
                        with_key.insert(guid);
                    }
                }
            }
            checked_scripts += 1;
        }
    }
    assert_eq!(checked_scripts, SCRIPTS.len() * types.landscape_types.len());

    // Helper node `blocking01` (D-356): which models have it, with which rotation?
    let (project_path, _) = (
        {
            let spec = base_spec(&types, terrain(&types, "continental"));
            let (path, defines) = script_and_defines(&spec, &types).unwrap();
            let mut rng = MsvcRand::new(0);
            parse_random_map_script(&path, defines, &mut |p| vfs.read(p).ok(), &mut || {
                rng.rand() as i32
            })
            .unwrap()
            .project_file
            .unwrap()
        },
        (),
    );
    let project = parse_project(&decode_cp1252(&vfs.read(&project_path).unwrap())).unwrap();
    let mut without: Vec<u32> = Vec::new();
    let mut odd_rotation: Vec<u32> = Vec::new();
    let mut negative_w: Vec<u32> = Vec::new();
    for &guid in &guids {
        let model = cache.get(&vfs, &project.objects[&guid].model).unwrap();
        let helpers: Vec<_> = model
            .rdo
            .meshes
            .iter()
            .filter(|m| m.kind == MeshKind::Helper && m.name.eq_ignore_ascii_case("blocking01"))
            .collect();
        if helpers.is_empty() {
            without.push(guid);
        }
        // `q` and `−q` rotate the same: (±1, 0, 0, 0) is the identity rotation (the quaternion sandwich
        // `0x00578180` then returns `v` unchanged).
        let identity = |q: [f32; 4]| q[0].abs() == 1.0 && q[1] == 0.0 && q[2] == 0.0 && q[3] == 0.0;
        if helpers.iter().any(|m| !identity(m.rotation_wxyz)) {
            odd_rotation.push(guid);
        }
        if helpers.iter().any(|m| m.rotation_wxyz[0] == -1.0) {
            negative_w.push(guid);
        }
    }
    let without_key: Vec<u32> = guids.difference(&with_key).copied().collect();
    eprintln!("GUIDs: {}", guids.len());
    eprintln!("without blocking01: {without:?}");
    eprintln!("blocking01 without identity rotation (±1,0,0,0): {odd_rotation:?}");
    eprintln!(
        "blocking01 with w = -1: {} GUIDs: {negative_w:?}",
        negative_w.len()
    );
    eprintln!("without OBJ_RANDOM_ROT other than 0: {without_key:?}");
    assert_eq!(without, WITHOUT_BLOCKING01);
    assert!(odd_rotation.is_empty(), "{odd_rotation:?}");
    // All 31 models with a helper node carry the identity rotation as w = -1 (never +1).
    assert_eq!(negative_w.len(), guids.len() - WITHOUT_BLOCKING01.len());
    // `OBJ_RANDOM_ROT ≠ 0` is missing for the fish, fishing baskets and the start object 4200
    // (D-356); all other placed types carry the key.
    assert_eq!(without_key, [3411, 3500, 3503, 3505, 4200]);
}

/// Measured values per map type with the object step (restart): all seven checksums, the
/// random number consumption of all six phases, the final state of the sequence and the number of
/// objects per GUID.
struct ExpectedWithObjects {
    script: &'static str,
    digests: Digests,
    counts: [u32; 6],
    state: u32,
    per_guid: &'static [(u32, usize)],
}

/// German edition with patch, measured with the reimplementation (seed `ZKA`, 128-tile, 4 players,
/// meadow, day, normal). Heights, steepness and ID grid equal the load path; texture numbers and
/// tile GUIDs do not (`GroundTexture FARMLAND`, and step 14 draws after the object step).
/// Confirmed by the comparison with the original (reference run):
/// in all six map types, world objects (GUID, grid, rotation), object count per GUID,
/// heights, tile GUIDs and the byte field `Terrain+0x10` equal the memory of the original after the restart.
/// The Caribbean landscape (PALM block with `NoGrid`, D-365) is checked separately by
/// [`caribbean_matches_the_reference_run`]: it equals the original only with the node scale of the blocking
/// hull `blocking01`, which changes nothing for the meadow maps measured here. On the original, Caribbean
/// was measured only for continental.
const GERMAN_WITH_OBJECTS: [ExpectedWithObjects; 6] = [
    ExpectedWithObjects {
        script: "continental",
        digests: Digests {
            heights: 15688425031788491322,
            tile_numbers: 2075121993716469806,
            tiles: 4215108859017580376,
            steep: 14814969126023810954,
            id_grid: 13282550426888627812,
            objects: 9228365342573668170,
            object_block: 17113576798726631756,
        },
        counts: [3, 514, 28493, 137322, 8272, 160264],
        state: 183993271,
        per_guid: &[
            (3411, 20),
            (3500, 8),
            (3503, 8),
            (3505, 8),
            (3629, 214),
            (3630, 211),
            (3631, 238),
            (3700, 44),
            (3701, 34),
            (3702, 40),
            (3703, 38),
            (3904, 11),
            (3905, 12),
            (3906, 14),
            (3908, 35),
            (3909, 25),
            (4000, 11),
            (4001, 6),
            (4002, 11),
            (4011, 9),
            (4012, 5),
            (4013, 11),
            (4200, 4),
        ],
    },
    ExpectedWithObjects {
        script: "flatland",
        digests: Digests {
            heights: 3662167105441457141,
            tile_numbers: 16252622295459617642,
            tiles: 11611790845774798436,
            steep: 11248824735641314085,
            id_grid: 7408498344658048752,
            objects: 4292529307044455125,
            object_block: 15891036526564309438,
        },
        counts: [1, 514, 521, 18793, 13336, 149263],
        state: 3995892319,
        per_guid: &[
            (3500, 8),
            (3503, 8),
            (3629, 956),
            (3630, 939),
            (3631, 944),
            (3700, 29),
            (3701, 43),
            (3702, 44),
            (3703, 44),
            (3904, 13),
            (3905, 13),
            (3906, 14),
            (4200, 4),
        ],
    },
    ExpectedWithObjects {
        script: "highland",
        digests: Digests {
            heights: 14225864972993761210,
            tile_numbers: 10268062207652681902,
            tiles: 3384315002018112169,
            steep: 12709006856134551297,
            id_grid: 5738537527996227056,
            objects: 5125290496903929100,
            object_block: 3130124922091954485,
        },
        counts: [0, 514, 521, 78382, 12373, 145350],
        state: 3561982711,
        per_guid: &[
            (3500, 8),
            (3503, 8),
            (3629, 249),
            (3630, 244),
            (3631, 267),
            (3700, 29),
            (3701, 34),
            (3702, 43),
            (3703, 54),
            (3904, 14),
            (3905, 11),
            (3906, 15),
            (4200, 4),
        ],
    },
    ExpectedWithObjects {
        script: "islands",
        digests: Digests {
            heights: 6098840426300009291,
            tile_numbers: 13811429477951689480,
            tiles: 8403439977921973352,
            steep: 9362813678678738318,
            id_grid: 7943208335420579395,
            objects: 2510957688934023276,
            object_block: 289099582918076738,
        },
        counts: [0, 514, 28413, 86943, 8499, 159308],
        state: 403150630,
        per_guid: &[
            (3411, 20),
            (3500, 8),
            (3503, 8),
            (3505, 8),
            (3629, 126),
            (3630, 122),
            (3631, 111),
            (3700, 38),
            (3701, 44),
            (3702, 33),
            (3703, 29),
            (3904, 20),
            (3905, 10),
            (3906, 8),
            (3908, 33),
            (3909, 27),
            (4000, 13),
            (4001, 8),
            (4002, 10),
            (4011, 7),
            (4012, 14),
            (4013, 8),
            (4200, 4),
        ],
    },
    ExpectedWithObjects {
        script: "lakes",
        digests: Digests {
            heights: 8019185744431526737,
            tile_numbers: 8371694920383829233,
            tiles: 1149630416873092257,
            steep: 11248824735641314085,
            id_grid: 7408498344658048752,
            objects: 15692312306229960209,
            object_block: 12835027147354562036,
        },
        counts: [1, 514, 521, 104776, 10186, 159983],
        state: 3055089430,
        per_guid: &[
            (3411, 30),
            (3500, 8),
            (3503, 8),
            (3505, 8),
            (3629, 128),
            (3630, 137),
            (3631, 130),
            (3700, 44),
            (3701, 38),
            (3702, 44),
            (3703, 34),
            (3904, 17),
            (3905, 12),
            (3906, 11),
            (3908, 33),
            (3909, 27),
            (4000, 14),
            (4001, 13),
            (4002, 7),
            (4011, 8),
            (4012, 6),
            (4013, 12),
            (4200, 4),
        ],
    },
    ExpectedWithObjects {
        script: "rivers",
        digests: Digests {
            heights: 13733303506249053175,
            tile_numbers: 15054948343759137764,
            tiles: 7375118373567604636,
            steep: 17939386152933399658,
            id_grid: 1892454617749300357,
            objects: 17233477264590157033,
            object_block: 16991921356803119169,
        },
        counts: [1, 514, 43246, 86505, 12644, 162224],
        state: 2437559265,
        per_guid: &[
            (3411, 21),
            (3500, 8),
            (3503, 8),
            (3505, 8),
            (3629, 389),
            (3630, 346),
            (3631, 352),
            (3700, 43),
            (3701, 32),
            (3702, 40),
            (3703, 45),
            (3904, 14),
            (3905, 12),
            (3906, 14),
            (3908, 35),
            (3909, 25),
            (4000, 12),
            (4001, 6),
            (4002, 11),
            (4011, 16),
            (4012, 5),
            (4013, 10),
            (4200, 4),
        ],
    },
];

/// Checksums with the object step per map type (meadow), confirmed by the comparison with the original
/// (reference run, see [`GERMAN_WITH_OBJECTS`]).
// QUIRK D-365: kept
// QUIRK D-341: deviation
#[test]
fn digests_with_objects_per_script() {
    let Some((vfs, types)) = open() else { return };
    for name in SCRIPTS {
        let spec = base_spec(&types, terrain(&types, name));
        let map = run(&vfs, &types, &spec);
        let load = run_load_path(&vfs, &types, &spec);
        // In all editions: the object step does not change heights, steepness and ID grid, the
        // terrain phases before it draw the same amount; four `TOWN_CENTER` occupy the slots 0 ... 3.
        let (d, l) = (map.digests(), load.digests());
        assert_eq!(
            (d.heights, d.steep, d.id_grid),
            (l.heights, l.steep, l.id_grid),
            "{name}"
        );
        assert_eq!(
            map.rand_counts.terrain()[..4],
            load.rand_counts.terrain()[..4],
            "{name}"
        );
        assert!(map.rand_counts.objects > 0, "{name}");
        assert_eq!(per_guid(&map).get(&4200), Some(&4), "{name}");
        assert_eq!(
            map.slots_used,
            [true, true, true, true, false, false, false, false],
            "{name}"
        );
        assert_eq!(map.object_block.len(), map.steep.len(), "{name}");
        assert!(map.objects.iter().all(|o| o.position[1] == 0.0), "{name}");
    }
    if game_version(&vfs).unwrap() != GameVersion::German {
        // No measurements exist for the English edition (only the German installation).
        return;
    }
    for expected in &GERMAN_WITH_OBJECTS {
        let spec = base_spec(&types, terrain(&types, expected.script));
        let map = run(&vfs, &types, &spec);
        let c = map.rand_counts;
        let counts = [
            c.preprocessor,
            c.land_areas,
            c.player_areas,
            c.terrain_areas,
            c.texture_guids,
            c.objects,
        ];
        assert_eq!(map.digests(), expected.digests, "{}", expected.script);
        assert_eq!(counts, expected.counts, "{}", expected.script);
        assert_eq!(map.rand_state, expected.state, "{}", expected.script);
        let per: Vec<(u32, usize)> = per_guid(&map).into_iter().collect();
        assert_eq!(per, expected.per_guid, "{}", expected.script);
    }
}

/// Caribbean (D-365), measured on the original (reference run, seed `ZKA`, continental, 128-tile,
/// 4 players, Caribbean, day, normal): number of objects per GUID as in the memory of the original
/// after the restart, plus the effects 3411 (20) and start objects 4200 (4), which the comparison
/// filters out. With the node scale of the blocking hull `blocking01` (palme04...06, 20/29)
/// objects, rotations and byte field equal the original.
const GERMAN_CARIBBEAN_PER_GUID: [(u32, usize); 35] = [
    (3411, 20),
    (3500, 8),
    (3503, 8),
    (3505, 8),
    (3625, 97),
    (3626, 91),
    (3627, 92),
    (3628, 82),
    (3629, 23),
    (3630, 16),
    (3631, 41),
    (3632, 73),
    (3633, 97),
    (3634, 19),
    (3635, 16),
    (3636, 19),
    (3637, 12),
    (3638, 21),
    (3639, 20),
    (3700, 31),
    (3701, 27),
    (3702, 38),
    (3703, 36),
    (3904, 13),
    (3905, 17),
    (3906, 7),
    (3908, 28),
    (3909, 32),
    (4000, 13),
    (4001, 11),
    (4002, 12),
    (4011, 7),
    (4012, 11),
    (4013, 6),
    (4200, 4),
];

/// Caribbean map with `NoGrid` palms against the reference run (D-365): the checksums `objects` and
/// `object_block` come from the counter-check of the debug copy with node scale, whose objects (slot
/// 8/9/10 = 8/719/305, all rotations) and byte field (with the `OBJ_BLOCK_PATH` rectangles at 0)
/// equal the memory of the original.
// QUIRK D-365: kept
#[test]
fn caribbean_matches_the_reference_run() {
    let Some((vfs, types)) = open() else { return };
    let spec = RandomMapSpec {
        landscape: types
            .landscape_types
            .iter()
            .position(|l| l == "CARIBBEAN")
            .unwrap(),
        ..base_spec(&types, terrain(&types, "continental"))
    };
    let map = run(&vfs, &types, &spec);
    let d = map.digests();
    eprintln!(
        "Caribbean: {} objects, objects {}, object_block {}",
        map.objects.len(),
        d.objects,
        d.object_block
    );
    if game_version(&vfs).unwrap() != GameVersion::German {
        return;
    }
    let per: Vec<(u32, usize)> = per_guid(&map).into_iter().collect();
    assert_eq!(per, GERMAN_CARIBBEAN_PER_GUID);
    assert_eq!(map.objects.len(), 1056);
    assert_eq!(
        (d.objects, d.object_block),
        (17240712984053409879, 11225647852868771634)
    );
}

// QUIRK D-323: kept
#[test]
fn continental_tree_block_per_landscape() {
    let Some((vfs, types)) = open() else { return };
    // D-323: the tree block of continental (`number 5`, `numGroups 20`, `ForAllPlayers`,
    // distance 8 ... 10, `maxGroupRadius 0`) places `TREE_SMALL` around every player for meadow, prairie
    // and Caribbean, `CONIFER_WINTER` for winter, nothing for desert; `PALM2` appears only for Caribbean
    // from the forest block (`Fill`), never from `place_object PALM2`.
    let continental = terrain(&types, "continental");
    let landscape = |name: &str| {
        types
            .landscape_types
            .iter()
            .position(|l| l == name)
            .unwrap()
    };
    let guids_of = |map: &RandomMap, name: &str| -> Vec<u32> {
        map.script
            .objects
            .iter()
            .find(|o| o.name == name)
            .map(|o| o.guids.clone())
            .unwrap_or_default()
    };
    // Objects with one of the GUIDs in the ring 7 ... 11 tiles (distance 8 ... 10, snapping ±1) around
    // the player area `area`.
    let ring_count = |map: &RandomMap, guids: &[u32], area: usize| {
        let a = map.player_areas[area];
        map.objects
            .iter()
            .filter(|o| guids.contains(&o.guid))
            .filter(|o| {
                let dx = o.position[0] / 128.0 - a.cx as f32;
                let dz = o.position[2] / 128.0 - a.cz as f32;
                (49.0..=121.0).contains(&(dx * dx + dz * dz))
            })
            .count()
    };
    for (name, head) in [
        ("MEADOW", Some("TREE_SMALL")),
        ("PRAIRIE", Some("TREE_SMALL")),
        ("CARIBBEAN", Some("TREE_SMALL")),
        ("WINTER", Some("CONIFER_WINTER")),
        ("DESERT", None),
    ] {
        let spec = RandomMapSpec {
            landscape: landscape(name),
            ..base_spec(&types, continental)
        };
        let map = run(&vfs, &types, &spec);
        let tree_block: Vec<_> = map
            .script
            .placements
            .iter()
            .filter(|p| p.number == 5 && p.num_groups == 20 && p.for_all_players)
            .collect();
        assert_eq!(map.player_areas.len(), 4);
        match head {
            Some(head) => {
                assert_eq!(tree_block.len(), 1, "{name}");
                assert_eq!(tree_block[0].name, head, "{name}");
                let guids = guids_of(&map, head);
                for area in 0..4 {
                    assert!(ring_count(&map, &guids, area) > 0, "{name}: player {area}");
                }
            }
            None => assert!(tree_block.is_empty(), "{name}"),
        }
        let palm2 = guids_of(&map, "PALM2");
        // The `PALM2` GUIDs belong to no other `#Object`.
        for other in map.script.objects.iter().filter(|o| o.name != "PALM2") {
            assert!(other.guids.iter().all(|g| !palm2.contains(g)), "{name}");
        }
        let palms = map
            .objects
            .iter()
            .filter(|o| palm2.contains(&o.guid))
            .count();
        assert_eq!(palms > 0, name == "CARIBBEAN", "{name}: {palms}");
        if name == "CARIBBEAN" {
            // `TREE_SMALL` only from the tree block: 4 players * 20 groups; the members (radius 0)
            // fall on the occupied tile of their center, all centers lie in the ring.
            let small = guids_of(&map, "TREE_SMALL");
            let total = map
                .objects
                .iter()
                .filter(|o| small.contains(&o.guid))
                .count();
            assert_eq!(total, 80);
            let in_rings: usize = (0..4).map(|a| ring_count(&map, &small, a)).sum();
            assert_eq!(in_rings, 80);
            let fill: Vec<_> = map.script.placements.iter().filter(|p| p.fill).collect();
            assert_eq!(fill.len(), 1);
            assert_eq!(fill[0].name, "PALM2");
        }
        if name == "DESERT" {
            let conifer = guids_of(&map, "CONIFER_WINTER");
            assert!(map.objects.iter().all(|o| !conifer.contains(&o.guid)));
        }
    }
}

#[test]
fn koth_places_one_flag_on_the_centre() {
    let Some((vfs, types)) = open() else { return };
    // `KING_OF_THE_HILL` (`Fill` on `FLAG`): exactly one flag 4300, neutral (owner 8), on the
    // center tile 64/64 of the 128-tile map, snapped to the tile center.
    for name in SCRIPTS {
        let spec = RandomMapSpec {
            game_type: game_type(&types, "KING_OF_THE_HILL"),
            ..base_spec(&types, terrain(&types, name))
        };
        let map = run(&vfs, &types, &spec);
        let flags: Vec<_> = map.objects.iter().filter(|o| o.guid == 4300).collect();
        assert_eq!(flags.len(), 1, "{name}");
        assert_eq!(flags[0].owner, 8, "{name}");
        assert_eq!(flags[0].position, [8256.0, 0.0, 8256.0], "{name}");
        // Without KotH no flag.
        let normal = run(&vfs, &types, &base_spec(&types, terrain(&types, name)));
        assert!(normal.objects.iter().all(|o| o.guid != 4300), "{name}");
    }
}

/// D-364: rules of a random map without `.lvl`. Active slots = `slots_used` of the generator,
/// without level `[RESOURCES]` and slot configs (time-of-day stage 0 even for winter and night),
/// world objects = placed objects, no level script, no fog, no start environment; checksum
/// repeatable and dependent on the seed.
// QUIRK D-364: kept
// QUIRK D-363: deviation
#[test]
fn random_rules_load_without_lvl() {
    use opennml_randommap::{build_level, rules_input};
    use opennml_sim::rules::Rules;
    use opennml_world::environment::{GameMode, level_game_mode, level_start_environment};
    let Some((vfs, types)) = open() else { return };
    let spec = base_spec(&types, terrain(&types, "continental"));
    let level = build_level(&vfs, &spec).unwrap();
    assert_eq!(level.rules, rules_input(&level.map));
    assert_eq!(level.rules.spec, spec);
    assert_eq!(level.rules.slots, level.map.slots_used);
    assert_eq!(level.rules.crt_start, level.map.rand_state);
    assert!(!level.rules.island);
    assert_eq!(level.scene.lvl_path, random_map_label(&spec));
    let rules = Rules::load_random(&vfs, &level.scene, level.rules.clone()).unwrap();
    assert_eq!(rules.random_map.as_ref(), Some(&level.rules));
    assert_eq!(rules.level_path, "randommap:ZKA");
    // One player per occupied slot, without a name, nation 0, human, without resources.
    let slots: Vec<u8> = (0..8u8)
        .filter(|&s| level.map.slots_used[usize::from(s)])
        .collect();
    assert_eq!(slots, [0, 1, 2, 3], "ZKA, 4 players");
    assert_eq!(
        rules
            .level
            .players
            .iter()
            .map(|p| p.slot)
            .collect::<Vec<_>>(),
        slots
    );
    for p in &rules.level.players {
        assert_eq!((p.name.as_str(), p.nation, p.kind), ("", 0, 0));
        assert!(p.resources.is_empty());
    }
    assert!(rules.level.level_resources.is_empty());
    assert!(rules.level.slot_configs.is_empty());
    assert_eq!(rules.level_daytime(), 0);
    assert_eq!(rules.world_objects.len(), level.map.objects.len());
    assert!(!rules.world_objects.is_empty());
    // No level script, no dialogs, no areas.
    assert!(rules.scripts.level.is_none());
    assert!(rules.scripts.dialogs.is_empty() && rules.scripts.areas.is_empty());
    // No fog, no start environment, free play (D-363).
    assert!(level.scene.world.fog.is_none());
    let env = level_start_environment(&rules.level_path);
    assert!(env.light.is_none() && env.weather.is_none() && env.music.is_empty());
    assert_eq!(level_game_mode(&rules.level_path), GameMode::Multiplayer);
    // Checksum: equal on repetition, different with another seed.
    let again = build_level(&vfs, &spec).unwrap();
    let again = Rules::load_random(&vfs, &again.scene, again.rules).unwrap();
    assert_eq!(again.checksum, rules.checksum);
    let other_spec = RandomMapSpec {
        seed_text: "ZKB".into(),
        ..spec.clone()
    };
    let other = build_level(&vfs, &other_spec).unwrap();
    assert_ne!(other.rules.digest, level.rules.digest);
    let other = Rules::load_random(&vfs, &other.scene, other.rules).unwrap();
    assert_ne!(other.checksum, rules.checksum);
    // Winter and night: the time-of-day upgrades stay at stage 0 (D-364).
    let winter_night = RandomMapSpec {
        landscape: types
            .landscape_types
            .iter()
            .position(|l| l == "WINTER")
            .unwrap(),
        daytime: types
            .daytime_types
            .iter()
            .position(|d| d == "NIGHT")
            .unwrap(),
        ..spec.clone()
    };
    let wn = build_level(&vfs, &winter_night).unwrap();
    let wn = Rules::load_random(&vfs, &wn.scene, wn.rules).unwrap();
    assert_eq!(wn.level_daytime(), 0);
    assert_ne!(wn.checksum, rules.checksum);
}

/// The files of the generator (`randommaptypes.ini`, the `.rnd` scripts) are read before the rules and are not part of
/// `RuleChecksums::logic`: loading a random map savegame does not need the generator. What the generator made counts
/// in `derived` (its checksum `digest`).
#[test]
fn random_map_logic_leaves_out_the_generator() {
    use opennml_randommap::{RANDOM_MAP_TYPES_PATH, build_level};
    use opennml_sim::rules::Rules;
    use std::collections::BTreeSet;
    let Some((vfs, types)) = open() else { return };
    let is_generator_file = |path: &str| path == RANDOM_MAP_TYPES_PATH || path.ends_with(".rnd");
    let (mut logic, mut derived) = (BTreeSet::new(), BTreeSet::new());
    for name in SCRIPTS {
        let (generator, generator_log) = vfs.recording();
        let level = build_level(&generator, &base_spec(&types, terrain(&types, name))).unwrap();
        let generator_files: Vec<String> = generator_log
            .take()
            .into_iter()
            .map(|e| e.path)
            .filter(|p| is_generator_file(p))
            .collect();
        assert!(generator_files.len() >= 2, "{name}: {generator_files:?}");
        let (reader, rules_log) = vfs.recording();
        let rules = Rules::load_random(&reader, &level.scene, level.rules).unwrap();
        let read: Vec<String> = rules_log.take().into_iter().map(|e| e.path).collect();
        assert!(!read.is_empty(), "{name}");
        assert!(!read.iter().any(|p| is_generator_file(p)), "{name}");
        logic.insert(rules.checksums.logic);
        derived.insert(rules.checksums.derived);
    }
    // Every map script reads the same shared files: one `logic`, but one `derived` per map.
    assert_eq!((logic.len(), derived.len()), (1, SCRIPTS.len()));
}

/// E-82: the island flag follows the map script - only `Islands.rnd` yields 1, the other five 0;
/// the AI query `0x5130` returns it.
// QUIRK E-82: kept
#[test]
fn island_query_follows_the_script() {
    use opennml_randommap::build_level;
    use opennml_sim::ki::query::query_once;
    use opennml_sim::player::Sight;
    use opennml_sim::rules::Rules;
    use opennml_sim::sim::Sim;
    use opennml_sim::state::SimState;
    let Some((vfs, types)) = open() else { return };
    for name in SCRIPTS {
        let level = build_level(&vfs, &base_spec(&types, terrain(&types, name))).unwrap();
        let expected = name == "islands";
        assert_eq!(level.rules.island, expected, "{name}");
        let rules = Rules::load_random(&vfs, &level.scene, level.rules).unwrap();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let (req, _) = query_once(&mut sim, 0, Sight::Omniscient, 0x5130, &[]);
        assert_eq!(req[2], i32::from(expected), "{name}");
    }
}

// --- Match start ---

/// Random map match like the restart: `build_level` -> `Rules::load_random` -> `new_match` with
/// game type = GTYPE index.
fn random_match(
    vfs: &Vfs,
    spec: &RandomMapSpec,
    lobby: Vec<opennml_sim::setup::LobbyEntry>,
    resource_setting: i32,
) -> (
    opennml_randommap::RandomLevel,
    opennml_sim::rules::Rules,
    opennml_sim::state::SimState,
) {
    use opennml_sim::setup::{MatchSetup, new_match};
    let level = opennml_randommap::build_level(vfs, spec).unwrap();
    let rules =
        opennml_sim::rules::Rules::load_random(vfs, &level.scene, level.rules.clone()).unwrap();
    let setup = MatchSetup {
        mode: spec.game_type as i32,
        lobby,
        resource_setting,
        ..MatchSetup::default()
    };
    let state = new_match(&rules, &setup).unwrap_or_else(|e| panic!("{spec:?}: {e}"));
    (level, rules, state)
}

/// Lobby of the reference run after `Game_StartLevel` (entries "Open" removed, O-432): human, AI.
fn human_and_ai() -> Vec<opennml_sim::setup::LobbyEntry> {
    use opennml_sim::setup::{LobbyEntry, SlotKind};
    let entry = |kind, name: &str, team| LobbyEntry {
        kind,
        name: name.into(),
        nation: 0,
        team,
        color: team,
        handicap: 0,
    };
    vec![
        entry(SlotKind::Human, "Mensch", 0),
        entry(SlotKind::Ai, "KI", 1),
    ]
}

/// Tile of a world position (`ftol(x / 128)`, truncated).
fn tile_of(p: [f32; 3]) -> (i32, i32) {
    ((p[0] / 128.0) as i32, (p[2] / 128.0) as i32)
}

/// Objects of a slot in table order.
fn table_objects(
    state: &opennml_sim::state::SimState,
    slot: usize,
) -> Vec<&opennml_sim::object::GameObject> {
    state.tables[slot].objects.iter().flatten().collect()
}

/// O-432 (reference run): lobby human, AI, open, open on continental with 4 players -
/// slots 0 ... 3 active, 0 human and 1 AI with HQ 1300 on the `TOWN_CENTER` tile and workers, 2
/// and 3 active without a player and without start objects; the deposits stand on all four start areas.
// QUIRK O-432: kept
#[test]
fn lobby_human_ai_open_open() {
    use opennml_sim::state::Control;
    let Some((vfs, types)) = open() else { return };
    let spec = base_spec(&types, terrain(&types, "continental"));
    let (level, _rules, state) = random_match(&vfs, &spec, human_and_ai(), 0);
    let active: Vec<bool> = state
        .players
        .iter()
        .take(8)
        .map(|p| p.slot_active)
        .collect();
    assert_eq!(active, [true, true, true, true, false, false, false, false]);
    let controls: Vec<Control> = state.players.iter().take(4).map(|p| p.control).collect();
    assert_eq!(
        controls,
        [Control::Human, Control::Ai, Control::Empty, Control::Empty]
    );
    let centres: Vec<_> = level
        .map
        .objects
        .iter()
        .filter(|o| o.guid == 4200)
        .collect();
    assert_eq!(
        centres.iter().map(|o| o.owner).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    for c in &centres {
        let slot = c.owner as usize;
        let objects = table_objects(&state, slot);
        if slot < 2 {
            let hq = objects
                .iter()
                .find(|o| o.guid == 1300)
                .unwrap_or_else(|| panic!("slot {slot} without HQ 1300"));
            assert_eq!(tile_of(hq.world_pos), tile_of(c.position), "slot {slot}");
            assert!(objects.len() > 1, "slot {slot}: workers missing");
        } else {
            assert!(objects.is_empty(), "slot {slot}");
        }
        // Gold (8 ... 12 tiles from the center, `place_Object GOLD` with `ForAllPlayers`) in the world.
        let (cx, cz) = tile_of(c.position);
        let gold_near = state.objects().filter(|o| o.guid == 3500).any(|o| {
            let (x, z) = tile_of(o.world_pos);
            (x - cx).abs().max((z - cz).abs()) <= 13
        });
        assert!(gold_near, "slot {slot}: no gold on the start area");
    }
}

/// Trees of the world in creation order (slot 9, table index ascending; the simulation has not
/// removed anything yet at the start).
fn tree_rot_counts(state: &opennml_sim::state::SimState) -> Vec<i32> {
    use opennml_sim::module::ModuleKind;
    table_objects(state, 9)
        .into_iter()
        .map(|o| match o.module(4).map(|m| &m.kind) {
            Some(ModuleKind::Tree(t)) => t.rot_count,
            _ => panic!("slot 9 without a tree module: {}", o.guid),
        })
        .collect()
}

/// D-342: `SimState.crt` starts at the final state of the generator, every created tree draws
/// one value in order of success (`PropertyTree_Construct` `0x00531BCF`); the other objects and the
/// start objects draw nothing.
// QUIRK D-342: kept
#[test]
fn crt_starts_at_the_generator_end_plus_one_per_tree() {
    use opennml_sim::module::tree::rot_count;
    use opennml_sim::rng::CrtRand;
    let Some((vfs, types)) = open() else { return };
    for name in SCRIPTS {
        let spec = base_spec(&types, terrain(&types, name));
        let (level, _rules, state) = random_match(&vfs, &spec, human_and_ai(), 0);
        let trees = tree_rot_counts(&state);
        assert!(!trees.is_empty(), "{name}");
        let mut crt = CrtRand::new(level.map.rand_state);
        let expected: Vec<i32> = trees.iter().map(|_| rot_count(crt.rand())).collect();
        assert_eq!(trees, expected, "{name}");
        assert_eq!(state.crt, crt, "{name}");
    }
}

/// W-61 (deviation): the fishing baskets `FISHING` 3505 draw nothing from `crt` in the reimplementation -
/// in the original 10 draws per basket (`FishEffect_Construct`, reference run). continental has baskets.
// QUIRK D-342: kept
// QUIRK W-61: deviation
#[test]
fn fishing_baskets_draw_nothing() {
    use opennml_sim::rng::CrtRand;
    let Some((vfs, types)) = open() else { return };
    let spec = base_spec(&types, terrain(&types, "continental"));
    let (level, _rules, state) = random_match(&vfs, &spec, human_and_ai(), 0);
    let baskets = state.objects().filter(|o| o.guid == 3505).count();
    let generated = level.map.objects.iter().filter(|o| o.guid == 3505).count();
    assert!(baskets > 0, "continental without a fishing basket");
    assert_eq!(baskets, generated);
    let mut crt = CrtRand::new(level.map.rand_state);
    for _ in 0..state.tables[9].count {
        crt.rand();
    }
    assert_eq!(state.crt, crt, "{baskets} baskets");
}

/// W-6: on the random map even stage 0 sets the starting resources (`values[0]` of all
/// start records) and the deposits (gold 3500: `CONFIG_GUID 26`, `values[stage]`).
// QUIRK W-6: kept
#[test]
fn deposits_use_resource_level_zero() {
    let Some((vfs, types)) = open() else { return };
    let spec = base_spec(&types, terrain(&types, "continental"));
    for stage in [0, 2] {
        let (_level, rules, state) = random_match(&vfs, &spec, human_and_ai(), stage);
        let gold = rules.resource_config.object_record(3500).unwrap();
        assert_eq!(gold.config_guid, 26);
        let amount = gold.values[stage as usize];
        let golds: Vec<_> = state.objects().filter(|o| o.guid == 3500).collect();
        assert!(!golds.is_empty());
        for o in golds {
            assert_eq!(o.attributes.current_value(26, 0), amount, "stage {stage}");
        }
        for r in rules.resource_config.start_records() {
            for slot in 0..8 {
                assert_eq!(
                    state.resource(slot, r.config_guid as usize),
                    r.values[stage as usize],
                    "stage {stage}, slot {slot}, resource {}",
                    r.config_guid
                );
            }
        }
    }
    // Shipped data (German edition with patch): gold 2000/3000/5000/8000.
    if game_version(&vfs) == Ok(GameVersion::German) {
        let (_l, rules, _s) = random_match(&vfs, &spec, human_and_ai(), 0);
        assert_eq!(
            rules.resource_config.object_record(3500).unwrap().values,
            [2000, 3000, 5000, 8000]
        );
    }
}

/// Domination (game type 2): the start objects of occupied slots bring the player flag 4300 along
/// (type neutral, category `0x400` -> slot 8) - one each for human and AI on their start area; the
/// slots without a player get none (re/random-map Z.8 step 3).
#[test]
fn domination_random_map_places_player_flags() {
    let Some((vfs, types)) = open() else { return };
    // GTYPE 0 ... 2 all stand on `NORMAL` (same map); the index is the game type.
    let spec = RandomMapSpec {
        game_type: 2,
        ..base_spec(&types, terrain(&types, "continental"))
    };
    assert_eq!(types.game_types[2].gtype, "NORMAL");
    let (level, _rules, state) = random_match(&vfs, &spec, human_and_ai(), 0);
    let flags: Vec<_> = state.objects().filter(|o| o.guid == 4300).collect();
    assert_eq!(flags.len(), 2);
    assert!(flags.iter().all(|o| o.id.slot() == 8));
    // Nearest start area of each flag: slot 0 and slot 1.
    let centres: Vec<_> = level
        .map
        .objects
        .iter()
        .filter(|o| o.guid == 4200)
        .collect();
    let mut owners: Vec<i32> = flags
        .iter()
        .map(|f| {
            let (fx, fz) = tile_of(f.world_pos);
            centres
                .iter()
                .min_by_key(|c| {
                    let (cx, cz) = tile_of(c.position);
                    (cx - fx).abs().max((cz - fz).abs())
                })
                .unwrap()
                .owner
        })
        .collect();
    owners.sort();
    assert_eq!(owners, [0, 1]);
}

/// King of the Hill (game type 3): exactly one flag 4300 on slot 8 (owner 8 with slot flag) on the
/// center tile; the players have none.
#[test]
fn koth_random_map_has_one_neutral_flag() {
    let Some((vfs, types)) = open() else { return };
    let spec = RandomMapSpec {
        game_type: game_type(&types, "KING_OF_THE_HILL"),
        ..base_spec(&types, terrain(&types, "continental"))
    };
    assert_eq!(spec.game_type, 3);
    let (_level, _rules, state) = random_match(&vfs, &spec, human_and_ai(), 0);
    let flags: Vec<_> = state.objects().filter(|o| o.guid == 4300).collect();
    assert_eq!(flags.len(), 1);
    assert_eq!(flags[0].id.slot(), 8);
    assert_eq!(tile_of(flags[0].world_pos), (64, 64));
}

/// Comparison key of a world object: GUID, bits of the rotation Y, tile.
type PlacedKey = (u32, u32, (i32, i32));

/// `FUN_005474B0`: every generator object except `TOWN_CENTER` and the ambient effects is created
/// with flags `0x8D3` - slot from the category or the owner, rotation only about the vertical
/// axis (bits equal), scale 1, height from the terrain; per slot in placement order.
#[test]
fn random_objects_use_flags_0x8d3() {
    use opennml_sim::setup::{AMBIENT, START_POINT};
    let Some((vfs, types)) = open() else { return };
    for name in SCRIPTS {
        let spec = base_spec(&types, terrain(&types, name));
        let (level, rules, state) = random_match(&vfs, &spec, human_and_ai(), 0);
        let mut want: BTreeMap<usize, Vec<PlacedKey>> = BTreeMap::new();
        for o in &level.map.objects {
            if o.guid == START_POINT || AMBIENT.contains(&o.guid) {
                continue;
            }
            let category = rules.types.get(&o.guid).map_or(0, |t| t.category());
            let slot = if category & 0x100 != 0 {
                9
            } else if category & 0x200 != 0 {
                10
            } else if category & 0x400 != 0 {
                8
            } else {
                o.owner as usize
            };
            want.entry(slot).or_default().push((
                o.guid,
                o.rotation_y.to_bits(),
                tile_of(o.position),
            ));
        }
        for (&slot, list) in &want {
            // Player slots carry the start objects before them; the generator objects are compared.
            let got: Vec<_> = table_objects(&state, slot)
                .into_iter()
                .filter(|o| list.iter().any(|w| w.0 == o.guid))
                .map(|o| {
                    assert_eq!(o.rotation[0], 0.0, "{name}");
                    assert_eq!(o.rotation[2], 0.0, "{name}");
                    assert_eq!(o.scale, [1.0; 3], "{name}");
                    (o.guid, o.rotation[1].to_bits(), tile_of(o.world_pos))
                })
                .collect();
            assert_eq!(got.len(), list.len(), "{name}, slot {slot}");
            for (g, w) in got.iter().zip(list) {
                assert_eq!(g, w, "{name}, slot {slot}");
            }
        }
        // Height: the position comes with y = 0, the world snaps onto the terrain (only checkable
        // where it is not 0 everywhere).
        if level.map.heights.iter().any(|&h| h != 0.0) {
            assert!(
                state
                    .objects()
                    .filter(|o| o.id.slot() >= 8)
                    .any(|o| o.world_pos[1] != 0.0),
                "{name}"
            );
        }
    }
}

// --- Savegame and load path (D-341, D-348) ---

/// Setup like [`random_match`] (game type = GTYPE index).
fn random_setup(
    spec: &RandomMapSpec,
    lobby: Vec<opennml_sim::setup::LobbyEntry>,
) -> opennml_sim::setup::MatchSetup {
    opennml_sim::setup::MatchSetup {
        mode: spec.game_type as i32,
        lobby,
        ..opennml_sim::setup::MatchSetup::default()
    }
}

/// Load path like `opennml-client` (`MapSource::of_start` -> `MapLoad::open` -> `start_sim_with`): read the savegame,
/// regenerate the map from the map description of the start package (with the object step, D-341 deviation),
/// rules without `.lvl`, check against the rules; the state (objects, CRT) comes from the savegame.
fn load_random_save(
    vfs: &Vfs,
    bytes: &[u8],
) -> (
    opennml_randommap::RandomLevel,
    opennml_sim::rules::Rules,
    opennml_sim::save::SaveGame,
) {
    use opennml_sim::save::SaveGame;
    let save = SaveGame::from_bytes(bytes).unwrap();
    let spec = save
        .start
        .random_map
        .clone()
        .expect("map description in the start package");
    assert_eq!(save.start.level, random_map_label(&spec));
    let level = opennml_randommap::build_level(vfs, &spec).unwrap();
    let rules =
        opennml_sim::rules::Rules::load_random(vfs, &level.scene, level.rules.clone()).unwrap();
    save.check(&rules, opennml_sim::save::RulesCompare::Full)
        .unwrap();
    (level, rules, save)
}

/// D-341 (deviation), D-342: play a match for 200 steps, save, load -> tiles as at the start (all seven
/// generator checksums and the rules checksum equal), objects from the savegame (state equal), and the checksum
/// of every following step equal to the unsaved match. The original generates without the object step when loading
/// (different ground) and saves the rotation Y lossily; the reimplementation reproduces neither.
// QUIRK D-341: deviation
#[test]
fn save_and_load_continue_bitwise() {
    use opennml_sim::save::{SaveGame, checksum};
    use opennml_sim::setup::start_package;
    use opennml_sim::tick::step;
    let Some((vfs, types)) = open() else { return };
    for name in ["continental", "islands"] {
        let spec = base_spec(&types, terrain(&types, name));
        let (level, rules, mut state) = random_match(&vfs, &spec, human_and_ai(), 0);
        for _ in 0..200 {
            let _ = step(&mut state, &rules, &[]);
        }
        let bytes = SaveGame {
            start: start_package(&rules, &random_setup(&spec, human_and_ai())),
            state: state.clone(),
            commands: Vec::new(),
        }
        .to_bytes();
        let (again, loaded_rules, save) = load_random_save(&vfs, &bytes);
        assert_eq!(save.start.random_map.as_ref(), Some(&spec), "{name}");
        assert_eq!(again.map.digests(), level.map.digests(), "{name}");
        assert_eq!(again.map.rand_state, level.map.rand_state, "{name}");
        assert_eq!(loaded_rules.checksum, rules.checksum, "{name}");
        let mut loaded = save.state;
        assert_eq!(loaded, state, "{name}: objects and CRT from the savegame");
        assert_eq!(loaded.steps, 200);
        for i in 0..50 {
            let _ = step(&mut state, &rules, &[]);
            let _ = step(&mut loaded, &loaded_rules, &[]);
            assert_eq!(checksum(&loaded), checksum(&state), "{name}, step {i}");
        }
    }
}

/// D-348 (deviation): a King-of-the-Hill savegame loads the KotH map - the game mode comes from the map
/// description of the savegame, not from the session (the original built the NORMAL map from the main menu).
// QUIRK D-348: deviation
#[test]
fn koth_save_loads_the_koth_map() {
    use opennml_sim::save::{SaveGame, checksum};
    use opennml_sim::setup::start_package;
    use opennml_sim::tick::step;
    let Some((vfs, types)) = open() else { return };
    let spec = RandomMapSpec {
        game_type: game_type(&types, "KING_OF_THE_HILL"),
        ..base_spec(&types, terrain(&types, "continental"))
    };
    let (level, rules, mut state) = random_match(&vfs, &spec, human_and_ai(), 0);
    for _ in 0..20 {
        let _ = step(&mut state, &rules, &[]);
    }
    let bytes = SaveGame {
        start: start_package(&rules, &random_setup(&spec, human_and_ai())),
        state: state.clone(),
        commands: Vec::new(),
    }
    .to_bytes();
    let (again, loaded_rules, save) = load_random_save(&vfs, &bytes);
    assert_eq!(save.start.setup.mode, 3);
    assert_eq!(
        loaded_rules.random_map.as_ref().unwrap().spec.game_type,
        spec.game_type
    );
    // The KotH map: flag 4300 on the center tile, terrain as at the start.
    let flags: Vec<_> = again
        .map
        .objects
        .iter()
        .filter(|o| o.guid == 4300)
        .collect();
    assert_eq!(flags.len(), 1);
    assert_eq!(flags[0].position, [8256.0, 0.0, 8256.0]);
    assert_eq!(again.map.digests(), level.map.digests());
    // The NORMAL map of the same seed is a different one (the original would load it from the main menu).
    let normal = opennml_randommap::build_level(
        &vfs,
        &RandomMapSpec {
            game_type: game_type(&types, "NORMAL"),
            ..spec.clone()
        },
    )
    .unwrap();
    assert_ne!(normal.map.digests(), level.map.digests());
    let mut loaded = save.state;
    let _ = step(&mut state, &rules, &[]);
    let _ = step(&mut loaded, &loaded_rules, &[]);
    assert_eq!(checksum(&loaded), checksum(&state));
}
