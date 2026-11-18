//! Game end (re/ingame-menus 6, re/script-actions C.2): the
//! player result `GameEngine_SetPlayerResult` (`0x00430CE0`) with follow-up defeats and messages, the
//! command `0x56`, the rule check per engine tick `GameEngine_CheckVictoryDefeat` (`0x00431300`) with
//! `GameTypeRules_Evaluate` (`0x0042C900`), the rule tree `FUN_0042C820` and the leaves
//! `FUN_0042C520`, and the timer list `m_CountDownList` (`GameEngine_TickTimedActions`
//! `0x004409E0`), which `CHECKTIMER` reads.
//!
//! Everything runs over slots 0-7 in order and with integers only. The end screen, the
//! music and the campaign booking of the "local player" branch are done by the host; the simulation reports
//! the result with [`SimEvent::PlayerResult`].

use opennml_formats::gametype_rules::{
    GAMETYPE_COUNT, GameTypeRules, NATION_COUNT, RuleLeaf, RuleNode,
};

use crate::command::reject;
use crate::economy::{announce, count_matching_in_table, message};
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};

/// Result "lost" (`Player+0x358`).
pub const DEFEAT: i32 = 1;
/// Result "won".
pub const VICTORY: i32 = 2;
/// Message "Player p has lost" = `200 + p` (`messages.ini`).
pub const DEFEAT_MESSAGE: i32 = 200;
/// Message "Player p has won" = `210 + p` (`0xD2 + p`).
pub const VICTORY_MESSAGE: i32 = 210;
/// Length of an engine tick in ms for the check counter `GameEngine+0x2CC8` and the timer list.
const STEP_MS: i32 = crate::tick::STEP_MS as i32;

/// `GameEngine_SetPlayerResult(slot, result)` (`0x00430CE0`, re/script-actions C.2):
/// 1. only slots 0-7 that are still playing (result 0); empty slots (3) and decided ones stay;
/// 2. set the result (without checking the value, like the original);
/// 3. defeat: message `200 + slot` to the other slots, only in free play (mode >= 0);
/// 4. victory: every slot 0-7 that is not **mutually** friends with `slot`
///    (`GameWorld_HasFriendRelation` in both directions) loses recursively; mutual friends
///    stay at 0 (O-181). Then message `210 + slot` to the others at mode >= -1;
/// 5. [`SimEvent::PlayerResult`] at the point of the "local player" branch (end of the call).
///
/// The original shows the messages only to the local player if it is not `slot`; the simulation
/// reports them to every other slot, the presentation filters (like [`announce`]).
pub fn set_player_result(sim: &mut Sim, slot: u8, result: i32) {
    let s = usize::from(slot);
    if s >= PLAYER_SLOTS || sim.state.players[s].result != 0 {
        return;
    }
    sim.state.players[s].result = result;
    let mode = sim.state.mode;
    if result == DEFEAT {
        if mode >= 0 {
            tell_others(sim, slot, DEFEAT_MESSAGE);
        }
    } else if result == VICTORY {
        let p = i32::from(slot);
        for q in 0..PLAYER_SLOTS as u8 {
            let d = &sim.state.diplomacy;
            if !(d.is_friend(p, i32::from(q)) && d.is_friend(i32::from(q), p)) {
                set_player_result(sim, q, DEFEAT);
            }
        }
        if mode >= -1 {
            tell_others(sim, slot, VICTORY_MESSAGE);
        }
    }
    sim.state
        .events
        .push(SimEvent::PlayerResult { slot, result });
}

/// Message `base + slot` to every other slot 0-7 (`MessageBar_ShowMessage(…, {−1, −1})`).
fn tell_others(sim: &mut Sim, slot: u8, base: i32) {
    for q in 0..PLAYER_SLOTS as u8 {
        if q != slot {
            announce(
                sim,
                i32::from(q),
                base + i32::from(slot),
                (-1, -1),
                message::flags::GAME_END,
            );
        }
    }
}

