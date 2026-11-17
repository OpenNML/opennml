//! Credits `CREDITS` (`FUN_004DB920`, called at `0x004DCDB2`; re/menu-flow, `CREDITS`). `OK` is
//! also triggered by ESC (`CHECKESCAPE`) and at the end of the credits by the broadcast `0x17E` (O-105).

use super::{ScreenEnv, close, open};
use crate::msg::{ElementMsg, code};

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    if msg.code == code::TRIGGERED && msg.element == "ok" {
        // `0x004DB920`...`0x004DB990`: `{3, dialog name}`, `{5,"start"}`.
        env.queue.push(close(&msg.dialog, true));
        env.queue.push(open("start"));
        return 1;
    }
    -1
}
