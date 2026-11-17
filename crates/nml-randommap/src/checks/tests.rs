use nml_formats::rnd::{PlaceObjectDef, RandomMapScript};

use super::*;
use crate::paint::tests::texture;

const S: f32 = 128.0;

/// Axis-aligned box `±half` around the origin (y 0 ... 10) as the snapping hull and the file hull.
fn box_geometry(half: f32, radius: f32) -> ModelGeometry {
    box_geometry_xz(half, half, radius)
}

/// Box `±half_x` x `±half_z` (y 0 ... 10) as the snapping hull and the file hull.
fn box_geometry_xz(half_x: f32, half_z: f32, radius: f32) -> ModelGeometry {
    let (mn, mx) = ([-half_x, 0.0, -half_z], [half_x, 10.0, half_z]);
    let file_box = std::array::from_fn(|i| {
        [
            if i & 1 != 0 { mx[0] } else { mn[0] },
            if i & 2 != 0 { mx[1] } else { mn[1] },
            if i & 4 != 0 { mx[2] } else { mn[2] },
        ]
    });
    ModelGeometry {
        blocking01: None,
        instance_box: (mn, mx),
        file_box,
        radius,
    }
}

fn script() -> RandomMapScript {
    RandomMapScript {
        textures: vec![texture(1, "GRASS"), texture(2, "SAND")],
        ..RandomMapScript::default()
    }
}

/// Empty project for contexts whose checks read neither the project nor files.
static NO_PROJECT: Project = Project {
    objects: BTreeMap::new(),
    textures: BTreeMap::new(),
    colors: BTreeMap::new(),
    upgrades: BTreeMap::new(),
};

/// Context without project and files (the checks read neither).
fn ctx<'a>(
    grid: &'a mut Grid,
    script: &'a RandomMapScript,
    players: &'a [PlayerArea],
    steep: &[u8],
) -> ObjectCtx<'a> {
    // Zero-sized closure: `Box::leak` allocates nothing.
    let read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>> = Box::leak(Box::new(|_: &str| None));
    ObjectCtx::new(grid, script, players, steep, &NO_PROJECT, read, S)
}

/// Grid `dim x dim` made entirely of texture `number`, flat.
fn grid_of(dim: usize, number: i32) -> Grid {
    let mut grid = Grid::new(dim, dim);
    grid.tiles.fill(number);
    grid
}

// QUIRK D-357: kept
#[test]
fn snap_centres_small_hulls_on_the_tile() {
    // w = 100 < 128: origin to the tile center, ftol(300/128) = 2 -> 2*128 + 64 = 320.
    assert_eq!(snap_to_grid(300.0, 250.0, 350.0, S), 320.0);
    // D-357: ftol rounds toward 0, positions in (-128, 0) land at 64.
    assert_eq!(snap_to_grid(-10.0, -60.0, 40.0, S), 64.0);
    assert_eq!(snap_to_grid(-127.9, -177.9, -77.9, S), 64.0);
    // -128 is exactly -1 tile: -128 + 64.
    assert_eq!(snap_to_grid(-128.0, -178.0, -78.0, S), -64.0);
    // The hull only decides the branch, not the position (origin, not hull center).
    assert_eq!(snap_to_grid(300.0, 300.0, 400.0, S), 320.0);
}

// QUIRK D-357: kept
#[test]
fn snap_aligns_wide_hulls_to_the_left_edge() {
    // w = 200 >= 128: t = 200/128 = 1.5625, floor 1, x = 300 - 128*0.5625 = 228; left edge 128.
    assert_eq!(snap_to_grid(300.0, 200.0, 400.0, S), 228.0);
    // w == S takes the edge branch: t = 1.84375 -> 300 - 108 = 192, edge 236 - 108 = 128.
    assert_eq!(snap_to_grid(300.0, 236.0, 364.0, S), 192.0);
    // Negative: floor(-1.5625) = -2, fraction 0.4375 -> -10 - 56 = -66, edge -256.
    assert_eq!(snap_to_grid(-10.0, -200.0, 0.0, S), -66.0);
    // Width NaN counts like w >= S (`TEST AH,5; JP`): edge branch with min = 0 leaves x as it is ...
    assert_eq!(snap_to_grid(300.0, 0.0, f32::NAN, S), 300.0);
    // ... and a NaN minimum makes the position NaN (no tile-center branch).
    assert!(snap_to_grid(300.0, f32::NAN, 400.0, S).is_nan());
}

