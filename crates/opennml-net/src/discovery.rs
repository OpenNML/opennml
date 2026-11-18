//! Discovery of open games on the LAN.
//!
//! In `LAN1` the client sends a UDP broadcast to `255.255.255.255:2303` ([`DISCOVERY_PORT`]) every second
//! ([`QUERY_INTERVAL_MS`]), and with `--net-loopback` also to `127.0.0.1:2303`; the host answers from the lobby with an
//! [`Announcement`]. A datagram is the magic [`DISCOVERY_MAGIC`], `NET_VERSION` (`u16` LE), the length of the body
//! (`u16` LE) and postcard, at most [`MAX_DATAGRAM`] bytes. Every response is checked and sanitized (E-301 deviation:
//! the original accepts the search response unchecked, `wcscpy` without a length limit, `0x004EAD29`...`0x004EADC6`).
//!
//! [`SessionList`] keeps the visible games like `FUN_004EB2E0`/`FUN_004E9540` (E-278 adopted): entries expire
//! [`ENTRY_TTL_MS`] after the last response; only games with a free slot, the same versions (network, protocol,
//! format, game edition, build ID) and a locally present map with the same checksum are visible; random maps always
//! are. Every response is checked again, even for an already known entry (E-301 deviation). The selection hangs on the
//! session ID `instance`, not on the list index (E-300, partly).
//!
//! The sockets sit behind [`crate::transport::Beacon`] and [`crate::transport::Seeker`]: [`udp`] over `std::net`
//! (non-blocking, read once per frame, no thread), [`mem`] in-process over [`crate::MemNet`].

pub(crate) mod mem;
pub(crate) mod udp;

use crate::lobby::{MapId, SessionInfo};
use crate::msg::{
    Hello, UNKNOWN_BUILD, clean_name, clean_session_name, clean_text, valid_save_name,
};
use crate::wire::{NET_VERSION, WireError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;

/// UDP port of the discovery.
pub const DISCOVERY_PORT: u16 = 2303;
/// Interval between discovery queries in `LAN1` (original: broadcast every second, E-278).
pub const QUERY_INTERVAL_MS: u64 = 1_000;
/// An entry expires this long after its last response (E-278).
pub const ENTRY_TTL_MS: u64 = 3_000;
/// Largest discovery datagram in bytes (fits into one Ethernet frame without fragmentation).
pub const MAX_DATAGRAM: usize = 1_400;
/// Magic at the start of every discovery datagram.
pub const DISCOVERY_MAGIC: [u8; 4] = *b"NMLD";

/// Header of a datagram: magic (4), version (2), length of the body (2).
const HEAD: usize = 8;
/// Maximum length of the game edition and the build ID of an announcement (like log and rejection in the host core).
const MAX_VERSION_TEXT: usize = 32;
/// This many hidden sessions the list remembers for the log (against repetition every second); then it starts over.
const MAX_HIDDEN_LOGGED: usize = 64;

/// A discovery datagram.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Datagram {
    /// Query from the seeker; `nonce` comes back in the announcement.
    Query { nonce: u32 },
    /// Response from the host.
    Announce(Box<Announcement>),
}

/// Announcement of a host in the lobby.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Announcement {
    /// ID of the session (chosen by the host); the selection in `LAN1` hangs on it (E-300).
    pub instance: u64,
    /// `nonce` of the query being answered.
    pub nonce: u32,
    /// Versions and build ID of the host as in its `Hello`; the name is not evaluated.
    pub versions: Hello,
    /// TCP port the host listens on (the seeker takes the address from the sender).
    pub tcp_port: u16,
    /// Free client slots (without the machine itself; computers count as free).
    pub free: u8,
    /// Client slots in total (`players - 1`).
    pub total: u8,
    pub session: SessionInfo,
}

