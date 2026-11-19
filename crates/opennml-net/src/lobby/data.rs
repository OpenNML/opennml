//! Game data check of a new lobby (re/multiplayer-menus L.18): before a network start every machine compares the rules
//! checksum of the session's map with the host's. The host keeps its own read recording (path and hash of every file
//! its rules computation read); a client whose checksum differs sends its recording, and the core names the files that
//! differ ([`compare_inputs`]). The verdict per row is a [`DataState`] in the lobby state.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Most entries of a read recording in a [`DataReport`] (a real recording has a few thousand).
pub const MAX_DATA_INPUTS: usize = 16_384;
/// Longest path of an entry in bytes.
pub const MAX_DATA_PATH: usize = 260;
/// Largest sum of the path lengths of a recording in bytes: with [`MAX_DATA_INPUTS`] hashes it stays well below
/// [`crate::wire::MAX_FRAME`].
pub const MAX_DATA_INPUT_BYTES: usize = 512 * 1024;
/// Number of differing files a [`DataState::Differs`] names.
pub const SHOWN_FILES: usize = 3;

/// One file of a read recording: normalized path and FNV-1a 64 of its bytes, `None` if reading failed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DataInput {
    pub path: String,
    pub hash: Option<u64>,
}

/// Verdict of the game data check for a lobby row.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub enum DataState {
    /// The machine of the row has not reported for the current session yet.
    Pending,
    /// Same rules checksum as the host (also: host row, computers, open rows, check switched off).
    #[default]
    Equal,
    /// The fixed map is missing on the machine or its map checksum differs.
    Missing,
    /// The rules differ: the first [`SHOWN_FILES`] differing paths (ascending) and the number of the others.
    Differs { files: Vec<String>, more: u16 },
}

/// Result of a client's check for the session ([`crate::LobbyWish::Data`]).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DataReport {
    /// The fixed map is missing or its map checksum differs.
    Missing,
    /// Own rules checksum (`None`: the own computation failed) and the own read recording, only if the checksum differs
    /// from the host's (otherwise empty; also empty if it exceeds the limits, see [`inputs_fit`]).
    Rules {
        checksum: Option<u64>,
        inputs: Vec<DataInput>,
    },
}

/// The recording stays within the limits of the wire ([`MAX_DATA_INPUTS`], [`MAX_DATA_PATH`],
/// [`MAX_DATA_INPUT_BYTES`]); the host rejects a larger one. The client checks this before sending.
pub fn inputs_fit(inputs: &[DataInput]) -> bool {
    inputs.len() <= MAX_DATA_INPUTS
        && inputs.iter().all(|i| i.path.len() <= MAX_DATA_PATH)
        && inputs.iter().map(|i| i.path.len()).sum::<usize>() <= MAX_DATA_INPUT_BYTES
}

/// Compares two read recordings: every path whose hash differs or that only one side has, ascending; the first
/// [`SHOWN_FILES`] and the number of the others (at most `u16::MAX`). Paths stay as they are (the display cleans them).
/// If a path occurs twice on one side, the last entry counts.
pub fn compare_inputs(host: &[DataInput], client: &[DataInput]) -> (Vec<String>, u16) {
    let map = |inputs: &[DataInput]| -> BTreeMap<String, Option<u64>> {
        inputs.iter().map(|i| (i.path.clone(), i.hash)).collect()
    };
    let (host, client) = (map(host), map(client));
    let differing: BTreeSet<&String> = host
        .keys()
        .chain(client.keys())
        .filter(|p| host.get(*p) != client.get(*p))
        .collect();
    let files: Vec<String> = differing
        .iter()
        .take(SHOWN_FILES)
        .map(|p| (*p).clone())
        .collect();
    let more = u16::try_from(differing.len() - files.len()).unwrap_or(u16::MAX);
    (files, more)
}
