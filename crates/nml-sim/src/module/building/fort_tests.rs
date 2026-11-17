//! Scene with `FORT_GOLDEN` (modeled on `tent_run_probed`; re/arming-gates 9.C and 9.D): slot 0
//! builds a ring of 6 x 6 with three colonists via command `0x18` and then places a gate on the
//! south wall (gate init: four wall pieces removed, gate becomes a member of the group), closes and
//! opens it. Three units of slot 1 walk through the gate into the fort: the enemy watch sets the
//! group to slot 8, the capture watch sees two owners (house of slot 0 and the units). Slot 0
//! sells the house, slot 1 captures and gets the enemy watch; a colonist of slot 0 walks into the
//! fort and makes the group neutral again. A real level start without original data; saving/loading
//! in every section.

use std::sync::LazyLock;

use nml_formats::config::LevelPlayer;

use super::fort_testkit::{self as kit, *};
use super::gate::{SFX_DOOR, door_tiles_of};
use super::{Building, INDEX, flags, msg};
use crate::area::{CAPTURE_WATCH, ENEMY_WATCH};
use crate::command::{Command, CommandKind, F32Bits, NO_SPREAD};
use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::module::ModuleKind;
use crate::module::base::event_flags::{TIMER, WATCHER};
use crate::path::grid::COST_FREE;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::step;

/// Tile of the master corner (part 0 of the ring, `fort_ring((20, 20), 4, 4)`).
const RING_BASE: (i32, i32) = (20, 20);
/// Hull rectangle of the ring (tiles 20 ... 25, re/arming-gates 9.D).
const FORT_RECT: [i32; 4] = [20, 20, 25, 25];
/// Cursor position of the gate on the south wall: x on the tile boundary 23, z in the middle of row 25
/// (re/arming-gates 9.C); the check snaps onto the wall piece.
const GATE_CURSOR: [f32; 3] = [23.0 * 128.0, 0.0, 25.5 * 128.0];
/// Rectangle and door tiles of the gate.
const GATE_RECT: [i32; 4] = [21, 25, 24, 25];
const DOORS: [(i32, i32); 2] = [(22, 25), (23, 25)];
/// Parts of the ring with gate: 4 corners + 16 - 4 straight pieces + gate.
const PARTS_WITH_GATE: usize = 17;

/// Setup: slot 0 HQ, three colonists west of the ring, house (one tile) in the fort;
/// slot 1 three combat units southeast.
const HQ_TILE: (i32, i32) = (8, 30);
const COLONIST_TILES: [(i32, i32); 3] = [(18, 22), (18, 23), (18, 24)];
const HOUSE_TILE: (i32, i32) = (22, 22);
const SOLDIER_TILES: [(i32, i32); 3] = [(36, 40), (37, 40), (38, 40)];
/// Targets: the colonists leave the fort after the gate construction (they build the gate from
/// inside), the units walk in, the third colonist last.
const LEAVE_TILES: [(i32, i32); 3] = [(19, 29), (21, 29), (23, 29)];
const SOLDIER_TARGETS: [(i32, i32); 3] = [(21, 23), (23, 23), (24, 21)];
const COLONIST_TARGET: (i32, i32) = (22, 23);

/// Command times as `step` calls (not object ticks: two `step`s are one tick); the command runs in
/// the `step` call with this index.
///
/// Ring `0x18` with all three colonists.
const RING_AT: usize = 0;
/// Gate `0x01` on the south wall, in the `step` after the completion of the ring ([`RING_FINISHED`]).
const GATE_AT: usize = 850;
/// Colonists out of the fort, in the `step` after the completion of the gate ([`GATE_FINISHED`]).
const LEAVE_AT: usize = 1039;
/// `0x1A` close gate, in the `step` after leaving ([`COLONISTS_OUT`]).
const CLOSE_AT: usize = 1081;
/// `0x19` open gate, 40 `step`s later.
const OPEN_AT: usize = CLOSE_AT + 40;
/// Slot 1: the three units into the fort, one object tick after opening.
const ENTER_AT: usize = OPEN_AT + 2;
/// Slot 0 sells the house (`0x67`), after the first unsuccessful capture check
/// ([`FIRST_CAPTURE_CHECK`]: two owners). Sale instead of attack: the units of the test world have
/// no combat module, and the sale is a command without pathfinding. The dead house stays listed
/// (K-296) until it is destroyed after the collapse ([`HOUSE_GONE`]); only then does slot 1 capture.
const SELL_AT: usize = 1540;
const _: () = assert!(SELL_AT > FIRST_CAPTURE_CHECK && SELL_AT < HOUSE_GONE);
/// Slot 0: the third colonist into the fort, after the enemy watch of the new owner
/// ([`ENEMY_WATCH_1`]).
const COLONIST_IN_AT: usize = 1670;
/// Length of the scene in `step` calls.
const FORT_STEPS: usize = 4000;

