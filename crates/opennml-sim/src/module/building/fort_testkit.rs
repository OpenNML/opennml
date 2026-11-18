//! Synthetic test world for gates, wall groups and the enemy watch (modeled on
//! [`crate::module::tent::testkit`]): 64 x 64 tiles, flat, fully revealed; slots 0 and 1 are human
//! and enemies of each other. Built on the economy world [`crate::command::testkit`] (SPA colonist
//! 1100 as construction worker, headquarters 1000, house 1001, build menu 110) it adds: a combat unit
//! with walk for the opponent ([`SOLDIER`]), tower 1010, wall 1011 (`0x384000`: straight pieces with
//! PALISADE, PALISADECORNER and OHNERAND), corner 1012 (`0x184000`) and gate 1013 (`0x10024000`,
//! hull 4 x 1 tiles, `frei01/frei02` from re/arming-gates 9.C). Build menu 110 contains the wall and
//! the gate; the gate command bar has 430, 431 (`TARGET_DO_MSG`, no `REQUIRED_GUID`) and 529.
//!
//! The world is created as in [`crate::upgrades::testkit`] via [`SimState::new`], not via
//! `setup::new_match`. The tests of the gates, the watch and the commands use this world; the scene
//! of `FORT_GOLDEN` (`fort_tests`) takes its rules for a real level start.

use opennml_formats::gifcon::parse_toolbar;

use crate::command::testkit as base;
use crate::command::{BuildingPart, F32Bits};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::building::{INDEX, building, msg};
use crate::rules::{BuildingShape, ModelBox, Rules};
use crate::sim::Sim;
use crate::state::{Control, SimState};
use crate::world::{ObjectDesc, create_object, desc_flags, world_to_grid};

pub use crate::command::testkit::{COLONIST, HOUSE, HQ, WALL, account, run, spawn, world};

/// Combat unit of the opponent (walk, stand, dying): test value, in the shipped data the units are
/// named differently.
pub const SOLDIER: u32 = 1200;
/// `SPA_Turm` (`TURM 0x10000`, 1 x 1 tile).
pub const TOWER: u32 = 1010;
/// Corner of the SPA wall (wall + 1, flags `0x184000`: wall piece without the gate-wall bit).
pub const CORNER: u32 = 1012;
/// `SPA_Tor`: `TOR | OHNERAND | CASTLEBUILDING` (`0x10024000`), hull 4 x 1 tiles.
pub const GATE: u32 = 1013;

/// Additional types (walk of the soldier like the worker of the tent world, test values).
const FORT_TYPES: &str = concat!(
    // Tower: TURM (`0x10000`).
    "GUID 1010\nNAME SPA_Turm\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 65536\n",
    // Corner: PALISADE | OHNERAND (`0x184000`).
    "GUID 1012\nNAME SPA_Ecke\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 1589248\n",
    // Gate: TOR | OHNERAND | CASTLEBUILDING (`0x10024000`).
    "GUID 1013\nNAME SPA_Tor\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 268582912\n",
    // Soldier: walk, stand, dying.
    "GUID 1200\nNAME SPA_Soldat\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 1 1 2\n",
);

/// The types of the economy world with wall 1011 as `0x384000` (instead of `0x300000`) and the
/// types of this world.
pub fn types() -> String {
    // 0x300000 = 3145728 (only the wall has this flag word), 0x384000 = 3686400.
    let wall = base::TYPES.replace("VALUEID 40003 6 1 3145728", "VALUEID 40003 6 1 3686400");
    debug_assert_ne!(wall, base::TYPES);
    format!("{wall}{FORT_TYPES}")
}

/// Gate command bar (re/arming-gates 2.C): 430 "open" and 431 "close" (both button 0), 529
/// "demolish" (button 13), all `TARGET_DO_MSG` without `REQUIRED_GUID`.
const GATE_TOOLBAR: &str = concat!(
    "GUID 1013\n",
    "BUTTONID 0\nBUTTONMSG 430\nTARGET_DO_MSG\n",
    "BUTTONID 0\nBUTTONMSG 431\nTARGET_DO_MSG\n",
    "BUTTONID 13\nBUTTONMSG 529\nTARGET_DO_MSG\n",
    "ENDGUID\n",
);

