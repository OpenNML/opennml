//! Flow of a match from the point of view of the menus, GPU-free: game effects of the UI
//! (`StartMatch`, `LoadGame`, `SaveGame`, `DeleteSave`, `ReloadLevel`, `ResumeGame`, `Surrender`,
//! `EndMatch`, `StartCampaignLevel`), the keys in the game (key path of the original), pausing while the game
//! menu is open, the loading screen in phases and the end of the game (end screen, accounting; campaign start).
//! The match itself sits behind [`SessionView`]: in `opennml-game` [`LiveSession`] around a `GameSession`, in the
//! tests a fake with `SimHost` without a map (so `UI_GOLDEN` is measured without graphics).
//!
//! One frame of the host with a match: menu frame (`MenuHost::frame_cmds`) -> [`GameFlow::keys`] (keys the UI
//! did not take) -> `MenuHost::apply_effects` -> [`GameFlow::apply_all`] with `HostRequests::game` (loading blocks
//! and draws per phase via `present`) -> [`GameFlow::frame`] (simulation steps). See re/ingame-menus 0-5,
//! re/singleplayer-menus part 2, 6 (`Game_StartLevel`, loading screen).
//!
//! **Loading** (deviation from the original): The original ends the running match first (`FUN_004BAF40`) and then
//! loads; if that fails, it stays without a game and without a menu (re/ingame-menus, section "Open points" 4).
//! The reimplementation reads and checks the savegame ([`GameSaveFile::from_bytes`]), builds the new session
//! completely (including [`GameSaveFile::prepare`] against the loaded rules) and replaces the old one only on
//! success; otherwise an error message in the log and the previous screen opens again (`MenuApp::reopen_menu`),
//! with one of three own texts over it ([`save_failure_text`]: other version or format, damaged, does not match the
//! game data).
//! The same applies to a match start that the map or `to_match_setup` rejects.
//!
//! **Match end** (re/ingame-menus 6.2, re/accounting 6): The flow does not see the events of the simulation; after
//! every frame and after surrendering it queries the result of the local player (`Player+0x358`). The first
//! result 1/2 stops the game music and shows the end screen (`MenuApp::show_game_end`), in the campaign it books
//! the entry (`MenuApp::book_campaign`), once per match. Surrendering (`0x27`) executes the simulation immediately
//! (`SimHost::execute_now`), like the original in single player even with the game menu open. `0x0E` builds the
//! accounting ([`accounting_view`]), ends the match and opens `accounting`.

use std::path::Path;

use anyhow::Context;
use opennml_audio::engine::AudioEngine;
use opennml_control::{
    UiGameData, UiHudData, UiInputData, UiMinimapData, UiPresentationData, UiReplayData,
    UiReplayMode, hex_checksum,
};
use opennml_formats::engine_ini::EngineConfig;
use opennml_net::{NetControl, StartInfo};
use opennml_randommap::RandomMap;
use opennml_rda::{GameVersion, Vfs};
use opennml_sim::command::{Command, CommandKind};
use opennml_sim::game_end::{DEFEAT, VICTORY};
use opennml_sim::rules::Rules;
use opennml_sim::save::RecordedCommand;
use opennml_sim::setup::{MENU_LEVEL_MODE, MatchSetup, StartPackage, start_package};
use opennml_ui::app::MenuApp;
use opennml_ui::assets::Texts;
use opennml_ui::coords::Screen;
use opennml_ui::draft::{LevelInfoData, MatchRequest};
use opennml_ui::keys::{chord_codes, chord_matches};
use opennml_ui::loading::{DEFAULT_LDG, MULTIPLAYER_LDG, RANDOM_MAP_LDG, ldg_path_for};
use opennml_ui::msg::UiEffect;
use opennml_ui::own_texts::{PLAYER, SAVE_DAMAGED, SAVE_MISMATCH, SAVE_OTHER_FORMAT};
use opennml_ui::profiles::name_text;
use opennml_ui::saves::{LocalStamp, SaveDir, SaveFiles};
use opennml_ui::screens::{
    AccountingView, DIPLOMACY_MENU_STATE, DiplomacyAction, DiplomacyView, GameInfo, MissionInfo,
    NetRoleView,
};

use crate::game::chat;
use crate::game::control_groups::{self, GroupsSave, NextGuidIndex};
use crate::game::diplomacy::{self, Masks};
use crate::game::hotkeys::{HotkeyCommand, Hotkeys, KeyContext};
use crate::game::hud::HudOptions;
use crate::game::input::CameraKeys;
use crate::game::input_gate::{InputGate, sim_gate};
use crate::game::map::MapSource;
use crate::game::net_driver::{
    LEAVE_REASON, NetDriver, NetFinish, NetFrame, NetPaths, NetSave, load_failed_message,
};
use crate::game::options::{Controls, ViewOptions};
use crate::game::pause::PauseState;
use crate::game::pointer::PointerChoice;
use crate::game::presentation::HostRequest;
use crate::game::script_camera::ScriptCamera;
use crate::game::sim_host::SimHost;
use crate::game_save::{
    CameraSave, GameSaveFile, HostPart, SaveError, SaveErrorKind, sim_checksum,
};
use crate::game_session::{GameSession, LoadPhase, SessionGpu, UiRequest};
use crate::keys::{InputEvent, MouseButton};
use crate::match_request::{campaign_setup, random_map_spec, to_match_setup};
use crate::net_session::{NetPhase, NetSession};
use crate::replay_file::{ReplayHead, ReplayMeta, ReplayRecorder, replay_name};
use crate::save_files::FsSaveFiles;

/// Error message of `ui_click` on `GAMEUI` without a running match.
pub const NO_MATCH: &str = "match is not running";
/// Texts of the quicksave names: 15786 + counter (`quicksave1`, `quicksave2`, `FUN_0042F490`).
const QUICKSAVE_TEXT: i32 = 15786;
/// "[CENTER]Game time:" before the game time of the accounting (`0x0E`, re/accounting 6).
const PLAY_TIME_TEXT: i32 = 15349;
/// "Computer" for an empty name in the accounting (`FUN_00403FA0`, O-177).
const COMPUTER_TEXT: i32 = 15195;

/// A match as the flow needs it. Loading and starting build the new match completely before they replace the old
/// one: on an error the previous one stays unchanged.
pub trait SessionView {
    /// Loads the map from `source` (fixed map or random map) and starts the match (`Game_StartLevel`); `setup`
    /// builds the setup from the just-loaded rules, `progress` listens to the phases 0, 40, 70, 100.
    fn start(
        &mut self,
        vfs: &Vfs,
        source: &MapSource,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()>;
    /// Starts the menu level `level` (`FUN_004C9140` -> `Game_StartLevel` at game type -2) like [`Self::start`], but
    /// without step 2: the menu music keeps playing (`MenuManager_CloseAllAndStopMusic` is omitted, re/menu-level
    /// ML.3). Default: [`Self::start`] with the fixed map.
    fn start_menu_level(
        &mut self,
        vfs: &Vfs,
        level: &str,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        self.start(
            vfs,
            &MapSource::Level(level.into()),
            setup,
            game_speed,
            progress,
        )
    }
    /// Resumes a savegame (`FUN_004C7C20` -> `Game_LoadLevelFromSavegame`) on the map of the savegame
    /// ([`MapSource::of_save`]), prepared for the loaded rules ([`GameSaveFile::prepare`]).
    fn resume(
        &mut self,
        vfs: &Vfs,
        save: GameSaveFile,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()>;
    fn sim(&self) -> Option<&SimHost>;
    fn sim_mut(&mut self) -> Option<&mut SimHost>;
    /// One frame at the program clock `now`; without `running` (game menu, saving, loading) no steps and no catching
    /// up. In the pause ([`SessionView::pause`]) the match itself only does the pause step (O-317). Number of engine
    /// ticks, in the pause the ticks without world tick. In the network ([`SessionView::net`]) clock and ticks run
    /// independently of `running` with the turns (O-143); `running` there only locks input and camera.
    fn advance(&mut self, now: u64, running: bool) -> u32;
    /// One frame of the halted menu level (`0x20`): like [`Self::advance`] without `running` (no ticks, no catching
    /// up of the simulation), but the frame time accumulates for the camera of the scripts, which gets it all at
    /// once in the next running frame (O-447: `GameApp_Tick` `0x004CA770` does not call `GameEngine_Update` while
    /// the menu level is halted, the time base `+0x2610` stands still, `GameEngine_UpdateCameraFollow`
    /// `0x00432590` then gets the real time since the last `Update` without an upper limit). Default: just
    /// [`Self::advance`] without `running`.
    fn hold(&mut self, now: u64) {
        self.advance(now, false);
    }
    /// Input on the map (selection, right-click, camera).
    fn input(&mut self, ev: &InputEvent, screen: Screen, now: u64);
    /// Center of an element of the game UI in window pixels, from the rectangles of the last frame (`ui_click` on
    /// `GAMEUI`, [`GameFlow::gameui_click`]). Default for sessions without a command bar (the fake of the tests): no
    /// element.
    fn gameui_center(&self, element: &str) -> Result<(f64, f64), String> {
        Err(format!("element {element} does not exist in GAMEUI"))
    }
    /// Path of the `.lvl` of the match (empty without a match; random map `randommap:<Seed>`).
    fn level(&self) -> &str;
    /// Generated map of the match's random map for the savegame (`GameSession::random_map`); `None` for fixed maps
    /// and without a match.
    fn random_map(&self) -> Option<&RandomMap>;
    /// Position of the game camera for the savegame.
    fn camera(&self) -> CameraSave;
    /// End the match.
    fn end(&mut self);
    /// Adopts settings, as the original reads them from `g_pEngineConfig` when needed: camera switches
    /// (`MouseScroll`, `KeyboardScroll`, `EnableMouseScroll`, `LockCamera`, `EnableZoom` per frame in
    /// `GameEngine_UpdateCamera` `0x0043C7C0`, `LockCamera` e.g. `0x0043CDCB`) and `UnitResponse` (per
    /// acknowledgment in `Unit_PlayResponseSound` `0x0047BBC0`) take effect immediately on the running match; the
    /// qualities (`TerrainQuality`, `ShadowQuality`, `ParticlesQuality`) not until the next start, the loading or
    /// `0x1F` (`Game_StartLevel` builds the match from `EngineConfig`). The font width of the map view stays the
    /// one from the program start (O-58).
    fn apply_settings(&mut self, cfg: &EngineConfig);
    /// Menu music on again (`MenuMusic_Restart` `0x0045CA20`, like broadcast `0x161`) after a start or loading
    /// without a running match has failed: `start`/`resume` stopped it before loading.
    fn restore_menu_music(&mut self, vfs: &Vfs);
    /// Game music off at the end of the game of the local player (`MusicManager_StopMusicAndReset`, step 4 of
    /// `GameEngine_SetPlayerResult` `0x00430CE0`); the end screens then play their own music.
    fn stop_game_music(&mut self);
    /// Requests of the scripts since the last call (508 quicksave, 382 profile variable), evaluated in
    /// [`GameFlow::frame`].
    fn take_host_requests(&mut self) -> Vec<HostRequest>;
    /// Remembered speed of the cinemascope for the host part (`HostPart::cinemascope_speed`).
    fn cinemascope_speed(&self) -> Option<u8>;
    /// Camera of the scripts for the host part (`HostPart::script_camera`).
    fn script_camera(&self) -> ScriptCamera;
    /// Control groups of all 8 slots for the host part (`HostPart::control_groups`). Default without a match:
    /// empty.
    fn control_groups(&self) -> Vec<GroupsSave> {
        control_groups::empty_save()
    }
    /// `selectnextguid` indices of all 8 slots for the host part (`HostPart::next_guid_index`). Default without a
    /// match: empty.
    fn next_guid_index(&self) -> Vec<NextGuidIndex> {
        control_groups::empty_next_guid_index()
    }
    /// Idle worker index of all 8 slots for the host part (`HostPart::idle_worker_index`). Default without a
    /// match: 0.
    fn idle_worker_index(&self) -> [i32; 8] {
        [0; 8]
    }
    /// Chat and tribute masks for the host part (`HostPart::masks`). Default without a match: the initial state
    /// ([`Masks::initial`]).
    fn masks(&self) -> Masks {
        Masks::initial()
    }
    /// Sets masks (diplomacy dialog, [`diplomacy::apply`]). Default without a match: nothing.
    fn set_masks(&mut self, _masks: Masks) {}
    /// Requests of the match to the flow since the last call (`GameSession::take_ui_requests`: button 579 opens the
    /// game menu), evaluated directly after key command and input ([`GameFlow::ui_requests`]). Default for the fake
    /// of the tests: none.
    fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        Vec::new()
    }
    /// Game type of the match (`GE+0x108`, -1 campaign) and whether the game menu, diplomacy or quest menu is open
    /// (`Engine+0x2CCC != 0`): at the start, before the keys ([`GameFlow::keys`]) and per frame before
    /// [`SessionView::advance`] (`GameSession::game_type`, `GameSession::menu_open`). Default for the fake of the
    /// tests: nothing.
    fn set_match_info(&mut self, _game_type: i32, _menu_open: bool) {}
    /// Display names of the eight slots for the chat lines (`GameSession::player_names`), per frame before
    /// [`SessionView::advance`]. Default for the fake of the tests: nothing.
    fn set_player_names(&mut self, _names: [String; opennml_sim::id::PLAYER_SLOTS]) {}
    /// What the input may do in this frame: `GameSession::input_gate`. Default for the fake of the tests: the part of
    /// the simulation ([`sim_gate`]: lock, cinemascope, dialog), without script camera, game type and game menu.
    fn input_gate(&self) -> InputGate {
        self.sim().map_or_else(InputGate::default, sim_gate)
    }
    /// A key command from `bindings2.ini` or `keys.ini` ([`GameFlow::key`]) at the program clock `now`; `true` if
    /// the match takes it. Default without a match: not taken.
    fn run_hotkey(&mut self, _cmd: &HotkeyCommand, _now: u64) -> bool {
        false
    }
    /// `changegamespeed(d)` of the match ([`GameSession::change_game_speed_played`]: clamps to 0 to 19, sets the
    /// speed and reports 9/10 with sound); `Some(v)` on a change, so that [`GameFlow`] writes the setting. Default
    /// without a match: nothing.
    fn change_game_speed(&mut self, _d: i32) -> Option<u8> {
        None
    }
    /// Keyboard context for `keys.ini` (`GameSession::key_context`, re/game-ui D5.3). Default without a match:
    /// type 1, nation 0, selection 0.
    fn key_context(&self) -> KeyContext {
        KeyContext::default()
    }
    /// The two pause bits of the match (`GameSession::pause`). Default without a match: no pause.
    fn pause(&self) -> PauseState {
        PauseState::default()
    }
    /// `togglepause` of the match (`GameSession::toggle_pause`); `true` if the pause has changed. Default without a
    /// match: nothing.
    fn toggle_pause(&mut self) -> bool {
        false
    }
    /// Pointer of the match (`GameSession::pointer_choice`). Default without a match: 7/0.
    fn pointer_choice(&self) -> PointerChoice {
        PointerChoice::NORMAL
    }
    /// Turn driver of the network match (`GameSession::net`); `None` in single player. Default: no network.
    fn net(&self) -> Option<&NetDriver> {
        None
    }
    fn net_mut(&mut self) -> Option<&mut NetDriver> {
        None
    }
    /// Control entries, dropouts and failures of the network match since the last call, evaluated in
    /// [`GameFlow::frame`] after [`SessionView::advance`]; events, speed changes and takeover are handled by the
    /// session itself ([`NetFrame::into_session`]). Default: empty.
    fn take_net_frame(&mut self) -> NetFrame {
        NetFrame::default()
    }
    /// Puts the turn driver into the just-started match (network start, [`GameFlow::start_net_match`]); after that it
    /// computes only with the turns ([`SessionView::net`]). Without a match the driver is lost (log);
    /// `start_net_match` calls it only after a successful start, i.e. after `SimHost::enter_rounds`: the session then
    /// positions the camera on the start of the own slot (`MapView::center_on_start`; not in a loaded match whose
    /// host part carries the camera) and starts the level music, which [`SessionView::start`] holds back in the
    /// network, with the nation music of the own slot (E-297).
    fn attach_net(&mut self, net: NetDriver);
    /// The just-started match becomes the replay of the recording `commands` ([`GameFlow::apply`] with
    /// [`UiEffect::PlayReplay`]): `GameSession::begin_playback` (replay mode of the simulation, fog off, selection
    /// locked, command bar hidden).
    fn begin_playback(&mut self, commands: Vec<RecordedCommand>);
}

/// Failed network start ([`GameFlow::start_net_match`]): reason (it already went to the driver as
/// `Loaded { error }`) and the driver back to the host, which keeps pumping until the host aborts the start.
pub struct NetStartFailure {
    pub error: String,
    pub net: NetDriver,
}

impl std::fmt::Debug for NetStartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetStartFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// The own text for a savegame that does not load. `kind` is the kind of the [`SaveError`] in the chain of the error,
/// if there is one. Without it the stage decides: `decoded == false` (name, reading the file) is damaged,
/// `decoded == true` (map, rules, start in [`SessionView::resume`]) does not match the game data, for example a map
/// that is missing here.
fn save_failure_text(kind: Option<SaveErrorKind>, decoded: bool) -> i32 {
    match kind {
        Some(SaveErrorKind::Format | SaveErrorKind::Newer) => SAVE_OTHER_FORMAT,
        Some(SaveErrorKind::Corrupted) => SAVE_DAMAGED,
        Some(SaveErrorKind::Mismatch) => SAVE_MISMATCH,
        None if decoded => SAVE_MISMATCH,
        None => SAVE_DAMAGED,
    }
}

/// Setup of the network match from the start package (E-262, E-263): first the full rules checksum of the just-loaded
/// map against that of the host (it covers map and game data; different game data would not compute the same), then
/// the host's setup unchanged - with its seed and the network flag. A start from a network savegame does not come here:
/// it resumes the savegame, whose check compares only the logic part ([`GameSaveFile::prepare`]).
fn net_setup(rules: &Rules, package: &StartPackage) -> anyhow::Result<MatchSetup> {
    if rules.checksum != package.rules.full() {
        anyhow::bail!(
            "Different game data or map than on the host (rules checksum {:016x} instead of {:016x})",
            rules.checksum,
            package.rules.full()
        );
    }
    if !package.setup.network {
        anyhow::bail!("start package without network flag");
    }
    Ok(package.setup.clone())
}

/// What the flow reads from the host: archives, texts (computer names in `to_match_setup`) and the savegames.
pub struct FlowEnv<'a> {
    pub vfs: &'a Vfs,
    pub texts: &'a Texts,
    pub files: &'a FsSaveFiles,
}

