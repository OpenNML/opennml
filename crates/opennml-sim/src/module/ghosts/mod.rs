//! Ghosts and banishing (re/ghosts-banish): module 26 `EVOKEGHOST` ([`evoke`], evoke
//! ghost), module 27 `GHOST` ([`ghost`], the evoked ghost) and module 31 `BANISH`
//! ([`banish`], banish ghosts).
//!
//! The module holds the states and `Init` (re/ghosts-banish 1.2-1.4, 5, 10.1) and the flows (re/ghosts-banish 3).
//! Effect IDs, effect and node names are presentation and absent from the state; the name counter of the ghosts
//! is `evoke::EvokeGhost::name_counter`.

pub mod banish;
pub mod evoke;
pub mod ghost;

#[cfg(test)]
mod tests;
