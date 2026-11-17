//! Spawning of new units and rally point of the building module (re/buildings): the finished
//! unit is created at the building center, moved to the helper node `ausgang` (if it lies in the
//! grid and its footprint is free there), receives attack mode and behavior, and walks
//! by FINDTO to the freest point near the rally point; afterward (success as well as failure) it looks
//! for the next work via `0x77F`. The rally point (`0x1203`) is a ground point with terrain or
//! water height, or the position of an object.
//!
//! Deviation: the original updates `m_MeetingTarget` only when drawing the rally point flag
//! (`PropertyBuilding_RenderMeetingFlag`, not deterministic); here it is stored but never
//! updated. The statistic "unit trained" goes to the owner of the building
//! ([`crate::accounting`]). The headhunter is spawned by the building via
//! `0x2D03 {guid, slot}` (`!SpawnProducedUnit(guid, slot)`, command `0x2C`,
//! [`crate::command::headhunter`]).

use crate::accounting::{add_statistic, kind as stat};
use crate::collision::find_lowest_cost_spot_near;
use crate::fx::{Fx, FxVec3};
use crate::height::terrain_height_fx;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::quarter::{attack_mode_msg, walk_behavior_msg};
use crate::module::walk::find_to_msg;
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::sim::Sim;
use crate::state::{Control, SimEvent};
use crate::world::{ObjectDesc, create_object, desc_flags, grid_to_world, set_grid_position};

use super::construction::SFX_CREATED;
use super::{building, building_mut, get_exit_pos};

/// "Resume nearest work" (`GameObject_ResumeNearestWork` `0x00467E40`).
pub const RESUME_WORK: u32 = 0x77F;
/// Gatherer modules (harvesting, woodcutting, gold mining): new gatherers receive attack mode 0.
const GATHERER_MODULES: [usize; 3] = [12, 8, 7];

/// `GameObject_GetTargetObject` (`0x00466910`): connected object `+0x2CC ∉ {−1, 0}` (mount).
fn target_object(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let t = sim.object(id)?.target;
    (!t.is_none() && t.0 != 0 && sim.object(t).is_some()).then_some(t)
}

/// `PropertyBuilding_SpawnProducedUnit(guid, slot)` (`0x0050B6B0`): object with `ObjDesc` flags
/// `0xC3` (type, position, slot, color of the building - not "finished", i.e. without object counter
/// and living space in `CreateObject`) at the world position of the building center; if that fails, `false`
/// (retry after 100 ms). If [`get_exit_pos`] returns a valid exit for the unit, it
/// is moved there (height recomputed from the terrain) - if mounted (`ISMOUNTED`) its mount, if that is missing,
/// nobody. Object counter +1 (population was already booked at the start), sound `0x68` as an event. Then
/// attack mode `0x611` and behavior `0x2100`: owner with control kind AI - gatherers only, mode 0;
/// otherwise gatherers mode 0, all others the attack mode of the building. Finally
/// [`send_unit_to_meeting_point`] (its return value).
pub fn spawn_produced_unit(sim: &mut Sim, id: ObjectId, guid: u32, slot: u8) -> bool {
    let cell = sim.rules.map.cell;
    let Some(o) = sim.object(id) else {
        return false;
    };
    let mut desc = ObjectDesc::new(guid, i32::from(slot), grid_to_world(o.grid, cell));
    desc.flags |= desc_flags::COLOR;
    desc.color = o.color;
    let Some(unit) = create_object(sim, &mut desc) else {
        return false;
    };
    let (ok, exit) = get_exit_pos(sim, id, Some(unit));
    if ok {
        let mounted = sim
            .object(unit)
            .is_some_and(|u| u.status & status::ISMOUNTED != 0);
        let moved = if mounted {
            target_object(sim, unit)
        } else {
            Some(unit)
        };
        if let Some(m) = moved {
            set_grid_position(sim, m, exit, true);
        }
    }
    // Statistic "unit trained" (`0x0050B7CD`: `FUN_00478A10(owner of the building, 0, guid)`).
    add_statistic(sim, i32::from(id.slot()), stat::UNIT_TRAINED, guid as i32);
    if let Some(g) = sim.object(unit).map(|u| u.guid) {
        sim.state.add_guid_count(unit.slot(), g, 1);
    }
    // `GameEngine+0x104` (unknown flag, without sound) counts as 0.
    sim.state.events.push(SimEvent::Sound {
        object: unit,
        sound: SFX_CREATED,
    });
    let gatherer = sim
        .object(unit)
        .is_some_and(|u| GATHERER_MODULES.iter().any(|&m| u.has_module(m, false)));
    let ai = sim
        .state
        .players
        .get(usize::from(unit.slot()))
        .is_some_and(|p| p.control == Control::Ai);
    let mode = match (ai, gatherer) {
        (true, false) => None,
        (_, true) => Some(0),
        (false, false) => Some(building(sim, id).map_or(3, |b| b.attack_mode)),
    };
    if let Some(mode) = mode {
        send(sim, unit, &attack_mode_msg(mode, false));
        send(sim, unit, &walk_behavior_msg(mode, false));
    }
    send_unit_to_meeting_point(sim, id, unit)
}