#[test]
fn tile_truncates_towards_zero() {
    assert_eq!(tile_of(300.0, 1000.0, S), (2, 7));
    assert_eq!(tile_of(-10.0, -127.0, S), (0, 0));
    assert_eq!(tile_of(-128.0, f32::NAN, S), (-1, i32::MIN));
}

#[test]
fn far_from_players_uses_fixed_hypot() {
    let areas = [PlayerArea {
        id: 1,
        cx: 10,
        cz: 10,
        min_size: 5,
    }];
    // (3, 4) -> 5: not less than minSize.
    assert!(far_from_players(13, 14, &areas).unwrap());
    // (3, 3) -> 4.24 -> ToInt 4 < 5.
    assert!(!far_from_players(13, 13, &areas).unwrap());
    assert!(!far_from_players(10, 10, &areas).unwrap());
    assert!(far_from_players(10, 10, &[]).unwrap());
}

// QUIRK D-340, D-358: kept
// QUIRK D-352: deviation
#[test]
fn put_onto_needs_the_whole_square() {
    let mut grid = grid_of(5, 1);
    grid.set_tile(3, 2, 2);
    // Square (0...2)^2 made entirely of 1.
    assert!(put_onto_ok(&grid, 1, 1, 1, 1));
    // (1...3) x (1...3) contains (3, 2).
    assert!(!put_onto_ok(&grid, 2, 2, 1, 1));
    // Only the own tile with AvoidOther 0.
    assert!(put_onto_ok(&grid, 2, 2, 1, 0));
    assert!(!put_onto_ok(&grid, 3, 2, 1, 0));
    // Tiles outside the map count as matching (D-340).
    assert!(put_onto_ok(&grid, 0, 0, 1, 2));
    assert!(put_onto_ok(&grid, 4, 4, 1, 1));
    assert!(put_onto_ok(&grid, -10, -10, 7, 1));
    // Unknown name -> -1: no tile matches (D-358).
    assert_eq!(texture_number(&script(), "WATER"), -1);
    assert!(!put_onto_ok(&grid, 1, 1, -1, 0));
}

// QUIRK D-358: kept
#[test]
fn negative_avoid_other_skips_put_onto() {
    let mut grid = grid_of(5, 1);
    grid.set_tile(3, 2, 2);
    // D-358: AvoidOther < 0 checks no tile, not even its own.
    assert!(put_onto_ok(&grid, 3, 2, 1, -1));
    assert!(put_onto_ok(&grid, 3, 2, -1, -5));
    // Via `check_attempt`: unknown name with AvoidOther -1 passes, with 0 it does not.
    let script = script();
    let steep = vec![0; 25];
    let ctx = ctx(&mut grid, &script, &[], &steep);
    let geo = box_geometry(20.0, 10.0);
    let mut def = PlaceObjectDef {
        put_onto: "WATER".into(),
        avoid_other: -1,
        ..PlaceObjectDef::default()
    };
    assert!(
        check_attempt(&ctx, &def, &geo, [0.0; 3], 300.0, 300.0, false)
            .unwrap()
            .is_some()
    );
    def.avoid_other = 0;
    assert!(
        check_attempt(&ctx, &def, &geo, [0.0; 3], 300.0, 300.0, false)
            .unwrap()
            .is_none()
    );
}

// QUIRK D-340: kept
#[test]
fn flat_needs_exact_normals() {
    let mut grid = grid_of(4, 1);
    let flat = FaceNormals::from_grid(&grid, S);
    // Flat terrain: both normals exactly (0, -1, 0) (pointing down, sign of the original).
    assert!(flat.a.iter().chain(&flat.b).all(|n| *n == [0.0, -1.0, 0.0]));
    assert!(flat_ok(&flat, [0, 0, 3, 3]));
    // D-340: 0.0001 at one corner is enough for "not flat" in the four neighboring tiles.
    grid.set_height(2, 2, 0.0001);
    let normals = FaceNormals::from_grid(&grid, S);
    assert!(!flat_ok(&normals, [0, 0, 3, 3]));
    for (x, z) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
        assert!(!flat_ok(&normals, [x, z, x, z]), "tile ({x}, {z})");
    }
    assert!(flat_ok(&normals, [0, 0, 0, 3]));
    assert!(flat_ok(&normals, [3, 0, 3, 3]));
    // Empty rectangle and tiles outside: yes.
    assert!(flat_ok(&normals, [3, 0, 2, 3]));
    assert!(flat_ok(&normals, [-5, -5, -1, -1]));
    assert!(flat_ok(&normals, [3, 3, 10, 10]));
    // Computation order like `steep_flags`: the same normals.
    let shared = relief::face_normals(&grid, S);
    assert_eq!(
        normals.a,
        shared.iter().map(|n| n.0.to_array()).collect::<Vec<_>>()
    );
    assert_eq!(
        normals.b,
        shared.iter().map(|n| n.1.to_array()).collect::<Vec<_>>()
    );
}

