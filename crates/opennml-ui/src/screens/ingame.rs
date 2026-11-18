//! Game menu `INGAMEOPTIONS` (campaign) / `INGAMEOPTIONS3` (free play) (`FUN_004D7110`, calls `0x004DC606` and
//! `0x004DC632`; re/ingame-menus 2, return command 2.1). Opening and closing (`FUN_0042E150`, `FUN_0042E290`,
//! `FUN_0042F6B0`) live in `MenuApp` (re/ingame-menus 1).
//!
//! Covers `SAVEGAME`/`SAVEGAME3`, `LOAD`, `OPTIONS`, `QUIT`, `RETURN`; `QUITGAME` (surrender, `{0x27}`); `RESTART`,
//! `QUESTMENU` and `TIPLOGMENU` of the campaign edition together with the frames `QUESTMENU2` (`FUN_004D1FF0`) and
//! `TIPLOGMENU` (`FUN_004DB6F0`), whose content (mission objectives, tip log) is filled from [`MissionInfo`]
//! (re/campaign-ui D6.8, D6.9). The network edition `INGAMEOPTIONS2` (re/multiplayer-menus L.7.1, same handler) with
//! `SAVEGAME2`, `OPTIONS2`, `QUITGAME`, `QUIT` and `RETURN2` (`{0x13}`) opens only with a network role
//! ([`GameInfo::net`]). The directory change `FUN_004BFC40(1)` before `savegame`/`savegame3`/`load` (in the network
//! `FUN_004BFC40(2)` for `SAVEGAME2`, `savegame/multiplayer`) sets [`super::ScreenState::save_dir`] (O-150); the
//! dialogs `SAVEGAME` and `INGAMELOADGAME` are in `save_load.rs`.

use super::{ScreenEnv, ask, close, open};
use crate::assets::Texts;
use crate::draft::text_of;
use crate::msg::{AppCmd, ElementMsg, Param, code};
use crate::saves::SaveDir;

/// Status "locked" (`DISABLE`).
const DISABLE: i32 = 0x2000_0000;

/// Confirmation prompt "Really quit the program?".
const ASK_QUIT: i32 = 15783;

/// Confirmation prompt "Really end the current game?" (`QUITGAME`, also `RESTART` of the campaign).
const ASK_QUIT_GAME: i32 = 15780;

/// The running match, as far as the menus need to know it (the host sets it with `MenuApp::set_game`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameInfo {
    /// Game type of the match (`Engine+0x108`): -1 campaign, otherwise the game mode of free play.
    pub game_type: i32,
    /// Slot of the local player (`GameEngine_GetLocalPlayer` -> `Player_GetSlotIndex`), for `{0x27}` from
    /// `QUITGAME`.
    pub local_slot: u8,
    /// Input lock of the scripts (`Engine+0x600`, 18/19): locks "Save" in the game menu (re/campaign-ui D6.9) and
    /// switches the pointer off when the game menu and the load dialog close (O-371, O-379). The host sets it every
    /// frame (`MenuApp::set_ui_locked`).
    pub ui_locked: bool,
    /// Role in the network match (`NetSession_IsActive`, DirectPlay host `FUN_004E85F0`), `None` without a network:
    /// with a role ESC opens `INGAMEOPTIONS2` (state 2), and only the host sees the speed slider. The host sets it
    /// when the match starts (`GameFlow::started`, from the network session).
    pub net: Option<NetRoleView>,
    /// The match is a replay (`GameApp+0x19F3`): locks `savegame*` and `load` in the game menu. The host sets it
    /// when the replay starts (`GameFlow::started`).
    pub playback: bool,
}

/// Role of the machine in the network match, as far as the menus need to know it (`opennml-ui` does not know `opennml-net`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRoleView {
    /// The host of the session (in the original the DirectPlay host).
    Host,
    /// A client.
    Client,
}

