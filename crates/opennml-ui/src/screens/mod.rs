//! Screen logic per frame name: the dispatcher chain `0x004DC280` of the `MenuListener` (re/menu-flow).
//!
//! The widget listener (`app::AppListener`, `0x004DCE00`) passes every element message `0x12E` to [`dispatch`]. The
//! chain compares the **lowercase** dialog name with fixed names and calls the handler of the screen; return value
//! 1 = handled, -1 = not responsible. A handler answers queries through the parameters of the message, writes
//! settings, queues application commands and produces effects - all via [`ScreenEnv`].
//!
//! Covered: main menu, option frames, credits, confirmation prompts; video, sound and game options; key layout;
//! profiles (`SELECTPROFILE`), single player menu, `LOADAUTOPLAY`; own game (`CREATESINGLEPLAYERGAME`,
//! `LOADMULTIPLAYERMAP`, `RANDOMMAP`); player slots (`SINGLEPLAYERSTART`); game menu (`INGAMEOPTIONS`,
//! `INGAMEOPTIONS3`); saving and loading (`SAVEGAME`, `LOADSAVEDGAME2`, `INGAMELOADGAME`); end screens
//! (`GAMEEND_VICTORY`, `GAMEEND_DEFEAT`) and accounting (`ACCOUNTING`, `ACCOUNT0...2`); campaign menus
//! (`SELECTCAMPAIGN`, `KAMPAGNE0...3`, `CAMPAIGNLEVEL`/`CAMPAIGNLEVEL2`, `CAMPAIGNVIDEO`).

use opennml_formats::campaign::CampaignTable;
use opennml_formats::engine_ini::EngineConfig;

use crate::assets::Texts;
use crate::draft::{MapCatalog, MatchDraft, NetMenuView, SeedRand, SelectedLevel};
use crate::menu::AppQueue;
use crate::msg::{AppCmd, DisplayMode, ElementMsg, Requery, SharedText, UiEffect, code};
use crate::profiles::{Profiles, name_units};
use crate::saves::Saves;

mod accounting;
mod campaign;
pub mod chat;
pub mod chatframe;
mod control;
mod create_game;
mod credits;
pub mod diplomacy;
mod game_end;
mod gameplay;
mod ingame;
mod lan1;
mod lan2;
pub mod lan4;
mod msgbox;
mod options;
mod player_slots;
mod profile;
mod save_load;
mod singleplayer;
mod sound;
mod start;
mod video;

pub use accounting::AccountingView;
pub(crate) use chat::show as chat_show;
pub use control::{KeyList, conflicts, load_default_keys, load_keys_ini, reset_keys, text_context};
pub use create_game::{CHANGE_GAME_REFUSED, close_change_game};
pub(crate) use diplomacy::toggle as diplomacy_toggle;
pub use diplomacy::{
    DiplomacyAction, DiplomacyRow, DiplomacyView, MENU_STATE as DIPLOMACY_MENU_STATE,
};
pub use ingame::{GameInfo, MissionInfo, NetRoleView, quest_items, tiplog_items};
pub use lan2::{close_lobby, open_lobby};
pub use player_slots::{TeamColor, team_color_check};
pub use video::{filter_display_modes, gamma_to_slider, slider_to_gamma};

/// Working copy of the display values of `VIDEOOPTIONS` (statics `0x00671EB4`...`0x00671ED0`): filled on code 5 of
/// the frame from `ScreenXSize`/`ScreenYSize`/`ScreenDepth` (`mode`), `TextureQuality`, `TerrainQuality`,
/// `ParticlesQuality`, `ShadowQuality` and `Gamma`; only `ACCEPT` writes it back (O-98).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoCopy {
    pub mode: DisplayMode,
    pub texture: i32,
    pub terrain: i32,
    pub particles: i32,
    pub shadow: i32,
    pub gamma: f32,
}

impl VideoCopy {
    /// The statics before the first opening (`.bss`, all 0).
    pub const ZERO: VideoCopy = VideoCopy {
        mode: DisplayMode {
            width: 0,
            height: 0,
            bits: 0,
        },
        texture: 0,
        terrain: 0,
        particles: 0,
        shadow: 0,
        gamma: 0.0,
    };

