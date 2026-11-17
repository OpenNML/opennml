use super::*;
use crate::game::sim_host::plain_testkit;
use nml_sim::command::{Command, CommandKind};

fn head() -> ReplayHead {
    let host = plain_testkit::host();
    ReplayHead {
        start: host.replay().unwrap().start,
        meta: ReplayMeta {
            level_info: LevelInfoData {
                name: "Test map".into(),
                path: "data/levels/test.lvl".into(),
                players: 2,
                ..LevelInfoData::default()
            },
            game_type: 1,
            lock_teams: true,
            game_speed: 14,
        },
    }
}

fn cmd(step: u64) -> RecordedCommand {
    RecordedCommand {
        step,
        command: Command {
            player: 0,
            kind: CommandKind::Diplomacy {
                p: 0,
                q: 1,
                mode: 0,
            },
        },
    }
}

fn chunks() -> [Vec<RecordedCommand>; 3] {
    [
        vec![cmd(0), cmd(3)],
        vec![cmd(3), cmd(7), cmd(9)],
        vec![cmd(10), cmd(11)],
    ]
}

fn file_bytes(chunks: &[Vec<RecordedCommand>]) -> Vec<u8> {
    let mut bytes = head_bytes(&head());
    for c in chunks {
        bytes.extend(chunk_bytes(c));
    }
    bytes
}

#[test]
fn head_and_chunks_round_trip() {
    let chunks = chunks();
    let file = ReplayFile::from_bytes(&file_bytes(&chunks)).unwrap();
    assert_eq!(file.head, head());
    assert_eq!(file.commands, chunks.concat());
    assert!(!file.torn);
    let replay = file.replay();
    assert_eq!(replay.start, head().start);
    assert_eq!(replay.commands.len(), 7);
}

// QUIRK E-337: deviation
#[test]
fn torn_tail_keeps_the_whole_chunks() {
    let chunks = chunks();
    let whole = file_bytes(&chunks);
    let last = chunk_bytes(&chunks[2]).len();
    let before_last = whole.len() - last;
    let kept = [chunks[0].clone(), chunks[1].clone()].concat();
    for cut in before_last + 1..whole.len() {
        let file = ReplayFile::from_bytes(&whole[..cut]).unwrap();
        assert_eq!(file.commands, kept, "cut at {cut}");
        assert!(file.torn, "cut at {cut}");
    }
    // Exactly at a chunk boundary the file is not torn.
    let file = ReplayFile::from_bytes(&whole[..before_last]).unwrap();
    assert_eq!(file.commands, kept);
    assert!(!file.torn);
}

// QUIRK E-337: deviation
#[test]
fn broken_chunk_checksum_ends_the_reading() {
    let chunks = chunks();
    let mut bytes = head_bytes(&head());
    bytes.extend(chunk_bytes(&chunks[0]));
    let second = bytes.len();
    bytes.extend(chunk_bytes(&chunks[1]));
    bytes.extend(chunk_bytes(&chunks[2]));
    bytes[second + 4] ^= 0xFF;
    let file = ReplayFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.commands, chunks[0]);
    assert!(file.torn);
}

#[test]
fn chunk_length_over_the_limit_is_torn() {
    let chunks = chunks();
    let mut bytes = file_bytes(&chunks[..1]);
    bytes.extend((MAX_CHUNK_BYTES + 1).to_le_bytes());
    bytes.extend(0u64.to_le_bytes());
    bytes.extend([0u8; 16]);
    let file = ReplayFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.commands, chunks[0]);
    assert!(file.torn);
}

#[test]
fn wrong_magic_is_no_recording() {
    let mut bytes = file_bytes(&chunks());
    bytes[0] = b'X';
    let err = ReplayFile::from_bytes(&bytes).unwrap_err().to_string();
    assert!(err.contains("not a recording"), "{err}");
    let err = ReplayFile::from_bytes(b"NML").unwrap_err().to_string();
    assert!(err.contains("not a recording"), "{err}");
}

#[test]
fn other_file_format_is_refused() {
    let mut bytes = file_bytes(&chunks());
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    let err = ReplayFile::from_bytes(&bytes).unwrap_err().to_string();
    assert!(err.contains("version 2, expected 1"), "{err}");
}

#[test]
fn other_format_version_is_refused_in_german() {
    let mut bytes = file_bytes(&chunks());
    bytes[12..16].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    let err = ReplayFile::from_bytes(&bytes).unwrap_err().to_string();
    assert!(
        err.contains("format version") && err.contains("different simulation behavior"),
        "{err}"
    );
}

#[test]
fn broken_head_is_refused() {
    let mut bytes = file_bytes(&chunks());
    bytes[FIXED_HEAD_LEN + 3] ^= 0xFF;
    let err = ReplayFile::from_bytes(&bytes).unwrap_err().to_string();
    assert!(err.contains("recording corrupted (header"), "{err}");
    let err = ReplayFile::from_bytes(&bytes[..FIXED_HEAD_LEN + 2])
        .unwrap_err()
        .to_string();
    assert!(err.contains("header incomplete"), "{err}");
}

#[test]
fn read_any_replay_reads_both_formats() {
    let chunks = chunks();
    let auto = file_bytes(&chunks);
    let (replay, meta, torn) = read_any_replay(&auto).unwrap();
    assert_eq!(replay.commands, chunks.concat());
    assert_eq!(meta, Some(head().meta));
    assert!(!torn);
    let (replay, meta, torn) = read_any_replay(&auto[..auto.len() - 1]).unwrap();
    assert_eq!(replay.commands.len(), 5);
    assert!(meta.is_some() && torn);

    let old = Replay {
        start: head().start,
        commands: chunks.concat(),
    };
    let (read, meta, torn) = read_any_replay(&old.to_bytes()).unwrap();
    assert_eq!(read, old);
    assert_eq!((meta, torn), (None, false));
    assert!(read_any_replay(b"anything").is_err());
}