/// Information about the running match that the simulation does not know (host part of the savegame).
#[derive(Debug, Clone, PartialEq)]
struct MatchMeta {
    level_info: LevelInfoData,
    game_type: i32,
    /// Fixed teams (`Engine+0x20` = session `+0x180` from `LockTeams`, `GameEngine_ApplyGameSettings` `0x00431160`;
    /// savegame key `m_IsLockTeams`): locks the status selection of diplomacy.
    lock_teams: bool,
    /// `SAVEREPLAY` switch of the match: new match from the setting of the request, network match from the lobby,
    /// loaded match from the host part; campaign, menu level and command-line path `false`. Goes into every savegame
    /// ([`HostPart::save_replay`]); the recording builds on it.
    save_replay: bool,
    /// The match is a replay (`GameApp+0x19F3`): name of the recording in `savegame/autoplay` (for
    /// `ui_state.game.replay`). F1, F4, script autosave 508, saving and restart have no effect; the UI learns of it
    /// via [`GameInfo::playback`].
    playback: Option<String>,
}

/// Queued input after a load: loading blocks the event loop, the window events of that time arrive in the next
/// round ([`GameFlow::begin_round`]) and are discarded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum StaleInput {
    #[default]
    None,
    /// Loaded; the next round brings the queued events.
    Pending,
    /// This round discards all input.
    Draining,
}

/// The flow of the match. Not `Clone`: the network session holds the transport of the program.
#[derive(Debug, Default)]
pub struct GameFlow {
    meta: Option<MatchMeta>,
    /// ESC flag `Engine+0x4F1` (`FUN_0042F1D0(1)`).
    escape: bool,
    /// Quicksave counter `Engine+0x368`: initial value 0 in the constructor of the engine (`0x0043C3D2`,
    /// re/ingame-menus 5); the reimplementation starts at 0 with every new match like a new engine.
    quicksave_counter: u32,
    /// Texts 15786 and 15787.
    quicksave_names: [String; 2],
    /// `.ldg` and phase of the running load.
    loading: Option<(String, LoadPhase)>,
    /// Percent per drawn phase of the last load (for the control channel: loading blocks the host).
    load_phases: Vec<u8>,
    stale: StaleInput,
    /// The end of the game of the local player has been reported (end screen open): exactly once per match.
    ended: bool,
    /// `Game_StartLevel` step 2 ran since the last [`GameFlow::take_menus_closed`].
    menus_closed: bool,
    /// Key data (`bindings2.ini`, `keys.ini`, `keymap.ini`); empty without [`Self::set_hotkeys`].
    hotkeys: Hotkeys,
    /// Screenshot request `GameApp+0xD698` (`KD_HARDCOPY`, [`Self::take_hardcopy`]).
    hardcopy: bool,
    /// Query of the input after the last frame ([`SessionView::input_gate`]): the change of the cinemascope switches
    /// the pointer ([`Self::refresh_lock`]). [`Self::key`] queries the session itself (state before the key).
    gate: InputGate,
    /// Last pointer position from [`Self::input`] (window pixels), for the rectangle rule of the chat window.
    cursor: Option<(f64, f64)>,
    /// A press left or right was in the chat window and did not go to the match; neither did the corresponding
    /// release.
    swallowed: [bool; 2],
    /// Network session of the network menus; none without [`Self::set_net_session`].
    net: Option<NetSession>,
    /// Menu level behind the main menu ([`menu_level`]); excludes `meta`.
    menu_level: Option<menu_level::MenuLevelRun>,
    /// Recording of the running match according to `SAVEREPLAY`; only with `meta`.
    recorder: Option<ReplayRecorder>,
    /// Fixed local time for the name of the recording (tests, check runs); otherwise the clock of the host
    /// ([`SaveFiles::now_local`]).
    fixed_clock: Option<LocalStamp>,
}

/// `GameSpeed` of the settings as the speed of the simulation (0 ... 255, O-153; the original takes the `int`
/// unchecked, the slider only goes up to 20). Negative gives 0 (K-325). The network start limits to 20 (`start_infos`).
pub fn config_speed(cfg: &EngineConfig) -> u8 {
    cfg.game_speed.clamp(0, i32::from(u8::MAX)) as u8
}

/// View of a match from the settings (`TerrainQuality`, `ShadowQuality`, `ParticlesQuality`,
/// `UnitResponse`) and the window width at the start; other values as the default.
pub fn view_options(cfg: &EngineConfig, width: u32) -> ViewOptions {
    let quality = |v: i32, max: u8| v.clamp(0, i32::from(max)) as u8;
    ViewOptions {
        terrain_quality: quality(cfg.terrain_quality, 2),
        shadow_quality: quality(cfg.shadow_quality, 3),
        particles_quality: quality(cfg.particles_quality, 2),
        text_width: width,
        language_tag: cfg.language_tag.clone(),
        unit_response: cfg.unit_response != 0,
        hud: HudOptions::from_config(cfg),
        back_color: cfg.back_color as u32,
        ..ViewOptions::default()
    }
}

/// Camera control from the settings (`MouseScroll`, `KeyboardScroll`, `EnableMouseScroll`, `LockCamera`,
/// `EnableZoom`, `DoubleClickTimeout`).
pub fn controls(cfg: &EngineConfig) -> Controls {
    let count = |v: i32| u32::try_from(v).unwrap_or(0);
    Controls {
        mouse_scroll: count(cfg.mouse_scroll),
        keyboard_scroll: count(cfg.keyboard_scroll),
        enable_mouse_scroll: cfg.enable_mouse_scroll != 0,
        lock_camera: cfg.lock_camera != 0,
        enable_zoom: cfg.enable_zoom != 0,
        double_click_ms: count(cfg.double_click_timeout),
    }
}

/// GUID of the active profile for the ending of the savegame (without profile 0, like `opennml_ui::screens::save_load`).
fn active_guid(app: &MenuApp) -> i32 {
    app.profiles().current_guid().unwrap_or(0)
}

/// A savegame name is usable as a file name in the folder (no empty name, no path). The original only checks while
/// typing (`FILENAME` code `0xB`); names from the list or the control channel arrive unfiltered.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("no name".to_string());
    }
    if name.contains(['/', '\\', ':']) || name == "." || name == ".." {
        return Err(format!("\"{name}\" is not a file name"));
    }
    Ok(())
}

/// Writes a file via `.tmp` (a failed write leaves an old state intact).
fn write_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating folder {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

/// Loading image when resuming a savegame (`0x0A`, O-139): the `.ldg` of the level via `ldg_path_for(..., true, ...)`,
/// otherwise `default.ldg`; a random map always `default.ldg` (deviating from O-431) - its map path
/// `randommap:<Seed>` is not a file.
fn saved_ldg(save: &GameSaveFile, vfs: &Vfs) -> String {
    if save.sim.start.random_map.is_some() {
        DEFAULT_LDG.to_string()
    } else {
        ldg_path_for(&save.sim.start.level, save.host.game_type, true, |p| {
            vfs.exists(p)
        })
    }
}

/// Own network savegame for the start of a loaded network match: name from `StartInfo::saved` checked
/// ([`opennml_net::valid_save_name`]: it comes from the network), read the file `savegame/multiplayer/<Name>` of this
/// machine ([`NetPaths::net_save`]), network part present, checksum of the simulation part ([`sim_checksum`]) and
/// tick as in the start, start package as distributed, own slot `local_slot == your_slot`; these comparisons use the
/// values as read, the same on every machine. The check against the rules of the map (logic part) and the
/// preparation follow when resuming ([`GameSaveFile::prepare`] in [`SessionView::resume`]). Errors as a reason.
fn own_net_save(paths: &NetPaths, info: &StartInfo) -> Result<GameSaveFile, String> {
    let saved = info
        .saved
        .as_ref()
        .ok_or_else(|| "start without network savegame".to_string())?;
    let name = &saved.name;
    if !opennml_net::valid_save_name(name) {
        return Err(format!("\"{name}\" is not a valid savegame name"));
    }
    let path = paths.net_save(name);
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("network savegame {} not readable: {e}", path.display()))?;
    let save = GameSaveFile::from_bytes(&bytes)
        .map_err(|e| format!("network savegame \"{name}\": {e:#}"))?;
    let net =
        save.host.net.as_ref().ok_or_else(|| {
            format!("\"{name}\" is not a network savegame (network part missing)")
        })?;
    let checksum = sim_checksum(&save.sim);
    if checksum != saved.checksum {
        return Err(format!(
            "The network savegame \"{name}\" differs from the host's (checksum {checksum:016x} instead of {:016x})",
            saved.checksum
        ));
    }
    if save.sim.state.steps != saved.steps {
        return Err(format!(
            "The network savegame \"{name}\" is at tick {}, the host's at tick {}",
            save.sim.state.steps, saved.steps
        ));
    }
    if save.sim.start != info.package {
        return Err(format!(
            "The network savegame \"{name}\" has a different start package than the host's start"
        ));
    }
    if net.local_slot != info.your_slot {
        return Err(format!(
            "In the network savegame \"{name}\" slot {} is the own one, the host assigns slot {}",
            u32::from(net.local_slot) + 1,
            u32::from(info.your_slot) + 1
        ));
    }
    Ok(save)
}

impl GameFlow {
    /// Flow without a match; `texts` supplies the quicksave names.
    pub fn new(texts: &Texts) -> Self {
        Self {
            quicksave_names: [
                texts.display(QUICKSAVE_TEXT),
                texts.display(QUICKSAVE_TEXT + 1),
            ],
            ..Self::default()
        }
    }

    /// Network session for the network menus: search, lobby and start from the menu; pumped in
    /// [`Self::net_step`].
    pub fn set_net_session(&mut self, net: NetSession) {
        self.net = Some(net);
    }

    pub fn net_session(&self) -> Option<&NetSession> {
        self.net.as_ref()
    }

    pub fn net_session_mut(&mut self) -> Option<&mut NetSession> {
        self.net.as_mut()
    }

    /// Fixed local time for the name of the recordings (tests and check runs, like [`NetSession::set_fixed_clock`]);
    /// `None` takes the clock of the host.
    pub fn set_fixed_clock(&mut self, clock: Option<LocalStamp>) {
        self.fixed_clock = clock;
    }

    /// The running recording (also an ended one, as long as the match runs); `None` without `SAVEREPLAY`.
    pub fn recorder(&self) -> Option<&ReplayRecorder> {
        self.recorder.as_ref()
    }

