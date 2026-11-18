//! Kill counter of the scripts (`m_BODYCounter`, `GameWorld+0x20E0 + p*0x1C`, E-189):
//! per slot 0-7 type GUID of the victim -> count. Written on dying (`PropertyDying_StartDying`,
//! normal death and ghost), on destroying a building and on the success of a headhunter;
//! read by query 1200 and the text markup `[BODYCOUNTER s g]`.

use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::SimState;

/// `FUN_00481680(p, guid, 1)`: increase counter `guid` of the slot `slot` by 1 (a missing entry
/// counts as 0, sum with overflow). A `slot` outside 0...7 is discarded (slots 8-10, 31 = no attacker,
/// negative ones).
pub fn add_kill_count(sim: &mut Sim, slot: i32, guid: u32) {
    let Some(counts) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| sim.state.kill_counts.get_mut(s))
    else {
        return;
    };
    let c = counts.entry(guid).or_insert(0);
    *c = c.wrapping_add(1);
}

/// Slot for which a death counts: `(last attacker SAR 11) & 0x1F` (`+0x1B8`); without an attacker
/// (`-1`) 31, which [`add_kill_count`] discards.
pub fn kill_slot(last_attacker: ObjectId) -> i32 {
    i32::from(last_attacker.slot())
}

/// `FUN_0047E7F0(p, guid)`: kills of `guid` by slot `slot`; a group GUID from
/// `guidgroups.ini` counts the sum of its members (its own entry is not read), with
/// the same group resolution as the object counter ([`crate::economy::guid_count`], cycle cut instead of
/// endless recursion). A `slot` outside 0...7 -> 0.
pub fn kill_count(sim: &Sim, slot: i32, guid: u32) -> i32 {
    kill_count_in(sim.state, sim.rules, slot, guid)
}

/// Reading version of [`kill_count`] on state and rules (the display holds no `&Sim`, game value tag
/// `[BODYCOUNTER a b]`). The same calculation, no state change.
pub fn kill_count_in(state: &SimState, rules: &Rules, slot: i32, guid: u32) -> i32 {
    let Some(counts) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| state.kill_counts.get(s))
    else {
        return 0;
    };
    crate::economy::count_with_groups(counts, &rules.guid_groups, guid, &mut Vec::new())
}