/// Measured `step` calls (debug = release) after which the state first holds.
///
/// Last part of the ring finished; in the same `step` another colonist sends its build step to a
/// finished part, which registers the enemy watch via the master (way b, W-271; with three colonists
/// without help).
const RING_FINISHED: usize = 849;
const WATCHED: usize = 849;
/// Gate finished (`TOROPEN` from the flag backup, re/arming-gates 3.C2).
const GATE_FINISHED: usize = 1038;
/// All colonists south of the wall (row >= 26).
const COLONISTS_OUT: usize = 1080;
/// The first unit of slot 1 in the area (tracking half-open, K-295); in the next `step` the watcher
/// sends `0x2703` (query away), in the one after that the group is at slot 8
/// (`process_owner_changes` in phase 0).
const ENEMY_LISTED: usize = 1487;
const ABANDONED: usize = 1488;
const NEUTRAL: usize = 1489;
/// Capture query (`0x2A03`, 22 `step`s after `0x2703`): house and three units.
const CAPTURE_REGISTERED: usize = 1510;
/// Capture checks `0x2903` at intervals of 22 `step`s as long as two owners are listed.
const FIRST_CAPTURE_CHECK: usize = CAPTURE_REGISTERED + 22;
/// The sold house is destroyed after the collapse and leaves the list.
const HOUSE_GONE: usize = 1633;
/// The next check finds only slot 1: `CaptureByPlayer(1)` unregisters the query, in the next `step` the group
/// belongs to slot 1.
const CAPTURE_CHECKED: usize = 1642;
const CAPTURED: usize = 1643;
/// Enemy watch for slot 1 (`0x2B03`, 22 `step`s after `CaptureByPlayer`), list empty.
const ENEMY_WATCH_1: usize = 1664;
/// The colonist in the area, `0x2703`, slot 8, capture query with colonist and units.
const COLONIST_LISTED: usize = 1733;
const ABANDONED_AGAIN: usize = 1734;
const NEUTRAL_AGAIN: usize = 1735;
const CAPTURE_REGISTERED_AGAIN: usize = 1756;

/// IDs of the scene (ring and gate change their ID on a change of owner and are looked up by type
/// and tile).
struct FortScene {
    hq: ObjectId,
    colonists: [ObjectId; 3],
    soldiers: [ObjectId; 3],
    house: ObjectId,
}

/// Rules of the scene: the test world of the gates ([`kit::rules`]) with two human players in the
/// `.lvl` (slot 0 and 1, nation SPA = 2 like wall, gate and colonist), 1000 gold, wood, food each.
fn scene_rules() -> Rules {
    let mut r = kit::rules();
    for slot in [0, 1] {
        r.level.players.push(LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation: 2,
            kind: 0,
            resources: vec![(0, 1000), (1, 1000), (2, 1000)],
        });
    }
    r
}

/// Scene via a real level start (`setup::new_match`, mode -1), map revealed; slots 0 and 1 are
/// enemies of each other (default of diplomacy).
fn fort_scene() -> (SimState, Rules, FortScene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    assert!(state.diplomacy.is_enemy(0, 1) && state.diplomacy.is_enemy(1, 0));
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, HQ_TILE);
        let colonists = COLONIST_TILES.map(|t| spawn(&mut sim, COLONIST, 0, t));
        let house = spawn(&mut sim, HOUSE, 0, HOUSE_TILE);
        let soldiers = SOLDIER_TILES.map(|t| spawn(&mut sim, SOLDIER, 1, t));
        FortScene {
            hq,
            colonists,
            soldiers,
            house,
        }
    };
    (state, rules, scene)
}

