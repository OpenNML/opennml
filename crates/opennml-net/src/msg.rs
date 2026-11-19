//! Message types of the wire.

use crate::lobby::{LobbyState, LobbyWish};
use opennml_sim::command::Command;
use opennml_sim::setup::StartPackage;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// All messages between host and clients.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum NetMessage {
    /// Client -> host: sign-in with versions.
    Hello(Hello),
    /// Host -> client: accepted, assigned connection slot (peer).
    Welcome { slot: u8 },
    /// Host -> client: rejected, reason as text.
    Reject { reason: String },
    /// Host -> client: start package and slot assignment.
    Start(Box<StartInfo>),
    /// Client -> host: loaded (or error while loading).
    Loaded { error: Option<String> },
    /// Host -> clients: all loaded, the match begins; plus the successor list (empty as long as the host does not fill
    /// it).
    Go { successors: Vec<Successor> },
    /// Client -> host: contribution to a turn.
    Turn(TurnContribution),
    /// Host -> clients: released bundle of a turn.
    Bundle(TurnBundle),
    /// A machine lifts a pause that it triggered itself.
    Resume,
    /// Piece of a large payload (chunking).
    Chunk {
        id: u32,
        index: u32,
        count: u32,
        bytes: Vec<u8>,
    },
    /// Client -> host: state taken over (resync done).
    Synced,
    /// The connection is ended in an orderly way.
    Bye { reason: String },
    /// Heartbeat.
    Ping,
    /// Host -> client (only in `Gathering`): whole lobby state after every change (E-303, E-304 deviation).
    Lobby(Box<LobbyState>),
    /// Client -> host: wish for the own row or a chat line; the host sets the row from the connection.
    LobbyWish(LobbyWish),
    /// Host -> client: chat line of the lobby from row `row` (sanitized, at most [`CHAT_INPUT`] characters).
    Chat { row: u8, text: String },
    /// Host -> client: latency probe (every [`crate::lockstep::PROBE_INTERVAL_MS`] in `Gathering`).
    Probe { id: u32 },
    /// Client -> host: response to `Probe` with the same ID.
    ProbeAck { id: u32 },
    /// Client -> new host: rejoining after the dropout of the host.
    Rejoin(Box<Rejoin>),
    /// New host -> client: migration completed.
    Migrated(Box<Migrated>),
    /// Host -> clients (in `Loading`, right before the `Bye`): the machine of the player `name` could not load the
    /// match, the start is cancelled (E-352). The client shows its own text with the name.
    LoadFailed { name: String },
}

/// Entry of the successor list: a client with peer, slot and the address it listens at (`None` = not reachable from
/// the host; no candidate).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Successor {
    pub peer: u8,
    pub slot: u8,
    pub addr: Option<SocketAddr>,
}

/// Client -> new host: state of the client after the dropout of the old host.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rejoin {
    pub peer: u8,
    pub slot: u8,
    /// Next turn whose contribution the client closes.
    pub next_close: u32,
    /// Next bundle the client expects.
    pub next_bundle: u32,
    /// Retained bundles (margin).
    pub bundles: Vec<TurnBundle>,
    /// Open own contributions (turn >= own next bundle).
    pub open: Vec<TurnContribution>,
    pub desyncs: u32,
}

/// New host -> client: the missing bundles and the state of the migration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Migrated {
    /// Peer of the new host.
    pub host: u8,
    /// Bundles the client is missing up to `K`.
    pub bundles: Vec<TurnBundle>,
    /// Halt from the view of the new host (slot of the pausing player).
    pub paused_by: Option<u8>,
    /// Successor list for a second migration.
    pub successors: Vec<Successor>,
    pub desyncs: u32,
}

/// Reason in the `Bye` of the host that hands over the running match in an orderly way.
pub const HANDED_OVER: &str = "The host has left the match.";

/// Message when a candidate rejects a `Rejoin` because it can still hear the old host.
pub const MIGRATION_REFUSED: &str =
    "No host migration: the host is still reachable for this machine.";

/// Reason in the `Reject` of the new host when the migration fails (a bundle is missing for everyone) or is no longer
/// possible for this machine (after collecting, `Migrated` too large). Whoever receives it gives up
/// ([`crate::Succession::rejected`]) and writes the abort savegame.
pub const MIGRATION_FAILED: &str =
    "Host migration failed: the new host cannot continue the match with this machine.";

/// Reason in the `Bye` to a client that the host throws out of the lobby.
pub const KICKED: &str = "The host has removed you from the game.";

