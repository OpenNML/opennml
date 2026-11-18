//! Confirmation prompt `INFOFRAME` and message `MSGBOX` (`FUN_004D11F0`, call `0x004DCCB6`; re/menu-flow,
//! `INFOFRAME`/`MSGBOX`). Both are modal; `OK` has `CHECKENTER`, only `INFOFRAME` has `CANCEL` with `CHECKESCAPE` - in
//! `MSGBOX` ESC has no effect (O-103). Text and commands are set by [`super::ask`] or [`super::tell`], a free text by
//! [`super::tell_text`].

use super::{ScreenEnv, close};
use crate::msg::{ElementMsg, Param, code};

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        // `textinfo` code 7: text from `GameApp+0x1994` (`FUN_004BA6A0`, `FUN_0045F490`). The original opens both
        // dialogs only via `FUN_004BCD70`/`FUN_004BCF10`, so the text is set; without a text (opening via the control
        // channel) the reimplementation does not answer, the label keeps the placeholder of the file.
        ("textinfo", code::QUERY_TEXT) => {
            // Free text before the text ID.
            if let Some(text) = &env.state.info_free {
                msg.params.insert("text".into(), Param::Text(text.clone()));
                return 1;
            }
            let Some(id) = env.state.info_text else {
                return -1;
            };
            // Like the pointer from `FUN_0055FFE0`: text of the number, if it is missing the last formatted number
            // (O-4).
            env.texts.resolve(id);
            let text = env.texts.display(id);
            msg.params.insert("text".into(), Param::Text(text));
            1
        }
        // `ok`/`cancel` code 8: `{3, dialog name}`, then queue the remembered command (`FUN_004BCE90` or
        // `FUN_004BCED0`, if present) and free both copies. The original does not set the freed pointers to 0, a second
        // OK/CANCEL queues the command again; the reimplementation empties them with `take()` (O-111, fixed as a
        // deviation).
        ("ok", code::TRIGGERED) | ("cancel", code::TRIGGERED) => {
            env.queue.push(close(&msg.dialog, true));
            let state = &mut *env.state;
            let (ok, cancel) = (state.info_ok.take(), state.info_cancel.take());
            // A free text applies only to this message; the text ID stays as in the original.
            state.info_free = None;
            let chosen = if msg.element == "ok" { ok } else { cancel };
            if let Some(cmd) = chosen {
                env.queue.push(cmd);
            }
            1
        }
        _ => -1,
    }
}
