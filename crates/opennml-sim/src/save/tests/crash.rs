//! Combined test of the crash checks on loading.
//!
//! For each remaining check that prevents a crash, a hang or unbounded memory there is one row here: a base scene
//! ([`Base`]), a damage to the savegame and the expected outcome ([`Expect`]). The loading path is the same for all
//! rows: `SaveGame::from_bytes` of the newly signed bytes, then `check` with the rules of the scene; for
//! [`Expect::Runs`] then [`RUN_TICKS`] ticks. Which check rejects and with which message does not matter (the message
//! appears only in the output); what is required is "rejected" or "loads and runs without a panic".
//!
//! Each row runs in its own thread ([`ROW_STACK`]) under `catch_unwind` with the time limit [`ROW_TIMEOUT`]; panic,
//! timeout and wrong outcome are collected and reported at the end with the row `id`. A stack overflow cannot be
//! caught: it ends the whole test program and counts as a failure just the same.
//!
//! The `id` is the number of the check (A or B series), with a suffix where a check has several rows. If a check is
//! dropped because the run time tolerates the value, its row switches to [`Expect::Runs`]; if it is dropped because a
//! type fixes the length, the row is removed.
//!
//! The fixed lengths A1 (object tables), A6 (players), A18 (upgrades per slot), A22 (module slots), A77/B202 (building
//! mask), A83 (layers of the path grid), A93 (planner), B16 (type tables), B88 (attribute lists), B143 (name table),
//! B163/B219 (callbacks) and B288 (group tables) are carried by the type `FixedVec`; that it rejects every other length
//! and serializes like `Vec` is shown by `crate::fixed_vec::tests::fixed_vec_serializes_like_vec`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::group::{FormationState, NO_GROUP, key};
use crate::id::{ObjectId, SLOT_CAPACITY};
use crate::module::Module;
use crate::object::status;

/// Time limit per row (debug build, including the setup of the scene).
const ROW_TIMEOUT: Duration = Duration::from_secs(60);
/// Stack of a row thread: in the debug build `rmp_serde` takes about 5 KB per nesting level, the depth limit
/// `codec::MAX_DEPTH` thus about 2.5 MB (release build: about 0.5 KB per level). The deep rows nest 100,000 levels and
/// would overflow it many times over without their limits.
const ROW_STACK: usize = 8 << 20;
/// Ticks after loading with [`Expect::Runs`].
const RUN_TICKS: usize = 4;

#[derive(Clone, Copy, Debug)]
enum Expect {
    /// Loading or the rules check reports an error.
    Rejected,
    /// Loading and the rules check succeed, the ticks run without a panic.
    Runs,
}

/// Base scenes of the rows.
#[derive(Clone, Copy, Debug)]
enum Base {
    /// `scene`: six objects on three slots, 16 x 16 tiles.
    Plain,
    /// `random_scene`: random map with game mode 3.
    Random,
    /// `walking_save`: two colonists walk toward each other (planner, records in use).
    Walking,
    /// `wall_world`: wall group of three finished houses.
    Walls,
    /// `train_world`: train with two slaves, cart with a rear part.
    Train,
    /// `ki_scene`: AI engine on slot 2 with two threads.
    Ki,
    /// [`march_scene`]: line with a rear group on the march.
    Groups,
    /// Empty state on a synthetic map of 17 x 16 tiles (width not a multiple of the sector size 16).
    /// Does not load; only for the row "A83 width", whose check would otherwise be covered by `SaveGame::check` (map
    /// size of the rules, B64) or the fog (B101).
    OddMap,
}

struct Case {
    id: &'static str,
    base: Base,
    damage: fn(&mut SaveGame),
    expect: Expect,
}

use Base::*;
use Expect::*;

/// Savegame of the state with the start package from `testkit::setup`.
fn wrap(rules: &Rules, state: SimState) -> SaveGame {
    SaveGame {
        start: start_package(rules, &testkit::setup()),
        state,
        commands: Vec::new(),
    }
}

fn base(base: Base) -> (Rules, SaveGame) {
    let (rules, state) = match base {
        Plain => scene(),
        // The game mode of the start package must be the one of the map description (B73), hence its own setup.
        Random => {
            let (rules, setup, state) = random_scene(random_spec("ZKA", 3), 0x1234);
            let save = random_save(&rules, &setup, &state);
            return (rules, save);
        }
        Walking => {
            let (rules, save, _) = walking_save();
            (rules, save.state)
        }
        Walls => {
            let (rules, save, _) = wall_world();
            (rules, save.state)
        }
        Train => {
            let (rules, state, _) = train_world();
            (rules, state)
        }
        Ki => ki_scene(),
        Groups => march_scene(),
        OddMap => {
            // `new_match` rejects the map size; the state is therefore created as in the module test worlds. The
            // height stays 16 (terrain 17 x 17, of which the map size of the rules applies), so that only the width
            // violates the sector division.
            let mut rules = Rules::synthetic(TYPES, 17);
            rules.map.dim_z = 16;
            let mut state = SimState::new(0, 0);
            state.init_map(&rules);
            (rules, state)
        }
    };
    let save = wrap(&rules, state);
    (rules, save)
}

/// Eight units of the group test world (four infantrymen, two pikemen, two warriors with rank 0) as a line
/// (formation 1) on the march: the main group carries a formation, the warriors stand in its rear group.
fn march_scene() -> (Rules, SimState) {
    use crate::group::testkit as gk;
    let rules = gk::rules();
    let mut state = new_match(&rules, &testkit::setup()).unwrap();
    let units = {
        let mut sim = crate::sim::Sim::new(&mut state, &rules);
        let mut units = Vec::new();
        for (guid, n) in [(gk::INFANTRY, 4), (gk::PIKEMAN, 2), (gk::WARRIOR, 2)] {
            for _ in 0..n {
                let x = 10 + units.len() as i32;
                units.push(gk::spawn(&mut sim, guid, (x, 10)));
            }
        }
        units
    };
    let before = state.object(units[0], false).unwrap().world_pos;
    let order = gk::mv(&units, (20, 30), 1, crate::motion::NO_DIRECTION);
    let _ = step(&mut state, &rules, &[order]);
    for _ in 0..20 {
        let _ = step(&mut state, &rules, &[]);
    }
    assert_ne!(
        state.object(units[0], false).unwrap().world_pos,
        before,
        "moving"
    );
    formation_with_rear(&state).expect("formation with a rear group");
    (rules, state)
}

