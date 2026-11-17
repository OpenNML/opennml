//! Late ability builtins `0x258…0x2F5` (re/ki-queries 7.1 pattern G, table 2 `0x503C … 0x512F`, 7.2, 7.4). Each flow
//! builds the command words of the original dispatcher as [`CommandKind`]: form 1 `(id)`, form 2 `(id, target or on)`,
//! form 3 `(id, kx, kz)` (only `0x2F1`); none pushes or writes outputs, only `0x2AA` sleeps 5000 ms afterwards (the
//! row, so after **every** query).
//!
//! The dispatch checks no ownership (E-26); the simulation checks the actor on the AI check path: ownership, death and
//! module remain; menu, button lock, MY filter, the button-to-kind tables and the target filter drop out - the AI sends
//! buttons that do not match the command bar (E-93 to E-95), and pays by the button sent.

use super::CmdCall;
use super::units::{give, oid};
use crate::combat::hit_zone::world_pos;
use crate::command::CommandKind;
use crate::ki::query::SimKiWorld;
use crate::ki::world::BuiltinCx;
use crate::module::building;
use crate::module::spell::{ART_EXEC_TEMP, ART_EXPLOSION, ART_REINCARNATE, ART_STORM};
use crate::object::status;

/// Button and index of `0x5E` per builtin (table 2; E-93: index and button do not match the command bar, the effect
/// follows the index, the payment follows the button).
fn exec_button(id: u16) -> (i32, i32) {
    match id {
        0x258 => (543, 0),
        0x259 => (544, 1),
        0x284 => (549, 2),
        0x296 => (522, 0),
        0x2A8 => (559, 1),
        0x2A9 => (560, 0),
        0x2AC => (557, 1),
        0x2BC => (561, 0),
        _ => (545, 2), // 0x2F0
    }
}

/// `0x258`, `0x259`, `0x284`, `0x296`, `0x2A8`, `0x2A9`, `0x2AC`, `0x2BC`, `0x2F0` `(id)` ->
/// `0x5E {id, button, idx}` ([`exec_button`]).
pub(super) fn exec_temp(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (button, idx) = exec_button(call.id);
    let kind = CommandKind::ExecTempUpgrade {
        object: oid(call.arg(0)),
        button,
        idx,
    };
    give(w, call, kind);
}

/// `0x26C` `(id)` -> Evoke ghost `0x1F {id, 436}`.
pub(super) fn evoke_ghost(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::EvokeGhost {
        unit: oid(call.arg(0)),
        button: 436,
    };
    give(w, call, kind);
}

/// `0x26D`-`0x26F` `(id)` -> Evoke animal `0x22`-`0x24 {id, 441 … 443}` (kind = type - `0x22`).
pub(super) fn evoke_animal(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let art = (call.id - 0x26D) as u8;
    let kind = CommandKind::EvokeAnimal {
        unit: oid(call.arg(0)),
        art,
        button: 441 + i32::from(art),
    };
    give(w, call, kind);
}

/// `0x280`/`0x281` `(id, target)` -> `0x36 {id, 1 or 2, x, z, 480}`: only if the target exists
/// (`GetObjectById(target, 0)`, also dead, E-96); x and z are the raw `f32` bits of its world position
/// (`GameObject_GetWorldPosition`). The storm (`0x281`) also sends button 480 (E-95).
pub(super) fn cast_at(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let Some(p) = world_pos(w.sim, oid(call.arg(1))) else {
        return;
    };
    let art = if call.id == 0x280 {
        ART_EXPLOSION
    } else {
        ART_STORM
    };
    let kind = CommandKind::CastSpell {
        caster: oid(call.arg(0)),
        art,
        anim: p[0].to_bits() as i32,
        idx: p[2].to_bits() as i32,
        button: 480,
    };
    give(w, call, kind);
}

/// `0x282` `(id)` -> Reincarnation `0x36 {id, 3, 0, 0, 482}`; `0x2BD` `(id)` -> kind 4 `0x36 {id, 4, 0, 0, 563}`.
pub(super) fn cast(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (art, button) = if call.id == 0x282 {
        (ART_REINCARNATE, 482)
    } else {
        (ART_EXEC_TEMP, 563)
    };
    let kind = CommandKind::CastSpell {
        caster: oid(call.arg(0)),
        art,
        anim: 0,
        idx: 0,
        button,
    };
    give(w, call, kind);
}

