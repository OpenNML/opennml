//! Audio files and tables of the original installation. Skipped if the installation
//! is missing.

use glam::Vec3;
use opennml_audio::db::SoundDb;
use opennml_audio::decode::{OggStream, PcmStream, parse_wav};
use opennml_audio::engine::AudioEngine;
use opennml_audio::music::MusicStatus;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_rda::{Vfs, normalize_path};
use opennml_world::environment::{GameMode, MusicAction, level_start_environment};

fn open() -> Option<Vfs> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    Some(Vfs::open_game_dir(&dir).unwrap())
}

#[test]
fn music_streams_report_length_and_seek_reproducibly() {
    let Some(vfs) = open() else { return };
    let file = vfs.loose_file("data/music/wid.ogg").unwrap();
    let mut stream = OggStream::open(Box::new(std::fs::File::open(&file).unwrap())).unwrap();
    assert_eq!(stream.channels(), 2);
    let length = stream.length_ms().unwrap();
    assert!(length > 300_000, "{length}");
    let mut first = vec![0i16; 44_100 * 2];
    assert_eq!(stream.read(&mut first), 44_100);
    assert_eq!(stream.position_ms(), 1000);
    stream.seek_ms(60_000).unwrap();
    assert_eq!(stream.position_ms(), 60_000);
    let mut a = vec![0i16; 4410 * 2];
    stream.read(&mut a);
    stream.seek_ms(60_000).unwrap();
    let mut b = vec![0i16; 4410 * 2];
    stream.read(&mut b);
    assert_eq!(a, b, "the same seek yields the same samples");
    assert!(a.iter().any(|&s| s != 0));
    stream.seek_ms(0).unwrap();
    let mut again = vec![0i16; 44_100 * 2];
    stream.read(&mut again);
    assert_eq!(again, first);
    // At the end the stream delivers fewer frames than requested.
    stream.seek_ms(length - 500).unwrap();
    let mut tail = vec![0i16; 44_100 * 2];
    assert!(stream.read(&mut tail) < 44_100);
}

#[test]
fn ogg_stream_still_plays_through_the_trait() {
    let Some(vfs) = open() else { return };
    let file = vfs.loose_file("data/music/wid.ogg").unwrap();
    let mut direct = OggStream::open(Box::new(std::fs::File::open(&file).unwrap())).unwrap();
    let mut stream: Box<dyn PcmStream> =
        Box::new(OggStream::open(Box::new(std::fs::File::open(&file).unwrap())).unwrap());
    assert_eq!(stream.channels(), 2);
    assert_eq!(stream.length_ms(), direct.length_ms());
    let mut a = vec![0i16; 4410 * 2];
    let mut b = vec![0i16; 4410 * 2];
    assert_eq!(stream.read(&mut a), 4410);
    assert_eq!(direct.read(&mut b), 4410);
    assert_eq!(a, b);
    assert_eq!(stream.position_ms(), 100);
    // Loop: jump back to the start like `FSOUND_Stream_SetTime(0)`.
    assert!(stream.rewind());
    assert_eq!(stream.position_ms(), 0);
    stream.read(&mut b);
    assert_eq!(a, b, "the same samples after seeking back");
}

/// Sample rate from the Vorbis identification header (`\x01vorbis`, version, channels, rate LE) at the file start.
fn vorbis_rate(bytes: &[u8]) -> Option<u32> {
    let head = &bytes[..bytes.len().min(256)];
    let at = head.windows(7).position(|w| w == b"\x01vorbis")?;
    let rate = head.get(at + 12..at + 16)?;
    Some(u32::from_le_bytes(rate.try_into().ok()?))
}