/// IDs of the main group with a formation and its rear group.
fn formation_with_rear(state: &SimState) -> Option<(i32, i32)> {
    state.groups.map.iter().find_map(|(_, g)| {
        let rear = g.formation.as_ref()?.rear;
        (rear != NO_GROUP).then_some((g.id, rear))
    })
}

/// Formation 1 with rear group `rear` (without ranks).
fn line_with_rear(rear: i32) -> Option<Box<FormationState>> {
    let mut f = FormationState::new(1);
    f.rear = rear;
    Some(Box::new(f))
}

/// All objects of the state in table order.
fn ids(s: &SaveGame) -> Vec<ObjectId> {
    s.state.objects().map(|o| o.id).collect()
}

fn object(s: &mut SaveGame, id: ObjectId) -> &mut crate::object::GameObject {
    s.state.object_mut(id, false).unwrap()
}

/// First unit with a walk module.
fn walker(s: &SaveGame) -> ObjectId {
    let o = s.state.objects().find(|o| o.walk().is_some());
    o.expect("unit with walk").id
}

fn walk(s: &mut SaveGame) -> &mut crate::module::walk::Walk {
    let id = walker(s);
    walk_of(s, id)
}

/// Length of the large lists of the rows "L5 ...".
const L5_LEN: usize = 200_000;

/// The `i`-th of pairwise distinct IDs (slot 0, index `i % 1000`, generation from 1).
fn distinct_id(i: usize) -> ObjectId {
    ObjectId::new((i % 1000) as u16, 0, (i / 1000 + 1) as u16)
}

/// The two walkers of the walking scene.
fn walkers(s: &SaveGame) -> [ObjectId; 2] {
    let units: Vec<ObjectId> = s
        .state
        .objects()
        .filter(|o| o.walk().is_some())
        .map(|o| o.id)
        .collect();
    [units[0], units[1]]
}

/// B180: the first walker has a fixed home (`mobile_home` off) at its position with tether `i32::MAX` and in its
/// input queue a FINDTO_UNIT (`0x400`, flag `0x02` keeps the home fixed) on the second one, with a minimum distance of
/// 64 tiles outside the attack distance. The next tick processes it, and `on_msg_find_to_unit` checks the tether with
/// `tether + dist` (`seek.rs`, wrapped like x86; unwrapped, the row panics in the debug build).
fn tether_to_quarry(s: &mut SaveGame) {
    let [me, quarry] = walkers(s);
    let o = object(s, me);
    let (p, dir) = (o.grid, o.direction.0);
    let msg =
        crate::message::Message::new(0x400, &[quarry.as_i32(), 64 << 12, 0, 0, 0, 0x02, 0, 0]);
    o.fifo
        .push_back(crate::message::Posted::Message { msg, abort: false });
    let w = walk_of(s, me);
    (w.home, w.tether, w.mobile_home) = ((p.x.0, p.z.0, dir), i32::MAX, false);
}

/// Row L4: the first walker escorts the second (escorting, no attack) with a minimum distance that sets the
/// push-out radius `desired = min_attack_radius + radii` to `349 525 · 4096`. Then
/// `rr = fixed_mul(desired, 1.5) = 2 147 481 600`, just below `i32::MAX`; `i32::MAX` itself would already overflow in
/// `desired` or give `rr = 0` (shift amount `12 − 13` modulo 32) and not reach the overflow. The last known position
/// lies two tiles in +x next to the target (angle 0, cosine 1): the next walk tick replans (counter 9 -> 10, distance
/// above 1 tile, distance to the target above the maximum distance -1), `PlanHighPathToUnit` pushes out, and
/// `PushOutPoint` computes `x + fixed_mul(rr, cos)` beyond `i32::MAX` (`seek.rs`, `push_out_point`).
fn escort_far_away(s: &mut SaveGame) {
    let [me, quarry] = walkers(s);
    let radius = |s: &mut SaveGame, id| object(s, id).hard_radius.max(0);
    let radii = radius(s, me) + radius(s, quarry);
    let q = object(s, quarry).grid;
    let w = walk_of(s, me);
    (w.quarry, w.escort, w.attack, w.ticks_since_update) = (quarry, true, false, 9);
    (w.min_attack_radius, w.max_attack_radius) = ((349_525 << 12) - radii, -1);
    w.last_known_pos = (q.x.0 + 0x2000, q.z.0);
}

/// Wall group `[master, member, member]`.
fn walls(s: &SaveGame) -> [ObjectId; 3] {
    s.state
        .objects()
        .find_map(|o| match &o.module(crate::module::building::INDEX)?.kind {
            ModuleKind::Building(b) if b.slaves.len() == 2 => {
                Some([o.id, b.slaves[0], b.slaves[1]])
            }
            _ => None,
        })
        .expect("wall group")
}

fn master(s: &mut SaveGame) -> &mut crate::module::building::Building {
    let [a, _, _] = walls(s);
    building_of(s, a)
}

/// Locomotive (train module with slaves).
fn loco(s: &mut SaveGame) -> &mut crate::module::train::Train {
    let id = s
        .state
        .objects()
        .find_map(|o| match &o.module(crate::module::train::INDEX)?.kind {
            ModuleKind::Train(t) if !t.slaves.is_empty() => Some(o.id),
            _ => None,
        });
    train_of(&mut s.state, id.expect("locomotive"))
}

/// Locomotive moving forward with a valid acceleration time and target.
fn moving_loco(s: &mut SaveGame) -> &mut crate::module::train::Train {
    use crate::module::train::status::{FORWARD, MOVING};
    let t = loco(s);
    (t.status, t.acc_time, t.target) = (MOVING | FORWARD, 100, Some((5, 6)));
    t
}

/// Rear part of the cart.
fn cart_tail(s: &mut SaveGame) -> &mut crate::module::multipart::MultiPart {
    use crate::module::multipart::PART;
    let id = s.state.objects().find(|o| o.module(PART).is_some());
    let id = id.expect("rear part").id;
    match &mut object(s, id).module_mut(PART).unwrap().kind {
        ModuleKind::MultiPart(p) => p,
        _ => unreachable!(),
    }
}

