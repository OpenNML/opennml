//! Idle workers (re/game-ui D5.4, O-293): the lists `m_WaitingWorkers` (`GameWorld+0x2428 + p·0x1C`, one `CPtrList` per
//! slot 0-7) for buttons 576/577 (sole reader `0x0048B610`; the AI does not read them). Filled at level start
//! (`0x0047ECA0`), extended through the message `0x87F` (`GameWorld_RegisterIdleWorker` `0x0047EDA0`) and
//! maintained in the 100 ms tick (`0x0047EB20`).
//!
//! A worker is an object with active module 8 (`PROPID 8`). In the tick it counts only without a status bit from
//! `0xF8EC0FFF` ([`status::BUSY_MASK`]). The index `m_WaitingWorkerIndex` belongs to the UI and lives in the host
//! (deviation from the original).

use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::filter;
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::object::status;
use crate::sim::Sim;

/// Lists of the idle workers per slot 0-7 in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleWorkers {
    pub lists: [Vec<ObjectId>; PLAYER_SLOTS],
}

impl IdleWorkers {
    /// List of slot `player` (empty for slots >= 8).
    pub fn list(&self, player: u8) -> &[ObjectId] {
        self.lists
            .get(usize::from(player))
            .map_or(&[], Vec::as_slice)
    }
}

/// Filter of level start and message: `{mode 0x114, mode2 0, PROPID 8}` - the player's list, AND, module 8 present
/// and active; `0x114` does not read the status.
fn start_filter() -> MatchFilter {
    MatchFilter {
        mode: 0x114,
        prop_id: 8,
        ..MatchFilter::constructed()
    }
}

/// Filter of the tick (`0x0047EB20`): `{mode 0x2114, mode2 0, status 0xF8EC0FFF, PROPID 8}` - like
/// [`start_filter`], plus "no status bit from `0xF8EC0FFF`".
fn tick_filter() -> MatchFilter {
    MatchFilter {
        mode: 0x2114,
        status: status::BUSY_MASK,
        prop_id: 8,
        ..MatchFilter::constructed()
    }
}

/// IDs of the object table of slot `p` in index order (`GameWorld+0x234[p]` up to the capacity
/// `+0x2B8[p]`).
fn table_ids(sim: &Sim, p: usize) -> Vec<ObjectId> {
    sim.state.tables[p]
        .objects
        .iter()
        .flatten()
        .map(|o| o.id)
        .collect()
}

/// `0x0047ECA0` (start of `GameWorld_ApplyPlayerHandicaps` `0x00484B70`, only for a new level): for each slot 0-7,
/// append every object of the table in index order that satisfies [`start_filter`] - without a duplicate check and
/// without clearing the list first (it is empty at the start). In the reimplementation this runs after the objects
/// from [`crate::setup::new_match`] are placed.
pub fn init(sim: &mut Sim) {
    let f = start_filter();
    for p in 0..PLAYER_SLOTS {
        for id in table_ids(sim, p) {
            if filter::matches(sim, id, &f, p as i32) {
                sim.state.idle_workers.lists[p].push(id);
            }
        }
    }
}

/// `GameWorld_RegisterIdleWorker` (`0x0047EDA0`, from `0x87F` to the object): if `id` satisfies [`start_filter`]
/// for its owner slot and is missing from that slot's list, append it.
///
/// Owner slot >= 8: nothing. The original would write the list behind `m_WaitingWorkers[7]`, that is onto
/// `m_WaitingWorkerIndex` (`0x2428 + 8·0x1C = 0x2508`, memory error, A7).
pub fn register(sim: &mut Sim, id: ObjectId) {
    let p = usize::from(id.slot());
    if p >= PLAYER_SLOTS || !filter::matches(sim, id, &start_filter(), p as i32) {
        return;
    }
    let list = &mut sim.state.idle_workers.lists[p];
    if !list.contains(&id) {
        list.push(id);
    }
}

/// `0x0048E99A` -> `0x0047EB20` (in the phase-0 step of `GameWorld_Tick` after the objects and `EffectManager_Tick`,
/// before the destruction list): for each slot 0-7, first remove every entry whose object is missing
/// (`GameWorld_GetObjectById(id, 0)`) or does not satisfy [`tick_filter`], then append every matching object of the
/// table in index order that is still missing. Independent of the tick groups and of the fade-out state of the slots.
pub fn refresh(sim: &mut Sim) {
    let f = tick_filter();
    for p in 0..PLAYER_SLOTS {
        let mut list = std::mem::take(&mut sim.state.idle_workers.lists[p]);
        list.retain(|&id| sim.object(id).is_some() && filter::matches(sim, id, &f, p as i32));
        for id in table_ids(sim, p) {
            if !list.contains(&id) && filter::matches(sim, id, &f, p as i32) {
                list.push(id);
            }
        }
        sim.state.idle_workers.lists[p] = list;
    }
}

#[cfg(test)]
mod tests;
