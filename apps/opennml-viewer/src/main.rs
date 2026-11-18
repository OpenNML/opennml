//! Viewer for the game data of No Man's Land: map view of a level with a faithful renderer and the original
//! camera.

mod app;
mod browser;
mod combat_overlay;
mod config;
mod debug_ui;
mod dialogs;
mod economy_ui;
mod effect_browser;
mod hud_view;
mod ki_ui;
#[cfg(test)]
mod script_control_tests;
mod script_ui;
mod snapshot;
#[cfg(test)]
mod testworld_viewer;
mod unit_bar_ui;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use opennml_client::clock::parse_time_ms;
use opennml_client::control::{self, ControlEvent};
use opennml_client::settings::settings_path_for;
use opennml_client::window::WindowMode;
use opennml_control::parse_control_addr;
use opennml_rda::{Vfs, find_game_dir, read_remembered, remembered_file};
use opennml_render::{BackendChoice, CAPTURE_FORMAT, headless_device, render_to_image};
use winit::event_loop::{ControlFlow, EventLoop};

use crate::browser::{BrowserView, find_model, model_paths};
use crate::config::{
    AspectSetting, Config, LightSetting, MAX_GAME_SPEED, MAX_PARTICLES_QUALITY, MAX_SHADOW_QUALITY,
    MAX_TERRAIN_QUALITY, MAX_VOLUME, Overrides, WeatherSetting, match_setup,
};
use crate::dialogs::DialogBrowser;
use opennml_client::game::map::MapView;
use opennml_client::game::overlay::{
    OverlayInfo, browser_lines, effect_line, environment_line, overlay_lines, shadow_line,
};

