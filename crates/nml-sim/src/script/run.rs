//! Container tick and condition combination (re/scripts 4).

use nml_formats::abl::{RtBlock, ScriptFile};

use super::container::{Container, update_wait};
use super::{ScriptTrace, actions, conditions, trace};
use crate::sim::Sim;

/// `ScriptContainer_Tick(dt)` (`0x0056D4E0`). Return value: number of remaining blocks.
///
/// 1. Every timer `> 0`: `max(0, t - dt)` (negative ones stay; new ones count from the following tick).
/// 2. Blocks from the last to the first: touched if it waits (without conditions) or its
///    conditions are true; then count down the wait (if it is still waiting, next block), then the
///    actions, then delete if `deleteable && delete_me && !running`. If an action has started a
///    dialog, the block loop ends.
/// 3. Countdown `> 0`: `max(0, c - dt)`, also in the tick in which it is set (E-129).
pub fn tick_container(sim: &mut Sim, c: &mut Container, file: &ScriptFile, dt: i32) -> i32 {
    for t in c.timers.values_mut() {
        if *t > 0 {
            *t = t.wrapping_sub(dt).max(0);
        }
    }
    let mut i = c.blocks.len();
    while i > 0 {
        i -= 1;
        let index = c.blocks[i].index;
        let Some(block) = file.blocks.get(index as usize) else {
            trace(
                sim,
                ScriptTrace::Guarded {
                    container: c.id,
                    block: index,
                    number: -1,
                    why: format!("block index {index} beyond the end of the file"),
                },
            );
            continue;
        };
        if !c.blocks[i].running && !evaluate(sim, c, index, block) {
            continue;
        }
        if update_wait(&mut c.blocks[i], dt) {
            continue;
        }
        trace(
            sim,
            ScriptTrace::Fired {
                container: c.id,
                block: index,
            },
        );
        let dialog_started = actions::execute_block(sim, c, i, block);
        let b = c.blocks[i];
        if b.deleteable && b.delete_me && !b.running {
            c.blocks.remove(i);
            trace(
                sim,
                ScriptTrace::Deleted {
                    container: c.id,
                    block: index,
                },
            );
        }
        if dialog_started {
            break;
        }
    }
    if c.counter > 0 {
        c.counter = c.counter.wrapping_sub(dt).max(0);
    }
    c.blocks.len() as i32
}

/// `ScriptBlock_EvaluateConditions` (`0x0056D450`, E-124): true without conditions; otherwise **all**
/// evaluated left to right, the prefix of the first has no effect, then `acc && b` (`&`) or
/// `acc || b` (any other prefix), without precedence and without short-circuiting. With the test switch
/// `Sim::all_conditions_true` the conditions (including messages) still run, the result is
/// true. `index` is the block index in the file (for messages).
pub fn evaluate(sim: &mut Sim, c: &mut Container, index: u32, block: &RtBlock) -> bool {
    let mut acc = true;
    for (k, cond) in block.conditions.iter().enumerate() {
        let b = conditions::condition(sim, c, index, cond);
        acc = if k == 0 {
            b
        } else if cond.or {
            acc || b
        } else {
            acc && b
        };
    }
    acc || sim.all_conditions_true
}
