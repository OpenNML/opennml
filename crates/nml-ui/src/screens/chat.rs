//! Chat windows `CHATWINDOW`, `CHATWINDOW2`, `CHATWINDOW3` (`chatwindow*.dlg`, handler `FUN_004CFF80`; re/game-ui
//! D5.7).
//!
//! The three frames are built alike (input field `INPUT`, button `SEND`, title 15790/15791/15792) and differ only
//! in the receivers: window 1 all, 2 the friends of the sender, 3 the chat mask (`FUN_004B9940`). The
//! window sets **no** menu state; the match continues. It is opened via [`show`]
//! (`showchatwindow(k)`, `FUN_0042EA20`). The host builds the command `0x5D` from [`UiEffect::SendChat`].

use super::{GameInfo, ScreenEnv, close};
use crate::menu::AppQueue;
use crate::msg::{AppCmd, ElementMsg, Param, SharedText, UiEffect, code};

/// Buffer size of the input field (`size 0x50`, `0x004CFFF0`): at most 79 characters.
pub const INPUT_SIZE: i32 = 0x50;
/// The frames by window number 1-3 (dialog names lowercase).
pub const DIALOGS: [&str; 3] = ["chatwindow", "chatwindow2", "chatwindow3"];
/// Key ENTER or ESC in the parameter `key` of code `0xB`.
const KEY_RETURN: i32 = 13;
const KEY_ESCAPE: i32 = 27;

/// `showchatwindow(k)` (`FUN_0042EA20`): only with a match, at menu state 0 (`Engine+0x2CCC`) and outside the
/// menu level (game type != -2, `Engine+0x108`); the host checks the cinemascope (`Engine+0x104`) beforehand
/// (`InputGate::chat`). Then the input buffer is cleared (`wcscpy(App+0x1418, L"")`, also for `k` not in 1...3) and for
/// `k` = 1...3 `{6, "chatwindow..."}` is queued. `true` if a window is queued. An already open window
/// takes the keys itself; ENTER then does not reach the host.
pub(crate) fn show(
    window: u8,
    game: Option<&GameInfo>,
    game_menu: u8,
    buffer: &SharedText,
    queue: &mut AppQueue,
) -> bool {
    let Some(game) = game else {
        return false;
    };
    if game_menu != 0 || game.game_type == -2 {
        return false;
    }
    buffer.set_units(Vec::new());
    let Some(name) = window_name(window) else {
        return false;
    };
    queue.push(AppCmd::Open {
        name: name.to_string(),
        wait: false,
    });
    true
}

/// Frame name for window 1-3.
pub fn window_name(window: u8) -> Option<&'static str> {
    DIALOGS.get(usize::from(window).checked_sub(1)?).copied()
}

/// Window number for a frame name (lowercase).
fn window_of(dialog: &str) -> Option<u8> {
    DIALOGS
        .iter()
        .position(|&d| d == dialog)
        .map(|i| i as u8 + 1)
}

/// Sending `FUN_004B9940(window)`: with an empty buffer (`wcslen == 0`) nothing; otherwise the effect
/// [`UiEffect::SendChat`] (the host builds `{0x5D, slot, mask, len, characters...}`) and clear the buffer
/// (`*buf = 0`).
fn send(window: u8, env: &mut ScreenEnv) {
    let units = env.state.chat_text.units();
    if units.is_empty() {
        return;
    }
    env.effects.push(UiEffect::SendChat {
        window,
        text: String::from_utf16_lossy(&units),
    });
    env.state.chat_text.set_units(Vec::new());
}

/// Handler `FUN_004CFF80`:
/// - `input` code 7: pointer to the buffer `App+0x1418` ([`super::ScreenState::chat_text`]), `size 0x50`,
///   `inputbuffer 1`; return value 1.
/// - `input` code `0xB`: without `key` 0; `key 13` sends according to the frame name and closes (`{4, frame}`),
///   `key 27` only closes, both 1; otherwise -1 (the field inserts the character).
/// - `send` code 8: send and close, 1.
/// - everything else -1.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let Some(window) = window_of(&msg.dialog) else {
        return -1;
    };
    let dialog = msg.dialog.clone();
    match (msg.element.as_str(), msg.code) {
        ("input", code::QUERY_TEXT) => {
            msg.params
                .insert("text".into(), Param::Shared(env.state.chat_text.clone()));
            msg.params.insert("size".into(), Param::Int(INPUT_SIZE));
            msg.params.insert("inputbuffer".into(), Param::Int(1));
            1
        }
        ("input", code::EDIT_KEY) => {
            let Some(Param::Int(key)) = msg.params.get("key").cloned() else {
                return 0;
            };
            match key {
                KEY_RETURN => send(window, env),
                KEY_ESCAPE => {}
                _ => return -1,
            }
            env.queue.push(close(&dialog, false));
            1
        }
        ("send", code::TRIGGERED) => {
            send(window, env);
            env.queue.push(close(&dialog, false));
            1
        }
        _ => -1,
    }
}
