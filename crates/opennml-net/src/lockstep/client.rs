//! Turn core of the client.
//!
//! The client signs on with `Hello` (right when created, in the outbox), loads after `Start`, reports `Loaded`, begins
//! with `Go` and then sends its contribution to the host each turn; the bundles come from the host. A halt in the
//! bundle puts it into `Halted`; it may lift only its own pause (`Resume` to the host, which passes it on to all).
//!
//! Resync: A `Resync` in the bundle puts it into `Halted(Resync)`. If its slot is a target, it accepts exactly one
//! large content (`CoreEvent::ResyncState`); its driver takes over the state after `e(k)` and calls
//! [`ClientCore::synced`]. The `Resume` of the host ends the resync. A `Stop` puts it into `Phase::Stopped` for good.
//!
//! End and abort: An `End` in the bundle ends the session (`Phase::Ended`); the driver still computes up to `e(k)`, a
//! `Bye` or `Closed` after that has no effect. The core reports the dropout of the host in the match (connection
//! closed, `Bye`, [`TIMEOUT_MS`] of silence) as `CoreEvent::Failed` with [`HOST_LOST`] (reason in the log); before
//! `Go` likewise on a closed connection and silence, only a `Bye` carries its cleaned reason in the text there. The
//! driver then writes its network savegame. A bundle in the match with a turn other than the next is a protocol error
//! and acts like a dropout of the host (`Bye` to it).
//!
//! Lobby: After the `Welcome` the client in `Gathering` accepts the lobby state of the host (`Lobby`,
//! `CoreEvent::LobbyChanged` on a new `revision`), chat lines (`CoreEvent::Chat`) and latency probes (`Probe` ->
//! `ProbeAck`) and sends wishes for its own row ([`ClientCore::lobby_wish`]). A `Bye` with [`KICKED`]/[`HOST_LEFT`] in
//! `Gathering` it reports as `CoreEvent::Kicked`/`CoreEvent::HostLeft`, a `Reject` before the `Welcome` as
//! `CoreEvent::Rejected`; both without `Failed`, then `Phase::Ended`.
//!
//! Host migration: The client keeps every own contribution until the bundle of its turn is there, and counts the
//! desyncs from the bundles. If the host drops out after `Go` and the successor list has a candidate ([`Succession`]),
//! it reports `CoreEvent::HostGone` instead of `Failed` and is "in the migration": the phase stays, it discards events
//! and silence of the old connection, closed contributions no longer go into the outbox. The driver fetches the
//! `Rejoin` for a candidate with [`ClientCore::rejoin`] (or with `into_parts` the state for
//! [`super::HostCore::from_client`] if it becomes host itself) and hands over the `Migrated` with
//! [`ClientCore::rehome`]; after that the held-back contributions go to the new host, and the connection [`HOST_PEER`]
//! is the new host's. With [`ClientCore::abandon`] it gives up (`Failed` with [`HOST_LOST`], today's abort).

use super::{
    BundleControls, ClientParts, CoreEvent, HOST_LOST, HOST_PEER, HaltReason, Inputs, Lockstep,
    Peer, Phase, RoleControl, Succession, SuccessionStep, TIMEOUT_MS, timeout_reason,
};
use crate::link::LinkEvent;
use crate::lobby::{LobbyState, LobbyWish, MAX_ROWS};
use crate::msg::{
    HOST_LEFT, Hello, KICKED, MAX_GAME_SPEED, Migrated, NetMessage, Rejoin, StartInfo, Successor,
    TurnBundle, TurnContribution, Wish, clean_chat, clean_name, clean_reason,
};
use crate::turn::TurnConfig;
use crate::wire::encode;
use opennml_sim::command::Command;
use std::collections::BTreeMap;

