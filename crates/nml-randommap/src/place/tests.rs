use std::cell::Cell;
use std::collections::BTreeMap;

use nml_formats::prj::{Project, ProjectObject, SelectionMarker};
use nml_formats::rdo::{MeshKind, RdoFile, RdoLayout, RdoMesh};
use nml_formats::rnd::{ObjectDef, PlaceObjectDef, RandomMapScript};
use nml_formats::testutil::{bounds, sample_rdo, write_rdo};
use nml_world::rand::MsvcRand;

use super::*;
use crate::PlayerArea;
use crate::checks::{Attempt, SetObject, inside_map};
use crate::grid::Grid;
use crate::objects::MapObject;
use crate::rng::Phase;
use crate::spec::{scale_factor, size_defines};

pub(crate) const S: f32 = 128.0;
/// Edge length of the test map.
pub(crate) const DIM: usize = 8;
/// Model `m.rdo`: box x/z 0 ... 100 (one tile from the position, sphere radius ~ 70.9).
pub(crate) const MODEL: u32 = 100;
/// Model `bad.rdo`, which the file source does not know (not loadable).
const UNLOADABLE: u32 = 200;
/// GUID without a project object.
const MISSING: u32 = 300;

pub(crate) fn project_object(guid: u32, model: &str) -> ProjectObject {
    ProjectObject {
        guid,
        name: format!("O{guid}"),
        model: model.into(),
        shadow_model: None,
        groundplane_model: None,
        scaffold_model: None,
        on_water: false,
        default_rotation: [0.0; 3],
        block_path: false,
        random_rot: false,
        selection: SelectionMarker::default(),
        upgrades: Vec::new(),
    }
}

/// `.rdo` with one rigid mesh and file hull x/z 0 ... 100, y 0 ... 10.
pub(crate) fn box_rdo() -> Vec<u8> {
    let b = bounds([0.0; 3], [100.0, 10.0, 100.0]);
    let sample = sample_rdo(RdoLayout::Standard);
    let mesh = RdoMesh {
        kind: MeshKind::from_type_id(3002).unwrap(),
        type_id: 3002,
        name: "kiste".into(),
        bounds: b.clone(),
        ..sample.meshes[2].clone()
    };
    write_rdo(&RdoFile {
        layout: RdoLayout::Standard,
        bounds: b,
        meshes: vec![mesh],
        bones: Vec::new(),
        materials: sample.materials,
    })
}

pub(crate) fn object(name: &str, guids: &[u32]) -> ObjectDef {
    ObjectDef {
        name: name.into(),
        guids: guids.to_vec(),
    }
}

/// Test world: map 8 x 8, flat, free; project with `MODEL` and `UNLOADABLE`.
pub(crate) struct World {
    pub(crate) grid: Grid,
    pub(crate) script: RandomMapScript,
    pub(crate) project: Project,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) steep: Vec<u8>,
    /// Player areas in list order.
    pub(crate) players: Vec<PlayerArea>,
    /// Reads per path.
    pub(crate) reads: Cell<[u32; 2]>,
}