    /// Copy on opening (`0x004CE30B`...`0x004CE366`).
    pub fn from_config(config: &EngineConfig) -> VideoCopy {
        VideoCopy {
            mode: DisplayMode {
                width: config.screen_x_size,
                height: config.screen_y_size,
                bits: config.screen_depth,
            },
            texture: config.texture_quality,
            terrain: config.terrain_quality,
            particles: config.particles_quality,
            shadow: config.shadow_quality,
            gamma: config.gamma,
        }
    }
}

/// State of the screen logic across messages (statics and fields of the application in the original).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScreenState {
    /// Flag "open subpage" of `OPTIONS` (static `CString` `0x00671EDC`).
    pub options_page: String,
    /// Text of the confirmation prompt or message (`GameApp+0x1994`). The original remembers a pointer into the text
    /// table, the reimplementation the text ID; `textinfo` resolves it on code 7 via [`ScreenEnv::texts`].
    pub info_text: Option<i32>,
    /// Free text of the message (no counterpart in the original, which shows only texts of the table): takes
    /// precedence over [`Self::info_text`] when `textinfo` is queried; set by [`tell_text`], cleared by [`ask`] and
    /// [`tell`]. For messages with names, e.g. of the network savegame.
    pub info_free: Option<String>,
    /// Command for `OK` of the confirmation prompt or message (copy `GameApp+0x1998`).
    pub info_ok: Option<AppCmd>,
    /// Command for `CANCEL` of the confirmation prompt (copy `GameApp+0x199C`).
    pub info_cancel: Option<AppCmd>,
    /// Working copy of `VIDEOOPTIONS`.
    pub video_copy: Option<VideoCopy>,
    /// A voice sample is still playing (`IsSoundPlaying(0x00671ED4)`); the host reports it after every frame
    /// (`MenuApp::set_speech_playing`), the logic sets it when a sample starts.
    pub speech_playing: bool,
    /// Remembered return command (`MenuListener+0x40`, re/ingame-menus 2.1): always `{5, frame}` of the game menu, set
    /// by `savegame*`, `load` and `options` (`FUN_004CDCC0`); `SOUNDOPTIONS2 RETURN`/`RETURN2` queue it and free it
    /// (`FUN_004CDD20`). Replacing frees the old one (O-155 does not apply).
    pub return_cmd: Option<AppCmd>,
    /// Selected action of the key layout (`GameApp+0x19B8`); `None` before the first choice.
    pub key_action: Option<i32>,
    /// Key text of the selected action (`GameApp+0x9A0`): buffer that the layout field `KEY` shares (it also writes
    /// into it); preserved beyond the closing of the dialog, as in the original.
    pub key_text: SharedText,
    /// List text of the selected action (`GameApp+0x1A0`): buffer that the label `KEYNAME` shares.
    pub key_description: SharedText,
    /// Profile selection of the list in `SELECTPROFILE` (static `DAT_00671F04`, initial value 0).
    pub profile_selection: i32,
    /// Mirror of `EngineConfig::net_name` (`EngineConfig+0x13C`), which the fields `PROFILENAME` with `inputbuffer`
    /// share (in the original they point directly to `NetName`): code 7 fills it from the settings, code `0xC`
    /// writes it back before the handler ([`dispatch`]); handlers that change `NetName` set both sides.
    pub net_name: SharedText,
    /// Mirror of `EngineConfig::net_session` (`+0x18C`), which `SESSIONNAME` in `CREATESINGLEPLAYERGAME` shares (like
    /// [`Self::net_name`]).
    pub net_session: SharedText,
    /// Mirror of `EngineConfig::random_map_seed` (`+0x1DC`), which `SEED` in `RANDOMMAP` shares (`FUN_004D7760` code 7:
    /// pointer, `size 30`, `inputbuffer 1`; like [`Self::net_session`]).
    pub random_map_seed: SharedText,
    /// Buffer `MenuListener+0x2A` of the field `TIMELIMIT` (`swprintf("%d", TimeLimit)` on code 7); code `0xC` reads
    /// `TimeLimit` back from it (`swscanf`, in the handler).
    pub time_limit_text: SharedText,
    /// Name of the savegame (`GameApp+0x177C`, re/ingame-menus 3.1): the buffer that `FILENAME` in `SAVEGAME` shows
    /// and edits (pointer with `inputbuffer`, `size 40`) and that selection (`FUN_004BC3E0`), saving, loading and
    /// deleting use.
    pub save_name: SharedText,
    /// Savegame list `GameApp+0xD6DC` (names without extension) from the last code 2 to `DIR` (`FUN_004C63A0`).
    pub save_entries: Vec<String>,
    /// Current savegame directory (`GameApp+0x1984`, `FUN_004BFC40`): determines the list, "exists" and deleting of
    /// the savegame frames.
    pub save_dir: crate::saves::SaveDir,
    /// Flag "entry selected" of `FUN_004D6100` (`DAT_00671EE5`, `SAVEGAME`/`LOADSAVEDGAME2`).
    pub save_selected: bool,
    /// Flag "entry selected" of `FUN_004D9830` (`DAT_00671F00`, `INGAMELOADGAME`).
    pub load_selected: bool,
    /// Flag "recording selected" of `FUN_004D9D30` (`DAT_00671F01`, `LOADAUTOPLAY`).
    pub autoplay_selected: bool,
    /// Between `0x26` and `0x0B` (savegame sent, not yet written): the original blocks the ticks via `Engine+0x115`
    /// (`GameEngine_CmdSaveGame`), the host does not step meanwhile. `0x0B` clears it, even if writing fails.
    pub saving: bool,
    /// Open tab page of the accounting (static `CString` `DAT_00671EF8`, initial value empty): every opening of
    /// `ACCOUNTING` sets it to `account1` (O-166).
    pub accounting_tab: String,
    /// Flag "3 levels" of `CAMPAIGNLEVEL`/`CAMPAIGNLEVEL2` (static `DAT_00671F02`): set on code 5 of the frame without
    /// a profile or after a defeat in the current entry (O-156); the box `LEVELLEVEL` then shows "Easy", "Normal",
    /// "Hard", otherwise only "Normal", "Hard" (O-157).
    pub campaign_three_levels: bool,
    /// Input buffer of the chat windows (`App+0x1418`): `INPUT` shares it (`inputbuffer`, `size 0x50`);
    /// `showchatwindow` and sending clear it.
    pub chat_text: SharedText,
    /// Flag "join in progress" of `LAN1` (`DAT_00671ED8`, re/multiplayer-menus L.1.1): 1 after `{7}`, 0 only on the
    /// next opening of `LAN1` - prevents double joins.
    pub lan_joining: bool,
    /// History of the lobby chat (`GameApp+0x9F0`, re/multiplayer-menus L.4): five lines that `CHAT` in `CHATFRAME`
    /// shows via a pointer; `MenuApp::set_net` writes it from [`crate::draft::LobbyView::chat`].
    pub lobby_chat: SharedText,
}

