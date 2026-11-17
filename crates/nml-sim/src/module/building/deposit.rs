//! Deposit of a mine or a field (re/buildings, re/gathering): the building remembers the nearest
//! gold deposit (3500) or the nearest fertile soil (3503) in `m_Target`, passes withdrawals by
//! gatherers (`0x303`/`0xA03`) on to the deposit, reports "deposit exhausted" once and mirrors its
//! amount in the building tick into its own attribute block.

use nml_formats::targets::MatchFilter;

use crate::collision::{edge_distance_to_object, find_nearest_matching, fixed_pos_distance};
use crate::economy::{announce, message};
use crate::id::{NATURE_SLOT, ObjectId};
use crate::message::Message;
use crate::messaging::send;
use crate::module::resource;
use crate::sim::Sim;

use super::{building, building_mut, flags, has_flags};

/// GUID of the gold deposit.
pub const GOLD_DEPOSIT: u32 = 3500;
/// GUID of fertile soil.
pub const FERTILE_SOIL: u32 = 3503;
/// A deposit counts only below this distance (`Fixed_ToInt`, whole tiles).
const MAX_TILES: i32 = 10;

/// Filter of the deposit search on the stack (part of `MatchFilter_Construct`: words 0xA-0xD and
/// 0x11 = -1, byte 0x13 = 0; the remaining words are partly uninitialized in the original and have
/// no effect with `CHECKGUID` alone).
fn deposit_filter(guid: u32) -> MatchFilter {
    MatchFilter {
        mode: 0x8,
        guid: guid as i32,
        ..MatchFilter::constructed()
    }
}

/// `m_Target` is an existing object (`GameWorld_GetObjectById(target, 0)`; ID 0 hits none).
fn current_target(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let t = building(sim, id)?.target;
    sim.object(t).map(|o| o.id)
}

fn set_target(sim: &mut Sim, id: ObjectId, target: ObjectId) {
    if let Some(b) = building_mut(sim, id) {
        b.target = target;
    }
}

/// `PropertyBuilding_OnGetGold` (`0x005056B0`, `0x303 {n}`) or `PropertyBuilding_OnGetFood`
/// (`0x00505890`, `0xA03 {n}`): if the deposit is missing, search for the nearest one on the whole map
/// (player 10; gold with edge distance, edge offset = own hard radius). None -> 1 without
/// change; distance `>> 12 ≥ 10` (gold: edge distance of the deposit to the mine,
/// `GameObject_GetEdgeDistanceToObject`; food: center distance) -> `m_Target = 0` and 1 -
/// **quirk:** without a nearby deposit every withdrawal returns 1, without limit. Otherwise
/// `0x106`/`0x206` to the deposit; if the amount differs from the request, once per building
/// message 5 (gold) or 4 (food) to the owner (`m_NoResourcenAnnounced`). The original reports only to
/// the local player and sets the flag for food only then; here for every owner.
/// The building itself is not checked (each one answers `0x303`/`0xA03`).
pub fn on_get_resource(sim: &mut Sim, id: ObjectId, want: i32, gold: bool) -> i32 {
    let Some(o) = sim.object(id) else { return -1 };
    let (pos, hard, tile) = ((o.grid.x.0, o.grid.z.0), o.hard_radius, o.tile);
    let deposit = match current_target(sim, id) {
        Some(d) => d,
        None => {
            let guid = if gold { GOLD_DEPOSIT } else { FERTILE_SOIL };
            let edge = if gold { hard } else { -1 };
            let filter = deposit_filter(guid);
            let player = i32::from(NATURE_SLOT);
            let Some(d) = find_nearest_matching(sim, pos, &filter, -1, player, edge) else {
                return 1;
            };
            let dist = if gold {
                edge_distance_to_object(sim, d, id)
            } else {
                let c = sim.object(d).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
                fixed_pos_distance(c, pos)
            };
            if dist >> 12 >= MAX_TILES {
                set_target(sim, id, ObjectId(0));
                return 1;
            }
            set_target(sim, id, d);
            d
        }
    };
    let code = if gold {
        resource::msg::TAKE_GOLD
    } else {
        resource::msg::TAKE_FOOD
    };
    let got = send(sim, deposit, &Message::new(code, &[want]));
    if got != want && building(sim, id).is_some_and(|b| !b.no_resources_announced) {
        let text = if gold {
            message::GOLD_EMPTY
        } else {
            message::FIELD_EMPTY
        };
        announce(
            sim,
            i32::from(id.slot()),
            text,
            tile,
            message::flags::DEPOSIT,
        );
        if let Some(b) = building_mut(sim, id) {
            b.no_resources_announced = true;
        }
    }
    got
}

