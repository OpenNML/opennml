//! New host from a client (host migration).
//!
//! If the host drops out and the client's own entry in the successor list is next
//! ([`crate::SuccessionStep::BecomeHost`]), [`HostCore::from_client`] turns the `ClientCore` into a host: peer 0, the
//! other clients keep their numbers, name and slot per client come from `StartInfo::seats`, grid, phase, bundles, open
//! own contributions and inputs from the client, the lobby state frozen ([`LobbyBook::frozen`]).
//!
//! Collecting: Until every expected client has sent its `Rejoin` ([`HostCore::on_rejoin`]; expected are the entries
//! whose slot has no result yet, after a handover only those under its `peers`) or [`REJOIN_WINDOW_MS`] have passed
//! since the switch (`tick`), the core forms no bundle and sends nothing. A slot with a result is accepted while the
//! core waits. A `Rejoin` that does not fit the list or comes too late is rejected (the driver sends `Reject`).
//!
//! Completing (deviates from E-348): `K` = highest bundle from the own stock and all `Rejoin`. The core takes its own
//! missing bundles up to `K` from the `Rejoin` (control entries as on reception), each client gets the bundles it lacks
//! in `Migrated`. If a bundle `≤ K` is missing everywhere, it aborts (`CoreEvent::MigrationFailed`). Open contributions
//! `> K` become ordinary contributions, those `≤ K` are already in the bundle and are dropped.
//!
//! Defeat (deviates from E-347, E-349): Bundle `K + 1` carries `SetPlayerResult` for the slot of the old host (from
//! `seats`, never via a network index; not after an orderly handover) and for every expected client that did not
//! return - via `losses` in `build_bundle`, which skips slots with a result. If the result of a slot is already in
//! a bundle `≤ K`, no second command follows.
//!
//! Halt: A running resync is dropped (a pause from the same bundle then applies), the new host lifts the pause of the
//! old host or of a client that did not return; `Migrated::paused_by` carries the halt from its point of view. The
//! desync count is the highest of its own count and all `Rejoin`; the checksum applies from the first turn without a
//! grace period (deviates from E-268, part "grace period").
//!
//! In the original, `FUN_004E9D20` takes over the new host from the DirectPlay message `0x004EACB7`; collecting and
//! completing are omitted there entirely (E-268, E-348).

use super::{
    HostCore, MAX_COMMANDS_PER_TURN, MAX_TURN_COMMAND_BYTES, Remote, encoded_len, sanitize,
};
use crate::link::LinkEvent;
use crate::lobby::{LobbyBook, LobbyRules, LobbyState, SessionInfo};
use crate::lockstep::{
    BundleControls, ClientCore, ClientParts, CoreEvent, HOST_PEER, HaltReason, Lockstep, Peer,
    Phase, REJOIN_WINDOW_MS, RoleControl,
};
use crate::msg::{
    MIGRATION_FAILED, Migrated, NetMessage, Rejoin, Seat, Successor, TurnBundle, seat_of,
};
use crate::wire::encode;
use nml_sim::command::CommandKind;
use std::collections::BTreeMap;

/// State of the host migration at the new host.
#[derive(Debug)]
pub(super) struct Migration {
    /// Own peer as a client (as `host` in `Migrated`).
    own_peer: Peer,
    /// End of collecting: time of the switch plus [`REJOIN_WINDOW_MS`].
    deadline_ms: u64,
    /// Entries of the successor list without the new host, by peer.
    entries: Vec<Successor>,
    /// Expected peers: entries whose slot has no result yet.
    expected: Vec<Peer>,
    /// Accepted `Rejoin` per peer (only while collecting).
    rejoins: BTreeMap<Peer, Rejoin>,
    /// Returnees whose connection closed again while collecting: they count as "did not return".
    departed: Vec<Peer>,
    /// Peers that received their `Migrated` after collecting.
    came: Vec<Peer>,
    /// Slot of the old host from `seats` (E-349).
    old_slot: Option<u8>,
    /// Orderly handover (`only`): the old host no longer gets a dropout command.
    planned: bool,
    stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Gathering,
    Done,
    Failed,
}

