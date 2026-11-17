use std::collections::BTreeMap;

use glam::Mat4;
use nml_effects::output::{DrawCommand, EffectFrame};
use nml_effects::world::{ObjectFrame, ObjectView};
use nml_formats::rdo::RdoLayout;
use nml_formats::testutil::{sample_rdo, write_rdo};
use nml_rda::testutil::{TestFile, build_rda};
use nml_world::visibility::Frustum;

use super::*;
use crate::game::formation_order::FormationOrder;
use crate::game::sim_host::testkit::*;

/// Tile size of the test maps.
const CELL: f32 = 128.0;

/// Center of tile `t`.
fn at(t: i32) -> f32 {
    t as f32 * CELL + 64.0
}

/// Map of the fake: sand (tracks) for tiles with `x < SAND`, water for world positions with
/// `x >= WATER` (ground -10 below water level 0), otherwise land at height 10.
const SAND: i32 = 16;
const WATER: f32 = 24.0 * CELL;

struct Fake {
    _dir: tempfile::TempDir,
    vfs: Vfs,
    models: ModelCache,
    effects: EffectWorld,
    /// Node positions per (ID, prefix).
    nodes: BTreeMap<(u32, &'static str), Vec<Vec3>>,
    /// Index per ID.
    carriers: BTreeMap<u32, usize>,
}

impl Fake {
    /// Archive with `rad_staubwolke.pef` (one looping emitter) and the ring model (`sample_rdo`), depending on
    /// `with_ring`.
    fn new(with_ring: bool) -> Fake {
        let pef = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
        let rdo = write_rdo(&sample_rdo(RdoLayout::Standard));
        let ring = nml_rda::normalize_path(SURF_RING_MODEL);
        let mut files = vec![TestFile {
            path: WHEEL_DUST,
            data: pef.as_bytes(),
            compress: false,
        }];
        if with_ring {
            files.push(TestFile {
                path: &ring,
                data: &rdo,
                compress: false,
            });
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        Fake {
            _dir: dir,
            vfs,
            models: ModelCache::new(),
            effects: EffectWorld::new(),
            nodes: BTreeMap::new(),
            carriers: BTreeMap::new(),
        }
    }

    /// Effect game tick (100 ms) with a view frustum over the whole map, everything seen.
    fn step_effects(&mut self) {
        // Clip space +-1 and depth 0 ... 1 for world positions up to 1e5 (x, y) and 0 ... 1e5 (z).
        let all = Frustum::from_view_proj(Mat4::from_scale(Vec3::splat(1e-5)));
        self.effects
            .step(100, &all, &View, &self.vfs, &mut self.models);
    }

    fn footprints(&self) -> Vec<Footprint> {
        self.effects.footprints().cloned().collect()
    }
}

impl MovementWorld for Fake {
    fn tracks_at(&self, x: f32, z: f32) -> bool {
        self.tracks_at_tile((x / CELL) as i32, (z / CELL) as i32)
    }

    fn tracks_at_tile(&self, x: i32, z: i32) -> bool {
        (0..SAND).contains(&x) && (0..64).contains(&z)
    }

    fn ground_height(&self, x: f32, _z: f32) -> f32 {
        if x >= WATER { -10.0 } else { 10.0 }
    }

    fn water_level(&self) -> f32 {
        0.0
    }

    fn node_positions(&self, id: ObjectId, prefix: &str) -> Vec<Vec3> {
        self.nodes
            .iter()
            .find(|((i, p), _)| *i == id.0 && *p == prefix)
            .map_or_else(Vec::new, |(_, v)| v.clone())
    }

    fn carrier(&self, id: ObjectId) -> Option<usize> {
        self.carriers.get(&id.0).copied()
    }