/// A placer (`place_general`, `place_for_all_players`, `place_fill`).
type Placer = fn(&mut ObjectCtx<'_>, &PlaceObjectDef, &mut GenRand) -> anyhow::Result<()>;

impl World {
    pub(crate) fn new() -> Self {
        let mut project = Project::default();
        project
            .objects
            .insert(MODEL, project_object(MODEL, "Data\\m.rdo"));
        project
            .objects
            .insert(UNLOADABLE, project_object(UNLOADABLE, "Data\\bad.rdo"));
        let script = RandomMapScript {
            objects: vec![
                object("A", &[MODEL]),
                object("AA", &[MODEL, MODEL]),
                object("AB", &[MODEL, MISSING]),
                object("A", &[MISSING]),
                object("BAD", &[UNLOADABLE, UNLOADABLE]),
                object("EMPTY", &[]),
            ],
            ..RandomMapScript::default()
        };
        World {
            grid: Grid::new(DIM, DIM),
            script,
            project,
            files: BTreeMap::from([("data/m.rdo".to_string(), box_rdo())]),
            steep: vec![0; DIM * DIM],
            players: Vec::new(),
            reads: Cell::new([0; 2]),
        }
    }

    /// All tiles steep (blocked) except `free`.
    pub(crate) fn only_free(mut self, free: &[(usize, usize)]) -> Self {
        self.steep.fill(1);
        for &(x, z) in free {
            self.steep[DIM * z + x] = 0;
        }
        self
    }

    /// Player areas with center `(cx, cz)` in list order (`minSize` 0).
    fn with_players(mut self, centres: &[(i32, i32)]) -> Self {
        self.players = centres
            .iter()
            .map(|&(cx, cz)| PlayerArea {
                id: 0,
                cx,
                cz,
                min_size: 0,
            })
            .collect();
        self
    }

    /// Places `def` generally with seed `seed` and hook `set`.
    pub(crate) fn place(&mut self, def: &PlaceObjectDef, seed: u32, set: Option<SetObject>) -> Run {
        self.run(place_general, def, seed, set)
    }

    /// Places `def` with `placer`, seed `seed` and hook `set`; result, context excerpt and
    /// randomness.
    pub(crate) fn run(
        &mut self,
        placer: Placer,
        def: &PlaceObjectDef,
        seed: u32,
        set: Option<SetObject>,
    ) -> Run {
        self.run_with(seed, set, |ctx, rng| placer(ctx, def, rng))
    }

    /// Like [`World::run`], with an arbitrary flow over the context.
    pub(crate) fn run_with(
        &mut self,
        seed: u32,
        set: Option<SetObject>,
        body: impl FnOnce(&mut ObjectCtx<'_>, &mut GenRand) -> anyhow::Result<()>,
    ) -> Run {
        let (files, reads) = (&self.files, &self.reads);
        let mut read = |path: &str| {
            let mut r = reads.get();
            match path {
                "data/m.rdo" => r[0] += 1,
                "data/bad.rdo" => r[1] += 1,
                _ => {}
            }
            reads.set(r);
            files.get(path).cloned()
        };
        let mut ctx = ObjectCtx::new(
            &mut self.grid,
            &self.script,
            &self.players,
            &self.steep,
            &self.project,
            &mut read,
            S,
        );
        if let Some(set) = set {
            ctx.set_object = set;
        }
        let mut rng = GenRand::new(seed);
        rng.set_phase(Phase::Objects);
        let result = body(&mut ctx, &mut rng);
        Run {
            result,
            slots_used: ctx.slots_used,
            tiles: ctx.grid.tiles.clone(),
            objects: ctx.objects.clone(),
            centres: ctx.centres.clone(),
            centre: ctx.centre,
            attempts: ctx.attempts_run,
            checks: ctx.checks_run.get(),
            unloadable_cached: ctx.geometries.get("data/bad.rdo").map(Option::is_none),
            draws: rng.counts().objects,
        }
    }
}

pub(crate) struct Run {
    pub(crate) result: anyhow::Result<()>,
    pub(crate) slots_used: [bool; 8],
    /// Texture numbers of the grid after the run.
    pub(crate) tiles: Vec<i32>,
    pub(crate) objects: Vec<MapObject>,
    pub(crate) centres: Vec<[f32; 3]>,
    pub(crate) centre: Option<[f32; 2]>,
    /// Attempts as single object/center and as member.
    pub(crate) attempts: [u32; 2],
    pub(crate) checks: [u32; 8],
    pub(crate) unloadable_cached: Option<bool>,
    pub(crate) draws: u32,
}

/// Hook that only records (no block map): members may go onto the tile of the center.
fn record_only(ctx: &mut ObjectCtx<'_>, guid: u32, a: &Attempt, owner: i32, _rng: &mut GenRand) {
    ctx.objects.push(MapObject {
        guid,
        position: a.position,
        rotation_y: 0.0,
        owner,
    });
}

/// `NoGrid` block (position = drawn position, without snapping).
pub(crate) fn block(name: &str, number: i32, num_groups: i32) -> PlaceObjectDef {
    PlaceObjectDef {
        name: name.into(),
        number,
        num_groups,
        no_grid: true,
        ..PlaceObjectDef::default()
    }
}

pub(crate) fn positions(objects: &[MapObject]) -> Vec<[f32; 2]> {
    objects
        .iter()
        .map(|o| [o.position[0], o.position[2]])
        .collect()
}

/// Border check of the test map (sphere radius of the model).
fn inside(x: f32, z: f32) -> bool {
    inside_map(x, z, bounds([0.0; 3], [100.0, 10.0, 100.0]).radius, 8, 8, S)
}

/// Trace of a single object in the replay of the draws: draw tile corners until one lies inside the
/// border and not in `used`.
fn replay_single(rand: &mut MsvcRand, used: &mut Vec<[f32; 2]>) -> [f32; 2] {
    loop {
        let x = (rand.rand() as i32 % 8 * 128) as f32;
        let z = (rand.rand() as i32 % 8 * 128) as f32;
        if inside(x, z) && !used.contains(&[x, z]) {
            used.push([x, z]);
            return [x, z];
        }
    }
}

/// Trace of a member attempt around `centre` with the divisor `divisor` = `R + 1`.
fn replay_member(rand: &mut MsvcRand, centre: [f32; 2], divisor: i32) -> [f32; 2] {
    let a = Fx::from_int(rand.rand() as i32 % 361).0;
    let r1 = rand.rand() as i32 % divisor;
    let x = centre[0]
        + Fx(fixed_mul(
            Fx::from_int(128).0,
            fixed_mul(Fx::from_int(r1).0, sin_deg(a)),
        ))
        .to_f32();
    let r2 = rand.rand() as i32 % divisor;
    let z = centre[1]
        + Fx(fixed_mul(
            Fx::from_int(128).0,
            fixed_mul(Fx::from_int(r2).0, cos_deg(a)),
        ))
        .to_f32();
    [x, z]
}

#[test]
fn general_draws_two_per_single_attempt() {
    // re/random-map ZK-b.3: single objects draw % W, % H per attempt, nothing else (one GUID -> no R1).
    let mut w = World::new();
    let run = w.place(&block("A", 3, 1), 1, None);
    run.result.unwrap();
    assert_eq!(run.attempts[1], 0);
    assert_eq!(run.draws, 2 * run.attempts[0]);
    assert_eq!(run.checks[1], run.attempts[0]);
    // Positions like the trace: tile corners, block map after each placement (rectangle = tile of the
    // position).
    let mut rand = MsvcRand::new(1);
    let mut used = Vec::new();
    let expected: Vec<_> = (0..3)
        .map(|_| replay_single(&mut rand, &mut used))
        .collect();
    assert_eq!(positions(&run.objects), expected);
    assert!(run.objects.iter().all(|o| o.guid == MODEL && o.owner == 8));
    // Two GUIDs: plus R1 per object (before the attempts).
    let run = World::new().place(&block("AA", 3, 1), 1, None);
    run.result.unwrap();
    assert_eq!(run.objects.len(), 3);
    assert_eq!(run.draws, 3 + 2 * run.attempts[0]);
}

// QUIRK D-338: kept
#[test]
fn member_draws_three_per_attempt() {
    // re/random-map ZK-b.3: center 2 draws per attempt, member 3 (% 361, % (R+1), % (R+1)).
    let mut def = block("A", 3, 2);
    def.max_group_radius = 1;
    let run = World::new().place(&def, 1, Some(record_only));
    run.result.unwrap();
    assert_eq!(run.objects.len(), 6);
    assert!(run.attempts[1] >= 4);
    assert_eq!(run.draws, 2 * run.attempts[0] + 3 * run.attempts[1]);
    // First member like the trace: around the unsnapped center of the first center attempt.
    let mut rand = MsvcRand::new(1);
    let centre = replay_single(&mut rand, &mut Vec::new());
    assert_eq!(positions(&run.objects)[0], centre);
    let member = replay_member(&mut rand, centre, 2);
    assert!(inside(member[0], member[1]), "{member:?}");
    assert_eq!(positions(&run.objects)[1], member);
    assert_eq!(run.centres.len(), 2);
}

// QUIRK D-338: kept
#[test]
fn centre_in_attempt_101_stands_without_members() {
    // D-338: seed 442 hits the only free tile (3, 3) not until the 101st attempt. The center
    // stands, the members are omitted (j = number). The second group finds nothing free any more:
    // 101 attempts, no members. 2 x 101 x 2 draws.
    let mut w = World::new().only_free(&[(3, 3)]);
    let run = w.place(&block("A", 3, 2), 442, None);
    run.result.unwrap();
    assert_eq!(positions(&run.objects), vec![[384.0, 384.0]]);
    assert_eq!(run.attempts, [202, 0]);
    assert_eq!(run.draws, 404);
    assert_eq!(run.centres, vec![[384.0, 0.0, 384.0]]);
    // Counter-check: a single object (no group) succeeds with the same seed in the 101st
    // attempt as well, and the other objects keep trying.
    let run = World::new()
        .only_free(&[(3, 3)])
        .place(&block("A", 2, 1), 442, None);
    run.result.unwrap();
    assert_eq!(positions(&run.objects), vec![[384.0, 384.0]]);
    assert_eq!(run.attempts, [202, 0]);
}

// QUIRK D-338: kept
#[test]
fn failed_centre_drops_the_group() {
    // D-338: if the center fails in all 101 attempts, the whole group is omitted - no member
    // attempts, no draws for them. The center stays the last drawn attempt.
    let mut w = World::new().only_free(&[]);
    let run = w.place(&block("A", 3, 2), 1, None);
    run.result.unwrap();
    assert!(run.objects.is_empty());
    assert_eq!(run.attempts, [202, 0]);
    assert_eq!(run.draws, 404);
    assert!(run.centres.is_empty());
    let mut rand = MsvcRand::new(1);
    let mut last = [0.0; 2];
    for _ in 0..202 {
        last = [
            (rand.rand() as i32 % 8 * 128) as f32,
            (rand.rand() as i32 % 8 * 128) as f32,
        ];
    }
    assert_eq!(run.centre, Some(last));
}

// QUIRK D-354: kept
#[test]
fn members_scatter_around_the_previous_centre() {
    // D-354: group 1 places its center P1 (128, 896); group 2 fails the group distance check (8) in
    // all 101 attempts, its center is the last drawn attempt Q (640, 128); group 3 loses its object
    // j = 0 (GUID without a project object), its member (R = 0) stands exactly on Q - not on P1.
    // Seed 6 yields this GUID sequence.
    let mut def = block("AB", 2, 3);
    def.min_group_distance = 100;
    let run = World::new().place(&def, 6, Some(record_only));
    run.result.unwrap();
    assert_eq!(
        positions(&run.objects),
        vec![[128.0, 896.0], [640.0, 128.0]]
    );
    assert_eq!(run.centre, Some([640.0, 128.0]));
    assert_eq!(run.centres, vec![[128.0, 0.0, 896.0]]);
}

// QUIRK D-354: kept
#[test]
fn first_group_without_centre_is_an_error() {
    // D-354: seed 2 chooses the GUID without a project object for j = 0, the model for j = 1 - the
    // member would have no center (stack values in the original) -> message, without R4.
    let run = World::new().place(&block("AB", 2, 2), 2, None);
    let error = format!("{:#}", run.result.unwrap_err());
    assert!(error.contains("without group center"), "{error}");
    assert!(error.contains("\"AB\""), "{error}");
    assert_eq!(run.draws, 2);
    // Without members (number 1) no error, even if every center fails.
    let mut w = World::new();
    w.script.objects = vec![object("X", &[MISSING])];
    let run = w.place(&block("X", 1, 3), 2, None);
    run.result.unwrap();
    assert_eq!(run.draws, 0);
}

// QUIRK D-359: deviation
#[test]
fn zero_divisors_are_errors_negative_divisors_are_c_remainders() {
    // D-359: maxGroupRadius -1 -> divisor 0 -> message as soon as a member draws.
    let mut def = block("A", 2, 2);
    def.max_group_radius = -1;
    let run = World::new().place(&def, 1, Some(record_only));
    let error = format!("{:#}", run.result.unwrap_err());
    assert!(error.contains("maxGroupRadius -1"), "{error}");
    assert_eq!(positions(&run.objects).len(), 1);
    // Negative divisor: C remainder with the magnitude (R = -3 -> % -2 -> 0 or 1).
    assert_eq!(c_rem(7, -2).unwrap(), 1);
    assert_eq!(c_rem(32767, -2).unwrap(), 1);
    assert_eq!(c_rem(-7, 2).unwrap(), -1);
    assert_eq!(c_rem(5, i32::MIN).unwrap(), 5);
    assert!(c_rem(5, 0).is_err());
    def.max_group_radius = -3;
    let run = World::new().place(&def, 1, Some(record_only));
    run.result.unwrap();
    let mut rand = MsvcRand::new(1);
    let centre = replay_single(&mut rand, &mut Vec::new());
    let member = replay_member(&mut rand, centre, -2);
    assert!(inside(member[0], member[1]), "{member:?}");
    assert_eq!(positions(&run.objects)[..2], [centre, member]);
}

// QUIRK D-353: deviation
#[test]
fn object_without_guid_is_an_error() {
    // D-353: `#Object` without GUID -> message as soon as a placer chooses from it; no draw.
    let run = World::new().place(&block("EMPTY", 1, 1), 1, None);
    let error = format!("{:#}", run.result.unwrap_err());
    assert!(error.contains("#Object \"EMPTY\" has no GUID"), "{error}");
    assert_eq!(run.draws, 0);
    // `pick_guid`: k = 1 without a draw, k = 3 -> rand() % 3 (seed 1: 41 % 3 = 2).
    let mut rng = GenRand::new(1);
    assert_eq!(pick_guid(&object("E", &[7]), &mut rng).unwrap(), 7);
    assert_eq!(rng.counts().preprocessor, 0);
    assert_eq!(pick_guid(&object("D", &[7, 8, 9]), &mut rng).unwrap(), 9);
    assert!(pick_guid(&object("L", &[]), &mut rng).is_err());
    assert_eq!(rng.counts().preprocessor, 1);
    // The first block of the name is used (`A` appears twice in the script).
    let w = World::new();
    assert_eq!(object_guid(&w.script, "A", &mut rng).unwrap(), MODEL);
}

#[test]
fn unknown_object_name_draws_nothing() {
    // GUID -1: no project object, the object is omitted without a draw.
    let mut rng = GenRand::new(1);
    let w = World::new();
    assert_eq!(
        object_guid(&w.script, "MISSING", &mut rng).unwrap(),
        UNKNOWN_GUID
    );
    let run = World::new().place(&block("MISSING", 3, 2), 1, None);
    run.result.unwrap();
    assert!(run.objects.is_empty());
    assert_eq!((run.draws, run.attempts), (0, [0, 0]));
}

// QUIRK D-355: kept
#[test]
fn unloadable_model_draws_only_the_guid() {
    // D-355: unloadable model -> only R1, no attempts, nothing placed; the cache remembers the
    // failure (read once). No placing, hence no block mark, no random rotation and no
    // `GroundTexture` (the case "`GroundTexture` anyway" is unreachable in the map construction).
    let mut w = with_textures(World::new());
    w.project.objects.get_mut(&UNLOADABLE).unwrap().random_rot = true;
    let def = PlaceObjectDef {
        ground_texture: "F".into(),
        ..block("BAD", 3, 1)
    };
    let run = w.place(&def, 1, None);
    assert_eq!(w.reads.get(), [0, 1]);
    run.result.unwrap();
    assert!(run.objects.is_empty());
    assert!(run.tiles.iter().all(|&t| t == 1));
    assert_eq!(run.slots_used, [false; 8]);
    assert_eq!(run.draws, 3);
    assert_eq!(run.attempts, [0, 0]);
    assert_eq!(run.checks, [0; 8]);
    assert_eq!(run.unloadable_cached, Some(true));
}

#[test]
fn geometry_is_cached_across_objects() {
    let mut w = World::new();
    let run = w.place(&block("A", 3, 1), 1, None);
    run.result.unwrap();
    assert_eq!(run.objects.len(), 3);
    assert_eq!(w.reads.get(), [1, 0]);
}

// QUIRK D-325: kept
#[test]
fn scaling_matches_fun_0053fb20() {
    // `FUN_0053FB20` with n = 3 players, 128-tile map (map factor 2.0).
    let f = scale_factor(size_defines(128).1);
    let scale = |number, num_groups, players: bool, map: bool, groups: bool| {
        let def = PlaceObjectDef {
            number,
            num_groups,
            scale_to_players: players,
            scale_to_map_size: map,
            scale_groups_to_map_size: groups,
            ..PlaceObjectDef::default()
        };
        let d = scale_place_object(&def, 3, f);
        (d.number, d.num_groups)
    };
    // Without switches unchanged.
    assert_eq!(scale(2, 2, false, false, false), (2, 2));
    // ScaleToPlayers: numGroups only if > 1.
    assert_eq!(scale(2, 1, true, false, false), (6, 1));
    assert_eq!(scale(2, 2, true, false, false), (6, 6));
    // ScaleToMapSize: numGroups only if >= 2.
    assert_eq!(scale(3, 1, false, true, false), (6, 1));
    assert_eq!(scale(3, 2, false, true, false), (6, 4));
    // ScaleGroupsToMapSize without a condition, but only without ScaleToMapSize.
    assert_eq!(scale(3, 1, false, false, true), (3, 2));
    assert_eq!(scale(3, 1, false, true, true), (6, 1));
    // All three: first players, then map (the condition applies after the player scaling).
    assert_eq!(scale(2, 2, true, true, true), (12, 12));
    assert_eq!(scale(2, 1, true, true, false), (12, 1));
    // The map factor truncates (fixed point): 64-tile map 1.0, 256-tile map 4.0.
    let small = scale_factor(size_defines(64).1);
    let def = PlaceObjectDef {
        number: 5,
        scale_to_map_size: true,
        ..PlaceObjectDef::default()
    };
    assert_eq!(scale_place_object(&def, 3, small).number, 5);
    assert_eq!(
        scale_place_object(&def, 3, scale_factor(size_defines(256).1)).number,
        20
    );
}

// --- `ForAllPlayers` (`FUN_00544B40`), `Fill` (`FUN_00545760`), block loop ---

/// `ForAllPlayers` block with ring distance `min ... max` (NoGrid).
fn ring_block(name: &str, number: i32, num_groups: i32, min: i32, max: i32) -> PlaceObjectDef {
    PlaceObjectDef {
        for_all_players: true,
        min_dist_to_players: min,
        max_dist_to_players: max,
        ..block(name, number, num_groups)
    }
}

/// Trace of a ring attempt around the area `(cx, cz)`: R8 distance, R9 angle.
fn replay_ring(rand: &mut MsvcRand, (cx, cz): (i32, i32), min: i32, max: i32) -> [f32; 2] {
    let d = rand.rand() as i32 % (max - min + 1) + min;
    let a = Fx::from_int(rand.rand() as i32 % 361).0;
    let axis = |trig: i32, c: i32| {
        Fx(fixed_mul(
            Fx::from_int(128).0,
            fixed_mul(Fx::from_int(d).0, trig) + Fx::from_int(c).0,
        ))
        .to_f32()
    };
    [axis(sin_deg(a), cx), axis(cos_deg(a), cz)]
}

#[test]
fn ring_coordinate_matches_hand_values() {
    // `0x00544CDA…0x00544D34`: ToFloat(Mul(FromInt(S), Mul(FromInt(d), Sin(a)) + FromInt(c))).
    let deg = |d: i32| Fx::from_int(d).0;
    assert_eq!(ring_coord(128, 1, sin_deg(deg(90)), 4), 640.0);
    assert_eq!(ring_coord(128, 1, cos_deg(deg(90)), 4), 512.0);
    // Table value sin 30 degrees = 2047: (2*2047 + 4*4096)*128 / 4096 = 639.9375.
    assert_eq!(sin_deg(deg(30)), 2047);
    assert_eq!(ring_coord(128, 2, sin_deg(deg(30)), 4), 639.9375);
    assert_eq!(ring_coord(128, 0, sin_deg(deg(17)), 3), 384.0);
    // Negative sine: d * sin(270 degrees) = -1 tile.
    assert_eq!(ring_coord(128, 1, sin_deg(deg(270)), 4), 384.0);
}

// QUIRK D-338: kept
#[test]
fn for_all_players_draws_two_plus_two() {
    // re/random-map ZK-b.3: per attempt R8/R9 (`% (max − min + 1)`, `% 361`), a member additionally
    // R10/R11 (`% (R+1)` per axis) with the angle already drawn; the distance is discarded.
    // Single objects: position on the ring around the area (trace).
    let mut w = World::new().with_players(&[(4, 4)]);
    let run = w.run(
        place_for_all_players,
        &ring_block("A", 3, 1, 1, 2),
        5,
        Some(record_only),
    );
    run.result.unwrap();
    assert_eq!(run.attempts, [3, 0]);
    assert_eq!(run.draws, 6);
    let mut rand = MsvcRand::new(5);
    let expected: Vec<_> = (0..3)
        .map(|_| replay_ring(&mut rand, (4, 4), 1, 2))
        .collect();
    assert_eq!(positions(&run.objects), expected);
    assert!(run.objects.iter().all(|o| o.owner == 8));
    // Groups on two areas: per area and group one center (2 draws) and one member
    // (4 draws); R = 0, distance 0 -> everything exactly on the area center.
    let mut w = World::new().with_players(&[(2, 2), (5, 5)]);
    let run = w.run(
        place_for_all_players,
        &ring_block("A", 2, 2, 0, 0),
        1,
        Some(record_only),
    );
    run.result.unwrap();
    assert_eq!(run.attempts, [4, 4]);
    assert_eq!(run.draws, 4 * 2 + 4 * 4);
    let expected: Vec<_> = [[256.0, 256.0]; 4]
        .into_iter()
        .chain([[640.0, 640.0]; 4])
        .collect();
    assert_eq!(positions(&run.objects), expected);
    // Member with R = 1: around the unsnapped center, with the angle from R9 (trace).
    let mut def = ring_block("A", 2, 2, 1, 1);
    def.max_group_radius = 1;
    let run =
        World::new()
            .with_players(&[(4, 4)])
            .run(place_for_all_players, &def, 3, Some(record_only));
    run.result.unwrap();
    let mut rand = MsvcRand::new(3);
    let centre = replay_ring(&mut rand, (4, 4), 1, 1);
    let _d = rand.rand();
    let a = Fx::from_int(rand.rand() as i32 % 361).0;
    let r1 = rand.rand() as i32 % 2;
    let x = centre[0] + member_offset(128, r1, sin_deg(a));
    let r2 = rand.rand() as i32 % 2;
    let z = centre[1] + member_offset(128, r2, cos_deg(a));
    assert_eq!(positions(&run.objects)[..2], [centre, [x, z]]);
}

// QUIRK D-339: kept
#[test]
fn failed_town_center_draws_202() {
    // re/random-map ZK-b.3, D-339: `TOWN_CENTER` (min = max = 0, IsPlayer) on a steep area center:
    // the position does not depend on the angle, all 101 attempts fail equally -> 202 draws, nothing
    // placed, slot 0 stays free. The second area gets owner 1 (the counter ran over the failed
    // body).
    let mut w = World::new()
        .with_players(&[(4, 4), (2, 5)])
        .only_free(&[(2, 5)]);
    let mut def = ring_block("A", 1, 1, 0, 0);
    def.is_player = true;
    let run = w.run(place_for_all_players, &def, 1, None);
    run.result.unwrap();
    assert_eq!(run.attempts, [102, 0]);
    assert_eq!(run.draws, 202 + 2);
    assert_eq!(positions(&run.objects), vec![[256.0, 640.0]]);
    assert_eq!(run.objects[0].owner, 1);
    let mut slots = [false; 8];
    slots[1] = true;
    assert_eq!(run.slots_used, slots);
}

// QUIRK D-338, D-339: kept
#[test]
fn is_player_counts_loop_bodies_not_areas() {
    // D-339: three areas, number 2 -> owners 0 ... 5 (one counter across all areas), not
    // 0, 0, 1, 1, 2, 2.
    let mut def = ring_block("A", 2, 1, 0, 0);
    def.is_player = true;
    let run = World::new().with_players(&[(1, 1), (4, 4), (6, 6)]).run(
        place_for_all_players,
        &def,
        1,
        Some(record_only),
    );
    run.result.unwrap();
    let owners: Vec<_> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, [0, 1, 2, 3, 4, 5]);
    // `0x0054515B`: members skipped by `j = number` do not count. Area 1: group 0 center 0,
    // member 1; group 1 fails at (8) 101 times (counter 2, member skipped).
    // Area 2: group 0 center 3, member 4; group 1 fails (5).
    let mut def = ring_block("A", 2, 2, 0, 0);
    def.is_player = true;
    def.min_group_distance = 5;
    let run = World::new().with_players(&[(1, 1), (6, 6)]).run(
        place_for_all_players,
        &def,
        1,
        Some(record_only),
    );
    run.result.unwrap();
    let owners: Vec<_> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, [0, 1, 3, 4]);
    assert_eq!(run.draws, 2 * (2 + 4 + 202));
    // The placer passes the counter on raw; from 8 on, placing makes it neutral
    // (`FUN_0053FD00`, `objects::set_object`).
    let mut def = ring_block("A", 10, 1, 0, 0);
    def.is_player = true;
    let run =
        World::new()
            .with_players(&[(4, 4)])
            .run(place_for_all_players, &def, 1, Some(record_only));
    run.result.unwrap();
    let owners: Vec<_> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, (0..10).collect::<Vec<_>>());
    let areas = [
        (1, 1),
        (1, 3),
        (1, 5),
        (3, 1),
        (3, 3),
        (3, 5),
        (5, 1),
        (5, 3),
        (5, 5),
        (6, 6),
    ];
    let mut def = ring_block("A", 1, 1, 0, 0);
    def.is_player = true;
    let run = World::new()
        .with_players(&areas)
        .run(place_for_all_players, &def, 1, None);
    run.result.unwrap();
    let owners: Vec<_> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, [0, 1, 2, 3, 4, 5, 6, 7, 8, 8]);
    assert_eq!(run.slots_used, [true; 8]);
    // Failed objects (GUID without a project object) count too: seed 2 first chooses the missing
    // GUID, then the model -> owner 1.
    let mut def = ring_block("AB", 2, 1, 0, 0);
    def.is_player = true;
    let run =
        World::new()
            .with_players(&[(4, 4)])
            .run(place_for_all_players, &def, 2, Some(record_only));
    run.result.unwrap();
    let owners: Vec<_> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, [1]);
}