/// Command `0x56` (`GameWorld_ExecuteCommand` -> `GameEngine_SetPlayerResult`, re/commands 4.6): slot
/// 0-7 and result 1 or 2 (A8); the sender may name any slot. The script actions
/// 4 (lost) and 13 (won) send it via the script path (E-161,
/// `script::command::player_result`); a slot outside 0-7 is already reported as
/// rejected there before the command (the original silently does nothing).
pub(crate) fn execute(sim: &mut Sim, player: u8, slot: u8, result: i32) {
    if usize::from(slot) >= PLAYER_SLOTS {
        reject(
            sim,
            player,
            "Player result",
            format!("slot {slot} (allowed 0 to 7)"),
        );
        return;
    }
    if result != DEFEAT && result != VICTORY {
        reject(
            sim,
            player,
            "Player result",
            format!("result {result} (allowed 1 lost, 2 won)"),
        );
        return;
    }
    set_player_result(sim, slot, result);
}

/// `GameEngine_CheckVictoryDefeat` (`0x00431300`), per engine tick from `GameEngine_Update`
/// (`0x00445650`; in the reimplementation [`crate::tick::step`] after world and AI, before the level scripts, also
/// during a dialog). The counter `GameEngine+0x2CC8` grows by 50 per tick and is never
/// reset (K-5); checking starts at `> 999`. The reimplementation derives it from `steps` (the tick runs
/// before `steps += 1`), wrapped to 32 bits like the original: first check in the 20th step.
///
/// Two passes over slots 0-7 with result 0: first defeat (`gametypedefeat.ini`) ->
/// result 1, then victory (`gametypevictory.ini`) -> result 2. A result from the first pass
/// or from a follow-up defeat applies at once to the later slots.
pub fn check_victory_defeat(sim: &mut Sim) {
    let counter = (sim.state.steps.wrapping_add(1) as u32).wrapping_mul(STEP_MS as u32) as i32;
    if counter <= 999 {
        return;
    }
    let rules = sim.rules;
    for (table, result) in [(&rules.defeat, DEFEAT), (&rules.victory, VICTORY)] {
        for slot in 0..PLAYER_SLOTS as u8 {
            let p = &sim.state.players[usize::from(slot)];
            let (playing, nation) = (p.result == 0, p.nation);
            if playing && evaluate(sim, table, nation, slot) {
                set_player_result(sim, slot, result);
            }
        }
    }
}

/// `GameTypeRules_Evaluate(game type, nation, slot)` (`0x0042C900`): only game type 0-6 and nation 0-5
/// with a root; otherwise (campaign -1, slot without a rule) false.
pub fn evaluate(sim: &mut Sim, table: &GameTypeRules, nation: i32, slot: u8) -> bool {
    let mode = sim.state.mode;
    let (Ok(m), Ok(n)) = (usize::try_from(mode), usize::try_from(nation)) else {
        return false;
    };
    if m >= GAMETYPE_COUNT || n >= NATION_COUNT {
        return false;
    }
    match &table.rules[m][n] {
        Some(root) => evaluate_node(sim, root, slot),
        None => false,
    }
}

/// `FUN_0042C820`: OR true at the first true child (empty false), AND false at the first false one
/// (empty true); `Not` maps the marker `!` of the parent block (O-198).
fn evaluate_node(sim: &mut Sim, node: &RuleNode, slot: u8) -> bool {
    match node {
        RuleNode::Or(children) => children.iter().any(|c| evaluate_node(sim, c, slot)),
        RuleNode::And(children) => children.iter().all(|c| evaluate_node(sim, c, slot)),
        RuleNode::Not(child) => !evaluate_node(sim, child, slot),
        RuleNode::Leaf(leaf) => evaluate_leaf(sim, leaf, slot),
    }
}

/// Leaf `FUN_0042C520` (re/ingame-menus 6.1). All leaves of the shipped data have their numbers (O-199).
fn evaluate_leaf(sim: &mut Sim, leaf: &RuleLeaf, slot: u8) -> bool {
    let p = i32::from(slot);
    match *leaf {
        RuleLeaf::MatchLowerThan(m, k) => match_count(sim, p, m) < k,
        RuleLeaf::MatchGreaterThan(m, k) => match_count(sim, p, m) > k,
        RuleLeaf::ResourceLowerThan(r, k) => resource(sim, slot, r) < k,
        RuleLeaf::ResourceGreaterThan(r, k) => resource(sim, slot, r) > k,
        RuleLeaf::CheckGuidTrigger(g, n) => guid_trigger(sim, slot, g, n),
        RuleLeaf::CheckPlayerId(q) => p == q,
        RuleLeaf::CheckTimer(t) => countdown_seconds(sim, t) < 1,
        RuleLeaf::CheckEnemy => enemies_in_play(sim, slot) == 0,
        // `FUN_0042E970`: `m_FlagOwner == slot` (`SimState::flag_leader`).
        RuleLeaf::CheckFlagOwner => sim.state.flag_leader == p,
    }
}