#[derive(Parser)]
#[command(name = "opennml-viewer", about = "Viewer for No Man's Land")]
struct Cli {
    /// Settings file (TOML); without it opennml-viewer.toml in the working directory, if present
    #[arg(long)]
    config: Option<PathBuf>,
    /// auto, vulkan or dx12
    #[arg(long)]
    backend: Option<BackendChoice>,
    /// windowed, borderless or exclusive
    #[arg(long)]
    window_mode: Option<WindowMode>,
    /// Monitor index (0-based)
    #[arg(long)]
    monitor: Option<usize>,
    /// Only this location as the original installation; without it, search as in docs/programs.md, section "Game
    /// directory"
    #[arg(long)]
    game_dir: Option<PathBuf>,
    /// Level as a game path, e.g. data/levels/kampagne1/kamp1-1/kamp1-1.lvl
    #[arg(long)]
    level: Option<String>,
    /// Random map from this seed text instead of a level: preview of the terrain without objects and without
    /// simulation; control command random_map
    #[arg(long, value_name = "SEED", conflicts_with_all = ["level", "mission"])]
    random_map: Option<String>,
    /// Size of the random map: 0 small (64), 1 medium (128), 2 large (256)
    #[arg(long, default_value_t = 1, requires = "random_map")]
    random_size: i32,
    /// Number of players of the random map 2-8
    #[arg(long, default_value_t = 4, requires = "random_map")]
    random_players: i32,
    /// Map type of the random map (index in MSCRIPT of randommaptypes.ini)
    #[arg(long, default_value_t = 0, requires = "random_map")]
    random_terrain: usize,
    /// Landscape of the random map (index in LTYPE)
    #[arg(long, default_value_t = 4, requires = "random_map")]
    random_landscape: usize,
    /// Time of day of the random map (index in DTYPE)
    #[arg(long, default_value_t = 0, requires = "random_map")]
    random_daytime: usize,
    /// Game mode of the random map (index in GTYPE)
    #[arg(long, default_value_t = 0, requires = "random_map")]
    random_game_type: usize,
    /// correct, original43 or stretch
    #[arg(long)]
    aspect: Option<AspectSetting>,
    /// Switch fog on
    #[arg(long)]
    fog: bool,
    /// Engine.ini ShadowQuality 0-3
    #[arg(long, value_parser = parse_shadow_quality)]
    shadow_quality: Option<u8>,
    /// Engine.ini ParticlesQuality 0-2
    #[arg(long, value_parser = parse_particles_quality)]
    particles_quality: Option<u8>,
    /// Engine.ini TerrainQuality 0-2 (from 2: water in detail texture mode)
    #[arg(long, value_parser = parse_terrain_quality)]
    terrain_quality: Option<u8>,
    /// Light at start: start (as in the game), level (.wld) or a name from lightdata.ini
    #[arg(long)]
    light: Option<LightSetting>,
    /// Weather at start: start (as in the game), none, rain or snow
    #[arg(long)]
    weather: Option<WeatherSetting>,
    /// Music volume 0-1000 (Engine.ini m_Volumes, default 349)
    #[arg(long, value_parser = parse_volume)]
    volume_music: Option<u16>,
    /// Sound effects volume 0-1000 (default 599)
    #[arg(long, value_parser = parse_volume)]
    volume_sfx: Option<u16>,
    /// Speech volume 0-1000 (default 499)
    #[arg(long, value_parser = parse_volume)]
    volume_speech: Option<u16>,
    /// Start without device output (F9 toggles); mixer and recording keep running
    #[arg(long)]
    mute: bool,
    /// Draw the game UI as in opennml-game (message bar, status bar, command bar, minimap) instead of the message lines
    /// in the overlay; in the window also via the control command hud
    #[arg(long)]
    hud: bool,
    /// Engine.ini GameSpeed 0-30 (default 14 = 1.2 times the game time)
    #[arg(long, value_parser = parse_game_speed)]
    game_speed: Option<u8>,
    /// Game type: -1 campaign, 0 normal, 1 kill hero, 3 King of the Hill, 4/5 Railroad
    #[arg(long, allow_hyphen_values = true)]
    game_mode: Option<i32>,
    /// Start index of the random sequence of the simulation (original 0)
    #[arg(long)]
    seed: Option<u32>,
    /// Nations of the lobby slots in order, e.g. SPA,ENG
    #[arg(long, value_delimiter = ',')]
    nations: Option<Vec<String>>,
    /// Teams of the lobby slots in order, e.g. 0,0,1,1
    #[arg(long, value_delimiter = ',')]
    teams: Option<Vec<i32>>,
    /// Map explored (MapExplored): set to "explored" completely for all players at the start
    #[arg(long)]
    map_explored: bool,
    /// Lobby slots in order: c closed, h human, a1 to a4 AI kind (handicap 5/2/0/0), a:h0 to a:h5 AI with a free
    /// handicap, e.g. h,a3,h,a3; nation and team still via --nations/--teams
    #[arg(long, value_delimiter = ',', value_parser = config::parse_slot)]
    slots: Option<Vec<opennml_control::SlotSetup>>,
    /// Start the AI of the AI slots: on or off (default on)
    #[arg(long, value_parser = config::parse_ki_switch)]
    ki: Option<bool>,
    /// Campaign difficulty 0-2 (default 1)
    #[arg(long, value_parser = parse_campaign_difficulty)]
    campaign_difficulty: Option<i32>,
    /// Start the level script: on or off (default off; `--mission` switches it on)
    #[arg(long, value_parser = config::parse_ki_switch)]
    script: Option<bool>,
    /// Start a campaign mission: file stem of the level from campaign.ini (e.g. kamp1-1, all four campaigns);
    /// sets game type -1 without lobby, AI and level script on, difficulty from --campaign-difficulty (default 1)
    #[arg(long, conflicts_with_all = ["level", "game_mode", "slots", "nations", "teams"])]
    mission: Option<String>,
    /// Write the offline mix from the start as WAV (PCM 16 bit, stereo, 44,100 Hz)
    #[arg(long, conflicts_with = "screenshot")]
    audio_capture: Option<PathBuf>,
    /// Start the model browser with this model (game path of an .rdo/.rdw)
    #[arg(long)]
    model: Option<String>,
    /// Dialog browser: open dialogs of the installation over the view (name from dialogs.map or file stem, several
    /// separated by commas in opening order, e.g. options,videooptions); operable in the window, with --screenshot
    /// static on a black background without a level. In the window: Ctrl+Page Down/Up reopens the next or previous
    /// dialog, Ctrl+D hides
    #[arg(long, value_delimiter = ',', num_args = 1.., conflicts_with = "model")]
    dialog: Vec<String>,
    /// Settings file of the dialog layer in the format of Engine.ini (like opennml-game --settings, never in the
    /// installation); without it the defaults apply only in memory and nothing is ever saved
    #[arg(long, requires = "dialog")]
    settings: Option<PathBuf>,
    /// Time of the animations for --screenshot in milliseconds (finite, >= 0)
    #[arg(long, default_value_t = 1000.0, value_parser = parse_time_ms)]
    time_ms: f32,
    /// Render one frame without a window, save as PNG and quit
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Open the control channel on IP:PORT (loopback only; port 0 = choose a free one), see
    /// docs/viewer-control.md
    #[arg(long, value_parser = parse_control_addr, conflicts_with = "screenshot")]
    control: Option<SocketAddr>,
    /// Start with the virtual clock halted at 0 ms (only with --control), so that a fresh session stays
    /// unchanged until the first control command
    #[arg(long, requires = "control")]
    paused: bool,
}

