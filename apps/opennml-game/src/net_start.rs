//! Network start by command line (E-254, E-262, E-288) and `net_status`.
//!
//! Command-line path without a lobby dialog:
//! - `--host [PORT]`: listens through the [`Transport`] of the program on the game port ([`host_port`]: without a value
//!   `NetPort` of the settings, 0 = 2302) and creates the lobby from the setup [`host_setup`] ([`host_core`]): one row
//!   per human (`--net-players`, row 0 the host and immediately ready), nation, team and color in order from the
//!   default of the map. The computers (`--net-ai`) appear only in the setup, not in the lobby; one machine too many
//!   therefore gets `Reject` "The match is full" from the core. As soon as every row has a human with a machine and all
//!   are ready, the host starts via `HostCore::start_from_lobby` ([`NetLaunch::pump`]): setup (resources, living space,
//!   time limit and "map explored" from the settings of the host, seed [`wall_seed`] or `--net-seed`, network flag),
//!   slots in row order ([`seats`]), per machine its slot, the game speed of the host, the session name
//!   (`opennml_client::net_session::session_name`).
//! - `--join <ADDRESS[:PORT]>`: connects in the event loop ([`Transport::connect`], non-blocking) and signs on; after
//!   the first lobby state the client sets itself ready (`ClientCore::set_auto_ready`). Without a host, without an
//!   answer or rejected: after at most [`JOIN_TIMEOUT_MS`] a log entry and back to the main menu (no program end;
//!   deviates from E-288), `net_status.failure` names the reason.
//! - Afterwards both load the map after the `Start` (`GameFlow::start_net_match`, in the host `game.rs`), report
//!   `Loaded` and compute from `Go`. Until then the input is locked, the state is in the log and in `net_status`.
//!
//! The control commands `net_status`, `net_part_hashes`, `net_save` and `net_debug` are answered by [`run_net`]; the
//! state is summarized by [`status_data`] from a [`NetStatusSource`] (command-line path [`NetLaunch::source`], menu
//! path [`session_source`] from `NetSessionStatus`, match [`driver_source`]). The direct start is not remembered in the
//! settings (unlike `NetIsHost` in the original).

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use opennml_client::game::net_driver::{
    LEAVE_REASON, MigrationStatus, NetDriver, NetPaths, NetRole, NetStatus, WaitReason,
};
use opennml_client::game::sim_host::SimHost;
use opennml_client::game_flow::config_speed;
use opennml_client::net_session::{
    NetPhase, NetSessionStatus, local_words, map_checksum, session_name,
};
use opennml_client::settings::savegame_dir;
use opennml_control::{
    Command, NetDataState, NetDebugAction, NetFoundData, NetHaltName, NetLinkData, NetLobbyData,
    NetLobbyRowData, NetMigrationData, NetMigrationPhaseName, NetPartHash, NetPartHashesData,
    NetPeerData, NetPhaseName, NetRoleName, NetRttData, NetSeatData, NetStatusData,
    NetSuccessorData, NetWaitName, hex_checksum,
};
use opennml_formats::engine_ini::EngineConfig;
use opennml_net::{
    AcceptAll, ClientCore, Connecting, CoreEvent, DEFAULT_PORT, DataState, FoundGame, HaltReason,
    Hello, HostCore, KIND_HUMAN, Listener, LobbyBook, LobbyRow, LobbyState, LobbyWish, LocalWords,
    Lockstep, MapId, Peer, Phase, RasterStage, Seat, SessionInfo, SessionSettings, StartInfo,
    Transport, TurnConfig, clean_name,
};
use opennml_rda::Vfs;
use opennml_sim::rules::Rules;
use opennml_sim::setup::{MatchSetup, SlotKind, start_package};
use opennml_ui::assets::{Texts, load_localisation};
use opennml_ui::own_texts::language;
use opennml_ui::profiles::name_text;
use opennml_ui::screens::chatframe::chat_line;
use serde_json::{Value, json};

/// Deadline from `--join` to the `Welcome` of the host (like the join from `LAN1`).
pub const JOIN_TIMEOUT_MS: u64 = opennml_client::net_session::JOIN_TIMEOUT_MS;
/// Error message of the network commands of the control channel without a network session.
pub const NO_NET: &str = "no network session";
/// This many chat lines the command-line path keeps for `net_status` (like the network session of the menu path).
const MAX_CHAT_LINES: usize = 64;

/// Setup values from the settings of the host: what `to_match_setup` takes from the draft in single player, which
/// the menu fills from the same keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetSettings {
    /// `Resourcen` (0 from the map, 1 low, 2 normal, 3 high).
    pub resources: i32,
    /// `UnitLimit` (living space, index 0 ... 5).
    pub unit_limit: i32,
    /// `TimeLimit` in minutes.
    pub time_limit: i32,
    /// `MapExplored != 0`.
    pub map_explored: bool,
}

impl NetSettings {
    pub fn from_config(cfg: &EngineConfig) -> Self {
        Self {
            resources: cfg.resourcen,
            unit_limit: cfg.unit_limit,
            time_limit: cfg.time_limit,
            map_explored: cfg.map_explored != 0,
        }
    }
}

/// Active `.lvl` slots ascending: `new_match` puts the lobby on them in order (`setup_players`).
pub fn active_slots(rules: &Rules) -> Vec<u8> {
    let mut active: Vec<u8> = rules.level.players.iter().map(|p| p.slot).collect();
    active.sort_unstable();
    active.dedup();
    active
}