// QUIRK D-354: kept
#[test]
fn group_centres_span_all_player_areas() {
    // The list of group centers (8) applies across all areas of the block. Area 1 places its
    // first center; every further center (also on area 2, 2.8 tiles away) lies closer than
    // 100 tiles -> 101 failed attempts each.
    let mut def = ring_block("A", 1, 2, 0, 0);
    def.min_group_distance = 100;
    let run = World::new().with_players(&[(2, 2), (4, 4)]).run(
        place_for_all_players,
        &def,
        1,
        Some(record_only),
    );
    run.result.unwrap();
    assert_eq!(positions(&run.objects), vec![[256.0, 256.0]]);
    assert_eq!(run.centres, vec![[256.0, 0.0, 256.0]]);
    assert_eq!(run.draws, 2 + 3 * 202);
}

// QUIRK D-354: kept
#[test]
fn members_of_the_next_area_scatter_around_the_previous_centre() {
    // D-354 with `ForAllPlayers` (`0x00544E6C…0x00544E83`): the center applies across all areas.
    // Seed 17 chooses (GUID `AB`): area 1 group 0 j = 0 model (center (2, 2)), then missing
    // GUIDs; on area 2 every j = 0 fails, the members (R = 0) stand on the center of
    // area 1.
    let def = ring_block("AB", 2, 2, 0, 0);
    let run = World::new().with_players(&[(2, 2), (5, 5)]).run(
        place_for_all_players,
        &def,
        17,
        Some(record_only),
    );
    run.result.unwrap();
    assert_eq!(positions(&run.objects), [[256.0, 256.0]; 3]);
    assert_eq!(run.centre, Some([256.0, 256.0]));
    assert_eq!(run.draws, 8 + 2 + 4 + 4);
}