/// The mission objectives and the tip log of the running match, as far as `QUESTMENU2` and `TIPLOGMENU` show them.
/// The host sets them every frame from the mission state of the simulation (`MenuApp::set_mission`); `opennml-ui` does
/// not know the simulation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissionInfo {
    /// The ten main objectives in slot order: (status, text ID). The original shows no secondary objectives (O-53).
    pub main_quests: [(i32, i32); 10],
    /// `m_InfoHistory` (`GE+4`): the text IDs of the tips and info windows, old -> new (re/campaign-ui D6.8).
    pub info_history: Vec<i32>,
}

/// Bit 4 of the objective status: fulfilled (status 6), set green (`FUN_004334E0`).
const QUEST_DONE: i32 = 4;
/// Bit 1 of the objective status: set (3) or new (1) (`FUN_004334E0`).
const QUEST_SET: i32 = 1;

/// Symbol texts of `extrasymbols.txt`: 2 `[DEFAULTCOLOR]`, 11 `[COLOR FF88FF88]`, 12 separator `[CR;CENTERXPOS]*[CR]`.
const SYMBOL_DEFAULT: i32 = 2;
const SYMBOL_GREEN: i32 = 11;
const SYMBOL_SEPARATOR: i32 = 12;

/// Text of the number, a missing one stays empty (`FUN_0055FFE0` remembers the number as everywhere, O-4).
fn text_or_empty(texts: &Texts, id: i32) -> String {
    texts.resolve(id).unwrap_or_default().to_string()
}

/// Content of `QUESTTEXT` (`FUN_004334E0`, re/campaign-ui D6.9): the ten main objectives in slot order, four entries
/// per objective, which the text field concatenates without separators (`FUN_00462980`, style `0x02`): status with
/// bit 4 (6, fulfilled) -> `[COLOR FF88FF88]`, text, `[DEFAULTCOLOR]`, separator; otherwise bit 1 (3, 1) ->
/// `[DEFAULTCOLOR]`, text, `[DEFAULTCOLOR]`, separator; otherwise nothing (0 empty, 10 failed). No numbers, the
/// separator also follows the last objective, never secondary objectives (O-53). With 0 objectives the list stays
/// empty: the default text 15034 "No mission objective selected!" does not appear because a text field label does
/// not take `+0x98` (O-365).
pub fn quest_items(m: &MissionInfo, texts: &Texts) -> Vec<String> {
    let mut items = Vec::new();
    for &(status, text) in &m.main_quests {
        let first = if status & QUEST_DONE != 0 {
            SYMBOL_GREEN
        } else if status & QUEST_SET != 0 {
            SYMBOL_DEFAULT
        } else {
            continue;
        };
        items.push(text_of(texts, first));
        items.push(text_or_empty(texts, text));
        items.push(text_of(texts, SYMBOL_DEFAULT));
        items.push(text_of(texts, SYMBOL_SEPARATOR));
    }
    items
}

