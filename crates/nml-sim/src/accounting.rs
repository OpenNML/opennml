//! Statistics log of the accounting (re/accounting): the log object
//! `m_Accounting` (`GameApp+0xD760`, constructor `FUN_00403D10`) with eight player records, the write path
//! `FUN_00478A10` -> `FUN_004BA620` -> `FUN_00403E10` and the evaluation `FUN_00403FA0` for the pages
//! `ACCOUNT0…2`.
//!
//! The original keeps the log in the application (it survives "end game" until the next
//! `Game_StartLevel`); in the reimplementation it belongs to the match state ([`SimState::accounting`]) so that
//! the savegame and the checksum carry it. The display reads it through [`accounting_table`]. The kinds
//! (re/accounting 2): 0 unit trained, 1 building finished, 2 upgrade researched, 3 without writer
//! (O-163), 4 unit killed, 5 building destroyed, 6 unit lost, 7 building lost, 8 food,
//! 9 wood, 10 gold, 11 trade; 0-7 are counted, 8-11 are summed.

use serde::{Deserialize, Serialize};

use crate::id::PLAYER_SLOTS;
use crate::sim::Sim;
use crate::state::SimState;

/// Kinds of the entries (columns of the evaluation, re/accounting 2).
pub mod kind {
    pub const UNIT_TRAINED: i32 = 0;
    pub const BUILDING_BUILT: i32 = 1;
    pub const UPGRADE_RESEARCHED: i32 = 2;
    pub const UNIT_KILLED: i32 = 4;
    pub const BUILDING_DESTROYED: i32 = 5;
    pub const UNIT_LOST: i32 = 6;
    pub const BUILDING_LOST: i32 = 7;
    pub const FOOD: i32 = 8;
    pub const WOOD: i32 = 9;
    pub const GOLD: i32 = 10;
    pub const TRADE: i32 = 11;
}

/// Columns of the evaluation (`wchar[12]` fields per row, `+0x708`).
pub const COLUMNS: usize = 12;
/// First summed kind (8-11 are summed, the ones before are counted; `FUN_00403270`/`FUN_00403220`).
const FIRST_SUMMED: i32 = 8;
/// Longest name of a record: `wchar[40]` with terminator (`+0x20`). The original copies unchecked
/// (`wcscpy`, O-175); the reimplementation truncates to 39 characters (deviation from the original).
pub const MAX_NAME_CHARS: usize = 39;
/// Default name of a record (`FUN_00403140`: `L"Jon Doe"`).
pub const DEFAULT_NAME: &str = "Jon Doe";
/// Identifier of the placeholder players (`FUN_004BA5D0`: `swscanf(name, "%d") == 0x7288`, O-174).
pub const DUMMY_PLAYER: i32 = 29320;
/// Log entry (`Entry`, 12 bytes of payload, re/accounting 1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEntry {
    /// `Type`: kind 0-11 (= column).
    pub kind: i32,
    /// `Time`: world tick `GameWorld+0x74` (100 ms steps); stored, never evaluated (O-178).
    pub time: i32,
    /// `Parm`: type GUID (kinds 0-7, never evaluated, O-178) or amount (8-11).
    pub parm: i32,
}

/// Player record `AccountRecord` (0x80 bytes, re/accounting 1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    /// `m_IsActivate` (`+0x7C`): only active records accept entries and appear in the table.
    pub active: bool,
    /// `m_Name` (`+0x20`, at most [`MAX_NAME_CHARS`] characters).
    pub name: String,
    /// `m_Color` (`+0x74`): color index of the name in the accounting.
    pub color: i32,
    /// `m_Narion` (`+0x78`, nation; stored only).
    pub nation: i32,
    /// `m_Team` (`+0x70`; stored only).
    pub team: i32,
    /// `m_AccountingList` (`+0x04`): entries in write order, without an upper limit.
    pub entries: Vec<AccountEntry>,
}

impl Default for AccountRecord {
    /// `FUN_00403140`: inactive, name "Jon Doe", color, nation and team 0, empty list.
    fn default() -> Self {
        Self {
            active: false,
            name: DEFAULT_NAME.to_string(),
            color: 0,
            nation: 0,
            team: 0,
            entries: Vec::new(),
        }
    }
}

/// Log object `Accounting` (`m_Accounting`, `m_Accounts` with a fixed size of 8).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Accounting {
    /// Records per slot 0-7.
    pub records: [AccountRecord; PLAYER_SLOTS],
}

impl Accounting {
    /// `FUN_00403E10(slot, kind, time, parm)`: only slots 0-7 with an active record; append the entry
    /// at the tail (`AddTail`). The kind is not checked (a kind outside 0-11 counts nowhere in the
    /// evaluation).
    pub fn add(&mut self, slot: i32, kind: i32, time: i32, parm: i32) {
        if let Some(r) = record_mut(self, slot)
            && r.active
        {
            r.entries.push(AccountEntry { kind, time, parm });
        }
    }

