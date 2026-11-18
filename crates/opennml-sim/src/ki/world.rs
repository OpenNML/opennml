//! Interface of the AI VM to the game (re/ki-vm 12.3 P1): builtins without a VM-internal implementation go to
//! [`KiWorld::builtin`]; the handler works through [`BuiltinCx`] on the stack, variables and pause of the executing
//! instance - like the handlers of the original (re/ki-builtins A 1, C 1).

use std::collections::BTreeMap;

use super::vars::KiList;
use super::vm::Thread;
use crate::id::PLAYER_SLOTS;

/// Counterpart of the AI: queries and commands to the game, plus a log.
pub trait KiWorld {
    /// Builtin without a VM-internal implementation; handles stack, variables and pause itself (re/ki-vm 12.3 P1).
    fn builtin(&mut self, id: u16, cx: &mut BuiltinCx<'_>);
    /// Anomaly, log only - no state.
    fn note(&mut self, _note: KiNote) {}
    /// For each executed instruction (statistics); default: nothing.
    fn trace(&mut self, _t: &KiTrace) {}
}

/// Read access to the value tables `m_iValues` of the other engines (builtin `0xE6`): per slot 0-7 the values of an
/// existing engine, `None` without an engine and for the slot of the running engine (its values are read by
/// [`BuiltinCx`] from its own memory). Built by `KiState::tick_engines` and `KiState::with_engine`; [`Peers::none`]
/// sees no foreign engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Peers<'a> {
    values: [Option<&'a BTreeMap<i32, i32>>; PLAYER_SLOTS],
}

impl<'a> Peers<'a> {
    /// No foreign engine.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn new(values: [Option<&'a BTreeMap<i32, i32>>; PLAYER_SLOTS]) -> Self {
        Self { values }
    }

    /// Values of the engine in slot `slot`; `None` outside 0-7 or without an engine.
    pub fn get(&self, slot: i32) -> Option<&'a BTreeMap<i32, i32>> {
        let i = usize::try_from(slot).ok()?;
        self.values.get(i).copied().flatten()
    }
}

/// Own values of an instance without an engine (handler tests, [`BuiltinCx::new`]).
static NO_VALUES: BTreeMap<i32, i32> = BTreeMap::new();

/// Access of a builtin handler to the executing instance.
pub struct BuiltinCx<'a> {
    slot: u8,
    thread: &'a mut Thread,
    overflow: bool,
    /// `m_iValues` of the own engine.
    own: &'a BTreeMap<i32, i32>,
    peers: Peers<'a>,
}

impl<'a> BuiltinCx<'a> {
    /// Without engine memory: own values empty, no foreign engines.
    pub fn new(slot: u8, thread: &'a mut Thread) -> Self {
        Self::with_engines(slot, thread, &NO_VALUES, Peers::none())
    }

    /// With the values of the own engine and read access to the others (`builtins::call`).
    pub fn with_engines(
        slot: u8,
        thread: &'a mut Thread,
        own: &'a BTreeMap<i32, i32>,
        peers: Peers<'a>,
    ) -> Self {
        Self {
            slot,
            thread,
            overflow: false,
            own,
            peers,
        }
    }

    /// `true` if a push at the stack limit was discarded (re/ki-vm 12.3 P6).
    pub fn overflowed(&self) -> bool {
        self.overflow
    }
}

impl BuiltinCx<'_> {
    /// Slot of the engine.
    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// `m_iSpieler` of the instance ("me" in the catalog).
    pub fn me(&self) -> u8 {
        self.thread.player
    }

    pub fn thread_id(&self) -> i32 {
        self.thread.id
    }

    pub fn module(&self) -> u32 {
        self.thread.module
    }

    /// Pop; empty -> 0 (E-3).
    pub fn pop(&mut self) -> i32 {
        self.thread.pop()
    }

    /// Push; at the limit the value is discarded and reported after the call.
    pub fn push(&mut self, v: i32) {
        if !self.thread.push(v) {
            self.overflow = true;
        }
    }

    /// `RemoveAll` of the stack.
    pub fn clear_stack(&mut self) {
        self.thread.stack.clear();
    }

    /// `FUN_00413840`: read a variable without resolving references.
    pub fn get(&self, addr: u32) -> i32 {
        self.thread.vars.get(addr)
    }

    /// `FUN_004137f0`: write a variable without resolving references (output parameter).
    pub fn set(&mut self, addr: u32, v: i32) {
        self.thread.vars.set(addr, v);
    }

    /// `FUN_00413ab0`: list object behind variable `addr`.
    pub fn list(&self, addr: u32) -> Option<&KiList> {
        self.thread.vars.list(addr)
    }

    pub fn list_mut(&mut self, addr: u32) -> Option<&mut KiList> {
        self.thread.vars.list_mut(addr)
    }

    /// List object behind an already read list pointer (`FUN_00413840` returns it without reference resolution,
    /// re/ki-vm 12.3 P2); `None` if there is no list there.
    pub fn list_at_mut(&mut self, pointer: i32) -> Option<&mut KiList> {
        self.thread.vars.list_at_mut(pointer)
    }

    /// `FUN_004b4180`: `m_iPauseCounter = ms`.
    pub fn set_pause(&mut self, ms: i32) {
        self.thread.pause_ms = ms;
    }

    /// `KIManager vf[6]` (`0x004A4B70`, re/ki-queries 5.13): `m_iValues[key]` of the engine in slot `slot` (missing ->
    /// 0 like builtin `0x1F`); the own slot reads the own memory. `None`: no engine in the slot or slot outside 0-7.
    pub fn engine_value(&self, slot: i32, key: i32) -> Option<i32> {
        let values = if slot == i32::from(self.slot) {
            self.own
        } else {
            self.peers.get(slot)?
        };
        Some(values.get(&key).copied().unwrap_or(0))
    }
}

/// Kind of a limit (re/ki-vm 12.3 P6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiLimit {
    Stack,
    Threads,
    CallDepth,
    SyncSteps,
}

/// Run-time anomaly; the original reports nothing, the reimplementation logs it.
///
/// `thread` is the thread ID of the executing instance; for starts from outside (`Engine::spawn`, `Engine::run_sync`),
/// where no instance is running yet, it is -1 and `module` the target module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiNote {
    /// A limit was reached (re/ki-vm 12.3 P6).
    Limit {
        slot: u8,
        thread: i32,
        module: u32,
        limit: KiLimit,
    },
    /// `CALLM` (E-6) or `SPAWN` to a module without a prototype.
    MissingModule {
        slot: u8,
        thread: i32,
        module: u32,
        target: u32,
        spawn: bool,
    },
    /// A crash case of the original was caught (re/ki-vm 12.3 P7).
    Guarded {
        slot: u8,
        thread: i32,
        module: u32,
        builtin: u16,
        what: &'static str,
    },
}

/// An executed instruction: word at program counter `pc` before the step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KiTrace {
    pub slot: u8,
    pub thread: i32,
    pub depth: usize,
    pub module: u32,
    pub pc: u32,
    pub op: u32,
}
