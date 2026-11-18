//! Mission objectives: the actions 27, 182 (set objective), 184-187 (objective done or
//! failed) and 198, 199 (prefill objectives). Source: D cases `0x15`, `0x47`, `0x49`-`0x4C`, `0x56`,
//! `0x57` with the setters `FUN_0042DDC0` (main objective text), `FUN_0042DDF0` (secondary objective text),
//! `FUN_0042DE20` (main objective status), `FUN_0042DE40` (secondary objective status) on `m_MainQuest`
//! `GameEngine+0x50C` and `m_SubQuest` `+0x55C` (re/campaign-ui 4.5, E-204).
//!
//! The script number is 1-based. All four setters check `0 <= i < 10` and otherwise silently do nothing
//! (no `Guarded`: the value is harmless in the original). The status actions leave the text untouched.
//! The countdown actions 15/16 live in [`crate::script::actions::dispatch`] because they need the running
//! container.

use super::super::Quest;
use crate::sim::Sim;

/// Status "set" (actions 27/182).
const STATUS_SET: i32 = 3;
/// Status "prefilled" (actions 198/199).
const STATUS_NEW: i32 = 1;

/// The objective with the script number `n` (1-based); outside 1...10 nothing.
fn quest<'a>(sim: &'a mut Sim<'_>, main: bool, n: i32) -> Option<&'a mut Quest> {
    let index = usize::try_from(n.wrapping_sub(1))
        .ok()
        .filter(|&i| i < 10)?;
    let mission = &mut sim.state.script.mission;
    Some(if main {
        &mut mission.main_quests[index]
    } else {
        &mut mission.sub_quests[index]
    })
}

/// Actions 27 (`main`) and 182: set the text `text`, status 3.
pub fn set_quest(sim: &mut Sim, main: bool, n: i32, text: i32) {
    if let Some(q) = quest(sim, main, n) {
        *q = Quest {
            status: STATUS_SET,
            text,
        };
    }
}

/// Actions 184-187: set only the status (6 done, 10 failed); the text stays.
pub fn set_quest_status(sim: &mut Sim, main: bool, n: i32, status: i32) {
    if let Some(q) = quest(sim, main, n) {
        q.status = status;
    }
}

/// Actions 199 (`main`) and 198: status 1 for the objectives `0...n-1` (from number 11 on only up to 9,
/// `n <= 0` nothing).
pub fn prefill_quests(sim: &mut Sim, main: bool, n: i32) {
    for number in 1..=n.min(10) {
        set_quest_status(sim, main, number, STATUS_NEW);
    }
}
