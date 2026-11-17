//! Command builtins for combat, behavior, AI modes, diplomacy, resources and removal (re/ki-queries 7.1/7.2). As in
//! [`super::units`] each flow builds the command words of the original dispatcher as [`CommandKind`]; ownership of the
//! actor and own slot are checked by the simulation (exception A8, AI check path). None of these rows pushes a result
//! or writes output variables.
//!
//! Pattern B (`0x154`, `0x155`, `0x158`/`0x15A`, `0x159`/`0x15B`, `0x18B`): the **last** pop (the first script argument
//! `a1`) is the slot or object, the handlers have no `me`.

use super::CmdCall;
use super::units::{give, oid};
use crate::command::CommandKind;
use crate::filter::matches;
use crate::id::ObjectId;
use crate::ki::query::SimKiWorld;
use crate::ki::query::handlers::checked_list_ids;
use crate::ki::world::BuiltinCx;
use crate::module::building;
use crate::module::trading::hq_guid;
use crate::upgrades::objects_of_slot;
use nml_formats::targets::MatchFilter;

/// Walk module (index 0): an attack target must have it switched on if it is not a building.
const WALK: usize = 0;

/// `0x97` `(p, q, m)` (`0x4E41`, pattern A): `0x63 {p, q, (m >> 2) & 1}`, then by `m & 3` `0x64 {p, q, 2/0/1}` (0 ->
/// neutral, 1 -> friend, 2 -> enemy, 3 -> none; table `0x004ABE9C`). Both slots are arbitrary; the simulation admits
/// only the own sender.
pub(super) fn diplomacy(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (p, q, m) = (call.arg(0), call.arg(1), call.arg(2));
    give(
        w,
        call,
        CommandKind::SharedVision {
            p,
            q,
            on: (m >> 2) & 1 != 0,
        },
    );
    let mode = match m & 3 {
        0 => 2,
        1 => 0,
        2 => 1,
        _ => return,
    };
    give(w, call, CommandKind::Diplomacy { p, q, mode });
}

/// `0xA1`-`0xA3`, `0x12F` `(obj)` (`0x4E46`/`0x4E87`, E-92): the object must exist (`GetObjectById(id, 0)`, also dead
/// or dying). With building module 3 **present and switched on** -> Sell `0x67 {1, id}`, otherwise Die `0x10 {1, id}`
/// (a building with module 3 switched off therefore dies).
pub(super) fn remove(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let id = oid(call.arg(0));
    let Some(o) = w.sim.object(id) else { return };
    let kind = if o.has_module(building::INDEX, false) {
        CommandKind::SellBuildings {
            buildings: vec![id],
        }
    } else {
        CommandKind::Die { units: vec![id] }
    };
    give(w, call, kind);
}

/// Filter of the main house search of `FUN_004316D0`: `MM_HL_MYLIST | MM_ANDALL | MM_CHECKGUID | HANDLETYPE`, type the
/// main house GUID of the nation, class `0x403` (finished building).
fn hq_filter(guid: u32) -> MatchFilter {
    MatchFilter {
        mode: 0x11A,
        guid: guid as i32,
        match_bits: 0x403,
        ..MatchFilter::constructed()
    }
}

/// `0xCE` `(target_player, guid, kind)` (`0x4E50`, without `argc` check): the main house is the **first** finished
/// object with the main house GUID of the nation of `me` in the object table of `me` (`FUN_00477B30`, table order, not
/// the nearest); none -> no command. Otherwise `0x2C {kind, target_player, guid, main_house}` (the field order is
/// swapped compared to the script arguments, `FUN_0049E710`).
pub(super) fn hire_headhunter(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let me = i32::from(call.me);
    let filter = hq_filter(hq_guid(w.sim, me));
    let hq = objects_of_slot(w.sim.state, call.me)
        .into_iter()
        .find(|&id| matches(w.sim, id, &filter, me));
    let Some(hq) = hq else { return };
    let kind = CommandKind::HireHeadhunter {
        art: call.arg(2),
        target_player: call.arg(0),
        target_guid: call.arg(1),
        building: hq,
    };
    give(w, call, kind);
}

/// `0x13B` `(bld, value)` -> `0x59 {bld, value}` (`0x5104`).
pub(super) fn occupancy_border(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let kind = CommandKind::OccupancyBorder {
        building: oid(call.arg(0)),
        border: call.arg(1),
    };
    give(w, call, kind);
}

/// Slot and number of leading arguments of the AI mode rows: `0x154`/`0x155` take the slot from the stack (`a1`,
/// pattern B), `0x156`/`0x157` the own one.
fn slot_and_rest(call: &CmdCall<'_>, with_slot: u16) -> (i32, usize) {
    if call.id == with_slot {
        (call.arg(0), 1)
    } else {
        (i32::from(call.me), 0)
    }
}

/// `0x154` `(slot, a, b)` / `0x156` `(a, b)` (`0x4FDD`) -> `0x68 {slot, a, b}`.
pub(super) fn ai_player_attack_mode(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let (slot, i) = slot_and_rest(call, 0x154);
    let kind = CommandKind::AiPlayerAttackMode {
        slot,
        aggressiveness: call.arg(i),
        stubbornness: call.arg(i + 1),
    };
    give(w, call, kind);
}

