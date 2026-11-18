//! Removing, ownership, hit points, attributes, clearing an event bit, collision and groups: actions 6,
//! 194, 195, 196 (remove), 35, 197, 202, 341 (ownership), 192, 201, 353, 354, 215 (hit points), 384, 385
//! (attributes), 231 (clear event bit), 391-395 and 397 (collision and groups). Source: `FUN_00437190`,
//! `FUN_004360D0`, `FUN_0043A230`, `FUN_0047EAC0`, `FUN_00438690` and the dispatcher cases `0x51`, `0x53`,
//! `0x59`, `0x66`, `0x69`, `0x71`, `0x72`.
//!
//! All of them act through object lists or names and read with `GetObjectById(id, 0)`: dying objects count,
//! stale IDs are skipped. A destruction or change of owner through the queue does not happen until the end of the
//! tick (`world_tick`); 202 changes immediately.

use opennml_formats::config::lower_german;
use opennml_formats::targets::MatchFilter;

use crate::combat::rank::add_experience;
use crate::filter;
use crate::id::ObjectId;
use crate::names;
use crate::object::status::EXEMPT_ZONE;
use crate::sim::Sim;
use crate::upgrades::objects_of_slot;
use crate::variants::refresh_module_params;
use crate::world::{change_owner, queue_destroy, queue_owner_change, set_object_event_bit};

use super::super::query::{area, filter_known, filter_or_raw_guid};

/// Attribute "hit points".
const HIT_POINTS: u32 = 14;
/// Attribute "special energy" (extra energy).
const SPECIAL_ENERGY: u32 = 15;
/// Attribute "experience" (`GameObject_AddExperience`).
const EXPERIENCE: u32 = 30;
/// Bit `MM_MYLIST` in the filter mode: only the object list of the reference player.
const MY_LIST: u32 = 0x100;
/// Largest slot index of the tables (0-7 players, 8 neutral, 9 trees, 10 nature).
const LAST_SLOT: i32 = 10;

/// Actions 6, 195 and 196 (`FUN_00437190(player, GUID, area)`): every object in the area that passes the
/// filter `guid` with the reference player `player` goes into the destroy list - **without death**, without
/// dying animation and without event bit 2 (E-149). An unknown filter ID (< 1000 without an entry in
/// `flowcontroltragets.ini`) or an unknown area does nothing; an area with swapped corners hits nothing
/// (E-190).
pub fn remove_in_area(sim: &mut Sim, guid: i32, player: i32, area_name: &str) {
    if !filter_known(sim, guid) {
        return;
    }
    let Some(rect) = area(sim, area_name) else {
        return;
    };
    let f = filter::from_id(sim.rules, guid);
    for id in crate::area::collect_in_tile_rect(sim, rect, &f, player) {
        queue_destroy(sim, id);
    }
}

/// Action 174 (`FUN_004372D0(area)`, E-156): every object with a switched-on walk module in the area
/// (filter `mode 4`, `prop 0`, reference player 0: **all** owners, also opponents and animals) directly gets
/// `AbortAllModules` - no command, no emptying of the queue, no effect on the animation lock. Dying (`DIED`)
/// and quartered objects already drop out in the filter, hidden slots and individually hidden objects are not
/// in the collision manager. An unknown area does nothing.
pub fn halt_in_area(sim: &mut Sim, area_name: &str) {
    let Some(rect) = area(sim, area_name) else {
        return;
    };
    let f = MatchFilter {
        mode: 4,
        prop_id: 0,
        ..MatchFilter::constructed()
    };
    for id in crate::area::collect_in_tile_rect(sim, rect, &f, 0) {
        crate::messaging::abort_all_modules(sim, id);
    }
}

/// Action 194 (D case `0x53`): every ID of the name goes into the destroy list **unchecked**. A stale ID
/// (or piece 0 of an empty piece) does not fail until it is destroyed, silently.
pub fn remove_named(sim: &mut Sim, name: &str) {
    for id in names::ids(&sim.state.names, name) {
        queue_destroy(sim, id);
    }
}

/// Slot index as target slot of the tables (`0-10`); the load check allows nothing else.
fn target_slot(slot: i32) -> Option<u8> {
    u8::try_from(slot)
        .ok()
        .filter(|&s| i32::from(s) <= LAST_SLOT)
}