/// Planner 1 anew: one root node, otherwise default (valid, cf. `Planner::is_consistent`).
fn fresh_planner(s: &mut SaveGame) -> &mut crate::bstar::Planner {
    let p = &mut s.state.planners[1];
    *p = crate::bstar::Planner::default();
    p.nodes = vec![crate::bstar::Node {
        x: 0,
        z: 0,
        dir: 0,
        parent: -1,
        h: 0,
        cost: 0,
    }];
    p
}

/// Planner 1 with three nodes and chain 0, 1, 2 in the smoothing.
fn smoothing_planner(s: &mut SaveGame) -> &mut crate::bstar::Planner {
    let p = fresh_planner(s);
    p.nodes = vec![p.nodes[0]; 3];
    (p.chain, p.smoothing) = (vec![0, 1, 2], true);
    p
}

/// Planner 1 belongs to a search (attempt counter via `OWNS_SEARCH`).
fn own(p: &mut crate::bstar::Planner) {
    p.attempts.insert(1, 0x3FFF_FFFF);
}

fn records(s: &mut SaveGame) -> &mut crate::collision::records::CollisionRecords {
    &mut s.state.collision_records
}

const HARD_RECORD: crate::collision::records::Record = crate::collision::records::Record {
    state: 0,
    hard: true,
    kind: 0,
};

/// A valid arrow in flight.
fn arrow() -> crate::combat::projectile::Projectile {
    use crate::combat::projectile::{Projectile, ProjectileKind};
    Projectile {
        id: 1,
        kind: ProjectileKind::Arrow,
        flags: 0x21,
        remaining_ms: 631,
        flight_ms: 731,
        shooter: ObjectId::new(1, 0, 1),
        target: ObjectId::new(2, 1, 1),
        damage: crate::message::Message::new(0x17F, &[-12, 0, 1402, 0, 0, 0, 0, 0, 0, 0, 1, 0]),
        aim: crate::fx::FxVec3::default(),
        target_pos: [512.0, 0.0, 512.0],
        start: [0.0, 0.0, 0.0],
        zone: 0,
    }
}

/// New module with index `index` on the object `id`.
fn add_module(s: &mut SaveGame, id: ObjectId, index: usize) -> &mut ModuleKind {
    let slot = &mut object(s, id).modules[index];
    *slot = Some(Module::new(index as u8));
    &mut slot.as_mut().unwrap().kind
}

fn dirty(s: &mut SaveGame, min_x: i32) {
    s.state.path.dirty = DirtyRect {
        min_x,
        min_z: 0,
        max_x: 3,
        max_z: 3,
        mask: 0x1,
    };
}

/// Rectangle corner on the right `beyond` tiles beyond the map.
fn rect_beyond(s: &SaveGame, beyond: i32) -> [i32; 4] {
    [0, 0, s.state.path.width + beyond, 3]
}

