//! Network session in the client: search -> connect -> lobby -> start, pumped every frame by the flow
//! (`GameFlow::net_step`), with the [`Transport`] of the program (`SystemTransport` in `nml-game`, `MemTransport` in
//! the tests and in `UI_GOLDEN`).
//!
//! The UI (`LAN1`, `LAN3`, `LAN2`, `CHATFRAME`) sees the session only through `MenuData::net` and the match draft
//! ([`MenuApp::set_net`], [`MenuApp::set_lobby_draft`]) and reaches it through the application commands
//! 7/8/9/`0x2D`/`0x2F` ([`NetSession::on_app_cmd`]), the search (`UiEffect::Search`, [`NetSession::search`]) and the
//! lobby wishes (`UiEffect::Lobby`, [`NetSession::on_lobby`]). The turn core (`HostCore`/`ClientCore` behind
//! [`NetDriver`]) and the lobby rules (`nml_net::lobby`) come from `nml-net`.
//!
//! Flows according to re/multiplayer-menus L.1 - L.9:
//! - **Hosting** (`{8}`, `0x004CA1B5`): session data from the draft (`LAN3 CREATE`: slots and settings,
//!   `FUN_004B9700`), session name ([`session_name`]), raster from `NetTurnRaster`, listening, announcing; if that
//!   works: clear the chat and open the lobby (`open_lobby`), otherwise silently `{5,"lan1"}` (E-279) and the session
//!   is closed (deviating from E-312).
//! - **Joining** (`{7}`, `0x004CA0FA`): connect to the chosen session (`MenuData::lan_choice`, E-300) up to
//!   [`JOIN_TIMEOUT_MS`], `Hello`, wait for the first lobby state; success as when hosting, a failure (host gone,
//!   `Reject` such as "The match is full", E-287) silently to `LAN1`.
//! - **Hosting a savegame** (`{0x12}`, `FUN_004C80D0`): read and check the network savegame from the name buffer
//!   ([`read_net_save`]), rows = human slots in the game, map `MapId::Saved`, host as above and open `LAN4`; if that
//!   fails, message 99 106 ... 99 108 via `{5,"lan3"}`.
//! - **Finding and joining a loaded session**: `LAN1` shows a session with `MapId::Saved` only with the same own
//!   network savegame ([`read_net_head`], cached per name); `{7}` reports the own slot of the file in the `Hello`
//!   (`SeatClaim`), the first lobby state opens `LAN4` instead of the lobby.
//! - **`LAN4`**: `{0x14}` of the host checks the start condition (every row occupied), closes `LAN4` and queues
//!   `{0x15}`, `{0x0D}`; `{0x2D}` (ESC in `LAN4`, "OK" of 15212) closes `LAN4` instead of the lobby frames.
//! - **Removal** and **host gone** (E-305, deviating from E-311): message 15212 exactly once with `{0x2D}`; the
//!   removal directly, the departure of the host via `{0x2F}` (`{9}`, message) like `HOST_MIGRATE` in the original.
//! - **Start** (re/multiplayer-menus L.3.4): `START` of the host checks the start condition here
//!   ([`HostCore::can_start`]), closes the lobby and queues `{0x15}`, `{0x0D}`; the start itself runs via
//!   `GameFlow::start_match` -> [`NetSession::start_host_match`] (`to_net_setup` -> `start_package` ->
//!   `HostCore::start_from_lobby`, checks once more) -> `GameFlow::start_net_match`. The client loads after the
//!   `Start` of the host ([`NetSession::take_start`]).

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;
use std::time::SystemTime;

use nml_formats::engine_ini::EngineConfig;
use nml_net::{
    Announcement, Beacon, ClientCore, Connecting, CoreEvent, FoundGame, HOST_PEER, Hello, HostCore,
    HostEdit, KIND_HUMAN, Listener, LobbyBook, LobbyRow, LobbyRules, LobbyState, LobbyWish,
    LocalTime, Lockstep, MapId, NET_VERSION, Phase, QUERY_INTERVAL_MS, RasterStage, RuleVerdict,
    Seat, SeatClaim, Seeker, SessionInfo, SessionList, SessionSettings, StartInfo, Transport,
    TurnConfig, clean_name, clean_session_name, valid_save_name,
};
use nml_rda::Vfs;
use nml_rda::testutil::{GameVersion, game_version};
use nml_sim::setup::{FORMAT_VERSION, MatchSetup, start_package};
use nml_ui::app::MenuApp;
use nml_ui::assets::Texts;
use nml_ui::draft::{
    DraftSettings, FoundGameView, LevelInfoData, LobbyView, MapCatalog, MatchDraft, MatchRequest,
    NetMenuView, SlotDraft,
};
use nml_ui::msg::{AppCmd, LobbyAction};
use nml_ui::own_texts::{MAP_MISSING, SAVE_NOT_LOADED, SAVE_ONE_PLAYER, SAVE_SLOT_OUT};
use nml_ui::profiles::{name_field, name_text, name_units};
use nml_ui::screens::chatframe::chat_line;
use nml_ui::screens::lan4::{self, close_lan4, open_lan4};
use nml_ui::screens::{
    CHANGE_GAME_REFUSED, TeamColor, close_change_game, close_lobby, open_lobby, team_color_check,
};

use crate::game::map::{load_random_rules, load_rules};
use crate::game::net_driver::{LEAVE_REASON, NetDriver, NetPaths};
use crate::game_flow::config_speed;
use crate::game_save::{GameSaveFile, sim_checksum};
use crate::match_request::{active_slots, random_map_spec, row_slots, to_net_setup};

/// Deadline of a join from `LAN1` to the first lobby state (like `--join`).
pub const JOIN_TIMEOUT_MS: u64 = 10_000;
/// Text 15789 "Session" of the default name "<15789> - <player>" (`FUN_004EB190`).
pub const SESSION_TEXT: i32 = 15789;
/// Text 15212 "The host has removed you from the game!" (removal and host gone, E-279).
pub const KICKED_TEXT: i32 = 15212;
/// Number of chat lines the session keeps (the display shows the last five).
const MAX_CHAT_LINES: usize = 64;
/// Reason in the `Bye` when the join is aborted without an answer.
const NO_ANSWER: &str = "No answer to the sign-on";
/// Dialog "Change game" (`lan31.dlg`).
const CHANGE_DIALOG: &str = "lan31";
/// Reason in the `Bye` of the client that lacks the map after a map change (E-322).
pub const MAP_MISSING_REASON: &str = "The map of the match is missing on this machine.";

/// Who this machine is on the network (from the program): own sign-on (versions, game edition, build ID; the session
/// sets the name when hosting and joining from `NetName`), game port of the host and the source of the seed of a
/// network match (the program takes the clock, tests and `UI_GOLDEN` a fixed value).
pub struct NetIdentity {
    pub hello: Hello,
    pub port: u16,
    pub seed: Box<dyn FnMut() -> u32 + Send>,
    /// Does the client listen on a free port from the join on? Default of the program `true`; `false` only for check
    /// runs (`--net-no-listen`): the client reports no port and stands without an address in the successor list.
    pub listen: bool,
}

/// Client: listen on a free port and enter it in `hello.listen_port` before the `Hello` goes out. If listening fails
/// or is switched off (`listen` off, `--net-no-listen`), the client reports no port and counts as unreachable to the
/// host. Shared by the menu path and the command line.
pub fn open_client_listener(
    transport: &mut dyn Transport,
    hello: &mut Hello,
    listen: bool,
) -> Option<Box<dyn Listener>> {
    hello.listen_port = None;
    if !listen {
        return None;
    }
    match transport.listen(0) {
        Ok(listener) => {
            hello.listen_port = Some(listener.port());
            log::info!("Network: client listening on port {}", listener.port());
            Some(listener)
        }
        Err(e) => {
            log::warn!("Network: client cannot listen ({e}), it counts as unreachable");
            None
        }
    }
}

/// Phase of the session for display, input lock and control channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetPhase {
    /// No session, no search.
    Idle,
    /// `LAN1` searches for open games.
    Searching,
    /// Join in progress (connection, sign-on, first lobby state).
    Connecting,
    /// In the lobby (also after removal or departure of the host, until the "OK" of the message).
    Lobby,
    /// The start has arrived: load the map, then wait for `Go` (input locked).
    Loading,
    /// The network match is running (the driver is inside the match).
    Match,
}

/// State of the session (control channel `net_status`).
#[derive(Debug, Clone, PartialEq)]
pub struct NetSessionStatus {
    pub phase: NetPhase,
    pub is_host: bool,
    /// Visible games of the search.
    pub games: Vec<FoundGame>,
    /// Lobby state (host: own; client: last from the host).
    pub lobby: Option<LobbyState>,
    /// Own row in the lobby.
    pub own_row: Option<u8>,
    /// Host: last measured round-trip time per machine (peer, ms).
    pub rtt_ms: Vec<(u8, u32)>,
    /// Host: game port it listens on.
    pub port: Option<u16>,
    /// Chat lines of the lobby, formatted, oldest first.
    pub chat: Vec<String>,
}

/// Lobby rule of the host: team and color like `FUN_004BA710` in the original ([`team_color_check`], the same
/// reimplementation as in the single-player dialog); in game type 4 the swap with the other row. Row `n` = key
/// `n - 1`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientLobbyRules;

