//! Upgrades, counter-upgrades and temporary upgrades (re/upgrades, re/temp-upgrades): state types of the
//! savegame, the type percentage table and applying, removing and revoking with the variant switch over the
//! objects of a slot (re/upgrades 3.2-3.4, 3.6), the world queue (3.5) and the time-of-day upgrades (3.8).
//! Counter-upgrades (start, expiry, revocation) live in [`counter`], temporary upgrades per object in
//! [`temp`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::attributes::{ATTRIBUTE_COUNT, FIRST_ATTRIBUTE, LAST_ATTRIBUTE};
use crate::id::{ObjectId, PLAYER_SLOTS, SLOT_COUNT};
use crate::sim::Sim;
use crate::state::SimState;

pub mod counter;
pub mod temp;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unit_tests;

/// Slots with a type table (`m_GuidUpgrades`, `GameWorld+0x4E8 + p*0x1C`, saved `fixedsize 9`,
/// re/upgrades 1.2, 5): players 0-7 and neutral 8.
pub const PERCENT_SLOTS: usize = 9;

/// `TypeUpgradeTable` (`0x26C` bytes, constructor `FUN_0048F460`, re/upgrades 1.2): per attribute
/// (index `ID - 13`, 0...21) a map upgrade GUID -> percentage. `SetPercent` (`0x0048F4B0`)
/// overwrites the value of the GUID, `SumPercent` (`0x0048F3C0`) sums all values of an attribute.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeUpgrades(pub [BTreeMap<u32, i32>; ATTRIBUTE_COUNT]);

impl TypeUpgrades {
    /// Map of the attribute `attribute` (13...34), otherwise none.
    fn index(attribute: u32) -> Option<usize> {
        (FIRST_ATTRIBUTE..=LAST_ATTRIBUTE)
            .contains(&attribute)
            .then(|| (attribute - FIRST_ATTRIBUTE) as usize)
    }

    /// `TypeUpgradeTable_SetPercent(g, id, v)` (`0x0048F4B0`): overwrite the value of the GUID (no
    /// adding per GUID, K-69); an attribute outside 13...34 stays without effect (index check).
    pub fn set_percent(&mut self, guid: u32, attribute: u32, value: i32) {
        if let Some(i) = Self::index(attribute) {
            self.0[i].insert(guid, value);
        }
    }

    /// `TypeUpgradeTable_SumPercent(id)` (`0x0048F3C0`): sum of all values of the map (unclamped,
    /// overflow like x86); an attribute outside 13...34 yields 0 (exception A7).
    pub fn sum_percent(&self, attribute: u32) -> i32 {
        Self::index(attribute).map_or(0, |i| {
            self.0[i].values().fold(0i32, |s, &v| s.wrapping_add(v))
        })
    }

    /// `TypeUpgradeTable_RemoveUpgrade(g)` (`0x0048F430`): `RemoveKey(g)` in all 22 maps.
    pub fn remove_upgrade(&mut self, guid: u32) {
        for map in &mut self.0 {
            map.remove(&guid);
        }
    }
}

/// Entry of the world queue `m_UpgradeStack` (`GameWorld+0x1F50`, 12 bytes `{slot, guid,
/// apply:u8}`, re/upgrades 1.2, 3.5): `apply` = apply (`FUN_0047E310`), otherwise remove
/// (`FUN_0047E350`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeQueueEntry {
    pub slot: u8,
    pub guid: u32,
    pub apply: bool,
}

/// Running counter-upgrade (`m_CounterUpgrades`, list `{guid, rest_ms, building_hid}` at
/// `GameWorld+0x990`, re/upgrades 3.6, re/temp-upgrades 3.5/3.6). The source may be missing (died
/// or removed, rest afterwards <= 0 via `ExpireBySource`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterUpgrade {
    pub guid: u32,
    pub rest_ms: i32,
    pub source: ObjectId,
}

