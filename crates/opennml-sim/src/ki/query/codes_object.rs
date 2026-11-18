//! Object queries of the dispatcher (re/ki-queries table 2, 5.7, 5.14, 5.15, 5.20).
//!
//! All cases search like `GetObjectById(id, 0)`, i.e. also dying and dead objects (E-55); only `0x4E2A` (and its
//! negation `0x4F65`) exclude dead ones. A missing object or one hidden under `Fog` ([`View::object`] -> `None`) is
//! answered by every case like the original: mostly 0, `0x4F2E`/`0x4F38`/`0x4FAB`/`0x4E28`/`0x4E29` leave `[2]` (the
//! player number) in place, `0x4FC9`/`0x5113` return -1, `0x4F65` 1.
//!
//! Queries to modules go through [`View::message`] (only messages from [`crate::player::QUERY_MESSAGES`]); a dead
//! object then answers as its module does (attack module -1, building and container module unchanged, 5.20).

use super::filters::filter_from_id;
use super::{Direct, argc, dispatch, put, word};
use crate::filter::type_has_module_desc;
use crate::fx::ftol;
use crate::id::ObjectId;
use crate::message::Message;
use crate::module::building::msg as building_msg;
use crate::player::View;

/// Answers the codes of this group; `false` for foreign codes.
pub(super) fn answer(
    view: &mut View<'_, '_>,
    code: u32,
    req: &mut [i32],
    _direct: &mut Vec<Direct>,
) -> bool {
    match code {
        0x4E28 | 0x4E29 => matches_filter(view, req),
        0x4E2A => alive(view, req),
        0x4E2B => percent(view, req, 14),
        0x4E2C => position(view, req),
        0x4E89 => production_guid(view, req),
        0x4F2E => sight_range(view, req),
        0x4F38 => production_percent(view, req),
        0x4F42 => queue_length(view, req),
        0x4F65 => dead_or_missing(view, req),
        0x4F6F => type_has_module(view, req, 3),
        0x4F79 => attack_mode(view, req),
        0x4F94 => type_has_module(view, req, 8),
        0x4F9C => percent(view, req, 15),
        0x4FA1 => percent(view, req, 0x1C),
        0x4FAB => attack_target(view, req),
        0x4FB5 => attacking(view, req),
        0x4FC4 => type_has_module(view, req, 0xD),
        0x4FC9 => type_guid(view, req),
        0x4FF1 => max_reach(view, req),
        0x502D => stubborn(view, req),
        0x5014 => commanded_target(view, req),
        0x50BE => last_attacker_exists(view, req),
        0x50D7 => resource_amount(view, req, 0x1A),
        0x50DC => resource_amount(view, req, 0x18),
        0x5113 => quarter_count(view, req),
        0x511D => own_color(view, req),
        0x5122 => color_owner_is_friend(view, req),
        _ => return false,
    }
    true
}

/// `0x4FB5` for object `id` from the view of slot `me`: 1 if it attacks (status 1-2 of the attack module). The path of
/// the list query `0x114`/`0x11F`: a handler call from the loop would not work next to a live [`View`].
pub(crate) fn is_attacking(view: &mut View<'_, '_>, me: i32, id: i32) -> i32 {
    let mut req = [0x4FB5, 2, me, id];
    dispatch(view, &mut req, &mut Vec::new());
    word(&req, 2)
}

/// `argc == n`, otherwise the field stays (E-27).
fn has_argc(req: &[i32], n: i32) -> bool {
    argc(req) == n
}

/// Object ID from word `i`.
fn id_at(req: &[i32], i: usize) -> ObjectId {
    ObjectId::from_i32(word(req, i))
}

/// Does the object exist (also dead)?
fn exists(view: &View<'_, '_>, id: ObjectId) -> bool {
    view.object(id, false).is_some()
}

/// Message `code` with `args` to the object; return value and reply, `None` without an object.
fn ask_object(
    view: &mut View<'_, '_>,
    id: ObjectId,
    code: u32,
    args: &[i32],
) -> Option<(i32, Message)> {
    let mut msg = Message::new(code, args);
    let r = view.message(id, &mut msg)?;
    Some((r, msg))
}

/// `0x4E28`/`0x4E29` (argc 3): does `[3]` match the filter `[4]` for the player `[2]`? Missing object -> `[2]` stays;
/// dead ones are checked and fail the DIED precheck (except filter 182).
fn matches_filter(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 3) {
        return;
    }
    let id = id_at(req, 3);
    if !exists(view, id) {
        return;
    }
    let filter = filter_from_id(view.rules(), word(req, 4));
    let ok = view.matches(id, &filter, word(req, 2));
    put(req, 2, i32::from(ok));
}