impl LobbyRules for ClientLobbyRules {
    fn check(
        &self,
        rows: &[LobbyRow],
        row: u8,
        proposed: &LobbyRow,
        game_type: i32,
    ) -> RuleVerdict {
        let slots: Vec<SlotDraft> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| slot_of(i, r))
            .collect();
        match team_color_check(&slots, &slot_of(usize::from(row), proposed), game_type) {
            TeamColor::Ok => RuleVerdict::Accept,
            TeamColor::Reject => RuleVerdict::Reject,
            TeamColor::Swap { key, team } => u8::try_from(key + 1)
                .map_or(RuleVerdict::Reject, |row| RuleVerdict::Swap { row, team }),
        }
    }
}

/// Lobby row `i` as a slot of the draft (key `i - 1`).
fn slot_of(i: usize, r: &LobbyRow) -> SlotDraft {
    SlotDraft {
        key: i32::try_from(i).unwrap_or(i32::MAX) - 1,
        name: r.name.clone(),
        kind: r.kind,
        nation: r.nation,
        team: r.team,
        handicap: r.handicap,
        color: r.color,
    }
}

/// `.wld` for the `.lvl` (`FUN_004BDDA0`: path without the last four characters + `".wld"`).
fn world_path(level: &str) -> String {
    let chars: Vec<char> = level.chars().collect();
    let keep = chars.len().saturating_sub(4);
    chars[..keep].iter().collect::<String>() + ".wld"
}

/// Continue FNV-1a 64.
fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Map checksum: FNV-1a 64 over the bytes of the `.lvl` and then of the `.wld`; if one is missing, `None`. The
/// original compares only the line checksum of the `.lvl` (re/multiplayer-menus L.1.3); the
/// reimplementation also covers the `.wld`.
pub fn map_checksum(vfs: &Vfs, lvl: &str) -> Option<u64> {
    let level = vfs.read(lvl).ok()?;
    let world = vfs.read(&world_path(lvl)).ok()?;
    Some(fnv(fnv(FNV_OFFSET, &level), &world))
}

/// Does this machine have the map `map` of the session (map filter of the game list, re/multiplayer-menus L.1.3;
/// also after a map change)? Fixed map: `.lvl` and `.wld` readable with the same [`map_checksum`]; random map:
/// always (differing generator data are rejected by the rules checksum when loading). Network savegame
/// (`MapId::Saved`): always returns `false` and must never be asked for it - it lies in the savegame folder of the
/// machine, which the session checks itself (`NetSession::saved_here`, `read_net_head`).
pub fn map_available(vfs: &Vfs, map: &MapId) -> bool {
    match map {
        MapId::Fixed { path, checksum } => map_checksum(vfs, path) == Some(*checksum),
        MapId::Random { .. } => true,
        MapId::Saved { .. } => false,
    }
}

/// Can the draft become a network session? Fixed maps with a path and random maps with a seed text (`random_slots`
/// always sets it); savegames take their own path (`{0x12}`).
fn check_draft(draft: &MatchDraft) -> Result<(), String> {
    match draft.info.level_type {
        0 if draft.info.path.is_empty() => Err("no map selected".into()),
        1 if draft.info.name.is_empty() => Err("random map without seed text".into()),
        0 | 1 => Ok(()),
        other => Err(format!(
            "LevelType {other} is not yet supported on the network"
        )),
    }
}

/// Session data from the match draft (`LAN3 CREATE` and `LAN31 CHANGEGAME`, `FUN_004B9700`): name `name` (beforehand
/// via [`session_name`]), fixed map of the draft with [`map_checksum`], map name, settings and player count (`Info`)
/// of the draft, raster `raster` (on a map change that of the running session). The rows are built by
/// [`rows_from_draft`] (only when hosting; on a change those of the book stay). Random map (`LevelType 1`):
/// `MapId::Random` from `Info` (seed text = `Info.Name`, size, player count = `SessionInfo.players` =
/// `RandomPlayers + 2`, map type, landscape, time of day), map name = seed text; no file is read (`Info.Path` is that
/// of the last chosen fixed map and does not apply).
pub fn session_from_draft(
    draft: &MatchDraft,
    name: String,
    vfs: &Vfs,
    raster: TurnConfig,
) -> Result<SessionInfo, String> {
    check_draft(draft)?;
    let players = u8::try_from(draft.info.players)
        .ok()
        .filter(|n| (2..=8).contains(n))
        .ok_or_else(|| format!("{} players (expected 2 to 8)", draft.info.players))?;
    let (spacing, lead) = raster.raw();
    let info = &draft.info;
    let map = if info.level_type == 1 {
        MapId::Random {
            seed_text: info.name.clone(),
            size: info.size_class,
            players: i32::from(players),
            terrain: info.random_terrain,
            landscape: info.random_landscape,
            daytime: info.random_daytime,
        }
    } else {
        let path = info.path.clone();
        let checksum =
            map_checksum(vfs, &path).ok_or_else(|| format!("map \"{path}\" not readable"))?;
        MapId::Fixed { path, checksum }
    };
    let s = &draft.settings;
    Ok(SessionInfo {
        name,
        map,
        map_name: info.name.clone(),
        settings: SessionSettings {
            game_type: s.game_type,
            unit_limit: s.unit_limit,
            resources: s.resources,
            time_limit: s.time_limit,
            lock_teams: s.lock_teams,
            map_explored: s.map_explored,
            save_replay: s.save_replay,
        },
        players,
        spacing,
        lead,
    })
}

/// Lobby rows 0 ... `players - 1` from the slots of the draft (key = row - 1; without a slot open with team and color
/// = row), type 0 ... 4 (otherwise open), never ready, without a machine. Names of the computers (type 1 ... 4) are
/// cleaned by [`clean_name`], so that host and client show them the same.
pub fn rows_from_draft(draft: &MatchDraft, players: u8) -> Vec<LobbyRow> {
    (0..i32::from(players))
        .map(|row| {
            let slot = draft.slots.iter().find(|s| s.key == row - 1);
            let kind = slot.map_or(0, |s| if (0..=4).contains(&s.kind) { s.kind } else { 0 });
            let name = slot.map_or_else(String::new, |s| {
                if (1..=4).contains(&kind) {
                    clean_name(&s.name)
                } else {
                    s.name.clone()
                }
            });
            LobbyRow {
                kind,
                name,
                nation: slot.map_or(0, |s| s.nation),
                team: slot.map_or(row, |s| s.team),
                handicap: slot.map_or(0, |s| s.handicap),
                color: slot.map_or(row, |s| s.color),
                ready: false,
                peer: None,
            }
        })
        .collect()
}

/// Map change of the host (`LAN31 CHANGEGAME`): session name as when hosting ([`session_name`]), session from the
/// draft in the raster of the running session, then [`HostCore::set_session`] (checks the rows, distributes the
/// state); only then does the cleaned name go back to `NetSession`. `Err` with a reason: nothing changed.
fn change_game(
    core: &mut HostCore,
    app: &mut MenuApp,
    vfs: &Vfs,
    texts: &Texts,
    player: &str,
) -> Result<(), String> {
    let raster = {
        let s = &core.lobby().session;
        TurnConfig::new(s.spacing, s.lead)?
    };
    let mut cfg = app.config().clone();
    let name = session_name(&mut cfg, texts, player);
    let session = session_from_draft(&app.data().draft, name, vfs, raster)?;
    core.set_session(session)?;
    app.set_net_session(name_units(&cfg.net_session));
    Ok(())
}

/// Session name (E-300 adopted, `FUN_004EB190`): `NetSession` of the settings; if empty (also only spaces) ->
/// "<15789> - <player name>" ("Session - Anna"); cleaned and shortened to 39 characters ([`clean_session_name`]) and
/// written back to `NetSession`.
pub fn session_name(cfg: &mut EngineConfig, texts: &Texts, player: &str) -> String {
    let own = name_text(&cfg.net_session);
    let name = if own.trim().is_empty() {
        clean_session_name(&format!("{} - {player}", texts.display(SESSION_TEXT)))
    } else {
        clean_session_name(&own)
    };
    cfg.net_session = name_field(&name);
    name
}

/// Game edition for `Hello`: "de" or "en" according to `nml_rda::testutil::game_version`, otherwise
/// `"unbekannt"` (wire value, compared between machines).
pub fn edition(vfs: &Vfs) -> String {
    match game_version(vfs) {
        Ok(GameVersion::German) => "de".into(),
        Ok(GameVersion::English) => "en".into(),
        Err(e) => {
            log::warn!("Network: {e}");
            "unbekannt".into()
        }
    }
}

/// Own sign-on: network version, `PROTOCOL_VERSION`, `FORMAT_VERSION`, game edition, name and the build ID `build` of
/// the program (`nml-game` sets it at build time, tests "test").
pub fn own_hello(vfs: &Vfs, name: &str, build: &str) -> Hello {
    Hello {
        listen_port: None,
        net_version: NET_VERSION,
        protocol: nml_control::PROTOCOL_VERSION,
        format: FORMAT_VERSION,
        edition: edition(vfs),
        name: name.to_string(),
        build: build.to_string(),
        seat: None,
    }
}

/// Step of the raster 0 ... 2 for `T`/`V` (display in `LEVELINFO`); a raster outside the steps (only via the command
/// line) shows the default 0.
fn stage_of(spacing: u8, lead: u8) -> i32 {
    (0..3)
        .find(|&i| RasterStage::from_index(i).config().raw() == (spacing, lead))
        .unwrap_or(0)
}