/// Data of the host and of the campaign management that the screens read and change: profiles (content of
/// `profiles.ini`, read by the host), the campaign table (`campaign.ini`, from the VFS at start), the map catalog,
/// the selected map and the match draft, the running match and the savegames; the accounting and the difficulty of
/// the campaign.
#[derive(Debug, Clone, Default)]
pub struct MenuData {
    pub profiles: Profiles,
    pub campaigns: CampaignTable,
    /// Multiplayer maps and `randommaptypes.ini` (deviation: once in `MenuApp::new`).
    pub maps: MapCatalog,
    /// Selected map before `CREATE2` (`GameApp+0xD654`, `Info`, texts of the `[CURLEVEL...]`).
    pub level: SelectedLevel,
    /// Session `GameApp+0xD650`: copy of the map, slots and settings from `CREATE2`.
    pub draft: MatchDraft,
    /// The running match (`FUN_004B9140() != 0`, engine present), set by the host.
    pub game: Option<GameInfo>,
    /// Mission objectives and tip log of the match for `QUESTMENU2`/`TIPLOGMENU`, set by the host every frame
    /// ([`MissionInfo`]); `None` without a match.
    pub mission: Option<MissionInfo>,
    /// State of the diplomacy dialog, set by the host every frame ([`DiplomacyView`]); `None` without a match. The
    /// handler writes set values into its copy until the host delivers the new state.
    pub diplomacy: Option<DiplomacyView>,
    /// The savegames of the host (`MenuOptions::saves`, otherwise none).
    pub saves: Saves,
    /// The accounting of the last match (table `GameApp+0xD760` and game time text `GameApp+0x10`), set by the host on
    /// command `0x0E` (`MenuApp::set_accounting`); it stays until the next one.
    pub accounting: Option<AccountingView>,
    /// Difficulty of the campaign (`GameApp+0x19B4`, `FUN_004BA5A0` set, `FUN_004BA5B0` read): initial value
    /// `Engine.ini CampaignDifficulty` (`MenuApp::new`), set by `CAMPAIGNLEVEL`, read by command `0x0C` (in the
    /// original at game setup, `GameEngine_ApplyGameSettings` `0x004311C2`).
    pub campaign_difficulty: i32,
    /// The menu's own `rand()` state for the empty seed of a random map (O-140): start 1 with `MenuApp`, not saved;
    /// `CREATE2` draws it when `RandomMapSeed` is empty ([`SeedRand::next`]).
    pub seed_rand: SeedRand,
    /// State of the network session (found games, lobby), set by the host every frame ([`NetMenuView`]); `None`
    /// without a network session.
    pub net: Option<NetMenuView>,
    /// The selected session in `LAN1` (copy `NS+0x08`, re/multiplayer-menus L.1.1): in the reimplementation the
    /// session ID instead of the list index (deviates from E-300) - if a game before it disappears, the selection
    /// stays on the selected game.
    pub lan_choice: Option<u64>,
}