    /// Starts the recording of the new match after a successful start or load (`FUN_004C68C0` from
    /// `Game_FinishLevelSetup` `0x004C777D`): only with `SAVEREPLAY` of the match and game type >= 0 (never campaign
    /// or menu level, E-334; the command-line path does not have the switch). Unlike the original also after loading
    /// (deviating from E-329): the file then begins with the whole log of the state from tick 0; if it is missing
    /// (state without log, e.g. after eight hours), there is no recording, only a warning. Name from the local time
    /// ([`replay_name`]), header with start package, map info, game type, team lock and the current speed (in the
    /// cinemascope the remembered speed instead of speed 10). If creating fails, the match runs without a recording
    /// (warning).
    fn begin_recording(&mut self, view: &dyn SessionView, env: &FlowEnv) {
        debug_assert!(self.recorder.is_none(), "old recording at the start");
        let (Some(meta), Some(sim)) = (self.meta.as_ref(), view.sim()) else {
            return;
        };
        if !meta.save_replay || meta.game_type < 0 {
            return;
        }
        if !sim.log_complete() {
            log::warn!(
                "No recording: the match is without a command log from the start of the match (savegame without log)"
            );
            return;
        }
        let name = replay_name(self.fixed_clock.unwrap_or_else(|| env.files.now_local()));
        let head = ReplayHead {
            start: start_package(&sim.rules, &sim.setup),
            meta: ReplayMeta {
                level_info: meta.level_info.clone(),
                game_type: meta.game_type,
                lock_teams: meta.lock_teams,
                game_speed: view.cinemascope_speed().unwrap_or(sim.game_speed),
            },
        };
        match ReplayRecorder::begin(&env.files.autoplay_dir(), &name, &head, sim) {
            Ok(recorder) => {
                log::info!(
                    "Recording {} started ({} commands from tick 0)",
                    recorder.path().display(),
                    sim.log.len()
                );
                self.recorder = Some(recorder);
            }
            Err(e) => log::warn!("Recording not started, the match runs without: {e:#}"),
        }
    }

    /// Catches the recording up: in every frame ([`Self::frame`]) and before every game effect ([`Self::apply`]; also
    /// two effects in one frame, e.g. surrender and restart), as long as `sim` is the recorded match. Nothing without
    /// a recording or simulation.
    fn sync_recording(&mut self, sim: Option<&SimHost>) {
        if let (Some(recorder), Some(sim)) = (self.recorder.as_mut(), sim) {
            recorder.sync(sim);
        }
    }

    /// End of the recording with the end of the match in the host (E-337): last catch-up from `sim` (the still
    /// standing, recorded match), then close. Called before the teardown of the match (main menu via `0x0E`) and at
    /// program end (`Game::shutdown`). A successful new start or a load discards the old recording without this
    /// catch-up (`sim` would then already be the new match; [`Self::apply`] caught up beforehand).
    pub fn end_recording(&mut self, sim: Option<&SimHost>) {
        self.sync_recording(sim);
        if let Some(recorder) = self.recorder.take() {
            log::info!("Recording {} ended", recorder.path().display());
        }
    }

    /// A new match is running (start or load succeeded): discard the old recording without catching up, then begin
    /// the new one ([`Self::begin_recording`]).
    fn restart_recording(&mut self, view: &dyn SessionView, env: &FlowEnv) {
        if let Some(old) = self.recorder.take() {
            log::info!("Recording {} ended (new match)", old.path().display());
        }
        self.begin_recording(view, env);
    }

    /// Key data of the installation for the key path ([`Hotkeys::load`]).
    pub fn set_hotkeys(&mut self, hotkeys: Hotkeys) {
        self.hotkeys = hotkeys;
    }

    /// Fetches and clears the screenshot request: like the reader `0x004C084A` after `EndScene` and `Present` of the
    /// frame (`if (D698) { FUN_004BD940(); D698 = 0; }`). The host writes the image (`opennml-game` as PNG).
    pub fn take_hardcopy(&mut self) -> bool {
        std::mem::take(&mut self.hardcopy)
    }

    /// Is a match running?
    pub fn running(&self) -> bool {
        self.meta.is_some()
    }

    /// `.ldg` path and phase of the running load (for the loading screen and the control channel).
    pub fn loading(&self) -> Option<(String, LoadPhase)> {
        self.loading.clone()
    }

    /// Report of the running match for `ui_state` of `opennml-game` (field `game`): map (`level`), game mode, game menu
    /// state of the UI, steps and checksum of the simulation, load phase and history of the last load, result of the
    /// local player (`Player+0x358`; slot as in [`Self::check_result`]) and the game UI `hud` (from the host via
    /// `GameSession::hud_state`), the input `input` (from the host via `GameSession::input_data` with its pointer)
    /// and the presentation of the campaign UI `presentation` (`GameSession::presentation_state`), plus the minimap
    /// `minimap` (`GameSession::minimap_state`), the active chat lines `chat_lines` (`GameSession::chat_lines`) and
    /// `diplomacy_open` (game menu state 10, [`DIPLOMACY_MENU_STATE`]). The state of the world display `world` is set
    /// afterwards by the host from `MapView::world_display`; here `None`. Recording or replay `replay`
    /// (`replay_data`). Without a match or without a simulation `None`.
    #[allow(clippy::too_many_arguments)]
    pub fn game_data(
        &self,
        app: &MenuApp,
        sim: Option<&SimHost>,
        level: &str,
        hud: Option<UiHudData>,
        input: Option<UiInputData>,
        presentation: Option<UiPresentationData>,
        minimap: Option<UiMinimapData>,
        chat_lines: Vec<String>,
    ) -> Option<UiGameData> {
        let meta = self.meta.as_ref()?;
        let sim = sim?;
        let local = app.game().map_or(sim.active_player, |g| g.local_slot);
        Some(UiGameData {
            level: level.to_string(),
            game_type: meta.game_type,
            menu_state: app.game_menu_state(),
            steps: sim.state.steps,
            checksum: hex_checksum(sim.checksum()),
            loading: self.loading.as_ref().map(|(_, phase)| phase.percent()),
            load_phases: self.load_phases.clone(),
            result: sim
                .state
                .players
                .get(usize::from(local))
                .map_or(0, |p| p.result),
            hud,
            input,
            presentation,
            minimap,
            diplomacy_open: app.game_menu_state() == DIPLOMACY_MENU_STATE,
            chat_lines,
            world: None,
            flags: crate::game::sim_host::flags_info(&sim.state),
            replay: self.replay_data(sim),
        })
    }

    /// Recording or replay for `ui_state.game.replay`: in the replay the name of the recording, the tick of the next
    /// command of the file ([`SimHost::playback`]) and, after the end of the file, the current tick (the file has no
    /// next command any more; the tick shows how far the match runs beyond it); when recording, the file name, the
    /// tick at the last catch-up ([`ReplayRecorder::synced_step`]: the file reaches up to there) and whether the
    /// recording has ended. Otherwise `None`.
    fn replay_data(&self, sim: &SimHost) -> Option<UiReplayData> {
        if let Some(name) = self.meta.as_ref().and_then(|m| m.playback.as_ref()) {
            let status = sim.playback()?;
            return Some(UiReplayData {
                mode: UiReplayMode::Play,
                file: name.clone(),
                step: status.next_step.unwrap_or(sim.state.steps),
                ended: status.ended,
            });
        }
        let recorder = self.recorder.as_ref()?;
        Some(UiReplayData {
            mode: UiReplayMode::Record,
            file: recorder
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            step: recorder.synced_step(),
            ended: recorder.ended(),
        })
    }

    /// Does the flow accept input? Not during loading, not in the round after it (queued events) and not while the
    /// network session is loading (start there until `Go`; free in search and lobby).
    pub fn accepts_input(&self) -> bool {
        self.loading.is_none()
            && self.stale != StaleInput::Draining
            && !self.net.as_ref().is_some_and(NetSession::phase_is_loading)
    }

    /// Just loaded (since the last round): the host releases held keys and does not catch up the load time.
    pub fn just_loaded(&self) -> bool {
        self.stale == StaleInput::Pending
    }

    /// Did `Game_StartLevel` step 2 (`close_menus`) run since the last call? Then the menu commands that the host
    /// produced before the game effects of this logic step are stale: in the original the menu only draws in
    /// rendering after the logic step, i.e. already with an empty image table (background image, see quirk D-255).
    /// Resets the flag.
    pub fn take_menus_closed(&mut self) -> bool {
        std::mem::take(&mut self.menus_closed)
    }

    /// `Game_StartLevel` step 2 (`0x004C85E3`...`0x004C85F8`, re/singleplayer-menus 6.1), before every load of a
    /// level (`0x0D`, `0x0C`, `0x1F`), even before a failing one: `MenuManager_CloseAllAndStopMusic` empties the
    /// image table of the desktop ([`MenuApp::drop_desktop_images`]; otherwise the background image of the menu would
    /// fade out above the world). [`SessionView::start`] stops the menu music. For the menu level (game type -2) the
    /// step is omitted ([`menu_level`]).
    fn close_menus(&mut self, app: &mut MenuApp) {
        app.drop_desktop_images();
        self.menus_closed = true;
    }

    /// A new round of the event loop begins (`new_events`): after a load the next round discards all input, after
    /// that it applies again.
    pub fn begin_round(&mut self) {
        self.stale = match self.stale {
            StaleInput::Pending => StaleInput::Draining,
            StaleInput::Draining | StaleInput::None => StaleInput::None,
        };
    }

    /// Host part of the savegame: camera of the session, map description and game mode of the match, name of the
    /// local player (`NetName`), speed of the simulation, camera of the scripts, control groups, `selectnextguid`
    /// indices, idle worker index, chat and tribute masks, fixed teams, the fog switch (`m_FogShowMode`) and in the
    /// network the network part ([`NetDriver::save_info`]). Without a match `None`.
    pub fn host_part(&self, app: &MenuApp, view: &dyn SessionView) -> Option<HostPart> {
        let meta = self.meta.as_ref()?;
        let sim = view.sim()?;
        Some(HostPart {
            camera: view.camera(),
            level_info: meta.level_info.clone(),
            game_type: meta.game_type,
            local_name: name_text(&app.config().net_name),
            game_speed: sim.game_speed,
            cinemascope_speed: view.cinemascope_speed(),
            script_camera: view.script_camera(),
            control_groups: view.control_groups(),
            next_guid_index: view.next_guid_index(),
            idle_worker_index: view.idle_worker_index(),
            masks: view.masks(),
            lock_teams: meta.lock_teams,
            fog_show_mode: sim.fog.show_mode,
            net: view.net().and_then(NetDriver::save_info),
            log: sim.save_log(),
            save_replay: meta.save_replay,
        })
    }

    /// The keys of the last menu frame that neither a broadcast nor a widget took (`MenuApp::take_untaken_keys`), to
    /// [`Self::key`] at the program clock `now`. Call after every `MenuHost::frame_cmds` (the list applies only to the
    /// last frame). Beforehand the match learns game type and menu state ([`SessionView::set_match_info`]).
    pub fn keys(&mut self, app: &mut MenuApp, view: &mut dyn SessionView, now: u64) {
        let names = app.take_untaken_keys();
        if names.is_empty() {
            return;
        }
        if let Some(meta) = &self.meta {
            view.set_match_info(meta.game_type, app.game_menu_state() != 0);
        } else if self.menu_level.is_some() {
            view.set_match_info(MENU_LEVEL_MODE, false);
        }
        for name in names {
            self.key(&name, app, view, now);
        }
    }

    /// A key in the game according to the dispatcher `FUN_004C6050` (re/game-ui D5.3), from the point where neither a
    /// broadcast nor the focus widget took it (`MenuApp` does that); `name` is the chord name of the event
    /// (`FUN_0049C340`, held keys including mouse buttons, O-116), `now` the program clock for the commands
    /// (double-tap). `true` if the key belongs to the game. Every auto-repeat arrives as its own event (O-300). Per
    /// event in this sequence:
    ///
    /// 1. Name exactly `ESCAPE` (O-14): at state 0 the flag `Engine+0x4F1`; the next frame with an engine tick opens
    ///    the game menu ([`Self::frame`], O-152), also under the lock and with an open construction or target cursor
    ///    (the cursor stays, O-308). At state != 0 no flag (`GameEngine_Update` clears it in the halt branch). The
    ///    original sets the flag already when only the broadcast `{0x17E}` does not take the key (re/menu-system
    ///    3.1); the reimplementation only receives keys that no widget took either. At state 0 a chat window with
    ///    focus can be open: its input field takes the broadcast ESC (closes only the window) and every other key, so
    ///    the flag does not arise, as in the original. As in the original the key continues afterwards (`ESCAPE` has
    ///    neither a binding nor a `keys.ini` line; the code does not query `KD_STOP_DIALOG`, O-305).
    /// 2. Binding from `bindings2.ini` (`FUN_00411CE0` -> `0x0040FC50`, exactly this set of keys,
    ///    [`Hotkeys::binding`]) -> [`SessionView::run_hotkey`]; taken, even without effect. A held mouse button misses
    ///    every binding (O-301).
    /// 3. Lock (`0x004C61FD`, [`InputGate::keys_ini`]): end here (O-304). ESC and the bindings before it keep working.
    /// 4. Not taken: `keys.ini` (`FUN_0049D110`, [`Hotkeys::keys_ini`]) in the context of the match
    ///    ([`SessionView::key_context`]) with the key layout of the settings (`m_KeyMap`); the name must match
    ///    exactly (`LBUTTON F1`, `ADD` hit nothing, O-301, O-307). A hit is taken, even without effect. Quicksave
    ///    (`quicksave()`, `FUN_0042F490`: only at state 0; counter > 1 -> 0, name = text 15786 + counter, `{0x26}`,
    ///    counter + 1 - alternating `quicksave1`/`quicksave2`, O-31, O-151) and quickload (`quickload()`,
    ///    `FUN_0042F5C0`, `MenuApp::quick_load` checks itself) are executed by the flow, everything else by the match
    ///    ([`SessionView::run_hotkey`]). Also with the game menu open (as in the original).
    /// 5. `KD_HARDCOPY` (`SNAPSHOT`) contained in the event (`FUN_0049BF40`, subset: also with a held mouse button;
    ///    [`InputGate::hardcopy`]): screenshot request `GameApp+0xD698`, **assigned** per event - another key in the
    ///    same frame clears it (O-326). Under the lock the old value stays.
    ///
    /// ESC thus stays open also in the cinemascope and under the lock (`0x0043E1A0`, E-246, O-56: no skipping of
    /// scenes); F1/F4 have no effect under the lock (stage 3). There is no quest menu key (O-364: F10 does nothing).
    ///
    /// Without a match, but with a menu level, only the screenshot ([`Self::menu_level_key`]).
    pub fn key(
        &mut self,
        name: &str,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        now: u64,
    ) -> bool {
        if !self.accepts_input() {
            return false;
        }
        if self.meta.is_none() {
            return self.menu_level_key(name, view);
        }
        let escape = name == "ESCAPE";
        if escape && app.game_menu_state() == 0 {
            self.escape = true;
        }
        let event = chord_codes(name);
        let gate = view.input_gate();
        // Stages 2-4 in one place with the viewer (`Hotkeys::resolve_named`): binding, lock, `keys.ini`.
        let cmd = self.hotkeys.resolve_named(
            name,
            &event,
            &gate,
            &view.key_context(),
            &app.config().key_map,
        );
        let taken = cmd.is_some();
        match cmd {
            // In the replay no quicksave and quickload (deviating from E-339).
            Some(HotkeyCommand::QuickSave | HotkeyCommand::QuickLoad) if self.playing_back() => {
                log::info!("Replay: {name} has no effect (watching only)");
            }
            Some(HotkeyCommand::QuickSave) => self.quick_save(app),
            Some(HotkeyCommand::QuickLoad) => {
                app.quick_load();
            }
            Some(cmd) => Self::run_command(&cmd, app, view, now),
            None => {}
        }
        // Under the lock no screenshot (`InputGate::hardcopy`, like the early end of the original, O-304).
        if gate.hardcopy() {
            self.hardcopy = chord_matches(self.hotkeys.keydef("KD_HARDCOPY"), &event);
        }
        taken || escape
    }

