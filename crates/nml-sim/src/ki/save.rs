//! Load check of the AI state: only what would otherwise let the VM crash, hang or use unbounded memory.
//! Catalog and code of the threads are part of the rules checksum (`KiCatalog::load`); the VM tolerates
//! everything else. The reasons are error messages; `save.rs` reports them as "savegame corrupted (AI: ...)".

use super::engine::{Engine, THREAD_BUCKETS};
use super::vars::LIST_BUCKETS;
use super::vm::Thread;
use super::{KiState, MAX_STACK};

/// Per engine the thread table (17 buckets, each key in its own bucket, none duplicated: otherwise `get`
/// divides by 0, or `insert` adds a key a second time and `run_slice_steps` runs forever), per thread
/// including children the lists (10 buckets) and the stack (at most [`MAX_STACK`] values: `SPAWN` clones it
/// into up to 512 threads). Each `MfcMap` is checked before it is accessed by key. The depth of the children
/// is already limited by reading (`vm::deserialize_child`).
pub fn check_state(ki: &KiState) -> Result<(), String> {
    for (index, engine) in ki.engines.iter().enumerate() {
        if let Some(engine) = engine {
            check_engine(index, engine)?;
        }
    }
    Ok(())
}

fn check_engine(index: usize, e: &Engine) -> Result<(), String> {
    if let Err(why) = e.threads.check(THREAD_BUCKETS) {
        return Err(format!("engine {index}: thread table: {why}"));
    }
    for (_, t) in e.threads.iter() {
        check_thread(index, t.id, t, 0)?;
    }
    Ok(())
}

/// A thread or child at depth `depth` (0 = thread of the table) with the ID `id` of the thread.
fn check_thread(index: usize, id: i32, t: &Thread, depth: usize) -> Result<(), String> {
    let at = || format!("engine {index}, thread {id}, depth {depth}");
    if t.stack.len() > MAX_STACK {
        return Err(format!(
            "{}: stack with {} values (at most {MAX_STACK})",
            at(),
            t.stack.len()
        ));
    }
    if let Some(why) = t
        .vars
        .ranges
        .values()
        .flat_map(|r| &r.lists)
        .find_map(|l| l.check(LIST_BUCKETS).err())
    {
        return Err(format!("{}: list: {why}", at()));
    }
    if let Some(child) = t.child.as_deref() {
        check_thread(index, id, child, depth + 1)?;
    }
    Ok(())
}