/// Setup of the network match of the host: the default of the map as in the menu (`MatchSetup::default_for`, game
/// type 0), humans with the names `names` on the first `players` active slots (host first, then the clients in row
/// order), the AI on the next `ai` (`ki.enabled`, name from the `.lvl`), the rest closed; resources, living space,
/// time limit and "map explored" from `settings`, seed `seed` (E-262), `network`. Errors: not a multiplayer map, too
/// few slots, wrong number of names.
pub fn host_setup(
    rules: &Rules,
    settings: &NetSettings,
    players: u8,
    ai: u8,
    seed: u32,
    names: &[String],
) -> Result<MatchSetup, String> {
    let mut setup = MatchSetup::default_for(rules);
    if setup.mode != 0 {
        return Err(format!(
            "\"{}\" is not a multiplayer map (expected: a path with /multiplayer/)",
            rules.level_path
        ));
    }
    let places = setup.lobby.len().min(active_slots(rules).len());
    let (humans, computers) = (usize::from(players), usize::from(ai));
    if humans < 1 {
        return Err("At least one player (the host) belongs in the match".into());
    }
    if humans + computers > places {
        return Err(format!(
            "The map has {places} slots: too few for {humans} players and {computers} AI"
        ));
    }
    if names.len() != humans {
        return Err(format!("{} names for {humans} players", names.len()));
    }
    for (i, entry) in setup.lobby.iter_mut().enumerate() {
        if i < humans {
            entry.kind = SlotKind::Human;
            entry.name = names[i].clone();
        } else if i < humans + computers {
            entry.kind = SlotKind::Ai;
        } else {
            entry.kind = SlotKind::Closed;
        }
    }
    setup.seed = seed;
    setup.network = true;
    setup.resource_setting = settings.resources;
    setup.living_space = settings.unit_limit;
    setup.time_limit = settings.time_limit;
    setup.map_explored = settings.map_explored;
    setup.ki.enabled = ai > 0;
    Ok(setup)
}

/// Seed of the network match from the clock of the host (deviates from E-262): milliseconds since 1970, folded to
/// `u32` (`as u32`, the upper bits are dropped). 0 before 1970 (clock set wrong). Also the seed source of the network
/// session of the menu path (`NetIdentity::seed`: the clock stays in the program).
pub fn wall_seed() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u32)
}

/// Game port of the host (deviates from E-254): `--host` without a value = `NetPort` of the settings, a value is
/// used; 0 (also as `NetPort`) means [`DEFAULT_PORT`] 2302, as does a value outside 1 ... 65535.
pub fn host_port(arg: Option<u16>, net_port: i32) -> u16 {
    let port = match arg {
        Some(port) => port,
        None => u16::try_from(net_port).unwrap_or(0),
    };
    if port == 0 { DEFAULT_PORT } else { port }
}

/// `--join <ADDRESS[:PORT]>`: IP address (IPv6 in square brackets) or machine name, without a port [`DEFAULT_PORT`];
/// a name is resolved immediately (IPv4 first).
pub fn parse_join_addr(text: &str) -> Result<SocketAddr, String> {
    let text = text.trim();
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = text
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let resolved: Vec<SocketAddr> = if text.contains(':') {
        text.to_socket_addrs()
    } else {
        (text, DEFAULT_PORT).to_socket_addrs()
    }
    .map_err(|e| format!("Address \"{text}\" is invalid (expected ADDRESS[:PORT]): {e}"))?
    .collect();
    resolved
        .iter()
        .find(|a| a.is_ipv4())
        .or(resolved.first())
        .copied()
        .ok_or_else(|| format!("Address \"{text}\" cannot be resolved"))
}

/// Own sign-on (game edition and versions are in `opennml-client`): like [`opennml_client::net_session::own_hello`] with the
/// build ID [`build_id`].
pub fn own_hello(vfs: &Vfs, name: &str) -> Hello {
    opennml_client::net_session::own_hello(vfs, name, build_id())
}

/// Build ID of the program: `NML_BUILD_ID` from `build.rs` - 12 characters of the commit, with changes in the working
/// tree followed by `-` and 8 hex characters of FNV-1a over `git diff HEAD`, without Git `opennml_net::UNKNOWN_BUILD`
/// ("unbekannt").
pub fn build_id() -> &'static str {
    env!("NML_BUILD_ID")
}

/// Player name: `--net-name`, otherwise the profile name `NetName`, otherwise the nameless player of the edition
/// (`fallback`); cleaned ([`clean_name`]).
pub fn player_name(cli: Option<&str>, cfg: &EngineConfig, fallback: &str) -> String {
    match cli {
        Some(name) => clean_name(name, fallback),
        None => clean_name(&name_text(&cfg.net_name), fallback),
    }
}

/// Storage locations of the network: `savegame/` (containing `multiplayer/`) and `netlog/` next to the settings
/// file.
pub fn net_paths(settings: &Path) -> NetPaths {
    NetPaths {
        saves: savegame_dir(settings),
        netlog: settings.with_file_name("netlog"),
    }
}

/// Slot assignment from the setup: the `i`-th lobby entry lies on the `i`-th active slot; humans get the machine
/// `peers[i]` (host first, clients in row order) with its name, AI and closed slots no machine (AI with the lobby
/// name).
pub fn seats(setup: &MatchSetup, active: &[u8], peers: &[(Peer, String)]) -> [Seat; 8] {
    let mut seats: [Seat; 8] = std::array::from_fn(|_| Seat {
        peer: None,
        name: String::new(),
    });
    let mut human = 0;
    for (entry, &slot) in setup.lobby.iter().zip(active) {
        let Some(seat) = seats.get_mut(usize::from(slot)) else {
            continue;
        };
        match entry.kind {
            SlotKind::Human => {
                if let Some((peer, name)) = peers.get(human) {
                    *seat = Seat {
                        peer: Some(*peer),
                        name: name.clone(),
                    };
                }
                human += 1;
            }
            SlotKind::Ai => seat.name = entry.name.clone(),
            SlotKind::Closed => {}
        }
    }
    seats
}

/// What the host needs for the start (from the command line and settings, checked before the window).
pub struct HostPlan {
    /// Rules of the map `--net-map` (for the default, slots and rules checksum of the start package).
    pub rules: Rules,
    pub settings: NetSettings,
    /// Humans including the host (`--net-players`) = rows of the lobby.
    pub players: u8,
    /// AI slots (`--net-ai`).
    pub ai: u8,
    /// Seed (`--net-seed` or [`wall_seed`]).
    pub seed: u32,
    /// Session name.
    pub session: String,
    /// Game speed of the host (`GameSpeed`), the same for all.
    pub game_speed: u8,
    /// Turn raster `T`/`V` (`--turn-raster`, `--turn-steps`, default 8/4).
    pub raster: TurnConfig,
    /// Map checksum for the session data; 0 without a readable `.wld`.
    pub map_checksum: u64,
    /// Words of the installation for the core (nameless player, prefix of the network savegame names).
    pub words: LocalWords,
}

