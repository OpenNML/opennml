//! Track building of the station (`PropertyBuilding_PlaceNextTrackSegment` `0x0050A710`,
//! re/train-multipart 3.6, 9.4): a finished order 2518 places a construction site on the
//! tile behind the last own track. Called from the production tick ([`super::production`]).

use nml_formats::targets::MatchFilter;

use crate::collision::find_nearest_matching;
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::setup::player_color;
use crate::sim::Sim;
use crate::world::{ObjectDesc, create_object, desc_flags, grid_to_world};

use crate::module::train::drive::grid_pos_in_map;

use super::{building, building_mut};

/// Search radius of the successor (`0x800` = half tile): a segment on the next tile.
const SUCCESSOR_RADIUS: i32 = 0x800;
/// Filtermodus `0x118`: `CHECKGUID | ANDALL | HL_MYLIST`.
const FILTER_MODE: u32 = 0x118;
/// Creation flags of the construction site (`0xD3`: color, slot, rotation Y, position, type - not finished).
const SITE_FLAGS: u32 = desc_flags::COLOR
    | desc_flags::PLAYER
    | desc_flags::ROT_Y
    | desc_flags::POSITION
    | desc_flags::GUID;

/// `PropertyBuilding_PlaceNextTrackSegment` (`0x0050A710`, re/train-multipart 3.6): places a
/// construction site of type `guid` (2518) on the tile behind the end of the own track chain.
///
/// The start is the segment placed last (`last_track`, `m_LastTrackHID` `+0x3A4`); if it is missing (never
/// placed or removed), the nearest own segment from the station across the whole map
/// (center distance). Without an own segment there is no track building: `false`. The step
/// (`cos`/`sin` of the direction = one tile) is computed **once** from the start segment
/// (`0x0050A880`...`0x0050A89F`, before the loop); the chain runs straight ahead as long as an own segment of the type
/// lies within `0x800` of `segment + step` (W-208: the directions of the
/// other segments do not count, a circle is not possible, the chain ends at the latest at the
/// map edge). The tile behind the end must lie in the map ([`grid_pos_in_map`], otherwise
/// `false`); there the construction site (`0xD3`, not finished) is created with type, player slot, the
/// player color of the slot (`GameWorld_GetPlayerColor` `0x004789E0`, not the color of the
/// station) and the rotation of the last segment, and becomes the new `last_track`. The production
/// ignores the return value (W-206): without a track or at the map edge the payment is lost.
pub fn place_next_track_segment(sim: &mut Sim, station: ObjectId, guid: u32) -> bool {
    let Some(o) = sim.object(station) else {
        return false;
    };
    let origin = (o.grid.x.0, o.grid.z.0);
    let player = i32::from(station.slot());
    let color = player_color(sim, player);
    let filter = MatchFilter {
        mode: FILTER_MODE,
        guid: guid as i32,
        ..MatchFilter::constructed()
    };
    let last = building(sim, station).map_or(ObjectId::NONE, |b| b.last_track);
    let start = if sim.object(last).is_some() {
        last
    } else {
        match find_nearest_matching(sim, origin, &filter, -1, player, -1) {
            Some(first) => first,
            None => return false,
        }
    };
    let Some(dir) = sim.object(start).map(|s| s.direction.0) else {
        return false;
    };
    // Step once from the start segment; each step advances at least half a tile
    // (successor at most `0x800` next to `p`), so the loop ends at the map edge.
    let step = (cos_deg(dir), sin_deg(dir));
    let mut segment = start;
    let (p, rot, seg_y) = loop {
        let Some(s) = sim.object(segment) else {
            return false;
        };
        let (rot, grid) = (s.rotation[1], s.grid);
        let p = (grid.x.0.wrapping_add(step.0), grid.z.0.wrapping_add(step.1));
        match find_nearest_matching(sim, p, &filter, SUCCESSOR_RADIUS, player, -1) {
            Some(next) => segment = next,
            None => break (p, rot, grid.y),
        }
    };
    if !grid_pos_in_map(sim, p) {
        log::debug!("Station {station:?}: track chain ends at the map edge, no track building");
        return false;
    }
    let position = grid_to_world(FxVec3::new(Fx(p.0), seg_y, Fx(p.1)), sim.rules.map.cell);
    let mut desc = ObjectDesc {
        flags: SITE_FLAGS,
        guid,
        player,
        position,
        scale: [1.0; 3],
        rotation: [0.0, rot, 0.0],
        color,
    };
    let Some(site) = create_object(sim, &mut desc) else {
        log::debug!("Station {station:?}: track construction site could not be created");
        return false;
    };
    if let Some(b) = building_mut(sim, station) {
        b.last_track = site;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::{guid_count, pending_guid_count, resource};
    use crate::message::Message;
    use crate::messaging::send;
    use crate::module::building::{INDEX, msg};
    use crate::module::train::testkit::{
        STATION, TRACK, lay_track, rules, spawn, spawn_dir, state,
    };
    use crate::rules::Rules;
    use crate::world::destroy_object;

    /// All track objects (2518) of slot `slot` in table order.
    fn tracks(sim: &Sim, slot: u8) -> Vec<ObjectId> {
        sim.state
            .objects()
            .filter(|o| o.guid == TRACK && o.id.slot() == slot)
            .map(|o| o.id)
            .collect()
    }

    /// Finished in the sense of the building module (`CONSTRUCTED_FINISHED`).
    fn finished(sim: &Sim, id: ObjectId) -> bool {
        crate::command::validate::is_finished(sim, id)
    }

    /// Tile, direction and rotation of the object.
    fn pose(sim: &Sim, id: ObjectId) -> ((i32, i32), i32, f32) {
        let o = sim.object(id).expect("object present");
        (o.tile, o.direction.0 >> 12, o.rotation[1])
    }

    fn last_track(sim: &Sim, station: ObjectId) -> ObjectId {
        building(sim, station).expect("building module").last_track
    }

    /// The new track object compared to `before`.
    fn added(sim: &Sim, before: &[ObjectId]) -> Vec<ObjectId> {
        tracks(sim, 0)
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect()
    }

    /// World with a station at (30, 30) and own tracks (40, 25) ... (40, 22) in direction -z.
    fn setup(rules: &Rules) -> crate::state::SimState {
        state(rules)
    }

    #[test]
    fn a_finished_rail_order_places_a_site_after_the_last_own_segment() {
        let rules = rules();
        let mut state = setup(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let station = spawn(&mut sim, STATION, 0, (30, 30));
        let own = lay_track(&mut sim, (40, 25), (40, 22));
        let before = tracks(&sim, 0);
        assert_eq!(before.len(), 4);
        assert_eq!(last_track(&sim, station), ObjectId::NONE);
        let rot = pose(&sim, own[3]).2;

        assert!(place_next_track_segment(&mut sim, station, TRACK));
        // Construction site on (40, 21), direction and rotation like the last segment, not finished.
        let new = added(&sim, &before);
        assert_eq!(new.len(), 1);
        assert_eq!(pose(&sim, new[0]), ((40, 21), -90, rot));
        assert!(!finished(&sim, new[0]));
        assert_eq!(sim.object(new[0]).map(|o| o.color), Some(0));
        assert_eq!(last_track(&sim, station), new[0]);

        // The next order places behind the construction site.
        let before = tracks(&sim, 0);
        assert!(place_next_track_segment(&mut sim, station, TRACK));
        let new2 = added(&sim, &before);
        assert_eq!(new2.len(), 1);
        assert_eq!(pose(&sim, new2[0]), ((40, 20), -90, rot));
        assert_eq!(last_track(&sim, station), new2[0]);
    }

    #[test]
    fn a_removed_last_segment_falls_back_to_the_nearest_own_segment() {
        let rules = rules();
        let mut state = setup(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let station = spawn(&mut sim, STATION, 0, (30, 30));
        lay_track(&mut sim, (40, 25), (40, 22));
        assert!(place_next_track_segment(&mut sim, station, TRACK));
        let site = last_track(&sim, station);
        assert_eq!(pose(&sim, site).0, (40, 21));

        // Second, nearer track (own, direction +x) and an even nearer foreign one: only the own one
        // counts. The construction site disappears; the ID in `last_track` stays.
        let near = spawn_dir(&mut sim, TRACK, 0, (25, 30), 0);
        spawn_dir(&mut sim, TRACK, 1, (29, 30), 0);
        destroy_object(&mut sim, site);
        assert!(sim.object(site).is_none());
        assert_eq!(last_track(&sim, station), site);

        let before = tracks(&sim, 0);
        assert!(place_next_track_segment(&mut sim, station, TRACK));
        let new = added(&sim, &before);
        assert_eq!(new.len(), 1);
        // From (25, 30) in direction +x: the chain ends at (26, 30).
        assert_eq!(pose(&sim, new[0]), ((26, 30), 0, pose(&sim, near).2));
        assert_eq!(last_track(&sim, station), new[0]);

        // Without the nearer track again the track at (40, 25).
        destroy_object(&mut sim, new[0]);
        destroy_object(&mut sim, near);
        let before = tracks(&sim, 0);
        assert!(place_next_track_segment(&mut sim, station, TRACK));
        let new = added(&sim, &before);
        assert_eq!(new.len(), 1);
        assert_eq!(pose(&sim, new[0]).0, (40, 21));
    }

    #[test]
    fn rail_orders_without_own_track_or_at_the_map_edge_are_lost() {
        let rules = rules();
        let mut state = setup(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let station = spawn(&mut sim, STATION, 0, (30, 30));
        // No own segment (a foreign one does not count): nothing is created.
        spawn_dir(&mut sim, TRACK, 1, (31, 30), 0);
        let count = sim.state.object_count();
        assert!(!place_next_track_segment(&mut sim, station, TRACK));
        assert_eq!(sim.state.object_count(), count);
        assert_eq!(last_track(&sim, station), ObjectId::NONE);

        // Chain ends at the map edge (z < 0 behind tile 0, x > 63 behind tile 63): nothing.
        lay_track(&mut sim, (40, 3), (40, 0));
        let count = sim.state.object_count();
        assert!(!place_next_track_segment(&mut sim, station, TRACK));
        assert_eq!(sim.state.object_count(), count);
        assert_eq!(last_track(&sim, station), ObjectId::NONE);
    }

    #[test]
    fn the_chain_ends_at_every_map_edge() {
        let rules = rules();
        for (from, to) in [
            ((40, 3), (40, 0)),
            ((3, 40), (0, 40)),
            ((60, 40), (63, 40)),
            ((40, 60), (40, 63)),
        ] {
            let mut state = setup(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let station = spawn(&mut sim, STATION, 0, (30, 30));
            lay_track(&mut sim, from, to);
            let count = sim.state.object_count();
            assert!(
                !place_next_track_segment(&mut sim, station, TRACK),
                "{to:?}"
            );
            assert_eq!(sim.state.object_count(), count, "{to:?}");
        }
    }

    /// Station with order 2518 (1 s) and cost 10 wood, ticked until the end of the order.
    fn run_order(sim: &mut Sim, station: ObjectId) {
        send(sim, station, &Message::new(msg::ENQUEUE, &[TRACK as i32]));
        for _ in 0..30 {
            crate::module::tick(sim, station, INDEX, 100);
        }
    }

    #[test]
    fn the_production_places_the_site_and_loses_the_payment_without_track() {
        let mut rules = rules();
        rules
            .defaults
            .insert(TRACK, [(11, 10), (13, 1)].into_iter().collect());
        let mut state = setup(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let station = spawn(&mut sim, STATION, 0, (30, 30));
        sim.state.players[0].resources_fx = [0, 50 << 12, 0, 0, 0];
        // Without an own track: payment (10 wood) gone, nothing is created, "in progress" 0.
        run_order(&mut sim, station);
        assert_eq!(sim.state.resource(0, resource::WOOD), 40);
        assert_eq!(tracks(&sim, 0).len(), 0);
        assert_eq!(pending_guid_count(&sim, 0, TRACK), 0);
        assert_eq!(guid_count(&sim, 0, TRACK), 0);
        assert_eq!(building(&sim, station).map(|b| b.make_guid), Some(0));
        // With a track the finished order places the construction site behind the last segment.
        lay_track(&mut sim, (40, 25), (40, 22));
        let before = tracks(&sim, 0);
        run_order(&mut sim, station);
        assert_eq!(sim.state.resource(0, resource::WOOD), 30);
        let new = added(&sim, &before);
        assert_eq!(new.len(), 1);
        assert_eq!(pose(&sim, new[0]).0, (40, 21));
        assert!(!finished(&sim, new[0]));
        assert_eq!(last_track(&sim, station), new[0]);
        // "In progress": the order is deducted, the construction site counts itself until completion.
        assert_eq!(pending_guid_count(&sim, 0, TRACK), 1);
    }

    #[test]
    fn the_chain_keeps_the_direction_of_its_start_segment() {
        // W-208: the step comes once from the start segment (Ghidra `0x0050A880`). Four
        // segments "in a circle" (0 deg, 90 deg, 180 deg, -90 deg): from (40, 40) with 0 deg it goes to
        // (41, 40), then on in +x - the direction 90 deg of (41, 40) does not count. The construction
        // site stands on (42, 40) and carries the rotation of the last segment (41, 40).
        let rules = rules();
        let mut state = setup(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let station = spawn(&mut sim, STATION, 0, (30, 30));
        spawn_dir(&mut sim, TRACK, 0, (40, 40), 0);
        let turned = spawn_dir(&mut sim, TRACK, 0, (41, 40), 90);
        spawn_dir(&mut sim, TRACK, 0, (41, 41), 180);
        spawn_dir(&mut sim, TRACK, 0, (40, 41), -90);
        let rot = pose(&sim, turned).2;
        let before = tracks(&sim, 0);
        assert!(place_next_track_segment(&mut sim, station, TRACK));
        let new = added(&sim, &before);
        assert_eq!(new.len(), 1);
        assert_eq!(pose(&sim, new[0]), ((42, 40), 90, rot));
    }

    #[test]
    fn the_site_takes_the_player_colour_of_the_station_slot() {
        // `GameWorld_GetPlayerColor(slot of the station)` (`0x0050AA14`), not the color of the
        // station object.
        let rules = rules();
        let mut state = setup(&rules);
        state.players[0].color = 3;
        let mut sim = Sim::new(&mut state, &rules);
        let station = spawn(&mut sim, STATION, 0, (30, 30));
        sim.object_mut(station).expect("station").color = 5;
        lay_track(&mut sim, (40, 25), (40, 22));
        let before = tracks(&sim, 0);
        assert!(place_next_track_segment(&mut sim, station, TRACK));
        let new = added(&sim, &before);
        assert_eq!(sim.object(new[0]).map(|o| o.color), Some(3));
    }
}
