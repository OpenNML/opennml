//! Player account, costs, counters, GUID groups, resource factor and market (re/buildings, re/garrison-and-commands,
//! re/economy). The account lives in the player ([`Player`], fixed point 20.12 per resource); this is where the
//! computations of the original are, with their check order, rounding (`>> 12` arithmetic, `/` toward 0) and the
//! overflow of `i32`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::fx::fixed_div;
use crate::id::{ObjectId, PLAYER_SLOTS, SLOT_COUNT};
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{Player, SimEvent, SimState};

/// Resource indices of the account (`Player+0x21C`).
pub mod resource {
    pub const GOLD: usize = 0;
    pub const WOOD: usize = 1;
    pub const FOOD: usize = 2;
    /// Occupied population.
    pub const POPULATION: usize = 3;
    /// Available living space (clamped to the living space limit when read).
    pub const LIVING_SPACE: usize = 4;
}

/// Messages to the player (`MessageBar_ShowMessage` `0x00494A60`, `messages.ini`).
pub mod message {
    pub const NO_FOOD: i32 = 0;
    pub const NO_WOOD: i32 = 1;
    pub const NO_GOLD: i32 = 2;
    pub const NO_LIVING_SPACE: i32 = 3;
    pub const FIELD_EMPTY: i32 = 4;
    pub const GOLD_EMPTY: i32 = 5;
    pub const RESEARCH_DONE: i32 = 7;
    pub const REPAIR_TOO_EXPENSIVE: i32 = 8;
    pub const LIVING_SPACE_FULL: i32 = 11;
    /// Message for the first missing resource 0-3 (gold, wood, food, population).
    pub const MISSING: [i32; 4] = [NO_GOLD, NO_WOOD, NO_FOOD, NO_LIVING_SPACE];

    /// Flag word of the message (`MessageBar_ShowMessage` `0x00494A60`, re/game-ui N.7.2): priority
    /// `<< 24`, `0x10000` (queue), byte 1 `[PLAYERNAMEPLID2]`, byte 0 `[PLAYERNAMEPLID1]`.
    /// The caller sets it as in the original; the presentation evaluates it.
    pub mod flags {
        /// Bit 16: the message is queued. Without the bit the bar discards it without sound and
        /// ping (O-42).
        pub const QUEUE: u32 = 0x1_0000;

        /// Priority in the top byte (compared signed, `p << 24`).
        pub const fn priority(p: u32) -> u32 {
            p.wrapping_shl(24)
        }

        /// Player for `[PLAYERNAMEPLID1]`/`[PLAYERNAMEPLID2]` (`(b | 0x100) << 8 | a`): the original
        /// sets bit 8 of `b` and shifts it to bit 16, which also means "queue". Byte 1
        /// thus lies on the sound and ping bits (O-216).
        pub const fn players(a: u32, b: u32) -> u32 {
            (b | 0x100).wrapping_shl(8) | a
        }

        /// `PropertyBuilding_FinishResearch` `0x00504EE0`: priority 2.
        pub const RESEARCH: u32 = priority(2) | QUEUE;
        /// `Player_CanAffordObject` `0x0047D500`: priority 3.
        pub const AFFORD: u32 = priority(3) | QUEUE;
        /// `GameWorld_CmdPlaceBuilding` `0x004899B0`: priority 4.
        pub const PLACE_BUILDINGS: u32 = priority(4) | QUEUE;
        /// `PropertyBuilding_PayTrade` `0x00505170`: priority 1.
        pub const TRADE: u32 = priority(1) | QUEUE;
        /// `PropertyBuilding_OnBuildTick` `0x00508FB0`, repair without resources (message 8).
        pub const REPAIR: u32 = priority(3) | QUEUE;
        /// `PropertyBuilding_OnBuildTick`, living space full (message 11): **not** queued.
        pub const LIVING_SPACE_FULL: u32 = priority(3);
        /// `PropertyBuilding_OnGetGold`/`OnGetFood`: priority 3.
        pub const DEPOSIT: u32 = priority(3) | QUEUE;
        /// `GameEngine_SetPlayerResult` `0x00430CE0`: priority 4.
        pub const GAME_END: u32 = priority(4) | QUEUE;
        /// `GameWorld_CmdSendTribute` `0x00478840`, shortage at the giver: priority 0.
        pub const TRIBUTE_LACK: u32 = QUEUE;
    }
}

/// Config number of the costs per resource index (`Resource_GetCostConfigGuid` `0x004FA930`, table
/// `0x00621AF8`): gold 12, wood 11, food 10, population 22, index 4 reads config 0.
pub const COST_CONFIG: [u32; 5] = [12, 11, 10, 22, 0];

/// Living space limit without a setting and in the campaign (`GameEngine+0x5EC`).
pub const DEFAULT_MAX_LIVING_SPACE: i32 = 125;
/// Living space limit per setting 0-5 (`GameEngine_ApplyGameSettings` `0x00431160`).
pub const LIVING_SPACE_STEPS: [i32; 6] = [75, 100, 125, 150, 175, 200];
/// Start price of wood `Fixed_FromFloat(0.25)` (`GameWorld+0x20BC`, constructor `0x0048CFFC`).
pub const WOOD_PRICE_START: i32 = 1024;
/// Start price of food `Fixed_FromFloat(0.33)` = `ftol(1351.68)` (`GameWorld+0x20C0`).
pub const FOOD_PRICE_START: i32 = 1351;
/// Price limits `Fixed_FromFloat(0.01)` and `Fixed_FromFloat(1.5)`.
pub const PRICE_MIN: i32 = 40;
pub const PRICE_MAX: i32 = 6144;