/// Start information per machine (host = peer 0, clients in row order): start package with seed and network flag,
/// slot assignment, own slot, raster, game speed of the host, session name.
pub fn start_infos(
    plan: &HostPlan,
    peers: &[(Peer, String)],
) -> Result<Vec<(Peer, StartInfo)>, String> {
    let names: Vec<String> = peers.iter().map(|(_, name)| name.clone()).collect();
    let setup = host_setup(
        &plan.rules,
        &plan.settings,
        plan.players,
        plan.ai,
        plan.seed,
        &names,
    )?;
    let active = active_slots(&plan.rules);
    let seats = seats(&setup, &active, peers);
    let package = start_package(&plan.rules, &setup);
    let (spacing, lead) = plan.raster.raw();
    peers
        .iter()
        .map(|(peer, _)| {
            let your_slot = (0u8..8)
                .find(|&s| seats[usize::from(s)].peer == Some(*peer))
                .ok_or_else(|| format!("machine {peer} has no slot"))?;
            Ok((
                *peer,
                StartInfo {
                    package: package.clone(),
                    seats: seats.clone(),
                    your_slot,
                    spacing,
                    lead,
                    game_speed: plan.game_speed,
                    session: plan.session.clone(),
                    saved: None,
                },
            ))
        })
        .collect()
}

/// Host core of the command-line path: lobby with one row per human (`plan.players`), nation, team, color and handicap
/// in order from the default of the map ([`host_setup`]); row 0 is the host and immediately ready, the remaining rows
/// are open and are filled by the joining humans. Session name, map (with checksum), settings and raster from the
/// plan; rules "everything allowed" (there is no lobby dialog).
pub fn host_core(hello: Hello, plan: &HostPlan) -> Result<HostCore, String> {
    let placeholders = vec![hello.name.clone(); usize::from(plan.players)];
    let setup = host_setup(
        &plan.rules,
        &plan.settings,
        plan.players,
        plan.ai,
        plan.seed,
        &placeholders,
    )?;
    let rows = setup
        .lobby
        .iter()
        .take(usize::from(plan.players))
        .map(|entry| LobbyRow {
            kind: 0,
            name: String::new(),
            nation: entry.nation,
            team: entry.team,
            handicap: entry.handicap,
            color: entry.color,
            ready: false,
            peer: None,
            data: DataState::Equal,
        })
        .collect();
    let mut session = SessionInfo::plain(
        &plan.session,
        &plan.rules.level_path,
        plan.players,
        plan.raster,
    );
    session.map = MapId::Fixed {
        path: plan.rules.level_path.clone(),
        checksum: plan.map_checksum,
    };
    session.settings = SessionSettings {
        game_type: setup.mode,
        unit_limit: plan.settings.unit_limit,
        resources: plan.settings.resources,
        time_limit: plan.settings.time_limit,
        lock_teams: false,
        map_explored: plan.settings.map_explored,
        save_replay: false,
    };
    let book = LobbyBook::new(session, rows, hello.name.clone())?;
    let mut core = HostCore::new(hello, book, Box::new(AcceptAll));
    core.set_words(plan.words.clone());
    // The host is always ready in the command-line path.
    core.lobby_wish(LobbyWish::Ready(true));
    Ok(core)
}

/// Stage of the command-line path.
enum Stage {
    /// Client: connection setup not yet begun (it begins in the first pass of the event loop so that the deadline
    /// counts from the window).
    Dial { addr: SocketAddr },
    /// Client: connection setup runs until `deadline` (program time).
    Connecting {
        connecting: Box<dyn Connecting>,
        addr: SocketAddr,
        deadline: u64,
    },
    /// Driver until loading; for the client until the `Welcome`, with a deadline.
    Driver {
        driver: Box<NetDriver>,
        welcome_by: Option<u64>,
    },
    /// The driver is in loading or in the match.
    Handed,
    /// Failed (reason in [`NetLaunch::failure`]); the program shows the main menu.
    Failed,
}

/// Network start of this program by command line: transport, driver (until loading here, afterwards in the match),
/// for the host listener and plan.
pub struct NetLaunch {
    transport: Box<dyn Transport>,
    stage: Stage,
    /// Client: own sign-on (until the connection).
    hello: Hello,
    /// Client: listens on a free port from the connection onward (`--net-no-listen` switches it off).
    listen: bool,
    paths: NetPaths,
    /// Client: words of the installation for the core ([`Self::with_words`]); the host has them in its plan.
    words: LocalWords,
    listener: Option<Box<dyn Listener>>,
    plan: Option<HostPlan>,
    /// Next connection number of the host (1, 2, ...).
    next_peer: u8,
    /// Host: own game port; client: port of the host.
    port: u16,
    /// Client: address of the host.
    target: Option<SocketAddr>,
    is_host: bool,
    /// Chat lines of the lobby (display only, in `net_status`).
    chat: Vec<String>,
    failure: Option<String>,
    /// Last written state (log only on change).
    note: String,
}

impl NetLaunch {
    /// Host: listens through `transport` on `port` and creates the core ([`host_core`]). Errors: port in use, plan
    /// invalid.
    pub fn host(
        mut transport: Box<dyn Transport>,
        port: u16,
        hello: Hello,
        plan: HostPlan,
        paths: NetPaths,
    ) -> Result<Self, String> {
        let core = host_core(hello.clone(), &plan)?;
        let listener = transport
            .listen(port)
            .map_err(|e| format!("opening game port {port}: {e}"))?;
        let port = listener.port();
        // The host probes the listen ports of its clients with a transport of its own.
        let mut driver = NetDriver::host(core, paths.clone());
        driver.set_transport(transport.fork());
        Ok(Self {
            transport,
            stage: Stage::Driver {
                driver: Box::new(driver),
                welcome_by: None,
            },
            hello,
            listen: false,
            paths,
            words: LocalWords::default(),
            listener: Some(listener),
            plan: Some(plan),
            next_peer: 1,
            port,
            target: None,
            is_host: true,
            chat: Vec::new(),
            failure: None,
            note: String::new(),
        })
    }

    /// Client: connects in the first [`Self::pump`] through `transport` to `addr`. With `listen` it listens on a free
    /// port from the connection onward and reports it in the `Hello`.
    pub fn client(
        transport: Box<dyn Transport>,
        addr: SocketAddr,
        hello: Hello,
        paths: NetPaths,
        listen: bool,
    ) -> Self {
        Self {
            transport,
            stage: Stage::Dial { addr },
            hello,
            listen,
            paths,
            words: LocalWords::default(),
            listener: None,
            plan: None,
            next_peer: 1,
            port: addr.port(),
            target: Some(addr),
            is_host: false,
            chat: Vec::new(),
            failure: None,
            note: String::new(),
        }
    }