    /// A key command of the match. The flow handles `changegamespeed` (`FUN_0042F050`) itself: the session changes the
    /// speed and reports 9/10 ([`SessionView::change_game_speed`]), the flow writes the return value `v` into the
    /// setting `GameSpeed` ([`MenuApp::set_game_speed`], `EngineConfig+0x34`; the menu host writes the file).
    /// Everything else is run by the session ([`SessionView::run_hotkey`]).
    fn run_command(cmd: &HotkeyCommand, app: &mut MenuApp, view: &mut dyn SessionView, now: u64) {
        match *cmd {
            HotkeyCommand::ChangeGameSpeed(d) => {
                if let Some(v) = view.change_game_speed(d) {
                    app.set_game_speed(i32::from(v));
                }
            }
            // `showchatwindow(k)` (`FUN_0042EA20`): a binding, so also under the lock; only without cinemascope and at
            // menu state 0 ([`InputGate::chat`]); menu state, menu level and buffer are checked by
            // [`MenuApp::show_chat_window`].
            HotkeyCommand::ShowChatWindow(k) => {
                if view.input_gate().chat() {
                    app.show_chat_window(k);
                }
            }
            ref cmd => {
                view.run_hotkey(cmd, now);
                Self::ui_requests(app, view);
            }
        }
    }

    /// Requests of the match to the flow, fetched directly after the key command or the input, i.e. before the next
    /// tick: [`UiRequest::GameMenu`] (button 579, case `0x0044C040` -> `FUN_0042E150`) opens the game menu only at
    /// menu state 0 (`Engine+0x2CCC == 0`); otherwise nothing. [`UiRequest::ToggleDiplomacy`] (key 572, button 580)
    /// toggles the diplomacy dialog ([`MenuApp::toggle_diplomacy`], `FUN_0042E060(1)`), with the lock of this moment
    /// ([`InputGate::diplomacy`]): under it, it neither opens nor closes.
    fn ui_requests(app: &mut MenuApp, view: &mut dyn SessionView) {
        for request in view.take_ui_requests() {
            match request {
                UiRequest::GameMenu => {
                    if app.game_menu_state() == 0 {
                        app.open_game_menu();
                    }
                }
                UiRequest::ToggleDiplomacy => {
                    app.set_ui_locked(!view.input_gate().diplomacy());
                    app.toggle_diplomacy(true);
                }
            }
        }
    }

    /// `quicksave()` (`FUN_0042F490`): only at state 0 and with a running match; counter > 1 -> 0, name = text 15786 +
    /// counter, `{0x26}`, counter + 1 (see [`Self::key`]).
    fn quick_save(&mut self, app: &mut MenuApp) {
        if app.game_menu_state() == 0 && app.game().is_some() {
            if self.quicksave_counter > 1 {
                self.quicksave_counter = 0;
            }
            let name = self.quicksave_names[self.quicksave_counter as usize].clone();
            app.quick_save(&name);
            self.quicksave_counter += 1;
        }
    }

    /// Input of the window to the map: only at state 0 and without a running save (in single player the game menu
    /// stops the game completely, O-143). Pointer movements and released keys always pass (they only set the pointer
    /// and the held keys), clicks, wheel and pressed keys do not - so an open menu issues no command. Afterwards the
    /// requests of the match ([`Self::ui_requests`], button 579).
    ///
    /// Chat window (hypothesis, to be checked in the reference run): The window sets no menu state, so the match
    /// would get every click. The original asks the menu tree first (`GameApp_DispatchEvent` ->
    /// `MenuWidget_DispatchInput`); the reimplementation does not pass a mouse-button press whose point lies in the
    /// frame of the open chat window ([`MenuApp::chat_window_rect`]) to the match, nor the corresponding release.
    ///
    /// No input of the window reaches the menu level (O-442): according to the reference run the world takes, besides
    /// the START frame, neither a left click nor a selection box nor a right click (`0x0047A490` draws selection
    /// rings even in the cinemascope, the photos show none); camera and minimap are off (O-441).
    pub fn input(
        &mut self,
        ev: &InputEvent,
        screen: Screen,
        now: u64,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
    ) {
        if self.meta.is_none() || !self.accepts_input() {
            return;
        }
        match *ev {
            InputEvent::CursorMoved { x, y } => self.cursor = Some((x, y)),
            InputEvent::CursorLeft => self.cursor = None,
            // The middle button is no widget event (raw events `0x2A`...`0x2C`, `FUN_004B8E70`): the chat window does
            // not take it (O-331).
            InputEvent::Button { button, pressed } if button != MouseButton::Middle => {
                let side = usize::from(button == MouseButton::Right);
                if pressed {
                    let inside = match (self.cursor, app.chat_window_rect(screen)) {
                        (Some((x, y)), Some(r)) => {
                            let (x, y) = (x.floor() as i32, y.floor() as i32);
                            x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
                        }
                        _ => false,
                    };
                    self.swallowed[side] = inside;
                    if inside {
                        return;
                    }
                } else if std::mem::take(&mut self.swallowed[side]) {
                    return;
                }
            }
            _ => {}
        }
        let passive = matches!(
            ev,
            InputEvent::CursorMoved { .. }
                | InputEvent::CursorLeft
                | InputEvent::Key { pressed: false, .. }
        );
        if passive || (app.game_menu_state() == 0 && !app.saving()) {
            view.input(ev, screen, now);
            Self::ui_requests(app, view);
        }
    }

    /// `ui_click` on `GAMEUI`: click on the center of an element of the command bar. The center comes from the last
    /// frame ([`SessionView::gameui_center`]); movement, press and release go through [`GameFlow::input`] like the
    /// mouse by hand - so an open game menu blocks the click, not the movement. Two clicks before the same frame
    /// (same `now`) are a double-click. Without a match the error [`NO_MATCH`]. Response: the point in window pixels.
    pub fn gameui_click(
        &mut self,
        element: &str,
        button: MouseButton,
        screen: Screen,
        now: u64,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
    ) -> Result<(i32, i32), String> {
        if self.meta.is_none() {
            return Err(NO_MATCH.to_string());
        }
        let (x, y) = view.gameui_center(element)?;
        let events = [
            InputEvent::CursorMoved { x, y },
            InputEvent::Button {
                button,
                pressed: true,
            },
            InputEvent::Button {
                button,
                pressed: false,
            },
        ];
        for ev in &events {
            self.input(ev, screen, now, app, view);
        }
        // `as` saturates; the points lie in the window.
        Ok((x.floor() as i32, y.floor() as i32))
    }

    /// One frame of the match: steps only at state 0, without a running save (`Engine+0x115`) and without loading;
    /// otherwise only the time base (`GameEngine_Update` `0x00445650`, halt branch). In the pause (`Engine+0x24E4`)
    /// the match itself does the pause step (camera, water) and counts engine ticks without world tick (O-317). The
    /// ESC flag opens the game menu in the frame that had at least one engine tick (`GameEngine_TickDialogScript`
    /// `0x0043E1A0` -> `FUN_0042E150`, O-152; also in the pause); at state != 0 it expires. Beforehand the match takes
    /// over the settings ([`SessionView::apply_settings`]: camera switches and `UnitResponse` take effect in the next
    /// frame, also from an open options menu). After the steps the requests of the scripts ([`Self::host_requests`],
    /// names via `texts`). Number of ticks. Without a match one frame of the menu level
    /// ([`Self::menu_level_frame`]).
    pub fn frame(
        &mut self,
        now: u64,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        texts: &Texts,
    ) -> u32 {
        if self.meta.is_none() {
            return self.menu_level_frame(now, app, view);
        }
        view.apply_settings(app.config());
        if app.game_menu_state() != 0 {
            self.escape = false;
        }
        let running = app.game_menu_state() == 0 && !app.saving() && self.loading.is_none();
        if let Some(meta) = &self.meta {
            view.set_match_info(meta.game_type, app.game_menu_state() != 0);
        }
        if let Some(sim) = view.sim() {
            let names = std::array::from_fn(|slot| player_display_name(sim, texts, slot as i32));
            view.set_player_names(names);
        }
        let steps = view.advance(now, running);
        if self.net_frame(app, view, texts) {
            // Handed over and left: the match is closed, the accounting is open.
            return steps;
        }
        // The frame step of the bar may issue button intentions; without a tick in between, as after an input.
        Self::ui_requests(app, view);
        self.host_requests(app, view, texts);
        self.refresh_lock(app, view);
        self.refresh_diplomacy(app, view, texts);
        self.check_result(app, view);
        if self.escape && steps >= 1 {
            self.escape = false;
            app.open_game_menu();
        }
        self.sync_recording(view.sim());
        steps
    }

    /// The network match after the frame ([`SessionView::take_net_frame`]): for `Save { name }`, `Stop` and `End`
    /// (they write their state) and for the abort of the host, the flow writes the network savegame of this machine
    /// ([`Self::write_net_save`]) with the simulation part that the driver kept
    /// ([`NetFrame::saves`](crate::game::net_driver::NetFrame::saves)). If the network match is over
    /// ([`NetFrame::finish`](crate::game::net_driver::NetFrame::finish): stop, end or abort of the host, failure), it
    /// stops the game music and shows the message
    /// ([`NetFinish::message`](crate::game::net_driver::NetFinish::message)) as a `MSGBOX` over the standing match
    /// ([`MenuApp::end_with_message`]); "OK" is the game end `0x0E`, which in the network after `Go` leads to the main
    /// menu via the accounting ([`Self::leave_net_match`]). A dropout needs no message of its own: the defeat arrives
    /// as a command in the next bundle, and the simulation reports 200 + p (text 17200 + p "... was eliminated!")
    /// itself on execution (`game_end::set_player_result`, free play; E-272). Speed, halt and resync have already
    /// been handled by the session. With the message the match counts as ended: if the own result falls in the ticks
    /// of the same frame, no end screen follows ([`Self::check_result`]).
    ///
    /// If the host has handed the match over ([`NetFinish::HandedOver`]), it goes straight to the accounting without
    /// a message ([`Self::close_net_match`]); return value `true`, the match is then closed. If this machine has become
    /// host after a migration ([`NetFrame::became_host`]), the UI gets the network role host
    /// ([`MenuApp::set_net_role`]: speed slider `GAMESPEED`; the speed keys check the role of the driver themselves).
    fn net_frame(&mut self, app: &mut MenuApp, view: &mut dyn SessionView, texts: &Texts) -> bool {
        let frame = view.take_net_frame();
        if frame.became_host {
            log::info!("Network: this machine has become host after the host migration");
            app.set_net_role(Some(NetRoleView::Host));
        }
        for control in &frame.controls {
            match control {
                NetControl::Stop { name } => {
                    log::error!(
                        "Network: match stopped after the third desync, saved as \"{name}\""
                    );
                }
                NetControl::End { name } => {
                    log::info!("Network: the host has ended the match, saved as \"{name}\"");
                }
                NetControl::Save { .. }
                | NetControl::Speed(_)
                | NetControl::Halt { .. }
                | NetControl::Resync { .. }
                | NetControl::Handover { .. } => {}
            }
        }
        let mut written = Vec::new();
        for save in frame.saves {
            let name = save.name.clone();
            if self.write_net_save(save, app, view) {
                written.push(name);
            }
        }
        for (slot, reason) in &frame.lost {
            log::warn!(
                "Network: slot {slot} dropped out ({reason}); the defeat follows in the next bundle"
            );
        }
        if let Some(text) = &frame.failed {
            log::error!("Network: {text}");
        }
        if frame.finish == Some(NetFinish::HandedOver) {
            log::info!("Network: match handed over, accounting without a message");
            // At program end the match can be handed over without an end screen (which already stopped the music).
            view.stop_game_music();
            if let Some(net) = view.net_mut() {
                net.leave(LEAVE_REASON);
            }
            self.close_net_match(true, app, view, texts);
            return true;
        }
        if let Some(finish) = frame.finish {
            let saved = finish
                .save_name()
                .is_some_and(|name| written.iter().any(|w| w == name));
            let text = finish.message(saved, texts);
            // The log stays English, whatever the edition.
            let log_text = finish.message(saved, &Texts::own_only(GameVersion::English));
            log::warn!("Network: match over: {log_text}");
            // The message carries the end.
            self.ended = true;
            self.escape = false;
            view.stop_game_music();
            app.end_with_message(text);
        }
        false
    }

    /// Writes the network savegame of this machine (E-274): `savegame/multiplayer/<Name>` without ending
    /// ([`NetPaths::net_save`](crate::game::net_driver::NetPaths::net_save)), overwritten without asking. The
    /// simulation part comes from the execution tick ([`NetSave`]) and is thus the same on all machines; the host part
    /// is the own one with network part ([`Self::host_part`], speed from the held tick). The name is checked once more
    /// beforehand; a violation or a write error appears only in the log. `true` if written.
    fn write_net_save(&self, save: NetSave, app: &MenuApp, view: &dyn SessionView) -> bool {
        let NetSave {
            name,
            sim,
            game_speed,
            log,
        } = save;
        if !opennml_net::valid_save_name(&name) {
            log::error!("Network: savegame {name:?} not written, not a valid name");
            return false;
        }
        let write = || -> anyhow::Result<std::path::PathBuf> {
            let net = view.net().context("no network match")?;
            let mut host = self.host_part(app, view).context("no running match")?;
            host.net.as_ref().context("network match without start")?;
            host.game_speed = game_speed;
            host.log = log;
            let file = GameSaveFile::new(sim, host, view.random_map().cloned().map(Box::new));
            let path = net.paths().net_save(&name);
            write_file(&path, &file.to_bytes())?;
            Ok(path)
        };
        match write() {
            Ok(path) => {
                log::info!("Network: savegame saved: {}", path.display());
                true
            }
            Err(e) => {
                log::warn!("Network: savegame \"{name}\" not saved: {e:#}");
                false
            }
        }
    }

    /// The pointer of this frame for `MenuHost::set_pointer`: without a match or with an open menu or dialog 7/0
    /// (`MenuFrame` `0x00460599`, "menus and open dialogs"); otherwise the pointer of the match
    /// ([`SessionView::pointer_choice`]). Over the menu level the arrow stays, also over units and buildings (no
    /// target pointer; reference run, O-442).
    pub fn pointer_choice(&self, app: &MenuApp, view: &dyn SessionView) -> PointerChoice {
        if self.meta.is_none() || !app.menu().open_names().is_empty() {
            return PointerChoice::NORMAL;
        }
        view.pointer_choice()
    }

