//! End screens `GAMEEND_VICTORY` / `GAMEEND_DEFEAT` (`FUN_004D9030`, re/ingame-menus 6.3). They are opened by
//! `MenuApp::show_game_end` (step 4 of `GameEngine_SetPlayerResult` `0x00430CE0`); the host thereby reports the result
//! of the local player.
//!
//! `.dlg`: `END_VICTORY`/`END_DEFEAT` ("End current game"), `RESTART`, `LOAD` only in defeat; all with
//! `CHECKENTER`, none with `CHECKESCAPE`. The buttons `continue_victory`/`continue_defeat` are only known to the code
//! (code 5 hides them in the campaign, code 8 `{0x16}`), the files do not contain them; the reimplementation omits
//! them (O-25). `LOAD` and `RESTART` take effect in the campaign; in free play they are locked (O-179).

use super::{ScreenEnv, close, open};
use crate::msg::{AppCmd, ElementMsg, Param, code};

/// Status "locked" (`DISABLE`).
const DISABLE: i32 = 0x2000_0000;

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let frame = msg.dialog.clone();
    match (msg.element.as_str(), msg.code) {
        // `0x004D931B`...`0x004D935C`: `DISABLE` if `Engine+0x108 != -1` - no loading and no restart after the end of
        // the game in free play (O-179). Without a match the original reads a missing engine; the reimplementation
        // locks.
        ("load" | "restart", code::QUERY_STATUS) => {
            if env.data.game.is_none_or(|g| g.game_type != -1) {
                msg.params.insert("status".into(), Param::Int(DISABLE));
            }
            1
        }
        // `0x004D93B1`: other elements at code 5 -> 1 without `status`.
        (_, code::QUERY_STATUS) => 1,
        // `0x004D9091`...`0x004D90FA`, `0x004D911C`...`0x004D9174`: `{4, frame}`, `{0x15}`, `{0x0E}` -> accounting.
        // If the message about the end of the network match is pending
        // (`MenuApp::end_with_message`, "OK" = `{0x0E}`), the button has no effect; the message carries the only
        // `0x0E` (otherwise `{0x15}` would wait behind it, and the accounting would open under the message). No
        // counterpart in the original, which does not know this case.
        ("end_victory" | "end_defeat", code::TRIGGERED)
            if env.state.info_ok == Some(AppCmd::EndMatch) =>
        {
            log::info!(
                "{frame}: end has no effect, the message about the end of the network match is pending"
            );
            1
        }
        ("end_victory" | "end_defeat", code::TRIGGERED) => {
            env.queue.push(AppCmd::Close {
                name: frame,
                wait: false,
            });
            env.queue.push(AppCmd::WaitDialogs);
            env.queue.push(AppCmd::EndMatch);
            1
        }
        // `0x004D91E5`...`0x004D9232`: `{3, frame}`, `{5,"ingameloadgame"}`, return command `{5, frame}` (the state
        // stays 7); `INGAMELOADGAME CANCEL` queues it and opens the defeat screen again. The button
        // `LOAD` exists only in `GAMEEND_DEFEAT`; in free play it is locked (O-179), the response does not depend
        // on the game mode.
        ("load", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            env.queue.push(open("ingameloadgame"));
            env.state.return_cmd = Some(open(&frame));
            1
        }
        // `0x004D9255`...`0x004D9294`: `{4, frame}`, `{0x15}`, `{0x1F}` - the restart of the level; the end of the game
        // has already booked, the restart books nothing.
        ("restart", code::TRIGGERED) => {
            env.queue.push(AppCmd::Close {
                name: frame,
                wait: false,
            });
            env.queue.push(AppCmd::WaitDialogs);
            env.queue.push(AppCmd::ReloadLevel);
            1
        }
        // `0x004D9296`: other elements (also `continue_*`, O-25) and other codes -1.
        _ => -1,
    }
}
