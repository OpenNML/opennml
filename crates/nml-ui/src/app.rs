//! `MenuApp`: the menu UI as a whole - widget tree, dialog stack, command queue, input buffers and the widget
//! listener (re/menu-system 12.1, re/menu-flow, sections "Commands" and "Program start to main menu").
//!
//! One frame follows `GameApp_Tick` (`0x004CA770`): keyboard (`FUN_004C6050`) -> mouse (`FUN_004BB100`) -> raw frame
//! event 6 (pending deletions) -> drawing pass with all `Tick`s ([`tick_tree`], `0x136` straight onto the stack) and
//! drawing ([`paint_ui_with`]) -> **one** application command ([`AppQueue::pop_ready`]).
//!
//! **Videos** (re/video): The commands `0x1B`/`0x1D` request a video from the host ([`UiEffect::PlayVideo`]), which
//! opens it and reports it with [`MenuApp::video_started`]. While it runs, the desktop draws the video instead of its
//! images, `0x1A` halts the queue, and a key that no widget takes aborts it; after the last frame or on abort,
//! [`UiEffect::StopVideo`] goes to the host.
//!
//! **Listener and borrowing.** The widgets talk only to `UiEnv::listener`. The listener [`AppListener`] bundles
//! `MenuListener` (`0x004DCE00`): it answers `0x12F` from the shared [`TriggerGate`] of the stack (the stack is
//! mutably borrowed in the meantime, e.g. while processing `0x136` in the drawing pass) and passes `0x12E` to the
//! screen logic ([`screens::dispatch`], dispatcher chain `0x004DC280`). The logic writes settings, enqueues commands
//! and produces effects.
//!
//! **Order.** As in the original there is one list each: the effects of widgets and logic land in the same list
//! (`UiEnv::fx`, the listener receives them with every message), the commands of the logic and the `.dlg` reader in
//! the same [`AppQueue`] (the stack enqueues via [`UiListener::push_command`]) - each in order of creation. When
//! building a frame, for example, `0x20` of the file comes before the commands of the queries in `OnAttach` and the
//! effects of the queries before the music (`0x160`); on a click, `SFX_KLICK` comes before the effects of code 8.

use std::collections::VecDeque;
use std::rc::Rc;

use nml_formats::campaign::EntryKind;
use nml_formats::config::lower_german;
use nml_formats::dlg::desc::{common_flags, frame_flags};
use nml_formats::dlg::upper_word;
use nml_formats::engine_ini::EngineConfig;
use nml_formats::keys_ini::KeysIniLine;
use nml_formats::profiles::ProfilesFile;
use nml_rda::Vfs;

use crate::assets::Texts;
use crate::coords::{Screen, to_screen_rect};
use crate::dialog::{DialogLibrary, UiContext};
use crate::draft::{
    DraftSettings, LevelInfoData, LevelNetInfo, MapCatalog, MatchDraft, MatchRequest, NetMenuView,
    SlotDraft, level_infos,
};
use crate::draw::{Blend, DrawCmd, Filter, TextureId};
use crate::geom::intersect;
use crate::input::{ClickTimer, Pointer, UiInput, dispatch_key, dispatch_mouse};
use crate::keys::chord_name;
use crate::menu::{AppQueue, MenuSystem, MusicLength, TriggerGate};
use crate::msg::{AppCmd, DisplayMode, ElementMsg, MenuLevelEffect, Requery, UiEffect, UiListener};
use crate::paint::{paint_ui_with, tick_tree};
use crate::profiles::{Profiles, load_campaigns};
use crate::saves::{SaveFiles, Saves};
use crate::screens::{
    self, AccountingView, DiplomacyView, GameInfo, KeyList, MenuData, MissionInfo, NetRoleView,
    ScreenEnv, ScreenState, units_field,
};
use crate::text::{CurLevelText, TextContext};
use crate::tree::{NodeId, NodeKind, UiEnv, UiTree};
use crate::video::{INTRO_VIDEOS, VideoInfo, VideoPlayback, video_rect, video_uv};

/// How many effects [`UiReport::effects`] looks back.
const REPORT_EFFECTS: usize = 32;

/// At most this many rounds of targeted requeries after a message (a requery can demand more through its response);
/// the original has no limit (recursion), the reimplementation aborts with a warning.
const REQUERY_ROUNDS: usize = 64;

/// What the host passes to the menu UI.
#[derive(Default)]
pub struct MenuOptions {
    /// Display modes of the monitor, **raw**, once at start (`VIDEOOPTIONS` filters them like the original,
    /// [`screens::filter_display_modes`]).
    pub display_modes: Vec<DisplayMode>,
    /// Enqueue the start sequence (intro commands with `IsIntro`, then `start`); otherwise the UI stays empty.
    pub start_menu: bool,
    /// With `start_menu`, skip the start videos even with `IsIntro`, same as `start` (E-295: network start from the
    /// command line like a direct start); `IsIntro` in the settings stays unchanged.
    pub skip_intro: bool,
    /// GameSpy Arcade is installed (`GameApp+0x198E`).
    pub game_spy_installed: bool,
    /// Display names of the sound drivers (list of the combobox `sounddriver`, index = `m_DriverID`).
    pub sound_drivers: Vec<String>,
    /// Length of a music file (path as read -> ms), queried synchronously when building a frame with `MUSIC`; without
    /// a supplier 0 applies (the credits then last 120,000 ms).
    pub music_length: Option<MusicLength>,
    /// Content of `profiles.ini`, read by the host (on error: default without profiles).
    pub profiles: ProfilesFile,
    /// Savegames of the host (list, existence, local time); without them [`crate::saves::NoSaveFiles`] applies.
    pub saves: Option<Rc<dyn SaveFiles>>,
}

impl std::fmt::Debug for MenuOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuOptions")
            .field("display_modes", &self.display_modes)
            .field("start_menu", &self.start_menu)
            .field("skip_intro", &self.skip_intro)
            .field("game_spy_installed", &self.game_spy_installed)
            .field("sound_drivers", &self.sound_drivers)
            .field("music_length", &self.music_length.is_some())
            .field("profiles", &self.profiles)
            .field("saves", &self.saves.is_some())
            .finish()
    }
}

/// State of the UI for the control channel and checks. Names lowercase as in the messages, rectangles in screen
/// pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct UiReport {
    /// Open frames in drawing order (non-modal, then modal).
    pub dialogs: Vec<ReportDialog>,
    /// The innermost widget with keyboard focus (dialog, element).
    pub focus: Option<(String, String)>,
    /// The innermost widget with "mouse over".
    pub hover: Option<(String, String)>,
    pub elements: Vec<ReportElement>,
    /// Images of the desktop with opacity in drawing order.
    pub images: Vec<(String, u8)>,
    /// The dialog stack has orders (triggering refused).
    pub busy: bool,
    /// Waiting application commands.
    pub queued: usize,
    /// The last 32 effects with the time of the frame.
    pub effects: Vec<(u64, UiEffect)>,
    /// The running video with the frame at the time of the report. If its last frame is over but the video has not yet
    /// been ended by [`MenuApp::frame`] (between expiry of the time and the next frame), the field is already `None`
    /// while [`MenuApp::video`] still returns `Some`.
    pub video: Option<ReportVideo>,
    /// The mouse pointer is off (start sequence, campaign video).
    pub pointer_hidden: bool,
}

/// Running video in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportVideo {
    pub path: String,
    /// Frame at the time of the report (from 0).
    pub frame: u32,
    pub frames: u32,
    pub abortable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportDialog {
    pub name: String,
    /// Screen coordinates.
    pub rect: [i32; 4],
    /// Fly-in or fly-out is running.
    pub animating: bool,
    pub modal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportElement {
    pub dialog: String,
    pub name: String,
    /// `button`, `combobox`, `listbox`, `editfield`, `slider`, `radiobutton` or `label`.
    pub kind: &'static str,
    /// Screen coordinates.
    pub rect: [i32; 4],
    /// Drawn in the last frame (visited by the drawing pass).
    pub visible: bool,
    pub enabled: bool,
    /// Flag `HIDE` (from the file or from the logic).
    pub hidden: bool,
    pub checked: Option<bool>,
    pub value: Option<i32>,
    pub selection: Option<i32>,
    pub items: Vec<String>,
    /// Text of the logic or of the edit field; the report does not resolve texts from text IDs (it does not know
    /// `Texts`).
    pub text: Option<String>,
}

/// Cache of the text-set state together with the inputs it is computed from (the map details are in
/// [`TextContext::cur_level`]).
struct TextCache {
    key_map: Vec<(i32, String)>,
    /// The draft from which `[CURLEVELINFOS]` is computed ([`level_infos`]).
    draft: MatchDraft,
    /// The network details of the lobby from which `[CURLEVELINFOS]` is computed.
    net: Option<LevelNetInfo>,
    context: TextContext,
}

/// Listener of the widgets (`MenuListener` `0x004DCE00` on the desktop, 3.3).
struct AppListener<'a> {
    gate: &'a TriggerGate,
    queue: &'a mut AppQueue,
    config: &'a mut EngineConfig,
    texts: &'a Texts,
    opts: &'a MenuOptions,
    key_list: &'a KeyList,
    state: &'a mut ScreenState,
    data: &'a mut MenuData,
    requery: &'a mut Vec<Requery>,
    game_menu: &'a mut u8,
    #[cfg(test)]
    logic: &'a mut Option<TestLogic>,
}

/// Test stand-in for the screen logic: message and environment of the logic, return value like a handler.
#[cfg(test)]
pub(crate) type TestLogic = Box<dyn FnMut(&mut ElementMsg, &mut ScreenEnv) -> i32>;