#[test]
fn border_check_is_strict_and_rejects_nan() {
    // Map 4 x 4 tiles = 512.
    assert!(inside_map(64.0, 64.0, 10.0, 4, 4, S));
    // Strict: 0 and 512 lie outside.
    assert!(!inside_map(0.0, 64.0, 0.0, 4, 4, S));
    assert!(!inside_map(64.0, 512.0, 0.0, 4, 4, S));
    assert!(!inside_map(10.0, 64.0, 10.0, 4, 4, S));
    assert!(inside_map(10.5, 64.0, 10.0, 4, 4, S));
    assert!(!inside_map(502.0, 64.0, 10.0, 4, 4, S));
    assert!(inside_map(501.5, 64.0, 10.0, 4, 4, S));
    // Checked are (x, z), (x + r, z + r), (x - r, z - r): z depends on the radius like x.
    assert!(!inside_map(64.0, 10.0, 10.0, 4, 4, S));
    // Not square: W and H separately.
    assert!(inside_map(600.0, 64.0, 10.0, 8, 4, S));
    assert!(!inside_map(64.0, 600.0, 10.0, 8, 4, S));
    // NaN -> no.
    assert!(!inside_map(f32::NAN, 64.0, 10.0, 4, 4, S));
    assert!(!inside_map(64.0, f32::NAN, 10.0, 4, 4, S));
    assert!(!inside_map(64.0, 64.0, f32::NAN, 4, 4, S));
}

// QUIRK D-360: kept
#[test]
fn group_distance_uses_three_d_length_and_sx() {
    let earlier = [[0.0, 0.0, 0.0]];
    // Limit f32(SX * 2) = 256.
    assert!(group_distance_ok([256.0, 0.0, 0.0], &earlier, 2, S));
    assert!(!group_distance_ok([255.9, 0.0, 0.0], &earlier, 2, S));
    // z with SX like x.
    assert!(group_distance_ok([0.0, 0.0, 256.0], &earlier, 2, S));
    // 3D: y counts too (2D 200 < 256, 3D 262.5 >= 256).
    assert!(group_distance_ok([200.0, 170.0, 0.0], &earlier, 2, S));
    assert!(!group_distance_ok([200.0, 150.0, 0.0], &earlier, 2, S));
    // Every earlier center counts.
    let two = [[1000.0, 0.0, 1000.0], [0.0, 0.0, 0.0]];
    assert!(!group_distance_ok([100.0, 0.0, 0.0], &two, 2, S));
    // NaN counts as far enough (D-360).
    assert!(group_distance_ok([f32::NAN, 0.0, 0.0], &earlier, 2, S));
    // minGroupDistance <= 0: no check.
    assert!(group_distance_ok([0.0, 0.0, 0.0], &earlier, 0, S));
    assert!(group_distance_ok([0.0, 0.0, 0.0], &earlier, -3, S));
    assert!(group_distance_ok([0.0, 0.0, 0.0], &[], 5, S));
}

// QUIRK D-337: kept
// QUIRK D-352: deviation
#[test]
fn block_map_starts_as_steep_and_blocks_set_rects() {
    // D-337: the block map starts as a copy of the steepness.
    let mut steep = vec![0u8; 16];
    steep[4 * 2 + 3] = 1;
    let mut block = BlockMap::from_steep(&steep, 4);
    assert_eq!(block.cells(), &steep[..]);
    assert!(!block.is_free([3, 2, 3, 2]));
    assert!(!block.is_free([0, 0, 3, 3]));
    assert!(block.is_free([0, 0, 2, 3]));
    // Footprints of placed objects block later attempts.
    block.set([1, 0, 2, 1]);
    assert!(!block.is_free([1, 1, 1, 1]));
    assert!(block.is_free([0, 0, 0, 3]));
    let mut expected = steep.clone();
    for (x, z) in [(1, 0), (2, 0), (1, 1), (2, 1)] {
        expected[4 * z + x] = 1;
    }
    assert_eq!(block.cells(), &expected[..]);
    // Only tiles inside the map: outside neither set nor checked.
    block.set([-2, -2, 0, 0]);
    expected[0] = 1;
    assert_eq!(block.cells(), &expected[..]);
    assert!(block.is_free([-5, -5, -1, -1]));
    assert!(block.is_free([4, 0, 9, 3]));
    // Empty rectangle (x0 > x1): free, `set` has no effect.
    assert!(block.is_free([3, 0, 2, 3]));
    block.set([3, 3, 2, 3]);
    assert_eq!(block.cells(), &expected[..]);
}

