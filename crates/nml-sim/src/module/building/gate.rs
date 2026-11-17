//! Gates in the building module (re/arming-gates 3.C1, 3.C2): gate init on placing
//! ([`init_gate`], 3.C1), door tiles ([`door_tiles`], [`mark_door_tiles`], `0x00504E20`), opening
//! and closing ([`open_door`], [`close_door`], `0x00506AA0`/`0x00506B80`, 3.C2). Collapse
//! (`destroy::on_collapse`) and neutral opening (`building::on_owner_changed`) call them from outside.

use nml_formats::targets::MatchFilter;

use super::{
    Building, building, building_mut, clear_flags, flags, has_flags, mask_index, msg,
    set_building_animation, set_flags,
};
use crate::collision::find_nearest_matching;
use crate::footprint::mark_tile_for_layers;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::tile_map;

/// Sound on opening and closing a gate (`SoundManager_PlayObjectSound3D(0x6E, 3, …)`,
/// re/arming-gates 7); only for a non-forced change.
pub const SFX_DOOR: i32 = 0x6E;

/// Radius of the search for the wall piece under a gate tile: half a tile (`[0x006726EC] = 0x800`,
/// static init `0x00610E20`).
const GATE_SEARCH_RADIUS: i32 = 0x800;

/// `PropertyBuilding_InitGate` (`0x00506820`, re/arming-gates 3.C1), called from `Init` for every
/// gate - also for the construction site, so already on placing (W-267). For each tile of the
/// rectangle (z outer, x inner) the nearest own object with the bit `0x200000` (straight wall pieces;
/// corners and gates do not carry it) within half a tile around the tile center (center distance).
/// The filter also hits **unfinished** pieces: `StartBuild` leaves the bits `0x30_0000` set.
///
/// On the first hit the gate asks the piece for its master (`0x2203`, answer in `args[0]`; without a
/// master the piece itself), stores it **immediately** in `master` and attaches itself to it as a
/// member with `0x2103`. If the master is missing or is the gate itself ("oops, I discovered
/// myself"), the whole loop ends; `master` stays set and no piece is removed (W-268). Every hit piece
/// gets `0x1E03` (destroyed, path free, deregistered at the master); a piece with members of its own
/// (e.g. the master of a group without a corner) survives (answer 1).
pub fn init_gate(sim: &mut Sim, id: ObjectId) {
    let Some(rect) = building(sim, id).map(|b| b.rect) else {
        return;
    };
    let filter = MatchFilter {
        // HANDLETYPE | ANDALL | MYLIST
        mode: 0x112,
        // Module 3, bit `0x200000` (straight wall pieces).
        match_bits: flags::PALISADECORNER,
        ..MatchFilter::constructed()
    };
    let owner = i32::from(id.slot());
    let mut joined = false;
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            let center = ((x << 12) + 0x800, (z << 12) + 0x800);
            let Some(piece) =
                find_nearest_matching(sim, center, &filter, GATE_SEARCH_RADIUS, owner, -1)
            else {
                continue;
            };
            // Guard branch of the original (`w == go`): no gate type carries `0x200000`, the
            // branch is unreachable in the shipped data.
            if piece == id {
                continue;
            }
            if !joined {
                let mut query = Message::new(msg::GET_MASTER, &[0]);
                send_mut(sim, piece, &mut query);
                let master = ObjectId::from_i32(query.arg(0));
                // `m_MasterBuilding` is set before the check, even if it aborts afterwards.
                if let Some(b) = building_mut(sim, id) {
                    b.master = master;
                }
                match sim.object(master).map(|o| o.id) {
                    None => return,
                    Some(m) if m == id => {
                        log::debug!("Gate {id:?}: oops, I discovered myself");
                        return;
                    }
                    Some(m) => {
                        send(sim, m, &Message::new(msg::ADD_SLAVE, &[id.as_i32()]));
                        joined = true;
                    }
                }
            }
            send(sim, piece, &Message::new(msg::FINALLY_DEATH, &[]));
        }
    }
}