    /// Requests of the scripts of a frame: 508 quicksaves via the save path of the UI (`MenuApp::quick_save` ->
    /// `{0x26}` -> [`UiEffect::SaveGame`] in the next round before its steps: the savegame has the state after the
    /// step with 508) under the text `AUTOSAVENAME` of the current campaign entry (`FUN_0042FA00` -> `FUN_004BA950`);
    /// the counter of F1 stays untouched. 382 writes the profile variable into the active profile
    /// (`MenuApp::set_profile_var`, `FUN_004BD900`).
    fn host_requests(&mut self, app: &mut MenuApp, view: &mut dyn SessionView, texts: &Texts) {
        for request in view.take_host_requests() {
            match request {
                // In the replay the level script does not save either.
                HostRequest::QuickSave if self.playing_back() => {
                    log::info!("Replay: quicksave (508) has no effect");
                }
                HostRequest::QuickSave => {
                    let entry =
                        opennml_ui::campaign::current_entry(app.profiles(), &app.data().campaigns);
                    match entry {
                        Some(entry) => {
                            let name = texts.display(entry.text_id);
                            app.quick_save(&name);
                        }
                        None => {
                            log::warn!("Quicksave (508): no current campaign entry, nothing saved")
                        }
                    }
                }
                HostRequest::ProfileVar { name, value } => app.set_profile_var(&name, value),
            }
        }
    }

    /// Remembers the query of the input after the frame (edges of the cinemascope), passes mission objectives and tip
    /// log to the UI ([`MenuApp::set_mission`]) and the lock to the UI ([`MenuApp::set_ui_locked`]: "Save" locked,
    /// pointer when closing the game menu, re/campaign-ui D6.9, O-379). Plus the pointer at the edges of the
    /// cinemascope (`FUN_00430920`, re/campaign-ui D6.17): start -> off only in the campaign (game type -1, O-350; the
    /// rule of [`InputGate::pointer_visible`], O-299), end -> on. Checked per frame, set only at the edges like the
    /// original: a game menu opened in the cinemascope (`FUN_0042E150` turns the pointer on) keeps it, also after
    /// closing without a lock. The other paths of the pointer are in `MenuApp` (ESC on, game end on, closing under
    /// the lock off); videos and loading switch it themselves. The only place where the flow switches the pointer.
    fn refresh_lock(&mut self, app: &mut MenuApp, view: &dyn SessionView) {
        let gate = view.input_gate();
        app.set_ui_locked(gate.ui_locked);
        app.set_mission(view.sim().map(mission_info));
        if gate.cinemascope != self.gate.cinemascope {
            let campaign = self.meta.as_ref().is_some_and(|m| m.game_type == -1);
            if !gate.cinemascope {
                app.set_pointer_hidden(false);
            } else if campaign {
                app.set_pointer_hidden(true);
            }
        }
        self.gate = gate;
    }

    /// State of the diplomacy dialog to the UI ([`MenuApp::set_diplomacy`], pattern `set_mission`): per frame from
    /// simulation, masks of the match, local slot and fixed team choice of the match ([`diplomacy::view`], names
    /// [`player_display_name`]); without a match `None`.
    fn refresh_diplomacy(&self, app: &mut MenuApp, view: &dyn SessionView, texts: &Texts) {
        let state = match (&self.meta, view.sim(), app.game()) {
            (Some(meta), Some(sim), Some(game)) => Some(diplomacy_view(
                sim,
                &view.masks(),
                game.local_slot,
                meta.lock_teams,
                texts,
            )),
            _ => None,
        };
        app.set_diplomacy(state);
    }

    /// A change from the diplomacy dialog ([`UiEffect::Diplomacy`]): [`diplomacy::apply`] for the local slot,
    /// commands via `SimHost::submit` (in single player the match stands while the dialog is open, so they take
    /// effect immediately; in the network the dialog stops nothing and the commands go through the turn with the own
    /// contribution, on all machines at the same execution tick, re/multiplayer-menus L.7.2, E-320), the masks back
    /// to the match ([`SessionView::set_masks`]). Nothing without a match.
    fn diplomacy(&mut self, action: DiplomacyAction, app: &MenuApp, view: &mut dyn SessionView) {
        let Some(local) = app
            .game()
            .map(|g| g.local_slot)
            .filter(|_| self.meta.is_some())
        else {
            log::warn!("Diplomacy without a running match: {action:?} dropped");
            return;
        };
        let mut masks = view.masks();
        let Some(sim) = view.sim_mut() else {
            return;
        };
        diplomacy::apply(action, local, sim, &mut masks);
        view.set_masks(masks);
    }

    /// Sending from the chat window ([`UiEffect::SendChat`], `FUN_004B9940`): `{0x5D, slot, mask, len, chars...}` for
    /// the local slot with the receivers of the window ([`chat::command`]) via `SimHost::submit` (the match keeps
    /// running, the command takes effect at the next tick boundary; in the pause immediately). Nothing without a
    /// match.
    fn send_chat(&mut self, window: u8, text: &str, app: &MenuApp, view: &mut dyn SessionView) {
        let Some(local) = app
            .game()
            .map(|g| g.local_slot)
            .filter(|_| self.meta.is_some())
        else {
            log::warn!("Chat without a running match: \"{text}\" dropped");
            return;
        };
        let masks = view.masks();
        let Some(sim) = view.sim_mut() else {
            return;
        };
        if let Some(command) = chat::command(window, local, text, sim, &masks) {
            sim.submit(command);
        }
    }

    /// Queries the result of the local player (instead of `SimEvent::PlayerResult`): the first result "lost" (1) or
    /// "won" (2) stops the game music and shows the end screen (`GameEngine_SetPlayerResult` steps 3-4,
    /// `MenuApp::show_game_end`); after that nothing until the next match. In the campaign (`Engine+0x108 == -1`,
    /// `0x00430EE1`/`0x00430FA2`) it then books the current entry and writes `profiles.ini` (`FUN_004C6B10(won)`,
    /// `MenuApp::book_campaign`).
    fn check_result(&mut self, app: &mut MenuApp, view: &mut dyn SessionView) {
        if self.ended || self.meta.is_none() {
            return;
        }
        let (Some(game), Some(sim)) = (app.game(), view.sim()) else {
            return;
        };
        let result = sim
            .state
            .players
            .get(usize::from(game.local_slot))
            .map_or(0, |p| p.result);
        if result != DEFEAT && result != VICTORY {
            return;
        }
        self.ended = true;
        self.escape = false;
        view.stop_game_music();
        app.show_game_end(result == VICTORY);
        if self.meta.as_ref().is_some_and(|m| m.game_type == -1) {
            app.book_campaign(result == VICTORY);
        }
    }

    /// The game effects of a frame in order of creation ([`Self::apply`]); what does not belong here goes to the
    /// log.
    pub fn apply_all(
        &mut self,
        effects: Vec<UiEffect>,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        for effect in effects {
            if !self.apply(&effect, app, view, env, present) {
                log::info!("Effect without match flow: {effect:?}");
            }
        }
    }

    /// One game effect; `false` if it does not belong here. `present` draws the loading screen of the `.ldg` at the
    /// phase (the wrapper with `LoadingScreen::draw`, then `present()`).
    pub fn apply(
        &mut self,
        effect: &UiEffect,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) -> bool {
        // What the frame or the previous effect put into the command log (e.g. the immediately executed surrender) is
        // in the file before an effect replaces or ends the match.
        self.sync_recording(view.sim());
        if self.meta.is_some() && view.net().is_some() && self.apply_net(effect, app, view) {
            return true;
        }
        match effect {
            UiEffect::StartMatch(req) => self.start_match(req, app, view, env, present),
            // In the replay no saving, loading or restart (the game menu locks it; the path via the control channel
            // stays without effect).
            UiEffect::LoadGame { name } if self.playing_back() => {
                log::info!("Replay: loading \"{name}\" has no effect");
            }
            UiEffect::LoadGame { name } => self.load_game(name, app, view, env, present),
            UiEffect::SaveGame { name } if self.playing_back() => {
                log::info!("Replay: saving as \"{name}\" has no effect");
            }
            UiEffect::ReloadLevel if self.playing_back() => {
                log::info!("Replay: restart has no effect");
            }
            UiEffect::SaveGame { name } => self.save_game(name, app, view, env),
            UiEffect::DeleteSave { name, dir } => delete_save(name, *dir, app, env),
            // `{0x19}` from `LOADAUTOPLAY` ([`playback`]).
            UiEffect::PlayReplay { name } => self.start_replay(name, app, view, env, present),
            UiEffect::ReloadLevel => self.reload_level(app, view, env, present),
            // `FUN_0042E290`: `Engine+0x2D30 = GameSpeed` (directly, O-153), but only without cinemascope
            // (`Engine+0x104 == 0`, re/ingame-menus 0.2): in the cinemascope speed 10 stays. `{0x37}` and `{0x26}`
            // without speed.
            UiEffect::ResumeGame { game_speed } => {
                if let (Some(speed), Some(sim)) = (game_speed, view.sim_mut())
                    && !sim.state.script.mission.cinemascope
                {
                    sim.game_speed = *speed;
                }
            }
            UiEffect::Surrender { slot } => self.surrender(*slot, app, view),
            UiEffect::EndMatch => self.end_match(app, view, env),
            UiEffect::StartCampaignLevel { level, difficulty } => {
                self.start_campaign_level(level, *difficulty, app, view, env, present);
            }
            UiEffect::Diplomacy(action) => self.diplomacy(*action, app, view),
            // Menu level (`0x20`/`0x21`).
            UiEffect::MenuLevel(effect) => self.apply_menu_level(effect, app, view, env, present),
            UiEffect::SendChat { window, text } => self.send_chat(*window, text, app, view),
            // Network session.
            UiEffect::Net(cmd) => match self.net.as_mut() {
                Some(net) => {
                    if !net.on_app_cmd(cmd, app, env.vfs, env.texts) {
                        log::info!("Network command {:#x} has no effect: {cmd:?}", cmd.code());
                    }
                }
                None => log::info!(
                    "Network command {:#x} without network session: {cmd:?}",
                    cmd.code()
                ),
            },
            UiEffect::Search(on) => match self.net.as_mut() {
                Some(net) => net.search(*on),
                None => log::info!("LAN search {on}: without network session"),
            },
            UiEffect::Lobby(action) => match self.net.as_mut() {
                Some(net) => net.on_lobby(action, app, env.vfs, env.texts),
                None => log::info!("Lobby request without network session: {action:?}"),
            },
            _ => return false,
        }
        true
    }

    /// Game effects that run differently in the network; `false` for all others (they take the single-player path,
    /// e.g. diplomacy and chat, whose commands the turn mode of the host steers into the contribution):
    /// - "Load" (`0x0A`, also quickload) and "restart" (`0x1F`): no effect, only a log line.
    /// - "Save" (`0x0B`, also quicksave): a `Save` wish with the name from the dialog ([`NetDriver::wish_save`]);
    ///   written after the execution tick on all machines ([`Self::write_net_save`]).
    /// - Closing the game menu with speed (`FUN_0042E290`): the speed stays that of the turns (E-265).
    /// - Surrender (`0x27`): first cancel an own pause, then the own `SetPlayerResult { slot: own, result: 1 }` via
    ///   `SimHost::submit` into the next contribution (no immediate execution); the defeat arrives at the execution
    ///   tick on all machines, the end screen is then shown by [`Self::check_result`]. For the host it is the orderly
    ///   end of the match for everyone ([`NetDriver::end`]): no own result command.
    fn apply_net(&mut self, effect: &UiEffect, app: &MenuApp, view: &mut dyn SessionView) -> bool {
        match effect {
            UiEffect::LoadGame { name } => {
                log::info!("Network: loading \"{name}\" in the game menu has no effect");
            }
            UiEffect::ReloadLevel => log::info!("Network: restart in the game menu has no effect"),
            // `LOADAUTOPLAY` is unreachable in the network (no replay in the network).
            UiEffect::PlayReplay { name } => {
                log::info!("Network: replay of \"{name}\" during the network match has no effect");
            }
            UiEffect::SaveGame { name } => {
                if let Some(net) = view.net_mut()
                    && net.wish_save(name)
                {
                    log::info!("Network: saving as \"{name}\" requested");
                }
            }
            UiEffect::ResumeGame { game_speed } => {
                log::info!(
                    "Network: game menu closed, speed {game_speed:?} of the settings has no effect"
                );
            }
            // Host like client (re/multiplayer M.10.4): the own defeat through the turn; the host stays host until it
            // leaves the end screen ([`Self::leave_net_match`]).
            UiEffect::Surrender { .. } => {
                let Some(local) = app
                    .game()
                    .map(|g| g.local_slot)
                    .or_else(|| view.sim().map(|s| s.active_player))
                else {
                    return true;
                };
                if let Some(net) = view.net_mut()
                    && net.resume_own_pause()
                {
                    log::info!("Network: own pause lifted before surrendering");
                }
                if let Some(sim) = view.sim_mut() {
                    sim.submit(Command {
                        player: local,
                        kind: CommandKind::SetPlayerResult {
                            slot: local,
                            result: DEFEAT,
                        },
                    });
                    log::info!(
                        "Network: slot {local} surrenders, the defeat follows with the bundle"
                    );
                }
            }
            _ => return false,
        }
        true
    }

    /// Loading with a loading screen: `run` gets the progress, every phase is drawn. Afterwards the queued input is
    /// due to be discarded.
    fn load(
        &mut self,
        ldg: &str,
        present: &mut dyn FnMut(&str, LoadPhase),
        run: impl FnOnce(&mut dyn FnMut(LoadPhase)) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.loading = Some((ldg.to_string(), LoadPhase::Start));
        self.load_phases.clear();
        let (loading, phases) = (&mut self.loading, &mut self.load_phases);
        let result = run(&mut |phase| {
            *loading = Some((ldg.to_string(), phase));
            phases.push(phase.percent());
            present(ldg, phase);
        });
        self.loading = None;
        self.stale = StaleInput::Pending;
        result
    }

    /// Load failed: previous screen open again; without a running match also the menu music that `start`/`resume`
    /// stopped before loading (otherwise the menu would stay silent). With a match none was running.
    fn failed_load(&self, app: &mut MenuApp, view: &mut dyn SessionView, env: &FlowEnv) {
        if self.meta.is_none() {
            view.restore_menu_music(env.vfs);
        }
        app.reopen_menu();
    }

    /// A new match is running: the UI learns game mode, slot of the local player and the network role from the turn
    /// driver (`set_game`, state 0), the match its game type ([`SessionView::set_match_info`]); ESC flag, screenshot
    /// request, quicksave counter as well as pointer position and swallowed presses of the rectangle rule begin anew.
    fn started(&mut self, meta: MatchMeta, app: &mut MenuApp, view: &mut dyn SessionView) {
        debug_assert!(
            self.menu_level.is_none(),
            "menu level still running at the start of a match"
        );
        let local_slot = view.sim().map_or(0, |sim| sim.active_player);
        // Network role from the attached driver: menu path, command line and tests all go through this place; ESC then
        // opens `INGAMEOPTIONS2`, only the host sees the speed slider.
        let net = view.net().map(|n| {
            if n.is_host() {
                NetRoleView::Host
            } else {
                NetRoleView::Client
            }
        });
        app.set_game(Some(GameInfo {
            game_type: meta.game_type,
            local_slot,
            ui_locked: false,
            net,
            playback: meta.playback.is_some(),
        }));
        view.set_match_info(meta.game_type, app.game_menu_state() != 0);
        self.meta = Some(meta);
        self.escape = false;
        self.hardcopy = false;
        self.quicksave_counter = 0;
        self.ended = false;
        // A press in the chat window of the old match swallows no release of the new one; the pointer position comes
        // with the next `CursorMoved`.
        self.cursor = None;
        self.swallowed = [false; 2];
        // New match without cinemascope: a savegame in the middle of the campaign cinemascope turns the pointer off
        // like the restored bars (`GameSession::restore_scene`).
        self.gate = InputGate::default();
        self.refresh_lock(app, view);
    }

