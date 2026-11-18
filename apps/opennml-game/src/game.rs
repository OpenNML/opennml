//! Window application of `opennml-game`: window shell (`WindowShell`), menu host (`MenuHost`) with `start_menu`, sound,
//! virtual clock and the control channel in the subset [`AVAILABLE`] (`state`, `step` and `ui_state` answer with the
//! state of the UI, `opennml_client::ui_control`; with a match additionally the field `game`).
//!
//! One frame: inputs go buffered to the host; the logic step ([`Game::advance`]) advances `MenuApp` by one frame
//! (`MenuHost::frame_cmds`, without pointer, then `apply_effects`, then the sound) and keeps the draw commands; drawing
//! is on black, the pointer last (per context): with `HardwareCursor 1` the TGA, with `HardwareCursor 0` the 3D pointer
//! from `mouse.rdo` (`MenuHost::pointer_draw`, `PointerRenderer`; without a model the TGA as a fallback). The window
//! hides the system pointer in both cases. Above the pointer, without a visible world and without a video, stands the
//! version line (`MenuHost::version_cmds`), in the loading screen always. Screenshots of the control channel draw the
//! same commands without a pointer. With the clock halted the UI continues only with `step`.
//!
//! Match: the game effects of the UI (`HostRequests::game`) go to `opennml_client::game_flow::GameFlow` with a
//! [`LiveSession`] around the `GameSession`; loading blocks and draws the loading screen of the `.ldg` per phase
//! (`LoadingScreen::draw`, with `--loading-shot` additionally one PNG per phase). With a match a frame draws the world
//! (`GameSession::render`), above it the messages, the UI and the pointer (`MenuHost::set_pointer` with
//! `GameFlow::pointer_choice`). ESC, F1 and F4 reach the flow as keys not taken by the UI (`GameFlow::keys`), mouse and
//! remaining keys reach the map only while the game runs (`GameFlow::input`).
//!
//! Videos: the host plays the start sequence and campaign videos from the `.bik` of the installation
//! (`opennml_client::video::open_bink`); their sound runs through the same sound system (silent with `--mute`), the pointer
//! is missing as long as the UI has it off. The program needs no further location for the videos.
//!
//! `MenuApp`/`MenuHost` are not `Send`: they live in the window thread, the control thread only sends messages
//! (`ControlEvent` via the `EventLoopProxy`), as in the viewer.
//!
//! Network play: the menu path (`LAN1`, `LAN3`, `LAN2`) runs via the `NetSession` in the flow
//! (`GameFlow::set_net_session` at window start, with `SystemTransport`, seed from the clock [`net_start::wall_seed`]),
//! pumped per logic step after the menu effects (`GameFlow::net_step`). The command-line path ([`NetLaunch`], without a
//! lobby dialog) pumps in every pass of the event loop, also without a match and before `Go`. As soon as the `Start` of
//! the host is there, the logic step loads the map (`GameFlow::start_net_match`); until `Go` the input is locked in the
//! command-line path (window and control channel), in the menu path only while loading; a failure before the start
//! leads back to the main menu. In the network the control channel rejects `pause` and `step` (the turns and the lobby
//! run in real time) as well as `sim_command`/`sim_result` for foreign slots; `ui_*` are free in search and lobby
//! ([`net_start::net_guard`]); `net_*` is answered by [`net_start::run_net`]. If the host closes the program or the
//! window in the running match, it hands it over to a client; without a reachable client it ends in an orderly way for
//! all ([`Game::net_closing`]).

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use opennml_audio::engine::AudioEngine;
use opennml_audio::volume::Category;
use opennml_client::clock::{Clock, ClockMode};
use opennml_client::control::ControlEvent;
use opennml_client::display::ModeInfo;
use opennml_client::frames::FrameCounter;
use opennml_client::game::hotkeys::Hotkeys;
use opennml_client::game::net_driver::{LEAVE_REASON, NetDriver};
use opennml_client::game::options::{Controls, ViewOptions};
use opennml_client::game::pointer::PointerChoice;
use opennml_client::game::sim_control::{execute_sim, parse_sim_command};
use opennml_client::game_flow::{
    FlowEnv, GameFlow, LiveSession, NetStartFailure, SessionView, controls, view_options,
};
use opennml_client::game_session::{GameSession, LoadPhase, SessionGpu, wheel_up};
use opennml_client::keys::{InputEvent, MouseButton, key_code, key_events};
use opennml_client::menu_host::{HostRequests, MenuHost, PointerDraw, log_game_effect};
use opennml_client::net_session::{NetIdentity, NetSession};
use opennml_client::snapshot::png_data;
use opennml_client::ui_control::{NO_UI, execute_ui, state_data};
use opennml_client::ui_layer::{UiLayer, atlas_generator, screen_of};
use opennml_client::vk::char_of;
use opennml_client::window::{WindowMode, WindowShell};
use opennml_control::{
    Command, GAME_UI_DIALOG, HelloInfo, MouseButtonName, PROTOCOL_VERSION, Request, Response,
    UiStateData,
};
use opennml_formats::engine_ini::EngineConfig;
use opennml_formats::ldg::parse_ldg;
use opennml_formats::text::decode_cp1252;
use opennml_rda::Vfs;
use opennml_render::{
    BackendChoice, CAPTURE_FORMAT, PointerRenderer, UiRenderer, headless_device, render_to_image,
};
use opennml_ui::app::MenuOptions;
use opennml_ui::assets::{Texts, UiAssets};
use opennml_ui::coords::Screen;
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::DrawCmd;
use opennml_ui::fonts::FontBook;
use opennml_ui::loading::LoadingScreen;
use opennml_ui::msg::{DisplayMode, UiEffect};
use opennml_ui::version_line::{VERSION_TEXT, draw_version_line};
use serde_json::Value;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::PhysicalKey;
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, WindowId};

use crate::net_start::{self, CliState, LaunchInfo, NetGuard, NetLaunch};

/// Window title.
const TITLE: &str = "No Man's Land";

/// Smallest window size: `ScreenXSize`/`ScreenYSize` below it (also 0 or negative, the file is taken unchecked,
/// `docs/formats/engine-ini.md`, section "Implementation": values without range check) count as 640 x 480 - no window
/// of size 0, no fonts with 0 px, no division by 0 in the conversion from the grid.
pub const MIN_WINDOW: (u32, u32) = (640, 480);
/// Largest side of the window (limit of common texture sizes).
pub const MAX_WINDOW_SIDE: u32 = 8192;

/// Frame time of the screenshot without a window (`--screenshot`): the menu runs in steps of 16 ms until `--time-ms`,
/// the last step ends exactly there.
const SCREENSHOT_FRAME_MS: u64 = 16;

/// Commands of the control channel of `opennml-game`: `state` and the UI commands `ui_*`, the simulation commands
/// `sim_hash`, `sim_query`, `sim_command`, `sim_result` (only with a running match), the mouse commands `mouse_move`,
/// `mouse_button` and `key`, `ui_present`, `ui_focus_lost` and `random_map`; all others are rejected with an error
/// message.
pub const AVAILABLE: &[&str] = &[
    "hello",
    "pause",
    "resume",
    "step",
    "screenshot",
    "state",
    "ui_state",
    "ui_click",
    "ui_key",
    "ui_open",
    "ui_video",
    "ui_present",
    "ui_focus_lost",
    "mouse_move",
    "mouse_button",
    "key",
    "sim_hash",
    "sim_query",
    "sim_command",
    "sim_result",
    "random_map",
    "net_status",
    "net_part_hashes",
    "net_save",
    "net_debug",
    "quit",
];

/// What the control channel rejects in network play, with an error message: the clock commands `pause` and `step` (the
/// turns run in real time; if one machine halted its clock, all the others would wait), every input until `Go`
/// (`locked`), and `sim_command`/`sim_result` for a slot other than the own `own_slot` (turn mode would otherwise
/// discard them, with only a log entry). `None`: the command runs.
pub fn net_refusal(command: &Command, locked: bool, own_slot: Option<u8>) -> Option<String> {
    let foreign = |player: u8| -> Option<String> {
        let own = own_slot?;
        (player != own).then(|| {
            format!("In network play only commands for your own slot {own} (not slot {player})")
        })
    };
    match command {
        Command::Pause | Command::Step { .. } => Some(
            "In network play the clock runs in real time: \"pause\" and \"step\" are locked"
                .to_string(),
        ),
        Command::UiClick { .. }
        | Command::UiKey { .. }
        | Command::UiOpen { .. }
        | Command::MouseMove { .. }
        | Command::MouseButton { .. }
        | Command::Key { .. }
            if locked =>
        {
            Some("Input locked until the start of the network match".to_string())
        }
        Command::SimCommand { command } => parse_sim_command(command.clone())
            .ok()
            .and_then(|c| foreign(c.player)),
        Command::SimResult { player, .. } => foreign(*player),
        _ => None,
    }
}

/// Window size from `ScreenXSize`/`ScreenYSize`, limited to [`MIN_WINDOW`] ... [`MAX_WINDOW_SIDE`].
pub fn window_size(cfg: &EngineConfig) -> (u32, u32) {
    clamp_size(cfg.screen_x_size, cfg.screen_y_size)
}

fn clamp_size(width: i32, height: i32) -> (u32, u32) {
    let side = |v: i32, min: u32| u32::try_from(v).unwrap_or(0).clamp(min, MAX_WINDOW_SIDE);
    (side(width, MIN_WINDOW.0), side(height, MIN_WINDOW.1))
}

/// Window mode: `--window-mode`, otherwise by `ScreenFormat` (!= 0 window, 0 fullscreen with mode switch like the
/// original, `docs/formats/engine-ini.md`, section "Meaning").
pub fn window_mode(cli: Option<WindowMode>, cfg: &EngineConfig) -> WindowMode {
    cli.unwrap_or(if cfg.screen_format != 0 {
        WindowMode::Windowed
    } else {
        WindowMode::Exclusive
    })
}

/// Display mode of the settings (`ScreenXSize`, `ScreenYSize`, `ScreenDepth`).
fn display_mode_of(cfg: &EngineConfig) -> DisplayMode {
    DisplayMode {
        width: cfg.screen_x_size,
        height: cfg.screen_y_size,
        bits: cfg.screen_depth,
    }
}

/// Video mode for `want`: same width and height, preferably the same color depth, otherwise 32 bit (Windows often
/// rejects 16-bit modes), then the highest frame rate.
pub fn matching_mode(modes: &[ModeInfo], want: DisplayMode) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            i64::from(m.width) == i64::from(want.width)
                && i64::from(m.height) == i64::from(want.height)
        })
        .max_by_key(|(_, m)| {
            (
                i32::from(m.bit_depth) == want.bits,
                m.bit_depth == 32,
                m.refresh_millihertz,
            )
        })
        .map(|(i, _)| i)
}

/// Error message for a command outside [`AVAILABLE`].
pub fn unavailable(command: &Command) -> String {
    let name = serde_json::to_value(command)
        .ok()
        .and_then(|v| v.get("cmd")?.as_str().map(String::from))
        .unwrap_or_else(|| "?".to_string());
    format!(
        "\"{name}\" is not available in opennml-game (available: {})",
        AVAILABLE.join(", ")
    )
}

