//! Script container and block state (re/scripts 5): one container per running script with the four
//! namespaces, the countdown, and the state of every remaining block.

use std::collections::BTreeMap;

use opennml_formats::abl::ScriptFile;
use serde::{Deserialize, Serialize};

/// Origin of a container: file (normalized path as in `ScriptRules`) and FNV-1a 64 of its bytes at
/// start (a changed file changes the rules checksum, `ScriptRules::load`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSource {
    pub path: String,
    pub checksum: u64,
}

/// Script container (`FUN_00564010`, serializer `FUN_0056DA60`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    /// Level containers 0, 1, ...; dialog container -1 (`m_LastTriggerEngineID`).
    pub id: i32,
    pub source: ScriptSource,
    /// Remaining blocks in file order.
    pub blocks: Vec<BlockState>,
    /// `m_mapSchalter` (`+0x50`): name -> 1.
    pub switches: BTreeMap<String, i32>,
    /// `m_mapRandomSchalter` (`+0x34`): name -> random value (action 33, condition 17).
    pub random_switches: BTreeMap<String, i32>,
    /// `m_mapActiontimer` (`+0x6C`): name -> ms.
    pub timers: BTreeMap<String, i32>,
    /// `m_mapVariables` (`+0x88`): name -> integer.
    pub variables: BTreeMap<String, i32>,
    /// `m_iCounter` (`+0xA4`): countdown in ms, start -1.
    pub counter: i32,
    /// Skip flag (`ScriptContainer_TickUnlessSkipped` `0x0056D900`): only the dialog container, set by the
    /// debug command `ScriptSkipDialog`; the next dialog tick ends the dialog without running a tick. On the
    /// level container (only from a corrupted savegame) nobody reads it.
    #[serde(default)]
    pub skipped: bool,
}

/// State of a block (`FUN_005631D0`, serializer `FUN_00563AB0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    /// Index in `ScriptFile::blocks`.
    pub index: u32,
    /// `m_bDeleteMe` (`+0x04`): executed.
    pub delete_me: bool,
    /// `m_bDeleteable` (`+0x05`): default true, action 10 clears it.
    pub deleteable: bool,
    /// `m_IsRunning` (`+0x34`): waits at `cur_action`.
    pub running: bool,
    /// `m_CurActionIndex` (`+0x38`); not reset at the end of the block (E-125).
    pub cur_action: i32,
    /// `m_TimeoutCountdown` (`+0x3C`) in ms; -1 = wait for an event.
    pub timeout: i32,
}

impl BlockState {
    /// Defaults of the block constructor `FUN_005631D0`.
    pub fn new(index: u32) -> Self {
        Self {
            index,
            delete_me: false,
            deleteable: true,
            running: false,
            cur_action: 0,
            timeout: -1,
        }
    }
}

impl Container {
    /// Container with all blocks of the file, empty namespaces and countdown -1.
    pub fn new(id: i32, source: ScriptSource, file: &ScriptFile) -> Self {
        Self {
            id,
            source,
            blocks: (0..file.blocks.len() as u32).map(BlockState::new).collect(),
            switches: BTreeMap::new(),
            random_switches: BTreeMap::new(),
            timers: BTreeMap::new(),
            variables: BTreeMap::new(),
            counter: -1,
            skipped: false,
        }
    }
}

/// `ScriptBlock_SuspendAtAction` (`0x00563170`): **always** sets `cur_action = 0` and `running = false`;
/// only with the `w` flag does the block stop at action `index` with the wait time `ms` (`−1` = waiting
/// for an event). Without `w`, a repeatable block therefore starts again from the front (E-172).
pub fn suspend(block: &mut BlockState, index: usize, flag: bool, ms: i32) {
    block.cur_action = 0;
    block.running = false;
    if flag {
        block.cur_action = index as i32;
        block.running = true;
        block.timeout = ms;
    }
}

/// `ScriptBlock_UpdateWaitTimeout` (`0x00562FE0`). Return value `true`: the block is still waiting and is
/// skipped in this tick. `timeout == −1` -> continue; otherwise `t − dt` (wrapped to 32 bits), negative ->
/// 0; != 0 -> skip; = 0 -> `running = false`, `cur_action + 1`, `timeout = −1` and continue with the
/// following action in the same tick (E-130).
pub fn update_wait(block: &mut BlockState, dt: i32) -> bool {
    if block.timeout == -1 {
        return false;
    }
    let t = block.timeout.wrapping_sub(dt).max(0);
    block.timeout = t;
    if t != 0 {
        return true;
    }
    block.running = false;
    block.cur_action = block.cur_action.wrapping_add(1);
    block.timeout = -1;
    false
}
