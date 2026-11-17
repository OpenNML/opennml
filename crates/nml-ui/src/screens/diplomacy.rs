//! Diplomacy dialog `DIPLOMATIE` (`diplomatie.dlg`, handler `FUN_004D0750`; re/game-ui D5.7).
//!
//! Row i is slot i (`LOOP 8`, the reader appends the counter: `chat0...chat7`, `name0...`, `status0...`, `view0...`,
//! `tribut0...`); the handler compares the beginning (`CString_Left`) and reads the slot with `atol` from the rest.
//! The data is refreshed by each element every 500 ms (code 9 -> `timeout 500`). `nml-ui` does not know the
//! simulation: the host stores the state per frame as [`DiplomacyView`] (`MenuApp::set_diplomacy`), changes go back
//! to it as [`UiEffect::Diplomacy`]. Opening and closing (`FUN_0042E060`) are in [`toggle`]; "Back" and ESC
//! (`MENUBUTTON_CHECKESCAPE`) close through the same function.
//!
//! Without a match the handler answers -1 (`FUN_004B9140() == 0`, `0x004D07C9`), likewise without a state from the
//! host.
//!
//! In network play (re/multiplayer-menus L.7.2, E-320) the handler has no branches of its own: structure, elements
//! and commands as in single player, dropped-out slots hidden, `LockTeams` from the session. The open dialog
//! does not pause the match there; the commands go through the turn and take effect on all machines in the same tick.

use super::{GameInfo, ScreenEnv, close};
use crate::draft::{atol, text_of};
use crate::menu::AppQueue;
use crate::msg::{AppCmd, ElementMsg, Param, UiEffect, code};

/// Status "hidden" (`HIDE`).
const HIDE: i32 = 0x1000_0000;
/// Status "locked" (`DISABLE`).
const DISABLE: i32 = 0x2000_0000;
/// Query interval of all row elements (code 9, `FUN_004636b0("timeout", 500)`).
pub const TIMEOUT_MS: i32 = 500;
/// Color name: text `Player+0x210 + 470` (`FUN_004FAB40`).
pub const COLOR_TEXT: i32 = 470;
/// Nation: text `Player+0x238 + 15120` (WID, PRI, SPA, SID, ENG, PAT).
pub const NATION_TEXT: i32 = 15120;
/// Entries of the status list: 15720 "Allied", 15721 "Enemy", 15722 "Neutral" (`0x3D68 + i`).
const STATUS_TEXT: i32 = 15720;
/// Menu state "diplomacy open" (`Engine+0x2CCC = 10`).
pub const MENU_STATE: u8 = 10;
/// Name of the frame.
pub const DIALOG: &str = "diplomatie";
/// Amount per tribute button (`FUN_004BA4D0(resource, 100)`).
pub const TRIBUTE_AMOUNT: i32 = 100;

/// A row of the dialog: slot N from the point of view of the local player.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiplomacyRow {
    pub slot: u8,
    /// Display name (`FUN_004FAB10`, `Player+0xC`).
    pub name: String,
    /// Text ID of the color name ([`COLOR_TEXT`] + color).
    pub color_text: i32,
    /// Text ID of the nation ([`NATION_TEXT`] + nation).
    pub nation_text: i32,
    /// Result `Player+0x358` (`m_GameEndStatus`): 0 in play, 1 lost, 2 won, 3 unoccupied.
    pub result: i32,
    /// `FUN_0042ED20(l, N)`: 1 enemy, otherwise 0 friend, otherwise 2 neutral.
    pub status: i32,
    /// `HasSharedVision(N, l)` (`FUN_0042EDB0`): N sees what the local player sees.
    pub view: bool,
    /// Bit N of the chat mask of the local player.
    pub chat: bool,
    /// Bit N of the tribute mask of the local player.
    pub tribute: bool,
}

/// State of the dialog, per frame from the host (`MenuApp::set_diplomacy`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiplomacyView {
    /// Slot of the local player (`Engine+0x24E0`).
    pub local: u8,
    pub rows: [DiplomacyRow; 8],
    /// Fixed teams (`Engine+0x20`, `m_IsLockTeams`): the status selection is locked.
    pub locked_teams: bool,
}