impl UiListener for AppListener<'_> {
    /// `0x12E`: dispatcher chain `0x004DC280` by dialog name ([`screens::dispatch`]).
    fn element(&mut self, msg: &mut ElementMsg, fx: &mut Vec<UiEffect>) -> i32 {
        let mut env = ScreenEnv {
            config: self.config,
            queue: self.queue,
            effects: fx,
            texts: self.texts,
            modes: &self.opts.display_modes,
            sound_drivers: &self.opts.sound_drivers,
            key_list: self.key_list,
            state: self.state,
            game_spy_installed: self.opts.game_spy_installed,
            data: self.data,
            requery: self.requery,
            game_menu: self.game_menu,
        };
        #[cfg(test)]
        if let Some(logic) = self.logic.as_mut() {
            return logic(msg, &mut env);
        }
        screens::dispatch(msg, &mut env)
    }

    /// `0x12F`: `FUN_004DCF20` returns 1 without orders, the listener turns that into 0 (allowed) or 1 (refused)
    /// (`NEG AL; SBB EAX, EAX; INC EAX` at `0x004DCE1E`).
    fn may_trigger(&mut self) -> i32 {
        i32::from(self.gate.busy())
    }

    /// Commands of the `.dlg` reader (`0x20`/`0x21`) go into the same queue as those of the logic.
    fn push_command(&mut self, cmd: AppCmd) {
        self.queue.push(cmd);
    }

    fn take_requery(&mut self) -> Vec<Requery> {
        std::mem::take(self.requery)
    }
}

/// The menu UI.
pub struct MenuApp {
    config: EngineConfig,
    opts: MenuOptions,
    tree: UiTree,
    menu: MenuSystem,
    queue: AppQueue,
    pointer: Pointer,
    clicks: ClickTimer,
    /// Buffered key events and mouse events of the next frame with their screen.
    keys_in: Vec<(UiInput, Screen)>,
    mouse_in: Vec<(UiInput, Screen)>,
    effects: Vec<UiEffect>,
    history: VecDeque<(u64, UiEffect)>,
    /// Effects that arose outside a frame ([`Self::show_game_end`]); the next frame takes them into the look-back of
    /// the report with its time.
    unreported: Vec<UiEffect>,
    /// State of the screen logic.
    screens: ScreenState,
    /// Profiles and campaigns.
    data: MenuData,
    /// Demanded targeted requeries, processed after each [`Self::step`].
    requery: Vec<Requery>,
    /// List of the key layout, built once in [`Self::new`] (`FUN_004BC540` at program start, O-102).
    key_list: KeyList,
    /// Menu state of the text set together with the inputs and map details it is computed from (rebuilt as soon as one
    /// of them changes).
    text_ctx: Option<TextCache>,
    /// The running video (menu root `+0xB8`).
    video: Option<VideoPlayback>,
    /// Flag "abortable" of the last `UiEffect::PlayVideo` request (`+0xBC`).
    video_abortable: bool,
    /// The mouse pointer is off (`FUN_004CD6A0(0)`).
    pointer_hidden: bool,
    /// Menu state of the match (`Engine+0x2CCC`): 0 running, 1 game menu (single player), 6 victory, 7 defeat, 10
    /// diplomacy.
    game_menu: u8,
    /// Key names of the last frame that neither the broadcast nor the widgets took.
    untaken_keys: Vec<String>,
    /// State of the menu level behind the menu (`GameApp+0xD5E0` bit `0x1`), set by the host.
    level_world: LevelWorld,
    #[cfg(test)]
    logic: Option<TestLogic>,
}

impl MenuApp {
    /// Loads the dialogs (`DialogLibrary` with `is_menu_level = false`, a deviation from the original; `texts` only
    /// for resolving while loading), the campaign table `data/gifcon/campaign.ini` ([`load_campaigns`], like
    /// `FUN_0040E080`; on error: empty and a warning), the map catalog ([`MapCatalog::load`], a deviation: once at
    /// start instead of at every choice), takes over the profiles from `opts.profiles`, builds the list of the key
    /// layout from `defaultkeys.ini` ([`KeyList::build`]), reads `keys.ini` for the conflict check
    /// ([`screens::load_keys_ini`]) and with `start_menu` enqueues the start sequence (re/menu-flow,
    /// section "Program start to main menu" 5, `0x004C96D3`...`0x004C97B1`): with `IsIntro` `Video(0)`, `WaitVideo`,
    /// `Video(1)`, `WaitVideo`, `Video(2)`, `WaitVideo`, `AfterIntro`, then `Open { "start", wait: true }`; with
    /// `skip_intro` only the `Open` (E-295).
    pub fn new(
        vfs: &Vfs,
        texts: &Texts,
        config: EngineConfig,
        opts: MenuOptions,
    ) -> Result<MenuApp, String> {
        let library = DialogLibrary::load(vfs, texts, false);
        if library.names().is_empty() {
            return Err("no menu dialogs found under data/menü/".to_string());
        }
        if opts.start_menu && library.expanded("start").is_none() {
            return Err("the main menu \"start\" is missing in data/menü/dialogs.map".to_string());
        }
        let mut app = MenuApp::from_library(library, config, opts);
        app.key_list = KeyList::build(&screens::load_default_keys(vfs), texts);
        app.key_list.keys_ini = screens::load_keys_ini(vfs);
        app.data.campaigns = load_campaigns(vfs);
        app.data.maps = MapCatalog::load(vfs, texts);
        Ok(app)
    }

