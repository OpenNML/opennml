//! Appendable replay file ("recording", identifier `NMLAUTO`).
//!
//! `NMLREPL` (`opennml_sim::save::Replay`) carries a checksum over the whole body and cannot be continued. The recording
//! instead writes any number of **chunks** after a header, each with its own length and checksum: `GameFlow` appends
//! the new commands every frame, and a crash or abort leaves at most the last chunk incomplete (E-337 except for half
//! records, which deviate: the original writes only when the match is torn down).
//!
//! ```text
//! Header: "NMLAUTO\0" | REPLAY_FILE_FORMAT u32 | FORMAT_VERSION u32 | len u32 | FNV-1a-64 of header u64 | header
//! Chunk:  length u32 (at most MAX_CHUNK_BYTES) | FNV-1a-64 u64 | postcard of Vec<RecordedCommand>
//! ```
//!
//! All numbers little-endian, the header is `postcard` of [`ReplayHead`]. Reading: if the header is broken or the
//! version a different one, it is an error; the first incomplete or wrongly checked chunk ends the reading ("torn", the
//! recording counts up to there). Reading does not check the command sequence: the replay catches up on unordered
//! commands, and the length bounds the playback.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use opennml_sim::rules::Fnv64;
use opennml_sim::save::{MAX_REPLAY_STEPS, REPLAY_MAGIC, RecordedCommand, Replay};
use opennml_sim::setup::{FORMAT_VERSION, StartPackage};
use opennml_ui::draft::LevelInfoData;
use opennml_ui::saves::LocalStamp;
use serde::{Deserialize, Serialize};

use crate::game::sim_host::SimHost;

/// Identifier of the file.
pub const REPLAY_FILE_MAGIC: &[u8; 8] = b"NMLAUTO\0";
/// Version of the file layout. History: 1 first version.
pub const REPLAY_FILE_FORMAT: u32 = 1;
/// Largest chunk (16 MiB): a larger length value counts as torn or damaged, not as a reason to allocate memory.
pub const MAX_CHUNK_BYTES: u32 = 1 << 24;

/// Length of the fixed part of the header: identifier, file version, `FORMAT_VERSION`, header length, checksum.
const FIXED_HEAD_LEN: usize = 28;
/// Length of the fixed part of a chunk: length, checksum.
const CHUNK_HEAD_LEN: usize = 12;

/// Host data that the start package does not carry (needed to set up the match as recorded).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayMeta {
    /// Map info (name, path, size class, slots, random map data).
    pub level_info: LevelInfoData,
    pub game_type: i32,
    pub lock_teams: bool,
    /// Speed at the start of the recording (after loading, the one from loading).
    pub game_speed: u8,
}

/// Header of the recording: start package and host data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayHead {
    pub start: StartPackage,
    pub meta: ReplayMeta,
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = Fnv64::default();
    h.write(data);
    h.0
}

/// The bytes at the start of the file (identifier up to the header body).
pub fn head_bytes(head: &ReplayHead) -> Vec<u8> {
    let body = postcard::to_stdvec(head).expect("serializable");
    let mut bytes = REPLAY_FILE_MAGIC.to_vec();
    bytes.extend(REPLAY_FILE_FORMAT.to_le_bytes());
    bytes.extend(FORMAT_VERSION.to_le_bytes());
    bytes.extend(
        u32::try_from(body.len())
            .expect("header under 4 GiB")
            .to_le_bytes(),
    );
    bytes.extend(fnv1a64(&body).to_le_bytes());
    bytes.extend(body);
    bytes
}

/// A chunk to append: length, checksum, commands.
///
/// The caller keeps every chunk below [`MAX_CHUNK_BYTES`] (a frame brings few commands; it splits a whole log after
/// loading into pieces).
pub fn chunk_bytes(commands: &[RecordedCommand]) -> Vec<u8> {
    let body = postcard::to_stdvec(commands).expect("serializable");
    let mut bytes = Vec::with_capacity(CHUNK_HEAD_LEN + body.len());
    bytes.extend(
        u32::try_from(body.len())
            .expect("chunk under 4 GiB")
            .to_le_bytes(),
    );
    bytes.extend(fnv1a64(&body).to_le_bytes());
    bytes.extend(body);
    bytes
}

/// A read recording.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayFile {
    pub head: ReplayHead,
    /// Commands of all whole chunks in file order (ordered by tick, as recorded; unchecked).
    pub commands: Vec<RecordedCommand>,
    /// The file ends in an incomplete or wrongly checked chunk.
    pub torn: bool,
}