// QUIRK D-354: kept
// QUIRK D-359: deviation
#[test]
fn ring_divisor_zero_is_an_error() {
    // D-359: maxDist - minDist + 1 == 0 -> message instead of a division by zero.
    let run = World::new().with_players(&[(4, 4)]).run(
        place_for_all_players,
        &ring_block("A", 1, 1, 3, 2),
        1,
        None,
    );
    let error = format!("{:#}", run.result.unwrap_err());
    assert!(error.contains("minDistToPlayers 3"), "{error}");
    assert!(error.contains("maxDistToPlayers 2"), "{error}");
    // Negative divisor (max - min + 1 = -1): C remainder 0, d = min.
    let run = World::new().with_players(&[(4, 4)]).run(
        place_for_all_players,
        &ring_block("A", 1, 1, 1, -1),
        1,
        None,
    );
    run.result.unwrap();
    assert_eq!(run.objects.len(), 1);
    // First group without a center (j = 0 fails, a member would have to draw) -> message.
    let run = World::new().with_players(&[(4, 4)]).run(
        place_for_all_players,
        &ring_block("AB", 2, 2, 0, 0),
        2,
        None,
    );
    let error = format!("{:#}", run.result.unwrap_err());
    assert!(error.contains("without group center"), "{error}");
}

