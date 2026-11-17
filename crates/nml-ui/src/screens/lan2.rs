//! Lobby `LAN2` (`lan2.dlg`, handler `FUN_004D38F0`, re/multiplayer-menus L.3).
//!
//! Eight fixed rows n = 0 ... 7 with `READYn`, `NAMEn`, `RACEn`, `TEAMn`, `HANDICAPn`, `COLORn`; the handler compares
//! the beginning (`CString_Left`) and takes **key = row - 1** (`atol` - 1). The rows are in the match draft
//! ([`crate::draft::MatchDraft::slots`]), ready flag, own row and host role in the lobby view ([`LobbyView`]); the
//! game host sets both every frame from the state of the network host (`MenuApp::set_lobby_draft`,
//! `MenuApp::set_net`). A choice sets the local copy (path W `0x004D3BCD`...`0x004D3C1F`: `FUN_004E9750` local) and
//! goes to the game host as [`UiEffect::Lobby`], which sends it to the network host (diplomacy pattern); the next
//! state of the network host overwrites the copy. All row elements, `CHANGEGAME` and `START` query again every
//! second (code 9, `timeout 1000`, `0x004D3A7E`, `0x004D4843`, `0x004D48CA`).
//!
//! Deviations: the kick check of the own `NAME` row (re/multiplayer-menus L.3.5, message 15212 for type != 999) is
//! dropped - the network host disconnects a kicked player, the game host shows the message once. For writing, the
//! handler additionally checks against the status rule (in the original only the lock of the element prevents it) so
//! that a foreign row never triggers a request; the network host checks the right itself anyway. `START` sends only
//! the request: checking, closing the lobby and the start are done by the network session of the host. `CHANGEGAME`
//! is seen only by the host; the click closes its lobby and opens `LAN31`, the core stays in the lobby.

use super::player_slots::{
    COLOR_TEXT, DISABLE, HANDICAP_TEXT, HIDE, OPEN_TEXT, RACE_TEXT, check_change, color_list, int,
    join_first_computer, occupied, put, put_team_items, put_texts,
};
use super::{ScreenEnv, close, open};
use crate::draft::{HUMAN, LobbyView, atol, text_of};
use crate::msg::{AppCmd, ElementMsg, LobbyAction, Param, UiEffect, code};

/// Query interval of the row elements, `CHANGEGAME` and `START` (code 9, `timeout 1000`).
pub const TIMEOUT_MS: i32 = 1000;
/// Name of the frame.
pub const DIALOG: &str = "lan2";
/// The two frames that form the lobby together with `LAN2`.
const LEVELINFO: &str = "levelinfo";
const CHATFRAME: &str = "chatframe";
/// "Change game" (`lan31.dlg`).
const CHANGE_DIALOG: &str = "lan31";

/// Open the lobby (`FUN_004BCBE0(1)`, `0x004BCBE0`): `{5,"lan2"}`, `{6,"levelinfo"}`, `{6,"chatframe"}`; the network
/// session queues this after the success of `{7}`/`{8}`.
pub fn open_lobby() -> [AppCmd; 3] {
    [
        open(DIALOG),
        AppCmd::Open {
            name: LEVELINFO.to_string(),
            wait: false,
        },
        AppCmd::Open {
            name: CHATFRAME.to_string(),
            wait: false,
        },
    ]
}

/// Close the lobby (`FUN_004BCBE0(0)`): `{3,"lan2"}`, `{4,"levelinfo"}`, `{4,"chatframe"}`.
pub fn close_lobby() -> [AppCmd; 3] {
    [
        close(DIALOG, true),
        close(LEVELINFO, false),
        close(CHATFRAME, false),
    ]
}

/// Columns of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Ready,
    Race,
    Handicap,
    Color,
    Team,
    Name,
}

