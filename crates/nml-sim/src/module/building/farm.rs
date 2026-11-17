//! Work slots of a farm (re/gathering, re/buildings): five slots `m_RegisteredHandles` at the helper
//! nodes `farmer01…05` of the model, assigned round-robin via `m_RequestPos`, not occupied until
//! arrival (`0x803`), freed only on abort (`0x903`).

use crate::footprint::helper_node_map_position;
use crate::id::ObjectId;
use crate::message::Message;
use crate::sim::Sim;

use super::{Building, FARM_SLOTS, building_mut, set_building_animation};

/// Slot index in range (the original does not check the bounds and reads or writes behind the
/// field - exception A7: here "no slot").
fn slot_index(idx: i32) -> Option<usize> {
    usize::try_from(idx).ok().filter(|&i| i < FARM_SLOTS)
}

/// Slot free or already this ID.
fn usable(slot: ObjectId, hid: ObjectId) -> bool {
    slot.is_none() || slot == hid
}

/// `PropertyBuilding_OnGetFarmerPos` (`0x00509440`, `0x503 {k, …}`): helper node `farmer%02d`
/// with `k + 1` (`GameObject_GetHelperNodeMapPosition`), grid position in `args[1..4]` = x, y, z,
/// return value 0; without a node -1.
pub fn on_get_farmer_pos(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let name = format!("farmer{:02}", m.arg(0).wrapping_add(1));
    let Some(p) = helper_node_map_position(sim, id, &name) else {
        return -1;
    };
    m.set_arg(1, p.x.0);
    m.set_arg(2, p.y.0);
    m.set_arg(3, p.z.0);
    0
}

/// `PropertyBuilding_OnFarmQuerySlot` (`0x00504970`, `0x603 {hid, idx}`), query only: `idx = −1`
/// -> round-robin from `m_RequestPos + 1` (the pointer advances with every attempt, even without
/// success), first free or own slot; `−2` -> first free or own one from the front; otherwise
/// exactly this slot. Return value slot or -1.
pub fn on_query_slot(b: &mut Building, hid: ObjectId, idx: i32) -> i32 {
    match idx {
        -1 => {
            for _ in 0..FARM_SLOTS {
                b.request_pos += 1;
                if b.request_pos >= FARM_SLOTS as i32 {
                    b.request_pos = 0;
                }
                if usable(b.registered_handles[b.request_pos as usize], hid) {
                    return b.request_pos;
                }
            }
            -1
        }
        -2 => b
            .registered_handles
            .iter()
            .position(|&s| usable(s, hid))
            .map_or(-1, |i| i as i32),
        _ => match slot_index(idx) {
            Some(i) if usable(b.registered_handles[i], hid) => idx,
            _ => -1,
        },
    }
}

/// `PropertyBuilding_OnFarmClaimSlot` (`0x00504A10`, `0x803 {hid, idx}`): occupy the slot if free
/// or own; the first occupied slot lets the field "grow" (`m_growed`, building animation 0,
/// never back). Return value slot or -1.
pub fn on_claim_slot(sim: &mut Sim, id: ObjectId, hid: ObjectId, idx: i32) -> i32 {
    let Some(b) = building_mut(sim, id) else {
        return -1;
    };
    let Some(i) = slot_index(idx).filter(|&i| usable(b.registered_handles[i], hid)) else {
        return -1;
    };
    b.registered_handles[i] = hid;
    if !b.growed {
        b.growed = true;
        set_building_animation(sim, id, 0);
    }
    idx
}

/// `PropertyBuilding_OnFarmReleaseSlot` (`0x00504A70`, `0x903 {hid}`): free the first slot of this
/// ID; return value slot or -1.
pub fn on_release_slot(b: &mut Building, hid: ObjectId) -> i32 {
    match b.registered_handles.iter().position(|&s| s == hid) {
        Some(i) => {
            b.registered_handles[i] = ObjectId::NONE;
            i as i32
        }
        None => -1,
    }
}