/// Focus loss of the window (`WindowEvent::Focused(false)`) or `ui_focus_lost`: winit then does not report the
/// release, so the menu host and the match release all held keys (a held key does not repeat afterwards). With `pause`
/// the match switches the pause on like the original (`0x33`, O-154, [`GameSession::focus_lost`]); `true` if it was
/// switched.
fn lose_focus(host: Option<&mut MenuHost>, session: Option<&mut GameSession>, pause: bool) -> bool {
    if let Some(host) = host {
        host.release_keys();
    }
    let Some(session) = session else {
        return false;
    };
    session.pointer.keys.clear();
    session.held_vk.clear();
    pause && session.focus_lost()
}

/// Syncs the table of held keys of the match (`GameSession::held_vk`) with that of the menu host (`FUN_004143A0` reads
/// the one table `0x0066FA60+0x9C` in the original). `opennml-game` calls it before every input to the map, before the key
/// path and before the frame of the match so that `KD_ADD_TO_GROUP` (shift path of the control groups),
/// `KD_CHECK_LBUTTON` and the `KD_` keys of the camera see the current state.
fn sync_held_vk(session: Option<&mut GameSession>, host: &MenuHost) {
    if let Some(session) = session {
        session.held_vk = host.held_vk();
        session.text_focus = host.text_focus();
    }
}

/// `random_map`: state of the random map of the running match ([`GameSession::random_map_state`], same fields as in
/// the viewer), `null` without a match or with a fixed map. Read-only. An error converting to JSON is reported as an
/// error response of the control channel, not swallowed.
fn random_map_value(session: Option<&GameSession>) -> Result<Value, String> {
    serde_json::to_value(session.and_then(GameSession::random_map_state))
        .map_err(|error| format!("state of the random map cannot be represented as JSON: {error}"))
}

/// Message of `sim_command` and `sim_result` at the menu level.
const MENU_LEVEL_REFUSES: &str = "Menu level takes no commands";

/// `sim_hash`, `sim_query`, `sim_command`, `sim_result` on the simulation of the running match ([`execute_sim`], as in
/// the viewer); without a match the message "no match". The session of the menu level lies in the same session as that
/// of a match: `sim_hash` and `sim_query` read it, `sim_command` and `sim_result` are rejected by `menu_level` (the
/// simulation of the menu level belongs to the scripts of `menü2.abl`).
fn run_sim(
    session: &mut Option<GameSession>,
    menu_level: bool,
    command: &Command,
) -> Result<Value, String> {
    if menu_level
        && matches!(
            command,
            Command::SimCommand { .. } | Command::SimResult { .. }
        )
    {
        return Err(MENU_LEVEL_REFUSES.into());
    }
    execute_sim(session.as_mut().and_then(GameSession::sim_mut), command)
}

/// Target of a pointer frame: device, queue, target view and its size.
struct PointerGpu<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    target: &'a wgpu::TextureView,
    size: (u32, u32),
}

/// Draws the pointer last into `gpu.target` (`GameApp_RenderUiAndPresent` `0x004BFE50`): the TGA quad over the UI, the
/// 3D pointer (`Cursor3D_Render` `0x004CCC50`) with the diamonds of the minimap for the depth pre-pass (O-457). Shared
/// by the window (`Game::present`) and the screenshot (`Game::capture`); the `PointerRenderer` gets the real target
/// size and at most one `draw` per `queue.submit`.
fn draw_pointer(
    pointer: Option<PointerDraw>,
    depth_zero: Vec<[i32; 4]>,
    (layer, renderer, pointer_renderer): (&mut UiLayer, &mut UiRenderer, &mut PointerRenderer),
    vfs: &Vfs,
    encoder: &mut wgpu::CommandEncoder,
    gpu: &PointerGpu<'_>,
) {
    let PointerGpu {
        device,
        queue,
        target,
        size,
    } = *gpu;
    match pointer {
        Some(PointerDraw::Sprite(quad)) => {
            layer.draw(renderer, device, queue, encoder, target, size, &[quad]);
        }
        Some(PointerDraw::Model(mut scene)) => {
            scene.depth_zero = depth_zero;
            pointer_renderer.draw(device, queue, encoder, target, size, vfs, &scene);
        }
        None => {}
    }
}

/// The draw commands over the world of a frame of size `size` ([`GameSession::over_world_cmds`], the same for window
/// and capture): the combat display (health bars, rank and counter-upgrade icons; in the original step 13 of the world
/// pass), above it messages, game windows with the cinemascope area, tooltip and text messages, the scene fade 145/146
/// and finally the info windows 168/213 as first children of the desktop (re/campaign-ui D6.4; the menu is drawn above
/// by the caller). After `GameSession::render` of the same frame so that the bars sit at the upper edges of this frame.
/// `present` calls this in every frame with a match (also in pause and game menu) with the one layer of the host: the
/// info windows measure themselves in the frame of their event and keep its texture IDs.
fn over_world(
    session: &GameSession,
    layer: &mut UiLayer,
    vfs: &Vfs,
    size: (u32, u32),
) -> Vec<DrawCmd> {
    let mut ctx = layer.context(vfs, screen_of(size));
    let texts = ctx.texts;
    session.over_world_cmds(&mut ctx, texts)
}

/// Black, opaque background (the menu draws over the existing image).
fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Menu background"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    }));
}

/// Volumes of the settings to the sound system (`m_Volumes`: music, sound effects, speech).
fn apply_volumes(audio: &mut AudioEngine, cfg: &EngineConfig) {
    for (category, volume) in [Category::Music, Category::Sfx, Category::Speech]
        .into_iter()
        .zip(cfg.sound.volumes)
    {
        audio.set_volume(category, volume);
    }
}

/// Host of the screenshot: main menu on `layer`, settings only in memory (location `None`) - `WriteSettings` and
/// `shutdown` structurally write nothing. `sound_drivers` fills the driver list (`SOUNDOPTIONS`).
fn screenshot_host(
    vfs: &Vfs,
    layer: UiLayer,
    config: EngineConfig,
    audio: &AudioEngine,
    sound_drivers: Vec<String>,
) -> Result<MenuHost> {
    let opts = MenuOptions {
        start_menu: true,
        sound_drivers,
        ..MenuOptions::default()
    };
    MenuHost::new(vfs, layer, config, opts, audio, None)
}

/// `--screenshot`: run the menu without a window and without sound output in steps of 16 ms until `time_ms`, save the
/// last frame (without pointer) on black in the window size as PNG. Writes no settings ([`screenshot_host`]); `config`
/// comes from the settings file.
pub fn screenshot(vfs: &Vfs, config: EngineConfig, time_ms: f32, out: &Path) -> Result<()> {
    let (device, queue, _info) = headless_device(BackendChoice::Auto)?;
    let size = window_size(&config);
    let max = device.limits().max_texture_dimension_2d;
    if size.0 > max || size.1 > max {
        bail!(
            "Image size {} x {} invalid for the screenshot (allowed: up to {max} per side)",
            size.0,
            size.1
        );
    }
    let mut audio = AudioEngine::open(vfs, false);
    apply_volumes(&mut audio, &config);
    let generator = atlas_generator(vfs);
    let layer = UiLayer::new(vfs, size.0, generator.as_deref()).context("loading the menu")?;
    // Without sound output, but with the driver list of the system as in the original.
    let drivers = opennml_audio::driver::names(&opennml_audio::driver::enumerate());
    let mut host = screenshot_host(vfs, layer, config, &audio, drivers)?;
    let screen = screen_of(size);
    // `as` saturates; `time_ms` is finite and >= 0 (`parse_time_ms`).
    let end = time_ms.floor() as u64;
    let mut now = 0;
    let cmds = loop {
        let cmds = host.frame_cmds(vfs, screen, now);
        let req = host.apply_effects(&mut audio, vfs);
        if req.quit || now >= end {
            break cmds;
        }
        now = (now + SCREENSHOT_FRAME_MS).min(end);
    };
    // Without world and video: with the version line.
    let version = host.version_cmds(screen, false);
    let mut renderer = UiRenderer::new(&device, CAPTURE_FORMAT);
    let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
        clear(encoder, view);
        host.layer
            .draw(&mut renderer, &device, &queue, encoder, view, size, &cmds);
        host.layer.draw(
            &mut renderer,
            &device,
            &queue,
            encoder,
            view,
            size,
            &version,
        );
    })?;
    img.save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    log::info!("Screenshot {} saved ({now} ms)", out.display());
    Ok(())
}

/// Menu UI of the window: start sequence like the original (`IsIntro`: start videos, then `start`). A network start by
/// command line (`--host`/`--join`, `net_start`) skips the start videos like a direct start (E-295): otherwise they
/// would run over the match from `Go` (closing the menus at the network start does not remove queued commands), and
/// any key would only abort a video. The commands are therefore not queued at all. `IsIntro` stays in the settings.
///
/// `sound_drivers` is the driver list of the sound system (`SOUNDOPTIONS`).
fn window_menu_options(
    display_modes: Vec<DisplayMode>,
    sound_drivers: Vec<String>,
    net_start: bool,
) -> MenuOptions {
    MenuOptions {
        display_modes,
        sound_drivers,
        start_menu: true,
        skip_intro: net_start,
        ..MenuOptions::default()
    }
}

/// Start values from the command line.
#[derive(Debug, Clone)]
pub struct GameOptions {
    pub mode: WindowMode,
    pub monitor: Option<usize>,
    pub mute: bool,
    /// Gamma ramp in the window image (on by default; `--no-gamma` switches it off).
    pub gamma: bool,
    /// Clock halted at 0 ms (`--paused`).
    pub paused: bool,
    /// Check runs (`--loading-shot`): one PNG per loading phase in this folder.
    pub loading_shot: Option<PathBuf>,
    /// A control channel is active (`--control`): then no pause on focus loss (deviation from the original, O-154).
    pub control: bool,
    /// Transport of the network session of the menu path (`--net-loopback`, discovery port 2303).
    pub transport: opennml_net::SystemTransport,
    /// The client of the menu path listens on a free port from the join onward (`--net-no-listen` switches it off).
    pub net_listen: bool,
}

/// Command-line paths without a menu level (`--host`, `--join`, `--screenshot`): they start without a main menu or only
/// with one image, no menu level runs behind them. `IsMenuLevel` goes off only in memory; the value of the settings
/// file is returned if it was on (for [`MenuHost::keep_saved_menu_level`] so that the file stays unchanged), otherwise
/// `None`.
pub fn skip_menu_level(config: &mut EngineConfig) -> Option<bool> {
    std::mem::replace(&mut config.is_menu_level, false).then_some(true)
}

/// After a load (`GameFlow::just_loaded`, from `started`): the loading time does not count as frame time (no catching
/// up). The exception is loading the menu level (`GameFlow::load_time_counts_for_menus`, re/menu-level ML.15 no. 1): in
/// the original the frames of the menus tick on `GetTickCount` (`FUN_004615A0`), the synchronous loading runs into the
/// fly-in of `START`, which then immediately stands at the target. In real time the program clock therefore advances by
/// the loading time (beyond the upper limit of the frame time); the first frame of the session already comes at this
/// clock. With the clock halted (control channel) it stays with discarding so that runs with `step` stay repeatable.
fn after_load(clock: &mut Clock, flow: &GameFlow, started: Instant) {
    if flow.load_time_counts_for_menus() && clock.mode() == ClockMode::Realtime {
        clock.advance(started.elapsed().as_secs_f32() * 1000.0);
    }
    clock.tick(Instant::now());
}

