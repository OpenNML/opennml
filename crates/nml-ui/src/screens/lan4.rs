//! Waiting room of a loaded network match `LAN4` (`lan4.dlg`, handler `FUN_004CF290`, re/multiplayer-saves S.4.1).
//!
//! `lan4.dlg`: three header lines 15411 ... 15413, `LOOP 8` with the labels `NAMEn`, `COLORn`, `STATUSn` (n = 0 ... 7)
//! and a button `START`; no `EXIT`, no element with `CHECKESCAPE`, no chat. The handler compares the beginning
//! (`CString_Left`) and takes **key = row - 1** (`atol` - 1) like `LAN2`; the rows are in the match draft
//! ([`crate::draft::MatchDraft::slots`]), the host role in the lobby view ([`LobbyView`]), both set by the game host
//! from the lobby state of the network host. All rows and `START` query again every second (code 9, `timeout 1000`),
//! so `START` also after every new lobby state.
//!
//! Deviation (E-326): the exit is ESC, which no element takes (`MenuApp` then queues `{0x2D}`, [`escape_leaves`]); the
//! handler keeps the dead branch `exit`. The reimplementation shows `START` only when every row is "Occupied"
//! (type 999); the original also accepts type 4, which does not occur in the loaded lobby.

use super::player_slots::{COLOR_TEXT, HIDE, put};
use super::{ScreenEnv, close};
use crate::draft::{HUMAN, LobbyView, atol, text_of};
use crate::msg::{AppCmd, ElementMsg, Param, code};

/// Name of the frame.
pub const DIALOG: &str = "lan4";
/// Query interval of the rows and of `START` (code 9, `timeout 1000`).
pub const TIMEOUT_MS: i32 = 1000;
/// "Open" (`0x3C3C`): name of an unoccupied row, status type 0.
const OPEN_TEXT: i32 = 15420;
/// "Occupied" (`0x3C3D`): status type 999.
const TAKEN_TEXT: i32 = 15421;

/// Open `LAN4` like `FUN_0042E510` (re/multiplayer-saves S.2.2 step 9): `{6,"lan4"}` over the existing menu. The
/// reimplementation does not set the menu state `Engine+0x2CCC = 5`: there is no match below `LAN4`.
pub fn open_lan4() -> AppCmd {
    AppCmd::Open {
        name: DIALOG.to_string(),
        wait: false,
    }
}

/// Close `LAN4` like `FUN_0042E570` (re/multiplayer-saves S.4.2): `{4,"lan4"}`.
pub fn close_lan4() -> AppCmd {
    close(DIALOG, false)
}

/// Exit from `LAN4` (deviates from E-326): an ESC that no element takes acts like `EXIT` of the lobby (`{0x2D}`) when
/// `LAN4` is on top (`open_names` in opening order). If something lies above it - for example message 15212 after
/// the host has left, whose `MSGBOX` does not take ESC (O-103) - ESC has no effect. If `{0x2D}` or the closing of
/// `LAN4` is already pending (`queued`), nothing more is queued; the key still counts as taken. Returns `None` if
/// the rule does not apply, otherwise whether `{0x2D}` is to be queued.
pub fn escape_leaves<'a>(
    open_names: &[String],
    mut queued: impl Iterator<Item = &'a AppCmd>,
) -> Option<bool> {
    if open_names.last().map(String::as_str) != Some(DIALOG) {
        return None;
    }
    let closing = close_lan4();
    Some(!queued.any(|c| *c == AppCmd::LeaveLobby || *c == closing))
}

/// Columns of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Color,
    Status,
    Name,
}

/// `CString_Left` + `_atol(Mid)` - 1 in the order of the handler (`color`, `status`, `name`): column and key.
fn column(element: &str) -> Option<(Column, i32)> {
    [
        ("color", Column::Color),
        ("status", Column::Status),
        ("name", Column::Name),
    ]
    .into_iter()
    .find_map(|(prefix, c)| {
        element
            .strip_prefix(prefix)
            .map(|rest| (c, atol(rest.as_bytes()).wrapping_sub(1)))
    })
}

fn lobby<'a>(env: &'a ScreenEnv) -> Option<&'a LobbyView> {
    env.data.net.as_ref().and_then(|n| n.lobby.as_ref())
}

/// `FUN_004CF290`.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    if let Some((col, key)) = column(&msg.element) {
        if msg.code == code::QUERY_TIMEOUT {
            put(msg, "timeout", Param::Int(TIMEOUT_MS));
            return 1;
        }
        // Without a lobby state of the game host (no network session) the handler answers no row (like `LAN2`).
        if lobby(env).is_none() {
            return -1;
        }
        return match msg.code {
            // `FUN_004E97E0`: slot missing -> `HIDE`.
            code::QUERY_STATUS => {
                let status = if env.data.draft.slot(key).is_some() {
                    0
                } else {
                    HIDE
                };
                put(msg, "status", Param::Int(status));
                1
            }
            // `NetSession_GetLobbyPlayerInfo`: no text without a slot.
            code::QUERY_TEXT => {
                if let Some(text) = row_text(env, col, key) {
                    put(msg, "text", Param::Text(text));
                }
                1
            }
            _ => -1,
        };
    }
    match (msg.element.as_str(), msg.code) {
        ("start", code::QUERY_TIMEOUT) => {
            put(msg, "timeout", Param::Int(TIMEOUT_MS));
            1
        }
        ("start", code::QUERY_STATUS) => {
            let status = if can_start(env) { 0 } else { HIDE };
            put(msg, "status", Param::Int(status));
            1
        }
        // `{0x14}` (`FUN_004BCB40`) without a check; the network session of the host checks and starts.
        ("start", code::TRIGGERED) => {
            env.queue.push(AppCmd::StartSavedGame);
            1
        }
        // `0x004CF7C0`: dead branch (E-326), return value 1 without effect.
        ("exit", code::QUERY_STATUS | code::TRIGGERED) => 1,
        _ => -1,
    }
}

/// Code 7 of a row (S.4.1): `NAME` type 999 -> name of the slot (`FUN_004E8CB0`), otherwise 15420; `COLOR` 15150 +
/// color; `STATUS` type 0 -> 15420, 999 -> 15421, otherwise no text. No text without a slot.
fn row_text(env: &ScreenEnv, col: Column, key: i32) -> Option<String> {
    let slot = env.data.draft.slot(key)?;
    match col {
        Column::Name if slot.kind == HUMAN => Some(slot.name.clone()),
        Column::Name => Some(text_of(env.texts, OPEN_TEXT)),
        Column::Color => Some(text_of(env.texts, COLOR_TEXT + slot.color)),
        Column::Status => match slot.kind {
            0 => Some(text_of(env.texts, OPEN_TEXT)),
            HUMAN => Some(text_of(env.texts, TAKEN_TEXT)),
            _ => None,
        },
    }
}

/// `START` code 5 (`FUN_004E85F0`, `FUN_004E8CE0`): only the host and every row "Occupied".
fn can_start(env: &ScreenEnv) -> bool {
    let slots = &env.data.draft.slots;
    lobby(env).is_some_and(|l| l.is_host)
        && !slots.is_empty()
        && slots.iter().all(|s| s.kind == HUMAN)
}
