//! Network state for the state sync: simulation part of the savegame ([`SaveGame`], without commands) and the whole
//! command log since the start.
//!
//! [`SaveGame::commands`] means "commands **from** the state on"; the whole log therefore does not belong in it. The
//! network state carries it alongside.
//!
//! File: identifier `NMLNETST`, [`NET_STATE_FORMAT`] as u32 LE, FNV-1a 64 of the body as u64 LE, body = `postcard` of
//! `{ sim: SaveGame::to_bytes(), log }` (layout like `GameSaveFile`: the simulation part stays in its own bytes, so
//! that a change of `FORMAT_VERSION` is noticed as a "format version"). The host sends it to the deviating clients
//! during the sync; the desync report stores both network states as `host.nmllog` and `local.nmllog`.
//!
//! The original never syncs; the reimplementation does (deviation from the original, E-260, E-261).

use anyhow::{Context, bail};
use opennml_sim::rules::Rules;
use opennml_sim::save::{RecordedCommand, SaveGame, read_container, write_container};
use serde::{Deserialize, Serialize};

/// Identifier of the file.
pub const NET_STATE_MAGIC: &[u8; 8] = b"NMLNETST";
/// Format version of the container. History: 1 initial.
pub const NET_STATE_FORMAT: u32 = 1;

/// State of a machine for sync and report.
#[derive(Debug, Clone, PartialEq)]
pub struct NetState {
    /// Start package and state; `commands` empty.
    pub save: SaveGame,
    /// All executed commands since the start (`SimHost::log`), ordered by tick, all before `save.state.steps`.
    pub log: Vec<RecordedCommand>,
}

/// Body of the file.
#[derive(Serialize, Deserialize)]
struct Body {
    /// [`SaveGame::to_bytes`] with identifier, version and checksum of the simulation.
    sim: Vec<u8>,
    log: Vec<RecordedCommand>,
}

/// Body for writing (without a copy of the log).
#[derive(Serialize)]
struct BodyRef<'a> {
    sim: &'a [u8],
    log: &'a [RecordedCommand],
}

impl NetState {
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = postcard::to_stdvec(&BodyRef {
            sim: &self.save.to_bytes(),
            log: &self.log,
        })
        .expect("serializable");
        write_container(NET_STATE_MAGIC, NET_STATE_FORMAT, &body)
    }

    /// Reads a network state: identifier, format version and checksum of the container ([`read_container`]), the
    /// simulation part via [`SaveGame::from_bytes`] (structure check of the state) and the log. It is not known to
    /// match the rules until [`Self::check`].
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let body = read_container(NET_STATE_MAGIC, NET_STATE_FORMAT, bytes, "network state")?;
        let (Body { sim, log }, rest) =
            postcard::take_from_bytes(body).context("network state corrupted")?;
        if !rest.is_empty() {
            bail!(
                "network state corrupted ({} extra bytes at the end of the body)",
                rest.len()
            );
        }
        let save = SaveGame::from_bytes(&sim).context("savegame in the network state")?;
        Ok(Self { save, log })
    }

    /// Does the network state match the rules ([`SaveGame::check`])? Loading does not check the log: `SimHost::adopt`
    /// only adopts it and writes it to the recording and replay.
    pub fn check(&self, rules: &Rules) -> anyhow::Result<()> {
        self.save.check(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::sim_host::plain_testkit;
    use opennml_sim::save::HEADER_LEN;

    /// Combined test of the crash checks: damaged network states are errors, not panics. The case "simulation part
    /// damaged" is also `net_state_with_a_damaged_sim_is_an_error`: a forged network state of the host goes through
    /// the same structure check as a savegame.
    #[test]
    fn damaged_containers_are_errors_not_panics() {
        let mut host = plain_testkit::host();
        for _ in 0..3 {
            host.step_once();
        }
        let state = NetState {
            save: host.save(),
            log: Vec::new(),
        };
        let bytes = state.to_bytes();
        NetState::from_bytes(&bytes).expect("undamaged");
        let edit = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = bytes.clone();
            f(&mut b);
            b
        };
        // Body with a valid checksum (for errors behind the header).
        let with_body = |body: &[u8]| write_container(NET_STATE_MAGIC, NET_STATE_FORMAT, body);
        // Simulation part with an object table that is too short, validly signed.
        let mut damaged = state.clone();
        damaged.save.state.tables[0].objects.pop();
        let cases = [
            ("11 bytes", bytes[..11].to_vec()),
            ("19 bytes", bytes[..19].to_vec()),
            (
                "foreign identifier",
                edit(&|b| b[..8].copy_from_slice(b"NMLSAVE\0")),
            ),
            (
                "other version",
                edit(&|b| b[8..12].copy_from_slice(&(NET_STATE_FORMAT + 1).to_le_bytes())),
            ),
            ("wrong checksum", edit(&|b| *b.last_mut().unwrap() ^= 0x55)),
            (
                "one byte left over",
                with_body(&[&bytes[HEADER_LEN..], &[0]].concat()),
            ),
            ("simulation part damaged", damaged.to_bytes()),
        ];
        for (what, bytes) in cases {
            match std::panic::catch_unwind(|| NetState::from_bytes(&bytes).map(|_| ())) {
                Ok(Err(e)) => println!("{what}: {e:#}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    }
}
