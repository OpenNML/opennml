//! Player and match queries of the dispatcher (re/ki-queries table 2, 5.16, 5.16.1). Left without a case (field
//! unchanged, E-27/E-37): `0x4E42`, `0x4E43`, `0x4E4A`, `0x4EE8`, `0x4EFC`, `0x4F47`, `0x4FD3`, `0x500F`, `0x50EB`
//! (without a counterpart in the original), the weather `0x5000` (E-39 does not apply) and the command codes.

use super::{Direct, argc, put, word};
use crate::message::Message;
use crate::module::headhunter;
use crate::player::View;
use crate::state::Control;

/// Query `20000`: nation of a player.
pub(crate) const PEOPLE: u32 = 0x4E20;

/// Answers the codes of this group; `false` for foreign codes.
pub(super) fn answer(
    view: &mut View<'_, '_>,
    code: u32,
    req: &mut [i32],
    _direct: &mut Vec<Direct>,
) -> bool {
    match code {
        PEOPLE => people(view, req),
        0x4E2D => single_player(view, req),
        0x4E2E => game_mode(view, req),
        0x4E2F => defined_slots(view, req),
        0x4E32 => control(view, req),
        0x4E37 | 0x4E38 => map_size(view, code, req),
        0x4E39 => difficulty(view, req),
        0x4E3C => constant_two(req),
        0x4E3E => resource(view, req),
        0x4E40 => relation(view, req),
        0x4E51 => headhunter_query(view, req),
        0x4E53 => living_space_limit(view, req),
        0x4E54 => free_living_space(view, req),
        0x4E55 => living_space(view, req),
        0x4EF7 => water_share(view, req),
        0x5118 => parity(req),
        0x5130 => island(view, req),
        _ => return false,
    }
    true
}

/// `argc == n`, otherwise the field stays (E-27).
fn has_argc(req: &[i32], n: i32) -> bool {
    argc(req) == n
}

/// `0x4E20` (argc 2): nation of `[3]`; slots 0-7 without an occupancy check, otherwise -1 (5.16).
fn people(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let nation = view.players(word(req, 3)).map_or(-1, |p| p.nation);
    put(req, 2, nation);
}

/// `0x4E2D` (argc 1): `GameEngine_IsSinglePlayerGame` = game type < 0 (E-75).
fn single_player(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 1) {
        put(req, 2, i32::from(view.mode() < 0));
    }
}

/// `0x4E2E` (argc 1): game type `GameEngine+0x108`.
fn game_mode(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 1) {
        put(req, 2, view.mode());
    }
}

/// `0x4E2F` (argc 1): number of the slots 0-7 defined in the level (`Player+0x218`, E-76).
fn defined_slots(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 1) {
        return;
    }
    let n = (0..8)
        .filter(|&p| view.players(p).is_some_and(|i| i.defined))
        .count();
    put(req, 2, n as i32);
}

/// `0x4E32` (argc 2): control kind of `[3]` (`Player+0x234` = `PLAYER_TYPE + 1`); slot missing -> 0.
fn control(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let kind = view.players(word(req, 3)).map_or(0, |p| match p.control {
        Control::Empty => 0,
        Control::Human => 1,
        Control::Ai => 2,
        Control::Passive => 3,
    });
    put(req, 2, kind);
}

/// `0x4E37`/`0x4E38` (argc 1): map width or height in tiles.
fn map_size(view: &View<'_, '_>, code: u32, req: &mut [i32]) {
    if !has_argc(req, 1) {
        return;
    }
    let (x, z) = view.map_size();
    put(req, 2, if code == 0x4E37 { x } else { z });
}

/// `0x4E39` (argc 1): difficulty `GameEngine+0x5F8`; at game type -1: 1 -> 3, 2 -> 5 (5.16). The clamp `< 0 → 1` is
/// done by the handler of `0x8C`, not by the dispatcher.
fn difficulty(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 1) {
        return;
    }
    let v = view.campaign_difficulty();
    let v = match (view.mode(), v) {
        (-1, 1) => 3,
        (-1, 2) => 5,
        _ => v,
    };
    put(req, 2, v);
}

