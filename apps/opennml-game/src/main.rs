//! `opennml-game`: the program of the Rust reimplementation. Starts into the main menu like the original: read the
//! settings file in the format of `Engine.ini`, open the window according to `ScreenXSize`/`ScreenYSize`/
//! `ScreenFormat`, run the menu UI (`opennml_client::menu_host::MenuHost`, `MenuApp` with `start_menu`). Quitting via
//! `QUIT` in the main menu or closing the window; both write the settings (O-32). Without `egui`.
//!
//! Check runs: `--mute --window-mode windowed`, with `--control` a control channel like in the viewer (subset,
//! `game::AVAILABLE`), with `--screenshot` an image without a window, with `--loading-shot <folder>` one image of the
//! loading screen per loading phase (the blocking loading is otherwise invisible through the control channel).
//!
//! Network play: `--host [PORT] --net-map <path.lvl>` or `--join <ADDRESS[:PORT]>` start a network match without a
//! lobby ([`net_start`]); until the start the program shows the main menu with input locked, a failed join releases
//! it again. The menu path (`LAN1`, `LAN3`, `LAN2`) is run by the network session in the flow
//! (`opennml_client::net_session`); both use the transport over `std::net` (`SystemTransport`, with `--net-loopback` only
//! 127.0.0.1).

mod game;
mod game_dir;
mod net_start;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use opennml_client::clock::parse_time_ms;
use opennml_client::control::{self, ControlEvent};
use opennml_client::settings::{load_settings, settings_path_for};
use opennml_client::window::WindowMode;
use opennml_control::parse_control_addr;
use opennml_rda::{SearchInput, Vfs, read_remembered, remembered_file};
use winit::event_loop::{ControlFlow, EventLoop};

use crate::game::{Game, GameOptions};
use crate::game_dir::{FolderPrompt, SystemPrompt, resolve_game_dir};

/// Time of the screenshot without `--time-ms` (ms).
const DEFAULT_TIME_MS: f32 = 1000.0;

