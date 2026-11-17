//! AI query facade: the quirks of the query codes `0x4E20`...`0x5130`.
//!
//! Two layers like in the original (re/ki-queries 1.2): the **handlers per builtin ID** ([`handlers`], one table) pop
//! the arguments, build the query word field `{code, argc, [2], …}` exactly like the original handler (also with a
//! wrong `argc`), call [`dispatch`] and push `[2]` or write output variables. [`dispatch`] answers per code (with
//! `argc` check; failure or unknown code leaves the field unchanged, E-27) and reads only through [`View`]. Direct
//! state changes (mark, taking an attack report) are returned by `dispatch` as [`Direct`]; only the real world
//! [`SimKiWorld`] applies them with [`apply`], [`query_once`] (control command `ki_query`) discards them.
//!
//! **Extension (every new query group):** one file `codes_<group>.rs` per group with
//! `answer(view, code, req, direct) -> bool` (code recognized?), registered in [`GROUPS`]; insert handler rows sorted
//! by builtin ID into [`handlers::ROWS`] (they are also [`is_query`]); tests in `tests_<group>.rs`.

pub mod filters;
pub(crate) mod handlers;
mod handlers_list;

mod codes_count;
mod codes_object;
mod codes_path;
mod codes_player;
mod codes_search;

use std::collections::BTreeMap;

use super::command;
use super::world::{BuiltinCx, KiNote, KiWorld};
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::player::{Sight, View};
use crate::sim::Sim;

/// Words of the query field from `[2]` that [`query_once`] allocates at least: codes without `argc` check read beyond
/// `argc`, the longest list query has `n + 5 < 105` words.
pub const QUERY_WORDS: usize = 128;

/// Direct state change of a query, executed in the middle of the dispatch in the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direct {
    /// Set or clear the AI mark `GameObject+4` bit `player` (`FUN_00466F90`); only generated for `player = p & 31 < 8`.
    Mark {
        object: ObjectId,
        player: u8,
        on: bool,
    },
    /// Remove the attack report of the victim `victim` from the table of slot `player` (`0x512E`).
    TakeReport { player: u8, victim: i32 },
}

/// A group of query codes: answers `code` and returns `true` if it knows it.
type Group = fn(&mut View<'_, '_>, u32, &mut [i32], &mut Vec<Direct>) -> bool;

/// Groups in check order; every group knows only its codes (every new group registers here).
const GROUPS: &[Group] = &[
    codes_player::answer,
    codes_object::answer,
    codes_search::answer,
    codes_count::answer,
    codes_path::answer,
];

/// Word `i` of the query field, outside 0.
pub(crate) fn word(req: &[i32], i: usize) -> i32 {
    req.get(i).copied().unwrap_or(0)
}

/// Write word `i`, outside nothing.
pub(crate) fn put(req: &mut [i32], i: usize, v: i32) {
    if let Some(w) = req.get_mut(i) {
        *w = v;
    }
}

/// `argc` of the query (`[1]`).
pub(crate) fn argc(req: &[i32]) -> i32 {
    word(req, 1)
}

/// `ScriptQuery_Dispatch` (`0x004A71A0`): answer the query `req = {code, argc, [2], …}`; the result is in the field
/// afterwards. Unknown code or wrong `argc` -> field unchanged (E-27). Reads only through `view`; direct changes end up
/// in `direct`.
pub fn dispatch(view: &mut View<'_, '_>, req: &mut [i32], direct: &mut Vec<Direct>) {
    let code = word(req, 0) as u32;
    for group in GROUPS {
        if group(view, code, req, direct) {
            return;
        }
    }
}

/// A query from outside (control command `ki_query`): field `[code, words.len(), words…]`, padded with 0 to at least
/// `2 + QUERY_WORDS` words; view of slot `player`. Returns the field and the direct changes **without** applying them.
pub fn query_once(
    sim: &mut Sim,
    player: u8,
    sight: Sight,
    code: u32,
    words: &[i32],
) -> (Vec<i32>, Vec<Direct>) {
    let mut req = vec![0; 2 + words.len().max(QUERY_WORDS)];
    req[0] = code as i32;
    req[1] = i32::try_from(words.len()).unwrap_or(i32::MAX);
    req[2..2 + words.len()].copy_from_slice(words);
    let mut direct = Vec::new();
    dispatch(&mut View::new(sim, player, sight), &mut req, &mut direct);
    (req, direct)
}

/// `true` for every world ID that is answered as a query (single source: [`handlers::ROWS`]).
pub fn is_query(id: u16) -> bool {
    handlers::row(id).is_some()
}

/// Apply direct changes in order: mark on every object still findable (also dead, `GetObjectById(id, 0)`; missing ->
/// nothing), remove an attack report (missing -> nothing).
pub fn apply(sim: &mut Sim, direct: &[Direct]) {
    for d in direct {
        match *d {
            Direct::Mark { object, player, on } => {
                if usize::from(player) >= PLAYER_SLOTS {
                    continue;
                }
                if let Some(o) = sim.state.object_mut(object, false) {
                    let bit = 1u8 << player;
                    if on {
                        o.ai_marks |= bit;
                    } else {
                        o.ai_marks &= !bit;
                    }
                }
            }
            Direct::TakeReport { player, victim } => {
                if let Some(reports) = sim.state.attack_reports.get_mut(usize::from(player)) {
                    reports.remove(victim);
                }
            }
        }
    }
}

/// The real counterpart of the AI: queries via [`handlers`] against the simulation, commands via [`crate::ki::command`]
/// (executed immediately as a game command). A world ID without a query row and without a command row is unreachable in
/// the shipped data (test `every_world_builtin_is_a_query_or_a_command`); if it comes anyway, the instance empties the
/// stack like the `default` of the interpreter and reports `KiNote::Guarded`. The answer by signature stays with the
/// [`crate::ki::stub::StubWorld`].
pub struct SimKiWorld<'s, 'a> {
    pub sim: &'s mut Sim<'a>,
    pub sight: Sight,
    /// Anomalies of the VM and the handlers.
    pub notes: Vec<KiNote>,
    /// Calls per builtin ID.
    pub counts: BTreeMap<u16, u64>,
}

impl<'s, 'a> SimKiWorld<'s, 'a> {
    pub fn new(sim: &'s mut Sim<'a>, sight: Sight) -> Self {
        Self {
            sim,
            sight,
            notes: Vec::new(),
            counts: BTreeMap::new(),
        }
    }
}

impl KiWorld for SimKiWorld<'_, '_> {
    fn builtin(&mut self, id: u16, cx: &mut BuiltinCx<'_>) {
        *self.counts.entry(id).or_default() += 1;
        if is_query(id) {
            let known = handlers::call(id, cx, self);
            debug_assert!(known, "query {id:#x} without a handler");
        } else if command::is_command(id) {
            command::call(id, cx, self);
        } else {
            cx.clear_stack();
            self.notes.push(KiNote::Guarded {
                slot: cx.slot(),
                thread: cx.thread_id(),
                module: cx.module(),
                builtin: id,
                what: "neither query nor command",
            });
        }
    }

    /// Collects (the default of [`KiWorld::note`] discards).
    fn note(&mut self, note: KiNote) {
        self.notes.push(note);
    }
}

#[cfg(test)]
mod tests_count;
#[cfg(test)]
mod tests_coverage;
#[cfg(test)]
mod tests_list;
#[cfg(test)]
mod tests_object;
#[cfg(test)]
mod tests_path;
#[cfg(test)]
mod tests_player;
#[cfg(test)]
mod tests_reports;
#[cfg(test)]
mod tests_search;
