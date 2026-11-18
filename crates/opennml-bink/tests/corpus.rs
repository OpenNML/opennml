//! Corpus tests against the original installation (`NML_GAME_DIR`; without the variable they skip silently).

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use opennml_bink::{AudioDecoder, BinkReader, VideoDecoder, YUV_HASH_BASIS, yuv_hash};

fn game_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("NML_GAME_DIR")?);
    dir.is_dir().then_some(dir)
}

/// Finds a file ignoring case (on disk `Intro.bik`).
fn find_ignore_case(dir: &Path, name: &str) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .unwrap_or_else(|| panic!("{name} missing in {}", dir.display()))
}

/// The twelve rows of the table in re/video:
/// name, width, height, frames, rate of the audio track (`None` = no audio track).
const VIDEOS: [(&str, u32, u32, u32, Option<u16>); 12] = [
    ("NML_Logo_CDV.bik", 640, 480, 452, Some(22050)),
    ("NML_Logo_Related.bik", 640, 356, 145, None),
    ("Intro.bik", 640, 356, 6248, Some(48000)),
    ("eng.bik", 640, 356, 1263, Some(44100)),
    ("ind-1.bik", 640, 356, 919, Some(44100)),
    ("ind-2.bik", 640, 356, 699, Some(44100)),
    ("ind-3.bik", 640, 356, 795, Some(44100)),
    ("pat-1.bik", 640, 356, 829, Some(48000)),
    ("pat-2.bik", 640, 356, 1043, Some(44100)),
    ("sid.bik", 640, 356, 1457, Some(44100)),
    ("spa-1.bik", 640, 356, 1014, Some(44100)),
    ("spa-2.bik", 640, 356, 802, Some(48000)),
];

#[test]
fn installation_headers_match_video_md() {
    let Some(dir) = game_dir() else { return };
    let videos = dir.join("DATA").join("Videos");
    let mut total_frames = 0u64;
    for (name, width, height, frames, rate) in VIDEOS {
        let path = find_ignore_case(&videos, name);
        let len = std::fs::metadata(&path).unwrap().len();
        let mut rd = BinkReader::open(BufReader::new(File::open(&path).unwrap()))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let h = rd.header.clone();
        assert_eq!(h.revision, b'i', "{name}");
        assert_eq!((h.width, h.height), (width, height), "{name}");
        assert_eq!(h.frames, frames, "{name}");
        assert_eq!((h.fps_num, h.fps_den), (25, 1), "{name}");
        assert_eq!(h.video_flags, 0, "{name}");
        assert_eq!(u64::from(h.file_size), len, "{name}: file size");
        match rate {
            Some(hz) => {
                assert_eq!(h.audio.len(), 1, "{name}");
                let a = &h.audio[0];
                assert_eq!(a.sample_rate, hz, "{name}");
                assert!(a.stereo() && a.bits16() && a.dct(), "{name}");
            }
            None => assert!(h.audio.is_empty(), "{name}"),
        }
        // Exactly one key frame (frame 0); the last frame ends at the end of the file.
        assert_eq!(h.index.len(), frames as usize, "{name}");
        assert_eq!(h.index.iter().filter(|e| e.keyframe).count(), 1, "{name}");
        assert!(h.index[0].keyframe, "{name}");
        let last = h.index.last().unwrap();
        assert_eq!(last.offset + u64::from(last.size), len, "{name}");
        assert_eq!(h.duration_ms(), u64::from(frames) * 40, "{name}");

        // Every frame can be split into packets (packets only, no decoding).
        for k in 0..frames {
            let p = rd
                .read_frame(k)
                .unwrap_or_else(|e| panic!("{name} frame {k}: {e}"));
            assert_eq!(p.audio.len(), h.audio.len(), "{name} frame {k}");
            assert!(!p.video.is_empty(), "{name} frame {k}");
        }
        total_frames += u64::from(frames);
    }
    assert_eq!(total_frames, 15_666);
}

/// FFmpeg reference (n9.0.2, `-pix_fmt yuv420p`): FNV-1a 64 over the cropped planes Y, U, V,
/// chained frame by frame, state after the first 250 frames (for `NML_Logo_Related` with 145 frames
/// after all of them). Measured with FFmpeg n9.0.2.
const BINK_YUV_GOLDEN: [(&str, u64); 12] = [
    ("eng.bik", 0x5699_4d30_027a_584f),
    ("ind-1.bik", 0x52f2_8887_b11c_4bca),
    ("ind-2.bik", 0x1b24_de2f_20ab_c2fa),
    ("ind-3.bik", 0x3214_dfdc_5b50_6b27),
    ("Intro.bik", 0xc76d_68a1_edfb_d117),
    ("NML_Logo_CDV.bik", 0x042b_2a61_1d55_2647),
    ("NML_Logo_Related.bik", 0x2bd4_365c_7d1a_afef),
    ("pat-1.bik", 0x2007_b3e5_83a1_2889),
    ("pat-2.bik", 0x090c_bf3a_0dcc_fd6e),
    ("sid.bik", 0xd2ca_00b7_3138_aedf),
    ("spa-1.bik", 0x6795_06c3_d0e7_0c62),
    ("spa-2.bik", 0xec85_96ee_fff6_d878),
];