/// `Fill` block (NoGrid).
pub(crate) fn fill_block(name: &str) -> PlaceObjectDef {
    PlaceObjectDef {
        fill: true,
        ..block(name, 1, 1)
    }
}

/// Script with the textures `G` (1) and `F` (2), grid entirely `G`.
pub(crate) fn with_textures(mut w: World) -> World {
    w.script.textures = ["G", "F"]
        .iter()
        .zip(1..)
        .map(|(name, number)| nml_formats::rnd::TextureDef {
            number,
            name: name.to_string(),
            height: 0.0,
            guids: Vec::new(),
        })
        .collect();
    w.grid.tiles.fill(1);
    w
}

#[test]
fn fill_runs_x_outer_z_inner_and_ignores_number() {
    // `FUN_00545760`: x outer, z inner over the whole map; position (x*SX, z*SZ); number and
    // numGroups have no effect, border (6) and PutOnFlat (5) do not run - (0, 0) stands on the
    // map edge. Two GUIDs: R1 per tile.
    let mut def = fill_block("AA");
    def.number = 5;
    def.num_groups = 3;
    def.put_on_flat = true;
    let run = World::new().run(place_fill, &def, 1, None);
    run.result.unwrap();
    let expected: Vec<_> = (0..8)
        .flat_map(|x| (0..8).map(move |z| [(x * 128) as f32, (z * 128) as f32]))
        .collect();
    assert_eq!(positions(&run.objects), expected);
    assert_eq!(run.draws, 64);
    assert_eq!(run.checks[4], 0, "PutOnFlat");
    assert_eq!(run.checks[5], 0, "border");
    assert_eq!(run.checks[6], 64, "occupancy");
    assert!(run.objects.iter().all(|o| o.owner == 8));
    assert!(run.centres.is_empty());
    // With snapping: origin to the tile center (hull 100 < 128).
    let mut def = fill_block("A");
    def.no_grid = false;
    let run = World::new().run(place_fill, &def, 1, Some(record_only));
    run.result.unwrap();
    assert_eq!(positions(&run.objects)[..2], [[64.0, 64.0], [64.0, 192.0]]);
}

