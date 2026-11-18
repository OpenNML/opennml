//! Test helpers of the AI tests: code words, addresses and prototypes without game files.

use super::catalog::{KiCatalog, KiPrototype};
use super::vars::VarTable;
use super::world::{BuiltinCx, KiNote, KiWorld};
use crate::rules::Fnv64;

/// Code word from opcode (bits 28-31) and operand (bits 0-27).
pub const fn word(op: u32, operand: u32) -> u32 {
    (op << 28) | operand
}

/// Address of an integer variable: kind 0, type 0, index `i`.
pub fn int(i: u16) -> u32 {
    u32::from(i)
}

/// Address of list variable `i`: kind 2, type 0.
pub const fn list(i: u32) -> u32 {
    (2 << 24) | i
}

/// Prototype `id` with the name "test{id}" without parameters; the checksum runs over the code words.
pub fn proto(id: u32, ranges: &[(u32, u32)], inits: &[(u32, i32)], code: &[u32]) -> KiPrototype {
    let mut hash = Fnv64::default();
    for w in code {
        hash.write(&w.to_le_bytes());
    }
    KiPrototype {
        id,
        name: format!("test{id}"),
        params: 0,
        code: code.to_vec(),
        vars: VarTable::from_module(ranges, inits),
        checksum: hash.0,
    }
}

pub fn catalog(protos: Vec<KiPrototype>) -> KiCatalog {
    KiCatalog::from_prototypes(protos)
}

/// World of the VM tests: every builtin without a VM implementation pops `pops` values, logs
/// `(slot, thread, ID, arguments in source order)` and pushes `answer`; anomalies end up in `notes`.
#[derive(Debug, Default)]
pub struct TestWorld {
    pub calls: Vec<(u8, i32, u16, Vec<i32>)>,
    pub notes: Vec<KiNote>,
    pub answer: i32,
    pub pops: usize,
}

impl KiWorld for TestWorld {
    fn builtin(&mut self, id: u16, cx: &mut BuiltinCx<'_>) {
        let mut args: Vec<i32> = (0..self.pops).map(|_| cx.pop()).collect();
        args.reverse();
        self.calls.push((cx.slot(), cx.thread_id(), id, args));
        cx.push(self.answer);
    }

    fn note(&mut self, note: KiNote) {
        self.notes.push(note);
    }
}