/// Turn core of a client.
pub struct ClientCore {
    hello: Hello,
    /// Own connection number from `Welcome`.
    peer: Option<Peer>,
    phase: Phase,
    cfg: TurnConfig,
    /// Own slot from `Start`.
    slot: Option<u8>,
    /// `Loaded` sent.
    load_reported: bool,
    /// Loaded without error.
    ready: bool,
    inputs: Inputs,
    next_close: u32,
    next_bundle: u32,
    bundles: BTreeMap<u32, TurnBundle>,
    outbox: Vec<(Peer, NetMessage)>,
    large: Vec<(Peer, u32, Vec<u8>)>,
    events: Vec<CoreEvent>,
    /// Running resync.
    resync: Option<Resync>,
    /// Pause from the same bundle as the running resync: applies after its end.
    pause_after: Option<u8>,
    /// Last lobby state from the host.
    lobby: Option<LobbyState>,
    /// After the first lobby state, wish for `Ready(true)` itself (command-line path).
    auto_ready: bool,
    /// The automatic Ready wish has gone out.
    auto_ready_sent: bool,
    /// Successor list from `Go`; empty before `Go` and for an unusable list.
    successors: Vec<Successor>,
    /// Own contributions whose bundle is still missing (turn >= `next_bundle`), by turn.
    sent: Vec<TurnContribution>,
    /// Desyncs from the bundles: one per `Resync` and `Stop`.
    desyncs: u32,
    /// Peers from a `Handover` in the bundle: the dropout afterwards is planned.
    handover: Option<Vec<Peer>>,
    /// After `HostGone` until `rehome` or `abandon`: events and silence of the old connection do not count, closed
    /// contributions are held back.
    migrating: bool,
    /// `next_close` at the last `rejoin`: contributions from this turn on go out only after `rehome`.
    rejoined_at: Option<u32>,
}

/// Resync from the client's point of view.
#[derive(Debug)]
struct Resync {
    /// The own slot is a target: the host sends its state.
    target: bool,
    /// State received (`CoreEvent::ResyncState` reported).
    received: bool,
    /// `Synced` sent.
    synced: bool,
}

impl ClientCore {
    /// New client; `Hello` is already in the outbox.
    pub fn new(hello: Hello) -> Self {
        let outbox = vec![(HOST_PEER, NetMessage::Hello(hello.clone()))];
        Self {
            hello,
            peer: None,
            phase: Phase::Gathering,
            cfg: TurnConfig::default(),
            slot: None,
            load_reported: false,
            ready: false,
            inputs: Inputs::default(),
            next_close: 0,
            next_bundle: 0,
            bundles: BTreeMap::new(),
            outbox,
            large: Vec::new(),
            events: Vec::new(),
            resync: None,
            pause_after: None,
            lobby: None,
            auto_ready: false,
            auto_ready_sent: false,
            successors: Vec::new(),
            sent: Vec::new(),
            desyncs: 0,
            handover: None,
            migrating: false,
            rejoined_at: None,
        }
    }

    /// Switch for the command-line path: after the first lobby state the client itself sends
    /// `LobbyWish::Ready(true)`.
    pub fn set_auto_ready(&mut self, on: bool) {
        self.auto_ready = on;
    }

    /// Last lobby state from the host (`None` before the first).
    pub fn lobby(&self) -> Option<&LobbyState> {
        self.lobby.as_ref()
    }

    /// Own lobby row: the row with the own peer in the last state.
    pub fn own_row(&self) -> Option<u8> {
        let peer = self.peer?;
        self.lobby
            .as_ref()?
            .rows
            .iter()
            .position(|r| r.peer == Some(peer))
            .map(|i| i as u8)
    }

    /// Wish for the own lobby row or a chat line; only in `Gathering` after the `Welcome`, otherwise has no effect
    /// (log). Pause, game speed and saving of the match stay [`Lockstep::wish`].
    pub fn lobby_wish(&mut self, w: LobbyWish) {
        if self.phase != Phase::Gathering || self.peer.is_none() {
            log::warn!(
                "Network (client): lobby wish {w:?} in phase {:?} (connection {:?}) discarded",
                self.phase,
                self.peer
            );
            return;
        }
        self.outbox.push((HOST_PEER, NetMessage::LobbyWish(w)));
    }

