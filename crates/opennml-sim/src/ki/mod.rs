//! AI basics: state of the script VM of No Man's Land.
//! See re/ki-vm and `docs/formats/kimodule.md`.

pub mod builtins;
pub mod catalog;
pub mod command;
pub mod engine;
pub mod query;
pub mod run;
pub mod save;
pub mod signature;
pub mod start;
pub mod stub;
pub mod vars;
pub mod vm;
pub mod world;

use serde::{Deserialize, Serialize};

use crate::id;
use crate::rng::CrtRand;
use catalog::KiCatalog;
use engine::{Engine, KiCtx};
use world::{KiWorld, Peers};

/// Maximum number of values on the stack of an instance (re/ki-vm 12.3 P6); a push beyond it discards the value.
pub const MAX_STACK: usize = 4096;
/// Maximum number of threads of an engine (re/ki-vm 12.3 P6); a `SPAWN` beyond it starts nothing.
pub const MAX_THREADS: usize = 512;
/// Maximum number of nested `CALLM` (re/ki-vm 12.3 P6); a `CALLM` beyond it does nothing.
pub const MAX_CALL_DEPTH: usize = 32;
/// Maximum number of steps of a synchronous run (re/ki-vm 12.3 P6).
pub const MAX_SYNC_STEPS: u32 = 100_000;

/// `m_iKiPaused` (`KIManager+0x9C`): 0 runs, 2 paused; 1 ("whole game") is omitted (E-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KiPause {
    Running,
    Paused,
}

/// AI settings of the start package (`MatchSetup::ki`, re/ki-vm 12.3 P13); with `enabled`, `new_match` starts the
/// AI of the AI slots (`ki::start::start_match`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiSetup {
    /// Start the AI of the AI slots (default off).
    pub enabled: bool,
    /// Campaign difficulty 0-2 (`FUN_004dabd0` writes only 0, 1, 2; default 1).
    pub campaign_difficulty: i32,
}

impl Default for KiSetup {
    fn default() -> Self {
        Self {
            enabled: false,
            campaign_difficulty: 1,
        }
    }
}

/// AI state of the simulation: one engine per occupied slot, pause state and the randomness of the
/// AI (re/ki-vm 12.3 P5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiState {
    pub engines: [Option<Engine>; 8],
    pub paused: KiPause,
    pub rand: CrtRand,
}

impl KiState {
    /// Without engines, running, randomness `CrtRand::new(seed)`.
    pub fn new(seed: u32) -> Self {
        Self {
            engines: Default::default(),
            paused: KiPause::Running,
            rand: CrtRand::new(seed),
        }
    }

    /// Engine of slot `slot` (0-7), created if needed.
    pub fn engine_mut(&mut self, slot: u8) -> &mut Engine {
        self.engines[usize::from(slot)].get_or_insert_with(|| Engine::new(slot))
    }

    /// `KIManager_TickAllPlayers`: paused -> nothing; otherwise one tick per slot 0-7 with an engine.
    /// `0x18C` pauses the AI only after the tick of all slots (re/ki-vm 12.3 P12). Each engine reads the values of the
    /// others in their current state (`0xE6`).
    pub fn tick_engines(&mut self, catalog: &KiCatalog, world: &mut dyn KiWorld) {
        if self.paused == KiPause::Paused {
            return;
        }
        let mut pause = false;
        for i in 0..self.engines.len() {
            let (engine, peers) = split_engines(&mut self.engines, i);
            let Some(engine) = engine.as_mut() else {
                continue;
            };
            let mut cx = KiCtx {
                catalog,
                rand: &mut self.rand,
                world: &mut *world,
                peers,
            };
            pause |= engine.tick(&mut cx);
        }
        if pause {
            self.paused = KiPause::Paused;
        }
    }

    /// Engine of slot `slot` (0-7, created if needed) with an environment that sees the values of the
    /// other engines (`0xE6`), for `Engine::spawn`/`Engine::run_sync` from outside. Slot
    /// outside 0-7 -> `None`.
    pub fn with_engine<R>(
        &mut self,
        slot: u8,
        catalog: &KiCatalog,
        world: &mut dyn KiWorld,
        f: impl FnOnce(&mut Engine, &mut KiCtx<'_>) -> R,
    ) -> Option<R> {
        let i = usize::from(slot);
        if i >= self.engines.len() {
            return None;
        }
        let (engine, peers) = split_engines(&mut self.engines, i);
        let engine = engine.get_or_insert_with(|| Engine::new(slot));
        let mut cx = KiCtx {
            catalog,
            rand: &mut self.rand,
            world,
            peers,
        };
        Some(f(engine, &mut cx))
    }
}

/// Entry `i` mutable and the values of all other engines as [`Peers`] (`split_at_mut`).
fn split_engines(engines: &mut [Option<Engine>], i: usize) -> (&mut Option<Engine>, Peers<'_>) {
    let (head, rest) = engines.split_at_mut(i);
    let (current, tail) = rest
        .split_first_mut()
        .expect("slot inside the engine table");
    let mut values = [None; id::PLAYER_SLOTS];
    for (j, e) in head
        .iter()
        .enumerate()
        .chain(tail.iter().enumerate().map(|(k, e)| (i + 1 + k, e)))
    {
        if let Some(v) = values.get_mut(j) {
            *v = e.as_ref().map(|e| &e.mem.values);
        }
    }
    (current, Peers::new(values))
}

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod list_tests;
#[cfg(test)]
mod memory_tests;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