/// What a handler sees and changes.
pub struct ScreenEnv<'a> {
    /// The settings (`g_pEngineConfig`).
    pub config: &'a mut EngineConfig,
    /// Application queue (`FUN_004BCB40`).
    pub queue: &'a mut AppQueue,
    /// The shared effect list of the widgets and the logic (order as in the original).
    pub effects: &'a mut Vec<UiEffect>,
    /// Text tables of the host (from `UiContext::texts` every frame).
    pub texts: &'a Texts,
    /// Display modes of the monitor, raw (`MenuOptions::display_modes`); `VIDEOOPTIONS` filters them like the
    /// original ([`filter_display_modes`]).
    pub modes: &'a [DisplayMode],
    /// Display names of the sound drivers (`MenuOptions::sound_drivers`; `SoundManager_FillDriverList` in the
    /// original).
    pub sound_drivers: &'a [String],
    /// List of the key layout, built once at start (`GameApp+0x19BC`, [`KeyList::build`]).
    pub key_list: &'a KeyList,
    pub state: &'a mut ScreenState,
    /// GameSpy Arcade is installed (`GameApp+0x198E`).
    pub game_spy_installed: bool,
    /// Profiles, campaigns and other data of the application ([`MenuData`]).
    pub data: &'a mut MenuData,
    /// Targeted re-queries ("re-query X", [`requery`]); `MenuApp` works them off in order after the message returns.
    pub requery: &'a mut Vec<Requery>,
    /// Menu state of the match (`Engine+0x2CCC`, `MenuApp::game_menu_state`): "Back" of diplomacy closes via the same
    /// function as the host (`FUN_0042E060`).
    pub game_menu: &'a mut u8,
}