    pub(crate) fn from_library(
        library: DialogLibrary,
        config: EngineConfig,
        mut opts: MenuOptions,
    ) -> MenuApp {
        let menu = MenuSystem::new(library, opts.music_length.take());
        let profiles = Profiles::new(std::mem::take(&mut opts.profiles));
        let saves = opts.saves.take().map(Saves).unwrap_or_default();
        // At `IsIntro` the program start turns the pointer off before enqueuing (`0x004C96D9`). Without start videos
        // (`skip_intro`, E-295) it stays on as with `IsIntro 0`.
        let intro = opts.start_menu && config.is_intro && !opts.skip_intro;
        let pointer_hidden = intro;
        let campaign_difficulty = config.campaign_difficulty;
        let mut queue = AppQueue::default();
        if opts.start_menu {
            if intro {
                for n in 0..3 {
                    queue.push(AppCmd::Video(n));
                    queue.push(AppCmd::WaitVideo);
                }
                queue.push(AppCmd::AfterIntro);
            }
            queue.push(AppCmd::Open {
                name: "start".to_string(),
                wait: true,
            });
        }
        MenuApp {
            config,
            opts,
            tree: UiTree::new(),
            menu,
            queue,
            pointer: Pointer::default(),
            clicks: ClickTimer::default(),
            keys_in: Vec::new(),
            mouse_in: Vec::new(),
            effects: Vec::new(),
            history: VecDeque::new(),
            unreported: Vec::new(),
            screens: ScreenState::default(),
            data: MenuData {
                profiles,
                saves,
                // `GameApp+0x19B4`: initial value `CampaignDifficulty` (re/singleplayer-menus part 1, 4.3).
                campaign_difficulty,
                ..MenuData::default()
            },
            requery: Vec::new(),
            key_list: KeyList::default(),
            text_ctx: None,
            video: None,
            video_abortable: true,
            pointer_hidden,
            game_menu: 0,
            untaken_keys: Vec::new(),
            level_world: LevelWorld::default(),
            #[cfg(test)]
            logic: None,
        }
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Writes the speed of the speed keys into the setting `GameSpeed` (`FUN_0042F050`: `EngineConfig+0x34 = v`). Only
    /// the setting in memory; the host writes the file as with every setting (`MenuHost::write_settings`, `shutdown`).
    /// The next closing of the game menu takes over the value (O-153).
    pub fn set_game_speed(&mut self, v: i32) {
        self.config.game_speed = v;
    }

    pub fn options(&self) -> &MenuOptions {
        &self.opts
    }

    pub fn tree(&self) -> &UiTree {
        &self.tree
    }

    pub fn menu(&self) -> &MenuSystem {
        &self.menu
    }

    pub fn queue(&self) -> &AppQueue {
        &self.queue
    }

    /// After every frame the host reports whether a speech sample is still playing (`SOUNDOPTIONS`).
    pub fn set_speech_playing(&mut self, playing: bool) {
        self.screens.speech_playing = playing;
    }

    pub fn speech_playing(&self) -> bool {
        self.screens.speech_playing
    }

    /// State of the screen logic (flags of the options, text and commands of the confirmation request, ...).
    pub fn screen_state(&self) -> &ScreenState {
        &self.screens
    }

    /// List of the key layout (`CONTROLOPTIONS`).
    pub fn key_list(&self) -> &KeyList {
        &self.key_list
    }

    /// The lines of `keys.ini` for the conflict check of the key layout (`CONTROLOPTIONS`, O-102). [`Self::new`] reads
    /// them at start from the game directory; a host without an archive sets them here.
    pub fn set_keys_ini(&mut self, lines: Vec<KeysIniLine>) {
        self.key_list.keys_ini = lines;
    }

    /// The profiles (the host writes them to `profiles.ini` on [`UiEffect::WriteProfiles`]).
    pub fn profiles(&self) -> &Profiles {
        &self.data.profiles
    }

    /// Profiles, campaign table and further data of the screens.
    pub fn data(&self) -> &MenuData {
        &self.data
    }

    /// Data of the screens for modifying (tests: own campaign table, game type of the session).
    #[cfg(test)]
    pub(crate) fn data_mut(&mut self) -> &mut MenuData {
        &mut self.data
    }

    /// `FUN_004C6B10(won)` (re/singleplayer-menus part 1, 5): books the current campaign entry (`FUN_0040C2B0`,
    /// [`crate::campaign::book`]) and writes `profiles.ini` (`FUN_004C6A90`, here [`UiEffect::WriteProfiles`]); for the
    /// report beforehand [`UiEffect::BookCampaign`]. Callers in the original: `GameEngine_SetPlayerResult` for the
    /// local player at game type -1 (in the reimplementation the host, `GameFlow`, outside a frame) and command `0x1C`
    /// (`0x004CA443`, always won).
    pub fn book_campaign(&mut self, won: bool) {
        for effect in self.book(won) {
            self.unreported.push(effect.clone());
            self.effects.push(effect);
        }
    }

    /// Script action 382 (`FUN_004BD900` -> `FUN_0040DAA0`, E-166): set the profile variable `name` of the active
    /// profile (`m_VarMap`, name German lowercase, search with case sensitivity as in the original, E-252) to `value`;
    /// without an active profile nothing ([`crate::profiles::Profiles::set_current_var`]). The original does not write
    /// `profiles.ini` here; the variable goes out with the next writing of the profiles (for example when booking the
    /// level, [`Self::book_campaign`]).
    pub fn set_profile_var(&mut self, name: &str, value: i32) {
        self.data.profiles.set_current_var(name, value);
    }

    /// Booking without the look-back outside a frame ([`Self::book_campaign`]); returns the effects.
    fn book(&mut self, won: bool) -> [UiEffect; 2] {
        crate::campaign::book(&mut self.data.profiles, &self.data.campaigns, won);
        [UiEffect::BookCampaign { won }, UiEffect::WriteProfiles]
    }

    /// The host reports the running match (`Some` after start or loading, `None` after the end). The menu state starts
    /// at 0 with every new match or without a match (the new engine starts with `+0x2CCC = 0`). A running match carries
    /// its game type into the session (`draft.settings.game_type` = `[GameApp+0xD650]+0x1F4`): after loading a
    /// savegame `Game_LoadLevelFromSavegame` (`0x004C7860`) sets `+0x1F4 = Engine+0x108`; at `0x0D`, `0x0C` and `0x1F`
    /// both are already equal (`+0x1F4` is the source of `Engine+0x108`). Without this, "Continue" of the accounting
    /// would stay visible after a campaign and the loading of a free-play savegame (O-167).
    ///
    /// Without a match it forgets the commands of a remembered confirmation request or message
    /// (`info_ok`/`info_cancel`): they belong to the ended match; a leftover flag `{0x0E}` would otherwise block
    /// `END_*` and the end screen of the next match.
    pub fn set_game(&mut self, game: Option<GameInfo>) {
        match game {
            Some(g) => self.data.draft.settings.game_type = g.game_type,
            None => {
                self.screens.info_ok = None;
                self.screens.info_cancel = None;
            }
        }
        self.data.game = game;
        self.game_menu = 0;
    }

    /// `Game_StartLevel` step 2 (`0x004C85F2`, re/singleplayer-menus 6.1): `MenuManager_CloseAllAndStopMusic` clears
    /// the image table of the desktop ([`crate::desktop::DesktopImages::clear`]); the host stops the menu music itself.
    /// Loading a savegame (`FUN_004C7C20`) does not call it.
    pub fn drop_desktop_images(&mut self) {
        self.tree.images.clear();
    }

    /// Operation lock of the scripts (`Engine+0x600`) of the running match, per frame from the host: writes only
    /// [`GameInfo::ui_locked`] - no commands, the menu state stays (unlike [`Self::set_game`]). Without a match
    /// nothing.
    pub fn set_ui_locked(&mut self, on: bool) {
        if let Some(game) = self.data.game.as_mut() {
            game.ui_locked = on;
        }
    }

    /// Network role of the running match ([`GameInfo::net`]), from the host after a host migration (the new host sees
    /// the speed slider `GAMESPEED`): writes only the role - no commands, the menu state stays (unlike
    /// [`Self::set_game`]). Without a match nothing.
    pub fn set_net_role(&mut self, net: Option<NetRoleView>) {
        if let Some(game) = self.data.game.as_mut() {
            game.net = net;
        }
    }

    /// Mission objectives and tip log of the running match (`FUN_004334E0`, `m_InfoHistory`, re/campaign-ui D6.8,
    /// D6.9), per frame from the host; `None` at the end of the match. Touches neither the command queue nor the menu
    /// state (unlike [`Self::set_game`]); `QUESTTEXT` and `TIPLOG` read the state only when opening (O-365).
    pub fn set_mission(&mut self, mission: Option<MissionInfo>) {
        self.data.mission = mission;
    }

    /// State of the diplomacy dialog (names, colors, nations, results, relation, vision, masks, fixed teams; re/game-ui
    /// D5.7) per frame from the host; `None` at the end of the match. The elements read it every 500 ms
    /// (`timeout 500`). Touches neither queue nor menu state.
    pub fn set_diplomacy(&mut self, view: Option<DiplomacyView>) {
        self.data.diplomacy = view;
    }

    /// State of the network session for the network menus (found games in `LAN1`, lobby), per frame from the host
    /// (pattern [`Self::set_diplomacy`]); `None` without a network session. Touches neither queue nor menu state. The
    /// history of the lobby chat (`CHATFRAME CHAT`, a pointer to `GameApp+0x9F0` in the original) shows the new state
    /// immediately ([`screens::chatframe::chat_text`]).
    pub fn set_net(&mut self, view: Option<NetMenuView>) {
        let text = screens::chatframe::chat_text(view.as_ref());
        if self.screens.lobby_chat.get() != text {
            self.screens.lobby_chat.set(&text);
        }
        self.data.net = view;
    }

    /// Lobby draft: the lines of the lobby as slots (key = line - 1), map description and settings of the session into
    /// the match draft ([`MenuData::draft`]), per frame from the host out of the lobby state. The chosen map
    /// (`draft.map`) stays.
    pub fn set_lobby_draft(
        &mut self,
        slots: Vec<SlotDraft>,
        info: LevelInfoData,
        settings: DraftSettings,
    ) {
        let draft = &mut self.data.draft;
        draft.slots = slots;
        draft.info = info;
        draft.settings = settings;
    }

    /// Enqueues an application command (`FUN_004BCB40`), for example the menu commands after success or failure of the
    /// network session.
    pub fn push_command(&mut self, cmd: AppCmd) {
        self.queue.push(cmd);
    }

    /// Message `MSGBOX` with the text `text_id` of the table and the command `ok` for "OK" (`FUN_004BCF10`,
    /// [`screens::tell`]), for example 15212 with `{0x2D}` after being kicked out or after the host has left.
    pub fn tell(&mut self, text_id: i32, ok: Option<AppCmd>) {
        screens::tell(&mut self.screens, &mut self.queue, text_id, ok);
    }

    /// Sets `NetName` (`wcscpy(EngineConfig+0x13C, ...)`): setting and mirror of the name fields
    /// ([`ScreenState::net_name`]); at most 39 units up to a null character.
    pub fn set_net_name(&mut self, units: &[u16]) {
        let field = units_field(units);
        self.config.net_name = field;
        self.screens
            .net_name
            .set_units(crate::profiles::name_units(&field).to_vec());
    }

    /// Sets `NetSession` (`FUN_004EB190` writes the default name back, re/multiplayer-menus L.1.2): setting and mirror
    /// of `SESSIONNAME` ([`ScreenState::net_session`]); at most 39 units.
    pub fn set_net_session(&mut self, units: &[u16]) {
        let field = units_field(units);
        self.config.net_session = field;
        self.screens
            .net_session
            .set_units(crate::profiles::name_units(&field).to_vec());
    }

    /// Toggles the diplomacy dialog (`FUN_0042E060(open)`, re/game-ui D5.7): key `SHIFT D` (`xbutton(572)`) and button
    /// 580 come via the host with `open = true`, "Back"/ESC of the dialog with `false` via the same function
    /// ([`screens::diplomacy_toggle`]). Under the operation lock ([`GameInfo::ui_locked`]) nothing happens with `open`;
    /// at state 0 state 10 and `{6,"DIPLOMATIE"}` (in single player the match stands still as with the game menu,
    /// commands of the dialog take effect immediately; in a network match the ticks keep running and the commands go
    /// through the turn, re/multiplayer-menus L.7.2, E-320); at state 10 state 0 and `{4,"DIPLOMATIE"}`. Without a
    /// match nothing. `true` if the state has changed.
    pub fn toggle_diplomacy(&mut self, open: bool) -> bool {
        screens::diplomacy_toggle(
            &mut self.game_menu,
            self.data.game.as_ref(),
            &mut self.queue,
            open,
        )
    }

    /// `showchatwindow(k)` (`FUN_0042EA20`): with a match, at menu state 0 and outside the menu level, clear the input
    /// buffer and for `k` = 1...3 enqueue `{6, "chatwindow…"}` ([`screens::chat_show`]); the host checks the
    /// cinemascope beforehand (`InputGate::chat`). The window sets no menu state. `true` if a window is enqueued.
    pub fn show_chat_window(&mut self, window: u8) -> bool {
        screens::chat_show(
            window,
            self.data.game.as_ref(),
            self.game_menu,
            &self.screens.chat_text,
            &mut self.queue,
        )
    }

    /// Screen rectangle of the open chat window (frame `CHATWINDOW…`, as drawn in the last frame), otherwise `None`.
    /// The host does not let mouse button presses inside it through to the match.
    pub fn chat_window_rect(&self, screen: Screen) -> Option<[i32; 4]> {
        self.chat_window_visit().map(|v| to_screen_rect(screen, v))
    }

    /// Does the open **chat window** have keyboard focus (O-393)? Its edit field attaches when opening and focuses
    /// itself (`FUN_0045F9B0`); as long as the window was drawn in the last frame, the host locks the camera keys of
    /// the match. Other edit fields (for example the name when saving in the game menu) do not count: unobserved in
    /// the original, open question (O-393).
    pub fn text_focus(&self) -> bool {
        self.chat_window_visit().is_some()
    }

    /// Rectangle of the chat window in the grid of the root, if it was drawn in the last frame.
    fn chat_window_visit(&self) -> Option<[i32; 4]> {
        let tree = &self.tree;
        dialog_frames(tree).into_iter().find_map(|f| {
            let node = tree.node(f);
            if !screens::chat::DIALOGS.contains(&lower_german(&node.dialog).as_str()) {
                return None;
            }
            node.visit
                .filter(|(pass, _)| *pass == tree.pass)
                .map(|(_, v)| v)
        })
    }

    /// The running match.
    pub fn game(&self) -> Option<&GameInfo> {
        self.data.game.as_ref()
    }

    /// Menu state of the match (`Engine+0x2CCC`): 0 running, 1 game menu (single player: the game stands still, O-143),
    /// 2 network game menu `INGAMEOPTIONS2` (the ticks keep running; after `QUITGAME` until the own defeat also without
    /// a visible menu, E-316), 6 victory or 7 defeat of the local player (end screen, the game stands still;
    /// [`Self::show_game_end`]), 10 diplomacy. It stays at 1 or 2 for all subpages of the game menu until `0x0F` (1),
    /// `0x13` (2), `0x37`, `0x26`, a game end or loading resets it (re/ingame-menus 2).
    pub fn game_menu_state(&self) -> u8 {
        self.game_menu
    }

    /// Opens the game menu (`FUN_0042E150`, re/ingame-menus 1.1): nothing without a match (the callers need the
    /// engine) or at state != 0; otherwise pointer on (`FUN_004CD6A0(1)`), without a network session state 1 and
    /// `{6,"INGAMEOPTIONS"}` at game type -1, otherwise `{6,"INGAMEOPTIONS3"}`; with a network role ([`GameInfo::net`])
    /// state 2 and `{6,"INGAMEOPTIONS2"}` (re/multiplayer-menus L.7.1); `true` if it opens. The resetting of the input
    /// state (`FUN_004BA6D0` -> `FUN_004DD2F0`, effect is a hypothesis) has no counterpart; the menu level
    /// (`Engine+0x108 < -1`) does not exist in the reimplementation (a deviation).
    pub fn open_game_menu(&mut self) -> bool {
        if self.game_menu != 0 {
            return false;
        }
        let Some(game) = self.data.game else {
            return false;
        };
        self.pointer_hidden = false;
        self.game_menu = if game.net.is_some() { NET_GAME_MENU } else { 1 };
        self.queue.push(AppCmd::Open {
            name: game_menu_name(&game).to_string(),
            wait: false,
        });
        true
    }

    /// Game end of the local player (`GameEngine_SetPlayerResult` `0x00430CE0` steps 3-4, re/ingame-menus 6.2): the
    /// host calls it once as soon as the result of the local player is known, and stops the game music beforehand
    /// (`MusicManager_StopMusicAndReset`). `FUN_0042F9A0`: at state 1 close the game menu like `{0x0F}`
    /// (`FUN_0042E290`: `{4, game menu}`, speed from the settings), `{0x34}` (all menus closed), state 0, pointer on;
    /// at state 2 (network game menu or waiting time after `QUITGAME`) `FUN_0042E5D0` without a speed command
    /// ([`Self::leave_net_game_menu`]); then state 7 and `{6,"GAMEEND_DEFEAT"}` or 6 and `{6,"GAMEEND_VICTORY"}` - the
    /// game stands still. The resetting of the inputs (`FUN_004BA6D0`) has no counterpart; the booking of the campaign
    /// (`FUN_004C6B10`) is called by the host afterwards itself via [`Self::book_campaign`] (only at game type -1). For
    /// the report it files [`UiEffect::GameEnd`] (the host ignores it).
    ///
    /// If the message about the end of the network match is pending ([`Self::end_with_message`], "OK" = `{0x0E}`), the
    /// call has no effect: the message carries the end; `{0x34}` would close it, and `END_*` of the end screen would
    /// stay without effect behind its flag. No counterpart in the original, which does not know the end with a message
    /// in the network.
    pub fn show_game_end(&mut self, victory: bool) {
        if self.screens.info_ok == Some(AppCmd::EndMatch) {
            log::info!(
                "End screen has no effect, the message about the end of the network match is pending"
            );
            return;
        }
        if let Some(game) = self.data.game
            && self.game_menu == 1
        {
            self.close_game_menu(&game);
        }
        self.leave_net_game_menu();
        // With diplomacy open (state 10) close the dialog first (`FUN_0042F9A0` knows only states 1 and 2, `{0x34}`
        // would close the frame anyway).
        if self.game_menu == screens::DIPLOMACY_MENU_STATE {
            self.toggle_diplomacy(false);
        }
        self.queue.push(AppCmd::CloseAll);
        self.pointer_hidden = false;
        let state = if victory { 6 } else { 7 };
        self.game_menu = state;
        self.queue.push(AppCmd::Open {
            name: game_end_name(state).to_string(),
            wait: false,
        });
        let effect = UiEffect::GameEnd { victory };
        self.unreported.push(effect.clone());
        self.effects.push(effect);
    }

    /// End of a match with a message instead of an end screen (network match): like [`Self::show_game_end`]
    /// (`FUN_0042F9A0`) first the game menu (also the network game menu or the waiting time after `QUITGAME`, state 2,
    /// without a speed command) or diplomacy is closed, `{0x34}`, pointer on - the state stays 0 -, then `MSGBOX` with
    /// the free text ([`Self::tell_text`]), whose "OK" enqueues `{0x0E}`: the game end (after `Go`, the way in the
    /// network leads via the accounting ([`Self::set_accounting`]) into the main menu; before `Go` directly into the
    /// main menu ([`Self::open_main_menu`])). The host stops the game music itself beforehand.
    ///
    /// The message carries the only `{0x0E}` of this end: if the player has clicked `END_*` earlier in the same frame,
    /// it removes the `{0x0E}` enqueued by that (otherwise the accounting would open under the message); `{4, frame}`
    /// and `{0x15}` of the chain stay and only close. An `END_*` afterwards stays without effect as long as the message
    /// is pending (`screens::game_end`).
    pub fn end_with_message(&mut self, text: String) {
        self.queue.retain(|c| *c != AppCmd::EndMatch);
        if let Some(game) = self.data.game
            && self.game_menu == 1
        {
            self.close_game_menu(&game);
        }
        self.leave_net_game_menu();
        if self.game_menu == screens::DIPLOMACY_MENU_STATE {
            self.toggle_diplomacy(false);
        }
        self.queue.push(AppCmd::CloseAll);
        self.pointer_hidden = false;
        self.tell_text(text, Some(AppCmd::EndMatch));
    }

    /// Message `MSGBOX` with free text and the command `ok` for "OK" ([`screens::tell_text`]): like the messages of the
    /// logic (`FUN_004BCF10`), but with a text that the text table does not know (e.g. the name of a network
    /// savegame).
    pub fn tell_text(&mut self, text: String, ok: Option<AppCmd>) {
        screens::tell_text(&mut self.screens, &mut self.queue, text, ok);
    }

    /// Close all open menus (`{0x34}`, `MenuManager_CloseAll`): before the network start from the command line, which
    /// loads from the main menu without a menu command - in the original the lobby closes its menus with the commands
    /// of its start button.
    pub fn close_all_menus(&mut self) {
        self.queue.push(AppCmd::CloseAll);
    }

    /// The main menu `{5,"start"}` (like "Main menu" of the accounting, `0x004D94BD`): after the end of a network
    /// match that did not reach `Go` (after `Go` via the accounting). The host has ended the match beforehand
    /// (`set_game(None)`).
    pub fn open_main_menu(&mut self) {
        self.queue.push(AppCmd::Open {
            name: "start".to_string(),
            wait: true,
        });
    }

    /// Step 6 of `0x0E` (re/accounting 6): store the accounting that the host has built (table and game time text,
    /// [`MenuData::accounting`]) and enqueue `{6,"accounting"}` (`FUN_004E3010(..., "accounting", 0)`, flag 0). The
    /// host has ended the match beforehand (`set_game(None)`).
    pub fn set_accounting(&mut self, view: AccountingView) {
        self.data.accounting = Some(view);
        self.queue.push(AppCmd::Open {
            name: "accounting".to_string(),
            wait: false,
        });
    }

    /// After a failed campaign start (`0x0C`, a deviation): `{5,"selectcampaign"}`. The original does not evaluate the
    /// result of `FUN_004C8EC0` (`0x004CA26A`) and stays without game and without menu; the reimplementation opens the
    /// campaign selection, from which every path to `0x0C` comes (`LEVELSTART`, "Continue" of the accounting, `0x28`
    /// after a video).
    pub fn reopen_campaign_menu(&mut self) {
        self.queue.push(AppCmd::Open {
            name: "selectcampaign".to_string(),
            wait: true,
        });
    }

    /// Reopen the menu after a failed load (a deviation): with a match the menu the loading came from - at state 6/7
    /// the end screen (`GAMEEND_VICTORY`/`GAMEEND_DEFEAT`), otherwise `{6, game menu}` (the state stays, it is
    /// still 1), without a match `{5,"singleplayermenu"}`. The original ends the match before loading and stays without
    /// a menu on an error (re/ingame-menus, section "Open points" 4).
    pub fn reopen_menu(&mut self) {
        let cmd = match &self.data.game {
            Some(_) if self.game_menu == 6 || self.game_menu == 7 => AppCmd::Open {
                name: game_end_name(self.game_menu).to_string(),
                wait: false,
            },
            Some(game) => AppCmd::Open {
                name: game_menu_name(game).to_string(),
                wait: false,
            },
            None => AppCmd::Open {
                name: "singleplayermenu".to_string(),
                wait: true,
            },
        };
        self.queue.push(cmd);
    }

    /// Quicksave (`FUN_0042F490`, re/ingame-menus 5): only with a match and at state 0 (`+0x2CCC == 0`; the menu level
    /// `+0x108 < -1` is missing in the reimplementation) - name to `GameApp+0x177C`, enqueue `{0x26}`. The host chooses
    /// the name (text 15786 + counter, `quicksave1`/`quicksave2`); an existing file is overwritten without a
    /// confirmation request (O-151).
    pub fn quick_save(&mut self, name: &str) {
        if self.data.game.is_none() || self.game_menu != 0 {
            return;
        }
        self.screens.save_name.set(name);
        self.queue.push(AppCmd::SubmitSave);
    }

    /// Quickload (`FUN_0042F5C0`, re/ingame-menus 5): only with a match, at state 0 and without a network session -
    /// state 1 (the game stands still) and `{6,"ingameloadgame"}`, **without** a directory change and without touching
    /// the return command (O-146). `true` if it opens. The resetting of the inputs (`FUN_004BA6D0`) has no
    /// counterpart. The original turns the pointer on only under the UI lock `Engine+0x600` (`0x0042F610`); under it
    /// F4 is locked (`keys.ini` path, `GameFlow::key`), so the branch is unreachable in the reimplementation and is
    /// missing (O-371: in the cinemascope without a lock the pointer stays invisible).
    pub fn quick_load(&mut self) -> bool {
        if self.data.game.is_none() || self.game_menu != 0 {
            return false;
        }
        self.game_menu = 1;
        self.queue.push(AppCmd::Open {
            name: "ingameloadgame".to_string(),
            wait: false,
        });
        true
    }

    /// A savegame has been submitted (`0x26`) but not yet written (`0x0B`): the original locks the ticks
    /// (`Engine+0x115`), the host does not step in the meantime.
    pub fn saving(&self) -> bool {
        self.screens.saving
    }

    /// Key names (`FUN_0049C340`, e.g. `ESCAPE`, `F1`) of the key events of the last [`Self::frame`] that neither the
    /// broadcast `0x17E`/`0x17F` nor a widget took and that no video aborted, in order of arrival. In the original such
    /// a key goes on to the game (`FUN_004C6050`: ESC flag `Engine+0x4F1`, `keys.ini`); the host takes that over. Every
    /// frame starts the list anew.
    pub fn take_untaken_keys(&mut self) -> Vec<String> {
        std::mem::take(&mut self.untaken_keys)
    }

    /// The most recently computed menu state of the text set (tests).
    #[cfg(test)]
    pub(crate) fn text_context(&self) -> Option<&TextContext> {
        self.text_ctx.as_ref().map(|c| &c.context)
    }

    /// Menu state of the text set for the current key layout and map; recomputed when `key_map`, the details of the
    /// chosen map or the match draft have changed (the original reads everything at every sentence). Runs after the
    /// steps of the frame: a map choice shows in the same frame. `[CURLEVELINFOS]` comes as in the original from the
    /// session (`FUN_004B9C90(1)`, [`level_infos`]), not from the chosen map; without a chosen map, name, size and
    /// players stay empty (they insert nothing, like a null pointer).
    fn refresh_text_context(&mut self, texts: &Texts) {
        let cached = self.text_ctx.as_ref();
        let net = LevelNetInfo::from_net(self.data.net.as_ref());
        let infos = match cached {
            Some(c) if c.draft == self.data.draft && c.net == net => c
                .context
                .cur_level
                .as_ref()
                .map(|l| l.infos.clone())
                .unwrap_or_default(),
            _ => level_infos(&self.data.draft, net.as_ref(), texts),
        };
        let cur_level = Some(CurLevelText {
            infos,
            ..self.data.level.text.clone().unwrap_or_default()
        });
        let stale = cached.is_none_or(|c| {
            c.key_map != self.config.key_map
                || c.draft != self.data.draft
                || c.net != net
                || c.context.cur_level != cur_level
        });
        if stale {
            let context = screens::text_context(&self.config.key_map, texts, cur_level);
            self.text_ctx = Some(TextCache {
                key_map: self.config.key_map.clone(),
                draft: self.data.draft.clone(),
                net,
                context,
            });
        }
    }

    /// Buffer input for the next frame. The host delivers movement, press, release, wheel and keys; `MenuApp` detects
    /// the double-click itself on release (`FUN_004145B0` with `DoubleClickTimeout`, 12.8: `DoubleClick` follows `Up`).
    /// The frame event is triggered by [`Self::frame`] itself; a `UiInput::Frame` from the host is ignored (it would
    /// otherwise process deletions in the middle of the mouse sequence).
    pub fn input(&mut self, input: UiInput, screen: Screen, now: u64) {
        match input {
            UiInput::Key { .. } => self.keys_in.push((input, screen)),
            UiInput::Frame => {}
            UiInput::Up(b) => {
                self.mouse_in.push((UiInput::Up(b), screen));
                let timeout = u32::try_from(self.config.double_click_timeout).unwrap_or(0);
                if self.clicks.release(b, now, timeout) {
                    self.mouse_in.push((UiInput::DoubleClick(b), screen));
                }
            }
            other => self.mouse_in.push((other, screen)),
        }
    }

    /// One frame (12.1): keyboard -> mouse -> frame event -> pass -> drawing -> one command.
    /// Returns the draw commands of this frame; `ctx.screen` applies to pass and drawing, every buffered input runs
    /// with its own screen.
    pub fn frame(&mut self, ctx: &mut UiContext, now: u64) -> Vec<DrawCmd> {
        let mark = self.effects.len();
        // `IsMenuLevel` at build time: building a dialog reads the matching version of the `.dlg`; open dialogs stay
        // as they were built (O-438).
        self.menu.set_menu_level(self.config.is_menu_level);
        self.tree.world_shown = self.level_world.shown;
        for e in std::mem::take(&mut self.unreported) {
            self.remember(now, &e);
        }
        self.untaken_keys.clear();
        for (input, screen) in std::mem::take(&mut self.keys_in) {
            if let UiInput::Key { held, ch } = &input {
                let taken = self.step(ctx, screen, now, |tree, _, _, env| {
                    dispatch_key(tree, held, *ch, env)
                });
                // If no widget takes the key (ESC/RETURN not even as a broadcast), it reaches the menu root:
                // `0x0045CAB0` (vtable slot `0x58`) stops an abortable video without checking the code (O-68).
                // With `LAN4` on top, ESC is the exit ([`screens::lan4::escape_leaves`]).
                // Otherwise it stays for the host ([`Self::take_untaken_keys`]).
                if !taken {
                    let name = chord_name(held);
                    if self.video.as_ref().is_some_and(|v| v.abortable) {
                        self.stop_video(None);
                        continue;
                    }
                    let lan4_exit = match name.as_str() {
                        "ESCAPE" => {
                            screens::lan4::escape_leaves(self.menu.open_names(), self.queue.iter())
                        }
                        _ => None,
                    };
                    match lan4_exit {
                        Some(true) => self.queue.push(AppCmd::LeaveLobby),
                        Some(false) => {}
                        None => self.untaken_keys.push(name),
                    }
                }
            }
        }
        for (input, screen) in std::mem::take(&mut self.mouse_in) {
            self.step(ctx, screen, now, |tree, _, pointer, env| {
                dispatch_mouse(tree, pointer, &input, env);
            });
        }
        let screen = ctx.screen;
        self.step(ctx, screen, now, |tree, _, pointer, env| {
            dispatch_mouse(tree, pointer, &UiInput::Frame, env);
        });
        self.step(ctx, screen, now, |tree, menu, _, env| {
            tick_tree(tree, env, &mut |t, frame, e| {
                menu.transition_done(t, frame, e)
            });
        });
        self.refresh_text_context(ctx.texts);
        // Frame tick (`FUN_0045CB30` before drawing): if the last frame is over, the video ends before drawing
        // (deviation: the video ends from `frames · 40` ms on).
        if self
            .video
            .as_ref()
            .is_some_and(|v| v.frame_at(now).is_none())
        {
            self.stop_video(None);
        }
        let video = self.video.as_ref().map(|v| {
            let dst = video_rect(ctx.screen, v.info.height);
            DrawCmd::Quad {
                texture: Some(v.texture),
                dst,
                uv: video_uv(dst),
                color: 0xFFFF_FFFF,
                filter: Filter::Linear,
                blend: Blend::Alpha,
            }
        });
        let text = self.text_ctx.as_ref().map(|c| &c.context);
        let out = paint_ui_with(&self.tree, ctx, now, text, video.as_ref());
        if let Some(cmd) = self.queue.pop_ready(&self.tree, now, self.video.is_some()) {
            self.execute(cmd, ctx, now);
        }
        for i in mark..self.effects.len() {
            let e = self.effects[i].clone();
            self.remember(now, &e);
        }
        out
    }

    /// Collect the gathered effects (in order of creation).
    pub fn take_effects(&mut self) -> Vec<UiEffect> {
        std::mem::take(&mut self.effects)
    }

    /// Enqueues `Open { name, wait: true }` (like command 5); `name` must be a dialog of the library (map key,
    /// case-insensitive), otherwise an error message with the known names.
    pub fn open_by_name(&mut self, name: &str) -> Result<(), String> {
        if self.menu.library().expanded(name).is_none() {
            return Err(format!(
                "unknown dialog \"{name}\" (known: {})",
                self.menu.library().names().join(", ")
            ));
        }
        self.queue.push(AppCmd::Open {
            name: lower_german(name),
            wait: true,
        });
        Ok(())
    }

    /// The host reports an opened video (`FUN_0045D560` after `_BinkOpen`, `0x0045D658`...): frame 0 is under
    /// `texture`, the video starts at time `now` (that of the frame whose effect `PlayVideo` it answers). A running
    /// video ends beforehand with [`UiEffect::StopVideo`] (`FUN_0045D560` stops first, `0x0045D584`).
    pub fn video_started(&mut self, path: &str, info: VideoInfo, texture: TextureId, now: u64) {
        self.stop_video(Some(now));
        self.video = Some(VideoPlayback {
            path: path.to_string(),
            info,
            started_ms: now,
            abortable: self.video_abortable,
            texture,
        });
    }

    /// The host ends the running video because a later frame cannot be decoded: like an abort by key (`0x0045CAB0` ->
    /// `FUN_0045CA70`), also with a non-abortable video. The effect [`UiEffect::StopVideo`] comes into the report with
    /// the time `now`; the queue keeps running. Without a video nothing happens.
    pub fn end_video(&mut self, now: u64) {
        self.stop_video(Some(now));
    }

    /// End the video (`FUN_0045CA70`: `root+0xB8 = 0`); on [`UiEffect::StopVideo`] the host closes file and sound. With
    /// `history` (time) the effect also enters the report if it arises outside a frame.
    fn stop_video(&mut self, history: Option<u64>) {
        if let Some(v) = self.video.take() {
            let e = UiEffect::StopVideo { path: v.path };
            if let Some(now) = history {
                self.remember(now, &e);
            }
            self.effects.push(e);
        }
    }

    /// An effect into the look-back of the report.
    fn remember(&mut self, now: u64, e: &UiEffect) {
        if self.history.len() == REPORT_EFFECTS {
            self.history.pop_front();
        }
        self.history.push_back((now, e.clone()));
    }

    /// Request a video (`FUN_0045D560(path, 1)`): a running one ends first (`0x0045D584`), then the host opens it.
    fn request_video(&mut self, path: String, abortable: bool) {
        self.stop_video(None);
        self.video_abortable = abortable;
        self.effects.push(UiEffect::PlayVideo { path, abortable });
    }

    /// The running video.
    pub fn video(&self) -> Option<&VideoPlayback> {
        self.video.as_ref()
    }

    /// Frame of the running video at time `now` ([`VideoPlayback::frame_at`]).
    pub fn video_frame(&self, now: u64) -> Option<u32> {
        self.video.as_ref().and_then(|v| v.frame_at(now))
    }

    /// The mouse pointer is off.
    pub fn pointer_hidden(&self) -> bool {
        self.pointer_hidden
    }

    /// Pointer off or on (`Cursor3D_SetVisible` `0x004CD6A0`) from the match: the edges of the cinemascope
    /// (`FUN_00430920`: start off only in the campaign, end on; re/campaign-ui D6.17), also loading in the middle of
    /// the cinemascope of the campaign; `GameFlow::refresh_lock` calls it. Videos and start sequence still switch it
    /// themselves.
    pub fn set_pointer_hidden(&mut self, hidden: bool) {
        self.pointer_hidden = hidden;
    }

    /// Probe video by name (control command `ui_video`), modeled on `videostart` in the menu `campaignvideo`
    /// (`FUN_004DA8A0` `0x004DAA14`...`0x004DAB18`), but without a campaign: the open menus closed (`CloseAll` instead
    /// of the three closing commands code 4), `WaitDialogs` (`0x15`), `PlayVideo("data/videos/<name>.bik")` (`0x1D`),
    /// `WaitVideo` (`0x1A`), `AfterProbeVideo` (pointer on, menu music anew - the part of `0x28` without clearing the
    /// queue and without `selectcampaign`), afterwards the menus that were open at the command open again in the same
    /// order (code 5). The path of the original with `0x1C` and `0x28` is taken by `VIDEOSTART` (`screens::campaign`).
    /// During the video no menu is open. Without open menus, closing, waiting and reopening are omitted (they would
    /// have nothing to do and would only cost three frames). The name consists only of `A`-`Z`, `a`-`z`, `0`-`9`, `_`
    /// and `-` (no path, no extension); otherwise an error message.
    ///
    /// Only with an idle UI: if a menu transition is running, commands are waiting (also the start sequence), a wait
    /// state applies or a video is running, the command is rejected with an error message and enqueues nothing. In the
    /// original `videostart` is a button that triggers only with idle menus (`0x12F` refuses as long as orders are
    /// open, O-21); otherwise `CloseAll` could pass over a still waiting dialog that opens afterwards, and
    /// `WaitDialogs` would never end.
    pub fn play_video_by_name(&mut self, name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("video name missing".to_string());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "video name \"{name}\" invalid: only A-Z, a-z, 0-9, _ and - are allowed (without path and extension)"
            ));
        }
        if self.menu.busy()
            || !self.queue.is_empty()
            || self.queue.waiting()
            || self.video.is_some()
        {
            return Err(format!(
                "UI busy (menu transition, enqueued commands or video): \
                 ui_video \"{name}\" only when it is idle (ui_state: busy false, queued 0, without video)"
            ));
        }
        let reopen = reopen_names(self.menu.open_names());
        if !reopen.is_empty() {
            self.queue.push(AppCmd::CloseAll);
            self.queue.push(AppCmd::WaitDialogs);
        }
        self.queue
            .push(AppCmd::PlayVideo(format!("data/videos/{name}.bik")));
        self.queue.push(AppCmd::WaitVideo);
        self.queue.push(AppCmd::AfterProbeVideo);
        for name in reopen {
            self.queue.push(AppCmd::Open { name, wait: true });
        }
        Ok(())
    }

    /// One application command (`GameApp_DispatchEvent` `0x004C9990`, re/menu-flow, section "Commands"). Wait commands
    /// have already been evaluated by the queue; what the reimplementation does not (yet) execute goes as
    /// [`UiEffect::Unhandled`] to the host (menu level, GameSpy). `0x32` clears the key layout and reloads
    /// `defaultkeys.ini` (O-102). The UI requests videos (`0x1B`, `0x1D`) with [`UiEffect::PlayVideo`]; `0x24`/`0x28`
    /// turn the pointer on again (re/video, section "After the video"). `0x0F`/`0x37` close the game menu with
    /// [`UiEffect::ResumeGame`], `0x1F` becomes [`UiEffect::ReloadLevel`] with a match.
    fn execute(&mut self, cmd: AppCmd, ctx: &mut UiContext, now: u64) {
        let screen = ctx.screen;
        match cmd {
            AppCmd::Open { name, wait } => {
                self.step(ctx, screen, now, |tree, menu, _, env| {
                    menu.open(tree, &name, wait, env);
                });
            }
            AppCmd::Close { name, wait } => {
                self.step(ctx, screen, now, |tree, menu, _, env| {
                    menu.close(tree, &name, wait, env);
                });
            }
            AppCmd::CloseAll => {
                self.step(ctx, screen, now, |tree, menu, _, env| {
                    menu.close_all(tree, env);
                });
            }
            AppCmd::WaitDialogs | AppCmd::WaitVideo | AppCmd::WaitMs(_) => {}
            AppCmd::Quit => self.effects.push(UiEffect::Quit),
            AppCmd::ResetKeys => {
                let defaults = screens::load_default_keys(ctx.vfs);
                screens::reset_keys(&mut self.config.key_map, &defaults);
            }
            // `0x1B(n)` (`0x004CA3E7`...`0x004CA441`): path by `n`, `FUN_0045D560(path, 1)`; no mode switch and no
            // pointer change (the program start does both beforehand). Bit `0x40` has no counterpart.
            AppCmd::Video(n) => match INTRO_VIDEOS.get(usize::from(n)) {
                Some(path) => self.request_video((*path).to_string(), true),
                None => log::warn!("Command 0x1B: video {n} does not exist (only 0 to 2)"),
            },
            // `0x1D(path)` (`0x004CA451`...`0x004CA4E2`): pointer off (`FUN_004CD6A0(0)`), `FUN_0045D560(path, 1)`;
            // without the switch to 640 x 480 (O-66).
            AppCmd::PlayVideo(path) => {
                self.pointer_hidden = true;
                self.request_video(path, true);
            }
            // `0x24` (`0x004C9E69`): pointer on (`FUN_004CD6A0(1)`); the reimplementation does not restore the display
            // mode, it never changed it.
            AppCmd::AfterIntro => self.pointer_hidden = false,
            // `0x28` (`0x004CA4F0`...`0x004CA643`, re/singleplayer-menus part 1, 6.1): bit `0x40` and display mode have
            // no counterpart, pointer on; game type of the session `+0x1F4 = -1`; then `FUN_004BA6B0`:
            // - true and level (`FUN_004BA550` == 0): `FUN_004B9830(entry)` (the path; in the reimplementation `0x0C`
            //   reads it via `current_entry`), clear the queue (`FUN_004BC6C0`), `{0x0C}` - no dialog, no music;
            // - otherwise (also a video that has just become available, O-158): clear, `MenuMusic_Restart`, open
            //   `selectcampaign` (`FUN_004E3010(..., 0)`, directly, not enqueued).
            // The clearing discards the `{5,"selectcampaign"}` that `VIDEOSTART` enqueues after `{0x28}` (O-33).
            AppCmd::AfterVideo => {
                self.pointer_hidden = false;
                self.data.draft.settings.game_type = -1;
                let (profiles, campaigns) = (&self.data.profiles, &self.data.campaigns);
                let level = crate::campaign::auto_continue(profiles, campaigns, true)
                    && crate::campaign::current_entry(profiles, campaigns)
                        .is_some_and(|e| e.kind == EntryKind::Level);
                self.queue.clear();
                if level {
                    self.queue.push(AppCmd::StartCampaignLevel);
                } else {
                    self.effects.push(UiEffect::RestartMusic);
                    self.step(ctx, screen, now, |tree, menu, _, env| {
                        menu.open(tree, "selectcampaign", false, env);
                    });
                }
            }
            // Probe video of `ui_video`: only pointer on and `MenuMusic_Restart`, like `0x28` without a campaign.
            AppCmd::AfterProbeVideo => {
                self.pointer_hidden = false;
                self.effects.push(UiEffect::RestartMusic);
            }
            // `0x1C` (`0x004CA443`): `FUN_004C6B10(1)` - book the entry as won, write `profiles.ini`.
            AppCmd::BookCampaign => {
                let effects = self.book(true);
                self.effects.extend(effects);
            }
            // `0x29` (`0x004C9D73`...`0x004C9E14`): `FUN_004C8550(i)` - remove the profile (index adjusted, a
            // deviation), delete the savegames `savegame/*.<GUID>` (`FUN_004C6D70`, the host), write `profiles.ini`;
            // then `{3,"selectprofile"}`, `{5,"selectprofile"}` (rebuild: Sel = cur). `NetName` stays (O-122). An
            // invalid index deletes `*.FFFFFFFF` in the original (GUID -1); the reimplementation deletes nothing.
            AppCmd::DeleteProfile(i) => {
                if let Some(guid) = self.data.profiles.delete(i) {
                    self.effects.push(UiEffect::DeleteProfileSaves { guid });
                }
                self.effects.push(UiEffect::WriteProfiles);
                for cmd in [
                    AppCmd::Close {
                        name: "selectprofile".to_string(),
                        wait: true,
                    },
                    AppCmd::Open {
                        name: "selectprofile".to_string(),
                        wait: true,
                    },
                ] {
                    self.queue.push(cmd);
                }
            }
            // `0x0D` (`0x004CA274`...`0x004CA28A`): `Game_StartLevel` first removes the open slots from the session
            // (`FUN_004E9800`, `0x004C85CB`) and starts with what is left there; the host takes over the start.
            AppCmd::StartMatch => {
                self.data.draft.remove_open_slots();
                let request = MatchRequest::from_draft(&self.data.draft);
                self.effects.push(UiEffect::StartMatch(request));
            }
            // `0x0F` (`0x004CA294`...`0x004CA2D3`): with engine `FUN_0042E290`, without it a fixed
            // `{4,"ingameoptions"}` (`FUN_004E2E80("ingameoptions", 0)`, even if `ingameoptions3` is open, O-195).
            AppCmd::CloseGameMenu => match self.data.game {
                Some(game) => self.close_game_menu(&game),
                None => self.step(ctx, screen, now, |tree, menu, _, env| {
                    menu.close(tree, "ingameoptions", false, env);
                }),
            },
            // `0x13` (`0x004CA328`...`0x004CA367`, re/multiplayer-menus L.9): with engine `FUN_0042E5D0` (without a
            // speed command), without it directly `FUN_004E2E80("ingameoptions2", 0)` - no lobby command.
            AppCmd::CloseNetGameMenu => match self.data.game {
                Some(_) => self.close_net_game_menu(),
                None => self.step(ctx, screen, now, |tree, menu, _, env| {
                    menu.close(tree, NET_GAME_MENU_NAME, false, env);
                }),
            },
            // `0x37` (`0x004C99CD`...`0x004C99E0`): only with engine `FUN_0042F6B0` - at state 1: state 0 and resume,
            // without speed and without closing (`INGAMELOADGAME CANCEL` has already closed itself). It turns the
            // pointer off only under the UI lock of the scripts (`Engine+0x600`) (`0x0042F6CD`, O-379).
            AppCmd::ResumeAfterLoadDialog => {
                if let Some(game) = self.data.game
                    && self.game_menu == 1
                {
                    if game.ui_locked {
                        self.pointer_hidden = true;
                    }
                    self.game_menu = 0;
                    self.effects.push(UiEffect::ResumeGame { game_speed: None });
                }
            }
            // `0x1F` (`0x004C9940`): end and restart only with a loaded level (`GameApp+0xD648`); in the main menu
            // (`VIDEOOPTIONS ACCEPT`) without effect.
            AppCmd::ReloadLevel => {
                if self.data.game.is_some() {
                    self.effects.push(UiEffect::ReloadLevel);
                }
            }
            // `0x0A` (`0x004CA224`): `FUN_004BAF40` (end game), `FUN_004C8DE0` (path
            // `"%s/%s.%03X"`, read); the host takes over both with the name from `GameApp+0x177C` and checks the file
            // before ending (a deviation).
            AppCmd::LoadGame => self.effects.push(UiEffect::LoadGame {
                name: self.screens.save_name.get(),
            }),
            // `0x26` (`Game_SubmitSaveGameCommand(0)` `0x004BEEA0`): with engine state 0 (`FUN_0042E4D0(0)`, the game
            // runs again), message `0x50000054` with the name; in single player immediately
            // `GameEngine_CmdSaveGame` (`0x0042DF50`): name back to `+0x177C`, `Engine+0x115 = 1` (ticks
            // locked, here [`ScreenState::saving`]), `{6,"SAVINGMESSAGE"}`, `{0x0B}`. Without an engine the original
            // crashes (`GameWorld_ExecuteCommand` on the missing world; reachable only via the locked button or the
            // control channel), the reimplementation does nothing (O-197).
            AppCmd::SubmitSave => {
                if self.data.game.is_none() {
                    log::warn!("Command 0x26 without a running match: no savegame");
                    return;
                }
                self.game_menu = 0;
                self.effects.push(UiEffect::ResumeGame { game_speed: None });
                self.screens.saving = true;
                self.queue.push(AppCmd::Open {
                    name: "savingmessage".to_string(),
                    wait: false,
                });
                self.queue.push(AppCmd::WriteSave);
            }
            // `0x0B` (`FUN_004C7D50(0)`): write (the host), `Engine+0x115 = 0`, close `savingmessage` immediately
            // (`FUN_004E2E80("savingmessage", 0)`, directly, not enqueued). If writing fails, lock and message stay in
            // the original (O-149); the reimplementation always closes, the host reports the error (a deviation).
            AppCmd::WriteSave => {
                self.screens.saving = false;
                if self.data.game.is_some() {
                    self.effects.push(UiEffect::SaveGame {
                        name: self.screens.save_name.get(),
                    });
                }
                self.step(ctx, screen, now, |tree, menu, _, env| {
                    menu.close(tree, "savingmessage", false, env);
                });
            }
            // `0x2A` (`0x004C9D4B`...`0x004C9D69`): `FUN_004BF9A0` - `DeleteFileA(savegame/<+0x177C>.<GUID>)` (the
            // host); then `{3, name}`, `{5, name}` with the frame name from the command.
            AppCmd::DeleteSave(frame) => {
                self.effects.push(UiEffect::DeleteSave {
                    name: self.screens.save_name.get(),
                    dir: self.screens.save_dir,
                });
                self.queue.push(AppCmd::Close {
                    name: frame.clone(),
                    wait: true,
                });
                self.queue.push(AppCmd::Open {
                    name: frame,
                    wait: true,
                });
            }
            // `0x19` (`FUN_004C9000`): play the recording `GameApp+0x177C` - the host does the start.
            AppCmd::PlayReplay => self.effects.push(UiEffect::PlayReplay {
                name: self.screens.save_name.get(),
            }),
            // `0x0E` (`0x004C9F77`...`0x004CA03B`, re/accounting 6): table, game time text and game end are done by the
            // host ([`UiEffect::EndMatch`]), which then calls [`Self::set_accounting`].
            AppCmd::EndMatch => self.effects.push(UiEffect::EndMatch),
            // `0x0C` (`0x004CA253`, `FUN_004C8EC0(path, -1)`, re/singleplayer-menus part 1, 6.2): game type of the
            // session `+0x1F4 = -1` (campaign; `+0x1F8`/`+0x1F0`/`+0x1EC` and the slots from the level header are not
            // needed by the reimplementation, `Resourcen`/`TimeLimit` stay, O-142), then `Game_StartLevel` with the
            // path of the current entry and the difficulty `GameApp+0x19B4` - the host takes that over
            // ([`UiEffect::StartCampaignLevel`]). Without an entry or path (only after O-203/O-205; an exception while
            // copying the path in the original) the reimplementation starts nothing and reopens `selectcampaign`
            // (O-206).
            AppCmd::StartCampaignLevel => {
                self.data.draft.settings.game_type = -1;
                let entry =
                    crate::campaign::current_entry(&self.data.profiles, &self.data.campaigns)
                        .filter(|e| !e.path.is_empty());
                match entry {
                    Some(e) => self.effects.push(UiEffect::StartCampaignLevel {
                        level: e.path.clone(),
                        difficulty: self.data.campaign_difficulty,
                    }),
                    None => {
                        let f = &self.data.profiles.file;
                        log::warn!(
                            "Command 0x0C: campaign {} has no entry {} with a level path - no start",
                            f.current_campaign,
                            f.current_level
                        );
                        self.queue.push(AppCmd::Open {
                            name: "selectcampaign".to_string(),
                            wait: true,
                        });
                    }
                }
            }
            // `0x27` (`0x004C9E19`...`0x004C9E44`): with engine and pause (`+0x24E4`) pause off (the pause does not
            // exist yet in the reimplementation); then the message `{0x50000056, 2, slot, 1}`, in single player
            // immediately `GameEngine_SetPlayerResult(slot, 1)` - the host executes that ([`UiEffect::Surrender`]).
            // Without an engine the original sends the message anyway (to the missing world); the reimplementation does
            // nothing.
            AppCmd::Surrender { slot } => {
                if self.data.game.is_none() {
                    log::warn!("Command 0x27 without a running match: nobody surrenders");
                    return;
                }
                self.effects.push(UiEffect::Surrender { slot });
            }
            // `7`, `8`, `9`, `0x2D`, `0x2F` (re/multiplayer-menus L.9): joining, hosting, closing and the consequences
            // of being kicked out and of the host leaving are done by the host with the network session; it enqueues
            // the menu commands of success or failure via [`Self::push_command`]. Likewise `0x12` and `0x14` (host and
            // start a network savegame).
            cmd @ (AppCmd::Join
            | AppCmd::Host
            | AppCmd::CloseNet
            | AppCmd::LeaveLobby
            | AppCmd::HostGone
            | AppCmd::HostSavedGame
            | AppCmd::StartSavedGame) => self.effects.push(UiEffect::Net(cmd)),
            // `0x21` (`FUN_004C9140`) and `0x20` (`FUN_004BA670`): the host runs the menu level.
            AppCmd::LoadMenuLevel(path) => self
                .effects
                .push(UiEffect::MenuLevel(MenuLevelEffect::Load { path })),
            AppCmd::HideLevelBackground => self
                .effects
                .push(UiEffect::MenuLevel(MenuLevelEffect::Hide)),
            other => self.effects.push(UiEffect::Unhandled(other)),
        }
    }

    /// `FUN_0042E290` (re/ingame-menus 1.2, O-153): only at state 1 - state 0, `{4,"INGAMEOPTIONS"}` or
    /// `{4,"INGAMEOPTIONS3"}` by game type, then `Engine+0x2D30 = EngineConfig.GameSpeed` directly (without command
    /// `0x6E`, without a message) as [`UiEffect::ResumeGame`]; `FUN_00476F40(ShowOccluded)` is missing in the
    /// reimplementation (the map view does not yet evaluate `ShowOccluded`, O-153). The cinemascope (`Engine+0x104`)
    /// blocks taking over the game speed; the host checks that when applying it. Under the UI lock `+0x600`
    /// ([`GameInfo::ui_locked`]) the pointer goes off (`0x0042E2BE`, O-371, O-379), otherwise it stays as ESC set it.
    /// `GameSpeed` is an `int`; the reimplementation clamps it to 0...255.
    fn close_game_menu(&mut self, game: &GameInfo) {
        if self.game_menu != 1 {
            return;
        }
        if game.ui_locked {
            self.pointer_hidden = true;
        }
        self.game_menu = 0;
        self.queue.push(AppCmd::Close {
            name: game_menu_name(game).to_string(),
            wait: false,
        });
        let speed = u8::try_from(self.config.game_speed.clamp(0, 255)).unwrap_or(u8::MAX);
        self.effects.push(UiEffect::ResumeGame {
            game_speed: Some(speed),
        });
    }

    /// `FUN_0042E5D0` (re/multiplayer-menus L.7.1, L.9): only at state 2 - state 0 and `{4,"INGAMEOPTIONS2"}`. The
    /// original then enqueues `0x6E` with its own setting `GameSpeed` if it differs from the running game speed
    /// (without a host check); the reimplementation sends no speed command and files no [`UiEffect::ResumeGame`] either
    /// (E-265: game speed only from the host via the keys). `FUN_00476F40(ShowOccluded)` is missing as with
    /// [`Self::close_game_menu`] (O-153). It does not touch the pointer.
    fn close_net_game_menu(&mut self) {
        if self.game_menu != NET_GAME_MENU {
            return;
        }
        self.game_menu = 0;
        self.queue.push(AppCmd::Close {
            name: NET_GAME_MENU_NAME.to_string(),
            wait: false,
        });
    }

    /// Game end at state 2 (`FUN_0042F9A0` -> `FUN_0042E5D0`): state 0 and `{4,"INGAMEOPTIONS2"}` if the network game
    /// menu is open (in the waiting time after `QUITGAME` it is closed); no speed command. At any other state nothing.
    fn leave_net_game_menu(&mut self) {
        if self.game_menu != NET_GAME_MENU {
            return;
        }
        if self
            .menu
            .open_names()
            .iter()
            .any(|n| n == NET_GAME_MENU_NAME)
        {
            self.close_net_game_menu();
        } else {
            self.game_menu = 0;
        }
    }

    /// One step with run-time context: the listener borrows queue, settings, texts, options of the host and the state
    /// of the logic, the context the shared effect list.
    fn step<R>(
        &mut self,
        ctx: &mut UiContext,
        screen: Screen,
        now: u64,
        f: impl FnOnce(&mut UiTree, &mut MenuSystem, &mut Pointer, &mut UiEnv) -> R,
    ) -> R {
        let MenuApp {
            config,
            opts,
            tree,
            menu,
            queue,
            pointer,
            effects,
            screens,
            key_list,
            data,
            requery,
            game_menu,
            #[cfg(test)]
            logic,
            ..
        } = self;
        let gate = menu.gate();
        let mut listener = AppListener {
            gate: &gate,
            queue,
            config,
            texts: ctx.texts,
            opts,
            key_list,
            state: screens,
            data,
            requery,
            game_menu,
            #[cfg(test)]
            logic,
        };
        let mut env = UiEnv {
            ui: UiContext {
                vfs: ctx.vfs,
                assets: &mut *ctx.assets,
                fonts: ctx.fonts,
                texts: ctx.texts,
                screen,
            },
            listener: &mut listener,
            fx: effects,
            now,
        };
        let out = f(tree, menu, pointer, &mut env);
        run_requeries(tree, &mut env);
        out
    }

    /// State of the UI (screen `screen`, images at time `now`).
    pub fn report(&self, screen: Screen, now: u64) -> UiReport {
        let tree = &self.tree;
        let mut dialogs = Vec::new();
        let mut elements = Vec::new();
        let mut focus = None;
        let mut hover = None;
        for frame in dialog_frames(tree) {
            let node = tree.node(frame);
            let NodeKind::Frame(state) = &node.kind else {
                continue;
            };
            dialogs.push(ReportDialog {
                name: lower_german(&node.dialog),
                rect: to_screen_rect(screen, tree.abs_rect(frame)),
                animating: state.moving,
                modal: node.flags & frame_flags::MODAL != 0
                    && node.parent.is_some_and(|p| {
                        matches!(tree.node(p).kind, NodeKind::ModalBlocker { .. })
                    }),
            });
            let mut stack = vec![frame];
            while let Some(id) = stack.pop() {
                let node = tree.node(id);
                let names = (lower_german(&node.dialog), lower_german(&node.name));
                if node.focused && !node.name.is_empty() {
                    focus = Some(names.clone());
                }
                if node.hover && !node.name.is_empty() {
                    hover = Some(names.clone());
                }
                if id != frame
                    && !node.name.is_empty()
                    && let Some(e) = report_element(tree, id, screen)
                {
                    elements.push(e);
                }
                stack.extend(node.children.iter().rev().filter(|&&c| tree.contains(c)));
            }
        }
        UiReport {
            dialogs,
            focus,
            hover,
            elements,
            images: tree.images.visible(now),
            busy: self.menu.busy(),
            queued: self.queue.len(),
            effects: self.history.iter().cloned().collect(),
            video: self.video.as_ref().and_then(|v| {
                v.frame_at(now).map(|frame| ReportVideo {
                    path: v.path.clone(),
                    frame,
                    frames: v.info.frames,
                    abortable: v.abortable,
                })
            }),
            pointer_hidden: self.pointer_hidden,
        }
    }

    /// Center of an element in screen pixels: of the part that the last frame drew. Error message if the dialog is not
    /// open, the element does not exist, is hidden (`HIDE`) or not drawn or lies outside the screen. Locked elements
    /// have a center (a click there does nothing, as in the original).
    pub fn element_center(
        &self,
        dialog: &str,
        element: &str,
        screen: Screen,
    ) -> Result<(i32, i32), String> {
        let tree = &self.tree;
        let open: Vec<String> = dialog_frames(tree)
            .into_iter()
            .map(|f| lower_german(&tree.node(f).dialog))
            .collect();
        let Some(frame) = tree
            .find(dialog)
            .filter(|&f| matches!(tree.node(f).kind, NodeKind::Frame(_)))
        else {
            return Err(format!(
                "dialog \"{dialog}\" is not open (open: {})",
                open.join(", ")
            ));
        };
        let key = format!("{}{}", tree.node(frame).dialog, upper_word(element));
        let Some(id) = tree.find(&key) else {
            let mut known = Vec::new();
            collect_names(tree, frame, &mut known);
            return Err(format!(
                "element \"{element}\" does not exist in dialog \"{dialog}\" (known: {})",
                known.join(", ")
            ));
        };
        let node = tree.node(id);
        if node.flags & common_flags::HIDE != 0 {
            return Err(format!("element \"{dialog}/{element}\" is hidden"));
        }
        let Some((_, v)) = node.visit.filter(|(pass, _)| *pass == tree.pass) else {
            return Err(format!(
                "element \"{dialog}/{element}\" was not drawn in the last frame"
            ));
        };
        let full = [0, 0, screen.width, screen.height];
        let Some(r) = intersect(to_screen_rect(screen, v), full) else {
            return Err(format!(
                "element \"{dialog}/{element}\" is outside the screen"
            ));
        };
        Ok(((r[0] + r[2]) / 2, (r[1] + r[3]) / 2))
    }
}