impl HostCore {
    /// Turns a client in the host migration into the new host. `seats` is the occupancy from `StartInfo` (slot of the
    /// old host = seat with peer 0, names of the clients), `results` the slots that already have a result in the own
    /// state (from the simulation): they are not expected. `only` are the peers of a `Handover` (orderly handover):
    /// only they are then expected (other entries are accepted while the core waits), and the handover bundle already
    /// carried the dropout of the old host (no second command). The old host still appears in `MigrationDone::lost`;
    /// the driver reports its dropout from there and not once more from the handover bundle. `now_ms` starts the
    /// deadline [`REJOIN_WINDOW_MS`]. If nobody is expected, collecting is done at once.
    ///
    /// `Err` with a reason and the unchanged client if it is not in the host migration (no `HostGone`, no running
    /// match, peer or slot missing): the driver then falls back to the abort.
    pub fn from_client(
        client: Box<ClientCore>,
        seats: &[Seat; 8],
        results: [bool; 8],
        rules: Box<dyn LobbyRules>,
        now_ms: u64,
        only: Option<&[u8]>,
    ) -> Result<HostCore, (Box<ClientCore>, String)> {
        let usable = client.in_migration() && client.phase().in_match();
        let (Some(own_peer), Some(own_slot)) = (client.peer(), client.slot()) else {
            return Err((
                client,
                "No host migration: connection number or slot is missing.".into(),
            ));
        };
        if !usable || own_peer == HOST_PEER || usize::from(own_slot) >= seats.len() {
            let text = format!(
                "No host migration: the client is not in the host migration (phase {:?}).",
                client.phase()
            );
            log::warn!("Network (new host): {text}");
            return Err((client, text));
        }
        let parts: ClientParts = (*client).into_parts();
        log::info!(
            "Network (new host): taking over as peer {:?} in slot {:?}, next bundle {}, next turn {}",
            parts.peer,
            parts.slot,
            parts.next_bundle,
            parts.next_close
        );
        if parts.resync {
            log::info!("Network (new host): the running resync is dropped with the host migration");
        }
        if parts.handover.is_some() != only.is_some() {
            log::warn!(
                "Network (new host): handover in the client {:?}, from the driver {only:?}",
                parts.handover
            );
        }
        let cfg = parts.cfg;
        let state = parts.lobby.unwrap_or_else(|| {
            log::warn!("Network (new host): no lobby state from the old host, using an empty one");
            LobbyState {
                revision: 0,
                session: SessionInfo::plain("", "", 0, cfg),
                rows: Vec::new(),
                max_rtt_ms: None,
                seats: Vec::new(),
            }
        });
        let old_slot = seat_of(seats, HOST_PEER).filter(|&s| s != own_slot);
        if old_slot.is_none() {
            log::warn!("Network (new host): slot of the old host unknown, no defeat for it");
        }
        let mut entries: Vec<Successor> = Vec::new();
        for s in parts.successors {
            if s.peer == own_peer {
                continue;
            }
            let fits = s.peer != HOST_PEER
                && usize::from(s.slot) < seats.len()
                && s.slot != own_slot
                && Some(s.slot) != old_slot
                && !entries.iter().any(|e| e.slot == s.slot);
            if !fits {
                log::warn!("Network (new host): entry {s:?} of the successor list discarded");
                continue;
            }
            entries.push(s);
        }
        entries.sort_by_key(|e| e.peer);
        // After a handover only the peers from `Handover`: whoever left earlier is still in the list from `Go` but does
        // not return.
        let expected: Vec<Peer> = entries
            .iter()
            .filter(|e| !results[usize::from(e.slot)])
            .filter(|e| only.is_none_or(|peers| peers.contains(&e.peer)))
            .map(|e| e.peer)
            .collect();
        // Locked until collecting ends: dropped-out clients get nothing, and no bundle is formed.
        let remotes = entries
            .iter()
            .map(|e| Remote {
                peer: e.peer,
                name: seats[usize::from(e.slot)].name.clone(),
                slot: Some(e.slot),
                loaded: true,
                next_turn: 0,
                gone: true,
                listen_port: e.addr.map(|a| a.port()),
                reachable: Some(e.addr),
            })
            .collect();
        let mut rounds: BTreeMap<u32, BTreeMap<Peer, _>> = BTreeMap::new();
        for c in parts.sent {
            rounds.entry(c.turn).or_default().insert(HOST_PEER, c);
        }
        let mut host = HostCore::new(parts.hello, LobbyBook::frozen(state), rules);
        host.cfg = cfg;
        host.phase = parts.phase;
        host.remotes = remotes;
        host.own_slot = Some(own_slot);
        host.self_loaded = true;
        host.inputs = parts.inputs;
        host.next_close = parts.next_close;
        host.next_bundle = parts.next_bundle;
        host.rounds = rounds;
        host.bundles = parts.bundles;
        host.results = results;
        host.desyncs = parts.desyncs;
        host.pause_after = parts.pause_after;
        log::info!(
            "Network (new host): collecting Rejoin, expected {expected:?}, list {:?}",
            entries.iter().map(|e| e.peer).collect::<Vec<_>>()
        );
        host.migration = Some(Migration {
            own_peer,
            deadline_ms: now_ms.saturating_add(REJOIN_WINDOW_MS),
            entries,
            expected,
            rejoins: BTreeMap::new(),
            departed: Vec::new(),
            came: Vec::new(),
            old_slot,
            planned: only.is_some(),
            stage: Stage::Gathering,
        });
        host.check_gathered();
        Ok(host)
    }

