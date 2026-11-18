//! Hiding whole slots (E-153, E-198; re/script-actions).
//!
//! A slot is **hidden** when `SimState::slot_running[p] == false` (`m_HandleListRunning`,
//! `GameWorld+0x62C`). Its objects stay in the slot table (counted, findable through `GetObjectById` and the
//! table readers), but are in no tick group and are removed from the collision manager and
//! the area queries. Exempt are the **individually shown** objects of the special list
//! `SimState::inactive_lists[p]` (`m_TempRunningHandles`, `L[p]`): they are ticked in phase 1 after
//! group 0 ([`tick_inactive_lists`]) and supply the fog cycle with the sight of the slot.
//!
//! The special list holds IDs; destroyed objects stay as dead IDs (tick and fog skip
//! them) until [`activate_slot`] empties the list (E-198). `L[p]` is non-empty only while the slot is
//! hidden: entries are only created for a hidden owner slot, and [`activate_slot`] empties the
//! list before it switches the slot on again.
//!
//! The node pointer `m_FogTempRunningHandlesPosition` (`+0x2064`) of the fog cycle is kept by the reimplementation
//! as the index `Fog::inactive_pos` into `L[fog.player]`: the index of the next entry to read,
//! `inactive_pos == L.len()` is the null pointer (end of list). So that an entry appended to an already
//! traversed list does not count until the next cycle, as in the original, the index moves along
//! when appending if it was at the end ([`push_inactive`]).

use std::collections::BTreeSet;

use crate::id::{ObjectId, PLAYER_SLOTS, SLOT_COUNT};
use crate::message::Message;
use crate::module::building::msg::{BLOCK_FOOTPRINT, UNBLOCK_FOOTPRINT};
use crate::sim::Sim;
use crate::state::SimState;
use crate::tick::{OBJECT_TICK_MS, object_tick};

/// Special lists in the tick (`GameWorld_Tick` `0x0048E89E CMP EBP, 9`): slots 0...8.
const TICKED_LISTS: usize = 9;

/// Slot index `0...10` or `None`.
fn slot_index(slot: i32) -> Option<usize> {
    usize::try_from(slot).ok().filter(|&s| s < SLOT_COUNT)
}

/// `GameObject_SendUnblockFootprint(flush)` (`0x0046FC10`): `0x3003 {flush}` to the building module
/// (`MarkFootprint` free), then `CollisionMgr_RemoveObject` (also removes from the area queries).
fn send_unblock_footprint(sim: &mut Sim, id: ObjectId, flush: bool) {
    crate::messaging::send(
        sim,
        id,
        &Message::new(UNBLOCK_FOOTPRINT, &[i32::from(flush)]),
    );
    crate::collision::remove(sim, id);
}

/// `GameObject_SendBlockFootprint(flush)`: `0x3103 {flush}` to the building module (`MarkFootprint`
/// block), then `CollisionMgr_InsertObject` (notifies the area queries).
fn send_block_footprint(sim: &mut Sim, id: ObjectId, flush: bool) {
    crate::messaging::send(sim, id, &Message::new(BLOCK_FOOTPRINT, &[i32::from(flush)]));
    crate::collision::insert(sim, id);
}

/// ID of the object at index `i` of the slot table `p` (gap -> `None`). The loops of 159/160 read the
/// table anew on every pass like the original (`i < +0x2B8[p]`).
fn table_id(state: &SimState, p: usize, i: usize) -> Option<ObjectId> {
    state.tables[p].objects.get(i)?.as_ref().map(|o| o.id)
}

/// Common finish of 159/160: `PathPlanner_FlushDirtyRect` (`0x004F70D0`, the collected
/// occupancy changes at once) and `FUN_0040EB60` (`GameWorld+0x3A4`: release all pair-tracking
/// records of the collisions, empty both lists).
fn finish_slot_switch(sim: &mut Sim) {
    sim.state.path.flush_dirty_rect();
    sim.state.collision_records = Default::default();
}