/// Encodes a datagram; `None` if it would be larger than [`MAX_DATAGRAM`] (then nothing is sent).
pub fn encode_datagram(d: &Datagram) -> Option<Vec<u8>> {
    let body = postcard::to_allocvec(d).ok()?;
    if HEAD + body.len() > MAX_DATAGRAM {
        return None;
    }
    let mut out = Vec::with_capacity(HEAD + body.len());
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.extend_from_slice(&NET_VERSION.to_le_bytes());
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

/// Reads a datagram: length (at most [`MAX_DATAGRAM`], checked before postcard), magic, version, length of the body,
/// postcard without remainder. The texts of an announcement come out sanitized: player name via [`clean_name`] (32),
/// session and map name via [`clean_session_name`] (39), game edition and build ID to 32 characters. Map path and
/// seed text stay unchanged: they are identifiers (map check, random map), not display text.
pub fn decode_datagram(bytes: &[u8]) -> Result<Datagram, WireError> {
    if bytes.len() > MAX_DATAGRAM {
        return Err(WireError::Decode(format!(
            "datagram too long ({} bytes, at most {MAX_DATAGRAM})",
            bytes.len()
        )));
    }
    if bytes.len() < HEAD {
        return Err(WireError::Decode(format!(
            "datagram too short ({} bytes)",
            bytes.len()
        )));
    }
    if bytes[..4] != DISCOVERY_MAGIC {
        return Err(WireError::BadMagic);
    }
    let got = u16::from_le_bytes([bytes[4], bytes[5]]);
    if got != NET_VERSION {
        return Err(WireError::Version { got });
    }
    let len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let body = &bytes[HEAD..];
    if len != body.len() {
        return Err(WireError::Decode(format!(
            "length of the datagram does not match ({len} announced, {} present)",
            body.len()
        )));
    }
    let (d, rest): (Datagram, _) =
        postcard::take_from_bytes(body).map_err(|e| WireError::Decode(e.to_string()))?;
    if !rest.is_empty() {
        return Err(WireError::Decode(format!(
            "{} bytes after the datagram",
            rest.len()
        )));
    }
    Ok(match d {
        Datagram::Announce(mut a) => {
            a.versions.name = clean_name(&a.versions.name);
            a.versions.edition = clean_text(&a.versions.edition, MAX_VERSION_TEXT);
            a.versions.build = clean_text(&a.versions.build, MAX_VERSION_TEXT);
            a.session.name = clean_session_name(&a.session.name);
            a.session.map_name = clean_session_name(&a.session.map_name);
            Datagram::Announce(a)
        }
        query => query,
    })
}

/// Queries from received datagrams (sender, `nonce`); anything else and anything broken goes only to the debug log.
pub(crate) fn queries_in(datagrams: Vec<(SocketAddr, Vec<u8>)>) -> Vec<(SocketAddr, u32)> {
    datagrams
        .into_iter()
        .filter_map(|(from, bytes)| match decode_datagram(&bytes) {
            Ok(Datagram::Query { nonce }) => Some((from, nonce)),
            Ok(Datagram::Announce(_)) => {
                log::debug!(
                    "Network (discovery): announcement from {from} at the beacon discarded"
                );
                None
            }
            Err(e) => {
                log::debug!("Network (discovery): datagram from {from} discarded: {e}");
                None
            }
        })
        .collect()
}

/// Announcements from received datagrams (sender, announcement); anything else and anything broken goes only to the
/// debug log.
pub(crate) fn announcements_in(
    datagrams: Vec<(SocketAddr, Vec<u8>)>,
) -> Vec<(SocketAddr, Announcement)> {
    datagrams
        .into_iter()
        .filter_map(|(from, bytes)| match decode_datagram(&bytes) {
            Ok(Datagram::Announce(a)) => Some((from, *a)),
            Ok(Datagram::Query { .. }) => None,
            Err(e) => {
                log::debug!("Network (discovery): datagram from {from} discarded: {e}");
                None
            }
        })
        .collect()
}

/// Encodes an announcement for sending; too large -> warning in the log and `None`.
pub(crate) fn announce_bytes(a: &Announcement) -> Option<Vec<u8>> {
    let bytes = encode_datagram(&Datagram::Announce(Box::new(a.clone())));
    if bytes.is_none() {
        log::warn!(
            "Network (discovery): announcement \"{}\" larger than {MAX_DATAGRAM} bytes, not sent",
            a.session.name
        );
    }
    bytes
}

/// A visible game in `LAN1`.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundGame {
    pub instance: u64,
    /// Sender IP of the last response with the TCP port of the announcement.
    pub addr: SocketAddr,
    /// Displayed session name (E-300: only the name).
    pub name: String,
    pub announcement: Announcement,
    /// Game time or program time from which the entry expires (last response + [`ENTRY_TTL_MS`]).
    pub expires_ms: u64,
}

/// List of open games (`FUN_004EB2E0`, `FUN_004E9540`, E-278), without a clock: the time is passed in.
#[derive(Debug, Default)]
pub struct SessionList {
    /// In the order of the first response.
    games: Vec<FoundGame>,
    /// Last logged reason per hidden session (only against repetition in the log).
    hidden: BTreeMap<u64, String>,
}