/// Draft of the lobby from the lobby state: rows as slots (key = row - 1), map description from the own catalog by the
/// path of the session (the map is checked locally), settings of the session.
fn lobby_draft(
    l: &LobbyState,
    maps: &MapCatalog,
) -> (Vec<SlotDraft>, LevelInfoData, DraftSettings) {
    let slots = l
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| slot_of(i, r))
        .collect();
    let s = &l.session;
    let players = i32::from(s.players);
    let info = match &s.map {
        MapId::Fixed { path, .. } => {
            let entry = maps
                .maps
                .iter()
                .find(|m| m.entry.path.eq_ignore_ascii_case(path));
            LevelInfoData {
                name: entry.map_or_else(|| s.map_name.clone(), |m| m.name.clone()),
                path: path.clone(),
                size_class: entry.map_or(0, |m| m.size_class),
                players,
                level_type: 0,
                ..LevelInfoData::default()
            }
        }
        // `players` = `SessionInfo.players` ([`session_from_draft`]).
        MapId::Random {
            seed_text,
            size,
            players: _,
            terrain,
            landscape,
            daytime,
        } => LevelInfoData {
            name: seed_text.clone(),
            path: String::new(),
            size_class: *size,
            players,
            level_type: 1,
            random_terrain: *terrain,
            random_landscape: *landscape,
            random_daytime: *daytime,
        },
        // Network savegame (re/multiplayer-saves S.2.2 step 5: `LevelType` 2, map name = savegame name).
        MapId::Saved { name, .. } => LevelInfoData {
            name: name.clone(),
            path: String::new(),
            players,
            level_type: 2,
            ..LevelInfoData::default()
        },
    };
    let settings = DraftSettings {
        game_type: s.settings.game_type,
        unit_limit: s.settings.unit_limit,
        resources: s.settings.resources,
        time_limit: s.settings.time_limit,
        lock_teams: s.settings.lock_teams,
        map_explored: s.settings.map_explored,
        save_replay: s.settings.save_replay,
        session: s.name.clone(),
    };
    (slots, info, settings)
}

/// The host listens and announces.
struct HostParts {
    listener: Box<dyn Listener>,
    /// `None` if the search port was taken (second host on the same machine): lobby without announcement.
    beacon: Option<Box<dyn Beacon>>,
    /// Next connection number (1, 2, ...).
    next_peer: u8,
    /// ID of the session in the announcement (E-300).
    instance: u64,
}

/// Cached head of a network savegame: length and modification time of the file when read and the result of
/// [`read_net_head`] (`None`: unreadable or not a network savegame).
struct SavedHead {
    len: u64,
    modified: Option<SystemTime>,
    head: Option<(u64, u8)>,
}

/// Stage of the session.
enum Stage {
    Idle,
    /// Connection setup to the chosen game.
    Connecting {
        connecting: Box<dyn Connecting>,
        deadline: u64,
        addr: SocketAddr,
    },
    /// Connected, waiting for `Welcome` and the first lobby state.
    Joining {
        driver: Box<NetDriver>,
        deadline: u64,
    },
    Lobby {
        driver: Box<NetDriver>,
    },
    /// Removal or host gone: connection closed, the message stands until the "OK" (`{0x2D}`).
    Gone,
    /// Start arrived; the driver, until the flow fetches it for loading ([`NetSession::take_start`]).
    Loading {
        driver: Option<Box<NetDriver>>,
    },
    Match,
}

/// The network session.
pub struct NetSession {
    transport: Box<dyn Transport>,
    identity: NetIdentity,
    paths: NetPaths,
    stage: Stage,
    host: Option<HostParts>,
    seeker: Option<Box<dyn Seeker>>,
    list: SessionList,
    next_query_ms: u64,
    nonce: u32,
    /// Program time of the last [`Self::pump`].
    now_ms: u64,
    /// Sign-on of the running session (with the player name).
    hello: Hello,
    is_host: bool,
    /// Chat lines, formatted ([`chat_line`]), oldest first.
    chat: Vec<String>,
    /// Lobby state (stays until the end of the session: display after removal, `LockTeams` at the start).
    lobby: Option<LobbyState>,
    own_row: Option<u8>,
    /// Message 15212 is shown or `{0x2F}` queued (once per session, deviating from E-311).
    told: bool,
    /// Map checksums per path (the search queries every second).
    map_sums: BTreeMap<String, Option<u64>>,
    /// Heads of the network savegames per name (the search queries every second; read again as soon as length or
    /// modification time of the file change).
    saved_heads: BTreeMap<String, SavedHead>,
    /// Client: last checked map of the session (at the join that of the announcement); if it changes in a lobby state,
    /// [`map_available`] checks again.
    checked_map: Option<MapId>,
    /// Last given to the UI (otherwise the local copy of the handlers would never persist).
    shown: Option<NetMenuView>,
    shown_revision: Option<u32>,
    fixed_clock: Option<LocalTime>,
}

impl fmt::Debug for NetSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetSession")
            .field("phase", &self.phase())
            .field("is_host", &self.is_host)
            .field("own_row", &self.own_row)
            .finish_non_exhaustive()
    }
}

impl NetSession {
    pub fn new(transport: Box<dyn Transport>, identity: NetIdentity, paths: NetPaths) -> Self {
        let hello = identity.hello.clone();
        Self {
            transport,
            identity,
            paths,
            stage: Stage::Idle,
            host: None,
            seeker: None,
            list: SessionList::new(),
            next_query_ms: 0,
            nonce: 0,
            now_ms: 0,
            hello,
            is_host: false,
            chat: Vec::new(),
            lobby: None,
            own_row: None,
            told: false,
            map_sums: BTreeMap::new(),
            saved_heads: BTreeMap::new(),
            checked_map: None,
            shown: None,
            shown_revision: None,
            fixed_clock: None,
        }
    }

    /// Fixed local time for the drivers of this session (tests, check runs; [`NetDriver::set_fixed_clock`]).
    pub fn set_fixed_clock(&mut self, clock: Option<LocalTime>) {
        self.fixed_clock = clock;
    }

    pub fn phase(&self) -> NetPhase {
        match &self.stage {
            Stage::Idle if self.seeker.is_some() => NetPhase::Searching,
            Stage::Idle => NetPhase::Idle,
            Stage::Connecting { .. } | Stage::Joining { .. } => NetPhase::Connecting,
            Stage::Lobby { .. } | Stage::Gone => NetPhase::Lobby,
            Stage::Loading { .. } => NetPhase::Loading,
            Stage::Match => NetPhase::Match,
        }
    }

    /// The start has arrived and `Go` not yet: input is locked.
    pub fn phase_is_loading(&self) -> bool {
        self.phase() == NetPhase::Loading
    }

    /// This machine is host of an open lobby (then `GameFlow::start_match` starts the network match).
    pub fn is_host_lobby(&self) -> bool {
        self.is_host && matches!(self.stage, Stage::Lobby { .. })
    }

    /// The lobby is a loaded one (`LobbyState::seats` not empty): its waiting room is `LAN4`, not `LAN2`.
    pub fn is_loaded_lobby(&self) -> bool {
        self.lobby.as_ref().is_some_and(LobbyState::is_loaded)
    }

    /// Session data of the lobby (also when loading: `LockTeams` of the match).
    pub fn session(&self) -> Option<&SessionInfo> {
        self.lobby.as_ref().map(|l| &l.session)
    }

    /// Driver of the lobby or of the join (before the start).
    pub fn driver(&self) -> Option<&NetDriver> {
        match &self.stage {
            Stage::Joining { driver, .. } | Stage::Lobby { driver } => Some(driver),
            Stage::Loading { driver } => driver.as_deref(),
            _ => None,
        }
    }

    /// Like [`Self::driver`], mutable (control channel `net_debug drop` in the lobby).
    pub fn driver_mut(&mut self) -> Option<&mut NetDriver> {
        match &mut self.stage {
            Stage::Joining { driver, .. } | Stage::Lobby { driver } => Some(driver),
            Stage::Loading { driver } => driver.as_deref_mut(),
            _ => None,
        }
    }

    pub fn status(&self) -> NetSessionStatus {
        let rtt_ms = self
            .driver()
            .and_then(NetDriver::host_core)
            .map(|core| {
                core.peers()
                    .into_iter()
                    .filter_map(|(peer, _)| core.rtt_ms(peer).map(|ms| (peer, ms)))
                    .collect()
            })
            .unwrap_or_default();
        NetSessionStatus {
            phase: self.phase(),
            is_host: self.is_host,
            games: self.list.games().to_vec(),
            lobby: self.lobby.clone(),
            own_row: self.own_row,
            rtt_ms,
            port: self.host.as_ref().map(|h| h.listener.port()),
            chat: self.chat.clone(),
        }
    }

    /// `UiEffect::Search` (`FUN_004EB2E0`): `LAN1` starts the search at every status query of its frame - only start if
    /// it is not already running (then with an empty list); off: stop asking, the list stays for the `JOIN` (the
    /// original takes the up to 1 s old copy).
    pub fn search(&mut self, on: bool) {
        if !on {
            self.seeker = None;
            return;
        }
        if self.seeker.is_some() {
            return;
        }
        if !matches!(self.stage, Stage::Idle) {
            log::debug!("Network (discovery): search discarded during a session");
            return;
        }
        match self.transport.seeker() {
            Ok(seeker) => {
                log::info!("Network (discovery): LAN search started");
                self.seeker = Some(seeker);
                self.list.clear();
                self.next_query_ms = self.now_ms;
            }
            Err(e) => log::warn!("Network (discovery): search not possible: {e}"),
        }
    }