/// Setup for `checks_short_circuit_in_order`: map 8 x 8 of `GRASS`, player area at (1, 1),
/// attempt at (650, 650) -> snapped (704, 704), tile (5, 5), rectangle [5, 5, 5, 5].
struct Fixture {
    grid: Grid,
    script: RandomMapScript,
    players: Vec<PlayerArea>,
    steep: Vec<u8>,
    geo: ModelGeometry,
    def: PlaceObjectDef,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            grid: grid_of(8, 1),
            script: script(),
            players: vec![PlayerArea {
                id: 1,
                cx: 1,
                cz: 1,
                min_size: 2,
            }],
            steep: vec![0; 64],
            geo: box_geometry(20.0, 30.0),
            def: PlaceObjectDef {
                avoid_player_areas: true,
                put_onto: "GRASS".into(),
                avoid_other: 1,
                put_on_flat: true,
                min_group_distance: 1,
                ..PlaceObjectDef::default()
            },
        }
    }

    /// Attempt as group center with the earlier centers `centres`; result and counter per check
    /// (snapping, tile, (3), (4), (5), (6), (7), (8)).
    fn run(&mut self, centres: &[[f32; 3]]) -> (Option<Attempt>, [u32; 8]) {
        let mut ctx = ctx(&mut self.grid, &self.script, &self.players, &self.steep);
        ctx.centres.extend_from_slice(centres);
        let attempt =
            check_attempt(&ctx, &self.def, &self.geo, [0.0; 3], 650.0, 650.0, true).unwrap();
        (attempt, ctx.checks_run.get())
    }
}

// QUIRK D-337: kept
#[test]
fn checks_short_circuit_in_order() {
    // Everything passes: each check exactly once, result snapped.
    let (attempt, run) = Fixture::new().run(&[]);
    assert_eq!(
        attempt,
        Some(Attempt {
            position: [704.0, 0.0, 704.0],
            tile: (5, 5),
            rect: [5, 5, 5, 5],
        })
    );
    assert_eq!(run, [1; 8]);

    // (3) Player area: distance 0 < 2.
    let mut f = Fixture::new();
    f.players[0].cx = 5;
    f.players[0].cz = 5;
    assert_eq!(f.run(&[]), (None, [1, 1, 1, 0, 0, 0, 0, 0]));

    // (4) PutOnto: neighboring tile (6, 4) is SAND.
    let mut f = Fixture::new();
    f.grid.set_tile(6, 4, 2);
    assert_eq!(f.run(&[]), (None, [1, 1, 1, 1, 0, 0, 0, 0]));

    // (5) PutOnFlat: height 0.0001 at grid point (5, 5) (steepness untouched).
    let mut f = Fixture::new();
    f.grid.set_height(5, 5, 0.0001);
    assert_eq!(f.run(&[]), (None, [1, 1, 1, 1, 1, 0, 0, 0]));

    // (6) Border: 704 + 330 > 1024.
    let mut f = Fixture::new();
    f.geo.radius = 330.0;
    assert_eq!(f.run(&[]), (None, [1, 1, 1, 1, 1, 1, 0, 0]));

    // (7) Occupancy: tile (5, 5) steep.
    let mut f = Fixture::new();
    f.steep[8 * 5 + 5] = 1;
    assert_eq!(f.run(&[]), (None, [1, 1, 1, 1, 1, 1, 1, 0]));

    // (8) Group distance: earlier center 100 away, limit 128.
    let mut f = Fixture::new();
    assert_eq!(
        f.run(&[[604.0, 0.0, 704.0]]),
        (None, [1, 1, 1, 1, 1, 1, 1, 1])
    );

    // Disabled checks do not run: NoGrid, without AvoidPlayerAreas, PutOnto, PutOnFlat,
    // distance and as a single object (no group center).
    let mut f = Fixture::new();
    f.def.no_grid = true;
    f.def.avoid_player_areas = false;
    f.def.put_onto.clear();
    f.def.put_on_flat = false;
    let mut ctx = ctx(&mut f.grid, &f.script, &f.players, &f.steep);
    ctx.centres.push([650.0, 0.0, 650.0]);
    let attempt = check_attempt(&ctx, &f.def, &f.geo, [0.0; 3], 650.0, 650.0, false).unwrap();
    assert_eq!(
        attempt,
        Some(Attempt {
            position: [650.0, 0.0, 650.0],
            tile: (5, 5),
            rect: [5, 5, 5, 5],
        })
    );
    assert_eq!(ctx.checks_run.get(), [0, 1, 0, 0, 0, 1, 1, 0]);
}