/// Reads the `.ldg` (cp1252); if it is missing or unreadable, an empty loading screen (black) with a warning.
fn read_ldg(vfs: &Vfs, path: &str) -> opennml_formats::ldg::Ldg {
    match vfs.read(path) {
        Ok(bytes) => parse_ldg(&decode_cp1252(&bytes)),
        Err(e) => {
            log::warn!("Loading screen \"{path}\" not readable: {e}");
            parse_ldg("")
        }
    }
}

/// The match behind `GameFlow` for this frame: copies of the device handles of the window (the window stays free for
/// the loading screen), session, view and sound system of the program. `GameFlow` rewrites view and camera control from
/// the settings (`SessionView::apply_settings`, per frame and before every load).
fn live<'a>(
    shell: &WindowShell,
    vfs: &'a Vfs,
    session: &'a mut Option<GameSession>,
    view: &'a mut ViewOptions,
    controls: &'a mut Controls,
    audio: &'a mut AudioEngine,
) -> LiveSession<'a> {
    LiveSession {
        device: shell.gpu.device.clone(),
        queue: shell.gpu.queue.clone(),
        format: shell.gpu.surface_format(),
        vfs,
        session,
        view,
        controls,
        size: shell.gpu.size(),
        audio: Some(audio),
    }
}

/// Draws the loading screen per phase (`LoadingScreen_RenderFrame`, re/singleplayer-menus 6.3): image, bar and texts of
/// the `.ldg` on black, without pointer (`FUN_004CD6A0(0)` while loading), then `present()`; with `shot` additionally
/// `loading-<load>-<percent>.png`.
struct LoadingPainter<'a> {
    shell: &'a mut WindowShell,
    renderer: &'a mut UiRenderer,
    assets: &'a mut UiAssets,
    fonts: &'a FontBook,
    texts: &'a Texts,
    vfs: &'a Vfs,
    frames: &'a mut FrameCounter,
    shot: Option<(&'a Path, u32)>,
    screen: Option<(String, LoadingScreen)>,
}

impl LoadingPainter<'_> {
    fn paint(&mut self, ldg: &str, phase: LoadPhase) {
        let size = self.shell.gpu.size();
        let mut ctx = UiContext {
            vfs: self.vfs,
            assets: &mut *self.assets,
            fonts: self.fonts,
            texts: self.texts,
            screen: screen_of(size),
        };
        if self.screen.as_ref().is_none_or(|(path, _)| path != ldg) {
            let loading = LoadingScreen::new(read_ldg(self.vfs, ldg), &mut ctx);
            self.screen = Some((ldg.to_string(), loading));
        }
        let mut cmds = match &self.screen {
            Some((_, loading)) => loading.draw(phase.percent(), &ctx),
            None => Vec::new(),
        };
        // `LoadingScreen_RenderFrame` draws the version line in every frame (`FUN_004BFCC0`).
        draw_version_line(VERSION_TEXT, ctx.screen, self.fonts, &mut cmds);
        let (renderer, assets) = (&mut *self.renderer, &*self.assets);
        if !self.shell.minimized() {
            let outcome = self
                .shell
                .gpu
                .render(|device, queue, encoder, target, size| {
                    clear(encoder, target);
                    renderer.draw(device, queue, encoder, target, size, assets, &cmds);
                });
            self.shell.finish_frame(outcome, self.frames);
        }
        let Some((dir, load)) = self.shot else {
            return;
        };
        let gpu = &self.shell.gpu;
        let image = render_to_image(
            &gpu.device,
            &gpu.queue,
            size,
            gpu.surface_format(),
            |encoder, view| {
                clear(encoder, view);
                renderer.draw(&gpu.device, &gpu.queue, encoder, view, size, assets, &cmds);
            },
        );
        let path = dir.join(format!("loading-{load:02}-{:03}.png", phase.percent()));
        match image.map_err(|e| format!("{e:#}")).and_then(|img| {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            img.save(&path).map_err(|e| e.to_string())
        }) {
            Ok(()) => log::info!("Loading screen {} saved", path.display()),
            Err(e) => log::warn!("Loading screen {} not saved: {e}", path.display()),
        }
    }
}

/// First free file `opennml-shot-NNNN.png` (0000 ... 0999) in `dir`, as the original searches its names (`FUN_004BD940`:
/// first number from 0000 whose file cannot be opened; gaps are filled). If all are taken, none (the original would
/// then stay in the folder `ScreenShots`; not reproduced).
fn free_shot_path(dir: &Path) -> Option<PathBuf> {
    (0..1000)
        .map(|n| dir.join(format!("opennml-shot-{n:04}.png")))
        .find(|path| !path.exists())
}

/// The window application.
pub struct Game {
    vfs: Vfs,
    settings: PathBuf,
    /// Settings until the host is built in `resumed`.
    config: Option<EngineConfig>,
    /// Saved value of `IsMenuLevel` of the command-line path ([`skip_menu_level`]), for the host.
    saved_menu_level: Option<bool>,
    opts: GameOptions,
    audio: AudioEngine,
    clock: Clock,
    frames: FrameCounter,
    shell: Option<WindowShell>,
    host: Option<MenuHost>,
    renderer: Option<UiRenderer>,
    /// Renderer of the 3D pointer (`HardwareCursor 0`), created next to the `renderer`.
    pointer_renderer: Option<PointerRenderer>,
    /// Draw commands of the last logic step, without pointer.
    cmds: Vec<DrawCmd>,
    /// Control commands that came before `resumed` (except `hello`).
    waiting: Vec<ControlEvent>,
    /// Quit via the control channel (`quit`).
    quit_requested: bool,
    /// The UI has demanded `0x25` (quit).
    quit_pending: bool,
    /// The middle button is pressed on the mouse of the user (winit, not the control channel): only then does
    /// `KD_CAM_FREEHAND` reset the mouse pointer of the system (quirk O-331).
    user_middle: bool,
    /// Settings written at program end.
    shut_down: bool,
    error: Option<anyhow::Error>,
    /// Flow of the match.
    flow: GameFlow,
    /// The running match.
    session: Option<GameSession>,
    /// View and camera control of a match: at window start from the settings (font width according to O-58),
    /// afterwards `GameFlow` rewrites them per frame and before every load (`SessionView::apply_settings`).
    view_options: ViewOptions,
    controls: Controls,
    /// Counted effect rounds with game effects (names of the images of `--loading-shot`).
    loads: u32,
    /// Network start by command line: driver until loading, listener, plan of the host.
    net: Option<NetLaunch>,
    /// Host closes in the running network match: it waits that long for the handover or end bundle
    /// ([`NetDriver::close_program`]).
    closing: bool,
    /// Wall clock for the closing deadline ([`NetDriver::close_program`]), independent of the program clock.
    epoch: Instant,
}

impl Game {
    pub fn new(vfs: Vfs, config: EngineConfig, settings: PathBuf, opts: GameOptions) -> Self {
        // Device from `m_DriverID` like `SoundManager_Serialize` when loading (`CreateInterface`, `InitDevice`); if it
        // is missing, the default device plays (D-372).
        let mut audio = AudioEngine::open_with_driver(&vfs, Some(config.sound.driver_id));
        apply_volumes(&mut audio, &config);
        audio.set_mute(opts.mute);
        let clock = if opts.paused {
            Clock::paused()
        } else {
            Clock::new()
        };
        Self {
            vfs,
            settings,
            config: Some(config),
            saved_menu_level: None,
            opts,
            audio,
            clock,
            frames: FrameCounter::new(Instant::now()),
            shell: None,
            host: None,
            renderer: None,
            pointer_renderer: None,
            cmds: Vec::new(),
            waiting: Vec::new(),
            quit_requested: false,
            quit_pending: false,
            user_middle: false,
            shut_down: false,
            error: None,
            flow: GameFlow::default(),
            session: None,
            view_options: ViewOptions::default(),
            controls: Controls::default(),
            loads: 0,
            net: None,
            closing: false,
            epoch: Instant::now(),
        }
    }

    /// Remembers the saved `IsMenuLevel` of the command-line path ([`skip_menu_level`]); `None` changes nothing.
    pub fn set_saved_menu_level(&mut self, saved: Option<bool>) {
        self.saved_menu_level = saved;
    }

    /// Network start by command line, created before the event loop.
    pub fn set_net(&mut self, launch: NetLaunch) {
        self.net = Some(launch);
    }

    /// Driver of the network session: in the network start (until loading), in the match or in the lobby of the menu
    /// path.
    fn net_driver(&self) -> Option<&NetDriver> {
        self.net
            .as_ref()
            .and_then(NetLaunch::driver)
            .or_else(|| self.session.as_ref().and_then(|s| s.net.as_ref()))
            .or_else(|| self.flow.net_session().and_then(NetSession::driver))
    }

    /// Locks in the network ([`net_start::net_guard`]): command-line path until `Go`, menu path while loading;
    /// `pause`/`step` in every network session. A driver of the match that failed before `Go` does not lock
    /// ([`net_start::match_state`]): it then reports `NetFinish::Failed`, the flow shows the message, and its "OK"
    /// leads to the main menu.
    fn net_guard(&self) -> NetGuard {
        let cli = self
            .net
            .as_ref()
            .map_or_else(CliState::default, |l| CliState {
                waiting: l.waiting(),
                handed: l.handed(),
            });
        let match_started =
            net_start::match_state(self.session.as_ref().and_then(|s| s.net.as_ref()));
        let session = self.flow.net_session().map(NetSession::phase);
        net_start::net_guard(cli, match_started, session)
    }

    /// Input locked: in the command-line path until `Go` (main menu or the loaded map without input), in the menu path
    /// while loading.
    fn input_locked(&self) -> bool {
        self.net_guard().locked
    }

    /// `net_status`: from the driver of the match (both paths), otherwise from the command-line path (waiting or
    /// failed), otherwise from the network session of the menu path (`NetSessionStatus`); without all of these `None`
    /// ("no network session").
    fn net_status_data(&self) -> Option<opennml_control::NetStatusData> {
        let session = self.session.as_ref();
        let steps = session.and_then(GameSession::sim).map(|s| s.state.steps);
        let source = match session.and_then(|s| s.net.as_ref()) {
            Some(driver) => Some(net_start::driver_source(
                driver,
                LaunchInfo::of(self.net.as_ref().filter(|l| l.handed())),
            )),
            None => self.net.as_ref().and_then(NetLaunch::source).or_else(|| {
                let net = self.flow.net_session()?;
                net_start::session_source(net.status(), net.driver())
            }),
        };
        source.map(|src| net_start::status_data(&src, steps))
    }

