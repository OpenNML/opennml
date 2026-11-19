//! Turn core of the host.
//!
//! The host accepts clients (`Hello` -> `Welcome`/`Reject`), distributes the start, waits for all `Loaded`, sends
//! `Go` and then forms the bundle for each turn from all contributions of the active machines. Its own contribution
//! goes directly into the turn without a network, its own bundle is delivered by [`Lockstep::bundle`] without a
//! network. It checks the senders, accepts wishes, lifts pauses and turns a dropout into the defeat command of the
//! slot.
//!
//! Checksum: When forming bundle `k` it compares the checksum of every client contribution with its own of the same
//! turn (state before `b(k)`). If clients deviate, that is **one** desync, no matter how many; contributions of
//! dropped-out machines do not count. Desync 1 and 2 -> `Resync { slots }` (slots ascending), all halt after `e(k)`;
//! the driver gets `CoreEvent::SendState`, calls [`HostCore::send_state`] with its state, the core sends it as large
//! content to the targets and waits for their `Synced`; then `Resume` to all. Desync 3 -> `Stop { name }` instead of
//! `Resync`, all stay in `Phase::Stopped` after `e(k)`.
//!
//! End: [`HostCore::end`] attaches `End { name }` to the next bundle; with this bundle the session ends
//! (`Phase::Ended`), right after it `Bye` goes to all. A contribution with the wrong turn is a protocol error and
//! costs the slot like a dropout.
//!
//! Lobby: In `Gathering` the host holds a [`LobbyBook`] with the rules of the game host ([`LobbyRules`]). After every
//! change of the `revision` the whole state goes individually to each client ([`HostCore::sync_lobby`]); wishes of the
//! clients apply only to the row of their connection. It measures the latency with `Probe`/`ProbeAck`. Starting is
//! checked ([`HostCore::start_from_lobby`]) or unchecked ([`HostCore::start`]).

use super::{
    CoreEvent, HOST_PEER, HaltReason, Inputs, LocalTime, Lockstep, PROBE_INTERVAL_MS, Peer, Phase,
    TIMEOUT_MS, loss_command, timeout_reason,
};
use crate::link::LinkEvent;
use crate::lobby::{
    DataInput, EditOutcome, HostEdit, KIND_HUMAN, LobbyBook, LobbyRules, LobbyState, LobbyWish,
    MapId, SessionInfo,
};
use crate::msg::{
    HANDED_OVER, HOST_LEFT, Hello, KICKED, MAX_GAME_SPEED, NetControl, NetMessage, SeatClaim,
    StartInfo, Successor, TurnBundle, TurnContribution, UNKNOWN_BUILD, Wish, clean_chat,
    clean_name, clean_text, valid_save_name,
};
use crate::turn::TurnConfig;
use crate::wire::MAX_FRAME;
use opennml_sim::command::{Command, CommandKind};
use std::collections::BTreeMap;
use std::net::SocketAddr;

mod migrate;

/// Maximum number of commands per contribution: An honest machine enters only a handful of commands per turn (a few
/// ticks); the host discards more with a log entry so that neither the bundle nor the command log of the match grows
/// through a single client.
pub const MAX_COMMANDS_PER_TURN: usize = 256;

/// Maximum number of wishes per contribution: only one per kind and turn applies anyway; the limit only keeps the log
/// small.
pub const MAX_WISHES_PER_TURN: usize = 16;

/// Room in the bundle for body, turn number, dropout commands (one per slot, a few bytes) and control entries (names at
/// most [`crate::msg::MAX_SAVE_NAME`] characters, `Resync` at most eight slots) - generously sized.
const BUNDLE_RESERVE: usize = 64 * 1024;

/// Largest encoded length (postcard) of the commands of one contribution. A command has no fixed size (`units:
/// Vec<ObjectId>` among others), so the count alone is not enough: eight contributions of at most this length each plus
/// [`BUNDLE_RESERVE`] stay safely under [`MAX_FRAME`] (1 MiB - 64 KiB, divided by 8: 120 KiB per contribution).
/// Otherwise `encode` of the bundle in the write thread would fail and all connections would close; this way the limit
/// hits only the contribution that exceeds it.
pub const MAX_TURN_COMMAND_BYTES: usize = (MAX_FRAME - BUNDLE_RESERVE) / 8;

/// Maximum length of a foreign game edition in log and rejection (`Hello.edition` has no length limit).
const MAX_EDITION_SHOWN: usize = 16;

/// Maximum length of a foreign build ID in log and rejection (own: 12 characters of commit, `-`, 8 hex characters).
const MAX_BUILD_SHOWN: usize = 32;

/// An accepted client.
#[derive(Debug)]
struct Remote {
    peer: Peer,
    name: String,
    /// Slot from the `Start`.
    slot: Option<u8>,
    /// `Loaded` received without error.
    loaded: bool,
    /// Next expected turn of its contribution.
    next_turn: u32,
    /// Dropped out in the match: no more waiting for it, no more messages from or to it.
    gone: bool,
    /// Listen port from the accepted `Hello`; `None` = the client does not listen.
    listen_port: Option<u16>,
    /// Result of the reachability probe: `None` = still open, `Some(None)` = not reachable, `Some(Some(a))` = reachable
    /// at `a`. Without a listen port it is `Some(None)` immediately; it is set only once.
    reachable: Option<Option<SocketAddr>>,
}

/// Turn core of the host.
pub struct HostCore {
    own: Hello,
    cfg: TurnConfig,
    phase: Phase,
    /// Lobby state; unchanged after the start.
    book: LobbyBook,
    /// Team and color rules of the game host.
    rules: Box<dyn LobbyRules>,
    /// `revision` of the lobby state distributed last.
    sent_revision: u32,
    /// ID of the last latency probe.
    probe_id: u32,
    /// Time of the next probe round.
    next_probe_ms: u64,
    /// Open probe per machine: (ID, send time).
    probes: BTreeMap<Peer, (u32, u64)>,
    /// Latency measured last per machine in ms.
    rtt: BTreeMap<Peer, u32>,
    /// Accepted clients in join order.
    remotes: Vec<Remote>,
    /// Own slot from the `Start`.
    own_slot: Option<u8>,
    self_loaded: bool,
    inputs: Inputs,
    /// Next own turn for `close_round`.
    next_close: u32,
    /// Next turn to form.
    next_bundle: u32,
    /// Received contributions per turn and peer (the own one under [`HOST_PEER`]).
    rounds: BTreeMap<u32, BTreeMap<Peer, TurnContribution>>,
    bundles: BTreeMap<u32, TurnBundle>,
    outbox: Vec<(Peer, NetMessage)>,
    large: Vec<(Peer, u32, Vec<u8>)>,
    events: Vec<CoreEvent>,
    /// Slots that already have a result in the host's state (from the driver after every tick, [`Self::set_results`]).
    results: [bool; 8],
    /// Slots with a dropout whose command goes into the next bundle not yet formed.
    losses: Vec<u8>,
    /// Number of desyncs of the match (desyncs, not resyncs).
    desyncs: u32,
    /// Running resync.
    resync: Option<Resync>,
    /// ID of the last state sent (counts up per resync).
    state_id: u32,
    /// Pause from the same bundle as the running resync: applies as soon as the resync is done.
    pause_after: Option<u8>,
    /// Local time of the host from the driver ([`Self::set_clock`]); without a clock [`LocalTime::FALLBACK`].
    clock: LocalTime,
    /// [`Self::end`] was called: the next bundle carries `End`.
    end_wanted: bool,
    /// [`Self::hand_over`] was called: the next bundle carries the own dropout and `Handover`.
    handover_wanted: bool,
    /// The handover bundle is formed: `Bye` with [`HANDED_OVER`] instead of [`HOST_ENDED_REASON`].
    handed_over: bool,
    /// Machines thrown out of the lobby: the `Bye` stays in the outbox until the next send, the driver still reads
    /// before that; the core discards everything that comes from this connection afterwards. Connection numbers are not
    /// reused.
    dropped: Vec<Peer>,
    /// Host migration, if the core came from a client ([`HostCore::from_client`]).
    migration: Option<migrate::Migration>,
}

/// A running resync.
#[derive(Debug)]
struct Resync {
    /// Targets whose `Synced` is still missing (peers; dropped-out ones fall out).
    waiting: Vec<Peer>,
    /// The state has been sent ([`HostCore::send_state`]).
    sent: bool,
}

