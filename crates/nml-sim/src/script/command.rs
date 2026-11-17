//! Command path of the script actions (layer D, part B; re/commands 9, re/script-actions B.5).
//!
//! In the original, script actions send commands via `NetSession_SubmitMessage` (`0x004E9B60`): in single
//! player synchronously and with `direct = 1`, without a sender, without ownership or UI checks (E-152,
//! E-214). The reimplementation executes them immediately in the script tick, with
//! [`Origin::Script`](crate::command::Origin::Script) ([`submit_script`]); they are in no command log
//! (E-162, the replay lets them arise anew in the scripts) and never go over the network. Each command
//! reports its verdict as [`Issued`]; [`super::actions`] turns it into [`super::ScriptTrace::Command`].
//!
//! [`dispatch_command`] is the common entry point (like `world::dispatch_world`): further numbers are added
//! as further arms. Every number follows the pattern of the halt action 191: read IDs from the name, send
//! one command per ID (or once for all) with [`issue`].

use super::actions::{Issued, ScriptMsg};
use super::area;
use super::query::filter_known;
use crate::area::collect_in_tile_rect;
use crate::command::ability::{BANISH_BUTTONS, EVOKE_ANIMAL_BUTTONS, EVOKE_GHOST_BUTTON};
use crate::command::headhunter::NATION_HQ;
use crate::command::validate::is_finished;
use crate::command::{Command, CommandKind, Outcome, TrainOp};
use crate::filter;
use crate::game_end::{DEFEAT, VICTORY};
use crate::group::none::grid_pos;
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::math::atan2_deg;
use crate::motion::NO_DIRECTION;
use crate::names;
use crate::object::status;
use crate::player::submit_script;
use crate::sim::Sim;
use crate::upgrades::objects_of_slot;
use crate::world::set_object_event_bit;

/// Direction "-1" of the script movements (E-151): not a special direction but a real target direction of
/// -1/4096 degrees, which the walk module aims for like any other with tolerance 0.
const DIRECTION_MINUS_1: i32 = -1;
/// Fixed-point distance of the tile center from the tile edge (`DAT_0066ff08`, 0.5 tiles).
const HALF_TILE: i32 = 0x800;
/// `spread` of commands 154/390/181 (-1); 153 and 8 send 0 (both have the same effect, E-148).
const SPREAD_MINUS_1: i32 = -1;