/// Entry of the list of temporary upgrades of an object (`GameObject+0x788`, 14 dwords,
/// re/temp-upgrades 1.2): flags (copy from the record), GUID and remaining time. The effect handle
/// (word 3) is presentation, the message copy (words 4...13) is read by the reimplementation from the record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempUpgradeEntry {
    pub flags: u32,
    pub guid: u32,
    pub rest_ms: i32,
}

/// `GameWorld_GetTypeUpgradePercent` (`0x0047DC90`, re/upgrades 3.3): sum of the percentages of all
/// upgrades of the type `guid` for `attribute` at the slot `slot` (overflow like x86); only players 0-7
/// (W-213), without table 0.
pub fn type_upgrade_percent(state: &SimState, slot: u8, guid: u32, attribute: u32) -> i32 {
    if usize::from(slot) >= PLAYER_SLOTS {
        return 0;
    }
    state
        .type_upgrades
        .get(usize::from(slot))
        .and_then(|tables| tables.get(&guid))
        .map_or(0, |t| t.sum_percent(attribute))
}

/// Objects of the slot table `slot` (`GameWorld+0x234[p]`, enumeration `FUN_0047B570`/`0x00476EE0`)
/// in table order, gaps skipped - also construction sites, occupants and dying objects
/// until their change of owner. Slot >= 11 -> empty.
pub fn objects_of_slot(state: &SimState, slot: u8) -> Vec<ObjectId> {
    state
        .tables
        .get(usize::from(slot))
        .map(|t| t.objects.iter().flatten().map(|o| o.id).collect())
        .unwrap_or_default()
}

/// Variant switch after apply/remove (re/upgrades 3.4): every object of the slot whose type names the
/// upgrade in `OBJ_UPGRADES` (otherwise none, K-70) recomputes its mask
/// (`GameObject_UpdateVariantMask(-1, 0)` `0x00469D80`: only `ReadParams`, matching modules are
/// switched on, K-282).
fn switch_variants_of_slot(sim: &mut Sim, slot: u8, types: &[u32]) {
    for id in objects_of_slot(sim.state, slot) {
        if sim.object(id).is_some_and(|o| types.contains(&o.guid)) {
            crate::variants::update_variant_mask(sim, id, u32::MAX, 0);
        }
    }
}

/// Change `m_ShowCounterUpgrade` (`GameObject+0x2C`) of all objects of the type `ty` on the slot
/// (`FUN_00468170(d)`: `v = max(0, v + d)`).
fn add_show_counter_upgrade(sim: &mut Sim, slot: u8, ty: u32, delta: i32) {
    for id in objects_of_slot(sim.state, slot) {
        if let Some(o) = sim.object_mut(id)
            && o.guid == ty
        {
            o.show_counter_upgrade = o.show_counter_upgrade.wrapping_add(delta).max(0);
        }
    }
}

/// `Player_ApplyUpgrade(p, g, incremental)` (`0x00481E60`, re/upgrades 3.2). Slot < 0 or >= 11
/// stays without effect (foreign memory in the original, exception A7). Counter +1 (only players 0-7,
/// also incrementally); for each type from `OBJ_UPGRADES` and attribute 13...34 with config `attribute + 100`
/// != 0 set the percentage (overwrite, K-69) - also for slot 8, but it is never read there
/// (W-213). Incremental (counter-upgrade, W-214): `m_ShowCounterUpgrade` of the objects of these types
/// +1, nothing to the list, no variant switch. Otherwise the GUID at the back of the list (duplicates,
/// K-69) and variant switch of the objects of the slot with a matching type.
pub fn apply_upgrade(sim: &mut Sim, slot: i32, guid: u32, incremental: bool) {
    let Some(p) = u8::try_from(slot)
        .ok()
        .filter(|&p| usize::from(p) < SLOT_COUNT)
    else {
        return;
    };
    sim.state.add_guid_count(p, guid, 1);
    let rules = sim.rules;
    let types = rules.types_with_upgrade(guid);
    for &ty in types {
        for attribute in FIRST_ATTRIBUTE..=LAST_ATTRIBUTE {
            let v = rules.config_value(p, guid, attribute + 100);
            if v != 0 && usize::from(p) < PERCENT_SLOTS {
                sim.state.type_upgrades[usize::from(p)]
                    .entry(ty)
                    .or_default()
                    .set_percent(guid, attribute, v);
            }
        }
        if incremental {
            add_show_counter_upgrade(sim, p, ty, 1);
        }
    }
    if !incremental {
        sim.state.applied_upgrades[usize::from(p)].push(guid);
        switch_variants_of_slot(sim, p, types);
    }
}

