//! Accounting after the end of the game (re/accounting 4.3 and 5): the frame `ACCOUNTING`
//! (`FUN_004D93F0`, called from the dispatcher chain `0x004DC280`) with the tab buttons `ACCOUNT0...2`, `MAINMENU` and
//! `QUIT` ("Continue"), and the tab pages `ACCOUNT0` (Technology), `ACCOUNT1` (Military), `ACCOUNT2` (Economy)
//! (`FUN_004CF8C0`).
//!
//! The host builds the table on command `0x0E` from the statistics log of the simulation (`FUN_00403FA0`) and puts
//! it with `MenuApp::set_accounting` as [`AccountingView`] into [`super::MenuData::accounting`]; the pages read it
//! anew on every code 7 like the original from `GameApp+0xD760`. `QUIT` continues automatically in the campaign
//! (`FUN_004BA6B0`: next level or video).

use opennml_formats::campaign::EntryKind;

use super::{ScreenEnv, campaign, close, open};
use crate::campaign::{auto_continue, current_entry};
use crate::msg::{AppCmd, ElementMsg, Param, code};
use crate::text::PLAYER_COLORS;

/// Status "hidden" (`HIDE`).
const HIDE: i32 = 0x1000_0000;

/// Flag when the frame is opened (`0x0061FE2C`): always "Military" (O-166).
const START_TAB: &str = "account1";

/// Gray of the name color from index 8 (`0x0061F744[8...12]`).
const GREY: u32 = 0xFFAC_ACAC;

/// Accounting as the host builds it on command `0x0E` (`FUN_00403FA0`, re/accounting 4.1, and step 4 of
/// `0x0E`, re/accounting 6). The UI does not know the simulation; it gets the table as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountingView {
    /// Rows in slot order (only active records, without gaps): name (`+0x488`), color index (`+0x408`, without
    /// range check) and the twelve values as `"%d"` (`+0x708`, `swprintf` `0x00404093`).
    pub rows: Vec<(String, i32, [String; 12])>,
    /// Game time text `GameApp+0x10`: "%s %d:%02d" from text 15349 and hours, minutes (O-164).
    pub time_text: String,
}

/// Frame `ACCOUNTING` (`FUN_004D93F0`, re/accounting 5; flag `DAT_00671EF8` =
/// [`super::ScreenState::accounting_tab`]).
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        // `0x004D9749`...`0x004D978B`: frame opened -> flag `account1`, `{5, flag}` - on every opening (O-166).
        ("accounting", code::QUERY_STATUS) => {
            env.state.accounting_tab = START_TAB.to_string();
            env.queue.push(open(START_TAB));
            1
        }
        // `0x004D97AD`...`0x004D97EC`: `status` 0 if the start parameter `[GameApp+0xD650]+0x1F4` (`FUN_004BA5C0`) is
        // -1, otherwise `HIDE`. The field is the game type of the session (`FUN_004CBB10(GameType)` 0...6 in free
        // play, -1 only at campaign start `0x0C`/`0x28`); in the reimplementation the draft (the match has already
        // ended after `0x0E`). So "Continue" is missing in **every** free play, not only in network play (O-167
        // corrected).
        ("quit", code::QUERY_STATUS) => {
            let status = if env.data.draft.settings.game_type == -1 {
                0
            } else {
                HIDE
            };
            msg.params.insert("status".into(), Param::Int(status));
            1
        }
        // `0x004D94A7`...`0x004D94F7`: The original checks `EngineConfig+0x0C` ("`Engine.ini` read", always 1 after the
        // start); the branch "Start Gamespy" (code 7 text 15118, code 8 `PostQuitMessage`) is omitted (O-26).
        // `{3,"accounting"}`, `{4, flag}`, `{5,"start"}`.
        ("mainmenu", code::TRIGGERED) => {
            close_tabs(msg, env);
            env.queue.push(open("start"));
            1
        }
        // `0x004D9506`...`0x004D9655`: `{3,"accounting"}`, `{4, flag}`; then automatic continue in the campaign
        // (`FUN_004BA6B0`: game type of the session -1, `STOP` 0 and flag "newly free") -> `{0x15}` and depending on
        // the kind of the current entry (`FUN_004BA550`) level: `FUN_004B9830(entry)` (path; in the reimplementation
        // `0x0C` reads it itself), `{0x0C}`; otherwise (video, also a headline): `{0x1C}`, `{0x1D, path}`, `{0x1A}`,
        // `{0x28}` - without `{5,"selectcampaign"}`. Only here does a video start by itself (O-158). Without continue:
        // campaign -> `selectcampaign`, free play without profile (`FUN_004BA580` < 0) -> `start`, with profile ->
        // `singleplayermenu`. The free-play branch is unreachable because the button is hidden there (O-202). None of
        // the paths touches the menu music (O-207).
        ("quit", code::TRIGGERED) => {
            close_tabs(msg, env);
            let campaign_mode = env.data.draft.settings.game_type == -1;
            if auto_continue(&env.data.profiles, &env.data.campaigns, campaign_mode) {
                env.queue.push(AppCmd::WaitDialogs);
                let level = current_entry(&env.data.profiles, &env.data.campaigns)
                    .is_some_and(|e| e.kind == EntryKind::Level);
                if level {
                    env.queue.push(AppCmd::StartCampaignLevel);
                } else {
                    for cmd in campaign::video_commands(env) {
                        env.queue.push(cmd);
                    }
                }
                return 1;
            }
            let target = if campaign_mode {
                "selectcampaign"
            } else if env.data.profiles.file.current < 0 {
                "start"
            } else {
                "singleplayermenu"
            };
            env.queue.push(open(target));
            1
        }
        // `0x004D965F`...: a button other than the remembered one closes its page (`{3}`), remembers the button name
        // and opens the page of the same name (`{5}`); the same button does nothing. Return value 1.
        (name, code::TRIGGERED) => {
            if name != env.state.accounting_tab {
                env.queue.push(close(&env.state.accounting_tab, true));
                env.state.accounting_tab = name.to_string();
                env.queue.push(open(name));
            }
            1
        }
        // Code 7 `mainmenu` (O-26), other elements at code 5 and other codes: -1.
        _ => -1,
    }
}

