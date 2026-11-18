//! Engine per slot (`KIEngine`): thread table, passes (`FUN_004acd00`), tick (`FUN_004acf60(3, 20)`),
//! start of a thread (`FUN_004acfb0`) and synchronous run (`FUN_004ac2f0`). See re/ki-vm 4.3, 4.4, 5 and
//! re/ki-lifecycle 1.4, 5.1.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::catalog::KiCatalog;
use super::vm::{self, Effect, Exec, Thread, status};
use super::world::{KiLimit, KiNote, KiWorld, Peers};
use super::{MAX_SYNC_STEPS, MAX_THREADS};
use crate::mfc::MfcMap;
use crate::rng::CrtRand;

/// `m_iSchrittweite` (`+0xC20`): steps per thread and pass, fixed 3 (`FUN_004a49e0`).
pub const STEPS_PER_SLICE: u32 = 3;
/// Passes per engine tick (`FUN_004acf60(3, 20)`).
pub const SLICES_PER_TICK: i32 = 3;
/// `dt` of an engine tick in ms.
pub const TICK_MS: i32 = 20;
/// `dt` of a pass: 20 / 3 in integer arithmetic = 6 (pauses run 8% too fast, E-8).
pub const SLICE_DT: i32 = TICK_MS / SLICES_PER_TICK;
/// `dt` per step of the synchronous run (`FUN_004ac2f0`), without game time passing.
pub const SYNC_DT: i32 = 20;
/// Buckets of the thread table `m_mapModuleObjects` (MFC `CMap`, hash size 17).
pub const THREAD_BUCKETS: usize = 17;

/// Number of attacker lists `m_arrayAttacker`/`m_arrayNotAttacker`.
pub const ATTACKER_LISTS: usize = 20;
/// Number of slots of `m_arrayRessource*`.
pub const RESOURCE_SLOTS: usize = 5;
/// Number of players of `m_arrayUnit*`/`m_arrayBuilding*`.
pub const POSITION_PLAYERS: usize = 8;

/// Player-wide memory of an engine (re/ki-builtins A 1, B 2, C 2), fields in the order of
/// `KIEngine_Serialize` (`FUN_004b3e40`). All threads of a player share it (re/ki-builtins A 3 no. 9).
/// The maps are accessed only by key, never iterated in their order, so `BTreeMap` suffices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineMem {
    /// `m_bSemaphoren`: key -> taken (re/ki-vm 12.3 P3).
    pub semaphores: BTreeMap<i32, u8>,
    /// `m_iValues`.
    pub values: BTreeMap<i32, i32>,
    /// `m_mapGoto`.
    pub goto: BTreeMap<i32, i32>,
    /// `m_arrayGlobal`: table -> (index -> value); in `0xBA` also only by key.
    pub tables: BTreeMap<i32, BTreeMap<i32, i32>>,
    /// `m_arrayAttacker[20]` (`CList`).
    pub attackers: Vec<Vec<i32>>,
    /// `m_arrayNotAttacker[20]`.
    pub not_attackers: Vec<Vec<i32>>,
    /// `m_arrayRessourceXPos/YPos/Type[5]`.
    pub resources: Vec<PosList>,
    /// `m_arrayUnitXPos/YPos/Type[8]`.
    pub units: Vec<PosList>,
    /// `m_arrayBuildingXPos/YPos/Type[8]`.
    pub buildings: Vec<PosList>,
    /// `m_listBuildingQueue`.
    pub building_queue: Vec<i32>,
    /// `m_listRepairerCountQueque` (parallel to the building list, drifts apart as described in re/ki-builtins C 5 A6).
    pub repairer_counts: Vec<i32>,
    /// `m_listBuildingEinquartierbar`.
    pub quarterable: Vec<i32>,
    /// `m_listGlobal`.
    pub global_list: Vec<i32>,
}

/// Three parallel `CList`s of a position list (X, Y, type).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PosList {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub kind: Vec<i32>,
}

