//! Destroying, selling and demolishing a building (re/buildings): `Destroy` (`0x3703`, from
//! `GameObject_CheckDeath` at HP < 1 or on a sale), `OnSell` (`0x3903`), the timers to itself
//! (`0x3B03` sinking, `0x3C03` fire out, `0x3D03` collapse, `0x4203` explosion), `FinallyDeath`
//! (`0x1E03`, also in the wall group) and waking a sleeping palisade (`0x2003`).
//!
//! Event bit 2 (`0x00506344`) and kill counter (`FUN_00481680` `0x00506445`) for the scripts as well
//! as the statistic "building lost/destroyed" ([`crate::accounting`]) are set by [`destroy`]. Not
//! reproduced: the selection (UI), smoke, sinking and fire (presentation:
//! `Building_SpawnDestructionSmoke`, `OnMsgStartSink`, `FrameUpdate`, `GameObject_UpdateDamageFires`)
//! and the sound of the explosion (host, via the name).
//! The door tiles of a closed gate are freed by `0x3D03` ([`on_collapse`], [`super::gate`]).

use nml_formats::targets::MatchFilter;

use crate::accounting::{add_statistic, kind as stat};
use crate::collision::find_nearest_matching;
use crate::economy::{add_owner_pending_guid_count, pay_costs_percent, resource};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::quarter;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::queue_destroy;

use super::{
    GROUP, INDEX, building, building_mut, clear_flags, flags, gate, has_flags, mark_footprint, msg,
};

/// Sound of the destruction (`SoundManager_PlayObjectSound3D(0x65, 0x23, …)`).
pub const SFX_DESTROYED: i32 = 0x65;
/// Explosion effect of a destroyed, non-exploding building.
pub const EXPLOSION_BUILDING: &str = "data/effects/explosion_building.pef";
/// Explosion effect of an exploding building (`0x4203`).
pub const EXPLOSION_POWDER_HOUSE: &str = "data/effects/explosion_pulverhaus.pef";
/// Impact per hit of the explosion (`0x4203`).
pub const IMPACT_GROUND: &str = "data/effects/einschlag_boden.pef";
/// Delay of the explosion (`BUILDINGEXPLODE`) in ms.
pub const EXPLODE_DELAY: i32 = 700;
/// Delay of the collapse (`0x3D03`) in ms.
pub const COLLAPSE_DELAY: i32 = 4500;
/// Delay after which a still existing building becomes a construction site again (`0x103`), in ms.
pub const REBUILD_DELAY: i32 = 1000;
/// Search radius of the sleeping palisade under a tower (`DAT_006726EC`, half a tile).
pub const UNDERGROUND_RADIUS: i32 = 0x800;

/// Filter of the palisade under a tower (`PropertyBuilding_OnMsgFinallyDeath`): `RETURN_DIED |
/// PLAYERLIST | ANDALL | CHECKGUID`, `MM2_NOHITPOINTS`; the original leaves words 2-4 and 6-9
/// unset, the check does not read them in this mode.
fn underground_filter(guid: u32) -> MatchFilter {
    MatchFilter {
        mode: 0x0404_0018,
        mode2: 0x10,
        guid: guid as i32,
        ..MatchFilter::constructed()
    }
}

fn post(sim: &mut Sim, id: ObjectId, delay: i32, m: Message) {
    if let Some(module) = sim.module_mut(id, INDEX) {
        module.base.post_delayed_message(delay, m, false, GROUP);
    }
}