#[rustfmt::skip]
static CASES: &[Case] = &[
    // Base scenes without damage.
    Case { id: "Base Plain", base: Plain, damage: |_| {}, expect: Runs },
    Case { id: "Base Random", base: Random, damage: |_| {}, expect: Runs },
    Case { id: "Base Walking", base: Walking, damage: |_| {}, expect: Runs },
    Case { id: "Base Walls", base: Walls, damage: |_| {}, expect: Runs },
    Case { id: "Base Train", base: Train, damage: |_| {}, expect: Runs },
    Case { id: "Base Ki", base: Ki, damage: |_| {}, expect: Runs },
    Case { id: "Base Groups", base: Groups, damage: |_| {}, expect: Runs },
    // Assignment to the loaded match.
    Case { id: "B62", base: Plain, damage: |s| s.state.network = !s.state.network, expect: Rejected },
    Case { id: "B64", base: Plain, damage: |s| {
        s.state = new_match(&Rules::synthetic(TYPES, 32), &testkit::setup()).unwrap();
    }, expect: Rejected },
    Case { id: "B71", base: Plain, damage: |s| s.start.level = "data/levels/other.lvl".into(), expect: Rejected },
    Case { id: "B72", base: Plain, damage: |s| s.start.random_map = Some(random_spec("ZKA", 0)), expect: Rejected },
    Case { id: "B73", base: Random, damage: |s| s.start.setup.mode = 0, expect: Rejected },
    Case { id: "B74", base: Plain, damage: |s| s.start.rules.logic ^= 1, expect: Rejected },
    // Object tables.
    // Slot 0 is not full: the last entry is empty, the occupancy stays the same.
    Case { id: "A2", base: Plain, damage: |s| { s.state.tables[0].objects.pop(); }, expect: Rejected },
    Case { id: "A3", base: Plain, damage: |s| { s.state.tables[0].generations.pop(); }, expect: Rejected },
    Case { id: "A4", base: Plain, damage: |s| s.state.tables[0].ring = SLOT_CAPACITY[0], expect: Rejected },
    // Slot 8 fully occupied, counter one too small: the allocation would search endlessly for a free slot.
    Case { id: "A5", base: Plain, damage: |s| {
        let template = s.state.objects().next().unwrap().clone();
        let table = &mut s.state.tables[8];
        for (i, entry) in table.objects.iter_mut().enumerate() {
            table.generations[i] = 1;
            let mut o = template.clone();
            o.id = ObjectId::new(i as u16, 8, 1);
            *entry = Some(o);
        }
        table.count = SLOT_CAPACITY[8] - 1;
    }, expect: Rejected },
    Case { id: "A14 duplicate", base: Plain, damage: |s| {
        let [a, b] = [ids(s)[0], ids(s)[1]];
        s.state.extra_tick = vec![a, b, a];
    }, expect: Rejected },
    Case { id: "A14 length", base: Plain, damage: |s| {
        s.state.extra_tick = (0..SLOT_CAPACITY.iter().sum::<usize>() as u32 + 1).map(ObjectId).collect();
    }, expect: Rejected },
    // Objects: radii, module switching.
    Case { id: "A31 hard", base: Walking, damage: |s| {
        let id = walker(s);
        object(s, id).hard_radius = 1;
    }, expect: Rejected },
    Case { id: "A31 radius", base: Walking, damage: |s| {
        let id = walker(s);
        object(s, id).soft_radius = 65 << 12;
    }, expect: Rejected },
    Case { id: "A31 limit", base: Walking, damage: |s| {
        let id = walker(s);
        object(s, id).soft_radius = 64 << 12;
    }, expect: Runs },
    // Switching outside the module table: `apply_module_activation_changes` skips the slot.
    Case { id: "A34 slot 60", base: Plain, damage: |s| first(s).pending_activation.push(60), expect: Runs },
    Case { id: "A34 i32::MAX", base: Plain, damage: |s| first(s).pending_activation.push(i32::MAX), expect: Runs },
    // Grid.
    Case { id: "A83 cells", base: Plain, damage: |s| { s.state.path.layers[3].cells.pop(); }, expect: Rejected },
    // Rules and state of a map of 17 x 16 tiles fit together (grid, fog, building map, collision, map size of the
    // rules): only the sector division does not hold. No damage is needed.
    Case { id: "A83 width", base: OddMap, damage: |_| {}, expect: Rejected },
    Case { id: "A83 occupancy", base: Plain, damage: |s| s.state.occupancy.cells.push(0), expect: Rejected },
    Case { id: "A88", base: Plain, damage: |s| dirty(s, -12), expect: Rejected },
    Case { id: "A88 limit", base: Plain, damage: |s| dirty(s, -11), expect: Runs },
    // Largest width that the grid check lets through (2^31 - 16, multiple of 16), height 0 and rectangle not empty:
    // the rectangle limit `width + margin` (2^31 - 5) does not overflow, the check still computes it saturating. The
    // collision manager rejects afterwards.
    Case { id: "A88 width", base: Plain, damage: |s| {
        let p = &mut s.state.path;
        (p.width, p.height) = (i32::MAX - 15, 0);
        for l in p.layers.iter_mut() {
            (l.width, l.height, l.cells) = (p.width, 0, Vec::new());
        }
        let o = &mut s.state.occupancy;
        (o.width, o.height, o.cells) = (p.width, 0, Vec::new());
        dirty(s, 0);
        s.state.path.dirty.max_z = 0;
    }, expect: Rejected },
    Case { id: "A94", base: Plain, damage: |s| s.state.steps = u64::MAX, expect: Runs },
    Case { id: "B101", base: Plain, damage: |s| { s.state.fog.explored.cells.pop(); }, expect: Rejected },
    // Negative side with empty cells (the product of the sides clamped to 0 fits): `any_in_rect` and `stamp`
    // overflowed.
    Case { id: "B101 side", base: Plain, damage: |s| {
        let g = &mut s.state.fog.explored;
        (g.width, g.height, g.cells) = (-5, 10, Vec::new());
    }, expect: Rejected },
    Case { id: "B110", base: Plain, damage: |s| {
        let c = &mut s.state.collision;
        c.cols = 0;
        (c.all, c.movers, c.trees) = (Vec::new(), Vec::new(), Vec::new());
    }, expect: Rejected },
    Case { id: "B111", base: Plain, damage: |s| { s.state.collision.movers.pop(); }, expect: Rejected },
    // Lists and tables of the world.
    // Hidden slot 1 with an object twice in its list.
    Case { id: "B6", base: Plain, damage: |s| {
        let id = ids(s).into_iter().find(|id| id.slot() == 1).unwrap();
        s.state.slot_running[1] = false;
        s.state.inactive_lists[1] = vec![id, id];
    }, expect: Rejected },
    Case { id: "B13", base: Plain, damage: |s| s.state.attack_reports[1] = raw_map(Vec::new()), expect: Rejected },
    // Companion of B162: key 16 (bucket 1) in bucket 0. `insert` created it a second time (duplicate, with
    // `first_key`/`next_key` a cycle).
    Case { id: "B161", base: Plain, damage: |s| {
        let mut buckets = vec![Vec::new(); crate::state::ATTACK_REPORT_BUCKETS];
        buckets[0] = vec![(16, crate::state::ReportTile { x: 1, z: 1 })];
        s.state.attack_reports[1] = raw_map(buckets);
    }, expect: Rejected },
    Case { id: "B85", base: Plain, damage: |s| s.state.economy.wood_price = 0, expect: Rejected },
    Case { id: "B93", base: Plain, damage: |s| {
        s.state.projectiles.list.push(crate::combat::projectile::Projectile {
            flight_ms: i32::MAX,
            remaining_ms: -1,
            ..arrow()
        });
    }, expect: Runs },
    // Quarters with 19,301 distinct occupants, one above the limit of all object slots (the limit protected only the
    // quadratic subset check B190, which was dropped along with it).
    Case { id: "B188", base: Plain, damage: |s| {
        let id = ids(s)[0];
        if let ModuleKind::Quarter(q) = add_module(s, id, crate::module::quarter::INDEX) {
            q.occupants = (0..19_301u16).map(|i| ObjectId::new(i % 1000, 0, i / 1000 + 1)).collect();
        }
    }, expect: Runs },
    // Rows L5: large lists load from the file in seconds (no remaining check runs quadratically in their length;
    // the duplicate checks of names, occupants and influences were dropped, A14 and the MFC tables check via
    // `BTreeSet`).
    Case { id: "L5 names", base: Plain, damage: |s| {
        let mut buckets = vec![Vec::new(); crate::names::NAME_BUCKETS];
        buckets[0] = (0..L5_LEN).map(|i| (format!("n{i}"), i.to_string())).collect::<Vec<_>>();
        s.state.names = postcard::from_bytes(&postcard::to_stdvec(&buckets).unwrap()).unwrap();
    }, expect: Runs },
    Case { id: "L5 occupants", base: Plain, damage: |s| {
        let id = ids(s)[0];
        if let ModuleKind::Quarter(q) = add_module(s, id, crate::module::quarter::INDEX) {
            q.occupants = (0..L5_LEN).map(distinct_id).collect();
        }
    }, expect: Runs },
    Case { id: "L5 influences", base: Plain, damage: |s| {
        let id = ids(s)[0];
        if let ModuleKind::ExecTempUpgrade(e) = add_module(s, id, crate::module::exec_temp::INDEX) {
            let influence = |guid| crate::module::exec_temp::Influence { guid, enemy: false };
            e.influences = (0..L5_LEN as u32).map(influence).collect();
        }
    }, expect: Runs },
    Case { id: "L5 influenced", base: Plain, damage: |s| {
        let id = ids(s)[0];
        if let ModuleKind::ExecTempUpgrade(e) = add_module(s, id, crate::module::exec_temp::INDEX) {
            e.influenced = Some((0..L5_LEN).map(distinct_id).collect());
        }
    }, expect: Runs },
    // Largest allowed extra tick list (A14): all object slots, every ID once.
    Case { id: "L5 extra tick list", base: Plain, damage: |s| {
        s.state.extra_tick = (0..SLOT_CAPACITY.iter().sum::<usize>()).map(distinct_id).collect();
    }, expect: Runs },
    // Chains without a cycle.
    Case { id: "B133", base: Plain, damage: |s| {
        let [a, b] = [ids(s)[0], ids(s)[1]];
        for (from, to) in [(a, b), (b, a)] {
            let o = object(s, from);
            o.status |= status::CARRIING;
            o.target = to;
        }
    }, expect: Rejected },
    // Boat ring of two dead occupants: dying and `0xC00` also forward via dead members. Without `ISMOUNTED` the
    // rejection depends solely on the boat bit of the rider chain.
    Case { id: "B229", base: Plain, damage: |s| {
        let [a, b] = [ids(s)[0], ids(s)[1]];
        for (from, to) in [(a, b), (b, a)] {
            let o = object(s, from);
            o.status |= status::ISMOUNTEDONBOAT | status::DIED;
            o.target = to;
        }
    }, expect: Rejected },
    // Two living riders (with a riding module) refer to each other, neither carries `CARRIING`.
    Case { id: "B231", base: Plain, damage: |s| {
        let [a, b] = [ids(s)[0], ids(s)[2]];
        for (from, to) in [(a, b), (b, a)] {
            add_module(s, from, crate::module::riding::RIDING);
            let o = object(s, from);
            o.status |= status::ISMOUNTED;
            o.target = to;
        }
    }, expect: Rejected },
    // Rider ring via a dead member: O lives, carries T (`CARRIING`) and rides on T; T is dead and rides on O. The
    // carrier chain O -> T ends, but a `0xC00` was forwarded back and forth endlessly via the rider bits.
    Case { id: "L3", base: Plain, damage: |s| {
        let [a, b] = [ids(s)[0], ids(s)[2]];
        let o = object(s, a);
        o.status |= status::ISMOUNTED | status::CARRIING;
        o.target = b;
        let t = object(s, b);
        t.status |= status::ISMOUNTED | status::DIED;
        t.target = a;
    }, expect: Rejected },
    // Groups: the rear group of the marching line itself gets a formation that points back to the main group
    // (two-cycle; B54 was absorbed by B55, B55 rejects).
    Case { id: "B54", base: Groups, damage: |s| {
        let (main, rear) = formation_with_rear(&s.state).unwrap();
        let r = s.state.groups.map.get_mut(key(rear)).unwrap();
        (r.kind, r.formation) = (1, line_with_rear(main));
    }, expect: Rejected },
    // Formation spacing `i32::MIN` (row L2): second factor in `fixed_mul` (march, geometry), which ends with 0 for it
    // (K-324). The row shows only "loads and runs"; the four ticks do not reach the factor with a negative first
    // factor, the hang is evidenced by `fx::tests::fixed_mul_with_int_min_ends_with_zero`.
    Case { id: "L2", base: Groups, damage: |s| {
        let (main, _) = formation_with_rear(&s.state).unwrap();
        let g = s.state.groups.map.get_mut(key(main)).unwrap();
        g.formation.as_mut().unwrap().spacing = i32::MIN;
    }, expect: Runs },
    Case { id: "B55", base: Groups, damage: |s| {
        let (main, rear) = formation_with_rear(&s.state).unwrap();
        let third = s.state.groups.next_id;
        s.state.groups.next_id += 1;
        let r = s.state.groups.map.get_mut(key(rear)).unwrap();
        (r.kind, r.formation) = (1, line_with_rear(third));
        let mut c = crate::group::Group::new(third, 1, (0, 0, crate::motion::NO_DIRECTION), false);
        c.formation = line_with_rear(main);
        s.state.groups.map.set(key(third), c);
    }, expect: Rejected },
    // Collision records (walking scene).
    Case { id: "B113", base: Walking, damage: |s| {
        let r = records(s);
        (r.hard, r.soft) = (vec![(1, 2)], vec![(1, 2)]);
        r.records = [((1, 2), HARD_RECORD), ((3, 4), HARD_RECORD)].into_iter().collect();
    }, expect: Rejected },
    Case { id: "B115", base: Walking, damage: |s| {
        let r = records(s);
        (r.hard, r.soft) = (vec![(1, 2)], Vec::new());
        r.records = [((3, 4), HARD_RECORD)].into_iter().collect();
    }, expect: Rejected },
    // B* planner (walking scene, planner 1 set up anew).
    Case { id: "B118", base: Walking, damage: |s| fresh_planner(s).open.push_back(1), expect: Rejected },
    Case { id: "B119", base: Walking, damage: |s| fresh_planner(s).chain = vec![0, 1], expect: Rejected },
    Case { id: "B121", base: Walking, damage: |s| fresh_planner(s).nodes[0].parent = 0, expect: Rejected },
    Case { id: "B122", base: Walking, damage: |s| fresh_planner(s).best = 1, expect: Rejected },
    // With step size 1, so that only "owner => node present" is violated (not B124 as well).
    Case { id: "B123", base: Walking, damage: |s| {
        let p = fresh_planner(s);
        (p.nodes, p.speed.cur) = (Vec::new(), 1);
        own(p);
    }, expect: Rejected },
    Case { id: "B124", base: Walking, damage: |s| {
        let p = fresh_planner(s);
        (p.speed.cur, p.smoothing) = (0, false);
        own(p);
    }, expect: Rejected },
    Case { id: "B125", base: Walking, damage: |s| fresh_planner(s).req.mult = 0, expect: Rejected },
    Case { id: "B128", base: Walking, damage: |s| fresh_planner(s).speed.turn = 0, expect: Rejected },
    Case { id: "B130", base: Walking, damage: |s| {
        let p = smoothing_planner(s);
        (p.from, p.to) = (0, -1);
    }, expect: Rejected },
    Case { id: "B131", base: Walking, damage: |s| {
        let p = smoothing_planner(s);
        (p.from, p.to) = (3, 0);
    }, expect: Rejected },
    Case { id: "B132", base: Walking, damage: |s| {
        let p = smoothing_planner(s);
        (p.from, p.to) = (1, 2);
    }, expect: Rejected },
    // Walk module.
    Case { id: "B166", base: Walking, damage: |s| walk(s).layer = 9, expect: Rejected },
    Case { id: "B171", base: Walking, damage: |s| walk(s).request.mult = 0, expect: Rejected },
    Case { id: "B173", base: Walking, damage: |s| walk(s).speed.cur = 0x801, expect: Rejected },
    Case { id: "B174", base: Walking, damage: |s| walk(s).speed.turn = 0, expect: Rejected },
    Case { id: "B176", base: Walking, damage: |s| {
        let w = walk(s);
        (w.state, w.speed.cur) = (4, 0);
    }, expect: Rejected },
    Case { id: "B177", base: Walking, damage: |s| {
        walk(s).hard_pairs = crate::module::walk::PairMap { buckets: vec![Vec::new(); 5] };
    }, expect: Rejected },
    Case { id: "B180", base: Walking, damage: tether_to_quarry, expect: Runs },
    Case { id: "L4", base: Walking, damage: escort_far_away, expect: Runs },
    // E-118: like B180, but the home lies exactly 2^22 (1024 tiles) in +x next to the target. The tether check measures
    // the target <-> home distance with `fixed_hypot`, whose divisor becomes 0 there; it ends with 0.
    Case { id: "E-118", base: Walking, damage: |s| {
        tether_to_quarry(s);
        let [me, quarry] = walkers(s);
        let q = object(s, quarry).grid;
        let w = walk_of(s, me);
        (w.home.0, w.home.1) = (q.x.0.wrapping_add(1 << 22), q.z.0);
    }, expect: Runs },
    // Collision: both walkers without a soft radius, the second stands in the bucket of its old position, but its
    // position lies 600 tiles further in +x. The cost queries of the first measure the distance with `fixed_hypot`,
    // which becomes negative there; the soft overlap then divided by the sum of the soft radii 0.
    Case { id: "Collision", base: Walking, damage: |s| {
        let [me, other] = walkers(s);
        object(s, me).soft_radius = 0;
        let o = object(s, other);
        o.soft_radius = 0;
        o.grid.x.0 = o.grid.x.0.wrapping_add(600 << 12);
    }, expect: Runs },
    // Planner: the first walker plans (state 4) and begins a new search (last result 8) in phase 2 (coarse) in mode
    // 3. This mode takes over the multiplier of the stored request, 2^21; with the step size 0x800 `mult · cur` runs to
    // 0 in the planner (base speed 1, so that `set_cur_speed(0)` applies). The search then divided by the step size 0
    // on setup and in `add_node`.
    Case { id: "Planner", base: Walking, damage: |s| {
        let [me, _] = walkers(s);
        let w = walk_of(s, me);
        (w.phase, w.last_bstar_result) = (2, 8);
        (w.request.mode, w.request.mult) = (3, 1 << 21);
        (w.speed.cur, w.speed.default) = (0x800, 1);
    }, expect: Runs },
    // Buildings (A78 = B209).
    Case { id: "B201", base: Walls, damage: |s| master(s).build_time = 0, expect: Rejected },
    Case { id: "B203", base: Walls, damage: |s| master(s).request_pos = -2, expect: Rejected },
    Case { id: "B204", base: Walls, damage: |s| master(s).request_pos = i32::MAX, expect: Rejected },
    Case { id: "B209", base: Walls, damage: |s| master(s).rect = rect_beyond(s, MASK_SIDE + 1), expect: Rejected },
    Case { id: "B209 limit", base: Walls, damage: |s| master(s).rect = rect_beyond(s, MASK_SIDE), expect: Runs },
    Case { id: "B210", base: Walls, damage: |s| {
        master(s).occupancy_rect = rect_beyond(s, MASK_SIDE + 1);
    }, expect: Rejected },
    // Wall group.
    // Cannot be triggered individually: a building that names itself master always violates B213, B214 or B215 too.
    Case { id: "B211", base: Walls, damage: |s| {
        let [a, _, _] = walls(s);
        building_of(s, a).master = a;
    }, expect: Rejected },
    Case { id: "B213", base: Walls, damage: |s| {
        let [_, b, c] = walls(s);
        building_of(s, b).slaves = vec![c];
    }, expect: Rejected },
    Case { id: "B214", base: Walls, damage: |s| {
        let [a, b, _] = walls(s);
        building_of(s, a).slaves.clear();
        building_of(s, a).master = b;
        building_of(s, b).master = a;
    }, expect: Rejected },
    Case { id: "B215", base: Walls, damage: |s| {
        let [a, b, _] = walls(s);
        building_of(s, b).master = ObjectId(0);
        building_of(s, b).slaves = vec![a];
    }, expect: Rejected },
    Case { id: "B216", base: Walls, damage: |s| {
        let [_, b, _] = walls(s);
        object(s, b).status |= status::DIED;
    }, expect: Rejected },
    // Train and multipart objects.
    // Acceleration time `i32::MIN` when accelerating or braking: `drive.rs` computes the times wrapped.
    Case { id: "B276", base: Train, damage: |s| {
        let t = moving_loco(s);
        (t.status, t.acc_time) = (t.status | crate::module::train::status::ACCEL, i32::MIN);
    }, expect: Runs },
    Case { id: "B276 braking", base: Train, damage: |s| {
        let t = moving_loco(s);
        (t.status, t.acc_time) = (t.status | crate::module::train::status::BRAKING, i32::MIN);
    }, expect: Runs },
    // Speed and drawbar `i32::MIN` enter as the second factor in `fixed_mul`, which ends with 0 for it (K-324). Like L2
    // only "loads and runs": in the four ticks no negative first factor meets the value.
    Case { id: "B279", base: Train, damage: |s| moving_loco(s).speed = i32::MIN, expect: Runs },
    Case { id: "B280", base: Train, damage: |s| moving_loco(s).next_speed = i32::MIN, expect: Runs },
    Case { id: "B282", base: Train, damage: |s| loco(s).raster_dir = (2, 0), expect: Rejected },
    Case { id: "B285", base: Train, damage: |s| cart_tail(s).offset = i32::MIN, expect: Runs },
    // Values the original tolerates; loading accepts them (no exception A7).
    // Attack module with aggressiveness 7 and negative stubbornness (E-101).
    Case { id: "E-101", base: Walking, damage: |s| {
        let id = walker(s);
        if let ModuleKind::Attack(a) = add_module(s, id, 17) {
            a.aggressiveness = 7;
            a.stubbornness = [-1; 3];
        }
    }, expect: Runs },
    // Time of day 1000 (E-199).
    Case { id: "E-199", base: Plain, damage: |s| s.state.daytime = 1000, expect: Runs },
    // Headhunter with target player 9 and customer 8 (E-218).
    Case { id: "E-218", base: Plain, damage: |s| {
        let id = ids(s)[0];
        if let ModuleKind::HeadHunter(h) = add_module(s, id, 0x22) {
            h.target_player = 9;
            h.customer = 8;
        }
    }, expect: Runs },
    // AI.
    Case { id: "B162/B308", base: Ki, damage: |s| {
        let t0 = ki_thread(&mut s.state, 0).clone();
        let mut buckets: Vec<Vec<(i32, crate::ki::vm::Thread)>> = vec![Vec::new(); 17];
        buckets[0] = vec![(0, t0.clone()), (0, t0)];
        ki_engine(&mut s.state).threads = raw_map(buckets);
    }, expect: Rejected },
    // Companion of B308: thread 1 (bucket 0) in bucket 1. `insert` created it a second time, `run_slice_steps` would
    // then run endlessly (the bucket check of the thread table stays).
    Case { id: "B307", base: Ki, damage: |s| {
        let t0 = ki_thread(&mut s.state, 0).clone();
        let t1 = ki_thread(&mut s.state, 1).clone();
        let mut buckets: Vec<Vec<(i32, crate::ki::vm::Thread)>> = vec![Vec::new(); 17];
        (buckets[0], buckets[1]) = (vec![(0, t0)], vec![(1, t1)]);
        ki_engine(&mut s.state).threads = raw_map(buckets);
    }, expect: Rejected },
    Case { id: "B306", base: Ki, damage: |s| ki_engine(&mut s.state).threads = raw_map(Vec::new()), expect: Rejected },
    Case { id: "B319", base: Ki, damage: |s| {
        ki_thread(&mut s.state, 1).vars.ranges.get_mut(&20).unwrap().lists[1] = raw_map(Vec::new());
    }, expect: Rejected },
    Case { id: "B316", base: Ki, damage: |s| {
        ki_thread(&mut s.state, 1).stack = vec![0; crate::ki::MAX_STACK + 1];
    }, expect: Rejected },
];