/// `CString_Left` + `_atol(Mid)` - 1: column and key.
fn column(element: &str) -> Option<(Column, i32)> {
    [
        ("ready", Column::Ready),
        ("race", Column::Race),
        ("handicap", Column::Handicap),
        ("color", Column::Color),
        ("team", Column::Team),
        ("name", Column::Name),
    ]
    .into_iter()
    .find_map(|(prefix, c)| {
        element
            .strip_prefix(prefix)
            .map(|rest| (c, atol(rest.as_bytes()).wrapping_sub(1)))
    })
}

/// Row 0 ... 7 for key -1 ... 6, otherwise `None`.
fn row_of(key: i32) -> Option<u8> {
    u8::try_from(key.wrapping_add(1)).ok().filter(|&r| r < 8)
}

fn lobby<'a>(env: &'a ScreenEnv) -> Option<&'a LobbyView> {
    env.data.net.as_ref().and_then(|n| n.lobby.as_ref())
}

fn lobby_mut<'a>(env: &'a mut ScreenEnv) -> Option<&'a mut LobbyView> {
    env.data.net.as_mut().and_then(|n| n.lobby.as_mut())
}

/// Status rule L.3.1 (`0x004D3AA7`...`0x004D3B65`, `NAME` `0x004D4481`...`0x004D454D`).
///
/// `READY`, `RACE`, `TEAM`, `HANDICAP`, `COLOR`: slot missing -> `HIDE`; foreign slot with type 999 -> `DISABLE`;
/// foreign open or computer slot and not host -> `DISABLE`; otherwise free. `HANDICAP` is additionally locked for
/// computer Easy/Normal/Hard (type 1 ... 3). `NAME`: key -1 -> `DISABLE` for all; slot missing -> `HIDE`; host
/// free, client `DISABLE`.
fn status(env: &ScreenEnv, lobby: &LobbyView, col: Column, key: i32) -> i32 {
    let slot = env.data.draft.slot(key);
    if col == Column::Name {
        return match slot {
            _ if key == -1 => DISABLE,
            None => HIDE,
            Some(_) if lobby.is_host => 0,
            Some(_) => DISABLE,
        };
    }
    let Some(slot) = slot else {
        return HIDE;
    };
    let own = key == i32::from(lobby.own_row).wrapping_sub(1);
    if !own && (slot.kind == HUMAN || !lobby.is_host) {
        return DISABLE;
    }
    if col == Column::Handicap && (1..=3).contains(&slot.kind) {
        return DISABLE;
    }
    0
}

/// `FUN_004D38F0` for `LAN2`.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    if let Some((col, key)) = column(&msg.element) {
        if msg.code == code::QUERY_TIMEOUT {
            put(msg, "timeout", Param::Int(TIMEOUT_MS));
            return 1;
        }
        // Without the lobby state of the game host (no network session) the handler answers no row.
        let Some(lobby) = lobby(env).cloned() else {
            return -1;
        };
        return match col {
            Column::Name => name(msg, env, &lobby, key),
            Column::Ready => ready(msg, env, &lobby, key),
            _ => cell(msg, env, &lobby, col, key),
        };
    }
    match (msg.element.as_str(), msg.code) {
        ("changegame" | "start", code::QUERY_TIMEOUT) => {
            put(msg, "timeout", Param::Int(TIMEOUT_MS));
            1
        }
        // `0x004D4818`...`0x004D4892` (re/multiplayer-menus L.16 no. 1): visible only for the host (`FUN_004E85F0`),
        // otherwise `HIDE`.
        ("changegame", code::QUERY_STATUS) => {
            let status = if lobby(env).is_some_and(|l| l.is_host) {
                0
            } else {
                HIDE
            };
            put(msg, "status", Param::Int(status));
            1
        }
        // Code 8 checks nothing (L.16 no. 1): `FUN_004BCBE0(0)` (lobby, `LEVELINFO` and chat of the host
        // closed, E-310), then `{5,"lan31"}`. Joins, requests and chat of the clients continue in the core.
        ("changegame", code::TRIGGERED) => {
            if lobby(env).is_some_and(|l| l.is_host) {
                for cmd in close_lobby() {
                    env.queue.push(cmd);
                }
                env.queue.push(open(CHANGE_DIALOG));
            }
            1
        }
        // `0x004D494E`...`0x004D49BD`: host and `FUN_004E8DF0` and map name and `FUN_004BA8F0() >= 2` -> free.
        ("start", code::QUERY_STATUS) => {
            let status = if can_start(env) { 0 } else { HIDE };
            put(msg, "status", Param::Int(status));
            1
        }
        // `0x004D48EE`...`0x004D4944` no longer checks anything; only the request to the network session of the host.
        ("start", code::TRIGGERED) => {
            if lobby(env).is_some_and(|l| l.is_host) {
                env.effects.push(UiEffect::Lobby(LobbyAction::Start));
            }
            1
        }
        // `0x004D49EE`...`0x004D4A44` (E-285): lobby closed, `{9}`, `{5,"start"}` - always to the main menu.
        ("exit", code::TRIGGERED) => {
            for cmd in close_lobby() {
                env.queue.push(cmd);
            }
            env.queue.push(AppCmd::CloseNet);
            env.queue.push(open("start"));
            1
        }
        _ => -1,
    }
}

