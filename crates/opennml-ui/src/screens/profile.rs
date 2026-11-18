//! Profile selection `SELECTPROFILE` (`FUN_004DB0D0`, dispatcher `0x004DCC22`; re/singleplayer-menus part 1,
//! table 1.1).
//!
//! The selection of the list is a static (`DAT_00671F04`, [`super::ScreenState::profile_selection`]): `PROFILENAME`
//! code 5 sets it to the active profile on construction, before the list and the buttons query (file order). `OK` and
//! `DELETE` are hidden without a selection (`HIDE`, not `DISABLE`). The field `PROFILENAME` edits `NetName` directly
//! (O-28): typing + `CANCEL` leaves the typed name behind (O-122). ESC and ENTER have no effect (no
//! `CHECKESCAPE`/`CHECKENTER` in the file).

use super::{ScreenEnv, ask, close, fill_net_name, open, requery, set_net_name, tell};
use crate::msg::{AppCmd, ElementMsg, Param, UiEffect, code};
use crate::profiles::name_text;

/// Status "hidden" (`HIDE`).
const HIDE: i32 = 0x1000_0000;
/// Size of the name field (`size 30`, `0x004DB19A`...`0x004DB202`).
const NAME_SIZE: i32 = 30;
/// Confirmation prompt before deleting (`FUN_004BCD70`, `0x004DB699`...`0x004DB6CC`).
const DELETE_QUESTION: i32 = 15747;
/// Message "Profile name already exists ..." (`FUN_004BCF10`, `0x004DB4AE`...`0x004DB634`).
const NAME_EXISTS: i32 = 15748;

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let dialog = msg.dialog.clone();
    match (msg.element.as_str(), msg.code) {
        // `0x004DB140`...`0x004DB14D`: Sel = cur (`FUN_004BA580` -> `FUN_0040BC00`), no `status`, return value -1.
        ("profilename", code::QUERY_STATUS) => {
            env.state.profile_selection = env.data.profiles.file.current;
            -1
        }
        // `0x004DB19A`...`0x004DB202`: pointer to `NetName`, `size 30`, `inputbuffer 1`; no character filter (O-125).
        ("profilename", code::QUERY_TEXT) => {
            let text = fill_net_name(env);
            msg.params.insert("text".into(), Param::Shared(text));
            msg.params.insert("size".into(), Param::Int(NAME_SIZE));
            msg.params.insert("inputbuffer".into(), Param::Int(1));
            1
        }
        // `0x004DB351`...`0x004DB35F`: `int` = Sel.
        ("profilelist", code::QUERY_VALUE) => {
            let sel = env.state.profile_selection;
            msg.params.insert("int".into(), Param::Int(sel));
            1
        }
        // `0x004DB30D`...`0x004DB347` -> `FUN_0040C010`: the names in order of creation.
        ("profilelist", code::QUERY_ITEMS) => {
            let names = env
                .data
                .profiles
                .file
                .profiles
                .iter()
                .map(|p| name_text(&p.name))
                .collect();
            msg.params.insert("array".into(), Param::Array(names));
            1
        }
        // `0x004DB237`...`0x004DB308`: Sel = `int`, query `ok` and `delete` again; 0 without `int`.
        ("profilelist", code::ITEM_SELECTED) => {
            let Some(Param::Int(sel)) = msg.params.get("int") else {
                return 0;
            };
            env.state.profile_selection = *sel;
            requery(env, &dialog, "ok", code::QUERY_STATUS);
            requery(env, &dialog, "delete", code::QUERY_STATUS);
            1
        }
        // `0x004DB39C`...`0x004DB3B5`, `0x004DB65D`...`0x004DB678`: hidden without a selection.
        ("ok" | "delete", code::QUERY_STATUS) => {
            let status = if env.state.profile_selection >= 0 {
                0
            } else {
                HIDE
            };
            msg.params.insert("status".into(), Param::Int(status));
            1
        }
        // `0x004DB3F6`...`0x004DB432`: `FUN_004C83F0(Sel)` (active, name to `NetName`, `profiles.ini`), `{3}`,
        // `{5,"singleplayermenu"}`.
        ("ok", code::TRIGGERED) => {
            env.data.profiles.select(env.state.profile_selection);
            let name = env.data.profiles.current_name_units();
            set_net_name(env, &name);
            env.effects.push(UiEffect::WriteProfiles);
            env.queue.push(close(&dialog, true));
            env.queue.push(open("singleplayermenu"));
            1
        }
        // `0x004DB45C`...`0x004DB496`: `{3}`; without an active profile `{5,"start"}`,
        // otherwise `{5,"singleplayermenu"}`.
        ("cancel", code::TRIGGERED) => {
            env.queue.push(close(&dialog, true));
            let target = if env.data.profiles.file.current < 0 {
                "start"
            } else {
                "singleplayermenu"
            };
            env.queue.push(open(target));
            1
        }
        ("create", code::TRIGGERED) => create(&dialog, env),
        // `0x004DB699`...`0x004DB6CC`: confirmation prompt with OK `{0x29, Sel}` (Sel copied on the
        // click), without a cancel command.
        ("delete", code::TRIGGERED) => {
            let sel = env.state.profile_selection;
            ask(
                env.state,
                env.queue,
                DELETE_QUESTION,
                Some(AppCmd::DeleteProfile(sel)),
                None,
            );
            1
        }
        _ => -1,
    }
}

/// `create` code 8 (`0x004DB4AE`...`0x004DB634`): empty `NetName` -> -1 without a message (O-123); otherwise
/// `FUN_004C8430(&Sel)`: failure -> message 15748; success -> write `profiles.ini`, Sel = new index, query again
/// `profilelist` code 2, `ok` and `delete` code 5. The field shows a name shortened by `[RD]` at once (same buffer,
/// O-27).
fn create(dialog: &str, env: &mut ScreenEnv) -> i32 {
    if env.config.net_name[0] == 0 {
        return -1;
    }
    let mut name = env.config.net_name;
    let result = env.data.profiles.create(&mut name, &env.data.campaigns);
    set_net_name(env, &name);
    match result {
        Err(()) => tell(env.state, env.queue, NAME_EXISTS, None),
        Ok(index) => {
            // In the original `FUN_004C8430` writes the file before `FUN_004B9860(0)`; the host writes after the frame,
            // so already with campaign 0 and the new entry (a difference only in case of a crash in between).
            env.effects.push(UiEffect::WriteProfiles);
            env.state.profile_selection = i32::try_from(index).unwrap_or(i32::MAX);
            requery(env, dialog, "profilelist", code::QUERY_ITEMS);
            requery(env, dialog, "ok", code::QUERY_STATUS);
            requery(env, dialog, "delete", code::QUERY_STATUS);
        }
    }
    1
}