#[test]
fn rect_follows_the_snapped_position_and_rotation() {
    // Wide hull (±100): snapping to the tile edge, rectangle from the file hull at the snapped
    // position: x = 650 - 128*frac(550/128) = 650 - 38 = 612, rectangle
    // [ftol(528)/128, ..., ftol(696)/128] = [4, 4, 5, 5].
    let mut f = Fixture::new();
    let def = PlaceObjectDef::default();
    let geo = box_geometry(100.0, 10.0);
    let ctx = ctx(&mut f.grid, &f.script, &f.players, &f.steep);
    let attempt = check_attempt(&ctx, &def, &geo, [0.0; 3], 650.0, 650.0, false)
        .unwrap()
        .unwrap();
    assert_eq!(attempt.position, [612.0, 0.0, 612.0]);
    assert_eq!(attempt.tile, (4, 4));
    assert_eq!(attempt.rect, [4, 4, 5, 5]);
}

#[test]
fn rotated_hull_swaps_the_snap_branches_and_the_rect() {
    // Hull ±100 in x, ±20 in z. Unrotated: x wide (edge, 612), z narrow (center, 704).
    let mut f = Fixture::new();
    let def = PlaceObjectDef::default();
    let geo = box_geometry_xz(100.0, 20.0, 10.0);
    let ctx = ctx(&mut f.grid, &f.script, &f.players, &f.steep);
    let plain = check_attempt(&ctx, &def, &geo, [0.0; 3], 650.0, 650.0, false)
        .unwrap()
        .unwrap();
    assert_eq!(plain.position, [612.0, 0.0, 704.0]);
    assert_eq!(plain.rect, [4, 5, 5, 5]);
    // YROT 90 degrees: the hull lies across, snapping and rectangle swap the axes. x snaps exactly
    // to the center; z to the edge, apart from the rounding of sin/cos (90 degrees is not exact in f32).
    let turned = check_attempt(
        &ctx,
        &def,
        &geo,
        [0.0, std::f32::consts::FRAC_PI_2, 0.0],
        650.0,
        650.0,
        false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(turned.position[0], 704.0);
    assert!(
        (turned.position[2] - 612.0).abs() < 0.01,
        "{:?}",
        turned.position
    );
    assert_eq!(turned.tile, (5, 4));
    assert_eq!(turned.rect, [5, 4, 5, 5]);
}

// QUIRK D-335: deviation
#[test]
fn player_distance_of_1024_tiles_is_an_error_not_a_panic() {
    // |tx - cx| = 1024 tiles: `Fixed_Hypot` divided by zero in the original (D-335).
    let far = [PlayerArea {
        id: 1,
        cx: 5 - 1024,
        cz: 5,
        min_size: 2,
    }];
    let error = far_from_players(5, 5, &far).unwrap_err();
    assert!(format!("{error:#}").contains("by zero"), "{error:#}");
    // 1023 tiles do not divide by zero (the value itself overflows in 32 bits like in the original).
    let near = [PlayerArea {
        cx: 5 - 1023,
        ..far[0]
    }];
    assert!(far_from_players(5, 5, &near).is_ok());
    // Via `check_attempt`: (3) runs before the border, the error comes through.
    let mut f = Fixture::new();
    f.players = far.to_vec();
    let ctx = ctx(&mut f.grid, &f.script, &f.players, &f.steep);
    assert!(check_attempt(&ctx, &f.def, &f.geo, [0.0; 3], 650.0, 650.0, true).is_err());
}