/// `{3, frame}`, `{4, flag}` before leaving the accounting.
fn close_tabs(msg: &ElementMsg, env: &mut ScreenEnv) {
    env.queue.push(close(&msg.dialog, true));
    env.queue.push(close(&env.state.accounting_tab, false));
}

/// Tab pages `ACCOUNT0...2` (`FUN_004CF8C0`, re/accounting 4.3): page `k` = digit at index 7 of the dialog name,
/// column offset `4*k`. `name<z>` and `value<c><z>` (first digit column `c`, second row `z`) are hidden from `z >=
/// rows` (code 5) and otherwise show name with color or the value of kind `c + 4*k` (code 7); `spielzeit` shows the
/// game time text.
pub(super) fn handle_tab(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let page = digit(&msg.dialog, 7);
    let view = env.data.accounting.as_ref();
    let rows = view.map_or(&[][..], |v| v.rows.as_slice());
    let element = msg.element.as_str();
    if element == "spielzeit" {
        // `0x004CF978`...`0x004CF9AB`: code 7 `text` = pointer to `GameApp+0x10`; other codes -1.
        if msg.code != code::QUERY_TEXT {
            return -1;
        }
        let text = view.map(|v| v.time_text.clone()).unwrap_or_default();
        msg.params.insert("text".into(), Param::Text(text));
        return 1;
    }
    let (row, column) = if let Some(rest) = element.strip_prefix("name") {
        (digit(rest, 0), None)
    } else if let Some(rest) = element.strip_prefix("value") {
        (digit(rest, 1), Some(digit(rest, 0) + 4 * page))
    } else {
        return -1;
    };
    // Row count `FUN_00403F00`; signed comparison like the original (`z >= rows`).
    let count = i32::try_from(rows.len()).unwrap_or(i32::MAX);
    match msg.code {
        // `0x004CFB4C`...`0x004CFB63`, `0x004CFC8C`...`0x004CFCA3`: `HIDE` from `z >= rows`, otherwise 1 without
        // `status`.
        code::QUERY_STATUS => {
            if row >= count {
                msg.params.insert("status".into(), Param::Int(HIDE));
            }
            1
        }
        // `0x004CFAD1`...`0x004CFB1C`, `0x004CFC31`...`0x004CFC5B`: only `z < rows`.
        code::QUERY_TEXT if row < count => {
            // `FUN_00403E80`/`FUN_00403ED0` outside the table: `L"error"` or empty text, color index 0.
            let entry = usize::try_from(row).ok().and_then(|z| rows.get(z));
            match column {
                None => {
                    let (name, color) = entry.map_or(("error", 0), |r| (r.0.as_str(), r.1));
                    msg.params.insert("text".into(), Param::Text(name.into()));
                    msg.params
                        .insert("color".into(), Param::Int(name_color(color) as i32));
                }
                Some(c) => {
                    let value = entry
                        .and_then(|r| usize::try_from(c).ok().and_then(|c| r.2.get(c)))
                        .cloned()
                        .unwrap_or_default();
                    msg.params.insert("text".into(), Param::Text(value));
                }
            }
            1
        }
        _ => -1,
    }
}

/// Digit at `index` (character - `'0'` like `(char)name[i] - 0x30`); if the character is missing, -0x30 like the null
/// character.
fn digit(name: &str, index: usize) -> i32 {
    name.as_bytes().get(index).map_or(0, |&b| i32::from(b)) - 0x30
}

/// Name color `0x0061F744[color index]` (ARGB, without range check). Entries 0...8 equal the table of
/// `[SETPLAYERCOLOR n]` (`0x006321B4`, [`PLAYER_COLORS`]), 9...12 are also gray `0xFFACACAC`; from 13 on and below 0
/// the original reads foreign data. The reimplementation takes gray there (the color index of a record is unchecked;
/// -1 reads the word before the table in the original).
fn name_color(index: i32) -> u32 {
    usize::try_from(index)
        .ok()
        .and_then(|i| PLAYER_COLORS.get(i))
        .copied()
        .unwrap_or(GREY)
}
