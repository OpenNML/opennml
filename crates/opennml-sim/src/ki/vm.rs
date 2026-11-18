//! The stack machine of the AI modules: instance (thread), instruction set (`FUN_004b4e10`) and step (`FUN_004b58c0`),
//! `CALLM`/`SPAWN`, error cases E-1 to E-7 and limits (re/ki-vm 12.3 P6).
//! See re/ki-vm 3, 4, 6.

use serde::{Deserialize, Serialize};

use super::catalog::{KiCatalog, KiPrototype};
use super::engine::EngineMem;
use super::vars::VarTable;
use super::world::{KiLimit, KiNote, KiTrace, KiWorld, Peers};
use super::{MAX_CALL_DEPTH, MAX_STACK, MAX_THREADS, builtins};
use crate::rng::CrtRand;

/// Opcodes (bits 28-31 of a code word); 10-15 do nothing.
pub mod op {
    pub const LOAD: u32 = 0;
    pub const STORE: u32 = 1;
    pub const PUSHA: u32 = 2;
    pub const STORE3: u32 = 3;
    pub const CALLB: u32 = 4;
    pub const CALLM: u32 = 5;
    pub const SPAWN: u32 = 6;
    pub const JMP: u32 = 7;
    pub const JZ: u32 = 8;
    pub const JNZ: u32 = 9;
}

/// Bits of `m_lStatus`.
pub mod status {
    /// Running.
    pub const RUNNING: u32 = 1;
    /// Done (end of code).
    pub const DONE: u32 = 4;
    /// Waits for `m_pChild` (running `CALLM`).
    pub const CHILD: u32 = 0x10;
}

/// Operand of a code word (bits 0-27).
const OPERAND_MASK: u32 = 0x0FFF_FFFF;
/// Target "discard" of `STORE`/`STORE3` (`STORE drop`).
const DROP: u32 = 0x0FFF_FFFF;
/// Word behind the end of code (`FUN_0040e8f0`): opcode 15, nothing.
const END_WORD: u32 = 0xFFFF_FFFF;

/// Instance of a module (0x6BC bytes), fields as in `KIModule_Serialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// `m_iPositionID` (= thread ID; a child carries the one of its caller).
    pub id: i32,
    /// `m_iSpieler`.
    pub player: u8,
    /// Module ID; the code is in the prototype of the catalog.
    pub module: u32,
    /// Code length and checksum of the module (re/ki-vm 12.3 P14). Loading does not check them: catalog and code are
    /// part of the rules checksum.
    pub code_len: u32,
    pub checksum: u64,
    /// `m_iCodePt`.
    pub pc: u32,
    /// `m_ModuleData`.
    pub vars: VarTable,
    /// `m_pStack`; the end is at the top.
    pub stack: Vec<i32>,
    /// `m_lStatus`.
    pub status: u32,
    /// `m_bStop`.
    pub stop: bool,
    /// `m_iLastAction`: opcode of the previous instruction, -1 at the start.
    pub last_op: i32,
    /// `m_iPauseCounter` (ms).
    pub pause_ms: i32,
    /// `m_iWaitingForSemaphore`, -1 = none.
    pub wait_semaphore: i32,
    /// `m_pChild` (running `CALLM`).
    #[serde(deserialize_with = "deserialize_child")]
    pub child: Option<Box<Thread>>,
}

/// Maximum nesting of the field `child` when reading a savegame: far above `MAX_CALL_DEPTH` (the limit of `CALLM` at
/// run time; a deeper chain that is read runs, `step` recurses at most that deep), but small enough that a corrupted
/// file with an arbitrarily deep chain does not blow the stack. It counts while reading the field (also an empty one),
/// so at most `MAX_CHILD_NESTING − 1` = 63 children can be read in a chain.
const MAX_CHILD_NESTING: usize = 2 * MAX_CALL_DEPTH;

std::thread_local! {
    /// Current nesting of `child` while reading (per thread of the program).
    static CHILD_NESTING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reads `child` with a depth counter; above [`MAX_CHILD_NESTING`] an error instead of a stack overflow.
fn deserialize_child<'de, D>(d: D) -> Result<Option<Box<Thread>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Level;
    impl Drop for Level {
        fn drop(&mut self) {
            CHILD_NESTING.with(|n| n.set(n.get() - 1));
        }
    }
    let depth = CHILD_NESTING.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let _level = Level;
    if depth > MAX_CHILD_NESTING {
        return Err(serde::de::Error::custom(format!(
            "AI thread with more than {} nested children",
            MAX_CHILD_NESTING - 1
        )));
    }
    Option::<Box<Thread>>::deserialize(d)
}