#[test]
fn first_250_frames_match_ffmpeg() {
    let Some(dir) = game_dir() else { return };
    let videos = dir.join("DATA").join("Videos");
    for (name, golden) in BINK_YUV_GOLDEN {
        let path = find_ignore_case(&videos, name);
        let mut rd = BinkReader::open(BufReader::new(File::open(&path).unwrap()))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut dec = VideoDecoder::new(&rd.header).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut hash = YUV_HASH_BASIS;
        for k in 0..rd.header.frames.min(250) {
            let p = rd
                .read_frame(k)
                .unwrap_or_else(|e| panic!("{name} frame {k}: {e}"));
            let f = dec
                .decode(&p.video)
                .unwrap_or_else(|e| panic!("{name} frame {k}: {e}"));
            yuv_hash(f, &mut hash);
        }
        assert_eq!(hash, golden, "{name}: {hash:016x} instead of {golden:016x}");
    }
}

/// Real packets with a flipped bit or truncated (fixed random generator): only `Ok` or `Err`,
/// never a panic; the real packets in between continue to decode.
#[test]
fn corrupted_real_packets_never_panic() {
    let Some(dir) = game_dir() else { return };
    let path = find_ignore_case(&dir.join("DATA").join("Videos"), "eng.bik");
    let mut rd = BinkReader::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let mut dec = VideoDecoder::new(&rd.header).unwrap();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut errors = 0;
    for k in 0..40 {
        let video = rd.read_frame(k).unwrap().video;
        for _ in 0..20 {
            let mut p = video.clone();
            if next() % 2 == 0 {
                let bit = (next() % (p.len() as u64 * 8)) as usize;
                p[bit / 8] ^= 1 << (bit % 8);
            } else {
                p.truncate((next() % p.len() as u64) as usize);
            }
            errors += usize::from(dec.decode(&p).is_err());
        }
        dec.decode(&video).unwrap();
    }
    assert!(errors > 0);
}

/// Samples per channel of the audio track (all stereo), measured with FFmpeg
/// n9.0.2 (`-map 0:a:0 -f f32le`). `NML_Logo_Related` has no sound.
const AUDIO_SAMPLES: [(&str, u32, usize); 11] = [
    ("eng.bik", 44100, 2_229_120),
    ("ind-1.bik", 44100, 1_622_400),
    ("ind-2.bik", 44100, 1_234_560),
    ("ind-3.bik", 44100, 1_403_520),
    ("Intro.bik", 48000, 11_996_160),
    ("NML_Logo_CDV.bik", 22050, 399_360),
    ("pat-1.bik", 48000, 1_591_680),
    ("pat-2.bik", 44100, 1_841_280),
    ("sid.bik", 44100, 2_570_880),
    ("spa-1.bik", 44100, 1_789_440),
    ("spa-2.bik", 48000, 1_539_840),
];

/// All the sound of every video decodes without errors to the length of the FFmpeg reference. The values are
/// finite and not all 0. They stay below 1.25 in magnitude, but not within [-1, 1]: as in FFmpeg
/// (also unclamped there), `NML_Logo_CDV` (up to 1.145), `eng` (1.099) and `ind-1` (1.010)
/// exceed 1 in individual places.
#[test]
fn audio_tracks_decode_with_expected_length() {
    let Some(dir) = game_dir() else { return };
    let videos = dir.join("DATA").join("Videos");
    for (name, rate, per_channel) in AUDIO_SAMPLES {
        let path = find_ignore_case(&videos, name);
        let mut rd = BinkReader::open(BufReader::new(File::open(&path).unwrap()))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut dec =
            AudioDecoder::new(&rd.header.audio[0]).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(dec.sample_rate(), rate, "{name}");
        assert_eq!(dec.channels(), 2, "{name}");
        let mut out = Vec::with_capacity(per_channel * 2);
        for k in 0..rd.header.frames {
            let p = rd
                .read_frame(k)
                .unwrap_or_else(|e| panic!("{name} frame {k}: {e}"));
            if let Some(packet) = &p.audio[0] {
                dec.decode_packet(packet, &mut out)
                    .unwrap_or_else(|e| panic!("{name} frame {k}: {e}"));
            }
        }
        assert_eq!(out.len(), per_channel * 2, "{name}");
        assert!(
            out.iter().all(|v| v.is_finite() && v.abs() < 1.25),
            "{name}"
        );
        assert!(out.iter().any(|&v| v != 0.0), "{name}");
    }
}
