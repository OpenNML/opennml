//! Counts and object counters of the dispatcher (re/ki-queries 5.12; E-30, E-64 ... E-67): object count per GUID and
//! filter, construction orders, counts per rectangle, buildability and purchasability, counter-upgrades, research
//! button.
//!
//! Quirks of the original that are listed here:
//! - The rectangle counts (`0x4E4B`, `0x4F8D`, `0x4F56`) count objects of **all** owners; only the list mode of the
//!   filter restricts to the reference player (E-65). The tile check is `PtInRect` with a wrapping `+ 1`
//!   ([`crate::area::collect_in_tile_rect`]).
//! - `0x4F56` (center and half-width) and `0x4F92` (corners like `0x4F8D`) sum over the reference players 0-6 (slot 7
//!   is missing, E-30): an object counts once per reference player for which it passes the filter, up to 7 times
//!   without list mode (E-64).
//! - `0x4E48`/`0x4E49` check `argc == 1` but read `[3]` (re/ki-queries 4 no. 14); `0x4E49` does not count quartered
//!   units, `0x4E47` with a GUID counts them (E-66).
//! - `0x510E` has no `argc` check.
//! - `0x4E8A` shows a shortage message in the original if the slot is the local player; the reimplementation is silent
//!   (E-67).
//!
//! A filter that does not exist (filter ID < 1000 without an entry in `flowcontroltragets.ini`) counts 0 (E-66). Under
//! `Sight::Fog`, foreign object counts and orders return 0, rectangles count only seen objects ([`View`]).

use opennml_formats::targets::MatchFilter;

use super::codes_search::HQ_BY_NATION;
use super::filters::filter_from_id;
use super::{Direct, argc, put, word};
use crate::player::View;

/// Reference players of the sum of `0x4F56`/`0x4F92`: 0-6, slot 7 is missing (`iVar3 < 7`, E-30).
const SUM_PLAYERS: i32 = 7;

/// Answers the codes of this group; `false` for foreign codes.
pub(super) fn answer(
    view: &mut View<'_, '_>,
    code: u32,
    req: &mut [i32],
    _direct: &mut Vec<Direct>,
) -> bool {
    match code {
        0x4E3F => has_guid(view, req),
        0x4E47 => owned_count(view, req, false),
        0x4EFD => owned_count(view, req, true),
        0x4E48 => module_count(view, req, 3),
        0x4E49 => module_count(view, req, 0),
        0x4E4B => centred_rect_count(view, req),
        0x4F8D => corner_rect_count(view, req),
        0x4F56 => summed_square_count(view, req),
        0x4F92 => summed_corner_count(view, req),
        0x4E8A => can_build_or_make(view, req),
        0x4F6A => pending_orders(view, req),
        0x4F33 => research_button(view, req),
        0x510E => counter_upgrades_of_slot(view, req),
        0x5032 => counter_upgrade_running(view, req),
        _ => return false,
    }
    true
}

/// Number as `i32` (lengths are small, otherwise saturated).
fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Filter `FUN_0047E390(id)`; `None` if the entry does not exist (filter ID < 1000 without an entry): the count is then
/// 0.
fn filter_of(view: &View<'_, '_>, id: i32) -> Option<MatchFilter> {
    let rules = view.rules();
    if id < 1000 && rules.flowcontrol_target(id).is_none() {
        return None;
    }
    Some(filter_from_id(rules, id))
}

/// Square `{x − r, z − r, x + r, z + r}` (32 bits wrapping); a negative half-width gives an empty rectangle (E-65).
fn square(x: i32, z: i32, r: i32) -> [i32; 4] {
    [
        x.wrapping_sub(r),
        z.wrapping_sub(r),
        x.wrapping_add(r),
        z.wrapping_add(r),
    ]
}

/// `0x4E3F` (argc 2): `[2] = GuidCount([2], [3]) > 0`. A filter ID < 1000 is only a map lookup (practically 0). Foreign
/// slot under fog: 0.
fn has_guid(view: &View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    let have = view
        .guid_count(word(req, 2), word(req, 3) as u32)
        .unwrap_or(0);
    put(req, 2, i32::from(have > 0));
}

/// `0x4E47`/`0x4EFD` (argc 3): object count of `[3]` in `[4]` (GUID >= 1000 or filter ID), with `with_pending` for
/// GUIDs plus construction orders (`FUN_004303F0`).
fn owned_count(view: &mut View<'_, '_>, req: &mut [i32], with_pending: bool) {
    if argc(req) != 3 {
        return;
    }
    let n = view.count_guid_or_filter(word(req, 3), word(req, 4), with_pending);
    put(req, 2, n);
}

/// `0x4E48` (buildings, module 3) and `0x4E49` (units, module 0): `argc` must be 1, `[3]` is read. Filter `mode = 4`
/// (`MM_CHECKPROPERTY`), `mode2 = 0`, `f[4]` = module; the object table of the slot is counted without dead and
/// quartered ones (`FUN_00477630`).
fn module_count(view: &mut View<'_, '_>, req: &mut [i32], module: u32) {
    if argc(req) != 1 {
        return;
    }
    let f = MatchFilter {
        mode: 4,
        mode2: 0,
        prop_id: module,
        ..MatchFilter::constructed()
    };
    let n = view.count_in_table(word(req, 3), &f);
    put(req, 2, n);
}