    /// Client: the words of the installation for the core, which is created on connecting.
    pub fn with_words(mut self, words: LocalWords) -> Self {
        self.words = words;
        self
    }

    /// Driver as long as it is not yet in the match.
    pub fn driver(&self) -> Option<&NetDriver> {
        match &self.stage {
            Stage::Driver { driver, .. } => Some(driver),
            _ => None,
        }
    }

    pub fn driver_mut(&mut self) -> Option<&mut NetDriver> {
        match &mut self.stage {
            Stage::Driver { driver, .. } => Some(driver),
            _ => None,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Host: humans of the match.
    pub fn players(&self) -> Option<u8> {
        self.plan.as_ref().map(|p| p.players)
    }

    /// Reason for the failure (the program then shows the main menu with input free).
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The network start is still waiting (connection, sign-on, fellow players, start): the input is locked.
    pub fn waiting(&self) -> bool {
        matches!(
            self.stage,
            Stage::Dial { .. } | Stage::Connecting { .. } | Stage::Driver { .. }
        )
    }

    /// The driver went to loading (afterwards the match locks until `Go`).
    pub fn handed(&self) -> bool {
        matches!(self.stage, Stage::Handed)
    }

    /// One pass (in every pass of the event loop): the client connects and signs on, the host accepts new connections,
    /// pump the driver, collect lobby events, at the host distribute the start as soon as all are there and ready.
    /// Failures before the start end the network start ([`Self::failure`]).
    pub fn pump(&mut self, now_ms: u64) {
        self.stage = match std::mem::replace(&mut self.stage, Stage::Handed) {
            Stage::Dial { addr } => {
                log::info!("Network: connecting to {addr}");
                Stage::Connecting {
                    connecting: self.transport.connect(addr, JOIN_TIMEOUT_MS),
                    addr,
                    deadline: now_ms.saturating_add(JOIN_TIMEOUT_MS),
                }
            }
            Stage::Connecting {
                mut connecting,
                addr,
                deadline,
            } => match connecting.poll() {
                Some(Ok(link)) => {
                    log::info!("Network: connected to {addr}, signing on");
                    // The listener is open before the `Hello` goes out; if listening fails, the client reports no
                    // port and counts as unreachable for the host.
                    let listener = opennml_client::net_session::open_client_listener(
                        self.transport.as_mut(),
                        &mut self.hello,
                        self.listen,
                    );
                    let mut core = ClientCore::new(self.hello.clone());
                    core.set_words(self.words.clone());
                    // Without a lobby dialog: ready after the first lobby state.
                    core.set_auto_ready(true);
                    let mut driver = NetDriver::client(core, link, self.paths.clone());
                    driver.set_transport(self.transport.fork());
                    if let Some(listener) = listener {
                        driver.set_listener(listener);
                    }
                    driver.pump(now_ms);
                    Stage::Driver {
                        driver: Box::new(driver),
                        welcome_by: Some(deadline),
                    }
                }
                Some(Err(e)) => {
                    return self.fail(format!(
                        "No host reachable at {addr}, given up after {} s ({e})",
                        JOIN_TIMEOUT_MS.div_ceil(1000)
                    ));
                }
                None if now_ms >= deadline => {
                    return self.fail(format!(
                        "No host reachable at {addr}, given up after {} s",
                        JOIN_TIMEOUT_MS.div_ceil(1000)
                    ));
                }
                None => Stage::Connecting {
                    connecting,
                    addr,
                    deadline,
                },
            },
            Stage::Driver {
                mut driver,
                mut welcome_by,
            } => {
                self.accept_links(&mut driver);
                driver.pump(now_ms);
                self.lobby_events(&mut driver);
                if let Some(by) = welcome_by {
                    if let Some(peer) = driver.client_core().and_then(ClientCore::peer) {
                        log::info!(
                            "Network: accepted by the host (connection {peer}), waiting for the start"
                        );
                        welcome_by = None;
                    } else if now_ms >= by && driver.failure().is_none() {
                        driver.leave("No answer to the sign-on");
                        let target = self
                            .target
                            .map_or_else(|| format!("port {}", self.port), |a| a.to_string());
                        return self.fail(format!(
                            "The host at {target} did not accept the sign-on within {} s",
                            JOIN_TIMEOUT_MS.div_ceil(1000)
                        ));
                    }
                }
                if let Some(text) = driver
                    .failure()
                    .filter(|_| driver.start_info().is_none())
                    .map(String::from)
                {
                    driver.leave(LEAVE_REASON);
                    let target = self.target.map_or_else(String::new, |a| format!(" to {a}"));
                    return self.fail(format!("Joining{target} failed: {text}"));
                }
                self.start_when_full(&mut driver);
                Stage::Driver { driver, welcome_by }
            }
            other => other,
        };
    }

    /// Host: new connections to the driver (numbers 1, 2, ... in order of acceptance).
    fn accept_links(&mut self, driver: &mut NetDriver) {
        let Some(listener) = self.listener.as_mut() else {
            return;
        };
        for link in listener.accept_pending() {
            if self.next_peer == u8::MAX {
                log::warn!("Network: no connection number free any more, connection discarded");
                continue;
            }
            driver.add_link(self.next_peer, link);
            self.next_peer += 1;
        }
    }

    /// Collect lobby events before the start (otherwise the list would grow): chat for `net_status`, kick, departure
    /// of the host and rejection set `failure` on the driver and end the network start in the same pass.
    fn lobby_events(&mut self, driver: &mut NetDriver) {
        for ev in driver.take_lobby_events() {
            match ev {
                CoreEvent::Chat { name, text, .. } => {
                    self.chat.push(chat_line(&name, &text));
                    if self.chat.len() > MAX_CHAT_LINES {
                        self.chat.remove(0);
                    }
                }
                CoreEvent::Rejected { reason } => log::warn!("Network: rejected: {reason}"),
                CoreEvent::Kicked => log::warn!("Network: removed by the host"),
                CoreEvent::HostLeft => log::warn!("Network: the host has left the match"),
                _ => {}
            }
        }
    }

    /// Host: every row has a human with a machine and all are ready (`HostCore::can_start`) -> start via
    /// `HostCore::start_from_lobby` (checks once more); afterwards it no longer listens. If the setup fails, the host
    /// aborts the start for all; if the lobby check fails (a machine just left), it keeps waiting.
    fn start_when_full(&mut self, driver: &mut NetDriver) {
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        let Some(core) = driver.host_core_mut() else {
            return;
        };
        if core.phase() != &Phase::Gathering {
            return;
        }
        let rows = &core.lobby().rows;
        let peers: Vec<(Peer, String)> = rows
            .iter()
            .filter(|r| r.kind == KIND_HUMAN)
            .filter_map(|r| Some((r.peer?, r.name.clone())))
            .collect();
        if peers.len() != rows.len() || !core.can_start() {
            return;
        }
        match start_infos(plan, &peers) {
            Ok(infos) => {
                let fallback = infos[0].1.clone();
                let started = core.start_from_lobby(|peer, _row| {
                    infos
                        .iter()
                        .find(|(p, _)| *p == peer)
                        .map_or_else(|| fallback.clone(), |(_, info)| info.clone())
                });
                match started {
                    Ok(()) => log::info!(
                        "Network: all {} players present and ready, distributing the start (seed {})",
                        plan.players,
                        plan.seed
                    ),
                    Err(e) => {
                        log::info!("Network: start not yet possible: {e}");
                        return;
                    }
                }
            }
            Err(e) => core.abort(format!("The host could not set up the match: {e}")),
        }
        // No further connections (a later `Hello` is rejected by the core anyway, E-287).
        self.listener = None;
    }

    /// Network start failed: log entry, driver and listener closed; the program shows the main menu with input free,
    /// `net_status.failure` names the reason.
    fn fail(&mut self, text: String) {
        log::error!("{text}; back to the main menu");
        if let Stage::Driver { driver, .. } = &mut self.stage {
            driver.leave(LEAVE_REASON);
        }
        self.stage = Stage::Failed;
        self.listener = None;
        self.failure = Some(text);
    }

    /// The driver as soon as the start is there and it has not been loaded yet: it goes to `GameFlow::start_net_match`.
    pub fn take_ready(&mut self) -> Option<NetDriver> {
        match &self.stage {
            Stage::Driver { driver, .. } if driver.start_info().is_some() => {}
            _ => return None,
        }
        match std::mem::replace(&mut self.stage, Stage::Handed) {
            Stage::Driver { driver, .. } => Some(*driver),
            _ => unreachable!("just checked"),
        }
    }

    /// Loading failed (`Loaded { error }` is already out, the flow has opened the main menu): leave the driver, close
    /// the network start.
    pub fn load_failed(&mut self, mut driver: NetDriver, text: String) {
        driver.leave(LEAVE_REASON);
        self.fail(text);
    }

    /// State into the log if it has changed.
    pub fn log_state(&mut self, match_driver: Option<&NetDriver>) {
        let note = match &self.stage {
            Stage::Dial { .. } | Stage::Connecting { .. } => "connecting to the host".to_string(),
            Stage::Failed => "network start failed".to_string(),
            Stage::Handed => match match_driver {
                Some(d) if d.started() => "match running".into(),
                Some(_) => "loaded, waiting for the other machines (Go)".into(),
                None => "session ended".into(),
            },
            Stage::Driver { driver, .. } => {
                let status = driver.status();
                match (&status.phase, status.role) {
                    (Phase::Gathering, NetRole::Host) => format!(
                        "waiting for fellow players: {} of {} signed on (port {})",
                        driver.host_core().map_or(0, |c| c.peers().len()),
                        self.players().unwrap_or(0),
                        self.port
                    ),
                    (Phase::Gathering, NetRole::Client) => {
                        "signed on, waiting for the start".into()
                    }
                    (Phase::Loading, _) => "start received, loading the map".into(),
                    (Phase::Running | Phase::Halted(_) | Phase::Stopped, _) => {
                        "match running".into()
                    }
                    (Phase::Ended, _) => "session ended".into(),
                }
            }
        };
        if note != self.note {
            log::info!("Network: {note}");
            self.note = note;
        }
    }

    /// State for `net_status`: while waiting and after a failure; if the driver is in the match, it supplies the state
    /// ([`driver_source`]), afterwards there is none.
    pub fn source(&self) -> Option<NetStatusSource<'_>> {
        let role = Some(if self.is_host {
            NetRoleName::Host
        } else {
            NetRoleName::Client
        });
        let base = NetStatusSource {
            role,
            port: Some(self.port),
            players: self.players(),
            chat: self.chat.clone(),
            ..NetStatusSource::default()
        };
        match &self.stage {
            Stage::Handed => None,
            Stage::Failed => Some(NetStatusSource {
                phase: Some(NetPhaseName::Ended),
                failure: self.failure().map(String::from),
                ..base
            }),
            Stage::Dial { .. } | Stage::Connecting { .. } => Some(NetStatusSource {
                phase: Some(NetPhaseName::Connecting),
                ..base
            }),
            Stage::Driver { driver, .. } => {
                let (lobby, own_row, rtt_ms) = lobby_parts(driver);
                Some(NetStatusSource {
                    driver: Some(driver),
                    lobby,
                    own_row,
                    rtt_ms,
                    ..base
                })
            }
        }
    }
}

/// Lobby state, own row and round-trip times (host) from the core of a driver before the start.
fn lobby_parts(driver: &NetDriver) -> (Option<LobbyState>, Option<u8>, Vec<(u8, u32)>) {
    if driver.start_info().is_some() {
        return (None, None, Vec::new());
    }
    if let Some(core) = driver.host_core() {
        let rtt = core
            .peers()
            .into_iter()
            .filter_map(|(peer, _)| core.rtt_ms(peer).map(|ms| (peer, ms)))
            .collect();
        return (Some(core.lobby().clone()), Some(0), rtt);
    }
    match driver.client_core() {
        Some(core) => (core.lobby().cloned(), core.own_row(), Vec::new()),
        None => (None, None, Vec::new()),
    }
}

/// Phase of the core as a name of the control channel.
fn phase_name(phase: &Phase) -> (NetPhaseName, Option<NetHaltName>, Option<u8>) {
    match phase {
        Phase::Gathering => (NetPhaseName::Gathering, None, None),
        Phase::Loading => (NetPhaseName::Loading, None, None),
        Phase::Running => (NetPhaseName::Running, None, None),
        Phase::Halted(HaltReason::Pause { by }) => {
            (NetPhaseName::Halted, Some(NetHaltName::Pause), Some(*by))
        }
        Phase::Halted(HaltReason::Resync) => {
            (NetPhaseName::Halted, Some(NetHaltName::Resync), None)
        }
        Phase::Stopped => (NetPhaseName::Stopped, None, None),
        Phase::Ended => (NetPhaseName::Ended, None, None),
    }
}

/// Information of the network start for `net_status`: port and (host) number of players.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchInfo {
    pub port: Option<u16>,
    pub players: Option<u8>,
}