impl ReplayFile {
    /// Reads header and chunks (see the module docs).
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        if bytes.len() < 8 || &bytes[..8] != REPLAY_FILE_MAGIC {
            bail!("not a recording (identifier missing)");
        }
        if bytes.len() < 16 {
            bail!("recording corrupted (header incomplete)");
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        let file_format = word(8);
        if file_format != REPLAY_FILE_FORMAT {
            bail!("recording has version {file_format}, expected {REPLAY_FILE_FORMAT}");
        }
        let version = word(12);
        if version != FORMAT_VERSION {
            bail!(
                "recording has format version {version}, expected {FORMAT_VERSION} (different simulation behavior)"
            );
        }
        if bytes.len() < FIXED_HEAD_LEN {
            bail!("recording corrupted (header incomplete)");
        }
        let head_len = word(16) as usize;
        let stored = u64::from_le_bytes(bytes[20..28].try_into().expect("8 bytes"));
        let Some(body) = bytes[FIXED_HEAD_LEN..].get(..head_len) else {
            bail!("recording corrupted (header incomplete)");
        };
        if fnv1a64(body) != stored {
            bail!("recording corrupted (header: checksum does not match)");
        }
        let head: ReplayHead =
            postcard::from_bytes(body).context("recording corrupted (header)")?;

        let mut commands: Vec<RecordedCommand> = Vec::new();
        let mut rest = &bytes[FIXED_HEAD_LEN + head_len..];
        let mut torn = false;
        while !rest.is_empty() {
            let Some((chunk, used)) = read_chunk(rest) else {
                torn = true;
                break;
            };
            commands.extend(chunk);
            rest = &rest[used..];
        }
        Ok(Self {
            head,
            commands,
            torn,
        })
    }

    /// The recording as a replay (start package and commands).
    pub fn replay(&self) -> Replay {
        Replay {
            start: self.head.start.clone(),
            commands: self.commands.clone(),
        }
    }
}

/// Reads the chunk at the start of `rest`: commands and length in bytes; `None` if it is incomplete, too long or
/// wrongly checked.
fn read_chunk(rest: &[u8]) -> Option<(Vec<RecordedCommand>, usize)> {
    if rest.len() < CHUNK_HEAD_LEN {
        return None;
    }
    let len = u32::from_le_bytes(rest[..4].try_into().expect("4 bytes"));
    if len > MAX_CHUNK_BYTES {
        return None;
    }
    let len = len as usize;
    let stored = u64::from_le_bytes(rest[4..CHUNK_HEAD_LEN].try_into().expect("8 bytes"));
    let body = rest[CHUNK_HEAD_LEN..].get(..len)?;
    if fnv1a64(body) != stored {
        return None;
    }
    let commands = postcard::from_bytes(body).ok()?;
    Some((commands, CHUNK_HEAD_LEN + len))
}

/// Reads `NMLREPL` (viewer, without host data) or `NMLAUTO`: the replay, with `NMLAUTO` the host data and whether the
/// file is torn.
pub fn read_any_replay(bytes: &[u8]) -> anyhow::Result<(Replay, Option<ReplayMeta>, bool)> {
    if bytes.starts_with(REPLAY_MAGIC) {
        return Ok((Replay::from_bytes(bytes)?, None, false));
    }
    let file = ReplayFile::from_bytes(bytes)?;
    let (replay, meta, torn) = (file.replay(), file.head.meta, file.torn);
    Ok((replay, Some(meta), torn))
}

/// Name of a recording from the local time at the start (`strftime("%y%m%d-%H%M")`, format `0x0061F0B8`,
/// `FUN_0042B810`; E-329): two-digit year, minute precision, e.g. `261108-1435`.
pub fn replay_name(now: LocalStamp) -> String {
    format!(
        "{:02}{:02}{:02}-{:02}{:02}",
        now.year.rem_euclid(100),
        now.month,
        now.day,
        now.hour,
        now.minute
    )
}

/// Chunks for `commands`, each body at most `max` bytes (a whole log after loading can exceed [`MAX_CHUNK_BYTES`]; the
/// reader would then discard it as torn). Halved until every piece fits; a single command always fits (a few bytes).
/// Nothing without commands.
fn chunks_within(commands: &[RecordedCommand], max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    push_chunks(commands, max, &mut out);
    out
}

