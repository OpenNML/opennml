//! Role change in the turn driver.
//!
//! If the host drops out after `Go` and the successor list has a candidate, the client core reports
//! `CoreEvent::HostGone`. The driver then closes the old connection and chooses with [`Succession`]:
//! - **Dialing** ([`MigrationRun::Dialing`]): with the machine's transport to the candidate (deadline
//!   [`SUCCESSOR_CONNECT_MS`]; a rejected connection costs the whole deadline). Once it stands, exactly the
//!   `Rejoin` ([`ClientCore::rejoin`]) goes out; what the core addresses to the host until then (`Resume`, `Synced`)
//!   is dropped because there is no connection [`HOST_PEER`] any more (the candidate would otherwise close it).
//! - **Waiting** ([`MigrationRun::AwaitMigrated`]): `Migrated` -> [`ClientCore::rehome`], the connection becomes
//!   [`HOST_PEER`], events after it go straight to the core. `Reject` -> [`Succession::rejected`] (with
//!   `MIGRATION_REFUSED` or `MIGRATION_FAILED` the client gives up), connection closed or deadline expired -> next
//!   entry. The deadline is [`AWAIT_MIGRATED_MS`] = 2 * `W` + [`SUCCESSOR_CONNECT_MS`] instead of `W`: a candidate that
//!   still hears the old host holds the `Rejoin` for up to `W` from its arrival and, if it becomes host in that time,
//!   gathers for up to `W` more from its switch; with a shorter deadline the caller's would expire before its `Reject`
//!   or `Migrated`, and it might itself become host (second host).
//! - **Becoming host** ([`SuccessionStep::BecomeHost`]): the client core turns into [`HostCore::from_client`] (on
//!   `Err` it comes back and gives up). The new host gathers; accepted connections stay in the waiting list of the
//!   listener until their `Rejoin` arrives, then [`HostCore::on_rejoin`] -> enter as a peer (before the outbox is
//!   sent, which may already hold `Migrated`) or `Reject`.
//! - **Giving up** ([`SuccessionStep::GiveUp`]): [`ClientCore::abandon`] -> today's abort savegame.
//!
//! A `Rejoin` on the listener while this machine is still a client (still connected to the old host or itself still in
//! the election) is held by the driver for up to `W`; if it becomes host in that time, the `Rejoin` goes to
//! `on_rejoin`, otherwise `Reject` with `MIGRATION_REFUSED` follows. After gathering (`MigrationDone`) the new host
//! reports the lost slots in [`NetFrame::lost`] (the old host only here, also after a handover), updates
//! `StartInfo::seats` and closes its listener; every client keeps listening (second migration). If gathering fails
//! (`MigrationFailed`), the new host writes the abort savegame like a client.
//!
//! In the original DirectPlay chooses the new host (`DPNSESSION_MIGRATE_HOST` `0x004EB2A7`, message `0x004EACB7`), the
//! game adopts it in `FUN_004E9D20` without gathering and supplementing (E-268, E-348 deviated).

use super::*;
use opennml_net::{AcceptAll, Hello, MIGRATION_REFUSED, NET_VERSION, Succession, SuccessionStep};

/// Deadline for the `Migrated` after the own `Rejoin`: a candidate that still hears the old host holds the `Rejoin`
/// for up to `W` from its arrival; if it becomes host in that time, it then gathers for up to `W` more from its switch
/// before the `Migrated` goes out. The deadline of dialing comes on top.
/// With a shorter deadline the caller would take the next entry before the `Migrated`, with four machines or more
/// possibly itself (second host).
pub(super) const AWAIT_MIGRATED_MS: u64 = 2 * REJOIN_WINDOW_MS + SUCCESSOR_CONNECT_MS;

/// Reason in [`NetFrame::lost`] for an expected client that did not come back when gathering.
const NOT_RETURNED: &str = "Did not return after the host change";

/// State of the election at the client.
pub(super) enum MigrationRun {
    /// Connection setup to the candidate `peer`.
    Dialing {
        succession: Succession,
        peer: Peer,
        connecting: Box<dyn Connecting>,
    },
    /// `Rejoin` sent to `peer`, `Migrated` expected until `deadline_ms`.
    AwaitMigrated {
        succession: Succession,
        peer: Peer,
        link: Box<dyn Link>,
        deadline_ms: u64,
    },
}