type ByteCase = (&'static str, fn(&mut Vec<u8>));

/// Damage to the bytes of a savegame of the base scene [`Base::Plain`]; loading path only `SaveGame::from_bytes`,
/// expected "rejected".
#[rustfmt::skip]
static BYTE_CASES: &[ByteCase] = &[
    ("B75", |b| b.truncate(11)),
    ("B77", |b| b.truncate(19)),
    ("B76", |b| b[8..12].copy_from_slice(&28u32.to_le_bytes())),
    ("B78", |b| *b.last_mut().unwrap() ^= 0x55),
    // Replaces the bytes with those of the AI scene with 100,000 nested children of a thread: the value itself could
    // not be built (serializing, cloning and freeing run recursively), the read limit of `deserialize_child` must
    // reject before the stack overflow.
    ("vm-depth", |b| *b = nested_ki_children(100_000)),
    // A zstd frame that announces one byte more than `codec::MAX_UNPACKED`: rejected before unpacking.
    ("unpacked size", |b| *b = signed(&announcing_frame(codec::MAX_UNPACKED as u64 + 1))),
    // An unknown field whose value nests 100,000 arrays deep: skipping it recurses, the depth limit
    // `codec::MAX_DEPTH` must reject before the stack overflow.
    ("nesting depth", |b| *b = signed(&pack(&unknown_deep_field(100_000)))),
    // An object table of 2,000,000 empty entries in about a hundred packed bytes: one byte each in MessagePack, an
    // object each in memory (gigabytes); reading must reject at the entry behind the largest slot capacity.
    ("object table length", |b| *b = signed(&pack(&empty_object_table(2_000_000)))),
];

/// Savegame bytes with a valid header around the packed body `packed`.
fn signed(packed: &[u8]) -> Vec<u8> {
    write_container(SAVE_MAGIC, FORMAT_VERSION, packed)
}

/// Packs MessagePack like `codec::encode_named`.
fn pack(plain: &[u8]) -> Vec<u8> {
    zstd::bulk::compress(plain, 3).unwrap()
}

/// zstd frame header with the content size `size` (single segment, without checksum), then one raw last block with
/// one byte.
fn announcing_frame(size: u64) -> Vec<u8> {
    let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0xE0];
    frame.extend(size.to_le_bytes());
    frame.extend([0x09, 0x00, 0x00, 0x2A]);
    frame
}