/// `0x4E2A` (argc 2): is `[3]` alive? The only one with `GetObjectById(id, 1)`; wrong argc -> 0 (`0x004AA799`).
fn alive(view: &View<'_, '_>, req: &mut [i32]) {
    let alive = has_argc(req, 2) && view.object(id_at(req, 3), true).is_some();
    put(req, 2, i32::from(alive));
}

/// `0x4F65` (argc 2): is `[3]` missing or dead? The negation of `0x4E2A`.
fn dead_or_missing(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 2) {
        put(
            req,
            2,
            i32::from(view.object(id_at(req, 3), true).is_none()),
        );
    }
}

/// Fill level of attribute `attr` in percent (`GetCurrentFraction / 1000`, 0...100); missing -> 0. `0x4E2B` (hit
/// points, 14), `0x4F9C` (mana, 15: without mana 100, E-70), `0x4FA1` (air, 28); argc 2.
fn percent(view: &View<'_, '_>, req: &mut [i32], attr: u32) {
    if has_argc(req, 2) {
        let pct = view
            .fraction(id_at(req, 3), attr)
            .map_or(0, |fraction| fraction / 1000);
        put(req, 2, pct);
    }
}

/// `0x4E2C` (argc 4, 5.15.1): `[2]` found, `[3]`/`[4]` tile (`grid >> 12`, rounded toward negative infinity) of the
/// current position. Without an object `[2] = 0` and the outputs 0 instead of the stack leftovers of the original
/// (E-73); `[5]` stays.
fn position(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 4) {
        return;
    }
    let (found, x, z) = view
        .object(id_at(req, 3), false)
        .map_or((0, 0, 0), |o| (1, o.grid.x.to_int(), o.grid.z.to_int()));
    put(req, 3, x);
    put(req, 2, found);
    put(req, 4, z);
}

/// `0x4E89` (argc 2): GUID of the running production order (`0xF03`, `msg[2]`), otherwise 0. The return value of the
/// message does not count.
fn production_guid(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let guid = ask_object(view, id_at(req, 3), building_msg::PROGRESS, &[0, 0])
        .map_or(0, |(_, msg)| msg.arg(0));
    put(req, 2, guid);
}

/// `0x4F38` (argc 2): progress in percent, `trunc(f32 · 100.0f)` (24-bit sequence of the x87); only on return value 1
/// (running order). Otherwise - no order, no or switched-off building module, object missing - `[2]` stays (B-6). Dead
/// buildings return the frozen state (5.20).
fn production_percent(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    if let Some((1, msg)) = ask_object(view, id_at(req, 3), building_msg::PROGRESS, &[0, 0]) {
        let progress = f32::from_bits(msg.arg(1) as u32);
        put(req, 2, ftol(progress * 100.0));
    }
}

/// `0x4F42` (without argc, 5.15.2): length of the build list (`0x1003` into a buffer for 40 GUIDs). Without a building
/// module or with a switched-off one the initial size 40 remains (E-69); dead buildings return their frozen list;
/// missing object -> 0.
fn queue_length(view: &mut View<'_, '_>, req: &mut [i32]) {
    let len = ask_object(view, id_at(req, 3), building_msg::LIST, &[0; 40])
        .map_or(0, |(_, msg)| msg.args.len());
    put(req, 2, i32::try_from(len).unwrap_or(i32::MAX));
}

/// `0x4F2E` (argc 2): sight range in whole tiles, `attribute 18 / 4` rounded toward 0; missing object -> `[2]` stays.
fn sight_range(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    if let Some(sight) = view.attribute(id_at(req, 3), 18) {
        put(req, 2, sight / 4);
    }
}

/// `0x4F6F`/`0x4F94`/`0x4FC4` (argc 1, 5.15.3): does the type with the GUID `[2]` have the module `module` (3 building,
/// 8 felling, 0xD riding) in any variant (descriptor with mask != 0)? Unknown GUID -> 0.
fn type_has_module(view: &View<'_, '_>, req: &mut [i32], module: u32) {
    if !has_argc(req, 1) {
        return;
    }
    let has = type_has_module_desc(view.rules(), word(req, 2) as u32, module);
    put(req, 2, i32::from(has));
}

/// `0x4F79` (argc 2): attack mode `0x711` (stance 0...4); missing object -> 0, dead or without (switched-on) attack
/// module -1 (E-71).
fn attack_mode(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let mode = ask_object(view, id_at(req, 3), 0x711, &[]).map_or(0, |(r, _)| r);
    put(req, 2, mode);
}

/// `0xC11 {0}` reply of the attack module: (status, target ID); `None` without an object. Status 1-2 = attacks
/// (approach, blow/shot); dead ones and objects without the module answer -1.
fn attack_status(view: &mut View<'_, '_>, id: ObjectId) -> Option<(bool, i32)> {
    let (status, msg) = ask_object(view, id, 0xC11, &[0])?;
    Some(((1..=2).contains(&status), msg.arg(0)))
}