fn push_chunks(commands: &[RecordedCommand], max: usize, out: &mut Vec<u8>) {
    if commands.is_empty() {
        return;
    }
    let bytes = chunk_bytes(commands);
    if bytes.len() - CHUNK_HEAD_LEN <= max || commands.len() == 1 {
        out.extend(bytes);
        return;
    }
    let (first, second) = commands.split_at(commands.len() / 2);
    push_chunks(first, max, out);
    push_chunks(second, max, out);
}

/// Running recording of a match: the file `savegame/autoplay/<name>` with header and the command log of the host
/// ([`SimHost::log`]); [`GameFlow`](crate::game_flow::GameFlow) catches it up in every frame and before every game
/// effect ([`Self::sync`]). The original writes per command unbuffered into a `CFile` (`Replay_WriteCommand`
/// `0x0047B340`, E-337); the reimplementation appends one chunk with its own checksum per catch-up (`File::write_all`,
/// unbuffered). A write error, the end of the log after eight hours or a log that has become shorter end the recording
/// ([`Self::ended`]); the match continues.
#[derive(Debug)]
pub struct ReplayRecorder {
    path: PathBuf,
    /// Open file; `None` as soon as the recording has ended.
    file: Option<File>,
    /// This many commands of the log are already in the file.
    written: usize,
    /// Tick of the last written command.
    last_step: Option<u64>,
    /// Tick of the simulation at the last successful catch-up (start or [`Self::sync`]): the file extends up to there
    /// (control channel).
    synced_step: u64,
}

impl ReplayRecorder {
    /// Begins the recording `dir/name`: create the folder (`create_dir_all`, deviating from E-335), a new file
    /// (one with the same name is overwritten without asking, E-329), header, then the existing log as the first chunks
    /// (after loading the whole match from tick 0). Requires a complete log ([`SimHost::log_complete`]); otherwise and
    /// on every write error an error message.
    pub fn begin(
        dir: &Path,
        name: &str,
        head: &ReplayHead,
        sim: &SimHost,
    ) -> anyhow::Result<ReplayRecorder> {
        if !sim.log_complete() {
            bail!("no recording: the match is without a command log from the start of the match");
        }
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating folder {}", dir.display()))?;
        let path = dir.join(name);
        let mut file =
            File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        let mut bytes = head_bytes(head);
        bytes.extend(chunks_within(&sim.log, MAX_CHUNK_BYTES as usize));
        file.write_all(&bytes)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(ReplayRecorder {
            path,
            file: Some(file),
            written: sim.log.len(),
            last_step: sim.log.last().map(|c| c.step),
            synced_step: sim.state.steps,
        })
    }

    /// Appends the commands that entered the log since the last call. Afterwards the recording ends if the log is no
    /// longer complete (eight hours, `log::info!`). A write error or a log that has become shorter (sync in the
    /// network) ends it with `log::warn!`. After the end it has no effect.
    pub fn sync(&mut self, sim: &SimHost) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if sim.log.len() < self.written {
            log::warn!(
                "Recording {} ended: the command log is shorter than the file ({} instead of {} commands, sync); the match continues",
                self.path.display(),
                sim.log.len(),
                self.written
            );
            self.file = None;
            return;
        }
        let new = &sim.log[self.written..];
        if !new.is_empty() {
            if let Err(e) = file.write_all(&chunks_within(new, MAX_CHUNK_BYTES as usize)) {
                log::warn!(
                    "Recording {} ended: write error ({e}); the match continues",
                    self.path.display()
                );
                self.file = None;
                return;
            }
            self.written = sim.log.len();
            self.last_step = new.last().map(|c| c.step);
        }
        self.synced_step = sim.state.steps;
        if !sim.log_complete() {
            log::info!(
                "Recording {} ended after eight hours (tick {MAX_REPLAY_STEPS})",
                self.path.display()
            );
            self.file = None;
        }
    }

    /// Path of the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The recording has ended (eight hours, write error, sync); the file counts up to the last chunk.
    pub fn ended(&self) -> bool {
        self.file.is_none()
    }

    /// Tick of the last written command; `None` without a command in the file. Serves the tests and check scripts; the
    /// control channel reports `synced_step`.
    pub fn last_step(&self) -> Option<u64> {
        self.last_step
    }

    /// Tick of the simulation at the last successful catch-up: the file extends up to there
    /// (`ui_state.game.replay.step` while recording). After the end of the recording the last value stays.
    pub fn synced_step(&self) -> u64 {
        self.synced_step
    }
}

#[cfg(test)]
mod tests;