/// `PropertyBuilding_Destroy` (`0x00506300`, `0x3703`): nothing if already dead. Otherwise in the
/// order of the original: out of the `BuildingTileMap` (flag 0); finished -> object counter -1 and
/// population/living space from the **attributes** 0x16/0x1B back to the owner, unfinished -> "in
/// progress" -1 and `m_IsBuildBuilding` 0; state dead, no longer selectable, sound `0x65`;
/// exploding -> timer `0x4203` after 700 ms, otherwise the explosion effect; `m_IsDead`;
/// `m_IsDying` 1, `0x2117` to the own object (quarters puts the occupants out), `m_IsDying`
/// immediately 0 again (quirk: sinking in the tick and `TickDying` are dead code); timers
/// `0x3B03 {−800, 300}` after 100 ms, `0x3B03 {−200, 2000}` after 1300 ms, `0x3C03` after 100 ms and
/// `0x3D03` (collapse) after 4500 ms. A running production stops (the tick ends at
/// `m_IsDead`); the refund does not happen until the destructor.
pub fn destroy(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    if o.status & status::DIED != 0 {
        return;
    }
    let (guid, attacker) = (o.guid, o.last_attacker);
    // Event bit 2 first (`0x00506344`, scripts); then `BuildingTileMap_RemoveObjectId`
    // (`0x00490B50`).
    crate::world::set_object_event_bit(sim, id, 2, true);
    if let Some(b) = building_mut(sim, id) {
        b.heightmap_inserted = false;
    }
    sim.state.tile_map.remove_id(id.as_i32());
    // Statistic "building lost" at the owner (`0x0050638A`) and "destroyed" at the slot of the last
    // attacker (`0x005063B6`, bits 11...15 of the ID, without enemy check - O-168); never palisades
    // (`HasFlags(0x300003)`), construction sites yes (no `0x403` test, O-169).
    if !has_flags(sim, id, flags::PALISADE | flags::PALISADECORNER) {
        add_statistic(sim, i32::from(id.slot()), stat::BUILDING_LOST, guid as i32);
        let attacker = sim.object(id).map_or(ObjectId::NONE, |o| o.last_attacker);
        if !attacker.is_none() {
            let by = i32::from(attacker.slot());
            add_statistic(sim, by, stat::BUILDING_DESTROYED, guid as i32);
        }
    }
    let slot = id.slot();
    if has_flags(sim, id, flags::CONSTRUCTED_FINISHED) {
        sim.state.add_guid_count(slot, guid, -1);
        let population = sim.attribute(id, 0x16);
        let living = sim.attribute(id, 0x1B);
        if let Some(p) = sim.state.players.get_mut(usize::from(slot)) {
            p.add_resource(resource::POPULATION, population.wrapping_neg());
            p.add_resource(resource::LIVING_SPACE, living.wrapping_neg());
        }
    } else {
        add_owner_pending_guid_count(sim.state, id, guid, -1);
        if let Some(b) = building_mut(sim, id) {
            b.is_build_building = false;
        }
    }
    // Kill counter of the last attacker (`FUN_00481680` `0x00506445`, E-189): unconditional, also
    // construction sites and palisades; discarded without an attacker (slot 31).
    crate::world::add_kill_count(sim, crate::world::kill_slot(attacker), guid);
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::DIED, true);
        // Out of the selection (`FUN_004801B0`, `GameObject_NotifyUiIfSelected`): UI.
        o.selectable = false;
    }
    // Sinking counter and speed: presentation. `Building_SpawnDestructionSmoke` (`0x0046C350`) comes
    // before sound and explosion and unconditionally, also for the exploding building: the host
    // draws smoke and crater for the event (re/buildings 8.5a).
    sim.state
        .events
        .push(SimEvent::BuildingDestroyed { object: id });
    sim.state.events.push(SimEvent::Sound {
        object: id,
        sound: SFX_DESTROYED,
    });
    if building(sim, id).is_some_and(|b| b.is_explode) {
        post(sim, id, EXPLODE_DELAY, Message::new(msg::EXPLODE, &[]));
    } else {
        sim.state.events.push(SimEvent::Effect {
            object: id,
            path: EXPLOSION_BUILDING.to_string(),
        });
    }
    if let Some(b) = building_mut(sim, id) {
        b.is_dead = true;
        b.is_dying = true;
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits &= !2;
    }
    send(
        sim,
        id,
        &Message::new(quarter::msg::CONTAINER_DESTROYED, &[]),
    );
    if let Some(b) = building_mut(sim, id) {
        b.is_dying = false;
    }
    post(sim, id, 100, Message::new(msg::START_SINK, &[-800, 300]));
    post(sim, id, 1300, Message::new(msg::START_SINK, &[-200, 2000]));
    post(sim, id, 100, Message::new(msg::CLEAR_FIRES, &[]));
    post(sim, id, COLLAPSE_DELAY, Message::new(msg::COLLAPSE, &[]));
    // Vision source over the rubble field (`0x0050664D`…): tile, size 8, owner, 10 s, explored
    // + seen (re/vision 4.4).
    let tile = sim.object(id).map_or((0, 0), |o| o.tile);
    crate::fog::set_vision_source(
        &mut sim.state.fog,
        crate::fog::VisionSource {
            x: tile.0,
            z_or_hid: tile.1,
            size: 8,
            player: i32::from(id.slot()),
            remaining_ms: 10_000,
            flags: crate::fog::EXPLORE | crate::fog::SEE,
        },
    );
}

/// `PropertyBuilding_OnSell` (`0x00507490`, `0x3903`): a construction site refunds the owner
/// `CostPerTick · remaining work / 100` raw per gold, wood, food (`Player_PayCostsPercent` with
/// `−remaining work`; quirk: 1 % of the remaining value), a finished building nothing; then [`destroy`].
/// The original does not check whether the building is already dead: a destroyed construction site
/// refunds again on every sale (quirk; the command check admits only living own buildings).
pub fn on_sell(sim: &mut Sim, id: ObjectId) -> i32 {
    if !has_flags(sim, id, flags::CONSTRUCTED_FINISHED)
        && let Some(b) = building(sim, id)
    {
        let (cost, counter) = (b.cost_per_tick, b.build_counter);
        if let Some(p) = sim.state.players.get_mut(usize::from(id.slot())) {
            pay_costs_percent(p, &cost, counter.wrapping_neg());
        }
    }
    destroy(sim, id);
    0
}