/// Reason in the `Bye` to all clients when the host leaves the lobby.
pub const HOST_LEFT: &str = "The host has left the lobby.";

/// Maximum length of a chat line to which the host truncates (protection against foreign peers; the original's input
/// field holds 79 characters, `size 0x50` with terminating null).
pub const CHAT_INPUT: usize = 80;

/// Displayed length of a chat line.
pub const CHAT_SHOWN: usize = 79;

/// Sign-in of a client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Hello {
    pub net_version: u16,
    pub protocol: u32,
    pub format: u32,
    pub edition: String,
    pub name: String,
    /// Build ID of the program (unequal -> `Reject`). Tests use "test".
    pub build: String,
    /// Slot claim for a loaded match (E-275 deviation): the own slot and the checksum of the simulation part of the
    /// own file; `None` for a new lobby.
    pub seat: Option<SeatClaim>,
    /// Port the client listens on; `None` if listening failed.
    pub listen_port: Option<u16>,
}

/// Slot claim in the `Hello`: the client joins a loaded match in the slot `slot` that it plays in its file; `checksum`
/// is the checksum of the simulation part of that file.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SeatClaim {
    pub slot: u8,
    pub checksum: u64,
}

/// Build ID without Git: two of them match (host core and discovery), with a warning in the log.
pub const UNKNOWN_BUILD: &str = "unbekannt";

/// Slot in the assignment: peer 0 = host, 1 ... 7 clients, `None` = no machine (AI or empty).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Seat {
    pub peer: Option<u8>,
    pub name: String,
}

/// Slot of the machine `peer` in the assignment `seats` (after a migration the slot of the old host has no peer and
/// that of the new host has peer 0); `None` if no slot belongs to it.
pub fn seat_of(seats: &[Seat], peer: u8) -> Option<u8> {
    seats
        .iter()
        .position(|s| s.peer == Some(peer))
        .and_then(|i| u8::try_from(i).ok())
}

/// Start information for a client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StartInfo {
    pub package: StartPackage,
    pub seats: [Seat; 8],
    pub your_slot: u8,
    /// Turn spacing `T` in ticks.
    pub spacing: u8,
    /// Lead `V` in ticks.
    pub lead: u8,
    pub game_speed: u8,
    pub session: String,
    /// Marker "loaded match": every machine continues it from its own file; `None` for a new match.
    pub saved: Option<SavedStart>,
}

/// Start of a loaded match: name of the network savegame (`savegame/multiplayer/<name>`), checksum of its simulation
/// part and tick of the savegame (`state.steps`, origin tick).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SavedStart {
    pub name: String,
    pub checksum: u64,
    pub steps: u64,
}

/// Contribution of a machine to turn `turn`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TurnContribution {
    pub turn: u32,
    pub hash: u64,
    pub commands: Vec<Command>,
    pub wishes: Vec<Wish>,
}

/// Wishes that the host accepts and returns as a control entry.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Wish {
    Pause,
    Speed(u8),
    Save(String),
}

/// Released bundle of a turn.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TurnBundle {
    pub turn: u32,
    pub commands: Vec<Command>,
    pub control: Vec<NetControl>,
}

/// Control entry of a bundle (`NetControl`, not `Control`, because of `opennml_sim::state::Control`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum NetControl {
    /// Pause from slot `by`: everyone halts after the execution tick of the bundle. If the `Resume` already arrived
    /// at a machine before this tick, it does not halt (only time, bundle sequence and state are equal; see
    /// `Phase::Halted`).
    Halt {
        by: u8,
    },
    Speed(u8),
    Save {
        name: String,
    },
    Resync {
        slots: Vec<u8>,
    },
    Stop {
        name: String,
    },
    End {
        name: String,
    },
    /// Orderly handover: the host leaves the match; `peers` are the active clients among whom the new host is chosen.
    Handover {
        peers: Vec<u8>,
    },
}

/// Highest game speed that the host accepts as `Wish::Speed`: `MAX_GAME_SPEED` of the game host
/// (`opennml-client/src/game_save.rs`; `opennml-net` does not depend on `opennml-client`). A larger value lies outside the slider
/// 0 ... 20 (and used to make every network savegame unloadable).
pub const MAX_GAME_SPEED: u8 = 20;