/// Where 154/390 look: at the center of an area or at the first object of a name.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Look<'a> {
    Area(&'a str),
    Object(&'a str),
}

/// Command action of the message `msg`. `Some(commands)`: the number belongs to the command path and has
/// been executed (`Some(vec![])`: no command arose, e.g. with an unknown name - silent, as in the original);
/// `None`: not a command action, the caller checks the world actions.
///
/// Value slots according to `actions::message`: 191 and 193 `{name}`; 167 `{name, name}`; 179 and 189
/// `{code, name}`; 190 `{code, name, target}`; 34 `{player, GUID, area}`; 153/188/374/376
/// `{name, area, area}`; 154 `{name, area}`; 390 and 181/375 `{name, name}`; 8/373
/// `{player, GUID, area, area}`; 163/164 `{name, area}`; 165/166 `{name}`.
pub(crate) fn dispatch_command(sim: &mut Sim, msg: &ScriptMsg) -> Option<Vec<Issued>> {
    let mut out = Vec::new();
    match msg.number {
        191 => halt(sim, &mut out, msg.text(0)),
        // `{0x11, n + 3, target, n, ids…, 0}`: one attack of all IDs of the first name on the first ID of the
        // second.
        167 => attack(sim, &mut out, msg.text(0), msg.text(1)),
        // `{0x12, 3, mode, 1, id}` per ID.
        179 => behavior(sim, &mut out, msg.int(0), msg.text(1)),
        // 34 `{player, GUID, area}`: one `Die` for the area selection; 193 `{name}`: one per ID.
        34 => kill_in_area(sim, &mut out, msg.int(0), msg.int(1), msg.text(2)),
        193 => kill_named(sim, &mut out, msg.text(0)),
        // 189 `{code, name}` (abilities) and 190 `{code, name, target}` (special commands with a target).
        189 => special(sim, &mut out, msg.int(0), msg.text(1)),
        190 => special_target(sim, &mut out, msg.int(0), msg.text(1), msg.text(2)),
        // 157 `{animation, seconds, name}`: `0x42` per ID; 158 `{name}`: `0x43` per ID.
        157 => animation_lock(sim, &mut out, msg.int(0), msg.int(1), msg.text(2)),
        158 => animation_unlock(sim, &mut out, msg.text(0)),
        // Move (`0x04`, handler `GameWorld_CmdMoveSelection` `0x0047CA10`): one command for all IDs of the
        // name (E-215), flags 2 (153), 3 (188), 0 (374), 1 (376).
        153 | 188 | 374 | 376 => {
            let flags = match msg.number {
                153 => 2,
                188 => 3,
                374 => 0,
                _ => 1,
            };
            move_named(sim, &mut out, msg.text(0), msg.text(1), msg.text(2), flags);
        }
        154 => turn_named(sim, &mut out, msg.text(0), Look::Area(msg.text(1))),
        390 => turn_named(sim, &mut out, msg.text(0), Look::Object(msg.text(1))),
        181 => move_to_object(sim, &mut out, msg.text(0), msg.text(1), 2),
        375 => move_to_object(sim, &mut out, msg.text(0), msg.text(1), 0),
        8 | 373 => {
            let flags = if msg.number == 8 { 2 } else { 0 };
            move_area(
                sim,
                &mut out,
                msg.int(0),
                msg.int(1),
                msg.text(2),
                msg.text(3),
                flags,
            );
        }
        163 | 164 => train(sim, &mut out, msg.number == 163, msg.text(0), msg.text(1)),
        165 | 166 => gate(sim, &mut out, msg.number == 165, msg.text(0)),
        // Quarter (334, 335 as alias), boat (336), mount (337) and dismount (338): case `0x6A`/`0x6B` of the
        // original. 402 (crew siege engine, case `0x6A`) has no command in the reimplementation (deviation from the
        // original, E-219): `actions::dispatch` answers it with `Done`.
        334 | 335 => quarter(sim, &mut out, PairKind::Quarter, msg.text(0), msg.text(1)),
        336 => quarter(sim, &mut out, PairKind::Boat, msg.text(0), msg.text(1)),
        337 => quarter(sim, &mut out, PairKind::Mount, msg.text(0), msg.text(1)),
        338 => quarter(sim, &mut out, PairKind::Dismount, msg.text(0), ""),
        // 4 `{player}` (defeat) and 13 `{player}` (victory): `{0x56, 2, p, 1|2}`.
        4 => player_result(sim, &mut out, msg.int(0), DEFEAT),
        13 => player_result(sim, &mut out, msg.int(0), VICTORY),
        // 38 `{player, GUID}`: `{0x39, 2, p, guid}`.
        38 => upgrade(sim, &mut out, msg.int(0), msg.int(1)),
        // 380 `{customer, target, GUID, kind}` (order changed by layer E).
        380 => hire_headhunter(
            sim,
            &mut out,
            msg.int(0),
            msg.int(3),
            msg.int(1),
            msg.int(2),
        ),
        _ => return None,
    }
    Some(out)
}

/// Center of an area as a fixed-point world position: `((x0 + x1) / 2, (z0 + z1) / 2)` with C division
/// (truncated toward zero), then `(c << 12) + 0x800` (`Fixed_FromInt(c) + DAT_0066ff08`).
fn center_fixed([x0, z0, x1, z1]: [i32; 4]) -> (i32, i32) {
    let fixed = |c: i32| c.wrapping_shl(12).wrapping_add(HALF_TILE);
    (
        fixed(x0.wrapping_add(x1) / 2),
        fixed(z0.wrapping_add(z1) / 2),
    )
}

/// Direction from `from` to `to` (fixed point): -1 for the same point, otherwise `Fixed_Atan2Deg(Δz, Δx)`.
fn direction_or_minus_1(from: (i32, i32), to: (i32, i32)) -> i32 {
    if from == to {
        DIRECTION_MINUS_1
    } else {
        atan2_deg(to.1.wrapping_sub(from.1), to.0.wrapping_sub(from.0))
    }
}

/// 153, 188, 374, 376 (`FUN_00436710(name, area1, area2, flags)`, `0x00436710`): **one** walk command
/// `0x04` with all IDs of the name to the center of `area1`, direction from there to the center of `area2`
/// (same center: -1, E-151), `target` -1, formation 0, `spread` 0.
///
/// Ghidra pre-check: `FUN_00436710` looks up **both** areas (`FUN_00435a60`) before it touches event bit 1;
/// an unknown area thus leaves the bit untouched too and sends nothing. Only then does the loop clear
/// `GameWorld_SetObjectEventBit(id, 1, 0)` per ID **without an object check** (dead ones too), each time
/// before `MoveCommand_AddUnit`. Without IDs (unknown name) no command goes out (`n = 0`).
pub(crate) fn move_named(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    name: &str,
    area1: &str,
    area2: &str,
    flags: i32,
) {
    for kind in plan_move_named(sim, name, area1, area2, flags) {
        issue_planned(sim, out, kind);
    }
}

/// The command of [`move_named`] (including clearing event bit 1); empty if nothing goes out.
pub(super) fn plan_move_named(
    sim: &mut Sim,
    name: &str,
    area1: &str,
    area2: &str,
    flags: i32,
) -> Vec<CommandKind> {
    let (Some(a1), Some(a2)) = (area(sim, area1), area(sim, area2)) else {
        return Vec::new();
    };
    let (from, to) = (center_fixed(a1), center_fixed(a2));
    let ids = names::ids(&sim.state.names, name);
    for &id in &ids {
        set_object_event_bit(sim, id, 1, false);
    }
    if ids.is_empty() {
        return Vec::new();
    }
    vec![CommandKind::Move {
        units: ids,
        x: from.0,
        z: from.1,
        target: -1,
        formation: 0,
        direction: direction_or_minus_1(from, to),
        spread: 0,
        flags,
    }]
}

/// 154 and 390 (`FUN_004394F0(name, target, mode)`, `0x004394F0`): per **existing** ID (also dying) a
/// separate walk command with `n = 1` to the unit's **own** position (grid position), direction toward the
/// look-at point (area center or position of the first object of the second name; if the unit is already
/// there: -1), `spread` -1, flags 0 - the unit turns on the spot. An unknown area or target name (or a
/// target without an object) does nothing; no event bit is cleared.
pub(crate) fn turn_named(sim: &mut Sim, out: &mut Vec<Issued>, name: &str, look: Look<'_>) {
    for kind in plan_turn_named(sim, name, look) {
        issue_planned(sim, out, kind);
    }
}

/// The commands of [`turn_named`], one per existing ID in name order.
pub(super) fn plan_turn_named(sim: &Sim, name: &str, look: Look<'_>) -> Vec<CommandKind> {
    let at = match look {
        Look::Area(a) => area(sim, a).map(center_fixed),
        Look::Object(n) => names::first_id(&sim.state.names, n).and_then(|id| grid_pos(sim, id)),
    };
    let Some(at) = at else { return Vec::new() };
    names::ids(&sim.state.names, name)
        .into_iter()
        .filter_map(|id| {
            let own = grid_pos(sim, id)?;
            Some(CommandKind::Move {
                units: vec![id],
                x: own.0,
                z: own.1,
                target: -1,
                formation: 0,
                direction: direction_or_minus_1(own, at),
                spread: SPREAD_MINUS_1,
                flags: 0,
            })
        })
        .collect()
}

/// 181 (flags 2) and 375 (flags 0) (`FUN_00439DF0(name, target, flags)`, `0x00439DF0`): **one** walk
/// command with all IDs of the name to the **current** grid position of the first target object (no
/// following, `target` -1). Direction: the angle of the first existing unit that is not already standing
/// there, otherwise -1; `spread` -1. Event bit 1 is cleared only for the existing IDs; all go into the
/// command (n raw). A target without an object or an empty name sends nothing.
pub(crate) fn move_to_object(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    name: &str,
    target: &str,
    flags: i32,
) {
    for kind in plan_move_to_object(sim, name, target, flags) {
        issue_planned(sim, out, kind);
    }
}

/// The command of [`move_to_object`] (including clearing event bit 1 of the existing IDs).
pub(super) fn plan_move_to_object(
    sim: &mut Sim,
    name: &str,
    target: &str,
    flags: i32,
) -> Vec<CommandKind> {
    let Some(goal) = names::first_id(&sim.state.names, target).and_then(|id| grid_pos(sim, id))
    else {
        return Vec::new();
    };
    let ids = names::ids(&sim.state.names, name);
    let mut direction = DIRECTION_MINUS_1;
    for &id in &ids {
        let Some(at) = grid_pos(sim, id) else {
            continue;
        };
        set_object_event_bit(sim, id, 1, false);
        // The original adopts only the first angle (`local_5c == -1`).
        if at != goal && direction == DIRECTION_MINUS_1 {
            direction = direction_or_minus_1(at, goal);
        }
    }
    if ids.is_empty() {
        return Vec::new();
    }
    vec![CommandKind::Move {
        units: ids,
        x: goal.0,
        z: goal.1,
        target: -1,
        formation: 0,
        direction,
        spread: SPREAD_MINUS_1,
        flags,
    }]
}

/// 8 (flags 2) and 373 (flags 0) (`FUN_00437C50(player, from, guid, to, flags)`, `0x00437C50`): the
/// selection in the area `from` (filter `guid` as for 6, reference player `player`, `MYLIST`) goes in
/// **one** walk command to the center of `to`, without direction ([`NO_DIRECTION`]), formation 0,
/// `spread` 0. An unknown area, an unknown filter ID or no hits do nothing; no event bit is cleared.
pub(crate) fn move_area(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    player: i32,
    guid: i32,
    from: &str,
    to: &str,
    flags: i32,
) {
    for kind in plan_move_area(sim, player, guid, from, to, flags) {
        issue_planned(sim, out, kind);
    }
}

/// The command of [`move_area`].
pub(super) fn plan_move_area(
    sim: &mut Sim,
    player: i32,
    guid: i32,
    from: &str,
    to: &str,
    flags: i32,
) -> Vec<CommandKind> {
    let Some(goal) = area(sim, to) else {
        return Vec::new();
    };
    let units = select_in_area(sim, player, guid, from);
    if units.is_empty() {
        return Vec::new();
    }
    let (x, z) = center_fixed(goal);
    vec![CommandKind::Move {
        units,
        x,
        z,
        target: -1,
        formation: 0,
        direction: NO_DIRECTION,
        spread: 0,
        flags,
    }]
}

/// Area selection of actions 6, 8/373 and 34 (`FUN_00437190`, `FUN_00437C50`, `FUN_00435D50`): the
/// objects in the area `area_name` that pass the filter `guid` (`FUN_0047E390`) with the reference player
/// `player`, in list order of the collision manager. An unknown area or an unknown filter ID yields no
/// hits.
fn select_in_area(sim: &mut Sim, player: i32, guid: i32, area_name: &str) -> Vec<ObjectId> {
    let Some(rect) = area(sim, area_name) else {
        return Vec::new();
    };
    if !filter_known(sim, guid) {
        return Vec::new();
    }
    let f = filter::from_id(sim.rules, guid);
    collect_in_tile_rect(sim, rect, &f, player)
}

/// 163 (`forward`) and 164 (`FUN_00439880(forward, name, area)`, `0x00439880`): the **first** ID of the
/// name (without an object check) gets event bit 1 cleared and `{0x29|0x2A, 3, id, cx, cz}` with the
/// integer tile center of the area. It waits for bit **0** (E-148: the dispatcher clears bit 1 but waits
/// for bit 0). Unknown name or area: nothing (no clearing).
pub(crate) fn train(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    forward: bool,
    name: &str,
    area_name: &str,
) {
    let Some(id) = names::first_id(&sim.state.names, name) else {
        return;
    };
    let Some([x0, z0, x1, z1]) = area(sim, area_name) else {
        return;
    };
    set_object_event_bit(sim, id, 1, false);
    let target = (x0.wrapping_add(x1) / 2, z0.wrapping_add(z1) / 2);
    let op = if forward {
        TrainOp::Forward
    } else {
        TrainOp::Backward
    };
    issue_to(sim, out, &[id], |units| CommandKind::Train {
        train: units[0],
        op,
        target: Some(target),
    });
}

/// 165 (`open`) and 166 (`{0x19|0x1A, 1, first ID}`, `0x00423D9D`): the first ID of the name, for an
/// unknown name **0** - the original sends the command anyway, the handler finds nothing. In the
/// reimplementation [`Issued`] then reports `Rejected` with a reason.
pub(crate) fn gate(sim: &mut Sim, out: &mut Vec<Issued>, open: bool, name: &str) {
    let id = names::first_id(&sim.state.names, name).unwrap_or(ObjectId(0));
    issue_to(sim, out, &[id], |units| {
        if open {
            CommandKind::OpenGate { gate: units[0] }
        } else {
            CommandKind::CloseGate { gate: units[0] }
        }
    });
}

/// The four commands of the dispatcher case `0x6A`/`0x6B` (334 to 338): quarter, board a boat, mount and
/// dismount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairKind {
    /// 334 and 335 (`0x13`): `{0x13, 3, container, 1, id}` per ID of name 1; container = first ID of name 2.
    Quarter,
    /// 336 (`0x0C`): like `Quarter`, with the boat as the second name.
    Boat,
    /// 337 (`0x0A`): `{0x0A, 3, horse, 1, rider}` per ID of name 1 as **horse**; rider = first ID of
    /// name 2 (the fields are swapped compared with 334/336, E-154).
    Mount,
    /// 338 (`0x0B`): `{0x0B, 2, 1, id}` per ID of name 1; name 2 is dropped.
    Dismount,
}

