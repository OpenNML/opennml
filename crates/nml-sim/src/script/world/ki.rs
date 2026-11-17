//! AI actions of the scripts: 0/1 start module, 37 delete engine, 340 set a value of the global list.
//! Direct world actions without a command and without a sender (cases `0x00`, `0x01`, `0x19`,
//! `0x7B` of `Script_DispatchAction` with `FUN_0042DB60`, `FUN_0042D900`, `FUN_0042DB40`, `FUN_00431640`);
//! the behavior of the AI side lives in [`crate::ki::start`]. The load check of the numbers (37: slot
//! 0...7, 340: index >= 0) lives in `nml_formats::abl::catalog`.
//!
//! Value slots per `actions::ki_words`: 0/1 `{slot, guid, count, words...}` (the count is only present
//! with at least three parameters), 37 `{slot}`, 340 `{slot, index, value}`.

use super::super::actions::{MsgValue, ScriptMsg};
use crate::ki::start;
use crate::sim::Sim;

/// Actions 0 (`sync`, `FUN_004A51D0`) and 1 (`FUN_004A5120`): run the module `msg.int(1)` on the slot
/// `msg.int(0)` synchronously or create it as a thread. The arguments are `values[3 .. 3 + values[2]]`
/// (the count limited to the existing values); with at most two parameters there are none. The thread
/// does not run until the next AI tick; if a dialog is running, the AI rests and with it the thread
/// (E-139). Slot outside 0-7, without an engine (also after 37) or unknown module: nothing. The original
/// does not use the return value of the AI side.
pub fn start(sim: &mut Sim, msg: &ScriptMsg, sync: bool) {
    let args = module_words(msg);
    let (slot, module) = (msg.int(0), msg.int(1) as u32);
    if sync {
        start::run_sync(sim, slot, module, &args);
    } else {
        start::spawn_thread(sim, slot, module, &args);
    }
}

/// The module words of the message: empty with fewer than three values, otherwise `values[3 .. 3 + count]`.
fn module_words(msg: &ScriptMsg) -> Vec<i32> {
    if msg.values.len() < 3 {
        return Vec::new();
    }
    let count = usize::try_from(msg.int(2)).unwrap_or(0);
    msg.values
        .iter()
        .skip(3)
        .take(count)
        .map(|v| match v {
            MsgValue::Int(i) => *i,
            MsgValue::Text(_) => 0,
        })
        .collect()
}

/// Action 37 (E-221): delete the engine of the slot.
pub fn delete(sim: &mut Sim, slot: i32) {
    start::delete_engine(sim, slot);
}

/// Action 340 (E-220): `m_listGlobal[index] = value` of the slot's engine.
pub fn set_global(sim: &mut Sim, slot: i32, index: i32, value: i32) {
    start::set_global(sim, slot, index, value);
}