    /// Application commands of the network menus (re/multiplayer-menus L.9); `false` for all others.
    pub fn on_app_cmd(
        &mut self,
        cmd: &AppCmd,
        app: &mut MenuApp,
        vfs: &Vfs,
        texts: &Texts,
    ) -> bool {
        match cmd {
            AppCmd::Join => self.join(app),
            AppCmd::Host => self.host(app, vfs, texts),
            AppCmd::HostSavedGame => self.host_saved(app, vfs, texts),
            AppCmd::StartSavedGame => self.start_saved(app),
            // `0x004CA214`: network closed, slot list empty.
            AppCmd::CloseNet => self.close(),
            // `0x004C9B84`: lobby closed, `{9}`, `{5,"start"}`. From `LAN4` (ESC or "OK" of 15212) `LAN4` closes
            // instead of the lobby frames; whether it is open is told by the open dialogs - after `HostGone` the
            // session is already reset.
            AppCmd::LeaveLobby => {
                if app.menu().open_names().iter().any(|n| n == lan4::DIALOG) {
                    app.push_command(close_lan4());
                } else {
                    for c in close_lobby() {
                        app.push_command(c);
                    }
                }
                app.push_command(AppCmd::CloseNet);
                app.push_command(AppCmd::Open {
                    name: "start".to_string(),
                    wait: true,
                });
            }
            // `0x004C9AFF` without a match: `{9}`, message 15212 with `{0x2D}`.
            AppCmd::HostGone => {
                self.close();
                app.tell(KICKED_TEXT, Some(AppCmd::LeaveLobby));
            }
            _ => return false,
        }
        true
    }

    /// `{0x14}` (`LAN4 START`, `0x004CA36C`, deviating from E-328): for the host of a loaded lobby with the start
    /// condition fulfilled ([`HostCore::can_start`]: every row occupied) `LAN4` closes, `{0x15}`, `{0x0D}` like
    /// `START` in `LAN2` (re/multiplayer-menus L.3.4); the core checks the start itself once more. Otherwise only a
    /// log entry; for the client without effect (the original sends `0x10000011` to all, re/multiplayer-saves S.4.2).
    fn start_saved(&mut self, app: &mut MenuApp) {
        let core = match &self.stage {
            Stage::Lobby { driver } if self.is_host => driver.host_core(),
            _ => None,
        };
        match core {
            Some(core) if core.lobby().is_loaded() && core.can_start() => {
                app.push_command(close_lan4());
                app.push_command(AppCmd::WaitDialogs);
                app.push_command(AppCmd::StartMatch);
            }
            Some(_) => log::info!(
                "Network: START of the loaded match discarded (start condition not fulfilled)"
            ),
            None => {
                log::info!("Network: START of the loaded match discarded (no own lobby as host)")
            }
        }
    }

    /// Failure when hosting or joining (E-279, deviating from E-312): session closed, silently `{5,"lan1"}`.
    fn fail_silently(&mut self, app: &mut MenuApp, why: &str) {
        log::info!("Network: {why}; back to LAN1");
        self.close();
        app.push_command(AppCmd::Open {
            name: "lan1".to_string(),
            wait: true,
        });
    }

    /// Player name from `NetName` (cleaned, `clean_name`).
    fn player(app: &MenuApp) -> String {
        clean_name(&name_text(&app.config().net_name))
    }

    /// `{7}`: connect to the chosen session. For a loaded session (`MapId::Saved`, deviating from E-275) the client
    /// reads its file and reports its slot and checksum in the `Hello` (`SeatClaim`); if it is missing in the
    /// meantime, silently `LAN1`. The message applies only to this join.
    fn join(&mut self, app: &mut MenuApp) {
        if !matches!(self.stage, Stage::Idle) {
            log::warn!("Network: join discarded during a session");
            return;
        }
        let Some(game) = app
            .data()
            .lan_choice
            .and_then(|i| self.list.find(i))
            .cloned()
        else {
            return self.fail_silently(app, "the chosen game is no longer there");
        };
        let seat = match &game.announcement.session.map {
            MapId::Saved { name, .. } => match self.saved_head(name) {
                Some((checksum, slot)) => Some(SeatClaim { slot, checksum }),
                None => {
                    return self.fail_silently(
                        app,
                        &format!("network savegame \"{name}\" not (or no longer) readable here"),
                    );
                }
            },
            MapId::Fixed { .. } | MapId::Random { .. } => None,
        };
        self.seeker = None;
        self.hello.seat = seat;
        self.hello.name = Self::player(app);
        self.is_host = false;
        self.told = false;
        self.chat.clear();
        self.lobby = None;
        self.own_row = None;
        // The game list has already checked this map (map filter, re/multiplayer-menus L.1.3).
        self.checked_map = Some(game.announcement.session.map.clone());
        log::info!("Network: joining \"{}\" at {}", game.name, game.addr);
        self.stage = Stage::Connecting {
            connecting: self.transport.connect(game.addr, JOIN_TIMEOUT_MS),
            deadline: self.now_ms.saturating_add(JOIN_TIMEOUT_MS),
            addr: game.addr,
        };
    }

    /// `{8}`: create the session from the draft, listen, announce, open the lobby.
    fn host(&mut self, app: &mut MenuApp, vfs: &Vfs, texts: &Texts) {
        if !matches!(self.stage, Stage::Idle) {
            log::warn!("Network: hosting discarded during a session");
            return;
        }
        match self.open_host(app, vfs, texts) {
            Ok(()) => {
                self.chat.clear();
                for c in open_lobby() {
                    app.push_command(c);
                }
            }
            Err(e) => self.fail_silently(app, &format!("hosting failed: {e}")),
        }
    }

    fn open_host(&mut self, app: &mut MenuApp, vfs: &Vfs, texts: &Texts) -> Result<(), String> {
        let draft = app.data().draft.clone();
        check_draft(&draft)?;
        let player = Self::player(app);
        let mut cfg = app.config().clone();
        let name = session_name(&mut cfg, texts, &player);
        app.set_net_session(name_units(&cfg.net_session));
        let raster = RasterStage::from_index(app.config().net_turn_raster).config();
        // Player count of the session = `Info.players` (2 ... 8, `session_from_draft`); the rows follow it (if a slot
        // is missing, the row is open), so that `LobbyBook::new` never fails on "X players, but Y rows".
        let session = session_from_draft(&draft, name, vfs, raster)?;
        let rows = rows_from_draft(&draft, session.players);
        let book = LobbyBook::new(session, rows, player.clone())?;
        self.start_hosting(book, player)
    }

