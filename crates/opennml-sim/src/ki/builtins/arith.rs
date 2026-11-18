//! Arithmetic builtins `0x01`-`0x08`, `0x0A`-`0x0F`, `0x14`-`0x16` and the aliases `0x118`-`0x11D` as well as the
//! remaining VM builtins `0x17`, `0x82`, `0x89`, `0x91`, `0xB6`, `0xBE` and `0xD1` (re/ki-builtins A 2). Pop order
//! `b = pop, a = pop`; 32-bit overflow wraps. The crash cases of the original (E-5) produce fixed results with
//! `KiNote::Guarded` (re/ki-vm 12.3 P7).

use crate::ki::vm::{Exec, Thread};
use crate::ki::world::KiNote;

/// `true` for the arithmetic IDs of this module.
pub fn handles(id: u32) -> bool {
    matches!(
        id,
        0x01..=0x08
            | 0x0A..=0x0F
            | 0x14..=0x17
            | 0x82
            | 0x89
            | 0x91
            | 0xB6
            | 0xBE
            | 0xD1
            | 0x118..=0x11D
    )
}

/// Arithmetic builtin `id` on the stack of `t`.
pub fn call(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    if misc(ex, t, id) {
        return;
    }
    if id == 0x16 {
        let a = t.pop();
        ex.push(t, i32::from(a == 0));
        return;
    }
    let b = t.pop();
    let a = t.pop();
    let v = match id {
        0x01 => a.wrapping_add(b),
        0x02 => a.wrapping_sub(b),
        0x03 => a.wrapping_mul(b),
        // Divisor 0 calculates with 1 like the original; `INT_MIN / −1` crashes there.
        0x04 => {
            let b = if b == 0 { 1 } else { b };
            if a == i32::MIN && b == -1 {
                guarded(ex, t, id, "INT_MIN / -1");
            }
            a.wrapping_div(b)
        }
        0x05 => a & b,
        0x06 => a | b,
        0x07 => a ^ b,
        // Unprotected in the original: `% 0` and `INT_MIN % −1` crash.
        0x08 => {
            if b == 0 {
                guarded(ex, t, id, "modulo by 0");
                0
            } else {
                if a == i32::MIN && b == -1 {
                    guarded(ex, t, id, "INT_MIN % -1");
                }
                a.wrapping_rem(b)
            }
        }
        0x0A => i32::from(a > b),
        0x0B => i32::from(a < b),
        0x0C | 0x118..=0x11A => i32::from(a == b),
        0x0D | 0x11B..=0x11D => i32::from(a != b),
        0x0E => i32::from(a >= b),
        0x0F => i32::from(a <= b),
        // Both pops are always evaluated (no short circuit).
        0x14 => i32::from(a != 0 && b != 0),
        0x15 => i32::from(a != 0 || b != 0),
        // Reachable only through `handles`.
        _ => return,
    };
    ex.push(t, v);
}

/// Types for which `0xD1` returns 1: three hero types per nation (`0x004AF328` to `0x004AF39F`).
fn is_hero_type(typ: i32) -> bool {
    matches!(
        typ,
        0x459..=0x45B | 0x582..=0x584 | 0x6B2..=0x6B4 | 0x7DD..=0x7DF | 0x909..=0x90B | 0xA36..=0xA38
    )
}

/// `0x17`, `0x82`, `0x89`, `0x91`, `0xB6`, `0xBE`, `0xD1`: `true` if `id` is handled here.
fn misc(ex: &mut Exec<'_>, t: &mut Thread, id: u32) -> bool {
    let v = match id {
        // (adr, x): `x` is discarded, the value of the variable `adr` with reference resolution.
        0x17 => {
            t.pop();
            let adr = t.pop();
            t.vars.resolve(adr as u32)
        }
        // () -> own player slot (`m_iSpieler`, `FUN_004a0260`), without a query.
        0x82 => i32::from(t.player),
        // (x1, z1, x2, z2) -> distance (`FUN_0049e250`, re/ki-vm 12.3 P11): sum of squares wrapped to 32 bits,
        // negative -> `FSQRT` of NaN, `_ftol` returns `i32::MIN`; otherwise truncated root.
        0x89 => {
            let (z2, x2, z1, x1) = (t.pop(), t.pop(), t.pop(), t.pop());
            let dz = z2.wrapping_sub(z1);
            let dx = x2.wrapping_sub(x1);
            let sum = dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz));
            match u32::try_from(sum) {
                Ok(sum) => sum.isqrt() as i32,
                Err(_) => i32::MIN,
            }
        }
        // (type) -> 1 in the type range of the query `0x4E3D` (re/ki-vm 12.3 P15).
        0x91 => i32::from(in_type_range(t.pop())),
        // (type) -> 1 for `0xD48 ≤ type ≤ 0xD57`.
        0xB6 => i32::from((0xD48..=0xD57).contains(&t.pop())),
        // (l, t, r, b, x, z) -> `PtInRect`: right and bottom edge excluded.
        0xBE => {
            let z = t.pop();
            let x = t.pop();
            let bottom = t.pop();
            let right = t.pop();
            let top = t.pop();
            let left = t.pop();
            i32::from(left <= x && x < right && top <= z && z < bottom)
        }
        // (type) -> 1 for the hero types (three per nation).
        0xD1 => i32::from(is_hero_type(t.pop())),
        _ => return false,
    };
    ex.push(t, v);
    true
}

/// Type range of the query `0x4E3D` (`FUN_004a0590`): `0xAF0 ≤ type < 0xD48`; used by `0x1C` and `0x91` (re/ki-vm 12.3
/// P15).
pub(crate) fn in_type_range(typ: i32) -> bool {
    (0xAF0..0xD48).contains(&typ)
}

/// `KiNote::Guarded` for a caught crash case of the original (re/ki-vm 12.3 P7).
pub(crate) fn guarded(ex: &mut Exec<'_>, t: &Thread, id: u32, what: &'static str) {
    ex.world.note(KiNote::Guarded {
        slot: ex.slot,
        thread: t.id,
        module: t.module,
        builtin: id as u16,
        what,
    });
}