#[test]
fn fill_draws_guid_only_on_passing_tiles() {
    // re/random-map ZK-b.3 R1: only on tiles that pass (3)/(4) (`FUN_00543410` at the tile itself,
    // before the GUID); the occupancy (7) is checked only afterwards - a blocked tile draws anyway.
    let mut w = with_textures(World::new());
    for (x, z) in [(1, 2), (3, 3), (6, 0)] {
        w.grid.set_tile(x, z, 2);
    }
    w.steep[DIM * 3 + 3] = 1;
    let mut def = fill_block("AA");
    def.put_onto = "F".into();
    let run = w.run(place_fill, &def, 1, None);
    run.result.unwrap();
    assert_eq!(run.draws, 3);
    assert_eq!(positions(&run.objects), vec![[128.0, 256.0], [768.0, 0.0]]);
    // AvoidPlayerAreas at the tile: minSize 3 around (4, 4) blocks the circle.
    let mut w = World::new().with_players(&[(4, 4)]);
    w.players[0].min_size = 3;
    let mut def = fill_block("AA");
    def.avoid_player_areas = true;
    let run = w.run(place_fill, &def, 1, None);
    run.result.unwrap();
    let outside = (0..8i32)
        .flat_map(|x| (0..8i32).map(move |z| (x - 4, z - 4)))
        .filter(|&(dx, dz)| dx * dx + dz * dz >= 9)
        .count() as u32;
    assert_eq!(run.draws, outside);
    assert_eq!(run.objects.len() as u32, outside);
}