    /// Flow with view, storage and loading screen (like [`Self::run_game_effects`]): `f` gets flow, UI, view,
    /// environment and the painter of the loading screen. Afterwards, after a load, held keys are released and the
    /// loading time does not count as frame time. `None` before the window or without a settings file.
    fn with_flow<R>(
        &mut self,
        f: impl FnOnce(
            &mut GameFlow,
            &mut opennml_ui::app::MenuApp,
            &mut dyn SessionView,
            &FlowEnv,
            &mut dyn FnMut(&str, LoadPhase),
        ) -> R,
    ) -> Option<R> {
        let (Some(host), Some(shell), Some(renderer)) = (
            self.host.as_mut(),
            self.shell.as_mut(),
            self.renderer.as_mut(),
        ) else {
            return None;
        };
        let files = host.save_files().cloned()?;
        let mut view = live(
            shell,
            &self.vfs,
            &mut self.session,
            &mut self.view_options,
            &mut self.controls,
            &mut self.audio,
        );
        let env = FlowEnv {
            vfs: &self.vfs,
            texts: &host.layer.texts,
            files: &files,
        };
        let mut painter = LoadingPainter {
            shell,
            renderer,
            assets: &mut host.layer.assets,
            fonts: &host.layer.fonts,
            texts: &host.layer.texts,
            vfs: &self.vfs,
            frames: &mut self.frames,
            shot: self
                .opts
                .loading_shot
                .as_deref()
                .map(|dir| (dir, self.loads + 1)),
            screen: None,
        };
        let loading_started = Instant::now();
        let result = f(
            &mut self.flow,
            &mut host.app,
            &mut view,
            &env,
            &mut |ldg, phase| painter.paint(ldg, phase),
        );
        // Only a load counts as an effect round (names of the images of `--loading-shot`).
        if painter.screen.is_some() {
            self.loads += 1;
        }
        if self.flow.just_loaded() {
            host.release_keys();
            after_load(&mut self.clock, &self.flow, loading_started);
        }
        if self.flow.take_menus_closed() {
            self.cmds.clear();
        }
        Some(result)
    }

    /// Pumps the network in every pass of the event loop, independent of clock mode and match.
    fn pump_net(&mut self) {
        let now = self.now();
        if let Some(launch) = self.net.as_mut() {
            launch.pump(now);
        }
        if let Some(net) = self.session.as_mut().and_then(|s| s.net.as_mut()) {
            net.pump(now);
        }
    }

    /// Network in the logic step. Menu path: `GameFlow::net_step` (search, connecting, lobby, state to the UI, loading
    /// at the client) runs AFTER the menu effects of this step (`run_game_effects` -> `GameFlow::apply_all`, before it
    /// in [`Self::advance`]): so a start order `{0x0D}` of the host and the lobby state from which
    /// `NetSession::start_host_match` starts fit together (order as in the tests of the network session). Command-line
    /// path: pump (the host distributes the start), after the `Start` load the map, the state into the log; a failure
    /// before the start leads back to the main menu ([`NetLaunch::failure`], no program end).
    fn net_step(&mut self) {
        let now = self.now();
        self.with_flow(|flow, app, view, env, present| flow.net_step(now, app, view, env, present));
        let Some(launch) = self.net.as_mut() else {
            return;
        };
        launch.pump(now);
        if let Some(driver) = launch.take_ready() {
            self.load_net_match(driver, now);
        }
        let session_net = self.session.as_ref().and_then(|s| s.net.as_ref());
        if let Some(launch) = self.net.as_mut() {
            launch.log_state(session_net);
        }
    }

    /// Loads the network match of the command-line path with the loading screen (`GameFlow::start_net_match`). An error
    /// goes out as `Loaded { error }` (pumped immediately, the host then aborts the start for all); afterwards the
    /// network start is closed and the main menu open (no program end).
    fn load_net_match(&mut self, driver: NetDriver, now: u64) {
        let mut slot = Some(driver);
        let result = self.with_flow(|flow, app, view, env, present| {
            let driver = slot.take().expect("driver for loading");
            flow.start_net_match(driver, app, view, env, present)
        });
        let Some(launch) = self.net.as_mut() else {
            return;
        };
        match result {
            Some(Ok(())) => {}
            Some(Err(failure)) => {
                let NetStartFailure { error, mut net } = *failure;
                net.pump(now);
                launch.load_failed(net, format!("Network match not loaded: {error}"));
            }
            // Before the window or without a settings file: no loading possible.
            None => {
                if let Some(mut driver) = slot.take() {
                    driver.loaded(Some("Network play without a settings file".into()));
                    driver.pump(now);
                    launch.load_failed(driver, "Network play needs a settings file".into());
                }
            }
        }
    }

    /// If the host closes the program or the window while the match runs, it hands it over to a client (without a
    /// reachable client it ends for all) and keeps computing until the bundle is executed or the closing deadline has
    /// passed; only then does the program end (`shutdown` -> `leave`). The decision and the waiting are in
    /// [`NetDriver::close_program`] (clock: wall clock since the window start). `true` as long as it waits. Clients, an
    /// abort with an error and every other situation end immediately.
    fn net_closing(&mut self) -> bool {
        if self.error.is_some() {
            return false;
        }
        let speed = self
            .session
            .as_ref()
            .and_then(GameSession::sim)
            .map_or(0, |sim| sim.game_speed);
        // The wall clock runs from the window start; beyond `u64::MAX` ms (never reached) it stands still.
        let now_ms = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        let waiting = self
            .session
            .as_mut()
            .and_then(|s| s.net.as_mut())
            .is_some_and(|net| net.close_program(speed, now_ms));
        self.closing = waiting;
        if waiting {
            self.quit_pending = true;
        }
        waiting
    }

    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Error that led to quitting.
    pub fn into_result(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    /// Program end: write the settings like the destructor `FUN_00496E50` (O-32), once.
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        // The program end ends a running recording with the last state of the command log (E-337); the session can be
        // without a window, hence only the simulation.
        self.flow
            .end_recording(self.session.as_ref().and_then(GameSession::sim));
        // Program end and closed window leave a network match with `Bye`; the host counts that like a dropout, for the
        // clients the host leaving is an abort. The host has handed over a running match beforehand or ended it in an
        // orderly way ([`Self::net_closing`]). Before loading, the driver is still in the network start.
        if let Some(net) = self.session.as_mut().and_then(|s| s.net.as_mut()) {
            net.leave(LEAVE_REASON);
        }
        if let Some(net) = self.net.as_mut().and_then(NetLaunch::driver_mut) {
            net.leave(LEAVE_REASON);
        }
        // Menu path: leave the lobby or the join (`Bye`, the host sends `HOST_LEFT` to all).
        if let Some(net) = self.flow.net_session_mut() {
            net.close();
        }
        if let Some(host) = &mut self.host {
            host.shutdown();
            if let Some(path) = host.settings_path() {
                log::info!("Settings written: {}", path.display());
            }
        }
    }

    fn exit(&mut self, event_loop: &ActiveEventLoop) {
        if self.net_closing() {
            return;
        }
        self.shutdown();
        event_loop.exit();
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        log::error!("{error:#}");
        self.error = Some(error);
        event_loop.exit();
    }

    /// Time of the UI in whole milliseconds.
    fn now(&self) -> u64 {
        // `as` saturates; the clock is >= 0.
        self.clock.now_ms() as u64
    }

    /// State of the UI at the current clock (`state`, `step`, `ui_state`), with a match including the field `game`
    /// (`GameFlow::game_data`).
    fn ui_state(&self) -> Result<UiStateData, String> {
        let host = self.host.as_ref().ok_or(NO_UI)?;
        let mut state = state_data(&host.app, self.screen(), self.now());
        let session = self.session.as_ref();
        // The input with the pointer the host draws (set in the frame step from `GameFlow::pointer_choice`), its
        // visibility and the setting `GameSpeed`.
        let input = session.map(|s| {
            s.input_data(
                host.pointer_choice(),
                !host.app.pointer_hidden(),
                host.app.config().game_speed,
            )
        });
        state.game = self.flow.game_data(
            &host.app,
            session.and_then(GameSession::sim),
            session.map_or("", GameSession::level),
            session.and_then(|s| s.hud_state(&host.layer.texts)),
            input,
            session.map(|s| s.presentation_state(!host.app.pointer_hidden())),
            session.and_then(GameSession::minimap_state),
            session.map_or_else(Vec::new, GameSession::chat_lines),
        );
        // The menu level in the same session (only without a match, `game` stays that of the match).
        state.menu_level = self
            .flow
            .menu_level_data(session.and_then(GameSession::sim));
        // The 3D pointer, also in the main menu; read-only (`pointer_model_data` stores nothing).
        state.pointer_model = host.pointer_model_report(self.clock.now_f32());
        // State of the world display of the map (read-only).
        if let Some(game) = state.game.as_mut() {
            game.world = session.and_then(|s| s.map.world_display());
        }
        Ok(state)
    }

    fn screen(&self) -> Screen {
        screen_of(self.shell.as_ref().map_or((0, 0), |s| s.gpu.size()))
    }