    /// Collecting of the `Rejoin` is still running: no bundle, no message to the clients.
    pub fn migrating(&self) -> bool {
        self.phase != Phase::Ended
            && self
                .migration
                .as_ref()
                .is_some_and(|m| m.stage == Stage::Gathering)
    }

    /// Expected clients by peer while collecting is running (status line of the driver), otherwise empty.
    pub fn expected(&self) -> Vec<Peer> {
        match &self.migration {
            Some(m) if self.migrating() => m.expected.clone(),
            _ => Vec::new(),
        }
    }

    /// Returned clients by peer: while collecting every accepted one, afterwards those that received their
    /// `Migrated`.
    pub fn rejoined(&self) -> Vec<Peer> {
        match &self.migration {
            None => Vec::new(),
            Some(m) if m.stage == Stage::Gathering => m.rejoins.keys().copied().collect(),
            Some(m) => m.came.clone(),
        }
    }

    /// `Rejoin` on a new connection. `Ok(peer)`: accepted, the driver assigns the connection to this peer before it
    /// sends the outbox (after collecting it holds its `Migrated`, after an abort a `Reject`). `Err(reason)`: the
    /// driver sends `Reject { reason }` on this connection and closes it. After collecting (also if the deadline has
    /// passed before `tick` came) and after an abort the reason is [`MIGRATION_FAILED`], so that the latecomer gives
    /// up instead of looking for a host of its own.
    ///
    /// Rejected is only what does not fit the list (unknown or duplicate peer, wrong slot) or endangers the core (more
    /// than `2 · kept_bundles` bundles or open contributions, bundles out of order, more than
    /// [`MAX_COMMANDS_PER_TURN`] commands or [`MAX_TURN_COMMAND_BYTES`] bytes per contribution; combined test
    /// `foreign_migration_inputs_never_panic`).
    pub fn on_rejoin(&mut self, now_ms: u64, r: Rejoin) -> Result<Peer, String> {
        let Some(m) = self.migration.as_ref() else {
            return Err("No host migration at this host.".into());
        };
        if m.stage == Stage::Gathering && now_ms >= m.deadline_ms {
            self.finish_migration();
        }
        if !self.migrating() {
            log::warn!(
                "Network (new host): Rejoin from peer {} rejected after collecting",
                r.peer
            );
            return Err(MIGRATION_FAILED.into());
        }
        if let Err(reason) = self.check_rejoin(&r) {
            log::warn!("Network (new host): {reason}");
            return Err(reason);
        }
        let peer = r.peer;
        log::info!(
            "Network (new host): Rejoin from peer {peer} accepted, next bundle {}, next turn {}",
            r.next_bundle,
            r.next_close
        );
        if let Some(m) = self.migration.as_mut() {
            m.departed.retain(|p| *p != peer);
            m.rejoins.insert(peer, r);
        }
        self.check_gathered();
        Ok(peer)
    }