/// Dispatcher chain `0x004DC280`...`0x004DCDF5`: handler by dialog name, otherwise -1. Order of the chain (only the
/// screens handled here): `options` (`0x004DC2E0`), `soundoptions` (`0x004DC2FC`), `videooptions` (`0x004DC318`),
/// `gameplayoptions` (`0x004DC334`), `start` (`0x004DC41C`), `ingameoptions` (`0x004DC606`), `ingameoptions3`
/// (`0x004DC632`), `questmenu2` (`FUN_004D1FF0`, `0x004DC39E`), `tiplogmenu` (`FUN_004DB6F0`, `0x004DCC61`),
/// `savegame`/`loadsavedgame2` (`FUN_004D6100`), `ingameloadgame` (`FUN_004D9830`), `controloptions`
/// (`0x004DC9A9`), `gameplayoptions2`/`3` (`0x004DC9FE`), `soundoptions2`/`3` (`0x004DCA53`), `infoframe`/`msgbox`
/// (`0x004DCCB6`), `credits` (`0x004DCDB2`), `gameend_*` (`FUN_004D9030`), `accounting` (`FUN_004D93F0`),
/// `account0...2` (`FUN_004CF8C0`), `kampagne0...3` (`FUN_004D6DA0`, `0x004DC5DC`), `selectcampaign`
/// (`FUN_004DA4A0`, `0x004DCB4F`), `campaignvideo` (`FUN_004DA8A0`, `0x004DCB8E`), `campaignlevel`/`campaignlevel2`
/// (`FUN_004DABD0`, `0x004DCBB6`...`0x004DCBE3`), `diplomatie` (`FUN_004D0750`),
/// `chatwindow`/`chatwindow2`/`chatwindow3` (`FUN_004CFF80`), `lan1` (`FUN_004D2FE0`), `lan3` (`FUN_004D4B60`, network
/// branch), `lan2` (`FUN_004D38F0`), `chatframe` (`FUN_004CFD10`), `lan31` (`FUN_004D4B60`, map change),
/// `ingameoptions2` (`FUN_004D7110`, network game menu), `lan4` (`FUN_004CF290`, waiting room of the loaded network
/// match).
pub fn dispatch(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    sync_mirrors(msg, env);
    match msg.dialog.as_str() {
        "options" => options::handle(msg, env),
        "soundoptions" | "soundoptions2" | "soundoptions3" => sound::handle(msg, env),
        "videooptions" => video::handle(msg, env),
        "gameplayoptions" | "gameplayoptions2" | "gameplayoptions3" => gameplay::handle(msg, env),
        "start" => start::handle(msg, env),
        "controloptions" => control::handle(msg, env),
        "infoframe" | "msgbox" => msgbox::handle(msg, env),
        "credits" => credits::handle(msg, env),
        "selectprofile" => profile::handle(msg, env),
        "singleplayermenu" => singleplayer::handle(msg, env),
        "loadautoplay" => singleplayer::handle_autoplay(msg, env),
        "createsingleplayergame" => create_game::handle(msg, env),
        "loadmultiplayermap" => create_game::handle_map(msg, env),
        "randommap" => create_game::handle_random(msg, env),
        "singleplayerstart" => player_slots::handle(msg, env),
        "lan1" => lan1::handle(msg, env),
        "lan3" | "lan31" => create_game::handle(msg, env),
        "lan2" => lan2::handle(msg, env),
        "lan4" => lan4::handle(msg, env),
        "chatframe" => chatframe::handle(msg, env),
        "ingameoptions" | "ingameoptions2" | "ingameoptions3" => ingame::handle(msg, env),
        "questmenu2" | "tiplogmenu" => ingame::handle_info(msg, env),
        "diplomatie" => diplomacy::handle(msg, env),
        "chatwindow" | "chatwindow2" | "chatwindow3" => chat::handle(msg, env),
        "savegame" | "loadsavedgame2" | "loadsavednetworkgame" => save_load::handle(msg, env),
        "ingameloadgame" => save_load::handle_ingame(msg, env),
        "gameend_victory" | "gameend_defeat" => game_end::handle(msg, env),
        "accounting" => accounting::handle(msg, env),
        "account0" | "account1" | "account2" => accounting::handle_tab(msg, env),
        "kampagne0" | "kampagne1" | "kampagne2" | "kampagne3" => campaign::handle_list(msg, env),
        "selectcampaign" => campaign::handle_select(msg, env),
        "campaignvideo" => campaign::handle_video(msg, env),
        "campaignlevel" | "campaignlevel2" => campaign::handle_level(msg, env),
        _ => -1,
    }
}

/// Mirror of the EditField buffers that lie directly in `EngineConfig` in the original: code `0xC` (the field reports
/// every change) transfers the text into the settings before the handler. `PROFILENAME` in `SELECTPROFILE` and
/// `SINGLEPLAYERMENU` and `PLAYERNAME` in `LAN1` -> `NetName` (`EngineConfig+0x13C`, 40 units including the
/// terminator); `SESSIONNAME` in `CREATESINGLEPLAYERGAME`, `LAN3` and `LAN31` -> `NetSession` (`+0x18C`); `SEED` in
/// `RANDOMMAP` -> `RandomMapSeed` (`+0x1DC`). `TIMELIMIT` is read by its handler itself (`swscanf`).
fn sync_mirrors(msg: &ElementMsg, env: &mut ScreenEnv) {
    if msg.code != code::EDIT_CHANGED {
        return;
    }
    match (msg.dialog.as_str(), msg.element.as_str()) {
        ("selectprofile" | "singleplayermenu", "profilename") | ("lan1", "playername") => {
            env.config.net_name = units_field(&env.state.net_name.units());
        }
        ("createsingleplayergame" | "lan3" | "lan31", "sessionname") => {
            env.config.net_session = units_field(&env.state.net_session.units());
        }
        ("randommap", "seed") => {
            env.config.random_map_seed = units_field(&env.state.random_map_seed.units());
        }
        _ => {}
    }
}

/// Field `wchar_t[40]` from UTF-16 units (up to a null character, at most 39), rest 0.
pub(crate) fn units_field(units: &[u16]) -> [u16; 40] {
    let units = name_units(units);
    let mut field = [0u16; 40];
    let n = units.len().min(39);
    field[..n].copy_from_slice(&units[..n]);
    field
}

