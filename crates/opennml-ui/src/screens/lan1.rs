//! Search list `LAN1` (`FUN_004D2FE0`, re/multiplayer-menus L.1.1).
//!
//! The host does the search itself (network session, `UiEffect::Search`); the list comes in every frame via
//! `MenuApp::set_net` ([`crate::draft::NetMenuView::games`]). Deviation E-300: the selection hangs on the session ID
//! ([`super::MenuData::lan_choice`]) instead of the list index - if a game disappears before the selected one, the
//! highlight stays on the selected game instead of silently jumping to the next; if the selected one disappears, the
//! selection is empty and `JOIN` is hidden. The display stays as in the original: one row per game with the session
//! name.

use super::{ScreenEnv, close, fill_net_name, open, requery};
use crate::msg::{AppCmd, ElementMsg, Param, UiEffect, code};

/// Status "hidden" (`HIDE`).
const HIDE: i32 = 0x1000_0000;
/// Size of `PLAYERNAME` (`size 0x1E`, `0x004D3272`...`0x004D32D8`).
const NAME_SIZE: i32 = 30;
/// Query interval of the list `OPENGAMES` (`timeout 1000`, `0x004D333E`).
const LIST_TIMEOUT: i32 = 1000;

/// The session IDs of the list in display order.
fn instances(env: &ScreenEnv) -> Vec<u64> {
    env.data
        .net
        .as_ref()
        .map(|n| n.games.iter().map(|g| g.instance).collect())
        .unwrap_or_default()
}

/// Index of the selected session in the list, otherwise `None` (no selection or disappeared).
fn chosen_index(env: &ScreenEnv) -> Option<i32> {
    let choice = env.data.lan_choice?;
    instances(env)
        .iter()
        .position(|&i| i == choice)
        .and_then(|i| i32::try_from(i).ok())
}

/// `NetName` is empty (`wcslen == 0`).
fn name_empty(env: &ScreenEnv) -> bool {
    env.config.net_name[0] == 0
}

/// Join like a double-click (`0x004D3478`...`0x004D3505`): abort the search (`FUN_004E8510`), `{3,"lan1"}`, `{7}`,
/// flag "join in progress".
fn join(env: &mut ScreenEnv, dialog: &str) {
    env.effects.push(UiEffect::Search(false));
    env.queue.push(close(dialog, true));
    env.queue.push(AppCmd::Join);
    env.state.lan_joining = true;
}

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let dialog = msg.dialog.clone();
    match (msg.element.as_str(), msg.code) {
        // `0x004D3058`...`0x004D3076`: start the search (`FUN_004E8500` -> `FUN_004EB2E0`, the host starts it only if
        // it is not running), selection -1, flag "join in progress" 0.
        ("lan1", code::QUERY_STATUS) => {
            env.effects.push(UiEffect::Search(true));
            env.data.lan_choice = None;
            env.state.lan_joining = false;
            1
        }
        // `0x004D3272`...`0x004D32D8`: pointer to `NetName`, `size 30`, `inputbuffer 1`; no character filter (no code
        // `0xB`, `[`/`]` allowed).
        ("playername", code::QUERY_TEXT) => {
            let text = fill_net_name(env);
            msg.params.insert("text".into(), Param::Shared(text));
            msg.params.insert("size".into(), Param::Int(NAME_SIZE));
            msg.params.insert("inputbuffer".into(), Param::Int(1));
            1
        }
        // `0x004D3155`...`0x004D3229`: `join` and `create` are queried again at once with code 5 (the mirror has
        // already written `NetName`, [`super::sync_mirrors`]).
        ("playername", code::EDIT_CHANGED) => {
            requery(env, &dialog, "join", code::QUERY_STATUS);
            requery(env, &dialog, "create", code::QUERY_STATUS);
            1
        }
        ("opengames", code::QUERY_TIMEOUT) => {
            msg.params
                .insert("timeout".into(), Param::Int(LIST_TIMEOUT));
            1
        }
        // `0x004D334F`...`0x004D33F2`: the names of the list (the host has already removed expired entries,
        // `FUN_004E9690`); if the selected session is no longer in it, the selection is empty; `join` is queried again
        // with code 5.
        ("opengames", code::QUERY_ITEMS) => {
            let names = env
                .data
                .net
                .as_ref()
                .map(|n| n.games.iter().map(|g| g.name.clone()).collect())
                .unwrap_or_default();
            msg.params.insert("array".into(), Param::Array(names));
            if chosen_index(env).is_none() {
                env.data.lan_choice = None;
            }
            requery(env, &dialog, "join", code::QUERY_STATUS);
            1
        }
        // `0x004D350A`...`0x004D3542`: `int` = selection; if the copy fails (`FUN_004E8B50`), selection -1 and return
        // value -1 without `int`. Here: index of the selected session.
        ("opengames", code::QUERY_VALUE) => match chosen_index(env) {
            Some(i) => {
                msg.params.insert("int".into(), Param::Int(i));
                1
            }
            None => {
                env.data.lan_choice = None;
                -1
            }
        },
        // `0x004D33FC`...`0x004D3473`: selection = `int`, copy of the session (fails -> -1); `join` is queried again
        // with code 5.
        ("opengames", code::ITEM_SELECTED) => {
            let Some(Param::Int(i)) = msg.params.get("int") else {
                return 0;
            };
            env.data.lan_choice = usize::try_from(*i)
                .ok()
                .and_then(|i| instances(env).get(i).copied());
            requery(env, &dialog, "join", code::QUERY_STATUS);
            1
        }
        // `0x004D3478`...`0x004D3505`: selection, name not empty, flag 0 -> join.
        ("opengames", code::DOUBLE_CLICK) => {
            if chosen_index(env).is_some() && !name_empty(env) && !env.state.lan_joining {
                join(env, &dialog);
            }
            1
        }
        // `0x004D35DC`...`0x004D360E`: hidden without a selection or with an empty name.
        ("join", code::QUERY_STATUS) => {
            let hidden = chosen_index(env).is_none() || name_empty(env);
            msg.params
                .insert("status".into(), Param::Int(if hidden { HIDE } else { 0 }));
            1
        }
        // `0x004D356F`...`0x004D35D2`: selection and flag 0 -> like a double-click (name unchecked; with an empty name
        // `JOIN` is hidden and unreachable, correction E-279).
        ("join", code::TRIGGERED) => {
            if chosen_index(env).is_some() && !env.state.lan_joining {
                join(env, &dialog);
            }
            1
        }
        // `0x004D3689`: hidden with an empty name.
        ("create", code::QUERY_STATUS) => {
            let status = if name_empty(env) { HIDE } else { 0 };
            msg.params.insert("status".into(), Param::Int(status));
            1
        }
        // `0x004D363B`...`0x004D3684`: abort the search, `{3,"lan1"}`, `{5,"lan3"}`.
        ("create", code::TRIGGERED) => {
            env.effects.push(UiEffect::Search(false));
            env.queue.push(close(&dialog, true));
            env.queue.push(open("lan3"));
            1
        }
        // `0x004D30A1`...`0x004D311F`: abort the search, `{3,"lan1"}`, `{5,"start"}`, `{9}`.
        ("cancel", code::TRIGGERED) => {
            env.effects.push(UiEffect::Search(false));
            env.queue.push(close(&dialog, true));
            env.queue.push(open("start"));
            env.queue.push(AppCmd::CloseNet);
            1
        }
        _ => -1,
    }
}