// QUIRK D-362: kept
#[test]
fn ground_texture_is_visible_to_the_next_fill_tile() {
    // D-362: `PutOnto G` with `AvoidOther 1`, `GroundTexture F`: every placed object paints its
    // tile `F` (`FUN_00543260` after placing). The next tile in the same pass sees that
    // immediately (no snapshot): only tiles with even x and z get an object.
    let mut w = with_textures(World::new());
    let mut def = fill_block("A");
    def.put_onto = "G".into();
    def.avoid_other = 1;
    def.ground_texture = "F".into();
    let run = w.run(place_fill, &def, 1, None);
    run.result.unwrap();
    let expected: Vec<_> = (0..8)
        .step_by(2)
        .flat_map(|x| {
            (0..8)
                .step_by(2)
                .map(move |z| [(x * 128) as f32, (z * 128) as f32])
        })
        .collect();
    assert_eq!(positions(&run.objects), expected);
    assert_eq!(run.tiles.iter().filter(|&&t| t == 2).count(), 16);
}

#[test]
fn place_objects_scales_once_and_dispatches_in_script_order() {
    // `FUN_005468D0`: blocks in script order, `Fill` before `ForAllPlayers` before general;
    // `FUN_0053FB20` once per block before the dispatch with the number of players `G+0x120`
    // (here 3, not the number of areas) and the map factor.
    let mut w = with_textures(World::new().with_players(&[(1, 1), (6, 6)]));
    w.grid.set_tile(0, 7, 2);
    let mut fill = fill_block("A");
    fill.put_onto = "F".into();
    fill.for_all_players = true;
    let mut ring = ring_block("A", 1, 1, 0, 0);
    ring.scale_to_players = true;
    ring.is_player = true;
    let mut general = block("A", 1, 1);
    general.scale_to_map_size = true;
    w.script.placements = vec![fill, ring, general];
    let scale = scale_factor(size_defines(128).1);
    let run = w.run_with(1, Some(record_only), |ctx, rng| {
        place_objects(ctx, 3, scale, rng)
    });
    run.result.unwrap();
    // Fill: one tile; ForAllPlayers: 3 per area (owners 0 ... 5); general: 1 * 2.0 = 2.
    let owners: Vec<_> = run.objects.iter().map(|o| o.owner).collect();
    assert_eq!(owners, [8, 0, 1, 2, 3, 4, 5, 8, 8]);
    assert_eq!(run.objects[0].position, [0.0, 0.0, 896.0]);
    let expected: Vec<_> = [[128.0, 128.0]; 3]
        .into_iter()
        .chain([[768.0, 768.0]; 3])
        .collect();
    assert_eq!(positions(&run.objects)[1..7], expected[..]);
}

#[test]
fn place_object_block_ends_only_at_a_closing_brace() {
    // `FUN_00544510` leaves the read loop only at a line that is exactly `}` (`0x0054497B`) and
    // scales exactly there (`0x00544984`); a block up to the end of file never ends in the original
    // (D-324), the reader reports it. So every block read is scaled exactly once - the reader
    // delivers the raw values, `place_objects` scales.
    let parse = |text: &str| {
        let mut read = |_: &str| Some(text.as_bytes().to_vec());
        nml_formats::rnd::parse_random_map_script("s.rnd", Vec::new(), &mut read, &mut || 0)
    };
    let script = parse("place_Object A\n{\nnumber 2\nScaleToPlayers\n}\n").unwrap();
    assert_eq!(script.placements[0].number, 2);
    let error = parse("place_Object A\n{\nnumber 2\nScaleToPlayers\n} \n").unwrap_err();
    assert!(
        error.to_string().contains("end of file in block"),
        "{error}"
    );
    let mut w = World::new();
    w.script.placements = script.placements;
    let run = w.run_with(1, Some(record_only), |ctx, rng| {
        place_objects(ctx, 3, Fx::ONE, rng)
    });
    run.result.unwrap();
    assert_eq!(run.objects.len(), 6);
}