    /// Lobby state from the host (only in `Gathering` after the `Welcome`); an inconsistent state (row count != player
    /// count, more than eight rows, slots neither empty nor one per row) is dropped with a log entry. `LobbyChanged`
    /// only on a new `revision`. The client cleans names from the state itself: row names with [`clean_name`] (an
    /// empty row stays empty), session and map name with `clean_session_name`; map path and seed stay unchanged.
    fn on_lobby(&mut self, mut state: LobbyState) {
        if self.phase != Phase::Gathering || self.peer.is_none() {
            log::warn!(
                "Network (client): lobby state in phase {:?} discarded",
                self.phase
            );
            return;
        }
        if state.rows.len() != usize::from(state.session.players)
            || state.rows.len() > MAX_ROWS
            || !(state.seats.is_empty() || state.seats.len() == state.rows.len())
        {
            log::warn!(
                "Network (client): inconsistent lobby state discarded ({} rows, {} players, {} slots)",
                state.rows.len(),
                state.session.players,
                state.seats.len()
            );
            return;
        }
        if self
            .lobby
            .as_ref()
            .is_some_and(|l| l.revision == state.revision)
        {
            return;
        }
        for row in &mut state.rows {
            if !row.name.is_empty() {
                row.name = clean_name(&row.name);
            }
        }
        state.session = state.session.cleaned();
        self.lobby = Some(state);
        self.events.push(CoreEvent::LobbyChanged);
        if self.auto_ready && !self.auto_ready_sent {
            self.auto_ready_sent = true;
            log::info!("Network (client): reporting myself ready (command-line path)");
            self.outbox
                .push((HOST_PEER, NetMessage::LobbyWish(LobbyWish::Ready(true))));
        }
    }

    /// Chat line from the host (only in `Gathering`); the name comes from the last lobby state.
    fn on_chat(&mut self, row: u8, text: &str) {
        if self.phase != Phase::Gathering || self.peer.is_none() {
            log::warn!("Network (client): chat in phase {:?} discarded", self.phase);
            return;
        }
        let name = self
            .lobby
            .as_ref()
            .and_then(|l| l.rows.get(usize::from(row)))
            .map(|r| r.name.clone())
            .unwrap_or_default();
        self.events.push(CoreEvent::Chat {
            row,
            name,
            text: clean_chat(text),
        });
    }

