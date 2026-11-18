//! Hiding whole slots and showing/hiding single objects: the actions 159, 160 (slot
//! off/on), 162/161 (show/hide the objects of a name), 366/365 (objects of a slot by type or
//! category) and 211/214 (objects of a slot by filter in an area). The world logic lives in
//! [`crate::world::hide`]; source: D cases `0x33`-`0x36`, `0x62`, `0x65`, `0x73`, `0x74`, `FUN_00436BD0`,
//! `FUN_00436C60`, `FUN_0047F2F0`, `FUN_00437E90`, `FUN_0047EA20`.
//!
//! "Showing" means adding to the special list `L[p]` (`GameWorld_AddToInactiveSlotList`, only effective
//! for a hidden owner slot), "hiding" means removing. **211 shows, 214 hides**.

use crate::filter;
use crate::id::{ObjectId, PLAYER_SLOTS, SLOT_COUNT};
use crate::names;
use crate::sim::Sim;
use crate::world::hide::{activate_slot, add_inactive, deactivate_slot, remove_inactive};

use super::super::query::filter_known;
use super::super::{area, area_key};

/// Slot `0...10`, otherwise the run-time error: the original does not check 159/160/211/214 and would
/// write or read beyond the fields (A7). The load check (`abl::catalog::check`) already rejects such
/// slots; the error stays as an unreachable guard.
fn table_slot(number: i32, slot: i32) -> Result<usize, String> {
    usize::try_from(slot)
        .ok()
        .filter(|&s| s < SLOT_COUNT)
        .ok_or_else(|| format!("action {number} with invalid slot {slot}"))
}

/// Action 159 (`GameWorld_DeactivatePlayerSlot`): hide slot `slot`.
pub fn deactivate(sim: &mut Sim, slot: i32) -> Result<(), String> {
    table_slot(159, slot)?;
    deactivate_slot(sim, slot);
    Ok(())
}

/// Action 160 (`GameWorld_ActivatePlayerSlot`): switch slot `slot` on again (destroys `L[p]`).
pub fn activate(sim: &mut Sim, slot: i32) -> Result<(), String> {
    table_slot(160, slot)?;
    activate_slot(sim, slot);
    Ok(())
}

/// Actions 162 (`show`, `FUN_00436BD0`: `Add(id, 0)`) and 161 (`FUN_00436C60`: `Remove(id)`): for each ID
/// of the name (`"id;id;..."`, unchecked; stale IDs fail silently in the world function).
pub fn named(sim: &mut Sim, name: &str, show: bool) {
    for id in names::ids(&sim.state.names, name) {
        toggle(sim, id, show);
    }
}

fn toggle(sim: &mut Sim, id: ObjectId, show: bool) {
    if show {
        add_inactive(sim, id, false);
    } else {
        remove_inactive(sim, id);
    }
}

/// Actions 366 (`show`) and 365 (`FUN_0047F2F0(p, guid, an)`): only `0 <= p < 8`, otherwise silently
/// nothing; for each object of the slot table in table order (also dying and quartered ones).
/// `guid >= 1000` compares the type (`+0x54`) without a filter; otherwise the category filter
/// `FUN_0047E390` (unknown -> no match) with `GameObject_MatchesFilter(f, -1, 0)`.
pub fn by_type(sim: &mut Sim, slot: i32, guid: i32, show: bool) {
    let Some(p) = usize::try_from(slot).ok().filter(|&p| p < PLAYER_SLOTS) else {
        return;
    };
    let f = (guid < 1000 && filter_known(sim, guid)).then(|| filter::from_id(sim.rules, guid));
    let mut i = 0;
    while i < sim.state.tables[p].objects.len() {
        let hit = sim.state.tables[p].objects[i]
            .as_ref()
            .map(|o| (o.id, o.guid));
        i += 1;
        let Some((id, type_guid)) = hit else { continue };
        let matched = if guid >= 1000 {
            type_guid == guid as u32
        } else {
            f.as_ref().is_some_and(|f| filter::matches(sim, id, f, -1))
        };
        if matched {
            toggle(sim, id, show);
        }
    }
}

/// Actions 211 (`show`) and 214 (`FUN_00437E90(p, type, area, 0 | 1)`): unknown area or unknown filter
/// ID -> nothing. Otherwise `FUN_0047EA20(p, f, rectangle)`: it enlarges **the stored rectangle** by one
/// column on the right and one row at the bottom (permanently, E-190, `area_overrides`) and then
/// collects from the slot table (also hidden objects) the objects with a tile in `PtInRect` of the
/// enlarged rectangle (so including the old right and bottom edge; swapped corners: empty) that pass
/// `MatchesFilter(f, -1, 0)`. 211 shows them (`Add(id, 0)`), 214 hides them (`Remove(id)`).
pub fn in_area(
    sim: &mut Sim,
    number: i32,
    slot: i32,
    kind: i32,
    area_name: &str,
    show: bool,
) -> Result<(), String> {
    let Some(rect) = area(sim, area_name) else {
        return Ok(());
    };
    if !filter_known(sim, kind) {
        return Ok(());
    }
    let p = table_slot(number, slot)?;
    let grown = [
        rect[0],
        rect[1],
        rect[2].wrapping_add(1),
        rect[3].wrapping_add(1),
    ];
    sim.state
        .script
        .area_overrides
        .insert(area_key(area_name), grown);
    let f = filter::from_id(sim.rules, kind);
    let [left, top, right, bottom] = grown;
    let candidates: Vec<(ObjectId, (i32, i32))> = sim.state.tables[p]
        .objects
        .iter()
        .flatten()
        .map(|o| (o.id, o.tile))
        .collect();
    let mut hits = Vec::new();
    for (id, (x, z)) in candidates {
        if left <= x && x < right && top <= z && z < bottom && filter::matches(sim, id, &f, -1) {
            hits.push(id);
        }
    }
    for id in hits {
        toggle(sim, id, show);
    }
    Ok(())
}
