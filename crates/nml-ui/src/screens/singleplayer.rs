//! Single player menu `SINGLEPLAYERMENU` (`FUN_004DA150`; re/singleplayer-menus part 1, table 1.3) and the list of
//! recordings `LOADAUTOPLAY` (`FUN_004D9D30`).

use super::save_load::{ask_delete, fill_list, int_param, put_status, take_entry};
use super::{ScreenEnv, close, fill_net_name, open, requery};
use crate::msg::{AppCmd, ElementMsg, Param, code};
use crate::saves::SaveDir;

/// Size of the name field (`size 30`, `0x004DA1B5`...`0x004DA226`).
const NAME_SIZE: i32 = 30;

/// `FUN_004DA150`: there is **no** code 5 branch - no button is locked or hidden (O-125); the field
/// `PROFILENAME` stays locked by the `DISABLE` of the file and shows `NetName` through the pointer.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        // `0x004DA1B5`...`0x004DA226`: pointer to `NetName`, `size 30`, `inputbuffer 1`.
        ("profilename", code::QUERY_TEXT) => {
            let text = fill_net_name(env);
            msg.params.insert("text".into(), Param::Shared(text));
            msg.params.insert("size".into(), Param::Int(NAME_SIZE));
            msg.params.insert("inputbuffer".into(), Param::Int(1));
            1
        }
        (element, code::TRIGGERED) => {
            // `0x004DA278`...`0x004DA435`: `{3, frame}`, then the target; `loadsavedgame` opens `loadsavedgame2`,
            // `loadautoplay` the button name (`{5, element name}`), `cancel` the main menu. The handler of
            // `LOADSAVEDGAME2` (`FUN_004D6100`) is in `save_load.rs`.
            let target = match element {
                "selectprofile" | "selectcampaign" | "createsingleplayergame" | "loadautoplay" => {
                    element
                }
                "loadsavedgame" => "loadsavedgame2",
                "cancel" => "start",
                _ => return -1,
            };
            env.queue.push(close(&msg.dialog, true));
            env.queue.push(open(target));
            1
        }
        _ => -1,
    }
}

/// `LOADAUTOPLAY` (`FUN_004D9D30`, re/singleplayer-menus 3). `DIR` code 2 sets the directory `savegame/autoplay`
/// (`FUN_004BFC40(3)`, [`SaveDir::Autoplay`]) and lists its files with the full name in the order of the host
/// (ascending by name, deviates from E-342), without preselection and with flag 0 (`DAT_00671F01`); code 4 selects
/// the row (`FUN_004BC3E0`), sets the flag and queries `START` and `DELETEFILE` again. Both are `HIDE` until then.
/// `START` also fires when hidden (`CHECKENTER`, O-186); the original then starts without a selection, the
/// reimplementation lets it have no effect (deviates from O-186). Otherwise `START` queues `{3}`, `{0x15}`, `{0x19}`,
/// `DELETEFILE` asks 15174 with OK `{0x2A}`, `CANCEL` resets the directory (`FUN_004BFC40(1)`).
pub(super) fn handle_autoplay(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let frame = msg.dialog.clone();
    match (msg.element.as_str(), msg.code) {
        // `0x004D9DA9` case 0: return value 1 without `int`.
        ("dir", code::QUERY_VALUE) => 1,
        // Case 2 (`0x004D9DB0`...`0x004D9DF1`): directory, list of the recordings; flag "selected" = 0.
        ("dir", code::QUERY_ITEMS) => {
            env.state.save_dir = SaveDir::Autoplay;
            env.state.autoplay_selected = false;
            fill_list(msg, env);
            1
        }
        // Case 3 (`0x004D9DF6`...`0x004D9E26`): free the list.
        ("dir", code::FREE_ITEMS) => 1,
        // Case 4 (`0x004D9E2B`...`0x004D9F21`): select a recording, flag = 1, query `START` and `DELETEFILE` again.
        ("dir", code::ITEM_SELECTED) => {
            let Some(i) = int_param(msg) else {
                return 0;
            };
            take_entry(env, i);
            env.state.autoplay_selected = true;
            requery(env, &frame, "start", code::QUERY_STATUS);
            requery(env, &frame, "deletefile", code::QUERY_STATUS);
            1
        }
        // `0x004D9F41`...`0x004D9F5A`, `0x004DA042`...`0x004DA05D`: `HIDE` without the flag, otherwise 0.
        ("start" | "deletefile", code::QUERY_STATUS) => {
            let selected = env.state.autoplay_selected;
            put_status(msg, selected);
            1
        }
        // `0x004D9F88`...`0x004D9FB7`: confirmation prompt 15174, OK `{0x2A, frame name}`, without a cancel command.
        ("deletefile", code::TRIGGERED) => {
            ask_delete(env, &frame);
            1
        }
        // `0x004D9FE2`...`0x004DA03D`: `{3, frame}`, `{0x15}`, `{0x19}`; no effect without a selection (O-186).
        ("start", code::TRIGGERED) => {
            if env.state.autoplay_selected {
                env.queue.push(close(&frame, true));
                env.queue.push(AppCmd::WaitDialogs);
                env.queue.push(AppCmd::PlayReplay);
            }
            1
        }
        // `0x004DA0A3`...`0x004DA0EE`: `FUN_004BFC40(1)`, `{3, frame}`, `{5,"singleplayermenu"}`.
        ("cancel", code::TRIGGERED) => {
            env.state.save_dir = SaveDir::Main;
            env.queue.push(close(&frame, true));
            env.queue.push(open("singleplayermenu"));
            1
        }
        _ => -1,
    }
}
