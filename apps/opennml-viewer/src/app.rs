//! winit application: map view with original camera and animation, model browser (B), debug overlay, mode
//! switch (F11), monitor switch (F10), quit (Esc).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, bail};
use glam::{Vec2, Vec3};
use image::RgbaImage;
use opennml_audio::engine::{AudioEngine, SoundSource};
use opennml_audio::volume::Category;
use opennml_client::clock::{Clock, ClockMode};
use opennml_client::control::ControlEvent;
use opennml_client::frames::FrameCounter;
use opennml_client::keys::{
    HeldKeys, InputEvent, InputSource, MouseButton, key_code, key_events, step_sign,
};
use opennml_client::menu_host::MenuHost;
use opennml_client::snapshot::{png_data, window_mode, window_mode_name};
use opennml_client::ui_control::{NO_UI, execute_ui};
use opennml_client::ui_layer::screen_of;
use opennml_client::vk::char_of;
use opennml_client::window::{WindowMode, WindowShell};
use opennml_control::{
    CaptureCommand, ClockState, Command, CursorState, EnvironmentState, HelloInfo, MouseButtonName,
    MusicCommand, PROTOCOL_VERSION, PlaySoundResult, RandomMapState, RecentSoundState, Request,
    Response, StateSnapshot, Toggles, ViewName, WindowState,
};
use opennml_rda::Vfs;
use opennml_render::FrameOutcome;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

use crate::browser::{BrowserView, sound_line, step_index};
use crate::config::{Config, MAX_SHADOW_QUALITY, Mission, apply_config, match_setup};
use crate::debug_ui::{DebugView, EguiLayer};
use crate::dialogs::{self, DialogLayer, DialogMenu, KeyRoute};
use crate::effect_browser::EffectBrowser;
use crate::hud_view::{self, HudLayer, OriginalKeys};
use crate::snapshot::{
    aspect_name, audio_state, camera_state, capture_data, clock_mode_name, describe_object,
    effect_state, free_camera_state, object_state, record_recent_sound, shadow_state, weather_kind,
    weather_name,
};
use opennml_client::game::economy::{EconomyUi, action_line, placement_line};
use opennml_client::game::formation_order::FormationOrder;
use opennml_client::game::grid::world_to_grid;
use opennml_client::game::hotkeys::HotkeyCommand;
use opennml_client::game::input::FreeCamera;
use opennml_client::game::map::{MapLoad, MapView};
use opennml_client::game::overlay::{
    OverlayInfo, OverlayLine, audio_line, browser_lines, effect_line, effect_lines,
    environment_line, overlay_lines, shadow_line,
};
use opennml_client::game::path_view::{PathView, path_overlay};
use opennml_client::game::unit_bar::{BarAction, UnitBar, formation_drag};
use opennml_client::game::voices::Voices;
use opennml_client::game_session::{GameSession, HostModes, SessionEvent, SessionGpu, wheel_up};

/// What the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Map,
    Browser,
    Effects,
}

/// Section `sim` with the number of flights of the map (protocol 12): the flights are only presentation and are
/// not part of the simulation state. All responses that carry the section build it here so that `flights` is
/// never left at 0; likewise `formation`, the formation chosen in the unit bar (viewer state, protocol 13).
fn section_with_flights(
    sim: &opennml_client::game::sim_host::SimHost,
    map: &MapView,
    selection: &[opennml_sim::id::ObjectId],
    formation: i32,
) -> opennml_control::SimSection {
    let mut section = sim.section(selection);
    section.flights = map.flight_count();
    section.formation = formation;
    section
}

/// Values of `sim_setup` (each optional; missing ones as last time).
#[derive(Debug, Default)]
pub(crate) struct SimSetupArgs {
    pub(crate) mode: Option<i32>,
    pub(crate) seed: Option<u32>,
    pub(crate) nations: Option<Vec<String>>,
    pub(crate) teams: Option<Vec<i32>>,
    pub(crate) game_speed: Option<u8>,
    pub(crate) resource_setting: Option<i32>,
    pub(crate) living_space: Option<i32>,
    pub(crate) map_explored: Option<bool>,
    /// Protocol 26.
    pub(crate) slots: Option<Vec<opennml_control::SlotSetup>>,
    pub(crate) ki_enabled: Option<bool>,
    pub(crate) campaign_difficulty: Option<i32>,
    /// Protocol 28.
    pub(crate) script_enabled: Option<bool>,
    pub(crate) time_per_char: Option<i32>,
    /// Time limit in minutes.
    pub(crate) time_limit: Option<i32>,
}

impl SimSetupArgs {
    /// Applies the given values to `config` (they also apply to later restarts); `true` if a new match is
    /// needed (everything except `game_speed`). `slots` replaces `nations` and `teams` and vice versa (they are
    /// mutually exclusive, protocol 26; [`Config::set_lobby`]).
    pub(crate) fn apply(self, config: &mut Config) -> bool {
        let Self {
            mode,
            seed,
            nations,
            teams,
            game_speed,
            resource_setting,
            living_space,
            map_explored,
            slots,
            ki_enabled,
            campaign_difficulty,
            script_enabled,
            time_per_char,
            time_limit,
        } = self;
        let restart = mode.is_some()
            || seed.is_some()
            || nations.is_some()
            || teams.is_some()
            || resource_setting.is_some()
            || living_space.is_some()
            || map_explored.is_some()
            || slots.is_some()
            || ki_enabled.is_some()
            || campaign_difficulty.is_some()
            || script_enabled.is_some()
            || time_per_char.is_some()
            || time_limit.is_some();
        config.set_lobby(nations, teams, slots);
        config.game_mode = mode.or(config.game_mode);
        config.seed = seed.unwrap_or(config.seed);
        config.game_speed = game_speed.unwrap_or(config.game_speed);
        config.resource_setting = resource_setting.unwrap_or(config.resource_setting);
        config.living_space = living_space.unwrap_or(config.living_space);
        config.map_explored = map_explored.unwrap_or(config.map_explored);
        config.ki_enabled = ki_enabled.unwrap_or(config.ki_enabled);
        config.campaign_difficulty = campaign_difficulty.unwrap_or(config.campaign_difficulty);
        config.script_enabled = script_enabled.unwrap_or(config.script_enabled);
        config.time_per_char = time_per_char.unwrap_or(config.time_per_char);
        config.time_limit = time_limit.or(config.time_limit);
        restart
    }
}

pub struct App {
    config: Config,
    vfs: Option<Vfs>,
    /// Window, GPU, window mode and frame timing; from `resumed`.
    shell: Option<WindowShell>,
    /// Match of the map view (map, simulation, camera control, selection, voices, click and drag state, target
    /// marker), from `resumed`.
    session: Option<GameSession>,
    /// Model browser, created at the first switch (B) or at start with `--model`.
    browser: Option<BrowserView>,
    /// Effect browser, created at the first switch (N).
    effect_browser: Option<EffectBrowser>,
    view: ViewMode,
    /// Model with which the browser starts (`--model`).
    start_model: Option<String>,
    left_button: bool,
    /// Mouse movement with the left button pressed since the last frame (browser only).
    drag: Vec2,
    /// Held keys of the user and of the control channel.
    keys: HeldKeys,
    cursor: Option<(f64, f64)>,
    look: Vec2,
    right_button: bool,
    help: bool,
    /// Virtual clock for map, browser and camera.
    clock: Clock,
    /// FPS and frame times (real time).
    frames: FrameCounter,
    /// Game object under the mouse in the last frame (index in `scene.objects`).
    hovered: Option<usize>,
    /// Overlay lines of the last frame.
    overlay: Vec<OverlayLine>,
    /// Control commands that arrived before `resumed` (without window and GPU).
    waiting: Vec<ControlEvent>,
    /// `quit` via the control channel.
    quit_requested: bool,
    /// Sound (music, surf, rain, browser sounds), on the virtual clock.
    audio: Option<AudioEngine>,
    /// Debug window visible (key I).
    debug_open: bool,
    /// Window "Match" (button in the window "Simulation") and its draft (read from the running match when
    /// opened, discarded when closed).
    match_open: bool,
    match_draft: Option<crate::ki_ui::MatchDraft>,
    /// Missions of the campaigns from `campaign.ini` for the window "Match" (read at start).
    missions: Vec<Mission>,
    /// Random map of the loaded match (`--random-map`) for the command `random_map`; `None` for a level.
    random_map: Option<RandomMapState>,
    /// Window "AI" (key K).
    ki_window: crate::ki_ui::KiWindow,
    /// Window "Script" (key J, button in the window "Simulation").
    script_window: crate::script_ui::ScriptWindow,
    /// AI overlay (key L): slot whose `ai_marks` bit is highlighted; `None` off.
    ki_overlay: Option<u8>,
    /// Window "Economy" (key T), placement mode, confirmation prompt when selling.
    economy: EconomyUi,
    /// Unit bar: chosen formation (viewer state, `GameWorld+0x20B8`) and its texts (`guids.deu` 509-519).
    unit_bar: UnitBar,
    bar_texts: BTreeMap<i32, String>,
    /// Text table of the language files (`localisation.*`) for the game tags of the message lines
    /// ([`GameSession::message_lines`]) and of the game UI ([`GameSession::hud_cmds`]); loaded once at start,
    /// independent of `--dialog`. Empty without a readable language file (warning).
    message_texts: opennml_ui::assets::Texts,
    /// Own drawing layer of the game UI (`--hud`/command `hud` without a dialog layer); with a dialog layer its
    /// layer draws. The switch itself is `config.hud`. The layer stays the same for the run time (the info
    /// windows of the match hold their texture IDs, `GameSession::info_frames`).
    hud_layer: Option<HudLayer>,
    /// Key path of the original with `--hud`: table of the held keys; free keys of the map go to
    /// `Hotkeys::resolve` and `GameSession::run_hotkey` ([`App::original_key`]).
    original_keys: OriginalKeys,
    /// Rejection of the last click in targeting mode ([`opennml_client::game::unit_bar::click_target`]); the status
    /// line shows it as long as targeting mode runs.
    target_notice: Option<String>,
    /// Left mouse button pressed in placement mode, with the unpack cursor or in targeting mode on the map (set
    /// on release).
    place_press: bool,
    /// Overlay of the pathfinding (key G): layer 0-8 or occupancy.
    grid_mode: Option<usize>,
    /// Path preview of the selected unit to the mouse pointer (cached per target and tick).
    path_preview: Option<(u64, PathView)>,
    /// Path shown with `sim_path`.
    shown_path: Option<PathView>,
    /// Attack mode (key X): the next right-click on the map attacks
    /// ([`opennml_client::game::sim_host::SimHost::issue_attack_action`]), after that it is off.
    attack_mode: bool,
    /// Rank textures of the health bar layer (loaded at the first icon).
    rank_icons: crate::combat_overlay::RankIcons,
    /// Most recently played combat sounds (`Sound3d`) for the state snapshot (protocol 11).
    recent_sounds: VecDeque<RecentSoundState>,
    /// egui (debug window, selection box); from `resumed`.
    egui: Option<EguiLayer>,
    /// Dialogs that `--dialog` opens at start (empty: no dialog layer), and the settings file of the menu UI
    /// (`--settings`, checked in `main`; `None`: in memory only).
    dialog_names: Vec<String>,
    dialog_settings: Option<PathBuf>,
    /// Operable dialog layer over the view (`--dialog`), from `resumed`; the control channel sees it in the
    /// frame (commands `ui_*`).
    dialogs: Option<DialogLayer>,
    /// The dialog layer has requested quit (`QUIT`, `0x25`).
    menu_quit: bool,
    error: Option<anyhow::Error>,
}

impl App {
    /// With `start_model` the window starts in the model browser, with `paused` the virtual clock stands at 0 ms
    /// from the start; with `audio_capture` the offline mix runs from the start (a path that cannot be created
    /// is an error before the event loop starts).
    pub fn new(
        config: Config,
        vfs: Vfs,
        start_model: Option<String>,
        paused: bool,
        audio_capture: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let mut audio = AudioEngine::open(&vfs, true);
        for (category, volume) in [
            (Category::Music, config.volume_music),
            (Category::Sfx, config.volume_sfx),
            (Category::Speech, config.volume_speech),
        ] {
            audio.set_volume(category, i32::from(volume));
        }
        audio.set_mute(config.mute);
        if let Some(path) = &audio_capture {
            begin_audio_capture(&mut audio, path)?;
        }
        let bar_texts = opennml_client::game::unit_bar::load_texts(&vfs);
        let missions =
            crate::config::campaign_missions(&opennml_ui::profiles::load_campaigns(&vfs));
        // Like `Notices::load` and `unit_bar::load_texts`: a missing language file only costs the texts.
        let localisation = opennml_ui::assets::load_localisation(&vfs, &config.language_tag)
            .unwrap_or_else(|e| {
                log::warn!("Message texts: language file not read: {e}");
                opennml_formats::localisation::Localisation::default()
            });
        let message_texts = opennml_ui::assets::Texts::load(&vfs, &localisation);
        Ok(Self {
            audio: Some(audio),
            view: if start_model.is_some() {
                ViewMode::Browser
            } else {
                ViewMode::Map
            },
            start_model,
            browser: None,
            effect_browser: None,
            left_button: false,
            drag: Vec2::ZERO,
            config,
            vfs: Some(vfs),
            shell: None,
            session: None,
            keys: HeldKeys::default(),
            cursor: None,
            look: Vec2::ZERO,
            right_button: false,
            help: false,
            clock: if paused {
                Clock::paused()
            } else {
                Clock::new()
            },
            frames: FrameCounter::new(Instant::now()),
            hovered: None,
            overlay: Vec::new(),
            waiting: Vec::new(),
            quit_requested: false,
            debug_open: false,
            match_open: false,
            match_draft: None,
            missions,
            random_map: None,
            ki_window: crate::ki_ui::KiWindow::default(),
            script_window: crate::script_ui::ScriptWindow::default(),
            ki_overlay: None,
            economy: EconomyUi::default(),
            unit_bar: UnitBar::default(),
            bar_texts,
            message_texts,
            hud_layer: None,
            original_keys: OriginalKeys::default(),
            target_notice: None,
            place_press: false,
            grid_mode: None,
            path_preview: None,
            shown_path: None,
            attack_mode: false,
            rank_icons: crate::combat_overlay::RankIcons::default(),
            recent_sounds: VecDeque::new(),
            egui: None,
            dialog_names: Vec::new(),
            dialog_settings: None,
            dialogs: None,
            menu_quit: false,
            error: None,
        })
    }

    /// Opens the dialog layer with `names` (`--dialog`) at window start; `settings` is the settings file of the
    /// menu UI (`None`: defaults in memory only).
    pub fn with_dialogs(mut self, names: Vec<String>, settings: Option<PathBuf>) -> Self {
        self.dialog_names = names;
        self.dialog_settings = settings;
        self
    }

    /// Ended via the control channel (`quit`).
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Finishes a running sound recording (`--audio-capture`) so that the WAV file gets its final header.
    pub fn finish_audio(&mut self) {
        if let Some(audio) = &mut self.audio {
            match audio.stop_capture() {
                Ok(Some(summary)) => log::info!(
                    "Sound recording {} finished: {:.0} ms",
                    summary.path.display(),
                    summary.length_ms
                ),
                Ok(None) => {}
                Err(e) => log::error!("Sound recording: {e:#}"),
            }
        }
    }