impl SessionList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in a response (filter applied anew with every response): visible -> entry new or replaced (in place),
    /// deadline extended; hidden -> a known entry drops out (E-301 deviation). The address is the sender IP with
    /// `tcp_port`. `own`: own versions and build ID; `map_ok`: whether the fixed map or the network savegame is present
    /// locally with the same checksum (not asked for random maps).
    pub fn update(
        &mut self,
        now_ms: u64,
        from: SocketAddr,
        a: Announcement,
        own: &Hello,
        map_ok: impl Fn(&MapId) -> bool,
    ) {
        let instance = a.instance;
        let name = clean_session_name(&a.session.name);
        if let Some(reason) = hide_reason(&a, own, map_ok) {
            self.games.retain(|g| g.instance != instance);
            if self.hidden.get(&instance) != Some(&reason) {
                log::info!("Network (discovery): game \"{name}\" from {from} not shown: {reason}");
                if self.hidden.len() >= MAX_HIDDEN_LOGGED {
                    self.hidden.clear();
                }
                self.hidden.insert(instance, reason);
            }
            return;
        }
        self.hidden.remove(&instance);
        let game = FoundGame {
            instance,
            addr: SocketAddr::new(from.ip(), a.tcp_port),
            name,
            announcement: a,
            expires_ms: now_ms.saturating_add(ENTRY_TTL_MS),
        };
        match self.games.iter_mut().find(|g| g.instance == instance) {
            Some(known) => *known = game,
            None => {
                if own.build == UNKNOWN_BUILD {
                    log::warn!(
                        "Network (discovery): both build IDs are \"{UNKNOWN_BUILD}\", version of \"{}\" not checked",
                        game.name
                    );
                }
                log::info!(
                    "Network (discovery): game \"{}\" found at {}",
                    game.name,
                    game.addr
                );
                self.games.push(game);
            }
        }
    }

    /// Removes entries whose last response was [`ENTRY_TTL_MS`] ago or longer.
    pub fn expire(&mut self, now_ms: u64) {
        self.games.retain(|g| now_ms < g.expires_ms);
    }

    /// Visible games in the order of the first response.
    pub fn games(&self) -> &[FoundGame] {
        &self.games
    }

    /// The game with this session ID, as long as it is visible.
    pub fn find(&self, instance: u64) -> Option<&FoundGame> {
        self.games.iter().find(|g| g.instance == instance)
    }

    /// Empties the list (leaving `LAN1`).
    pub fn clear(&mut self) {
        self.games.clear();
        self.hidden.clear();
    }
}

/// Reason why an announcement is not shown, or `None`.
fn hide_reason(a: &Announcement, own: &Hello, map_ok: impl Fn(&MapId) -> bool) -> Option<String> {
    let v = &a.versions;
    if a.free == 0 {
        return Some("The match is full".into());
    }
    if a.free > a.total {
        return Some(format!(
            "Invalid slot count: {} free of {}",
            a.free, a.total
        ));
    }
    if v.net_version != own.net_version {
        return Some(format!(
            "Different network version: {} instead of {}",
            v.net_version, own.net_version
        ));
    }
    if v.protocol != own.protocol {
        return Some(format!(
            "Different protocol version: {} instead of {}",
            v.protocol, own.protocol
        ));
    }
    if v.format != own.format {
        return Some(format!(
            "Different format version: {} instead of {}",
            v.format, own.format
        ));
    }
    if v.edition != own.edition {
        return Some(format!(
            "Different game edition: \"{}\" instead of \"{}\"",
            clean_text(&v.edition, MAX_VERSION_TEXT),
            own.edition
        ));
    }
    if v.build != own.build {
        // Both IDs go into the log.
        return Some(format!(
            "Different program version: {} instead of {}",
            clean_text(&v.build, MAX_VERSION_TEXT),
            own.build
        ));
    }
    match &a.session.map {
        MapId::Random { .. } => None,
        map @ MapId::Fixed { path, .. } => (!map_ok(map)).then(|| {
            format!(
                "Map \"{}\" is missing here or differs",
                clean_text(path, MAX_VERSION_TEXT * 4)
            )
        }),
        // Only with the same file; a name that must not be a file name is hidden without asking back.
        MapId::Saved { name, .. } if !valid_save_name(name) => Some(format!(
            "Invalid savegame name \"{}\"",
            clean_text(name, MAX_VERSION_TEXT * 4)
        )),
        map @ MapId::Saved { name, .. } => (!map_ok(map)).then(|| {
            format!(
                "Savegame \"{}\" is missing here or differs",
                clean_text(name, MAX_VERSION_TEXT * 4)
            )
        }),
    }
}

#[cfg(test)]
mod tests;