/// Start condition L.3.4: host, every row with type != 0 ready (`FUN_004E8DF0`), map name not empty
/// (`FUN_004BA6E0`), at least two rows with type != 0 (`FUN_004BA8F0`).
fn can_start(env: &ScreenEnv) -> bool {
    let Some(lobby) = lobby(env) else {
        return false;
    };
    let draft = &env.data.draft;
    lobby.is_host
        && !draft.info.name.is_empty()
        && occupied(draft) >= 2
        && draft
            .slots
            .iter()
            .filter(|s| s.kind != 0)
            .all(|s| row_of(s.key).is_some_and(|r| lobby.ready[usize::from(r)]))
}

/// `READYn`: code 0 ready flag of the row (`FUN_004E8DC0`, without slot -1); code 1 sets it (path W); code 5 status
/// rule.
fn ready(msg: &mut ElementMsg, env: &mut ScreenEnv, lobby: &LobbyView, key: i32) -> i32 {
    let row = row_of(key);
    match msg.code {
        code::QUERY_VALUE => {
            let v = match (env.data.draft.slot(key), row) {
                (Some(_), Some(r)) => i32::from(lobby.ready[usize::from(r)]),
                _ => -1,
            };
            put(msg, "int", Param::Int(v));
            1
        }
        code::VALUE_CHANGED => {
            let Some(row) = row.filter(|_| status(env, lobby, Column::Ready, key) == 0) else {
                return 0;
            };
            let ready = int(msg).unwrap_or(0) != 0;
            if let Some(l) = lobby_mut(env) {
                l.ready[usize::from(row)] = ready;
            }
            env.effects
                .push(UiEffect::Lobby(LobbyAction::Ready { row, ready }));
            1
        }
        code::QUERY_STATUS => {
            put(
                msg,
                "status",
                Param::Int(status(env, lobby, Column::Ready, key)),
            );
            1
        }
        _ => -1,
    }
}