    /// `0x27` (re/ingame-menus 4): with a pause (`+0x24E4`) first `togglepause` (`FUN_0042D780`, lifts it), then the
    /// original executes the message `{0x50000056, 2, slot, 1}` of the local player immediately in single player
    /// (`NetSession_SubmitMessage` without a session -> `GameEngine_SetPlayerResult(slot, 1)`), also with the game
    /// menu open, in which no tick runs (`SimHost::execute_now`). Then immediately the query of the result (end screen
    /// in the same frame).
    fn surrender(&mut self, slot: u8, app: &mut MenuApp, view: &mut dyn SessionView) {
        if self.meta.is_none() {
            log::warn!("Surrender without a running match: nothing to do");
            return;
        }
        if view.pause().sim {
            view.toggle_pause();
        }
        let player = app.game().map_or(slot, |g| g.local_slot);
        if let Some(sim) = view.sim_mut() {
            sim.execute_now(Command {
                player,
                kind: CommandKind::SetPlayerResult {
                    slot,
                    result: DEFEAT,
                },
            });
        } else {
            log::warn!("Surrender without a simulation in the session: the command is dropped");
        }
        self.check_result(app, view);
    }

    /// `0x0E` (`0x004C9F77`...`0x004CA03B`, re/accounting 6): (1 there is no network; 2 nobody in the original reads
    /// the result `App+0xD730` - the only access is this writer `0x004C9FA2` -, the reimplementation does not have it)
    /// 3 table `FUN_00403FA0` and 4 game time text ([`accounting_view`]), 5 end the game (`FUN_004BAF40`: match
    /// closed, `set_game(None)`), 6 `{6,"accounting"}` (`MenuApp::set_accounting`). Without a match the original
    /// reads a missing engine; the reimplementation does nothing.
    fn end_match(&mut self, app: &mut MenuApp, view: &mut dyn SessionView, env: &FlowEnv) {
        if self.meta.is_some() && view.net().is_some() {
            return self.leave_net_match(app, view, env);
        }
        let Some(sim) = view.sim().filter(|_| self.meta.is_some()) else {
            log::warn!("Match end (command 0x0E) without a running match: no accounting");
            return;
        };
        let accounting = accounting_view(sim, env.texts);
        self.end_recording(view.sim());
        view.end();
        self.meta = None;
        self.escape = false;
        self.ended = false;
        app.set_game(None);
        app.set_mission(None);
        app.set_diplomacy(None);
        app.set_accounting(accounting);
        log::info!("Match ended, accounting open");
    }

    /// `0x0E` in the network match: "OK" of the message at the end of the network match ([`Self::net_frame`]: end by
    /// the host, abort of the host, stop after the third desync, failure) or `END_VICTORY`/`END_DEFEAT` of the end
    /// screen. Step 1 of `0x0E` (`0x004C9F77`..., re/accounting 6, re/multiplayer-menus L.8) ends the network in the
    /// original, here [`NetDriver::leave`] (`Bye` to the peer(s), a dropout for the host); then like
    /// [`Self::end_match`]: table and game time from the own statistics at the moment of the click
    /// ([`accounting_view`], before `view.end()`; the simulation stays untouched), match closed and
    /// `{6,"accounting"}` ([`MenuApp::set_accounting`]), whose `MAINMENU` leads to the main menu - also after a
    /// command-line start (deviating from E-299). If the match had not reached `Go` ([`NetDriver::started`]: abort
    /// after loading), there is nothing to account: to the main menu `START` without accounting
    /// ([`MenuApp::open_main_menu`]). The network session of the menus is over in both cases.
    ///
    /// The host whose turns are still running (e.g. it dropped out and closes its end screen) leaves the match in an
    /// orderly way ([`NetDriver::host_leaves`]): it hands it over to a reachable active client
    /// ([`NetDriver::hand_over`]; afterwards to the accounting without a message, [`NetFinish::HandedOver`] in
    /// [`Self::net_frame`]), otherwise it ends for everyone ([`NetDriver::end`]): accounting and `Bye` then come only
    /// with the "OK" of the end message ([`NetFinish::HostEnded`]), which leads back here. Until then a further `0x0E`
    /// stays without effect. After the end `meta` is empty, a further `0x0E` falls into [`Self::end_match`] without a
    /// match (one accounting, one `Bye`).
    fn leave_net_match(&mut self, app: &mut MenuApp, view: &mut dyn SessionView, env: &FlowEnv) {
        let went = view.net().is_some_and(NetDriver::started);
        if let Some(net) = view.net_mut() {
            if net.ending() {
                log::info!("Network: game end during the end of the match has no effect");
                return;
            }
            if net.host_leaves() {
                log::info!("Network: the host leaves the match (handover or end for everyone)");
                return;
            }
            net.leave(LEAVE_REASON);
        }
        self.close_net_match(went, app, view, env.texts);
    }

    /// Steps after leaving the network match ([`Self::leave_net_match`], handover in [`Self::net_frame`]): accounting
    /// from the own statistics if the match had reached `Go` (`went`), otherwise the main menu; match closed, network
    /// session of the menus reset.
    fn close_net_match(
        &mut self,
        went: bool,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        texts: &Texts,
    ) {
        let accounting = view
            .sim()
            .filter(|_| went)
            .map(|sim| accounting_view(sim, texts));
        self.end_recording(view.sim());
        view.end();
        self.meta = None;
        self.escape = false;
        self.ended = false;
        app.set_game(None);
        app.set_mission(None);
        app.set_diplomacy(None);
        let opened = match accounting {
            Some(view) => {
                app.set_accounting(view);
                "accounting"
            }
            None => {
                app.open_main_menu();
                if went {
                    "main menu (without simulation)"
                } else {
                    "main menu (before Go)"
                }
            }
        };
        // The network session of the menus is over with the match.
        if let Some(net) = self.net.as_mut() {
            net.reset();
        }
        log::info!("Network match left, {opened}");
    }

    /// `0x0D` -> `Game_StartLevel` (re/singleplayer-menus part 2, 6.1): background images of the desktop removed
    /// ([`Self::close_menus`]), loading screen by `ldg_path_for` (restart), load level, setup via `to_match_setup` from
    /// the rules of the map, speed `GameSpeed`. Empty map path (O-188), unreadable map or rejected setup: message in
    /// the log, `reopen_menu` (without a match `singleplayermenu`; the draft has already lost its open slots, as after
    /// `EXIT2`). Random map (`Info.LevelType 1`): map from [`random_map_spec`] ([`MapSource::Random`]), loading image
    /// `randommap.ldg`; an error of the generator or the rules ends the same way (D-327).
    ///
    /// Host of a lobby: instead of single player the network start ([`Self::start_host_net`]).
    fn start_match(
        &mut self,
        req: &MatchRequest,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        if let Some(net) = &self.net {
            if net.is_host_lobby() {
                // The host starts from the session, not from the order of the draft.
                return self.start_host_net(app, view, env, present);
            }
            // A second `{0x0D}` (double-click on `START`) or a `{0x0D}` in a foreign lobby starts no single player via
            // the network session.
            if !matches!(net.phase(), NetPhase::Idle | NetPhase::Searching) {
                log::warn!(
                    "Match start (0x0D) during the network session ({:?}) discarded",
                    net.phase()
                );
                return;
            }
        }
        // Every start via `Game_StartLevel` ends the menu level before loading (`FUN_004BAF40`), even a failing one;
        // the way back via `START` restarts it.
        self.end_menu_level(app, view);
        self.close_menus(app);
        // The random map branches off before the check of the map path (`Game_StartLevel` `0x004C877C`: at
        // `LevelType 1` `Game_StartRandomMapFromSeedText` instead of the `.lvl`); loading image of the restart
        // `randommap.ldg` (`FUN_00547560`, O-431), set at the start instead of derived from the map name.
        let (source, ldg) = match random_map_spec(req) {
            Some(spec) => (MapSource::Random(spec), RANDOM_MAP_LDG.to_string()),
            None if req.level.is_empty() => {
                log::warn!("Match not started: no map chosen (empty map path)");
                app.reopen_menu();
                return;
            }
            None => (
                MapSource::Level(req.level.clone()),
                ldg_path_for(&req.level, req.game_type, false, |p| env.vfs.exists(p)),
            ),
        };
        let level = source.label();
        let speed = config_speed(app.config());
        view.apply_settings(app.config());
        let texts = env.texts;
        let result = self.load(&ldg, present, |progress| {
            view.start(
                env.vfs,
                &source,
                &mut |rules| to_match_setup(req, rules, texts).map_err(anyhow::Error::msg),
                speed,
                progress,
            )
        });
        match result {
            Ok(()) => {
                let meta = MatchMeta {
                    level_info: req.info.clone(),
                    game_type: req.game_type,
                    lock_teams: req.settings.lock_teams,
                    save_replay: req.settings.save_replay,
                    playback: None,
                };
                self.started(meta, app, view);
                self.restart_recording(view, env);
                log::info!("Match started: {level}");
            }
            // D-327: a generator error of the random map also aborts the start with a message.
            Err(e) => {
                log::warn!("Match \"{level}\" not started: {e:#}");
                self.failed_load(app, view, env);
            }
        }
    }

    /// `START` of the host after `{0x0D}` (re/multiplayer-menus L.3.4): [`NetSession::start_host_match`] (order from
    /// the session with the own map catalog; rules, `to_net_setup`, `start_package`, `HostCore::start_from_lobby`),
    /// then [`Self::start_net_match`] with the driver. If the start from the lobby fails (e.g. a player just left), the
    /// lobby opens again (`START` closed it with `{3,"lan2"}` ...) and nothing loads; if loading fails, the session is
    /// closed.
    fn start_host_net(
        &mut self,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        let Some(net) = self.net.as_mut() else {
            return;
        };
        match net.start_host_match(&app.data().maps, app.config(), env.vfs, env.texts) {
            Ok(driver) => {
                if let Err(failure) = self.start_net_match(driver, app, view, env, present)
                    && let Some(net) = self.net.as_mut()
                {
                    net.start_failed(failure.net);
                }
            }
            Err(e) => {
                log::warn!("Network: start from the lobby rejected: {e}");
                net.start_rejected();
                // The waiting room of the loaded lobby is `LAN4` (`{0x14}` closed it).
                if net.is_loaded_lobby() {
                    app.push_command(opennml_ui::screens::lan4::open_lan4());
                } else {
                    for cmd in opennml_ui::screens::open_lobby() {
                        app.push_command(cmd);
                    }
                }
            }
        }
    }

    /// Network session per frame: pump ([`NetSession::pump`]: search, connect, lobby, state to the UI), follow the
    /// driver of the match (`Go`, end), and for the client load the map after the `Start` of the host
    /// ([`Self::start_net_match`]; if it fails, the session is closed). Nothing without a network session. The host
    /// calls it in every frame after [`Self::apply_all`] (`opennml-game`, the tests in `Run::frame`).
    pub fn net_step(
        &mut self,
        now: u64,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        let running = self.meta.is_some();
        let Some(net) = self.net.as_mut() else {
            return;
        };
        net.pump(now, app, env.vfs, env.texts);
        net.follow_match(view.net(), running);
        let Some(driver) = net.take_start() else {
            return;
        };
        if let Err(failure) = self.start_net_match(driver, app, view, env, present)
            && let Some(net) = self.net.as_mut()
        {
            net.start_failed(failure.net);
        }
    }

    /// Network start (E-321): after the `Start` of the host, loads the map of the start package the same way as
    /// [`Self::start_match`] (loading screen: fixed map always `multiplayer.ldg`, E-321; [`SessionView::start`]), with
    /// the setup of the host ([`net_setup`]: rules checksum equal, otherwise error) and its speed. After that the own
    /// slot `your_slot` becomes the local player in turn mode ([`SimHost::enter_rounds`], before [`Self::started`],
    /// which reads it), the driver goes into the match ([`SessionView::attach_net`]) and reports `Loaded` - for the
    /// host `loaded_self`. Tick 0 computes only after `Go`.
    ///
    /// Before that the menus close (`{0x34}`, the main menu of the command-line start) and the background images
    /// ([`Self::close_menus`]). If loading, setup or the slot (empty, no human) fails: `Loaded { error }` with the
    /// reason (for the host this aborts the start for everyone), a half-started match is ended, menu music and main
    /// menu are on again, and the driver comes back in [`NetStartFailure`].
    ///
    /// If the start carries the mark "loaded" (`StartInfo::saved`), each machine resumes its own file instead
    /// ([`Self::start_saved_net_match`]).
    pub fn start_net_match(
        &mut self,
        mut net: NetDriver,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) -> Result<(), Box<NetStartFailure>> {
        let info = match (net.start_info().cloned(), self.meta.is_some()) {
            (Some(info), false) => info,
            (info, running) => {
                let error = if running {
                    "network start during a running match".to_string()
                } else {
                    "network start without start information of the host".to_string()
                };
                log::error!("Network: {error} (start present: {})", info.is_some());
                net.loaded(Some(error.clone()));
                return Err(Box::new(NetStartFailure { error, net }));
            }
        };
        if info.saved.is_some() {
            return self.start_saved_net_match(net, info, app, view, env, present);
        }
        self.end_menu_level(app, view);
        self.close_menus(app);
        app.close_all_menus();
        let package = &info.package;
        let source = MapSource::of_start(package);
        let level = source.label();
        // With a network session always `multiplayer.ldg` (E-321; `Game_StartLevel` `0x004C8710` `NetSession_IsActive`
        // -> `0x004C8749`), never the `<Level>.ldg`; random map `randommap.ldg`.
        let ldg = match &source {
            MapSource::Random(_) | MapSource::SavedRandom(_) => RANDOM_MAP_LDG.to_string(),
            MapSource::Level(_) => MULTIPLAYER_LDG.to_string(),
        };
        view.apply_settings(app.config());
        let host = net.is_host();
        let result = self
            .load(&ldg, present, |progress| {
                view.start(
                    env.vfs,
                    &source,
                    &mut |rules| net_setup(rules, package),
                    info.game_speed,
                    progress,
                )
            })
            .and_then(|()| {
                let sim = view
                    .sim_mut()
                    .context("the map has no simulation (rules not loadable)")?;
                sim.enter_rounds(info.your_slot, host)
                    .map_err(anyhow::Error::msg)
            });
        match result {
            Ok(()) => {
                net.loaded(None);
                view.attach_net(net);
                let meta = MatchMeta {
                    level_info: LevelInfoData {
                        path: level.clone(),
                        players: i32::try_from(package.setup.lobby.len()).unwrap_or(i32::MAX),
                        ..LevelInfoData::default()
                    },
                    game_type: package.setup.mode,
                    // `LockTeams` of the session (command line without lobby: off).
                    lock_teams: self
                        .net
                        .as_ref()
                        .and_then(NetSession::session)
                        .is_some_and(|s| s.settings.lock_teams),
                    save_replay: self
                        .net
                        .as_ref()
                        .and_then(NetSession::session)
                        .is_some_and(|s| s.settings.save_replay),
                    playback: None,
                };
                self.started(meta, app, view);
                self.restart_recording(view, env);
                log::info!(
                    "Network: match \"{level}\" loaded, own slot {}, waiting for Go",
                    info.your_slot
                );
                Ok(())
            }
            Err(e) => {
                let error = format!("{e:#}");
                log::warn!("Network: match \"{level}\" not started: {error}");
                Err(Self::net_start_failed(net, error, app, view, env))
            }
        }
    }