/// Door tiles of a gate from rectangle and mask (without [`Sim`], for the viewer): the tiles of the
/// rectangle with mask byte 0 (the `frei` nodes), z outer, x inner. A tile outside the
/// 20 x 20 mask is not a door tile.
pub fn door_tiles_of(b: &Building) -> Vec<(i32, i32)> {
    let rect = b.rect;
    let mut tiles = Vec::new();
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            if mask_index(rect, x, z).is_some_and(|i| b.mask[i] == 0) {
                tiles.push((x, z));
            }
        }
    }
    tiles
}

/// Door tiles of the building `id` ([`door_tiles_of`]); empty without a building.
pub fn door_tiles(sim: &Sim, id: ObjectId) -> Vec<(i32, i32)> {
    building(sim, id).map_or_else(Vec::new, door_tiles_of)
}

/// `PropertyBuilding_MarkDoorTiles(mask, set)` (`0x00504E20`, re/arming-gates 3.C2): block every
/// door tile in all layers (`blocked`) or release it (then the terrain is classified anew),
/// afterwards always `FlushDirtyRect` - without the flag `+0x38C` of `MarkFootprint`. Applies to all
/// players; an automatic opening for own units does not exist (K-291).
pub fn mark_door_tiles(sim: &mut Sim, id: ObjectId, blocked: bool) {
    for (x, z) in door_tiles(sim, id) {
        mark_tile_for_layers(sim, u32::MAX, x, z, blocked);
    }
    sim.state.path.flush_dirty_rect();
}

/// Common part of [`open_door`] and [`close_door`] (mirror images in the original): without `force`
/// nothing if the target bit is already set; switch the bits, door tiles, animation (0 open, 1
/// closed, speed 128), re-stamp the building map (open: `frei` tiles with height 0, closed:
/// passage with full height, D-230), `heightmap_inserted`, sound `0x6E` only without `force`. The
/// selection message `NotifyUiIfSelected(2)` is UI. Always returns 0.
fn set_door(sim: &mut Sim, id: ObjectId, open: bool, force: bool) -> i32 {
    let (set, clear) = if open {
        (flags::TOROPEN, flags::TORCLOSE)
    } else {
        (flags::TORCLOSE, flags::TOROPEN)
    };
    if !force && has_flags(sim, id, set) {
        return 0;
    }
    set_flags(sim, id, set);
    clear_flags(sim, id, clear);
    mark_door_tiles(sim, id, !open);
    set_building_animation(sim, id, i32::from(!open));
    sim.state.tile_map.remove_id(id.as_i32());
    tile_map::add_building(sim, id, open);
    if let Some(b) = building_mut(sim, id) {
        b.heightmap_inserted = true;
    }
    if !force {
        sim.state.events.push(SimEvent::Sound {
            object: id,
            sound: SFX_DOOR,
        });
    }
    0
}

/// `PropertyBuilding_OpenDoor(force)` (`0x00506AA0`, `0x2303`, re/arming-gates 3.C2).
pub fn open_door(sim: &mut Sim, id: ObjectId, force: bool) -> i32 {
    set_door(sim, id, true, force)
}

/// `PropertyBuilding_CloseDoor(force)` (`0x00506B80`, `0x2403`, re/arming-gates 3.C2).
pub fn close_door(sim: &mut Sim, id: ObjectId, force: bool) -> i32 {
    set_door(sim, id, false, force)
}

#[cfg(test)]
mod tests {
    use super::super::INDEX;
    use super::super::fort_testkit::*;
    use super::super::{footprint_rect, has_flags, mask_index};
    use super::*;
    use crate::command::CommandKind;
    use crate::path::grid::COST_FREE;
    use crate::world::{
        ObjectDesc, create_object, destroy_object, process_owner_changes, queue_owner_change,
    };

    /// Construction site of the gate (rotation 0) at the world position `(x, z)`.
    fn gate_site(sim: &mut Sim, slot: i32, (x, z): (f32, f32)) -> ObjectId {
        let mut desc = ObjectDesc::new(GATE, slot, [x, 0.0, z]);
        create_object(sim, &mut desc).expect("gate created")
    }

    /// Gate on the tile boundary 13 in the middle of row 15: rectangle `[11, 15, 14, 15]`.
    const SOUTH_GATE: (f32, f32) = (13.0 * 128.0, 15.5 * 128.0);