/// 334 to 338 (`0x00425279…0x004253C7`, handlers `0x0047D8D0`, `0x0047D2A0`, `0x0047CFB0`,
/// `0x00475B10`). An unknown second name yields ID 0 - the original sends the command anyway (E-154):
/// quarter and mount then do nothing, the reimplementation rejects them with a reason (no effect); boarding
/// with boat 0 **has an effect** (abort of all modules, acknowledgment, emptying; the reimplementation
/// carries `NONE` instead of the ID, `riding.rs`). An unknown first name sends nothing (n = 0). No handler
/// checks ownership (E-214, E-217); the dispatcher does not clear event bit 1 (E-148, layer E handles the
/// waiting).
pub(crate) fn quarter(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    kind: PairKind,
    name1: &str,
    name2: &str,
) {
    let ids = names::ids(&sim.state.names, name1);
    // First ID of the second name, 0 for an unknown name (no object).
    let second = || names::first_id(&sim.state.names, name2).unwrap_or(ObjectId(0));
    match kind {
        PairKind::Quarter | PairKind::Boat => {
            let target = second();
            for id in ids {
                issue_to(sim, out, &[id], |units| match kind {
                    PairKind::Quarter => CommandKind::Quarter {
                        container: target,
                        units,
                    },
                    _ => CommandKind::BoardBoat {
                        boat: target,
                        units,
                    },
                });
            }
        }
        PairKind::Mount => {
            let rider = second();
            for horse in ids {
                let from = sender(sim, &[rider, horse]);
                let kind = CommandKind::Mount {
                    horse,
                    units: vec![rider],
                };
                issue(sim, out, from, kind);
            }
        }
        PairKind::Dismount => {
            for id in ids {
                issue_to(sim, out, &[id], |units| CommandKind::Dismount { units });
            }
        }
    }
}

