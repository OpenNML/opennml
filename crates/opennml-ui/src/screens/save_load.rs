//! Saving and loading (re/ingame-menus 3): the shared handler `FUN_004D6100` for `SAVEGAME` (`save.dlg`) and
//! `LOADSAVEDGAME2` (`load2.dlg`, from the single player menu) and `FUN_004D9830` for `INGAMELOADGAME`
//! (`ingameload.dlg`, from the game menu and on quickloading). The commands `0x0A`, `0x26`, `0x0B`, `0x2A` are
//! executed by `MenuApp`.
//!
//! `LOADSAVEDNETWORKGAME` (network) shares `FUN_004D6100`: `dir` code 0 returns `int = 0` there, the list comes from
//! the current savegame directory ([`crate::saves::SaveDir`], `FUN_004BFC40`, O-150). Not reimplemented (O-24):
//! `LOADSAVEDGAME` (`load.dlg`, only from the unreachable `KAMPAGNEN`) and the dead buttons `load2` (-> `{0x12}`, which
//! the network host reaches via `LAN3 CREATE`) and `cancel2` in the network frame.

use super::{ScreenEnv, ask, close, open, requery};
use crate::msg::{AppCmd, ElementMsg, Param, code};
use crate::saves::{default_save_name, display_name};

/// Status "hidden" (`HIDE`).
const HIDE: i32 = 0x1000_0000;
/// `size` of `FILENAME` in `SAVEGAME` (`0x28`, `0x004D61F2`...`0x004D6255`).
const NAME_SIZE: i32 = 0x28;
/// `size` of the dead `FILENAME` in `INGAMELOADGAME` (`0x104`).
const DEAD_NAME_SIZE: i32 = 0x104;
/// Confirmation prompt "Really delete the savegame?".
const ASK_DELETE: i32 = 15174;
/// Confirmation prompt "Really overwrite the savegame?".
const ASK_OVERWRITE: i32 = 15785;
/// Characters that `FILENAME` discards (table `0x004D6A5C`): `" * . / : < > ? \ |`.
const RESERVED: [char; 10] = ['"', '*', '.', '/', ':', '<', '>', '?', '\\', '|'];