#[test]
fn speech_with_another_sample_rate_is_converted_like_fmod() {
    // FMOD converts every stream to the output rate (`FSOUND_Stream_Open…`); the German patch brings speech at
    // 48,000 Hz (`data/sprache/ind-1.ogg`). The English installation has no such file: then skipped.
    let Some(vfs) = open() else { return };
    let found = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.starts_with("data/sprache/") && p.ends_with(".ogg"))
        .find_map(|p| {
            let bytes = vfs.read(&p).ok()?;
            let rate = vorbis_rate(&bytes)?;
            (rate != 44_100).then_some((p, bytes, rate))
        });
    let Some((path, bytes, rate)) = found else {
        eprintln!("no speech file with another sample rate - skipped");
        return;
    };
    let mut stream = OggStream::open(Box::new(std::io::Cursor::new(bytes)))
        .unwrap_or_else(|e| panic!("{path} ({rate} Hz): {e:#}"));
    let length = stream.length_ms().expect("length from the container");
    assert!(length > 500, "{path}: {length} ms");
    let channels = stream.channels();
    let mut all = Vec::new();
    let mut buf = vec![0i16; 4096 * channels];
    loop {
        let got = stream.read(&mut buf);
        all.extend_from_slice(&buf[..got * channels]);
        if got < 4096 {
            break;
        }
    }
    // Length in frames ~ duration * 44,100 (the duration is truncated to whole ms).
    let frames = (all.len() / channels) as u64;
    let expected = length * 441 / 10;
    assert!(
        frames.abs_diff(expected) <= 50,
        "{path} ({rate} Hz): {frames} frames instead of ~ {expected}"
    );
    assert!(
        stream.position_ms().abs_diff(length) <= 1,
        "{path}: position {} at the end, length {length}",
        stream.position_ms()
    );
    assert!(
        all.iter().any(|s| s.unsigned_abs() > 1000),
        "{path}: silent"
    );
    // Loop: after seeking back the same start, position 0 again.
    assert!(stream.rewind());
    assert_eq!(stream.position_ms(), 0);
    let mut again = vec![0i16; 4410 * channels];
    assert_eq!(stream.read(&mut again), 4410);
    assert_eq!(
        again[..],
        all[..4410 * channels],
        "{path}: start after rewind"
    );
    // Position in source time: 4410 output frames are 100 ms.
    assert!(
        stream.position_ms().abs_diff(100) <= 1,
        "{}",
        stream.position_ms()
    );
}

#[test]
fn every_database_sound_and_every_start_track_resolves() {
    let Some(vfs) = open() else { return };
    let db = SoundDb::load(&vfs);
    // All sounds of the database are WAV mono 44.1 kHz (apart from the known missing files).
    let (mut ok, mut missing) = (0, 0);
    for entry in db.sounds.values() {
        match vfs.read(&entry.path) {
            Ok(bytes) => {
                let pcm = parse_wav(&bytes).unwrap_or_else(|e| panic!("{}: {e:#}", entry.path));
                assert_eq!(pcm.channels, 1, "{}", entry.path);
                ok += 1;
            }
            Err(_) => missing += 1,
        }
    }
    // The German patch supplies the 11 missing hero voice lines of the English edition.
    let expected = match game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) {
        GameVersion::English => (1159, 23),
        GameVersion::German => (1170, 12),
    };
    assert_eq!((ok, missing), expected);
    // Every music action of the start table names a track from sound.txt that is present as a loose file.
    let mut levels: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".lvl"))
        .collect();
    levels.dedup();
    let mut actions = 0;
    for lvl in &levels {
        for action in level_start_environment(lvl).music {
            let id = match *action {
                MusicAction::Play(id) | MusicAction::Attack(id) => id,
                MusicAction::AttackStartPoints(_) => continue,
            };
            let path = db
                .tracks
                .get(&id)
                .unwrap_or_else(|| panic!("{lvl}: ID {id}"));
            assert!(vfs.loose_file(path).is_some(), "{lvl}: {path}");
            actions += 1;
        }
    }
    assert_eq!(actions, 42);
}