impl EngineMem {
    /// Empty memory with 20/20 attacker lists, 5 resource slots and 8 unit and building slots each
    /// (ctor `FUN_004ac730`).
    pub fn new() -> Self {
        Self {
            semaphores: BTreeMap::new(),
            values: BTreeMap::new(),
            goto: BTreeMap::new(),
            tables: BTreeMap::new(),
            attackers: vec![Vec::new(); ATTACKER_LISTS],
            not_attackers: vec![Vec::new(); ATTACKER_LISTS],
            resources: vec![PosList::default(); RESOURCE_SLOTS],
            units: vec![PosList::default(); POSITION_PLAYERS],
            buildings: vec![PosList::default(); POSITION_PLAYERS],
            building_queue: Vec::new(),
            repairer_counts: Vec::new(),
            quarterable: Vec::new(),
            global_list: Vec::new(),
        }
    }
}

impl Default for EngineMem {
    /// Same as [`EngineMem::new`] (20/20/5/8/8 lists).
    fn default() -> Self {
        Self::new()
    }
}

impl PosList {
    /// Number of entries; for parallel lists their common length, otherwise the shortest (only
    /// from a corrupted savegame; all accesses thus stay within each of the three lists).
    pub fn len(&self) -> usize {
        self.x.len().min(self.y.len()).min(self.kind.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entry `i` as `(x, y, type)`; `None` for `i` outside.
    pub fn get(&self, i: usize) -> Option<(i32, i32, i32)> {
        (i < self.len()).then(|| (self.x[i], self.y[i], self.kind[i]))
    }

    /// Overwrites entry `i`; nothing if outside.
    pub fn set(&mut self, i: usize, x: i32, y: i32, kind: i32) {
        if i < self.len() {
            self.x[i] = x;
            self.y[i] = y;
            self.kind[i] = kind;
        }
    }

    /// `AddTail` on all three lists.
    pub fn push(&mut self, x: i32, y: i32, kind: i32) {
        self.x.push(x);
        self.y.push(y);
        self.kind.push(kind);
    }

    /// `InsertBefore` before entry `i` on all three lists; `i ≥ len` appends.
    pub fn insert(&mut self, i: usize, x: i32, y: i32, kind: i32) {
        let i = i.min(self.len());
        self.x.insert(i, x);
        self.y.insert(i, y);
        self.kind.insert(i, kind);
    }

    /// `RemoveAt` of entry `i` on all three lists; nothing if outside.
    pub fn remove(&mut self, i: usize) {
        if i < self.len() {
            self.x.remove(i);
            self.y.remove(i);
            self.kind.remove(i);
        }
    }

    /// First entry with the type `kind`.
    pub fn find_kind(&self, kind: i32) -> Option<usize> {
        self.kind[..self.len()].iter().position(|&k| k == kind)
    }
}

/// Result of a synchronous run ([`Engine::run_sync`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncRun {
    /// Consumed thread ID of the instance.
    pub id: i32,
    /// The run triggered `0x18C` (pause the AI after the run).
    pub pause: bool,
}

/// AI engine of a slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engine {
    /// `+0x08`: slot; player of all threads started here.
    pub slot: u8,
    pub mem: EngineMem,
    /// `m_iHandleID` (`+0xC3C`): next thread ID, initially 0.
    pub next_thread_id: i32,
    /// `m_mapModuleObjects` (`+0xC04`): thread ID -> thread in MFC order.
    pub threads: MfcMap<Thread>,
}

/// Shared environment of all engines: catalog, randomness of the AI state, world and read access
/// to the values of the other engines (`0xE6`).
pub struct KiCtx<'a> {
    pub catalog: &'a KiCatalog,
    pub rand: &'a mut CrtRand,
    pub world: &'a mut dyn KiWorld,
    pub peers: Peers<'a>,
}