/// MessagePack of the savegame of the base scene [`Base::Plain`] with an additional field `x`: `depth` nested arrays
/// around `nil`.
fn unknown_deep_field(depth: usize) -> Vec<u8> {
    let (_, save) = base(Plain);
    let mut plain = rmp_serde::to_vec_named(&save).unwrap();
    assert_eq!(plain[0], 0x83, "map with the three fields of `SaveGame`");
    plain[0] = 0x84;
    plain.extend([0xA1, b'x']);
    plain.extend(std::iter::repeat_n(0x91, depth));
    plain.push(0xC0);
    plain
}

/// MessagePack of a savegame body that is only an object table with `len` empty entries:
/// `{state: {tables: [{objects: [nil; len]}]}}`.
fn empty_object_table(len: u32) -> Vec<u8> {
    let mut plain = vec![0x81, 0xA5];
    plain.extend(b"state");
    plain.extend([0x81, 0xA6]);
    plain.extend(b"tables");
    plain.extend([0x91, 0x81, 0xA7]);
    plain.extend(b"objects");
    plain.push(0xDD);
    plain.extend(len.to_be_bytes());
    plain.extend(std::iter::repeat_n(0xC0, len as usize));
    plain
}

/// Savegame bytes of the AI scene in which thread 1 carries a chain of `depth` children (newly signed).
fn nested_ki_children(depth: usize) -> Vec<u8> {
    let (rules, state) = ki_scene();
    let mut thread = ki_thread(&mut state.clone(), 1).clone();
    thread.child = None;
    // `child` is the last field: without its `nil` the bytes of a thread open the value of its child.
    let mut one = rmp_serde::to_vec_named(&thread).unwrap();
    assert_eq!(one.pop(), Some(0xC0), "`child: None` at the end");
    let body = rmp_serde::to_vec_named(&wrap(&rules, state)).unwrap();
    let at = body
        .windows(one.len())
        .position(|w| w == one.as_slice())
        .expect("thread 1 in the body");
    let end = at + one.len();
    assert_eq!(body[end], 0xC0, "`child: None` of thread 1");
    // `child: None` behind thread 1 becomes the chain.
    let mut damaged = body[..end].to_vec();
    for _ in 0..depth {
        damaged.extend(&one);
    }
    damaged.push(0xC0);
    damaged.extend(&body[end + 1..]);
    signed(&pack(&damaged))
}