    fn assets(&mut self) -> (&mut EffectWorld, &Vfs, &mut ModelCache) {
        (&mut self.effects, &self.vfs, &mut self.models)
    }
}

/// All objects visible at the origin.
struct View;

impl ObjectView for View {
    fn frame(&self, _object: usize) -> ObjectFrame {
        ObjectFrame {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            visible: true,
        }
    }

    fn node(&self, _object: usize, _name: &str) -> Option<Vec3> {
        Some(Vec3::ZERO)
    }
}

/// A simulation step without simulation: phase and step counter like `world_tick`.
fn fake_step(host: &mut SimHost) {
    host.state.phase = if host.state.phase == 1 { 0 } else { 1 };
    host.state.steps += 1;
}

/// An object tick (two steps), the sync and an effect game tick.
fn tick(host: &mut SimHost, fx: &mut Fake, m: &mut MovementEffects, quality: u8) {
    fake_step(host);
    fake_step(host);
    m.sync(host, quality, fx);
    fx.step_effects();
}

fn set(host: &mut SimHost, id: ObjectId, bits: u32, on: bool) {
    host.state
        .object_mut(id, false)
        .unwrap()
        .set_status_flags(bits, on);
}

fn object(host: &SimHost, id: ObjectId) -> GameObject {
    host.state.object(id, false).unwrap().clone()
}

/// New rings of a sync (rings die only in the effect tick).
fn rings(fx: &Fake) -> usize {
    fx.effects.surf_ring_count()
}

#[test]
fn counter_runs_every_second_tick_or_at_a_change() {
    let mut t = UnitTrack::default();
    // Constructor: counter 0, standing.
    assert!(t.due(false), "counter -1 < 1");
    assert_eq!(t.counter(), 2);
    assert!(!t.due(false), "counter 1, unchanged");
    assert!(t.due(false), "counter 0");
    // A change runs at once, also with counter >= 1.
    assert!(t.due(true));
    assert_eq!(t.counter(), 2);
    assert!(!t.due(true));
    assert!(t.due(true));
    assert!(t.due(false), "halt at once");
}

#[test]
fn footprint_kind_table() {
    let mut host = host();
    let id = spawn(&mut host, WARRIOR, at(2), at(2));
    let base = object(&host, id);
    let with = |flags: u32, modules: &[usize], guid: u32| {
        let mut o = base.clone();
        o.guid = guid;
        let walk = o.modules[0].clone().unwrap();
        o.modules[0].as_mut().unwrap().base.flags |= flags;
        for &m in modules {
            o.modules[m] = Some(walk.clone());
        }
        o
    };
    // Humans 1; SADDLE 6; animal 6; animal with SADDLE 0x802.
    assert_eq!(footprint_kind(&with(0, &[], WARRIOR), false), 1);
    assert_eq!(footprint_kind(&with(0x8000, &[], WARRIOR), false), 6);
    assert_eq!(footprint_kind(&with(0, &[0x14], WARRIOR), false), 6);
    assert_eq!(
        footprint_kind(&with(0x8000, &[0x14], WARRIOR), false),
        0x802
    );
    // Large vehicle (`0x20000`) 0, also with SADDLE; ghost (`0x1B`) 0.
    assert_eq!(footprint_kind(&with(0x2_0000, &[], WARRIOR), false), 0);
    assert_eq!(footprint_kind(&with(0x2_8000, &[0x14], WARRIOR), false), 0);
    assert_eq!(footprint_kind(&with(0, &[0x1B], WARRIOR), false), 0);
    // GUID 2005 6, also as a vehicle; foot nodes `foot01` take precedence over everything.
    assert_eq!(footprint_kind(&with(0x2_0000, &[], 2005), false), 6);
    assert_eq!(footprint_kind(&with(0, &[], 2005), true), 0);
    assert_eq!(footprint_kind(&with(0x8000, &[], WARRIOR), true), 0);
    // A switched-off animal module does not count.
    let mut off = with(0, &[0x14], WARRIOR);
    off.modules[0x14].as_mut().unwrap().base.enabled = false;
    assert_eq!(footprint_kind(&off, false), 1);
    // Marks at the foot nodes.
    assert_eq!(
        [1110, 1713, 2611, 1400].map(foot_node_kind),
        [0x802, 0x802, 0x802, 1]
    );
}

#[test]
fn marks_only_on_track_textures_and_on_land() {
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let sand = spawn(&mut host, WARRIOR, at(2), at(2));
    let grass = spawn(&mut host, WARRIOR, at(20), at(2));
    let standing = spawn(&mut host, WARRIOR, at(4), at(4));
    for id in [sand, grass] {
        set(&mut host, id, status::MOVE, true);
    }
    // `0x10` does not count as movement.
    let held = spawn(&mut host, WARRIOR, at(6), at(6));
    set(&mut host, held, status::MOVE | 0x10, true);
    for _ in 0..8 {
        tick(&mut host, &mut fx, &mut m, 2);
    }
    // Only the runner on sand: every 200 ms (ticks 1, 3, 5, 7) one mark at `GO+0x08`, sides alternating
    // (`m_LeftRight` 1, 2, 3, 4 -> side 1, 0, 1, 0).
    let o = object(&host, sand);
    let pos = Vec3::from(o.world_pos);
    let yaw = o.rotation[1];
    let marks = fx.footprints();
    assert_eq!(marks.len(), 4);
    for (i, f) in marks.iter().enumerate() {
        let side = i32::from(i % 2 == 0);
        let want = Footprint::new(pos, yaw, side, 1, 2);
        assert_eq!(
            (f.quad(), f.used(), f.yaw(), f.kind()),
            (want.quad(), want.used(), want.yaw(), want.kind()),
            "mark {i}"
        );
    }
    assert_eq!(m.track(sand).unwrap().footprint_kind(), Some(1));
    // Grass: no mark, track type undetermined; the standing one and the one with `0x10` only run the counter.
    for id in [grass, standing, held] {
        assert_eq!(m.track(id).unwrap().footprint_kind(), None);
    }
    // Runner in water (ground below water level): no footprint.
    let wet = spawn(&mut host, WARRIOR, WATER + 64.0, at(2));
    set(&mut host, wet, status::MOVE, true);
    for _ in 0..4 {
        tick(&mut host, &mut fx, &mut m, 2);
    }
    assert_eq!(m.track(wet).unwrap().footprint_kind(), None);
    assert_eq!(fx.footprints().len(), 6, "only the sand runner");
}

#[test]
fn rider_leaves_none_and_horse_leaves_hooves() {
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let rider = spawn(&mut host, RIDER, at(3), at(3));
    let horse = spawn(&mut host, HORSE, at(3), at(3));
    link(&mut host, rider, horse);
    for id in [rider, horse] {
        set(&mut host, id, status::MOVE, true);
    }
    for _ in 0..4 {
        tick(&mut host, &mut fx, &mut m, 2);
    }
    // Rider (`ISMOUNTED`) nothing; horse (SADDLE) type 6 as a pair.
    let marks = fx.footprints();
    assert_eq!(marks.len(), 2);
    assert!(marks.iter().all(|f| f.kind() == 6 && f.used() == 3));
    assert_eq!(m.track(rider).unwrap().footprint_kind(), None);
}

#[test]
fn stealthed_unit_leaves_marks() {
    // D-269: no stealth check.
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let masked = spawn(&mut host, WARRIOR, at(2), at(2));
    set(&mut host, masked, status::MOVE | status::MASKED, true);
    let wading = spawn(&mut host, WARRIOR, WATER + 64.0, at(2));
    set(&mut host, wading, status::MOVE | status::MASKED, true);
    for _ in 0..4 {
        tick(&mut host, &mut fx, &mut m, 2);
    }
    assert_eq!(fx.footprints().len(), 2);
    assert_eq!(m.take_sounds().len(), 2);
}

#[test]
fn siege_crew_bit_blocks_footprints_only() {
    // Status `0x4000000`: blocks the footprint, not the ring.
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let crew = spawn(&mut host, WARRIOR, at(2), at(2));
    set(&mut host, crew, status::MOVE | SIEGE_CREW, true);
    let wading = spawn(&mut host, WARRIOR, WATER + 64.0, at(2));
    set(&mut host, wading, status::MOVE | SIEGE_CREW, true);
    fake_step(&mut host);
    fake_step(&mut host);
    m.sync(&host, 2, &mut fx);
    assert!(fx.footprints().is_empty());
    assert_eq!(rings(&fx), 1);
}

#[test]
fn wading_unit_rings_and_sounds() {
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let diver = spawn(&mut host, DIVER, WATER + 64.0, at(5));
    set(&mut host, diver, status::MOVE, true);
    let o = object(&host, diver);
    let mut rng = MsvcRand::new(nml_effects::EFFECTS_SEED);
    let mut created = 0;
    let mut sounds = Vec::new();
    for _ in 0..8 {
        fake_step(&mut host);
        fake_step(&mut host);
        let before = rings(&fx);
        m.sync(&host, 2, &mut fx);
        created += rings(&fx) - before;
        sounds.extend(m.take_sounds());
        fx.step_effects();
    }
    // Walking: every 200 ms ring and sound (`SFX_WATERWALK<rand() % 8>`, flags 0x11, at `GO+0x08`).
    assert_eq!(created, 4);
    let expected: Vec<SoundRequest> = (0..4)
        .map(|_| SoundRequest::Event3d {
            name: format!("SFX_WATERWALK{}", rng.rand() % 8),
            flags: 0x11,
            position: Vec3::from(o.world_pos),
        })
        .collect();
    assert_eq!(sounds, expected);
    // The ring lies at the grid position at water level, in the yaw of the unit, tinted 1.5.
    let (mut below, mut above) = (EffectFrame::default(), EffectFrame::default());
    fx.effects
        .draw(Mat4::IDENTITY, &View, &mut below, &mut above);
    let grid = grid_to_world(o.grid, host.rules.map.cell);
    let ring = above
        .commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Model(d) => Some(d.clone()),
            DrawCommand::Quads(_) => None,
        })
        .expect("ring drawn");
    assert_eq!(
        Vec3::from(ring.world.translation),
        Vec3::new(grid[0], 0.0, grid[2])
    );
    assert_eq!((ring.tint, ring.fog), (Some(1.5), true));