impl LaunchInfo {
    pub fn of(launch: Option<&NetLaunch>) -> Self {
        launch.map_or_else(Self::default, |l| Self {
            port: Some(l.port()),
            players: l.players(),
        })
    }
}

/// What `net_status` is made of: role and phase (from the driver if not given), the driver if there is
/// one, lobby state, round-trip times and found games.
#[derive(Default)]
pub struct NetStatusSource<'a> {
    /// `None` during the search; otherwise from the driver if not set.
    pub role: Option<NetRoleName>,
    /// Fixed phase (search, connecting, lobby, failed); `None` = phase of the driver.
    pub phase: Option<NetPhaseName>,
    pub driver: Option<&'a NetDriver>,
    pub port: Option<u16>,
    pub players: Option<u8>,
    pub lobby: Option<LobbyState>,
    pub own_row: Option<u8>,
    pub chat: Vec<String>,
    pub rtt_ms: Vec<(u8, u32)>,
    pub found: Vec<FoundGame>,
    pub failure: Option<String>,
}

/// State of the network session of the menu path with its driver or that of the match: without a session (`Idle`)
/// none; search without a role with the found games; connecting; lobby with lobby state, chat and round-trip times;
/// loading and match with the phase of the driver.
pub fn session_source<'a>(
    status: NetSessionStatus,
    driver: Option<&'a NetDriver>,
) -> Option<NetStatusSource<'a>> {
    let own = Some(if status.is_host {
        NetRoleName::Host
    } else {
        NetRoleName::Client
    });
    let mut src = NetStatusSource {
        role: own,
        driver,
        port: status.port,
        ..NetStatusSource::default()
    };
    match status.phase {
        NetPhase::Idle => return None,
        NetPhase::Searching => {
            src.role = None;
            src.driver = None;
            src.phase = Some(NetPhaseName::Searching);
            src.found = status.games;
        }
        NetPhase::Connecting => {
            src.role = Some(NetRoleName::Client);
            src.phase = Some(NetPhaseName::Connecting);
        }
        NetPhase::Lobby => {
            src.phase = Some(NetPhaseName::Lobby);
            src.lobby = status.lobby;
            src.own_row = status.own_row;
            src.chat = status.chat;
            src.rtt_ms = status.rtt_ms;
        }
        NetPhase::Loading | NetPhase::Match => {
            if driver.is_none() {
                src.phase = Some(NetPhaseName::Loading);
            }
        }
    }
    Some(src)
}