/// Economy data of the world outside the players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Economy {
    /// Living space limit (`GameEngine+0x5EC`, `GameEngine_GetMaxLivingSpace` `0x0042DA10`).
    pub max_living_space: i32,
    /// Resource factor in percent per player 0-7 (`GameWorld+0x2274`, handicap).
    pub resource_factor: [i32; PLAYER_SLOTS],
    /// Market prices (fixed point, gold per unit of goods): wood (`GameWorld+0x20BC`), food (`+0x20C0`).
    pub wood_price: i32,
    pub food_price: i32,
}

impl Default for Economy {
    fn default() -> Self {
        Self {
            max_living_space: DEFAULT_MAX_LIVING_SPACE,
            resource_factor: [100; PLAYER_SLOTS],
            wood_price: WOOD_PRICE_START,
            food_price: FOOD_PRICE_START,
        }
    }
}

impl Economy {
    /// `GameWorld_GetResourceFactor` (`0x00478600`): outside 0-7 -> 100.
    pub fn resource_factor(&self, slot: i32) -> i32 {
        usize::try_from(slot)
            .ok()
            .and_then(|s| self.resource_factor.get(s))
            .copied()
            .unwrap_or(100)
    }

    /// `GameWorld_SetResourceFactor` (`0x004785E0`): slots 0-7 only.
    pub fn set_resource_factor(&mut self, slot: i32, percent: i32) {
        if let Some(f) = usize::try_from(slot)
            .ok()
            .and_then(|s| self.resource_factor.get_mut(s))
        {
            *f = percent;
        }
    }

    /// Savegame check: no price 0 (the market divides by it, `market_convert`).
    pub fn is_consistent(&self) -> bool {
        self.wood_price != 0 && self.food_price != 0
    }
}

/// `Player_FindUnaffordableResource` (`0x004FAD80`), whole units: gold, wood, food against the
/// stock, population (index 3) against `living space − occupied` (living space clamped); costs of 0 are
/// skipped. Returns: first missing index or -1.
pub fn find_unaffordable_resource(p: &Player, max_living_space: i32, costs: &[i32; 5]) -> i32 {
    for (i, &c) in costs.iter().enumerate().take(4) {
        if c == 0 {
            continue;
        }
        let have = if i == resource::POPULATION {
            p.resource(resource::LIVING_SPACE, max_living_space)
                .wrapping_sub(p.resource(resource::POPULATION, max_living_space))
        } else {
            p.resource(i, max_living_space)
        };
        if c > have {
            return i as i32;
        }
    }
    -1
}

/// `Player_FindUnaffordableCostsPercent` (`0x004FAA50`), fixed point: `cost · pct / 100` (product
/// with overflow, division toward 0) against the raw stock of gold, wood, food.
pub fn find_unaffordable_costs_percent(p: &Player, costs_fx: &[i32; 5], percent: i32) -> i32 {
    for (i, &c) in costs_fx.iter().enumerate().take(3) {
        if c != 0 && c.wrapping_mul(percent) / 100 > p.resource_raw(i) {
            return i as i32;
        }
    }
    -1
}

/// `Player_PayCostsPercent` (`0x004FADF0`): per resource 0-2 raw `+= cost · pct / −100`
/// (toward 0, so at most the truncated share).
pub fn pay_costs_percent(p: &mut Player, costs_fx: &[i32; 5], percent: i32) {
    for (i, &c) in costs_fx.iter().enumerate().take(3) {
        p.add_resource_raw(i, c.wrapping_mul(percent) / -100);
    }
}

/// `Player_PayCosts` (`0x004FB080`): subtract whole units 0-2.
pub fn pay_costs(p: &mut Player, costs: &[i32; 5]) {
    for (i, &c) in costs.iter().enumerate().take(3) {
        p.add_resource(i, c.wrapping_neg());
    }
}

/// `Player_GetObjectCosts` (`0x004FA990`): the five costs from the type value set of the slot
/// ([`crate::rules::Rules::type_value_set`]); `None` without a value set. Unlike
/// `PlayerConfig_GetValue`, a value of 0 in the set of the slot does **not** fall back to `default.dat`:
/// the whole set applies.
pub fn object_costs(sim: &Sim, slot: i32, guid: u32) -> Option<[i32; 5]> {
    let set = sim.rules.type_value_set(slot, guid)?;
    Some(COST_CONFIG.map(|cfg| set.get(&cfg).copied().unwrap_or(0)))
}

/// `Player_GetObjectCostsFixed` (`0x004FA9F0`): the same `<< 12` (overflow as on x86).
pub fn object_costs_fixed(sim: &Sim, slot: i32, guid: u32) -> Option<[i32; 5]> {
    object_costs(sim, slot, guid).map(|c| c.map(|v| v.wrapping_shl(12)))
}

/// Message to a player as an event (the original shows it only to the local player, the caller
/// checks that; the simulation reports it for every player slot 0-7, the presentation filters). Neutral
/// and nature slots (8-10) are never the local player and get no message. `flags` is the flag word of
/// the caller ([`message::flags`]).
pub fn announce(sim: &mut Sim, player: i32, id: i32, tile: (i32, i32), flags: u32) {
    if let Ok(player) = u8::try_from(player)
        && usize::from(player) < PLAYER_SLOTS
    {
        sim.state.events.push(SimEvent::PlayerMessage {
            to: Some(player),
            id,
            tile,
            flags,
        });
    }
}

/// Message to everyone, without checking the local player (`to: None`): callers that in the original call
/// `MessageBar_ShowMessage` without comparing with the local slot (diplomacy `0x00478720`).
pub fn announce_all(sim: &mut Sim, id: i32, tile: (i32, i32), flags: u32) {
    sim.state.events.push(SimEvent::PlayerMessage {
        to: None,
        id,
        tile,
        flags,
    });
}