/// `PropertyBuilding_OnMsgCollapse` (`0x00508EB0`, `0x3D03`, 4.5 s after [`destroy`]): path grid
/// free (with clearing), timer `0x103` (construction site again) after 1000 ms - it fires only if the
/// building still stands then (quirk: a destroyed member of a living wall group becomes a
/// construction site again) -, `CONSTRUCTED_FINISHED` off, `m_IsDying` 0. Tracks (`TRACK`) stay
/// standing; with a living master its `0x1E03` decides, otherwise [`on_finally_death`].
pub fn on_collapse(sim: &mut Sim, id: ObjectId) -> i32 {
    // A closed gate frees its door tiles first (`PropertyBuilding_MarkDoorTiles(mask, 0)`,
    // `0x00504E20`, re/arming-gates 3.C2); only here, not in `FinallyDeath` (K-293).
    if has_flags(sim, id, flags::TORCLOSE) {
        gate::mark_door_tiles(sim, id, false);
    }
    mark_footprint(sim, id, false, true);
    post(sim, id, REBUILD_DELAY, Message::new(msg::START_BUILD, &[]));
    clear_flags(sim, id, flags::CONSTRUCTED_FINISHED);
    if let Some(b) = building_mut(sim, id) {
        b.is_dying = false;
    }
    if has_flags(sim, id, flags::TRACK) {
        return 0;
    }
    let master = building(sim, id).map_or(ObjectId(0), |b| b.master);
    if master != ObjectId(0) && sim.object(master).is_some() {
        return send(sim, master, &Message::new(msg::FINALLY_DEATH, &[]));
    }
    on_finally_death(sim, id)
}

/// `PropertyBuilding_OnMsgFinallyDeath` (`0x00508900`, `0x1E03`): with members the group lives as
/// long as the building itself or an existing member has an HP share != 0 (return value 1);
/// otherwise all existing members get `0x1E03`. Then mark for destruction, path grid free,
/// deregister at the master with `0x1C03 {id}` and wake a sleeping own palisade under a tower (GUID
/// `m_UndergroundBuilding`, also dead ones, within half a tile around the center) with `0x2003`.
/// Return value 0.
pub fn on_finally_death(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(b) = building(sim, id) else {
        return 0;
    };
    let (slaves, master, underground) = (b.slaves.clone(), b.master, b.underground_building);
    if !slaves.is_empty() {
        let lives = |sim: &Sim, s: ObjectId| {
            sim.object(s)
                .is_some_and(|o| o.attributes.current_fraction(14) != 0)
        };
        if lives(sim, id) || slaves.iter().any(|&s| lives(sim, s)) {
            return 1;
        }
        for s in slaves {
            if sim.object(s).is_some() {
                send(sim, s, &Message::new(msg::FINALLY_DEATH, &[]));
            }
        }
    }
    queue_destroy(sim, id);
    mark_footprint(sim, id, false, true);
    if master != ObjectId(0) && sim.object(master).is_some() {
        send(
            sim,
            master,
            &Message::new(msg::REMOVE_SLAVE, &[id.as_i32()]),
        );
    }
    if underground != 0 {
        let Some(o) = sim.object(id) else { return 0 };
        let pos = (o.grid.x.0, o.grid.z.0);
        let filter = underground_filter(underground);
        let owner = i32::from(id.slot());
        if let Some(p) = find_nearest_matching(sim, pos, &filter, UNDERGROUND_RADIUS, owner, -1) {
            send(sim, p, &Message::new(msg::WAKE, &[]));
        }
    }
    0
}

/// `PropertyBuilding_OnMsgWake` (`0x00504DE0`, `0x2003`): awake, no longer dead, render bits 4,
/// selectable (stays a construction site). Return value 0.
pub fn on_wake(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(b) = building_mut(sim, id) {
        b.is_sleeping = false;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::DIED, false);
        o.render_bits = 4;
        o.selectable = true;
    }
    0
}