    // Standing: ring at once on halting, then every 20 ticks (2 s), no sound.
    set(&mut host, diver, status::MOVE, false);
    let mut at_ticks = Vec::new();
    for t in 1..=41 {
        fake_step(&mut host);
        fake_step(&mut host);
        let before = rings(&fx);
        m.sync(&host, 2, &mut fx);
        if rings(&fx) > before {
            at_ticks.push(t);
        }
        fx.step_effects();
    }
    assert_eq!(at_ticks, [1, 21, 41]);
    assert!(m.take_sounds().is_empty());

    // Swimming: ring without sound; diving: nothing.
    set(&mut host, diver, status::MOVE | status::SWIMMING, true);
    fake_step(&mut host);
    fake_step(&mut host);
    let before = rings(&fx);
    m.sync(&host, 2, &mut fx);
    assert_eq!(rings(&fx), before + 1);
    assert!(m.take_sounds().is_empty());
    fx.step_effects();
    set(&mut host, diver, status::SWIMMING, false);
    set(&mut host, diver, status::DIVING, true);
    for _ in 0..20 {
        tick(&mut host, &mut fx, &mut m, 2);
    }
    assert_eq!(rings(&fx), 0, "old rings expired, no new ones");
    assert!(m.take_sounds().is_empty());
}

