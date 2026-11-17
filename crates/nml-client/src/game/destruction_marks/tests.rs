use std::collections::BTreeMap;

use nml_effects::world::EffectWorld;
use nml_rda::Vfs;
use nml_rda::testutil::{TestFile, build_rda};
use nml_sim::rules::BuildingShape;
use nml_world::assets::ModelCache;

use super::*;
use crate::game::sim_host::testkit::*;

/// Terrain height of the fake: `x / 100 + z / 1000`, so that every cloud has its own height.
fn ground(x: f32, z: f32) -> f32 {
    x / 100.0 + z / 1000.0
}

/// Map with `building_smoke.pef` (one emitter in a loop) and the height [`ground`].
struct Fake {
    _dir: tempfile::TempDir,
    vfs: Vfs,
    models: ModelCache,
    effects: EffectWorld,
}

impl Fake {
    fn new() -> Fake {
        let pef = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
        let dir = tempfile::tempdir().unwrap();
        let files = [TestFile {
            path: BUILDING_SMOKE,
            data: pef.as_bytes(),
            compress: false,
        }];
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        Fake {
            _dir: dir,
            vfs,
            models: ModelCache::new(),
            effects: EffectWorld::new(),
        }
    }

    fn footprints(&self) -> Vec<(Vec3, u32, f32, i32)> {
        self.effects
            .footprints()
            .map(|f| (f.quad()[0], f.kind(), f.yaw(), f.life_ms()))
            .collect()
    }
}

impl MovementWorld for Fake {
    fn tracks_at(&self, _x: f32, _z: f32) -> bool {
        false
    }

    fn tracks_at_tile(&self, _x: i32, _z: i32) -> bool {
        false
    }

    fn ground_height(&self, x: f32, z: f32) -> f32 {
        ground(x, z)
    }

    fn water_level(&self) -> f32 {
        -100.0
    }

    fn node_positions(&self, _id: ObjectId, _prefix: &str) -> Vec<Vec3> {
        Vec::new()
    }

    fn carrier(&self, _id: ObjectId) -> Option<usize> {
        None
    }

    fn assets(&mut self) -> (&mut EffectWorld, &Vfs, &mut ModelCache) {
        (&mut self.effects, &self.vfs, &mut self.models)
    }
}

/// Expected rotation for the random value `r`.
fn yaw_of(r: u32) -> f32 {
    r as f32 / 32768.0 * std::f32::consts::TAU
}

#[test]
fn smoke_columns_follow_the_hull_in_tile_steps() {
    // Hull 384 x 256, tile 128: x 0/128/256/384 at both z edges (4 + 4), z 0/128/256 at
    // both x edges (3 + 3); `<=` includes the end edge, the corners come twice.
    let min = Vec2::new(1000.0, 2000.0);
    let max = Vec2::new(1384.0, 2256.0);
    let got = smoke_points(min, max, Vec2::splat(128.0), &ground);
    let p = |x: f32, z: f32| Vec3::new(x, ground(x, z), z);
    let mut want = Vec::new();
    for x in [1000.0, 1128.0, 1256.0, 1384.0] {
        want.push(p(x, 2000.0));
        want.push(p(x, 2256.0));
    }
    for z in [2000.0, 2128.0, 2256.0] {
        want.push(p(1000.0, z));
        want.push(p(1384.0, z));
    }
    assert_eq!(got, want);
    // Narrower than a tile: only the start per axis.
    let got = smoke_points(min, Vec2::new(1100.0, 2050.0), Vec2::splat(128.0), &ground);
    assert_eq!(got.len(), 4);
}

#[test]
fn hull_is_the_rectangle_of_the_two_turned_corners() {
    // `0x0046C350` rotates only `+-half`: at 90 degrees the axes swap, at 45 degrees the rectangle becomes narrow
    // (narrowing adopted, re/buildings 8.5a).
    let pos = Vec3::new(1000.0, 5.0, 2000.0);
    let half = Vec3::new(192.0, 50.0, 128.0);
    let (min, max) = hull_rect(pos, 0.0, half);
    assert_eq!(
        (min, max),
        (Vec2::new(808.0, 1872.0), Vec2::new(1192.0, 2128.0))
    );
    let (min, max) = hull_rect(pos, std::f32::consts::FRAC_PI_2, half);
    assert!(
        (min - Vec2::new(872.0, 1808.0)).abs().max_element() < 1e-3,
        "{min}"
    );
    assert!(
        (max - Vec2::new(1128.0, 2192.0)).abs().max_element() < 1e-3,
        "{max}"
    );
    let (min, max) = hull_rect(pos, std::f32::consts::FRAC_PI_4, half);
    let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
    let hx = (192.0 * c - 128.0 * s).abs();
    let hz = 192.0 * s + 128.0 * c;
    assert!(
        ((max.x - min.x) * 0.5 - hx).abs() < 1e-2,
        "narrow: {min} {max}"
    );
    assert!(((max.y - min.y) * 0.5 - hz).abs() < 1e-2);
}

