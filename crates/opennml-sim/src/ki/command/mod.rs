//! AI command layer: the command builtins that send a game command (`0x500000xx`) in the original (re/ki-queries
//! 7.1/7.2, pops according to re/ki-builtins). A separate table [`ROWS`], sorted by ID, next to the query handlers
//! ([`super::query::handlers`]); it is the single source of [`is_command`].
//!
//! Flow per call ([`call`]): pop `pops` arguments `a1 … aN` (first pop = `aN`), call the flow of the row (it builds
//! exactly the commands of the original dispatcher, issues them immediately into the simulation via [`issue`], pushes
//! itself and writes the output variables), then `pause_ms ≠ 0` -> `m_iPauseCounter` (`FUN_004b4180`). Every command
//! given becomes a [`SimEvent::KiCommand`] with its [`Outcome`], also an accepted one.
//!
//! **Extension:** insert rows sorted by ID and without overlap (also not with the queries) (test
//! `rows_are_sorted_and_disjoint_from_queries`); flows per group in their own files (`units.rs` movement, economy,
//! construction; `control.rs` combat, behavior, AI modes, diplomacy, resources, removal; `abilities.rs` late abilities
//! `0x258…0x2F5`; tests in `tests_<group>.rs`).

mod abilities;
mod control;
mod units;

#[cfg(test)]
mod tests_abilities;
#[cfg(test)]
mod tests_control;
#[cfg(test)]
mod tests_units;

use super::query::SimKiWorld;
use super::world::{BuiltinCx, KiNote};
use crate::command::{Command, CommandKind, Origin, Outcome};
use crate::player::submit;
use crate::state::SimEvent;

/// A call of a command builtin: ID, arguments in script order `a1 … aN`, player of the instance.
pub struct CmdCall<'a> {
    pub id: u16,
    pub args: &'a [i32],
    pub me: u8,
}

impl CmdCall<'_> {
    /// Argument `a(i+1)`; outside 0 (the row always pops `pops` values).
    pub fn arg(&self, i: usize) -> i32 {
        self.args.get(i).copied().unwrap_or(0)
    }
}

/// Flow of a command builtin: builds the commands, pushes and writes outputs itself.
pub type CmdFn = fn(&CmdCall<'_>, &mut BuiltinCx<'_>, &mut SimKiWorld<'_, '_>);

/// Command builtins of the IDs `lo..=hi`.
#[derive(Clone, Copy)]
pub struct CmdRow {
    pub lo: u16,
    pub hi: u16,
    /// Query code of the original handler (`{code, argc, …}` to the dispatcher).
    pub code: u32,
    /// Number of arguments.
    pub pops: u8,
    /// Pushes a result (the flow pushes itself; the row describes it).
    pub push: bool,
    /// Positions in `a1 … aN` (0-based) that are addresses of output variables.
    pub outs: &'static [u8],
    /// `m_iPauseCounter` afterwards (ms), 0 = none.
    pub pause_ms: i32,
    pub run: CmdFn,
}

impl CmdRow {
    /// With result (push).
    const fn pushes(self) -> Self {
        Self { push: true, ..self }
    }

    /// With the output variables `outs`.
    const fn outs(self, outs: &'static [u8]) -> Self {
        Self { outs, ..self }
    }

    /// With `m_iPauseCounter = ms` afterwards.
    const fn pause(self, pause_ms: i32) -> Self {
        Self { pause_ms, ..self }
    }
}

/// Command without result, without outputs and without pause.
const fn cmd(lo: u16, hi: u16, code: u32, pops: u8, run: CmdFn) -> CmdRow {
    CmdRow {
        lo,
        hi,
        code,
        pops,
        push: false,
        outs: &[],
        pause_ms: 0,
        run,
    }
}