    /// Checks a `Rejoin` against the list and the limits of the core (returns the reason).
    fn check_rejoin(&self, r: &Rejoin) -> Result<(), String> {
        let Some(m) = self.migration.as_ref() else {
            return Err("No host migration at this host.".into());
        };
        let peer = r.peer;
        let Some(entry) = m.entries.iter().find(|e| e.peer == peer) else {
            return Err(format!(
                "Rejoin from peer {peer} rejected: not in the successor list"
            ));
        };
        if entry.slot != r.slot {
            return Err(format!(
                "Rejoin from peer {peer} rejected: slot {} instead of {}",
                r.slot, entry.slot
            ));
        }
        if m.rejoins.contains_key(&peer) {
            return Err(format!(
                "Rejoin from peer {peer} rejected: already accepted"
            ));
        }
        let limit = 2 * self.cfg.kept_bundles() as usize;
        if r.bundles.len() > limit || r.open.len() > limit {
            return Err(format!(
                "Rejoin from peer {peer} rejected: {} bundles and {} contributions (at most {limit})",
                r.bundles.len(),
                r.open.len()
            ));
        }
        let in_order = r
            .bundles
            .windows(2)
            .all(|w| w[0].turn.checked_add(1) == Some(w[1].turn));
        let fits = r
            .bundles
            .last()
            .is_none_or(|b| b.turn.checked_add(1) == Some(r.next_bundle));
        if !in_order || !fits {
            let turns: Vec<u32> = r.bundles.iter().map(|b| b.turn).collect();
            return Err(format!(
                "Rejoin from peer {peer} rejected: bundles {turns:?} do not fit bundle {}",
                r.next_bundle
            ));
        }
        for c in &r.open {
            let bytes: usize = c.commands.iter().map(encoded_len).sum();
            if c.commands.len() > MAX_COMMANDS_PER_TURN || bytes > MAX_TURN_COMMAND_BYTES {
                return Err(format!(
                    "Rejoin from peer {peer} rejected: contribution {} with {} commands, {bytes} bytes",
                    c.turn,
                    c.commands.len()
                ));
            }
        }
        Ok(())
    }

    /// Deadline of collecting (from `tick`).
    pub(super) fn migration_tick(&mut self, now_ms: u64) {
        if self
            .migration
            .as_ref()
            .is_some_and(|m| now_ms >= m.deadline_ms)
        {
            log::info!("Network (new host): collecting deadline passed");
            self.finish_migration();
        }
    }

    /// Event of an assigned connection while collecting: if it closes (`Closed`, `Bye`), the client counts as not
    /// returned; anything else has no effect (until `Migrated` the clients send only their `Rejoin`).
    pub(super) fn on_gathering_event(&mut self, from: Peer, ev: LinkEvent) {
        let reason = match ev {
            LinkEvent::Closed(reason) | LinkEvent::Message(NetMessage::Bye { reason }) => reason,
            other => {
                log::debug!(
                    "Network (new host): event from peer {from} discarded while collecting: {other:?}"
                );
                return;
            }
        };
        let Some(m) = self.migration.as_mut() else {
            return;
        };
        if m.rejoins.remove(&from).is_some() {
            log::warn!("Network (new host): peer {from} left while collecting: {reason}");
            m.departed.push(from);
            self.check_gathered();
        }
    }

    /// Once all expected clients are here (or have left again), collecting is done.
    fn check_gathered(&mut self) {
        let complete = self.migrating()
            && self.migration.as_ref().is_some_and(|m| {
                m.expected
                    .iter()
                    .all(|p| m.rejoins.contains_key(p) || m.departed.contains(p))
            });
        if complete {
            self.finish_migration();
        }
    }