    /// Latency probe of the host: answer immediately (only in `Gathering`).
    fn on_probe(&mut self, id: u32) {
        if self.phase != Phase::Gathering {
            log::debug!(
                "Network (client): probe {id} in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.outbox.push((HOST_PEER, NetMessage::ProbeAck { id }));
    }

    /// `Bye` from the host: in `Gathering` kick and departure of the host are events of their own, otherwise like any
    /// dropout of the host.
    fn on_bye(&mut self, reason: &str) {
        if self.phase == Phase::Gathering {
            let event = match reason {
                KICKED => Some(CoreEvent::Kicked),
                HOST_LEFT => Some(CoreEvent::HostLeft),
                _ => None,
            };
            if let Some(event) = event {
                log::info!("Network (client): {reason}");
                return self.end_with(event);
            }
        }
        self.host_gone(reason);
    }

    /// `LoadFailed` from the host (only in `Loading`): a machine could not load the match, the start is cancelled.
    /// `CoreEvent::LoadFailed` with the cleaned name instead of `Failed`, then `Phase::Ended` (the `Bye` behind it has
    /// no effect any more).
    fn on_load_failed(&mut self, name: &str) {
        if self.phase != Phase::Loading {
            log::warn!(
                "Network (client): LoadFailed in phase {:?} discarded",
                self.phase
            );
            return;
        }
        let name = clean_name(name);
        log::warn!("Network (client): \"{name}\" could not load the match, start cancelled");
        self.end_with(CoreEvent::LoadFailed { name });
    }

    /// `Reject` from the host: before the `Welcome` it is the rejection of the sign-on (`Rejected`), afterwards a
    /// failure; the reason cleaned ([`clean_reason`]).
    fn on_reject(&mut self, reason: String) {
        let reason = clean_reason(&reason);
        if self.peer.is_none() {
            log::info!("Network (client): sign-on rejected: {reason}");
            self.end_with(CoreEvent::Rejected { reason });
        } else {
            self.fail(format!("Rejected by the host: {reason}"));
        }
    }

    /// Session over with an event of its own instead of `Failed`.
    fn end_with(&mut self, event: CoreEvent) {
        self.phase = Phase::Ended;
        self.events.push(event);
    }

    /// Loading finished (`NetSession_ReportLevelLoaded` `0x004E8A60`): `None` = done, otherwise the reason for the
    /// error. Only once, only in `Loading`.
    pub fn loaded(&mut self, error: Option<String>) {
        if self.phase != Phase::Loading || self.load_reported {
            log::warn!(
                "Network (client): loading in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.load_reported = true;
        self.ready = error.is_none();
        self.outbox.push((HOST_PEER, NetMessage::Loaded { error }));
    }

    /// The driver has taken over the state from `CoreEvent::ResyncState`: sends `Synced` to the host. Only once per
    /// resync and only after reception; otherwise has no effect (log).
    pub fn synced(&mut self) {
        match self.resync.as_mut() {
            Some(run) if run.received && !run.synced => {
                run.synced = true;
                self.outbox.push((HOST_PEER, NetMessage::Synced));
            }
            run => {
                log::warn!("Network (client): synced without a received state discarded ({run:?})")
            }
        }
    }

    /// Own connection number after `Welcome`.
    pub fn peer(&self) -> Option<Peer> {
        self.peer
    }

    /// Own slot after `Start`.
    pub fn slot(&self) -> Option<u8> {
        self.slot
    }

    fn on_welcome(&mut self, peer: Peer) {
        if self.phase != Phase::Gathering || self.peer.is_some() {
            log::warn!(
                "Network (client): Welcome in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.peer = Some(peer);
        self.events.push(CoreEvent::Joined {
            peer,
            name: self.hello.name.clone(),
        });
    }

    /// Start from the host; the turn length comes from the network and is checked (no panic on nonsense).
    fn on_start(&mut self, info: Box<StartInfo>) {
        if self.phase != Phase::Gathering || self.peer.is_none() {
            log::warn!(
                "Network (client): Start in phase {:?} discarded",
                self.phase
            );
            return;
        }
        // Grid and game speed come from the host; a game speed above `MAX_GAME_SPEED` lies outside the slider 0 ... 20.
        let cfg = TurnConfig::new(info.spacing, info.lead);
        let cfg = cfg.and_then(|cfg| {
            if info.game_speed > MAX_GAME_SPEED {
                Err(format!(
                    "Game speed {} above {MAX_GAME_SPEED}",
                    info.game_speed
                ))
            } else {
                Ok(cfg)
            }
        });
        match cfg {
            Err(e) => {
                let text = format!("Invalid start from the host: {e}");
                self.outbox.push((
                    HOST_PEER,
                    NetMessage::Bye {
                        reason: text.clone(),
                    },
                ));
                self.fail(text);
            }
            Ok(cfg) => {
                self.cfg = cfg;
                self.slot = Some(info.your_slot);
                self.phase = Phase::Loading;
                self.events.push(CoreEvent::Start(info));
            }
        }
    }

    /// Successor list from the host.
    pub fn successors(&self) -> &[Successor] {
        &self.successors
    }

    fn on_go(&mut self, successors: Vec<Successor>) {
        if self.phase != Phase::Loading || !self.ready {
            log::warn!(
                "Network (client): Go in phase {:?} without finished loading discarded",
                self.phase
            );
            return;
        }
        self.successors = checked_successors(successors);
        self.phase = Phase::Running;
        self.events.push(CoreEvent::Go);
    }

    fn on_bundle(&mut self, b: TurnBundle) {
        if !self.phase.in_match() {
            log::warn!(
                "Network (client): bundle {} in phase {:?} discarded",
                b.turn,
                self.phase
            );
            return;
        }
        if b.turn != self.next_bundle {
            // Protocol error: An honest host sends the bundles without gaps in turn order.
            let reason = format!(
                "Protocol error: bundle {} instead of {}",
                b.turn, self.next_bundle
            );
            log::error!("Network (client): {reason} from the host, connection ended");
            self.outbox.push((
                HOST_PEER,
                NetMessage::Bye {
                    reason: reason.clone(),
                },
            ));
            return self.link_lost(&reason);
        }
        self.apply_bundle(b);
    }

    /// Accepts the next bundle `b` (turn = `next_bundle`, checked by the caller): report and apply control entries,
    /// count desyncs, forget finished own contributions.
    fn apply_bundle(&mut self, b: TurnBundle) {
        self.next_bundle += 1;
        let next = self.next_bundle;
        self.sent.retain(|c| c.turn >= next);
        // Control entries take effect after `e(turn)`; the driver halts there.
        for c in &b.control {
            let role = BundleControls {
                phase: &mut self.phase,
                desyncs: &mut self.desyncs,
                pause_after: &mut self.pause_after,
                inputs: &mut self.inputs,
                events: &mut self.events,
            }
            .apply(b.turn, c, "client");
            match role {
                Some(RoleControl::Resync(slots)) => {
                    let target = self.slot.is_some_and(|s| slots.contains(&s));
                    self.resync = Some(Resync {
                        target,
                        received: false,
                        synced: false,
                    });
                }
                Some(RoleControl::Handover(peers)) => {
                    log::info!("Network (client): the host hands over to {peers:?}");
                    self.handover = Some(peers.to_vec());
                }
                None => {}
            }
        }
        self.bundles.insert(b.turn, b);
    }

    /// `Resume` from the host (`GameEngine_SetRemotePause` `0x0042F480`): the halt is lifted. During a resync it ends
    /// the resync; if the same bundle contained a halt, the client then stands in this pause (without `Resumed`) until
    /// the next `Resume` lifts it.
    fn on_resume(&mut self) {
        if !matches!(self.phase, Phase::Halted(_)) {
            log::warn!(
                "Network (client): Resume in phase {:?} discarded",
                self.phase
            );
            return;
        }
        if self.resync.take().is_some()
            && let Some(by) = self.pause_after.take()
        {
            self.phase = Phase::Halted(HaltReason::Pause { by });
            return;
        }
        self.phase = Phase::Running;
        self.events.push(CoreEvent::Resumed);
    }

    /// Large content from the host: the state for the resync, only as a target and only once per resync.
    fn on_large(&mut self, id: u32, bytes: Vec<u8>) {
        match self.resync.as_mut() {
            Some(run) if run.target && !run.received => {
                log::info!(
                    "Network (client): state {id} ({} bytes) received for the resync",
                    bytes.len()
                );
                run.received = true;
                self.events.push(CoreEvent::ResyncState { bytes });
            }
            run => log::warn!(
                "Network (client): large content {id} without an expected resync discarded ({run:?})"
            ),
        }
    }

    fn fail(&mut self, text: String) {
        log::warn!("Network (client): {text}");
        self.phase = Phase::Ended;
        self.events.push(CoreEvent::Failed(text));
    }

    /// The host ended with `Bye`: in the match (also after a stop) [`HOST_LOST`] with the reason in the log, before
    /// `Go` "The host has ended the connection: ..." with the cleaned reason ([`clean_reason`]): the text comes from a
    /// foreign machine and goes into the `MSGBOX`.
    fn host_gone(&mut self, reason: &str) {
        if self.phase.in_match() || self.phase == Phase::Stopped {
            self.link_lost(reason);
        } else {
            log::warn!("Network (client): host has ended: {reason}");
            self.fail(format!(
                "The host has ended the connection: {}",
                clean_reason(reason)
            ));
        }
    }

    /// Connection to the host closed or silent (or host gone in the match): in every phase [`HOST_LOST`]; the raw
    /// reason (e.g. the system text "... (os error 10054)" in the language of the operating system) only into the log.
    ///
    /// After `Go` with a candidate in the successor list `CoreEvent::HostGone` instead: The phase stays, queued
    /// contributions leave the outbox (they travel in the `Rejoin`).
    fn link_lost(&mut self, reason: &str) {
        if self.phase.in_match() && self.has_candidate() {
            log::warn!(
                "Network (client): host dropped out: {reason}; host migration (planned: {})",
                self.handover.is_some()
            );
            self.migrating = true;
            self.outbox
                .retain(|(_, m)| !matches!(m, NetMessage::Turn(_)));
            self.events.push(CoreEvent::HostGone {
                reason: reason.to_string(),
                planned: self.handover.is_some(),
            });
            return;
        }
        log::warn!("Network (client): host dropped out: {reason}");
        self.fail(HOST_LOST.into());
    }

    /// The successor list has a candidate (the own entry too), after a `Handover` among its peers.
    fn has_candidate(&self) -> bool {
        let Some(own) = self.peer else {
            return false;
        };
        Succession::new(&self.successors, own, self.handover.as_deref()).next()
            != SuccessionStep::GiveUp
    }

    /// Desyncs of the match from the bundles; after [`ClientCore::rehome`] the number of the new host.
    pub fn desyncs(&self) -> u32 {
        self.desyncs
    }

    /// Peers from the `Handover` of the host, if one came: the candidates of the planned handover.
    pub fn handover(&self) -> Option<&[u8]> {
        self.handover.as_deref()
    }

    /// `Rejoin` for a candidate, only after `CoreEvent::HostGone`: own peer and slot, `next_close`, `next_bundle`, the
    /// kept bundles (at most the last `2 · kept_bundles`), all open own contributions and the desync count. Remembers
    /// `next_close`: contributions that the client closes afterwards it holds back until [`ClientCore::rehome`]. Call
    /// again for every further candidate.
    ///
    /// `Err` (text, also in the log): no host migration, or the `Rejoin` would be encoded longer than
    /// [`crate::wire::MAX_FRAME`] (honest bundles are small, chunking does not happen). The driver then gives up
    /// ([`ClientCore::abandon`]).
    pub fn rejoin(&mut self) -> Result<Rejoin, String> {
        if !self.migrating {
            return Err("No Rejoin: the host has not dropped out".into());
        }
        let (Some(peer), Some(slot)) = (self.peer, self.slot) else {
            return Err("No Rejoin: connection number or slot missing".into());
        };
        let keep = 2 * self.cfg.kept_bundles() as usize;
        let skip = self.bundles.len().saturating_sub(keep);
        let rejoin = Rejoin {
            peer,
            slot,
            next_close: self.next_close,
            next_bundle: self.next_bundle,
            bundles: self.bundles.values().skip(skip).cloned().collect(),
            open: self.sent.clone(),
            desyncs: self.desyncs,
        };
        if let Err(e) = encode(&NetMessage::Rejoin(Box::new(rejoin.clone()))) {
            let text = format!("Rejoin not possible: {e}");
            log::error!("Network (client): {text}");
            return Err(text);
        }
        self.rejoined_at = Some(self.next_close);
        Ok(rejoin)
    }

    /// Takes over the `Migrated` of the new host after [`ClientCore::rejoin`]: The bundles must begin without a gap at
    /// the next one (at most `2 · kept_bundles`); their control entries take effect like in an ordinary bundle. A
    /// running resync lapses, the phase follows `paused_by` (`None` = running), successor list (checked as with `Go`)
    /// and desync count come from the new host. The held-back contributions go into the outbox (to [`HOST_PEER`], now
    /// the new host). Reports `CoreEvent::Rehomed`, after a halt with a switch to `Running` also `CoreEvent::Resumed`.
    ///
    /// `Err` (text, also in the log) without any change: no host migration, no `Rejoin`, bundles out of order or too
    /// many. The driver then tries the next candidate or gives up.
    pub fn rehome(&mut self, m: Migrated) -> Result<(), String> {
        let Some(mark) = self.rejoined_at.filter(|_| self.migrating) else {
            return Err("Migrated without a running host migration discarded".into());
        };
        let limit = 2 * self.cfg.kept_bundles() as usize;
        if m.bundles.len() > limit {
            let text = format!(
                "Migrated with {} bundles (at most {limit})",
                m.bundles.len()
            );
            log::warn!("Network (client): {text}");
            return Err(text);
        }
        let in_order = m
            .bundles
            .iter()
            .zip(self.next_bundle..)
            .all(|(b, k)| b.turn == k);
        if !in_order {
            let turns: Vec<u32> = m.bundles.iter().map(|b| b.turn).collect();
            let text = format!(
                "Migrated with bundles {turns:?} instead of without a gap from {}",
                self.next_bundle
            );
            log::warn!("Network (client): {text}");
            return Err(text);
        }
        let was_halted = matches!(self.phase, Phase::Halted(_));
        for b in m.bundles {
            self.apply_bundle(b);
        }
        self.resync = None;
        self.pause_after = None;
        if self.phase.in_match() {
            self.phase = match m.paused_by {
                Some(by) => {
                    self.inputs.drop_pauses();
                    Phase::Halted(HaltReason::Pause { by })
                }
                None => Phase::Running,
            };
        }
        self.successors = checked_successors(m.successors);
        self.desyncs = m.desyncs;
        self.handover = None;
        self.migrating = false;
        self.rejoined_at = None;
        log::info!(
            "Network (client): signed on at the new host (peer {}), next bundle {}",
            m.host,
            self.next_bundle
        );
        for c in self.sent.iter().filter(|c| c.turn >= mark) {
            self.outbox.push((HOST_PEER, NetMessage::Turn(c.clone())));
        }
        self.events.push(CoreEvent::Rehomed);
        if was_halted && self.phase == Phase::Running {
            self.events.push(CoreEvent::Resumed);
        }
        Ok(())
    }

    /// Gives up the host migration (or ends the session if none is running): `Failed` with [`HOST_LOST`], today's
    /// abort. Has no effect in `Phase::Ended`.
    pub fn abandon(&mut self) {
        if self.phase == Phase::Ended {
            return;
        }
        self.migrating = false;
        self.rejoined_at = None;
        self.fail(HOST_LOST.into());
    }

    /// The client is in the host migration (after `HostGone`, before `rehome` or `abandon`); [`HostCore::from_client`]
    /// checks that before it takes the client apart.
    ///
    /// [`HostCore::from_client`]: super::HostCore::from_client
    pub(super) fn in_migration(&self) -> bool {
        self.migrating
    }

    /// Takes the client apart for the new host ([`HostCore::from_client`]).
    ///
    /// [`HostCore::from_client`]: super::HostCore::from_client
    pub(super) fn into_parts(self) -> ClientParts {
        ClientParts {
            hello: self.hello,
            peer: self.peer,
            slot: self.slot,
            cfg: self.cfg,
            phase: self.phase,
            inputs: self.inputs,
            next_close: self.next_close,
            next_bundle: self.next_bundle,
            bundles: self.bundles,
            sent: self.sent,
            lobby: self.lobby,
            pause_after: self.pause_after,
            resync: self.resync.is_some(),
            desyncs: self.desyncs,
            successors: self.successors,
            handover: self.handover,
        }
    }
}

/// Checks a successor list from outside (`Go`, `Migrated`): more than `MAX_ROWS` entries or a duplicate peer exists in
/// no match; the list then counts as empty (no candidate), the match runs anyway.
fn checked_successors(successors: Vec<Successor>) -> Vec<Successor> {
    let mut peers: Vec<Peer> = successors.iter().map(|s| s.peer).collect();
    peers.sort_unstable();
    peers.dedup();
    if successors.len() > MAX_ROWS || peers.len() != successors.len() {
        log::warn!(
            "Network (client): successor list discarded ({} entries, {} distinct peers)",
            successors.len(),
            peers.len()
        );
        return Vec::new();
    }
    successors
}

impl Lockstep for ClientCore {
    fn handle(&mut self, _now_ms: u64, from: Peer, ev: LinkEvent) {
        if from != HOST_PEER {
            log::warn!(
                "Network (client): event from peer {from} instead of from the host discarded"
            );
            return;
        }
        if self.phase == Phase::Ended {
            return;
        }
        if self.migrating {
            // The old connection to the host no longer counts; `rehome` takes over the new host.
            log::debug!(
                "Network (client): event of the old connection during the host migration discarded"
            );
            return;
        }
        let msg = match ev {
            LinkEvent::Closed(reason) => return self.link_lost(&reason),
            LinkEvent::Large { id, bytes } => return self.on_large(id, bytes),
            LinkEvent::Message(msg) => msg,
        };
        match msg {
            NetMessage::Welcome { slot } => self.on_welcome(slot),
            NetMessage::Reject { reason } => self.on_reject(reason),
            NetMessage::Start(info) => self.on_start(info),
            NetMessage::Go { successors } => self.on_go(successors),
            NetMessage::Rejoin(r) => {
                log::info!(
                    "Network (client): Rejoin from peer {} on the host connection discarded",
                    r.peer
                );
            }
            NetMessage::Migrated(m) => {
                // Without a host migration it has no effect; during one the driver passes it to `rehome`.
                log::warn!(
                    "Network (client): Migrated from peer {} without a host migration discarded",
                    m.host
                );
            }
            NetMessage::Bundle(b) => self.on_bundle(b),
            NetMessage::Resume => self.on_resume(),
            NetMessage::Bye { reason } => self.on_bye(&reason),
            NetMessage::LoadFailed { name } => self.on_load_failed(&name),
            NetMessage::Lobby(state) => self.on_lobby(*state),
            NetMessage::Chat { row, text } => self.on_chat(row, &text),
            NetMessage::Probe { id } => self.on_probe(id),
            other => log::warn!("Network (client): unexpected message: {other:?}"),
        }
    }

    fn tick(&mut self, _now_ms: u64) {}

    fn silence(&mut self, _now_ms: u64, peer: Peer, silent_ms: u64) {
        if peer != HOST_PEER
            || silent_ms < TIMEOUT_MS
            || self.phase == Phase::Ended
            || self.migrating
        {
            return;
        }
        // Like a closed connection; plus a `Bye` in case the host still hears (the driver closes afterwards).
        let reason = timeout_reason();
        self.outbox.push((
            HOST_PEER,
            NetMessage::Bye {
                reason: reason.clone(),
            },
        ));
        self.link_lost(&reason);
    }

    /// `GameEngine_TogglePauseNet` `0x0042D780`: only the own pause (O-316).
    fn resume(&mut self) {
        match self.phase {
            Phase::Halted(HaltReason::Pause { by }) if self.slot == Some(by) => {
                self.outbox.push((HOST_PEER, NetMessage::Resume));
            }
            _ => log::info!(
                "Network (client): lifting has no effect in phase {:?}, own slot {:?}",
                self.phase,
                self.slot
            ),
        }
    }

    fn submit(&mut self, cmd: Command) {
        if !self.phase.in_match() {
            log::warn!(
                "Network (client): command in phase {:?} discarded",
                self.phase
            );
            return;
        }
        self.inputs.commands.push(cmd);
    }

    fn wish(&mut self, w: Wish) {
        if !self.phase.in_match() {
            log::warn!("Network (client): wish in phase {:?} discarded", self.phase);
            return;
        }
        if w == Wish::Pause
            && (matches!(self.phase, Phase::Halted(HaltReason::Pause { .. }))
                || self.pause_after.is_some())
        {
            log::info!("Network (client): pause during a pause has no effect");
            return;
        }
        self.inputs.wishes.push(w);
    }

    fn close_round(&mut self, turn: u32, hash: u64) {
        if !self.phase.in_match() {
            log::warn!(
                "Network (client): turn {turn} in phase {:?} not closed",
                self.phase
            );
            return;
        }
        if turn != self.next_close {
            log::error!(
                "Network (client): turn {turn} closed out of order, expected {}",
                self.next_close
            );
            return;
        }
        self.next_close += 1;
        let c = self.inputs.close(turn, hash);
        // Keep until the bundle of the turn is there; during the host migration only keep.
        self.sent.push(c.clone());
        if !self.migrating {
            self.outbox.push((HOST_PEER, NetMessage::Turn(c)));
        }
    }

    fn bundle(&self, turn: u32) -> Option<&TurnBundle> {
        self.bundles.get(&turn)
    }

    fn forget_before(&mut self, turn: u32) {
        self.bundles = self.bundles.split_off(&turn);
    }

    /// The client leaves the session (main menu, program exit, window closed): `Bye` to the host, which treats it like
    /// a dropout, then `Phase::Ended` without `Failed`.
    fn leave(&mut self, reason: String) {
        if self.phase == Phase::Ended {
            log::debug!("Network (client): leaving after the end has no effect ({reason})");
            return;
        }
        log::info!("Network (client): session left: {reason}");
        self.outbox.push((HOST_PEER, NetMessage::Bye { reason }));
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