/// State from the driver of the running match (both paths), with port and number of players of the command-line path.
pub fn driver_source(driver: &NetDriver, launch: LaunchInfo) -> NetStatusSource<'_> {
    NetStatusSource {
        driver: Some(driver),
        port: launch.port,
        players: launch.players,
        ..NetStatusSource::default()
    }
}

/// Name of the raster stage (`net_status.lobby.stage`): `original`, `lan`, `lan_fast`, otherwise `T/V`.
fn stage_name(spacing: u8, lead: u8) -> String {
    match (0..3)
        .map(RasterStage::from_index)
        .find(|s| s.config().raw() == (spacing, lead))
    {
        Some(RasterStage::Original) => "original".into(),
        Some(RasterStage::Lan) => "lan".into(),
        Some(RasterStage::LanFast) => "lan_fast".into(),
        None => format!("{spacing}/{lead}"),
    }
}

/// Lobby state for the control channel; a loaded lobby with savegame name (`saved`) and the player slot per row
/// (`seat`, from `LobbyState::seats`; only read where it exists).
fn lobby_data(l: &LobbyState, own_row: Option<u8>, chat: &[String]) -> NetLobbyData {
    let s = &l.session;
    NetLobbyData {
        session: s.name.clone(),
        map: match &s.map {
            MapId::Fixed { path, .. } => path.clone(),
            MapId::Random { .. } => s.map_name.clone(),
            // Network savegame: the map name of the session is the savegame name.
            MapId::Saved { name, .. } => name.clone(),
        },
        stage: stage_name(s.spacing, s.lead),
        own_row,
        rows: l
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| NetLobbyRowData {
                row: i as u8,
                kind: r.kind,
                name: r.name.clone(),
                nation: r.nation,
                team: r.team,
                handicap: r.handicap,
                color: r.color,
                ready: r.ready,
                peer: r.peer,
                seat: l.seats.get(i).copied(),
                data: data_state(&r.data),
            })
            .collect(),
        chat: chat.to_vec(),
        max_rtt_ms: l.max_rtt_ms,
        saved: match &s.map {
            MapId::Saved { name, .. } => Some(name.clone()),
            MapId::Fixed { .. } | MapId::Random { .. } => None,
        },
        session_revision: l.session_revision,
        rules_checksum: l.rules_checksum.map(hex_checksum),
    }
}

/// Verdict of the game data check of a lobby row for `net_status`.
fn data_state(data: &DataState) -> NetDataState {
    match data {
        DataState::Pending => NetDataState::Pending,
        DataState::Equal => NetDataState::Equal,
        DataState::Missing => NetDataState::Missing,
        DataState::Differs { files, more } => NetDataState::Differs {
            files: files.clone(),
            more: *more,
        },
    }
}