type ReplayCase = (&'static str, fn(&mut Replay), Option<u64>);

/// Replays: damage to a valid replay and the target tick of the playback (`None`: the end without a specification,
/// `Replay::implied_end`); loading path like the host, then `Replay::play_observed`. Expected "rejected".
#[rustfmt::skip]
static REPLAY_CASES: &[ReplayCase] = &[
    ("B65", |r| r.commands.push(RecordedCommand { step: u64::MAX, command: diplomacy() }), None),
    ("B67", |_| {}, Some(MAX_REPLAY_STEPS + 1)),
];

fn diplomacy() -> Command {
    Command {
        player: 0,
        kind: CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 0,
        },
    }
}

/// One command at tick 3, so the end without a specification is tick 4.
fn replay() -> (Rules, Replay) {
    let rules = Rules::synthetic(TYPES, 16);
    let replay = Replay {
        start: start_package(&rules, &testkit::setup()),
        commands: vec![RecordedCommand {
            step: 3,
            command: diplomacy(),
        }],
    };
    (rules, replay)
}

/// Loading path of a row; `Err` with the reason if the outcome is not the expected one.
fn run_case(case: &Case) -> Result<(), String> {
    let (rules, mut save) = base(case.base);
    (case.damage)(&mut save);
    let loaded = SaveGame::from_bytes(&encode(SAVE_MAGIC, &save)).and_then(|save| {
        save.check(&rules, crate::save::RulesCompare::Full)
            .map(|()| save)
    });
    match (case.expect, loaded) {
        (Rejected, Err(e)) => {
            println!("{}: rejected: {e:#}", case.id);
            Ok(())
        }
        (Rejected, Ok(_)) => Err("loads, expected a rejection".into()),
        (Runs, Err(e)) => Err(format!("rejected, expected to run: {e:#}")),
        (Runs, Ok(save)) => {
            let mut state = save.state;
            for _ in 0..RUN_TICKS {
                let _ = crate::tick::step(&mut state, &rules, &[]);
            }
            Ok(())
        }
    }
}