/// `0x155` `(slot, a, b, c, d)` / `0x157` `(a, b, c, d)` (`0x4FE2`) ->
/// `0x6A {slot, a == 1, b, c == 1, d == 1}` (walk flags, no attack flags, E-102).
pub(super) fn ai_player_walk_flags(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let (slot, i) = slot_and_rest(call, 0x155);
    let kind = CommandKind::AiPlayerWalkFlags {
        slot,
        mobile_home: call.arg(i) == 1,
        tether: call.arg(i + 1),
        follow: call.arg(i + 2) == 1,
        abort: call.arg(i + 3) == 1,
    };
    give(w, call, kind);
}

/// `0x158`/`0x15A` `(obj, a, b)` (`0x4FE7`) -> `0x6B {obj, a, b}`; the object may belong to anyone (the slot comes from
/// its ID).
pub(super) fn ai_object_attack_mode(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let kind = CommandKind::AiObjectAttackMode {
        object: oid(call.arg(0)),
        aggressiveness: call.arg(1),
        stubbornness: call.arg(2),
    };
    give(w, call, kind);
}

/// `0x159`/`0x15B` `(obj, a, b, c, d)` (`0x4FEC`) -> `0x6C {obj, a == 1, b, c == 1, d == 1}`.
pub(super) fn ai_object_walk_flags(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let kind = CommandKind::AiObjectWalkFlags {
        object: oid(call.arg(0)),
        mobile_home: call.arg(1) == 1,
        tether: call.arg(2),
        follow: call.arg(3) == 1,
        abort: call.arg(4) == 1,
    };
    give(w, call, kind);
}

/// `0x18B` `(slot, resource, amount)` (`0x5005`) -> `0x7E {slot, resource, amount}`; slot -1 =
/// own slot.
pub(super) fn add_resource(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let slot = match call.arg(0) {
        -1 => i32::from(call.me),
        slot => slot,
    };
    let kind = CommandKind::AddResource {
        slot,
        resource: call.arg(1),
        amount: call.arg(2),
    };
    give(w, call, kind);
}

/// `0x198`/`0x199` `(u, mode)` (`0x4F3D`) -> `0x12 {mode, 1, u}`.
pub(super) fn set_behavior(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::SetBehavior {
        units: vec![oid(call.arg(0))],
        mode: call.arg(1),
    };
    give(w, call, kind);
}

/// The list rows of pattern E `(&obj, start, n, value)`: `var(&obj) ≠ 0` and `0 < n < 100`, the IDs `obj[start + i]`
/// creating missing keys like the group movement ([`checked_list_ids`]), but **without** a filter per ID: dead, missing
/// and duplicate ones are included. Plus the fourth value.
fn unfiltered_list(
    call: &CmdCall<'_>,
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) -> Option<(Vec<ObjectId>, i32)> {
    let ids = checked_list_ids(cx, w, call.id, call.arg(0) as u32, call.arg(1), call.arg(2))?;
    Some((ids.into_iter().map(oid).collect(), call.arg(3)))
}

/// `0x19B`/`0x19C` `(&obj, start, n, mode)` (`0x4F5B`, pattern E): per ID of the list one `0x12 {mode, 1, id}`, without
/// a filter.
pub(super) fn set_behavior_list(
    call: &CmdCall<'_>,
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let Some((ids, mode)) = unfiltered_list(call, cx, w) else {
        return;
    };
    for id in ids {
        give(
            w,
            call,
            CommandKind::SetBehavior {
                units: vec![id],
                mode,
            },
        );
    }
}

/// `0x1A2` `(u)` (`0x4FBA`) -> `0x38 {1, u}` (Arm).
pub(super) fn arm(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::ToggleArming {
        units: vec![oid(call.arg(0))],
        arm: true,
    };
    give(w, call, kind);
}

/// `0x1A4`-`0x1A6` `(u, target)` (`0x4F60`): the target must exist (also dead) and have module 3 or 0 switched on
/// (dying units with Walk switched off are no target, dead buildings are); then `0x11 {target, 1, u, 0}`. The attacker
/// is unchecked.
pub(super) fn attack(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let target = oid(call.arg(1));
    let valid = w
        .sim
        .object(target)
        .is_some_and(|t| t.has_module(building::INDEX, false) || t.has_module(WALK, false));
    if valid {
        let kind = CommandKind::Attack {
            units: vec![oid(call.arg(0))],
            target,
            pos: None,
            mods: 0,
        };
        give(w, call, kind);
    }
}

/// `0x1A7`-`0x1A9` `(&obj, start, n, target)` (`0x4F74`, pattern E): per ID of the list one `0x11 {target, n, id, 0}`
/// without a filter and without a module check of the target. The original writes n instead of 1 into the count and
/// thus reads nonsense behind the command (E-88; exception A7): exactly one attack of this unit per ID.
pub(super) fn attack_list(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let Some((ids, target)) = unfiltered_list(call, cx, w) else {
        return;
    };
    for id in ids {
        give(
            w,
            call,
            CommandKind::Attack {
                units: vec![id],
                target: oid(target),
                pos: None,
                mods: 0,
            },
        );
    }
}

/// `0x1B8` `(mode)` (`0x4FFB`): `0x69 {me, mode}`, then in the same buffer for mode 3 or 2
/// `0x68 {me, 4, 2 or 1}`.
pub(super) fn ai_behavior(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (slot, mode) = (i32::from(call.me), call.arg(0));
    give(w, call, CommandKind::AiPlayerBehavior { slot, mode });
    let stubbornness = match mode {
        3 => 2,
        2 => 1,
        _ => return,
    };
    let kind = CommandKind::AiPlayerAttackMode {
        slot,
        aggressiveness: 4,
        stubbornness,
    };
    give(w, call, kind);
}