/// `PropertyBuilding_SendUnitToMeetingPoint(id)` (`0x00507800`, also `0x4103`): if the unit is missing,
/// `false`. Otherwise search the point with the lowest collision cost near the rally point
/// ([`find_lowest_cost_spot_near`]); if the unit stands exactly on the **raw** rally point, `0x77F`
/// immediately, otherwise FINDTO `{x, z, −1, no direction, −1, 1}` as a command envelope directly to the
/// unit (`GameObject_DispatchPostedCommand`) with `0x77F` as success and failure. Return value `true`.
pub fn send_unit_to_meeting_point(sim: &mut Sim, id: ObjectId, unit: ObjectId) -> bool {
    if sim.object(unit).is_none() {
        return false;
    }
    let raw = building(sim, id).map_or(FxVec3::ZERO, |b| b.meeting_point);
    let mut spot = (raw.x.0, raw.z.0);
    find_lowest_cost_spot_near(sim, &mut spot, unit);
    let done = Message::new(RESUME_WORK, &[]);
    let at_point = sim
        .object(unit)
        .is_some_and(|u| u.grid.x == raw.x && u.grid.z == raw.z);
    if at_point {
        send(sim, unit, &done);
    } else {
        let env = Envelope {
            msg: find_to_msg(spot.0, spot.1, -1, NO_DIRECTION, -1, false, 1),
            success: Some(done.clone()),
            fault: Some(done),
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        dispatch_command(sim, unit, &env);
    }
    true
}

/// `0x1203 {a, b}` (`0x00507AC0`, command `0x27`): `a = −1` - rally point = grid position of
/// object `b` and `m_MeetingTarget = b` (if it is missing, nothing); otherwise ground point `(a, b)` with the
/// terrain height (`GameEngine_GetTerrainHeightFixed` under the point, the ray starts above the
/// old height), at least the water level (`GameEngine_GetWaterLevelFixed`), without a target.
/// Return value 0.
pub fn on_set_meeting_point(sim: &mut Sim, id: ObjectId, a: i32, b: i32) -> i32 {
    if a == -1 {
        let target = ObjectId::from_i32(b);
        if let Some(g) = sim.object(target).map(|o| o.grid)
            && let Some(bm) = building_mut(sim, id)
        {
            bm.meeting_point = g;
            bm.meeting_target = target;
        }
        return 0;
    }
    let rules = sim.rules;
    let map = rules.map;
    let Some(bm) = building_mut(sim, id) else {
        return 0;
    };
    let mut p = bm.meeting_point;
    p.x = Fx(a);
    p.z = Fx(b);
    let ground = terrain_height_fx(rules, grid_to_world(p, map.cell));
    let water = Fx::from_f32(map.water_level).0 / map.cell[1];
    p.y = Fx(if ground > water { ground } else { water });
    bm.meeting_point = p;
    bm.meeting_target = ObjectId::NONE;
    0
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::module::building::{msg, production};
    use crate::module::test_hook;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::step;
    use crate::world::desc_flags;

    /// Colonist 1100 (walk, stand, woodcutting 8 and idle attack 17), soldier 1101 (without
    /// gatherer module), SPA headquarters 1000.
    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\nPROPID 2\nVALUEID 30001 1 1 0\nPROPID 8\nPROPID 17\nGUID 1101\nNAME SPA_Soldat\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\nPROPID 17\nGUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n";

    /// Colonist 60 food, 15 s, population 1. Exit 400 units in front of the headquarters (-z): tile (7, 12) in front
    /// of the footprint `[5, 14, 10, 17]`.
    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        let kolonist = [(10, 60), (13, 15), (16, 183), (22, 1)];
        r.defaults.insert(1100, kolonist.into_iter().collect());
        let mut shape = crate::module::building::tests::spa_hq();
        shape
            .helper_nodes
            .insert("ausgang".into(), [0.0, 0.0, -400.0]);
        r.building_shapes.insert(1000, shape);
        r
    }

    fn place_hq(sim: &mut Sim, slot: i32) -> ObjectId {
        let mut desc = ObjectDesc::new(1000, slot, [1000.0, 0.0, 2000.0]);
        desc.flags |= desc_flags::PLACED;
        create_object(sim, &mut desc).unwrap()
    }

    thread_local! {
        /// Queries received by modules per object.
        static SEEN: RefCell<Vec<(ObjectId, Message)>> = const { RefCell::new(Vec::new()) };
    }

    fn record(_: &mut Sim, id: ObjectId, _: usize, m: &Message) {
        SEEN.with(|s| s.borrow_mut().push((id, m.clone())));
    }

    fn seen(id: ObjectId, code: u32) -> Vec<Vec<i32>> {
        SEEN.with(|s| {
            s.borrow()
                .iter()
                .filter(|(o, m)| *o == id && m.code == code)
                .map(|(_, m)| m.args.clone())
                .collect()
        })
    }

    fn unit_of(sim: &Sim, guid: u32) -> ObjectId {
        sim.state.tables[0]
            .objects
            .iter()
            .flatten()
            .find(|o| o.guid == guid)
            .map(|o| o.id)
            .expect("unit created")
    }

    // QUIRK W-62: kept
    #[test]
    fn new_units_appear_at_the_exit_and_resume_work_at_the_meeting_point() {
        SEEN.with(|s| s.borrow_mut().clear());
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 0);
        let (_, exit) = get_exit_pos(&mut sim, hq, None);
        assert_eq!((exit.x.0, exit.z.0), (32_000, 51_200));
        // Rally point exactly on the exit: the unit already stands there -> `0x77F` immediately.
        let m = Message::new(msg::SET_MEETING_POINT, &[exit.x.0, exit.z.0]);
        assert_eq!(send(&mut sim, hq, &m), 0);
        test_hook::observe(record, || {
            assert!(spawn_produced_unit(&mut sim, hq, 1100, 0));
        });
        let u = unit_of(&sim, 1100);
        let o = sim.object(u).unwrap();
        assert_eq!((o.grid.x, o.grid.z), (exit.x, exit.z));
        assert_eq!(o.color, sim.object(hq).unwrap().color);
        // Not created "finished": the object counter +1 comes from the spawn.
        assert_eq!(sim.state.guid_counts[0][&1100], 1);
        assert!(sim.state.events.contains(&SimEvent::Sound {
            object: u,
            sound: SFX_CREATED
        }));
        // `0x77F` queries wood, harvest, gold (here the unit has only the woodcutter module 8).
        assert_eq!(seen(u, 0xA08), [vec![-1, 0]]);
        // Gatherer: attack mode 0 to the attack module.
        assert_eq!(seen(u, 0x611), [attack_mode_msg(0, false).args]);
    }

    #[test]
    fn new_units_walk_to_the_freest_spot_near_the_meeting_point() {
        SEEN.with(|s| s.borrow_mut().clear());
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let hq = {
            let mut sim = Sim::new(&mut state, &rules);
            let hq = place_hq(&mut sim, 0);
            sim.state.players[0].resources_fx = [0, 0, 100 << 12, 0, 5 << 12];
            let point = [(8 << 12) + 0x800, (4 << 12) + 0x800];
            send(&mut sim, hq, &Message::new(msg::SET_MEETING_POINT, &point));
            assert_eq!(production::enqueue(&mut sim, hq, 1100), 1);
            hq
        };
        let mut steps = 0;
        test_hook::observe(record, || {
            loop {
                step(&mut state, &rules, &[]);
                steps += 1;
                let sim = Sim::new(&mut state, &rules);
                if sim.state.guid_counts[0].get(&1100) == Some(&1) {
                    let u = unit_of(&sim, 1100);
                    if !seen(u, 0xA08).is_empty() {
                        break;
                    }
                }
                assert!(steps < 2000, "no arrival");
            }
        });
        let sim = Sim::new(&mut state, &rules);
        let u = unit_of(&sim, 1100);
        let o = sim.object(u).unwrap();
        let target = ((8 << 12) + 0x800, (4 << 12) + 0x800);
        let d = crate::collision::fixed_pos_distance((o.grid.x.0, o.grid.z.0), target);
        assert!(d <= 0x400, "distance {d}");
        assert_eq!(seen(u, 0xA08), [vec![-1, 0]]);
        assert!(building(&sim, hq).unwrap().make_guid == 0);
    }

    #[test]
    fn attack_modes_depend_on_gatherers_and_the_control_type() {
        SEEN.with(|s| s.borrow_mut().clear());
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 0);
        let enemy = place_hq(&mut sim, 1);
        sim.state.players[1].control = Control::Ai;
        building_mut(&mut sim, hq).unwrap().attack_mode = 2;
        test_hook::observe(record, || {
            assert!(spawn_produced_unit(&mut sim, hq, 1101, 0));
            assert!(spawn_produced_unit(&mut sim, enemy, 1101, 1));
            assert!(spawn_produced_unit(&mut sim, enemy, 1100, 1));
        });
        let soldier = unit_of(&sim, 1101);
        assert_eq!(seen(soldier, 0x611), [attack_mode_msg(2, false).args]);
        let ai_units: Vec<ObjectId> = sim.state.tables[1]
            .objects
            .iter()
            .flatten()
            .filter(|o| o.guid != 1000)
            .map(|o| o.id)
            .collect();
        let (ai_soldier, ai_gatherer) = (ai_units[0], ai_units[1]);
        // AI: soldiers get nothing, gatherers mode 0; `0x77F` does nothing for the AI.
        assert!(seen(ai_soldier, 0x611).is_empty());
        assert_eq!(seen(ai_gatherer, 0x611), [attack_mode_msg(0, false).args]);
        assert!(seen(ai_gatherer, 0xA08).is_empty());
        // Without a type the spawn fails.
        assert!(!spawn_produced_unit(&mut sim, hq, 4711, 0));
    }

    #[test]
    fn meeting_points_take_the_ground_or_water_height_or_an_object() {
        let mut rules = rules();
        rules.map.water_level = 50.0;
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 0);
        let set = |sim: &mut Sim, a: i32, b: i32| {
            send(sim, hq, &Message::new(msg::SET_MEETING_POINT, &[a, b]))
        };
        // Ground below the water: water height `ftol(50 · 4096) / 128 = 1600`.
        assert_eq!(set(&mut sim, 40_000, 20_000), 0);
        let b = building(&sim, hq).unwrap();
        assert_eq!(
            b.meeting_point,
            FxVec3::new(Fx(40_000), Fx(1600), Fx(20_000))
        );
        assert!(b.meeting_target.is_none());
        let mut m = Message::new(msg::MEETING_POINT, &[]);
        crate::messaging::send_mut(&mut sim, hq, &mut m);
        assert_eq!(m.args, [40_000, 20_000]);
        // Object target: its grid position, the target is stored (never updated).
        let mut desc = ObjectDesc::new(1101, 0, [300.0, 0.0, 400.0]);
        let u = create_object(&mut sim, &mut desc).unwrap();
        set(&mut sim, -1, u.as_i32());
        let g = sim.object(u).unwrap().grid;
        let b = building(&sim, hq).unwrap();
        assert_eq!((b.meeting_point, b.meeting_target), (g, u));
        // Unknown object: nothing.
        set(&mut sim, -1, 12_345);
        assert_eq!(building(&sim, hq).unwrap().meeting_target, u);
        // `0x4103`: send the unit to the rally point; if it is missing, 0.
        let m = Message::new(msg::SEND_TO_MEETING_POINT, &[u.as_i32()]);
        assert_eq!(send(&mut sim, hq, &m), 1);
        let m = Message::new(msg::SEND_TO_MEETING_POINT, &[12_345]);
        assert_eq!(send(&mut sim, hq, &m), 0);
    }
}