impl<'a> KiCtx<'a> {
    /// Without foreign engines ([`Peers::none`]); with them `KiState::with_engine` builds the environment.
    pub fn new(catalog: &'a KiCatalog, rand: &'a mut CrtRand, world: &'a mut dyn KiWorld) -> Self {
        Self {
            catalog,
            rand,
            world,
            peers: Peers::none(),
        }
    }
}

impl Engine {
    pub fn new(slot: u8) -> Self {
        Self {
            slot,
            mem: EngineMem::default(),
            next_thread_id: 0,
            threads: MfcMap::new(THREAD_BUCKETS),
        }
    }

    /// `FUN_004acfb0`: enter a thread of module `module` with the arguments `args` (in order on
    /// the stack); returns the thread ID. It does not run until the next pass. Without a prototype,
    /// or else at the limit `MAX_THREADS` (re/ki-vm 12.3 P6): nothing, no ID consumed, a note
    /// with `thread = −1` (start from outside).
    pub fn spawn(&mut self, cx: &mut KiCtx<'_>, module: u32, args: &[i32]) -> Option<i32> {
        if cx.catalog.get(module).is_some() && self.threads.len() >= MAX_THREADS {
            cx.world.note(KiNote::Limit {
                slot: self.slot,
                thread: -1,
                module,
                limit: KiLimit::Threads,
            });
            return None;
        }
        let t = self.instance(cx, module, args, true)?;
        let id = t.id;
        self.threads.insert(id, t);
        Some(id)
    }

    /// `FUN_004ac2f0`: run the module immediately to the end, `dt = 20` per step (pauses
    /// lapse without game time). The instance consumes an ID but is not in the
    /// table; threads it starts enter it normally. After `MAX_SYNC_STEPS` steps
    /// the run aborts with `KiLimit::SyncSteps` (re/ki-vm 12.3 P6; the original hangs). Returns the ID and the
    /// `0x18C` flag, `None` without a prototype.
    pub fn run_sync(&mut self, cx: &mut KiCtx<'_>, module: u32, args: &[i32]) -> Option<SyncRun> {
        let mut t = self.instance(cx, module, args, false)?;
        let mut pause = false;
        for _ in 0..MAX_SYNC_STEPS {
            // In the synchronous run `0x18C` is only reported by the run; the caller sets `KiState::paused`
            // afterwards (`ki::start::run_sync`). In the run the original does not check the pause,
            // so the effect is the same.
            let mut ex = exec(
                cx,
                self.slot,
                &mut self.mem,
                &mut self.next_thread_id,
                self.threads.len(),
                false,
            );
            let s = vm::step(&mut ex, &mut t, SYNC_DT, 0);
            let effects = ex.effects;
            pause |= apply(&mut self.threads, effects);
            if s & status::DONE != 0 {
                return Some(SyncRun { id: t.id, pause });
            }
        }
        cx.world.note(KiNote::Limit {
            slot: self.slot,
            thread: t.id,
            module: t.module,
            limit: KiLimit::SyncSteps,
        });
        Some(SyncRun { id: t.id, pause })
    }

    /// `FUN_004acd00`: one pass over all threads in MFC order, up to `STEPS_PER_SLICE` steps per
    /// thread. The successor is determined **before** a thread runs: a thread started in the
    /// process runs in the same pass only if it lands behind this position (MFC inserts at the front
    /// of the bucket; see re/ki-vm 12.3 P8). A paused thread consumes its step slots without effect; a
    /// finished one leaves the table in the same step slot (re/ki-vm 12.3 P4). `true` if `0x18C` was triggered.
    pub fn run_slice(&mut self, cx: &mut KiCtx<'_>, dt: i32) -> bool {
        self.run_slice_steps(cx, dt, STEPS_PER_SLICE)
    }

    /// Single step of the debugger `0x4B0` (re/ki-debugger): one pass with step size 1 and `dt = 20` -
    /// each thread takes exactly one step. `true` if `0x18C` was triggered.
    pub fn step_threads(&mut self, cx: &mut KiCtx<'_>) -> bool {
        self.run_slice_steps(cx, TICK_MS, 1)
    }