/// Reads a quality level 0 to `max`; the message follows `Config::check`.
fn parse_quality(name: &str, max: u8, text: &str) -> Result<u8, String> {
    let value: i64 = text
        .trim()
        .parse()
        .map_err(|_| format!("\"{text}\" is not an integer ({name}: 0 to {max} expected)"))?;
    if !(0..=i64::from(max)).contains(&value) {
        return Err(format!("{name} = {value} invalid (allowed: 0 to {max})"));
    }
    Ok(value as u8)
}

fn parse_shadow_quality(text: &str) -> Result<u8, String> {
    parse_quality("shadow_quality", MAX_SHADOW_QUALITY, text)
}

fn parse_particles_quality(text: &str) -> Result<u8, String> {
    parse_quality("particles_quality", MAX_PARTICLES_QUALITY, text)
}

fn parse_terrain_quality(text: &str) -> Result<u8, String> {
    parse_quality("terrain_quality", MAX_TERRAIN_QUALITY, text)
}

fn parse_game_speed(text: &str) -> Result<u8, String> {
    parse_quality("game_speed", MAX_GAME_SPEED, text)
}

/// Campaign difficulty 0-2; the message follows `Config::check`.
fn parse_campaign_difficulty(text: &str) -> Result<i32, String> {
    parse_quality("campaign_difficulty", 2, text).map(i32::from)
}

/// Reads a volume control 0-1000; the message follows `Config::check`.
fn parse_volume(text: &str) -> Result<u16, String> {
    let value: i64 = text.trim().parse().map_err(|_| {
        format!("\"{text}\" is not an integer (volume: 0 to {MAX_VOLUME} expected)")
    })?;
    if !(0..=i64::from(MAX_VOLUME)).contains(&value) {
        return Err(format!(
            "volume = {value} invalid (allowed: 0 to {MAX_VOLUME})"
        ));
    }
    Ok(value as u16)
}

/// Settings file of the dialog layer: only with `--settings` (checked as in `opennml-game`, a location in the
/// installation is an error before the window). Without the switch `None`: defaults only in memory, so that check
/// runs neither read nor change the settings of the game (`NML_SETTINGS`, `%LOCALAPPDATA%\OpenNML\Engine.ini`).
fn dialog_settings(explicit: Option<&Path>, game_dir: &Path) -> Result<Option<PathBuf>> {
    explicit
        .map(|path| settings_path_for(Some(path), game_dir))
        .transpose()
}

/// Checks the image size of a screenshot against the limits of the device.
fn check_screenshot_size(size: (u32, u32), max_dimension: u32) -> Result<()> {
    if size.0 == 0 || size.1 == 0 || size.0 > max_dimension || size.1 > max_dimension {
        bail!(
            "image size {} x {} invalid for the screenshot (allowed: 1 to {max_dimension} per side)",
            size.0,
            size.1
        );
    }
    Ok(())
}

/// Renders a model in the browser without a window into a PNG file (animation at `time_ms`).
fn screenshot_model(
    config: &Config,
    vfs: &Vfs,
    model: &str,
    time_ms: f32,
    out: &Path,
) -> Result<()> {
    let (device, queue, info) = headless_device(config.backend)?;
    let size = (config.width, config.height);
    check_screenshot_size(size, device.limits().max_texture_dimension_2d)?;
    // The browser time is set to `time_ms` (`set_time`), the clock time does not matter.
    let mut browser = BrowserView::new(
        &device,
        &queue,
        CAPTURE_FORMAT,
        vfs,
        Some(model),
        0.0,
        config.shadow_quality,
        config.width,
    )?;
    // Effects up to `time_ms` in steps of 25 ms with the poses of the run time.
    let mut done = 0.0;
    while done < time_ms {
        let dt = (time_ms - done).min(EFFECT_STEP_MS);
        done += dt;
        browser.step_effects(vfs, dt, done);
    }
    browser.set_time(time_ms);
    browser.update(&device, &queue, vfs, 0.0);
    let params = browser.view_params(size, config.aspect.into(), 0.0);
    let lines = browser_lines(
        0,
        &format!("{:?}", info.backend),
        browser.describe(0.0),
        browser.text.line_height(),
    );
    // Pipelines and capture use the same format (`CAPTURE_FORMAT`).
    let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
        browser.render_scene(&device, &queue, vfs, encoder, view, size, &params);
        browser.draw_overlay(&device, &queue, encoder, view, size, &lines);
    })?;
    img.save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    log::info!("Screenshot {} saved", out.display());
    Ok(())
}