#[test]
fn ships_keep_their_ship_stand_and_rings_need_the_model() {
    let mut host = host();
    let mut fx = Fake::new(false);
    let mut m = MovementEffects::default();
    // Canoe (walk `0x20000`) in water: neither ring nor sound.
    let canoe = spawn(&mut host, CANOE, WATER + 64.0, at(3));
    set(&mut host, canoe, status::MOVE, true);
    // Wader without ring model in the archive: no ring, the sound stays.
    let wader = spawn(&mut host, WARRIOR, WATER + 64.0, at(6));
    set(&mut host, wader, status::MOVE, true);
    tick(&mut host, &mut fx, &mut m, 2);
    assert_eq!(rings(&fx), 0);
    assert_eq!(m.take_sounds().len(), 1);
}

/// Stagecoach (walk `0x20000` as in the shipped data) on sand with two wheels at head and trailer; head at index
/// 0, trailer at index 1.
struct Wagon {
    host: SimHost,
    fx: Fake,
    m: MovementEffects,
    head: ObjectId,
    trailer: ObjectId,
}

impl Wagon {
    fn new() -> Wagon {
        let mut host = train_host();
        let head = spawn_dir(&mut host, CART, 0, (3, 3), 0);
        // The rear part comes into being in the first object ticks.
        for _ in 0..4 {
            object_tick(&mut host);
        }
        host.state
            .object_mut(head, false)
            .unwrap()
            .module_mut(0)
            .unwrap()
            .base
            .flags |= 0x2_0000;
        let trailer = match &object(&host, head).module(multipart::HEAD).unwrap().kind {
            ModuleKind::MultiHead(h) => h.linked,
            _ => panic!("no front part"),
        };
        assert!(host.state.object(trailer, false).is_some(), "trailer there");
        let mut fx = Fake::new(true);
        fx.carriers.insert(head.0, 0);
        fx.carriers.insert(trailer.0, 1);
        // Sync from the state after the setup (like `MapView::set_particles_quality`).
        let mut m = MovementEffects::default();
        m.reset(&mut fx.effects, host.state.steps);
        let mut w = Wagon {
            host,
            fx,
            m,
            head,
            trailer,
        };
        w.place(at(3));
        w
    }