fn of_guid(state: &SimState, guids: &[u32]) -> Vec<ObjectId> {
    state
        .objects()
        .filter(|o| guids.contains(&o.guid))
        .map(|o| o.id)
        .collect()
}

/// The gate (there is at most one).
fn gate(state: &SimState) -> Option<ObjectId> {
    of_guid(state, &[GATE]).first().copied()
}

/// The master (corner at [`RING_BASE`]) under its current ID.
fn master(state: &SimState) -> Option<ObjectId> {
    of_guid(state, &[CORNER])
        .into_iter()
        .find(|&c| state.object(c, false).is_some_and(|o| o.tile == RING_BASE))
}

/// Parts of the group (corners, wall pieces, gate), without those marked for destruction.
fn parts(state: &SimState) -> Vec<ObjectId> {
    of_guid(state, &[CORNER, WALL, GATE])
        .into_iter()
        .filter(|id| !state.destroy_queue.contains(id))
        .collect()
}

fn building_of(state: &SimState, id: ObjectId) -> Building {
    match &state
        .object(id, false)
        .expect("building")
        .module(INDEX)
        .expect("module 3")
        .kind
    {
        ModuleKind::Building(b) => (**b).clone(),
        k => panic!("not a building: {k:?}"),
    }
}

/// Flag word of the building module: is one of the bits set (without low byte)?
fn has(state: &SimState, id: ObjectId, bits: u32) -> bool {
    state
        .object(id, false)
        .and_then(|o| o.module(INDEX))
        .is_some_and(|m| m.base.has_flags(bits))
}

/// Watchers of the building module.
fn watchers(state: &SimState, id: ObjectId) -> usize {
    state
        .object(id, false)
        .and_then(|o| o.module(INDEX))
        .map_or(0, |m| {
            m.base
                .events
                .iter()
                .filter(|e| e.flags & WATCHER != 0)
                .count()
        })
}

/// The only query of the world: (mode, player, list); `None` without a query.
fn watch(state: &SimState) -> Option<(u32, i32, Vec<ObjectId>)> {
    let qs = &state.area_queries.queries;
    assert!(qs.len() <= 1, "at most one query: {qs:?}");
    qs.values()
        .next()
        .map(|q| (q.mode, q.player, q.list.clone()))
}

/// Door tile blocked in all nine layers?
fn blocked_in_all_layers(state: &SimState, (x, z): (i32, i32)) -> bool {
    (0..9).all(|l| state.path.cell_cost(x, z, l) != COST_FREE)
}

/// Door tile free in land layers 0 to 3?
fn free_in_base_layers(state: &SimState, (x, z): (i32, i32)) -> bool {
    (0..4).all(|l| state.path.cell_cost(x, z, l) == COST_FREE)
}

fn move_to(units: Vec<ObjectId>, t: (i32, i32)) -> CommandKind {
    CommandKind::Move {
        units,
        x: (t.0 << 12) + 0x800,
        z: (t.1 << 12) + 0x800,
        target: -1,
        formation: 0,
        direction: crate::motion::NO_DIRECTION,
        spread: NO_SPREAD,
        flags: 0,
    }
}