    /// Listen, announce and create the core with the book `book` (`{8}` and `{0x12}`); the own row is that of the host
    /// in the book (in the loaded lobby that of its slot).
    fn start_hosting(&mut self, book: LobbyBook, player: String) -> Result<(), String> {
        let listener = self
            .transport
            .listen(self.identity.port)
            .map_err(|e| format!("game port {} not open: {e}", self.identity.port))?;
        let beacon = match self.transport.beacon() {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!(
                    "Network: search port taken ({e}), the lobby opens without announcement"
                );
                None
            }
        };
        self.hello.name = player;
        // The host reports no listening port (only clients are probed).
        self.hello.listen_port = None;
        // The host reports no slot, also after a join to a savegame.
        self.hello.seat = None;
        let own_row = book.row_of(HOST_PEER);
        let (spacing, lead) = (book.state().session.spacing, book.state().session.lead);
        let core = HostCore::new(self.hello.clone(), book, Box::new(ClientLobbyRules));
        let mut driver = NetDriver::host(core, self.paths.clone());
        driver.set_fixed_clock(self.fixed_clock);
        // The probe of the listening ports of its clients runs over a transport of its own.
        driver.set_transport(self.transport.fork());
        let seed = (self.identity.seed)();
        let instance = fnv(
            fnv(
                fnv(FNV_OFFSET, self.hello.name.as_bytes()),
                &self.now_ms.to_le_bytes(),
            ),
            &seed.to_le_bytes(),
        );
        log::info!(
            "Network: host listening on port {}, session \"{}\", raster {spacing}/{lead}",
            listener.port(),
            driver
                .host_core()
                .map_or("", |c| c.lobby().session.name.as_str())
        );
        self.host = Some(HostParts {
            listener,
            beacon,
            next_peer: 1,
            instance,
        });
        self.seeker = None;
        self.is_host = true;
        self.told = false;
        self.own_row = own_row;
        self.lobby = driver.host_core().map(|c| c.lobby().clone());
        self.stage = Stage::Lobby {
            driver: Box::new(driver),
        };
        Ok(())
    }

    /// `{0x12}` (`FUN_004C80D0`, re/multiplayer-saves S.2.2; deviating from E-330): name from the name buffer
    /// (`GameApp+0x177C`), empty -> back to `LAN3` without a message (deviating).
    /// Otherwise read and check the network savegame `savegame/multiplayer/<Name>` ([`read_net_save`]); if that fails,
    /// message 99 106 via `{5,"lan3"}`. Fewer than two human slots in the game -> 99 107, own slot not among them ->
    /// 99 108. Otherwise host ([`Self::open_saved_host`]) and open `LAN4` (`FUN_0042E510`); if hosting itself fails
    /// (e.g. the game port), like an unreadable state 99 106.
    fn host_saved(&mut self, app: &mut MenuApp, vfs: &Vfs, texts: &Texts) {
        if !matches!(self.stage, Stage::Idle) {
            log::warn!("Network: hosting a savegame discarded during a session");
            return;
        }
        let name = app.screen_state().save_name.get();
        // Open `LAN3` again (`{5,"lan3"}`), with message `text` or without (`None`).
        let refuse = |app: &mut MenuApp, text: Option<i32>, why: &str| {
            log::info!("Network: savegame \"{name}\" not hosted: {why}");
            app.push_command(AppCmd::Open {
                name: "lan3".to_string(),
                wait: true,
            });
            if let Some(text) = text {
                app.tell(text, None);
            }
        };
        if name.is_empty() {
            // Deviation from the original (E-330): there the screen stays black without a way out; here back to
            // `LAN3`, without a message (nothing went wrong).
            return refuse(app, None, "no name in the name buffer");
        }
        let check = if valid_save_name(&name) {
            read_net_save(&self.paths.net_save(&name), vfs)
        } else {
            Err("not a valid savegame name".to_string())
        };
        let check = match check {
            Ok(check) => check,
            Err(e) => return refuse(app, Some(SAVE_NOT_LOADED), &e),
        };
        let local_slot = check.local_slot();
        if check.seats.len() < 2 {
            let why = format!("only {} human slot in the game", check.seats.len());
            return refuse(app, Some(SAVE_ONE_PLAYER), &why);
        }
        if !check.seats.contains(&local_slot) {
            let why = format!("own slot {} has dropped out", u32::from(local_slot) + 1);
            return refuse(app, Some(SAVE_SLOT_OUT), &why);
        }
        match self.open_saved_host(&name, &check, app, texts) {
            Ok(()) => {
                self.chat.clear();
                app.push_command(open_lan4());
            }
            Err(e) => {
                self.close();
                refuse(app, Some(SAVE_NOT_LOADED), &format!("hosting failed: {e}"));
            }
        }
    }

    /// Loaded lobby from the checked state: session name as when hosting ([`session_name`]), map `MapId::Saved { name,
    /// checksum }`, map name = savegame name, settings from the start package (fixed teams from the host part), raster
    /// from `NetTurnRaster`, player count = row count, one row per human slot in the game (nation, team, color,
    /// handicap from the state); the host is put by [`LobbyBook::loaded`] into the row of its slot.
    fn open_saved_host(
        &mut self,
        name: &str,
        check: &NetSaveCheck,
        app: &mut MenuApp,
        texts: &Texts,
    ) -> Result<(), String> {
        let player = Self::player(app);
        let mut cfg = app.config().clone();
        let session_name = session_name(&mut cfg, texts, &player);
        app.set_net_session(name_units(&cfg.net_session));
        let raster = RasterStage::from_index(app.config().net_turn_raster).config();
        let (spacing, lead) = raster.raw();
        let setup = &check.file.sim.start.setup;
        let players = u8::try_from(check.seats.len()).map_err(|_| "too many slots".to_string())?;
        let session = SessionInfo {
            name: session_name,
            map: MapId::Saved {
                name: name.to_string(),
                checksum: check.checksum,
            },
            map_name: name.to_string(),
            settings: SessionSettings {
                game_type: setup.mode,
                unit_limit: setup.living_space,
                resources: setup.resource_setting,
                time_limit: setup.time_limit,
                lock_teams: check.file.host.lock_teams,
                map_explored: setup.map_explored,
                save_replay: false,
            },
            players,
            spacing,
            lead,
        };
        let state = &check.file.sim.state;
        let rows = check
            .seats
            .iter()
            .map(|&slot| {
                let p = &state.players[usize::from(slot)];
                LobbyRow {
                    kind: 0,
                    name: String::new(),
                    nation: p.nation,
                    team: p.team,
                    handicap: p.handicap,
                    color: p.color,
                    ready: false,
                    peer: None,
                }
            })
            .collect();
        let book = LobbyBook::loaded(
            session,
            rows,
            check.seats.clone(),
            check.local_slot(),
            &player,
        )?;
        self.start_hosting(book, player)
    }

    /// `{9}`: session closed (`Bye` to the counterpart(s), host: `Bye { HOST_LEFT }` to all), search off, everything
    /// empty.
    pub fn close(&mut self) {
        if let Some(driver) = self.driver_mut() {
            driver.leave(LEAVE_REASON);
        }
        self.reset();
    }

    /// Forget the session without `Bye` (the match has left its driver itself).
    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.host = None;
        self.seeker = None;
        self.list.clear();
        self.is_host = false;
        self.chat.clear();
        self.lobby = None;
        self.own_row = None;
        self.told = false;
        self.checked_map = None;
    }

    /// Lobby wishes of the UI (`UiEffect::Lobby`): for the host wishes for row 0 and changes of other rows (the core
    /// checks the rights), `START` with the start condition and the map change `CHANGEGAME` (the session judges); for
    /// the client only its row, ready and chat (other rows, `START` and `CHANGEGAME` are dropped).
    pub fn on_lobby(&mut self, a: &LobbyAction, app: &mut MenuApp, vfs: &Vfs, texts: &Texts) {
        let is_host = self.is_host;
        let Stage::Lobby { driver } = &mut self.stage else {
            log::info!("Network: lobby wish without a lobby discarded: {a:?}");
            return;
        };
        if is_host {
            let Some(core) = driver.host_core_mut() else {
                return;
            };
            let edit = match a.clone() {
                LobbyAction::Row {
                    row: 0,
                    nation,
                    team,
                    handicap,
                    color,
                } => {
                    core.lobby_wish(LobbyWish::Row {
                        nation,
                        team,
                        handicap,
                        color,
                    });
                    None
                }
                LobbyAction::Row {
                    row,
                    nation,
                    team,
                    handicap,
                    color,
                } => Some(HostEdit::Row {
                    row,
                    nation,
                    team,
                    handicap,
                    color,
                }),
                LobbyAction::Kind { row, kind } => Some(HostEdit::Kind { row, kind }),
                LobbyAction::Ready { row: 0, ready } => {
                    core.lobby_wish(LobbyWish::Ready(ready));
                    None
                }
                LobbyAction::Ready { row, ready } => Some(HostEdit::Ready { row, ready }),
                LobbyAction::Chat(text) => {
                    core.lobby_wish(LobbyWish::Chat(text));
                    None
                }
                // re/multiplayer-menus L.3.4 (`0x004D48EE`...`0x004D4944`): lobby closed, `{0x15}`, `{0x0D}`; the
                // reimplementation checks beforehand itself, the core once more at the start.
                LobbyAction::Start => {
                    if core.can_start() {
                        for c in close_lobby() {
                            app.push_command(c);
                        }
                        app.push_command(AppCmd::WaitDialogs);
                        app.push_command(AppCmd::StartMatch);
                    } else {
                        log::info!("Network: START discarded (start condition not fulfilled)");
                    }
                    None
                }
                // `LAN31 CHANGEGAME` (`0x004D50C4`...`0x004D518C`): the session judges. `LAN31` has filled the draft;
                // on success as on rejection the next state to the UI rebuilds it from the lobby state. Success:
                // `LAN31` and side frame closed, lobby open (original: after the broadcast `0x10000014`); rejection
                // (e.g. a join onto a row that drops out): message 15402 without a follow-up command, `LAN31` stays
                // open.
                LobbyAction::ChangeGame => {
                    match change_game(core, app, vfs, texts, &self.hello.name) {
                        Ok(()) => {
                            log::info!(
                                "Network: map change distributed: \"{}\" to \"{}\"",
                                core.lobby().session.name,
                                core.lobby().session.map_name
                            );
                            // Only with an open `LAN31`: otherwise `open_lobby` would open the already open lobby
                            // frames a second time.
                            if app.menu().open_names().iter().any(|d| d == CHANGE_DIALOG) {
                                for c in close_change_game(app.config().level_type) {
                                    app.push_command(c);
                                }
                            } else {
                                log::info!(
                                    "Network: map change without an open LAN31, the frames stay"
                                );
                            }
                        }
                        Err(e) => {
                            log::info!("Network: map change rejected: {e}");
                            app.tell(CHANGE_GAME_REFUSED, None);
                        }
                    }
                    self.shown_revision = None;
                    None
                }
            };
            if let Some(edit) = edit
                && let Err(e) = core.host_edit(edit)
            {
                log::info!("Network: lobby change rejected: {e}");
            }
            return;
        }
        let Some(core) = driver.client_core_mut() else {
            return;
        };
        let own = core.own_row();
        match a.clone() {
            LobbyAction::Row {
                row,
                nation,
                team,
                handicap,
                color,
            } if Some(row) == own => core.lobby_wish(LobbyWish::Row {
                nation,
                team,
                handicap,
                color,
            }),
            LobbyAction::Ready { row, ready } if Some(row) == own => {
                core.lobby_wish(LobbyWish::Ready(ready));
            }
            LobbyAction::Chat(text) => core.lobby_wish(LobbyWish::Chat(text)),
            other => log::info!("Network: lobby wish of the client discarded: {other:?}"),
        }
    }

    /// One frame of the session (in every frame): search, connect, accept, announce, driver, lobby events into
    /// application commands, finally the state to the UI (`MenuData::net`, draft of the lobby).
    pub fn pump(&mut self, now_ms: u64, app: &mut MenuApp, vfs: &Vfs) {
        self.now_ms = now_ms;
        self.pump_search(vfs);
        self.pump_stage(app, vfs);
        self.publish(app);
    }

    fn pump_search(&mut self, vfs: &Vfs) {
        let now = self.now_ms;
        let Some(seeker) = self.seeker.as_mut() else {
            return;
        };
        if now >= self.next_query_ms {
            seeker.query(self.nonce);
            self.nonce = self.nonce.wrapping_add(1);
            self.next_query_ms = now.saturating_add(QUERY_INTERVAL_MS);
        }
        for (from, a) in seeker.poll() {
            // Map filter (re/multiplayer-menus L.1.3): fixed map locally with the same checksum; random map always;
            // network savegame only with the same own file.
            let map_ok = match &a.session.map {
                MapId::Fixed { path, checksum } => {
                    *self
                        .map_sums
                        .entry(path.clone())
                        .or_insert_with(|| map_checksum(vfs, path))
                        == Some(*checksum)
                }
                MapId::Random { .. } => true,
                MapId::Saved { name, checksum } => self.saved_here(name, *checksum),
            };
            self.list.update(now, from, a, &self.hello, |_| map_ok);
        }
        self.list.expire(now);
    }

    fn pump_stage(&mut self, app: &mut MenuApp, vfs: &Vfs) {
        let now = self.now_ms;
        self.stage = match std::mem::replace(&mut self.stage, Stage::Idle) {
            Stage::Connecting {
                mut connecting,
                deadline,
                addr,
            } => match connecting.poll() {
                Some(Ok(link)) => {
                    log::info!("Network: connected to {addr}, signing on");
                    // The listener is open before the `Hello` goes out: its port is in it.
                    let listener = open_client_listener(
                        self.transport.as_mut(),
                        &mut self.hello,
                        self.identity.listen,
                    );
                    let mut driver = NetDriver::client(
                        ClientCore::new(self.hello.clone()),
                        link,
                        self.paths.clone(),
                    );
                    driver.set_fixed_clock(self.fixed_clock);
                    driver.set_transport(self.transport.fork());
                    if let Some(listener) = listener {
                        driver.set_listener(listener);
                    }
                    Stage::Joining {
                        driver: Box::new(driver),
                        deadline,
                    }
                }
                Some(Err(e)) => {
                    self.fail_silently(app, &format!("No host reachable at {addr} ({e})"));
                    return;
                }
                None => Stage::Connecting {
                    connecting,
                    deadline,
                    addr,
                },
            },
            Stage::Joining {
                mut driver,
                deadline,
            } => {
                driver.pump(now);
                let events = driver.take_lobby_events();
                if let Some(text) = driver.failure().map(String::from) {
                    self.fail_silently(app, &format!("joining failed: {text}"));
                    return;
                }
                let welcomed = driver.client_core().is_some_and(|c| c.lobby().is_some());
                // The first state can already carry a different map than the announcement.
                let map_ok = !welcomed
                    || driver
                        .client_core()
                        .and_then(ClientCore::lobby)
                        .is_none_or(|l| self.map_still_here(&l.session.map, vfs));
                if !map_ok {
                    self.leave_for_missing_map(&mut driver, app, false);
                    Stage::Gone
                } else if welcomed {
                    self.chat.clear();
                    // The first state of a loaded lobby opens `LAN4` instead of the lobby.
                    let loaded = driver
                        .client_core()
                        .and_then(ClientCore::lobby)
                        .is_some_and(|l| !l.seats.is_empty());
                    if loaded {
                        log::info!("Network: in the waiting room of the loaded match (LAN4)");
                        app.push_command(open_lan4());
                    } else {
                        log::info!("Network: in the lobby");
                        for c in open_lobby() {
                            app.push_command(c);
                        }
                    }
                    self.lobby_events(events, app);
                    self.lobby = driver.client_core().and_then(|c| c.lobby().cloned());
                    self.own_row = driver.client_core().and_then(ClientCore::own_row);
                    Stage::Lobby { driver }
                } else if now >= deadline {
                    driver.leave(NO_ANSWER);
                    self.fail_silently(app, "the host did not accept the sign-on");
                    return;
                } else {
                    Stage::Joining { driver, deadline }
                }
            }
            Stage::Lobby { mut driver } => {
                self.accept_links(&mut driver);
                driver.pump(now);
                self.answer_queries(&driver);
                let events = driver.take_lobby_events();
                let gone = self.lobby_events(events, app);
                let mut map_missing = false;
                if self.is_host {
                    self.lobby = driver.host_core().map(|c| c.lobby().clone());
                } else if let Some(core) = driver.client_core() {
                    if let Some(l) = core.lobby() {
                        if gone || self.map_still_here(&l.session.map, vfs) {
                            self.lobby = Some(l.clone());
                        } else {
                            map_missing = true;
                        }
                    }
                    self.own_row = core.own_row().or(self.own_row);
                }
                if gone {
                    Stage::Gone
                } else if map_missing {
                    self.leave_for_missing_map(&mut driver, app, true);
                    Stage::Gone
                } else if !self.is_host && driver.failure().is_some() {
                    // Connection to the host lost without `Bye` (crash, line): like "host gone".
                    log::info!(
                        "Network: connection to the host lost: {}",
                        driver.failure().unwrap_or_default()
                    );
                    self.host_gone(app);
                    Stage::Gone
                } else if driver.start_info().is_some() {
                    log::info!("Network: start of the host arrived, loading the map");
                    Stage::Loading {
                        driver: Some(driver),
                    }
                } else {
                    Stage::Lobby { driver }
                }
            }
            Stage::Loading {
                driver: Some(mut driver),
            } => {
                driver.pump(now);
                Stage::Loading {
                    driver: Some(driver),
                }
            }
            other => other,
        };
    }

    /// Lobby events: chat lines, removal (message 15212 once), departure of the host and an unexpected `Rejected`
    /// (`{0x2F}` once); `true` if the lobby is over for this machine.
    fn lobby_events(&mut self, events: Vec<CoreEvent>, app: &mut MenuApp) -> bool {
        let mut gone = false;
        for ev in events {
            match ev {
                CoreEvent::Chat { name, text, .. } => {
                    self.chat.push(chat_line(&name, &text));
                    if self.chat.len() > MAX_CHAT_LINES {
                        self.chat.remove(0);
                    }
                }
                CoreEvent::Kicked => {
                    // `0x004D44B8`...`0x004D44F2` (deviating from E-305): the client recognizes the removal by the
                    // `Bye`.
                    if !self.told {
                        self.told = true;
                        app.tell(KICKED_TEXT, Some(AppCmd::LeaveLobby));
                    }
                    gone = true;
                }
                CoreEvent::HostLeft => {
                    self.host_gone(app);
                    gone = true;
                }
                CoreEvent::Rejected { reason } => {
                    // The core reports `Rejected` only before the `Welcome`; if it arrives in the lobby anyway, it does
                    // not end silently but like "host gone".
                    log::warn!("Network: rejected in the lobby: {reason}");
                    self.host_gone(app);
                    gone = true;
                }
                _ => {}
            }
        }
        gone
    }

    /// Host gone (`HOST_MIGRATE` -> `{0x2F}`, once).
    fn host_gone(&mut self, app: &mut MenuApp) {
        if !self.told {
            self.told = true;
            app.push_command(AppCmd::HostGone);
        }
    }

    /// Client: is the map `map` of a lobby state still here? Equal to the last checked one (at the first state that of
    /// the announcement) -> yes, without reading; otherwise [`map_available`] (network savegame: [`Self::saved_here`]),
    /// and on success it is the new checked map.
    fn map_still_here(&mut self, map: &MapId, vfs: &Vfs) -> bool {
        if self.checked_map.as_ref() == Some(map) {
            return true;
        }
        let here = match map {
            MapId::Saved { name, checksum } => self.saved_here(name, *checksum),
            MapId::Fixed { .. } | MapId::Random { .. } => map_available(vfs, map),
        };
        if here {
            self.checked_map = Some(map.clone());
            true
        } else {
            false
        }
    }

    /// Is the network savegame `name` with the checksum `checksum` of the simulation part here (map filter)?
    fn saved_here(&mut self, name: &str, checksum: u64) -> bool {
        self.saved_head(name)
            .is_some_and(|(sum, _)| sum == checksum)
    }

    /// Head ([`read_net_head`]: checksum of the simulation part, own slot) of the own network savegame `name`, cached
    /// per name with length and modification time of the file; `None` if the name must not be a file name (before every
    /// path construction), the file is missing or is not a readable network savegame.
    fn saved_head(&mut self, name: &str) -> Option<(u64, u8)> {
        if !valid_save_name(name) {
            return None;
        }
        let path = self.paths.net_save(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            self.saved_heads.remove(name);
            return None;
        };
        let (len, modified) = (meta.len(), meta.modified().ok());
        if let Some(known) = self.saved_heads.get(name)
            && known.len == len
            && known.modified == modified
        {
            return known.head;
        }
        let head = read_net_head(&path)
            .inspect_err(|e| log::info!("Network: network savegame \"{name}\" not usable: {e}"))
            .ok();
        self.saved_heads.insert(
            name.to_string(),
            SavedHead {
                len,
                modified,
                head,
            },
        );
        head
    }

    /// Client without the map of the session (deviating from E-322): `Bye` with [`MAP_MISSING_REASON`] (before `{9}`,
    /// which would otherwise send [`LEAVE_REASON`]), lobby closed (if open), `{9}`, `{5,"lan1"}`, above it message
    /// [`MAP_MISSING`] without a follow-up command. For the host the row becomes open (departure as before); `{9}`
    /// forgets the session.
    fn leave_for_missing_map(
        &mut self,
        driver: &mut NetDriver,
        app: &mut MenuApp,
        lobby_open: bool,
    ) {
        log::info!("Network: the map of the match is missing or differs, leaving the lobby");
        driver.leave(MAP_MISSING_REASON);
        self.told = true;
        if lobby_open {
            for c in close_lobby() {
                app.push_command(c);
            }
        }
        app.push_command(AppCmd::CloseNet);
        app.push_command(AppCmd::Open {
            name: "lan1".to_string(),
            wait: true,
        });
        app.tell(MAP_MISSING, None);
    }

    /// Host: new connections to the driver (numbers 1, 2, ... in order of acceptance).
    fn accept_links(&mut self, driver: &mut NetDriver) {
        let Some(h) = self.host.as_mut() else {
            return;
        };
        for link in h.listener.accept_pending() {
            if h.next_peer == u8::MAX {
                log::warn!("Network: no connection number free anymore, connection discarded");
                continue;
            }
            driver.add_link(h.next_peer, link);
            h.next_peer += 1;
        }
    }

    /// Host: answer search queries, only in the lobby with a free index.
    fn answer_queries(&mut self, driver: &NetDriver) {
        let (Some(h), Some(core)) = (self.host.as_mut(), driver.host_core()) else {
            return;
        };
        let Some(beacon) = h.beacon.as_mut() else {
            return;
        };
        let queries = beacon.poll_queries();
        if queries.is_empty() || core.phase() != &Phase::Gathering {
            return;
        }
        let lobby = core.lobby();
        // Free rows = without a machine, over all rows (in the loaded lobby the host stands in the row of its slot, not
        // always in row 0); for new lobbies the same values as before.
        let free = lobby.rows.iter().filter(|r| r.peer.is_none()).count();
        let total = lobby.rows.len().saturating_sub(1);
        if free == 0 {
            return;
        }
        for (from, nonce) in queries {
            beacon.answer(
                from,
                &Announcement {
                    instance: h.instance,
                    nonce,
                    versions: self.hello.clone(),
                    tcp_port: h.listener.port(),
                    free: free as u8,
                    total: total as u8,
                    session: lobby.session.clone(),
                },
            );
        }
    }

    /// State to the UI, only on change (the handlers keep their local copy until the next state).
    fn publish(&mut self, app: &mut MenuApp) {
        let in_lobby = matches!(self.stage, Stage::Lobby { .. } | Stage::Gone);
        let visible = self.seeker.is_some()
            || in_lobby
            || matches!(self.stage, Stage::Connecting { .. } | Stage::Joining { .. });
        let view = visible.then(|| NetMenuView {
            games: self
                .list
                .games()
                .iter()
                .map(|g| FoundGameView {
                    instance: g.instance,
                    name: g.name.clone(),
                })
                .collect(),
            lobby: in_lobby.then(|| self.lobby_view()).flatten(),
        });
        if view != self.shown {
            app.set_net(view.clone());
            self.shown = view;
        }
        match self.lobby.as_ref().filter(|_| in_lobby) {
            Some(l) if self.shown_revision != Some(l.revision) => {
                let (slots, info, settings) = lobby_draft(l, &app.data().maps);
                app.set_lobby_draft(slots, info, settings);
                self.shown_revision = Some(l.revision);
            }
            Some(_) => {}
            None => self.shown_revision = None,
        }
    }

    fn lobby_view(&self) -> Option<LobbyView> {
        let l = self.lobby.as_ref()?;
        let mut ready = [false; 8];
        let mut peers = [false; 8];
        for (i, r) in l.rows.iter().enumerate().take(8) {
            ready[i] = r.ready;
            peers[i] = r.peer.is_some();
        }
        Some(LobbyView {
            is_host: self.is_host,
            own_row: self.own_row.unwrap_or(0),
            ready,
            peers,
            chat: self.chat.clone(),
            stage: stage_of(l.session.spacing, l.session.lead),
            max_rtt_ms: if self.is_host { l.max_rtt_ms } else { None },
        })
    }

    /// Client: the driver as soon as the start of the host has arrived; it goes to `GameFlow::start_net_match`.
    pub fn take_start(&mut self) -> Option<NetDriver> {
        match &mut self.stage {
            Stage::Loading { driver } => driver.take().map(|d| *d),
            _ => None,
        }
    }

    /// Host: start from the lobby after `{0x0D}`. The start request comes from the **session**: [`lobby_draft`] of the
    /// lobby state with the map catalog `maps`, without the open rows (`FUN_004E9800`), as `{0x0D}` builds it from the
    /// draft - never from `Info` or what `LAN31` last showed (deviating from E-281). Then rules of the map
    /// ([`net_rules`]: `.lvl` or random map), setup [`to_net_setup`] with the seed of the identity, slots of the rows
    /// from the same calculation ([`row_slots`]), start package, start information per row (game speed `GameSpeed` of
    /// the host, session name), then [`HostCore::start_from_lobby`] (checks the lobby once more). Success: the driver
    /// with its own start for `GameFlow::start_net_match`, listener and announcement closed. Error (reason as text):
    /// the lobby stays.
    pub fn start_host_match(
        &mut self,
        maps: &MapCatalog,
        cfg: &EngineConfig,
        vfs: &Vfs,
        texts: &Texts,
    ) -> Result<NetDriver, String> {
        if !self.is_host_lobby() {
            return Err("no lobby as host".into());
        }
        if let Some(MapId::Saved { name, checksum }) = self.session().map(|s| s.map.clone()) {
            return self.start_saved_host_match(&name, checksum, vfs);
        }
        let request = self.session_request(maps)?;
        let req = &request;
        let rules = net_rules(vfs, req)?;
        let seed = (self.identity.seed)();
        let setup = to_net_setup(req, &rules, texts, seed)?;
        let slots = row_slots(req, &rules, texts)?;
        let active = active_slots(&rules);
        let now = self.now_ms;
        let Stage::Lobby { driver } = &mut self.stage else {
            return Err("no lobby as host".into());
        };
        let core = driver
            .host_core_mut()
            .ok_or_else(|| "no host core".to_string())?;
        let infos = start_infos(
            core.lobby(),
            &slots,
            &active,
            &setup,
            &rules,
            config_speed(cfg),
        )?;
        Self::distribute_start(core, &infos)?;
        log::info!("Network: start from the lobby distributed (seed {seed})");
        self.take_started(now)
    }

    /// [`HostCore::start_from_lobby`] with the start information per row `infos`; the host gets that of its row
    /// (`row_of(HOST_PEER)`, in the loaded lobby not necessarily row 0).
    fn distribute_start(core: &mut HostCore, infos: &[(u8, StartInfo)]) -> Result<(), String> {
        let host_row = core
            .lobby()
            .rows
            .iter()
            .position(|r| r.peer == Some(HOST_PEER))
            .and_then(|i| u8::try_from(i).ok())
            .ok_or_else(|| "the host has no row in the lobby".to_string())?;
        let host_info = infos
            .iter()
            .find(|(row, _)| *row == host_row)
            .map(|(_, info)| info.clone())
            .ok_or_else(|| format!("row {} (host) has no slot", host_row + 1))?;
        core.start_from_lobby(|_, row| {
            infos
                .iter()
                .find(|(r, _)| *r == row)
                .map_or_else(|| host_info.clone(), |(_, info)| info.clone())
        })
    }

    /// After [`HostCore::start_from_lobby`]: pump the driver (the core reports the own start), session into `Loading`,
    /// listener and announcement closed; the driver goes to `GameFlow::start_net_match`.
    fn take_started(&mut self, now: u64) -> Result<NetDriver, String> {
        let Stage::Lobby { driver } = &mut self.stage else {
            return Err("no lobby as host".into());
        };
        driver.pump(now);
        if driver.start_info().is_none() {
            return Err("the core did not report the start".into());
        }
        let Stage::Lobby { driver } =
            std::mem::replace(&mut self.stage, Stage::Loading { driver: None })
        else {
            unreachable!("just checked");
        };
        // No new players anymore (`T+7 = 0`, re/multiplayer-menus L.3.4); the core rejects later `Hello`s anyway
        // (E-287).
        self.host = None;
        Ok(*driver)
    }

    /// Start of the loaded lobby (`LAN4 START` -> `{0x14}` -> `{0x0D}`; deviating from E-328): the host reads its
    /// network savegame `savegame/multiplayer/<name>` once more ([`read_net_save`]) and builds the start information
    /// per row ([`saved_start_infos`]): start package and speed from its file, slot of the row from
    /// `LobbyState::seats`, occupation from rows and state, marker `SavedStart { name, checksum, steps }` with the
    /// checksum of the session (`MapId::Saved`, which every client confirmed when joining). On loading, every machine
    /// checks its own file against it (`GameFlow::start_saved_net_match`). If the file of the host has changed since
    /// hosting (checksum differs from that of the session) or is not readable, the start counts as rejected: reason
    /// in the log, `LAN4` opens again at the host, the clients stay in `LAN4`.
    fn start_saved_host_match(
        &mut self,
        name: &str,
        checksum: u64,
        vfs: &Vfs,
    ) -> Result<NetDriver, String> {
        if !valid_save_name(name) {
            return Err(format!("\"{name}\" is not a valid savegame name"));
        }
        let check = read_net_save(&self.paths.net_save(name), vfs)
            .map_err(|e| format!("network savegame \"{name}\": {e}"))?;
        // The checksum of the session comes from hosting; start package and tick come from the newly read file. Both
        // must fit together, otherwise all machines fail with misleading messages.
        if check.checksum != checksum {
            return Err(format!("the savegame \"{name}\" has changed since hosting"));
        }
        let now = self.now_ms;
        let Stage::Lobby { driver } = &mut self.stage else {
            return Err("no lobby as host".into());
        };
        let core = driver
            .host_core_mut()
            .ok_or_else(|| "no host core".to_string())?;
        let saved = nml_net::SavedStart {
            name: name.to_string(),
            checksum,
            steps: check.file.sim.state.steps,
        };
        let infos = saved_start_infos(core.lobby(), &check.file, saved)?;
        Self::distribute_start(core, &infos)?;
        log::info!(
            "Network: start of the loaded match \"{name}\" distributed (tick {})",
            check.file.sim.state.steps
        );
        self.take_started(now)
    }

    /// Start request from the lobby state of the core: slots, map description and settings like the draft of the
    /// lobby ([`lobby_draft`]), open rows removed ([`MatchDraft::remove_open_slots`]), then
    /// [`MatchRequest::from_draft`] - the same request as before via `{0x0D}` from the draft of the host.
    fn session_request(&self, maps: &MapCatalog) -> Result<MatchRequest, String> {
        let Stage::Lobby { driver } = &self.stage else {
            return Err("no lobby as host".into());
        };
        let core = driver
            .host_core()
            .ok_or_else(|| "no host core".to_string())?;
        let (slots, info, settings) = lobby_draft(core.lobby(), maps);
        let mut draft = MatchDraft {
            map: None,
            info,
            slots,
            settings,
        };
        draft.remove_open_slots();
        Ok(MatchRequest::from_draft(&draft))
    }

    /// The start from the lobby failed before loading ([`Self::start_host_match`]): the lobby stays. `{0x0D}` removed
    /// the open rows from the draft (`remove_open_slots`); the next [`Self::pump`] therefore rebuilds the draft from
    /// the lobby state, even if the state does not change anymore.
    pub fn start_rejected(&mut self) {
        self.shown_revision = None;
    }

    /// The network start failed (`GameFlow::start_net_match` has already reported `Loaded { error }` and opened the
    /// main menu): leave the driver, session closed.
    pub fn start_failed(&mut self, mut driver: NetDriver) {
        driver.leave(LEAVE_REASON);
        self.reset();
    }

    /// After every frame (`GameFlow::net_step`) with the driver of the match: `Go` -> match runs; if the start fails
    /// before `Go` (host aborts, connection gone), loading also counts as over, so that input is free for the "OK" of
    /// the message; if the match is over (no driver anymore or no match), the session is closed.
    pub fn follow_match(&mut self, match_net: Option<&NetDriver>, running: bool) {
        match &self.stage {
            Stage::Loading { driver: None } => match match_net {
                Some(d) if d.started() || d.failure().is_some() => self.stage = Stage::Match,
                Some(_) => {}
                None if !running => self.reset(),
                None => {}
            },
            Stage::Match if match_net.is_none() || !running => self.reset(),
            _ => {}
        }
    }
}