    /// Wheels at world position x.
    fn place(&mut self, x: f32) {
        let z = at(3);
        self.fx.nodes.insert(
            (self.head.0, WHEEL_NODE),
            vec![Vec3::new(x, 0.0, z - 20.0), Vec3::new(x, 0.0, z + 20.0)],
        );
        self.fx.nodes.insert(
            (self.trailer.0, WHEEL_NODE),
            vec![
                Vec3::new(x - 100.0, 0.0, z - 20.0),
                Vec3::new(x - 100.0, 0.0, z + 20.0),
            ],
        );
    }

    fn drive(&mut self, on: bool) {
        set(&mut self.host, self.head, status::MOVE, on);
    }

    fn tick(&mut self) {
        tick(&mut self.host, &mut self.fx, &mut self.m, 2);
    }

    /// Own effects of the object at index `object`: does it still run (not stopped)?
    fn owned(&mut self, object: usize) -> Vec<bool> {
        let keys: Vec<u32> = self
            .fx
            .effects
            .objects()
            .iter()
            .filter(|s| s.object == object)
            .flat_map(|s| s.keys())
            .collect();
        keys.into_iter()
            .map(|k| match self.fx.effects.owned_mut(object, k) {
                Some(OwnedKind::Attached(a)) => a.effect.is_active(),
                _ => false,
            })
            .collect()
    }
}