impl HostCore {
    /// `own` carries the own versions: a `Hello` must be equal in `net_version`, `protocol`, `format`, `edition` and
    /// `build`; `own.name` is the name of the host. Turn raster and maximum number of clients (`players − 1`) come from
    /// the session of the book (checked by [`LobbyBook::new`]); `rules` checks team and color.
    pub fn new(own: Hello, book: LobbyBook, rules: Box<dyn LobbyRules>) -> Self {
        let session = &book.state().session;
        let cfg = TurnConfig::new(session.spacing, session.lead).unwrap_or_else(|e| {
            log::error!(
                "Network (host): turn raster of the session invalid ({e}), using the default"
            );
            TurnConfig::default()
        });
        let sent_revision = book.state().revision;
        Self {
            own,
            cfg,
            phase: Phase::Gathering,
            book,
            rules,
            sent_revision,
            probe_id: 0,
            next_probe_ms: 0,
            probes: BTreeMap::new(),
            rtt: BTreeMap::new(),
            remotes: Vec::new(),
            own_slot: None,
            self_loaded: false,
            inputs: Inputs::default(),
            next_close: 0,
            next_bundle: 0,
            rounds: BTreeMap::new(),
            bundles: BTreeMap::new(),
            outbox: Vec::new(),
            large: Vec::new(),
            events: Vec::new(),
            results: [false; 8],
            losses: Vec::new(),
            desyncs: 0,
            resync: None,
            state_id: 0,
            pause_after: None,
            clock: LocalTime::FALLBACK,
            end_wanted: false,
            handover_wanted: false,
            handed_over: false,
            dropped: Vec::new(),
            migration: None,
        }
    }

    /// Unchecked start (without lobby check; tests, `NET_GOLDEN`, `connect_net`, command line), only in `Gathering`:
    /// `info_for(peer)` returns the start information per machine, also for the host itself (`HOST_PEER`, reported as
    /// `CoreEvent::Start`). The core sets the grid (`T`/`V`) from its setting. Then `Loading`; later `Hello`s are
    /// rejected (E-287). The checked way is [`Self::start_from_lobby`].
    pub fn start(&mut self, info_for: impl Fn(Peer) -> StartInfo) {
        if self.phase != Phase::Gathering {
            log::warn!("Network (host): start in phase {:?} discarded", self.phase);
            return;
        }
        self.distribute(info_for);
    }

    /// Checked start from the lobby: only in `Gathering`, only if [`LobbyBook::can_start`] holds and every human row
    /// has a connected machine; otherwise `Err` with the reason, the phase stays and no `Start` message goes out.
    /// `info_for(peer, row)` returns the whole start information per machine (the game host computes slot and
    /// occupancy, the core does not assume "slot = row"); the core sets the grid from the session.
    pub fn start_from_lobby(
        &mut self,
        info_for: impl Fn(Peer, u8) -> StartInfo,
    ) -> Result<(), String> {
        if self.phase != Phase::Gathering {
            return Err("The match is already running".into());
        }
        let rows = self.check_start()?;
        log::info!("Network (host): start from the lobby, (machine, row) {rows:?}");
        self.distribute(|peer| {
            let row = rows
                .iter()
                .find(|(p, _)| *p == peer)
                .map_or(0, |(_, row)| *row);
            info_for(peer, row)
        });
        Ok(())
    }

    /// Start condition of the lobby (and a connected machine per human row): (machine, row) of the host and all clients
    /// or the reason. The row of the host comes from the book (in a loaded lobby it stands in the row of its slot, not
    /// necessarily in row 0).
    fn check_start(&self) -> Result<Vec<(Peer, u8)>, String> {
        if !self.book.can_start() {
            return Err("The match cannot start yet: not all players are ready.".into());
        }
        for (i, row) in self.book.state().rows.iter().enumerate() {
            if row.kind != KIND_HUMAN {
                continue;
            }
            let connected = match row.peer {
                Some(HOST_PEER) => true,
                Some(peer) => self.remote(peer).is_some_and(|r| !r.gone),
                None => false,
            };
            if !connected {
                return Err(format!(
                    "The player in row {} is no longer connected.",
                    i + 1
                ));
            }
        }
        let host_row = self
            .book
            .row_of(HOST_PEER)
            .ok_or_else(|| "The host has no row in the lobby.".to_string())?;
        let mut rows = vec![(HOST_PEER, host_row)];
        for r in &self.remotes {
            let row = self
                .book
                .row_of(r.peer)
                .ok_or_else(|| format!("\"{}\" has no row in the lobby.", r.name))?;
            rows.push((r.peer, row));
        }
        Ok(rows)
    }

    /// The lobby meets the start condition of [`Self::start_from_lobby`] (for showing `START`).
    pub fn can_start(&self) -> bool {
        self.phase == Phase::Gathering && self.check_start().is_ok()
    }

    /// Lobby state of the host.
    pub fn lobby(&self) -> &LobbyState {
        self.book.state()
    }

    /// Latency measured last to `peer` in ms.
    pub fn rtt_ms(&self, peer: Peer) -> Option<u32> {
        self.rtt.get(&peer).copied()
    }

    /// Change of the lobby by the host (rights as in the original), only in `Gathering`. A human set to "Open" is a
    /// kick: `Bye { KICKED }` to them (the driver closes afterwards), they drop out, the row is open. Then broadcast.
    pub fn host_edit(&mut self, e: HostEdit) -> Result<(), String> {
        if self.phase != Phase::Gathering {
            return Err("The lobby is already closed.".into());
        }
        let result = match self.book.edit(&e, self.rules.as_ref()) {
            Ok(EditOutcome::Changed) => Ok(()),
            Ok(EditOutcome::Kicked { peer }) => {
                self.kick(peer);
                Ok(())
            }
            Err(reason) => {
                log::info!("Network (host): change {e:?} rejected: {reason}");
                Err(reason)
            }
        };
        self.sync_lobby();
        result
    }