/// `Player_RemoveUpgrade(p, g)` (`0x00481480`, re/upgrades 3.2): only players 0-7. Counter -1, also
/// without a previous apply (negative, W-215); the first occurrence from the list; the percentage entry
/// of the GUID disappears for **all** types of the slot in all 22 maps, even if the list
/// still has duplicates (K-281); then variant switch as when applying.
pub fn remove_upgrade(sim: &mut Sim, slot: i32, guid: u32) {
    let Some(p) = u8::try_from(slot)
        .ok()
        .filter(|&p| usize::from(p) < PLAYER_SLOTS)
    else {
        return;
    };
    sim.state.add_guid_count(p, guid, -1);
    let list = &mut sim.state.applied_upgrades[usize::from(p)];
    if let Some(n) = list.iter().position(|&g| g == guid) {
        list.remove(n);
    }
    for t in sim.state.type_upgrades[usize::from(p)].values_mut() {
        t.remove_upgrade(guid);
    }
    let types = sim.rules.types_with_upgrade(guid);
    switch_variants_of_slot(sim, p, types);
}

/// `GameWorld_RevokeUpgradeAllPlayers(g)` (`0x004812F0`, re/upgrades 3.6), end of a
/// counter-upgrade: the counter of every player 0-7 with an object counter != 0 (`Player_GetGuidCount`, with
/// groups) -1; for each type from `OBJ_UPGRADES`, attribute and player with config `attribute + 100` != 0 the
/// entry from its type table; for each player `m_ShowCounterUpgrade` of the objects of the type -1 (not
/// below 0). No variant switch.
pub fn revoke_upgrade_all_players(sim: &mut Sim, guid: u32) {
    for p in 0..PLAYER_SLOTS as u8 {
        if crate::economy::guid_count(sim, i32::from(p), guid) != 0 {
            sim.state.add_guid_count(p, guid, -1);
        }
    }
    let rules = sim.rules;
    for &ty in rules.types_with_upgrade(guid) {
        for attribute in FIRST_ATTRIBUTE..=LAST_ATTRIBUTE {
            for p in 0..PLAYER_SLOTS as u8 {
                if rules.config_value(p, guid, attribute + 100) != 0
                    && let Some(t) = sim.state.type_upgrades[usize::from(p)].get_mut(&ty)
                {
                    t.remove_upgrade(guid);
                }
            }
        }
        for p in 0..PLAYER_SLOTS as u8 {
            add_show_counter_upgrade(sim, p, ty, -1);
        }
    }
}

/// `GameWorld_CmdSetArrowType(cmd, brand)` (`0x00475B90`, re/arming-gates 3.B), effect of the
/// commands `0x40`/`0x41` (used by tests; the check rejects player commands, A8):
/// for each existing object without an owner, type or upgrade check the variant mask with forced
/// bit 13 (`fire`) or 12 (`UpdateVariantMask(~0x1000, 0x2000)` or `(~0x2000, 0x1000)`, only
/// slots 0-7). The arrow type itself is still chosen by every shot from the player mask (C-96); the
/// forced mask lives only until the next recomputation of the object (upgrade, arming) and
/// in the shipped data only acts on the transport ship 1108 (C-224). `NotifyUiIfSelected` is UI.
pub fn set_arrow_type(sim: &mut Sim, units: &[ObjectId], fire: bool) {
    let (and, or) = if fire {
        (!0x1000, 0x2000)
    } else {
        (!0x2000, 0x1000)
    };
    for &u in units {
        if sim.object(u).is_some() {
            crate::variants::update_variant_mask(sim, u, and, or);
        }
    }
}