/// Set `NetName` (`wcscpy(EngineConfig+0x13C, ...)`): settings and mirror ([`ScreenState::net_name`]).
fn set_net_name(env: &mut ScreenEnv, name: &[u16; 40]) {
    env.config.net_name = *name;
    env.state.net_name.set_units(name_units(name).to_vec());
}

/// Fill the mirror from `NetName` (code 7 of the fields `PROFILENAME`).
fn fill_net_name(env: &mut ScreenEnv) -> SharedText {
    let units = name_units(&env.config.net_name).to_vec();
    env.state.net_name.set_units(units);
    env.state.net_name.clone()
}

/// "Query again" `{frame, element, code}` (`FUN_00464970` + `FUN_004DD600`, [`Requery`]).
fn requery(env: &mut ScreenEnv, dialog: &str, element: &str, code: u32) {
    env.requery.push(Requery::new(dialog, element, code));
}

/// Confirmation prompt `FUN_004BCD70(text, ok, cancel)`: remember the text and copies of the two commands, queue
/// `{5,"infoframe"}` (re/menu-flow, `INFOFRAME`/`MSGBOX`).
pub fn ask(
    state: &mut ScreenState,
    queue: &mut AppQueue,
    text_id: i32,
    ok: Option<AppCmd>,
    cancel: Option<AppCmd>,
) {
    state.info_text = Some(text_id);
    state.info_free = None;
    state.info_ok = ok;
    state.info_cancel = cancel;
    queue.push(open("infoframe"));
}

/// Message `FUN_004BCF10(text, ok)`: like [`ask`] without a cancel command, `{5,"msgbox"}`.
pub fn tell(state: &mut ScreenState, queue: &mut AppQueue, text_id: i32, ok: Option<AppCmd>) {
    state.info_text = Some(text_id);
    state.info_free = None;
    state.info_ok = ok;
    state.info_cancel = None;
    queue.push(open("msgbox"));
}

/// Message with free text: like [`tell`], `textinfo` shows `text` instead of a text of the table
/// ([`ScreenState::info_free`]).
pub fn tell_text(state: &mut ScreenState, queue: &mut AppQueue, text: String, ok: Option<AppCmd>) {
    state.info_free = Some(text);
    state.info_ok = ok;
    state.info_cancel = None;
    queue.push(open("msgbox"));
}

/// Command 5 `{5, name}` (flag 1).
fn open(name: &str) -> AppCmd {
    AppCmd::Open {
        name: name.to_string(),
        wait: true,
    }
}

/// Command 3 (`wait`) or 4 `{3|4, name}`.
fn close(name: &str, wait: bool) -> AppCmd {
    AppCmd::Close {
        name: name.to_string(),
        wait,
    }
}

/// Shared construction of the environment in tests (`screens/tests.rs`, `screens/control/tests.rs`): everything a
/// handler sees, except the texts of the world.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct EnvParts {
    pub(crate) config: EngineConfig,
    pub(crate) queue: AppQueue,
    pub(crate) effects: Vec<UiEffect>,
    pub(crate) state: ScreenState,
    pub(crate) data: MenuData,
    pub(crate) requery: Vec<Requery>,
    pub(crate) modes: Vec<DisplayMode>,
    pub(crate) sound_drivers: Vec<String>,
    pub(crate) key_list: KeyList,
    pub(crate) game_spy_installed: bool,
    pub(crate) game_menu: u8,
}

#[cfg(test)]
impl EnvParts {
    /// The environment over these parts and the texts `texts`.
    pub(crate) fn env<'a>(&'a mut self, texts: &'a Texts) -> ScreenEnv<'a> {
        ScreenEnv {
            config: &mut self.config,
            queue: &mut self.queue,
            effects: &mut self.effects,
            texts,
            modes: &self.modes,
            sound_drivers: &self.sound_drivers,
            key_list: &self.key_list,
            state: &mut self.state,
            game_spy_installed: self.game_spy_installed,
            data: &mut self.data,
            requery: &mut self.requery,
            game_menu: &mut self.game_menu,
        }
    }

    /// Message to the dispatcher chain.
    pub(crate) fn deliver(&mut self, texts: &Texts, msg: &mut ElementMsg) -> i32 {
        dispatch(msg, &mut self.env(texts))
    }
}

#[cfg(test)]
mod tests;
