//! Conditions (`FUN_00564180`, re/script-conditions D.1) and the script queries they need
//! (`FUN_00425F90`, D.2).
//!
//! Implemented: 1, 4, 5, 16, 17, 18, 19, 22, 23, 24, 27, 45, 46, 47; with query layer I
//! ([`super::query`]): 2, 3, 6-14, 20, 21, 25, 26, 40, 41, 50-55; with query layer II (object-related):
//! 28-33 and 56; with [`super::ui_mirror`]: 35-38. Every number with a case is thus implemented;
//! [`ScriptTrace::ConditionPending`] remains as a safety net for future numbers. Numbers without a case
//! (15, 34, 39, 42-44, 48, 49, outside 1...56) are false without a message (E-133). Only 19 (text event)
//! and 6 (swaps the corners of the area, E-190) write. All comparisons are signed.

use opennml_formats::abl::catalog::{self, CondEffect};
use opennml_formats::abl::{RtCondition, mid5};
use opennml_formats::property::atol;

use super::container::Container;
use super::{ScriptTrace, query, speech, trace, ui_mirror};
use crate::names;
use crate::sim::Sim;
use crate::world;

/// A condition. `block` is the block index in the file (for messages). A missing parameter (an exception
/// in the original, impossible after the load check) yields false and `Guarded`.
pub fn condition(sim: &mut Sim, c: &mut Container, block: u32, cond: &RtCondition) -> bool {
    match evaluate(sim, c, cond) {
        Ok(Some(value)) => value,
        Ok(None) => {
            let has_case = catalog::condition(cond.number)
                .is_some_and(|spec| spec.effect != CondEffect::NoCase);
            if has_case {
                trace(
                    sim,
                    ScriptTrace::ConditionPending {
                        container: c.id,
                        block,
                        condition: cond.number,
                    },
                );
            }
            false
        }
        Err(why) => {
            trace(
                sim,
                ScriptTrace::Guarded {
                    container: c.id,
                    block,
                    number: cond.number,
                    why,
                },
            );
            false
        }
    }
}

/// Value of a condition for the display (`script_state`, [`super::condition_values`]): like
/// [`condition`], but without messages; `None` = number with a case that is not yet implemented (false in
/// the tick with `ConditionPending`). Numbers without a case and missing parameters are false as in the tick.
pub fn shown_value(sim: &mut Sim, c: &Container, cond: &RtCondition) -> Option<bool> {
    match evaluate(sim, c, cond) {
        Ok(Some(value)) => Some(value),
        Ok(None) => catalog::condition(cond.number)
            .is_none_or(|spec| spec.effect == CondEffect::NoCase)
            .then_some(false),
        Err(_) => Some(false),
    }
}

/// Parameter `i` as text.
fn text(cond: &RtCondition, i: usize) -> Result<&str, String> {
    cond.params
        .get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("condition {}: parameter {} missing", cond.number, i + 1))
}

/// Parameter `i` via `atol`, truncated to 32 bits as in the original.
fn int(cond: &RtCondition, i: usize) -> Result<i32, String> {
    text(cond, i).map(|p| atol(p) as i32)
}