    fn objects_of(sim: &Sim, guid: u32) -> Vec<ObjectId> {
        sim.state
            .objects()
            .filter(|o| o.guid == guid)
            .map(|o| o.id)
            .collect()
    }

    /// The straight pieces of the south wall (tiles 11 ... 14 of row 15) in the order of x.
    fn south_pieces(sim: &Sim) -> Vec<ObjectId> {
        let mut v: Vec<(i32, ObjectId)> = objects_of(sim, WALL)
            .into_iter()
            .filter_map(|id| {
                let t = sim.object(id)?.tile;
                (t.1 == 15 && (11..=14).contains(&t.0)).then_some((t.0, id))
            })
            .collect();
        v.sort_by_key(|&(x, _)| x);
        v.into_iter().map(|(_, id)| id).collect()
    }

    fn tile_free(sim: &Sim, x: i32, z: i32) -> bool {
        sim.state.path.cell_cost(x, z, 0) == COST_FREE
    }

    fn assert_pieces_gone_and_tiles_free(sim: &Sim, pieces: &[ObjectId]) {
        for (i, &p) in pieces.iter().enumerate() {
            assert!(sim.state.destroy_queue.contains(&p), "piece {i} marked");
            assert!(!building(sim, p).unwrap().blocked, "piece {i}: path free");
        }
        for x in 11..=14 {
            assert!(tile_free(sim, x, 15), "tile ({x}, 15) free");
        }
    }

