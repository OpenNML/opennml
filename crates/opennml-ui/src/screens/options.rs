//! Option frame `OPTIONS` (`FUN_004D3700`, call `0x004DC2E0`; re/menu-flow, `OPTIONS`). On the left the buttons of the
//! subpages, on the right the open subpage (flag [`super::ScreenState::options_page`]). `OK` has no `CHECKESCAPE`:
//! ESC has no effect here (O-103).

use super::{ScreenEnv, close, open};
use crate::msg::{ElementMsg, UiEffect, code};

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let page = &mut env.state.options_page;
    // `0x004D3791`...`0x004D37A1`: "frame opened" (code 5 with the frame name) clears the flag.
    if msg.code == code::QUERY_STATUS && msg.element == "options" {
        page.clear();
    }
    // `0x004D37DF`...`0x004D37F0`: every message of the open subpage is not handled - another click on it does
    // nothing (O-103).
    if msg.element == *page {
        return -1;
    }
    if msg.code != code::TRIGGERED {
        return -1;
    }
    if msg.element == "ok" {
        // `0x004D3809`...`0x004D38A8`: write `Engine.ini` **at once** (`FUN_00496D40`, synchronously before the menu
        // commands, O-103), then `{3, flag}` (also empty), `{4,"options"}`, flag = `"ok"`, `{5,"start"}`.
        env.effects.push(UiEffect::WriteSettings);
        env.queue.push(close(page, true));
        env.queue.push(close("options", false));
        *page = "ok".to_string();
        env.queue.push(open("start"));
    } else {
        // `0x004D386A`...`0x004D38A4`: switch the subpage.
        env.queue.push(close(page, true));
        *page = msg.element.clone();
        env.queue.push(open(page));
    }
    1
}