/// `FUN_004313B0(slot, m)`: filter `FUN_0047E390(m)` (ID < 1000 without an entry in
/// `flowcontroltragets.ini` -> 0), plus `mode2 |= 4` (quartered count too), then
/// `FUN_00477630` (object table of the slot without the dead).
fn match_count(sim: &mut Sim, slot: i32, m: i32) -> i32 {
    let Some(mut f) = filter_of(sim, m) else {
        return 0;
    };
    f.mode2 |= 4;
    count_matching_in_table(sim, slot, &f)
}

/// `FUN_0047E390(id)`; `None` without an entry (ID < 1000), where the original returns false.
fn filter_of(sim: &Sim, id: i32) -> Option<opennml_formats::targets::MatchFilter> {
    if id < 1000 && sim.rules.flowcontrol_target(id).is_none() {
        return None;
    }
    Some(crate::filter::from_id(sim.rules, id))
}

/// `FUN_0042E880(slot, kind)`: `Player_GetResource(kind)`; a kind outside 0-4 gives 0.
fn resource(sim: &Sim, slot: u8, art: i32) -> i32 {
    usize::try_from(art).map_or(0, |a| sim.state.resource(usize::from(slot), a))
}

/// `FUN_00431490(slot, g, n)`: first object of the object table of the slot for filter `g`
/// (`FUN_00477B30`, table order, reference player -1 = owner), then bit `n & 31` of its
/// event bits (`FUN_0047E7B0`, `GameWorld+0x20C4`); without filter, object or entry false.
fn guid_trigger(sim: &mut Sim, slot: u8, g: i32, n: i32) -> bool {
    let Some(f) = filter_of(sim, g) else {
        return false;
    };
    let first: Option<ObjectId> = crate::upgrades::objects_of_slot(sim.state, slot)
        .into_iter()
        .find(|&id| crate::filter::matches(sim, id, &f, -1));
    first
        .and_then(|id| sim.state.object_event_bits.get(&id))
        .is_some_and(|bits| bits & (1u32 << (n & 31)) != 0)
}

/// `FUN_00431580(t)`: remaining time of timer `t` in whole seconds (`/ 1000`, toward zero);
/// if it is missing, 0 - `CHECKTIMER` is then true (O-180).
pub fn countdown_seconds(sim: &Sim, t: i32) -> i32 {
    countdown_seconds_in(sim.state, t)
}

/// Read-only version of [`countdown_seconds`] on the state (game value tag `[COUNTDOWN n]` of the display, which
/// holds no `&Sim`). The same computation, no state change.
pub fn countdown_seconds_in(state: &SimState, t: i32) -> i32 {
    state.countdowns.get(&t).map_or(0, |&ms| ms / 1000)
}

/// `FUN_0042E900(slot)`: active slots with result 0 for which enmity exists in one of the two
/// directions (`GameWorld_HasEnemyRelation`).
fn enemies_in_play(sim: &Sim, slot: u8) -> i32 {
    let p = i32::from(slot);
    let d = &sim.state.diplomacy;
    (0..PLAYER_SLOTS)
        .filter(|&q| {
            let pl = &sim.state.players[q];
            let q = q as i32;
            pl.slot_active && pl.result == 0 && (d.is_enemy(p, q) || d.is_enemy(q, p))
        })
        .count() as i32
}

/// `GameEngine_TickTimedActions(50)` (`0x004409E0`): every remaining time -50 (32 bits); if it is
/// negative afterwards, the timer is removed (`FUN_00440960`). A timer with remaining time 0 therefore stays
/// one more tick. In the original it runs directly after `GameWorld_Tick` under the same
/// conditions (also during a dialog), before the AI (`GameEngine_GameTick` `0x00441B70`).
pub fn tick_countdowns(sim: &mut Sim) {
    sim.state.countdowns.retain(|_, ms| {
        *ms = ms.wrapping_sub(STEP_MS);
        *ms >= 0
    });
}

#[cfg(test)]
mod tests;
