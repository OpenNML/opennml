//! List queries (re/ki-queries 5.8 and 5.15.1): the builtins `0x112`/`0x117` (distance), `0x113`/`0x11E` (all match),
//! `0x114`/`0x11F` (pair with attack check) and `0x115`/`0x121` (one matches).
//!
//! Common flow ([`walk`]): the pointer of the list variable (`FUN_00413840`, without reference resolution) is 0 -> push
//! **nothing**; `n` not in (0, 100) -> push 0 (the order in the original, before any list access); if the pointer then
//! points to no list, that would be an access into the void in the original: `KiNote::Guarded` and push nothing (like
//! `lists.rs`). Otherwise every ID is **not read until its own pass** with creation (`FUN_004E7B30`, E-32): after an
//! abort later keys stay uncreated. All objects share **one** word field, `[2]` (the player) is set to `me` only before
//! the loop and afterwards carried by the result of the previous query (E-28, E-56): `0x4E28` overwrites it with 0/1
//! and leaves it unchanged for a missing object (ID 0, removed).

use super::codes_object::is_attacking;
use super::handlers::list_ids;
use super::{SimKiWorld, dispatch};
use crate::ki::world::{BuiltinCx, KiNote};
use crate::player::View;

/// Filter ID `0xB3` ("status MOVE and module PROP_PTTOPT"): the only one whose failure can abort the loop in
/// `0x114`/`0x11F`.
const MOVE_FILTER: i32 = 0xB3;

/// Make a query `req`. The codes of the lists (`0x4E28`, `0x4E2C`, `0x4FB5`) produce no direct changes.
fn ask(view: &mut View<'_, '_>, req: &mut [i32]) {
    let mut direct = Vec::new();
    dispatch(view, req, &mut direct);
    debug_assert!(direct.is_empty(), "list queries change nothing");
}

/// Prologue and loop of all list queries. `args` are `(list, start, n, …)`. Calls `step` with the view of slot `me` and
/// the object ID of the pass, until `step` reports `false` or `n` passes are over. `false` if no result is to be pushed
/// (pointer 0, no list) or 0 was already pushed (`n` outside 1...99); `true` if the loop ran.
fn walk(
    id: u16,
    args: &[i32],
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
    mut step: impl FnMut(&mut View<'_, '_>, i32) -> bool,
) -> bool {
    let (list, start, n) = (args[0] as u32, args[1], args[2]);
    let pointer = cx.get(list);
    if pointer == 0 {
        return false;
    }
    if !(0 < n && n < 100) {
        cx.push(0);
        return false;
    }
    if cx.list_at_mut(pointer).is_none() {
        w.notes.push(KiNote::Guarded {
            slot: cx.slot(),
            thread: cx.thread_id(),
            module: cx.module(),
            builtin: id,
            what: "list pointer without a list",
        });
        return false;
    }
    let mut view = View::new(w.sim, cx.me(), w.sight);
    for i in 0..n {
        let object = list_ids(cx, list, start.wrapping_add(i), 1)
            .and_then(|ids| ids.first().copied())
            .expect("list checked above; the loop changes no variable");
        if !step(&mut view, object) {
            break;
        }
    }
    true
}

/// Argument `i` in source order, 0 if missing.
fn arg(args: &[i32], i: usize) -> i32 {
    args.get(i).copied().unwrap_or(0)
}

/// `0x112`/`0x117` (`0x004A2ED0`, `(list, start, n, x, z, R)`): 1 if every found unit is at most `R` tiles away from
/// `(x, z)`, otherwise 0. The query is `0x4E2C`; a missing ID counts as inside (the query then reports `[2] = 0`).
///
/// The original computes `trunc(sqrt(dx² + dz²)) > R` with `FILD`/`FSQRT`/`ftol`; here integer `isqrt` of the sum,
/// which gives exactly the same for distances below 2048 tiles (5.15.1). The squares and the sum overflow like the
/// `IMUL` of the original (wrapping); a **negative** sum resulting from this gives NaN in the x87, `ftol` of it
/// `i32::MIN`, and the unit never counts as too far. `R < 0`: every found unit is too far.
pub(super) fn distance(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (x0, z0, radius) = (arg(args, 3), arg(args, 4), arg(args, 5));
    let mut req = [0x4E2C, 4, i32::from(cx.me()), 0, 0, 0];
    let mut inside = true;
    let ran = walk(id, args, cx, w, |view, object| {
        req[3] = object;
        ask(view, &mut req);
        if req[2] != 0 {
            let dx = req[3].wrapping_sub(x0);
            let dz = req[4].wrapping_sub(z0);
            let sum = dz.wrapping_mul(dz).wrapping_add(dx.wrapping_mul(dx));
            if sum >= 0 && sum.isqrt() > radius {
                inside = false;
                return false;
            }
        }
        true
    });
    if ran {
        cx.push(i32::from(inside));
    }
}

/// `0x113`/`0x11E` (`0x004A3020`, `(list, start, n, filter)`): 1 if all objects pass the filter; the first with result
/// 0 aborts with 0. After the first passed object `[2] = 1`, the further ones are checked for player 1 (E-28).
pub(super) fn all_match(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let mut req = [0x4E28, 3, i32::from(cx.me()), 0, arg(args, 3)];
    let mut all = true;
    let ran = walk(id, args, cx, w, |view, object| {
        req[3] = object;
        ask(view, &mut req);
        all = req[2] != 0;
        all
    });
    if ran {
        cx.push(i32::from(all));
    }
}

/// `0x115`/`0x121` (`0x004A3D80`, `(list, start, n, filter)`): 1 if one of the objects passes the filter, otherwise 0.
/// After a failure `[2] = 0`, the further ones are checked for player 0; if the first object is already missing,
/// `[2] = me` stays and `me ≠ 0` immediately gives 1 (E-28, E-56).
pub(super) fn any_match(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let mut req = [0x4E28, 3, i32::from(cx.me()), 0, arg(args, 3)];
    let mut any = false;
    let ran = walk(id, args, cx, w, |view, object| {
        req[3] = object;
        ask(view, &mut req);
        any = req[2] != 0;
        !any
    });
    if ran {
        cx.push(i32::from(any));
    }
}

/// `0x114`/`0x11F` (`0x004A3BE0`, script `f(list, start, n, B, A)`): A is the **last** script argument (first pop) and
/// is asked first. Per object filter A, then B with the result of A as player (and the next A with that of B, E-28).
/// Abort with 0 only if one of the filters `0xB3` fails, the other passes and the object attacks (`0x4FB5`, E-56),
/// otherwise the result is 1. The attack question is asked only if A or B is `0xB3` (otherwise the original never reads
/// it), and before the filters.
pub(super) fn pair(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (b, a) = (arg(args, 3), arg(args, 4));
    let me = i32::from(cx.me());
    let mut req = [0x4E28, 3, me, 0, a];
    let mut result = 1;
    let ran = walk(id, args, cx, w, |view, object| {
        let attacking = if a == MOVE_FILTER || b == MOVE_FILTER {
            is_attacking(view, me, object)
        } else {
            0
        };
        req[3] = object;
        req[4] = a;
        ask(view, &mut req);
        let passed_a = req[2];
        req[4] = b;
        ask(view, &mut req);
        let passed_b = req[2];
        let one_failed = !(passed_a != 0 && passed_b != 0);
        let move_failed =
            (b == MOVE_FILTER && passed_a != 0) || (a == MOVE_FILTER && passed_b != 0);
        if one_failed && move_failed && attacking != 0 {
            result = 0;
            return false;
        }
        true
    });
    if ran {
        cx.push(result);
    }
}