/// Actions 35 and 197 (`FUN_004360D0(GUID, slot, area)`): the filter as with 6, but with `MYLIST` cleared
/// and reference player -1 - objects of **all** owners. An unknown filter ID filters with
/// `{mode 8, GUID = raw value}`. For each hit first the color = the **slot index** is set immediately (not the
/// player color, E-196), then the change of owner goes into the queue (at the end of the tick; if the object
/// already has the target owner, there is no entry and the color stays wrong). An unknown area does nothing.
pub fn give_in_area(sim: &mut Sim, guid: i32, slot: i32, area_name: &str) {
    let Some(target) = target_slot(slot) else {
        return;
    };
    let Some(rect) = area(sim, area_name) else {
        return;
    };
    let mut f = filter_or_raw_guid(sim, guid);
    f.mode &= !MY_LIST;
    for id in crate::area::collect_in_tile_rect(sim, rect, &f, -1) {
        if let Some(o) = sim.object_mut(id) {
            o.color = i32::from(target);
        }
        queue_owner_change(sim, id, target);
    }
}

/// Action 202 (`FUN_0043A230(name, slot)`): the IDs of the name are read once beforehand; every one whose
/// object still exists (even dying) changes owner **immediately**. If the change succeeds, the name points to
/// the new ID (it is set anew after every object, so in the end it holds only the last one transferred; also
/// for slot 8), and the linked object (mount or rider) changes along without a name entry of its own. If the
/// object already has the target owner, `change_owner` does nothing: no name, and the back link stays
/// (deviation from the original, E-197). Event bits are not carried over.
pub fn give_named(sim: &mut Sim, name: &str, slot: i32) {
    let Some(target) = target_slot(slot) else {
        return;
    };
    for id in names::ids(&sim.state.names, name) {
        if sim.object(id).is_none() {
            continue;
        }
        let Some(new_id) = change_owner(sim, id, target) else {
            continue;
        };
        names::register(&mut sim.state.names, name, new_id);
        let linked = sim.object(new_id).map_or(ObjectId::NONE, |o| o.target);
        if !linked.is_none() && linked.0 != 0 && sim.object(linked).is_some() {
            change_owner(sim, linked, target);
        }
    }
}

/// Action 341 (`FUN_0047EAC0(p, q)`): all objects of the slot table of `p` - also dying ones and
/// construction sites - go into the change-of-owner queue to `q` (no immediate color). Only
/// `0 <= p, q < 8`; `p == q` does nothing.
pub fn give_slot(sim: &mut Sim, p: i32, q: i32) {
    if !(0..8).contains(&p) || !(0..8).contains(&q) {
        return;
    }
    for id in objects_of_slot(sim.state, p as u8) {
        queue_owner_change(sim, id, q as u8);
    }
}

/// All still existing objects of the name (`GetObjectById(id, 0)`, so also dying ones).
fn named_objects(sim: &Sim, name: &str) -> Vec<ObjectId> {
    names::ids(&sim.state.names, name)
        .into_iter()
        .filter(|&id| sim.object(id).is_some())
        .collect()
}

/// Upgrade percentage of the owner for the attribute of the object (`GetTypeUpgradePercent`).
fn upgrade_percent(sim: &Sim, id: ObjectId, attribute: u32) -> i32 {
    sim.object(id).map_or(100, |o| {
        sim.type_upgrade_percent(o.owner(), o.guid, attribute)
    })
}

/// Action 192 (D case `0x51`): absolute modifier with key 0 on the maximum hit points
/// (`SetModifier(14, value, 0, 0)`); an existing entry with key 0 is replaced. The current hit points scale
/// along because the fraction stays (E-158).
pub fn set_hp_base(sim: &mut Sim, name: &str, value: i32) {
    for id in named_objects(sim, name) {
        if let Some(o) = sim.object_mut(id) {
            o.attributes.set_modifier(HIT_POINTS, value, false, 0);
        }
    }
}