/// Rules of the map of a start request at the host, before loading for the start package: fixed map from its `.lvl`
/// ([`load_rules`]), random map (`Info.LevelType 1`, [`random_map_spec`]) from its description
/// ([`load_random_rules`], the same generator as when loading). The start package then carries `random_map`, the
/// client generates the map from it; differing generator data are rejected by the rules checksum when loading
/// (deviating from E-314). The command-line path (`nml-game`, `net_start`) stays with `.lvl`. Errors as a reason
/// text.
pub fn net_rules(vfs: &Vfs, req: &MatchRequest) -> Result<nml_sim::rules::Rules, String> {
    match random_map_spec(req) {
        Some(spec) => load_random_rules(vfs, &spec),
        None => load_rules(vfs, &req.level),
    }
    .map_err(|e| format!("{e:#}"))
}

/// Start information per human row: slot from [`row_slots`], occupation of all slots (humans with their machine and
/// name, computers with the name of the setup), start package, raster and name of the session, speed of the host. A
/// human row without a slot is an error.
pub fn start_infos(
    lobby: &LobbyState,
    slots: &[Option<u8>; 8],
    active: &[u8],
    setup: &MatchSetup,
    rules: &nml_sim::rules::Rules,
    game_speed: u8,
) -> Result<Vec<(u8, StartInfo)>, String> {
    let mut seats: [Seat; 8] = std::array::from_fn(|_| Seat {
        peer: None,
        name: String::new(),
    });
    for (i, row) in lobby.rows.iter().enumerate().take(8) {
        let Some(slot) = slots[i] else {
            continue;
        };
        let seat = &mut seats[usize::from(slot)];
        if row.kind == KIND_HUMAN {
            *seat = Seat {
                peer: row.peer,
                name: row.name.clone(),
            };
        } else if let Some(entry) = active
            .iter()
            .position(|&a| a == slot)
            .and_then(|k| setup.lobby.get(k))
        {
            seat.name = entry.name.clone();
        }
    }
    let package = start_package(rules, setup);
    let mut infos = Vec::new();
    for (i, row) in lobby.rows.iter().enumerate().take(8) {
        if row.kind != KIND_HUMAN {
            continue;
        }
        let your_slot =
            slots[i].ok_or_else(|| format!("\"{}\" in row {} has no slot", row.name, i + 1))?;
        infos.push((
            i as u8,
            StartInfo {
                package: package.clone(),
                seats: seats.clone(),
                your_slot,
                spacing: lobby.session.spacing,
                lead: lobby.session.lead,
                // Rule of the protocol, slider 0 ... 20; a higher value would hold up the start.
                game_speed: game_speed.min(nml_net::MAX_GAME_SPEED),
                session: lobby.session.name.clone(),
                saved: None,
            },
        ));
    }
    Ok(infos)
}