/// Commands in the `step` call `i`. The gate uses the position and rotation that the check snaps from
/// the cursor position (exception A8; computed on a copy of the state, the check gets `&mut
/// Sim`).
fn fort_orders(i: usize, s: &FortScene, state: &SimState, rules: &Rules) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    match i {
        RING_AT => vec![cmd(
            0,
            CommandKind::PlaceBuildings {
                parts: fort_ring(RING_BASE, 4, 4),
                workers: s.colonists.to_vec(),
            },
        )],
        GATE_AT => {
            let mut copy = state.clone();
            let mut sim = Sim::new(&mut copy, rules);
            let (pos, rot) = crate::placement::snapped_command_pose(
                &mut sim,
                GATE,
                GATE_CURSOR,
                0.0,
                0,
                Some(s.colonists[0]),
            )
            .expect("gate can be placed on the wall");
            vec![cmd(
                0,
                CommandKind::PlaceBuilding {
                    guid: GATE,
                    rotation: F32Bits::new(rot),
                    x: F32Bits::new(pos[0]),
                    y: F32Bits::new(pos[1]),
                    z: F32Bits::new(pos[2]),
                    workers: s.colonists.to_vec(),
                    border: None,
                },
            )]
        }
        LEAVE_AT => s
            .colonists
            .iter()
            .zip(LEAVE_TILES)
            .map(|(&c, t)| cmd(0, move_to(vec![c], t)))
            .collect(),
        CLOSE_AT => vec![cmd(
            0,
            CommandKind::CloseGate {
                gate: gate(state).expect("gate"),
            },
        )],
        OPEN_AT => vec![cmd(
            0,
            CommandKind::OpenGate {
                gate: gate(state).expect("gate"),
            },
        )],
        ENTER_AT => s
            .soldiers
            .iter()
            .zip(SOLDIER_TARGETS)
            .map(|(&u, t)| cmd(1, move_to(vec![u], t)))
            .collect(),
        SELL_AT => vec![cmd(
            0,
            CommandKind::SellBuildings {
                buildings: vec![s.house],
            },
        )],
        COLONIST_IN_AT => vec![cmd(0, move_to(vec![s.colonists[2]], COLONIST_TARGET))],
        _ => vec![],
    }
}