    /// End of collecting: complete, align halt and desyncs, `Migrated` to every returnee, defeats into the dropout
    /// list, `MigrationDone`, then bundle again from `K + 1`.
    fn finish_migration(&mut self) {
        if !self.migrating() {
            return;
        }
        let Some(mut m) = self.migration.take() else {
            return;
        };
        let kept = 2 * u64::from(self.cfg.kept_bundles());
        // Stock of all: own bundles first, then those from the `Rejoin`.
        let mut pool = self.bundles.clone();
        for r in m.rejoins.values() {
            for b in &r.bundles {
                pool.entry(b.turn).or_insert_with(|| b.clone());
            }
        }
        // `n` = K + 1.
        let n = m
            .rejoins
            .values()
            .map(|r| r.next_bundle)
            .fold(self.next_bundle, u32::max);
        // Whoever would need more than `2 · kept_bundles` bundles cannot take them over with `rehome`.
        let mut come: Vec<Peer> = Vec::new();
        let mut refused: Vec<Peer> = Vec::new();
        for (&peer, r) in &m.rejoins {
            if u64::from(n - r.next_bundle) > kept {
                log::warn!(
                    "Network (new host): peer {peer} is at bundle {}, K + 1 = {n}: too far behind",
                    r.next_bundle
                );
                refused.push(peer);
            } else {
                come.push(peer);
            }
        }
        // If a needed bundle is missing everywhere, it cannot continue. The search ends at the first missing one, so
        // after at most as many steps as there are bundles in the stock.
        let missing = std::iter::once(self.next_bundle)
            .chain(come.iter().map(|p| m.rejoins[p].next_bundle))
            .flat_map(|from| from..n)
            .find(|t| !pool.contains_key(t));
        if let Some(t) = missing {
            self.migration = Some(m);
            return self
                .abort_migration(format!("bundle {t} is missing everywhere (K = {})", n - 1));
        }
        // Own stock up to K (control entries as on reception).
        for t in self.next_bundle..n {
            if let Some(b) = pool.get(&t) {
                self.absorb(b.clone());
            }
        }
        self.next_bundle = n;
        self.desyncs = m
            .rejoins
            .values()
            .map(|r| r.desyncs)
            .fold(self.desyncs, u32::max);
        // `Migrated` per returnee; if one were longer than a frame, the client counts as not returned.
        let (out, halt, departed) = loop {
            for r in &mut self.remotes {
                r.gone = !come.contains(&r.peer);
            }
            let departed: Vec<u8> = m
                .entries
                .iter()
                .filter(|e| !come.contains(&e.peer))
                .map(|e| e.slot)
                .chain(m.old_slot)
                .collect();
            let halt = self.halt_after(&departed);
            let successors = self.successors();
            let mut out = Vec::new();
            let mut too_big = None;
            for &peer in &come {
                let r = &m.rejoins[&peer];
                let msg = NetMessage::Migrated(Box::new(Migrated {
                    host: m.own_peer,
                    bundles: (r.next_bundle..n)
                        .filter_map(|t| pool.get(&t).cloned())
                        .collect(),
                    paused_by: halt,
                    successors: successors.clone(),
                    desyncs: self.desyncs,
                }));
                if let Err(e) = encode(&msg) {
                    too_big = Some((peer, e));
                    break;
                }
                out.push((peer, msg));
            }
            match too_big {
                Some((peer, e)) => {
                    log::error!(
                        "Network (new host): Migrated to peer {peer} not possible ({e}), it counts as not returned"
                    );
                    come.retain(|p| *p != peer);
                    refused.push(peer);
                }
                None => break (out, halt, departed),
            }
        };
        // Halt from the point of view of the new host; a resync is dropped.
        let was_halted = matches!(self.phase, Phase::Halted(_));
        if self.phase.in_match() {
            self.phase = match halt {
                Some(by) => {
                    self.inputs.drop_pauses();
                    Phase::Halted(HaltReason::Pause { by })
                }
                None => Phase::Running,
            };
        }
        self.resync = None;
        self.pause_after = None;
        if was_halted && self.phase == Phase::Running {
            self.events.push(CoreEvent::Resumed);
        }
        // Open contributions: `≤ K` are in the bundle, `> K` are ordinary contributions.
        self.rounds = self.rounds.split_off(&n);
        for &peer in &come {
            let r = &m.rejoins[&peer];
            let slot = self.remote(peer).and_then(|x| x.slot);
            if let Some(remote) = self.remote_mut(peer) {
                remote.next_turn = r.next_close;
            }
            for c in r.open.iter().filter(|c| c.turn >= n) {
                let c = sanitize(peer, slot, c.clone());
                self.rounds.entry(c.turn).or_default().insert(peer, c);
            }
        }
        // Defeats in bundle `K + 1`.
        let losers: Vec<u8> = m
            .entries
            .iter()
            .filter(|e| m.expected.contains(&e.peer) && !come.contains(&e.peer))
            .map(|e| e.slot)
            .collect();
        let mut lost: Vec<u8> = losers.iter().copied().chain(m.old_slot).collect();
        lost.sort_unstable();
        lost.dedup();
        let old = m.old_slot.filter(|_| !m.planned);
        for slot in losers.into_iter().chain(old) {
            if self.result_in_bundles(slot) {
                log::info!(
                    "Network (new host): slot {slot} already has its result in a bundle, no dropout command"
                );
                continue;
            }
            self.losses.push(slot);
        }
        log::info!(
            "Network (new host): collecting done, K = {}, back {come:?}, turned down {refused:?}, lost {lost:?}, \
             halt {halt:?}, desyncs {}, gone {departed:?}",
            i64::from(n) - 1,
            self.desyncs
        );
        self.outbox.extend(out);
        for peer in refused {
            self.outbox.push((
                peer,
                NetMessage::Reject {
                    reason: MIGRATION_FAILED.into(),
                },
            ));
        }
        m.rejoins.clear();
        m.came = come;
        m.stage = Stage::Done;
        self.migration = Some(m);
        self.events.push(CoreEvent::MigrationDone { lost });
        self.try_bundle();
    }