/// Windows device names: as the stem before the first dot not a file name (`NUL.txt` is the device `NUL`). Also the
/// consoles `CONIN$`/`CONOUT$` and the ports with a superscript digit (`COM¹`-`COM³`, `LPT¹`-`LPT³`, U+00B9, U+00B2,
/// U+00B3), which Windows also reads as a device.
const DEVICE_NAMES: [&str; 30] = [
    "CON",
    "NUL",
    "AUX",
    "PRN",
    "CONIN$",
    "CONOUT$",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{B9}",
    "COM\u{B2}",
    "COM\u{B3}",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{B9}",
    "LPT\u{B2}",
    "LPT\u{B3}",
];

/// Longest savegame name in characters.
pub const MAX_SAVE_NAME: usize = 64;

/// Checks a savegame name from the network: the name of a `Wish::Save` becomes a file name on every machine under
/// `savegame/multiplayer/`, so it must not form a path. Rules as when saving in single player (`GameFlow::check_name`:
/// not empty, no `/`, `\`, `:`, not `.`), plus at most [`MAX_SAVE_NAME`] characters, no control characters, none of
/// the characters forbidden on Windows `<>"|?*`, no Windows device name (`CON`, `NUL`, `AUX`, `PRN`, `CONIN$`,
/// `CONOUT$`, `COM1`-`COM9`, `COM¹`-`COM³`, `LPT1`-`LPT9`, `LPT¹`-`LPT³`, as the stem before the first dot, regardless
/// of case), `..` nowhere and no dot or space at the end (Windows strips both when opening: `CON.` and `NUL ` would be
/// devices, `name.` would hit the file `name`; network savegames have no extension). The host accepts a wish only with
/// a valid name, the driver checks once more before every write.
pub fn valid_save_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && !name.contains("..")
        && name.chars().count() <= MAX_SAVE_NAME
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        && !is_device_name(name)
}

/// Stem before the first dot is a Windows device name.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    DEVICE_NAMES.iter().any(|d| stem.eq_ignore_ascii_case(d))
}

/// Longest player name in characters after [`clean_name`].
pub const MAX_PLAYER_NAME: usize = 32;

/// Name of a player without content (empty or only whitespace and control characters).
const FALLBACK_NAME: &str = "Spieler";

/// Sanitizes a player name from the network or from the command line before it goes into `Seat.name` and `StartInfo` -
/// it ends up in the desync report, in `report.txt` and in savegames: control characters and invisible direction and
/// format characters (`U+200B`-`U+200F`, `U+202A`-`U+202E`, `U+2066`-`U+2069`, `U+FEFF`) become spaces, runs of
/// whitespace one space, the edge is dropped; at most [`MAX_PLAYER_NAME`] characters.
/// If nothing remains, the player is called "Spieler". The host sanitizes every `Hello` on acceptance
/// (`HostCore`), the game host its own name. `[` and `]` are dropped (E-309 deviation): the original's lobby displays
/// names with markup such as `[COLOR …]` evaluated.
pub fn clean_name(name: &str) -> String {
    let cut = clean_text(&strip_brackets(name), MAX_PLAYER_NAME);
    if cut.is_empty() {
        FALLBACK_NAME.to_string()
    } else {
        cut
    }
}

/// Sanitizes session and map names: like [`clean_text`] to at most 39 characters, without `[` and `]`.
pub fn clean_session_name(text: &str) -> String {
    clean_text(&strip_brackets(text), 39)
}

/// Sanitizes a chat line of the lobby: without `[` and `]`, like [`clean_text`] to at most [`CHAT_INPUT`] characters.
pub fn clean_chat(text: &str) -> String {
    clean_text(&strip_brackets(text), CHAT_INPUT)
}

/// Maximum length of a foreign reason (`Bye`, `Reject`) in the client's message.
pub const MAX_REASON_SHOWN: usize = 160;

/// Sanitizes the reason of a `Bye` or `Reject` from the host before it goes into a message (`MSGBOX`): without `[` and
/// `]`, like [`clean_text`] to at most [`MAX_REASON_SHOWN`] characters.
pub fn clean_reason(text: &str) -> String {
    clean_text(&strip_brackets(text), MAX_REASON_SHOWN)
}

/// Text without `[` and `]` (no markup from the network).
fn strip_brackets(text: &str) -> String {
    text.chars().filter(|&c| c != '[' && c != ']').collect()
}

/// Sanitizes a text from the network like [`clean_name`], but to at most `max` characters and without a replacement
/// for an empty text (e.g. a foreign `Hello.edition` for log and rejection).
pub fn clean_text(text: &str, max: usize) -> String {
    let spaced: String = text
        .chars()
        .map(|c| if is_hidden(c) { ' ' } else { c })
        .collect();
    let joined = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(max).collect();
    cut.trim_end().to_string()
}

/// Control character or invisible format or direction character.
fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}