/// Menu state of the network game menu `INGAMEOPTIONS2` (`Engine+0x2CCC == 2`, re/multiplayer-menus L.7.1).
const NET_GAME_MENU: u8 = 2;

/// Frame of the network game menu.
const NET_GAME_MENU_NAME: &str = "ingameoptions2";

/// Frame of the game menu: with a network role `ingameoptions2` (`FUN_0042E150`), otherwise by game type
/// (`Engine+0x108`): -1 campaign `ingameoptions`, otherwise `ingameoptions3`.
fn game_menu_name(game: &GameInfo) -> &'static str {
    if game.net.is_some() {
        NET_GAME_MENU_NAME
    } else if game.game_type == -1 {
        "ingameoptions"
    } else {
        "ingameoptions3"
    }
}

/// End screen for the menu state: 6 victory `gameend_victory`, 7 defeat `gameend_defeat`
/// (`GameEngine_SetPlayerResult` `0x00430CE0`, re/ingame-menus 6.2).
fn game_end_name(state: u8) -> &'static str {
    if state == 6 {
        "gameend_victory"
    } else {
        "gameend_defeat"
    }
}

/// The targeted requeries that the logic demanded during a step ([`Requery`]), in order; what a requery in turn demands
/// follows in the next round (at most [`REQUERY_ROUNDS`]).
fn run_requeries(tree: &mut UiTree, env: &mut UiEnv) {
    for _ in 0..REQUERY_ROUNDS {
        let pending = env.listener.take_requery();
        if pending.is_empty() {
            return;
        }
        for r in pending {
            tree.requery(&r.dialog, &r.element, r.code, env);
        }
    }
    let dropped = env.listener.take_requery();
    if !dropped.is_empty() {
        log::warn!(
            "Requeries aborted after {REQUERY_ROUNDS} rounds: {} discarded",
            dropped.len()
        );
    }
}

