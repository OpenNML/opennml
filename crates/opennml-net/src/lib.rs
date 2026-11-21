//! Network core of "No Man's Land": link, connections, messages, turn raster, lobby, LAN discovery and transport.
//!
//! The crate knows neither GPU nor windows nor `tokio`. It depends only on `opennml-sim`, to carry the start package and
//! commands; the simulation itself sees nothing of the network.

pub mod discovery;
pub mod link;
pub mod lobby;
pub mod lockstep;
pub mod msg;
pub mod transport;
pub mod turn;
pub mod wire;

pub use discovery::{
    Announcement, DISCOVERY_MAGIC, DISCOVERY_PORT, Datagram, ENTRY_TTL_MS, FoundGame, MAX_DATAGRAM,
    QUERY_INTERVAL_MS, SessionList, decode_datagram, encode_datagram,
};
pub use link::mem::{MemLink, MemNet};
pub use link::tcp::{DEFAULT_PORT, HEARTBEAT_MS, TcpHost, TcpLink};
pub use link::{Link, LinkEvent, Outbox};
pub use lobby::{
    AcceptAll, DataInput, DataReport, DataState, EditOutcome, HostEdit, KIND_HUMAN, LobbyBook,
    LobbyRow, LobbyRules, LobbyState, LobbyWish, MAX_DATA_INPUT_BYTES, MAX_DATA_INPUTS,
    MAX_DATA_PATH, MapId, RuleVerdict, SHOWN_FILES, SessionInfo, SessionSettings, compare_inputs,
    inputs_fit, rows_fit,
};
pub use lockstep::{
    ClientCore, CoreEvent, HOST_LOST, HOST_PEER, HaltReason, HostCore, LocalTime, LocalWords,
    Lockstep, PROBE_INTERVAL_MS, Peer, Phase, REJOIN_WINDOW_MS, SUCCESSOR_CONNECT_MS, Succession,
    SuccessionStep, TIMEOUT_MS,
};
pub use msg::{
    CHAT_INPUT, CHAT_SHOWN, HANDED_OVER, HOST_LEFT, Hello, KICKED, MAX_GAME_SPEED, MAX_PLAYER_NAME,
    MAX_SAVE_NAME, MIGRATION_FAILED, MIGRATION_REFUSED, Migrated, NetControl, NetMessage, Rejoin,
    SavedStart, Seat, SeatClaim, StartInfo, Successor, TurnBundle, TurnContribution, UNKNOWN_BUILD,
    Wish, clean_chat, clean_name, clean_session_name, seat_of, valid_save_name,
};
pub use transport::{
    Beacon, CONNECT_ATTEMPT_MS, CONNECT_RETRY_MS, Connecting, Listener, MemTransport, Seeker,
    SystemTransport, Transport,
};
pub use turn::{RasterStage, TurnConfig};
pub use wire::{
    Assembler, CHUNK_SIZE, MAGIC, MAX_ASSEMBLED, MAX_FRAME, NET_VERSION, WireError, encode,
    read_frame, split,
};