/// `PropertyBuilding_OnMsgFindDeposit` (`0x005070A0`, `0x3A03`, timer from `Init`): only without an
/// existing deposit; a field (checked first) searches soil, a gold mine gold, otherwise nothing.
/// Search on the whole map (player 10) **with center distance, also for gold** (unlike `0x303`);
/// found and `>> 12 < 10` -> `m_Target`, found and farther -> 0, not found -> unchanged.
/// Return value 1.
pub fn on_find_deposit(sim: &mut Sim, id: ObjectId) -> i32 {
    if current_target(sim, id).is_some() {
        return 1;
    }
    let guid = if has_flags(sim, id, flags::FARM) {
        FERTILE_SOIL
    } else if has_flags(sim, id, flags::GOLDMINE) {
        GOLD_DEPOSIT
    } else {
        return 1;
    };
    let Some(pos) = sim.object(id).map(|o| (o.grid.x.0, o.grid.z.0)) else {
        return 1;
    };
    let filter = deposit_filter(guid);
    let Some(d) = find_nearest_matching(sim, pos, &filter, -1, i32::from(NATURE_SLOT), -1) else {
        return 1;
    };
    let c = sim.object(d).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
    let target = if fixed_pos_distance(c, pos) >> 12 < MAX_TILES {
        d
    } else {
        ObjectId(0)
    };
    set_target(sim, id, target);
    1
}