/// Rules of the test world.
pub fn rules() -> Rules {
    rules_from(&types())
}

/// [`rules`] with other types (e.g. [`types`] with additional types of a test).
pub fn rules_from(types: &str) -> Rules {
    let mut r = base::rules_from(types);
    let set = |v: &[(u32, i32)]| v.iter().copied().collect();
    // Costs in wood (11), build time in s (13) and hit points (14), test values like the wall.
    r.defaults
        .insert(CORNER, set(&[(11, 4), (13, 6), (14, 1000)]));
    r.defaults
        .insert(GATE, set(&[(11, 12), (13, 6), (14, 1000)]));
    r.defaults
        .insert(TOWER, set(&[(11, 20), (13, 10), (14, 1500)]));
    r.defaults.insert(SOLDIER, set(&[(14, 160), (16, 145)]));
    // The house occupies one tile (the economy world gives it 3 x 3).
    set_shape(&mut r, HOUSE, [0.0; 3], [50.0, 50.0, 50.0]);
    for guid in [CORNER, TOWER] {
        set_shape(&mut r, guid, [0.0; 3], [50.0, 50.0, 50.0]);
    }
    // Gate: hull center (0, 0, 0), half extents (256, 50, 64) = 4 x 1 tiles; the door nodes `frei01` and
    // `frei02` are in `free_nodes` (mask) and in `helper_nodes` (`tile_map::add_building` reads them
    // from there).
    let frei = [[-67.505, 0.0, 5.378], [57.336, 0.0, 5.378]];
    r.building_shapes.insert(
        GATE,
        BuildingShape {
            center: [0.0; 3],
            half: [256.0, 50.0, 64.0],
            free_nodes: frei.to_vec(),
            helper_nodes: [("frei01", frei[0]), ("frei02", frei[1])]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        },
    );
    r.model_boxes.insert(
        GATE,
        ModelBox {
            center: [0.0; 3],
            half: [256.0, 50.0, 64.0],
        },
    );
    // Build menu 110: the gate after the wall.
    let toolbar = base::TOOLBAR.replace(
        "BUTTONID 10\nBUTTONMSG 1011\nBUILD_MSG\nGROUP_AND_MSG\nENDGUID\n",
        "BUTTONID 10\nBUTTONMSG 1011\nBUILD_MSG\nGROUP_AND_MSG\n\
         BUTTONID 11\nBUTTONMSG 1013\nBUILD_MSG\nGROUP_AND_MSG\nENDGUID\n",
    );
    debug_assert_ne!(toolbar, base::TOOLBAR);
    r.toolbar = parse_toolbar(&format!("{toolbar}{GATE_TOOLBAR}"));
    r
}

/// Building hull and model box from `center` and `half`.
fn set_shape(r: &mut Rules, guid: u32, center: [f32; 3], half: [f32; 3]) {
    r.building_shapes.insert(
        guid,
        BuildingShape {
            center,
            half,
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        },
    );
    r.model_boxes.insert(guid, ModelBox { center, half });
}

/// Empty, fully revealed 64 x 64 world; slots 0 and 1 are human and enemies of each other
/// (default of diplomacy). Accounts empty: [`give_wood`] fills the wood account.
pub fn state(rules: &Rules) -> SimState {
    let mut s = base::state(rules);
    for slot in [0, 1] {
        let p = &mut s.players[slot];
        p.slot_active = true;
        p.control = Control::Human;
    }
    debug_assert!(s.diplomacy.is_enemy(0, 1) && s.diplomacy.is_enemy(1, 0));
    s
}

/// Wood account of the slot (whole units; gold and food 0, living space for ten).
pub fn give_wood(sim: &mut Sim, slot: usize, wood: i32) {
    account(sim, slot, [0, wood, 0, 0, 10]);
}

/// Finished object (pre-placed) at the world position `(x, z)` with rotation `rotation` (radians
/// around the vertical axis).
pub fn spawn_at(
    sim: &mut Sim,
    guid: u32,
    slot: i32,
    (x, z): (f32, f32),
    rotation: f32,
) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [x, 0.0, z]);
    desc.rotation = [0.0, rotation, 0.0];
    desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
    create_object(sim, &mut desc).expect("object created")
}