/// Start information per row of a loaded lobby: start package of the file of the host `file`, slot of the row from
/// `LobbyState::seats`, occupation per player slot (slots with a row: machine and name of the row; otherwise without
/// machine with the name from the state), raster and name of the session, speed from the file of the host and the
/// marker `saved`. A row without a slot or without a machine is an error (the core required every row occupied).
pub fn saved_start_infos(
    lobby: &LobbyState,
    file: &GameSaveFile,
    saved: nml_net::SavedStart,
) -> Result<Vec<(u8, StartInfo)>, String> {
    if lobby.seats.len() != lobby.rows.len() {
        return Err(format!(
            "{} slots for {} rows of the loaded lobby",
            lobby.seats.len(),
            lobby.rows.len()
        ));
    }
    let players = &file.sim.state.players;
    let mut seats: [Seat; 8] = std::array::from_fn(|slot| Seat {
        peer: None,
        name: players
            .get(slot)
            .map(|p| p.name.clone())
            .unwrap_or_default(),
    });
    for (i, (row, &slot)) in lobby.rows.iter().zip(&lobby.seats).enumerate() {
        let Some(peer) = row.peer.filter(|_| row.kind == KIND_HUMAN) else {
            return Err(format!("row {} is not occupied", i + 1));
        };
        let seat = seats
            .get_mut(usize::from(slot))
            .ok_or_else(|| format!("slot {slot} of row {} invalid", i + 1))?;
        *seat = Seat {
            peer: Some(peer),
            name: row.name.clone(),
        };
    }
    Ok((0u8..)
        .zip(&lobby.seats)
        .map(|(row, &your_slot)| {
            (
                row,
                StartInfo {
                    package: file.sim.start.clone(),
                    seats: seats.clone(),
                    your_slot,
                    spacing: lobby.session.spacing,
                    lead: lobby.session.lead,
                    // Rule of the protocol, slider 0 ... 20; a higher value would hold up the start.
                    game_speed: file.host.game_speed.min(nml_net::MAX_GAME_SPEED),
                    session: lobby.session.name.clone(),
                    saved: Some(saved.clone()),
                },
            )
        })
        .collect())
}