/// `Player_CanAffordObject` (`0x0047D500`, GameWorld method): without a value set or player `true`;
/// otherwise gold, wood, food, population (against `living space − occupied`) in this order, costs
/// of 0 skipped; the first shortage reports (unless `silent`) message 2/1/0/3 at the tile and returns
/// `false`.
pub fn can_afford_object(
    sim: &mut Sim,
    slot: i32,
    guid: u32,
    silent: bool,
    tile: (i32, i32),
) -> bool {
    let Some(costs) = object_costs(sim, slot, guid) else {
        return true;
    };
    let max = sim.state.economy.max_living_space;
    let Some(p) = usize::try_from(slot)
        .ok()
        .and_then(|s| sim.state.players.get(s))
    else {
        return true;
    };
    for (i, &c) in costs.iter().enumerate().take(4) {
        if c == 0 {
            continue;
        }
        let mut have = p.resource(i, max);
        if i == resource::POPULATION {
            have = p.resource(resource::LIVING_SPACE, max).wrapping_sub(have);
        }
        if c > have {
            if !silent {
                announce(sim, slot, message::MISSING[i], tile, message::flags::AFFORD);
            }
            return false;
        }
    }
    true
}

/// `Player_AddObjectCostsPercent` (`0x0047D650`): per resource 0-2 with cost != 0
/// `AddResource(cost · pct / 100)` (whole units, toward 0); `pct = −100` pays, a
/// positive percentage refunds. The caller books the population itself.
pub fn add_object_costs_percent(sim: &mut Sim, slot: i32, guid: u32, percent: i32) -> bool {
    let Some(costs) = object_costs(sim, slot, guid) else {
        return true;
    };
    let Some(p) = usize::try_from(slot)
        .ok()
        .and_then(|s| sim.state.players.get_mut(s))
    else {
        return true;
    };
    for (i, &c) in costs.iter().enumerate().take(3) {
        if c != 0 {
            p.add_resource(i, c.wrapping_mul(percent) / 100);
        }
    }
    true
}

/// `GameObject_CanOwnerAffordObject` (`0x00469340`): for the owner slot of the ID, message at
/// the tile of the object.
pub fn can_owner_afford_object(sim: &mut Sim, id: ObjectId, guid: u32, silent: bool) -> bool {
    let tile = sim.object(id).map_or((0, 0), |o| o.tile);
    can_afford_object(sim, i32::from(id.slot()), guid, silent, tile)
}

/// `GameObject_AddOwnerObjectCostsPercent` (`0x00466980`).
pub fn add_owner_object_costs_percent(
    sim: &mut Sim,
    id: ObjectId,
    guid: u32,
    percent: i32,
) -> bool {
    add_object_costs_percent(sim, i32::from(id.slot()), guid, percent)
}

/// Greatest depth of the group resolution (protection of the stack, exception A7); a deeper acyclic
/// chain counts 0 in the deeper branch. The shipped data has depth 1.
const MAX_GROUP_DEPTH: usize = 256;

/// One-time warning about `guidgroups.ini`: `cycle` = group contains itself (indirectly)
/// (original: endless recursion), otherwise the chain is deeper than [`MAX_GROUP_DEPTH`].
fn warn_group_cut(guid: u32, cycle: bool) {
    static WARNED_CYCLE: AtomicBool = AtomicBool::new(false);
    static WARNED_DEPTH: AtomicBool = AtomicBool::new(false);
    if cycle {
        if !WARNED_CYCLE.swap(true, Ordering::Relaxed) {
            log::warn!(
                "guidgroups.ini: group {guid} contains itself indirectly; branch counts 0 (original: endless recursion)"
            );
        }
    } else if !WARNED_DEPTH.swap(true, Ordering::Relaxed) {
        log::warn!(
            "guidgroups.ini: group chain at {guid} deeper than {MAX_GROUP_DEPTH} levels; the deeper branch counts 0"
        );
    }
}

/// Counting with groups: a group GUID counts the sum of its members (recursive, sum with
/// overflow), its own entry is never read. The kill counter reads this way too
/// ([`crate::world::kill_count`], `FUN_0047E7F0`).
pub(crate) fn count_with_groups(
    counts: &BTreeMap<u32, i32>,
    groups: &BTreeMap<u32, Vec<u32>>,
    guid: u32,
    path: &mut Vec<u32>,
) -> i32 {
    let Some(members) = groups.get(&guid) else {
        return counts.get(&guid).copied().unwrap_or(0);
    };
    // Cycle: the group is already on its own path (not merely several times in the tree).
    if path.contains(&guid) {
        warn_group_cut(guid, true);
        return 0;
    }
    if path.len() >= MAX_GROUP_DEPTH {
        warn_group_cut(guid, false);
        return 0;
    }
    path.push(guid);
    let sum = members.iter().fold(0i32, |s, &m| {
        s.wrapping_add(count_with_groups(counts, groups, m, path))
    });
    path.pop();
    sum
}

/// `Player_GetGuidCount` (`0x0047DD90`): object counter (`GameWorld+0x9BC`, finished objects and
/// applied upgrades) of slots 0-7, groups from `guidgroups.ini` resolved; otherwise 0.
pub fn guid_count(sim: &Sim, slot: i32, guid: u32) -> i32 {
    guid_count_in(sim.state, sim.rules, slot, guid)
}

