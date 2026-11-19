//! Rules computation of the game data check (re/multiplayer-menus L.18) outside the frame: the host computes the rules
//! of its session after `CREATE` and after every accepted `CHANGEGAME`, a client after the first lobby state and after
//! every new session revision. A job carries a clone of the [`Vfs`] and the map ([`MapSource`]); the result carries the
//! rules (or the reason text) and the read recording ([`source_rules_recorded`]).
//!
//! [`ThreadWorker`] computes on one thread per job (the program), [`InlineWorker`] on the next [`RulesWorker::poll`]
//! (tests and `UI_GOLDEN`, so that every run takes the same frames). A new job makes the previous one meaningless: its
//! result never arrives, a running thread is not cancelled. The session compares the session revision of every result
//! with its current one anyway.

use std::sync::mpsc::{self, Receiver, TryRecvError};
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

/// Reason of the result of a job whose thread stopped without one (a panic in the computation).
pub const RULES_THREAD_STOPPED: &str = "rules check stopped unexpectedly";

/// One thread per job, result over a channel; a new job drops the channel of the previous one. A thread that ends
/// without a result (panic) yields an error result for its job ([`RULES_THREAD_STOPPED`]), so that the session takes
/// its error path instead of waiting forever.
pub struct ThreadWorker {
    results: Option<Receiver<RulesDone>>,
    /// Session revision of the last submitted job.
    revision: u32,
    /// The computation of a job ([`RulesJob::run`]; tests put in a failing one).
    run: fn(RulesJob) -> RulesDone,
    /// The thread of the last job (tests wait for it).
    #[cfg(test)]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Default for ThreadWorker {
    fn default() -> Self {
        Self {
            results: None,
            revision: 0,
            run: RulesJob::run,
            #[cfg(test)]
            thread: None,
        }
    }
}

impl RulesWorker for ThreadWorker {
    fn submit(&mut self, job: RulesJob) {
        let (tx, rx) = mpsc::channel();
        self.results = Some(rx);
        self.revision = job.session_revision;
        let run = self.run;
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
                    let _ = sender.send(run(job));
                }
            });
        match spawned {
            #[cfg(test)]
            Ok(thread) => self.thread = Some(thread),
            #[cfg(not(test))]
            Ok(_) => {}
            Err(e) => {
                log::warn!("Network: no thread for the rules check ({e}), computing in this frame");
                let job = slot.lock().ok().and_then(|mut j| j.take());
                if let Some(job) = job {
                    let _ = tx.send(run(job));
                }
            }
        }
    }

    fn poll(&mut self) -> Option<RulesDone> {
        let done = match self.results.as_ref()?.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return None,
            // Every sender is gone without a result: the thread has stopped (panic in the computation).
            Err(TryRecvError::Disconnected) => {
                log::warn!(
                    "Network: the rules check of session {} stopped without a result",
                    self.revision
                );
                RulesDone {
                    session_revision: self.revision,
                    rules: Err(RULES_THREAD_STOPPED.to_string()),
                    inputs: Vec::new(),
                }
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A computation that panics without a panic message in the log (`resume_unwind` skips the panic hook).
    fn panics(_: RulesJob) -> RulesDone {
        std::panic::resume_unwind(Box::new("rules computation panicked"))
    }

    /// A panic in the thread is not a silent hang: the next poll after the thread's end yields an error result for
    /// the job's session revision, and the worker waits for nothing afterwards.
    #[test]
    fn panicking_thread_reports_an_error() {
        let mut worker = ThreadWorker {
            run: panics,
            ..ThreadWorker::default()
        };
        worker.submit(RulesJob {
            session_revision: 7,
            vfs: Vfs::from_archives(Vec::new()),
            source: MapSource::Level(String::new()),
        });
        let thread = worker.thread.take().expect("thread started");
        assert!(thread.join().is_err(), "the thread panicked");
        let done = worker.poll().expect("result after the thread's end");
        assert_eq!(done.session_revision, 7);
        assert_eq!(done.rules.err().as_deref(), Some(RULES_THREAD_STOPPED));
        assert!(done.inputs.is_empty());
        assert!(worker.poll().is_none(), "nothing more");
    }
}