#[test]
fn wagon_dust_on_start_and_stop_and_treads_with_gaps() {
    let mut w = Wagon::new();
    w.drive(true);
    w.tick();
    // Starting: dust at rad01, rad02 of head and trailer, one running own effect each.
    assert_eq!(w.owned(0), [true, true]);
    assert_eq!(w.owned(1), [true, true]);
    assert_eq!(w.m.track(w.head).unwrap().dust().len(), 2);
    assert_eq!(w.m.take_new_owners(), [0, 0, 1, 1]);
    // Wheel tracks: one strip per wheel with the first point; no footprint (type 0, without `foot01`).
    assert_eq!(w.m.tread_keys(w.head).len(), 2);
    assert_eq!(w.m.tread_keys(w.trailer).len(), 2);
    assert_eq!(w.fx.effects.tread_points(), 4);
    assert!(w.fx.footprints().is_empty());
    // Further on sand: every 200 ms one point per wheel.
    for i in 1..=4 {
        w.place(at(3) + 10.0 * i as f32);
        w.tick();
    }
    assert_eq!(w.fx.effects.tread_points(), 4 * 3);
    let marks = w.fx.effects.ground_marks(&|_, _| 0.0, &|_| 7);
    assert_eq!(marks.treads.len(), 4 * 2);
    assert!(
        marks
            .treads
            .iter()
            .all(|q| q.alpha.iter().all(|&a| a <= 0x60))
    );
    // On grass (tile 20): a gap instead of a point, only once.
    for _ in 0..4 {
        w.place(at(20));
        w.tick();
    }
    let key = w.m.tread_keys(w.head)[0];
    let tread = w.fx.effects.tread_mut(key).unwrap();
    assert_eq!(tread.points().len(), 4);
    assert!(tread.points().back().unwrap().gap);
    // Halting: dust of the head stopped (stays at the object and runs out), the trailer hands all its own effects
    // over to the world.
    let world_before = w.fx.effects.counts().world;
    w.drive(false);
    w.tick();
    assert_eq!(w.owned(0), [false, false]);
    assert!(w.owned(1).is_empty());
    assert_eq!(w.fx.effects.counts().world, world_before + 2);
    assert!(w.m.track(w.head).unwrap().dust().is_empty());
    // At most 100 points per wheel (a gap at the maximum extends it by one).
    w.drive(true);
    for i in 0..300 {
        w.place(at(3) + i as f32);
        w.tick();
    }
    for id in [w.head, w.trailer] {
        for &key in w.m.tread_keys(id) {
            let n = w.fx.effects.tread_mut(key).unwrap().points().len();
            assert!(n <= 101, "{n} points");
        }
    }
}

#[test]
fn trailer_releases_all_its_effects_on_stop() {
    let mut w = Wagon::new();
    // An additional owned effect of the trailer goes to the world when halting too (D-274).
    add_node_effect(&mut w.fx, 1, "x01", WHEEL_DUST).unwrap();
    w.drive(true);
    w.tick();
    assert_eq!(w.owned(1).len(), 3);
    let world_before = w.fx.effects.counts().world;
    w.drive(false);
    w.tick();
    assert!(w.owned(1).is_empty());
    assert_eq!(w.fx.effects.counts().world, world_before + 3);
}

#[test]
fn dust_list_is_cleared_per_stop() {
    let mut w = Wagon::new();
    for _ in 0..2 {
        w.drive(true);
        w.tick();
        assert_eq!(
            w.m.track(w.head).unwrap().dust().len(),
            2,
            "only the new ones"
        );
        w.drive(false);
        w.tick();
        assert!(w.m.track(w.head).unwrap().dust().is_empty());
    }
    // Four dust effects at the head, all stopped.
    assert_eq!(w.owned(0), [false; 4]);
}