/// Set the object to the center of tile `tile` (tile change via `set_grid_position`, as movement
/// does it: remove, enter, notify area queries).
pub fn move_to(sim: &mut Sim, id: ObjectId, tile: (i32, i32)) {
    let pos = world_to_grid([world(tile.0), 0.0, world(tile.1)], sim.rules.map.cell);
    crate::world::set_grid_position(sim, id, pos, true);
}

/// `n` engine ticks without commands (two `step`s are one object tick).
pub fn run_steps(sim: &mut Sim, n: usize) {
    for _ in 0..n {
        crate::tick::step(sim.state, sim.rules, &[]);
    }
}

/// Ring of the fort cursor like `FUN_00450BE0` for `dx, dz > 0` around the tile `base` (tile
/// center): part 0 corner, then `dx` wall pieces, corner, `dz` wall pieces, corner, `dx` back,
/// corner, `dz` back; rotations in quarter steps like the original in `f32`.
pub fn fort_ring(base: (i32, i32), dx: i32, dz: i32) -> Vec<BuildingPart> {
    let (sx, sz) = (128.0f32, 128.0f32);
    let (b0, b2) = (world(base.0), world(base.1));
    let mut parts = Vec::new();
    let mut push = |guid, (ox, oz): (f32, f32), rot: f32| {
        parts.push(BuildingPart {
            guid,
            rotation: F32Bits::new(rot),
            x: F32Bits::new(b0 + ox),
            y: F32Bits::new(0.0),
            z: F32Bits::new(b2 + oz),
        });
    };
    let (f40, f38) = (-1.570_796_4f32, 1.570_796_4f32);
    let mut f9 = std::f32::consts::PI;
    let (ex, ez) = ((dx + 1) as f32 * sx, (dz + 1) as f32 * sz);
    push(CORNER, (0.0, 0.0), f40 + f9);
    for i in 1..=dx {
        push(WALL, (i as f32 * sx, 0.0), f9);
    }
    f9 += f38;
    push(CORNER, (ex, 0.0), f40 + f9);
    for k in 1..=dz {
        push(WALL, (ex, k as f32 * sz), f9);
    }
    f9 += f38;
    push(CORNER, (ex, ez), f40 + f9);
    for i in (1..=dx).rev() {
        push(WALL, (i as f32 * sx, ez), f9);
    }
    push(CORNER, (0.0, ez), f40 + f38 + f9);
    for k in (1..=dz).rev() {
        push(WALL, (0.0, k as f32 * sz), f38 + f9);
    }
    parts
}

/// Ring created as finished (way a, re/arming-gates 3.D1): all parts of the [`fort_ring`] as finished
/// buildings of `slot`, then `0x1B03` to part 0 (the corner becomes master, the other parts members
/// in command order). Returns the IDs in ring order (part 0 first).
pub fn finished_ring(
    sim: &mut Sim,
    slot: i32,
    base: (i32, i32),
    dx: i32,
    dz: i32,
) -> Vec<ObjectId> {
    let ids: Vec<ObjectId> = fort_ring(base, dx, dz)
        .iter()
        .map(|p| spawn_at(sim, p.guid, slot, (p.x.get(), p.z.get()), p.rotation.get()))
        .collect();
    let mut args = vec![i32::try_from(ids.len() - 1).expect("few parts")];
    args.extend(ids[1..].iter().map(|id| id.as_i32()));
    send(sim, ids[0], &Message::new(msg::SET_SLAVE_LIST, &args));
    ids
}

/// Master of the building (`ObjectId(0)` = none).
pub fn master_of(sim: &Sim, id: ObjectId) -> ObjectId {
    building(sim, id).map_or(ObjectId(0), |b| b.master)
}

/// Members of the building in list order.
pub fn links_of(sim: &Sim, id: ObjectId) -> Vec<ObjectId> {
    building(sim, id).map_or_else(Vec::new, |b| b.slaves.clone())
}