/// Content of `TIPLOG` (`FUN_004DB6F0`, re/campaign-ui D6.8): `m_InfoHistory` from front to back (old -> new), for
/// each entry with an existing text that text, **between** the entries (not after the last) the separator; no colors.
/// Duplicates stay, there is no length limit (O-366). Mission objectives are not part of it.
pub fn tiplog_items(m: &MissionInfo, texts: &Texts) -> Vec<String> {
    let mut items = Vec::new();
    for &id in &m.info_history {
        let Some(text) = texts.resolve(id) else {
            continue;
        };
        if !items.is_empty() {
            items.push(text_of(texts, SYMBOL_SEPARATOR));
        }
        items.push(text.to_string());
    }
    items
}

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let frame = msg.dialog.clone();
    match (msg.element.as_str(), msg.code) {
        // `0x004D7643`...`0x004D771E`: `DISABLE` when a replay is running (`GameApp+0x19F3`, [`GameInfo::playback`]),
        // there is no engine (`FUN_004B9140() == 0`) or the scripts lock the UI (`Engine+0x600`, re/campaign-ui
        // D6.9); otherwise no `status`. Return value 1.
        ("savegame" | "savegame2" | "savegame3", code::QUERY_STATUS) => {
            if env.data.game.is_none_or(|g| g.ui_locked || g.playback) {
                msg.params.insert("status".into(), Param::Int(DISABLE));
            }
            1
        }
        // `0x004D7681`...`0x004D76B9`: `DISABLE` only during a replay (`FUN_004BAA30`).
        ("load", code::QUERY_STATUS) => {
            if env.data.game.is_some_and(|g| g.playback) {
                msg.params.insert("status".into(), Param::Int(DISABLE));
            }
            1
        }
        // `0x004D71AB`...`0x004D7243`: cancel `{5, frame}`, `{3, frame}`, prompt 15783 with OK `{0x25}`
        // (`PostQuitMessage`).
        ("quit", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            ask(
                env.state,
                env.queue,
                ASK_QUIT,
                Some(AppCmd::Quit),
                Some(open(&frame)),
            );
            1
        }
        // `0x004D7248`...`0x004D72E8`: `{3, frame}`, prompt 15780 with OK `{0x27, 2, slot of the local player, 1}`
        // (surrender -> defeat, end screen, accounting) and cancel `{5, frame}`; the original does not answer code 5
        // for `quitgame` (the button is free).
        ("quitgame", code::TRIGGERED) => {
            let slot = env.data.game.map_or(0, |g| g.local_slot);
            env.queue.push(close(&frame, true));
            ask(
                env.state,
                env.queue,
                ASK_QUIT_GAME,
                Some(AppCmd::Surrender { slot }),
                Some(open(&frame)),
            );
            1
        }
        // `0x004D72FF`...`0x004D7355`: return command `{5, frame}` (the original does not free the old one, O-155),
        // `{3, frame}`, `{5,"savegame"}`. `SAVEGAME2` (`0x004D735E`) switches to the network directory beforehand
        // (`FUN_004BFC40(2)`, network folder `savegame/multiplayer`), `SAVEGAME`/`SAVEGAME3` to the main directory
        // (`FUN_004BFC40(1)`).
        ("savegame" | "savegame2" | "savegame3", code::TRIGGERED) => {
            env.state.save_dir = if msg.element == "savegame2" {
                SaveDir::Multiplayer
            } else {
                SaveDir::Main
            };
            env.state.return_cmd = Some(open(&frame));
            env.queue.push(close(&frame, true));
            env.queue.push(open("savegame"));
            1
        }
        // `0x004D7392`...`0x004D73DD`: `{0x0F}`, without closing itself (`FUN_0042E290` does that).
        ("return", code::TRIGGERED) => {
            env.queue.push(AppCmd::CloseGameMenu);
            1
        }
        // `0x004D73CD` (`RETURN2`, `CHECKESCAPE`): `{0x13}`, without closing itself (`FUN_0042E5D0` does that).
        ("return2", code::TRIGGERED) => {
            env.queue.push(AppCmd::CloseNetGameMenu);
            1
        }
        // `0x004D73F0`...`0x004D7465`: `{3, frame}`, `{5,"ingameloadgame"}`, return command `{5, frame}`.
        ("load", code::TRIGGERED) => {
            env.state.save_dir = SaveDir::Main;
            env.queue.push(close(&frame, true));
            env.queue.push(open("ingameloadgame"));
            env.state.return_cmd = Some(open(&frame));
            1
        }
        // `0x004D75D3`...`0x004D763E`: `{3, frame}`, `{6,"gameplayoptions2"}`, `{6,"soundoptions2"}`, return command
        // `{5, frame}` (`FUN_004CDCC0` frees the old one); `SOUNDOPTIONS2 RETURN` queues it. `OPTIONS2` of the network
        // menu likewise (return to `ingameoptions2`).
        ("options" | "options2", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            env.queue.push(open_now("gameplayoptions2"));
            env.queue.push(open_now("soundoptions2"));
            env.state.return_cmd = Some(open(&frame));
            1
        }
        // `0x004D749E`...`0x004D74E7`: `{3, frame}`, `{5,"questmenu2"}`.
        ("questmenu", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            env.queue.push(open("questmenu2"));
            1
        }
        // `0x004D74EC`...`0x004D7535`: `{3, frame}`, `{5,"tiplogmenu"}`.
        ("tiplogmenu", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            env.queue.push(open("tiplogmenu"));
            1
        }
        // `0x004D753A`...`0x004D75CE` (O-147): `{3, frame}`, build cancel command `{5, frame}`, **again**
        // `{3, frame}` (without `{0x15}`), prompt 15780 with OK `{0x1F}` and cancel `{5, frame}`. The game menu has
        // the button only in the campaign (`INGAMEOPTIONS3` does not have it).
        ("restart", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            env.queue.push(close(&frame, true));
            ask(
                env.state,
                env.queue,
                ASK_QUIT_GAME,
                Some(AppCmd::ReloadLevel),
                Some(open(&frame)),
            );
            1
        }
        // `0x004D7720`: other buttons 1 without effect.
        (_, code::TRIGGERED) => 1,
        // `0x004D76BE`, `0x004D7164`: other elements with code 5 and other codes -1.
        _ => -1,
    }
}