/// `PropertyBuilding_OnReplaceRegisteredHandle` (`0x005050A0`, `0x3F03 {old, new}`, change of owner
/// of a farmer): check all five slots and rewrite them. Return value 0.
pub fn on_replace_handle(b: &mut Building, old: ObjectId, new: ObjectId) -> i32 {
    for s in &mut b.registered_handles {
        if *s == old {
            *s = new;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::super::{building, msg, tests::spa_hq};
    use super::*;
    use crate::footprint::local_offset_to_grid;
    use crate::fx::{Fx, FxVec3};
    use crate::messaging::send_mut;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    const TYPES: &str =
        "GUID 1003\nNAME Feld\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 512\n";

    fn farm(sim: &mut Sim) -> ObjectId {
        let mut desc = ObjectDesc::new(1003, 0, [1000.0, 0.0, 2000.0]);
        desc.flags |= desc_flags::PLACED;
        create_object(sim, &mut desc).unwrap()
    }

    fn ask(sim: &mut Sim, id: ObjectId, code: u32, args: &[i32]) -> i32 {
        send_mut(sim, id, &mut Message::new(code, args))
    }

    #[test]
    fn slots_go_round_robin_and_stay_until_released() {
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let f = farm(&mut sim);
        let hid = |n: i32| 0x1_0000 * n + 1;
        // Seven farmers ask without occupying: round-robin, the pointer then stands at 1.
        let asked: Vec<i32> = (1..=7)
            .map(|n| ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(n), -1]))
            .collect();
        assert_eq!(asked, [0, 1, 2, 3, 4, 0, 1]);
        assert_eq!(building(&sim, f).unwrap().request_pos, 1);
        // Ask and occupy: the remaining slots, then full (the pointer advances).
        let mut got = Vec::new();
        for n in 1..=7 {
            let i = ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(n), -1]);
            if i >= 0 {
                assert_eq!(ask(&mut sim, f, msg::FARM_CLAIM_SLOT, &[hid(n), i]), i);
            }
            got.push(i);
        }
        assert_eq!(got, [2, 3, 4, 0, 1, -1, -1]);
        // Own slot stays reachable; `−2` = first free or own.
        assert_eq!(ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(4), -2]), 0);
        assert_eq!(ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(9), -2]), -1);
        // Slot k: free or own, otherwise -1; outside 0...4 -1 (exception A7).
        assert_eq!(ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(3), 4]), 4);
        assert_eq!(ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(9), 4]), -1);
        for idx in [5, -3, i32::MAX] {
            assert_eq!(ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(9), idx]), -1);
            assert_eq!(ask(&mut sim, f, msg::FARM_CLAIM_SLOT, &[hid(9), idx]), -1);
        }
        assert_eq!(ask(&mut sim, f, msg::FARM_CLAIM_SLOT, &[hid(9), 2]), -1);
        // Release of slot 2 (farmer 1): the next one gets slot 2.
        assert_eq!(ask(&mut sim, f, msg::FARM_RELEASE_SLOT, &[hid(1)]), 2);
        assert_eq!(ask(&mut sim, f, msg::FARM_RELEASE_SLOT, &[hid(1)]), -1);
        assert_eq!(ask(&mut sim, f, msg::FARM_QUERY_SLOT, &[hid(8), -1]), 2);
        // Change of owner: rewrite all entries of the old ID.
        assert_eq!(ask(&mut sim, f, msg::REPLACE_HANDLE, &[hid(2), hid(8)]), 0);
        let h = building(&sim, f).unwrap().registered_handles;
        assert_eq!(h[3], ObjectId(hid(8) as u32));
        assert!(!h.contains(&ObjectId(hid(2) as u32)));
    }

    #[test]
    fn the_first_claim_lets_the_field_grow_once() {
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let f = farm(&mut sim);
        assert!(!building(&sim, f).unwrap().growed);
        sim.object_mut(f).unwrap().anim.wanted = 7;
        assert_eq!(ask(&mut sim, f, msg::FARM_CLAIM_SLOT, &[0x1_0001, 3]), 3);
        assert!(building(&sim, f).unwrap().growed);
        assert_eq!(sim.object(f).unwrap().anim.wanted, 0);
        // Only the first time; releasing does not take it back.
        sim.object_mut(f).unwrap().anim.wanted = 7;
        ask(&mut sim, f, msg::FARM_RELEASE_SLOT, &[0x1_0001]);
        assert_eq!(ask(&mut sim, f, msg::FARM_CLAIM_SLOT, &[0x2_0001, 1]), 1);
        assert_eq!(sim.object(f).unwrap().anim.wanted, 7);
        assert!(building(&sim, f).unwrap().growed);
    }

    #[test]
    fn farmer_nodes_are_the_numbered_helper_nodes() {
        let mut rules = Rules::synthetic(TYPES, 32);
        let mut shape = spa_hq();
        shape
            .helper_nodes
            .insert("farmer02".into(), [64.0, 16.0, -32.0]);
        rules.building_shapes.insert(1003, shape);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let f = farm(&mut sim);
        let o = sim.object(f).unwrap();
        let (g, dir) = (o.grid, o.direction);
        let (x, z) = local_offset_to_grid([64.0, 16.0, -32.0], g, dir, [128; 3]);
        let y = (16 << 12) / 128 + g.y.0;
        let mut m = Message::new(msg::FARMER_POS, &[1, -1, -1, -1]);
        assert_eq!(send_mut(&mut sim, f, &mut m), 0);
        assert_eq!(m.args, [1, x, y, z]);
        assert_eq!(
            helper_node_map_position(&sim, f, "farmer02"),
            Some(FxVec3::new(Fx(x), Fx(y), Fx(z)))
        );
        // `farmer01` is missing: -1, arguments unchanged.
        let mut m = Message::new(msg::FARMER_POS, &[0, -1, -1, -1]);
        assert_eq!(send_mut(&mut sim, f, &mut m), -1);
        assert_eq!(m.args, [0, -1, -1, -1]);
    }
}
