//! Stand-in game of the AI (re/ki-vm 12.3 P1): answers every world builtin according to its [`Signature`] with a fixed
//! answer, without a game world. Serves the test vectors from the corpus and the dry run of all modules.
//!
//! The real counterpart [`super::query::SimKiWorld`] does not use the answer by signature: every world ID there is a
//! query or a command with its own row. Only the stand-in game uses [`answer_by_signature`] (without a game world,
//! re/ki-vm 12.3 P1).

use std::collections::BTreeMap;

use super::signature::signature;
use super::world::{BuiltinCx, KiNote, KiWorld};

/// A logged call: slot, thread, ID and arguments in source order `a1 … aN` (output parameters as variable address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubCall {
    pub slot: u8,
    pub thread: i32,
    pub id: u16,
    pub args: Vec<i32>,
}

/// World without a game: fixed answers, call counters, log.
#[derive(Debug, Clone, Default)]
pub struct StubWorld {
    /// Fixed answer per ID, otherwise 0; goes into the result and into all output variables.
    pub answers: BTreeMap<u16, i32>,
    /// `true`: log calls with a signature in `calls`.
    pub record: bool,
    pub calls: Vec<StubCall>,
    /// Calls per ID, also without a signature and without `record`.
    pub counts: BTreeMap<u16, u64>,
    pub notes: Vec<KiNote>,
}

/// Answer according to the [`Signature`](super::signature::Signature) of `id`: pop the arguments, `answer` into all
/// output variables (without reference resolution like `FUN_004137f0`), push `answer`, set the pause; returns the call.
/// Without a signature (world ID unused in the corpus) empty the stack like the `default` of the interpreter and
/// `None`; the caller writes the note ([`missing_signature`]).
pub fn answer_by_signature(id: u16, cx: &mut BuiltinCx<'_>, answer: i32) -> Option<StubCall> {
    let Some(sig) = signature(id) else {
        cx.clear_stack();
        return None;
    };
    let mut args: Vec<i32> = (0..sig.pops).map(|_| cx.pop()).collect();
    args.reverse();
    for &o in sig.outs {
        cx.set(args[usize::from(o)] as u32, answer);
    }
    if sig.push {
        cx.push(answer);
    }
    if sig.pause_ms != 0 {
        cx.set_pause(sig.pause_ms);
    }
    Some(StubCall {
        slot: cx.slot(),
        thread: cx.thread_id(),
        id,
        args,
    })
}

/// `KiNote::Guarded` "without signature" for a world builtin without [`answer_by_signature`] (only [`StubWorld`]).
pub fn missing_signature(id: u16, cx: &BuiltinCx<'_>) -> KiNote {
    KiNote::Guarded {
        slot: cx.slot(),
        thread: cx.thread_id(),
        module: cx.module(),
        builtin: id,
        what: "without signature",
    }
}

impl KiWorld for StubWorld {
    /// According to the signature with the fixed answer ([`answer_by_signature`]); without a signature report
    /// `KiNote::Guarded`.
    fn builtin(&mut self, id: u16, cx: &mut BuiltinCx<'_>) {
        *self.counts.entry(id).or_default() += 1;
        let answer = self.answers.get(&id).copied().unwrap_or(0);
        match answer_by_signature(id, cx, answer) {
            Some(call) if self.record => self.calls.push(call),
            Some(_) => {}
            None => self.notes.push(missing_signature(id, cx)),
        }
    }

    fn note(&mut self, note: KiNote) {
        self.notes.push(note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ki::testkit::{int, proto};
    use crate::ki::vm::Thread;

    fn thread() -> Thread {
        Thread::from_prototype(&proto(7, &[(0, 4)], &[], &[]), 2, 1, Vec::new())
    }

    // QUIRK E-36: kept
    #[test]
    fn signature_pops_writes_outputs_pushes_and_pauses() {
        let mut world = StubWorld {
            answers: BTreeMap::from([(0x12C, 9)]),
            record: true,
            ..Default::default()
        };
        let mut t = thread();
        t.stack = vec![-5, 10, 11, 12, 13, int(1) as i32, int(2) as i32, 14];
        world.builtin(0x12C, &mut BuiltinCx::new(4, &mut t));
        assert_eq!(t.stack, [-5, 9]);
        assert_eq!((t.vars.get(int(1)), t.vars.get(int(2))), (9, 9));
        assert_eq!(t.pause_ms, 500);
        assert_eq!(
            world.calls,
            [StubCall {
                slot: 4,
                thread: 2,
                id: 0x12C,
                args: vec![10, 11, 12, 13, 1, 2, 14],
            }]
        );
        assert_eq!(world.counts, BTreeMap::from([(0x12C, 1)]));
        assert!(world.notes.is_empty());
    }

    #[test]
    fn missing_signature_clears_the_stack_and_notes_it() {
        let mut world = StubWorld {
            record: true,
            ..Default::default()
        };
        let mut t = thread();
        t.stack = vec![1, 2, 3];
        world.builtin(0x48, &mut BuiltinCx::new(4, &mut t));
        assert!(t.stack.is_empty());
        assert!(world.calls.is_empty());
        assert_eq!(world.counts, BTreeMap::from([(0x48, 1)]));
        assert_eq!(
            world.notes,
            [KiNote::Guarded {
                slot: 4,
                thread: 2,
                module: 7,
                builtin: 0x48,
                what: "without signature",
            }]
        );
    }
}
