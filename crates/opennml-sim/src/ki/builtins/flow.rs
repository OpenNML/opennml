//! Flow builtins `0x19`, `0x1A`, `0x2F`-`0x31`, `0x8E`, `0x18C` and the AI randomness `0x8D` (re/ki-builtins A 2, C).
//! Changes to the thread table and to the AI state go to the engine as effects (re/ki-vm 12.3 P8).

use crate::ki::vm::{Effect, Exec, Thread};
use crate::rng::CrtRand;

/// `true` for the flow IDs of this module.
pub fn handles(id: u32) -> bool {
    matches!(id, 0x19 | 0x1A | 0x2F..=0x31 | 0x8D | 0x8E | 0x18C)
}

/// Flow builtin `id` of the instance `t` at depth `depth` below its thread.
pub fn call(ex: &mut Exec<'_>, t: &mut Thread, id: u32, depth: usize) {
    match id {
        // `FUN_004ac650(instance)` (re/ki-vm 12.3 P9, E-44): `stop` for all threads of the table with a child except
        // the instance itself and the thread whose direct child it is. That is the own thread up to depth 1 - from a
        // grandchild it hits that one too, and in the synchronous run (topmost instance not in the table) every thread
        // with a child.
        0x19 => {
            let spare = (depth <= 1 && ex.top_in_table).then_some(t.id);
            ex.effects.push(Effect::StopOthersWithChild { spare });
        }
        // `FUN_004ac5f0`: free all threads.
        0x1A => ex.effects.push(Effect::ReleaseAll),
        // `m_bSemaphoren[k] = 1` (occupy without waiting).
        0x2F => {
            let k = t.pop();
            ex.mem.semaphores.insert(k, 1);
        }
        // `m_bSemaphoren[k] = 0`, only for an existing key.
        0x30 => {
            let k = t.pop();
            if let Some(v) = ex.mem.semaphores.get_mut(&k) {
                *v = 0;
            }
        }
        // Value of the semaphore, missing -> 0 without creating it.
        0x31 => {
            let k = t.pop();
            let v = ex.mem.semaphores.get(&k).copied().unwrap_or(0);
            ex.push(t, i32::from(v));
        }
        // (lo, hi) -> random number (re/ki-vm 12.3 P5, `0x004A7FAD` to `0x004A8006`).
        0x8D => {
            let hi = t.pop();
            let lo = t.pop();
            let v = random(ex.rand, lo, hi);
            ex.push(t, v);
        }
        // `FUN_004b4180`: `m_iPauseCounter = ms`; the fourth handler argument is dead.
        0x8E => t.pause_ms = t.pop(),
        // Pause AI (re/ki-vm 12.3 P12): effective from the next AI tick.
        0x18C => ex.effects.push(Effect::PauseKi),
        // Reachable only through `handles`.
        _ => {}
    }
}

/// A draw: `rand() · (hi − lo) / 0x7FFF + lo` in 32 bits with overflow (`IMUL`, the reciprocal `0x80010003` is the
/// truncating division by `0x7FFF`).
fn draw(rand: &mut CrtRand, lo: i32, d: i32) -> i32 {
    (rand.rand().wrapping_mul(d) / 0x7FFF).wrapping_add(lo)
}

/// `0x8D` per re/ki-vm 12.3 P5: the first draw only serves the check `lo > v` (then `lo` applies), otherwise the second
/// one applies (re/ki-builtins A 3 no. 6). Both draws consume the generator.
fn random(rand: &mut CrtRand, lo: i32, hi: i32) -> i32 {
    let d = hi.wrapping_sub(lo);
    if lo > draw(rand, lo, d) {
        lo
    } else {
        draw(rand, lo, d)
    }
}