    /// Start of a loaded network match from the own file (deviating from E-328; loading path of E-275). Like
    /// [`Self::start_net_match`], menu level, menus and background images close first. Then each machine reads
    /// **its own** network savegame `savegame/multiplayer/<Name>` ([`own_net_save`]: name valid, checksum of the
    /// simulation part, tick, start package and `local_slot == your_slot` as in the start of the host) and resumes it
    /// like [`Self::load_game`] ([`SessionView::resume`] with the logic comparison against the rules of the map and
    /// [`GameSaveFile::prepare`]; host part own: camera, groups, masks, fog) with the loading image of savegame
    /// loading ([`saved_ldg`]) and the speed of the host (`info.game_speed` instead of the own host part). Then turn
    /// mode on the own slot ([`SimHost::enter_rounds`]), origin tick = tick of the state ([`NetDriver::set_origin`]:
    /// turn 0 begins there), `Loaded`, driver into the match and the match data from the own host part. The names in
    /// the simulation stay those of the state. Every error becomes `Loaded { error }` (the host aborts the start for
    /// everyone) with the main menu as in [`Self::start_net_match`].
    fn start_saved_net_match(
        &mut self,
        mut net: NetDriver,
        info: StartInfo,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) -> Result<(), Box<NetStartFailure>> {
        self.end_menu_level(app, view);
        self.close_menus(app);
        app.close_all_menus();
        let mut save = match own_net_save(net.paths(), &info) {
            Ok(save) => save,
            Err(error) => {
                log::warn!("Network: loaded match not started: {error}");
                return Err(Self::net_start_failed(net, error, app, view, env));
            }
        };
        let name = info.saved.as_ref().map_or("", |s| s.name.as_str());
        let origin = save.sim.state.steps;
        let ldg = saved_ldg(&save, env.vfs);
        let meta = MatchMeta {
            level_info: save.host.level_info.clone(),
            game_type: save.host.game_type,
            lock_teams: save.host.lock_teams,
            save_replay: save.host.save_replay,
            playback: None,
        };
        save.host.game_speed = info.game_speed;
        view.apply_settings(app.config());
        let host = net.is_host();
        let result = self
            .load(&ldg, present, |progress| {
                view.resume(env.vfs, save, progress)
            })
            .and_then(|()| {
                let sim = view
                    .sim_mut()
                    .context("the savegame has no simulation (rules not loadable)")?;
                sim.enter_rounds(info.your_slot, host)
                    .map_err(anyhow::Error::msg)
            });
        match result {
            Ok(()) => {
                net.set_origin(origin);
                net.loaded(None);
                view.attach_net(net);
                self.started(meta, app, view);
                self.restart_recording(view, env);
                log::info!(
                    "Network: savegame \"{name}\" loaded at tick {origin}, own slot {}, waiting for Go",
                    info.your_slot
                );
                Ok(())
            }
            Err(e) => {
                let error = format!("{e:#}");
                log::warn!("Network: savegame \"{name}\" not resumed: {error}");
                Err(Self::net_start_failed(net, error, app, view, env))
            }
        }
    }

    /// Failed network start: a half-started match does not stay (none was running before), `Loaded { error }` to the
    /// driver (for the host an abort for everyone), menu music and main menu on again; the driver goes back. Over the
    /// main menu message 99 113 with the own cleaned name, like on the other machines from the host's `LoadFailed`
    /// (deviation from E-352); "OK" without a follow-up command. The reason itself stays in the log.
    fn net_start_failed(
        mut net: NetDriver,
        error: String,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
    ) -> Box<NetStartFailure> {
        view.end();
        net.loaded(Some(error.clone()));
        view.restore_menu_music(env.vfs);
        app.open_main_menu();
        let name = opennml_net::clean_name(
            &name_text(&app.config().net_name),
            &env.texts.display(PLAYER),
        );
        app.tell_text(load_failed_message(&name, env.texts), None);
        Box::new(NetStartFailure { error, net })
    }

    /// `0x0C` -> `FUN_004C8EC0(path, -1)` -> `Game_StartLevel` (re/singleplayer-menus part 1, 6.2): background images
    /// of the desktop removed as for `0x0D`, loading screen by `ldg_path_for` (game type -1, restart: the `.ldg` of the
    /// level, otherwise `default.ldg`), load level, setup [`campaign_setup`] with the difficulty `GameApp+0x19B4`,
    /// `TimePerChar` and the profile name `NetName`, speed `GameSpeed`. `0x0C` does not set the information
    /// `[CURLEVELINFOS]` (`GameApp+0xD654`); the host part has the default. If loading or setup fails: message in the
    /// log, menu music on again and `selectcampaign` (`MenuApp::reopen_campaign_menu`; deviating from the original -
    /// the original would stay without a game and menu, `0x0C` does not evaluate the result).
    fn start_campaign_level(
        &mut self,
        level: &str,
        difficulty: i32,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        self.end_menu_level(app, view);
        self.close_menus(app);
        let ldg = ldg_path_for(level, -1, false, |p| env.vfs.exists(p));
        let speed = config_speed(app.config());
        view.apply_settings(app.config());
        let time_per_char = app.config().time_per_char;
        let local_name = name_text(&app.config().net_name);
        // Profile variables (`m_VarMap`) of the active profile for 383.
        let vars = app.profiles().current_vars().to_vec();
        let result = self.load(&ldg, present, |progress| {
            view.start(
                env.vfs,
                &MapSource::Level(level.to_string()),
                &mut |_| {
                    Ok(campaign_setup(
                        difficulty,
                        time_per_char,
                        Some(local_name.clone()),
                        &vars,
                    ))
                },
                speed,
                progress,
            )
        });
        match result {
            Ok(()) => {
                // `0x0C` does not reset the session: `Engine+0x20` comes from `LockTeams` of the settings (default 0).
                let meta = MatchMeta {
                    level_info: LevelInfoData::default(),
                    game_type: -1,
                    lock_teams: app.config().lock_teams != 0,
                    save_replay: false,
                    playback: None,
                };
                self.started(meta, app, view);
                // E-334: no recording in the campaign; an old one ends here.
                self.restart_recording(view, env);
                log::info!("Campaign level started: {level} (difficulty {difficulty})");
            }
            Err(e) => {
                log::warn!("Campaign level \"{level}\" not started: {e:#}");
                // `0x0C` comes only without a running match (`LEVELSTART`, `ACCOUNTING`, `0x28`); `start` has already
                // stopped the menu music.
                view.restore_menu_music(env.vfs);
                app.reopen_campaign_menu();
            }
        }
    }

    /// `0x0A` (re/ingame-menus 4, re/singleplayer-menus 6.4): read and check `savegame/<Name>.<GUID>`, load with
    /// `default.ldg` or the `.ldg` of the level (O-139), then replace the old match; errors as in the module docs.
    /// If the savegame does not load, the menu opens again with a message ([`save_failure_text`]), "OK" without a
    /// follow-up command.
    fn load_game(
        &mut self,
        name: &str,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        let read = || -> anyhow::Result<GameSaveFile> {
            check_name(name).map_err(anyhow::Error::msg)?;
            let path = env.files.path(name, active_guid(app));
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            Ok(GameSaveFile::from_bytes(&bytes)?)
        };
        let save = match read() {
            Ok(save) => save,
            Err(e) => {
                log::warn!("Savegame \"{name}\" not loaded: {e:#}");
                app.reopen_menu();
                let kind = e.downcast_ref::<SaveError>().map(|s| s.kind);
                app.tell(save_failure_text(kind, false), None);
                return;
            }
        };
        let ldg = saved_ldg(&save, env.vfs);
        let meta = MatchMeta {
            level_info: save.host.level_info.clone(),
            game_type: save.host.game_type,
            lock_teams: save.host.lock_teams,
            save_replay: save.host.save_replay,
            playback: None,
        };
        // Loading ends the menu level (`FUN_004BAF40`) before loading.
        self.end_menu_level(app, view);
        view.apply_settings(app.config());
        match self.load(&ldg, present, |progress| {
            view.resume(env.vfs, save, progress)
        }) {
            Ok(()) => {
                self.started(meta, app, view);
                self.restart_recording(view, env);
                log::info!("Savegame \"{name}\" loaded");
            }
            Err(e) => {
                log::warn!("Savegame \"{name}\" not loaded: {e:#}");
                self.failed_load(app, view, env);
                let kind = e.downcast_ref::<SaveError>().map(|s| s.kind);
                app.tell(save_failure_text(kind, true), None);
            }
        }
    }

    /// `0x0B` (`FUN_004C7D50`, re/ingame-menus 4.1): write the savegame with host part. `SAVINGMESSAGE` has already
    /// closed `0x0B`, the game keeps running afterwards; if writing fails, only a message in the log (O-149; the
    /// original would stay stuck with `SAVINGMESSAGE` and `Engine+0x115`).
    fn save_game(&mut self, name: &str, app: &MenuApp, view: &dyn SessionView, env: &FlowEnv) {
        let write = || -> anyhow::Result<std::path::PathBuf> {
            check_name(name).map_err(anyhow::Error::msg)?;
            let host = self.host_part(app, view).context("no running match")?;
            let sim = view.sim().context("no simulation")?;
            let file =
                GameSaveFile::new(sim.save(), host, view.random_map().cloned().map(Box::new));
            let path = env.files.path(name, active_guid(app));
            write_file(&path, &file.to_bytes())?;
            Ok(path)
        };
        match write() {
            Ok(path) => log::info!("Savegame saved: {}", path.display()),
            Err(e) => {
                log::warn!("Savegame \"{name}\" not saved, the game keeps running: {e:#}")
            }
        }
    }

    /// `0x1F` (`FUN_004C8EC0`): restart the running match with its start package (level, setup), speed `GameSpeed`,
    /// background images removed ([`Self::close_menus`]); nothing without a match. If it fails, the old one stays (as
    /// for loading), message in the log. A random map restarts from its description ([`MapSource::of_match`]) with
    /// the loading image of the restart `randommap.ldg` (`FUN_00547560` at `RM+0x58 = 0`, O-431).
    fn reload_level(
        &mut self,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        let (Some(meta), Some(sim)) = (self.meta.clone(), view.sim()) else {
            log::info!("Restart level without a running match: nothing to do");
            return;
        };
        let setup = sim.setup.clone();
        let level = view.level().to_string();
        let source = MapSource::of_match(&sim.rules, &level);
        self.close_menus(app);
        let ldg = if source.is_random() {
            RANDOM_MAP_LDG.to_string()
        } else {
            ldg_path_for(&level, meta.game_type, false, |p| env.vfs.exists(p))
        };
        let speed = config_speed(app.config());
        view.apply_settings(app.config());
        let result = self.load(&ldg, present, |progress| {
            view.start(
                env.vfs,
                &source,
                &mut |_| Ok(setup.clone()),
                speed,
                progress,
            )
        });
        match result {
            Ok(()) => {
                self.started(meta, app, view);
                self.restart_recording(view, env);
            }
            Err(e) => {
                log::warn!("Level \"{level}\" not restarted: {e:#}");
                app.reopen_menu();
            }
        }
    }
}

/// Game time text of `0x0E` (`0x004C9FAD`...`0x004CA001`, format `0x0061F278`): `t` = game time in ms,
/// `sek = t/1000`, `min = sek/60`, `"%s %d:%02d"` with `label`, `min/60` and `min%60` - **hours:minutes** (O-164).
/// In the reimplementation the game time is `steps * 50` ms of the simulation (deviation from the original; the
/// original counts `Engine+0x360` in real-time ticks including pause and stutters, O-165), as `int` in 32 bits like
/// `Engine+0x360`.
pub fn play_time(label: &str, steps: u64) -> String {
    let t = (steps as u32).wrapping_mul(opennml_sim::tick::STEP_MS) as i32;
    let minutes = t / 1000 / 60;
    format!("{label} {}:{:02}", minutes / 60, minutes % 60)
}

/// Slot with the profile name (`Game_SelectLocalPlayer` `0x004BB3B0`: the first active slot with control type human),
/// only if the match carries a profile name ([`MatchSetup::local_name`], O-250).
fn profile_slot(sim: &SimHost) -> Option<usize> {
    sim.setup.local_name.as_ref().and_then(|_| {
        sim.state
            .players
            .iter()
            .position(|p| p.slot_active && p.control == opennml_sim::state::Control::Human)
    })
}

/// Display name from the raw name of a slot: in the campaign (game type < 0) `PlayerSetup_ParseLvl` (`0x00486640`,
/// `0x00486925...0x00486951`) creates the records with the resolved `PLAYER_NAME`: if it begins with a number >= 1
/// other than 29320 (`_atol`), the text of this number (`FUN_0055FFE0`) stands in its place, here shortened to 39
/// characters like every name (deviation from the original). Excepted are the profile slot (`is_profile`: its name
/// is the profile name, raw) and free play (all names raw from the lobby).
fn resolve_display_name(raw: &str, mode: i32, is_profile: bool, texts: &Texts) -> String {
    match opennml_formats::lvl::name_text_id(raw) {
        Some(id) if mode < 0 && !is_profile => {
            let id = i32::try_from(id).unwrap_or(i32::MAX);
            texts.resolve(id);
            texts
                .display(id)
                .chars()
                .take(opennml_sim::accounting::MAX_NAME_CHARS)
                .collect()
        }
        _ => raw.to_string(),
    }
}

/// Name of the slot `slot` for display (game value tags `[PLAYERNAME n]`, messages; O-250). The original writes the
/// name into `Player+0xC`: the resolved map name (`PlayerSetup_ParseLvl`) and for the local player always the profile
/// name `NetName` (`Game_SelectLocalPlayer` `0x004BB446`, also in the campaign). The reimplementation leaves
/// `Player.name` raw (checksum, savegame) and computes the name here:
/// - slot outside 0...7: empty (`GameEngine_GetPlayer` `0x0042D560`);
/// - profile slot ([`profile_slot`]): the profile name;
/// - game type < 0 with a text ID in the name: the text, shortened to 39 characters;
/// - otherwise the raw name.
pub fn player_display_name(sim: &SimHost, texts: &Texts, slot: i32) -> String {
    let Some(index) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < opennml_sim::id::PLAYER_SLOTS)
    else {
        return String::new();
    };
    let Some(player) = sim.state.players.get(index) else {
        return String::new();
    };
    if Some(index) == profile_slot(sim) {
        return sim.setup.local_name.clone().unwrap_or_default();
    }
    resolve_display_name(&player.name, sim.state.mode, false, texts)
}

