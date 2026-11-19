//! Rules computation of the game data check (re/multiplayer-menus L.18) outside the frame: the host computes the rules
//! of its session after `CREATE` and after every accepted `CHANGEGAME`, a client after the first lobby state and after
//! every new session revision. A job carries a clone of the [`Vfs`] and the map ([`MapSource`]); the result carries the
//! rules (or the reason text) and the read recording ([`source_rules_recorded`]).
//!
//! [`ThreadWorker`] computes on one thread per job (the program), [`InlineWorker`] on the next [`RulesWorker::poll`]
//! (tests and `UI_GOLDEN`, so that every run takes the same frames). A new job makes the previous one meaningless: its
//! result never arrives, a running thread is not cancelled. The session compares the session revision of every result
//! with its current one anyway.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use opennml_rda::{ReadEntry, Vfs};
use opennml_sim::rules::Rules;

use super::source_rules_recorded;
use crate::game::map::MapSource;

/// Rules computation for the session with `session_revision`.
pub struct RulesJob {
    pub session_revision: u32,
    pub vfs: Vfs,
    pub source: MapSource,
}

/// Result of a [`RulesJob`]: the rules (or the reason text) and the files the computation read.
pub struct RulesDone {
    pub session_revision: u32,
    pub rules: Result<Rules, String>,
    pub inputs: Vec<ReadEntry>,
}

impl RulesJob {
    /// Computes the rules on a recording copy of the [`Vfs`] ([`source_rules_recorded`]).
    pub fn run(self) -> RulesDone {
        let (rules, inputs) = source_rules_recorded(&self.vfs, &self.source);
        RulesDone {
            session_revision: self.session_revision,
            rules,
            inputs,
        }
    }
}

// Jobs and results cross threads.
const _: () = {
    const fn send<T: Send>() {}
    send::<Rules>();
    send::<RulesJob>();
    send::<RulesDone>();
};

/// Computes rules for the session; [`Self::poll`] returns the result of the last submitted job once it is there.
pub trait RulesWorker: Send {
    fn submit(&mut self, job: RulesJob);
    fn poll(&mut self) -> Option<RulesDone>;
}

/// One thread per job, result over a channel; a new job drops the channel of the previous one.
#[derive(Default)]
pub struct ThreadWorker {
    results: Option<Receiver<RulesDone>>,
}

impl RulesWorker for ThreadWorker {
    fn submit(&mut self, job: RulesJob) {
        let (tx, rx) = mpsc::channel();
        self.results = Some(rx);
        // If no thread can be started, the job runs here, in this frame.
        let slot = Arc::new(Mutex::new(Some(job)));
        let theirs = Arc::clone(&slot);
        let sender = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("rules check".into())
            .spawn(move || {
                let job = theirs.lock().ok().and_then(|mut j| j.take());
                if let Some(job) = job {
                    // The session may have moved on (receiver dropped): then nobody waits for the result.
                    let _ = sender.send(job.run());
                }
            });
        if let Err(e) = spawned {
            log::warn!("Network: no thread for the rules check ({e}), computing in this frame");
            let job = slot.lock().ok().and_then(|mut j| j.take());
            if let Some(job) = job {
                let _ = tx.send(job.run());
            }
        }
    }

    fn poll(&mut self) -> Option<RulesDone> {
        let done = self.results.as_ref()?.try_recv().ok()?;
        self.results = None;
        Some(done)
    }
}

/// Computes the last submitted job on the next [`Self::poll`], in the frame of the caller.
#[derive(Default)]
pub struct InlineWorker {
    job: Option<RulesJob>,
}

impl RulesWorker for InlineWorker {
    fn submit(&mut self, job: RulesJob) {
        self.job = Some(job);
    }

    fn poll(&mut self) -> Option<RulesDone> {
        self.job.take().map(RulesJob::run)
    }
}
