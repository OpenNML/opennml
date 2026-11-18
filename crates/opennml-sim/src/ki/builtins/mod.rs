//! Builtin interpreter of the AI (`FUN_004ad380`): dispatcher by the jump table `0x004B2A64`. The VM implements the VM,
//! memory, list, flow and random builtins itself; all others go to
//! [`KiWorld::builtin`](super::world::KiWorld::builtin) (re/ki-vm 12.3 P1).
//! See re/ki-builtins.

pub mod arith;
pub mod flow;
pub mod lists;
pub mod memory;

use super::vm::{Exec, Thread};
use super::world::{BuiltinCx, KiLimit};

/// Highest ID with an entry in the jump table (`CMP ECX,0x2F4` on `id − 1`).
const MAX_ID: u32 = 757;

/// IDs between 1 and 757 without a `case` (target `default`, 255 IDs), measured on the jump table `0x004B2A64`; one
/// closed range per line.
const GAPS: [(u32, u32); 35] = [
    (0x009, 0x009),
    (0x010, 0x013),
    (0x02B, 0x02B),
    (0x02E, 0x02E),
    (0x032, 0x032),
    (0x03B, 0x03C),
    (0x06D, 0x06D),
    (0x077, 0x077),
    (0x090, 0x090),
    (0x0B3, 0x0B3),
    (0x0C1, 0x0C7),
    (0x0CA, 0x0CC),
    (0x0E0, 0x0E0),
    (0x0E5, 0x0E5),
    (0x0E7, 0x0F4),
    (0x125, 0x127),
    (0x166, 0x166),
    (0x16E, 0x16E),
    (0x17B, 0x17B),
    (0x189, 0x18A),
    (0x18D, 0x18D),
    (0x1AC, 0x1AD),
    (0x1B4, 0x1B7),
    (0x1C0, 0x1E4),
    (0x1FD, 0x1FD),
    (0x22E, 0x22F),
    (0x232, 0x234),
    (0x238, 0x239),
    (0x23C, 0x257),
    (0x25A, 0x26B),
    (0x270, 0x27F),
    (0x285, 0x293),
    (0x298, 0x2A7),
    (0x2AD, 0x2BB),
    (0x2BF, 0x2ED),
];

/// `true` if the interpreter has a `case` for `id` (1 to 757 without the gaps).
pub fn has_case(id: u32) -> bool {
    (1..=MAX_ID).contains(&id) && !GAPS.iter().any(|&(lo, hi)| (lo..=hi).contains(&id))
}

/// Submodule of a VM-internal implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    Arith,
    Flow,
    Lists,
    Memory,
}

/// Single source for "local": dispatcher and [`is_local`] both ask here.
fn local(id: u32) -> Option<Local> {
    if arith::handles(id) {
        Some(Local::Arith)
    } else if flow::handles(id) {
        Some(Local::Flow)
    } else if lists::handles(id) {
        Some(Local::Lists)
    } else if memory::handles(id) {
        Some(Local::Memory)
    } else {
        None
    }
}

/// `true` if the VM implements the builtin itself.
pub fn is_local(id: u32) -> bool {
    local(id).is_some()
}

/// `CALLB id`: without a `case` empty the stack (E-4); local in the submodule; otherwise to the world, afterwards a
/// push discarded there as `KiNote::Limit`. `depth` is the depth of the instance below its thread (for `0x19`, re/ki-vm
/// 12.3 P9).
pub fn call(ex: &mut Exec<'_>, t: &mut Thread, id: u32, depth: usize) {
    if !has_case(id) {
        t.stack.clear();
        return;
    }
    match local(id) {
        Some(Local::Arith) => arith::call(ex, t, id),
        Some(Local::Flow) => flow::call(ex, t, id, depth),
        Some(Local::Lists) => lists::call(ex, t, id),
        Some(Local::Memory) => memory::call(ex, t, id),
        None => {
            let overflow = {
                let mut cx = BuiltinCx::with_engines(ex.slot, t, &ex.mem.values, ex.peers);
                ex.world.builtin(id as u16, &mut cx);
                cx.overflowed()
            };
            if overflow {
                ex.limit(t, KiLimit::Stack);
            }
        }
    }
}