/// `FUN_0047E310` (apply) or `FUN_0047E350` (remove) (re/upgrades 3.5): entry
/// `{slot, GUID, apply}` at the back of `m_UpgradeStack` (`GameWorld+0x1F50`). The slot stays as it
/// comes (also 11, the init of module 30 clamps to it); one outside `u8` is discarded.
pub fn queue_upgrade(state: &mut SimState, slot: i32, guid: u32, apply: bool) {
    let Ok(slot) = u8::try_from(slot) else {
        log::debug!("Upgrade {guid} for slot {slot} not queued");
        return;
    };
    state
        .upgrade_queue
        .push(UpgradeQueueEntry { slot, guid, apply });
}

/// Processing of `m_UpgradeStack` in `GameWorld_Tick` (`0x0048E8C1`, re/upgrades 3.5): only in the
/// phase 0 step, after both tick groups, before the extra list; FIFO via
/// [`apply_upgrade`] (not incremental) or [`remove_upgrade`], then empty (`RemoveAll`).
/// Deviation (exception A7): entries with slot >= 11 wrote foreign memory in the original
/// (`m_FriendList`, W-220) and are discarded.
pub fn process_upgrade_queue(sim: &mut Sim) {
    for e in std::mem::take(&mut sim.state.upgrade_queue) {
        if usize::from(e.slot) >= SLOT_COUNT {
            log::debug!(
                "Upgrade queue: entry {} for slot {} discarded",
                e.guid,
                e.slot
            );
            continue;
        }
        if e.apply {
            apply_upgrade(sim, i32::from(e.slot), e.guid, false);
        } else {
            remove_upgrade(sim, i32::from(e.slot), e.guid);
        }
    }
}

/// First GUID of the time-of-day upgrades (`4500 + t + 10*k`, re/upgrades 3.8).
pub const DAYTIME_BASE: u32 = 4500;
/// Time-of-day upgrades per slot (`k` 0...5).
pub const DAYTIME_UPGRADES: u32 = 6;
/// Slots with time-of-day upgrades (0-8; counter only for 0-7).
pub const DAYTIME_SLOTS: i32 = 9;

/// GUID of the time-of-day upgrade `k` at the time of day `t` (integer like x86, unchecked).
fn daytime_guid(t: i32, k: u32) -> u32 {
    (DAYTIME_BASE as i32)
        .wrapping_add(t)
        .wrapping_add(10 * k as i32) as u32
}

/// `FUN_00482160` (from `GameWorld_ApplyPlayerHandicaps` `0x00484B70`, also script action 173/205;
/// re/upgrades 3.8, W-221): same time of day -> nothing; first remove an old time of day 0...3,
/// then for each slot 0...8 (outer) and `k` 0...5 (inner) apply `4500 + t + 10k` (not incremental).
/// The GUIDs are not defined: effect only counter (0-7) and lists (0-8). Removing only acts
/// on 0-7 ([`remove_upgrade`]), slot 8 keeps its old entries.
pub fn apply_daytime_upgrades(sim: &mut Sim, t: i32) {
    let old = sim.state.daytime;
    if old == t {
        return;
    }
    if (0..4).contains(&old) {
        for p in 0..DAYTIME_SLOTS {
            for k in 0..DAYTIME_UPGRADES {
                remove_upgrade(sim, p, daytime_guid(old, k));
            }
        }
    }
    for p in 0..DAYTIME_SLOTS {
        for k in 0..DAYTIME_UPGRADES {
            apply_upgrade(sim, p, daytime_guid(t, k), false);
        }
    }
    sim.state.daytime = t;
}
