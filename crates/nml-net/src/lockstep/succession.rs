//! Choice of the new host: pure state machine over the successor list from `Go`.
//!
//! The client walks through the candidates by peer in order: only entries with an address (the old host reached them),
//! after an orderly handover only the peers from `Handover`. At its own entry it becomes host itself; otherwise the
//! driver connects to the candidate (deadline [`super::SUCCESSOR_CONNECT_MS`]), sends the `Rejoin` and waits for
//! `Migrated` (deadline [`super::REJOIN_WINDOW_MS`]). If that fails (deadline passed, connection closed, `Reject`),
//! the driver calls [`Succession::next`] once more. It reports the reason of a `Reject` with
//! [`Succession::rejected`]: With [`crate::msg::MIGRATION_REFUSED`] the candidate still hears the old host, with
//! [`crate::msg::MIGRATION_FAILED`] the new host has given up the host migration (for this machine); another attempt
//! could then produce a second host, so the client gives up. If the list is exhausted, the client gives up too (abort
//! savegame as today).
//!
//! In the original DirectPlay chooses alone (`DPNSESSION_MIGRATE_HOST` when hosting `0x004EB2A7`, `dpnidNewHost` from
//! the message `0x004EACB7`); the game takes over the new host in `FUN_004E9D20` (E-268 deviation).

use super::Peer;
use crate::msg::{MIGRATION_FAILED, MIGRATION_REFUSED, Successor};
use std::net::SocketAddr;

/// Next step of the choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuccessionStep {
    /// Connect to the candidate `peer` at `addr` and send `Rejoin`.
    Dial { peer: Peer, addr: SocketAddr },
    /// The own entry is next: become host itself.
    BecomeHost,
    /// No candidate left (or refused with `MIGRATION_REFUSED`): give up.
    GiveUp,
}

/// Choice of the new host among the candidates of the successor list.
#[derive(Debug, Clone)]
pub struct Succession {
    /// Candidates by peer: (peer, address).
    candidates: Vec<(Peer, SocketAddr)>,
    own: Peer,
    /// Next candidate.
    pos: usize,
    /// A candidate has refused with `MIGRATION_REFUSED`.
    refused: bool,
}

impl Succession {
    /// Choice over `list` for the machine with the peer `own`; with `only` (peers from `Handover`) only among these.
    pub fn new(list: &[Successor], own: Peer, only: Option<&[u8]>) -> Self {
        let mut candidates: Vec<(Peer, SocketAddr)> = list
            .iter()
            .filter(|s| only.is_none_or(|peers| peers.contains(&s.peer)))
            .filter_map(|s| Some((s.peer, s.addr?)))
            .collect();
        candidates.sort_by_key(|&(peer, _)| peer);
        Self {
            candidates,
            own,
            pos: 0,
            refused: false,
        }
    }

    /// Next step: the next candidate, otherwise [`SuccessionStep::GiveUp`] (also on every further call).
    #[allow(
        clippy::should_implement_trait,
        reason = "not an iterator: after the end GiveUp always comes again, never None"
    )]
    pub fn next(&mut self) -> SuccessionStep {
        if self.refused {
            return SuccessionStep::GiveUp;
        }
        let Some(&(peer, addr)) = self.candidates.get(self.pos) else {
            return SuccessionStep::GiveUp;
        };
        self.pos += 1;
        if peer == self.own {
            SuccessionStep::BecomeHost
        } else {
            SuccessionStep::Dial { peer, addr }
        }
    }

    /// The candidate chosen last has refused with [`crate::msg::MIGRATION_REFUSED`]: Every further
    /// [`Succession::next`] gives up.
    pub fn refused(&mut self) {
        self.refused = true;
    }

    /// The candidate chosen last has refused with `reason` (`Reject`): [`crate::msg::MIGRATION_REFUSED`] like
    /// [`Succession::refused`], likewise [`crate::msg::MIGRATION_FAILED`] (the new host gives up the host migration
    /// for this machine; a further candidate would produce a second host). Any other reason changes nothing: the
    /// driver then calls [`Succession::next`].
    pub fn rejected(&mut self, reason: &str) {
        if reason == MIGRATION_REFUSED || reason == MIGRATION_FAILED {
            self.refused = true;
        }
    }
}