/// `0x4FAB` (argc 2): target ID (`m_TargetHID`, unchecked) as long as the object attacks, otherwise 0; missing object
/// -> `[2]` stays.
fn attack_target(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    if let Some((attacking, target)) = attack_status(view, id_at(req, 3)) {
        put(req, 2, if attacking { target } else { 0 });
    }
}

/// `0x4FB5` (argc 2): 1 as long as the object attacks, otherwise (also missing) 0.
fn attacking(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let yes = attack_status(view, id_at(req, 3)).is_some_and(|(attacking, _)| attacking);
    put(req, 2, i32::from(yes));
}

/// `0x4FC9` (argc 1, `[2]` = object ID): type GUID (`GO+0x54`); missing -> -1.
fn type_guid(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 1) {
        let guid = view
            .object(id_at(req, 2), false)
            .map_or(-1, |o| o.guid as i32);
        put(req, 2, guid);
    }
}

/// `0x4FF1` (without argc, `[2]` = object ID): greatest attack range `0xD11`, fixed point (4096 = 1 tile); negative
/// (pure melee fighters, dead ones) or missing object -> 0.
fn max_reach(view: &mut View<'_, '_>, req: &mut [i32]) {
    let reach = ask_object(view, id_at(req, 2), 0xD11, &[]).map_or(0, |(r, _)| r.max(0));
    put(req, 2, reach);
}

/// `0x502D` (argc 2): stubbornness (`0xF11`) exactly 4 ("deaf to the target search"); larger values and -1 give 0.
fn stubborn(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let yes = ask_object(view, id_at(req, 3), 0xF11, &[]).is_some_and(|(r, _)| r == 4);
    put(req, 2, i32::from(yes));
}

/// `0x5014` (argc 3): reply to `0x1411 {[4]}`. The attack module compares the argument counter (1) with the target ID,
/// i.e. 1 only for target ID 1 and stubbornness > 3; `[4]` never has an effect (E-74). Dead ones and objects without an
/// attack module -1 (E-71); missing object -> 0.
fn commanded_target(view: &mut View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 3) {
        return;
    }
    let r = ask_object(view, id_at(req, 3), 0x1411, &[word(req, 4)]).map_or(0, |(r, _)| r);
    put(req, 2, r);
}

/// `0x50BE` (argc 2, 5.14): do the object and its last attacker still exist under their ID? "Exists" means
/// `GetObjectById(…, 0)`, dead attackers count (E-55).
fn last_attacker_exists(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let both = view
        .object(id_at(req, 3), false)
        .map(|o| o.last_attacker)
        .is_some_and(|attacker| exists(view, attacker));
    put(req, 2, i32::from(both));
}

/// `0x50D7`/`0x50DC` (argc 2): gold (attribute 26) or food (attribute 24) of the object - for resource objects the
/// remaining amount, otherwise the maximum; missing -> 0.
fn resource_amount(view: &View<'_, '_>, req: &mut [i32], attr: u32) {
    if has_argc(req, 2) {
        put(req, 2, view.attribute(id_at(req, 3), attr).unwrap_or(0));
    }
}

/// `0x5113` (without argc): occupant count (`0x2217`); without a container module and with a missing object -1, no
/// death check.
fn quarter_count(view: &mut View<'_, '_>, req: &mut [i32]) {
    let count = ask_object(view, id_at(req, 3), 0x2217, &[]).map_or(-1, |(r, _)| r);
    put(req, 2, count);
}

/// Color of slot `p` (`GameWorld_GetPlayerColor`): slots 0-7 their color, otherwise 8.
fn player_color(view: &View<'_, '_>, p: i32) -> i32 {
    view.players(p).map_or(8, |info| info.color)
}

/// `0x511D` (without argc): does the object have the display color (`GO+0x2D0`) of the slot `[2]`? The color is
/// compared, not the owner (E-72); missing object -> 0.
fn own_color(view: &View<'_, '_>, req: &mut [i32]) {
    let same = view
        .object(id_at(req, 3), false)
        .is_some_and(|o| o.color == player_color(view, word(req, 2)));
    put(req, 2, i32::from(same));
}

/// `0x5122` (without argc): `q` = first slot 0-7 with the color of the object (otherwise 8); result "`q` regards `[2]`
/// as a friend" (`HasFriendRelation(q, [2])`, own slot -> 1, `q = 8` -> 0); missing object -> 0 (E-72).
fn color_owner_is_friend(view: &View<'_, '_>, req: &mut [i32]) {
    let friend = view.object(id_at(req, 3), false).is_some_and(|o| {
        let q = (0..8)
            .find(|&p| view.players(p).is_some_and(|info| info.color == o.color))
            .unwrap_or(8);
        view.relation(q, word(req, 2)).0
    });
    put(req, 2, i32::from(friend));
}