/// Renders the dialogs `names` without a level and without a window on a black background into a PNG file
/// (`config.width x config.height`, fonts with this width).
fn screenshot_dialogs(config: &Config, vfs: &Vfs, names: &[String], out: &Path) -> Result<()> {
    let (device, queue, _info) = headless_device(config.backend)?;
    let size = (config.width, config.height);
    check_screenshot_size(size, device.limits().max_texture_dimension_2d)?;
    let generator = opennml_client::ui_layer::atlas_generator(vfs);
    let mut dialogs = DialogBrowser::new(
        vfs,
        &device,
        CAPTURE_FORMAT,
        names,
        config.width,
        generator.as_deref(),
    )?;
    let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
        // Black, opaque background (a new texture is transparent).
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Dialog background"),
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
        dialogs.draw(vfs, &device, &queue, encoder, view, size);
    })?;
    img.save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    log::info!(
        "Screenshot {} saved (dialog {})",
        out.display(),
        dialogs.title()
    );
    Ok(())
}

/// Step size of the effects in the screenshot (`--screenshot`, `--time-ms`).
const EFFECT_STEP_MS: f32 = 25.0;

/// Renders the start view of the level or the random map without a window into a PNG file (animation at
/// `time_ms`).
fn screenshot(config: &Config, vfs: &Vfs, time_ms: f32, out: &Path) -> Result<()> {
    let (device, queue, info) = headless_device(config.backend)?;
    let size = (config.width, config.height);
    check_screenshot_size(size, device.limits().max_texture_dimension_2d)?;
    let mut map = match &config.random_map {
        // Random map: scene from the generator, without simulation.
        Some(spec) => {
            let (load, _) = app::random_map_load(vfs, spec)?;
            MapView::build(
                &device,
                &queue,
                CAPTURE_FORMAT,
                vfs,
                load,
                &config.view_options(),
            )?
        }
        None => MapView::load(
            &device,
            &queue,
            CAPTURE_FORMAT,
            vfs,
            &config.level,
            &config.view_options(),
            config.game_speed,
            |rules| match_setup(config, rules),
        )?,
    };
    map.update(&device, &queue, vfs, time_ms, config.animate);
    let params = map.view_params(size, config.aspect.into(), config.fog, time_ms);
    // Effects up to `time_ms` in steps of 25 ms (remainder in the last step) with the start camera.
    let view_proj = params.projection * params.view;
    let mut done = 0.0;
    while done < time_ms {
        let dt = (time_ms - done).min(EFFECT_STEP_MS);
        done += dt;
        map.advance_effects(vfs, view_proj, dt, config.animate.then_some(done));
    }
    map.prepare_weather(&device, &queue, &params);
    map.prepare_shadows(&device, &queue, &params);
    map.prepare_effects(
        &device,
        &queue,
        vfs,
        &params,
        config.animate.then_some(time_ms),
    );
    let info = OverlayInfo {
        fps: 0,
        backend: format!("{:?}", info.backend),
        adapter: info.name,
        level: map.scene.lvl_path.clone(),
        tiles: (map.scene.world.terrain.dim_x, map.scene.world.terrain.dim_z),
        objects: map.renderer.instance_count(),
        camera: Some((
            map.camera.distance(),
            map.camera.pitch_deg(),
            map.camera.target.x,
            map.camera.target.z,
        )),
        aspect: config.aspect,
        fog: config.fog,
        // Without a logic step: the particles stand at their start points.
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
        // Screenshots without a window have no sound.
        audio: "Sound: off (screenshot)".into(),
        sim: map.sim_line(),
        account: map
            .sim
            .as_ref()
            .map(opennml_client::game::sim_host::SimHost::account_line),
        messages: Vec::new(),
        economy: Vec::new(),
        combat: None,
        hovered: None,
        help: false,
    };
    let lines = overlay_lines(&info, map.text.line_height());
    // Pipelines and capture use the same format (`CAPTURE_FORMAT`).
    let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
        map.render_world(&device, &queue, encoder, view, size, &params);
        map.draw_overlay(&device, &queue, encoder, view, size, &lines);
    })?;
    img.save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    log::info!("Screenshot {} saved", out.display());
    Ok(())
}