#[test]
fn quality_change_clears_marks_and_gates_new_ones() {
    let mut w = Wagon::new();
    // The stagecoach also has foot nodes here (track type 0: marks `0x802` at the nodes); a second one without
    // `0x20000` wades in water (ring and sound).
    let runner = w.head;
    w.fx.nodes
        .insert((runner.0, FOOT_NODE), vec![Vec3::new(at(3), 0.0, at(3))]);
    let wader = spawn_dir(&mut w.host, CART, 0, (30, 8), 0);
    set(&mut w.host, wader, status::MOVE, true);
    w.drive(true);
    for _ in 0..4 {
        w.tick();
    }
    assert!(!w.fx.footprints().is_empty());
    assert!(w.fx.effects.tread_points() > 0);
    assert!(rings(&w.fx) > 0);
    assert!(!w.m.take_sounds().is_empty());
    // Change to 0 (`MapView::set_particles_quality`): effect world empty, sync anew.
    let steps = w.host.state.steps;
    w.fx.effects.clear();
    w.m.reset(&mut w.fx.effects, steps);
    assert!(w.m.track(runner).is_none());
    assert!(w.m.tread_keys(w.head).is_empty());
    for _ in 0..6 {
        tick(&mut w.host, &mut w.fx, &mut w.m, 0);
    }
    assert!(w.fx.footprints().is_empty());
    assert_eq!(w.fx.effects.tread_points(), 0);
    assert_eq!(rings(&w.fx), 0);
    assert!(w.m.take_sounds().is_empty());
    assert!(w.m.track(runner).is_none(), "no counter either");
    // To 1: new tracks with 10 s, wheel tracks with new keys.
    let steps = w.host.state.steps;
    w.fx.effects.clear();
    w.m.reset(&mut w.fx.effects, steps);
    fake_step(&mut w.host);
    fake_step(&mut w.host);
    w.m.sync(&w.host, 1, &mut w.fx);
    let marks = w.fx.footprints();
    assert_eq!(marks.len(), 1);
    assert_eq!((marks[0].kind(), marks[0].life_ms()), (0x802, 10_000));
    assert_eq!(w.m.tread_keys(w.head).len(), 2);
    assert_eq!(w.fx.effects.tread_points(), 4);
}

#[test]
fn mark_count_is_bounded_by_the_lifetime() {
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let runners: Vec<ObjectId> = (0..200)
        .map(|i| spawn(&mut host, WARRIOR, at(i % SAND), at(i / SAND)))
        .collect();
    for &id in &runners {
        set(&mut host, id, status::MOVE, true);
    }
    for t in 1..=300 {
        tick(&mut host, &mut fx, &mut m, 2);
        let counts = fx.effects.mark_counts();
        let (count, listed): (usize, usize) = counts
            .iter()
            .fold((0, 0), |(c, l), x| (c + x.count, l + x.visible));
        // One mark per runner and 200 ms, 20 s listed: at most 100 listed per runner, in the vector up to 101 (the
        // freshly created one, not yet listed).
        assert!(listed <= 200 * 100, "tick {t}: {listed}");
        assert!(count <= 200 * 101, "tick {t}: {count}");
        if t >= 200 {
            assert_eq!(listed, 200 * 100, "tick {t}: equilibrium");
        }
    }
}

#[test]
fn movement_effects_restart_after_load() {
    let mut host = host();
    let mut fx = Fake::new(true);
    let mut m = MovementEffects::default();
    let runner = spawn(&mut host, WARRIOR, at(2), at(2));
    set(&mut host, runner, status::MOVE, true);
    for _ in 0..3 {
        tick(&mut host, &mut fx, &mut m, 2);
    }
    assert_eq!(fx.footprints().len(), 2);
    // A jump without a reset catches up nothing (more than 16 ticks).
    host.state.steps += 1000;
    m.sync(&host, 2, &mut fx);
    assert_eq!(fx.footprints().len(), 2);
    // Loading: effect world empty (rebuild), sync from the loaded tick; the first tick afterwards lays a track at
    // once (counter 0), the side begins anew.
    fx.effects.clear();
    m.reset(&mut fx.effects, host.state.steps);
    assert!(m.track(runner).is_none());
    m.sync(&host, 2, &mut fx);
    assert!(fx.footprints().is_empty(), "no tick caught up");
    fake_step(&mut host);
    m.sync(&host, 2, &mut fx);
    fake_step(&mut host);
    m.sync(&host, 2, &mut fx);
    let marks = fx.footprints();
    assert_eq!(marks.len(), 1);
    let o = object(&host, runner);
    assert_eq!(
        marks[0],
        Footprint::new(Vec3::from(o.world_pos), o.rotation[1], 1, 1, 2)
    );
}