/// What the dialog changes at the host ([`UiEffect::Diplomacy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiplomacyAction {
    /// `statusN` code 4: `{0x64, l, N, choice}` (`FUN_0042ED70`), one-sided.
    Status { slot: u8, mode: i32 },
    /// `viewN` code 1: `{0x63, N, l, on}` (`FUN_0042EDD0`, O-322).
    View { slot: u8, on: bool },
    /// `chatN` code 1: bit N of the chat mask (`FUN_004FAC00`), local without a command.
    Chat { slot: u8, on: bool },
    /// `tributN` code 1: bit N of the tribute mask (`FUN_004FAC70`), local without a command.
    Tribute { slot: u8, on: bool },
    /// `chatall`/`chatfriend`/`chatenemy`/`chatneutral` (`FUN_004BA3F0(-1/0/1/2)`): chat mask by relation.
    ChatBy { relation: i32 },
    /// `tributgold`/`tributwood`/`tributfood` (`FUN_004BA4D0(0/1/2, 100)`): `{0x65, l, tribute mask, resource, 100}`.
    SendTribute { resource: i32 },
    /// "Back" or ESC has closed the dialog (report only: [`toggle`] has already set state and command).
    Close,
}

/// `FUN_0042E060(open)` (re/game-ui D5.7): under the input lock (`Engine+0x600`, [`GameInfo::ui_locked`]) nothing
/// opens - toggling with an open dialog does not close either then; at state 0 open (state 10,
/// `{6,"DIPLOMATIE"}`, the match stops in single player, not in the network); at state 10 close (state 0,
/// `{4,"DIPLOMATIE"}`).
/// `open = false` ("Back") only closes. Without a match nothing (the function belongs to the engine). `true` if
/// the state has changed.
pub(crate) fn toggle(
    game_menu: &mut u8,
    game: Option<&GameInfo>,
    queue: &mut AppQueue,
    open: bool,
) -> bool {
    let Some(game) = game else {
        return false;
    };
    if game.ui_locked {
        if open {
            return false;
        }
    } else if open && *game_menu == 0 {
        *game_menu = MENU_STATE;
        queue.push(AppCmd::Open {
            name: DIALOG.to_string(),
            wait: false,
        });
        return true;
    }
    if *game_menu != MENU_STATE {
        return false;
    }
    *game_menu = 0;
    queue.push(close(DIALOG, false));
    true
}

fn int(msg: &ElementMsg) -> Option<i32> {
    match msg.params.get("int") {
        Some(Param::Int(v)) => Some(*v),
        _ => None,
    }
}

fn put(msg: &mut ElementMsg, name: &str, value: Param) {
    msg.params.insert(name.into(), value);
}

/// Columns of a row in the check order of the original (`chat` 4, `name` 4, `color` 5, `nation` 6, `status` 6,
/// `view` 4, `tribut` 6 characters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Chat,
    Name,
    Color,
    Nation,
    Status,
    View,
    Tribute,
}

/// Column and slot (`_atol` of the rest); slots outside 0-7 do not exist (`GameEngine_GetPlayer` null -> -1).
fn column(element: &str) -> Option<(Column, usize)> {
    let (col, rest) = [
        ("chat", Column::Chat),
        ("name", Column::Name),
        ("color", Column::Color),
        ("nation", Column::Nation),
        ("status", Column::Status),
        ("view", Column::View),
        ("tribut", Column::Tribute),
    ]
    .into_iter()
    .find_map(|(prefix, col)| element.strip_prefix(prefix).map(|rest| (col, rest)))?;
    let slot = usize::try_from(atol(rest.as_bytes()))
        .ok()
        .filter(|&s| s < 8)?;
    Some((col, slot))
}