impl Thread {
    /// `FUN_004b49c0`: clone of the prototype with code, variables (including `CONSTDAT`) and the stack `stack`;
    /// `m_lStatus = 1`, pause 0, no semaphore.
    pub fn from_prototype(p: &KiPrototype, id: i32, player: u8, stack: Vec<i32>) -> Thread {
        Thread {
            id,
            player,
            module: p.id,
            code_len: p.code.len() as u32,
            checksum: p.checksum,
            pc: 0,
            vars: p.vars.clone(),
            stack,
            status: status::RUNNING,
            stop: false,
            last_op: -1,
            pause_ms: 0,
            wait_semaphore: -1,
            child: None,
        }
    }

    /// `RemoveTail`; empty -> 0 (E-3).
    pub fn pop(&mut self) -> i32 {
        self.stack.pop().unwrap_or(0)
    }

    /// `AddTail`; `false` at the limit `MAX_STACK`, the value is then discarded (re/ki-vm 12.3 P6).
    pub fn push(&mut self, v: i32) -> bool {
        if self.stack.len() >= MAX_STACK {
            return false;
        }
        self.stack.push(v);
        true
    }

    /// Number of nested children; 0 without a child.
    pub fn depth(&self) -> usize {
        let mut depth = 0;
        let mut t = self;
        while let Some(child) = t.child.as_deref() {
            depth += 1;
            t = child;
        }
        depth
    }
}

/// Change to the thread table, applied immediately after the triggering instruction (re/ki-vm 12.3 P8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// New thread from `SPAWN` (ID already assigned).
    Spawn(Thread),
    /// `0x19`: `stop` for all other threads with a child (re/ki-vm 12.3 P9).
    StopOthersWithChild { spare: Option<i32> },
    /// `0x1A`: `stop = false` for all threads.
    ReleaseAll,
    /// `0x18C`: pause AI (re/ki-vm 12.3 P12).
    PauseKi,
}

/// Environment of an instruction: catalog, state of the engine and world.
pub struct Exec<'a> {
    pub catalog: &'a KiCatalog,
    pub rand: &'a mut CrtRand,
    pub world: &'a mut dyn KiWorld,
    /// Slot of the engine.
    pub slot: u8,
    pub mem: &'a mut EngineMem,
    /// `m_iHandleID`: next thread ID.
    pub next_thread_id: &'a mut i32,
    /// Threads in the table + created spawns (`MAX_THREADS`).
    pub thread_count: usize,
    /// `false` in the synchronous run (the topmost instance is not in the table).
    pub top_in_table: bool,
    pub effects: Vec<Effect>,
    /// Values of the other engines for `0xE6`.
    pub peers: Peers<'a>,
}

impl Exec<'_> {
    /// `KiNote::Limit` for the instance `t`.
    pub(crate) fn limit(&mut self, t: &Thread, limit: KiLimit) {
        self.world.note(KiNote::Limit {
            slot: self.slot,
            thread: t.id,
            module: t.module,
            limit,
        });
    }

    /// Push with a note at the stack limit.
    pub(crate) fn push(&mut self, t: &mut Thread, v: i32) {
        if !t.push(v) {
            self.limit(t, KiLimit::Stack);
        }
    }
}

/// `FUN_004b4e10`: execute one instruction of the instance `t` (depth `depth` below the thread).
pub fn exec_one(ex: &mut Exec<'_>, t: &mut Thread, depth: usize) {
    let pc = t.pc;
    let word = match ex
        .catalog
        .get(t.module)
        .and_then(|p| p.code.get(pc as usize))
    {
        Some(&w) => {
            t.pc = pc + 1;
            w
        }
        None => END_WORD,
    };
    let opcode = word >> 28;
    let operand = word & OPERAND_MASK;
    ex.world.trace(&KiTrace {
        slot: ex.slot,
        thread: t.id,
        depth,
        module: t.module,
        pc,
        op: opcode,
    });
    match opcode {
        op::LOAD => {
            let v = t.vars.load(operand);
            ex.push(t, v);
        }
        op::STORE => {
            let mut v = t.pop();
            // E-2: directly after PUSHA the pushed address is resolved.
            if t.last_op == op::PUSHA as i32 {
                v = t.vars.get(v as u32);
            }
            if operand != DROP {
                t.vars.store(operand, v);
            }
        }
        op::PUSHA => ex.push(t, operand as i32),
        op::STORE3 => {
            let v = t.pop();
            if operand != DROP {
                t.vars.set(operand, v);
            }
        }
        op::CALLB => builtins::call(ex, t, operand, depth),
        op::CALLM => call_module(ex, t, operand, depth),
        op::SPAWN => spawn(ex, t, operand),
        op::JMP => t.pc = operand,
        op::JZ => {
            let v = t.pop();
            if v == 0 {
                t.pc = operand;
            }
        }
        op::JNZ => {
            let v = t.pop();
            if v != 0 {
                t.pc = operand;
            }
        }
        _ => {}
    }
    t.last_op = opcode as i32;
}