/// Value of the implemented conditions; `None` = not yet implemented or without a case.
fn evaluate(sim: &mut Sim, c: &Container, cond: &RtCondition) -> Result<Option<bool>, String> {
    let value = match cond.number {
        1 => true,
        // Countdown set (!= -1) and at most `Z0` seconds (wrapped like `Z0 · 1000` in 32 bits).
        4 => c.counter != -1 && c.counter <= int(cond, 0)?.wrapping_mul(1000),
        5 => c.switches.contains_key(text(cond, 0)?),
        16 => c.timers.get(text(cond, 0)?).is_some_and(|&t| t < 1),
        17 => match c.random_switches.get(text(cond, 0)?) {
            Some(&v) => v == int(cond, 1)?,
            None => false,
        },
        18 => !c.switches.contains_key(text(cond, 0)?),
        // Query 600: consumes the text event, even if the block does not run because of another condition
        // (all conditions are evaluated, E-124).
        19 => speech::take_event(sim.state, int(cond, 0)?),
        // Query 900: difficulty `GameEngine+0x5F8`.
        22 => sim.state.campaign_difficulty == int(cond, 0)?,
        23 => query_1000(sim, text(cond, 0)?),
        24 => !query_1000(sim, text(cond, 0)?),
        27 => {
            let name = text(cond, 0)?;
            query_1300(sim, name, mid5(text(cond, 1)?))
        }
        // Query 1400: hit points in percent (parameters: object, n). 28: `r < n` and the name is known
        // (`r ≠ −1`); 30: `r ≥ n`. A stale ID yields 0 (E-203).
        28 => {
            let r = query::hp_percent(sim, text(cond, 0)?);
            r != -1 && r < int(cond, 1)?
        }
        30 => query::hp_percent(sim, text(cond, 0)?) >= int(cond, 1)?,
        // Query 1500: entry of the AI global list (parameters: player, index, value); index < 0 is an error
        // (an exception in the original): false with `Guarded`.
        29 => query::ki_global(sim, int(cond, 0)?, int(cond, 1)?)? == int(cond, 2)?,
        // Query 2000 runs in the original and its result is discarded: always false (E-134).
        31 => false,
        // Query 2100: number of seen objects (parameters: object, n, GUID).
        32 => query::sees_count(sim, int(cond, 2)?, text(cond, 0)?) >= int(cond, 1)?,
        // Query 2200: tile of the object currently seen (parameters: player, object). Player < 0 is the local
        // player in the original (E-191) and is rejected by the load check.
        33 => {
            let player = int(cond, 0)?;
            if player < 0 {
                return Err(format!(
                    "condition 33: player {player} is the local player (query 2200)"
                ));
            }
            query::tile_seen(sim, player, text(cond, 1)?)
        }
        // Query 2600: cone (parameters: object, GUID, player, angle).
        56 => query::sees_in_cone(
            sim,
            text(cond, 0)?,
            int(cond, 1)?,
            int(cond, 2)?,
            int(cond, 3)?,
        ),
        // Queries 1600/1700 (parameter: object): the name stays text, not `atol`. Unknown name or no existing
        // object: both true (E-135, E-228).
        35 => ui_mirror::query_selected(sim, text(cond, 0)?, true),
        36 => ui_mirror::query_selected(sim, text(cond, 0)?, false),
        // Query 1800 (parameters: player, GUID): only `atol(p1)` is read, the player does not count (E-136);
        // equal to the type GUID of any selected entry, not a filter ID (E-231).
        37 => {
            let _player = int(cond, 0)?;
            ui_mirror::any_selected_guid(sim, int(cond, 1)?)
        }
        // Query 1900 (parameters: player, button): the player is read and discarded; the button stays set
        // (E-229).
        38 => {
            let _player = int(cond, 0)?;
            ui_mirror::query_last_button(sim) == int(cond, 1)?
        }
        // Query 100: resource. Kind 0...4 with a player not in 0...7 accesses a null pointer in the original
        // (E-193); the load check does not allow it, a savegame cannot produce it.
        2 | 12 => {
            let (player, kind) = (int(cond, 0)?, int(cond, 2)?);
            if !(0..8).contains(&player) && (0..=4).contains(&kind) {
                return Err(format!(
                    "condition {}: player {player} outside 0...7 (query 100)",
                    cond.number
                ));
            }
            let have = query::resource(sim, player, kind);
            if cond.number == 2 {
                have >= int(cond, 1)?
            } else {
                have < int(cond, 1)?
            }
        }
        // Query 200: count in the area.
        3 | 11 | 13 | 14 => {
            let n = int(cond, 1)?;
            let have =
                query::count_in_area(sim, int(cond, 0)?, int(cond, 2)?, mid5(text(cond, 3)?));
            match cond.number {
                3 => have >= n,
                11 => have == n,
                13 => have > n,
                _ => have < n,
            }
        }
        // Query 300: area currently seen (swaps swapped corners in the stored area).
        6 => query::area_seen(sim, int(cond, 0)?, mid5(text(cond, 1)?)),
        // Query 400: 7 = 9 and 8 = 10 (same code in the original, E-134).
        7..=10 => {
            let n = int(cond, 1)?;
            let have = query::count(sim, int(cond, 0)?, int(cond, 2)?);
            if matches!(cond.number, 7 | 9) {
                have < n
            } else {
                have > n
            }
        }
        // Query 700 without deleting.
        20 => query_2500(sim, text(cond, 0)?, int(cond, 1)? as u8, false),
        // Query 800 does nothing, the result is the name length in characters (E-134).
        21 => text(cond, 0)?.chars().count() == 1,
        // Query 1200: kill counter (parameters: player, GUID, n).
        25 | 26 => {
            let k = query::kill_count(sim, int(cond, 0)?, int(cond, 1)?);
            let n = int(cond, 2)?;
            if cond.number == 25 {
                k != 0 && k > n
            } else {
                k != 0 && k < n
            }
        }
        // Queries 2300/2400: object counter (parameters: player, GUID, n).
        40 => query::guid_count(sim, int(cond, 0)?, int(cond, 1)?) >= int(cond, 2)?,
        41 => query::guid_count(sim, int(cond, 0)?, int(cond, 1)?) > 0,
        45..=47 => match c.variables.get(text(cond, 0)?) {
            Some(&v) => {
                let z = int(cond, 1)?;
                match cond.number {
                    45 => v == z,
                    46 => v < z,
                    _ => v > z,
                }
            }
            None => false,
        },
        // 50...52: query 400 against the variable (parameters: player, variable, GUID); `atol(p1)` is
        // discarded in the original, the variable is looked up under the raw text, and if it is missing the
        // condition is false without the query running.
        50..=52 => match c.variables.get(text(cond, 1)?) {
            Some(&v) => {
                let have = query::count(sim, int(cond, 0)?, int(cond, 2)?);
                match cond.number {
                    50 => have < v,
                    51 => have > v,
                    _ => have == v,
                }
            }
            None => false,
        },
        // 53...55: query 200 against the variable (parameters: player, variable, GUID, area).
        53..=55 => match c.variables.get(text(cond, 1)?) {
            Some(&v) => {
                let have =
                    query::count_in_area(sim, int(cond, 0)?, int(cond, 2)?, mid5(text(cond, 3)?));
                match cond.number {
                    53 => have > v,
                    54 => have < v,
                    _ => have == v,
                }
            }
            None => false,
        },
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// Query 1000 (`FUN_00437840`): the ID list of the name is not empty and every ID has an object without
/// `DIED` (`GetObjectById(id, 1)`). Query 1100 is the negation (an unknown name is "true" there).
pub fn query_1000(sim: &Sim, name: &str) -> bool {
    let ids = names::ids(&sim.state.names, name);
    !ids.is_empty() && ids.iter().all(|&id| sim.state.object(id, true).is_some())
}

/// Query 1300 (`FUN_00438120`): area `area` (already without `area `) unknown -> false. IDs without an
/// object (`GetObjectById(id, 0)`, dying ones count) are skipped; every remaining object must lie in the
/// rectangle with its tile `(clamp(x >> 12, 0, W − 1), clamp(z >> 12, 0, H − 1))` (`PtInRect` with
/// `x1 + 1`, `z1 + 1`: inclusive, without corner swap). True if at least one remains and all lie inside.
pub fn query_1300(sim: &Sim, name: &str, area: &str) -> bool {
    let Some([x0, z0, x1, z1]) = super::area(sim, area) else {
        return false;
    };
    let (w, h) = (sim.rules.map.dim_x, sim.rules.map.dim_z);
    let mut found = false;
    for id in names::ids(&sim.state.names, name) {
        let Some(o) = sim.state.object(id, false) else {
            continue;
        };
        let tx = (o.grid.x.0 >> 12).clamp(0, (w - 1).max(0));
        let tz = (o.grid.z.0 >> 12).clamp(0, (h - 1).max(0));
        let inside = tx >= x0 && tx < x1.wrapping_add(1) && tz >= z0 && tz < z1.wrapping_add(1);
        if !inside {
            return false;
        }
        found = true;
    }
    found
}

/// Query 700/2500 (`FUN_00436D30(name, bit, clear, at_least_one = 1)`): true if the ID list of the
/// name (`FUN_00436240`, like [`names::ids`]) is not empty and **every** ID in the event word
/// (`FUN_0047E7B0`, without object check; without an entry 0) has the bit `bit`. Only then, and with
/// `clear` (2500), does it clear the bit for all IDs of the list (`GameWorld_SetObjectEventBit`).
/// An empty or unknown list is false.
pub fn query_2500(sim: &mut Sim, name: &str, bit: u8, clear: bool) -> bool {
    let ids = names::ids(&sim.state.names, name);
    let mask = 1u32.wrapping_shl(u32::from(bit));
    let all = !ids.is_empty()
        && ids
            .iter()
            .all(|&id| world::object_event_bits(sim, id) & mask != 0);
    if all && clear {
        for id in ids {
            world::set_object_event_bit(sim, id, u32::from(bit), false);
        }
    }
    all
}