/// Handler `FUN_004D0750`. Return values like the original: the chat buttons and "Back" 1 for every code, the tribute
/// buttons -1 (also after sending, `0x004D1159`), row elements 1 for their codes, otherwise -1.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    if env.data.game.is_none() || env.data.diplomacy.is_none() {
        return -1;
    }
    let triggered = msg.code == code::TRIGGERED;
    let relation = match msg.element.as_str() {
        "back" => {
            if triggered && toggle(env.game_menu, env.data.game.as_ref(), env.queue, false) {
                env.effects
                    .push(UiEffect::Diplomacy(DiplomacyAction::Close));
            }
            return 1;
        }
        "chatall" => Some(-1),
        "chatfriend" => Some(0),
        "chatenemy" => Some(1),
        "chatneutral" => Some(2),
        _ => None,
    };
    if let Some(relation) = relation {
        if triggered {
            env.effects
                .push(UiEffect::Diplomacy(DiplomacyAction::ChatBy { relation }));
        }
        return 1;
    }
    let resource = match msg.element.as_str() {
        "tributgold" => Some(0),
        "tributwood" => Some(1),
        "tributfood" => Some(2),
        _ => None,
    };
    if let Some(resource) = resource {
        if triggered {
            env.effects
                .push(UiEffect::Diplomacy(DiplomacyAction::SendTribute {
                    resource,
                }));
        }
        return -1;
    }
    let Some((col, slot)) = column(&msg.element) else {
        return -1;
    };
    row(msg, env, col, slot)
}

/// A row element (table re/game-ui D5.7). Values that are set are also written by the handler into the copy of the
/// state (status, vision, masks): the original takes effect at once (command at once in single player, masks
/// directly), and the ComboBox queries again right after the choice; the host delivers the new state only after the
/// frame (in the network only after the execution tick of the command).
fn row(msg: &mut ElementMsg, env: &mut ScreenEnv, col: Column, slot: usize) -> i32 {
    let Some(view) = env.data.diplomacy.as_mut() else {
        return -1;
    };
    let own = slot == usize::from(view.local);
    let locked_teams = view.locked_teams;
    let r = &mut view.rows[slot];
    let slot_u8 = slot as u8;
    let out = r.result != 0;
    match msg.code {
        code::QUERY_TIMEOUT => {
            put(msg, "timeout", Param::Int(TIMEOUT_MS));
            1
        }
        code::QUERY_STATUS => {
            // Name, color, nation hidden only at `res != 0`; everything else also in the own row. Status locked
            // with fixed teams (`FUN_004BA960`, `0x004D0E08`, O-323).
            let label = matches!(col, Column::Name | Column::Color | Column::Nation);
            let status = match col {
                _ if out => HIDE,
                _ if label => 0,
                _ if own => HIDE,
                Column::Status if locked_teams => DISABLE,
                _ => 0,
            };
            put(msg, "status", Param::Int(status));
            1
        }
        code::QUERY_TEXT if matches!(col, Column::Name | Column::Color | Column::Nation) => {
            let text = match col {
                Column::Name => r.name.clone(),
                Column::Color => text_of(env.texts, r.color_text),
                _ => text_of(env.texts, r.nation_text),
            };
            put(msg, "text", Param::Text(text));
            1
        }
        code::QUERY_VALUE => {
            let v = match col {
                Column::Chat => i32::from(r.chat),
                Column::View => i32::from(r.view),
                Column::Tribute => i32::from(r.tribute),
                Column::Status => r.status,
                _ => return -1,
            };
            put(msg, "int", Param::Int(v));
            1
        }
        code::VALUE_CHANGED if matches!(col, Column::Chat | Column::View | Column::Tribute) => {
            // `FUN_0045E220("int")`: if the value is missing, 0 applies (off).
            let on = int(msg).unwrap_or(0) != 0;
            let action = match col {
                Column::Chat => {
                    r.chat = on;
                    DiplomacyAction::Chat { slot: slot_u8, on }
                }
                Column::View => {
                    r.view = on;
                    DiplomacyAction::View { slot: slot_u8, on }
                }
                _ => {
                    r.tribute = on;
                    DiplomacyAction::Tribute { slot: slot_u8, on }
                }
            };
            env.effects.push(UiEffect::Diplomacy(action));
            1
        }
        code::QUERY_ITEMS if col == Column::Status => {
            let items = (0..3)
                .map(|i| text_of(env.texts, STATUS_TEXT + i))
                .collect();
            put(msg, "array", Param::Array(items));
            1
        }
        // `0x004D0D7A`: without `int` 0, otherwise `{0x64, l, N, choice}`; a mode outside 0-2 does nothing in the
        // command.
        code::ITEM_SELECTED if col == Column::Status => {
            let Some(mode) = int(msg) else {
                return 0;
            };
            if (0..=2).contains(&mode) {
                r.status = mode;
            }
            env.effects
                .push(UiEffect::Diplomacy(DiplomacyAction::Status {
                    slot: slot_u8,
                    mode,
                }));
            1
        }
        _ => -1,
    }
}