// ---- Recording ------------------------------------------------------------------------------------------------------

/// `Diplomacy` command of slot 0 (has no effect on the flow of the tests).
fn diplomacy() -> Command {
    Command {
        player: 0,
        kind: CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 0,
        },
    }
}

/// Reads the recording of the recorder.
fn read(recorder: &ReplayRecorder) -> ReplayFile {
    ReplayFile::from_bytes(&std::fs::read(recorder.path()).unwrap()).unwrap()
}

#[test]
fn recorder_writes_head_the_log_so_far_and_new_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("savegame").join("autoplay");
    let mut host = plain_testkit::host();
    host.execute_now(diplomacy());
    host.step_with(vec![diplomacy()]);
    let mut recorder = ReplayRecorder::begin(&dir, "261108-1435", &head(), &host).unwrap();
    assert_eq!(recorder.path(), dir.join("261108-1435"));
    assert!(!recorder.ended());
    assert_eq!(recorder.last_step(), Some(0));
    let file = read(&recorder);
    assert_eq!(file.head, head());
    assert_eq!(file.commands, host.log, "the log so far as the first chunk");
    // Without new commands `sync` writes nothing.
    let len = std::fs::metadata(recorder.path()).unwrap().len();
    recorder.sync(&host);
    assert_eq!(std::fs::metadata(recorder.path()).unwrap().len(), len);
    for _ in 0..5 {
        host.step_with(Vec::new());
    }
    host.step_with(vec![diplomacy(), diplomacy()]);
    recorder.sync(&host);
    assert_eq!(recorder.last_step(), Some(6));
    let file = read(&recorder);
    assert_eq!(file.commands, host.log);
    assert!(!file.torn);
    // A new recorder of the same name overwrites the file without asking.
    let fresh = plain_testkit::host();
    let recorder = ReplayRecorder::begin(&dir, "261108-1435", &head(), &fresh).unwrap();
    assert_eq!(recorder.last_step(), None);
    assert!(read(&recorder).commands.is_empty());
}

// QUIRK E-337: deviation
#[test]
fn recorder_splits_a_long_log_into_chunks_within_the_limit() {
    // A whole log after loading can exceed `MAX_CHUNK_BYTES`; the recorder splits it into chunks.
    let commands: Vec<RecordedCommand> = (0..100).map(cmd).collect();
    let one = chunk_bytes(&commands[..1]).len() - CHUNK_HEAD_LEN;
    let limit = 4 * one + 3;
    let bytes = chunks_within(&commands, limit);
    let mut rest = &bytes[..];
    let mut read_back = Vec::new();
    let mut count = 0;
    while !rest.is_empty() {
        let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        assert!(len <= limit, "chunk {count}: {len} > {limit}");
        let (chunk, used) = read_chunk(rest).expect("whole chunk");
        read_back.extend(chunk);
        rest = &rest[used..];
        count += 1;
    }
    assert_eq!(read_back, commands);
    assert!(count >= 25, "split: {count} chunks");
    // Under the limit it stays one chunk.
    assert_eq!(chunks_within(&commands, 1 << 20), chunk_bytes(&commands));
    assert!(chunks_within(&[], 1 << 20).is_empty());
}

#[test]
fn recorder_ends_when_the_log_gets_shorter() {
    // A network resync (`SimHost::adopt`) replaces the log; if it becomes shorter than what was written, the
    // recording ends (warning) instead of writing commands twice or wrongly.
    let tmp = tempfile::tempdir().unwrap();
    let mut host = plain_testkit::host();
    host.step_with(vec![diplomacy(), diplomacy()]);
    let mut recorder = ReplayRecorder::begin(tmp.path(), "a", &head(), &host).unwrap();
    host.log.pop();
    crate::testutil::capture_warnings();
    recorder.sync(&host);
    let warnings = crate::testutil::take_warnings();
    assert!(recorder.ended());
    assert!(
        warnings.iter().any(|w| w.contains("shorter")),
        "{warnings:?}"
    );
    host.step_with(vec![diplomacy()]);
    recorder.sync(&host);
    assert_eq!(
        read(&recorder).commands.len(),
        2,
        "nothing more after the end"
    );
}

// QUIRK E-335: deviation
#[test]
fn recorder_write_error_ends_the_recording() {
    // A write error ends the recording with a warning; the match (the host) stays untouched.
    let tmp = tempfile::tempdir().unwrap();
    let mut host = plain_testkit::host();
    let mut recorder = ReplayRecorder::begin(tmp.path(), "a", &head(), &host).unwrap();
    // Opened read-only: every write fails.
    recorder.file = Some(std::fs::File::open(recorder.path()).unwrap());
    host.step_with(vec![diplomacy()]);
    let before = host.checksum();
    crate::testutil::capture_warnings();
    recorder.sync(&host);
    let warnings = crate::testutil::take_warnings();
    assert!(recorder.ended());
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("Recording") && w.contains("ended")),
        "{warnings:?}"
    );
    assert_eq!(host.checksum(), before);
    assert_eq!(recorder.last_step(), None);
}

#[test]
fn recorder_refuses_a_match_without_complete_log() {
    let tmp = tempfile::tempdir().unwrap();
    let mut host = plain_testkit::host();
    let save = host.save();
    host.load(save, None);
    let err = ReplayRecorder::begin(tmp.path(), "a", &head(), &host)
        .unwrap_err()
        .to_string();
    assert!(err.contains("without a command log"), "{err}");
    assert!(!tmp.path().join("a").exists());
}