/// `0x283` `(id, target)` -> Kill `0x32 {id, target, 548}`; `0x2F5` with button 546 (E-95); `0x2EF` -> Eliminate
/// `0x33 {id, target, 546}` (E-95).
pub(super) fn special_attack(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let (unit, target) = (oid(call.arg(0)), oid(call.arg(1)));
    let kind = match call.id {
        0x283 => CommandKind::KillUnit {
            unit,
            target,
            button: 548,
        },
        0x2EF => CommandKind::SnipeUnit {
            unit,
            target,
            button: 546,
        },
        _ => CommandKind::KillUnit {
            unit,
            target,
            button: 546,
        },
    };
    give(w, call, kind);
}

/// `0x2AA`/`0x2AB` `(actor, target)` -> dynamite at building `0x30 {actor, target, 454}` or ship
/// `0x31 {actor, target, 455}`, only if the actor exists (also dead) and does not carry the lock bit `0x800000`
/// (`ANIM_LOCKED`) (E-96). The pause 5000 of `0x2AA` is set by the row, so also without a command; `0x2AB` never
/// sleeps.
pub(super) fn dynamite(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (unit, target) = (oid(call.arg(0)), oid(call.arg(1)));
    let free = w
        .sim
        .object(unit)
        .is_some_and(|o| o.status & status::ANIM_LOCKED == 0);
    if !free {
        return;
    }
    let kind = if call.id == 0x2AA {
        CommandKind::DynamiteBuilding {
            unit,
            target,
            button: 454,
        }
    } else {
        CommandKind::DynamiteShip {
            unit,
            target,
            button: 455,
        }
    };
    give(w, call, kind);
}

/// `0x294` `(id)` -> Whirlwind `0x4F {id, 542}`.
pub(super) fn whirlwind(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Whirlwind {
        unit: oid(call.arg(0)),
        button: 542,
    };
    give(w, call, kind);
}

/// `0x295` `(id, target)` -> Damage building `0x5F {id, Target+0x58, 551}`, only if the target exists (also dead) and
/// has building module 3 present and switched on (`GameObject_HasModule(3, 0)` on the **target**, E-96); the second
/// word is the ID of the found object.
pub(super) fn damage_building(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let Some(target) = w
        .sim
        .object(oid(call.arg(1)))
        .filter(|o| o.has_module(building::INDEX, false))
        .map(|o| o.id)
    else {
        return;
    };
    let kind = CommandKind::DamageBuilding {
        unit: oid(call.arg(0)),
        target,
        button: 551,
    };
    give(w, call, kind);
}

/// `0x297` `(id)` -> Self-healing `0x60 {id, 555}`.
pub(super) fn self_heal(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::SelfHeal {
        unit: oid(call.arg(0)),
        button: 555,
    };
    give(w, call, kind);
}

/// `0x2BE` `(id)` -> Rob corpses `0x4C {id, 540}`.
pub(super) fn rob_corpses(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::RobCorpses {
        unit: oid(call.arg(0)),
        button: 540,
    };
    give(w, call, kind);
}

/// `0x2EE` `(id)` -> Banish `0x25 {id, 547}` for every banisher type (E-94).
pub(super) fn banish(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Banish {
        unit: oid(call.arg(0)),
        button: 547,
    };
    give(w, call, kind);
}

/// `0x2F1` `(id, kx, kz)` -> Scout area `0x37 {id, kx, kz, 30, 5000, 1, 556}` (E-95: always 556).
pub(super) fn scout_area(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::ScoutArea {
        unit: oid(call.arg(0)),
        x: call.arg(1),
        z: call.arg(2),
        size: 30,
        duration_ms: 5000,
        flag: 1,
        button: 556,
    };
    give(w, call, kind);
}

/// `0x2F2` `(id, on)` -> `on ≠ 0` Start healing `0x34 {1, id}`, otherwise Stop healing `0x35`; `0x2F3` -> `on == 1`
/// Camouflage `0x1D {1, id}`, otherwise Uncamouflage `0x1E` (also for 2 or -1, E-96); `0x2F4` -> `on ≠ 0` Dig in
/// `0x20 {1, id}`, otherwise Dig out `0x21`.
pub(super) fn toggle(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let units = vec![oid(call.arg(0))];
    let on = call.arg(1);
    let kind = match (call.id, on) {
        (0x2F2, 0) => CommandKind::StopHealing { units },
        (0x2F2, _) => CommandKind::StartHealing { units },
        (0x2F3, 1) => CommandKind::Mask { units },
        (0x2F3, _) => CommandKind::Unmask { units },
        (_, 0) => CommandKind::DigOut { units },
        _ => CommandKind::DigIn { units },
    };
    give(w, call, kind);
}