impl NetDriver {
    /// Debug aid `net_debug mute`: every connection stays silent (`on`), not even keepalives go out; the remote ends
    /// see a dropout after [`opennml_net::TIMEOUT_MS`] of silence. Also applies to connections added later (clients of the
    /// host, listener, migration). `false` lifts it.
    pub fn set_muted(&mut self, on: bool) {
        log::warn!(
            "Network: check run turns all connections {}",
            if on { "silent" } else { "loud again" }
        );
        self.muted = on;
        for (_, link) in &mut self.links {
            link.mute(on);
        }
        for a in &mut self.accepted {
            a.link.mute(on);
        }
        if let Some(MigrationRun::AwaitMigrated { link, .. }) = &mut self.migration {
            link.mute(on);
        }
    }

    /// Orderly handover by the host ([`HostCore::hand_over`]): the next bundle carries the own dropout and `Handover`,
    /// after that the match ends for this machine with [`NetFinish::HandedOver`] (without savegame), and a client takes
    /// over. `false` at a client and if the core refuses (then [`Self::end`]); [`Self::ending`] includes the running
    /// handover.
    pub fn hand_over(&mut self) -> bool {
        let Some(h) = self.host_core_mut() else {
            log::warn!("Network: only the host can hand over the match");
            return false;
        };
        let done = h.hand_over();
        self.handing_over |= done;
        // A lifting of the pause (`Resume`) goes out right away.
        self.flush();
        done
    }

    /// The host leaves the running match (`END_*` of the end screen, end of the program, window closed): hand over
    /// ([`Self::hand_over`]), without a reachable active client the end for all ([`Self::end`]). `true` if one of them
    /// is accepted; at a client and outside the match `false` without effect.
    pub fn host_leaves(&mut self) -> bool {
        self.host_in_match() && (self.hand_over() || self.end())
    }

    /// End of the program (`opennml-game` `Game::net_closing`): if the host leaves a running match ([`Self::host_leaves`])
    /// or an end or handover is already running ([`Self::ending`]), the program waits until the bundle is executed
    /// ([`Self::finished`]: handed over or the network savegame written) or the closing deadline ([`Self::close_ms`] at
    /// the game speed `game_speed`, at least 3 s) from the first call has expired; the caller keeps computing frames in
    /// the meantime and calls it each frame with its clock `now_ms`. `true` while it has to wait; clients and every
    /// other situation end at once (`false`).
    pub fn close_program(&mut self, game_speed: u8, now_ms: u64) -> bool {
        let Some(deadline) = self.close_deadline else {
            if !(self.ending() || self.host_leaves()) {
                return false;
            }
            let close_ms = self.close_ms(game_speed);
            log::info!(
                "Network: end of the program at the host, the match is {} (waiting at most {} s)",
                if self.handing_over {
                    "handed over"
                } else {
                    "ended for all"
                },
                close_ms / 1000
            );
            self.close_deadline = Some(now_ms.saturating_add(close_ms));
            return true;
        };
        if self.finished() {
            log::info!("Network: handover or end of the match distributed, end of the program");
        } else if now_ms >= deadline {
            log::warn!(
                "Network: handover or end of the match not distributed in time, end of the program"
            );
        } else {
            return true;
        }
        self.close_deadline = None;
        false
    }

    /// In the same frame as the change to host: the game host sends the mirror commands from now on.
    pub(super) fn follow_role(&mut self, sim: &mut SimHost) {
        if self.frame.became_host && !sim.net_host {
            sim.set_net_host(true);
        }
    }

    /// `CoreEvent::HostGone` at the client: old connection gone, begin the election.
    pub(super) fn start_migration(&mut self, reason: String, planned: bool) {
        let Core::Client(c) = &self.core else {
            log::error!("Network: HostGone without a client core discarded");
            return;
        };
        log::warn!(
            "Network: host dropped out ({reason}, planned: {planned}), looking for the new host"
        );
        let own = c.peer().unwrap_or(HOST_PEER);
        let only = c.handover().map(<[u8]>::to_vec);
        let succession = Succession::new(c.successors(), own, only.as_deref());
        self.gone_reason = Some(reason);
        // The old connection no longer counts; the one to the new host becomes `HOST_PEER` again.
        if let Some(i) = self.links.iter().position(|(p, _)| *p == HOST_PEER) {
            let (_, mut link) = self.links.remove(i);
            link.close("Host dropped out");
        }
        self.closed.retain(|p| *p != HOST_PEER);
        self.next_candidate(succession);
    }