/// `PropertyBuilding_OnMsgExplode` (`0x0050A1B0`, `0x4203`): effect `explosion_pulverhaus.pef`, then
/// area damage with the dead building as source: without `CASTLEBUILDING` -100 to enemy units
/// (walk) within 5 tiles, with `CASTLEBUILDING` -250 to all objects of slots 0-7 within 3 tiles
/// (also own ones and buildings); `einschlag_boden.pef` per hit. Center distance in grid order. The
/// sound runs via the name (`SFX_BUILDINGEXPLODE`, host).
/// The impact carries the scale 0.5 of the original (`SimEvent::EffectAt`). Return value 0.
pub fn on_explode(sim: &mut Sim, id: ObjectId) -> i32 {
    sim.state.events.push(SimEvent::Effect {
        object: id,
        path: EXPLOSION_POWDER_HOUSE.to_string(),
    });
    let castle = has_flags(sim, id, flags::CASTLEBUILDING);
    let (amount, filter, radius) = if castle {
        (
            -250,
            MatchFilter {
                mode: 0x4_0000,
                ..MatchFilter::constructed()
            },
            3 * 4096,
        )
    } else {
        (
            -100,
            MatchFilter {
                mode: 0x414,
                prop_id: 0,
                ..MatchFilter::constructed()
            },
            5 * 4096,
        )
    };
    let pos = sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
    let hits =
        crate::collision::query_radius(sim, pos, &filter, radius, i32::from(id.slot()), false);
    for o in hits {
        if let Some(pos) = crate::combat::hit_zone::world_pos(sim, o) {
            sim.state.events.push(SimEvent::EffectAt {
                path: IMPACT_GROUND.to_string(),
                pos,
                yaw: 0.0,
                scale: 0.5,
                start_ms: 0,
            });
        }
        let msg = crate::combat::damage::damage_message(sim, id, amount);
        send(sim, o, &msg);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::{guid_count, pending_guid_count};
    use crate::messaging::send_mut;
    use crate::module::building::Building;
    use crate::path::BLOCKED;
    use crate::rules::{BuildingShape, Rules};
    use crate::state::SimState;
    use crate::world::{
        ObjectDesc, change_owner, create_object, desc_flags, process_destroy_queue,
    };

    /// Colonist (walk, `CANQUARTER`), HQ 1000 (quarters), house 1001, powder house 1004
    /// (`BUILDINGEXPLODE`), wall 1011 (palisade + corner), track 1012 (`TRACK`), palisade 1050,
    /// tower 1051.
    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nPROPID 23\nGUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 1004\nNAME Pulverhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 16777216\nGUID 1011\nNAME SPA_Mauer\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 3145728\nGUID 1012\nNAME Schiene\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 33554432\nGUID 1050\nNAME Palisade\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 1048576\nGUID 1051\nNAME Turm\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 65536\n";

    /// `default.dat` values: HQ 500 wood, 90 s, 4000 HP, quarters 5; house 150 wood, 20 s,
    /// 900 HP, living space 15; colonist 60 food, 15 s, population 1, 120 HP.
    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 64);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        r.defaults.insert(
            1100,
            set(&[(10, 60), (13, 15), (14, 120), (16, 183), (22, 1)]),
        );
        r.defaults
            .insert(1000, set(&[(11, 500), (13, 90), (14, 4000), (23, 5)]));
        r.defaults
            .insert(1001, set(&[(11, 150), (13, 20), (14, 900), (27, 15)]));
        r.defaults.insert(1004, set(&[(13, 20), (14, 900)]));
        r.defaults.insert(1011, set(&[(13, 6), (14, 1000)]));
        r.defaults.insert(1012, set(&[(13, 1), (14, 100)]));
        r.defaults.insert(1050, set(&[(13, 10), (14, 100)]));
        r.defaults.insert(1051, set(&[(13, 10), (14, 500)]));
        // Hull of the `SPA_Hauptquartier` with an exit in front of the building (quarters).
        let f = f32::from_bits;
        let mut hq = BuildingShape {
            center: [f(0x3F82_3B80), f(0x439B_AB3F), f(0x41FF_1998)],
            half: [f(0x4399_C662), f(0x439B_401D), f(0x4345_B4F9)],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        };
        hq.helper_nodes.insert("ausgang".into(), [0.0, 0.0, 600.0]);
        r.building_shapes.insert(1000, hq);
        r
    }

    fn place(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32), ready: bool) -> ObjectId {
        let world = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
        if ready {
            desc.flags |= desc_flags::PLACED;
        }
        create_object(sim, &mut desc).unwrap()
    }

    fn bld(sim: &Sim, id: ObjectId) -> Building {
        building(sim, id).unwrap().clone()
    }

    fn tick(sim: &mut Sim, id: ObjectId) {
        crate::module::tick(sim, id, INDEX, 100);
    }

    fn query(sim: &mut Sim, id: ObjectId, code: u32) -> i32 {
        send_mut(sim, id, &mut Message::new(code, &[]))
    }

    /// `0x1403` (`OnMsgRaiseLowHp`): the branch is dead, the HP stay, the answer is 0 (not
    /// -1 like an unknown code).
    // QUIRK C-132: kept
    #[test]
    fn the_dead_low_hitpoint_raise_answers_zero_and_changes_nothing() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        let hp = |sim: &Sim| sim.object(house).unwrap().attributes.current_value(14, 0);
        let max = hp(&sim);
        sim.object_mut(house)
            .unwrap()
            .attributes
            .add_to_current_value(14, 10 - max, 0);
        assert_eq!(hp(&sim), 10);
        assert_eq!(query(&mut sim, house, msg::RAISE_LOW_HP), 0);
        assert_eq!(hp(&sim), 10);
    }

    // QUIRK W-81: kept
    #[test]
    fn selling_a_site_refunds_one_percent_of_the_rest_value() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), false);
        let hq = place(&mut sim, 1000, 0, (40, 40), false);
        let wood = sim.state.players[0].resource_raw(resource::WOOD);
        // Sold immediately: 307 * 2000 / 100 = 6140 raw (quirk: 1 %, not 614,000).
        assert_eq!(query(&mut sim, house, msg::SELL), 0);
        assert_eq!(
            sim.state.players[0].resource_raw(resource::WOOD),
            wood + 6140
        );
        // HQ: 227 * 9000 / 100 = 20,430 raw.
        query(&mut sim, hq, msg::SELL);
        assert_eq!(
            sim.state.players[0].resource_raw(resource::WOOD),
            wood + 6140 + 20_430
        );
        // Partly built: only the remainder counts (remainder 1000 -> 3070).
        let half = place(&mut sim, 1001, 0, (20, 40), false);
        building_mut(&mut sim, half).unwrap().build_counter = 1000;
        let wood = sim.state.players[0].resource_raw(resource::WOOD);
        query(&mut sim, half, msg::SELL);
        assert_eq!(
            sim.state.players[0].resource_raw(resource::WOOD),
            wood + 3070
        );
        // Both construction sites dead, "in progress" restored, no object counter.
        assert!(bld(&sim, house).is_dead && !bld(&sim, house).is_build_building);
        assert_eq!(pending_guid_count(&sim, 0, 1001), 0);
        assert_eq!(guid_count(&sim, 0, 1001), 0);
    }

    // QUIRK W-81, D-248: kept
    #[test]
    fn selling_a_finished_house_refunds_nothing_and_takes_its_living_space() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        // Pre-placed: living space 15 booked (`GameObject_FinishPlacement`).
        let before = sim.state.players[0].resources_fx;
        assert_eq!(guid_count(&sim, 0, 1001), 1);
        sim.state.events.clear();
        assert_eq!(query(&mut sim, house, msg::SELL), 0);
        let after = sim.state.players[0].resources_fx;
        assert_eq!(after[..3], before[..3], "no refund");
        assert_eq!(after[4], before[4] - (15 << 12));
        assert_eq!(guid_count(&sim, 0, 1001), 0);
        let o = sim.object(house).unwrap();
        assert_ne!(o.status & status::DIED, 0);
        assert!(!o.selectable);
        assert_eq!(o.render_bits & 2, 0);
        let b = bld(&sim, house);
        assert!(b.is_dead && !b.is_dying && !b.heightmap_inserted);
        assert_eq!(
            sim.state.events,
            [
                SimEvent::BuildingDestroyed { object: house },
                SimEvent::Sound {
                    object: house,
                    sound: SFX_DESTROYED
                },
                SimEvent::Effect {
                    object: house,
                    path: EXPLOSION_BUILDING.into()
                }
            ]
        );
        // Timers in the order of the original (reversed in `m_TimersWS`), group 0x80.
        let timers: Vec<(i32, Message)> = sim
            .module(house, INDEX)
            .unwrap()
            .base
            .timers_ws
            .iter()
            .map(|e| (e.b, e.msg.clone()))
            .collect();
        assert_eq!(
            timers,
            [
                (COLLAPSE_DELAY, Message::new(msg::COLLAPSE, &[])),
                (100, Message::new(msg::CLEAR_FIRES, &[])),
                (1300, Message::new(msg::START_SINK, &[-200, 2000])),
                (100, Message::new(msg::START_SINK, &[-800, 300])),
            ]
        );
        assert!(
            sim.module(house, INDEX)
                .unwrap()
                .base
                .timers_ws
                .iter()
                .all(|e| e.group() == u32::from(GROUP))
        );
        // Second destroy: nothing more (already dead).
        sim.state.events.clear();
        assert_eq!(query(&mut sim, house, msg::DESTROY), 1);
        assert!(sim.state.events.is_empty());
        assert_eq!(sim.state.players[0].resources_fx, after);
        // Demolition: the destructor balances the -1 from `GameWorld_DestroyObject` (finished is
        // cleared after the collapse), living space is not deducted again.
        tick(&mut sim, house);
        for _ in 0..46 {
            tick(&mut sim, house);
        }
        process_destroy_queue(&mut sim);
        assert!(sim.object(house).is_none());
        assert_eq!(guid_count(&sim, 0, 1001), 0);
        assert_eq!(sim.state.players[0].resources_fx, after);
    }

    #[test]
    fn a_destroyed_building_frees_its_path_after_four_and_a_half_seconds_and_vanishes() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        assert_eq!(sim.state.path.layers[0].cell(20, 20), BLOCKED);
        assert_eq!(query(&mut sim, house, msg::DESTROY), 1);
        let group = sim
            .state
            .tick_groups
            .iter()
            .position(|g| g.contains(&house))
            .unwrap();
        // The first building tick takes over the timers, from the second on they count 100 ms per
        // tick; `0x3D03` (4500) fires at 4600 ms, i.e. in the 47th building tick (engine step
        // 2 * 47 - 1 + group); the destruction follows at the end of the phase 0 step (2 * 47).
        let free_step = 2 * 47 - 1 + group;
        let mut freed = None;
        let mut gone = None;
        for s in 1..=100 {
            crate::tick::step(&mut state, &rules, &[]);
            let sim = Sim::new(&mut state, &rules);
            if freed.is_none() && sim.state.path.layers[0].cell(20, 20) != BLOCKED {
                freed = Some(s);
                assert!(
                    sim.object(house)
                        .is_some_and(|o| o.status & status::DIED != 0)
                );
                assert!(!has_flags(&sim, house, flags::CONSTRUCTED_FINISHED));
            }
            if gone.is_none() && sim.object(house).is_none() {
                gone = Some(s);
            }
        }
        assert_eq!((freed, gone), (Some(free_step), Some(2 * 47)));
    }

    #[test]
    fn destroying_a_building_takes_only_its_own_tiles_out_of_the_tile_map() {
        let mut rules = rules();
        rules.building_shapes.insert(
            1001,
            BuildingShape {
                center: [0.0; 3],
                half: [300.0, 50.0, 100.0],
                free_nodes: Vec::new(),
                helper_nodes: Default::default(),
            },
        );
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        let other = place(&mut sim, 1001, 0, (40, 40), true);
        let tiles = |sim: &Sim, id: ObjectId| {
            sim.state
                .tile_map
                .occupied()
                .filter(|&(_, _, hid)| hid == id.as_i32())
                .count()
        };
        assert_eq!((tiles(&sim, house), tiles(&sim, other)), (5, 5));
        // The dying building stays in the object table, its tiles go immediately.
        assert_eq!(query(&mut sim, house, msg::DESTROY), 1);
        assert!(sim.object(house).is_some());
        assert_eq!((tiles(&sim, house), tiles(&sim, other)), (0, 5));
    }

    #[test]
    fn zero_hitpoints_destroy_a_building_through_the_death_check() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        sim.object_mut(house)
            .unwrap()
            .attributes
            .set_current_value(14, 0, 0);
        crate::tick::object_tick(&mut sim, house, 100);
        assert_ne!(sim.object(house).unwrap().status & status::DIED, 0);
        assert!(bld(&sim, house).is_dead);
        assert_eq!(guid_count(&sim, 0, 1001), 0);
    }

    #[test]
    fn destroying_a_site_returns_the_pending_count_and_keeps_the_type_count() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let site = place(&mut sim, 1001, 0, (20, 20), false);
        // A second construction site, demolished immediately: the destructor returns "in progress"
        // and balances the type counter.
        let other = place(&mut sim, 1001, 0, (30, 20), false);
        assert_eq!(pending_guid_count(&sim, 0, 1001), 2);
        crate::world::destroy_object(&mut sim, other);
        assert_eq!(pending_guid_count(&sim, 0, 1001), 1);
        assert_eq!(guid_count(&sim, 0, 1001), 0);
        // Destroy: "in progress" -1 immediately, object counter untouched.
        query(&mut sim, site, msg::DESTROY);
        assert_eq!(pending_guid_count(&sim, 0, 1001), 0);
        assert!(!bld(&sim, site).is_build_building);
        assert_eq!(guid_count(&sim, 0, 1001), 0);
        for _ in 0..47 {
            tick(&mut sim, site);
        }
        assert_eq!(sim.state.destroy_queue, [site]);
        process_destroy_queue(&mut sim);
        assert_eq!(pending_guid_count(&sim, 0, 1001), 0);
        assert_eq!(guid_count(&sim, 0, 1001), 0);
    }

    #[test]
    fn production_stops_on_destruction_and_the_destructor_refunds_the_rest() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place(&mut sim, 1000, 0, (20, 20), true);
        sim.state.players[0].resources_fx = [0, 0, 100 << 12, 0, 5 << 12];
        send(&mut sim, hq, &Message::new(msg::ENQUEUE, &[1100]));
        for _ in 0..51 {
            tick(&mut sim, hq);
        }
        assert_eq!(bld(&sim, hq).make_time, 10_000);
        query(&mut sim, hq, msg::DESTROY);
        // Dead: the tick drives only the timers, the production is halted.
        for _ in 0..46 {
            tick(&mut sim, hq);
        }
        assert_eq!(bld(&sim, hq).make_time, 10_000);
        assert_eq!(sim.state.resource(0, resource::FOOD), 40);
        tick(&mut sim, hq);
        process_destroy_queue(&mut sim);
        // 66 % of 60 food back, population -1.
        assert_eq!(sim.state.resource(0, resource::FOOD), 79);
        assert_eq!(sim.state.resource(0, resource::POPULATION), 0);
        assert_eq!(pending_guid_count(&sim, 0, 1100), 0);
    }

    #[test]
    fn a_destroyed_quarter_releases_its_occupants() {
        let rules = rules();
        let mut state = SimState::new(0, 3);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place(&mut sim, 1000, 0, (8, 16), true);
        let units: Vec<ObjectId> = (2..5)
            .map(|x| place(&mut sim, 1100, 0, (x, 3), true))
            .collect();
        for &u in &units {
            send(
                &mut sim,
                hq,
                &Message::new(quarter::msg::ENTER, &[u.as_i32()]),
            );
        }
        let q = |sim: &Sim| match &sim.module(hq, quarter::INDEX).unwrap().kind {
            crate::module::ModuleKind::Quarter(q) => q.cur,
            _ => unreachable!(),
        };
        assert_eq!(q(&sim), 3);
        let start = sim.state.rng.index();
        query(&mut sim, hq, msg::DESTROY);
        // One table random number per occupant, all outside.
        assert_eq!(
            usize::from(sim.state.rng.index()),
            (usize::from(start) + 3) % 1000
        );
        assert_eq!(q(&sim), 0);
        for &u in &units {
            assert_eq!(sim.object(u).unwrap().status & status::QUARTERED, 0);
        }
    }

    // QUIRK D-248: kept
    #[test]
    fn exploding_buildings_set_a_timer_for_the_blast() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let powder = place(&mut sim, 1004, 0, (20, 20), true);
        assert!(bld(&sim, powder).is_explode);
        sim.state.events.clear();
        query(&mut sim, powder, msg::DESTROY);
        // No effect immediately, only smoke (`Building_SpawnDestructionSmoke` comes before the query
        // `+0x10C`) and sound; `0x4203` after 700 ms before the other timers.
        assert_eq!(
            sim.state.events,
            [
                SimEvent::BuildingDestroyed { object: powder },
                SimEvent::Sound {
                    object: powder,
                    sound: SFX_DESTROYED
                }
            ]
        );
        let timers = &sim.module(powder, INDEX).unwrap().base.timers_ws;
        let last = timers.back().unwrap();
        assert_eq!(
            (last.b, &last.msg),
            (EXPLODE_DELAY, &Message::new(msg::EXPLODE, &[]))
        );
        sim.state.events.clear();
        // Takeover, then not at 700 ms, yes at 800 ms: in the 9th building tick.
        for _ in 0..8 {
            tick(&mut sim, powder);
        }
        assert!(sim.state.events.is_empty());
        tick(&mut sim, powder);
        assert_eq!(
            sim.state.events,
            [SimEvent::Effect {
                object: powder,
                path: EXPLOSION_POWDER_HOUSE.into()
            }]
        );
    }

    // QUIRK C-155: kept
    #[test]
    fn the_powder_house_blasts_5_tiles_at_enemies_or_3_tiles_at_everyone() {
        use crate::combat::testkit as ck;
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let p = ck::spawn(&mut sim, ck::POWDER, 0, (20, 20));
        let near = ck::spawn(&mut sim, ck::SABRE, 1, (23, 20)); // 3 tiles
        let far = ck::spawn(&mut sim, ck::SABRE, 1, (26, 20)); // 6 tiles
        let own = ck::spawn(&mut sim, ck::SABRE, 0, (20, 22));
        let hq = ck::spawn(&mut sim, ck::HQ, 1, (20, 16));
        query(&mut sim, p, msg::DESTROY);
        sim.state.events.clear();
        query(&mut sim, p, msg::EXPLODE);
        // One impact per hit with scale 0.5 at the hit position (re/combat-host 6).
        let near_pos = crate::combat::hit_zone::world_pos(&sim, near).unwrap();
        let impacts: Vec<_> = sim
            .state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::EffectAt {
                    path, pos, scale, ..
                } if path == IMPACT_GROUND => Some((*pos, *scale)),
                _ => None,
            })
            .collect();
        assert_eq!(impacts, vec![(near_pos, 0.5)]);
        let f = |sim: &Sim, id| sim.object(id).unwrap().attributes.current_fraction(14);
        assert_eq!(f(&sim, near), 100_000 - 52_631, "-100 HP of 190");
        assert_eq!(
            (f(&sim, far), f(&sim, own), f(&sim, hq)),
            (100_000, 100_000, 100_000),
            "only enemy units within 5 tiles"
        );
        // Castle powder house: -250 to all objects of slots 0-7 within 3 tiles, also own ones and buildings.
        let c = ck::spawn(&mut sim, ck::CASTLE_POWDER, 0, (40, 40));
        let own2 = ck::spawn(&mut sim, ck::SABRE, 0, (40, 42));
        let hq2 = ck::spawn(&mut sim, ck::HQ, 1, (42, 40));
        query(&mut sim, c, msg::DESTROY);
        query(&mut sim, c, msg::EXPLODE);
        // Stored raw fraction (`current_fraction` clamps to >= 0).
        assert!(
            sim.object(own2).unwrap().attributes.raw_fraction(14) < 0,
            "own unit -250 HP"
        );
        assert_eq!(
            f(&sim, hq2),
            100_000 - 5000,
            "building -250 * 0.8 (UNIT_BUILDING, source dead = not NOTHUMAN)"
        );
    }

    // QUIRK W-85, W-276: kept
    #[test]
    fn a_destroyed_wall_link_becomes_a_site_again_while_the_wall_lives() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let master = place(&mut sim, 1011, 0, (20, 20), true);
        let a = place(&mut sim, 1011, 0, (21, 20), true);
        let b = place(&mut sim, 1011, 0, (22, 20), true);
        let list = Message::new(msg::SET_SLAVE_LIST, &[2, a.as_i32(), b.as_i32()]);
        send(&mut sim, master, &list);
        assert_eq!(guid_count(&sim, 0, 1011), 3);
        query(&mut sim, a, msg::DESTROY);
        assert_eq!(guid_count(&sim, 0, 1011), 2);
        for _ in 0..47 {
            tick(&mut sim, a);
        }
        // Collapsed, but the group lives (master 0x1E03 -> 1): not destroyed, path free.
        assert!(sim.state.destroy_queue.is_empty());
        assert_eq!(sim.state.path.layers[0].cell(21, 20), 0);
        assert!(!has_flags(&sim, a, flags::CONSTRUCTED_FINISHED));
        // `0x103` after 1000 ms (counted from the next tick, fires at 1100 ms).
        for _ in 0..10 {
            tick(&mut sim, a);
        }
        assert!(bld(&sim, a).is_dead);
        tick(&mut sim, a);
        let site = bld(&sim, a);
        assert!(!site.is_dead && site.is_build_building);
        assert_eq!(site.build_counter, 600);
        let o = sim.object(a).unwrap();
        assert_eq!(o.status & status::DIED, 0);
        assert_eq!((o.render_bits, o.selectable), (4, true));
        assert_eq!(o.attributes.current_value(14, 0), 1);
        assert_eq!(pending_guid_count(&sim, 0, 1011), 1);
        assert_eq!(bld(&sim, master).slaves, [a, b]);
        // If the group is dead (all HP shares 0), the collapse of the last member tears all down.
        for id in [master, a] {
            sim.object_mut(id)
                .unwrap()
                .attributes
                .set_current_fraction(14, 0);
        }
        sim.object_mut(b)
            .unwrap()
            .attributes
            .set_current_fraction(14, 0);
        query(&mut sim, b, msg::DESTROY);
        for _ in 0..47 {
            tick(&mut sim, b);
        }
        assert_eq!(sim.state.destroy_queue, [a, b, master]);
        assert!(bld(&sim, master).slaves.is_empty());
    }

    #[test]
    fn rails_stay_after_the_collapse_and_turn_into_a_site() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let rail = place(&mut sim, 1012, 0, (20, 20), true);
        query(&mut sim, rail, msg::DESTROY);
        for _ in 0..58 {
            tick(&mut sim, rail);
        }
        assert!(sim.state.destroy_queue.is_empty());
        assert!(bld(&sim, rail).is_build_building && !bld(&sim, rail).is_dead);
    }

    #[test]
    fn a_fallen_tower_wakes_the_palisade_below() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let palisade = place(&mut sim, 1050, 0, (20, 20), true);
        let tower = place(&mut sim, 1051, 0, (20, 20), true);
        assert!(bld(&sim, palisade).is_sleeping);
        query(&mut sim, tower, msg::DESTROY);
        for _ in 0..47 {
            tick(&mut sim, tower);
        }
        assert_eq!(sim.state.destroy_queue, [tower]);
        let b = bld(&sim, palisade);
        assert!(!b.is_sleeping && b.is_build_building);
        let o = sim.object(palisade).unwrap();
        assert_eq!(o.status & status::DIED, 0);
        assert_eq!((o.render_bits, o.selectable), (4, true));
        // Directly: 0x2003 returns 0.
        assert_eq!(query(&mut sim, palisade, msg::WAKE), 0);
    }

    // QUIRK W-87: kept
    #[test]
    fn pending_counts_stay_with_the_old_slot_after_an_owner_change() {
        // Quirk W-87 (re/buildings 8.6, 12.17): `GameWorld_ChangeObjectOwner` rebooks only the object
        // counter; "in progress" of construction site and production stays with the old slot, later
        // bookings hit the new one.
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let site = place(&mut sim, 1001, 0, (20, 20), false);
        let hq = place(&mut sim, 1000, 0, (40, 40), true);
        assert_eq!(pending_guid_count(&sim, 0, 1001), 1);
        for _ in 0..2 {
            assert_eq!(send(&mut sim, hq, &Message::new(msg::ENQUEUE, &[1100])), 1);
        }
        assert_eq!(pending_guid_count(&sim, 0, 1100), 2);
        change_owner(&mut sim, site, 1).unwrap();
        let new_hq = change_owner(&mut sim, hq, 1).unwrap();
        assert_eq!(
            [1001, 1100].map(|g| (
                pending_guid_count(&sim, 0, g),
                pending_guid_count(&sim, 1, g)
            )),
            [(1, 0), (2, 0)]
        );
        // Cancel of the running order at the new HQ: -1 at the new slot.
        assert_eq!(send(&mut sim, new_hq, &Message::new(msg::CANCEL, &[0])), 1);
        assert_eq!(
            (
                pending_guid_count(&sim, 0, 1100),
                pending_guid_count(&sim, 1, 1100)
            ),
            (2, -1)
        );
    }

    // QUIRK W-86: kept
    #[test]
    fn owner_changes_hand_the_new_id_to_the_wall_links() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let master = place(&mut sim, 1011, 0, (20, 20), true);
        let a = place(&mut sim, 1011, 0, (21, 20), true);
        let b = place(&mut sim, 1011, 0, (22, 20), true);
        send(
            &mut sim,
            master,
            &Message::new(msg::SET_SLAVE_LIST, &[2, a.as_i32(), b.as_i32()]),
        );
        let new_master = change_owner(&mut sim, master, 1).unwrap();
        // Members: new master; the master forgets them (quirk). The farm workers (`0xA0C {new}` to
        // module 12) are checked by the harvest tests (gathering).
        assert_eq!(bld(&sim, a).master, new_master);
        assert_eq!(bld(&sim, b).master, new_master);
        assert!(bld(&sim, new_master).slaves.is_empty());
        // A member registers with the master under the new ID.
        let new_a = change_owner(&mut sim, a, 1).unwrap();
        assert_eq!(bld(&sim, new_master).slaves, [new_a]);
    }

    #[test]
    fn destroyed_building_sets_bit_2() {
        // `PropertyBuilding_Destroy` `0x00506344`: bit 2 after the `DIED` check, no change of
        // owner; already dead -> nothing.
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        destroy(&mut sim, house);
        assert_eq!(crate::world::object_event_bits(&sim, house), 1 << 2);
        assert!(sim.state.owner_changes.is_empty());
        crate::world::set_object_event_bit(&mut sim, house, 2, false);
        destroy(&mut sim, house);
        assert_eq!(crate::world::object_event_bits(&sim, house), 0);
    }

    // QUIRK E-189: kept
    #[test]
    fn building_destroy_counts_even_unfinished_sites() {
        // `0x00506445`: kill counter for the slot of the last attacker with the building GUID,
        // unconditional (finished, construction site, palisade); discarded without an attacker.
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let attacker = ObjectId::new(7, 1, 1);
        let finished = place(&mut sim, 1001, 0, (20, 20), true);
        let site = place(&mut sim, 1001, 0, (30, 20), false);
        let palisade = place(&mut sim, 1050, 0, (40, 20), true);
        let unattacked = place(&mut sim, 1001, 0, (20, 40), true);
        for b in [finished, site, palisade] {
            sim.object_mut(b).unwrap().last_attacker = attacker;
        }
        for b in [finished, site, palisade, unattacked] {
            destroy(&mut sim, b);
        }
        assert_eq!(crate::world::kill_count(&sim, 1, 1001), 2);
        assert_eq!(crate::world::kill_count(&sim, 1, 1050), 1);
        assert!(sim.state.kill_counts[0].is_empty());
        // Second `Destroy` (already dead): not again.
        destroy(&mut sim, finished);
        assert_eq!(crate::world::kill_count(&sim, 1, 1001), 2);
    }
}