/// `0x4E3C` (argc 1): `GameEngine+0x5FC`, set to 2 only in the constructor.
fn constant_two(req: &mut [i32]) {
    if has_argc(req, 1) {
        put(req, 2, 2);
    }
}

/// Resource `r` of slot `p`; slot or index invalid -> 0; foreign and hidden under `Fog` -> 0.
fn resource_of(view: &View<'_, '_>, p: i32, r: i32) -> i32 {
    if view.players(p).is_none() {
        return 0;
    }
    view.resource(p, r).unwrap_or(0)
}

/// `0x4E3E` (argc 3): `Player_GetResource([3], [4])`.
fn resource(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 3) {
        put(req, 2, resource_of(view, word(req, 3), word(req, 4)));
    }
}

/// `0x4E40` (argc 3): bit 0 friend, bit 1 enemy, bit 2 shared vision from `[3]` to `[4]` (E-81).
fn relation(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 3) {
        return;
    }
    let (friend, enemy, vision) = view.relation(word(req, 3), word(req, 4));
    put(
        req,
        2,
        i32::from(friend) | i32::from(enemy) << 1 | i32::from(vision) << 2,
    );
}

/// `0x4E51` (without argc): headhunter of kind `[3]` (5.16.1). No headhunter (also hidden) -> `[2] = −1`, `[4]…[6]`
/// stay; otherwise message `0x222` and `[2]` target player, `[4]` customer, `[5]` target GUID, `[6]` price. If the
/// module does not answer (dead headhunter), the zeros of the message remain.
fn headhunter_query(view: &mut View<'_, '_>, req: &mut [i32]) {
    let Some(hh) = view.headhunter(word(req, 3)) else {
        put(req, 2, -1);
        return;
    };
    let mut msg = Message::new(headhunter::msg::PRICE, &[0; 4]);
    if view.message(hh, &mut msg).is_none() {
        put(req, 2, -1);
        return;
    }
    // Reply in the arguments: price, target player, target GUID, customer (`msg[2]…msg[5]`).
    put(req, 2, msg.arg(1));
    put(req, 6, msg.arg(0));
    put(req, 4, msg.arg(3));
    put(req, 5, msg.arg(2));
}

/// `0x4E53` (argc 1): living space limit `GameEngine+0x5EC`.
fn living_space_limit(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 1) {
        put(req, 2, view.max_living_space());
    }
}

/// `0x4E54` (argc 2): free living space `R4 (limited) − R3` of `[3]`; slot invalid -> 0.
fn free_living_space(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 2) {
        return;
    }
    let p = word(req, 3);
    let free = resource_of(view, p, 4).wrapping_sub(resource_of(view, p, 3));
    put(req, 2, free);
}

/// `0x4E55` (argc 2): living space `R4` (limited) of `[3]`; slot invalid -> 0.
fn living_space(view: &View<'_, '_>, req: &mut [i32]) {
    if has_argc(req, 2) {
        put(req, 2, resource_of(view, word(req, 3), 4));
    }
}

/// `0x4EF7` (argc 1): water share `+0x5F0 · 100 / +0x5F4` (E-83). Without tiles like before loading 100 % (the original
/// divides by 0, exception A7).
fn water_share(view: &View<'_, '_>, req: &mut [i32]) {
    if !has_argc(req, 1) {
        return;
    }
    let (water, tiles) = view.water_share();
    put(
        req,
        2,
        water.wrapping_mul(100).checked_div(tiles).unwrap_or(100),
    );
}

/// `0x5118` (without argc): `[2] % 2` as a C remainder.
fn parity(req: &mut [i32]) {
    put(req, 2, word(req, 2).wrapping_rem(2));
}

/// `0x5130` (without argc): island flag `GameEngine+0x30C` - 1 only for random maps from the template `Islands.rnd`
/// (`GameEngine_CreateRandomMapLevel` `0x004402F0`), fixed maps 0 (E-82).
fn island(view: &View<'_, '_>, req: &mut [i32]) {
    let island = view.rules().random_map.as_ref().is_some_and(|r| r.island);
    put(req, 2, i32::from(island));
}