#[derive(Parser)]
#[command(
    name = "opennml-game",
    about = "No Man's Land (Rust reimplementation): starts into the main menu"
)]
#[command(group(clap::ArgGroup::new("net").args(["host", "join"])))]
struct Cli {
    /// Only this location as the original installation; without it, search as in docs/programs.md, section "Game
    /// directory"
    #[arg(long)]
    game_dir: Option<PathBuf>,
    /// Settings file in the format of Engine.ini; without it NML_SETTINGS, otherwise
    /// %LOCALAPPDATA%\OpenNML\Engine.ini (never in the installation)
    #[arg(long)]
    settings: Option<PathBuf>,
    /// windowed, borderless or exclusive; without it from ScreenFormat (!= 0 window, 0 fullscreen)
    #[arg(long)]
    window_mode: Option<WindowMode>,
    /// Monitor index (0-based)
    #[arg(long)]
    monitor: Option<usize>,
    /// Start without sound output
    #[arg(long)]
    mute: bool,
    /// Without gamma ramp: the window image stays as at gamma 1.0, the slider in VIDEOOPTIONS still computes and saves
    /// (screenshots never contain the ramp, as in the original)
    #[arg(long)]
    no_gamma: bool,
    /// Open the control channel on IP:PORT (loopback only; port 0 = choose a free one), see docs/programs.md,
    /// section "opennml-game"
    #[arg(long, value_parser = parse_control_addr, conflicts_with = "screenshot")]
    control: Option<SocketAddr>,
    /// Start with the virtual clock halted at 0 ms (only with --control; not in network play, the turns run in real
    /// time)
    #[arg(long, requires = "control", conflicts_with = "net")]
    paused: bool,
    /// Run the menu without a window until --time-ms, save as PNG and quit
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Time of the screenshot in milliseconds (finite, >= 0; default 1000)
    #[arg(long, value_parser = parse_time_ms, requires = "screenshot")]
    time_ms: Option<f32>,
    /// Only for check runs: one image of the loading screen per loading phase (0, 40, 70, 100 %) as PNG in this folder
    #[arg(long, conflicts_with = "screenshot")]
    loading_shot: Option<PathBuf>,
    /// Network play as host: listen on PORT (without a value: NetPort of the settings; 0 = 2302), wait for
    /// --net-players - 1 clients and set up the match on --net-map
    #[arg(
        long,
        value_name = "PORT",
        num_args = 0..=1,
        conflicts_with_all = ["join", "screenshot"],
        requires = "net_map"
    )]
    host: Option<Option<u16>>,
    /// Network play as client: connect to the host at ADDRESS[:PORT] (without a port: 2302); without a host back to the
    /// main menu after 10 s
    #[arg(
        long,
        value_name = "ADDRESS[:PORT]",
        value_parser = net_start::parse_join_addr,
        conflicts_with = "screenshot"
    )]
    join: Option<SocketAddr>,
    /// Map of the network match (path of the .lvl under data/levels/multiplayer/; only with --host)
    #[arg(long, value_name = "PATH", requires = "host")]
    net_map: Option<String>,
    /// Humans of the network match including the host (2 to 8; only with --host)
    #[arg(long, value_name = "N", default_value_t = 2, value_parser = clap::value_parser!(u8).range(2..=8),
        requires = "host")]
    net_players: u8,
    /// AI slots of the network match after the humans (0 to 7; only with --host)
    #[arg(long, value_name = "K", default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=7),
        requires = "host")]
    net_ai: u8,
    /// Turn length L (1 to 10, raster T = 4L, V = 2L; default 2 = 8/4 as in the original; only with --host)
    #[arg(long, value_name = "L", default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=10),
        requires = "host", conflicts_with = "turn_raster")]
    turn_steps: u8,
    /// Turn raster T/V in ticks (1 <= V < T <= 40, e.g. 8/4 original, 4/2, 2/1; only with --host)
    #[arg(long, value_name = "T/V", value_parser = net_start::parse_raster, requires = "host")]
    turn_raster: Option<opennml_net::TurnConfig>,
    /// Player name in the network (default: profile name NetName, otherwise "Spieler")
    #[arg(long, value_name = "NAME", requires = "net")]
    net_name: Option<String>,
    /// Fixed seed of the network match for check runs (default: clock of the host; only with --host)
    #[arg(long, value_name = "S", requires = "host")]
    net_seed: Option<u32>,
    /// Only for check runs: a host (--host or from the menu) listens only on 127.0.0.1 instead of on all interfaces
    /// (no Windows firewall exception needed; clients only on the same machine), the discovery additionally asks
    /// 127.0.0.1
    #[arg(long)]
    net_loopback: bool,
    /// Only for check runs: the client (--join or from the menu) does not listen on a free port and counts as
    /// unreachable for the host in the successor list
    #[arg(long)]
    net_no_listen: bool,
}

impl Cli {
    /// Does a command-line network path (`--host` or `--join`) run?
    fn is_net(&self) -> bool {
        self.host.is_some() || self.join.is_some()
    }

    /// May the start open the folder dialog? Only on a plain start, never in check runs and network paths.
    fn allows_dialog(&self) -> bool {
        self.game_dir.is_none()
            && self.screenshot.is_none()
            && self.loading_shot.is_none()
            && self.control.is_none()
            && !self.is_net()
    }

    /// Does the program run without the menu level? In a network path and for the screenshot.
    fn skips_menu_level(&self) -> bool {
        self.is_net() || self.screenshot.is_some()
    }

    /// The network switches for [`net_start::launch`].
    fn net_args(&self) -> net_start::NetArgs {
        net_start::NetArgs {
            host: self.host,
            join: self.join,
            map: self.net_map.clone(),
            players: self.net_players,
            ai: self.net_ai,
            raster: self.turn_raster.unwrap_or_else(|| {
                // The range of `--turn-steps` is limited to 1 ... 10 by the parser.
                opennml_net::TurnConfig::from_turn_steps(self.turn_steps).unwrap_or_default()
            }),
            name: self.net_name.clone(),
            seed: self.net_seed,
            listen: !self.net_no_listen,
        }
    }