/// State of the diplomacy dialog ([`diplomacy::view`]) with the display names of the slots ([`player_display_name`]).
fn diplomacy_view(
    sim: &SimHost,
    masks: &Masks,
    local: u8,
    lock_teams: bool,
    texts: &Texts,
) -> DiplomacyView {
    diplomacy::view(sim, masks, local, lock_teams, |slot| {
        player_display_name(sim, texts, slot)
    })
}

/// Mission objectives and tip log for `QUESTMENU2`/`TIPLOGMENU` from the mission state (`GameEngine` tables of the ten
/// main objectives, `m_InfoHistory`; re/campaign-ui D6.8, D6.9). The original shows secondary objectives nowhere
/// (O-53).
fn mission_info(sim: &SimHost) -> MissionInfo {
    let mission = &sim.state.script.mission;
    MissionInfo {
        main_quests: mission.main_quests.map(|q| (q.status, q.text)),
        info_history: mission.info_history.clone(),
    }
}

/// Accounting of the match for the UI (`FUN_00403FA0` and step 4 of `0x0E`, re/accounting 4.1 and 6): the table of
/// the simulation ([`opennml_sim::accounting::accounting_table`], empty name -> text 15195) with the values as `"%d"` and
/// the game time text ([`play_time`] with text 15349).
///
/// **Names with a text ID:** like [`player_display_name`] ([`resolve_display_name`]); here the names come from the
/// statistics log, though (the record of the profile slot carries the profile name, `FUN_00403F50` copies it raw).
pub fn accounting_view(sim: &SimHost, texts: &Texts) -> AccountingView {
    let state = &sim.state;
    let table = opennml_sim::accounting::accounting_table(state, &texts.display(COMPUTER_TEXT));
    let records = &state.accounting.records;
    let slots = (0..records.len()).filter(|&p| records[p].active);
    let profile_slot = profile_slot(sim);
    let rows = table
        .into_iter()
        .zip(slots)
        .map(|(row, slot)| {
            let name =
                resolve_display_name(&row.name, state.mode, Some(slot) == profile_slot, texts);
            (name, row.color, row.values.map(|v| v.to_string()))
        })
        .collect();
    AccountingView {
        rows,
        time_text: play_time(&texts.display(PLAY_TIME_TEXT), state.steps),
    }
}

/// `0x2A` (`FUN_004BF9A0`): `DeleteFileA(savegame/<Name>.<GUID>)`, in the network folder `savegame/multiplayer/<Name>`
/// without ending (O-150); the UI has already reopened the dialog (list refreshed). An empty name (O-144 with an
/// empty list) or an error: message in the log.
fn delete_save(name: &str, dir: SaveDir, app: &MenuApp, env: &FlowEnv) {
    if let Err(e) = check_name(name) {
        log::warn!("Savegame not deleted: {e}");
        return;
    }
    let path = env.files.path_in(dir, name, active_guid(app));
    match std::fs::remove_file(&path) {
        Ok(()) => log::info!("Savegame deleted: {}", path.display()),
        Err(e) => log::warn!("Savegame {} not deleted: {e}", path.display()),
    }
}

/// The match in `opennml-game`: a `GameSession` with device, archives, view and sound system of the host, rebuilt per
/// frame. Device and queue are copies of the handles (wgpu counts them), so that the host can draw its window while
/// loading. `view` and `controls` belong to the host; [`SessionView::apply_settings`] rewrites them from the
/// settings, the next start takes them over.
pub struct LiveSession<'a> {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub vfs: &'a Vfs,
    pub session: &'a mut Option<GameSession>,
    pub view: &'a mut ViewOptions,
    pub controls: &'a mut Controls,
    /// Image size at the start.
    pub size: (u32, u32),
    pub audio: Option<&'a mut AudioEngine>,
}

impl LiveSession<'_> {
    /// `MenuMusic_Stop` before loading (`Game_StartLevel` step 2 `MenuManager_CloseAllAndStopMusic`,
    /// `FUN_004C7C20`).
    fn stop_menu_music(&mut self) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.stop_menu_music();
        }
    }

    /// The old session goes (new start, load, end): its dialog line fades out instead of sounding over the new
    /// level.
    fn stop_old_speech(&mut self) {
        if let (Some(old), Some(audio)) = (self.session.as_mut(), self.audio.as_deref_mut()) {
            old.stop_speech(audio);
        }
    }

    /// [`SessionView::start`] or, with `menu_level`, [`SessionView::start_menu_level`] (menu music keeps playing).
    fn start_session(
        &mut self,
        vfs: &Vfs,
        source: &MapSource,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
        menu_level: bool,
    ) -> anyhow::Result<()> {
        if !menu_level {
            self.stop_menu_music();
        }
        let gpu = SessionGpu {
            device: &self.device,
            queue: &self.queue,
            format: self.format,
        };
        let mut session = GameSession::start_source(
            &gpu, vfs, source, setup, game_speed, self.view, self.size, progress,
        )?;
        session.controls = self.controls.clone();
        // Level music like in the viewer: with a level script without start music. In the network only with the own
        // slot ([`SessionView::attach_net`]): until `enter_rounds` the first human of the map is the local player.
        let network = session.map.sim.as_ref().is_some_and(|s| s.state.network);
        if let Some(audio) = self.audio.as_deref_mut().filter(|_| !network) {
            session.start_level_audio(audio, vfs);
        }
        // Only after a successful start: the old line fades out instead of sounding over the new level.
        self.stop_old_speech();
        *self.session = Some(session);
        Ok(())
    }
}

impl SessionView for LiveSession<'_> {
    fn start(
        &mut self,
        vfs: &Vfs,
        source: &MapSource,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        self.start_session(vfs, source, setup, game_speed, progress, false)
    }

    fn start_menu_level(
        &mut self,
        vfs: &Vfs,
        level: &str,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        let source = MapSource::Level(level.into());
        self.start_session(vfs, &source, setup, game_speed, progress, true)
    }

    fn resume(
        &mut self,
        vfs: &Vfs,
        save: GameSaveFile,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        self.stop_menu_music();
        let gpu = SessionGpu {
            device: &self.device,
            queue: &self.queue,
            format: self.format,
        };
        let mut session =
            GameSession::resume_from(&gpu, vfs, save, self.view, self.size, progress)?;
        session.controls = self.controls.clone();
        self.stop_old_speech();
        *self.session = Some(session);
        Ok(())
    }

    fn sim(&self) -> Option<&SimHost> {
        self.session.as_ref().and_then(GameSession::sim)
    }

    fn sim_mut(&mut self) -> Option<&mut SimHost> {
        self.session.as_mut().and_then(GameSession::sim_mut)
    }

    fn advance(&mut self, now: u64, running: bool) -> u32 {
        let Some(session) = self.session.as_mut() else {
            return 0;
        };
        let gpu = SessionGpu {
            device: &self.device,
            queue: &self.queue,
            format: self.format,
        };
        session.advance(&gpu, self.vfs, now, running, self.audio.as_deref_mut());
        session.ticks
    }

    fn hold(&mut self, now: u64) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let gpu = SessionGpu {
            device: &self.device,
            queue: &self.queue,
            format: self.format,
        };
        session.hold(&gpu, self.vfs, now, self.audio.as_deref_mut());
    }

    fn input(&mut self, ev: &InputEvent, screen: Screen, now: u64) {
        if let Some(session) = self.session.as_mut() {
            session.input(ev, screen, now, self.vfs, self.audio.as_deref_mut());
        }
    }

    fn gameui_center(&self, element: &str) -> Result<(f64, f64), String> {
        self.session
            .as_ref()
            .ok_or_else(|| NO_MATCH.to_string())?
            .gameui_center(element)
    }

    fn level(&self) -> &str {
        self.session.as_ref().map_or("", GameSession::level)
    }

    fn random_map(&self) -> Option<&RandomMap> {
        self.session.as_ref().and_then(GameSession::random_map)
    }

    fn camera(&self) -> CameraSave {
        self.session.as_ref().map_or(NO_CAMERA, GameSession::camera)
    }

    fn end(&mut self) {
        self.stop_old_speech();
        *self.session = None;
    }

    fn apply_settings(&mut self, cfg: &EngineConfig) {
        *self.view = view_options(cfg, self.view.text_width);
        *self.controls = controls(cfg);
        if let Some(session) = self.session.as_mut() {
            session.controls = self.controls.clone();
            // Camera keys like the original: `KD_` keys, rotating only with `LockCamera 0`.
            session.camera_keys = CameraKeys::Original {
                lock_camera: cfg.lock_camera != 0,
            };
            // Tinting of the pointer by the construction cursor only for the 3D pointer (`HardwareCursor 0`, O-394).
            session.software_pointer = !cfg.hardware_cursor;
            session.view.unit_response = self.view.unit_response;
            session.voices.unit_response = self.view.unit_response;
            // Key layout for `[KEYID]` and `EnhancedTooltips` of the tooltip.
            if session.key_map != cfg.key_map {
                session.key_map = cfg.key_map.clone();
            }
            session.hud.enhanced_tooltips = cfg.enhanced_tooltips != 0;
        }
    }

    fn restore_menu_music(&mut self, vfs: &Vfs) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.restart_menu_music(vfs);
        }
    }

    fn stop_game_music(&mut self) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.stop_music();
        }
    }

    fn take_host_requests(&mut self) -> Vec<HostRequest> {
        self.session
            .as_mut()
            .map_or_else(Vec::new, GameSession::take_host_requests)
    }

    fn cinemascope_speed(&self) -> Option<u8> {
        self.session
            .as_ref()
            .and_then(GameSession::cinemascope_speed)
    }

    fn script_camera(&self) -> ScriptCamera {
        self.session
            .as_ref()
            .map_or_else(ScriptCamera::default, |s| s.presentation.camera.clone())
    }

    fn control_groups(&self) -> Vec<GroupsSave> {
        self.session
            .as_ref()
            .map_or_else(control_groups::empty_save, GameSession::control_groups_save)
    }

    fn next_guid_index(&self) -> Vec<NextGuidIndex> {
        self.session.as_ref().map_or_else(
            control_groups::empty_next_guid_index,
            GameSession::next_guid_index_save,
        )
    }

    fn idle_worker_index(&self) -> [i32; 8] {
        self.session
            .as_ref()
            .map_or([0; 8], |s| s.idle_worker_index)
    }

    fn masks(&self) -> Masks {
        self.session
            .as_ref()
            .map_or_else(Masks::initial, |s| s.masks)
    }

    fn set_masks(&mut self, masks: Masks) {
        if let Some(session) = self.session.as_mut() {
            session.masks = masks;
        }
    }

    fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        self.session
            .as_mut()
            .map_or_else(Vec::new, GameSession::take_ui_requests)
    }

    fn set_match_info(&mut self, game_type: i32, menu_open: bool) {
        if let Some(session) = self.session.as_mut() {
            session.set_game_type(game_type);
            session.menu_open = menu_open;
        }
    }

    fn set_player_names(&mut self, names: [String; opennml_sim::id::PLAYER_SLOTS]) {
        if let Some(session) = self.session.as_mut() {
            session.player_names = Some(names);
        }
    }

    fn input_gate(&self) -> InputGate {
        self.session
            .as_ref()
            .map_or_else(InputGate::default, GameSession::input_gate)
    }

    fn run_hotkey(&mut self, cmd: &HotkeyCommand, now: u64) -> bool {
        let vfs = self.vfs;
        let audio = self.audio.as_deref_mut();
        self.session
            .as_mut()
            .is_some_and(|s| s.run_hotkey(cmd, now, vfs, audio))
    }

    fn change_game_speed(&mut self, d: i32) -> Option<u8> {
        let vfs = self.vfs;
        let audio = self.audio.as_deref_mut();
        self.session
            .as_mut()
            .and_then(|s| s.change_game_speed_played(d, vfs, audio))
    }

    fn key_context(&self) -> KeyContext {
        self.session
            .as_ref()
            .map_or_else(KeyContext::default, GameSession::key_context)
    }

    fn pause(&self) -> PauseState {
        self.session
            .as_ref()
            .map_or_else(PauseState::default, |s| s.pause)
    }

    fn toggle_pause(&mut self) -> bool {
        self.session.as_mut().is_some_and(GameSession::toggle_pause)
    }

    fn pointer_choice(&self) -> PointerChoice {
        self.session
            .as_ref()
            .map_or(PointerChoice::NORMAL, GameSession::pointer_choice)
    }

    fn net(&self) -> Option<&NetDriver> {
        self.session.as_ref().and_then(|s| s.net.as_ref())
    }

    fn net_mut(&mut self) -> Option<&mut NetDriver> {
        self.session.as_mut().and_then(|s| s.net.as_mut())
    }

    fn take_net_frame(&mut self) -> NetFrame {
        self.session
            .as_mut()
            .map_or_else(NetFrame::default, GameSession::take_net_frame)
    }

    fn attach_net(&mut self, net: NetDriver) {
        match self.session.as_mut() {
            Some(session) => {
                // The own slot is only fixed now (`enter_rounds`); the camera of loading stood at the start of the
                // first human of the map. A loaded match keeps the camera of its host part.
                if net.start_info().is_none_or(|s| s.saved.is_none()) {
                    session.map.center_on_start();
                }
                // Likewise the level music that `start` held back in the network: the nation music of the own slot, as
                // the original sets the local player before `Game_FinishLevelSetup`.
                if let Some(audio) = self.audio.as_deref_mut() {
                    session.start_level_audio(audio, self.vfs);
                }
                session.net = Some(net);
            }
            None => log::error!("Network: driver without a match discarded"),
        }
    }

    fn begin_playback(&mut self, commands: Vec<RecordedCommand>) {
        let Some(session) = self.session.as_mut() else {
            log::error!("Replay without a match discarded");
            return;
        };
        session.begin_playback(commands);
        // A network recording: `start` held back the level music for the own slot (`attach_net`); the replay has no
        // network, the music is that of the viewing slot.
        let network = session.sim().is_some_and(|s| s.state.network);
        if let Some(audio) = self.audio.as_deref_mut().filter(|_| network) {
            session.start_level_audio(audio, self.vfs);
        }
    }
}

/// Camera without a match (never in the savegame: without a match there is no host part).
pub const NO_CAMERA: CameraSave = CameraSave {
    from: [0.0; 3],
    at: [0.0; 3],
    fov: 0.0,
};

mod menu_level;
mod playback;

#[cfg(test)]
mod chat_tests;
#[cfg(test)]
mod diplomacy_tests;
#[cfg(test)]
mod gameui_tests;
#[cfg(test)]
mod menu_level_tests;
#[cfg(test)]
mod net_tests;
#[cfg(test)]
mod replay_tests;
#[cfg(test)]
mod speed_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_golden;
