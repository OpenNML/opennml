//! Speech lengths of the speech files: the Ogg length reader of the simulation (`nml_formats::ogg`) against the
//! decoder (`OggStream`) and the length according to FMOD. Skipped if the installation is missing.

use nml_audio::decode::{OggStream, PcmStream};
use nml_formats::ogg::{fmod_stream_length_ms, ogg_info};
use nml_rda::Vfs;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};

#[test]
fn speech_lengths_match_the_decoder() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap();

    // All `.ogg` files directly in the folder `data/sprache` (archives, sorted by path).
    let paths: Vec<String> = vfs
        .files()
        .map(|(_, entry)| entry.path.clone())
        .filter(|path| {
            let lower = path.to_lowercase();
            lower
                .strip_prefix("data/sprache/")
                .is_some_and(|rest| rest.ends_with(".ogg") && !rest.contains('/'))
        })
        .collect();
    if version == GameVersion::German {
        // Measured on the reference: 695 (686 with a text ID as name, 9 others). Only 566 of them are
        // dialog text IDs with a speech file (re/campaign-ui); the folder holds more files.
        assert_eq!(paths.len(), 695, "number of speech Ogg files (reference)");
    } else {
        assert!(!paths.is_empty(), "no speech Ogg files found");
    }

    let mut fmod_differs = 0usize;
    for path in &paths {
        let bytes = vfs.read(path).unwrap();
        let info = ogg_info(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(info.channels <= 2, "{path}: {} channels", info.channels);
        assert!(info.rate > 0 && info.pcm_total > 0, "{path}: {info:?}");
        let stream = OggStream::open(Box::new(std::io::Cursor::new(bytes))).unwrap();
        let exact = info.pcm_total as u64 * 1000 / u64::from(info.rate);
        assert_eq!(
            Some(exact),
            stream.length_ms(),
            "{path}: length against the decoder ({info:?})"
        );
        if u64::from(fmod_stream_length_ms(&info)) != exact {
            fmod_differs += 1;
        }
    }
    eprintln!(
        "{} speech files, FMOD length differs from pcm_total*1000/rate: {fmod_differs}",
        paths.len()
    );
}