#[test]
fn level_music_fades_in_and_surf_is_audible_near_the_listener() {
    let Some(vfs) = open() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut engine = AudioEngine::open(&vfs, false);
    let start = level_start_environment("data/levels/kampagne1/kamp1-1/kamp1-1.lvl");
    engine.start_level(&vfs, GameMode::Campaign, Some(2), start.music);
    assert_eq!(engine.music.status, MusicStatus::Normal);
    assert_eq!(engine.music.playing(), Some("data/Music/atmo_karibik.ogg"));
    engine
        .sound
        .set_listener(Vec3::ZERO, 0.0, Vec3::new(0.0, 500.0, -500.0));
    engine.start_capture(&dir.path().join("k.wav")).unwrap();
    for _ in 0..30 {
        engine.advance(&vfs, 100.0);
    }
    let music = engine.stop_capture().unwrap().unwrap();
    // Fade-in over 2 s: the first 100 ms are quieter than the windows after the fade.
    let early = music.envelope[0][0];
    let late = music.envelope[25][0];
    assert!(early < late / 4.0, "{early} / {late}");
    // Surf (4104) two tiles to the right of the listener.
    let mut surf = AudioEngine::open(&vfs, false);
    surf.sound
        .set_listener(Vec3::ZERO, 0.0, Vec3::new(0.0, 500.0, -500.0));
    surf.start_capture(&dir.path().join("s.wav")).unwrap();
    surf.play_surf(&vfs, 4104, Vec3::new(256.0, 0.0, 0.0));
    for _ in 0..10 {
        surf.advance(&vfs, 100.0);
    }
    let s = surf.stop_capture().unwrap().unwrap();
    assert!(
        s.rms[1] > s.rms[0] * 2.0,
        "louder on the right: {:?}",
        s.rms
    );
}

#[test]
fn normal_music_resumes_at_the_remembered_position_after_attack_music() {
    let Some(vfs) = open() else { return };
    let mut engine = AudioEngine::open(&vfs, false);
    let start = level_start_environment("data/levels/kampagne1/kamp1-1/kamp1-1.lvl");
    engine.start_level(&vfs, GameMode::Campaign, Some(2), start.music);
    assert_eq!(engine.music.status, MusicStatus::Normal);
    let normal = engine.music.current.clone();
    let attack = engine.music.attack.clone();
    assert!(
        !attack.is_empty() && attack != normal,
        "{attack} / {normal}"
    );
    // The level music runs for a while (steps of 100 ms; the fade is over after 2 s).
    for _ in 0..100 {
        engine.advance(&vfs, 100.0);
    }
    let playing = engine.music.handle.unwrap();
    assert_eq!(engine.music.playing(), Some(normal.as_str()));
    assert!(engine.sound.position_ms(playing) >= 9_000);
    // Combat music on: after the start time the state machine switches and remembers the position.
    engine.set_attack_override(true);
    let mut steps = 0;
    while engine.music.status != MusicStatus::Attack {
        engine.advance(&vfs, 100.0);
        steps += 1;
        assert!(steps < 100, "combat music does not start");
    }
    let remembered = engine.music.position_normal;
    assert!(remembered >= 9_000, "{remembered}");
    assert_eq!(engine.music.playing(), Some(attack.as_str()));
    // Without points the combat music ends after ATTACKSTOPTIME (5 s) with the fade.
    engine.set_attack_override(false);
    steps = 0;
    while engine.music.status != MusicStatus::Normal {
        engine.advance(&vfs, 100.0);
        steps += 1;
        assert!(steps < 200, "combat music does not end");
    }
    assert_eq!(engine.music.playing(), Some(normal.as_str()));
    // The normal music continues at the remembered point, not at 0. Tolerance: the stream
    // seeks to the exact millisecond (`seek_ms`), after which the step in which the
    // state machine switches still mixes up to 100 ms (plus a mix block of 1024 frames ~ 24 ms).
    let resumed = engine.sound.position_ms(engine.music.handle.unwrap());
    assert!(
        (remembered..=remembered + 150).contains(&resumed),
        "remembered {remembered} ms, resumed at {resumed} ms"
    );
}

/// The travois 1717 has no selection sound in the sound database (action 100, only
/// 104), so the selection stays silent in the original (re/audio, section "Acknowledgments"). For comparison,
/// PRI units and the dwelling tent have an action 100. Assertions only, no original data.
#[test]
fn the_travois_has_no_selection_sound_in_the_database() {
    let Some(vfs) = open() else { return };
    let db = SoundDb::load(&vfs);
    let mut rng = opennml_world::rand::MsvcRand::new(1);
    assert_eq!(
        db.actions(1717),
        vec![104],
        "travois: only \"unit created\""
    );
    assert_eq!(db.pick(1717, 100, -1, &mut rng), None);
    assert!(db.pick(1717, 104, -1, &mut rng).is_some());
    for guid in [1601, 1700, 1702, 1716] {
        assert!(
            db.pick(guid, 100, -1, &mut rng).is_some(),
            "{guid} has a selection sound"
        );
    }
}