/// 191 (`{0x3C, 2, 1, id}` per ID -> `CmdHalt` `0x004770A0`): abort all modules of the unit and (since
/// `direct`) empty their command queue. Unknown name: no command.
pub(crate) fn halt(sim: &mut Sim, out: &mut Vec<Issued>, name: &str) {
    for id in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[id], |units| CommandKind::Halt { units });
    }
}

/// 157 (`FUN_004368F0(name, anim, seconds)`, `{0x42, 3, id, anim, seconds}` per ID -> `CmdStartAnimationLock`
/// `0x004778D0`): one command per ID, in value order. No check of its own and no event bit; an unknown
/// name sends nothing, a stale ID fails in the handler (here: rejected and reported).
pub(crate) fn animation_lock(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    anim: i32,
    seconds: i32,
    name: &str,
) {
    for unit in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[unit], |_| CommandKind::StartAnimationLock {
            unit,
            anim,
            seconds,
        });
    }
}

/// 158 (`FUN_004369A0(name)`, `{0x43, 1, id}` per ID -> `CmdStopAnimationLock` `0x00477910`): one command
/// per ID; also clears a lock bit of other setters (E-213).
pub(crate) fn animation_unlock(sim: &mut Sim, out: &mut Vec<Issued>, name: &str) {
    for unit in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[unit], |_| CommandKind::StopAnimationLock {
            unit,
        });
    }
}

