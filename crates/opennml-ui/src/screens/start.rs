//! Main menu `START` (`FUN_004D16D0`, call `0x004DC41C`; re/menu-flow, `START`, section "Transitions", main menu).
//! Music and background are controlled by the frame (`MUSIC`, `BACKGROUND`), not by the handler.

use super::{ScreenEnv, ask, close, open, set_net_name};
use crate::msg::{AppCmd, ElementMsg, Param, UiEffect, code};
use crate::saves::SaveDir;

/// Status "locked" (`DISABLE`, `0x20000000`).
const DISABLE: i32 = 0x2000_0000;

/// Confirmation prompt "Quit the program and start GameSpy Arcade?".
const GAMESPY_QUESTION: i32 = 15117;

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.code, msg.element.as_str()) {
        // `0x004D1895`...`0x004D18E1`: locked without a GameSpy Arcade path (`GameApp+0x198E == 0`).
        (code::QUERY_STATUS, "gamespy") => {
            let status = if env.game_spy_installed { 0 } else { DISABLE };
            msg.params.insert("status".into(), Param::Int(status));
            1
        }
        (code::TRIGGERED, element) => {
            match element {
                // `0x004D1747`: `PostQuitMessage(0)` at once, without a prompt and without a command (O-30).
                "quit" => env.effects.push(UiEffect::Quit),
                // `0x004D1764`...`0x004D17B9`: `{3,"start"}`, directory `savegame` (`FUN_004BFC40(1)`, in the
                // reimplementation [`super::ScreenState::save_dir`] = `Main`, O-150); without an active profile
                // `{5,"selectprofile"}`, otherwise `NetName` = profile name (`FUN_0040C060`) and
                // `{5,"singleplayermenu"}`.
                "singleplayermenu" => {
                    env.state.save_dir = SaveDir::Main;
                    env.queue.push(close("start", true));
                    if env.data.profiles.file.current < 0 {
                        env.queue.push(open("selectprofile"));
                    } else {
                        let name = env.data.profiles.current_name_units();
                        set_net_name(env, &name);
                        env.queue.push(open("singleplayermenu"));
                    }
                }
                // `0x004D17DA`...`0x004D17FF`: confirmation prompt with OK `{0x36}`, without a cancel command.
                "gamespy" => ask(
                    env.state,
                    env.queue,
                    GAMESPY_QUESTION,
                    Some(AppCmd::GameSpy),
                    None,
                ),
                // `0x004D1826`...`0x004D1855`: every other button (`lan1`, `options`, `credits`, also unknown ones).
                other => {
                    env.queue.push(close("start", true));
                    env.queue.push(open(other));
                }
            }
            1
        }
        _ => -1,
    }
}