/// Action 159 (`GameWorld_DeactivatePlayerSlot` `0x00480990`): only for a running slot. Flag off, then for
/// every object of the slot table (also quartered, carried, dying ones) `SendUnblockFootprint(0)` and out of
/// the tick group (`tick_group` stays); then empty once. Selection, type and object counters,
/// tables, `L[p]`, module states, projectiles, sight sources and names stay untouched. A slot outside 0...10
/// is not checked by the original (writing behind the field); here without effect, the script path reports it
/// beforehand.
pub fn deactivate_slot(sim: &mut Sim, slot: i32) {
    let Some(p) = slot_index(slot) else { return };
    if !sim.state.slot_running[p] {
        return;
    }
    sim.state.slot_running[p] = false;
    let mut removed = BTreeSet::new();
    let mut i = 0;
    while i < sim.state.tables[p].objects.len() {
        if let Some(id) = table_id(sim.state, p, i) {
            send_unblock_footprint(sim, id, false);
            removed.insert(id);
        }
        i += 1;
    }
    // `GameWorld_RemoveFromTickGroup` per object takes it out of the group `+0x7C8`; an object is only ever in
    // the group of its field, removing from both is equivalent. Collected, with one `retain` per group
    // instead of one `retain` over all groups per object: same result, the order of the other
    // entries stays, because `UNBLOCK_FOOTPRINT` and `collision::remove` neither read nor change the
    // tick groups.
    for group in &mut sim.state.tick_groups {
        group.retain(|id| !removed.contains(id));
    }
    finish_slot_switch(sim);
}

/// Action 160 (`GameWorld_ActivatePlayerSlot` `0x00482090`): only for a hidden slot. Every existing
/// object from `L[p]` goes into the destroy list (dead IDs skipped), `L[p]` is emptied, then
/// `GameWorld_ProcessDestroyQueue` processes **the whole queue** at once - also already queued
/// foreign destructions (E-198). Afterwards flag on and for every remaining object of the slot table
/// `SendBlockFootprint(0)` and `AddToTickGroup` (at the back, in table order); finally empty once.
///
/// If the original empties `L[p]` while the fog cycle stands in it, its node pointer stays
/// dangling (A7); the reimplementation sets the index to the end of the list (0).
pub fn activate_slot(sim: &mut Sim, slot: i32) {
    let Some(p) = slot_index(slot) else { return };
    if sim.state.slot_running[p] {
        return;
    }
    let list = std::mem::take(&mut sim.state.inactive_lists[p]);
    for id in list {
        if sim.object(id).is_some() {
            sim.state.destroy_queue.push(id);
        }
    }
    if usize::from(sim.state.fog.player) == p {
        sim.state.fog.inactive_pos = 0;
    }
    super::process_destroy_queue(sim);
    sim.state.slot_running[p] = true;
    let mut i = 0;
    while i < sim.state.tables[p].objects.len() {
        if let Some(id) = table_id(sim.state, p, i) {
            send_block_footprint(sim, id, false);
            super::add_to_tick_group(sim, id);
        }
        i += 1;
    }
    finish_slot_switch(sim);
}

/// Appends `id` to `L[p]` (`AddTail`). If the fog cycle currently stands at the end of this list (null pointer),
/// it stays at the end: the new entry does not count until the next cycle, as in the original.
pub(crate) fn push_inactive(state: &mut SimState, p: usize, id: ObjectId) {
    let fog = &mut state.fog;
    let at_end = usize::from(fog.player) == p
        && p < PLAYER_SLOTS
        && fog.inactive_pos as usize == state.inactive_lists[p].len();
    state.inactive_lists[p].push(id);
    if at_end {
        fog.inactive_pos += 1;
    }
}