    #[test]
    fn a_placed_gate_removes_the_walls_below_and_joins_the_ring() {
        // Case 1: ring from the command `0x18`, all parts unfinished (master = corner (10, 10)).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (30, 30));
        give_wood(&mut sim, 0, 1000);
        run(
            &mut sim,
            0,
            CommandKind::PlaceBuildings {
                parts: fort_ring((10, 10), 4, 4),
                workers: vec![w],
            },
        );
        let corner = objects_of(&sim, CORNER)
            .into_iter()
            .find(|&c| sim.object(c).unwrap().tile == (10, 10))
            .expect("master corner");
        let pieces = south_pieces(&sim);
        assert_eq!(pieces.len(), 4);
        assert_eq!(links_of(&sim, corner).len(), 19);
        assert!(pieces.iter().all(|&p| master_of(&sim, p) == corner));
        assert!(sim.state.destroy_queue.is_empty());
        let gate = gate_site(&mut sim, 0, SOUTH_GATE);
        assert_eq!(footprint_rect(&sim, gate), [11, 15, 14, 15]);
        assert_pieces_gone_and_tiles_free(&sim, &pieces);
        assert_eq!(master_of(&sim, gate), corner);
        let links = links_of(&sim, corner);
        assert_eq!(links.len(), 16, "19 - 4 pieces + gate");
        assert_eq!(links.last(), Some(&gate));
        assert!(pieces.iter().all(|p| !links.contains(p)));
        // `StartBuild` clears `TOROPEN` of the construction site; the flag backup keeps it for
        // completion (re/arming-gates 3.C2).
        assert!(!has_flags(&sim, gate, flags::TOROPEN));
        assert_eq!(
            building(&sim, gate).unwrap().flags_backup & 0x40_0000,
            0x40_0000
        );
        // The first build step blocks the pillars (11, 15) and (14, 15); the door tiles stay free.
        send_mut(&mut sim, gate, &mut Message::new(msg::BUILD_TICK, &[10, 0]));
        assert!(!tile_free(&sim, 11, 15) && !tile_free(&sim, 14, 15));
        assert!(tile_free(&sim, 12, 15) && tile_free(&sim, 13, 15));
        // The group with the gate can be saved and loaded.
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.objects().count(), sim.state.objects().count());
    }

    #[test]
    fn a_gate_on_a_finished_ring_frees_the_blocked_tiles_below() {
        // Case 2: finished ring (way a): the pieces block their tiles before the gate is placed, afterwards
        // they are free under the gate until the build step blocks the pillars (W-267).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ring = finished_ring(&mut sim, 0, (10, 10), 4, 4);
        let pieces = south_pieces(&sim);
        assert_eq!(pieces.len(), 4);
        for x in 11..=14 {
            assert!(
                !tile_free(&sim, x, 15),
                "tile ({x}, 15) blocked before the gate is placed"
            );
        }
        assert!(pieces.iter().all(|&p| building(&sim, p).unwrap().blocked));
        let gate = gate_site(&mut sim, 0, SOUTH_GATE);
        assert_pieces_gone_and_tiles_free(&sim, &pieces);
        assert_eq!(master_of(&sim, gate), ring[0]);
        assert_eq!(links_of(&sim, ring[0]).last(), Some(&gate));
        assert_eq!(links_of(&sim, ring[0]).len(), 19 - 4 + 1);
        // The corners stay blocked.
        assert!(!tile_free(&sim, 10, 15) && !tile_free(&sim, 15, 15));
        // Pillars block with the footprint of the gate, the door tiles stay free.
        super::super::mark_footprint(&mut sim, gate, true, true);
        assert!(!tile_free(&sim, 11, 15) && !tile_free(&sim, 14, 15));
        assert!(tile_free(&sim, 12, 15) && tile_free(&sim, 13, 15));
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.objects().count(), sim.state.objects().count());
    }

    #[test]
    fn the_9c_vector_gives_rect_and_door_tiles() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = spawn_at(&mut sim, GATE, 0, (4992.065, 4927.995), 0.0);
        let rect = footprint_rect(&sim, gate);
        assert_eq!(rect, [37, 38, 40, 38]);
        let mask = building(&sim, gate).unwrap().mask.clone();
        let at = |x: i32| mask[mask_index(rect, x, 38).expect("in the mask")];
        assert_eq!([at(37), at(38), at(39), at(40)], [1, 0, 0, 1]);
    }

    #[test]
    fn a_gate_without_walls_below_finds_nothing() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = spawn_at(&mut sim, GATE, 0, (40.0 * 128.0, 40.5 * 128.0), 0.0);
        assert_eq!(master_of(&sim, gate), ObjectId(0));
        assert!(links_of(&sim, gate).is_empty());
        assert!(sim.state.destroy_queue.is_empty());
        assert!(has_flags(&sim, gate, flags::TOROPEN));
        // As a construction site `StartBuild` clears the bit, the backup keeps it for completion.
        let site = gate_site(&mut sim, 0, (50.0 * 128.0, 50.5 * 128.0));
        assert!(!has_flags(&sim, site, flags::TOROPEN));
        assert_eq!(
            building(&sim, site).unwrap().flags_backup & 0x40_0000,
            0x40_0000
        );
        assert_eq!(master_of(&sim, site), ObjectId(0));
    }

    #[test]
    fn a_gate_on_a_wall_without_master_makes_the_piece_its_master() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let pieces: Vec<ObjectId> = (11..=14)
            .map(|x| spawn(&mut sim, WALL, 0, (x, 15)))
            .collect();
        assert!(pieces.iter().all(|&p| master_of(&sim, p) == ObjectId(0)));
        let gate = gate_site(&mut sim, 0, SOUTH_GATE);
        // The first piece (tile 11) reports itself as master and takes the gate as a member.
        assert_eq!(master_of(&sim, gate), pieces[0]);
        assert_eq!(links_of(&sim, pieces[0]), [gate]);
        // It survives `0x1E03` (answer 1); the other three are destroyed.
        assert_eq!(sim.state.destroy_queue, pieces[1..]);
        let reply = send(&mut sim, pieces[0], &Message::new(msg::FINALLY_DEATH, &[]));
        assert_eq!(reply, 1);
        assert_eq!(sim.state.destroy_queue, pieces[1..]);
        assert!(!tile_free(&sim, 11, 15), "the master piece keeps blocking");
        assert!(tile_free(&sim, 12, 15) && tile_free(&sim, 14, 15));
        // Without a group under the gate the master piece stays a master without a master: loadable.
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.objects().count(), sim.state.objects().count());
    }

    #[test]
    fn gate_init_stops_when_the_master_is_gone_or_itself() {
        // Master missing: the ID is in the piece, the object no longer exists.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ghost = spawn(&mut sim, CORNER, 0, (50, 50));
        let pieces: Vec<ObjectId> = (11..=14)
            .map(|x| spawn(&mut sim, WALL, 0, (x, 15)))
            .collect();
        building_mut(&mut sim, pieces[0]).unwrap().master = ghost;
        destroy_object(&mut sim, ghost);
        assert!(sim.object(ghost).is_none());
        let gate = gate_site(&mut sim, 0, SOUTH_GATE);
        // No wall removed, `master` stays on the missing ID (W-268).
        assert!(sim.state.destroy_queue.is_empty());
        assert_eq!(master_of(&sim, gate), ghost);
        assert!(links_of(&sim, gate).is_empty());
        assert!(pieces.iter().all(|&p| building(&sim, p).unwrap().blocked));
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.objects().count(), sim.state.objects().count());

        // Master is the gate itself: reachable only via foreign states (the savegame rejects them,
        // `wall_group_fault`), not saved here.
        let mut state2 = super::super::fort_testkit::state(&rules);
        let mut sim = Sim::new(&mut state2, &rules);
        let gate = gate_site(&mut sim, 0, SOUTH_GATE);
        let pieces: Vec<ObjectId> = (11..=14)
            .map(|x| spawn(&mut sim, WALL, 0, (x, 15)))
            .collect();
        building_mut(&mut sim, pieces[0]).unwrap().master = gate;
        init_gate(&mut sim, gate);
        assert!(sim.state.destroy_queue.is_empty());
        assert_eq!(master_of(&sim, gate), gate);
        assert!(links_of(&sim, pieces[0]).is_empty() && links_of(&sim, gate).is_empty());
    }
    // --- Doors (re/arming-gates 3.C2, 9.C) ---

    /// Finished gate by re/arming-gates 9.C (rectangle `[37, 38, 40, 38]`, door tiles (38, 38) and
    /// (39, 38)).
    fn vector_gate(sim: &mut Sim) -> ObjectId {
        spawn_at(sim, GATE, 0, (4992.065, 4927.995), 0.0)
    }

    /// Path grid in all nine layers: tile blocked?
    fn blocked_in_all_layers(sim: &Sim, x: i32, z: i32) -> bool {
        (0..9).all(|l| sim.state.path.cell_cost(x, z, l) != COST_FREE)
    }

    /// Path grid in land layers 0 to 3: free? Layer 4 (water) is blocked on land, the derived layers
    /// 5 to 8 (large units) next to the pillars as well.
    fn free_in_base_layers(sim: &Sim, x: i32, z: i32) -> bool {
        (0..4).all(|l| sim.state.path.cell_cost(x, z, l) == COST_FREE)
    }

    fn door_sounds(sim: &Sim, gate: ObjectId) -> usize {
        sim.state
            .events
            .iter()
            .filter(|e| {
                matches!(e, SimEvent::Sound { object, sound }
                    if *object == gate && *sound == SFX_DOOR)
            })
            .count()
    }

    /// Build steps until the building is finished.
    fn complete_site(sim: &mut Sim, id: ObjectId) {
        for _ in 0..400 {
            if has_flags(sim, id, flags::CONSTRUCTED_FINISHED) {
                return;
            }
            send_mut(sim, id, &mut Message::new(msg::BUILD_TICK, &[1000, 0]));
        }
        panic!("construction site {id:?} not finished");
    }

    #[test]
    fn closing_blocks_the_doors_in_all_layers_and_opening_frees_them() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = vector_gate(&mut sim);
        let doors = [(38, 38), (39, 38)];
        assert_eq!(door_tiles(&sim, gate), doors);
        assert!(has_flags(&sim, gate, flags::TOROPEN));
        for (x, z) in doors {
            assert!(free_in_base_layers(&sim, x, z));
            assert_eq!(sim.state.tile_map.handle(x, z), 0, "open: passage height 0");
        }
        // The pillars carry the building height.
        assert!(sim.state.tile_map.handle(37, 38) > 0);
        sim.state.events.clear();
        let starts = sim.object(gate).unwrap().anim.starts;

        // Closing: bits switched, all layers blocked, height at both, animation 1, one sound.
        assert_eq!(close_door(&mut sim, gate, false), 0);
        assert!(has_flags(&sim, gate, flags::TORCLOSE));
        assert!(!has_flags(&sim, gate, flags::TOROPEN));
        for (x, z) in doors {
            assert!(blocked_in_all_layers(&sim, x, z), "door tile ({x}, {z})");
            assert!(sim.state.tile_map.handle(x, z) > 0, "closed: full height");
            assert_eq!(sim.state.tile_map.hid(x, z), gate.as_i32());
        }
        assert!(building(&sim, gate).unwrap().heightmap_inserted);
        assert_eq!(sim.object(gate).unwrap().anim.wanted, 1);
        crate::tick::object_tick(&mut sim, gate, 100);
        let anim = &sim.object(gate).unwrap().anim;
        assert_eq!((anim.starts, anim.current), (starts + 1, 1));
        assert_eq!(door_sounds(&sim, gate), 1);

        // Already closed: nothing, no second sound.
        let before = sim.state.path.cell_cost(38, 38, 0);
        assert_eq!(close_door(&mut sim, gate, false), 0);
        assert_eq!(door_sounds(&sim, gate), 1);
        assert_eq!(sim.state.path.cell_cost(38, 38, 0), before);
        assert!(has_flags(&sim, gate, flags::TORCLOSE) && !has_flags(&sim, gate, flags::TOROPEN));

        // Opening: free, height 0, animation 0, second sound.
        assert_eq!(open_door(&mut sim, gate, false), 0);
        assert!(has_flags(&sim, gate, flags::TOROPEN));
        assert!(!has_flags(&sim, gate, flags::TORCLOSE));
        for (x, z) in doors {
            assert!(free_in_base_layers(&sim, x, z), "door tile ({x}, {z})");
            assert_eq!(sim.state.tile_map.handle(x, z), 0);
        }
        assert!(sim.state.tile_map.handle(37, 38) > 0, "pillars stay high");
        assert_eq!(sim.object(gate).unwrap().anim.wanted, 0);
        assert_eq!(door_sounds(&sim, gate), 2);

        // Forced (loading): same effect, but no sound - also with the bit already set.
        sim.state.events.clear();
        close_door(&mut sim, gate, true);
        close_door(&mut sim, gate, true);
        assert!(blocked_in_all_layers(&sim, 38, 38) && blocked_in_all_layers(&sim, 39, 38));
        open_door(&mut sim, gate, true);
        assert!(free_in_base_layers(&sim, 38, 38));
        assert_eq!(door_sounds(&sim, gate), 0);
        // The state can be saved and loaded.
        close_door(&mut sim, gate, false);
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.objects().count(), sim.state.objects().count());
    }

    #[test]
    fn door_tiles_come_from_the_mask_without_a_sim() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = vector_gate(&mut sim);
        let b = building(&sim, gate).unwrap();
        assert_eq!(door_tiles_of(b), [(38, 38), (39, 38)]);
        // A tile outside the mask is not a door tile: a rectangle that is too wide ends at
        // the mask (20 columns with border 1).
        let mut wide = b.clone();
        wide.rect = [37, 38, 37 + 24, 38];
        assert!(door_tiles_of(&wide).iter().all(|&(x, _)| x < 37 + 19));
        // Without a building no tiles.
        assert!(door_tiles(&sim, ObjectId(0)).is_empty());
    }

    #[test]
    fn own_units_are_blocked_by_a_closed_gate() {
        // K-291: door tiles block for everyone, also for own units; no auto-opening.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ring = finished_ring(&mut sim, 0, (10, 10), 4, 4);
        let gate = spawn_at(&mut sim, GATE, 0, SOUTH_GATE, 0.0);
        assert_eq!(master_of(&sim, gate), ring[0]);
        let doors = door_tiles(&sim, gate);
        assert_eq!(doors, [(12, 15), (13, 15)]);
        let unit = spawn(&mut sim, COLONIST, 0, (12, 12));
        let walk_to = |sim: &mut Sim, tile: (i32, i32)| {
            let kind = CommandKind::Move {
                units: vec![unit],
                x: (tile.0 << 12) + 0x800,
                z: (tile.1 << 12) + 0x800,
                target: -1,
                formation: 0,
                direction: crate::motion::NO_DIRECTION,
                spread: crate::command::NO_SPREAD,
                flags: 0,
            };
            run(sim, 0, kind);
        };
        let target = (13, 17);
        close_door(&mut sim, gate, false);
        walk_to(&mut sim, target);
        run_steps(&mut sim, 400);
        let tile = sim.object(unit).unwrap().tile;
        assert_ne!(tile, target, "through the closed gate");
        assert!(!doors.contains(&tile), "on a door tile");
        assert!(tile.1 < 15, "stays in the fort");
        // Opened and sent off again it reaches the target.
        open_door(&mut sim, gate, false);
        walk_to(&mut sim, target);
        run_steps(&mut sim, 400);
        assert_eq!(sim.object(unit).unwrap().tile, target);
    }

    #[test]
    fn a_finished_gate_is_open() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = vector_gate(&mut sim);
        assert!(has_flags(&sim, gate, flags::TOROPEN));
        assert!(!has_flags(&sim, gate, flags::TORCLOSE));
        assert!(free_in_base_layers(&sim, 38, 38) && free_in_base_layers(&sim, 39, 38));
        // Completion of a construction site: `TOROPEN` from the flag backup, door tiles free,
        // without sound for the gate.
        let site = gate_site(&mut sim, 0, (50.0 * 128.0, 50.5 * 128.0));
        assert!(!has_flags(&sim, site, flags::TOROPEN));
        complete_site(&mut sim, site);
        assert!(has_flags(&sim, site, flags::TOROPEN) && !has_flags(&sim, site, flags::TORCLOSE));
        assert!(free_in_base_layers(&sim, 49, 50) && free_in_base_layers(&sim, 50, 50));
        assert_eq!(door_sounds(&sim, site), 0);
    }

    #[test]
    fn opening_during_construction_is_overwritten_by_completion() {
        // K-292: `CloseDoor` during construction (script only) blocks the door tiles; completion
        // however restores `TOROPEN` from the flag backup and leaves the tiles blocked.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let site = gate_site(&mut sim, 0, (50.0 * 128.0, 50.5 * 128.0));
        assert_eq!(door_tiles(&sim, site), [(49, 50), (50, 50)]);
        close_door(&mut sim, site, false);
        assert!(has_flags(&sim, site, flags::TORCLOSE) && !has_flags(&sim, site, flags::TOROPEN));
        assert!(blocked_in_all_layers(&sim, 49, 50) && blocked_in_all_layers(&sim, 50, 50));
        complete_site(&mut sim, site);
        assert!(
            has_flags(&sim, site, flags::TOROPEN),
            "open after completion"
        );
        assert!(!has_flags(&sim, site, flags::TORCLOSE));
        assert!(
            blocked_in_all_layers(&sim, 49, 50) && blocked_in_all_layers(&sim, 50, 50),
            "door tiles stay blocked"
        );
        // Close -> open restores consistency.
        close_door(&mut sim, site, false);
        open_door(&mut sim, site, false);
        assert!(free_in_base_layers(&sim, 49, 50) && free_in_base_layers(&sim, 50, 50));
    }

    #[test]
    fn a_collapsing_closed_gate_frees_its_doors() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = vector_gate(&mut sim);
        close_door(&mut sim, gate, false);
        send_mut(&mut sim, gate, &mut Message::new(msg::DESTROY, &[]));
        // Destroyed, but not yet collapsed: blocked.
        assert!(blocked_in_all_layers(&sim, 38, 38) && blocked_in_all_layers(&sim, 39, 38));
        for _ in 0..47 {
            crate::module::tick(&mut sim, gate, INDEX, 100);
        }
        assert!(
            sim.state.destroy_queue.contains(&gate),
            "no group: straight to destruction"
        );
        assert!(free_in_base_layers(&sim, 38, 38) && free_in_base_layers(&sim, 39, 38));
        // `MarkFootprint` frees the pillars.
        assert!(free_in_base_layers(&sim, 37, 38) && free_in_base_layers(&sim, 40, 38));
    }

    #[test]
    fn the_wall_dying_before_the_collapse_keeps_closed_doors_blocked() {
        // K-293 (hypothesis, `0x00508900`/`0x00508EB0`): if the group dies via `FinallyDeath` of the
        // master before the closed gate reaches its own `0x3D03`, `MarkFootprint` frees only the
        // mask tiles - the door tiles stay blocked.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ring = finished_ring(&mut sim, 0, (10, 10), 4, 4);
        let gate = spawn_at(&mut sim, GATE, 0, SOUTH_GATE, 0.0);
        assert_eq!(master_of(&sim, gate), ring[0]);
        close_door(&mut sim, gate, false);
        let doors = door_tiles(&sim, gate);
        assert!(
            doors
                .iter()
                .all(|&(x, z)| blocked_in_all_layers(&sim, x, z))
        );
        // Master and all members to 0 HP; do not tick between here and `0x1E03` (otherwise
        // `check_death` triggers `0x3703` beforehand).
        let all: Vec<ObjectId> = ring.iter().copied().chain([gate]).collect();
        for id in all {
            if let Some(o) = sim.object_mut(id) {
                o.attributes.set_current_fraction(14, 0);
            }
        }
        send(&mut sim, ring[0], &Message::new(msg::FINALLY_DEATH, &[]));
        assert!(
            sim.state.destroy_queue.contains(&gate),
            "gate marked for destruction"
        );
        assert!(sim.state.destroy_queue.contains(&ring[0]));
        assert!(
            doors
                .iter()
                .all(|&(x, z)| blocked_in_all_layers(&sim, x, z)),
            "door tiles stay blocked"
        );
        // The pillars are free (footprint).
        assert!(free_in_base_layers(&sim, 11, 15) && free_in_base_layers(&sim, 14, 15));
    }

    #[test]
    fn a_gate_handed_to_slot_8_opens_with_a_sound() {
        // W-270: `OnOwnerChanged` -> `HasFlags(TOR)` and new slot 8 -> `OpenDoor(0)` with sound.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = vector_gate(&mut sim);
        close_door(&mut sim, gate, false);
        sim.state.events.clear();
        queue_owner_change(&mut sim, gate, 8);
        process_owner_changes(&mut sim);
        let neutral = objects_of(&sim, GATE);
        assert_eq!(neutral.len(), 1);
        let new = neutral[0];
        assert_eq!(new.slot(), 8);
        assert_ne!(new, gate);
        assert!(has_flags(&sim, new, flags::TOROPEN) && !has_flags(&sim, new, flags::TORCLOSE));
        assert!(free_in_base_layers(&sim, 38, 38) && free_in_base_layers(&sim, 39, 38));
        assert_eq!(door_sounds(&sim, new), 1);
        assert_eq!(sim.state.tile_map.handle(38, 38), 0);
        assert_eq!(sim.state.tile_map.hid(37, 38), new.as_i32());

        // The change to another slot does not open.
        let other = spawn_at(&mut sim, GATE, 0, (20.0 * 128.0, 20.5 * 128.0), 0.0);
        close_door(&mut sim, other, false);
        sim.state.events.clear();
        queue_owner_change(&mut sim, other, 1);
        process_owner_changes(&mut sim);
        let moved: Vec<ObjectId> = objects_of(&sim, GATE)
            .into_iter()
            .filter(|id| id.slot() == 1)
            .collect();
        assert_eq!(moved.len(), 1);
        assert!(has_flags(&sim, moved[0], flags::TORCLOSE));
        assert!(!has_flags(&sim, moved[0], flags::TOROPEN));
        assert_eq!(door_sounds(&sim, moved[0]), 0);

        // No gate (a wall piece) falls to slot 8: no sound.
        let wall = spawn(&mut sim, WALL, 0, (30, 30));
        sim.state.events.clear();
        queue_owner_change(&mut sim, wall, 8);
        process_owner_changes(&mut sim);
        assert!(
            !sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::Sound { sound, .. } if *sound == SFX_DOOR))
        );
    }
}