    /// Aborts the host migration: `Reject` with [`MIGRATION_FAILED`] to every returnee, `MigrationFailed`, phase
    /// `Ended`.
    fn abort_migration(&mut self, why: String) {
        log::error!("Network (new host): host migration aborted: {why}");
        if let Some(m) = self.migration.as_mut() {
            for &peer in m.rejoins.keys() {
                self.outbox.push((
                    peer,
                    NetMessage::Reject {
                        reason: MIGRATION_FAILED.into(),
                    },
                ));
            }
            m.rejoins.clear();
            m.stage = Stage::Failed;
        }
        self.phase = Phase::Ended;
        self.events.push(CoreEvent::MigrationFailed { reason: why });
    }

    /// Halt after the host migration: the pausing player if still present; a running resync is dropped in favor of a
    /// pause from the same bundle.
    fn halt_after(&self, departed: &[u8]) -> Option<u8> {
        let by = match self.phase {
            Phase::Halted(HaltReason::Pause { by }) => Some(by),
            Phase::Halted(HaltReason::Resync) => self.pause_after,
            _ => None,
        };
        by.filter(|by| !departed.contains(by))
    }

    /// Takes a completed bundle into the own stock: reports and applies control entries like `ClientCore` on reception
    /// (shared in [`BundleControls`]). A resync only sets the phase (it is dropped when collecting ends), a `Handover`
    /// appears only in the log.
    fn absorb(&mut self, b: TurnBundle) {
        for c in &b.control {
            let role = BundleControls {
                phase: &mut self.phase,
                desyncs: &mut self.desyncs,
                pause_after: &mut self.pause_after,
                inputs: &mut self.inputs,
                events: &mut self.events,
            }
            .apply(b.turn, c, "new host");
            match role {
                Some(RoleControl::Handover(peers)) => {
                    log::info!("Network (new host): handover to {peers:?} in the completed bundle");
                }
                Some(RoleControl::Resync(_)) | None => {}
            }
        }
        self.bundles.insert(b.turn, b);
    }

    /// A bundle in the stock already carries a result for `slot`.
    fn result_in_bundles(&self, slot: u8) -> bool {
        self.bundles.values().any(|b| {
            b.commands.iter().any(
                |c| matches!(c.kind, CommandKind::SetPlayerResult { slot: s, .. } if s == slot),
            )
        })
    }
}