    /// `FUN_004BA5D0(slot, name, color, nation, team)` from `PlayerSetup_ParseLvl` and
    /// `GameEngine_SetupPlayersFromLobby`: names that begin with the number 29320 (placeholders,
    /// [`is_dummy_name`], O-174) create no record; otherwise `FUN_00403DE0` (slots 0-7 only) ->
    /// `FUN_004031E0`: name (truncated to 39 characters, O-175), color, nation, team, active, clear the list.
    pub fn init_record(&mut self, slot: i32, name: &str, color: i32, nation: i32, team: i32) {
        if is_dummy_name(name) {
            return;
        }
        if let Some(r) = record_mut(self, slot) {
            r.name = cut_name(name);
            r.color = color;
            r.nation = nation;
            r.team = team;
            r.active = true;
            r.entries.clear();
        }
    }

    /// `FUN_00403F50(slot, name)` from `Game_SelectLocalPlayer`: set the name of an **active** record
    /// (profile name, truncated like [`Accounting::init_record`]). The original does not check the slot
    /// (re/accounting E14); here a slot outside 0-7 has no effect.
    pub fn set_local_name(&mut self, slot: i32, name: &str) {
        if let Some(r) = record_mut(self, slot)
            && r.active
        {
            r.name = cut_name(name);
        }
    }
}

fn record_mut(a: &mut Accounting, slot: i32) -> Option<&mut AccountRecord> {
    usize::try_from(slot)
        .ok()
        .and_then(|s| a.records.get_mut(s))
}

/// Name truncated to [`MAX_NAME_CHARS`] characters (deviation from the original: no overflow of `wcscpy`).
fn cut_name(name: &str) -> String {
    name.chars().take(MAX_NAME_CHARS).collect()
}

/// `swscanf(name, L"%d", &n) == 29320` (`FUN_004BA5D0` `0x004BA5E3…0x004BA5F4`): skip leading
/// whitespace, sign, digits (wrapped to 32 bits like the CRT); without a digit the read fails
/// and the original compares the pointer value, which is never 29320.
pub fn is_dummy_name(name: &str) -> bool {
    let mut it = name
        .chars()
        .skip_while(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}'))
        .peekable();
    let negative = match it.peek() {
        Some('-') => {
            it.next();
            true
        }
        Some('+') => {
            it.next();
            false
        }
        _ => false,
    };
    let mut digits = 0usize;
    let mut v = 0u32;
    while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
        v = v.wrapping_mul(10).wrapping_add(d);
        digits += 1;
        it.next();
    }
    let v = if negative { v.wrapping_neg() } else { v } as i32;
    digits > 0 && v == DUMMY_PLAYER
}

/// `GameWorld_AddStatistic` (`FUN_00478A10(slot, kind, parm)` -> `FUN_004BA620` -> `FUN_00403E10`):
/// entry with the world tick [`SimState::tick`] (`GameWorld+0x74`) as `Time`; only slots 0-7 with an
/// active record. Also public for the script actions 21/22 (re/accounting 2, O-170).
pub fn add_statistic(sim: &mut Sim, slot: i32, kind: i32, parm: i32) {
    let time = sim.state.tick as i32;
    sim.state.accounting.add(slot, kind, time, parm);
}

/// One row of the evaluation (`+0x408`/`+0x488`/`+0x708` of the table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingRow {
    /// Display name (empty -> `computer`).
    pub name: String,
    /// Color index of the name (`m_Color`, without a range check).
    pub color: i32,
    /// Column `s` = kind `s`: 0-7 number of entries, 8-11 sum of the `Parm` (wrapped to 32 bits).
    pub values: [i32; COLUMNS],
}

/// `FUN_00403FA0` (from command `0x0E`): active records in slot order, without gaps; empty name ->
/// `computer` (text 15195 "Computer", O-177, in the table only); each column counted or summed over the
/// whole list without a time filter. Kinds outside 0-11 count nowhere.
pub fn accounting_table(state: &SimState, computer: &str) -> Vec<AccountingRow> {
    state
        .accounting
        .records
        .iter()
        .filter(|r| r.active)
        .map(|r| {
            let mut values = [0i32; COLUMNS];
            for e in &r.entries {
                let Some(v) = usize::try_from(e.kind).ok().and_then(|k| values.get_mut(k)) else {
                    continue;
                };
                *v = v.wrapping_add(if e.kind >= FIRST_SUMMED { e.parm } else { 1 });
            }
            AccountingRow {
                name: if r.name.is_empty() {
                    computer.to_string()
                } else {
                    r.name.clone()
                },
                color: r.color,
                values,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