    /// Error that led to quitting.
    pub fn into_result(self) -> anyhow::Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        log::error!("{error:#}");
        self.error = Some(error);
        event_loop.exit();
    }

    /// Next window mode (F11).
    fn cycle_mode(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(shell) = &mut self.shell {
            shell.cycle_mode(event_loop);
        }
        self.update_title();
    }

    /// Control command `window`: monitor and/or window mode like F10/F11 ([`WindowShell::set_window`]). No
    /// control commands run before `resumed` (they wait in `waiting`).
    fn set_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        mode: Option<WindowMode>,
        monitor: Option<usize>,
    ) -> Result<(), String> {
        let Some(shell) = &mut self.shell else {
            return Ok(());
        };
        shell.set_window(event_loop, mode, monitor)?;
        self.update_title();
        Ok(())
    }

    fn apply_mode(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(shell) = &mut self.shell {
            shell.apply_mode(event_loop);
        }
        self.update_title();
    }

    fn update_title(&self) {
        let Some(shell) = &self.shell else {
            return;
        };
        let view = match self.view {
            ViewMode::Map => "",
            ViewMode::Browser => " - Model browser",
            ViewMode::Effects => " - Effect browser",
        };
        let dialog = self
            .dialogs
            .as_ref()
            .filter(|d| d.menu.visible())
            .map_or(String::new(), |d| format!(" - Dialog {}", d.menu.title()));
        shell.window.set_title(&format!(
            "NML Viewer - {:?} - {} - Monitor {} - {:?} - {} FPS{view}{dialog}",
            shell.gpu.backend(),
            shell.gpu.adapter_name(),
            shell.monitor_index + 1,
            shell.mode,
            self.frames.fps()
        ));
    }

    /// Completion of a frame (map and browser): with a hidden window or a missing surface draw only
    /// occasionally, count the frame ([`WindowShell::finish_frame`]).
    fn finish_frame(&mut self, outcome: FrameOutcome) {
        if let Some(shell) = &mut self.shell
            && shell.finish_frame(outcome, &mut self.frames)
        {
            self.update_title();
        }
    }

    /// Minimized or size 0: then nothing is drawn.
    fn minimized(&self) -> bool {
        self.shell.as_ref().is_some_and(WindowShell::minimized)
    }

    /// Inner window size in pixels (mouse coordinates, [`WindowShell::window_size`]); 0 without a window.
    fn window_size(&self) -> (u32, u32) {
        self.shell.as_ref().map_or((0, 0), WindowShell::window_size)
    }

    /// Map view of the match.
    fn map(&self) -> Option<&MapView> {
        self.session.as_ref().map(|s| &s.map)
    }

    fn map_mut(&mut self) -> Option<&mut MapView> {
        self.session.as_mut().map(|s| &mut s.map)
    }

    /// State of the viewer into the match, before every call of the session: pointer, mouse buttons and held
    /// keys of both sources, window and surface size, virtual clock, switches (aspect ratio, fog, animation)
    /// and camera control. The viewer manages these values itself (browser, dialog layer, control channel).
    fn sync_session(&mut self) {
        let size = self.shell.as_ref().map_or((0, 0), |s| s.gpu.size());
        let window = self.window_size();
        let Some(session) = &mut self.session else {
            return;
        };
        session.pointer.cursor = self.cursor;
        session.pointer.keys = self.keys.effective();
        session.pointer.left = self.left_button;
        session.pointer.right = self.right_button;
        session.size = size;
        session.screen = window;
        session.hud.resize(screen_of(size));
        session.hud_enabled = self.config.hud;
        session.clock.now_ms = self.clock.now_ms();
        // Program clock of the target point of the command bar: in the viewer the virtual clock.
        session.clock.program_ms = self.clock.now_ms();
        session.view.aspect = self.config.aspect;
        session.view.fog = self.config.fog;
        session.view.animate = self.config.animate;
        session.controls = self.config.controls.clone();
        // The viewer keeps its camera keys (Home/End tilt, Insert/Delete rotate): `session.camera_keys` stays
        // `CameraKeys::Viewer`, `held_vk` empty.
    }

    /// Mouse event on the map to the match, with the modes of the viewer ([`ViewerModes`]).
    fn session_input(&mut self, event: &InputEvent) {
        self.sync_session();
        let window = self.window_size();
        let now = self.clock.now_ms();
        let (Some(session), Some(vfs)) = (&mut self.session, &self.vfs) else {
            return;
        };
        let mut modes = ViewerModes {
            economy: &mut self.economy,
            unit_bar: &mut self.unit_bar,
            attack_mode: &mut self.attack_mode,
            target_notice: &mut self.target_notice,
            place_press: &mut self.place_press,
            hud: self.config.hud,
        };
        session.input_with(event, window, now, vfs, self.audio.as_mut(), &mut modes);
    }

    /// Sound at level start (`GameSession::start_level_audio`, shared with `opennml-game`): voices back, note the
    /// nation music of the local player, then without a level script the music actions of the start blocks
    /// (campaign) or the nation music (multiplayer); with a level script the scripts set the music.
    fn start_level_audio(&mut self) {
        self.recent_sounds.clear();
        if let (Some(session), Some(audio), Some(vfs)) =
            (&mut self.session, &mut self.audio, &self.vfs)
        {
            session.start_level_audio(audio, vfs);
        } else if let Some(session) = &mut self.session {
            session.voices.reset();
        }
    }

    /// Sound of a logic step: listener (map: camera target of the game camera; browser: origin), rain only on
    /// the map, combat points of the match (`GameSession::update_level_audio`, shared with `opennml-game`), then
    /// mix `dt_ms`.
    fn advance_audio(&mut self, dt_ms: f32) {
        let (Some(audio), Some(vfs)) = (&mut self.audio, &self.vfs) else {
            return;
        };
        match (self.view, self.session.as_ref()) {
            (ViewMode::Map, Some(session)) => {
                let map = &session.map;
                let target = map.camera.target;
                let height = map.scene.terrain.ground_height(target.x, target.z);
                audio.sound.set_listener(target, height, map.camera.eye);
                session.update_level_audio(audio, vfs);
            }
            (ViewMode::Browser, _) => {
                if let Some(browser) = &self.browser {
                    audio
                        .sound
                        .set_listener(Vec3::ZERO, 0.0, browser.orbit.eye());
                }
            }
            _ => {
                if let Some(effects) = &self.effect_browser {
                    audio
                        .sound
                        .set_listener(Vec3::ZERO, 0.0, effects.orbit.eye());
                }
            }
        }
        if self.view != ViewMode::Map || self.session.is_none() {
            // No rain outside the map; combat music: the last reported value of the active player, without a
            // simulation 0 (during the pause it stays, C-161).
            audio.set_rain(vfs, false);
            let points = self.session.as_ref().and_then(|s| s.sim()).map_or(
                0,
                opennml_client::game::sim_host::SimHost::active_attack_points,
            );
            audio.set_attack_points(points);
        }
        audio.advance(vfs, dt_ms);
    }

    /// Overlay line of the sound.
    fn audio_overlay(&self) -> String {
        match &self.audio {
            Some(a) => audio_line(
                a.music.playing().or(a.menu.playing(&a.sound)),
                a.music.status.name(),
                a.attack_override(),
                a.sound.voices().count(),
                a.mute(),
                a.capture_status().is_some(),
            ),
            None => "Sound: off".into(),
        }
    }

    /// One frame in real-time operation: logic with the frame time of the clock, then draw. With a paused
    /// clock only draw; mouse movements stay for the next step.
    fn redraw(&mut self) {
        let dt_ms = self.clock.tick(Instant::now());
        if self.clock.mode() == ClockMode::Realtime {
            self.advance(dt_ms);
        }
        self.present();
    }

    /// Logic of one frame with `dt_ms` milliseconds: advance the clock, advance the view, then the sound.
    fn advance(&mut self, dt_ms: f32) {
        self.clock.advance(dt_ms);
        self.advance_view(dt_ms);
        self.advance_menu();
        self.advance_audio(dt_ms);
    }

    /// One frame of the dialog layer (`--dialog`) at the viewer clock, at the size of the surface; its sounds
    /// go to the sound system of the viewer. Display mode and hardware pointer have no effect in the viewer
    /// (its window follows `opennml-viewer.toml`), `QUIT` ends the viewer.
    fn advance_menu(&mut self) {
        let (Some(dialogs), Some(vfs), Some(audio), Some(shell)) =
            (&mut self.dialogs, &self.vfs, &mut self.audio, &self.shell)
        else {
            return;
        };
        let screen = screen_of(shell.gpu.size());
        // `as` saturates; the clock is >= 0.
        let now = self.clock.now_ms() as u64;
        let req = dialogs.menu.advance(vfs, screen, now, audio);
        if let Some(mode) = req.display_mode {
            log::info!(
                "Display mode {} x {}: no effect in the viewer",
                mode.width,
                mode.height
            );
        }
        if let Some(on) = req.hardware_cursor {
            log::info!("HardwareCursor {on}: no effect in the viewer (system pointer stays)");
        }
        if let Some(gamma) = req.gamma {
            // The gamma ramp exists only in `opennml-game`; the viewer stays neutral.
            log::info!("Gamma {gamma}: no effect in the viewer");
        }
        if req.quit {
            self.menu_quit = true;
        }
        // A match from the menu exists only in `opennml-game`.
        for effect in &req.game {
            opennml_client::menu_host::log_game_effect(effect);
        }
    }

    /// Camera control or free camera with the accumulated mouse movements, in the browser rotation and the
    /// caught-up animation switch; on the map weather, effects and the surf sounds of the game ticks.
    fn advance_view(&mut self, dt_ms: f32) {
        let clock_ms = self.clock.now_f32();
        if self.view == ViewMode::Browser {
            let drag = std::mem::take(&mut self.drag);
            if let (Some(browser), Some(vfs)) = (&mut self.browser, &self.vfs) {
                browser.orbit.drag(drag);
                browser.apply_pending(clock_ms);
                browser.advance_effects(vfs, dt_ms, clock_ms);
            }
            return;
        }
        if self.view == ViewMode::Effects {
            let drag = std::mem::take(&mut self.drag);
            if let (Some(effects), Some(vfs)) = (&mut self.effect_browser, &self.vfs) {
                effects.orbit.drag(drag);
                effects.advance(vfs, dt_ms);
            }
            return;
        }
        // The map: camera, simulation, presentation, sounds and voices in the match (`GameSession::advance_by`).
        self.sync_session();
        let look = std::mem::take(&mut self.look);
        let (Some(session), Some(shell), Some(vfs)) = (&mut self.session, &self.shell, &self.vfs)
        else {
            return;
        };
        if self.dialogs.is_some() {
            // Ctrl+Page Up/Down and Ctrl+D belong to the dialog layer, not to zoom and camera.
            dialogs::strip_dialog_keys(&mut session.pointer.keys);
        }
        session.pointer.look = look;
        let gpu = SessionGpu::of(&shell.gpu);
        // After the end of the game (won/lost of the active slot) the viewer sends no more steps.
        let running = crate::script_ui::viewer_steps(session.sim());
        for event in session.advance_by(&gpu, vfs, dt_ms, running, self.audio.as_mut()) {
            let SessionEvent::Sound(sound) = event;
            record_recent_sound(&mut self.recent_sounds, &sound);
        }
        // Fading of the UI with the frame time, also after the end of the game.
        session.tick_presentation(dt_ms);
        // Quicksave and profile variables of the scripts are written only by `opennml-game`; the viewer logs.
        for request in session.take_host_requests() {
            log::info!("Script: {request:?} (no effect in the viewer)");
        }
        // Requests of the match to the flow (button 579 game menu): the viewer has no game menu.
        for request in session.take_ui_requests() {
            log::info!("Match: {request:?} (no effect in the viewer)");
        }
    }

    /// Draws a frame at the current clock time (map or model browser).
    fn present(&mut self) {
        match self.view {
            ViewMode::Browser => self.present_browser(),
            ViewMode::Map => self.present_map(),
            ViewMode::Effects => self.present_effects(),
        }
    }

    /// Actions of the browser model with names (`sounddatabase.dat`, `actionidlist.txt`).
    fn browser_sound_actions(&self) -> Vec<(i32, String)> {
        let (Some(audio), Some(browser)) = (&self.audio, &self.browser) else {
            return Vec::new();
        };
        let Some((guid, _)) = browser.sound_type() else {
            return Vec::new();
        };
        let db = &audio.sound.db;
        db.actions(guid as i32)
            .into_iter()
            .map(|a| (a, db.action_name(a)))
            .collect()
    }

    /// T / Shift+T: sound action forward/back; Enter: play the chosen action.
    fn browser_sound_key(&mut self, code: KeyCode) {
        let actions = self.browser_sound_actions();
        let shift =
            self.keys.contains(KeyCode::ShiftLeft) || self.keys.contains(KeyCode::ShiftRight);
        let (Some(browser), Some(audio), Some(vfs)) =
            (&mut self.browser, &mut self.audio, &self.vfs)
        else {
            return;
        };
        if actions.is_empty() {
            return;
        }
        if code == KeyCode::KeyT {
            browser.sound_choice = step_index(
                browser.sound_choice,
                actions.len(),
                if shift { -1 } else { 1 },
            );
            return;
        }
        let Some((guid, unit)) = browser.sound_type() else {
            return;
        };
        let action = actions[browser.sound_choice.min(actions.len() - 1)].0;
        audio.play_action(vfs, guid, action, unit, &mut browser.responses);
    }

    /// One frame in the model browser: pose, model and overlay.
    fn present_browser(&mut self) {
        let clock_ms = self.clock.now_f32();
        let actions = self.browser_sound_actions();
        let (Some(gpu), Some(browser), Some(vfs)) = (
            self.shell.as_mut().map(|s| &mut s.gpu),
            &mut self.browser,
            &self.vfs,
        ) else {
            return;
        };
        browser.update(&gpu.device, &gpu.queue, vfs, clock_ms);
        let params = browser.view_params(gpu.size(), self.config.aspect.into(), clock_ms);
        let mut describe = browser.describe(clock_ms);
        let guid = browser.sound_type().map(|(g, _)| g);
        describe.push(sound_line(guid, &actions, browser.sound_choice));
        let lines = browser_lines(
            self.frames.fps(),
            &format!("{:?}", gpu.backend()),
            describe,
            browser.text.line_height(),
        );
        let dialogs = &mut self.dialogs;
        let outcome = gpu.render(|device, queue, encoder, target, size| {
            browser.render_scene(device, queue, vfs, encoder, target, size, &params);
            if let Some(dialogs) = dialogs {
                dialogs.draw(device, queue, encoder, target, size);
            }
            browser.draw_overlay(device, queue, encoder, target, size, &lines);
        });
        self.hovered = None;
        self.overlay = lines;
        self.finish_frame(outcome);
    }

    /// One frame in the effect browser: ground plane, effects and overlay.
    fn present_effects(&mut self) {
        let clock_ms = self.clock.now_f32();
        let (Some(gpu), Some(effects), Some(vfs)) = (
            self.shell.as_mut().map(|s| &mut s.gpu),
            &mut self.effect_browser,
            &self.vfs,
        ) else {
            return;
        };
        let params = effects.view_params(gpu.size(), self.config.aspect.into(), clock_ms);
        let describe = effects.describe(vfs, clock_ms);
        let lines = effect_lines(
            self.frames.fps(),
            &format!("{:?}", gpu.backend()),
            describe,
            effects.text.line_height(),
        );
        let dialogs = &mut self.dialogs;
        let outcome = gpu.render(|device, queue, encoder, target, size| {
            effects.render_scene(device, queue, vfs, encoder, target, size, &params);
            if let Some(dialogs) = dialogs {
                dialogs.draw(device, queue, encoder, target, size);
            }
            effects.draw_overlay(device, queue, encoder, target, size, &lines);
        });
        self.hovered = None;
        self.overlay = lines;
        self.finish_frame(outcome);
    }

    /// One frame of the map: poses, object under the mouse, world and overlay.
    fn present_map(&mut self) {
        self.sync_session();
        let clock_ms = self.clock.now_f32();
        let audio_line_text = self.audio_overlay();
        let window_size = self.window_size();
        let (Some(WindowShell { window, gpu, .. }), Some(session), Some(vfs)) =
            (&mut self.shell, &mut self.session, &self.vfs)
        else {
            return;
        };
        let size = gpu.size();
        let session_gpu = SessionGpu::of(gpu);
        // Poses and renderer data of the frame (weather, shadows, effects) in the match.
        let params = session.prepare(&session_gpu, vfs, size);
        let map = &mut session.map;
        // Two target searches per frame as in the original: the click target (`0x0047A210`) for the overlay
        // "under the mouse", the right-click target (`0x00480E80`, also hits trees) for the mouse pointer hint
        // of the command UI.
        let hovered = self
            .cursor
            .and_then(|c| map.hovered(&params, c, window_size));
        let target = self.cursor.and_then(|c| {
            map.pick_right_click_target(&params, c, window_size, &session.selection.ids)
        });
        // Command UI: action or building site under the mouse and content of the window "Economy" (queries
        // without a state change).
        let view_proj = params.projection * params.view;
        let pointed = self
            .cursor
            .map(|c| (target, map.ground_point(&params, c, window_size)));
        let mut economy_lines = Vec::new();
        let mut preview = None;
        let mut panel = None;
        // Unit bar: the lock check of the buttons needs the simulation context (reads only).
        let mut bar_summary = opennml_client::game::unit_bar::SelectionSummary::default();
        if let Some(sim) = map.sim.as_mut() {
            let selection = &session.selection.ids;
            bar_summary = opennml_client::game::unit_bar::summarize(sim, selection);
            self.economy.follow_selection(sim, selection);
            opennml_client::game::unit_bar::follow_tent_cursor(&mut self.unit_bar, sim, selection);
            // Targeting mode: ends with every selection change, death or lock.
            opennml_client::game::unit_bar::follow_target_mode(&mut self.unit_bar, sim, selection);
            if self.unit_bar.target_mode.is_none() {
                self.target_notice = None;
            }
            let ground = pointed.and_then(|p| p.1);
            if let Some(p) = self.economy.placement {
                preview = ground.and_then(|g| sim.placement_preview(p.guid, g, p.steps, selection));
                economy_lines.push(placement_line(&sim.rules, &p, preview.as_ref()));
            } else if let Some(c) = self.unit_bar.tent_cursor {
                // Unpack cursor (button 453): build preview with GUID and rotation of the tent.
                preview = ground.and_then(|g| {
                    opennml_client::game::unit_bar::tent_preview(sim, &c, g, selection)
                });
                economy_lines.push(opennml_client::game::unit_bar::tent_cursor_line(
                    &c,
                    preview.as_ref(),
                ));
            } else if let Some(mode) = self.unit_bar.target_mode {
                // Targeting mode: button text and the last rejection.
                economy_lines.push(opennml_client::game::unit_bar::target_mode_line(
                    &mode,
                    &self.bar_texts,
                ));
                economy_lines.extend(self.target_notice.clone());
            } else if self.attack_mode {
                economy_lines.push("Attack: select target".into());
            } else if !selection.is_empty()
                && let Some(g) = ground
            {
                let action = sim.right_click_action(selection, pointed.and_then(|p| p.0), g);
                economy_lines.push(action_line(action.as_ref()));
            }
            if self.economy.open {
                panel = Some(sim.panel_with_hire(selection, &self.economy.hire));
            }
        }
        // Build preview as a tinted model (`Cursor3D_RenderWorldModel`), before drawing.
        map.set_build_preview(&gpu.device, &gpu.queue, vfs, preview.as_ref());
        // Ground markers (step 8): selection, target marker, object under the mouse (the click target, not the
        // right-click target: no ring on trees).
        let hovered_id = hovered
            .and_then(|i| map.scene.objects[i].sim_id)
            .map(opennml_sim::id::ObjectId);
        session.mark_ground(&session_gpu, vfs, hovered_id);
        // Messages of the match, in the viewer between the balance line and "under the mouse"; with `--hud` the
        // game UI draws them as in `opennml-game`, and the lines are omitted.
        let messages = if self.config.hud {
            Vec::new()
        } else {
            session.message_lines(&self.message_texts)
        };
        let hud = hud_view::hud_cmds(
            session,
            &self.message_texts,
            vfs,
            size,
            self.dialogs.as_mut(),
            self.hud_layer.as_mut(),
        );
        // Health bars and rank icons (presentation only), after the top edges of the hulls; the right axis of
        // the camera aligns the billboards. In cinemascope without hover bars.
        let bar_hovered = session.hover_shown(hovered_id);
        let map = &mut session.map;
        let tops = map.sim_top_points();
        let camera_right = params.view.inverse().x_axis.truncate().normalize_or_zero();
        let combat = map.sim.as_ref().map_or_else(Default::default, |sim| {
            opennml_client::game::combat_calc::frame_overlay(
                sim,
                &session.selection.ids,
                bar_hovered,
                &tops,
                view_proj,
                camera_right,
                size,
            )
        });
        // AI overlay (key L): frame around the objects with the `ai_marks` bit of the slot (presentation only).
        let ki_marks = self.ki_overlay.zip(map.sim.as_ref()).map(|(slot, sim)| {
            (
                slot,
                crate::ki_ui::marked_objects(&sim.state, slot).len(),
                crate::ki_ui::mark_frames(sim, slot, &tops, view_proj, camera_right, size),
            )
        });
        let info = OverlayInfo {
            fps: self.frames.fps(),
            backend: format!("{:?}", gpu.backend()),
            adapter: gpu.adapter_name().to_string(),
            level: map.scene.lvl_path.clone(),
            tiles: (map.scene.world.terrain.dim_x, map.scene.world.terrain.dim_z),
            objects: map.renderer.instance_count(),
            camera: map.free.is_none().then(|| {
                let c = &map.camera;
                (c.distance(), c.pitch_deg(), c.target.x, c.target.z)
            }),
            aspect: self.config.aspect,
            fog: self.config.fog,
            environment: environment_line(
                map.light_name(),
                map.weather.as_ref().map(|w| w.kind()),
                map.renderer.weather_count(),
            ),
            shadows: shadow_line(map.shadow_quality, &map.shadow_stats()),
            effects: effect_line(
                map.particles_quality,
                &map.effect_counts(),
                map.renderer.effect_quads(),
                map.renderer.effect_models(),
            ),
            audio: audio_line_text,
            sim: map.sim_line(),
            account: map
                .sim
                .as_ref()
                .map(opennml_client::game::sim_host::SimHost::account_line),
            messages,
            economy: economy_lines,
            combat: map
                .sim
                .as_ref()
                .and_then(|s| s.combat_line(&session.selection.ids, hovered_id)),
            hovered: hovered.map(|i| {
                format!(
                    "{} | {}",
                    describe_object(&map.scene.objects[i]),
                    map.describe_class(i)
                )
            }),
            help: self.help,
        };
        let mut lines = overlay_lines(&info, map.text.line_height());
        // Floating numbers (credits, damage, dynamite) at the game time of the message bar.
        lines.extend(map.notices.float_lines(
            params.projection * params.view,
            size,
            session.hud.bar_ms,
        ));
        // egui: selection box, debug and economy windows (the selection is shown by the ground rings).
        let mut panel_actions = Vec::new();
        let mut bar_actions: Vec<BarAction> = Vec::new();
        let mut match_actions = Vec::new();
        let mut ki_actions = Vec::new();
        let mut egui = self.egui.as_mut().zip(Some(&*window));
        if let (Some((layer, window)), Some(sim)) = (&mut egui, &map.sim) {
            let drag = session
                .select_start
                .zip(self.cursor)
                .filter(|(a, b)| (a.0 - b.0).abs() >= 2.0 && (a.1 - b.1).abs() >= 2.0)
                .map(|(a, b)| ([a.0 as f32, a.1 as f32], [b.0 as f32, b.1 as f32]));
            let inspected = session.selection.ids.first().copied().or_else(|| {
                hovered
                    .and_then(|i| map.scene.objects[i].sim_id)
                    .map(opennml_sim::id::ObjectId)
            });
            let ground = self
                .cursor
                .and_then(|c| map.ground_point(&params, c, window_size));
            let (mut grid_cells, path_tiles, mut path_line, path_rows) = path_overlay(
                sim,
                &map.scene.terrain,
                map.camera.target,
                params.projection * params.view,
                size,
                self.grid_mode,
                &session.selection.ids,
                ground,
                &mut self.path_preview,
                self.shown_path.as_ref(),
            );
            // Fog overlay (key F) above the pathfinding: tiles from the chosen stage.
            if let Some(level) = sim.fog.overlay {
                let center = opennml_client::game::path_view::camera_tile(sim, map.camera.target);
                grid_cells.extend(opennml_client::game::path_view::tile_quads(
                    &map.scene.terrain,
                    params.projection * params.view,
                    size,
                    opennml_client::game::fog_view::overlay_cells(
                        &sim.state,
                        sim.active_player,
                        level,
                        center,
                    ),
                ));
            }
            // Without a path preview: path of the single selected unit while it is walking.
            if path_line.is_empty()
                && let [unit] = session.selection.ids[..]
            {
                path_line = opennml_client::game::path_view::walk_route(
                    sim,
                    unit,
                    params.projection * params.view,
                    size,
                );
            }
            let view = DebugView {
                sim,
                inspected,
                selected: session.selection.ids.len(),
                drag,
                grid_cells,
                path_tiles,
                path_line,
                path_rows,
                flights: map.flight_count(),
            };
            let economy_view = crate::economy_ui::EconomyView {
                rules: &sim.rules,
                model: panel.as_ref(),
                placement: self.economy.placement.as_ref(),
                preview: preview.as_ref(),
                sell_armed: self.economy.sell_armed.is_some(),
                // Tile frame and anchor cross only as a viewer aid with the path grid overlay (G); the original
                // shows only the tinted model.
                footprint: preview
                    .as_ref()
                    .filter(|_| self.grid_mode.is_some())
                    .map_or_else(Vec::new, |p| {
                        crate::economy_ui::footprint_outline(&sim.rules, p, view_proj, size)
                    }),
                anchor: preview
                    .as_ref()
                    .filter(|_| self.grid_mode.is_some())
                    .and_then(|p| crate::economy_ui::anchor_point(&sim.rules, p, view_proj, size)),
                counter_upgrades: crate::economy_ui::counter_upgrade_rows(sim, &self.bar_texts),
                hire_back: opennml_client::game::unit_bar::back_label(&self.bar_texts),
            };
            let bar_view = crate::unit_bar_ui::BarView {
                header: opennml_client::game::unit_bar::header_for(
                    &self.unit_bar,
                    &self.bar_texts,
                    &bar_summary,
                ),
                buttons: opennml_client::game::unit_bar::buttons(
                    &self.unit_bar,
                    &self.bar_texts,
                    &bar_summary,
                ),
            };
            // Window "AI": reads only the AI state (`ki_state`).
            let ki_state = self
                .ki_window
                .open
                .then(|| sim.control_ki_state(None, true).ok())
                .flatten();
            // Window "Script": reads only the script state (`script_state`). A chosen block that no longer
            // exists (restart, end of dialog) is deselected.
            let script_state = if self.script_window.open {
                let (block, names, log) = self.script_window.request();
                match sim.control_script_state(block, names, log) {
                    Ok(state) => Some(state),
                    Err(_) => {
                        self.script_window.selected = None;
                        let (_, names, log) = self.script_window.request();
                        sim.control_script_state(None, names, log).ok()
                    }
                }
            } else {
                None
            };
            // Presentation of the scripts (sections "Presentation" and "Events"): the last 50 steps.
            let script_events = script_state.as_ref().and_then(|_| {
                crate::script_ui::script_events(sim, &session.presentation.camera, None).ok()
            });
            // Cinemascope bars and game-end display in the map frame; with `--hud` the original surface draws the
            // bars (`hud_view::hud_cmds`).
            let cinemascope =
                crate::script_ui::egui_bars(session.presentation.cinemascope, self.config.hud);
            let game_end = crate::script_ui::game_end(sim);
            let texts = &self.message_texts;
            let open = &mut self.debug_open;
            let economy_open = &mut self.economy.open;
            let actions = &mut panel_actions;
            let bar_actions = &mut bar_actions;
            let rank_icons = &mut self.rank_icons;
            let (match_open, match_draft, config, missions) = (
                &mut self.match_open,
                &mut self.match_draft,
                &self.config,
                &self.missions,
            );
            let ki_window = &mut self.ki_window;
            let script_window = &mut self.script_window;
            let (match_actions, ki_actions) = (&mut match_actions, &mut ki_actions);
            layer.run(
                window,
                &gpu.device,
                &gpu.queue,
                f64::from(clock_ms),
                |ctx| {
                    crate::debug_ui::build(ctx, open, match_open, &mut script_window.open, &view);
                    if *match_open {
                        let draft = match_draft.get_or_insert_with(|| {
                            crate::ki_ui::MatchDraft::new(config, &sim.setup)
                        });
                        crate::ki_ui::build_match(ctx, match_open, draft, missions, match_actions);
                    }
                    if let Some(state) = &ki_state {
                        crate::ki_ui::build_ki(ctx, ki_window, state, ki_actions);
                    }
                    crate::script_ui::paint_presentation(ctx, cinemascope, game_end);
                    if let Some(state) = &script_state {
                        crate::script_ui::build_script(
                            ctx,
                            script_window,
                            state,
                            script_events.as_ref(),
                            &|id| texts.get(id).map(str::to_owned),
                        );
                    }
                    crate::economy_ui::build(ctx, economy_open, &economy_view, actions);
                    crate::unit_bar_ui::build(ctx, &bar_view, bar_actions);
                    crate::combat_overlay::paint(ctx, &combat, rank_icons, vfs);
                    if let Some((slot, marked, frames)) = &ki_marks {
                        crate::ki_ui::paint_marks(ctx, *slot, *marked, frames);
                    }
                },
            );
            if !self.match_open {
                self.match_draft = None;
            }
        }
        let dialogs = &mut self.dialogs;
        let hud_layer = &mut self.hud_layer;
        let outcome = gpu.render(|device, queue, encoder, target, size| {
            map.render_world(device, queue, encoder, target, size, &params);
            // Game UI (`--hud`) with scene fade and info windows between world and dialogs, as in `opennml-game`
            // before the menu (re/campaign-ui D6.4).
            hud_view::draw_hud(
                &hud,
                dialogs.as_mut(),
                hud_layer.as_mut(),
                device,
                queue,
                encoder,
                target,
                size,
            );
            if let Some(dialogs) = dialogs {
                dialogs.draw(device, queue, encoder, target, size);
            }
            map.draw_overlay(device, queue, encoder, target, size, &lines);
            if let Some((layer, _)) = &mut egui {
                layer.paint(device, queue, encoder, target, size);
            }
        });
        // Clicks in the window "Economy" (in the frame of the virtual clock, also via the control channel); the
        // closed window "Economy" ends the placement mode and the headhunter menu.
        if !self.economy.open {
            self.economy.placement = None;
            self.economy.sell_armed = None;
            self.economy.hire = opennml_client::game::economy::HireMenu::default();
        }
        if let Some(sim) = map.sim.as_mut() {
            for action in panel_actions {
                // Building placement and unpack cursor are mutually exclusive.
                if matches!(action, opennml_client::game::economy::PanelAction::Build(_)) {
                    self.unit_bar.tent_cursor = None;
                    opennml_client::game::unit_bar::cancel_target_mode(&mut self.unit_bar);
                }
                self.economy.apply(sim, &session.selection.ids, action);
            }
            for action in bar_actions {
                opennml_client::game::unit_bar::apply(
                    &mut self.unit_bar,
                    sim,
                    &session.selection.ids,
                    action,
                );
                if action == BarAction::UnpackTent && self.unit_bar.tent_cursor.is_some() {
                    self.economy.placement = None;
                }
                // Targeting mode ends building placement and attack mode; the unpack cursor is already ended by
                // `apply`.
                if matches!(action, BarAction::Target(_)) && self.unit_bar.target_mode.is_some() {
                    self.economy.placement = None;
                    self.attack_mode = false;
                    self.target_notice = None;
                }
            }
        }
        self.hovered = hovered;
        self.overlay = lines;
        self.finish_frame(outcome);
        self.apply_ki_ui(match_actions, ki_actions);
    }

    /// Buttons of the windows "Match" and "AI" of a frame: "Restart" via the same path as `sim_setup`
    /// ([`Self::setup_match`]); pause, continue and single step as `ki_command` (commands of the simulation at
    /// the next tick, in the command log).
    fn apply_ki_ui(
        &mut self,
        match_actions: Vec<crate::ki_ui::MatchAction>,
        ki_actions: Vec<crate::ki_ui::KiUiAction>,
    ) {
        use crate::ki_ui::{KiUiAction, MatchAction, MatchDraft};
        for action in match_actions {
            let Some(draft) = &self.match_draft else {
                continue;
            };
            let mission = draft.mission;
            let result = match action {
                MatchAction::Restart => self
                    .setup_match(draft.args())
                    .map(|_| "new match started".to_string()),
                MatchAction::Reload => Ok("read from the running match".to_string()),
                MatchAction::StartMission(index) => self
                    .start_mission(index, draft.campaign_difficulty)
                    .map(|name| format!("Mission {name} started")),
            };
            // After an error the draft stays (to correct it), otherwise anew from the match.
            if result.is_ok()
                && let Some(sim) = self.session.as_ref().and_then(|s| s.sim())
            {
                // The mission choice stays after "Start".
                self.match_draft = Some(MatchDraft::renewed(mission, &self.config, &sim.setup));
            }
            if let Some(draft) = &mut self.match_draft {
                draft.status = Some(result.unwrap_or_else(|e| e));
            }
        }
        for action in ki_actions {
            let (kind, slot, paused, passes) = match action {
                KiUiAction::Pause(paused) => {
                    (opennml_control::KiAction::Pause, None, Some(paused), None)
                }
                KiUiAction::Step { slot, passes } => (
                    opennml_control::KiAction::Step,
                    Some(slot),
                    None,
                    Some(passes),
                ),
            };
            let request = opennml_client::game::sim_control::KiCommandRequest {
                action: kind,
                slot,
                module: None,
                args: None,
                sync: None,
                paused,
                passes,
            };
            let result = self
                .sim_mut()
                .and_then(|sim| sim.control_ki_command(request));
            self.ki_window.status = Some(match result {
                Ok(()) => match action {
                    KiUiAction::Pause(true) => "Pause takes effect at the next tick".into(),
                    KiUiAction::Pause(false) => "Continue takes effect at the next tick".into(),
                    KiUiAction::Step { slot, passes } => {
                        format!("Step slot {slot} ({passes} passes) at the next tick")
                    }
                },
                Err(e) => e,
            });
        }
    }

    /// State snapshot (`docs/viewer-control.md`).
    fn snapshot(&self, event_loop: &ActiveEventLoop) -> StateSnapshot {
        let inner = self.window_size();
        let surface = self.shell.as_ref().map_or((0, 0), |s| s.gpu.size());
        let map = self.map();
        let idle_voices = Voices::new(self.config.unit_response);
        let voices = self.session.as_ref().map_or(&idle_voices, |s| &s.voices);
        let selection: &[opennml_sim::id::ObjectId] =
            self.session.as_ref().map_or(&[], |s| &s.selection.ids);
        StateSnapshot {
            view: match self.view {
                ViewMode::Map => ViewName::Map,
                ViewMode::Browser => ViewName::Browser,
                ViewMode::Effects => ViewName::Effects,
            },
            window: WindowState {
                mode: window_mode_name(
                    self.shell
                        .as_ref()
                        .map_or(self.config.window_mode, |s| s.mode),
                ),
                monitor: self.shell.as_ref().map_or(0, |s| s.monitor_index),
                monitor_count: event_loop.available_monitors().count(),
                inner_size: [inner.0, inner.1],
                surface_size: [surface.0, surface.1],
                minimized: self.minimized(),
                occluded: self.shell.as_ref().is_some_and(WindowShell::occluded),
            },
            clock: ClockState {
                mode: clock_mode_name(self.clock.mode()),
                now_ms: self.clock.now_ms(),
            },
            frames: self.frames.stats(),
            camera: map.map(|m| camera_state(&m.camera)),
            free_camera: map.and_then(|m| m.free.as_ref()).map(free_camera_state),
            hovered: map.zip(self.hovered).and_then(|(m, i)| {
                let object = m.scene.objects.get(i)?;
                let (base, kind) = m.object_class(i);
                Some(object_state(object, base, kind))
            }),
            toggles: Toggles {
                fog: self.config.fog,
                aspect: aspect_name(self.config.aspect),
                animate: self.config.animate,
                help: self.help,
            },
            browser: self.browser.as_ref().map(|b| {
                let mut state = b.state(self.clock.now_f32());
                state.sound_actions = self.browser_sound_actions().iter().map(|a| a.0).collect();
                state.sound_action = state.sound_actions.get(b.sound_choice).copied();
                state
            }),
            environment: map.map(|m| EnvironmentState {
                light: m.light_name().to_string(),
                weather: weather_name(m.weather.as_ref().map(|w| w.kind())),
                weather_active: m.weather.as_ref().map_or(0, |w| w.active_count()),
                weather_moved: m.weather_moved,
                weather_drawn: m.renderer.weather_count(),
                shadow_quality: self.config.shadow_quality,
                particles_quality: self.config.particles_quality,
                terrain_quality: self.config.terrain_quality,
            }),
            shadows: map.map(|m| shadow_state(m.shadow_quality, &m.shadow_stats())),
            effects: map.map(|m| {
                effect_state(
                    m.particles_quality,
                    &m.effect_counts(),
                    m.renderer.effect_quads(),
                    m.renderer.effect_models(),
                )
            }),
            effect_browser: self
                .effect_browser
                .as_ref()
                .map(|e| e.state(self.clock.now_f32())),
            audio: self
                .audio
                .as_ref()
                .map(|a| audio_state(a, voices, &self.recent_sounds)),
            sim: map.and_then(|m| Some((m, m.sim.as_ref()?))).map(|(m, s)| {
                let mut section = section_with_flights(s, m, selection, self.unit_bar.formation());
                // Message texts from `texte.deu` (the simulation knows only the numbers).
                for notice in &mut section.notices {
                    notice.text = m.notices.message_text(notice.id).map(String::from);
                }
                section.grid_layer = self.grid_mode.map(|m| m as u8);
                section.path = self
                    .path_preview
                    .as_ref()
                    .map(|(_, v)| v)
                    .or(self.shown_path.as_ref())
                    .map(PathView::summary);
                // As in the next frame: expired or object destroyed -> none.
                section.target_marker = opennml_client::game::markers::target_marker_state(
                    self.session.as_ref().and_then(|s| s.target_highlight),
                    &s.state,
                    self.clock.now_f32(),
                );
                section.ground_markers = self.session.as_ref().map_or_else(Vec::new, |s| {
                    s.ground_markers.iter().map(|id| id.0).collect()
                });
                section
            }),
            // State of the world display (read only).
            world_display: map.and_then(|m| m.world_display()),
            cursor: CursorState {
                position: self.cursor.map(|(x, y)| [x, y]),
                left_button: self.left_button,
                right_button: self.right_button,
                keys_real: self.keys.names(InputSource::User),
                keys_control: self.keys.names(InputSource::Control),
            },
            overlay: self.overlay.iter().map(|l| l.text.clone()).collect(),
        }
    }

    /// Renders the current frame (map or model browser) anew at the clock time, optionally with the overlay of
    /// the last frame. Size of the surface (minimized: the last size != 0).
    fn capture(&mut self, overlay: bool) -> anyhow::Result<RgbaImage> {
        self.sync_session();
        let Some(gpu) = self.shell.as_ref().map(|s| &s.gpu) else {
            bail!("graphics not ready yet")
        };
        let size = gpu.size();
        let aspect = self.config.aspect.into();
        let clock_ms = self.clock.now_f32();
        let window_size = self.window_size();
        let cursor = self.cursor;
        let lines = &self.overlay;
        let egui = self.egui.as_mut();
        // The dialog layer lies over the view in every screenshot, also without overlay; likewise the game UI
        // (`--hud`) on the map.
        let mut dialogs = self.dialogs.as_mut();
        let mut hud_layer = self.hud_layer.as_mut();
        let texts = &self.message_texts;
        // The pipelines are built for the surface format, the capture has to use the same format (Bgra8 is
        // swapped to RGBA when read back).
        match (self.view, &mut self.session, &mut self.browser, &self.vfs) {
            (ViewMode::Browser, _, Some(browser), Some(vfs)) => {
                browser.update(&gpu.device, &gpu.queue, vfs, clock_ms);
                let params = browser.view_params(size, aspect, clock_ms);
                opennml_render::render_to_image(
                    &gpu.device,
                    &gpu.queue,
                    size,
                    gpu.surface_format(),
                    |encoder, view| {
                        browser.render_scene(
                            &gpu.device,
                            &gpu.queue,
                            vfs,
                            encoder,
                            view,
                            size,
                            &params,
                        );
                        if let Some(dialogs) = dialogs {
                            dialogs.draw(&gpu.device, &gpu.queue, encoder, view, size);
                        }
                        if overlay {
                            browser.draw_overlay(
                                &gpu.device,
                                &gpu.queue,
                                encoder,
                                view,
                                size,
                                lines,
                            );
                        }
                    },
                )
            }
            (ViewMode::Effects, _, _, Some(vfs)) => {
                let Some(effects) = &mut self.effect_browser else {
                    bail!("effect browser not created")
                };
                let params = effects.view_params(size, aspect, clock_ms);
                opennml_render::render_to_image(
                    &gpu.device,
                    &gpu.queue,
                    size,
                    gpu.surface_format(),
                    |encoder, view| {
                        effects.render_scene(
                            &gpu.device,
                            &gpu.queue,
                            vfs,
                            encoder,
                            view,
                            size,
                            &params,
                        );
                        if let Some(dialogs) = dialogs {
                            dialogs.draw(&gpu.device, &gpu.queue, encoder, view, size);
                        }
                        if overlay {
                            effects.draw_overlay(
                                &gpu.device,
                                &gpu.queue,
                                encoder,
                                view,
                                size,
                                lines,
                            );
                        }
                    },
                )
            }
            (ViewMode::Map, Some(session), _, Some(vfs)) => {
                let session_gpu = SessionGpu::of(gpu);
                let params = session.prepare(&session_gpu, vfs, size);
                // Ground markers as in the frame (object under the mouse via the same ray test).
                let hovered = cursor.and_then(|c| session.map.pick_target(&params, c, window_size));
                session.mark_ground(&session_gpu, vfs, hovered);
                let hud = hud_view::hud_cmds(
                    session,
                    texts,
                    vfs,
                    size,
                    dialogs.as_deref_mut(),
                    hud_layer.as_deref_mut(),
                );
                let map = &mut session.map;
                opennml_render::render_to_image(
                    &gpu.device,
                    &gpu.queue,
                    size,
                    gpu.surface_format(),
                    |encoder, view| {
                        map.render_world(&gpu.device, &gpu.queue, encoder, view, size, &params);
                        hud_view::draw_hud(
                            &hud,
                            dialogs.as_deref_mut(),
                            hud_layer,
                            &gpu.device,
                            &gpu.queue,
                            encoder,
                            view,
                            size,
                        );
                        if let Some(dialogs) = dialogs {
                            dialogs.draw(&gpu.device, &gpu.queue, encoder, view, size);
                        }
                        if overlay {
                            map.draw_overlay(&gpu.device, &gpu.queue, encoder, view, size, lines);
                            // Debug window and selection box of the last frame.
                            if let Some(layer) = egui {
                                layer.paint(&gpu.device, &gpu.queue, encoder, view, size);
                            }
                        }
                    },
                )
            }
            _ => bail!("no view loaded"),
        }
    }

    /// F12: current frame (map or model browser) without overlay as PNG in the working directory.
    fn screenshot(&mut self) {
        let name = format!(
            "opennml-viewer-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        );
        match self
            .capture(false)
            .and_then(|img| img.save(&name).map_err(Into::into))
        {
            Ok(()) => log::info!("Screenshot {name} saved"),
            Err(e) => log::error!("Screenshot: {e:#}"),
        }
    }

    /// Switches between map and model browser; the browser is created the first time (with the start model
    /// from `--model`). The map stays unchanged.
    fn toggle_browser(&mut self) -> anyhow::Result<()> {
        // Do not carry over mouse movements of the other view.
        self.drag = Vec2::ZERO;
        self.look = Vec2::ZERO;
        if self.view == ViewMode::Browser {
            self.view = ViewMode::Map;
            self.update_title();
            return Ok(());
        }
        if self.browser.is_none() {
            let (Some(gpu), Some(vfs)) = (self.shell.as_ref().map(|s| &s.gpu), &self.vfs) else {
                return Ok(());
            };
            self.browser = Some(BrowserView::new(
                &gpu.device,
                &gpu.queue,
                gpu.surface_format(),
                vfs,
                self.start_model.as_deref(),
                self.clock.now_f32(),
                self.config.shadow_quality,
                &self.config.language_tag,
                self.config.width,
            )?);
        }
        self.view = ViewMode::Browser;
        self.update_title();
        Ok(())
    }

    /// N: effect browser on (created the first time) or back to the map.
    fn toggle_effects(&mut self) -> anyhow::Result<()> {
        self.drag = Vec2::ZERO;
        self.look = Vec2::ZERO;
        if self.view == ViewMode::Effects {
            self.view = ViewMode::Map;
            self.update_title();
            return Ok(());
        }
        if self.effect_browser.is_none() {
            let (Some(gpu), Some(vfs)) = (self.shell.as_ref().map(|s| &s.gpu), &self.vfs) else {
                return Ok(());
            };
            self.effect_browser = Some(EffectBrowser::new(
                &gpu.device,
                &gpu.queue,
                gpu.surface_format(),
                vfs,
                self.clock.now_f32(),
                &self.config.language_tag,
                self.config.width,
            )?);
        }
        self.view = ViewMode::Effects;
        self.update_title();
        Ok(())
    }

    /// Arrow keys, space bar and -/+ ([`step_sign`]) in the effect browser.
    fn effect_key(&mut self, code: KeyCode) {
        let shift =
            self.keys.contains(KeyCode::ShiftLeft) || self.keys.contains(KeyCode::ShiftRight);
        let clock_ms = self.clock.now_f32();
        let (Some(effects), Some(vfs)) = (&mut self.effect_browser, &self.vfs) else {
            return;
        };
        let step = if shift { 10 } else { 1 };
        match code {
            KeyCode::ArrowLeft => effects.step(vfs, -step, clock_ms),
            KeyCode::ArrowRight => effects.step(vfs, step, clock_ms),
            KeyCode::Space => effects.restart(vfs, clock_ms),
            _ => {
                if let Some(sign) = step_sign(code) {
                    effects.step_radius(vfs, sign, clock_ms);
                }
            }
        }
    }

    /// Arrow keys, space bar, V and -/+ ([`step_sign`]) in the model browser.
    fn browser_key(&mut self, code: KeyCode) {
        let shift =
            self.keys.contains(KeyCode::ShiftLeft) || self.keys.contains(KeyCode::ShiftRight);
        let clock_ms = self.clock.now_f32();
        let (Some(browser), Some(vfs)) = (&mut self.browser, &self.vfs) else {
            return;
        };
        let step = if shift { 10 } else { 1 };
        match code {
            KeyCode::ArrowLeft => browser.step_model(vfs, -step, clock_ms),
            KeyCode::ArrowRight => browser.step_model(vfs, step, clock_ms),
            KeyCode::ArrowUp => browser.step_animation(-1, clock_ms),
            KeyCode::ArrowDown => browser.step_animation(1, clock_ms),
            KeyCode::Space => browser.toggle_pause(clock_ms),
            KeyCode::KeyV => browser.step_preview(vfs),
            _ => {
                if let Some(sign) = step_sign(code) {
                    browser.step_damage(sign);
                }
            }
        }
    }

    /// Ctrl+Page Up/Down and Ctrl+D while a dialog layer exists (`--dialog`): switches the dialog or hides the
    /// layer. Returns `true` if the key belongs to the dialog browser.
    fn dialog_key(&mut self, code: KeyCode) -> bool {
        let ctrl =
            self.keys.contains(KeyCode::ControlLeft) || self.keys.contains(KeyCode::ControlRight);
        let (Some(dialogs), Some(vfs), Some(audio)) = (
            self.dialogs.as_mut().filter(|_| ctrl),
            &self.vfs,
            &self.audio,
        ) else {
            return false;
        };
        let stepped = match code {
            dialogs::KEY_NEXT => dialogs.menu.step(1, vfs, audio),
            dialogs::KEY_PREVIOUS => dialogs.menu.step(-1, vfs, audio),
            dialogs::KEY_TOGGLE => {
                dialogs.menu.toggle();
                Ok(())
            }
            _ => return false,
        };
        if let Err(e) = stepped {
            log::error!("Dialog browser: {e:#}");
        }
        self.update_title();
        true
    }

    fn key_pressed(&mut self, event_loop: &ActiveEventLoop, code: KeyCode) {
        if self.dialog_key(code) {
            return;
        }
        match code {
            // In placement mode Esc only ends the mode.
            KeyCode::Escape if self.view == ViewMode::Map && self.economy.placement.is_some() => {
                self.economy.cancel_placement();
            }
            // Likewise the unpack cursor (button 453).
            KeyCode::Escape
                if self.view == ViewMode::Map && self.unit_bar.tent_cursor.is_some() =>
            {
                self.unit_bar.tent_cursor = None;
            }
            // Likewise the targeting mode of a target button (before quitting the viewer).
            KeyCode::Escape
                if self.view == ViewMode::Map && self.unit_bar.target_mode.is_some() =>
            {
                opennml_client::game::unit_bar::cancel_target_mode(&mut self.unit_bar);
            }
            // In attack mode Esc only ends the mode.
            // Likewise the headhunter menu with a chosen kind: back to level 1.
            KeyCode::Escape if self.view == ViewMode::Map && self.economy.hire.art.is_some() => {
                self.economy.cancel_hire();
            }
            KeyCode::Escape if self.view == ViewMode::Map && self.attack_mode => {
                self.attack_mode = false;
            }
            KeyCode::Escape => event_loop.exit(),
            KeyCode::F11 => self.cycle_mode(event_loop),
            KeyCode::F10 => {
                if let Some(shell) = &mut self.shell {
                    shell.cycle_monitor(event_loop);
                }
                self.update_title();
            }
            KeyCode::F1 => self.help = !self.help,
            KeyCode::F2 => {
                if let Some(map) = self.session.as_mut().map(|s| &mut s.map) {
                    map.free = match map.free {
                        Some(_) => None,
                        None => Some(FreeCamera::from_game(&map.camera)),
                    };
                }
            }
            KeyCode::F3 => self.config.fog = !self.config.fog,
            KeyCode::F4 => self.config.aspect = self.config.aspect.next(),
            KeyCode::F5 => self.config.animate = !self.config.animate,
            KeyCode::F6 if self.view == ViewMode::Map => {
                let shift = self.keys.contains(KeyCode::ShiftLeft)
                    || self.keys.contains(KeyCode::ShiftRight);
                if let (Some(map), Some(gpu)) = (
                    self.session.as_mut().map(|s| &mut s.map),
                    self.shell.as_ref().map(|s| &s.gpu),
                ) {
                    map.step_light(&gpu.queue, if shift { -1 } else { 1 });
                }
            }
            KeyCode::F7 if self.view == ViewMode::Map => {
                if let Some(map) = self.session.as_mut().map(|s| &mut s.map) {
                    map.cycle_weather();
                }
            }
            KeyCode::F9 => {
                let shift = self.keys.contains(KeyCode::ShiftLeft)
                    || self.keys.contains(KeyCode::ShiftRight);
                if let Some(audio) = &mut self.audio {
                    if shift {
                        audio.set_attack_override(!audio.attack_override());
                    } else {
                        audio.set_mute(!audio.mute());
                    }
                }
            }
            KeyCode::F8 => match self.view {
                ViewMode::Effects => {}
                ViewMode::Map => {
                    let next = (self.config.shadow_quality + 1) % (MAX_SHADOW_QUALITY + 1);
                    self.set_quality(Some(next), None, None);
                }
                ViewMode::Browser => {
                    if let Some(browser) = &mut self.browser {
                        browser.step_shadow();
                    }
                }
            },
            KeyCode::KeyB => {
                if let Err(e) = self.toggle_browser() {
                    log::error!("Model browser: {e:#}");
                }
            }
            KeyCode::KeyN => {
                if let Err(e) = self.toggle_effects() {
                    log::error!("Effect browser: {e:#}");
                }
            }
            KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::Space
                if self.view == ViewMode::Effects =>
            {
                self.effect_key(code)
            }
            KeyCode::ArrowLeft
            | KeyCode::ArrowRight
            | KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::Space
            | KeyCode::KeyV
                if self.view == ViewMode::Browser =>
            {
                self.browser_key(code)
            }
            KeyCode::KeyT | KeyCode::Enter | KeyCode::NumpadEnter
                if self.view == ViewMode::Browser =>
            {
                self.browser_sound_key(code)
            }
            KeyCode::KeyT if self.view == ViewMode::Map => self.economy.toggle(),
            KeyCode::KeyR if self.view == ViewMode::Map => {
                if let Some(sim) = self.session.as_ref().and_then(|s| s.sim()) {
                    self.economy.rotate(&sim.rules);
                }
            }
            KeyCode::F12 => self.screenshot(),
            KeyCode::KeyI => self.debug_open = !self.debug_open,
            KeyCode::KeyK if self.view == ViewMode::Map => {
                self.ki_window.open = !self.ki_window.open;
            }
            KeyCode::KeyJ if self.view == ViewMode::Map => {
                self.script_window.open = !self.script_window.open;
            }
            // Key L: AI overlay off -> AI slots with engine -> off.
            KeyCode::KeyL if self.view == ViewMode::Map => {
                if let Some(sim) = self.session.as_ref().and_then(|s| s.sim()) {
                    self.ki_overlay = crate::ki_ui::next_overlay_slot(&sim.state, self.ki_overlay);
                }
            }
            KeyCode::KeyP if self.view == ViewMode::Map => {
                let shift = self.keys.contains(KeyCode::ShiftLeft)
                    || self.keys.contains(KeyCode::ShiftRight);
                if let Some(session) = &mut self.session
                    && let Some(sim) = session.map.sim.as_mut()
                {
                    let before = sim.active_player;
                    sim.switch_player(if shift { -1 } else { 1 });
                    let switched = sim.active_player != before;
                    session.selection.clear();
                    // As command 28 (`0x0047B510` -> `GameEngine_SetLocalPlayer`): nation music of the new player.
                    if switched && let (Some(audio), Some(vfs)) = (&mut self.audio, &self.vfs) {
                        session
                            .presentation
                            .switch_nation_music(&session.map, audio, vfs);
                    }
                }
            }
            KeyCode::KeyH if self.view == ViewMode::Map => self.halt_selection(),
            // Key X: attack mode for the next right-click on/off.
            KeyCode::KeyX if self.view == ViewMode::Map => {
                self.attack_mode = !self.attack_mode;
                if self.attack_mode {
                    opennml_client::game::unit_bar::cancel_target_mode(&mut self.unit_bar);
                }
            }
            // Shift + 1...5: stance 0...4 of the own selected units (command `0x12`); the digits without Shift
            // stay free for groups.
            KeyCode::Digit1
            | KeyCode::Digit2
            | KeyCode::Digit3
            | KeyCode::Digit4
            | KeyCode::Digit5
                if self.view == ViewMode::Map
                    && (self.keys.contains(KeyCode::ShiftLeft)
                        || self.keys.contains(KeyCode::ShiftRight)) =>
            {
                self.set_behavior_selection(match code {
                    KeyCode::Digit1 => 0,
                    KeyCode::Digit2 => 1,
                    KeyCode::Digit3 => 2,
                    KeyCode::Digit4 => 3,
                    _ => 4,
                });
            }
            KeyCode::KeyF if self.view == ViewMode::Map => {
                if let Some(sim) = self.map_mut().and_then(|m| m.sim.as_mut()) {
                    sim.fog.overlay = opennml_client::game::fog_view::next_overlay(sim.fog.overlay);
                }
            }
            KeyCode::KeyO if self.view == ViewMode::Map => {
                if let Some(sim) = self.map_mut().and_then(|m| m.sim.as_mut()) {
                    let off = sim.fog.show_mode != 0;
                    sim.set_fog_shown(off);
                }
            }
            KeyCode::KeyG if self.view == ViewMode::Map => {
                let shift = self.keys.contains(KeyCode::ShiftLeft)
                    || self.keys.contains(KeyCode::ShiftRight);
                self.grid_mode = opennml_client::game::path_view::next_mode(self.grid_mode, shift);
                self.path_preview = None;
            }
            _ if step_sign(code).is_some() => match self.view {
                ViewMode::Effects => self.effect_key(code),
                ViewMode::Browser => self.browser_key(code),
                ViewMode::Map => {}
            },
            _ => {}
        }
    }

    /// Processes an input of the user or of the control channel. Keys are held per source; mouse pointer and
    /// mouse buttons are shared by both sources (the last input counts).
    fn handle_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        source: InputSource,
        event: InputEvent,
    ) {
        self.handle_input_with(event_loop, source, event, None);
    }

    /// Like [`Self::handle_input`], keys with the character `ch` (`vk::char_of`, user keys only). With a visible
    /// dialog layer with an open dialog, mouse and keys go to the UI instead of map and browser; Ctrl+Page
    /// Up/Down and Ctrl+D stay keys of the viewer, Esc ends the viewer only without an open dialog (in the
    /// original Esc has no effect without an acceptor, re/menu-system 12.7).
    fn handle_input_with(
        &mut self,
        event_loop: &ActiveEventLoop,
        source: InputSource,
        event: InputEvent,
        ch: Option<u16>,
    ) {
        if self.menu_input(event_loop, source, event, ch) {
            return;
        }
        // Mouse events of the control channel also reach egui (those of the user come through
        // `window_event`); a press over an egui window belongs to egui alone.
        if source == InputSource::Control
            && self.view == ViewMode::Map
            && self.control_to_egui(event)
        {
            return;
        }
        match event {
            InputEvent::Key {
                code,
                pressed: true,
                repeat,
            } => {
                self.keys.press(source, code);
                if !repeat {
                    self.key_pressed(event_loop, code);
                }
                self.original_key(code, true);
            }
            InputEvent::Key {
                code,
                pressed: false,
                ..
            } => {
                self.keys.release(source, code);
                self.original_key(code, false);
            }
            InputEvent::CursorMoved { x, y } => {
                if let Some((old_x, old_y)) = self.cursor {
                    let delta = Vec2::new((x - old_x) as f32, (y - old_y) as f32);
                    if self.right_button {
                        self.look += delta;
                    }
                    // The left mouse button rotates the camera only in the browsers.
                    if self.left_button && self.view != ViewMode::Map {
                        self.drag += delta;
                    }
                }
                self.cursor = Some((x, y));
                // Formation dragging in the match: every mouse movement with the button held (`0x00453790`).
                self.session_input(&event);
            }
            InputEvent::CursorLeft => self.cursor = None,
            InputEvent::Button {
                button: MouseButton::Right,
                pressed,
            } => {
                self.right_button = pressed;
                if self.view == ViewMode::Map {
                    // Right-click and formation dragging in the match, the modes of the viewer via `ViewerModes`.
                    self.session_input(&event);
                }
            }
            InputEvent::Button {
                button: MouseButton::Left,
                pressed,
            } => {
                self.left_button = pressed;
                if self.view == ViewMode::Map {
                    // Selection in the match; placement, unpack and target clicks via `ViewerModes`.
                    self.session_input(&event);
                }
            }
            // The free mouse (`KD_CAM_FREEHAND`, quirk O-331) exists only in the camera of the original in `opennml-game`.
            InputEvent::Button {
                button: MouseButton::Middle,
                ..
            } => {}
            InputEvent::Wheel { up } => match self.view {
                ViewMode::Browser => {
                    if let Some(browser) = &mut self.browser {
                        browser.orbit.zoom(up);
                    }
                }
                ViewMode::Effects => {
                    if let Some(effects) = &mut self.effect_browser {
                        effects.orbit.zoom(up);
                    }
                }
                ViewMode::Map => self.session_input(&event),
            },
        }
    }

    /// Input to the dialog layer as long as it takes mouse and keys; `true` if the input is thereby handled.
    /// Release always reaches the UI (its table of held keys stays consistent); Ctrl is also held in the viewer
    /// so that Ctrl+Page Up/Down and Ctrl+D reach it.
    fn menu_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        source: InputSource,
        event: InputEvent,
        ch: Option<u16>,
    ) -> bool {
        let Some(shell) = &self.shell else {
            return false;
        };
        let screen = screen_of(shell.gpu.size());
        // `as` saturates; the clock is >= 0.
        let now = self.clock.now_ms() as u64;
        let Some(dialogs) = &mut self.dialogs else {
            return false;
        };
        let takes = dialogs.menu.takes_input();
        let (visible, open) = (dialogs.menu.visible(), dialogs.menu.dialog_open());
        match event {
            InputEvent::Key {
                code,
                pressed: true,
                repeat,
            } => {
                let ctrl = matches!(code, KeyCode::ControlLeft | KeyCode::ControlRight);
                if ctrl {
                    self.keys.press(source, code);
                }
                let ctrl_held = self.keys.contains(KeyCode::ControlLeft)
                    || self.keys.contains(KeyCode::ControlRight);
                match dialogs::route_key(code, ctrl_held, visible, open) {
                    KeyRoute::Browser => {
                        if !repeat {
                            self.dialog_key(code);
                        }
                        true
                    }
                    KeyRoute::Menu => {
                        if let Some(dialogs) = &mut self.dialogs {
                            dialogs.menu.on_key(code, true, ch, screen, now);
                        }
                        true
                    }
                    KeyRoute::Swallow => {
                        if !repeat {
                            log::info!("Esc: a dialog is open (Ctrl+D shows it)");
                        }
                        true
                    }
                    // Ctrl is already held; the rest like any other key.
                    KeyRoute::Viewer if ctrl => {
                        if !repeat {
                            self.key_pressed(event_loop, code);
                        }
                        true
                    }
                    KeyRoute::Viewer => false,
                }
            }
            InputEvent::Key {
                code,
                pressed: false,
                ..
            } => {
                dialogs.menu.on_key(code, false, None, screen, now);
                // The viewer releases its key as well (`handle_input`).
                false
            }
            _ if !takes => false,
            InputEvent::Button { button, pressed } => {
                dialogs.menu.on_input(&event, screen, now);
                if !pressed {
                    // A click begun on the map ends without effect.
                    match button {
                        MouseButton::Left => self.left_button = false,
                        MouseButton::Right => self.right_button = false,
                        MouseButton::Middle => {}
                    }
                }
                true
            }
            InputEvent::CursorMoved { .. } | InputEvent::CursorLeft => {
                dialogs.menu.on_input(&event, screen, now);
                // No edge scrolling and no selection below the UI.
                self.cursor = None;
                true
            }
            InputEvent::Wheel { .. } => {
                dialogs.menu.on_input(&event, screen, now);
                true
            }
        }
    }

    /// Mouse event of the control channel to egui; `true` if a press lies over an egui window (then it does not
    /// reach the map, as with the user; release always).
    fn control_to_egui(&mut self, event: InputEvent) -> bool {
        let shift =
            self.keys.contains(KeyCode::ShiftLeft) || self.keys.contains(KeyCode::ShiftRight);
        let ctrl =
            self.keys.contains(KeyCode::ControlLeft) || self.keys.contains(KeyCode::ControlRight);
        let Some(layer) = &mut self.egui else {
            return false;
        };
        match event {
            InputEvent::CursorMoved { x, y } => {
                layer.pointer_moved(x, y);
                false
            }
            InputEvent::CursorLeft => {
                layer.pointer_gone();
                false
            }
            InputEvent::Button { button, pressed } => {
                let Some((x, y)) = self.cursor else {
                    return false;
                };
                if button == MouseButton::Middle {
                    return false;
                }
                let primary = button == MouseButton::Left;
                layer.pointer_button(x, y, primary, pressed, shift, ctrl) && pressed
            }
            InputEvent::Key { .. } | InputEvent::Wheel { .. } => false,
        }
    }

    /// IDs of the control channel; empty = the selection (in selection order).
    fn ids_or_selection(&self, ids: Vec<u32>) -> Vec<opennml_sim::id::ObjectId> {
        if ids.is_empty() {
            self.session
                .as_ref()
                .map_or_else(Vec::new, |s| s.selection.ids.clone())
        } else {
            ids.into_iter().map(opennml_sim::id::ObjectId).collect()
        }
    }

    /// Simulation of the map (error without a map or without loadable rules).
    fn sim(&self) -> Result<&opennml_client::game::sim_host::SimHost, String> {
        self.map()
            .and_then(|m| m.sim.as_ref())
            .ok_or_else(|| "no simulation (map without loadable rules)".to_string())
    }

    /// Section `sim` for the responses of `sim_setup`, `sim_load` and `sim_view`: with the number of flights of
    /// the map.
    fn sim_section(&self) -> Result<opennml_control::SimSection, String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "no simulation (map without loadable rules)".to_string())?;
        let map = &session.map;
        let sim = map
            .sim
            .as_ref()
            .ok_or_else(|| "no simulation (map without loadable rules)".to_string())?;
        Ok(section_with_flights(
            sim,
            map,
            &session.selection.ids,
            self.unit_bar.formation(),
        ))
    }

    fn sim_mut(&mut self) -> Result<&mut opennml_client::game::sim_host::SimHost, String> {
        self.map_mut()
            .and_then(|m| m.sim.as_mut())
            .ok_or_else(|| "no simulation (map without loadable rules)".to_string())
    }

    /// After loading or restart: selection empty, map from the new state.
    fn after_sim_reset(&mut self) {
        // The combat sounds belong to the old match (like `SimHost::attack_points`).
        self.recent_sounds.clear();
        self.attack_mode = false;
        opennml_client::game::unit_bar::cancel_target_mode(&mut self.unit_bar);
        self.target_notice = None;
        self.path_preview = None;
        self.shown_path = None;
        // Selection, voices, target marker and ground markers empty, map from the new state.
        if let (Some(session), Some(shell), Some(vfs)) = (&mut self.session, &self.shell, &self.vfs)
        {
            // The dialog line of the old match fades out.
            if let Some(audio) = &mut self.audio {
                session.stop_speech(audio);
            }
            session.reset_view(&SessionGpu::of(&shell.gpu), vfs);
        }
    }

    /// `sim_setup`: only `game_speed` changes the game speed; otherwise a new match with the settings in which
    /// the given values are replaced (they also apply to further restarts).
    fn sim_setup(&mut self, id: u64, setup: SimSetupArgs) -> Result<Response, String> {
        self.setup_match(setup)?;
        let section = self.sim_section()?;
        Ok(Response::ok(id, section))
    }

    /// Common path of `sim_setup` and the window "Match": values into the settings, check, new match or game
    /// speed; on an error everything stays unchanged. `true` after a restart.
    fn setup_match(&mut self, setup: SimSetupArgs) -> Result<bool, String> {
        let mut config = self.config.clone();
        let restart = setup.apply(&mut config);
        config.check().map_err(|e| format!("{e:#}"))?;
        apply_config(self.sim_mut()?, &config, restart)?;
        self.config = config;
        if restart {
            self.after_sim_reset();
        }
        Ok(restart)
    }

    /// Window "Match", "Start": loads mission `index` of the list [`Self::missions`] as a new match like
    /// `--mission` (level, game type -1 without lobby, AI and level script on) with the difficulty `difficulty`.
    /// On an error (index, settings, loading the level) the running match stays unchanged. Response: the name of
    /// the mission.
    fn start_mission(&mut self, index: usize, difficulty: i32) -> Result<String, String> {
        let mission = self
            .missions
            .get(index)
            .ok_or_else(|| format!("Mission {index} unknown"))?
            .clone();
        let mut config = self.config.clone();
        config.use_mission(&mission.path);
        config.campaign_difficulty = difficulty;
        config.check().map_err(|e| format!("{e:#}"))?;
        let (Some(shell), Some(vfs)) = (&self.shell, &self.vfs) else {
            return Err("no window".into());
        };
        let (session, random_map) =
            open_session(shell, vfs, &config).map_err(|e| format!("{e:#}"))?;
        // The dialog line of the old mission fades out.
        if let (Some(old), Some(audio)) = (&mut self.session, &mut self.audio) {
            old.stop_speech(audio);
        }
        self.session = Some(session);
        self.random_map = random_map;
        self.config = config;
        self.after_sim_reset();
        self.start_level_audio();
        Ok(mission.name)
    }

    /// Shift + 1...5: stance `mode` (0...4, command `0x12`) for the own selected objects of the active
    /// player.
    fn set_behavior_selection(&mut self, mode: i32) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(sim) = session.map.sim.as_mut() else {
            return;
        };
        let player = sim.active_player;
        let units: Vec<_> = session
            .selection
            .ids
            .iter()
            .copied()
            .filter(|&id| {
                sim.state
                    .object(id, false)
                    .is_some_and(|o| o.owner() == player)
            })
            .collect();
        if units.is_empty() {
            return;
        }
        sim.submit(opennml_sim::command::Command {
            player,
            kind: opennml_sim::command::CommandKind::SetBehavior { units, mode },
        });
    }

    /// Key path of the original: with `--hud` on the map, keys without a viewer binding
    /// ([`hud_view::viewer_key`]) go through the same stages as in `opennml-game` - binding from `bindings2.ini`,
    /// under the lock only it, otherwise the line from `keys.ini` in the keyboard context
    /// (`Hotkeys::resolve`) - and the match executes the command (`GameSession::run_hotkey`; groups, game speed,
    /// `selectnextguid`, bar shortcuts, Home). Quicksave and quickload do not exist without a menu host; neither
    /// does the pause of the original (`togglepause`): the viewer advances the match with `advance_by`, which
    /// does not evaluate the pause bits (its pause is the virtual clock). Without `--hud` everything stays as
    /// before.
    fn original_key(&mut self, code: KeyCode, pressed: bool) {
        let shift =
            self.keys.contains(KeyCode::ShiftLeft) || self.keys.contains(KeyCode::ShiftRight);
        let dispatch = self.config.hud && self.view == ViewMode::Map && self.session.is_some();
        let Some(held) = self.original_keys.key(dispatch, code, pressed, shift) else {
            return;
        };
        self.sync_session();
        // `as` saturates; the clock is >= 0.
        let now = self.clock.now_ms() as u64;
        let (Some(session), Some(vfs)) = (&mut self.session, &self.vfs) else {
            return;
        };
        let Some(cmd) = session.hotkeys.resolve(
            &held,
            &session.input_gate(),
            &session.key_context(),
            &session.key_map,
        ) else {
            return;
        };
        match cmd {
            HotkeyCommand::QuickSave | HotkeyCommand::QuickLoad | HotkeyCommand::TogglePause => {
                log::debug!("Key {held:?}: {cmd:?} exists only in opennml-game");
            }
            cmd => {
                session.run_hotkey(&cmd, now, vfs, self.audio.as_mut());
            }
        }
    }

    /// Key H: halt (`0x3C`) for the own selected objects of the active player.
    fn halt_selection(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(sim) = session.map.sim.as_mut() else {
            return;
        };
        let player = sim.active_player;
        let units: Vec<_> = session
            .selection
            .ids
            .iter()
            .copied()
            .filter(|&id| {
                sim.state
                    .object(id, false)
                    .is_some_and(|o| o.owner() == player)
            })
            .collect();
        if units.is_empty() {
            return;
        }
        sim.submit(opennml_sim::command::Command {
            player,
            kind: opennml_sim::command::CommandKind::Halt { units },
        });
    }

    /// `set_quality`: apply stages; `shadow_quality` determines the shadows of all objects anew and
    /// `particles_quality` creates the effects anew (like reloading the map), `terrain_quality` >= 2 switches the
    /// water to the detail texture mode.
    fn set_quality(&mut self, shadow: Option<u8>, particles: Option<u8>, terrain: Option<u8>) {
        if let Some(shadow) = shadow {
            self.config.shadow_quality = shadow;
            if let (Some(map), Some(gpu), Some(vfs)) = (
                self.session.as_mut().map(|s| &mut s.map),
                self.shell.as_ref().map(|s| &s.gpu),
                &self.vfs,
            ) {
                map.set_shadow_quality(&gpu.device, &gpu.queue, vfs, shadow);
            }
            if let Some(browser) = &mut self.browser {
                browser.set_quality(shadow);
            }
        }
        if let Some(particles) = particles {
            self.config.particles_quality = particles;
            if let (Some(map), Some(vfs)) = (self.session.as_mut().map(|s| &mut s.map), &self.vfs) {
                map.set_particles_quality(vfs, particles);
            }
        }
        if let Some(terrain) = terrain {
            self.config.terrain_quality = terrain;
            if let Some(map) = self.session.as_mut().map(|s| &mut s.map) {
                map.renderer.set_water_detail(terrain >= 2);
            }
        }
    }

    /// Draws a frame unless the window is minimized (`step`; after `set_light`, `set_weather` and
    /// `set_quality`) so that the following state snapshot (drawn weather particles, overlay) belongs to the new
    /// state and not to the last frame before.
    fn present_unless_minimized(&mut self) {
        if !self.minimized() {
            self.present();
        }
    }

    /// Game UI on/off (`--hud`, command `hud`): without a dialog layer the first switch-on creates the own
    /// drawing layer (fonts with the window width, O-58). Before `resumed` (without a window) only the switch
    /// counts. If creation fails, the UI stays off.
    fn set_hud(&mut self, on: bool) -> anyhow::Result<()> {
        let needs_layer = hud_view::apply_hud(
            &mut self.config,
            on,
            self.dialogs.is_some(),
            self.hud_layer.is_some(),
        );
        if needs_layer && let (Some(shell), Some(vfs)) = (&self.shell, &self.vfs) {
            match HudLayer::new(
                vfs,
                &shell.gpu.device,
                shell.gpu.surface_format(),
                &self.config.language_tag,
                shell.ui_width(),
            ) {
                Ok(layer) => self.hud_layer = Some(layer),
                Err(e) => {
                    self.config.hud = false;
                    return Err(e.context("game UI"));
                }
            }
        }
        self.sync_session();
        Ok(())
    }

    /// `step`: `frames` logic steps of `frame_ms` each, then one frame (omitted with a minimized window); the
    /// clock stays paused afterward.
    fn step(&mut self, frames: u32, frame_ms: f32) {
        self.clock.pause();
        for _ in 0..frames {
            self.advance(frame_ms);
        }
        self.present_unless_minimized();
    }

    /// Executes a control command and sends the response to the control thread.
    fn run_control(&mut self, event_loop: &ActiveEventLoop, event: ControlEvent) {
        let ControlEvent { request, reply } = event;
        let Request { id, command } = request;
        let response = command
            .validate()
            .and_then(|()| self.execute(event_loop, id, command))
            .unwrap_or_else(|message| Response::error(id, message));
        // If the control thread is gone, nobody needs the response any more.
        let _ = reply.send(response);
    }

    fn execute(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: u64,
        command: Command,
    ) -> Result<Response, String> {
        let input = |event| vec![event];
        let events = match command {
            Command::Hello => {
                return Ok(Response::ok(
                    id,
                    HelloInfo {
                        protocol: PROTOCOL_VERSION,
                        pid: std::process::id(),
                        backend: self
                            .shell
                            .as_ref()
                            .map(|s| format!("{:?}", s.gpu.backend())),
                        adapter: self
                            .shell
                            .as_ref()
                            .map(|s| s.gpu.adapter_name().to_string()),
                    },
                ));
            }
            Command::Key {
                code,
                action,
                repeat,
            } => {
                let code = key_code(&code).ok_or_else(|| format!("unknown key \"{code}\""))?;
                key_events(code, action, repeat)
            }
            Command::UiFocusLost => {
                return Err("\"ui_focus_lost\" exists only in opennml-game".into());
            }
            // Network play only in `opennml-game` (`--host`/`--join`).
            Command::NetStatus
            | Command::NetPartHashes
            | Command::NetSave { .. }
            | Command::NetDebug { .. } => {
                return Err("Network commands exist only in opennml-game (--host/--join)".into());
            }
            Command::MouseMove { x, y } => input(InputEvent::CursorMoved { x, y }),
            Command::MouseLeave => input(InputEvent::CursorLeft),
            Command::MouseButton { button, down } => input(InputEvent::Button {
                button: match button {
                    MouseButtonName::Left => MouseButton::Left,
                    MouseButtonName::Right => MouseButton::Right,
                    // Quirk O-331: the free mouse (`KD_CAM_FREEHAND`) exists only in the camera of the original.
                    MouseButtonName::Middle => {
                        return Err("mouse_button \"middle\" exists only in opennml-game".into());
                    }
                },
                pressed: down,
            }),
            Command::Wheel { up } => input(InputEvent::Wheel { up }),
            Command::Pause => {
                self.clock.pause();
                Vec::new()
            }
            Command::Resume => {
                self.clock.resume();
                Vec::new()
            }
            Command::Step { frames, frame_ms } => {
                self.step(frames, frame_ms as f32);
                return Ok(Response::ok(id, self.snapshot(event_loop)));
            }
            // The viewer shows the system pointer; `pointer` is ignored.
            Command::Screenshot {
                overlay, max_size, ..
            } => {
                let img = self.capture(overlay).map_err(|e| format!("{e:#}"))?;
                return png_data(&img, max_size).map(|data| Response::ok(id, data));
            }
            Command::State => return Ok(Response::ok(id, self.snapshot(event_loop))),
            // Presentation event by hand (presentation only, the checksum stays); changes the host state (game
            // speed, fog mode, selection). The fades run with the frame time of the viewer.
            Command::UiPresent { event } => {
                let (Some(session), Some(shell), Some(vfs)) = (
                    self.session.as_mut(),
                    self.shell.as_ref(),
                    self.vfs.as_ref(),
                ) else {
                    return Err("no map loaded".into());
                };
                let gpu = SessionGpu::of(&shell.gpu);
                session.present_json(&gpu, vfs, self.audio.as_mut(), &event)?;
                self.present_unless_minimized();
                Vec::new()
            }
            Command::UiState
            | Command::UiClick { .. }
            | Command::UiKey { .. }
            | Command::UiOpen { .. }
            | Command::UiVideo { .. } => {
                // Protocol 24: only the operable dialog layer (`--dialog` in the window), in the window thread.
                let screen = screen_of(self.shell.as_ref().map_or((0, 0), |s| s.gpu.size()));
                // `as` saturates; the clock is >= 0.
                let now = self.clock.now_ms() as u64;
                let dialogs = self.dialogs.as_mut().ok_or_else(|| {
                    format!("{NO_UI} (the viewer needs --dialog <names> in the window)")
                })?;
                return execute_ui(&mut dialogs.menu.host.app, &command, screen, now)
                    .map(|value| Response::ok(id, value));
            }
            Command::Window { mode, monitor } => {
                self.set_window(event_loop, mode.map(window_mode), monitor)?;
                Vec::new()
            }
            Command::Hud { on } => {
                self.set_hud(on).map_err(|e| format!("{e:#}"))?;
                self.present_unless_minimized();
                Vec::new()
            }
            Command::Quit => {
                self.quit_requested = true;
                event_loop.exit();
                Vec::new()
            }
            Command::RandomMap => return Ok(Response::ok(id, &self.random_map)),
            Command::Lights => {
                let map = self
                    .session
                    .as_ref()
                    .map(|s| &s.map)
                    .ok_or("no map loaded")?;
                return Ok(Response::ok(id, map.light_names()));
            }
            Command::SetLight { name } => {
                let (Some(map), Some(gpu)) = (
                    self.session.as_mut().map(|s| &mut s.map),
                    self.shell.as_ref().map(|s| &s.gpu),
                ) else {
                    return Err("no map loaded".into());
                };
                map.set_light_named(&gpu.queue, &name)
                    .map_err(|e| format!("{e:#}"))?;
                self.present_unless_minimized();
                Vec::new()
            }
            Command::SetWeather { kind } => {
                let map = self
                    .session
                    .as_mut()
                    .map(|s| &mut s.map)
                    .ok_or("no map loaded")?;
                map.set_weather(weather_kind(kind));
                self.present_unless_minimized();
                Vec::new()
            }
            Command::SetQuality {
                shadow,
                particles,
                terrain,
            } => {
                self.set_quality(shadow, particles, terrain);
                self.present_unless_minimized();
                Vec::new()
            }
            Command::SpawnEffect {
                path,
                position,
                object_index,
                scale,
            } => {
                let position = position.map(Vec3::from);
                let scale = scale.unwrap_or(1.0);
                let vfs = self.vfs.as_ref().ok_or("no archives")?;
                match self.view {
                    ViewMode::Map => {
                        let map = self
                            .session
                            .as_mut()
                            .map(|s| &mut s.map)
                            .ok_or("no map loaded")?;
                        map.spawn_effect(vfs, &path, position, object_index, scale)?;
                    }
                    _ if object_index.is_some() => {
                        return Err("object_index exists only in the map view".into());
                    }
                    ViewMode::Browser => {
                        let browser = self.browser.as_mut().ok_or("no model browser")?;
                        browser.spawn_effect(vfs, &path, position, scale)?;
                    }
                    ViewMode::Effects => {
                        let effects = self.effect_browser.as_mut().ok_or("no effect browser")?;
                        effects.spawn_effect(vfs, &path, position, scale)?;
                    }
                }
                self.present_unless_minimized();
                Vec::new()
            }
            Command::ClearEffects => {
                match self.view {
                    ViewMode::Map => {
                        if let Some(map) = self.session.as_mut().map(|s| &mut s.map) {
                            map.effects.clear_spawned();
                        }
                    }
                    ViewMode::Browser => {
                        if let Some(browser) = &mut self.browser {
                            browser.effects.world.clear_spawned();
                        }
                    }
                    ViewMode::Effects => {
                        if let Some(effects) = &mut self.effect_browser {
                            effects.effects.clear_spawned();
                        }
                    }
                }
                self.present_unless_minimized();
                Vec::new()
            }
            Command::SetDamage { value } => {
                let browser = self.browser.as_mut().ok_or("no model browser (key B)")?;
                browser.effects.set_damage(value);
                self.present_unless_minimized();
                Vec::new()
            }
            Command::PlaySound {
                guid,
                action,
                path,
                position,
            } => {
                let (Some(audio), Some(vfs)) = (&mut self.audio, &self.vfs) else {
                    return Err("no sound".into());
                };
                let source = match (guid.zip(action), &path) {
                    (Some((guid, action)), _) => SoundSource::Action { guid, action },
                    (None, Some(path)) => SoundSource::File(path),
                    (None, None) => {
                        return Err("play_sound needs guid and action or path".into());
                    }
                };
                let (played, sound_id) = audio.play_sound(vfs, source, position.map(Vec3::from));
                return Ok(Response::ok(
                    id,
                    PlaySoundResult {
                        played,
                        id: sound_id,
                    },
                ));
            }
            Command::Music { action, track, on } => {
                let (Some(audio), Some(vfs)) = (&mut self.audio, &self.vfs) else {
                    return Err("no sound".into());
                };
                match action {
                    MusicCommand::Play => {
                        audio.play_menu_music(vfs, track.as_deref().unwrap_or_default())
                    }
                    MusicCommand::Stop => audio.stop_music(),
                    MusicCommand::Attack => audio.set_attack_override(on.unwrap_or(false)),
                }
                Vec::new()
            }
            Command::SetVolume {
                music,
                sfx,
                speech,
                mute,
                unit_response,
            } => {
                if let Some(on) = unit_response
                    && let Some(session) = &mut self.session
                {
                    session.voices.unit_response = on;
                }
                let audio = self.audio.as_mut().ok_or("no sound")?;
                for (category, value) in [
                    (Category::Music, music),
                    (Category::Sfx, sfx),
                    (Category::Speech, speech),
                ] {
                    if let Some(value) = value {
                        audio.set_volume(category, i32::from(value));
                    }
                }
                if let Some(mute) = mute {
                    audio.set_mute(mute);
                }
                Vec::new()
            }
            Command::AudioCapture { action, path } => {
                let audio = self.audio.as_mut().ok_or("no sound")?;
                match action {
                    CaptureCommand::Start => {
                        let path = PathBuf::from(path.unwrap_or_default());
                        audio.start_capture(&path).map_err(|e| format!("{e:#}"))?;
                        Vec::new()
                    }
                    CaptureCommand::Stop => {
                        let summary = audio
                            .stop_capture()
                            .map_err(|e| format!("{e:#}"))?
                            .ok_or("no recording is running")?;
                        return Ok(Response::ok(id, capture_data(summary)));
                    }
                }
            }
            Command::SimSetup {
                mode,
                seed,
                nations,
                teams,
                game_speed,
                resource_setting,
                living_space,
                map_explored,
                slots,
                ki_enabled,
                campaign_difficulty,
                script_enabled,
                time_per_char,
                time_limit,
            } => {
                let setup = SimSetupArgs {
                    mode,
                    seed,
                    nations,
                    teams,
                    game_speed,
                    resource_setting,
                    living_space,
                    map_explored,
                    slots,
                    ki_enabled,
                    campaign_difficulty,
                    script_enabled,
                    time_per_char,
                    time_limit,
                };
                return self.sim_setup(id, setup);
            }
            // First read and check the command, then the simulation (order of the messages unchanged).
            Command::SimCommand { command } => {
                let command = opennml_client::game::sim_control::parse_sim_command(command)?;
                let result = self.sim_mut()?.control_submit(command);
                return Ok(Response::ok(id, result));
            }
            // `sim_result` queues, like `sim_command`, command `0x56` for the next tick.
            command @ (Command::SimQuery { .. } | Command::SimHash | Command::SimResult { .. }) => {
                let result = opennml_client::game::sim_control::execute_sim(
                    Some(self.sim_mut()?),
                    &command,
                )?;
                return Ok(Response::ok(id, result));
            }
            Command::KiQuery {
                player,
                code,
                words,
                sight,
            } => {
                let result = self.sim_mut()?.control_ki_query(
                    player,
                    code,
                    words.as_deref(),
                    sight.as_deref(),
                )?;
                return Ok(Response::ok(id, result));
            }
            Command::SimSave { path, replay } => {
                let sim = self.sim()?;
                let bytes = if replay {
                    sim.replay().map_err(|e| format!("{e:#}"))?.to_bytes()
                } else {
                    sim.save().to_bytes()
                };
                std::fs::write(&path, &bytes).map_err(|e| format!("writing {path}: {e}"))?;
                return Ok(Response::ok(
                    id,
                    opennml_control::SimSaveResult {
                        path,
                        bytes: bytes.len(),
                        steps: sim.state.steps,
                        checksum: opennml_control::hex_checksum(sim.checksum()),
                    },
                ));
            }
            Command::SimLoad { path, steps } => {
                let bytes = std::fs::read(&path).map_err(|e| format!("reading {path}: {e}"))?;
                let sim = self.sim_mut()?;
                let loaded = if bytes.starts_with(opennml_sim::save::SAVE_MAGIC) {
                    // `SimHost::load` does not check against the loaded rules itself (hence `check` first).
                    opennml_sim::save::SaveGame::from_bytes(&bytes).and_then(|s| {
                        s.check(&sim.rules, opennml_sim::save::RulesCompare::Full)?;
                        sim.load(s, None);
                        Ok(())
                    })
                } else if bytes.starts_with(opennml_sim::save::REPLAY_MAGIC)
                    || bytes.starts_with(opennml_client::replay_file::REPLAY_FILE_MAGIC)
                {
                    // `NMLREPL` and the appendable recording `NMLAUTO`.
                    opennml_client::replay_file::read_any_replay(&bytes).and_then(|(r, _, torn)| {
                        if torn {
                            log::warn!(
                                "{path}: recording torn off, read up to the last whole section"
                            );
                        }
                        sim.load_replay(r, steps)
                    })
                } else {
                    Err(anyhow::anyhow!(
                        "{path} is neither a savegame nor a replay (identifier missing)"
                    ))
                };
                loaded.map_err(|e| format!("{e:#}"))?;
                self.after_sim_reset();
                let section = self.sim_section()?;
                return Ok(Response::ok(id, section));
            }
            Command::SimPath {
                unit,
                from,
                to,
                layer,
                allow_override,
                show,
            } => {
                let sim = self.sim()?;
                let cell = sim.rules.map.cell;
                let ground = |p: [f32; 2]| world_to_grid(glam::Vec3::new(p[0], 0.0, p[1]), cell);
                let goal = ground(to);
                let layer = layer.map(usize::from);
                let view = match (unit, from) {
                    (Some(u), _) => opennml_client::game::path_view::plan(
                        sim,
                        opennml_sim::id::ObjectId(u),
                        goal,
                        layer,
                        allow_override,
                    )
                    .ok_or_else(|| {
                        format!("Object {u} is missing or has no walk module (specify layer)")
                    })?,
                    (None, Some(f)) => {
                        let (from, layer) = (ground(f), layer.unwrap_or(0));
                        PathView {
                            unit: opennml_sim::id::ObjectId::NONE,
                            layer,
                            from,
                            goal,
                            allow_override,
                            result: opennml_sim::path::highpath::plan_high_path(
                                &sim.state.path,
                                layer,
                                from,
                                goal,
                                allow_override,
                                0x201,
                            ),
                        }
                    }
                    (None, None) => return Err("sim_path needs unit or from".into()),
                };
                let result = opennml_client::game::path_view::path_result(&view, cell);
                self.shown_path = show.then_some(view);
                return Ok(Response::ok(id, result));
            }
            Command::SimMove {
                units,
                to,
                append,
                cyclic,
                drag_to,
            } => {
                let units = self.ids_or_selection(units);
                // Shift (append) before Ctrl (cyclic) like `Input_SubmitCommandWithQueueKeys`.
                let queue = opennml_client::game::economy::queue_keys(append, cyclic);
                // With a drag point, facing direction and spread as with right-click dragging; the command always
                // carries the chosen formation of the bar.
                let bar = self.unit_bar;
                let sim = self.sim_mut()?;
                // Ground points in world coordinates (height 0) as with right-click dragging.
                let ground = |p: [f32; 2]| Vec3::new(p[0], 0.0, p[1]);
                let drag = drag_to.and_then(|d| formation_drag(ground(to), ground(d)));
                let order = FormationOrder::new(&bar, drag);
                if !sim.move_units(&units, Vec3::new(to[0], 0.0, to[1]), queue, order) {
                    return Err(format!(
                        "no own unit with a walk module (player {})",
                        sim.active_player
                    ));
                }
                return Ok(Response::ok(
                    id,
                    serde_json::json!({ "step": sim.state.steps }),
                ));
            }
            Command::SimGrid { layer, rect } => {
                let sim = self.sim()?;
                return Ok(Response::ok(
                    id,
                    opennml_client::game::path_view::grid_result(&sim.state, layer, rect),
                ));
            }
            Command::SimFog { player, rect } => {
                let sim = self.sim()?;
                let player = player.unwrap_or(sim.active_player);
                return Ok(Response::ok(
                    id,
                    opennml_client::game::fog_view::fog_result(&sim.state, player, rect),
                ));
            }
            Command::SimView {
                active_player,
                fog_shown,
                fog_overlay,
            } => {
                let sim = self.sim_mut()?;
                let before = sim.active_player;
                if let Some(slot) = active_player {
                    sim.set_active_player(slot)?;
                }
                if let Some(on) = fog_shown {
                    sim.set_fog_shown(on);
                }
                if let Some(level) = fog_overlay {
                    sim.fog.overlay = (level != 0).then_some(level);
                }
                // Like key P: a new active player starts without a selection and with their nation music.
                if sim.active_player != before
                    && let Some(session) = &mut self.session
                {
                    session.selection.clear();
                    if let (Some(audio), Some(vfs)) = (&mut self.audio, &self.vfs) {
                        session
                            .presentation
                            .switch_nation_music(&session.map, audio, vfs);
                    }
                }
                let section = self.sim_section()?;
                return Ok(Response::ok(id, section));
            }
            Command::SimBuild {
                guid,
                to,
                rotation_steps,
                units,
                append,
            } => {
                let units = self.ids_or_selection(units);
                let result =
                    self.sim_mut()?
                        .control_build(guid, to, rotation_steps, &units, append)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimTrain { building, guid } => {
                let sim = self.sim_mut()?;
                let result = sim.control_train(opennml_sim::id::ObjectId(building), guid)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimCancel { building, index } => {
                let sim = self.sim_mut()?;
                let result = sim.control_cancel(opennml_sim::id::ObjectId(building), index)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimComplete {
                building,
                index,
                guid,
            } => {
                let sim = self.sim_mut()?;
                let result =
                    sim.control_complete(opennml_sim::id::ObjectId(building), index, guid)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimSell { buildings } => {
                let buildings = self.ids_or_selection(buildings);
                let result = self.sim_mut()?.control_sell(&buildings)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimUpgrade {
                player,
                guid,
                remove,
            } => {
                let result = self.sim_mut()?.control_upgrade(player, guid, remove)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimTrade {
                player,
                units,
                ware,
                target,
            } => {
                let units = self.ids_or_selection(units);
                let result = self
                    .sim_mut()?
                    .control_trade(player, &units, ware, target)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimTribute {
                player,
                recipients,
                resource,
                amount,
            } => {
                let result = self
                    .sim_mut()?
                    .control_tribute(player, recipients, resource, amount)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimUnitBar {
                button,
                target,
                ground,
            } => {
                let selection = self.ids_or_selection(Vec::new());
                let mut bar = self.unit_bar;
                let sim = self.sim_mut()?;
                opennml_client::game::unit_bar::control_unit_bar(
                    &mut bar, sim, &selection, button, target, ground,
                )?;
                let step = sim.state.steps;
                // 453 starts the unpack cursor; it excludes building placement. A target button changes nothing
                // about the targeting mode via the control channel (the command goes at once).
                if button == 453 && bar.tent_cursor.is_some() {
                    self.economy.placement = None;
                }
                self.unit_bar = bar;
                return Ok(Response::ok(
                    id,
                    serde_json::json!({ "step": step, "formation": bar.formation() }),
                ));
            }
            Command::SimAction {
                units,
                target,
                to,
                append,
                cyclic,
            } => {
                let units = self.ids_or_selection(units);
                // Shift (append) before Ctrl (cyclic) like `Input_SubmitCommandWithQueueKeys`.
                let queue = opennml_client::game::economy::queue_keys(append, cyclic);
                let target = target.map(opennml_sim::id::ObjectId);
                let now = self.clock.now_f32();
                let mut highlight = self.session.as_ref().and_then(|s| s.target_highlight);
                let bar = self.unit_bar;
                let result = opennml_client::game::markers::control_action_marked(
                    self.sim_mut()?,
                    &mut highlight,
                    &units,
                    target,
                    to,
                    queue,
                    FormationOrder::new(&bar, None),
                    now,
                )?;
                if let Some(session) = &mut self.session {
                    session.set_target_highlight(highlight);
                }
                return Ok(Response::ok(id, result));
            }
            Command::SimMenu { units, building } => {
                let units = units.map(|u| self.ids_or_selection(u));
                let building = building.map(opennml_sim::id::ObjectId);
                let result = self.sim_mut()?.control_menu(units.as_deref(), building)?;
                return Ok(Response::ok(id, result));
            }
            Command::SimHireMenu {
                player,
                art,
                target_player,
            } => {
                let result = self
                    .sim_mut()?
                    .control_hire_menu(player, art, target_player)?;
                return Ok(Response::ok(id, result));
            }
            // Scripts (protocol 28).
            Command::ScriptState { block, names, log } => {
                let result = self
                    .sim()?
                    .control_script_state(block, names.as_deref(), log)?;
                return Ok(Response::ok(id, result));
            }
            // Presentation events of the scripts (protocol 33); the camera belongs to the session.
            Command::ScriptEvents { steps } => {
                let session = self
                    .session
                    .as_ref()
                    .ok_or_else(|| "no simulation (map without loadable rules)".to_string())?;
                let result = crate::script_ui::script_events(
                    self.sim()?,
                    &session.presentation.camera,
                    steps,
                )?;
                return Ok(Response::ok(id, result));
            }
            // Debug intervention in the script flow as a command (protocol 32).
            Command::ScriptCommand { action } => {
                let result = self.sim_mut()?.control_script_command(action)?;
                return Ok(Response::ok(id, result));
            }
            // AI match (protocol 26).
            Command::KiState { slot, values } => {
                let result = self
                    .sim()?
                    .control_ki_state(slot, values.unwrap_or(false))?;
                return Ok(Response::ok(id, result));
            }
            Command::KiCommand {
                action,
                slot,
                module,
                args,
                sync,
                paused,
                passes,
            } => {
                let request = opennml_client::game::sim_control::KiCommandRequest {
                    action,
                    slot,
                    module,
                    args,
                    sync,
                    paused,
                    passes,
                };
                let sim = self.sim_mut()?;
                sim.control_ki_command(request)?;
                return Ok(Response::ok(
                    id,
                    serde_json::json!({ "step": sim.state.steps }),
                ));
            }
        };
        for event in events {
            self.handle_input(event_loop, InputSource::Control, event);
        }
        Ok(Response::ok(id, ()))
    }
}

impl ApplicationHandler<ControlEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shell.is_some() {
            return;
        }
        let shell = match WindowShell::create(
            event_loop,
            "NML Viewer",
            (self.config.width, self.config.height),
            self.config.backend,
            self.config.monitor,
            self.config.window_mode,
        ) {
            Ok(shell) => shell,
            Err(e) => return self.fail(event_loop, e),
        };
        let gpu = &shell.gpu;
        let vfs = self.vfs.as_ref().expect("VFS present at start");
        match open_session(&shell, vfs, &self.config) {
            Ok((session, random_map)) => {
                self.session = Some(session);
                self.random_map = random_map;
            }
            Err(e) => return self.fail(event_loop, e),
        }
        self.egui = Some(EguiLayer::new(
            &shell.window,
            &gpu.device,
            gpu.surface_format(),
        ));
        // Dialog layer (`--dialog`): fonts with the width at start (O-58); the names and the location of the
        // settings file are checked in `main`.
        if !self.dialog_names.is_empty() {
            let Some(audio) = &self.audio else {
                return self.fail(
                    event_loop,
                    anyhow::anyhow!("dialog layer without a sound system"),
                );
            };
            match DialogMenu::new(
                vfs,
                &self.config.language_tag,
                &self.dialog_names,
                // Minimized window at start: no fonts with 0 px.
                shell.ui_width(),
                opennml_client::ui_layer::atlas_generator(vfs).as_deref(),
                self.dialog_settings.clone(),
                audio,
                MenuHost::raw_display_modes(&shell.window),
            ) {
                Ok(menu) => {
                    self.dialogs = Some(DialogLayer::new(menu, &gpu.device, gpu.surface_format()));
                }
                Err(e) => return self.fail(event_loop, e),
            }
        }
        self.shell = Some(shell);
        // Game UI (`--hud`): after the dialog layer, whose layer it then shares.
        if let Err(e) = self.set_hud(self.config.hud) {
            return self.fail(event_loop, e);
        }
        self.start_level_audio();
        // `--model`: start directly in the browser; an unknown model ends the program.
        if self.view == ViewMode::Browser {
            self.view = ViewMode::Map;
            if let Err(e) = self.toggle_browser() {
                return self.fail(event_loop, e);
            }
        }
        self.apply_mode(event_loop);
        for event in std::mem::take(&mut self.waiting) {
            self.run_control(event_loop, event);
        }
    }

    /// Control command from the control thread. Before `resumed` (no window, no GPU) only `hello` is answered at
    /// once, everything else is executed afterward.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ControlEvent) {
        if self.shell.is_none() && event.request.command != Command::Hello {
            self.waiting.push(event);
            return;
        }
        self.run_control(event_loop, event);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Clicks and keys that egui claims for itself (debug window) do not reach the map; release of the left
        // button always, so that a selection box ends.
        if let (Some(layer), Some(shell)) = (&mut self.egui, &self.shell)
            && self.view == ViewMode::Map
        {
            let consumed = layer.on_window_event(&shell.window, &event);
            let release = matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    ..
                }
            );
            let input = matches!(
                event,
                WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
                    | WindowEvent::KeyboardInput { .. }
            );
            if consumed && input && !release {
                return;
            }
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // On a DPI change (other monitor) winit then delivers a Resized with the new physical size.
            WindowEvent::Resized(size) => {
                if let Some(shell) = &mut self.shell {
                    shell.resized(size.width, size.height);
                }
            }
            WindowEvent::Focused(true) | WindowEvent::Occluded(false) => {
                if let Some(shell) = &mut self.shell {
                    shell.shown();
                }
            }
            WindowEvent::Focused(false) => {
                self.keys.clear_user();
                self.original_keys.clear();
                if let Some(dialogs) = &mut self.dialogs {
                    dialogs.menu.release_keys();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state,
                        physical_key: PhysicalKey::Code(code),
                        repeat,
                        text,
                        ..
                    },
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                // The character of the key for input fields of the dialog layer (cp1252, re/menu-system 12.7).
                let ch = if pressed {
                    char_of(text.as_deref())
                } else {
                    None
                };
                self.handle_input_with(
                    event_loop,
                    InputSource::User,
                    InputEvent::Key {
                        code,
                        pressed,
                        repeat,
                    },
                    ch,
                );
            }
            WindowEvent::CursorMoved { position, .. } => self.handle_input(
                event_loop,
                InputSource::User,
                InputEvent::CursorMoved {
                    x: position.x,
                    y: position.y,
                },
            ),
            WindowEvent::CursorLeft { .. } => {
                self.handle_input(event_loop, InputSource::User, InputEvent::CursorLeft)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    _ => return,
                };
                let pressed = state == ElementState::Pressed;
                self.handle_input(
                    event_loop,
                    InputSource::User,
                    InputEvent::Button { button, pressed },
                );
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(up) = wheel_up(delta) {
                    self.handle_input(event_loop, InputSource::User, InputEvent::Wheel { up });
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.menu_quit {
            return event_loop.exit();
        }
        if let Some(shell) = &mut self.shell {
            shell.schedule(event_loop);
        }
    }
}

/// Modes of the viewer that take clicks on the map ([`HostModes`] of the match): building placement (window
/// "Economy"), unpack cursor (button 453), targeting mode of a target button, headhunter menu, attack mode
/// (key X) and the formation of the unit bar.
struct ViewerModes<'a> {
    economy: &'a mut EconomyUi,
    unit_bar: &'a mut UnitBar,
    attack_mode: &'a mut bool,
    target_notice: &'a mut Option<String>,
    /// Left mouse button pressed in one of these modes (set on release).
    place_press: &'a mut bool,
    /// Game UI on (`--hud`): the command bar of the session sees the mouse before the modes, its formation
    /// applies to the right-clicks.
    hud: bool,
}

impl HostModes for ViewerModes<'_> {
    /// In placement mode, with the unpack cursor and in targeting mode the click places instead of selecting (no
    /// selection change).
    fn left_press(&mut self) -> bool {
        *self.place_press = self.economy.placement.is_some()
            || self.unit_bar.tent_cursor.is_some()
            || self.unit_bar.target_mode.is_some();
        *self.place_press
    }

    fn left_release(&mut self, session: &mut GameSession) -> bool {
        if !std::mem::take(self.place_press) {
            return false;
        }
        if self.unit_bar.target_mode.is_some() {
            self.finish_target_click(session);
        } else {
            self.finish_placement_click(session);
        }
        true
    }

    /// In placement mode a right-click ends the mode, likewise the unpack cursor, the targeting mode and the
    /// headhunter menu with a chosen kind.
    fn cancel_right_click(&mut self) -> bool {
        if self.economy.cancel_placement()
            || self.unit_bar.tent_cursor.take().is_some()
            || self.unit_bar.target_mode.is_some()
            || self.economy.cancel_hire()
        {
            opennml_client::game::unit_bar::cancel_target_mode(self.unit_bar);
            return true;
        }
        false
    }

    fn take_attack(&mut self) -> bool {
        std::mem::take(self.attack_mode)
    }

    /// With `--hud` the formation of the command bar of the session (the original bar is then the UI of the
    /// match), otherwise that of the unit bar of the viewer.
    fn formation(&self, drag: Option<(i32, i32)>, toolbar: &UnitBar) -> FormationOrder {
        if self.hud {
            FormationOrder::new(toolbar, drag)
        } else {
            FormationOrder::new(self.unit_bar, drag)
        }
    }
}

impl ViewerModes<'_> {
    /// Left mouse button released in placement mode: place the building at the terrain point if the site is
    /// valid (Shift: place more and queued, Ctrl cyclic). With the unpack cursor unpack the tent instead
    /// ([`opennml_client::game::unit_bar::click_unpack`]).
    fn finish_placement_click(&mut self, session: &mut GameSession) {
        let Some(end) = session.pointer.cursor else {
            return;
        };
        let shift = session.pointer.shift();
        let ctrl = session.pointer.ctrl();
        let Some(ground) = session.ground_at(end) else {
            return;
        };
        if let Some(sim) = session.map.sim.as_mut() {
            if self.unit_bar.tent_cursor.is_some() {
                opennml_client::game::unit_bar::click_unpack(
                    self.unit_bar,
                    sim,
                    &session.selection.ids,
                    ground,
                );
            } else {
                self.economy
                    .click_place(sim, &session.selection.ids, ground, shift, ctrl);
            }
        }
    }

    /// Left mouse button released in targeting mode: object under the mouse pointer
    /// ([`opennml_client::game::map::MapView::pick_target`]) or terrain point, from that the target
    /// ([`opennml_client::game::unit_bar::ability_target`]), then [`opennml_client::game::unit_bar::click_target`]. The
    /// selection stays; a rejection appears in the status line, the targeting mode stays.
    fn finish_target_click(&mut self, session: &mut GameSession) {
        let Some(end) = session.pointer.cursor else {
            return;
        };
        let Some(mode) = self.unit_bar.target_mode else {
            return;
        };
        let size = session.screen;
        let params = session.pick_params();
        let map = &mut session.map;
        let object = map.pick_target(&params, end, size);
        let ground = map.ground_point(&params, end, size);
        let Some(target) =
            opennml_client::game::unit_bar::ability_target(mode.button, object, ground)
        else {
            return;
        };
        if let Some(sim) = map.sim.as_mut() {
            *self.target_notice =
                opennml_client::game::unit_bar::click_target(self.unit_bar, sim, target).err();
        }
    }
}

/// Open the game session for `config` (start and `start_mission` share the call): map, game rules and match
/// from the configuration, the window size as the view. With `config.random_map` the random map without a
/// simulation together with its state for `random_map`.
fn open_session(
    shell: &WindowShell,
    vfs: &Vfs,
    config: &Config,
) -> anyhow::Result<(GameSession, Option<RandomMapState>)> {
    let gpu = SessionGpu::of(&shell.gpu);
    if let Some(spec) = &config.random_map {
        let (load, state) = random_map_load(vfs, spec)?;
        let session = GameSession::from_load(
            &gpu,
            vfs,
            load,
            &config.view_options(),
            shell.gpu.size(),
            &mut |_| {},
        )?;
        return Ok((session, Some(state)));
    }
    let session = GameSession::start(
        &gpu,
        vfs,
        &config.level,
        &mut |rules| match_setup(config, rules),
        config.game_speed,
        &config.view_options(),
        shell.gpu.size(),
        &mut |_| {},
    )?;
    Ok((session, None))
}

/// Generate random map `spec` (`opennml_randommap::build_level`, the same chain as the game) and prepare it as a
/// scene without a simulation for loading ([`MapLoad::from_scene`]); plus its state for the control command
/// `random_map`. Continue with `MapView::build` or [`GameSession::from_load`]. An error ends the viewer like a
/// missing level.
pub(crate) fn random_map_load(
    vfs: &Vfs,
    spec: &opennml_randommap::RandomMapSpec,
) -> anyhow::Result<(MapLoad, RandomMapState)> {
    let context = || format!("generating random map \"{}\"", spec.seed_text);
    let opennml_randommap::RandomLevel { map, mut scene, .. } =
        opennml_randommap::build_level(vfs, spec).with_context(context)?;
    // The objects of the generator lie at y = 0 (the world does not set the height until snapping); the preview
    // without a simulation lifts them to the terrain height, the scene of the generator stays unchanged.
    for object in &mut scene.objects {
        object.position.y = scene
            .terrain
            .ground_height(object.position.x, object.position.z);
    }
    let state = opennml_client::random_map_state::random_map_state(&map);
    log::info!(
        "Random map \"{}\": {} (size {}), defines {}, hash {}",
        state.seed_text,
        state.script,
        state.dim,
        state.defines.join(","),
        state.hash
    );
    Ok((MapLoad::from_scene(scene), state))
}

/// `--audio-capture`: start the offline mix in the WAV file `path`.
fn begin_audio_capture(audio: &mut AudioEngine, path: &Path) -> anyhow::Result<()> {
    audio
        .start_capture(path)
        .with_context(|| format!("sound recording to {} not possible", path.display()))?;
    log::info!("Sound recording to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unwritable_audio_capture_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut audio = AudioEngine::new(Default::default(), None);
        begin_audio_capture(&mut audio, &dir.path().join("sound.wav")).unwrap();
        assert!(audio.capture_status().is_some());
        let error = begin_audio_capture(&mut audio, &dir.path().join("missing/sound.wav"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("sound recording") && error.contains("missing"),
            "{error}"
        );
    }

    /// `random_map` counts the placed objects per GUID (sorted), names the town centers as start points, and the
    /// preview lifts the objects to the terrain height. Corpus (German edition with patch: seed ZKA, size 128, 4
    /// players = 4 x town center 4200).
    #[test]
    fn random_map_state_lists_objects_per_guid() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let spec = opennml_randommap::RandomMapSpec {
            seed_text: "ZKA".into(),
            size: 1,
            players: 4,
            terrain: 0,
            landscape: 4,
            daytime: 0,
            game_type: 0,
        };
        let (load, state) = random_map_load(&vfs, &spec).unwrap();
        let total: u32 = state.objects.iter().map(|[_, n]| n).sum();
        assert!(total > 0);
        assert!(state.rand_counts[5] > 0);
        assert!(state.objects.windows(2).all(|w| w[0][0] < w[1][0]));
        let centres = state.objects.iter().find(|[g, _]| *g == 4200).unwrap()[1];
        assert_eq!(centres, 4);
        assert_eq!(state.start_points.len(), 4);
        assert_eq!(state.slots_used[..4], [true; 4]);
        // Preview: objects stand on the terrain, not at y = 0.
        let scene = &load.scene;
        assert!(!scene.objects.is_empty());
        for o in &scene.objects {
            assert_eq!(
                o.position.y,
                scene.terrain.ground_height(o.position.x, o.position.z)
            );
        }
        assert!(scene.objects.iter().any(|o| o.position.y != 0.0));
    }
}