/// Open frames in drawing order: children of the dialog root, then the modal ones under their blockers on the desktop
/// (the blocker of a ComboBox list carries a list, not a frame).
fn dialog_frames(tree: &UiTree) -> Vec<NodeId> {
    let is_frame =
        |id: &NodeId| tree.contains(*id) && matches!(tree.node(*id).kind, NodeKind::Frame(_));
    let mut out: Vec<NodeId> = tree
        .node(tree.dialog_root())
        .children
        .iter()
        .copied()
        .filter(is_frame)
        .collect();
    for &b in &tree.node(tree.desktop()).children {
        if tree.contains(b) && matches!(tree.node(b).kind, NodeKind::ModalBlocker { .. }) {
            out.extend(tree.node(b).children.iter().copied().filter(is_frame));
        }
    }
    out
}

/// Names of the named elements under `id` (lowercase, pre-order).
fn collect_names(tree: &UiTree, id: NodeId, out: &mut Vec<String>) {
    for &c in &tree.node(id).children {
        if tree.contains(c) {
            let node = tree.node(c);
            if !node.name.is_empty() && !matches!(node.kind, NodeKind::ComboPopup(_)) {
                out.push(lower_german(&node.name));
            }
            collect_names(tree, c, out);
        }
    }
}

/// Menus that [`MenuApp::play_video_by_name`] reopens after the video: the opened names in opening order, one `logo`
/// fewer per `start`/`start2`, because `open("start")` opens it itself (`FUN_004E3010`, 12.1.1).
fn reopen_names(open: &[String]) -> Vec<String> {
    let mut names = open.to_vec();
    for _ in open.iter().filter(|n| *n == "start" || *n == "start2") {
        if let Some(i) = names.iter().position(|n| n == "logo") {
            names.remove(i);
        }
    }
    names
}