/// Action 201 (D case `0x59`): current hit points to `value` (at most the maximum, not below 0).
pub fn set_hp(sim: &mut Sim, name: &str, value: i32) {
    for id in named_objects(sim, name) {
        let p = upgrade_percent(sim, id, HIT_POINTS);
        if let Some(o) = sim.object_mut(id) {
            o.attributes.set_current_value(HIT_POINTS, value, p);
        }
    }
}

/// Action 353 (D case `0x71`): `AddScaledToCurrent(14, value, 100)`, i.e. `value / 100` hit points via the
/// fraction (E-194), capped at full at the top, open at the bottom.
pub fn add_hp(sim: &mut Sim, name: &str, value: i32) {
    for id in named_objects(sim, name) {
        let p = upgrade_percent(sim, id, HIT_POINTS);
        if let Some(o) = sim.object_mut(id) {
            o.attributes
                .add_scaled_to_current(HIT_POINTS, value, 100, p);
        }
    }
}

/// Action 354 (D case `0x72`): hit points full.
pub fn fill_hp(sim: &mut Sim, name: &str) {
    fill(sim, name, HIT_POINTS);
}

/// Action 215 (D case `0x66`): special energy full.
pub fn fill_special(sim: &mut Sim, name: &str) {
    fill(sim, name, SPECIAL_ENERGY);
}

fn fill(sim: &mut Sim, name: &str, attribute: u32) {
    for id in named_objects(sim, name) {
        if let Some(o) = sim.object_mut(id) {
            o.attributes.fill_current(attribute);
        }
    }
}

/// Actions 384 and 385 (`FUN_00438690(name, attribute, value, flag)` -> `FUN_00479380`): for each object of
/// the name, only if it has the attribute (`IsPresent`):
/// - 385 (`current == false`): absolute modifier with key 0 (new maximum), attribute full, then
///   `RefreshModuleParams` (the parameters of the switched-on modules are read anew);
/// - 384 (`current == true`), attribute 30 (experience): `value` runs as experience through the ranks - as long
///   as the remainder up to the full rank is smaller than the value, `remainder * 11` experience units are
///   credited (the rank rises), the value drops by the remainder, the remainder becomes the new maximum;
///   finally `value * 10`. Without `RefreshModuleParams`;
/// - 384, other attribute: current value, then `RefreshModuleParams`.
pub fn set_attribute(sim: &mut Sim, name: &str, attribute: i32, value: i32, current: bool) {
    let Ok(attribute) = u32::try_from(attribute) else {
        return;
    };
    for id in named_objects(sim, name) {
        if !sim
            .object(id)
            .is_some_and(|o| o.attributes.is_present(attribute))
        {
            continue;
        }
        if !current {
            if let Some(o) = sim.object_mut(id) {
                o.attributes.set_modifier(attribute, value, false, 0);
                o.attributes.fill_current(attribute);
            }
            refresh_module_params(sim, id);
        } else if attribute == EXPERIENCE {
            add_rank_experience(sim, id, value);
        } else {
            let p = upgrade_percent(sim, id, attribute);
            if let Some(o) = sim.object_mut(id) {
                o.attributes.set_current_value(attribute, value, p);
            }
            refresh_module_params(sim, id);
        }
    }
}

/// Experience `value` after the loop of `FUN_00479380`: `rest = max - current` (from the rounded values, so
/// already 0 for a fraction just below full); as long as `rest < value`, there is `AddExperience(rest * 11)`
/// (a call without effect for `rest == 0`), `value -= rest` and `rest = max`; then `AddExperience(value * 10)`.
/// Only a maximum <= 0 ended endlessly in the original (the value would never drop); here the loop ends then
/// (exception A7).
fn add_rank_experience(sim: &mut Sim, id: ObjectId, mut value: i32) {
    let rest_of = |sim: &Sim| {
        let p = upgrade_percent(sim, id, EXPERIENCE);
        sim.object(id).map_or(0, |o| {
            o.attributes
                .max_value(EXPERIENCE, p)
                .wrapping_sub(o.attributes.current_value(EXPERIENCE, p))
        })
    };
    let max_of = |sim: &Sim| {
        let p = upgrade_percent(sim, id, EXPERIENCE);
        sim.object(id)
            .map_or(0, |o| o.attributes.max_value(EXPERIENCE, p))
    };
    let mut rest = rest_of(sim);
    while rest < value {
        if max_of(sim) <= 0 {
            return;
        }
        add_experience(sim, id, rest.wrapping_mul(11));
        value = value.wrapping_sub(rest);
        rest = max_of(sim);
    }
    add_experience(sim, id, value.wrapping_mul(10));
}