/// 167 (`FUN_00439990(attacker, target)`, command `0x11`): **one** attack of all IDs of the name
/// `attackers` on the first ID of the name `target`, modifier 0. The target is not checked (also dead,
/// own, tree, stale); an unknown target name or no attackers send nothing. The action does not wait
/// (catalog `WaitKind::None`).
pub(crate) fn attack(sim: &mut Sim, out: &mut Vec<Issued>, attackers: &str, target: &str) {
    let Some(target) = names::first_id(&sim.state.names, target) else {
        return;
    };
    let ids = names::ids(&sim.state.names, attackers);
    if ids.is_empty() {
        return;
    }
    issue_to(sim, out, &ids, |units| CommandKind::Attack {
        units,
        target,
        pos: None,
        mods: 0,
    });
}

/// 179 (`FUN_004373F0`, `{0x12, 3, mode, 1, id}` per ID -> `CmdSetBehavior` `0x004762E0`): one command per
/// ID. Stances outside 0 to 4 are rejected by the command with a reason (A7, the original does not check;
/// the shipped data uses 0 to 3).
pub(crate) fn behavior(sim: &mut Sim, out: &mut Vec<Issued>, mode: i32, name: &str) {
    for id in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[id], |units| CommandKind::SetBehavior {
            units,
            mode,
        });
    }
}