fn report_element(tree: &UiTree, id: NodeId, screen: Screen) -> Option<ReportElement> {
    let node = tree.node(id);
    let mut e = ReportElement {
        dialog: lower_german(&node.dialog),
        name: lower_german(&node.name),
        kind: "",
        rect: to_screen_rect(screen, tree.abs_rect(id)),
        visible: node.visit.is_some_and(|(pass, _)| pass == tree.pass),
        enabled: node.enabled,
        hidden: node.flags & common_flags::HIDE != 0,
        checked: None,
        value: None,
        selection: None,
        items: Vec::new(),
        text: None,
    };
    match &node.kind {
        NodeKind::Button(s) => {
            e.kind = if s.list.is_some() {
                e.selection = Some(s.selection);
                e.items = s.items.clone();
                "combobox"
            } else {
                "button"
            };
            e.text = s.text.clone();
        }
        NodeKind::ListBox(s) => {
            e.kind = "listbox";
            e.selection = Some(s.selection);
            e.items = s.items.clone();
        }
        NodeKind::EditField(s) => {
            e.kind = "editfield";
            e.text = Some(String::from_utf16_lossy(&s.current()));
        }
        NodeKind::Slider(s) => {
            e.kind = "slider";
            e.value = Some(s.value);
        }
        NodeKind::Radio(s) => {
            e.kind = "radiobutton";
            e.checked = Some(s.checked);
        }
        NodeKind::Label(s) => {
            e.kind = "label";
            e.text = s.logic_text();
        }
        _ => return None,
    }
    Some(e)
}

mod menu_level;
pub use menu_level::LevelWorld;

#[cfg(test)]
pub(crate) mod tests;