/// `QUESTMENU2` (`FUN_004D1FF0`, from the game menu of the campaign) and `TIPLOGMENU` (`FUN_004DB6F0`): without an
/// engine both answer 0. `OK2` or `OK` (code 8) close the frame and open `ingameoptions` unconditionally (`{3, frame}`,
/// `{5,"ingameoptions"}`). The lists `QUESTTEXT` (main objectives, [`quest_items`]) and `TIPLOG` (`m_InfoHistory`,
/// [`tiplog_items`]) answer code 2 from [`MissionInfo`] (empty without it); code 9 answers 1 **without** `timeout`,
/// so the list never queries again: the content is created once when opening (O-365). The button `OK` of the frame
/// `QUESTMENU` (state 3, key `KD_CHECK_QUESTMENU`: `{0x10}`) belongs to the key layout (re/game-ui D5.3); the key is
/// unassigned in the original (O-364).
pub(super) fn handle_info(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    if env.data.game.is_none() {
        return 0;
    }
    let frame = msg.dialog.clone();
    match (frame.as_str(), msg.element.as_str(), msg.code) {
        // Code 5 of the frame: `FUN_004D1FF0` clears its text flag `+0x28` (return value 1).
        ("questmenu2", "questmenu2", code::QUERY_STATUS) => 1,
        // Code 2: fill the list (`FUN_0045E220("array")` checks whether the response carries an `array`):
        // `FUN_004334E0` or the `m_InfoHistory` loop; empty without a mission state.
        ("questmenu2", "questtext", code::QUERY_ITEMS) => {
            let items = env
                .data
                .mission
                .as_ref()
                .map_or_else(Vec::new, |m| quest_items(m, env.texts));
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        ("tiplogmenu", "tiplog", code::QUERY_ITEMS) => {
            let items = env
                .data
                .mission
                .as_ref()
                .map_or_else(Vec::new, |m| tiplog_items(m, env.texts));
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        // Code 7: the text is there only with the flag `+0x28` set (never); otherwise 0.
        ("questmenu2", "questtext", code::QUERY_TEXT) => 0,
        // Code 9: 1 without `timeout` (= 0; `FUN_004D1FF0`, `FUN_004DB6F0`): never query again (O-365).
        ("questmenu2", "questtext", code::QUERY_TIMEOUT)
        | ("tiplogmenu", "tiplog", code::QUERY_TIMEOUT) => 1,
        ("questmenu2", "ok2", code::TRIGGERED) | ("tiplogmenu", "ok", code::TRIGGERED) => {
            env.queue.push(close(&frame, true));
            env.queue.push(open("ingameoptions"));
            1
        }
        _ => -1,
    }
}

/// Command 6 `{6, name}` (open without waiting, flag 0).
fn open_now(name: &str) -> AppCmd {
    AppCmd::Open {
        name: name.to_string(),
        wait: false,
    }
}