/// All command builtins, sorted by ID (re/ki-queries 7.1: pattern in parentheses).
pub static ROWS: &[CmdRow] = &[
    // A: shared vision and diplomacy `(p, q, m)`, removal `(obj)` (Sell or Die).
    cmd(0x97, 0x97, 0x4E41, 3, control::diplomacy),
    cmd(0xA1, 0xA3, 0x4E46, 1, control::remove),
    // A: Build/Repair `(u, bld)` -> `0x02 {bld, 1, u}`.
    cmd(0xB4, 0xB4, 0x4E4C, 2, units::construct),
    // A′: Mount and dismount `(u, horse)` / `(u)`, push 0.
    cmd(0xC8, 0xC8, 0x4E4E, 2, units::mount).pushes(),
    cmd(0xC9, 0xC9, 0x4E4F, 1, units::dismount).pushes(),
    // A: Hire headhunter `(target_player, guid, kind)`.
    cmd(0xCE, 0xCE, 0x4E50, 3, control::hire_headhunter),
    // C/D: Move `(u, x, z)` or `(&obj, start, n, x, z, formation)`; flags per ID
    // ([`units::move_flags`]), alive check only 0xF5/0xF6 and their aliases 0xF9/0xFA.
    cmd(0xF5, 0xF5, 0x4E5C, 3, units::move_alive),
    cmd(0xF6, 0xF6, 0x5019, 3, units::move_alive),
    cmd(0xF7, 0xF7, 0x4EFE, 6, units::move_group),
    cmd(0xF8, 0xF8, 0x501E, 6, units::move_group),
    cmd(0xF9, 0xF9, 0x4E5C, 3, units::move_alive),
    cmd(0xFA, 0xFA, 0x5019, 3, units::move_alive),
    cmd(0xFB, 0xFB, 0x4EFE, 6, units::move_group),
    cmd(0xFC, 0xFC, 0x501E, 6, units::move_group),
    cmd(0xFD, 0xFD, 0x4F4C, 3, units::move_unchecked),
    cmd(0xFE, 0xFE, 0x5023, 3, units::move_unchecked),
    cmd(0xFF, 0xFF, 0x4F51, 6, units::move_group),
    cmd(0x100, 0x100, 0x5028, 6, units::move_group),
    cmd(0x101, 0x101, 0x4F4C, 3, units::move_unchecked),
    cmd(0x102, 0x102, 0x5023, 3, units::move_unchecked),
    cmd(0x103, 0x103, 0x4F51, 6, units::move_group),
    cmd(0x104, 0x104, 0x5028, 6, units::move_group),
    // F: find a building site and build `(worker, guid, x, z, &x', &z', border)`, push 1/0, pause 500.
    cmd(0x12C, 0x12C, 0x4E84, 7, units::place_building)
        .pushes()
        .outs(&[4, 5])
        .pause(500),
    // A/A′: Production `(bld, guid)` push 100, rally point `(bld, x, z)`, cancel `(bld)`.
    cmd(0x12D, 0x12D, 0x4E85, 2, units::enqueue_production).pushes(),
    cmd(0x12E, 0x12E, 0x4E86, 3, units::rally_point),
    cmd(0x12F, 0x12F, 0x4E87, 1, control::remove),
    cmd(0x130, 0x130, 0x4E88, 1, units::cancel_production),
    // A: Occupancy border `(bld, value)`.
    cmd(0x13B, 0x13B, 0x5104, 2, control::occupancy_border),
    // A/B: AI attack and walk mode of a slot (`0x154`/`0x155` with slot from the stack,
    // `0x156`/`0x157` own slot) and of an object (object from the stack, alias `0x15A`/`0x15B`).
    cmd(0x154, 0x154, 0x4FDD, 3, control::ai_player_attack_mode),
    cmd(0x155, 0x155, 0x4FE2, 5, control::ai_player_walk_flags),
    cmd(0x156, 0x156, 0x4FDD, 2, control::ai_player_attack_mode),
    cmd(0x157, 0x157, 0x4FE2, 4, control::ai_player_walk_flags),
    cmd(0x158, 0x158, 0x4FE7, 3, control::ai_object_attack_mode),
    cmd(0x159, 0x159, 0x4FEC, 5, control::ai_object_walk_flags),
    cmd(0x15A, 0x15A, 0x4FE7, 3, control::ai_object_attack_mode),
    cmd(0x15B, 0x15B, 0x4FEC, 5, control::ai_object_walk_flags),
    // A′/A: Train halt (push `me`), forward, backward `(train)`.
    cmd(0x15D, 0x15D, 0x5127, 1, units::train).pushes(),
    cmd(0x167, 0x167, 0x512C, 1, units::train),
    cmd(0x168, 0x168, 0x512D, 1, units::train),
    // A: Quarter `(u, container)` (0x172/0x173 the same), Board `(u, boat)`,
    // Unquarter `(container, unit)`, Disembark `(u)`.
    cmd(0x172, 0x172, 0x4F7E, 2, units::quarter),
    cmd(0x173, 0x173, 0x4F83, 2, units::quarter),
    cmd(0x174, 0x174, 0x4F88, 2, units::board_boat),
    cmd(0x176, 0x176, 0x4FCE, 2, units::unquarter),
    cmd(0x178, 0x178, 0x4FD8, 1, units::leave_boat),
    // B: Credit resource `(slot, resource, amount)`, slot -1 = own.
    cmd(0x18B, 0x18B, 0x5005, 3, control::add_resource),
    // A/A′/F: Halt `(u)`, Harvest `(u, field)` push 0, find and fell tree
    // `(u, x, z, &kx, &kz)` push 1/0, Hunt `(u, animal)` push 100, Gold `(u, mine)` push 1.
    cmd(0x191, 0x191, 0x4EE9, 1, units::halt),
    cmd(0x192, 0x192, 0x4EEA, 2, units::harvest).pushes(),
    cmd(0x193, 0x193, 0x4EEB, 5, units::chop_nearest_tree)
        .pushes()
        .outs(&[3, 4]),
    cmd(0x194, 0x194, 0x4EEC, 2, units::hunt).pushes(),
    cmd(0x195, 0x195, 0x4EED, 2, units::mine_gold).pushes(),
    // A/E: Behavior `(u, mode)` and for a list `(&obj, start, n, mode)`.
    cmd(0x198, 0x199, 0x4F3D, 2, control::set_behavior),
    cmd(0x19A, 0x19A, 0x4EE9, 1, units::halt),
    cmd(0x19B, 0x19C, 0x4F5B, 4, control::set_behavior_list),
    // A/E: Arm `(u)`, Attack `(u, target)` and for a list `(&obj, start, n, target)`.
    cmd(0x1A2, 0x1A2, 0x4FBA, 1, control::arm),
    cmd(0x1A4, 0x1A6, 0x4F60, 2, control::attack),
    cmd(0x1A7, 0x1A9, 0x4F74, 4, control::attack_list),
    // A: AI behavior of the own slot `(mode)`.
    cmd(0x1B8, 0x1B8, 0x4FFB, 1, control::ai_behavior),
    // A: Counter-upgrade `(bld, guid)`, unquarter all `(container)`.
    cmd(0x231, 0x231, 0x5037, 2, units::counter_upgrade),
    cmd(0x23A, 0x23A, 0x512F, 1, units::unquarter_all),
    // G: late abilities `(id)`, `(id, target or on)`, `(id, kx, kz)` (7.1, table 2
    // `0x503C … 0x512F`); buttons as in the original, also where they do not match the command bar (E-93 to E-95).
    cmd(0x258, 0x258, 0x503C, 1, abilities::exec_temp),
    cmd(0x259, 0x259, 0x5041, 1, abilities::exec_temp),
    cmd(0x26C, 0x26C, 0x5046, 1, abilities::evoke_ghost),
    cmd(0x26D, 0x26D, 0x504B, 1, abilities::evoke_animal),
    cmd(0x26E, 0x26E, 0x5050, 1, abilities::evoke_animal),
    cmd(0x26F, 0x26F, 0x5055, 1, abilities::evoke_animal),
    cmd(0x280, 0x280, 0x505A, 2, abilities::cast_at),
    cmd(0x281, 0x281, 0x505F, 2, abilities::cast_at),
    cmd(0x282, 0x282, 0x50F0, 1, abilities::cast),
    cmd(0x283, 0x283, 0x50F5, 2, abilities::special_attack),
    cmd(0x284, 0x284, 0x50FA, 1, abilities::exec_temp),
    cmd(0x294, 0x294, 0x5064, 1, abilities::whirlwind),
    cmd(0x295, 0x295, 0x5069, 2, abilities::damage_building),
    cmd(0x296, 0x296, 0x506E, 1, abilities::exec_temp),
    cmd(0x297, 0x297, 0x5073, 1, abilities::self_heal),
    cmd(0x2A8, 0x2A8, 0x5078, 1, abilities::exec_temp),
    cmd(0x2A9, 0x2A9, 0x507D, 1, abilities::exec_temp),
    // Dynamite: `0x2AA` sleeps 5000 ms after every query (E-96), `0x2AB` never.
    cmd(0x2AA, 0x2AA, 0x5082, 2, abilities::dynamite).pause(5000),
    cmd(0x2AB, 0x2AB, 0x5087, 2, abilities::dynamite),
    cmd(0x2AC, 0x2AC, 0x5109, 1, abilities::exec_temp),
    cmd(0x2BC, 0x2BC, 0x508C, 1, abilities::exec_temp),
    cmd(0x2BD, 0x2BD, 0x5091, 1, abilities::cast),
    cmd(0x2BE, 0x2BE, 0x5096, 1, abilities::rob_corpses),
    cmd(0x2EE, 0x2EE, 0x509B, 1, abilities::banish),
    cmd(0x2EF, 0x2EF, 0x50A0, 2, abilities::special_attack),
    cmd(0x2F0, 0x2F0, 0x50A5, 1, abilities::exec_temp),
    cmd(0x2F1, 0x2F1, 0x50AA, 3, abilities::scout_area),
    cmd(0x2F2, 0x2F2, 0x50AF, 2, abilities::toggle),
    cmd(0x2F3, 0x2F3, 0x50B4, 2, abilities::toggle),
    cmd(0x2F4, 0x2F4, 0x50B9, 2, abilities::toggle),
    cmd(0x2F5, 0x2F5, 0x50E1, 2, abilities::special_attack),
];