    /// Next step of the election: dial, become host or give up.
    fn next_candidate(&mut self, mut succession: Succession) {
        self.migration = None;
        loop {
            match succession.next() {
                SuccessionStep::Dial { peer, addr } => {
                    let Some(t) = self.transport.as_mut() else {
                        log::warn!("Network: no transport for the candidate {peer} ({addr})");
                        continue;
                    };
                    log::info!("Network: connecting to the candidate {peer} at {addr}");
                    let connecting = t.connect(addr, SUCCESSOR_CONNECT_MS);
                    self.migration = Some(MigrationRun::Dialing {
                        succession,
                        peer,
                        connecting,
                    });
                    return;
                }
                SuccessionStep::BecomeHost => return self.become_host(),
                SuccessionStep::GiveUp => {
                    log::warn!("Network: no new host found, abort as without migration");
                    if let Some(c) = self.client_core_mut() {
                        c.abandon();
                    }
                    return;
                }
            }
        }
    }

    /// The own entry is up: the client core becomes host.
    fn become_host(&mut self) {
        let seats = self.start.as_ref().map(|s| s.seats.clone());
        let only = self
            .client_core()
            .and_then(|c| c.handover().map(<[u8]>::to_vec));
        let (Some(seats), Core::Client(_)) = (seats, &self.core) else {
            log::error!("Network: host change without start or client core, abort");
            if let Some(c) = self.client_core_mut() {
                c.abandon();
            }
            return;
        };
        let Core::Client(client) = std::mem::replace(&mut self.core, placeholder_core()) else {
            unreachable!("checked above: client core");
        };
        let result = HostCore::from_client(
            client,
            &seats,
            self.results,
            Box::new(AcceptAll),
            self.now_ms,
            only.as_deref(),
        );
        match result {
            Ok(host) => {
                log::info!("Network: this machine is now the host and gathers the Rejoin");
                self.core = Core::Host(Box::new(host));
                // A running resync lapses with the migration.
                self.adopt_due = None;
                self.resync_bytes = None;
                self.frame.became_host = true;
            }
            Err((client, text)) => {
                log::error!("Network: {text}");
                self.core = Core::Client(client);
                if let Some(c) = self.client_core_mut() {
                    c.abandon();
                }
            }
        }
    }

    /// Client: advance the connection setup and the wait for `Migrated`.
    pub(super) fn pump_migration(&mut self, now_ms: u64) {
        let Some(run) = self.migration.take() else {
            return;
        };
        match run {
            MigrationRun::Dialing {
                succession,
                peer,
                mut connecting,
            } => match connecting.poll() {
                None => {
                    self.migration = Some(MigrationRun::Dialing {
                        succession,
                        peer,
                        connecting,
                    });
                }
                Some(Err(e)) => {
                    log::warn!("Network: candidate {peer} not reachable: {e}");
                    self.next_candidate(succession);
                }
                Some(Ok(mut link)) => {
                    if self.muted {
                        link.mute(true);
                    }
                    let rejoin = match self.client_core_mut().map(ClientCore::rejoin) {
                        Some(Ok(r)) => r,
                        Some(Err(text)) => {
                            // Too large or no migration any more - give up.
                            log::error!("Network: {text}");
                            link.close(&text);
                            if let Some(c) = self.client_core_mut() {
                                c.abandon();
                            }
                            return;
                        }
                        None => return,
                    };
                    log::info!(
                        "Network: Rejoin to the candidate {peer} (next bundle {}, next turn {})",
                        rejoin.next_bundle,
                        rejoin.next_close
                    );
                    link.send(NetMessage::Rejoin(Box::new(rejoin)));
                    self.migration = Some(MigrationRun::AwaitMigrated {
                        succession,
                        peer,
                        link,
                        deadline_ms: now_ms.saturating_add(AWAIT_MIGRATED_MS),
                    });
                }
            },
            MigrationRun::AwaitMigrated {
                mut succession,
                peer,
                mut link,
                deadline_ms,
            } => {
                let mut events = link.poll().into_iter();
                while let Some(ev) = events.next() {
                    match ev {
                        LinkEvent::Message(NetMessage::Migrated(m)) => {
                            let result = match self.client_core_mut() {
                                Some(c) => c.rehome(*m),
                                None => return,
                            };
                            if let Err(text) = result {
                                log::warn!(
                                    "Network: Migrated from candidate {peer} discarded: {text}"
                                );
                                link.close(&text);
                                return self.next_candidate(succession);
                            }
                            self.rehomed(peer, link);
                            // What came after the `Migrated` (e.g. already the next bundle) is for the new host.
                            for ev in events {
                                self.lockstep().handle(now_ms, HOST_PEER, ev);
                            }
                            return;
                        }
                        LinkEvent::Message(NetMessage::Reject { reason }) => {
                            log::warn!("Network: candidate {peer} rejects: {reason:?}");
                            succession.rejected(&reason);
                            link.close("Rejected");
                            return self.next_candidate(succession);
                        }
                        LinkEvent::Closed(reason)
                        | LinkEvent::Message(NetMessage::Bye { reason }) => {
                            log::warn!(
                                "Network: connection to the candidate {peer} closed: {reason}"
                            );
                            link.close(&reason);
                            return self.next_candidate(succession);
                        }
                        other => {
                            log::debug!(
                                "Network: discarded before the Migrated from candidate {peer}: {other:?}"
                            );
                        }
                    }
                }
                if now_ms >= deadline_ms {
                    log::warn!(
                        "Network: no Migrated from candidate {peer} within {AWAIT_MIGRATED_MS} ms"
                    );
                    link.close("No Migrated within the deadline");
                    return self.next_candidate(succession);
                }
                self.migration = Some(MigrationRun::AwaitMigrated {
                    succession,
                    peer,
                    link,
                    deadline_ms,
                });
            }
        }
    }

