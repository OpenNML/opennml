//! Lobby chat `CHATFRAME` (`chatframe.dlg`, handler `FUN_004CFD10`, re/multiplayer-menus L.4).
//!
//! The frame belongs to the lobby (`FUN_004BCBE0` opens it with `LAN2` and `LEVELINFO`, [`super::open_lobby`]): `LABEL
//! CHAT` shows the history (`GameApp+0x9F0`, live via a pointer in the original), `EDITFIELD INPUT` the input buffer
//! (`GameApp+0x1418`, the same as in the chat windows of the match, `size 0x50`), `SEND` 15584 sends. Sending
//! (`FUN_004BC480`) hands the text to the host as [`UiEffect::Lobby`]; distribution, echo of the host and the lines of
//! the history are done by the network session, the finished lines ([`chat_line`]) come back via [`LobbyView::chat`].

use super::ScreenEnv;
use super::chat::INPUT_SIZE;
use crate::draft::{LobbyView, NetMenuView};
use crate::msg::{ElementMsg, LobbyAction, Param, UiEffect, code};

/// Lines of the history (`FUN_004B9BD0`: lines 0...3 <- 1...4, new line 4 = bottom).
pub const CHAT_LINES: usize = 5;
/// Maximum length of the text of a line at the receiver (`FUN_004B9BD0`: over 79 characters cut off at 79).
pub const CHAT_SHOWN: usize = 79;
/// Key ENTER in the parameter `key` of code `0xB`.
const KEY_RETURN: i32 = 13;
/// `[` and `]`: discarded (code `0xB` -> 1), so that no markup can be created through the keyboard.
const KEY_OPEN_BRACKET: i32 = b'[' as i32;
const KEY_CLOSE_BRACKET: i32 = b']' as i32;
/// Icons from `extrasymbols.txt`: 1 red, 2 base color, 6 end of line (`FUN_004B9A80`).
const RED: &str = "[COLOR FFFF0000]";
const DEFAULT_COLOR: &str = "[DEFAULTCOLOR]";
const CR: &str = "[CR]";

/// A line of the history (`FUN_004B9A80` per line with a name): icon 1 `[COLOR FFFF0000]` + name + `": "` + icon 2
/// `[DEFAULTCOLOR]` + text + `" "` + icon 6 `[CR]` - name red, text in base color. The text is cut to 79 characters
/// like at the receiver in the original (`FUN_004B9BD0`, E-310). Public for the network session of the host, which
/// fills [`LobbyView::chat`] with it.
pub fn chat_line(name: &str, text: &str) -> String {
    let text: String = text.chars().take(CHAT_SHOWN).collect();
    format!("{RED}{name}: {DEFAULT_COLOR}{text} {CR}")
}

/// The history that `CHAT` shows (`FUN_004B9A80`): five lines, the oldest on top, the newest at the bottom; missing
/// lines on top as an empty line `" [CR]"`. Empty without a lobby.
pub fn chat_text(net: Option<&NetMenuView>) -> String {
    let Some(lobby) = net.and_then(|n| n.lobby.as_ref()) else {
        return String::new();
    };
    lobby_chat_text(lobby)
}

fn lobby_chat_text(lobby: &LobbyView) -> String {
    let lines = &lobby.chat[lobby.chat.len().saturating_sub(CHAT_LINES)..];
    let mut out = format!(" {CR}").repeat(CHAT_LINES - lines.len());
    for line in lines {
        out.push_str(line);
    }
    out
}

/// Sending `FUN_004BC480`: with an empty buffer nothing; otherwise [`LobbyAction::Chat`] to the host and clear the
/// buffer.
fn send(env: &mut ScreenEnv) {
    let units = env.state.chat_text.units();
    if units.is_empty() {
        return;
    }
    env.effects.push(UiEffect::Lobby(LobbyAction::Chat(
        String::from_utf16_lossy(&units),
    )));
    env.state.chat_text.set_units(Vec::new());
}

/// Handler `FUN_004CFD10`:
/// - `chat` code 7: pointer to the history ([`super::ScreenState::lobby_chat`], refilled here from the state of the
///   host); 1.
/// - `input` code 7: pointer to the input buffer ([`super::ScreenState::chat_text`]), `size 0x50` (79 characters),
///   `inputbuffer 1`; 1.
/// - `input` code `0xB`: `key 13` sends, 1; `[` and `]` are discarded, 1; otherwise -1 (the field inserts the
///   character). Without timer.
/// - `send` code 8: send, 1.
/// - everything else -1.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        ("chat", code::QUERY_TEXT) => {
            let text = chat_text(env.data.net.as_ref());
            if env.state.lobby_chat.get() != text {
                env.state.lobby_chat.set(&text);
            }
            msg.params
                .insert("text".into(), Param::Shared(env.state.lobby_chat.clone()));
            1
        }
        ("input", code::QUERY_TEXT) => {
            msg.params
                .insert("text".into(), Param::Shared(env.state.chat_text.clone()));
            msg.params.insert("size".into(), Param::Int(INPUT_SIZE));
            msg.params.insert("inputbuffer".into(), Param::Int(1));
            1
        }
        ("input", code::EDIT_KEY) => match msg.params.get("key") {
            Some(Param::Int(KEY_RETURN)) => {
                send(env);
                1
            }
            Some(Param::Int(KEY_OPEN_BRACKET | KEY_CLOSE_BRACKET)) => 1,
            _ => -1,
        },
        ("send", code::TRIGGERED) => {
            send(env);
            1
        }
        _ => -1,
    }
}