/// Flag word of the building module (with the module index in the low byte as in the original: every
/// bit counts without the low byte).
pub fn module_flags(sim: &Sim, id: ObjectId) -> u32 {
    sim.module(id, INDEX).map_or(0, |m| m.base.flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandKind;
    use crate::module::building::footprint_rect;

    fn live_objects(sim: &Sim, guid: u32) -> Vec<ObjectId> {
        sim.state
            .objects()
            .filter(|o| o.guid == guid)
            .map(|o| o.id)
            .collect()
    }

    #[test]
    fn the_gate_has_the_hull_of_four_tiles_and_both_door_nodes() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Gate x on the tile boundary 13, z in the middle of tile row 15 (re/arming-gates 9.C).
        let a = spawn_at(&mut sim, GATE, 0, (13.0 * 128.0, 15.5 * 128.0), 0.0);
        assert_eq!(footprint_rect(&sim, a), [11, 15, 14, 15]);
        let b = spawn_at(&mut sim, GATE, 0, (4992.065, 4927.995), 0.0);
        assert_eq!(footprint_rect(&sim, b), [37, 38, 40, 38]);
        let shape = &rules.building_shapes[&GATE];
        assert_eq!(shape.free_nodes.len(), 2);
        assert!(
            shape.helper_nodes.contains_key("frei01") && shape.helper_nodes.contains_key("frei02")
        );
        assert_eq!(module_flags(&sim, b) & 0x10_0000, 0);
        // Command bar of the gate: 430 and 431 (button 0), 529 (button 13), without `REQUIRED_GUID`;
        // the build menu of the colonist contains wall and gate.
        let gate_menu = &rules.toolbar.menus[&1013];
        let buttons: Vec<(i32, i32)> = gate_menu.iter().map(|b| (b.id, b.msg)).collect();
        assert_eq!(buttons, [(0, 430), (0, 431), (13, 529)]);
        assert!(gate_menu.iter().all(|b| b.required == [0; 5]));
        let build: Vec<i32> = rules.toolbar.menus[&110].iter().map(|b| b.msg).collect();
        assert!(build.contains(&(WALL as i32)) && build.contains(&(GATE as i32)));
    }

    #[test]
    fn the_ring_and_a_gate_on_it_can_be_placed() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        give_wood(&mut sim, 0, 1000);
        let ring = fort_ring((20, 20), 4, 4);
        assert_eq!(ring.len(), 20);
        run(
            &mut sim,
            0,
            CommandKind::PlaceBuildings {
                parts: ring,
                workers: vec![w],
            },
        );
        let (corners, walls) = (live_objects(&sim, CORNER), live_objects(&sim, WALL));
        assert_eq!((corners.len(), walls.len()), (4, 16));
        assert_eq!(links_of(&sim, corners[0]).len(), 19);
        assert_eq!(master_of(&sim, walls[0]), corners[0]);
        // The gate on the north wall (tiles 21 ... 24 of row 20): the check snaps position and rotation
        // onto the wall piece (exception A8), the command carries them bit-identical.
        let (pos, rot) = crate::placement::snapped_command_pose(
            &mut sim,
            GATE,
            [23.0 * 128.0, 0.0, 20.5 * 128.0],
            0.0,
            0,
            Some(w),
        )
        .expect("gate can be placed on the wall");
        let gate = CommandKind::PlaceBuilding {
            guid: GATE,
            rotation: F32Bits::new(rot),
            x: F32Bits::new(pos[0]),
            y: F32Bits::new(pos[1]),
            z: F32Bits::new(pos[2]),
            workers: vec![w],
            border: None,
        };
        run(&mut sim, 0, gate);
        assert_eq!(live_objects(&sim, GATE).len(), 1);
    }

    #[test]
    fn a_finished_ring_has_the_corner_as_master() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ids = finished_ring(&mut sim, 0, (20, 20), 4, 4);
        assert_eq!(ids.len(), 20);
        assert_eq!(links_of(&sim, ids[0]), ids[1..]);
        assert!(ids[1..].iter().all(|&s| master_of(&sim, s) == ids[0]));
        assert_eq!(master_of(&sim, ids[0]), ObjectId(0));
    }

    #[test]
    fn soldiers_of_the_opponent_are_enemies_with_a_walk_module() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let enemy = spawn(&mut sim, SOLDIER, 1, (30, 30));
        let o = sim.object(enemy).unwrap();
        assert!(o.has_module(0, true));
        run_steps(&mut sim, 2);
        move_to(&mut sim, enemy, (31, 30));
        assert_eq!(sim.object(enemy).unwrap().tile, (31, 30));
    }
}
