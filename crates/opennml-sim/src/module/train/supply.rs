//! Supply of the Railroad game types (re/train-multipart 3.5, re/object-lifecycle 4): The locomotive
//! that leaves the map backward in game type 4 loads supply and drives in again by itself after 180 s
//! (self-departure in [`super::drive::tick`]); when halting inside the map the supply stands at the
//! locomotive. Game type 5 and the forward exit load nothing (the victory "Railroad" is not handled
//! here).

use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::setup::place_supply_objects;
use crate::sim::Sim;

use super::drive::loco_outside_map;
use super::{train, train_mut};

/// Game type Railroad 1 (`GameEngine+0x108`): the only one that loads supply.
const RAILROAD_MODE: i32 = 4;
/// Wait time until the self-departure in ms (`Engine_OnTrainLeftMap`: `0x821 {180000}`).
const SUPPLY_WAIT_MS: i32 = 180_000;

/// `Engine_OnTrainLeftMap(loco, forward)` (`0x004459B0`): the locomotive has just left the map. Only
/// in game type 4 **and** backward: `0x821 {180 000}` to the locomotive (`Train_LoadSupply`: only without
/// loaded supply, sets counter and flag). Note: the call comes from the tick that sets the
/// outside status, with the status **after** the drive step - if the locomotive drives out for the
/// first time in the same tick and exceeds the overshoot (`LeaveMap`), the reimplementation reports,
/// like the original, "backward" (not reachable in the game).
///
/// Deviation from the original: the second call of the original (`0x00445350(2, 180000, 1, 16000,
/// ...)`, timer list `GameEngine+0x2738`, ID 16000) is the countdown display "Supply"
/// (hypothesis) and is omitted; it does not change the simulation.
pub fn on_left_map(sim: &mut Sim, id: ObjectId, forward: bool) {
    if sim.state.mode != RAILROAD_MODE || forward {
        return;
    }
    send(sim, id, &Message::new(0x821, &[SUPPLY_WAIT_MS]));
}

/// End of braking (`0x0052F960`): unload supply if loaded **and** the locomotive stands in the map.
/// "In the map" is the same check as the outside check of the tick (`0x0052EFD0` ->
/// `0x004F69C0`, [`loco_outside_map`]): the unclamped grid position `(grid >> 12)` with
/// `0 <= x < width`, `0 <= z < height`, not the clamped tile of the object and not
/// `GameWorld_IsGridPosInMap` (whose upper bound is inclusive). Then: flag, counter and
/// outside status to 0 and `Train_UnloadSupply` (`0x00440910`): [`place_supply_objects`] of the
/// slot of the locomotive at its world position.
pub fn unload_if_due(sim: &mut Sim, id: ObjectId) {
    if !train(sim, id).is_some_and(|t| t.has_supply) {
        return;
    }
    let Some(o) = sim.object(id) else { return };
    if loco_outside_map(sim, id) {
        return;
    }
    let (slot, pos) = (id.slot(), o.world_pos);
    if let Some(t) = train_mut(sim, id) {
        t.has_supply = false;
        t.time_counter = 0;
        t.outside = 0;
    }
    place_supply_objects(sim, slot, pos);
}