/// `net_status` from a [`NetStatusSource`] and the tick of the match. Without a driver, round, ticks, slots and
/// connections are empty or 0; the raster comes from the start, otherwise from the lobby, otherwise (host) from the
/// core.
pub fn status_data(src: &NetStatusSource, steps: Option<u64>) -> NetStatusData {
    let status = src.driver.map(NetDriver::status);
    let driver_phase = status.as_ref().map(|s| phase_name(&s.phase));
    let (phase, halt, paused_by) = match (src.phase, driver_phase) {
        (Some(phase), _) => (phase, None, None),
        (None, Some(p)) => p,
        (None, None) => (NetPhaseName::Ended, None, None),
    };
    let role = src.role.or(status.as_ref().map(|s| match s.role {
        NetRole::Host => NetRoleName::Host,
        NetRole::Client => NetRoleName::Client,
    }));
    let (spacing, lead) = match (src.driver, &src.lobby) {
        (Some(d), _) if d.start_info().is_some() => {
            let (t, v) = d.raster().raw();
            (Some(t), Some(v))
        }
        (_, Some(l)) => (Some(l.session.spacing), Some(l.session.lead)),
        (Some(d), None) if d.is_host() => {
            let (t, v) = d.raster().raw();
            (Some(t), Some(v))
        }
        _ => (None, None),
    };
    let mut data = NetStatusData {
        role,
        phase,
        halt,
        paused_by,
        port: src.port,
        players: src.players,
        peers: src
            .driver
            .and_then(NetDriver::host_core)
            .map(|c| {
                c.peers()
                    .into_iter()
                    .map(|(peer, name)| NetPeerData { peer, name })
                    .collect()
            })
            .unwrap_or_default(),
        own_slot: None,
        seats: Vec::new(),
        round: 0,
        next_exec_step: 0,
        steps,
        waiting: None,
        waiting_turn: None,
        missing: Vec::new(),
        desyncs: None,
        links: Vec::new(),
        started: src.driver.is_some_and(NetDriver::started),
        finished: src.driver.is_some_and(NetDriver::finished),
        failure: src
            .failure
            .clone()
            .or_else(|| src.driver.and_then(NetDriver::failure).map(String::from)),
        spacing,
        lead,
        build: build_id().to_string(),
        rtt_ms: src
            .rtt_ms
            .iter()
            .map(|&(peer, ms)| NetRttData { peer, ms })
            .collect(),
        lobby: src
            .lobby
            .as_ref()
            .map(|l| lobby_data(l, src.own_row, &src.chat)),
        found: src
            .found
            .iter()
            .map(|g| NetFoundData {
                instance: format!("{:016x}", g.instance),
                name: g.name.clone(),
                addr: g.addr.to_string(),
            })
            .collect(),
        successors: Vec::new(),
        migration: None,
    };
    if let Some(NetStatus {
        own_slot,
        seats,
        round,
        next_exec_step,
        waiting,
        missing,
        desyncs,
        links,
        successors,
        migration,
        ..
    }) = status
    {
        let (waiting, waiting_turn) = match waiting {
            None => (None, None),
            Some(WaitReason::Starting) => (Some(NetWaitName::Starting), None),
            Some(WaitReason::Bundle { turn }) => (Some(NetWaitName::Bundle), Some(turn)),
            Some(WaitReason::Halted(_)) => (Some(NetWaitName::Halted), None),
            Some(WaitReason::Stopped) => (Some(NetWaitName::Stopped), None),
            Some(WaitReason::Ended) => (Some(NetWaitName::Ended), None),
        };
        data.own_slot = own_slot;
        data.seats = seats
            .map(|s| {
                s.iter()
                    .enumerate()
                    .map(|(slot, seat)| NetSeatData {
                        slot: slot as u8,
                        peer: seat.peer,
                        name: seat.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        data.round = round;
        data.next_exec_step = next_exec_step;
        data.waiting = waiting;
        data.waiting_turn = waiting_turn;
        data.missing = missing;
        data.desyncs = desyncs;
        data.successors = successors
            .into_iter()
            .map(|s| NetSuccessorData {
                peer: s.peer,
                slot: s.slot,
                addr: s.addr.map(|a| a.to_string()),
            })
            .collect();
        data.migration = migration.map(|m| match m {
            MigrationStatus::Dialing { peer } => NetMigrationData {
                phase: NetMigrationPhaseName::Dialing,
                peer: Some(peer),
                expected: Vec::new(),
                arrived: Vec::new(),
            },
            MigrationStatus::Rejoining { peer } => NetMigrationData {
                phase: NetMigrationPhaseName::Rejoining,
                peer: Some(peer),
                expected: Vec::new(),
                arrived: Vec::new(),
            },
            MigrationStatus::Gathering { expected, arrived } => NetMigrationData {
                phase: NetMigrationPhaseName::Gathering,
                peer: None,
                expected,
                arrived,
            },
        });
        data.links = links
            .into_iter()
            .map(|l| NetLinkData {
                peer: l.peer,
                slot: l.slot,
                silent_ms: l.silent_ms,
            })
            .collect();
    }
    data
}

/// `net_status`, `net_part_hashes`, `net_save`, `net_debug`. `status` is the state of the session ([`status_data`];
/// `None` without a network session -> "no network session"), `driver` its driver (network start, lobby or match;
/// none during search and connection setup), `sim` the simulation of the match. Commands on the state without a match
/// "no match"; `net_save` not until the match is running.
pub fn run_net(
    status: Option<NetStatusData>,
    driver: Option<&mut NetDriver>,
    sim: Option<&mut SimHost>,
    command: &Command,
) -> Result<Value, String> {
    let status = status.ok_or(NO_NET)?;
    let value = |v: Result<Value, serde_json::Error>| {
        v.map_err(|e| format!("response not serializable: {e}"))
    };
    match command {
        Command::NetStatus => value(serde_json::to_value(status)),
        Command::NetPartHashes => {
            let sim = sim.ok_or("no match")?;
            let parts = opennml_sim::save::part_checksums(&sim.state)
                .into_iter()
                .map(|(part, hash)| NetPartHash {
                    part: part.to_string(),
                    checksum: hex_checksum(hash),
                })
                .collect();
            value(serde_json::to_value(NetPartHashesData {
                steps: sim.state.steps,
                checksum: hex_checksum(sim.checksum()),
                parts,
            }))
        }
        Command::NetSave { name } => {
            let driver = driver
                .filter(|d| d.started() && !d.finished())
                .ok_or("Saving in the network only in the running match")?;
            if !driver.wish_save(name) {
                return Err(format!(
                    "Savegame name \"{name}\" invalid (no path, no special characters, at most {} characters)",
                    opennml_net::MAX_SAVE_NAME
                ));
            }
            Ok(json!({ "name": name }))
        }
        Command::NetDebug {
            action: NetDebugAction::Corrupt,
        } => {
            let sim = sim.ok_or("no match")?;
            let changed = sim.debug_corrupt();
            Ok(json!({
                "action": "corrupt",
                "changed": changed,
                "steps": sim.state.steps,
                "checksum": hex_checksum(sim.checksum()),
            }))
        }
        Command::NetDebug {
            action: NetDebugAction::Drop,
        } => {
            let Some(driver) = driver else {
                return Ok(json!({ "action": "drop", "links": 0 }));
            };
            let links = driver.status().links.len();
            driver.drop_links();
            Ok(json!({ "action": "drop", "links": links }))
        }
        Command::NetDebug {
            action: action @ (NetDebugAction::Mute | NetDebugAction::Unmute),
        } => {
            let on = *action == NetDebugAction::Mute;
            let name = if on { "mute" } else { "unmute" };
            let Some(driver) = driver else {
                return Ok(json!({ "action": name, "links": 0 }));
            };
            let links = driver.status().links.len();
            driver.set_muted(on);
            Ok(json!({ "action": name, "links": links }))
        }
        _ => Err(
            "not a network command (expected: net_status, net_part_hashes, net_save, net_debug)"
                .into(),
        ),
    }
}

/// Locks of the control channel and of the input in the network.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetGuard {
    /// A network session is running (search, connecting, lobby, loading, match, command-line path): `pause` and `step`
    /// are locked (the turns and the lobby run in real time).
    pub in_net: bool,
    /// Input locked: in the command-line path until `Go`, in the menu path while loading.
    pub locked: bool,
}

/// State of the command-line path for [`net_guard`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CliState {
    /// The network start is still waiting ([`NetLaunch::waiting`]).
    pub waiting: bool,
    /// The driver went to loading ([`NetLaunch::handed`]).
    pub handed: bool,
}

/// State of the driver of the match for [`net_guard`]: `None` without a match driver, otherwise `Some(free)` - free
/// after `Go` and after a failure before `Go` (then the driver reports `NetFinish::Failed` or `NetFinish::LoadFailed`,
/// and the input must be free for the "OK" of the message).
pub fn match_state(driver: Option<&NetDriver>) -> Option<bool> {
    driver.map(|d| d.started() || d.failure().is_some())
}

/// Locks from command-line path `cli`, the driver of the match (`Some(started)`) and the phase of the network session
/// of the menu path: `ui_*` are free in search and lobby and locked only while loading; `pause`/`step` stay locked in
/// search, lobby and match.
pub fn net_guard(
    cli: CliState,
    match_started: Option<bool>,
    session: Option<NetPhase>,
) -> NetGuard {
    let session_active = session.is_some_and(|p| p != NetPhase::Idle);
    NetGuard {
        in_net: cli.waiting || match_started.is_some() || session_active,
        locked: cli.waiting
            || (cli.handed && match_started == Some(false))
            || session == Some(NetPhase::Loading),
    }
}

/// `--turn-raster T/V` (e.g. `2/1`): turn spacing and lead in ticks, checked like [`TurnConfig::new`].
pub fn parse_raster(text: &str) -> Result<TurnConfig, String> {
    let (t, v) = text
        .split_once('/')
        .ok_or_else(|| format!("Turn raster \"{text}\" invalid: expected T/V, e.g. 8/4"))?;
    let num = |s: &str| {
        s.trim().parse::<u8>().map_err(|_| {
            format!("Turn raster \"{text}\" invalid: \"{s}\" is not a number from 0 to 255")
        })
    };
    TurnConfig::new(num(t)?, num(v)?)
}

/// Network switches of the command line.
#[derive(Debug, Clone, Default)]
pub struct NetArgs {
    pub host: Option<Option<u16>>,
    pub join: Option<SocketAddr>,
    pub map: Option<String>,
    pub players: u8,
    pub ai: u8,
    /// Turn raster `T`/`V` (default: that of the original, 8/4).
    pub raster: TurnConfig,
    pub name: Option<String>,
    pub seed: Option<u32>,
    /// The client listens from the connection onward (command-line default `true`, `--net-no-listen` switches it off).
    pub listen: bool,
}

/// Rules of a map without a match (in `opennml-client`).
pub use opennml_client::game::map::load_rules;

/// Network start before the window: `--host` checks map and setup, listens through `transport` and creates the
/// lobby; `--join` creates the join, which connects in the event loop. `None` without network switches. Errors of the
/// host (map, setup, game port) end the program with an error message and an error code; a failed join later leads to
/// the main menu ([`NetLaunch::failure`]). The session name of the host is chosen by `session_name` and is afterwards
/// in `NetSession` of `cfg`.
pub fn launch(
    args: &NetArgs,
    vfs: &Vfs,
    cfg: &mut EngineConfig,
    settings: &Path,
    transport: Box<dyn Transport>,
) -> Result<Option<NetLaunch>> {
    let words = local_words(language(vfs));
    let name = player_name(args.name.as_deref(), cfg, &words.player);
    let paths = net_paths(settings);
    if let Some(addr) = args.join {
        let launch = NetLaunch::client(transport, addr, own_hello(vfs, &name), paths, args.listen);
        return Ok(Some(launch.with_words(words)));
    }
    let Some(arg) = args.host else {
        return Ok(None);
    };
    let map = args
        .map
        .as_deref()
        .context("--host needs --net-map <path.lvl>")?;
    let rules = load_rules(vfs, map)?;
    let settings_values = NetSettings::from_config(cfg);
    // Pre-check with placeholders: map, slots and settings before anyone joins.
    let placeholders = vec![name.clone(); usize::from(args.players)];
    host_setup(
        &rules,
        &settings_values,
        args.players,
        args.ai,
        0,
        &placeholders,
    )
    .map_err(anyhow::Error::msg)?;
    let seed = args.seed.unwrap_or_else(wall_seed);
    let port = host_port(arg, cfg.net_port);
    let loc = load_localisation(vfs).map_err(anyhow::Error::msg)?;
    let session = session_name(cfg, &Texts::load(vfs, &loc), &name);
    let plan = HostPlan {
        map_checksum: map_checksum(vfs, map).unwrap_or(0),
        rules,
        settings: settings_values,
        players: args.players,
        ai: args.ai,
        seed,
        session,
        game_speed: config_speed(cfg),
        raster: args.raster,
        words,
    };
    let launch = NetLaunch::host(transport, port, own_hello(vfs, &name), plan, paths)
        .map_err(anyhow::Error::msg)?;
    log::info!(
        "Network: host listening on port {}, map {map}, {} players, {} AI, seed {seed}, raster {}/{}",
        launch.port(),
        args.players,
        args.ai,
        args.raster.raw().0,
        args.raster.raw().1
    );
    Ok(Some(launch))
}

#[cfg(test)]
mod tests;