    /// Creates window, display mode, UI and renderer.
    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let config = self.config.take().context("settings are missing")?;
        let size = window_size(&config);
        let shell = WindowShell::create(
            event_loop,
            TITLE,
            size,
            BackendChoice::Auto,
            self.opts.monitor,
            self.opts.mode,
        )?;
        // Size in pixels like `ScreenXSize`/`ScreenYSize` (not in logical pixels of the display scaling).
        if shell.mode == WindowMode::Windowed {
            let _ = shell
                .window
                .request_inner_size(PhysicalSize::new(size.0, size.1));
        }
        let mut shell = shell;
        // Exclusive goes straight into the video mode of the settings (one mode switch); only without a matching mode
        // the best mode of the monitor (`WindowShell::apply_mode`).
        let exclusive = shell.mode == WindowMode::Exclusive && {
            let monitor = event_loop.available_monitors().nth(shell.monitor_index);
            set_exclusive(&shell, monitor, display_mode_of(&config))
        };
        if !exclusive {
            shell.apply_mode(event_loop);
        }
        // The drawn pointer (TGA or 3D) replaces the system pointer (re/menu-system 12.9).
        shell.window.set_cursor_visible(false);
        // Gamma ramp like `App_InitInstance` after creating the device (`0x0042BEBF`). The original sets it again on
        // mode switch and activation; the postprocessing keeps it anyway.
        shell.gpu.set_gamma_enabled(self.opts.gamma);
        shell.gpu.set_gamma(config.gamma);
        // Fonts once with the width from the settings (O-58, like the original with `ScreenXSize`).
        let generator = atlas_generator(&self.vfs);
        let layer =
            UiLayer::new(&self.vfs, size.0, generator.as_deref()).context("loading the menu")?;
        let opts = window_menu_options(
            MenuHost::raw_display_modes(&shell.window),
            self.audio.sound_drivers(),
            self.net.is_some(),
        );
        self.view_options = view_options(&config, size.0);
        self.controls = controls(&config);
        self.flow = GameFlow::new(&layer.texts);
        self.flow.set_hotkeys(Hotkeys::load(&self.vfs));
        // Network session of the menu path: transport over `std::net`, own sign-on with build ID (the session sets the
        // name from `NetName`), game port from `NetPort`, seed from the clock.
        self.flow.set_net_session(NetSession::new(
            Box::new(self.opts.transport),
            NetIdentity {
                hello: net_start::own_hello(&self.vfs, ""),
                port: net_start::host_port(None, config.net_port),
                seed: Box::new(net_start::wall_seed),
                listen: self.opts.net_listen,
            },
            net_start::net_paths(&self.settings),
        ));
        let mut host = MenuHost::new(
            &self.vfs,
            layer,
            config,
            opts,
            &self.audio,
            Some(self.settings.clone()),
        )?;
        self.renderer = Some(UiRenderer::new(
            &shell.gpu.device,
            shell.gpu.surface_format(),
        ));
        // The 3D pointer draws into the same target as the UI (image or intermediate image of the gamma ramp, both in
        // the surface format); as with the `UiRenderer` no rebuild on a format change.
        self.pointer_renderer = Some(PointerRenderer::new(
            &shell.gpu.device,
            &shell.gpu.queue,
            shell.gpu.surface_format(),
        ));
        if let Some(saved) = self.saved_menu_level {
            host.keep_saved_menu_level(saved);
        }
        self.host = Some(host);
        self.shell = Some(shell);
        Ok(())
    }

    /// One frame in real-time operation: logic with the frame time of the clock, then draw; when halted only draw.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let dt_ms = self.clock.tick(Instant::now());
        if self.clock.mode() == ClockMode::Realtime {
            self.advance(dt_ms);
        }
        self.present();
        if self.flow.take_hardcopy() {
            self.write_hardcopy();
        }
        if self.quit_pending {
            self.exit(event_loop);
        }
    }

    /// Logic step: clock forward, one frame of the UI (keep commands without pointer), the untaken keys to the flow,
    /// the effects, the game effects (loading with loading screen), the steps of the match (after `SaveGame`), the
    /// sound.
    fn advance(&mut self, dt_ms: f32) {
        self.clock.advance(dt_ms);
        let screen = self.screen();
        let now = self.now();
        let Some(host) = &mut self.host else {
            return;
        };
        self.cmds = host.frame_cmds(&self.vfs, screen, now);
        // The untaken keys to the key path of the match (`FUN_004C6050`), also at the menu level (only the screenshot
        // takes effect there); in the plain menu they lapse (the screenshot in the main menu is missing).
        let keys = self.flow.running() || self.flow.menu_level().is_some();
        match (self.shell.as_ref(), keys) {
            (Some(shell), true) => {
                // Before the key path: `KD_ADD_TO_GROUP` and `KD_CHECK_LBUTTON` would otherwise see the state of the
                // previous frame.
                sync_held_vk(self.session.as_mut(), host);
                let mut view = live(
                    shell,
                    &self.vfs,
                    &mut self.session,
                    &mut self.view_options,
                    &mut self.controls,
                    &mut self.audio,
                );
                self.flow.keys(&mut host.app, &mut view, now);
            }
            _ => drop(host.app.take_untaken_keys()),
        }
        let mut req = host.apply_effects(&mut self.audio, &self.vfs);
        let game = std::mem::take(&mut req.game);
        if !game.is_empty() {
            self.run_game_effects(game);
        }
        self.net_step();
        // The clock after the game effects: after loading the menu level it is ahead by the loading time
        // (`after_load`), the first frame of the session then has frame time 0; otherwise the same time as above.
        self.step_session(self.now());
        self.audio.advance(&self.vfs, dt_ms);
        self.handle_requests(req);
    }

    /// Game effects to the flow; a load draws the loading screen per phase. Afterwards held keys are released, and the
    /// loading time does not count as frame time (no catching up).
    fn run_game_effects(&mut self, effects: Vec<UiEffect>) {
        let (Some(host), Some(shell), Some(renderer)) = (
            self.host.as_mut(),
            self.shell.as_mut(),
            self.renderer.as_mut(),
        ) else {
            return;
        };
        let Some(files) = host.save_files().cloned() else {
            // Without a settings file there is no storage (only `--screenshot`, without a match).
            effects.iter().for_each(log_game_effect);
            return;
        };
        let mut view = live(
            shell,
            &self.vfs,
            &mut self.session,
            &mut self.view_options,
            &mut self.controls,
            &mut self.audio,
        );
        let env = FlowEnv {
            vfs: &self.vfs,
            texts: &host.layer.texts,
            files: &files,
        };
        self.loads += 1;
        let mut painter = LoadingPainter {
            shell,
            renderer,
            assets: &mut host.layer.assets,
            fonts: &host.layer.fonts,
            texts: &host.layer.texts,
            vfs: &self.vfs,
            frames: &mut self.frames,
            shot: self
                .opts
                .loading_shot
                .as_deref()
                .map(|dir| (dir, self.loads)),
            screen: None,
        };
        let loading_started = Instant::now();
        self.flow.apply_all(
            effects,
            &mut host.app,
            &mut view,
            &env,
            &mut |ldg, phase| painter.paint(ldg, phase),
        );
        if self.flow.just_loaded() {
            host.release_keys();
            after_load(&mut self.clock, &self.flow, loading_started);
            if self.flow.menu_level().is_some() {
                // The commands from before loading still show `START` in its fly-in (clock before the jump by the
                // loading time): regenerate at the new clock so that the first frame after loading shows `START` at the
                // target.
                let screen = screen_of(shell.gpu.size());
                let now = self.clock.now_ms() as u64;
                self.cmds = host.frame_cmds(&self.vfs, screen, now);
            }
        }
        // The menu commands of this logic step were created before `Game_StartLevel` and still showed the background
        // image that step 2 removed (D-255); the menu draws again from the next logic step.
        if self.flow.take_menus_closed() {
            self.cmds.clear();
        }
    }

    /// One frame of the match (`GameFlow::frame`: steps only while the game runs, ESC flag).
    fn step_session(&mut self, now: u64) {
        let (Some(host), Some(shell)) = (self.host.as_mut(), self.shell.as_ref()) else {
            return;
        };
        if !self.flow.running() {
            // Without a match only menus: pointer 7/0, also over the menu level, whose frame `GameFlow::frame`
            // computes.
            host.set_pointer(PointerChoice::NORMAL);
            if self.flow.menu_level().is_some() {
                let mut view = live(
                    shell,
                    &self.vfs,
                    &mut self.session,
                    &mut self.view_options,
                    &mut self.controls,
                    &mut self.audio,
                );
                self.flow
                    .frame(now, &mut host.app, &mut view, &host.layer.texts);
            }
            return;
        }
        // The `KD_` keys of the camera query the table of held keys (`FUN_004143A0`). Once more after the game effects:
        // a load releases all keys (`release_keys`).
        sync_held_vk(self.session.as_mut(), host);
        let mut view = live(
            shell,
            &self.vfs,
            &mut self.session,
            &mut self.view_options,
            &mut self.controls,
            &mut self.audio,
        );
        self.flow
            .frame(now, &mut host.app, &mut view, &host.layer.texts);
        // Pointer per context: that of the match, with an open menu or dialog 7/0.
        let choice = self.flow.pointer_choice(&host.app, &view);
        host.set_pointer(choice);
        // `KD_CAM_FREEHAND` holds the pointer fixed (`SetCursorPos` at `0x0043D083`, quirk O-331): reset the mouse
        // pointer of the system only if the user holds the middle button - a check run via the control channel does not
        // move it.
        let warp = self
            .session
            .as_mut()
            .and_then(GameSession::take_cursor_warp);
        if let Some((x, y)) = warp
            && self.user_middle
            && let Err(e) = shell
                .window
                .set_cursor_position(PhysicalPosition::new(x, y))
        {
            log::warn!("Mouse pointer not reset: {e}");
        }
    }

    /// Requests of the UI: display mode (`ACCEPT`), hardware pointer, gamma, quit.
    fn handle_requests(&mut self, req: HostRequests) {
        if let Some(mode) = req.display_mode {
            self.apply_display_mode(mode);
        }
        if let (Some(gamma), Some(shell)) = (req.gamma, &mut self.shell) {
            shell.gpu.set_gamma(gamma);
        }
        if let Some(on) = req.hardware_cursor {
            // Takes effect from the next frame (`MenuHost::pointer_draw` reads the setting live): on the TGA, off the
            // 3D pointer (O-297).
            let now = self.clock.now_f32();
            let has_model = self
                .host
                .as_ref()
                .is_some_and(|h| h.pointer_model_data(now).is_some());
            if on {
                log::info!("HardwareCursor {on}: TGA pointer");
            } else if has_model {
                log::info!("HardwareCursor {on}: 3D pointer");
            } else {
                log::info!(
                    "HardwareCursor {on}: 3D pointer missing (mouse.rdo not loadable), TGA pointer"
                );
            }
        }
        if req.quit {
            self.quit_pending = true;
        }
    }

    /// `ACCEPT` in `VIDEOOPTIONS` (deviation from the original): in a window the window size, in exclusive the
    /// video mode; borderless: the size follows the monitor. The fonts stay (O-58).
    fn apply_display_mode(&mut self, mode: DisplayMode) {
        let Some(shell) = &self.shell else {
            return;
        };
        match shell.mode {
            WindowMode::Windowed => {
                let (w, h) = clamp_size(mode.width, mode.height);
                let _ = shell.window.request_inner_size(PhysicalSize::new(w, h));
            }
            WindowMode::Exclusive => {
                set_exclusive(shell, shell.window.current_monitor(), mode);
            }
            WindowMode::Borderless => log::info!(
                "Display mode {} x {}: borderless: the size follows the monitor",
                mode.width,
                mode.height
            ),
        }
    }

    /// Draws a frame: with a match or a visible menu level the world (`GameSession::render`, clears itself), otherwise
    /// black; above it the commands over the world ([`over_world`]: combat display, then the game UI with messages and
    /// game windows; PAUSE; scene fade and info windows), the commands of the last logic step (menu) and last the
    /// chosen pointer at the current mouse position (`GameApp_RenderUiAndPresent` `0x004BFE50`, re/pointer-3d MZ.8:
    /// over world, messages, UI, crossfade and menus). The pointer is the TGA quad or the 3D pointer (`Cursor3D_Render`
    /// `0x004CCC50`); over a drawn minimap its depth cuts off the 3D pointer (O-457).
    fn present(&mut self) {
        if self.shell.as_ref().is_none_or(WindowShell::minimized) {
            return;
        }
        let (Some(shell), Some(host), Some(renderer), Some(pointer_renderer)) = (
            &mut self.shell,
            &mut self.host,
            &mut self.renderer,
            &mut self.pointer_renderer,
        ) else {
            return;
        };
        let size = shell.gpu.size();
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        // Exactly once per `present` (a frame skipped by the window counts): `pointer_draw` stores an open restart of
        // the loop.
        let pointer = host.pointer_draw(self.clock.now_f32(), size);
        // The world only with a running match or a visible menu level (bit `0x1`).
        let world = self.flow.world_shown();
        let version = host.version_cmds(screen_of(size), world);
        let (layer, cmds, vfs, session) =
            (&mut host.layer, &self.cmds, &self.vfs, &mut self.session);
        let format = shell.gpu.surface_format();
        let outcome = shell.gpu.render(|device, queue, encoder, target, size| {
            let (over, depth_zero) = match session.as_mut().filter(|_| world) {
                Some(session) => {
                    let gpu = SessionGpu {
                        device,
                        queue,
                        format,
                    };
                    session.render(&gpu, vfs, encoder, target, size);
                    // The same `Screen` as the UI of this frame (`over_world`): the diamonds of the minimap for the
                    // depth pre-pass of the 3D pointer.
                    let depth_zero = session.minimap_depth_rects(screen_of(size));
                    (over_world(session, layer, vfs, size), depth_zero)
                }
                None => {
                    clear(encoder, target);
                    (Vec::new(), Vec::new())
                }
            };
            layer.draw(renderer, device, queue, encoder, target, size, &over);
            layer.draw(renderer, device, queue, encoder, target, size, cmds);
            let gpu = PointerGpu {
                device,
                queue,
                target,
                size,
            };
            draw_pointer(
                pointer,
                depth_zero,
                (layer, renderer, pointer_renderer),
                vfs,
                encoder,
                &gpu,
            );
            // Second scene over the pointer: the version line (O-440).
            layer.draw(renderer, device, queue, encoder, target, size, &version);
        });
        shell.finish_frame(outcome, &mut self.frames);
    }

    /// Screenshot by the print key (`KD_HARDCOPY`): the same image as [`Self::capture`] as PNG `opennml-shot-NNNN.png` next
    /// to the settings file, first free number ([`free_shot_path`]). As in the original (`FUN_004BD940` reads the front
    /// buffer after `Present`), the software pointer (`HardwareCursor 0`) is in the image, the hardware pointer is not.
    /// Deviation from the original: PNG next to the settings file instead of JPEG `ScreenShots/screenshotNNNN.jpg`
    /// relative to the working directory (re/game-ui D5.3).
    fn write_hardcopy(&mut self) {
        let Some(dir) = self.settings.parent().map(Path::to_path_buf) else {
            log::warn!("Screenshot: no folder of the settings file");
            return;
        };
        let Some(path) = free_shot_path(&dir) else {
            log::warn!("Screenshot: all names in {} taken", dir.display());
            return;
        };
        let software = self
            .host
            .as_ref()
            .is_some_and(|h| !h.app.config().hardware_cursor);
        match self
            .capture(software)
            .and_then(|img| img.save(&path).map_err(|e| e.to_string()))
        {
            Ok(()) => log::info!("Screenshot {} saved", path.display()),
            Err(e) => log::warn!("Screenshot {} not saved: {e}", path.display()),
        }
    }

    /// Screenshot of the control channel: the same image as in the window (world, combat display, game UI, scene fade,
    /// info windows, menu). With `pointer` additionally the pointer at the mouse position, sprite or 3D model as in
    /// [`Self::present`], but via [`MenuHost::pointer_preview`]: the query does not store the restart of the loop.
    /// Without `pointer` the image stays without a pointer (like the hardware pointer).
    fn capture(&mut self, pointer: bool) -> Result<image::RgbaImage, String> {
        let (Some(shell), Some(host), Some(renderer), Some(pointer_renderer)) = (
            &self.shell,
            &mut self.host,
            &mut self.renderer,
            &mut self.pointer_renderer,
        ) else {
            return Err("graphics not ready yet".into());
        };
        let gpu = &shell.gpu;
        let size = gpu.size();
        let format = gpu.surface_format();
        let world = self.flow.world_shown();
        let version = host.version_cmds(screen_of(size), world);
        let drawn = pointer
            .then(|| host.pointer_preview(self.clock.now_f32(), size))
            .flatten();
        let (layer, cmds, vfs, session) =
            (&mut host.layer, &self.cmds, &self.vfs, &mut self.session);
        render_to_image(&gpu.device, &gpu.queue, size, format, |encoder, view| {
            let mut depth_zero = Vec::new();
            let over = match session.as_mut().filter(|_| world) {
                Some(session) => {
                    let session_gpu = SessionGpu {
                        device: &gpu.device,
                        queue: &gpu.queue,
                        format,
                    };
                    session.render(&session_gpu, vfs, encoder, view, size);
                    depth_zero = session.minimap_depth_rects(screen_of(size));
                    over_world(session, layer, vfs, size)
                }
                None => {
                    clear(encoder, view);
                    Vec::new()
                }
            };
            layer.draw(
                renderer,
                &gpu.device,
                &gpu.queue,
                encoder,
                view,
                size,
                &over,
            );
            layer.draw(renderer, &gpu.device, &gpu.queue, encoder, view, size, cmds);
            let target = PointerGpu {
                device: &gpu.device,
                queue: &gpu.queue,
                target: view,
                size,
            };
            draw_pointer(
                drawn,
                depth_zero,
                (layer, renderer, pointer_renderer),
                vfs,
                encoder,
                &target,
            );
            layer.draw(
                renderer,
                &gpu.device,
                &gpu.queue,
                encoder,
                view,
                size,
                &version,
            );
        })
        .map_err(|e| format!("{e:#}"))
    }

    /// An input of the window to the host and to the map ([`Self::map_input`]); discarded after a load while the
    /// backlogged events arrive (`GameFlow::accepts_input`).
    fn input(&mut self, event: InputEvent) {
        if !self.flow.accepts_input() || self.input_locked() {
            return;
        }
        let (screen, now) = (self.screen(), self.now());
        if let Some(host) = &mut self.host {
            host.on_input(&event, screen, now);
        }
        self.map_input(&event);
    }

    /// An input of the control channel (`mouse_move`, `mouse_button`, `key`): exactly the path of the window. Keys go
    /// like `WindowEvent::KeyboardInput` to the menu host (without a character) and the map, mouse like
    /// [`Self::input`]. Discarded after a load while the backlogged events arrive.
    fn control_input(&mut self, event: InputEvent) {
        let InputEvent::Key { code, pressed, .. } = event else {
            self.input(event);
            return;
        };
        if !self.flow.accepts_input() || self.input_locked() {
            return;
        }
        let (screen, now) = (self.screen(), self.now());
        if let Some(host) = &mut self.host {
            host.on_key(code, pressed, None, screen, now);
        }
        self.map_input(&event);
    }

    /// `ui_click` on `GAMEUI` ([`GameFlow::gameui_click`]): the center of the element from the last frame, then
    /// movement, press and release through the flow (only the map, not the menu host: the game menu stays untouched, an
    /// open one blocks the click). After `step`/`screenshot`, which draws the rectangles.
    fn game_ui_click(&mut self, element: &str, button: MouseButton) -> Result<(i32, i32), String> {
        let (screen, now) = (self.screen(), self.now());
        let (Some(host), Some(shell)) = (self.host.as_mut(), self.shell.as_ref()) else {
            return Err(NO_UI.to_string());
        };
        let mut view = live(
            shell,
            &self.vfs,
            &mut self.session,
            &mut self.view_options,
            &mut self.controls,
            &mut self.audio,
        );
        self.flow
            .gameui_click(element, button, screen, now, &mut host.app, &mut view)
    }

    /// An input to the map of the match (`GameFlow::input`: only while the game runs). No input of the window reaches
    /// the menu level (O-442); its keys run only through the key path in [`Self::advance`].
    fn map_input(&mut self, event: &InputEvent) {
        let (screen, now) = (self.screen(), self.now());
        let (Some(host), Some(shell)) = (self.host.as_mut(), self.shell.as_ref()) else {
            return;
        };
        if !self.flow.running() {
            return;
        }
        // With the state after this event (`on_input`/`on_key` already ran): the shift path sees the current shift key
        // on a click.
        sync_held_vk(self.session.as_mut(), host);
        let mut view = live(
            shell,
            &self.vfs,
            &mut self.session,
            &mut self.view_options,
            &mut self.controls,
            &mut self.audio,
        );
        self.flow
            .input(event, screen, now, &mut host.app, &mut view);
    }

    fn run_control(&mut self, event_loop: &ActiveEventLoop, event: ControlEvent) {
        let ControlEvent { request, reply } = event;
        let Request { id, command } = request;
        let response = command
            .validate()
            .and_then(|()| self.execute(event_loop, id, command))
            .unwrap_or_else(|message| Response::error(id, message));
        let _ = reply.send(response);
    }

    fn execute(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: u64,
        command: Command,
    ) -> Result<Response, String> {
        let guard = self.net_guard();
        if guard.in_net {
            let own = self.net_driver().and_then(NetDriver::own_slot);
            if let Some(refusal) = net_refusal(&command, guard.locked, own) {
                return Err(refusal);
            }
        }
        match command {
            Command::Hello => Ok(Response::ok(
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
            )),
            Command::Pause => {
                self.clock.pause();
                Ok(Response::ok(id, ()))
            }
            Command::Resume => {
                self.clock.resume();
                Ok(Response::ok(id, ()))
            }
            // Response: state of the UI after the steps, like `state`.
            Command::Step { frames, frame_ms } => {
                self.clock.pause();
                for _ in 0..frames {
                    self.advance(frame_ms as f32);
                }
                self.present();
                if self.quit_pending {
                    self.exit(event_loop);
                }
                self.ui_state().map(|state| Response::ok(id, state))
            }
            Command::State | Command::UiState => {
                self.ui_state().map(|state| Response::ok(id, state))
            }
            Command::SimHash
            | Command::SimQuery { .. }
            | Command::SimCommand { .. }
            | Command::SimResult { .. } => run_sim(
                &mut self.session,
                self.flow.menu_level().is_some(),
                &command,
            )
            .map(|value| Response::ok(id, value)),
            Command::RandomMap => {
                random_map_value(self.session.as_ref()).map(|value| Response::ok(id, value))
            }
            // Network commands on the state of the session, its driver and the simulation of the match.
            Command::NetStatus
            | Command::NetPartHashes
            | Command::NetSave { .. }
            | Command::NetDebug { .. } => {
                let status = self.net_status_data();
                let (session_net, sim) = match self.session.as_mut() {
                    Some(s) => (s.net.as_mut(), s.map.sim.as_mut()),
                    None => (None, None),
                };
                let driver = match (
                    self.net.as_mut().and_then(NetLaunch::driver_mut),
                    session_net,
                ) {
                    (Some(driver), _) | (None, Some(driver)) => Some(driver),
                    (None, None) => self.flow.net_session_mut().and_then(NetSession::driver_mut),
                };
                net_start::run_net(status, driver, sim, &command)
                    .map(|value| Response::ok(id, value))
            }
            // The command bar of the match is intercepted by the host before `execute_ui`, which stays menu-only. Three
            // mouse events through the flow, an open game menu blocks the click.
            Command::UiClick {
                dialog,
                element,
                button,
            } if dialog == GAME_UI_DIALOG => {
                let button = match button.unwrap_or(MouseButtonName::Left) {
                    MouseButtonName::Left => MouseButton::Left,
                    MouseButtonName::Right => MouseButton::Right,
                    // `Command::validate` already rejects that.
                    MouseButtonName::Middle => {
                        return Err(opennml_control::UI_CLICK_MIDDLE.to_string());
                    }
                };
                self.game_ui_click(&element, button)
                    .map(|(x, y)| Response::ok(id, serde_json::json!({"x": x, "y": y})))
            }
            // Mouse and key as in the viewer, without a step (the pointer takes effect in the next frame).
            Command::MouseMove { x, y } => {
                self.control_input(InputEvent::CursorMoved { x, y });
                Ok(Response::ok(id, ()))
            }
            Command::MouseButton { button, down } => {
                let button = match button {
                    MouseButtonName::Left => MouseButton::Left,
                    MouseButtonName::Right => MouseButton::Right,
                    MouseButtonName::Middle => MouseButton::Middle,
                };
                self.control_input(InputEvent::Button {
                    button,
                    pressed: down,
                });
                Ok(Response::ok(id, ()))
            }
            // With `repeat` the repeats as events of their own (O-300), like holding by hand.
            Command::Key {
                code,
                action,
                repeat,
            } => {
                let code = key_code(&code).ok_or_else(|| format!("unknown key \"{code}\""))?;
                for event in key_events(code, action, repeat) {
                    self.control_input(event);
                }
                Ok(Response::ok(id, ()))
            }
            // Like the focus loss of the window, the pause also with an active control channel.
            Command::UiFocusLost => {
                lose_focus(self.host.as_mut(), self.session.as_mut(), true);
                Ok(Response::ok(id, ()))
            }
            // Presentation event by hand (presentation only, the checksum stays); changes the host state (game speed,
            // fog mode, selection).
            Command::UiPresent { event } => {
                let (Some(session), Some(shell)) = (self.session.as_mut(), self.shell.as_ref())
                else {
                    return Err("no match".to_string());
                };
                let gpu = SessionGpu::of(&shell.gpu);
                session.present_json(&gpu, &self.vfs, Some(&mut self.audio), &event)?;
                Ok(Response::ok(id, ()))
            }
            Command::UiClick { .. }
            | Command::UiKey { .. }
            | Command::UiOpen { .. }
            | Command::UiVideo { .. } => {
                let (screen, now) = (self.screen(), self.now());
                let host = self.host.as_mut().ok_or(NO_UI)?;
                execute_ui(&mut host.app, &command, screen, now)
                    .map(|value| Response::ok(id, value))
            }
            Command::Screenshot {
                max_size, pointer, ..
            } => {
                let img = self.capture(pointer)?;
                png_data(&img, max_size).map(|data| Response::ok(id, data))
            }
            Command::Quit => {
                self.quit_requested = true;
                self.exit(event_loop);
                Ok(Response::ok(id, ()))
            }
            other => Err(unavailable(&other)),
        }
    }
}