/// Deposit mirror in the building tick (`PropertyBuilding_Tick` `0x0050CD30`, before the branches
/// "dead"/"finished", so also at the construction site if the flag is set): a gold mine or field
/// with an existing `m_Target` takes over its attribute 26 or 24 (`AttributeSet_CopyAttribute`
/// `0x0048FDB0`; the gold mine wins if both flags are set).
pub fn mirror_deposit(sim: &mut Sim, id: ObjectId) {
    let gold = has_flags(sim, id, flags::GOLDMINE);
    if !gold && !has_flags(sim, id, flags::FARM) {
        return;
    }
    let Some(t) = current_target(sim, id) else {
        return;
    };
    let attr = if gold {
        resource::GOLD_AMOUNT
    } else {
        resource::FOOD_AMOUNT
    };
    let Some(src) = sim.object(t).map(|o| o.attributes.clone()) else {
        return;
    };
    if let Some(o) = sim.object_mut(id) {
        o.attributes.copy_attribute(attr, &src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::send_mut;
    use crate::module::building::{INDEX, msg};
    use crate::rules::Rules;
    use crate::state::{SimEvent, SimState};
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// Mine (`GOLDMINE` 0x100) and field (`FARM` 0x200) without a model hull: rectangle = own
    /// tile; plus gold (0x500) and soil (0x600), category nature (slot 10).
    const TYPES: &str = "GUID 1010\nNAME Mine\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 256\nGUID 1003\nNAME Feld\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 512\nGUID 3500\nNAME Goldvorkommen\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1280\nGUID 3503\nNAME Boden\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1536\n";

    fn rules() -> Rules {
        Rules::synthetic(TYPES, 64)
    }

    fn tile_pos(tile: (f32, f32)) -> [f32; 3] {
        [(tile.0 + 0.5) * 128.0, 0.0, (tile.1 + 0.5) * 128.0]
    }

    fn building_at(sim: &mut Sim, guid: u32, tile: (f32, f32)) -> ObjectId {
        let mut desc = ObjectDesc::new(guid, 0, tile_pos(tile));
        desc.flags |= desc_flags::PLACED;
        create_object(sim, &mut desc).unwrap()
    }

    fn deposit_at(sim: &mut Sim, guid: u32, cfg: u32, amount: i32, tile: (f32, f32)) -> ObjectId {
        sim.state.object_config = [(cfg, amount)].into_iter().collect();
        let id = create_object(sim, &mut ObjectDesc::new(guid, 10, tile_pos(tile))).unwrap();
        sim.state.object_config.clear();
        id
    }

    fn get(sim: &mut Sim, id: ObjectId, code: u32, n: i32) -> i32 {
        send_mut(sim, id, &mut Message::new(code, &[n]))
    }

    fn target(sim: &Sim, id: ObjectId) -> ObjectId {
        building(sim, id).unwrap().target
    }

    /// Messages with receiver and flag word. `OnGetGold`/`OnGetFood`: priority 3, enqueue.
    fn messages(sim: &Sim) -> Vec<(u8, i32)> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::PlayerMessage {
                    to: Some(player),
                    id,
                    flags,
                    ..
                } => {
                    assert_eq!(*flags, 0x0301_0000);
                    Some((*player, *id))
                }
                _ => None,
            })
            .collect()
    }

    // QUIRK W-45: kept
    #[test]
    fn without_a_deposit_every_take_yields_one_forever() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mine = building_at(&mut sim, 1010, (20.0, 20.0));
        assert_eq!(target(&sim, mine), ObjectId(0));
        // No deposit on the map: 1, target unchanged, no message.
        for _ in 0..3 {
            assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 1), 1);
            assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 5), 1);
        }
        assert_eq!(target(&sim, mine), ObjectId(0));
        assert!(messages(&sim).is_empty());
        // A deposit too far away (edge distance >= 10 tiles): target 0, still 1.
        let far = deposit_at(&mut sim, 3500, 26, 100, (20.0, 40.0));
        assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 1), 1);
        assert_eq!(target(&sim, mine), ObjectId(0));
        let amount = |sim: &Sim| sim.object(far).unwrap().attributes.current_value(26, 0);
        assert_eq!(amount(&sim), 100);
    }

    // QUIRK W-46: kept
    #[test]
    fn gold_is_accepted_by_edge_distance_and_the_empty_message_comes_once() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mine = building_at(&mut sim, 1010, (20.0, 20.0));
        // Centers 10 tiles apart, edge of the mine tile at z = 21: edge distance
        // 9.5 tiles -> accepted (via the center it would be rejected).
        let gold = deposit_at(&mut sim, 3500, 26, 3, (20.0, 30.0));
        assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 2), 2);
        assert_eq!(target(&sim, mine), gold);
        assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 2), 1);
        assert_eq!(messages(&sim), [(0, message::GOLD_EMPTY)]);
        assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 2), 0);
        assert_eq!(messages(&sim).len(), 1);
        assert!(building(&sim, mine).unwrap().no_resources_announced);
        // A field does not take gold: `0xA03` searches soil.
        let field = building_at(&mut sim, 1003, (40.0, 20.0));
        assert_eq!(get(&mut sim, field, msg::GET_FOOD, 1), 1);
        assert_eq!(target(&sim, field), ObjectId(0));
    }

    #[test]
    fn food_uses_the_centre_distance_and_rejects_exactly_ten_tiles() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let field = building_at(&mut sim, 1003, (20.0, 20.0));
        let soil = deposit_at(&mut sim, 3503, 24, 50, (20.0, 30.0));
        // Exactly 10 tiles (40960 fixed point) center distance: rejected.
        assert_eq!(get(&mut sim, field, msg::GET_FOOD, 4), 1);
        assert_eq!(target(&sim, field), ObjectId(0));
        assert_eq!(
            sim.object(soil).unwrap().attributes.current_value(24, 0),
            50
        );
        // Closer (9 tiles): accepted, the withdrawal goes to the soil.
        let near = deposit_at(&mut sim, 3503, 24, 50, (29.0, 20.0));
        assert_eq!(get(&mut sim, field, msg::GET_FOOD, 4), 4);
        assert_eq!(target(&sim, field), near);
        assert_eq!(
            sim.object(near).unwrap().attributes.current_value(24, 0),
            46
        );
    }

    // QUIRK W-46: kept
    #[test]
    fn the_init_timer_finds_the_deposit_by_centre_distance_and_the_tick_mirrors_it() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gold = deposit_at(&mut sim, 3500, 26, 5000, (20.0, 29.0));
        deposit_at(&mut sim, 3500, 26, 5000, (50.0, 20.0));
        let mine = building_at(&mut sim, 1010, (20.0, 20.0));
        let lone = building_at(&mut sim, 1010, (50.0, 40.0));
        // Timer `0x3A03` after 100 ms: take over, then it fires in the second tick after that.
        for _ in 0..2 {
            crate::module::tick(&mut sim, mine, INDEX, 100);
            crate::module::tick(&mut sim, lone, INDEX, 100);
        }
        assert_eq!(target(&sim, mine), ObjectId(0));
        crate::module::tick(&mut sim, mine, INDEX, 100);
        crate::module::tick(&mut sim, lone, INDEX, 100);
        assert_eq!(target(&sim, mine), gold);
        // The nearest one is 20 tiles away: found, but too far -> 0.
        assert_eq!(target(&sim, lone), ObjectId(0));
        // The mirror takes over the amount (in the same tick, after the timers).
        let attr = |sim: &Sim, id| sim.object(id).unwrap().attributes.current_value(26, 0);
        assert_eq!(attr(&sim, mine), 5000);
        assert_eq!(get(&mut sim, mine, msg::GET_GOLD, 20), 20);
        assert_eq!(attr(&sim, mine), 5000);
        crate::module::tick(&mut sim, mine, INDEX, 100);
        assert_eq!(attr(&sim, mine), 4980);
        // With a target `0x3A03` does not search again.
        assert_eq!(
            send_mut(&mut sim, mine, &mut Message::new(msg::FIND_DEPOSIT, &[])),
            1
        );
        assert_eq!(target(&sim, mine), gold);
    }
}