    /// Client after `rehome`: the connection to the new host (old peer `peer`) is [`HOST_PEER`] again, the slots
    /// follow, a resync lapses.
    fn rehomed(&mut self, peer: Peer, link: Box<dyn Link>) {
        log::info!("Network: new host is peer {peer}");
        self.closed.retain(|p| *p != HOST_PEER);
        self.links.push((HOST_PEER, link));
        self.adopt_due = None;
        self.resync_bytes = None;
        self.gone_reason = None;
        let slot = self.start.as_ref().and_then(|s| seat_of(&s.seats, peer));
        self.reseat(slot);
    }

    /// The slot of the old host without a peer, the slot of the new host `new_host` peer 0.
    fn reseat(&mut self, new_host: Option<u8>) {
        let Some(start) = self.start.as_mut() else {
            return;
        };
        for seat in &mut start.seats {
            if seat.peer == Some(HOST_PEER) {
                seat.peer = None;
            }
        }
        match new_host.and_then(|s| start.seats.get_mut(usize::from(s))) {
            Some(seat) => seat.peer = Some(HOST_PEER),
            None => log::warn!("Network: slot of the new host unknown ({new_host:?})"),
        }
    }

    /// New host: gathering done. The lost slots go into [`NetFrame::lost`] (their defeat is in the bundle `K + 1`; the
    /// simulation creates the message 17200 + p once, also after a handover), the listener closes.
    pub(super) fn migration_done(&mut self, lost: Vec<u8>) {
        let old_slot = self
            .start
            .as_ref()
            .and_then(|s| seat_of(&s.seats, HOST_PEER));
        let gone = self.gone_reason.take();
        log::info!("Network: host change done, lost {lost:?}");
        for slot in lost {
            let reason = match &gone {
                Some(r) if Some(slot) == old_slot => r.clone(),
                _ => NOT_RETURNED.to_string(),
            };
            self.frame.lost.push((slot, reason));
        }
        let own = self.own_slot();
        self.reseat(own);
        // The new host no longer listens; latecomers get `MIGRATION_FAILED` through the waiting list.
        self.listener = None;
    }

    /// New host: the migration has failed. Like the dropout of the host without a successor: abort savegame from the
    /// own bundles (`NetFinish::HostLost`); the `Reject` to the returned ones are already out.
    pub(super) fn migration_failed(&mut self, reason: &str) {
        log::warn!("Network: host change failed: {reason}");
        self.listener = None;
        self.failure.get_or_insert_with(|| HOST_LOST.to_string());
        self.frame
            .failed
            .get_or_insert_with(|| HOST_LOST.to_string());
        if self.in_play && !self.finished && !self.gave_up {
            self.abort_due = true;
        }
    }

    /// End the running election (end, leaving, failure): close the connection to the candidate.
    pub(super) fn stop_migration(&mut self, reason: &str) {
        match self.migration.take() {
            Some(MigrationRun::AwaitMigrated { mut link, .. }) => link.close(reason),
            Some(MigrationRun::Dialing { .. }) | None => {}
        }
    }