#[test]
fn owner_change_keeps_the_track_and_vanished_objects_release_their_treads() {
    let mut w = Wagon::new();
    w.drive(true);
    w.tick();
    let track = w.m.track(w.head).cloned().unwrap();
    let keys = w.m.tread_keys(w.trailer).to_vec();
    // Re-hang onto a new ID (the fake no longer knows the old one).
    let new = ObjectId(w.head.0 ^ 0x4000_0000);
    w.m.rename(&[(w.head.0, new.0)]);
    assert_eq!(w.m.track(new), Some(&track));
    assert!(w.m.track(w.head).is_none());
    // Vanished IDs drop out at the sync; their wheel tracks are released.
    w.m.sync(&w.host, 2, &mut w.fx);
    assert!(w.m.track(new).is_none());
    let rules = w.host.rules.clone();
    {
        let mut sim = nml_sim::sim::Sim::new(&mut w.host.state, &rules);
        nml_sim::world::destroy_object(&mut sim, w.trailer);
    }
    w.m.sync(&w.host, 2, &mut w.fx);
    assert!(w.m.tread_keys(w.trailer).is_empty());
    for key in keys {
        assert!(w.fx.effects.tread_mut(key).unwrap().released());
    }
}

#[test]
fn movement_effects_leave_the_checksum_alone() {
    let run = |with_effects: bool| -> (Vec<u64>, usize) {
        let mut host = host();
        let mut fx = Fake::new(true);
        let mut m = MovementEffects::default();
        let runner = spawn(&mut host, WARRIOR, at(2), at(2));
        let wader = spawn(&mut host, WARRIOR, at(2), at(10));
        assert!(host.move_units(
            &[runner, wader],
            Vec3::new(at(12), 0.0, at(6)),
            None,
            FormationOrder::default(),
        ));
        let mut sums = Vec::new();
        for _ in 0..200 {
            host.step_once();
            if with_effects {
                m.sync(&host, 2, &mut fx);
                fx.step_effects();
            }
            sums.push(host.checksum());
        }
        (sums, fx.footprints().len())
    };
    let (plain, none) = run(false);
    let (with, marks) = run(true);
    assert_eq!(none, 0);
    assert!(marks > 0, "the runners left tracks");
    assert_eq!(plain, with);
}

/// Foot and wheel nodes rotate only around the yaw (`GameWorld_LocalOffsetToGridPos` `0x00479490` gets only
/// `GO+0x1C0` from `0x0046D190`): an x/z tilt of the object lays the track exactly as without tilt; the yaw rotates
/// it along.
#[test]
fn node_frame_ignores_tilt() {
    let mut plain = GameObject::new(ObjectId(1), 0);
    plain.world_pos = [500.0, 20.0, 700.0];
    plain.rotation = [0.0, 30.0, 0.0];
    let mut tilted = plain.clone();
    tilted.rotation = [25.0, 30.0, -40.0];
    let node = Vec3::new(12.0, 3.0, -7.0);
    let a = node_frame(&plain).transform_point3(node);
    let b = node_frame(&tilted).transform_point3(node);
    assert!((a - b).abs().max_element() < 1e-4, "{a} != {b}");
    let unturned = GameObject {
        rotation: [0.0; 3],
        ..plain.clone()
    };
    let c = node_frame(&unturned).transform_point3(node);
    assert!(
        (a - c).abs().max_element() > 1.0,
        "the yaw rotates the node"
    );
}