fn run_byte_case(id: &str, damage: fn(&mut Vec<u8>)) -> Result<(), String> {
    let (_, save) = base(Plain);
    let mut bytes = encode(SAVE_MAGIC, &save);
    damage(&mut bytes);
    match SaveGame::from_bytes(&bytes) {
        Ok(_) => Err("loads, expected a rejection".into()),
        Err(e) => {
            println!("{id}: rejected: {e:#}");
            Ok(())
        }
    }
}

fn run_replay_case(id: &str, damage: fn(&mut Replay), steps: Option<u64>) -> Result<(), String> {
    let (rules, mut replay) = replay();
    damage(&mut replay);
    let played = match steps {
        Some(steps) => Ok(steps),
        None => replay.implied_end(),
    }
    .and_then(|steps| replay.play_observed(&rules, steps, &mut |_| {}));
    match played {
        Ok(_) => Err("runs, expected rejected".into()),
        Err(e) => {
            println!("{id}: rejected: {e:#}");
            Ok(())
        }
    }
}

/// Runs `row` in its own thread under `catch_unwind` with [`ROW_TIMEOUT`]; `Some` with the reason on failure (wrong
/// outcome, panic, timeout).
fn guarded(id: &str, row: impl FnOnce() -> Result<(), String> + Send + 'static) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(id.to_string())
        .stack_size(ROW_STACK)
        .spawn(move || {
            let _ = tx.send(catch_unwind(AssertUnwindSafe(row)));
        })
        .expect("starting thread");
    let why = match rx.recv_timeout(ROW_TIMEOUT) {
        Ok(Ok(Ok(()))) => return None,
        Ok(Ok(Err(why))) => why,
        Ok(Err(panic)) => {
            let text = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            format!("panic: {text}")
        }
        Err(_) => format!("not finished after {ROW_TIMEOUT:?} (hang)"),
    };
    Some(format!("{id}: {why}"))
}

fn assert_all(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} rows failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn damaged_saves_are_rejected_or_run_without_panic() {
    let failures = CASES
        .iter()
        .filter_map(|case| guarded(case.id, move || run_case(case)))
        .collect();
    assert_all(failures);
}

#[test]
fn damaged_save_bytes_are_rejected() {
    let failures = BYTE_CASES
        .iter()
        .filter_map(|&(id, damage)| guarded(id, move || run_byte_case(id, damage)))
        .collect();
    assert_all(failures);
}

#[test]
fn damaged_replays_are_rejected() {
    let failures = REPLAY_CASES
        .iter()
        .filter_map(|&(id, damage, steps)| guarded(id, move || run_replay_case(id, damage, steps)))
        .collect();
    assert_all(failures);
}