/// State assertion at the time of saving: state, rules, IDs of the scene.
type FortProbe = fn(&mut SimState, &Rules, &FortScene);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` it saves after the `step` call `i`,
/// checks against the rules (like the viewer on loading) and reloads; `at_save` sees the state
/// immediately before.
fn fort_run_probed(save_at: Option<usize>, at_save: FortProbe) -> Vec<u64> {
    let (mut state, rules, scene) = fort_scene();
    let mut out = Vec::with_capacity(FORT_STEPS);
    for i in 0..FORT_STEPS {
        let orders = fort_orders(i, &scene, &state, &rules);
        step(&mut state, &rules, &orders);
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene);
            assert_loads(&state, &rules);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

/// Read a savegame and check it against the rules (like the viewer on loading; copy from
/// `tent::tests`, private there).
fn assert_loads(state: &SimState, rules: &Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules)
        .expect("savegame valid");
}

/// Elapsed time of the timer with the message `code` on the building (`events`, otherwise one just
/// set in `timers_ws` with 0).
fn timer_elapsed(state: &SimState, id: ObjectId, code: u32) -> Option<i32> {
    let m = state.object(id, false)?.module(INDEX)?;
    m.base
        .events
        .iter()
        .chain(m.base.timers_ws.iter())
        .find(|e| e.flags & TIMER != 0 && e.msg.code == code)
        .map(|e| e.a)
}

/// Expected owner of all parts of the group after the `step` call `i`.
fn expected_owner(i: usize) -> u8 {
    match i {
        _ if i < NEUTRAL => 0,
        _ if i < CAPTURED => NEUTRAL_SLOT,
        _ if i < NEUTRAL_AGAIN => 1,
        _ => NEUTRAL_SLOT,
    }
}

/// Expected watch (mode, player) after the `step` call `i`.
fn expected_watch(i: usize) -> Option<(u32, i32)> {
    let neutral = i32::from(NEUTRAL_SLOT);
    match i {
        _ if i < WATCHED => None,
        _ if i < ABANDONED => Some((ENEMY_WATCH, 0)),
        _ if i < CAPTURE_REGISTERED => None,
        _ if i < CAPTURE_CHECKED => Some((CAPTURE_WATCH, neutral)),
        _ if i < ENEMY_WATCH_1 => None,
        _ if i < ABANDONED_AGAIN => Some((ENEMY_WATCH, 1)),
        _ if i < CAPTURE_REGISTERED_AGAIN => None,
        _ => Some((CAPTURE_WATCH, neutral)),
    }
}

/// Expected list of the watch after the `step` call `i` (sorted; the order changes when a unit in the
/// area changes tile: remove, re-enter at the end).
fn expected_list(i: usize, s: &FortScene) -> Vec<ObjectId> {
    let mut v = match i {
        _ if i < ENEMY_LISTED => vec![],
        _ if i < ABANDONED => vec![s.soldiers[0]],
        _ if i < HOUSE_GONE => vec![s.house, s.soldiers[0], s.soldiers[1], s.soldiers[2]],
        _ if i < ENEMY_WATCH_1 => s.soldiers.to_vec(),
        _ if i < COLONIST_LISTED => vec![],
        _ if i < ABANDONED_AGAIN => vec![s.colonists[2]],
        _ => vec![s.colonists[2], s.soldiers[0], s.soldiers[1], s.soldiers[2]],
    };
    v.sort();
    v
}

#[test]
fn the_fort_scene_shows_every_state() {
    // Counter-check before the golden checksums (all times `step` calls, measured): ring finished and
    // watched (way b); the gate removes the four wall pieces under it and becomes a member of the ring;
    // door tiles blocked in all layers when closed, free when open, one sound each; after the entry
    // the ring on slot 8 and the gate open (without sound); capture query with two owners;
    // after the sale of the house the ring on slot 1 with enemy watch for slot 1; at the end after
    // the colonist slot 8 again with capture watch.
    let (mut state, rules, s) = fort_scene();
    assert_eq!(state.object(s.hq, false).unwrap().tile, HQ_TILE);
    let mut capture_checks = Vec::new();
    let mut last_timer = None;
    for i in 0..FORT_STEPS {
        let orders = fort_orders(i, &s, &state, &rules);
        let events = step(&mut state, &rules, &orders);
        let parts = parts(&state);
        let m = master(&state).expect("master corner");
        // Owner: all parts the same, the change hits the whole group.
        let owners: Vec<u8> = parts.iter().map(|p| p.slot()).collect();
        assert!(
            owners.iter().all(|&o| o == expected_owner(i)),
            "step {i}: {owners:?}"
        );
        // Watch: mode, player, list; the watcher only with the enemy watch (on the master).
        let w = watch(&state);
        assert_eq!(
            w.as_ref().map(|(mode, p, _)| (*mode, *p)),
            expected_watch(i),
            "step {i}"
        );
        if let Some((mode, _, mut list)) = w {
            list.sort();
            assert_eq!(list, expected_list(i, &s), "step {i}");
            let watching = usize::from(mode == ENEMY_WATCH);
            assert_eq!(watchers(&state, m), watching, "step {i}");
            assert_eq!(building_of(&state, m).check_area_rect, FORT_RECT);
        } else {
            assert_eq!(watchers(&state, m), 0, "step {i}");
        }
        // Construction: the ring is not finished until the measured `step`.
        let finished = parts
            .iter()
            .all(|&p| has(&state, p, flags::CONSTRUCTED_FINISHED));
        if i < GATE_AT {
            assert_eq!(parts.len(), 20, "step {i}");
            assert!(gate(&state).is_none());
            assert_eq!(finished, i >= RING_FINISHED, "step {i}");
        } else {
            // Gate init: the four wall pieces under the gate are gone, the gate is the last member.
            assert_eq!(parts.len(), PARTS_WITH_GATE, "step {i}");
            let g = gate(&state).expect("gate");
            let b = building_of(&state, g);
            assert_eq!(b.rect, GATE_RECT);
            assert_eq!(door_tiles_of(&b), DOORS);
            assert_eq!(b.master, m);
            let links = building_of(&state, m).slaves;
            assert_eq!(links.len(), PARTS_WITH_GATE - 1, "step {i}");
            assert_eq!(links.last(), Some(&g));
            let below = parts.iter().filter(|&&p| {
                let t = state.object(p, false).unwrap().tile;
                p != g && t.1 == 25 && (21..=24).contains(&t.0)
            });
            assert_eq!(below.count(), 0, "step {i}: wall piece under the gate");
            assert_eq!(finished, i >= GATE_FINISHED, "step {i}");
            // Gate: neither open nor closed during construction, open when finished, shut between the commands.
            let open = has(&state, g, flags::TOROPEN);
            let closed = has(&state, g, flags::TORCLOSE);
            let shut = (CLOSE_AT..OPEN_AT).contains(&i);
            let expected = match i {
                _ if i < GATE_FINISHED => (false, false),
                _ if shut => (false, true),
                _ => (true, false),
            };
            assert_eq!((open, closed), expected, "step {i}");
            for d in DOORS {
                if shut {
                    assert!(blocked_in_all_layers(&state, d), "step {i}: {d:?}");
                } else {
                    assert!(free_in_base_layers(&state, d), "step {i}: {d:?}");
                }
            }
        }
        // Sound `0x6E` only for the two gate commands (the neutral opening hits an open gate).
        let sounds = events
            .iter()
            .filter(|e| matches!(e, SimEvent::Sound { sound, .. } if *sound == SFX_DOOR))
            .count();
        let commanded = i == CLOSE_AT || i == OPEN_AT;
        assert_eq!(sounds, usize::from(commanded), "step {i}");
        // Colonists: not outside until the measured `step`.
        if i == COLONISTS_OUT - 1 || i == COLONISTS_OUT {
            let out = s
                .colonists
                .iter()
                .all(|&c| state.object(c, false).unwrap().tile.1 >= 26);
            assert_eq!(out, i == COLONISTS_OUT, "step {i}");
        }
        // House: dead from the sale, destroyed in the measured `step`.
        let house = state.object(s.house, false);
        assert_eq!(house.is_some(), i < HOUSE_GONE, "step {i}");
        if let Some(h) = house {
            let died = h.status & crate::object::status::DIED != 0;
            assert_eq!(died, i >= SELL_AT, "step {i}");
        }
        // Capture checks: the timer `0x2903` on the master jumps to 0 when it fires.
        let t = timer_elapsed(&state, m, msg::CHECK_CAPTURE_AREA);
        if let (Some(prev), Some(now)) = (last_timer, t)
            && now < prev
        {
            capture_checks.push(i);
        }
        last_timer = t;
        // Slot 1 waits between capturing and enemy watch for `0x2B03`.
        if (CAPTURED..ENEMY_WATCH_1).contains(&i) {
            assert!(timer_elapsed(&state, m, msg::REGISTER_ENEMY_WATCH).is_some());
        }
    }
    // Five unsuccessful checks with two owners (or the dying house in the list, K-296), the sixth
    // captures; after the colonist again every 22 `step`s without capturing.
    let first: Vec<usize> = (0..5).map(|k| FIRST_CAPTURE_CHECK + 22 * k).collect();
    assert_eq!(capture_checks[..5], first[..]);
    assert_eq!(first[4] + 22, CAPTURE_CHECKED);
    let again = &capture_checks[5..];
    assert_eq!(again.first(), Some(&(CAPTURE_REGISTERED_AGAIN + 22)));
    assert!(again.windows(2).all(|w| w[1] - w[0] == 22), "{again:?}");
    // At the end: slot 8, capture watch with two owners, no change of owner pending.
    assert!(state.owner_changes.is_empty());
    assert_eq!(master(&state).unwrap().slot(), NEUTRAL_SLOT);
    assert!(has(&state, gate(&state).unwrap(), flags::TOROPEN));
}

/// Measured (gate, enemy watch, neutralizing, capturing).
/// Changed: world field `headhunters` (serialization, behavior the same).
/// Changed: state field `ki` (AI state, serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: state fields `ai_marks`, `attack_reports` and `campaign_difficulty` (serialization,
/// behavior the same; counter-check with `serde(skip)`).
/// Changed: state fields `script`, `names` and evocation counter (serialization, behavior the
/// same; counter-check with `serde(skip)`).
/// Changed: event bits 1, 2, 3, 5, 7, kill counter, message sequence `0x2100`/`0x611` and new
/// world fields (counter-check: without setter and with `serde(skip)` the values of the preceding
/// step).
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: serialization of `accounting` and `countdowns` added to the value of the preceding
/// step (counter-check: with `serde(skip)` on both, the values of the preceding step).
/// Changed: serialization of new fields (presentation state, mirror); behavior the same
/// (counter-check with `serde(skip)`).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26);
/// behavior the same (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and
/// `time_limit`, FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all values
/// unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same;
/// counter-check with `serde(skip)`: all values unchanged).
const FORT_GOLDEN: u64 = 0xcfdf_b990_ca3d_1fd3;

/// Continuous sequence of the scene without saving, computed once per test process and shared by
/// the saving tests.
static FORT_SEQUENCE: LazyLock<Vec<u64>> = LazyLock::new(|| fort_run_probed(None, |_, _, _| {}));

#[test]
fn fort_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in FORT_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, FORT_GOLDEN, "{:#018x}", h.0);
}

/// Savegames of the scene (measured): gate closed; enemy listed, the watcher has not fired yet;
/// neutral with waiting `0x2A03`; capture query with two owners; after capturing with waiting
/// `0x2B03`.
const SAVE_GATE_CLOSED: usize = CLOSE_AT + 20;
const SAVE_WATCHER_PENDING: usize = ENEMY_LISTED;
const SAVE_NEUTRAL: usize = NEUTRAL + 10;
const SAVE_TWO_OWNERS: usize = CAPTURE_REGISTERED + 10;
const SAVE_CAPTURED: usize = CAPTURED + 10;

/// Scene with saving/loading after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: FortProbe, what: &str) {
    let saved = fort_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(FORT_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

#[test]
fn fort_checksums_survive_saving_and_loading() {
    // One run per save point, side by side (run time, as with `TENT_GOLDEN`). Each sequence equals
    // the continuous one.
    let cases: [(usize, FortProbe, &str); 5] = [
        (
            SAVE_GATE_CLOSED,
            |state, _, _| {
                let g = gate(state).unwrap();
                assert!(has(state, g, flags::TORCLOSE) && !has(state, g, flags::TOROPEN));
                assert!(DOORS.iter().all(|&d| blocked_in_all_layers(state, d)));
            },
            "gate closed",
        ),
        (
            SAVE_WATCHER_PENDING,
            |state, _, s| {
                let m = master(state).unwrap();
                assert_eq!(watch(state), Some((ENEMY_WATCH, 0, vec![s.soldiers[0]])));
                assert_eq!(watchers(state, m), 1, "watcher before firing");
                assert_eq!(m.slot(), 0);
            },
            "enemy listed, watcher waiting",
        ),
        (
            SAVE_NEUTRAL,
            |state, _, _| {
                let m = master(state).unwrap();
                assert_eq!(m.slot(), NEUTRAL_SLOT);
                assert_eq!(watch(state), None);
                assert!(timer_elapsed(state, m, msg::REGISTER_CAPTURE_WATCH).is_some());
            },
            "neutral with waiting 0x2A03",
        ),
        (
            SAVE_TWO_OWNERS,
            |state, _, s| {
                let (mode, _, list) = watch(state).unwrap();
                assert_eq!(mode, CAPTURE_WATCH);
                let mut owners: Vec<u8> = list.iter().map(|id| id.slot()).collect();
                owners.dedup();
                assert_eq!(owners, [0, 1], "two owners");
                assert!(list.contains(&s.house));
            },
            "capture query with two owners",
        ),
        (
            SAVE_CAPTURED,
            |state, _, _| {
                let m = master(state).unwrap();
                assert_eq!(m.slot(), 1);
                assert_eq!(watch(state), None);
                assert!(timer_elapsed(state, m, msg::REGISTER_ENEMY_WATCH).is_some());
            },
            "after capturing",
        ),
    ];
    std::thread::scope(|scope| {
        let runs: Vec<_> = cases
            .into_iter()
            .map(|(save_at, at_save, what)| {
                scope.spawn(move || assert_survives_saving(save_at, at_save, what))
            })
            .collect();
        for run in runs {
            if let Err(panic) = run.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
}