    /// [`Self::run_slice`] with `steps` steps per thread (`m_iSchrittweite`).
    fn run_slice_steps(&mut self, cx: &mut KiCtx<'_>, dt: i32, steps: u32) -> bool {
        let mut pause = false;
        let mut pos = self.threads.first_key();
        while let Some(id) = pos {
            pos = self.threads.next_key(id);
            for _ in 0..steps {
                let count = self.threads.len();
                // The thread stays in the table during the step; `0x19` from a grandchild
                // thus hits it too (re/ki-vm 12.3 P9).
                let Some(t) = self.threads.get_mut(id) else {
                    break;
                };
                if t.stop {
                    break;
                }
                let mut ex = exec(
                    cx,
                    self.slot,
                    &mut self.mem,
                    &mut self.next_thread_id,
                    count,
                    true,
                );
                let s = vm::step(&mut ex, t, dt, 0);
                let effects = ex.effects;
                pause |= apply(&mut self.threads, effects);
                if s & status::DONE != 0 {
                    self.threads.remove(id);
                    break;
                }
            }
        }
        pause
    }

    /// `FUN_004acf60(3, 20)`: three passes with `dt = 6`. `true` if `0x18C` was triggered.
    pub fn tick(&mut self, cx: &mut KiCtx<'_>) -> bool {
        let mut pause = false;
        for _ in 0..SLICES_PER_TICK {
            pause |= self.run_slice(cx, SLICE_DT);
        }
        pause
    }

    /// Instance for `spawn`/`run_sync` (`FUN_004b49c0` + `FUN_004b40c0`): clone of the prototype with
    /// the arguments on the stack, ID `next_thread_id` (then `+1`), player = slot.
    fn instance(
        &mut self,
        cx: &mut KiCtx<'_>,
        module: u32,
        args: &[i32],
        spawn: bool,
    ) -> Option<Thread> {
        let Some(p) = cx.catalog.get(module) else {
            cx.world.note(KiNote::MissingModule {
                slot: self.slot,
                thread: -1,
                module,
                target: module,
                spawn,
            });
            return None;
        };
        let id = self.next_thread_id;
        self.next_thread_id = id.wrapping_add(1);
        let mut t = Thread::from_prototype(p, id, self.slot, Vec::new());
        for &v in args {
            if !t.push(v) {
                cx.world.note(KiNote::Limit {
                    slot: self.slot,
                    thread: id,
                    module,
                    limit: KiLimit::Stack,
                });
                break;
            }
        }
        Some(t)
    }
}

/// Environment for a step of a top-level instance of the engine.
fn exec<'a>(
    cx: &'a mut KiCtx<'_>,
    slot: u8,
    mem: &'a mut EngineMem,
    next_thread_id: &'a mut i32,
    thread_count: usize,
    top_in_table: bool,
) -> Exec<'a> {
    Exec {
        catalog: cx.catalog,
        rand: &mut *cx.rand,
        world: &mut *cx.world,
        slot,
        mem,
        next_thread_id,
        thread_count,
        top_in_table,
        effects: Vec::new(),
        peers: cx.peers,
    }
}

/// Applies the effects of a command to the table in order (re/ki-vm 12.3 P8): new thread at the front of its
/// bucket, set or release `stop`. `true` if `0x18C` was among them.
fn apply(threads: &mut MfcMap<Thread>, effects: Vec<Effect>) -> bool {
    let mut pause = false;
    for effect in effects {
        match effect {
            Effect::Spawn(new) => threads.insert(new.id, new),
            Effect::StopOthersWithChild { spare } => {
                for (id, t) in threads.iter_mut() {
                    if Some(id) != spare && t.child.is_some() {
                        t.stop = true;
                    }
                }
            }
            Effect::ReleaseAll => {
                for (_, t) in threads.iter_mut() {
                    t.stop = false;
                }
            }
            Effect::PauseKi => pause = true,
        }
    }
    pause
}