/// `CALLM m` (re/ki-vm 4.2): the child gets the whole stack (E-7) and inherits ID, player and `stop`. Without a
/// prototype (E-6) or above `MAX_CALL_DEPTH` (re/ki-vm 12.3 P6): nothing, only a note.
fn call_module(ex: &mut Exec<'_>, t: &mut Thread, target: u32, depth: usize) {
    let Some(p) = ex.catalog.get(target) else {
        ex.world.note(KiNote::MissingModule {
            slot: ex.slot,
            thread: t.id,
            module: t.module,
            target,
            spawn: false,
        });
        return;
    };
    if depth + 1 > MAX_CALL_DEPTH {
        ex.limit(t, KiLimit::CallDepth);
        return;
    }
    let mut child = Thread::from_prototype(p, t.id, t.player, std::mem::take(&mut t.stack));
    child.stop = t.stop;
    t.child = Some(Box::new(child));
    t.status |= status::CHILD;
}

/// `SPAWN m` (`FUN_004ad0d0`, re/ki-vm 4.3): new thread with a copy of the whole stack, next thread ID and the slot of
/// the engine as player; registered after the instruction (re/ki-vm 12.3 P8). Without a prototype nothing; above
/// `MAX_THREADS` nothing and no ID consumed (re/ki-vm 12.3 P6).
fn spawn(ex: &mut Exec<'_>, t: &mut Thread, target: u32) {
    let Some(p) = ex.catalog.get(target) else {
        ex.world.note(KiNote::MissingModule {
            slot: ex.slot,
            thread: t.id,
            module: t.module,
            target,
            spawn: true,
        });
        return;
    };
    if ex.thread_count >= MAX_THREADS {
        ex.limit(t, KiLimit::Threads);
        return;
    }
    let id = *ex.next_thread_id;
    *ex.next_thread_id = id.wrapping_add(1);
    ex.thread_count += 1;
    let new = Thread::from_prototype(p, id, ex.slot, t.stack.clone());
    ex.effects.push(Effect::Spawn(new));
}

/// `FUN_004b58c0` without debugger: one step of the instance `t`; returns `m_lStatus`.
///
/// Order: check and take over the semaphore (afterwards the step continues, re/ki-vm 12.3 P3); with a running child and
/// `!stop` one step of the child (a return uses up the step); otherwise count down the pause; otherwise end of code ->
/// `DONE`; otherwise one instruction.
pub fn step(ex: &mut Exec<'_>, t: &mut Thread, dt: i32, depth: usize) -> u32 {
    if t.wait_semaphore >= 0 {
        let key = t.wait_semaphore;
        if ex.mem.semaphores.get(&key).is_some_and(|&v| v != 0) {
            return t.status;
        }
        ex.mem.semaphores.insert(key, 1);
        t.wait_semaphore = -1;
    }
    // Status 0x10 without a child occurs only in corrupted savegames; then the own code runs.
    if t.status & status::CHILD != 0
        && !t.stop
        && let Some(child) = t.child.as_deref_mut()
    {
        if step(ex, child, dt, depth + 1) & status::DONE != 0 {
            // Return = everything the child leaves on the stack (E-7).
            let rest = t.child.take().map(|c| c.stack).unwrap_or_default();
            for v in rest {
                ex.push(t, v);
            }
            t.status = status::RUNNING;
        }
        return t.status;
    }
    if t.pause_ms > 0 {
        t.pause_ms = t.pause_ms.wrapping_sub(dt);
        return t.status;
    }
    let at_end = match ex.catalog.get(t.module) {
        Some(p) => t.pc as usize >= p.code.len(),
        // Prototype missing (conceivable only after a corrupted load): end the thread.
        None => true,
    };
    if at_end {
        t.status = status::DONE;
    } else {
        exec_one(ex, t, depth);
    }
    t.status
}