    /// Transport of both network paths: `std::net`, discovery port 2303, with `--net-loopback` only 127.0.0.1.
    fn transport(&self) -> opennml_net::SystemTransport {
        opennml_net::SystemTransport {
            loopback: self.net_loopback,
            discovery_port: opennml_net::DISCOVERY_PORT,
        }
    }
}

/// Location of the settings file: `--settings`, otherwise `NML_SETTINGS` or `%LOCALAPPDATA%\OpenNML\Engine.ini`; a
/// location in the installation is an error.
fn settings_path(explicit: Option<&Path>, game_dir: &Path) -> Result<PathBuf> {
    settings_path_for(explicit, game_dir)
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let remembered = remembered_file(cli.settings.as_deref(), true);
    let input = SearchInput::from_environment(
        cli.game_dir.as_deref(),
        remembered.as_deref().and_then(read_remembered),
    );
    let game_dir = resolve_game_dir(
        &input,
        remembered.as_deref(),
        cli.settings.as_deref(),
        cli.allows_dialog()
            .then_some(&mut SystemPrompt as &mut dyn FolderPrompt),
    )?;
    let vfs = Vfs::open_game_dir(&game_dir)
        .with_context(|| format!("opening archives in {}", game_dir.display()))?;
    // Blood switch of the localisation (`BloodModeEnabled`, O-392): once per program, `bloodmode` toggles it.
    opennml_client::game::blood::init_from_localisation(&vfs);
    let settings = settings_path(cli.settings.as_deref(), &game_dir)?;
    log::info!("Settings file: {}", settings.display());
    let mut config = load_settings(&settings, &vfs);
    // `--host`, `--join` and the screenshot run without the menu level, only in memory; the settings file stays.
    let saved_menu_level = cli
        .skips_menu_level()
        .then(|| game::skip_menu_level(&mut config));
    if let Some(out) = &cli.screenshot {
        let time_ms = cli.time_ms.unwrap_or(DEFAULT_TIME_MS);
        return game::screenshot(&vfs, config, time_ms, out);
    }

    // Network start before the window: the host checks map and setup and listens (an error ends here with an error
    // message and an error code); the client does not connect until the event loop.
    let launch = net_start::launch(
        &cli.net_args(),
        &vfs,
        &mut config,
        &settings,
        Box::new(cli.transport()),
    )
    .inspect_err(|e| {
        log::error!("Network start: {e:#}");
    })?;

    let event_loop = EventLoop::<ControlEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    // As in the viewer: the port is bound before the event loop, the log line names it (the MCP server reads it).
    let control = match cli.control {
        Some(addr) => {
            let server = control::start(addr, event_loop.create_proxy())
                .with_context(|| format!("opening control channel {addr}"))?;
            log::info!("Steuerkanal: {}", server.addr());
            Some(server)
        }
        None => None,
    };
    let transport = cli.transport();
    let options = GameOptions {
        mode: game::window_mode(cli.window_mode, &config),
        monitor: cli.monitor,
        mute: cli.mute,
        gamma: !cli.no_gamma,
        paused: cli.paused,
        loading_shot: cli.loading_shot,
        control: control.is_some(),
        transport,
        net_listen: !cli.net_no_listen,
    };
    let mut game = Game::new(vfs, config, settings, options);
    game.set_saved_menu_level(saved_menu_level.flatten());
    if let Some(launch) = launch {
        game.set_net(launch);
    }
    let run = event_loop.run_app(&mut game);
    // Every program end writes the settings (O-32); after closing the window and `QUIT` this has already
    // happened and is not repeated.
    game.shutdown();
    run?;
    // After `quit`, let the response go out before the process ends.
    if let Some(server) = &control
        && game.quit_requested()
    {
        server.wait_quit_written(Duration::from_secs(2));
    }
    game.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_client::settings::default_settings_path;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("opennml-game").chain(args.iter().copied()))
    }

    #[test]
    fn dialog_only_for_plain_start() {
        assert!(parse(&[]).unwrap().allows_dialog());
        for args in [
            &["--game-dir", "x"][..],
            &["--screenshot", "x.png"],
            &["--loading-shot", "x"],
            &["--control", "127.0.0.1:0"],
            &["--join", "127.0.0.1"],
            &["--host", "--net-map", "m.lvl"],
        ] {
            assert!(!parse(args).unwrap().allows_dialog(), "{args:?}");
        }
    }

    #[test]
    fn control_requires_loopback() {
        let cli = parse(&["--control", "127.0.0.1:0"]).unwrap();
        assert_eq!(cli.control, Some("127.0.0.1:0".parse().unwrap()));
        assert!(parse(&["--control", "[::1]:4711"]).is_ok());
        for addr in ["0.0.0.0:4711", "192.168.1.2:4711", "4711", "localhost"] {
            assert!(parse(&["--control", addr]).is_err(), "{addr}");
        }
        assert_eq!(
            parse(&[]).unwrap().control,
            None,
            "without --control no control channel"
        );
    }

    #[test]
    fn paused_requires_control() {
        assert!(parse(&["--paused"]).is_err());
        let cli = parse(&["--control", "127.0.0.1:0", "--paused"]).unwrap();
        assert!(cli.paused);
        assert!(!parse(&["--control", "127.0.0.1:0"]).unwrap().paused);
        assert!(!parse(&[]).unwrap().paused);
    }

    #[test]
    fn screenshot_conflicts_with_control() {
        assert!(parse(&["--control", "127.0.0.1:0", "--screenshot", "x.png"]).is_err());
        let cli = parse(&["--screenshot", "x.png", "--time-ms", "250"]).unwrap();
        assert_eq!(cli.screenshot, Some(PathBuf::from("x.png")));
        assert_eq!(cli.time_ms, Some(250.0));
        assert_eq!(parse(&["--screenshot", "x.png"]).unwrap().time_ms, None);
        // `--time-ms` applies only to the screenshot; infinite, NaN and negative are errors.
        assert!(parse(&["--time-ms", "250"]).is_err());
        for bad in ["inf", "NaN", "-1"] {
            assert!(
                parse(&["--screenshot", "x.png", "--time-ms", bad]).is_err(),
                "{bad}"
            );
        }
        // The remaining switches of the check runs.
        let cli = parse(&["--mute", "--window-mode", "windowed", "--monitor", "1"]).unwrap();
        assert!(cli.mute);
        assert!(!cli.no_gamma, "gamma ramp on by default");
        assert!(parse(&["--no-gamma"]).unwrap().no_gamma);
        assert_eq!(cli.window_mode, Some(WindowMode::Windowed));
        assert_eq!(cli.monitor, Some(1));
        assert!(parse(&["--window-mode", "fullscreen"]).is_err());
    }

    #[test]
    fn loading_shot_takes_a_folder() {
        let cli = parse(&["--loading-shot", "out/loading"]).unwrap();
        assert_eq!(cli.loading_shot, Some(PathBuf::from("out/loading")));
        assert_eq!(parse(&[]).unwrap().loading_shot, None);
        assert!(parse(&["--loading-shot"]).is_err(), "folder is missing");
        assert!(parse(&["--loading-shot", "x", "--screenshot", "y.png"]).is_err());
    }

    /// `--host` and `--join` run without the menu level, only in memory; the settings file keeps the saved value
    /// (test of the host: `shutdown_keeps_the_saved_menu_level_...`).
    #[test]
    fn cli_net_paths_skip_the_menu_level() {
        let cases: [(&[&str], bool); 4] = [
            (&["--host", "--net-map", "m.lvl"], true),
            (&["--join", "127.0.0.1"], true),
            (&["--screenshot", "x.png"], false),
            (&[], false),
        ];
        for (args, net) in cases {
            let cli = parse(args).unwrap();
            assert_eq!(cli.is_net(), net, "{args:?}");
            let mut config = opennml_formats::engine_ini::EngineConfig {
                is_menu_level: true,
                ..Default::default()
            };
            assert_eq!(cli.skips_menu_level(), net || cli.screenshot.is_some());
            let saved = cli
                .skips_menu_level()
                .then(|| game::skip_menu_level(&mut config));
            if cli.skips_menu_level() {
                assert!(!config.is_menu_level, "{args:?}: off in memory");
                assert_eq!(saved, Some(Some(true)), "{args:?}: saved value remembered");
            } else {
                assert!(config.is_menu_level, "{args:?}: menu path stays");
            }
        }
        // A saved checkmark that is off stays off, there is nothing to write back.
        let mut config = opennml_formats::engine_ini::EngineConfig {
            is_menu_level: false,
            ..Default::default()
        };
        assert_eq!(game::skip_menu_level(&mut config), None);
        assert!(!config.is_menu_level);
    }

    /// `--host` and `--join` exclude each other, as do `--screenshot` and `--paused`.
    // QUIRK E-288: deviation
    #[test]
    fn cli_rejects_host_with_join() {
        assert!(parse(&["--host", "--net-map", "m.lvl", "--join", "127.0.0.1"]).is_err());
        assert!(parse(&["--join", "127.0.0.1", "--screenshot", "x.png"]).is_err());
        assert!(parse(&["--host", "--net-map", "m.lvl", "--screenshot", "x.png"]).is_err());
        assert!(
            parse(&[
                "--join",
                "127.0.0.1",
                "--control",
                "127.0.0.1:0",
                "--paused"
            ])
            .is_err()
        );
        let cli = parse(&["--join", "127.0.0.1:23020", "--net-name", "Guest"]).unwrap();
        assert_eq!(cli.join, Some("127.0.0.1:23020".parse().unwrap()));
        assert_eq!(cli.net_name.as_deref(), Some("Guest"));
        assert_eq!(
            parse(&["--join", "10.0.0.7"]).unwrap().join,
            Some("10.0.0.7:2302".parse().unwrap()),
            "without a port the game port"
        );
        assert!(parse(&["--join", "1.2.3.4:x"]).is_err());
        // The remaining network switches only with a network start.
        assert!(parse(&["--net-name", "Guest"]).is_err());
        assert!(parse(&["--net-players", "3"]).is_err());
        assert!(parse(&["--join", "127.0.0.1", "--net-players", "3"]).is_err());
        assert!(parse(&["--join", "127.0.0.1", "--net-seed", "3"]).is_err());
        let cli = parse(&[]).unwrap();
        assert_eq!((cli.host, cli.join), (None, None));
        assert!(!cli.net_loopback);
        // `--net-loopback` also applies to the client and the host from the menu (without `--host`).
        assert!(parse(&["--net-loopback"]).unwrap().net_loopback);
        let cli = parse(&["--join", "127.0.0.1", "--net-loopback"]).unwrap();
        assert!(cli.net_loopback);
        assert_eq!(
            cli.transport(),
            opennml_net::SystemTransport {
                loopback: true,
                discovery_port: 2303
            }
        );
        assert_eq!(
            parse(&[]).unwrap().transport(),
            opennml_net::SystemTransport {
                loopback: false,
                discovery_port: 2303
            },
            "without the switch on all interfaces, discovery port 2303"
        );
    }

    /// `--net-no-listen` applies to `--join` and the menu path; without the switch the client listens.
    #[test]
    fn net_no_listen_switches_the_client_listener_off() {
        let cli = parse(&[]).unwrap();
        assert!(!cli.net_no_listen);
        assert!(cli.net_args().listen, "default: the client listens");
        let cli = parse(&["--join", "127.0.0.1", "--net-no-listen"]).unwrap();
        assert!(cli.net_no_listen);
        assert!(!cli.net_args().listen);
        assert!(parse(&["--net-no-listen"]).unwrap().net_no_listen);
    }

    /// `--turn-raster T/V`: parser, limits, exclusion of `--turn-steps`, assembly in `NetArgs`.
    // QUIRK E-298: deviation
    #[test]
    fn cli_turn_raster_parses_and_excludes_turn_steps() {
        let base = ["--host", "--net-map", "m.lvl"];
        let with = |extra: &[&str]| {
            let mut v = base.to_vec();
            v.extend_from_slice(extra);
            parse(&v)
        };
        let cli = with(&[]).unwrap();
        assert_eq!(cli.turn_raster, None);
        assert_eq!(
            cli.net_args().raster,
            opennml_net::TurnConfig::default(),
            "without either the raster of the original"
        );
        let cli = with(&["--turn-raster", "2/1"]).unwrap();
        assert_eq!(cli.net_args().raster.raw(), (2, 1));
        let cli = with(&["--turn-steps", "3"]).unwrap();
        assert_eq!(cli.net_args().raster.raw(), (12, 6));
        assert!(
            with(&["--turn-raster", "2/1", "--turn-steps", "3"]).is_err(),
            "the two exclude each other"
        );
        for bad in ["2/2", "1/0", "41/4", "8", "a/b", "8/", "/4", ""] {
            assert!(with(&["--turn-raster", bad]).is_err(), "{bad}");
        }
        assert!(
            parse(&["--turn-raster", "2/1"]).is_err(),
            "only with --host"
        );
    }

    /// `--net-map` is mandatory with `--host`; values and defaults of the host switches.
    // QUIRK E-288, E-298: deviation
    #[test]
    fn cli_requires_net_map_with_host() {
        assert!(parse(&["--host"]).is_err());
        assert!(parse(&["--host", "23020"]).is_err());
        assert!(
            parse(&["--net-map", "m.lvl"]).is_err(),
            "map without --host"
        );
        let cli = parse(&["--host", "--net-map", "data/levels/multiplayer/m.lvl"]).unwrap();
        assert_eq!(cli.host, Some(None), "without a value: NetPort");
        assert_eq!(
            cli.net_map.as_deref(),
            Some("data/levels/multiplayer/m.lvl")
        );
        assert_eq!(
            (cli.net_players, cli.net_ai, cli.turn_steps, cli.net_seed),
            (2, 0, 2, None)
        );
        assert!(!cli.net_loopback);
        let cli = parse(&[
            "--host",
            "0",
            "--net-map",
            "m.lvl",
            "--net-players",
            "8",
            "--net-ai",
            "0",
            "--turn-steps",
            "10",
            "--net-name",
            "Anna",
            "--net-seed",
            "123",
            "--net-loopback",
        ])
        .unwrap();
        assert_eq!(cli.host, Some(Some(0)));
        assert_eq!(
            (cli.net_players, cli.net_ai, cli.turn_steps, cli.net_seed),
            (8, 0, 10, Some(123))
        );
        assert!(cli.net_loopback);
        // `--turn-raster` with all remaining host switches, without `--turn-steps`.
        let cli = parse(&[
            "--host",
            "--net-map",
            "m.lvl",
            "--net-players",
            "3",
            "--turn-raster",
            "4/2",
            "--net-loopback",
        ])
        .unwrap();
        assert_eq!(cli.net_args().raster.raw(), (4, 2));
        assert_eq!(cli.net_args().players, 3);
        assert!(cli.net_loopback);
        assert!(
            parse(&["--turn-raster", "4/2", "--net-loopback"]).is_err(),
            "raster only with --host"
        );
        assert_eq!(
            parse(&["--host", "4711", "--net-map", "m.lvl"])
                .unwrap()
                .host,
            Some(Some(4711))
        );
        for bad in [
            ["--net-players", "1"],
            ["--net-players", "9"],
            ["--turn-steps", "0"],
            ["--turn-steps", "11"],
            ["--net-ai", "8"],
        ] {
            let mut args = vec!["--host", "--net-map", "m.lvl"];
            args.extend(bad);
            assert!(parse(&args).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn settings_flag_overrides_the_default_path() {
        let game = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let own = home.path().join("own").join("Engine.ini");
        let cli = parse(&["--settings", own.to_str().unwrap()]).unwrap();
        assert_eq!(cli.settings.as_deref(), Some(own.as_path()));
        assert_eq!(
            settings_path(cli.settings.as_deref(), game.path()).unwrap(),
            own
        );
        // Without the switch: NML_SETTINGS or %LOCALAPPDATA%\OpenNML\Engine.ini (the same way as the host).
        let cli = parse(&[]).unwrap();
        match default_settings_path() {
            Some(default) => assert_eq!(
                settings_path(cli.settings.as_deref(), game.path()).unwrap(),
                default
            ),
            None => assert!(settings_path(None, game.path()).is_err()),
        }
        // Never into the installation, not even via --settings.
        let inside = game.path().join("Engine.ini");
        let err = settings_path(Some(&inside), game.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("installation"), "{err}");
    }
}