/// Fullscreen exclusive in the video mode `mode` on `monitor` ([`matching_mode`]); `false` without a monitor or
/// without a matching mode (then the previous one stays, warning).
fn set_exclusive(shell: &WindowShell, monitor: Option<MonitorHandle>, mode: DisplayMode) -> bool {
    let Some(monitor) = monitor else {
        log::warn!("Display mode {} x {}: no monitor", mode.width, mode.height);
        return false;
    };
    let modes: Vec<_> = monitor.video_modes().collect();
    let infos: Vec<ModeInfo> = modes
        .iter()
        .map(|v| ModeInfo {
            width: v.size().width,
            height: v.size().height,
            refresh_millihertz: v.refresh_rate_millihertz(),
            bit_depth: v.bit_depth(),
        })
        .collect();
    match matching_mode(&infos, mode) {
        Some(i) => {
            shell
                .window
                .set_fullscreen(Some(Fullscreen::Exclusive(modes[i].clone())));
            true
        }
        None => {
            log::warn!(
                "Display mode {} x {} ({} bit) is not available on the monitor; the previous one stays",
                mode.width,
                mode.height,
                mode.bits
            );
            false
        }
    }
}

impl ApplicationHandler<ControlEvent> for Game {
    /// New round of the event loop: after a load the next round discards the backlogged inputs.
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        self.flow.begin_round();
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shell.is_some() || self.error.is_some() {
            return;
        }
        if let Err(e) = self.start(event_loop) {
            return self.fail(event_loop, e);
        }
        for event in std::mem::take(&mut self.waiting) {
            self.run_control(event_loop, event);
        }
    }

    /// Control command from the control thread; before `resumed` only `hello` immediately, everything else afterwards.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ControlEvent) {
        if self.shell.is_none() && event.request.command != Command::Hello {
            self.waiting.push(event);
            return;
        }
        self.run_control(event_loop, event);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            // Closing the window (also Alt+F4) writes the settings (O-32).
            WindowEvent::CloseRequested => self.exit(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(shell) = &mut self.shell {
                    shell.resized(size.width, size.height);
                }
                if let Some(session) = &mut self.session {
                    session.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(true) | WindowEvent::Occluded(false) => {
                if let Some(shell) = &mut self.shell {
                    shell.shown();
                }
            }
            // Focus pause of the original only without a control channel (check runs mostly run without focus;
            // deviation from the original, O-154); the control channel triggers it deliberately with `ui_focus_lost`.
            WindowEvent::Focused(false) => {
                self.user_middle = false;
                lose_focus(
                    self.host.as_mut(),
                    self.session.as_mut(),
                    !self.opts.control,
                );
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state,
                        physical_key: PhysicalKey::Code(code),
                        text,
                        repeat,
                        ..
                    },
                ..
            } => {
                if !self.flow.accepts_input() || self.input_locked() {
                    return;
                }
                let pressed = state == ElementState::Pressed;
                let ch = if pressed {
                    char_of(text.as_deref())
                } else {
                    None
                };
                let (screen, now) = (self.screen(), self.now());
                if let Some(host) = &mut self.host {
                    host.on_key(code, pressed, ch, screen, now);
                }
                self.map_input(&InputEvent::Key {
                    code,
                    pressed,
                    repeat,
                });
            }
            WindowEvent::CursorMoved { position, .. } => self.input(InputEvent::CursorMoved {
                x: position.x,
                y: position.y,
            }),
            WindowEvent::CursorLeft { .. } => self.input(InputEvent::CursorLeft),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    // `KD_CAM_FREEHAND` (quirk O-331).
                    winit::event::MouseButton::Middle => {
                        self.user_middle = state == ElementState::Pressed;
                        MouseButton::Middle
                    }
                    _ => return,
                };
                self.input(InputEvent::Button {
                    button,
                    pressed: state == ElementState::Pressed,
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(up) = wheel_up(delta) {
                    self.input(InputEvent::Wheel { up });
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit_pending {
            self.exit(event_loop);
            // The host waits for the end bundle at program end: keep drawing frames.
            if !self.closing {
                return;
            }
        }
        self.pump_net();
        if let Some(shell) = &mut self.shell {
            shell.schedule(event_loop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// E-295: only the network start skips the start videos; the main menu always comes.
    // QUIRK E-295: kept
    #[test]
    fn net_start_skips_the_intro_videos() {
        let single = window_menu_options(Vec::new(), Vec::new(), false);
        assert!(single.start_menu && !single.skip_intro);
        let net = window_menu_options(Vec::new(), Vec::new(), true);
        assert!(net.start_menu && net.skip_intro);
    }

    /// First free number from 0000, gaps are filled; with 1000 taken names none.
    #[test]
    fn hardcopy_takes_the_first_free_number() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(free_shot_path(dir), Some(dir.join("opennml-shot-0000.png")));
        for n in [0, 1, 3] {
            std::fs::write(dir.join(format!("opennml-shot-{n:04}.png")), b"x").unwrap();
        }
        assert_eq!(free_shot_path(dir), Some(dir.join("opennml-shot-0002.png")));
        for n in 0..1000 {
            std::fs::write(dir.join(format!("opennml-shot-{n:04}.png")), b"x").unwrap();
        }
        assert_eq!(free_shot_path(dir), None);
    }

    #[test]
    fn screenshot_host_never_writes_settings() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let audio = AudioEngine::open(&vfs, false);
        let layer = UiLayer::new(&vfs, 1024, None).unwrap();
        let config = opennml_client::settings::default_config(&vfs);
        let mut host = screenshot_host(&vfs, layer, config, &audio, Vec::new()).unwrap();
        // No location: `WriteSettings` (`OPTIONS OK`) and the program end write nothing.
        assert_eq!(host.settings_path(), None);
        host.shutdown();
        assert_eq!(host.settings_path(), None);
    }

    #[test]
    fn window_size_follows_the_settings_with_a_minimum() {
        let cfg = |w, h| EngineConfig {
            screen_x_size: w,
            screen_y_size: h,
            ..EngineConfig::default()
        };
        assert_eq!(window_size(&EngineConfig::default()), (1024, 768));
        assert_eq!(window_size(&cfg(1280, 1024)), (1280, 1024));
        // 0, negative, too small, too large: no window of size 0, no absurd size.
        assert_eq!(window_size(&cfg(0, 0)), MIN_WINDOW);
        assert_eq!(window_size(&cfg(-5, 300)), MIN_WINDOW);
        assert_eq!(window_size(&cfg(i32::MAX, 768)), (MAX_WINDOW_SIDE, 768));
    }

    #[test]
    fn screen_format_chooses_the_window_mode() {
        let format = |f| EngineConfig {
            screen_format: f,
            ..EngineConfig::default()
        };
        assert_eq!(window_mode(None, &format(0)), WindowMode::Exclusive);
        assert_eq!(window_mode(None, &format(1)), WindowMode::Windowed);
        assert_eq!(window_mode(None, &format(-1)), WindowMode::Windowed);
        assert_eq!(
            window_mode(Some(WindowMode::Windowed), &format(0)),
            WindowMode::Windowed
        );
        assert_eq!(
            window_mode(Some(WindowMode::Borderless), &format(1)),
            WindowMode::Borderless
        );
    }

    #[test]
    fn exclusive_mode_matches_size_then_depth_then_rate() {
        let m = |width, height, refresh_millihertz, bit_depth| ModeInfo {
            width,
            height,
            refresh_millihertz,
            bit_depth,
        };
        let modes = [
            m(1280, 1024, 60_000, 32),
            m(1024, 768, 60_000, 32),
            m(1024, 768, 75_000, 32),
            m(1024, 768, 60_000, 16),
        ];
        let want = |width, height, bits| DisplayMode {
            width,
            height,
            bits,
        };
        assert_eq!(matching_mode(&modes, want(1024, 768, 16)), Some(3));
        assert_eq!(matching_mode(&modes, want(1024, 768, 32)), Some(2));
        assert_eq!(matching_mode(&modes, want(1024, 768, 24)), Some(2));
        assert_eq!(matching_mode(&modes, want(800, 600, 32)), None);
        assert_eq!(matching_mode(&modes, want(-1, 0, 32)), None);
    }

    /// `ui_present` goes through the control channel of `opennml-game`; the command leaves the checksum of the simulation
    /// unchanged until the next step (`ui_present_leaves_the_checksum` in `opennml_client::game_session`, there with a
    /// match).
    #[test]
    fn opennml_game_offers_ui_present() {
        assert!(AVAILABLE.contains(&"ui_present"));
        let command = Command::UiPresent {
            event: serde_json::json!({"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}),
        };
        assert_eq!(command.validate(), Ok(()));
        assert!(
            unavailable(&Command::Lights)
                .contains("ui_video, ui_present, ui_focus_lost, mouse_move")
        );
    }

    #[test]
    fn game_lists_sim_hash_query_command() {
        for name in ["sim_hash", "sim_query", "sim_command", "sim_result"] {
            assert!(AVAILABLE.contains(&name), "{name}");
        }
        // Further simulation commands (build, train, AI) stay with the viewer.
        assert!(!AVAILABLE.contains(&"sim_build"));
        assert!(!AVAILABLE.contains(&"sim_setup"));
    }

    /// Mouse and key go through the control channel of `opennml-game`, the remaining inputs (wheel, leaving the window) do
    /// not.
    #[test]
    fn opennml_game_accepts_mouse_and_key_commands() {
        for name in ["mouse_move", "mouse_button", "key"] {
            assert!(AVAILABLE.contains(&name), "{name}");
        }
        for command in [
            Command::MouseMove { x: 5.0, y: 6.0 },
            Command::MouseButton {
                button: MouseButtonName::Right,
                down: true,
            },
            Command::Key {
                code: "ShiftLeft".into(),
                action: opennml_control::KeyAction::Down,
                repeat: 0,
            },
        ] {
            assert_eq!(command.validate(), Ok(()), "{command:?}");
        }
        for command in [Command::MouseLeave, Command::Wheel { up: true }] {
            assert!(
                unavailable(&command).contains("is not available in opennml-game"),
                "{command:?}"
            );
        }
        // The message names the new commands.
        assert!(unavailable(&Command::Lights).contains("mouse_move, mouse_button, key"));
        // Key and mouse of the control channel are the same events as in the viewer.
        assert_eq!(
            key_code("ShiftLeft"),
            Some(winit::keyboard::KeyCode::ShiftLeft)
        );
        assert_eq!(key_code("Hyper"), None);
    }

    /// `ui_focus_lost` acts like the focus loss of the window (O-154) - held keys released, the match switches the
    /// pause on, also with an active control channel; the window with a control channel only releases.
    // QUIRK O-154: kept
    #[test]
    fn ui_focus_lost_pauses() {
        assert!(AVAILABLE.contains(&"ui_focus_lost"));
        assert_eq!(Command::UiFocusLost.validate(), Ok(()));
        assert!(!lose_focus(None, None, true), "nothing without a match");
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: CAPTURE_FORMAT,
        };
        let mut session = GameSession::start(
            &gpu,
            &vfs,
            "data/levels/multiplayer/[2players]-tintebariver.lvl",
            &mut opennml_client::game::sim_host::testkit::default_setup,
            10,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        session.held_vk = vec![0xBB];
        session
            .pointer
            .keys
            .insert(winit::keyboard::KeyCode::BracketRight);
        // Window with control channel (`!opts.control` false): keys released, no pause.
        assert!(!lose_focus(None, Some(&mut session), false));
        assert!(session.held_vk.is_empty() && session.pointer.keys.is_empty());
        assert!(!session.pause.sim);
        // `ui_focus_lost`: the pause of the original, also with a control channel.
        session.held_vk = vec![0xBB];
        assert!(lose_focus(None, Some(&mut session), true));
        assert!(session.held_vk.is_empty());
        assert!(session.pause.sim && session.pause.shown);
        assert!(session.sim().unwrap().halted);
        // Already visibly paused: another focus loss changes nothing.
        assert!(!lose_focus(None, Some(&mut session), true));
        assert!(session.pause.sim);
    }

    #[test]
    fn game_rejects_sim_commands_without_a_match() {
        let command = json!({"player": 0, "kind": {"Halt": {"units": [65537]}}});
        let commands = [
            Command::SimHash,
            Command::SimQuery {
                player: None,
                guid: None,
                ids: None,
                limit: None,
            },
            Command::SimCommand { command },
            Command::SimResult {
                player: 0,
                result: 2,
            },
        ];
        for command in &commands {
            assert_eq!(
                run_sim(&mut None, false, command),
                Err("no match".to_string()),
                "{command:?}"
            );
        }
        // A broken command is reported first (like in the viewer), not "no match".
        let broken = Command::SimCommand {
            command: json!({"x": 1}),
        };
        let message = run_sim(&mut None, false, &broken).unwrap_err();
        assert!(message.starts_with("command invalid"), "{message}");
        let broken = Command::SimResult {
            player: 0,
            result: 0,
        };
        let message = run_sim(&mut None, false, &broken).unwrap_err();
        assert!(message.starts_with("result = 0 invalid"), "{message}");
    }

    #[test]
    fn sim_command_is_refused_in_the_menu_level() {
        // Commands to the simulation are refused by the menu level before they are queued; even a broken command gets
        // this message. `sim_hash`/`sim_query` keep running through `execute_sim` (without a session "no match").
        let command = json!({"player": 0, "kind": {"Halt": {"units": [65537]}}});
        for refused in [
            Command::SimCommand { command },
            Command::SimCommand {
                command: json!({"x": 1}),
            },
            Command::SimResult {
                player: 0,
                result: 2,
            },
        ] {
            assert_eq!(
                run_sim(&mut None, true, &refused),
                Err("Menu level takes no commands".to_string()),
                "{refused:?}"
            );
        }
        for read in [
            Command::SimHash,
            Command::SimQuery {
                player: None,
                guid: None,
                ids: None,
                limit: None,
            },
        ] {
            assert_eq!(
                run_sim(&mut None, true, &read),
                Err("no match".to_string()),
                "{read:?}"
            );
        }
    }

    #[test]
    fn random_map_without_a_random_match_is_null() {
        // `random_map` answers with the state of the random map of the running match (`GameSession::random_map_state`),
        // `null` without a match or with a fixed map.
        assert!(AVAILABLE.contains(&"random_map"));
        assert_eq!(random_map_value(None), Ok(Value::Null));
    }

    #[test]
    fn other_commands_are_not_available_in_german() {
        let message = unavailable(&Command::Lights);
        assert_eq!(
            message,
            "\"lights\" is not available in opennml-game (available: hello, pause, resume, step, screenshot, state, ui_state, ui_click, ui_key, ui_open, ui_video, ui_present, ui_focus_lost, mouse_move, mouse_button, key, sim_hash, sim_query, sim_command, sim_result, random_map, net_status, net_part_hashes, net_save, net_debug, quit)"
        );
        assert!(
            unavailable(&Command::MouseLeave)
                .starts_with("\"mouse_leave\" is not available in opennml-game")
        );
        // `hud` only switches the game UI of the viewer; `opennml-game` always draws it.
        assert!(
            unavailable(&Command::Hud { on: false })
                .starts_with("\"hud\" is not available in opennml-game")
        );
    }

    /// In the network the control channel rejects the clock commands, every input until `Go`, and simulation commands
    /// of foreign slots; the own slot, `resume` and the queries run.
    #[test]
    fn net_refuses_clock_input_and_foreign_slots() {
        for command in [
            Command::Pause,
            Command::Step {
                frames: 1,
                frame_ms: 16.0,
            },
        ] {
            let text = net_refusal(&command, false, Some(1)).expect("rejected");
            assert!(text.contains("real time"), "{text}");
        }
        assert_eq!(net_refusal(&Command::Resume, false, Some(1)), None);
        let click = Command::MouseMove { x: 1.0, y: 2.0 };
        let text = net_refusal(&click, true, None).expect("locked until Go");
        assert!(text.contains("locked"), "{text}");
        assert_eq!(net_refusal(&click, false, Some(1)), None, "free after Go");
        let key = Command::Key {
            code: "Escape".into(),
            action: opennml_control::KeyAction::Tap,
            repeat: 0,
        };
        assert!(net_refusal(&key, true, None).is_some());
        let own = Command::SimCommand {
            command: json!({"player": 1, "kind": {"Halt": {"units": [65537]}}}),
        };
        assert_eq!(net_refusal(&own, false, Some(1)), None, "own slot");
        let foreign = Command::SimCommand {
            command: json!({"player": 0, "kind": {"Halt": {"units": [65537]}}}),
        };
        let text = net_refusal(&foreign, false, Some(1)).expect("foreign slot");
        assert!(text.contains("own slot 1"), "{text}");
        assert!(
            net_refusal(
                &Command::SimResult {
                    player: 2,
                    result: 1
                },
                false,
                Some(1)
            )
            .is_some()
        );
        assert_eq!(
            net_refusal(
                &Command::SimResult {
                    player: 1,
                    result: 1
                },
                false,
                Some(1)
            ),
            None
        );
        for command in [Command::NetStatus, Command::SimHash, Command::State] {
            assert_eq!(net_refusal(&command, true, None), None, "{command:?}");
        }
    }

    /// In search and lobby of the menu path the control channel accepts `ui_*`; `pause` and `step` stay locked; while
    /// loading the input is locked, without a network session nothing is locked. The command-line path locks until
    /// `Go`; a start that failed before `Go` releases the input.
    #[test]
    fn ui_commands_allowed_in_the_lobby() {
        use crate::net_start::net_guard;
        use opennml_client::net_session::NetPhase;
        let click = Command::UiClick {
            dialog: "lan2".into(),
            element: "READY1".into(),
            button: None,
        };
        let open = Command::UiOpen {
            dialog: "lan1".into(),
        };
        for phase in [NetPhase::Searching, NetPhase::Connecting, NetPhase::Lobby] {
            let guard = net_guard(CliState::default(), None, Some(phase));
            assert!(guard.in_net && !guard.locked, "{phase:?}");
            assert_eq!(net_refusal(&click, guard.locked, None), None, "{phase:?}");
            assert_eq!(net_refusal(&open, guard.locked, None), None, "{phase:?}");
            assert!(net_refusal(&Command::Pause, guard.locked, None).is_some());
        }
        let loading = net_guard(CliState::default(), None, Some(NetPhase::Loading));
        assert!(loading.locked);
        let text = net_refusal(&click, loading.locked, None).expect("locked while loading");
        assert!(text.contains("locked"), "{text}");
        // Match of the menu path: loading over, `Go` (or failure) -> free, clock commands locked.
        let running = net_guard(CliState::default(), Some(true), Some(NetPhase::Match));
        assert!(running.in_net && !running.locked);
        assert_eq!(
            net_guard(CliState::default(), None, Some(NetPhase::Idle)),
            NetGuard::default(),
            "without a network session nothing is locked"
        );
        assert_eq!(
            net_guard(CliState::default(), None, None),
            NetGuard::default()
        );
        // Command-line path: locked until `Go`, free afterwards.
        let waiting = CliState {
            waiting: true,
            handed: false,
        };
        assert!(net_guard(waiting, None, Some(NetPhase::Idle)).locked);
        let handed = CliState {
            waiting: false,
            handed: true,
        };
        assert!(net_guard(handed, Some(false), Some(NetPhase::Idle)).locked);
        assert!(!net_guard(handed, Some(true), Some(NetPhase::Idle)).locked);
        // Failed join: back in the main menu, free.
        assert_eq!(
            net_guard(CliState::default(), None, Some(NetPhase::Idle)),
            NetGuard::default()
        );
    }
}