/// 34 (`FUN_00435D50(player, guid, area)`, command `0x10`): **one** `Die` for the area selection
/// (reference player `player`, filter as for 6); no hits, an unknown area or an unknown filter ID send
/// nothing. The sender is `player` if it is 0 to 7.
pub(crate) fn kill_in_area(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    player: i32,
    guid: i32,
    area_name: &str,
) {
    let ids = select_in_area(sim, player, guid, area_name);
    if ids.is_empty() {
        return;
    }
    let from = u8::try_from(player)
        .ok()
        .filter(|&p| usize::from(p) < PLAYER_SLOTS)
        .unwrap_or_else(|| sender(sim, &ids));
    issue(sim, out, from, CommandKind::Die { units: ids });
}

/// 193 (`FUN_00437790(name, 0x10)`, `{0x10, 2, 1, id}` per ID -> `CmdDie` `0x00480D90`): one `Die` per ID,
/// without ownership, building, death or quartering check (E-149, E-214).
pub(crate) fn kill_named(sim: &mut Sim, out: &mut Vec<Issued>, name: &str) {
    for id in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[id], |units| CommandKind::Die { units });
    }
}

/// 189 (`FUN_004376E0`/`FUN_00437790`, D case `0x4E`): per ID of the name the ability command for the
/// code - 0 ghost (button 436), 1 to 3 wolf, bear, wildcat (kind 0 to 2, buttons 441 to 443), 4 banishment
/// (button 439), 5 camouflage, 6 uncamouflage (reads the count as the ID, E-222), 7 arm, 8 dig in, 9 dig
/// out; any other code sends nothing. The buttons are fixed and always match; the original checks neither
/// menu, lock, module nor mana (the module draws the mana when the command is delivered).
pub(crate) fn special(sim: &mut Sim, out: &mut Vec<Issued>, code: i32, name: &str) {
    if !(0..=9).contains(&code) {
        return;
    }
    for id in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[id], |units| match code {
            0 => CommandKind::EvokeGhost {
                unit: id,
                button: EVOKE_GHOST_BUTTON,
            },
            1..=3 => {
                let art = (code - 1) as u8;
                CommandKind::EvokeAnimal {
                    unit: id,
                    art,
                    button: EVOKE_ANIMAL_BUTTONS[usize::from(art)],
                }
            }
            4 => CommandKind::Banish {
                unit: id,
                button: BANISH_BUTTONS[0],
            },
            5 => CommandKind::Mask { units },
            6 => CommandKind::Unmask { units },
            7 => CommandKind::ToggleArming { units, arm: true },
            8 => CommandKind::DigIn { units },
            _ => CommandKind::DigOut { units },
        });
    }
}