    /// Accept new connections of the listener and check the waiting ones. A connection is dropped on `Closed` (e.g.
    /// the probe of the host, which closes at once), on a first message other than `Rejoin` and without a message after
    /// [`REJOIN_WINDOW_MS`]. At the host a `Rejoin` goes to [`HostCore::on_rejoin`] (`Ok(peer)` -> connection of this
    /// peer, `Err` -> `Reject`); at the client the driver holds it for up to `W` and then rejects it with
    /// `MIGRATION_REFUSED`.
    pub(super) fn pump_accepted(&mut self, now_ms: u64) {
        if let Some(listener) = self.listener.as_mut() {
            for mut link in listener.accept_pending() {
                log::debug!("Network: connection accepted on the listener");
                #[cfg(test)]
                {
                    self.accepted_total += 1;
                }
                if self.muted {
                    link.mute(true);
                }
                self.accepted.push(Accepted {
                    link,
                    since_ms: now_ms,
                    rejoin: None,
                });
            }
        }
        for mut a in std::mem::take(&mut self.accepted) {
            let mut open = true;
            for ev in a.link.poll() {
                match ev {
                    LinkEvent::Closed(_) => open = false,
                    LinkEvent::Message(NetMessage::Rejoin(r)) if a.rejoin.is_none() => {
                        log::info!("Network: Rejoin from peer {} on the listener", r.peer);
                        a.rejoin = Some(r);
                        a.since_ms = now_ms;
                    }
                    other if a.rejoin.is_none() => {
                        log::warn!(
                            "Network: unexpected input on the listener, connection closed: {other:?}"
                        );
                        a.link.close("Unexpected message");
                        open = false;
                    }
                    other => log::debug!("Network: discarded after the Rejoin: {other:?}"),
                }
                if !open {
                    break;
                }
            }
            if !open {
                continue;
            }
            let waited = now_ms.saturating_sub(a.since_ms);
            match a.rejoin.take() {
                Some(r) => match &mut self.core {
                    Core::Host(h) => match h.on_rejoin(now_ms, *r) {
                        Ok(peer) => self.adopt_link(peer, a.link),
                        Err(reason) => {
                            a.link.send(NetMessage::Reject {
                                reason: reason.clone(),
                            });
                            a.link.close(&reason);
                        }
                    },
                    Core::Client(_) if waited >= REJOIN_WINDOW_MS => {
                        log::warn!("Network: Rejoin rejected, this machine has not become host");
                        a.link.send(NetMessage::Reject {
                            reason: MIGRATION_REFUSED.to_string(),
                        });
                        a.link.close(MIGRATION_REFUSED);
                    }
                    Core::Client(_) => {
                        a.rejoin = Some(r);
                        self.accepted.push(a);
                    }
                },
                None if waited >= REJOIN_WINDOW_MS => {
                    a.link.close("No message within the deadline");
                }
                None => self.accepted.push(a),
            }
        }
    }

    /// New host: the connection of an accepted `Rejoin` now belongs to `peer` (before the outbox is sent).
    fn adopt_link(&mut self, peer: Peer, link: Box<dyn Link>) {
        if let Some(i) = self.links.iter().position(|(p, _)| *p == peer) {
            let (_, mut old) = self.links.remove(i);
            old.close("Reconnected");
        }
        self.closed.retain(|p| *p != peer);
        self.links.push((peer, link));
    }

    /// State of the migration for [`NetStatus::migration`].
    pub(super) fn migration_status(&self) -> Option<MigrationStatus> {
        match &self.migration {
            Some(MigrationRun::Dialing { peer, .. }) => {
                Some(MigrationStatus::Dialing { peer: *peer })
            }
            Some(MigrationRun::AwaitMigrated { peer, .. }) => {
                Some(MigrationStatus::Rejoining { peer: *peer })
            }
            None => match &self.core {
                Core::Host(h) if h.migrating() => Some(MigrationStatus::Gathering {
                    expected: h.expected(),
                    arrived: h.rejoined(),
                }),
                _ => None,
            },
        }
    }
}

/// Placeholder for the moment in which the client core sits in [`HostCore::from_client`].
fn placeholder_core() -> Core {
    Core::Client(Box::new(ClientCore::new(Hello {
        net_version: NET_VERSION,
        protocol: 0,
        format: 0,
        edition: String::new(),
        name: String::new(),
        build: String::new(),
        seat: None,
        listen_port: None,
    })))
}