    /// Wish of the host for its own row (same rules as for clients) or a chat line; only in `Gathering`, otherwise has
    /// no effect (log).
    pub fn lobby_wish(&mut self, w: LobbyWish) {
        if self.phase != Phase::Gathering {
            log::warn!(
                "Network (host): lobby wish in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.apply_wish(HOST_PEER, w);
        self.sync_lobby();
    }

    /// New session data (map switch), only in `Gathering`: grid from the session, then broadcast.
    pub fn set_session(&mut self, s: SessionInfo) -> Result<(), String> {
        if self.phase != Phase::Gathering {
            return Err("The lobby is already closed.".into());
        }
        if let Err(reason) = self.book.set_session(s) {
            log::info!("Network (host): map switch rejected: {reason}");
            return Err(reason);
        }
        let session = &self.book.state().session;
        self.cfg = TurnConfig::new(session.spacing, session.lead)?;
        self.sync_lobby();
        Ok(())
    }

    /// Switches the game data check of the lobby on ([`LobbyBook::enable_data_check`]; menu path only), then
    /// broadcast.
    pub fn enable_data_check(&mut self) {
        self.book.enable_data_check();
        self.sync_lobby();
    }

    /// The host's rules checksum and read recording for the session with `session_revision`
    /// ([`LobbyBook::set_rules`]), only in `Gathering`; the checksum goes to every client with the next state. `false`
    /// for another session revision, without the data check or outside `Gathering`.
    pub fn set_rules(
        &mut self,
        session_revision: u32,
        checksum: Option<u64>,
        inputs: Vec<DataInput>,
    ) -> bool {
        if self.phase != Phase::Gathering {
            log::warn!(
                "Network (host): rules checksum in phase {:?} discarded",
                self.phase
            );
            return false;
        }
        let applied = self.book.set_rules(session_revision, checksum, inputs);
        if !applied {
            log::info!(
                "Network (host): rules checksum for session {session_revision} discarded (session {})",
                self.book.state().session_revision
            );
        }
        self.sync_lobby();
        applied
    }

    /// Kick: `Bye { KICKED }`, the machine drops out (without `Lost`; the book has already opened the row). Until the
    /// driver closes the connection, the core discards everything from it (no second `Hello`).
    fn kick(&mut self, peer: Peer) {
        log::info!("Network (host): peer {peer} kicked from the lobby");
        if !self.dropped.contains(&peer) {
            self.dropped.push(peer);
        }
        self.outbox.push((
            peer,
            NetMessage::Bye {
                reason: KICKED.into(),
            },
        ));
        self.forget(peer);
    }

    /// Forgets a machine of the lobby including its latency.
    fn forget(&mut self, peer: Peer) {
        self.remotes.retain(|r| r.peer != peer);
        self.probes.remove(&peer);
        self.rtt.remove(&peer);
        self.update_rtt();
    }

    /// Largest latency into the state (counts `revision` only on change).
    fn update_rtt(&mut self) {
        let max = self.rtt.values().copied().max();
        self.book.set_rtt(max);
    }

    /// Wish of the machine `from` (host: [`HOST_PEER`]): chat to all, otherwise through the book for its own row. In a
    /// loaded lobby there are neither wishes nor chat: log, nothing else.
    fn apply_wish(&mut self, from: Peer, w: LobbyWish) {
        if self.book.state().is_loaded() {
            log::info!(
                "Network (host): wish {} from peer {from} rejected in the loaded lobby",
                w.summary()
            );
            return;
        }
        match w {
            LobbyWish::Chat(text) => self.chat(from, &text),
            w => {
                if let Err(reason) = self.book.wish(from, &w, self.rules.as_ref()) {
                    log::info!(
                        "Network (host): wish {} from peer {from} rejected: {reason}",
                        w.summary()
                    );
                }
            }
        }
    }

    /// Chat line (E-286, deviating from E-310): the sender is the row of the connection, the text cleaned without
    /// `[`/`]` to at most [`crate::msg::CHAT_INPUT`] characters; `Chat` to every client (also to the sender) and as
    /// an event to the host. An empty text is dropped.
    fn chat(&mut self, from: Peer, text: &str) {
        let Some(row) = self.book.row_of(from) else {
            log::warn!("Network (host): chat from peer {from} without a row discarded");
            return;
        };
        let text = clean_chat(text);
        if text.is_empty() {
            log::info!("Network (host): empty chat line from peer {from} discarded");
            return;
        }
        let name = self.book.state().rows[usize::from(row)].name.clone();
        self.send_all(NetMessage::Chat {
            row,
            text: text.clone(),
        });
        self.events.push(CoreEvent::Chat { row, name, text });
    }

    /// Broadcast: If the `revision` has changed since the last broadcast, the whole state goes individually to each
    /// client, and the host reports `LobbyChanged`. Only in `Gathering`.
    fn sync_lobby(&mut self) {
        if self.phase != Phase::Gathering || self.book.state().revision == self.sent_revision {
            return;
        }
        self.sent_revision = self.book.state().revision;
        let msg = NetMessage::Lobby(Box::new(self.book.state().clone()));
        self.send_all(msg);
        self.events.push(CoreEvent::LobbyChanged);
    }

    /// `LobbyWish` of a client: only in `Gathering` (otherwise log).
    fn on_lobby_wish(&mut self, from: Peer, w: LobbyWish) {
        if self.phase != Phase::Gathering {
            log::warn!(
                "Network (host): lobby wish from peer {from} in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.apply_wish(from, w);
    }

    /// Response to a latency probe: measures with `now_ms` (last value per machine).
    fn on_probe_ack(&mut self, now_ms: u64, from: Peer, id: u32) {
        match self.probes.get(&from).copied() {
            Some((pid, sent)) if pid == id => {
                self.probes.remove(&from);
                let ms = u32::try_from(now_ms.saturating_sub(sent)).unwrap_or(u32::MAX);
                self.rtt.insert(from, ms);
                self.update_rtt();
            }
            other => log::warn!(
                "Network (host): probe response {id} from peer {from} discarded (open: {other:?})"
            ),
        }
    }

    /// Distributes the start to all clients and reports its own (`Loading`).
    fn distribute(&mut self, info_for: impl Fn(Peer) -> StartInfo) {
        let (spacing, lead) = self.cfg.raw();
        for r in &mut self.remotes {
            let mut info = info_for(r.peer);
            info.spacing = spacing;
            info.lead = lead;
            r.slot = Some(info.your_slot);
            self.outbox
                .push((r.peer, NetMessage::Start(Box::new(info))));
        }
        let mut own = info_for(HOST_PEER);
        own.spacing = spacing;
        own.lead = lead;
        self.own_slot = Some(own.your_slot);
        self.events.push(CoreEvent::Start(Box::new(own)));
        self.phase = Phase::Loading;
        log::info!(
            "Network (host): start to {} clients, grid {spacing}/{lead}",
            self.remotes.len()
        );
    }

    /// The host has loaded itself (only in `Loading`).
    pub fn loaded_self(&mut self) {
        if self.phase != Phase::Loading {
            log::warn!(
                "Network (host): own loading in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.self_loaded = true;
        self.check_go();
    }

    /// After every tick the driver reports which slots already have a result in the host's state (victory or defeat).
    /// The dropout command does not apply for these slots. Result 3 ("not participating") also counts as "no longer
    /// playing"; the command would have no effect there anyway.
    pub fn set_results(&mut self, defeated: [bool; 8]) {
        self.results = defeated;
    }

    /// Aborts the session for everyone (E-293; e.g. the host could not set up the match; its own loading error goes
    /// through [`Self::load_failed`]): `Bye { reason: text }` to every active client, then `CoreEvent::Failed(text)` and
    /// `Phase::Ended`. The driver closes the connections after sending. In `Phase::Ended` it has no effect (log).
    pub fn abort(&mut self, text: String) {
        if self.phase == Phase::Ended {
            log::warn!("Network (host): abort after the end discarded: {text}");
            return;
        }
        log::warn!("Network (host): session aborted: {text}");
        self.send_all(NetMessage::Bye {
            reason: text.clone(),
        });
        self.fail(text);
    }

    /// Orderly end of the match by the host (it gives up or leaves the match; E-268 deviation instead of host
    /// migration): The next bundle carries `End { name }` with [`LocalTime::net_end_name`] (local time when forming);
    /// whoever forms or receives it is in `Phase::Ended` afterwards, all drivers still compute up to its execution
    /// tick, write their network savegame and stop. Right after the bundle `Bye` goes to every client. The host lifts
    /// a running pause for this (a foreign one too: without ticks no bundle would be formed any more), a pause after a
    /// running resync does not apply; the resync itself runs to the end. `true` if accepted; outside the match (before
    /// `Go`, after stop or end) and a second time it has no effect (log).
    pub fn end(&mut self) -> bool {
        if !self.phase.in_match() || self.end_wanted || self.handover_wanted {
            log::warn!(
                "Network (host): end in phase {:?} has no effect (already requested: {}, handover requested: {})",
                self.phase,
                self.end_wanted,
                self.handover_wanted
            );
            return false;
        }
        self.end_wanted = true;
        log::info!("Network (host): end of the match requested, it comes with the next bundle");
        self.clear_halt_for_leaving();
        true
    }

    /// Orderly handover: the host leaves the running match, a client takes over. The next bundle carries the dropout
    /// command of the own slot (does not apply if the slot already has a result, [`Self::set_results`]) and
    /// `Handover { peers }` with all then active clients by peer; whoever forms it is in `Phase::Ended` afterwards, and
    /// right behind it `Bye` with [`HANDED_OVER`] goes to every client. The clients then see the dropout as planned
    /// (`CoreEvent::HostGone { planned: true }`) and choose only among `peers`.
    ///
    /// The handover lifts a running pause beforehand, like [`Self::end`] (without ticks no bundle would be formed any
    /// more), a pause after a running resync does not apply. In the handover bundle resync and stop do not apply, as
    /// with the end: the host that would have to send the state leaves; the desync counts only at the old host (log),
    /// the new host detects a persisting deviation at its first checksum.
    ///
    /// `true` if accepted. Rejected (`false`, log) outside the match, while a new host is collecting, while `end` is
    /// running or a handover is already requested and if nobody can take over ([`Self::can_hand_over`]); the driver
    /// then takes [`Self::end`]. Conversely `end` rejects after an accepted handover. If the handover is no longer
    /// possible when forming the bundle, it carries `End` instead of `Handover` ([`Self::controls`]).
    pub fn hand_over(&mut self) -> bool {
        let possible = self.can_hand_over();
        if !self.phase.in_match()
            || self.migrating()
            || self.end_wanted
            || self.handover_wanted
            || !possible
        {
            log::warn!(
                "Network (host): handover in phase {:?} has no effect (end requested: {}, handover requested: {}, \
                 reachable and playing client: {possible})",
                self.phase,
                self.end_wanted,
                self.handover_wanted
            );
            return false;
        }
        self.handover_wanted = true;
        log::info!("Network (host): handover requested, it comes with the next bundle");
        self.clear_halt_for_leaving();
        true
    }

    /// Somebody can take over the match: at least one active client is reachable (address from the probe,
    /// [`Self::set_reachable`]) and at least one active client is still playing, i.e. its slot has no result
    /// ([`Self::set_results`]). Both independent: A defeated reachable client may become host for an unreachable client
    /// who is still playing. Only clients count, not the own slot and not the AI; if the match is decided, the end
    /// applies to everyone as today (re/multiplayer M.10.4).
    fn can_hand_over(&self) -> bool {
        let reachable = self.active().any(|r| r.reachable.flatten().is_some());
        let playing = self.active().any(|r| {
            r.slot
                .is_some_and(|s| !self.results.get(usize::from(s)).copied().unwrap_or(false))
        });
        reachable && playing
    }

    /// End or handover need one more bundle: lift a running pause; a pause after the resync does not apply.
    fn clear_halt_for_leaving(&mut self) {
        if let Some(by) = self.pause_after.take() {
            log::info!(
                "Network (host): pause of slot {by} after the resync does not apply, the host leaves the match"
            );
        }
        if self.paused_by().is_some() {
            self.lift_pause();
        }
    }

    /// Local time of the host (from the driver, e.g. per frame): names the network savegame at the stop and at the end.
    /// The time set last applies when forming the bundle; without a call [`LocalTime::FALLBACK`] (1970-01-01 00:00).
    pub fn set_clock(&mut self, local: LocalTime) {
        self.clock = local;
    }

    /// Number of desyncs of the match (several deviating ones of one turn count once).
    pub fn desyncs(&self) -> u32 {
        self.desyncs
    }

    /// Response of the driver to `CoreEvent::SendState`: its state after `e(k)` (call only once it is there). The core
    /// sends the bytes as large content ([`Lockstep::large_outbox`]) to every still connected target, with an ID that
    /// counts up per resync. Without a running resync, or a second time, it has no effect (log).
    pub fn send_state(&mut self, bytes: Vec<u8>) {
        let Some(run) = self.resync.as_mut().filter(|r| !r.sent) else {
            log::warn!("Network (host): state without a waiting resync discarded");
            return;
        };
        run.sent = true;
        let targets = run.waiting.clone();
        self.state_id = self.state_id.wrapping_add(1);
        let id = self.state_id;
        log::info!(
            "Network (host): state {id} ({} bytes) to peers {targets:?}",
            bytes.len()
        );
        for peer in targets {
            self.large.push((peer, id, bytes.clone()));
        }
        self.check_synced();
    }

    /// All machines (host first, then the clients in join order) with names.
    pub fn peers(&self) -> Vec<(Peer, String)> {
        std::iter::once((HOST_PEER, self.own.name.clone()))
            .chain(self.remotes.iter().map(|r| (r.peer, r.name.clone())))
            .collect()
    }

    /// Slot of a machine after the start.
    pub fn slot_of(&self, peer: Peer) -> Option<u8> {
        if peer == HOST_PEER {
            self.own_slot
        } else {
            self.remote(peer)?.slot
        }
    }

    /// Slots whose contribution is missing for the turn the host bundles next, ascending (status line, "missing
    /// slots" like `FUN_004EA5D0` "waiting for clients"). The host and the active clients are counted (not the
    /// dropped-out ones); empty outside the match.
    pub fn missing(&self) -> Vec<u8> {
        if !self.phase.in_match() {
            return Vec::new();
        }
        let parts = self.rounds.get(&self.next_bundle);
        let has = |peer: Peer| parts.is_some_and(|m| m.contains_key(&peer));
        let mut slots: Vec<u8> = std::iter::once((HOST_PEER, self.own_slot))
            .chain(self.active().map(|r| (r.peer, r.slot)))
            .filter(|&(peer, _)| !has(peer))
            .filter_map(|(_, slot)| slot)
            .collect();
        slots.sort_unstable();
        slots
    }

    /// Turns with buffered contributions (for tests).
    #[cfg(test)]
    pub(super) fn buffered_rounds(&self) -> Vec<(u32, Vec<Peer>)> {
        self.rounds
            .iter()
            .map(|(k, parts)| (*k, parts.keys().copied().collect()))
            .collect()
    }

    /// Listen port that the client `peer` reported in its accepted `Hello`.
    pub fn listen_port(&self, peer: Peer) -> Option<u16> {
        self.remote(peer).and_then(|r| r.listen_port)
    }

    /// Result of the reachability probe for `peer`: `Some(addr)` = reachable, `None` = not. Only the first call per
    /// peer counts; a client without a listen port already has its result ("not reachable"). `Go` waits for the result
    /// of every active client, so the report can trigger it.
    pub fn set_reachable(&mut self, peer: Peer, addr: Option<SocketAddr>) {
        let Some(r) = self.remote_mut(peer) else {
            return;
        };
        if r.reachable.is_some() {
            return;
        }
        r.reachable = Some(addr);
        self.check_go();
    }

    /// Successor list: all active clients with a slot, ordered by peer; `addr` is `None` as long as no result is there
    /// or the client was not reachable.
    pub fn successors(&self) -> Vec<Successor> {
        let mut list: Vec<Successor> = self
            .active()
            .filter_map(|r| {
                Some(Successor {
                    peer: r.peer,
                    slot: r.slot?,
                    addr: r.reachable.flatten(),
                })
            })
            .collect();
        list.sort_by_key(|s| s.peer);
        list
    }

    fn remote(&self, peer: Peer) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.peer == peer)
    }

    fn remote_mut(&mut self, peer: Peer) -> Option<&mut Remote> {
        self.remotes.iter_mut().find(|r| r.peer == peer)
    }

    /// Machines whose contribution every turn needs (without dropped-out ones).
    fn active(&self) -> impl Iterator<Item = &Remote> {
        self.remotes.iter().filter(|r| !r.gone)
    }

    /// Peer has dropped out in the match.
    fn is_gone(&self, peer: Peer) -> bool {
        peer != HOST_PEER && self.remote(peer).is_none_or(|r| r.gone)
    }

    /// Slot of the running pause.
    fn paused_by(&self) -> Option<u8> {
        match self.phase {
            Phase::Halted(HaltReason::Pause { by }) => Some(by),
            _ => None,
        }
    }

    fn send_all(&mut self, msg: NetMessage) {
        let peers: Vec<Peer> = self.active().map(|r| r.peer).collect();
        for peer in peers {
            self.outbox.push((peer, msg.clone()));
        }
    }

    /// Checks a `Hello` (`NetSession_AcceptClient` `0x004E8960`); `Err` with the reason.
    fn check_hello(&self, h: &Hello) -> Result<(), String> {
        let own = &self.own;
        if self.phase != Phase::Gathering {
            return Err("The match is already running".into());
        }
        if h.net_version != own.net_version {
            return Err(format!(
                "Different network version: {} instead of {}",
                h.net_version, own.net_version
            ));
        }
        if h.protocol != own.protocol {
            return Err(format!(
                "Different protocol version: {} instead of {}",
                h.protocol, own.protocol
            ));
        }
        if h.format != own.format {
            return Err(format!(
                "Different format version: {} instead of {}",
                h.format, own.format
            ));
        }
        if h.edition != own.edition {
            // Shortened and cleaned: the foreign edition goes into the log and the rejection.
            return Err(format!(
                "Different game edition: \"{}\" instead of \"{}\"",
                clean_text(&h.edition, MAX_EDITION_SHOWN),
                own.edition
            ));
        }
        if h.build != own.build {
            // Both IDs in rejection and log, the foreign one shortened and cleaned.
            return Err(format!(
                "Different program version: {} instead of {}",
                clean_text(&h.build, MAX_BUILD_SHOWN),
                own.build
            ));
        }
        if own.build == UNKNOWN_BUILD {
            log::warn!(
                "Network (host): both build IDs are \"{UNKNOWN_BUILD}\", program version not checked"
            );
        }
        // E-278: a row without a machine (a computer too). Counted over all rows: in a loaded lobby the host does not
        // necessarily stand in row 0, in a new one row 0 always has it.
        if !self.book.state().rows.iter().any(|r| r.peer.is_none()) {
            return Err("The match is full".into());
        }
        Ok(())
    }

    /// A `Hello`: the name is cleaned first ([`clean_name`]), also for log and rejection; it goes into `Seat.name`,
    /// `CoreEvent::Joined` and [`Self::peers`] that way. The row comes from the book: without a slot claim the first
    /// free one ([`LobbyBook::join`], rejected in a loaded lobby), with a slot claim the row of the claimed slot if the
    /// checksum is that of the savegame ([`Self::join_claim`]).
    fn on_hello(&mut self, from: Peer, mut h: Hello) {
        h.name = clean_name(&h.name);
        if self.remote(from).is_some() {
            log::warn!("Network (host): second Hello from peer {from} discarded");
            return;
        }
        match self.check_hello(&h) {
            Err(reason) => {
                log::info!(
                    "Network (host): peer {from} (\"{}\") rejected: {reason}",
                    h.name
                );
                self.outbox.push((from, NetMessage::Reject { reason }));
            }
            Ok(()) => {
                let joined = match &h.seat {
                    None => self.book.join(from, &h.name),
                    Some(claim) => self.join_claim(from, &h.name, claim),
                };
                let row = match joined {
                    Ok(row) => row,
                    Err(reason) => {
                        log::info!(
                            "Network (host): peer {from} (\"{}\") rejected: {reason}",
                            h.name
                        );
                        self.outbox.push((from, NetMessage::Reject { reason }));
                        return;
                    }
                };
                log::info!(
                    "Network (host): peer {from} (\"{}\") accepted, row {row}",
                    h.name
                );
                self.remotes.push(Remote {
                    peer: from,
                    name: h.name.clone(),
                    slot: None,
                    loaded: false,
                    next_turn: 0,
                    gone: false,
                    listen_port: h.listen_port,
                    reachable: if h.listen_port.is_none() {
                        Some(None)
                    } else {
                        None
                    },
                });
                self.outbox.push((from, NetMessage::Welcome { slot: from }));
                self.events.push(CoreEvent::Joined {
                    peer: from,
                    name: h.name,
                });
            }
        }
    }

    /// Join with a slot claim (E-275 deviation): only in a loaded lobby and only with the checksum of the simulation
    /// part from the session; then [`LobbyBook::join_seat`] (slot without a row or occupied -> its reason).
    fn join_claim(&mut self, from: Peer, name: &str, claim: &SeatClaim) -> Result<u8, String> {
        match self.book.state().session.map {
            MapId::Saved { checksum, .. } if checksum == claim.checksum => {
                self.book.join_seat(from, name, claim.slot)
            }
            MapId::Saved { .. } => {
                Err("The savegame on this machine differs from the host's.".into())
            }
            _ => Err("This match is not a loaded savegame.".into()),
        }
    }

    /// `NetSession_OnClientLoaded` `0x004E8470`: a loading error aborts the start for everyone ([`Self::cancel_load`]).
    fn on_loaded(&mut self, from: Peer, error: Option<String>) {
        if self.phase != Phase::Loading {
            log::warn!(
                "Network (host): Loaded from peer {from} in phase {:?} discarded",
                self.phase
            );
            return;
        }
        let Some(r) = self.remote_mut(from) else {
            return;
        };
        match error {
            None => {
                r.loaded = true;
                self.check_go();
            }
            Some(err) => {
                let name = r.name.clone();
                self.cancel_load(name, &err);
            }
        }
    }

    /// The host itself could not load the match (only in `Loading`): like a client's loading error, with the host's
    /// name - `LoadFailed` and `Bye` to every client, `CoreEvent::LoadFailed`, `Phase::Ended`. Otherwise has no effect
    /// (log).
    pub fn load_failed(&mut self, error: String) {
        if self.phase != Phase::Loading {
            log::warn!(
                "Network (host): own loading error in phase {:?} discarded: {error}",
                self.phase
            );
            return;
        }
        let name = self
            .book
            .row_of(HOST_PEER)
            .map(|row| self.book.state().rows[usize::from(row)].name.clone())
            .unwrap_or_else(|| clean_name(&self.own.name));
        self.cancel_load(name, &error);
    }

    /// Loading error of the player `name`: `LoadFailed { name }` and then `Bye` with the reason to every client (the
    /// failing one too), `CoreEvent::LoadFailed`, `Phase::Ended`.
    fn cancel_load(&mut self, name: String, error: &str) {
        let text = format!("\"{name}\" could not load the match: {error}");
        log::warn!("Network (host): {text}");
        self.send_all(NetMessage::LoadFailed { name: name.clone() });
        self.send_all(NetMessage::Bye { reason: text });
        self.phase = Phase::Ended;
        self.events.push(CoreEvent::LoadFailed { name });
    }

    /// `NetSession_SendClientListWhenAllLoaded` `0x004E83B0`: `Go` only when all clients and the host have loaded.
    fn check_go(&mut self) {
        if self.phase != Phase::Loading
            || !self.self_loaded
            || !self.active().all(|r| r.loaded && r.reachable.is_some())
        {
            return;
        }
        let successors = self.successors();
        self.send_all(NetMessage::Go { successors });
        self.phase = Phase::Running;
        self.events.push(CoreEvent::Go);
        log::info!("Network (host): all loaded, Go");
    }

    /// `NetSession_OnClientTurnPacket` `0x004E99E0`: contribution of a client.
    fn on_turn(&mut self, from: Peer, c: TurnContribution) {
        if !self.phase.in_match() {
            log::warn!(
                "Network (host): contribution {} from peer {from} in phase {:?} discarded",
                c.turn,
                self.phase
            );
            return;
        }
        let next_bundle = self.next_bundle;
        let Some(r) = self.remote_mut(from) else {
            return;
        };
        // Protocol error: An honest client sends its contributions without gaps in turn order over the ordered wire and
        // closes turn `k` only after `e(k−1)`, i.e. with bundle `k−1`. A duplicate, a skipped, an already bundled or a
        // too early contribution therefore comes only from a broken or foreign peer: connection closed, the slot loses
        // (instead of just discarding the contribution).
        let error = if c.turn != r.next_turn {
            Some(format!(
                "Protocol error: contribution to turn {} instead of {}",
                c.turn, r.next_turn
            ))
        } else if c.turn > next_bundle {
            Some(format!(
                "Protocol error: contribution to turn {}, turn {next_bundle} is still open",
                c.turn
            ))
        } else {
            None
        };
        if let Some(reason) = error {
            return self.protocol_error(from, reason);
        }
        r.next_turn += 1;
        let c = sanitize(from, r.slot, c);
        self.rounds.entry(c.turn).or_default().insert(from, c);
        self.try_bundle();
    }

    /// Protocol error of a client: message into the log, `Bye` with the reason to it (the driver closes afterwards),
    /// then like any dropout.
    fn protocol_error(&mut self, from: Peer, reason: String) {
        log::error!("Network (host): peer {from}: {reason}, connection ended");
        self.outbox.push((
            from,
            NetMessage::Bye {
                reason: reason.clone(),
            },
        ));
        self.on_gone(from, reason);
    }

    /// Forms all complete bundles in turn order and distributes them. After the bundle with `End` (phase `Ended`) `Bye`
    /// follows to all right away, and no further bundle is formed.
    fn try_bundle(&mut self) {
        loop {
            // The new host bundles only after collecting.
            if !self.phase.in_match() || self.migrating() {
                return;
            }
            let k = self.next_bundle;
            let Some(parts) = self.rounds.get(&k) else {
                return;
            };
            let complete = parts.contains_key(&HOST_PEER)
                && self.active().all(|r| parts.contains_key(&r.peer));
            if !complete {
                return;
            }
            let Some(parts) = self.rounds.remove(&k) else {
                return;
            };
            let bundle = self.build_bundle(k, parts);
            self.send_all(NetMessage::Bundle(bundle.clone()));
            self.bundles.insert(k, bundle);
            self.next_bundle += 1;
            if self.phase == Phase::Ended {
                let reason = if self.handed_over {
                    HANDED_OVER
                } else {
                    HOST_ENDED_REASON
                };
                log::info!("Network (host): match left with bundle {k}, Bye to all: {reason}");
                self.send_all(NetMessage::Bye {
                    reason: reason.into(),
                });
            }
        }
    }

    /// Commands by slot, within a slot in input order (stable; with equal slot first the host, then join order), then
    /// the dropout commands by slot, then the control entries from the wishes. A halt puts the core into `Halted`;
    /// every control entry is reported as `CoreEvent::Control`.
    fn build_bundle(
        &mut self,
        turn: u32,
        mut parts: BTreeMap<Peer, TurnContribution>,
    ) -> TurnBundle {
        // Contributions of dropped-out machines count too if they arrived before the dropout.
        let order = std::iter::once((self.own_slot, HOST_PEER))
            .chain(self.remotes.iter().map(|r| (r.slot, r.peer)));
        let mut ordered: Vec<(u8, Peer, TurnContribution)> = order
            .filter_map(|(slot, peer)| Some((slot.unwrap_or(u8::MAX), peer, parts.remove(&peer)?)))
            .collect();
        ordered.sort_by_key(|(slot, _, _)| *slot);
        let deviants = self.deviants(turn, &ordered);
        let wishes = self.accept_wishes(turn, &ordered);
        let control = self.controls(turn, wishes, deviants);
        let mut commands: Vec<Command> = ordered
            .into_iter()
            .flat_map(|(_, _, c)| c.commands)
            .collect();
        let mut losses = std::mem::take(&mut self.losses);
        // The handover bundle carries the dropout of the own slot (checked like any dropout).
        if self.handover_wanted {
            losses.extend(self.own_slot);
        }
        losses.sort_unstable();
        losses.dedup();
        for slot in losses {
            if self
                .results
                .get(usize::from(slot))
                .copied()
                .unwrap_or(false)
            {
                log::info!("Network (host): slot {slot} already has a result, no dropout command");
                continue;
            }
            let surrendered = commands.iter().any(|c| {
                c.player == slot
                    && matches!(c.kind, CommandKind::SetPlayerResult { slot: s, result: 1 } if s == slot)
            });
            if surrendered {
                log::info!(
                    "Network (host): slot {slot} gives up in the same bundle, no dropout command"
                );
                continue;
            }
            commands.push(loss_command(slot));
        }
        for c in &control {
            self.events.push(CoreEvent::Control {
                turn,
                control: c.clone(),
            });
            match c {
                NetControl::Stop { .. } => self.phase = Phase::Stopped,
                NetControl::End { .. } => {
                    self.phase = Phase::Ended;
                    self.end_wanted = false;
                }
                NetControl::Resync { slots } => self.begin_resync(slots),
                NetControl::Halt { by } if self.resync.is_some() => {
                    // The resync runs first; the pause applies afterwards (`check_synced`).
                    self.pause_after = Some(*by);
                    self.inputs.drop_pauses();
                }
                NetControl::Halt { by } => {
                    self.phase = Phase::Halted(HaltReason::Pause { by: *by });
                    self.inputs.drop_pauses();
                }
                NetControl::Handover { peers } => {
                    log::info!("Network (host): handover to {peers:?} with bundle {turn}");
                    self.phase = Phase::Ended;
                    self.handover_wanted = false;
                    self.handed_over = true;
                }
                _ => {}
            }
        }
        TurnBundle {
            turn,
            commands,
            control,
        }
    }

    /// Checksum comparison (in the original only the client compares, `NetSession_OnHostTurnPacket` `0x004E9A20`,
    /// E-261): slots of the clients whose checksum for turn `turn` deviates from the own, ascending. Contributions of
    /// dropped-out machines do not count. `ordered` is sorted by slot and contains the own contribution.
    fn deviants(&self, turn: u32, ordered: &[(u8, Peer, TurnContribution)]) -> Vec<u8> {
        let Some(own) = ordered
            .iter()
            .find(|(_, peer, _)| *peer == HOST_PEER)
            .map(|(_, _, c)| c.hash)
        else {
            return Vec::new();
        };
        let mut slots: Vec<u8> = ordered
            .iter()
            .filter(|(_, peer, c)| *peer != HOST_PEER && !self.is_gone(*peer) && c.hash != own)
            .map(|(slot, peer, c)| {
                log::warn!(
                    "Network (host): checksum of peer {peer} (slot {slot}) in turn {turn} deviates: {:#018x} instead of {own:#018x}",
                    c.hash
                );
                *slot
            })
            .collect();
        slots.dedup();
        slots
    }

    /// Control entries of the bundle in the order: game speed, save, stop or end, resync, halt. Deviating slots are a
    /// desync: the first and second resync, the third stops with the name from the host's local time instead of
    /// resyncing; a halt in the same bundle does not apply then because the match stands anyway. If the bundle carries
    /// the end ([`Self::end`]), resync, stop and halt do not apply (the desync counts and is in the log; the network
    /// savegames then deviate, which loading detects). If it carries the handover ([`Self::hand_over`]), resync and
    /// stop do not apply either, a halt stays, and `Handover` comes last.
    ///
    /// If nobody can take over any more since the accepted `hand_over` ([`Self::can_hand_over`]: the last reachable
    /// client dropped out, or the last one still playing has a result), the handover first becomes the end for all;
    /// the bundle then carries no dropout of the own slot either (`build_bundle`).
    fn controls(&mut self, turn: u32, wishes: Accepted, deviants: Vec<u8>) -> Vec<NetControl> {
        if self.handover_wanted && !self.can_hand_over() {
            log::warn!(
                "Network (host): handover in turn {turn} no longer possible (no reachable or no playing \
                 client), the match ends for all"
            );
            self.handover_wanted = false;
            self.end_wanted = true;
        }
        let mut control = Vec::new();
        control.extend(wishes.speed.map(NetControl::Speed));
        control.extend(wishes.save.map(|name| NetControl::Save { name }));
        let mut pause = wishes.pause;
        if !deviants.is_empty() {
            self.desyncs += 1;
            if self.end_wanted || self.handover_wanted {
                log::warn!(
                    "Network (host): desync {} in turn {turn} (slots {deviants:?}), no resync: the host leaves the match",
                    self.desyncs
                );
            } else if self.desyncs >= 3 {
                let name = self.clock.checksum_error_name();
                log::warn!(
                    "Network (host): desync {} in turn {turn} (slots {deviants:?}), match stopped as \"{name}\"",
                    self.desyncs
                );
                control.push(NetControl::Stop { name });
                if let Some(by) = pause.take() {
                    log::info!(
                        "Network (host): pause of slot {by} does not apply, the match is stopped"
                    );
                }
            } else {
                log::warn!(
                    "Network (host): desync {} in turn {turn}, resync for slots {deviants:?}",
                    self.desyncs
                );
                control.push(NetControl::Resync { slots: deviants });
            }
        }
        if self.end_wanted {
            let name = self.clock.net_end_name();
            log::info!("Network (host): end of the match in turn {turn}, savegame \"{name}\"");
            control.push(NetControl::End { name });
            if let Some(by) = pause.take() {
                log::info!("Network (host): pause of slot {by} does not apply, the match ends");
            }
        }
        control.extend(pause.map(|by| NetControl::Halt { by }));
        // Last, so that the host is `Ended` after all other entries; a pause from this bundle keeps applying at the
        // clients and after the host migration if the pausing player is among them.
        if self.handover_wanted {
            let peers: Vec<Peer> = self.successors().iter().map(|s| s.peer).collect();
            log::info!("Network (host): handover in turn {turn} to {peers:?}");
            control.push(NetControl::Handover { peers });
        }
        control
    }

    /// Begins the resync: all halt after `e(k)`, the driver is to send the state.
    fn begin_resync(&mut self, slots: &[u8]) {
        let waiting: Vec<Peer> = self
            .active()
            .filter(|r| r.slot.is_some_and(|s| slots.contains(&s)))
            .map(|r| r.peer)
            .collect();
        self.phase = Phase::Halted(HaltReason::Resync);
        self.resync = Some(Resync {
            waiting,
            sent: false,
        });
        self.events.push(CoreEvent::SendState {
            slots: slots.to_vec(),
        });
    }

    /// `Synced` of a target.
    fn on_synced(&mut self, from: Peer) {
        let Some(run) = self.resync.as_mut().filter(|r| r.sent) else {
            log::warn!(
                "Network (host): Synced from peer {from} without a running resync discarded"
            );
            return;
        };
        let before = run.waiting.len();
        run.waiting.retain(|p| *p != from);
        if run.waiting.len() == before {
            log::warn!("Network (host): Synced from peer {from} discarded, no waiting target");
            return;
        }
        log::info!("Network (host): peer {from} has taken over the state");
        self.check_synced();
    }

    /// Once the state is sent and all remaining targets have taken it over, `Resume` goes to all. If the bundle also
    /// had a halt, the match then stands in this pause, without `Resumed`; if the pausing player has dropped out in
    /// the meantime, the host lifts it again right away (second `Resume`, like [`Self::lose`]).
    fn check_synced(&mut self) {
        if !self
            .resync
            .as_ref()
            .is_some_and(|r| r.sent && r.waiting.is_empty())
        {
            return;
        }
        self.resync = None;
        log::info!("Network (host): resync done");
        self.send_all(NetMessage::Resume);
        match self.pause_after.take() {
            None => {
                self.phase = Phase::Running;
                self.events.push(CoreEvent::Resumed);
            }
            Some(by) => {
                self.phase = Phase::Halted(HaltReason::Pause { by });
                if self.slot_gone(by) {
                    self.lift_pause();
                }
            }
        }
    }

    /// The slot belongs to a client that has dropped out in the match.
    fn slot_gone(&self, slot: u8) -> bool {
        self.remotes.iter().any(|r| r.slot == Some(slot) && r.gone)
    }

    /// At most one `Pause` per turn (smallest slot), one `Speed` (only from the own contribution) and one `Save`
    /// (smallest slot with a valid name); duplicates and invalid ones are discarded with a log entry. A `Pause` during
    /// a pause has no effect. `ordered` is sorted by slot; the order of the control entries is set by
    /// [`Self::controls`].
    fn accept_wishes(&self, turn: u32, ordered: &[(u8, Peer, TurnContribution)]) -> Accepted {
        let mut pause: Option<u8> = None;
        let mut speed: Option<u8> = None;
        let mut save: Option<String> = None;
        for (slot, peer, c) in ordered {
            for w in &c.wishes {
                match w {
                    // A dropped-out player could never lift their pause: their contribution counts, their pause does
                    // not.
                    Wish::Pause if self.is_gone(*peer) => {
                        log::info!(
                            "Network (host): pause of slot {slot} discarded, the machine has dropped out"
                        );
                    }
                    Wish::Pause if self.paused_by().is_some() => {
                        log::info!(
                            "Network (host): pause of slot {slot} during a pause has no effect"
                        );
                    }
                    Wish::Pause if pause.is_some() => {
                        log::info!(
                            "Network (host): further pause wish from slot {slot} in turn {turn} discarded"
                        );
                    }
                    Wish::Pause => pause = Some(*slot),
                    // `GameEngine_ChangeSpeedHostOnly` `0x0042F050`: game speed only from the host (E-265, E-294).
                    Wish::Speed(v) if *peer != HOST_PEER => {
                        log::warn!(
                            "Network (host): game speed {v} from slot {slot} discarded, only the host sets the game speed"
                        );
                    }
                    Wish::Speed(v) if *v > MAX_GAME_SPEED => {
                        log::warn!(
                            "Network (host): game speed {v} discarded, at most {MAX_GAME_SPEED}"
                        );
                    }
                    Wish::Speed(v) if speed.is_some() => {
                        log::info!(
                            "Network (host): further game speed wish {v} in turn {turn} discarded"
                        );
                    }
                    Wish::Speed(v) => speed = Some(*v),
                    Wish::Save(name) if !valid_save_name(name) => {
                        log::warn!(
                            "Network (host): saving as {name:?} from slot {slot} discarded, not a valid name"
                        );
                    }
                    Wish::Save(name) if save.is_some() => {
                        log::info!(
                            "Network (host): further save wish {name:?} from slot {slot} in turn {turn} discarded"
                        );
                    }
                    Wish::Save(name) => save = Some(name.clone()),
                }
            }
        }
        Accepted { pause, speed, save }
    }

    /// Lifts the running pause for all (`Resume` to all, `Resumed`).
    fn lift_pause(&mut self) {
        log::info!("Network (host): pause lifted");
        self.send_all(NetMessage::Resume);
        self.phase = Phase::Running;
        self.events.push(CoreEvent::Resumed);
    }

    /// `Resume` of a client: only the pausing player lifts it (O-316, `GameEngine_SetRemotePause` `0x0042F480`).
    fn on_resume(&mut self, from: Peer) {
        if self.resync.is_some() {
            log::warn!(
                "Network (host): Resume from peer {from} during the resync discarded, a resync is not a pause"
            );
            return;
        }
        match self.paused_by() {
            Some(by) if self.slot_of(from) == Some(by) => self.lift_pause(),
            by => log::warn!(
                "Network (host): Resume from peer {from} (slot {:?}) discarded, pause of {by:?}",
                self.slot_of(from)
            ),
        }
    }

    /// Connection to `from` closed, `Bye` or timeout. Before the start the client drops out of the session
    /// (`Gathering`) or aborts the start for all (`Loading`); in the match its slot loses.
    fn on_gone(&mut self, from: Peer, reason: String) {
        let Some(r) = self.remote(from) else {
            return;
        };
        if r.gone {
            log::debug!("Network (host): peer {from} has already dropped out ({reason})");
            return;
        }
        match self.phase {
            Phase::Gathering => {
                log::info!("Network (host): peer {from} left before the start: {reason}");
                self.forget(from);
                // Its row becomes open; the broadcast follows at the end of the call.
                self.book.leave(from);
                self.events.push(CoreEvent::Lost {
                    peer: from,
                    slot: None,
                    reason,
                });
            }
            Phase::Loading => {
                // Like a loading error: without it the match cannot begin as distributed.
                let text = format!("\"{}\" dropped out while loading: {reason}", r.name);
                log::warn!("Network (host): {text}");
                if let Some(r) = self.remote_mut(from) {
                    r.gone = true;
                }
                self.send_all(NetMessage::Bye {
                    reason: text.clone(),
                });
                self.fail(text);
            }
            _ => self.lose(from, reason),
        }
    }

    /// Dropout in the match (`NetSession_OnClientLost` `0x004E9C70`): `Lost` once, dropout command into the next
    /// bundle, no more waiting for the peer; if it was the pausing player, the host lifts the pause.
    fn lose(&mut self, from: Peer, reason: String) {
        let Some(r) = self.remote_mut(from) else {
            return;
        };
        r.gone = true;
        let slot = r.slot;
        log::warn!("Network (host): peer {from} (slot {slot:?}) dropped out: {reason}");
        self.events.push(CoreEvent::Lost {
            peer: from,
            slot,
            reason,
        });
        if let Some(slot) = slot {
            self.losses.push(slot);
            if self.paused_by() == Some(slot) {
                self.lift_pause();
            }
        }
        // A dropped-out target of the resync: do not wait for its `Synced` any more.
        if let Some(run) = self.resync.as_mut() {
            run.waiting.retain(|p| *p != from);
            self.check_synced();
        }
        self.try_bundle();
    }

    fn fail(&mut self, text: String) {
        self.phase = Phase::Ended;
        self.events.push(CoreEvent::Failed(text));
    }
}

impl HostCore {
    /// An event of the connection to `from` (without broadcast, see [`Lockstep::handle`]).
    fn handle_event(&mut self, now_ms: u64, from: Peer, ev: LinkEvent) {
        if from == HOST_PEER {
            log::warn!("Network (host): event from own peer discarded");
            return;
        }
        if self.phase == Phase::Ended {
            return;
        }
        if self.dropped.contains(&from) {
            log::debug!("Network (host): event from kicked peer {from} discarded");
            return;
        }
        if self.migrating() {
            return self.on_gathering_event(from, ev);
        }
        let msg = match ev {
            LinkEvent::Closed(reason) => return self.on_gone(from, reason),
            LinkEvent::Large { id, .. } => {
                log::warn!("Network (host): large content {id} from peer {from} discarded");
                return;
            }
            LinkEvent::Message(msg) => msg,
        };
        if let NetMessage::Hello(h) = msg {
            return self.on_hello(from, h);
        }
        match self.remote(from) {
            None => {
                log::warn!("Network (host): message from unknown peer {from} discarded");
                return;
            }
            Some(r) if r.gone => {
                log::warn!("Network (host): message from dropped-out peer {from} discarded");
                return;
            }
            Some(_) => {}
        }
        match msg {
            NetMessage::Loaded { error } => self.on_loaded(from, error),
            NetMessage::Turn(c) => self.on_turn(from, c),
            NetMessage::Resume => self.on_resume(from),
            NetMessage::Synced => self.on_synced(from),
            NetMessage::Bye { reason } => self.on_gone(from, reason),
            NetMessage::LobbyWish(w) => self.on_lobby_wish(from, w),
            NetMessage::ProbeAck { id } => self.on_probe_ack(now_ms, from, id),
            NetMessage::Rejoin(r) => {
                // A `Rejoin` comes over a new connection that the driver passes to `on_rejoin`; on an already assigned
                // connection it does nothing.
                log::warn!(
                    "Network (host): Rejoin from peer {from} (peer {}) on assigned connection discarded",
                    r.peer
                );
            }
            other => log::warn!("Network (host): unexpected message from peer {from}: {other:?}"),
        }
    }
}

impl Lockstep for HostCore {
    fn handle(&mut self, now_ms: u64, from: Peer, ev: LinkEvent) {
        self.handle_event(now_ms, from, ev);
        self.sync_lobby();
    }

    /// In `Gathering` one `Probe` per [`PROBE_INTERVAL_MS`] to every client without an open probe. At the new host the
    /// collecting ends here after [`super::REJOIN_WINDOW_MS`].
    fn tick(&mut self, now_ms: u64) {
        if self.migrating() {
            return self.migration_tick(now_ms);
        }
        if self.phase != Phase::Gathering || now_ms < self.next_probe_ms {
            return;
        }
        self.next_probe_ms = now_ms.saturating_add(PROBE_INTERVAL_MS);
        let peers: Vec<Peer> = self
            .active()
            .map(|r| r.peer)
            .filter(|p| !self.probes.contains_key(p))
            .collect();
        for peer in peers {
            self.probe_id = self.probe_id.wrapping_add(1);
            let id = self.probe_id;
            self.probes.insert(peer, (id, now_ms));
            self.outbox.push((peer, NetMessage::Probe { id }));
        }
    }

    fn silence(&mut self, _now_ms: u64, peer: Peer, silent_ms: u64) {
        if silent_ms < TIMEOUT_MS || self.phase == Phase::Ended {
            return;
        }
        if self.remote(peer).is_none_or(|r| r.gone) {
            return;
        }
        let reason = timeout_reason();
        // The peer learns about it if it is still listening; the driver closes after the `Bye`.
        self.outbox.push((
            peer,
            NetMessage::Bye {
                reason: reason.clone(),
            },
        ));
        self.on_gone(peer, reason);
        self.sync_lobby();
    }

    fn resume(&mut self) {
        if self.resync.is_some() {
            log::info!("Network (host): lifting has no effect, the resync is running");
            return;
        }
        match self.paused_by() {
            Some(by) if self.own_slot == Some(by) => self.lift_pause(),
            by => log::info!(
                "Network (host): lifting has no effect, pause of {by:?}, own slot {:?}",
                self.own_slot
            ),
        }
    }

    fn submit(&mut self, cmd: Command) {
        if !self.phase.in_match() {
            log::warn!(
                "Network (host): command in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.inputs.commands.push(cmd);
    }

    fn wish(&mut self, w: Wish) {
        if !self.phase.in_match() {
            log::warn!("Network (host): wish in phase {:?} discarded", self.phase);
            return;
        }
        if w == Wish::Pause && (self.paused_by().is_some() || self.pause_after.is_some()) {
            log::info!("Network (host): pause during a pause has no effect");
            return;
        }
        self.inputs.wishes.push(w);
    }

    fn close_round(&mut self, turn: u32, hash: u64) {
        if !self.phase.in_match() {
            log::warn!(
                "Network (host): turn {turn} in phase {:?} not closed",
                self.phase
            );
            return;
        }
        if turn != self.next_close {
            log::error!(
                "Network (host): turn {turn} closed out of order, expected {}",
                self.next_close
            );
            return;
        }
        self.next_close += 1;
        // Own contribution without a network (same delay for all, E-256 deviation).
        let mut own = self.inputs.close(turn, hash);
        limit_commands(HOST_PEER, &mut own.commands);
        self.rounds.entry(turn).or_default().insert(HOST_PEER, own);
        self.try_bundle();
    }

    fn bundle(&self, turn: u32) -> Option<&TurnBundle> {
        self.bundles.get(&turn)
    }

    fn forget_before(&mut self, turn: u32) {
        self.bundles = self.bundles.split_off(&turn);
    }

    /// The host leaves the session (e.g. program exit): `Bye` to every active client - for them an abort by the host,
    /// they write their state themselves - then `Phase::Ended`. The orderly end with the same state for all is
    /// [`HostCore::end`]. In the lobby (`Gathering`) the `Bye` carries [`HOST_LEFT`]; the reason is only in the log.
    fn leave(&mut self, reason: String) {
        if self.phase == Phase::Ended {
            log::debug!("Network (host): leaving after the end has no effect ({reason})");
            return;
        }
        log::info!("Network (host): session left: {reason}");
        let reason = if self.phase == Phase::Gathering {
            HOST_LEFT.to_string()
        } else {
            reason
        };
        // The new host leaves the match while collecting: the returnees learn about it too.
        let rejoined = if self.migrating() {
            self.rejoined()
        } else {
            Vec::new()
        };
        for peer in rejoined {
            self.outbox.push((
                peer,
                NetMessage::Bye {
                    reason: reason.clone(),
                },
            ));
        }
        self.send_all(NetMessage::Bye { reason });
        self.phase = Phase::Ended;
    }

    fn outbox(&mut self) -> Vec<(Peer, NetMessage)> {
        std::mem::take(&mut self.outbox)
    }

    fn large_outbox(&mut self) -> Vec<(Peer, u32, Vec<u8>)> {
        std::mem::take(&mut self.large)
    }

    fn events(&mut self) -> Vec<CoreEvent> {
        std::mem::take(&mut self.events)
    }

    fn phase(&self) -> &Phase {
        &self.phase
    }

    fn config(&self) -> TurnConfig {
        self.cfg
    }
}

/// Reason in the `Bye` of the host after the bundle with `End`.
const HOST_ENDED_REASON: &str = "The host has ended the match.";

/// Accepted wishes of a turn.
struct Accepted {
    pause: Option<u8>,
    speed: Option<u8>,
    save: Option<String>,
}

/// Commands of a client contribution with a foreign sender and the mirror commands `ScriptSelection`/`ScriptButton`
/// (they mirror the host's UI on the network) are dropped, likewise game speed wishes (only from the host) and save
/// wishes without a valid name. After that the limits [`MAX_COMMANDS_PER_TURN`], [`MAX_TURN_COMMAND_BYTES`]
/// ([`limit_commands`]) and [`MAX_WISHES_PER_TURN`] apply. Log, no abort.
fn sanitize(peer: Peer, slot: Option<u8>, mut c: TurnContribution) -> TurnContribution {
    c.commands.retain(|cmd| {
        if Some(cmd.player) != slot {
            log::warn!(
                "Network (host): command with sender {} from peer {peer} (slot {slot:?}) discarded",
                cmd.player
            );
            return false;
        }
        if matches!(
            cmd.kind,
            CommandKind::ScriptSelection { .. } | CommandKind::ScriptButton { .. }
        ) {
            log::warn!(
                "Network (host): mirror command from peer {peer} discarded: {:?}",
                cmd.kind
            );
            return false;
        }
        true
    });
    c.wishes.retain(|w| match w {
        Wish::Speed(v) => {
            log::warn!(
                "Network (host): game speed {v} from peer {peer} discarded, only the host sets the game speed"
            );
            false
        }
        Wish::Save(name) if !valid_save_name(name) => {
            log::warn!(
                "Network (host): saving as {name:?} from peer {peer} discarded, not a valid name"
            );
            false
        }
        _ => true,
    });
    limit_commands(peer, &mut c.commands);
    if c.wishes.len() > MAX_WISHES_PER_TURN {
        log::warn!(
            "Network (host): {} of {} wishes from peer {peer} in turn {} discarded, at most {MAX_WISHES_PER_TURN}",
            c.wishes.len() - MAX_WISHES_PER_TURN,
            c.wishes.len(),
            c.turn
        );
        c.wishes.truncate(MAX_WISHES_PER_TURN);
    }
    c
}

/// Limits the commands of a contribution (the host's own too): the longest prefix with at most
/// [`MAX_COMMANDS_PER_TURN`] commands and at most [`MAX_TURN_COMMAND_BYTES`] encoded bytes stays; the rest is dropped
/// with a log line. The bundle thus stays under [`MAX_FRAME`], whatever the contributions carry.
fn limit_commands(peer: Peer, commands: &mut Vec<Command>) {
    let mut bytes = 0usize;
    let mut keep = 0usize;
    for cmd in commands.iter().take(MAX_COMMANDS_PER_TURN) {
        let size = encoded_len(cmd);
        if bytes + size > MAX_TURN_COMMAND_BYTES {
            break;
        }
        bytes += size;
        keep += 1;
    }
    if keep < commands.len() {
        log::warn!(
            "Network (host): {} of {} commands from peer {peer} discarded (at most {MAX_COMMANDS_PER_TURN} commands and              {MAX_TURN_COMMAND_BYTES} bytes per turn)",
            commands.len() - keep,
            commands.len()
        );
        commands.truncate(keep);
    }
}

/// Encoded length of a command (postcard, as in the bundle), without encoding it.
fn encoded_len(cmd: &Command) -> usize {
    // The size count of postcard does not fail (no buffer); if in doubt the command counts as too large.
    postcard::serialize_with_flavor(cmd, postcard::ser_flavors::Size::default())
        .unwrap_or(usize::MAX / 2)
}