/// Read-only version of [`guid_count`] on what the count really reads (state and rules): the display (game value tags
/// of the text layout, `[GUIDCOUNTER a b]`) holds only `&SimState`/`&Rules`, no `&Sim`. The same computation, no state
/// change.
pub fn guid_count_in(state: &SimState, rules: &Rules, slot: i32, guid: u32) -> i32 {
    let Some(counts) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| state.guid_counts.get(s))
    else {
        return 0;
    };
    count_with_groups(counts, &rules.guid_groups, guid, &mut Vec::new())
}

/// `Player_GetPendingGuidCount` (`0x0047DCE0`): counter "in progress" (`GameWorld+0xAB8`: construction sites,
/// queued production), otherwise like [`guid_count`].
pub fn pending_guid_count(sim: &Sim, slot: i32, guid: u32) -> i32 {
    pending_guid_count_in(sim.state, sim.rules, slot, guid)
}

/// Read-only version of [`pending_guid_count`] on state and rules (like [`guid_count_in`]; the bar hides one-time
/// buttons with a running order). No state change.
pub fn pending_guid_count_in(state: &SimState, rules: &Rules, slot: i32, guid: u32) -> i32 {
    let Some(counts) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| state.pending_counts.get(s))
    else {
        return 0;
    };
    count_with_groups(counts, &rules.guid_groups, guid, &mut Vec::new())
}

/// The heroes of slot `slot` in the order of the object table (`GameWorld+0x234[slot]`, D-232): a run over the table
/// from index 0 up to the capacity (`+0x2B8` is the capacity, not the occupancy), every occupied object whose type is
/// in the hero list (`GameWorld_GetHeroIndex` `0x0047BD30` >= 0, [`Rules::hero_index`]). The sequence is **not** the
/// creation order: allocation is a ring from the last index ([`crate::state::SlotTable::alloc`]), a free index is
/// reused, a change of owner puts the object at the ring index of the target table. Dead and quartered heroes stay in
/// the table until destruction and count (the run of the original only checks whether the entry is occupied). Not meant
/// is the hard-coded hero list of the AI (`ki::builtins::arith::is_hero_type`). Invalid slot: empty. No state change.
pub fn heroes_of(state: &SimState, rules: &Rules, slot: u8) -> Vec<ObjectId> {
    let Some(table) = state.tables.get(usize::from(slot)) else {
        return Vec::new();
    };
    table
        .objects
        .iter()
        .flatten()
        .filter(|o| rules.hero_index(o.guid) >= 0)
        .map(|o| o.id)
        .collect()
}

/// Common run of `FUN_00477630` and `FUN_0047EA20`: the objects of the object table of slot `slot` (slots
/// `0..slot_limit`, otherwise empty) in table order that pass `filter::matches(f, reference)`. The IDs are copied
/// beforehand because the filter takes `&mut Sim`.
fn collect_in_table_below(
    sim: &mut Sim,
    slot: i32,
    slot_limit: usize,
    f: &MatchFilter,
    reference: i32,
) -> Vec<ObjectId> {
    let Some(table) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < slot_limit)
        .and_then(|s| sim.state.tables.get(s))
    else {
        return Vec::new();
    };
    let ids: Vec<ObjectId> = table.objects.iter().flatten().map(|o| o.id).collect();
    ids.into_iter()
        .filter(|&id| crate::filter::matches(sim, id, f, reference))
        .collect()
}

/// `FUN_00477630(p, f)` (`GameWorld` method, re/ki-queries 5.12.1) as a list: the objects of the **object table of the
/// slot** (`GameWorld+0x234[p]`) in table order that pass `filter::matches(f, slot)` - without the dead (except
/// `MM_RETURN_DIED`) and without the quartered (except `mode2 & 4`). `slot` counts as `u32` and must be `< 8`,
/// otherwise the list is empty.
pub fn collect_matching_in_table(sim: &mut Sim, slot: i32, f: &MatchFilter) -> Vec<ObjectId> {
    collect_in_table_below(sim, slot, PLAYER_SLOTS, f, slot)
}

/// `FUN_0047EA20(p, f, rect)` (re/ki-queries 5.10) without the rectangle check: like [`collect_matching_in_table`], but
/// the slot may be 0-10 (all object tables, also neutral on 8 and nature on 10) and the reference player of the filter
/// is `−1` (= owner of the object). For a slot > 10 the original reads behind the table (A7): empty here. Only for
/// `0x50D2`; the count codes read only 0-7 via [`collect_matching_in_table`].
pub fn collect_matching_in_slot_table(sim: &mut Sim, slot: i32, f: &MatchFilter) -> Vec<ObjectId> {
    collect_in_table_below(sim, slot, SLOT_COUNT, f, -1)
}

/// [`collect_matching_in_table`] as a number (`FUN_00477630`).
pub fn count_matching_in_table(sim: &mut Sim, slot: i32, f: &MatchFilter) -> i32 {
    i32::try_from(collect_matching_in_table(sim, slot, f).len()).unwrap_or(i32::MAX)
}

/// `FUN_004303F0(p, id, with_pending)` (re/ki-queries 5.12.1): `id ≥ 1000` is a type GUID and counts
/// `Player_GetGuidCount` (with quartered and groups) plus, with `with_pending`, `Player_GetPendingGuidCount` (32 bit,
/// with overflow); otherwise (also negative) a filter ID ([`crate::filter::from_id`]) and the table of the slot is
/// counted ([`count_matching_in_table`], without dead and quartered; `with_pending` has no effect there; an unknown ID
/// gives 0, E-66).
pub fn count_guid_or_filter(sim: &mut Sim, slot: i32, id: i32, with_pending: bool) -> i32 {
    if id >= 1000 {
        let guid = id as u32;
        let have = guid_count(sim, slot, guid);
        return if with_pending {
            have.wrapping_add(pending_guid_count(sim, slot, guid))
        } else {
            have
        };
    }
    let f = crate::filter::from_id(sim.rules, id);
    count_matching_in_table(sim, slot, &f)
}