/// 190 (`FUN_0043A010(name, code, target)`, D case `0x4F`): per ID of the name a command with the target
/// "first ID of the target name" (unchecked: foreign, dead, tree, stale) - 0 construct, 1 mine gold,
/// 2 harvest, 3 deliver, 4 attack (modifier 0), 5 quarter, 6 chop wood. An unknown target name, a code
/// outside 0 to 6 or no IDs send nothing.
pub(crate) fn special_target(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    code: i32,
    name: &str,
    target: &str,
) {
    if !(0..=6).contains(&code) {
        return;
    }
    let Some(target) = names::first_id(&sim.state.names, target) else {
        return;
    };
    for id in names::ids(&sim.state.names, name) {
        issue_to(sim, out, &[id], |units| match code {
            0 => CommandKind::ConstructAt {
                building: target,
                units,
            },
            1 => CommandKind::MineGold {
                mine: target,
                units,
            },
            2 => CommandKind::Harvest {
                farm: target,
                units,
            },
            3 => CommandKind::Deliver {
                building: target,
                units,
            },
            4 => CommandKind::Attack {
                units,
                target,
                pos: None,
                mods: 0,
            },
            5 => CommandKind::Quarter {
                container: target,
                units,
            },
            _ => CommandKind::ChopWood {
                tree: target,
                units,
            },
        });
    }
}

/// 4 (`result` = [`DEFEAT`]) and 13 ([`VICTORY`]) (`{0x50000056, 2, p, 1|2}`, D cases `0x00422D52` and
/// `0x0042300E`; handler `0x0048D545` -> `GameEngine_SetPlayerResult` `0x00430CE0`): `p` is `atol` as
/// `i32`. A slot outside 0 to 7 silently does nothing in the original; the reimplementation reports this
/// as rejected and lets the block continue (no `Guarded`). Otherwise `0x56` runs via the script path
/// ([`game_end::set_player_result`](crate::game_end::set_player_result): an already decided slot stays, a
/// victory makes all slots that are not mutually allied lose, E-161); the sender is `p`.
pub(crate) fn player_result(sim: &mut Sim, out: &mut Vec<Issued>, p: i32, result: i32) {
    let Some(slot) = u8::try_from(p)
        .ok()
        .filter(|&s| usize::from(s) < PLAYER_SLOTS)
    else {
        out.push(Issued {
            code: 0x56,
            outcome: Outcome::Rejected(format!("slot {p} (allowed 0 to 7)")),
        });
        return;
    };
    issue(
        sim,
        out,
        slot,
        CommandKind::SetPlayerResult { slot, result },
    );
}