/// `FUN_004D6100` (`SAVEGAME`, `LOADSAVEDGAME2`, `LOADSAVEDNETWORKGAME`; flag `DAT_00671EE5` =
/// [`super::ScreenState::save_selected`]).
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let frame = msg.dialog.clone();
    let c = msg.code;
    match msg.element.as_str() {
        "filename" => match c {
            // `0x004D61F2`...`0x004D6255`: pointer to `GameApp+0x177C`, `size 40`, `inputbuffer 1`.
            code::QUERY_TEXT => {
                name_buffer(msg, env, NAME_SIZE);
                1
            }
            // `0x004D617E`...`0x004D61ED`: reserved characters -> 1 (discard); otherwise and without `key` -1 (also
            // RETURN and ESC: Enter does not save via the field, ESC goes to `CANCEL3`).
            code::EDIT_KEY => match msg.params.get("key") {
                Some(Param::Int(k))
                    if u32::try_from(*k)
                        .ok()
                        .and_then(char::from_u32)
                        .is_some_and(|ch| RESERVED.contains(&ch)) =>
                {
                    1
                }
                _ => -1,
            },
            // `0x004D6A11`: other codes 0.
            _ => 0,
        },
        "dir" => match c {
            // `0x004D650C`...`0x004D6541`: return value 1 without `int`; in the network frame `LOADSAVEDNETWORKGAME`
            // `int = 0`.
            code::QUERY_VALUE => {
                if frame == "loadsavednetworkgame" {
                    msg.params.insert("int".into(), Param::Int(0));
                }
                1
            }
            // `0x004D62B7`...`0x004D630E`: flag 0, fill the list; the count is never negative, so always
            // `FUN_004BC3E0(0)` (newest name, no effect with an empty list) and flag 1 (O-144).
            code::QUERY_ITEMS => {
                env.state.save_selected = false;
                fill_list(msg, env);
                take_entry(env, 0);
                env.state.save_selected = true;
                1
            }
            // `0x004D6313`...`0x004D6343`: free the entries.
            code::FREE_ITEMS => 1,
            // `0x004D6348`...`0x004D6502`: `int >= 0` -> take over, flag = `int >= 0`; query code 5 again at `load`,
            // `load4`, `load2`, `deletefile`. 0 without `int`.
            code::ITEM_SELECTED => {
                let Some(i) = int_param(msg) else {
                    return 0;
                };
                take_entry(env, i);
                env.state.save_selected = i >= 0;
                for element in ["load", "load4", "load2", "deletefile"] {
                    requery(env, &frame, element, code::QUERY_STATUS);
                }
                1
            }
            // `0x004D6546`...`0x004D65C9`: load only with the flag and only in `loadsavedgame`/`loadsavedgame2`; no
            // effect in `SAVEGAME` (O-148). Return value 1.
            code::DOUBLE_CLICK => {
                if env.state.save_selected && frame == "loadsavedgame2" {
                    load(env, &frame);
                }
                1
            }
            _ => 0,
        },
        // `0x004D663B`, `0x004D66A3`: `HIDE` without the flag, otherwise 0.
        "load" | "load4" | "deletefile" if c == code::QUERY_STATUS => {
            let selected = env.state.save_selected;
            put_status(msg, selected);
            1
        }
        // `0x004D65F2`...`0x004D6623`: `{3, frame}`, `{0x0A}`.
        "load" | "load4" if c == code::TRIGGERED => {
            load(env, &frame);
            1
        }
        // `0x004D66C5`...`0x004D66F4`: confirmation prompt 15174, OK `{0x2A, frame name}`, without a cancel command
        // (O-145: `GameApp+0x177C` is deleted, so in `SAVEGAME` the typed name).
        "deletefile" if c == code::TRIGGERED => {
            ask_delete(env, &frame);
            1
        }
        // `0x004D6742`...`0x004D6813`: empty name -> default name (`FUN_004BC710`); `{4, frame}`; file exists
        // (`FUN_004C6B30`) -> confirmation prompt 15785 (OK `{0x26}`, cancel `{5, frame}`), otherwise `{0x26}`. The
        // return command stays (O-146).
        "save" if c == code::TRIGGERED => {
            if env.state.save_name.units().is_empty() {
                let name = default_save_name(env.data.saves.now_local());
                env.state.save_name.set(&name);
            }
            env.queue.push(close(&frame, false));
            let guid = active_guid(env);
            if env
                .data
                .saves
                .exists(env.state.save_dir, guid, &env.state.save_name.get())
            {
                ask(
                    env.state,
                    env.queue,
                    ASK_OVERWRITE,
                    Some(AppCmd::SubmitSave),
                    Some(open(&frame)),
                );
            } else {
                env.queue.push(AppCmd::SubmitSave);
            }
            1
        }
        // `0x004D6877`...: `{3, frame}`, `{5,"singleplayermenu"}`.
        "cancel4" if c == code::TRIGGERED => {
            env.queue.push(close(&frame, true));
            env.queue.push(open("singleplayermenu"));
            1
        }
        // `0x004D68AA`...`0x004D6924`: `{3, frame}`; return command -> queue and free, otherwise
        // `{5,"ingameoptions"}`.
        "cancel3" if c == code::TRIGGERED => {
            env.queue.push(close(&frame, true));
            let back = env
                .state
                .return_cmd
                .take()
                .unwrap_or_else(|| open("ingameoptions"));
            env.queue.push(back);
            1
        }
        // `0x004D6A36`: otherwise 0.
        _ => 0,
    }
}