/// Row of the ID `id` (binary search over the sorted table).
pub fn row(id: u16) -> Option<&'static CmdRow> {
    let i = ROWS.partition_point(|r| r.hi < id);
    ROWS.get(i).filter(|r| r.lo <= id)
}

/// `true` for every world ID with a command row (single source: [`ROWS`]).
pub fn is_command(id: u16) -> bool {
    row(id).is_some()
}

/// Command builtin `id`: pop arguments, flow of the row, then the pause of the row. Without a row (excluded by the
/// caller) empty the stack like the `default` of the interpreter and report `KiNote::Guarded`.
pub fn call(id: u16, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let Some(row) = row(id) else {
        cx.clear_stack();
        w.notes.push(KiNote::Guarded {
            slot: cx.slot(),
            thread: cx.thread_id(),
            module: cx.module(),
            builtin: id,
            what: "without command row",
        });
        return;
    };
    let mut args: Vec<i32> = (0..row.pops).map(|_| cx.pop()).collect();
    args.reverse();
    let call = CmdCall {
        id,
        args: &args,
        me: cx.me(),
    };
    (row.run)(&call, cx, w);
    if row.pause_ms != 0 {
        cx.set_pause(row.pause_ms);
    }
}

/// Give an AI command (`NetSession_SubmitMessage` of the dispatcher): execute `Command { player: me, kind }`
/// immediately via [`submit`] with [`Origin::Ki`] (AI check path) and report the result as [`SimEvent::KiCommand`];
/// `code` is the network command `0x01…0x7E`.
pub(crate) fn issue(w: &mut SimKiWorld<'_, '_>, me: u8, code: u32, kind: CommandKind) -> Outcome {
    debug_assert_eq!(
        code,
        kind.type_code() & 0xFF,
        "network command for {kind:?}"
    );
    let cmd = Command { player: me, kind };
    let outcome = submit(w.sim, &cmd, Origin::Ki);
    w.sim.state.events.push(SimEvent::KiCommand {
        player: me,
        code,
        outcome: outcome.clone(),
    });
    outcome
}