/// `RACEn`, `TEAMn`, `HANDICAPn`, `COLORn` (table L.3.2). Code 0 and 4 without a slot -> 0.
fn cell(
    msg: &mut ElementMsg,
    env: &mut ScreenEnv,
    lobby: &LobbyView,
    col: Column,
    key: i32,
) -> i32 {
    match msg.code {
        code::QUERY_VALUE => {
            let Some(s) = env.data.draft.slot(key) else {
                return 0;
            };
            let v = match col {
                Column::Race => s.nation,
                Column::Handicap => s.handicap,
                Column::Color => s.color,
                _ => s.team,
            };
            put(msg, "int", Param::Int(v));
            1
        }
        code::QUERY_ITEMS => {
            match col {
                Column::Race => put_texts(msg, env, RACE_TEXT..RACE_TEXT + 6),
                Column::Handicap => put_texts(msg, env, HANDICAP_TEXT..HANDICAP_TEXT + 6),
                // `FUN_004BD0A0`: without the colors of **all** other slots.
                Column::Color => {
                    let colors = color_list(&env.data.draft, key);
                    put_texts(msg, env, colors.iter().map(|c| COLOR_TEXT + c));
                    put(msg, "indexarray", Param::IndexArray(colors));
                }
                _ => put_team_items(msg, env),
            }
            1
        }
        code::ITEM_SELECTED => {
            let (Some(v), Some(mut slot), Some(row)) =
                (int(msg), env.data.draft.slot(key).cloned(), row_of(key))
            else {
                return 0;
            };
            if status(env, lobby, col, key) != 0 {
                return 0;
            }
            match col {
                // Nation and handicap without a check, color **without** `FUN_004BA710` (the list excludes others).
                Column::Race => slot.nation = v,
                Column::Handicap => slot.handicap = v,
                Column::Color => slot.color = v,
                _ => {
                    slot.team = v;
                    check_change(&mut env.data.draft, &mut slot);
                }
            }
            env.data.draft.set_slot(slot.clone());
            env.effects.push(UiEffect::Lobby(LobbyAction::Row {
                row,
                nation: slot.nation,
                team: slot.team,
                handicap: slot.handicap,
                color: slot.color,
            }));
            1
        }
        code::QUERY_STATUS => {
            put(msg, "status", Param::Int(status(env, lobby, col, key)));
            1
        }
        _ => -1,
    }
}

/// `NAMEn`: code 0 type (without slot -1); code 2 15190 "Open" (0), for the human only his name (999), otherwise
/// 15191 ... 15194 (1 ... 4); code 4 set type (host only, status rule): Open -> handicap 0, ready 0, team = row;
/// computer Easy 5/Normal 2/Hard 0/Manual 0 and ready 1, team of the first other computer (`FUN_004BA980`) - Open
/// on a human is the kick; code 5 status rule.
fn name(msg: &mut ElementMsg, env: &mut ScreenEnv, lobby: &LobbyView, key: i32) -> i32 {
    match msg.code {
        code::QUERY_VALUE => {
            put(msg, "int", Param::Int(env.data.draft.kind_of(key)));
            1
        }
        code::QUERY_ITEMS => {
            let mut items = vec![text_of(env.texts, OPEN_TEXT)];
            let mut values = vec![0];
            if env.data.draft.kind_of(key) == HUMAN {
                items.push(
                    env.data
                        .draft
                        .slot(key)
                        .map_or_else(String::new, |s| s.name.clone()),
                );
                values.push(HUMAN);
            } else {
                for kind in 1..=4 {
                    items.push(text_of(env.texts, OPEN_TEXT + kind));
                    values.push(kind);
                }
            }
            put(msg, "array", Param::Array(items));
            put(msg, "indexarray", Param::IndexArray(values));
            1
        }
        code::ITEM_SELECTED => {
            let (Some(kind), Some(mut slot), Some(row)) =
                (int(msg), env.data.draft.slot(key).cloned(), row_of(key))
            else {
                return 0;
            };
            if status(env, lobby, Column::Name, key) != 0 {
                return 0;
            }
            // 999 (the human keeps his name) and unknown types change nothing.
            if !(0..=4).contains(&kind) || (slot.kind == HUMAN && kind != 0) {
                return 1;
            }
            slot.kind = kind;
            let ready = kind != 0;
            if kind == 0 {
                slot.handicap = 0;
                slot.team = i32::from(row);
            } else {
                slot.handicap = match kind {
                    1 => 5,
                    2 => 2,
                    _ => 0,
                };
                join_first_computer(&env.data.draft, &mut slot);
            }
            env.data.draft.set_slot(slot);
            if let Some(l) = lobby_mut(env) {
                l.ready[usize::from(row)] = ready;
            }
            env.effects
                .push(UiEffect::Lobby(LobbyAction::Kind { row, kind }));
            1
        }
        code::QUERY_STATUS => {
            put(
                msg,
                "status",
                Param::Int(status(env, lobby, Column::Name, key)),
            );
            1
        }
        _ => -1,
    }
}
