//! Builtins for list objects (re/ki-builtins A 2 `0x33`-`0x44`, B 3 `0xDC`-`0xDF`, `0xE1`-`0xE4`, A 3 no. 8;
//! decompilation `FUN_004ad380` from `0x004AE0EC`, `FUN_004a1540`, `FUN_004a1580`).
//!
//! Pop order as in the original: the first pop is the last pushed argument. The list variable `L` is read with
//! reference resolution for `0x33`-`0x44` (`FUN_00413ab0`), without it for `0xDC`-`0xE4` (`FUN_00413840`). A pointer 0
//! counts as "no list": the original then does nothing and **pushes nothing either** (`JZ 0x004B2A57`, stack imbalance
//! E-34, preserved). A pointer other than 0 without a list behind it would be a memory access to nonsense there; the
//! reimplementation reports `KiNote::Guarded` and pushes 0 where the normal path pushes (re/ki-vm 12.3 P7).

use super::arith::guarded;
use crate::ki::vars::{self, KiList};
use crate::ki::vm::{Exec, Thread};

/// `true` for the list IDs of this module: `0x33`-`0x44` without the gaps `0x3B`, `0x3C` as well as `0xDC`-`0xDF` and
/// `0xE1`-`0xE4`.
pub fn handles(id: u32) -> bool {
    matches!(id, 0x33..=0x3A | 0x3D..=0x44 | 0xDC..=0xDF | 0xE1..=0xE4)
}

/// Kind of access; the four groups `0x33`-`0x36`, `0x37`-`0x3A`, `0x3D`-`0x40`, `0x41`-`0x44` are repetitions of the
/// same type (A 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `(L, k) → v`: read `L[k]`, creates a missing key with 0.
    Read,
    /// `(L, k, v)`: `L[k] = v`.
    Write,
    /// `(L1, L2)`: write all entries of `L1` to `L2` (merging).
    Copy,
    /// `(L, k, &v)`: `L[k] = get(&v)`.
    WriteVar,
    /// `(L, i)`: `remove_index`.
    Remove,
    /// `(L, i, v)`: `insert_index`.
    Insert,
}

fn op_of(id: u32) -> Option<Op> {
    Some(match id {
        0x33 | 0x37 | 0x3D | 0x41 => Op::Read,
        0x34 | 0x38 | 0x3E | 0x42 => Op::Write,
        0x35 | 0x39 | 0x3F | 0x43 => Op::Copy,
        0x36 | 0x3A | 0x40 | 0x44 => Op::WriteVar,
        0xDC..=0xDF => Op::Remove,
        0xE1..=0xE4 => Op::Insert,
        _ => return None,
    })
}

/// List builtin `id` on the stack of `t`.
pub fn call(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let Some(op) = op_of(id) else { return };
    match op {
        Op::Read => {
            let k = t.pop();
            let pointer = resolve_pop(t);
            if pointer == 0 {
                return;
            }
            let v = match t.vars.list_at_mut(pointer) {
                Some(list) => *list.entry_or_default(k),
                None => {
                    guarded(ex, t, id, "read attempt on no list");
                    0
                }
            };
            ex.push(t, v);
        }
        Op::Write => {
            let v = t.pop();
            let k = t.pop();
            let pointer = resolve_pop(t);
            write(ex, t, id, pointer, k, v);
        }
        Op::WriteVar => {
            let var = t.pop();
            let k = t.pop();
            let pointer = resolve_pop(t);
            let v = t.vars.get(var as u32);
            write(ex, t, id, pointer, k, v);
        }
        Op::Copy => {
            let dst = resolve_pop(t);
            let src = resolve_pop(t);
            if src == 0 {
                return;
            }
            let Some(source) = t.vars.list_at(src).cloned() else {
                guarded(ex, t, id, "copy source is not a list");
                return;
            };
            match t.vars.list_at_mut(dst) {
                Some(target) => vars::copy_list(&source, target),
                // Write access to address 4 only for a non-empty source (re/ki-builtins A 3 no. 8).
                None if source.is_empty() => {}
                None => guarded(ex, t, id, "copy target is not a list"),
            }
        }
        Op::Remove => {
            let i = t.pop();
            let pointer = get_pop(t);
            edit(ex, t, id, pointer, |list| vars::remove_index(list, i));
        }
        Op::Insert => {
            let v = t.pop();
            let i = t.pop();
            let pointer = get_pop(t);
            edit(ex, t, id, pointer, |list| vars::insert_index(list, v, i));
        }
    }
}

/// Reads the next pop as a list variable with reference resolution (`FUN_00413ab0`).
fn resolve_pop(t: &mut Thread) -> i32 {
    let addr = t.pop() as u32;
    t.vars.resolve(addr)
}

/// Reads the next pop as a list variable without reference resolution (`FUN_00413840`).
fn get_pop(t: &mut Thread) -> i32 {
    let addr = t.pop() as u32;
    t.vars.get(addr)
}

/// `L[k] = v` for `0x34`/`0x36`: pointer 0 -> nothing, pointer without a list -> crash case.
fn write(ex: &mut Exec<'_>, t: &mut Thread, id: u32, pointer: i32, k: i32, v: i32) {
    if pointer == 0 {
        return;
    }
    match t.vars.list_at_mut(pointer) {
        Some(list) => list.insert(k, v),
        None => guarded(ex, t, id, "write attempt on no list"),
    }
}

/// `0xDC`-`0xE4`: pointer 0 -> nothing (`TEST EAX,EAX; JZ`), pointer without a list -> crash case.
fn edit(ex: &mut Exec<'_>, t: &mut Thread, id: u32, pointer: i32, f: impl FnOnce(&mut KiList)) {
    if pointer == 0 {
        return;
    }
    match t.vars.list_at_mut(pointer) {
        Some(list) => f(list),
        None => guarded(ex, t, id, "list access on no list"),
    }
}