/// `FUN_004D9830` (`INGAMELOADGAME`; flag `DAT_00671F00` = [`super::ScreenState::load_selected`]).
pub(super) fn handle_ingame(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let frame = msg.dialog.clone();
    let c = msg.code;
    match (msg.element.as_str(), c) {
        // Dead `filename` (no such element in `ingameload.dlg`): pointer, `size 0x104`, `inputbuffer 1`.
        ("filename", code::QUERY_TEXT) => {
            name_buffer(msg, env, DEAD_NAME_SIZE);
            1
        }
        // Jump table `0x004D9D1C` case 0: return value 1 without `int`.
        ("dir", code::QUERY_VALUE) => 1,
        // `0x004D9967`...`0x004D999E`: flag 0, fill the list, **no** preselection.
        ("dir", code::QUERY_ITEMS) => {
            env.state.load_selected = false;
            fill_list(msg, env);
            1
        }
        ("dir", code::FREE_ITEMS) => 1,
        // `0x004D99D8`...`0x004D9ADB`: take over, flag = `int >= 0`; query code 5 again at `load`, `deletefile`.
        ("dir", code::ITEM_SELECTED) => {
            let Some(i) = int_param(msg) else {
                return 0;
            };
            take_entry(env, i);
            env.state.load_selected = i >= 0;
            requery(env, &frame, "load", code::QUERY_STATUS);
            requery(env, &frame, "deletefile", code::QUERY_STATUS);
            1
        }
        // `0x004D9AF6`...`0x004D9B14`, `0x004D9C03`...`0x004D9C37`: `HIDE` without the flag, otherwise 0.
        ("deletefile" | "load", code::QUERY_STATUS) => {
            let selected = env.state.load_selected;
            put_status(msg, selected);
            1
        }
        // `0x004D9B4C`...`0x004D9B84`: confirmation prompt 15174, OK `{0x2A, frame name}`.
        ("deletefile", code::TRIGGERED) => {
            ask_delete(env, &frame);
            1
        }
        // `0x004D9BC0`...`0x004D9BFE`: `{3, frame}`, free the return command without queueing it, `{0x0A}` (no
        // confirmation prompt).
        ("load", code::TRIGGERED) => {
            env.state.return_cmd = None;
            load(env, &frame);
            1
        }
        // `0x004D9C4F`...`0x004D9CC1`: `{3, frame}`; return command -> queue and free, otherwise `{0x37}` (quickload
        // case: the game continues).
        ("cancel", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            let back = env
                .state
                .return_cmd
                .take()
                .unwrap_or(AppCmd::ResumeAfterLoadDialog);
            env.queue.push(back);
            1
        }
        // Everything else, also `dir` code `0xD` (a double-click does not load here, O-148): -1.
        _ => -1,
    }
}

/// GUID of the active profile (`FUN_0040BC40` reads `KV+0x7C`; without an active profile the initial value 0 or the
/// last set GUID is there - the reimplementation takes 0).
fn active_guid(env: &ScreenEnv) -> i32 {
    env.data.profiles.current_guid().unwrap_or(0)
}

/// Code 7 at `FILENAME`: pointer to the name buffer `GameApp+0x177C`, `size`, `inputbuffer 1`.
fn name_buffer(msg: &mut ElementMsg, env: &ScreenEnv, size: i32) {
    msg.params
        .insert("text".into(), Param::Shared(env.state.save_name.clone()));
    msg.params.insert("size".into(), Param::Int(size));
    msg.params.insert("inputbuffer".into(), Param::Int(1));
}

/// Fill the list (`FUN_004C63A0`, re/ingame-menus 3.1): savegames of the active profile, names to `GameApp+0xD6DC`,
/// display texts to `array`.
pub(super) fn fill_list(msg: &mut ElementMsg, env: &mut ScreenEnv) {
    let entries = env.data.saves.list(env.state.save_dir, active_guid(env));
    let shown = entries.iter().map(display_name).collect();
    env.state.save_entries = entries.into_iter().map(|e| e.name).collect();
    msg.params.insert("array".into(), Param::Array(shown));
}

/// `FUN_004BC3E0(i)`: `wcscpy(GameApp+0x177C, entry[i].name)`; no effect without an entry `i`.
pub(super) fn take_entry(env: &mut ScreenEnv, i: i32) {
    let name = usize::try_from(i)
        .ok()
        .and_then(|i| env.state.save_entries.get(i));
    if let Some(name) = name {
        env.state.save_name.set(name);
    }
}

/// `{3, frame}`, `{0x0A}`.
fn load(env: &mut ScreenEnv, frame: &str) {
    env.queue.push(close(frame, true));
    env.queue.push(AppCmd::LoadGame);
}

/// Confirmation prompt 15174 with OK `{0x2A, frame name}` (`FUN_004BCD70` with two arguments: no cancel command).
pub(super) fn ask_delete(env: &mut ScreenEnv, frame: &str) {
    ask(
        env.state,
        env.queue,
        ASK_DELETE,
        Some(AppCmd::DeleteSave(frame.to_string())),
        None,
    );
}

/// `status` = `HIDE` without the flag, otherwise 0.
pub(super) fn put_status(msg: &mut ElementMsg, selected: bool) {
    let status = if selected { 0 } else { HIDE };
    msg.params.insert("status".into(), Param::Int(status));
}

pub(super) fn int_param(msg: &ElementMsg) -> Option<i32> {
    match msg.params.get("int") {
        Some(Param::Int(v)) => Some(*v),
        _ => None,
    }
}