/// `Player_AddGuidCount` (`0x00481190`): change the counter "in progress" of a slot 0-7.
pub fn add_pending_guid_count(state: &mut SimState, slot: i32, guid: u32, delta: i32) {
    if let Some(map) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| state.pending_counts.get_mut(s))
    {
        let v = map.entry(guid).or_insert(0);
        *v = v.wrapping_add(delta);
    }
}

/// `GameObject_AddOwnerGuidCount` (`0x00466F50`): for the owner slot of the ID.
pub fn add_owner_pending_guid_count(state: &mut SimState, id: ObjectId, guid: u32, delta: i32) {
    add_pending_guid_count(state, i32::from(id.slot()), guid, delta);
}

/// `Market_ConvertWoodGold` (`0x00477970`) / `Market_ConvertFoodGold` (`0x004779C0`): `n > 0`
/// goods -> gold `(price · n) >> 12` (product with overflow, arithmetic); otherwise gold `−n` -> goods
/// `Fixed_Div(−n << 12, price) >> 12`.
pub fn market_convert(price: i32, n: i32) -> i32 {
    if n > 0 {
        price.wrapping_mul(n) >> 12
    } else {
        fixed_div(n.wrapping_neg().wrapping_shl(12), price) >> 12
    }
}

/// `Market_AdjustWoodPrice` (`0x00477AA0`) / `Market_AdjustFoodPrice` (`0x00477A10`):
/// `price += d / 25` (toward 0), then limited to [`PRICE_MIN`] ... [`PRICE_MAX`].
pub fn market_adjust(price: &mut i32, d: i32) {
    *price = price.wrapping_add(d / 25).clamp(PRICE_MIN, PRICE_MAX);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::Fx;
    use crate::rules::Rules;
    use std::collections::BTreeMap;

    /// Player with whole stocks of gold, wood, food, occupied, living space.
    fn player(r: [i32; 5]) -> Player {
        let mut p = Player::new(0);
        for (i, &v) in r.iter().enumerate() {
            p.set_resource(i, v);
        }
        p
    }

    fn rules() -> Rules {
        let mut rules = Rules::synthetic("", 16);
        // House 1001 (150 wood), colonist 1100 (60 food, population 1), from `default.dat`.
        rules.defaults = BTreeMap::from([
            (1001, BTreeMap::from([(11, 150), (13, 20), (27, 15)])),
            (1100, BTreeMap::from([(10, 60), (13, 15), (22, 1)])),
        ]);
        rules
    }

    #[test]
    fn account_checks_follow_the_order_and_clamp_the_living_space() {
        // Gold 10, wood 150, population 4 of 5.
        let p = player([10, 150, 0, 4, 5]);
        assert_eq!(find_unaffordable_resource(&p, 125, &[0, 150, 0, 0, 0]), -1);
        assert_eq!(find_unaffordable_resource(&p, 125, &[0, 0, 60, 1, 0]), 2);
        assert_eq!(find_unaffordable_resource(&p, 125, &[0, 0, 0, 2, 0]), 3);
        assert_eq!(find_unaffordable_resource(&p, 125, &[11, 151, 0, 0, 0]), 0);
        // Living space 200 at limit 125: clamped; population 124 leaves room for 1.
        let big = player([0, 0, 0, 124, 200]);
        assert_eq!(big.resource(resource::LIVING_SPACE, 125), 125);
        assert_eq!(big.resource_raw(resource::LIVING_SPACE), 200 << 12);
        assert_eq!(find_unaffordable_resource(&big, 125, &[0, 0, 0, 1, 0]), -1);
        assert_eq!(find_unaffordable_resource(&big, 125, &[0, 0, 0, 2, 0]), 3);
        // Negative stock rounds toward -∞ when read (SAR); index out of range: 0.
        let mut neg = player([0; 5]);
        neg.add_resource_raw(0, -1);
        assert_eq!((neg.resource(0, 125), neg.resource(7, 125)), (-1, 0));
        // By percentage (fixed point, toward 0): repair step 2 % on 307 raw per tick.
        let mut w = player([0, 1, 0, 0, 0]);
        let cpt = [0, 307, 0, 0, 0];
        assert_eq!(find_unaffordable_costs_percent(&w, &cpt, 200), -1);
        pay_costs_percent(&mut w, &cpt, 200);
        assert_eq!(w.resource_raw(1), 4096 - 614);
        assert_eq!(
            find_unaffordable_costs_percent(&w, &[0, 4096, 0, 0, 0], 100),
            1
        );
        pay_costs(&mut w, &[0, 0, 0, 5, 0]);
        assert_eq!(w.resource_raw(3), 0, "PayCosts books only 0-2");
    }

    #[test]
    fn affordability_reports_the_first_missing_resource_and_refunds_truncate() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.players[0] = player([10, 150, 0, 4, 5]);
        let mut sim = Sim::new(&mut state, &rules);
        assert!(can_afford_object(&mut sim, 0, 1001, false, (3, 4)));
        assert!(sim.state.events.is_empty());
        assert!(!can_afford_object(&mut sim, 0, 1100, true, (3, 4)));
        assert!(sim.state.events.is_empty(), "silent");
        assert!(!can_afford_object(&mut sim, 0, 1100, false, (3, 4)));
        assert_eq!(
            sim.state.events,
            [SimEvent::PlayerMessage {
                to: Some(0),
                id: message::NO_FOOD,
                tile: (3, 4),
                flags: message::flags::AFFORD,
            }]
        );
        // Without a value set or player: affordable.
        assert!(can_afford_object(&mut sim, 0, 4711, false, (0, 0)));
        assert!(can_afford_object(&mut sim, 9, 1100, false, (0, 0)));
        // Pay for a colonist (-100 %), abort after 51 ticks: remaining 10000 of 15000 ms -> 66 %
        // -> +39 food (60 * 66 / 100 = 39.6 truncated).
        sim.state.players[0].set_resource(resource::FOOD, 200);
        add_object_costs_percent(&mut sim, 0, 1100, -100);
        assert_eq!(sim.state.players[0].resource(resource::FOOD, 125), 140);
        let pct = 10_000 * 100 / 15_000;
        assert_eq!(pct, 66);
        add_object_costs_percent(&mut sim, 0, 1100, pct);
        assert_eq!(sim.state.players[0].resource(resource::FOOD, 125), 179);
        assert_eq!(
            sim.state.players[0].resource(resource::POPULATION, 125),
            4,
            "the caller books the population"
        );
        assert_eq!(
            object_costs_fixed(&sim, 0, 1001),
            Some([0, 150 << 12, 0, 0, 0])
        );
    }

    #[test]
    fn object_costs_take_the_whole_type_set_of_the_slot() {
        let mut rules = rules();
        // Slot 3 has its own set for the house with wood 0: GetTypeValueSet takes the
        // whole set (cost 0), PlayerConfig_GetValue falls back to default.dat value by value.
        rules.level.slot_configs = BTreeMap::from([
            (
                3,
                BTreeMap::from([(1001, BTreeMap::from([(11, 0), (13, 20)]))]),
            ),
            (8, BTreeMap::from([(1100, BTreeMap::from([(10, 70)]))])),
        ]);
        let mut state = SimState::new(0, 0);
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(object_costs(&sim, 3, 1001), Some([0; 5]));
        assert_eq!(sim.rules.config_value(3, 1001, 11), 150);
        assert_eq!(object_costs(&sim, 0, 1001), Some([0, 150, 0, 0, 0]));
        // Slots from 8 read set 8 ([NEUTRAL]).
        assert_eq!(object_costs(&sim, 10, 1100), Some([0, 0, 70, 0, 0]));
        assert_eq!(object_costs(&sim, 0, 4711), None);
    }

    #[test]
    fn market_prices_follow_the_trade_sequence() {
        assert_eq!(
            [0.25f32, 0.33, 0.01, 1.5].map(|v| Fx::from_f32(v).0),
            [WOOD_PRICE_START, FOOD_PRICE_START, PRICE_MIN, PRICE_MAX]
        );
        let e = Economy::default();
        let (mut wood, mut food) = (e.wood_price, e.food_price);
        // Buying 490 wood twice: 25 gold each, price +4.
        assert_eq!(market_convert(wood, 100), 25);
        market_adjust(&mut wood, 100);
        assert_eq!(wood, 1028);
        assert_eq!(market_convert(wood, 100), 25);
        market_adjust(&mut wood, 100);
        assert_eq!(wood, 1032);
        // Buying 491 food: 32 gold.
        assert_eq!(market_convert(food, 100), 32);
        market_adjust(&mut food, 100);
        assert_eq!(food, 1355);
        // Selling 492 wood: 396 wood for 100 gold, price -396/25 = -15.
        let w = market_convert(wood, -100);
        assert_eq!(w, 396);
        market_adjust(&mut wood, -w);
        assert_eq!(wood, 1017);
        // Selling 493 food: 302 food.
        let f = market_convert(food, -100);
        assert_eq!(f, 302);
        market_adjust(&mut food, -f);
        assert_eq!(food, 1343);
        // Limits.
        assert_eq!(market_convert(PRICE_MIN, -100), 10240);
        assert_eq!(market_convert(PRICE_MAX, 100), 150);
        let mut p = 50;
        market_adjust(&mut p, -1000);
        assert_eq!(p, PRICE_MIN);
        market_adjust(&mut p, 1_000_000);
        assert_eq!(p, PRICE_MAX);
        assert!(e.is_consistent());
        assert!(
            !Economy {
                wood_price: 0,
                ..e.clone()
            }
            .is_consistent()
        );
        // Resource factor: default 100, outside 0-7 always 100.
        let mut e = e;
        e.set_resource_factor(2, 80);
        e.set_resource_factor(9, 10);
        assert_eq!(
            (
                e.resource_factor(2),
                e.resource_factor(0),
                e.resource_factor(9),
                e.resource_factor(-1)
            ),
            (80, 100, 100, 100)
        );
    }

    #[test]
    fn guid_counts_resolve_groups_recursively_and_cut_cycles() {
        let mut rules = Rules::synthetic("", 16);
        rules.guid_groups = BTreeMap::from([
            (1114, vec![1113]),
            (3411, vec![3412, 3413]),
            (50, vec![51, 3411]),
            (60, vec![61, 62]),
            (62, vec![60, 61]),
        ]);
        let mut state = SimState::new(0, 0);
        for (g, n) in [
            (1113, 2),
            (3412, 1),
            (3413, 4),
            (51, 5),
            (61, 7),
            (1114, 99),
        ] {
            state.add_guid_count(2, g, n);
        }
        add_pending_guid_count(&mut state, 2, 3412, 3);
        add_pending_guid_count(&mut state, 2, 3412, -1);
        add_pending_guid_count(&mut state, 9, 3412, 1);
        add_owner_pending_guid_count(&mut state, ObjectId::new(0, 2, 1), 51, 1);
        let sim = Sim::new(&mut state, &rules);
        // Group instead of own entry; nested.
        assert_eq!(guid_count(&sim, 2, 1114), 2);
        assert_eq!(guid_count(&sim, 2, 3411), 5);
        assert_eq!(guid_count(&sim, 2, 50), 10);
        // Cycle 60 -> 62 -> 60: the way back counts 0, the other members count (7 + 7).
        assert_eq!(guid_count(&sim, 2, 60), 14);
        // Slots outside 0-7: 0.
        assert_eq!(
            (guid_count(&sim, 8, 1113), guid_count(&sim, -1, 1113)),
            (0, 0)
        );
        assert_eq!(pending_guid_count(&sim, 2, 3411), 2);
        assert_eq!(pending_guid_count(&sim, 2, 50), 3);
        assert_eq!(pending_guid_count(&sim, 2, 1113), 0);
    }

    #[test]
    fn living_space_full_is_not_queued() {
        // `PropertyBuilding_OnBuildTick` `0x00508FB0`: message 11 with `0x03000000` (without `0x10000`),
        // all other callers queue.
        use message::flags::*;
        assert_eq!(LIVING_SPACE_FULL, 0x0300_0000);
        assert_eq!(LIVING_SPACE_FULL & QUEUE, 0);
        for f in [
            RESEARCH,
            AFFORD,
            PLACE_BUILDINGS,
            TRADE,
            REPAIR,
            DEPOSIT,
            GAME_END,
            TRIBUTE_LACK,
        ] {
            assert_eq!(f & QUEUE, QUEUE, "{f:#x}");
        }
        assert_eq!(
            [
                RESEARCH,
                AFFORD,
                PLACE_BUILDINGS,
                TRADE,
                REPAIR,
                DEPOSIT,
                GAME_END,
                TRIBUTE_LACK
            ],
            [
                0x0201_0000,
                0x0301_0000,
                0x0401_0000,
                0x0101_0000,
                0x0301_0000,
                0x0301_0000,
                0x0401_0000,
                0x0001_0000
            ]
        );
    }

    #[test]
    fn player_bytes_of_the_flags_follow_the_original() {
        // `(b | 0x100) << 8 | a`: byte 1 lies on bit 16 (O-216).
        use message::flags::players;
        assert_eq!(players(2, 5), 0x1_0502);
        assert_eq!(players(0, 0), 0x1_0000);
        assert_eq!(players(7, 4), 0x1_0407);
    }

    #[test]
    fn messages_go_only_to_player_slots_0_to_7() {
        // The original shows messages only to the local player (`GameEngine_GetLocalPlayer`,
        // comparison of the slots in `0x0047D500`), and that is always a slot 0-7.
        let rules = rules();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        for slot in [8, 9, 10, -1] {
            announce(&mut sim, slot, message::NO_FOOD, (1, 2), 0);
        }
        assert!(sim.state.events.is_empty());
        announce(&mut sim, 7, message::NO_FOOD, (1, 2), 0x1234_0000);
        assert_eq!(
            sim.state.events,
            [SimEvent::PlayerMessage {
                to: Some(7),
                id: message::NO_FOOD,
                tile: (1, 2),
                flags: 0x1234_0000,
            }]
        );
    }

    #[test]
    fn deep_acyclic_group_chains_count_fully() {
        // Chain 10 -> 11 -> ... -> 49 (40 levels, acyclic), at the end the member 7000.
        let mut rules = Rules::synthetic("", 16);
        rules.guid_groups = (10u32..50)
            .map(|g| (g, vec![if g == 49 { 7000 } else { g + 1 }]))
            .collect();
        let mut state = SimState::new(0, 0);
        state.add_guid_count(1, 7000, 3);
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(guid_count(&sim, 1, 10), 3);
        assert_eq!(guid_count(&sim, 1, 30), 3);
    }

    mod counting {
        use super::*;
        use crate::module::building::fort_testkit::{self as kit, SOLDIER, spawn};
        use crate::object::status;
        use nml_formats::targets::{MatchFilter, parse_target_defines, parse_targets};

        /// Filter ID 5: all objects of the reference player (`MM_ANDALL`, `MM_HL_MYLIST`).
        const FLOWCONTROL: &str = "BEGIN
BUTTONMSG 5
MY_MM
MM_MODE MM_ANDALL
MM_MODE MM_HL_MYLIST
MM_END
TARGET_MM
MM_END
END
";

        fn rules() -> Rules {
            let mut rules = kit::rules();
            rules.flowcontrol_targets =
                parse_targets(FLOWCONTROL, &parse_target_defines("")).entries;
            rules
        }

        fn mine() -> MatchFilter {
            MatchFilter {
                mode: 0x110,
                ..MatchFilter::constructed()
            }
        }

        #[test]
        fn count_matching_in_table_counts_only_the_slot_and_skips_quartered() {
            let rules = rules();
            let mut state = kit::state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let a = spawn(&mut sim, SOLDIER, 0, (10, 10));
            let b = spawn(&mut sim, SOLDIER, 0, (11, 10));
            let dead = spawn(&mut sim, SOLDIER, 0, (12, 10));
            let quartered = spawn(&mut sim, SOLDIER, 0, (13, 10));
            let other = spawn(&mut sim, SOLDIER, 1, (14, 10));
            sim.object_mut(dead).unwrap().status |= status::DIED;
            sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
            // Table order, without dead and quartered (filter with `mode2 = 0`).
            assert_eq!(collect_matching_in_table(&mut sim, 0, &mine()), [a, b]);
            assert_eq!(count_matching_in_table(&mut sim, 0, &mine()), 2);
            assert_eq!(collect_matching_in_table(&mut sim, 1, &mine()), [other]);
            // `MM2_DONOTIGNOREQUARTEREDUNITS` (`mode2 & 4`) brings the quartered back.
            let with_quartered = MatchFilter {
                mode2: 0x4,
                ..mine()
            };
            assert_eq!(count_matching_in_table(&mut sim, 0, &with_quartered), 3);
            // Slot as `u32` < 8: -1 and 8 count nothing, not even for slot 8 with objects.
            spawn(&mut sim, SOLDIER, 8, (20, 20));
            let any = MatchFilter {
                mode: 0x10,
                ..MatchFilter::constructed()
            };
            for slot in [-1, 8, 9, 1000, i32::MIN] {
                assert_eq!(count_matching_in_table(&mut sim, slot, &any), 0, "{slot}");
                assert!(collect_matching_in_table(&mut sim, slot, &any).is_empty());
            }
        }

        #[test]
        fn count_guid_or_filter_adds_pending_only_for_guids() {
            let rules = rules();
            let mut state = kit::state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            for tile in [(10, 10), (11, 10), (12, 10)] {
                spawn(&mut sim, SOLDIER, 0, tile);
            }
            let quartered = spawn(&mut sim, SOLDIER, 0, (13, 10));
            sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
            // The object counter (kept by `create_object`) counts the quartered too (E-66),
            // the table does not.
            add_pending_guid_count(sim.state, 0, SOLDIER, 3);
            assert_eq!(count_guid_or_filter(&mut sim, 0, SOLDIER as i32, false), 4);
            assert_eq!(count_guid_or_filter(&mut sim, 0, SOLDIER as i32, true), 7);
            // A filter ID counts the table; "with pending" has no effect there.
            assert_eq!(count_guid_or_filter(&mut sim, 0, 5, false), 3);
            assert_eq!(count_guid_or_filter(&mut sim, 0, 5, true), 3);
            // Unknown filter IDs (also 0 and negative) and invalid slots: 0.
            for id in [0, -5, 999, 6] {
                assert_eq!(count_guid_or_filter(&mut sim, 0, id, true), 0, "{id}");
            }
            assert_eq!(count_guid_or_filter(&mut sim, 8, 5, false), 0);
            // Invalid slot for a GUID: the counter returns 0.
            assert_eq!(count_guid_or_filter(&mut sim, -1, SOLDIER as i32, true), 0);
            // The counter overflows without panicking.
            sim.state.add_guid_count(0, SOLDIER, i32::MAX);
            let _ = count_guid_or_filter(&mut sim, 0, SOLDIER as i32, true);
        }
    }

    #[test]
    fn heroes_come_in_object_table_order() {
        // D-232: the hero row of the bar follows the object table of the slot (ring from the last index,
        // gaps when destroying, free indices are reused), not the creation order.
        use crate::upgrades::testkit::{COLONIST, GOVERNOR, MISSIONARY, rules, spawn, state};
        let mut rules = rules();
        rules.heroes = vec![GOVERNOR, MISSIONARY];
        let mut st = state(&rules);
        let mut sim = Sim::new(&mut st, &rules);
        let guid_of = |sim: &Sim, id: ObjectId| sim.object(id).unwrap().guid;
        let heroes = |sim: &Sim, slot: u8| heroes_of(sim.state, sim.rules, slot);

        // Creating: index 0 governor, 1 colonist (no hero), 2 missionary, 3 governor.
        let h0 = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        let worker = spawn(&mut sim, COLONIST, 0, (11, 10));
        let h2 = spawn(&mut sim, MISSIONARY, 0, (12, 10));
        let h3 = spawn(&mut sim, GOVERNOR, 0, (13, 10));
        assert_eq!([h0, worker, h2, h3].map(|id| id.index()), [0, 1, 2, 3]);
        assert_eq!(heroes(&sim, 0), [h0, h2, h3]);
        assert!(heroes(&sim, 1).is_empty());
        assert!(heroes(&sim, 9).is_empty(), "invalid slot");

        // Destroying: gap at index 2, the others keep their indices.
        crate::world::destroy_object(&mut sim, h2);
        assert_eq!(heroes(&sim, 0), [h0, h3]);

        // Reusing: if the ring points to the free index 2, the new hero stands before h3 - although it
        // was created later.
        sim.state.tables[0].ring = 0;
        let h2b = spawn(&mut sim, MISSIONARY, 0, (14, 10));
        assert_eq!(h2b.index(), 2);
        assert_eq!(heroes(&sim, 0), [h0, h2b, h3]);
        // Without a reset the ring continues: the next hero comes after h3.
        let h4 = spawn(&mut sim, GOVERNOR, 0, (15, 10));
        assert_eq!(h4.index(), 4);
        assert_eq!(heroes(&sim, 0), [h0, h2b, h3, h4]);

        // Change of owner: slot 1 has heroes at index 0 and 1; index 0 becomes free, the ring points there.
        let p0 = spawn(&mut sim, MISSIONARY, 1, (30, 10));
        let p1 = spawn(&mut sim, GOVERNOR, 1, (31, 10));
        assert_eq!(heroes(&sim, 1), [p0, p1]);
        crate::world::destroy_object(&mut sim, p0);
        sim.state.tables[1].ring = 0;
        let moved = crate::world::change_owner(&mut sim, h3, 1).expect("owner change");
        assert_eq!((moved.slot(), moved.index()), (1, 0));
        assert_eq!(guid_of(&sim, moved), GOVERNOR);
        // The old slot loses the hero (gap at index 3), the new one holds it at its ring index.
        assert_eq!(heroes(&sim, 0), [h0, h2b, h4]);
        assert_eq!(heroes(&sim, 1), [moved, p1]);
        // A hero that would only be quartered or dead would stay in the table: no filtering by state.
        assert_eq!(sim.state.tables[1].count, 2);
    }
}