#[test]
fn craters_one_in_the_middle_and_seven_inside_the_shrunken_hull() {
    // Start value `EFFECTS_SEED` (2): center rotation 45, then per crater x, z, rotation. Hull 384 x 256
    // from (0, 0): sx = 234, sz = 106.
    let mut rng = MsvcRand::new(EFFECTS_SEED);
    let center = Vec3::new(192.0, 7.0, 128.0);
    let got = building_craters(center, Vec2::ZERO, Vec2::new(384.0, 256.0), &mut rng);
    let want: Vec<(Vec3, u32, f32)> = [
        (center, CRATER_CENTER, 45),
        (Vec3::new(200.0, 7.0, 30.0), CRATER_EVEN, 17795),
        (Vec3::new(0.0, 7.0, 40.0), CRATER_EVEN, 14590),
        (Vec3::new(223.0, 7.0, 105.0), CRATER_ODD, 18316),
        (Vec3::new(175.0, 7.0, 99.0), CRATER_ODD, 12652),
        (Vec3::new(138.0, 7.0, 35.0), CRATER_EVEN, 17829),
        (Vec3::new(95.0, 7.0, 51.0), CRATER_ODD, 6658),
        (Vec3::new(179.0, 7.0, 15.0), CRATER_ODD, 30085),
    ]
    .into_iter()
    .map(|(p, k, r)| (p, k, yaw_of(r)))
    .collect();
    assert_eq!(got, want);
    for &(p, kind, yaw) in &got[1..] {
        assert!((0.0..234.0).contains(&p.x) && (0.0..106.0).contains(&p.z));
        assert_eq!(Footprint::radius(kind), 300.0);
        assert!((0.0..std::f32::consts::TAU).contains(&yaw));
    }
    assert_eq!(Footprint::radius(CRATER_CENTER), 300.0);
}

#[test]
fn tiny_hull_puts_all_craters_at_the_corner() {
    // sx = 90 - 150 < 1 -> `rand % 1` = 0: all seven at the corner `min` (even -> `0x250`).
    let mut rng = MsvcRand::new(EFFECTS_SEED);
    let center = Vec3::new(55.0, 3.0, 70.0);
    let got = building_craters(
        center,
        Vec2::new(10.0, 20.0),
        Vec2::new(100.0, 120.0),
        &mut rng,
    );
    assert_eq!(got.len(), 8);
    assert_eq!((got[0].0, got[0].1), (center, CRATER_CENTER));
    for &(p, kind, _) in &got[1..] {
        assert_eq!((p, kind), (Vec3::new(10.0, 3.0, 20.0), CRATER_EVEN));
    }
}

/// Host with a "building" (shape 384 x 256 for the warrior type) at `(1000, 2000)`.
fn host_with_building() -> (SimHost, ObjectId) {
    let mut host = host();
    host.rules.building_shapes.insert(
        WARRIOR,
        BuildingShape {
            center: [0.0; 3],
            half: [192.0, 50.0, 128.0],
            free_nodes: Vec::new(),
            helper_nodes: BTreeMap::new(),
        },
    );
    let id = spawn(&mut host, WARRIOR, 1000.0, 2000.0);
    (host, id)
}

#[test]
fn a_destroyed_building_smokes_and_leaves_eight_craters_at_any_quality() {
    let (host, id) = host_with_building();
    let center = Vec3::from(host.state.object(id, false).unwrap().world_pos);
    let (min, max) = hull_rect(center, 0.0, Vec3::new(192.0, 50.0, 128.0));
    for quality in [0, 2] {
        let mut world = Fake::new();
        let mut marks = DestructionMarks::default();
        let events = [
            SimEvent::BuildingDestroyed { object: id },
            SimEvent::Crater {
                pos: [500.0, 1.0, 600.0],
            },
        ];
        marks.apply(&events, &host, &mut world, quality);
        // Tile 128: 4 + 4 + 3 + 3 clouds.
        assert_eq!(world.effects.counts().world, 14, "quality {quality}");
        let life = if quality < 2 { 10_000 } else { 20_000 };
        let mut rng = MsvcRand::new(EFFECTS_SEED);
        let mut want: Vec<(Vec3, u32, f32, i32)> = building_craters(center, min, max, &mut rng)
            .into_iter()
            .map(|(p, k, y)| (p, k, y, life))
            .collect();
        want.push((
            Vec3::new(500.0, 1.0, 600.0),
            CANNON_CRATER,
            yaw_of(rng.rand()),
            life,
        ));
        assert_eq!(world.footprints(), want);
    }
}

#[test]
fn without_shape_or_object_nothing_appears_and_reset_repeats_the_dice() {
    let (mut host, id) = host_with_building();
    let mut world = Fake::new();
    let mut marks = DestructionMarks::default();
    // Unknown object: nothing.
    marks.apply(
        &[SimEvent::BuildingDestroyed {
            object: ObjectId(0x7FFF),
        }],
        &host,
        &mut world,
        2,
    );
    assert_eq!(world.effects.counts().world, 0);
    assert_eq!(world.footprints().len(), 0);
    // First with a shape, then after `reset` the same craters.
    let event = [SimEvent::BuildingDestroyed { object: id }];
    marks.apply(&event, &host, &mut world, 2);
    let first = world.footprints();
    marks.reset();
    let mut again = Fake::new();
    marks.apply(&event, &host, &mut again, 2);
    assert_eq!(again.footprints(), first);
    // Without a shape: neither smoke nor craters.
    host.rules.building_shapes.clear();
    let mut bare = Fake::new();
    marks.apply(&event, &host, &mut bare, 2);
    assert_eq!(bare.effects.counts().world, 0);
    assert!(bare.footprints().is_empty());
}