/// `GameWorld_AddToInactiveSlotList(id, no_block)` (`0x0047E620`, 162/366/211 and change of owner): list
/// by the slot of the ID. Only if `id` is not yet in it, the object exists (also dying)
/// and its owner slot is hidden: append, without `no_block` `SendBlockFootprint(1)` (register,
/// empty at once), and `tick_group = 0` (always; the tick groups themselves stay untouched).
pub fn add_inactive(sim: &mut Sim, id: ObjectId, no_block: bool) {
    let p = usize::from(id.slot());
    if p >= SLOT_COUNT || sim.state.inactive_lists[p].contains(&id) {
        return;
    }
    let Some(owner) = sim.object(id).map(|o| usize::from(o.id.slot())) else {
        return;
    };
    if sim.state.slot_running.get(owner).copied().unwrap_or(true) {
        return;
    }
    push_inactive(sim.state, p, id);
    if !no_block {
        send_block_footprint(sim, id, true);
    }
    if let Some(o) = sim.object_mut(id) {
        o.tick_group = 0;
    }
}

/// `GameWorld_RemoveFromInactiveSlotList(id)` (`0x0047E6A0`, 161/365/214 and change of owner): only if `id`
/// is in `L[p]`. If the fog cycle points at this entry, it moves on to the next one (same index),
/// if it stands behind it, the index moves along; then remove from the list and, if the object still
/// exists, `SendUnblockFootprint(1)`. No check of the slot flag; `tick_group` stays.
pub fn remove_inactive(sim: &mut Sim, id: ObjectId) {
    let p = usize::from(id.slot());
    let Some(j) = sim
        .state
        .inactive_lists
        .get(p)
        .and_then(|l| l.iter().position(|&x| x == id))
    else {
        return;
    };
    let fog = &mut sim.state.fog;
    if usize::from(fog.player) == p && p < PLAYER_SLOTS && j < fog.inactive_pos as usize {
        fog.inactive_pos -= 1;
    }
    sim.state.inactive_lists[p].remove(j);
    if sim.object(id).is_some() {
        send_unblock_footprint(sim, id, true);
    }
}

/// Is `id` hidden (slot of the ID off and `id` not in `L[p]`)? Also applies to dead IDs; slots
/// outside 0...10 are never hidden. Readers: `sim_query` and the presentation (deviation from the original).
pub fn is_hidden(state: &SimState, id: ObjectId) -> bool {
    let p = usize::from(id.slot());
    p < SLOT_COUNT && !state.slot_running[p] && !state.inactive_lists[p].contains(&id)
}

/// Tick of the special lists (`GameWorld_Tick` `0x0048E845...0x0048E8A1`): only in the phase 1 call directly after
/// group 0, slots 0...8 in order, only hidden ones. For each entry `GetObjectById(id, 0)`: present ->
/// `GameObject_Tick(100)`, otherwise skipped (not removed). Like the MFC list, the run remembers the
/// following entry **before** the tick: an entry appended during the tick is still reached, unless the ticked one was
/// the last; if the remembered following entry itself disappears (a dangling pointer in the original, A7), the
/// list ends.
pub(crate) fn tick_inactive_lists(sim: &mut Sim) {
    for p in 0..TICKED_LISTS {
        if sim.state.slot_running[p] {
            continue;
        }
        let mut i = 0;
        while let Some(&id) = sim.state.inactive_lists[p].get(i) {
            let next = sim.state.inactive_lists[p].get(i + 1).copied();
            if sim.object(id).is_some() {
                object_tick(sim, id, OBJECT_TICK_MS);
            }
            let Some(next) = next else { break };
            // Fast path: if the following entry still stands at `i + 1`, it is there (the remembered
            // node of the original). The list has no duplicates (`add_inactive` checks, the load check in
            // `save.rs` rejects them), so the search would find the same place.
            let list = &sim.state.inactive_lists[p];
            if list.get(i + 1) == Some(&next) {
                i += 1;
                continue;
            }
            match list.iter().position(|&x| x == next) {
                Some(n) => i = n,
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests;