/// `0x4E4B` (argc 6): player `[3]`, filter ID `[4]`, center `([5], [6])`, half-width `[7]` (`FUN_00430320`).
fn centred_rect_count(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 6 {
        return;
    }
    let rect = square(word(req, 5), word(req, 6), word(req, 7));
    let n = filter_of(view, word(req, 4)).map_or(0, |f| {
        count(view.collect_in_rect(rect, &f, word(req, 3)).len())
    });
    put(req, 2, n);
}

/// `0x4F8D` (argc 7): player `[3]`, filter ID `[4]`, corners `[5]…[8]` unchanged (`FUN_00430320`); swapped corners give
/// an empty rectangle.
fn corner_rect_count(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 7 {
        return;
    }
    let rect = [word(req, 5), word(req, 6), word(req, 7), word(req, 8)];
    let n = filter_of(view, word(req, 4)).map_or(0, |f| {
        count(view.collect_in_rect(rect, &f, word(req, 3)).len())
    });
    put(req, 2, n);
}

/// Sum of the rectangle count over the reference players 0-6 (`FUN_00430FF0`): the filter `id` is built once (if it is
/// missing: 0), slot 7 is missing (E-30), an object counts per reference player for which it fits (E-64). The sum
/// wraps.
fn sum_over_players(view: &mut View<'_, '_>, id: i32, rect: [i32; 4]) -> i32 {
    let mut sum = 0i32;
    if let Some(f) = filter_of(view, id) {
        for p in 0..SUM_PLAYERS {
            sum = sum.wrapping_add(count(view.collect_in_rect(rect, &f, p).len()));
        }
    }
    sum
}

/// `0x4F56` (argc 5): filter ID `[3]`, center `([4], [5])`, half-width `[6]`; sum over the reference players 0-6
/// ([`sum_over_players`]).
fn summed_square_count(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 5 {
        return;
    }
    let rect = square(word(req, 4), word(req, 5), word(req, 6));
    let n = sum_over_players(view, word(req, 3), rect);
    put(req, 2, n);
}

/// `0x4F92` (argc 5; `FUN_00430FF0` with the rectangle as a pointer): filter ID `[3]`, corners `{[4], [5], [6], [7]}`
/// directly like `0x4F8D`. It checks `argc == 5` but reads `[7]`, one word behind the arguments (via `ki_query` with
/// five words the padding 0). Unreachable in the original because the handler of builtin `0xB9` sends `argc` 6 (E-37).
fn summed_corner_count(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 5 {
        return;
    }
    let rect = [word(req, 4), word(req, 5), word(req, 6), word(req, 7)];
    let n = sum_over_players(view, word(req, 3), rect);
    put(req, 2, n);
}

/// `0x4E8A` (argc 2): `[2] = Player_CanBuildOrMakeGuid([2], [3])` (5.12.4, silent). Foreign slot under fog: 0.
fn can_build_or_make(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    let ok = view
        .can_build_or_make(word(req, 2), word(req, 3) as u32)
        .unwrap_or(false);
    put(req, 2, i32::from(ok));
}

/// `0x4F6A` (argc 2): `[2] = PendingGuidCount([2], [3])`, the number of orders.
fn pending_orders(view: &View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    let n = view
        .pending_count(word(req, 2), word(req, 3) as u32)
        .unwrap_or(0);
    put(req, 2, n);
}

/// `0x4F33` (argc 3): `[2] = 0`, then 1 if `[3]` is a main house (1000, 1300, 1600, 1900, 2200, 2500) and `[4]` an
/// opponent upgrade 3350...3357, otherwise if the `toolbar.ini` menu of `[3]` has a button with `BUTTONMSG == [4]`
/// (`FUN_0042DD70`; without submenus, kind, player, lock).
fn research_button(view: &View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 3 {
        return;
    }
    let (menu, msg) = (word(req, 3), word(req, 4));
    let fixed = HQ_BY_NATION.contains(&menu) && (3350..=3357).contains(&msg);
    let listed = view
        .rules()
        .toolbar
        .menus
        .get(&menu)
        .is_some_and(|buttons| buttons.iter().any(|b| b.msg == msg));
    put(req, 2, i32::from(fixed || listed));
}

/// `0x510E` (**without** argc check): `[2] = running counter-upgrades with source on slot [3] > 0` (`FUN_00479690`).
fn counter_upgrades_of_slot(view: &View<'_, '_>, req: &mut [i32]) {
    put(req, 2, i32::from(view.counter_upgrades(word(req, 3)) > 0));
}

/// `0x5032` (argc 2): `[2] = ` the counter-upgrade with GUID `[3]` is running somewhere (`FUN_00479640`).
fn counter_upgrade_running(view: &View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    put(req, 2, i32::from(view.counter_running(word(req, 3) as u32)));
}