/// Action 231 (D case `0x69`): clear the event bit `bit` of every ID of the name - without checking whether
/// the object is still alive (the entry is created in the process, E-157).
pub fn clear_event_bit(sim: &mut Sim, name: &str, bit: i32) {
    for id in names::ids(&sim.state.names, name) {
        set_object_event_bit(sim, id, bit as u32, false);
    }
}

/// Actions 391 and 392 (`FUN_00438730(name, word)`): for each ID of the name whose object is **alive**
/// (`GetObjectById(id, 1)`: dying and stale ones are skipped), set (391, `on`) or clear (392) the exception
/// object -1 ("always collision-free", replaces an existing circle) and the state bit `EXEMPT_ZONE`
/// (`0x1000`). 392 leaves the exception object -1 in place; without the bit it has no effect.
pub fn set_collision(sim: &mut Sim, name: &str, on: bool) {
    for id in names::ids(&sim.state.names, name) {
        if let Some(o) = sim.state.object_mut(id, true) {
            o.set_collision_exempt_object(ObjectId::NONE);
            o.set_status_flags(EXEMPT_ZONE, on);
        }
    }
}

/// Write the group as a name value (`Join(";")`, key lowercased as in the resolution).
fn write_group(sim: &mut Sim, group: &str, ids: &[ObjectId]) {
    let list = ids
        .iter()
        .map(|id| id.as_i32().to_string())
        .collect::<Vec<_>>()
        .join(";");
    sim.state.names.set(&lower_german(group), list);
}

/// Action 393 (`FUN_0043EDD0(name, group)`): every ID of the name that is not yet in the group (not even
/// through one just appended) is appended to the group; if the group is not empty afterwards, it is
/// written. The old IDs of the group stay in front, also duplicates in it (E-195).
pub fn group_add(sim: &mut Sim, group: &str, name: &str) {
    let mut members = names::ids(&sim.state.names, group);
    for id in names::ids(&sim.state.names, name) {
        if !members.contains(&id) {
            members.push(id);
        }
    }
    if !members.is_empty() {
        write_group(sim, group, &members);
    }
}

/// Action 394 (`FUN_0043EFC0(name, group)`): for every ID of the name, remove the **first** occurrence from
/// the group. If the list stays empty, nothing is written: the old value stays, so the group cannot be
/// emptied this way.
pub fn group_remove(sim: &mut Sim, group: &str, name: &str) {
    let mut members = names::ids(&sim.state.names, group);
    for id in names::ids(&sim.state.names, name) {
        if let Some(i) = members.iter().position(|m| *m == id) {
            members.remove(i);
        }
    }
    if !members.is_empty() {
        write_group(sim, group, &members);
    }
}

/// Action 395 (D case `0x84`, `RemoveKey`): delete the entry with exactly this name - object name as well as
/// group. The dispatcher unpacks the text with `FUN_004229F0`, which lowercases it German-style
/// (`CString_MakeLowerGerman`), before `FUN_00430FD0` passes it to `RemoveKey` (E-210).
pub fn remove_name(sim: &mut Sim, name: &str) {
    sim.state.names.remove(&lower_german(name));
}

/// Action 397 (`FUN_0043F1B0(player, area, group)`): append all objects of the player `player` in the area
/// (filter `MYLIST`, reference player `player`) to the group - **without** a duplicate check - and write the
/// group, but only with at least one hit. An unknown area does nothing.
pub fn group_from_area(sim: &mut Sim, player: i32, area_name: &str, group: &str) {
    let mut members = names::ids(&sim.state.names, group);
    let Some(rect) = area(sim, area_name) else {
        return;
    };
    let f = MatchFilter {
        mode: MY_LIST,
        ..MatchFilter::constructed()
    };
    let hits = crate::area::collect_in_tile_rect(sim, rect, &f, player);
    if hits.is_empty() {
        return;
    }
    members.extend(hits);
    write_group(sim, group, &members);
}