/// Checked network savegame: the file, the checksum of its simulation part ([`sim_checksum`]) and the human slots in
/// the game (control kind human, result 0) in slot order.
pub struct NetSaveCheck {
    pub file: GameSaveFile,
    pub checksum: u64,
    pub seats: Vec<u8>,
}

impl NetSaveCheck {
    /// Own slot of the machine that wrote the file (`NetSaveInfo::local_slot`; [`read_net_save`] has checked the
    /// network part).
    pub fn local_slot(&self) -> u8 {
        self.file.host.net.as_ref().map_or(0, |n| n.local_slot)
    }
}

/// Read and check the network savegame `path` (for host and start): file ([`GameSaveFile::from_bytes`]), rules of
/// the map as when resuming (`GameSession::resume_from`: fixed map via [`load_rules`], random map from the
/// description in the start package via [`load_random_rules`]), [`GameSaveFile::check`] and the network part. Errors
/// as a reason text.
pub fn read_net_save(path: &Path, vfs: &Vfs) -> Result<NetSaveCheck, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} not readable: {e}", path.display()))?;
    let file = GameSaveFile::from_bytes(&bytes).map_err(|e| format!("{e:#}"))?;
    let start = &file.sim.start;
    let rules = match &start.random_map {
        Some(spec) => load_random_rules(vfs, spec),
        None => load_rules(vfs, &start.level),
    }
    .map_err(|e| format!("{e:#}"))?;
    file.check(&rules).map_err(|e| format!("{e:#}"))?;
    if file.host.net.is_none() {
        return Err("not a network savegame (network part missing)".to_string());
    }
    let seats = (0u8..)
        .zip(&file.sim.state.players)
        .filter(|(_, p)| p.control == nml_sim::state::Control::Human && p.result == 0)
        .map(|(slot, _)| slot)
        .collect();
    let checksum = sim_checksum(&file.sim);
    Ok(NetSaveCheck {
        file,
        checksum,
        seats,
    })
}

/// Head of a network savegame for map filter and join: read the file ([`GameSaveFile::from_bytes`]), network part
/// present, network flag set in start package and state; the result is the checksum of the simulation part
/// ([`sim_checksum`]) and the own slot (`local_slot`). Without rules of the map (the search tick queries every
/// second); the full check is done by [`read_net_save`] when hosting and at the start. Errors as a reason text.
pub fn read_net_head(path: &Path) -> Result<(u64, u8), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} not readable: {e}", path.display()))?;
    let file = GameSaveFile::from_bytes(&bytes).map_err(|e| format!("{e:#}"))?;
    let Some(net) = &file.host.net else {
        return Err("not a network savegame (network part missing)".to_string());
    };
    if !(file.sim.start.setup.network && file.sim.state.network) {
        return Err("savegame corrupted (network savegame without network flag)".to_string());
    }
    Ok((sim_checksum(&file.sim), net.local_slot))
}

#[cfg(test)]
mod tests;