/// 38 (`{0x50000039, 2, p, guid}`, handler `0x0048D8B6` -> `Player_ApplyUpgrade(p, guid, 0)`): without
/// cost and ownership, without a duplicate check - sent twice it takes effect twice. `guid < 0` applies
/// all GUIDs 2800 to 3399. A slot outside 0 to 10 is rejected by the command (A7). The sender is `p` if it
/// is 0 to 7, otherwise slot 0.
pub(crate) fn upgrade(sim: &mut Sim, out: &mut Vec<Issued>, p: i32, guid: i32) {
    let from = u8::try_from(p)
        .ok()
        .filter(|&s| usize::from(s) < PLAYER_SLOTS)
        .unwrap_or(0);
    issue(sim, out, from, CommandKind::ApplyUpgrade { slot: p, guid });
}

/// 380 (`FUN_004316D0(k, z, guid, kind)`, E-168, E-218): the headquarters of the customer `k` is the first
/// object of the slot table `k` with the headquarters GUID of its nation (1300, 1600, 1000, 2500, 1900,
/// 2200 by `Player::nation`), finished, not dying and not quartered. If there is none (or `k` not in 0 to 7,
/// or no nation 0 to 5), silently nothing happens. Otherwise
/// `HireHeadhunter { art, target_player: z, target_guid: guid, building: HQ }` with the sender `k` goes via
/// the script path: button, lock, target menus and building state are dropped (`headhunter::check`), what
/// remains are kind 0 to 2, gold of the payer and
/// message 10002.
pub(crate) fn hire_headhunter(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    k: i32,
    art: i32,
    z: i32,
    guid: i32,
) {
    let Some(from) = u8::try_from(k)
        .ok()
        .filter(|&s| usize::from(s) < PLAYER_SLOTS)
    else {
        return;
    };
    let Some(hq_guid) = sim
        .state
        .players
        .get(usize::from(from))
        .and_then(|p| usize::try_from(p.nation).ok())
        .and_then(|n| NATION_HQ.get(n))
        .copied()
    else {
        return;
    };
    let hq = objects_of_slot(sim.state, from).into_iter().find(|&id| {
        sim.object(id).is_some_and(|o| {
            o.guid == hq_guid && o.status & (status::DIED | status::QUARTERED) == 0
        }) && is_finished(sim, id)
    });
    let Some(building) = hq else { return };
    issue(
        sim,
        out,
        from,
        CommandKind::HireHeadhunter {
            art,
            target_player: z,
            target_guid: guid,
            building,
        },
    );
}

/// Execute the command `kind` of the sender `player` via the script path and append the verdict to `out`.
/// `code` is the command type of the original without the high part `0x5000_0000`.
pub(crate) fn issue(sim: &mut Sim, out: &mut Vec<Issued>, player: u8, kind: CommandKind) {
    let code = kind.type_code() & !0x5000_0000;
    let outcome = submit_script(sim, &Command { player, kind });
    out.push(Issued { code, outcome });
}

/// Send a planned walk command via the script path (sender according to [`sender`] of the units).
fn issue_planned(sim: &mut Sim, out: &mut Vec<Issued>, kind: CommandKind) {
    let player = match &kind {
        CommandKind::Move { units, .. } => sender(sim, units),
        _ => 0,
    };
    issue(sim, out, player, kind);
}

/// [`issue`] for the units `units` with the sender according to [`sender`].
fn issue_to(
    sim: &mut Sim,
    out: &mut Vec<Issued>,
    units: &[ObjectId],
    kind: impl FnOnce(Vec<ObjectId>) -> CommandKind,
) {
    let player = sender(sim, units);
    issue(sim, out, player, kind(units.to_vec()));
}

/// Sender of a script command: the original has none; `Command::player` serves only log and messages, no
/// effect derives from it. The slot from the action parameter, where a command carries one (4/13, 34, 38,
/// 380), is passed by the caller directly to [`issue`]; otherwise the owner of the first existing unit
/// applies if it is 0 to 7, otherwise slot 0 (animals in slot 8, trees, nature and missing units, so that
/// `dispatch` does not reject the command because of the sender).
pub(crate) fn sender(sim: &Sim, units: &[ObjectId]) -> u8 {
    units
        .iter()
        .find_map(|&id| sim.object(id))
        .map(|o| o.owner())
        .filter(|&owner| usize::from(owner) < PLAYER_SLOTS)
        .unwrap_or(0)
}