/// Random map of the command line (`--random-map` with the `--random-*` values); the value ranges are checked by
/// [`opennml_randommap::generate`].
fn random_map_spec(cli: &Cli) -> Option<opennml_randommap::RandomMapSpec> {
    cli.random_map
        .as_ref()
        .map(|seed| opennml_randommap::RandomMapSpec {
            seed_text: seed.clone(),
            size: cli.random_size,
            players: cli.random_players,
            terrain: cli.random_terrain,
            landscape: cli.random_landscape,
            daytime: cli.random_daytime,
            game_type: cli.random_game_type,
        })
}

/// Values of the command line for [`Config::with_overrides`]. With `--slots`, `--nations` and `--teams` go into the
/// slots ([`config::merge_lobby_lists`]); in the settings, `slots` then replaces `nations`/`teams` as with
/// `sim_setup` ([`Config::set_lobby`]).
fn overrides(cli: &Cli) -> Result<Overrides> {
    let (slots, nations, teams) = match &cli.slots {
        Some(slots) => (
            Some(config::merge_lobby_lists(
                slots.clone(),
                cli.nations.as_deref(),
                cli.teams.as_deref(),
            )?),
            None,
            None,
        ),
        None => (None, cli.nations.clone(), cli.teams.clone()),
    };
    Ok(Overrides {
        backend: cli.backend,
        window_mode: cli.window_mode,
        monitor: cli.monitor,
        game_dir: cli.game_dir.clone(),
        level: cli.level.clone(),
        random_map: random_map_spec(cli),
        aspect: cli.aspect,
        fog: cli.fog.then_some(true),
        shadow_quality: cli.shadow_quality,
        particles_quality: cli.particles_quality,
        terrain_quality: cli.terrain_quality,
        light: cli.light.clone(),
        weather: cli.weather,
        volume_music: cli.volume_music,
        volume_sfx: cli.volume_sfx,
        volume_speech: cli.volume_speech,
        mute: cli.mute.then_some(true),
        hud: cli.hud.then_some(true),
        game_speed: cli.game_speed,
        game_mode: cli.game_mode,
        seed: cli.seed,
        nations,
        teams,
        map_explored: cli.map_explored.then_some(true),
        slots,
        ki_enabled: cli.ki,
        campaign_difficulty: cli.campaign_difficulty,
        script_enabled: cli.script,
    })
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?.with_overrides(overrides(&cli)?);
    let game_dir = find_game_dir(
        config.game_dir.as_deref(),
        remembered_file(cli.settings.as_deref(), false)
            .as_deref()
            .and_then(read_remembered),
    )?
    .path;
    let vfs = Vfs::open_game_dir(&game_dir)
        .with_context(|| format!("opening archives in {}", game_dir.display()))?;
    // Blood switch of the localisation (`BloodModeEnabled`, O-392): once per program, `bloodmode` toggles it.
    opennml_client::game::blood::init_from_localisation(&vfs);
    // `--mission`: level from campaign.ini; unknown names are an error before window and GPU.
    let config = match &cli.mission {
        Some(name) => config
            .with_mission(
                &opennml_ui::profiles::load_campaigns(&vfs),
                name,
                cli.script,
            )
            .map_err(anyhow::Error::msg)?,
        None => config,
    };
    config.check()?;
    log::info!("Settings: {config:?}");
    // An unknown model is an error before a window or GPU is created.
    if let Some(model) = &cli.model {
        find_model(&model_paths(&vfs), model)?;
    }
    // Likewise dialog names: unknown or without a file is an error before window and GPU.
    if !cli.dialog.is_empty() {
        dialogs::check_names(&vfs, &cli.dialog)?;
    }
    if let Some(out) = &cli.screenshot {
        return match (&cli.model, cli.dialog.is_empty()) {
            (Some(model), _) => screenshot_model(&config, &vfs, model, cli.time_ms, out),
            (None, false) => screenshot_dialogs(&config, &vfs, &cli.dialog, out),
            (None, true) => screenshot(&config, &vfs, cli.time_ms, out),
        };
    }

    let event_loop = EventLoop::<ControlEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    // The control channel is bound before the event loop starts; the log line names the actual port (the MCP
    // server reads it).
    let control = match cli.control {
        Some(addr) => {
            let server = control::start(addr, event_loop.create_proxy())
                .with_context(|| format!("opening control channel {addr}"))?;
            log::info!("Steuerkanal: {}", server.addr());
            Some(server)
        }
        None => None,
    };
    let dialog_settings = dialog_settings(cli.settings.as_deref(), &game_dir)?;
    let mut app = app::App::new(config, vfs, cli.model, cli.paused, cli.audio_capture)?
        .with_dialogs(cli.dialog, dialog_settings);
    event_loop.run_app(&mut app)?;
    app.finish_audio();
    // After `quit`, let the response go out before the process ends.
    if let Some(server) = &control
        && app.quit_requested()
    {
        server.wait_quit_written(Duration::from_secs(2));
    }
    app.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_ms_must_be_finite_and_not_negative() {
        assert_eq!(parse_time_ms("0"), Ok(0.0));
        assert_eq!(parse_time_ms("1250.5"), Ok(1250.5));
        for text in ["inf", "-inf", "NaN", "-1", "1e40"] {
            let error = parse_time_ms(text).unwrap_err();
            assert!(error.contains("invalid"), "{text}: {error}");
        }
        assert!(
            parse_time_ms("abc")
                .unwrap_err()
                .contains("is not a number")
        );
        // Via the command line: an error instead of a panic in the animation.
        assert!(Cli::try_parse_from(["opennml-viewer", "--time-ms", "inf"]).is_err());
        let cli = Cli::try_parse_from(["opennml-viewer", "--time-ms", "250"]).unwrap();
        assert_eq!(cli.time_ms, 250.0);
        assert_eq!(
            Cli::try_parse_from(["opennml-viewer"]).unwrap().time_ms,
            1000.0
        );
    }

    #[test]
    fn quality_switches_report_german_range_errors() {
        for (flag, value, max) in [
            ("--shadow-quality", "4", 3),
            ("--particles-quality", "3", 2),
            ("--terrain-quality", "3", 2),
        ] {
            let error = Cli::try_parse_from(["opennml-viewer", flag, value])
                .err()
                .unwrap()
                .to_string();
            assert!(
                error.contains("invalid") && error.contains(&format!("0 to {max}")),
                "{flag}: {error}"
            );
        }
        let error = Cli::try_parse_from(["opennml-viewer", "--shadow-quality", "many"])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("is not an integer"), "{error}");
        let cli = Cli::try_parse_from(["opennml-viewer", "--particles-quality", "1"]).unwrap();
        assert_eq!(cli.particles_quality, Some(1));
    }

    #[test]
    fn control_address_must_be_loopback_and_excludes_screenshots() {
        let cli = Cli::try_parse_from(["opennml-viewer", "--control", "127.0.0.1:0"]).unwrap();
        assert_eq!(cli.control, Some("127.0.0.1:0".parse().unwrap()));
        assert!(Cli::try_parse_from(["opennml-viewer", "--control", "[::1]:4711"]).is_ok());
        assert!(Cli::try_parse_from(["opennml-viewer", "--control", "0.0.0.0:4711"]).is_err());
        assert!(Cli::try_parse_from(["opennml-viewer", "--control", "4711"]).is_err());
        assert!(
            Cli::try_parse_from([
                "opennml-viewer",
                "--control",
                "127.0.0.1:0",
                "--screenshot",
                "x.png"
            ])
            .is_err()
        );
        assert_eq!(
            Cli::try_parse_from(["opennml-viewer"]).unwrap().control,
            None
        );
    }

    #[test]
    fn paused_start_needs_the_control_channel() {
        assert!(Cli::try_parse_from(["opennml-viewer", "--paused"]).is_err());
        let cli = Cli::try_parse_from(["opennml-viewer", "--control", "127.0.0.1:0", "--paused"])
            .unwrap();
        assert!(cli.paused);
        let cli = Cli::try_parse_from(["opennml-viewer", "--control", "127.0.0.1:0"]).unwrap();
        assert!(!cli.paused);
        assert!(!Cli::try_parse_from(["opennml-viewer"]).unwrap().paused);
    }

    #[test]
    fn environment_switches_parse_and_check_ranges() {
        let cli = Cli::try_parse_from([
            "opennml-viewer",
            "--light",
            "Sommer Nacht",
            "--weather",
            "snow",
            "--shadow-quality",
            "3",
            "--terrain-quality",
            "2",
        ])
        .unwrap();
        assert_eq!(cli.light, Some(LightSetting::Preset("Sommer Nacht".into())));
        assert_eq!(cli.weather, Some(WeatherSetting::Snow));
        assert_eq!(
            (cli.shadow_quality, cli.terrain_quality),
            (Some(3), Some(2))
        );
        assert!(Cli::try_parse_from(["opennml-viewer", "--shadow-quality", "4"]).is_err());
        assert!(Cli::try_parse_from(["opennml-viewer", "--particles-quality", "3"]).is_err());
        assert!(Cli::try_parse_from(["opennml-viewer", "--weather", "hail"]).is_err());
    }

    /// Without `--hud` and without `hud = true` in the settings the game UI is off.
    #[test]
    fn viewer_hud_is_off_by_default() {
        assert!(!Config::default().hud);
        let cli = Cli::try_parse_from(["opennml-viewer"]).unwrap();
        assert!(!cli.hud);
        assert!(
            !Config::default()
                .with_overrides(overrides(&cli).unwrap())
                .hud
        );
        let cli = Cli::try_parse_from(["opennml-viewer", "--hud"]).unwrap();
        assert!(
            Config::default()
                .with_overrides(overrides(&cli).unwrap())
                .hud
        );
        // From the settings file; without `--hud` the value of the file stays.
        let file = Config::from_toml("hud = true\n").unwrap();
        assert!(file.hud);
        let cli = Cli::try_parse_from(["opennml-viewer"]).unwrap();
        assert!(file.with_overrides(overrides(&cli).unwrap()).hud);
    }

    #[test]
    fn audio_switches_parse_and_check_ranges() {
        let cli = Cli::try_parse_from([
            "opennml-viewer",
            "--volume-music",
            "0",
            "--volume-sfx",
            "1000",
            "--mute",
            "--audio-capture",
            "sound.wav",
        ])
        .unwrap();
        assert_eq!(
            (cli.volume_music, cli.volume_sfx, cli.volume_speech),
            (Some(0), Some(1000), None)
        );
        assert!(cli.mute);
        assert_eq!(cli.audio_capture, Some(PathBuf::from("sound.wav")));
        let error = Cli::try_parse_from(["opennml-viewer", "--volume-speech", "1001"])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("0 to 1000"), "{error}");
        assert!(
            Cli::try_parse_from([
                "opennml-viewer",
                "--audio-capture",
                "a.wav",
                "--screenshot",
                "b.png"
            ])
            .is_err()
        );
    }

    #[test]
    fn match_switches_parse_lists_and_negative_modes() {
        let cli = Cli::try_parse_from([
            "opennml-viewer",
            "--game-speed",
            "30",
            "--game-mode",
            "-1",
            "--seed",
            "5",
            "--nations",
            "SPA,eng",
            "--teams",
            "0,0,1",
        ])
        .unwrap();
        assert_eq!(
            (cli.game_speed, cli.game_mode, cli.seed),
            (Some(30), Some(-1), Some(5))
        );
        assert_eq!(cli.nations, Some(vec!["SPA".into(), "eng".into()]));
        assert_eq!(cli.teams, Some(vec![0, 0, 1]));
        assert!(!cli.map_explored);
        let cli = Cli::try_parse_from(["opennml-viewer", "--map-explored"]).unwrap();
        assert!(cli.map_explored);
        let error = Cli::try_parse_from(["opennml-viewer", "--game-speed", "31"])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("0 to 30"), "{error}");
    }

    #[test]
    fn dialog_and_model_conflict() {
        let cli =
            Cli::try_parse_from(["opennml-viewer", "--dialog", "options,videooptions"]).unwrap();
        assert_eq!(cli.dialog, ["options", "videooptions"]);
        let cli = Cli::try_parse_from([
            "opennml-viewer",
            "--dialog",
            "start",
            "--screenshot",
            "a.png",
        ])
        .unwrap();
        assert_eq!(cli.dialog, ["start"]);
        assert!(
            Cli::try_parse_from(["opennml-viewer"])
                .unwrap()
                .dialog
                .is_empty()
        );
        let error =
            Cli::try_parse_from(["opennml-viewer", "--dialog", "start", "--model", "a.rdo"])
                .err()
                .unwrap()
                .to_string();
        assert!(
            error.contains("--dialog") && error.contains("--model"),
            "{error}"
        );
        // An empty list is not a value.
        assert!(Cli::try_parse_from(["opennml-viewer", "--dialog"]).is_err());
    }

    #[test]
    fn dialog_settings_need_the_switch_and_stay_outside_the_game_dir() {
        let game = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        // Without the switch: no file, even if NML_SETTINGS or LOCALAPPDATA are set.
        let cli = Cli::try_parse_from(["opennml-viewer", "--dialog", "options"]).unwrap();
        assert_eq!(cli.settings, None);
        assert_eq!(dialog_settings(None, game.path()).unwrap(), None);
        let own = home.path().join("Engine.ini");
        let cli = Cli::try_parse_from([
            "opennml-viewer",
            "--dialog",
            "options",
            "--settings",
            own.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(
            dialog_settings(cli.settings.as_deref(), game.path()).unwrap(),
            Some(own)
        );
        // Only with --dialog; never into the installation.
        assert!(Cli::try_parse_from(["opennml-viewer", "--settings", "a.ini"]).is_err());
        let inside = game.path().join("Engine.ini");
        let error = dialog_settings(Some(&inside), game.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("installation"), "{error}");
    }

    /// `--random-map <SEED>` with the defaults of the six menu values; excludes `--level` and `--mission`, the other
    /// `--random-*` need it. The value goes into the settings via `overrides`.
    #[test]
    fn random_map_options_parse_and_exclude_level() {
        let cli = Cli::try_parse_from(["opennml-viewer", "--random-map", "ZKA"]).unwrap();
        let spec = opennml_randommap::RandomMapSpec {
            seed_text: "ZKA".into(),
            size: 1,
            players: 4,
            terrain: 0,
            landscape: 4,
            daytime: 0,
            game_type: 0,
        };
        assert_eq!(random_map_spec(&cli), Some(spec.clone()));
        assert_eq!(
            Config::default()
                .with_overrides(overrides(&cli).unwrap())
                .random_map,
            Some(spec)
        );
        let cli = Cli::try_parse_from([
            "opennml-viewer",
            "--random-map",
            "Hello World",
            "--random-size",
            "2",
            "--random-players",
            "8",
            "--random-terrain",
            "5",
            "--random-landscape",
            "1",
            "--random-daytime",
            "2",
            "--random-game-type",
            "3",
        ])
        .unwrap();
        assert_eq!(
            random_map_spec(&cli),
            Some(opennml_randommap::RandomMapSpec {
                seed_text: "Hello World".into(),
                size: 2,
                players: 8,
                terrain: 5,
                landscape: 1,
                daytime: 2,
                game_type: 3,
            })
        );
        // Without the switch no random map.
        let cli = Cli::try_parse_from(["opennml-viewer"]).unwrap();
        assert_eq!(random_map_spec(&cli), None);
        assert_eq!(
            Config::default()
                .with_overrides(overrides(&cli).unwrap())
                .random_map,
            None
        );
        // Exclusions and dependencies.
        for args in [
            &["--random-map", "ZKA", "--level", "data/levels/x.lvl"][..],
            &["--random-map", "ZKA", "--mission", "kamp1-1"],
            &["--random-size", "2"],
            &["--random-players", "3"],
            &["--random-game-type", "1"],
        ] {
            let mut argv = vec!["opennml-viewer"];
            argv.extend_from_slice(args);
            assert!(Cli::try_parse_from(&argv).is_err(), "{args:?}");
        }
        // Allowed with a screenshot.
        assert!(
            Cli::try_parse_from([
                "opennml-viewer",
                "--random-map",
                "ZKA",
                "--screenshot",
                "a.png"
            ])
            .is_ok()
        );
    }

    #[test]
    fn screenshot_size_must_be_within_device_limits() {
        assert!(check_screenshot_size((1280, 720), 8192).is_ok());
        assert!(check_screenshot_size((8192, 8192), 8192).is_ok());
        for size in [(0, 720), (1280, 0), (8193, 720), (1280, 9000)] {
            let error = check_screenshot_size(size, 8192).unwrap_err();
            assert!(format!("{error}").contains("invalid"), "{error}");
        }
    }
}
