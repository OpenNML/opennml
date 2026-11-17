//! Videos of the game: Bink files from `DATA/Videos`, loose in the installation (`Vfs::loose_file`, case-insensitive;
//! the original lets `_BinkOpen` open the file itself, re/video, section "Opening" 3). The menu host opens them through
//! a [`VideoOpener`] (default [`open_bink`]) as a [`VideoFile`]: header, frame k as RGBA ([`BinkVideoFile`] decodes in
//! sequence and converts only the shown frame) and the sound track as a stream for the mixer
//! ([`bink_track_stream`]).

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use image::RgbaImage;
use nml_audio::decode::PcmStream;
use nml_audio::resample::{ResampledStream, SampleSource};
use nml_bink::{AudioDecoder, BinkReader, VideoDecoder, to_rgba};
use nml_rda::Vfs;
use nml_ui::video::VideoInfo;

/// An opened video from the point of view of the menu host (tests take a fake).
pub trait VideoFile {
    /// Header data (frame size, frame count, frame rate).
    fn info(&self) -> VideoInfo;
    /// Frame `k` as RGBA. Frames come only in sequence (`k` never smaller than in the previous call); an error comes
    /// as an error message with the file name.
    fn image_at(&mut self, k: u32) -> Result<&RgbaImage, String>;
    /// Sound track 0 as a stream for the mixer; `None` without a sound track.
    fn track(&mut self) -> Option<Result<Box<dyn PcmStream>, String>>;
}

/// Opens a video by the path of the command (`data/videos/<name>.bik`).
pub type VideoOpener = Box<dyn Fn(&Vfs, &str) -> Result<Box<dyn VideoFile>, String>>;

/// A Bink file of the installation: container, frame decoder and RGBA buffer of the last converted frame. Because
/// there is only one key frame, [`VideoFile::image_at`] decodes all intermediate frames on clock jumps but converts
/// only the requested one (deviation from the original).
pub struct BinkVideoFile {
    /// File name for messages.
    name: String,
    /// Location of the file (for the own file access of the sound track).
    path: PathBuf,
    reader: BinkReader<BufReader<File>>,
    decoder: VideoDecoder,
    /// Number of frames decoded so far (= number of the next).
    decoded: u32,
    /// Frame that is currently in `rgba`.
    converted: Option<u32>,
    /// Number of conversions to RGBA.
    conversions: u32,
    rgba: RgbaImage,
}

impl BinkVideoFile {
    /// Opens `path` (`data/videos/<name>.bik`) as a loose file of the installation. If it is missing: "Video ... is
    /// missing from the installation"; errors of container and decoder with the file name.
    pub fn open(vfs: &Vfs, path: &str) -> Result<Self, String> {
        let file_path = vfs
            .loose_file(path)
            .ok_or_else(|| format!("Video {path} is missing from the installation"))?;
        let name = file_name(&file_path);
        let file = File::open(&file_path)
            .map_err(|e| format!("Video {name}: file cannot be opened ({e})"))?;
        let reader =
            BinkReader::open(BufReader::new(file)).map_err(|e| format!("Video {name}: {e}"))?;
        let decoder =
            VideoDecoder::new(&reader.header).map_err(|e| format!("Video {name}: {e}"))?;
        Ok(BinkVideoFile {
            name,
            path: file_path,
            reader,
            decoder,
            decoded: 0,
            converted: None,
            conversions: 0,
            rgba: RgbaImage::new(0, 0),
        })
    }
}

impl VideoFile for BinkVideoFile {
    fn info(&self) -> VideoInfo {
        let h = &self.reader.header;
        VideoInfo {
            width: h.width,
            height: h.height,
            frames: h.frames,
            fps_num: h.fps_num,
            fps_den: h.fps_den,
        }
    }

    fn image_at(&mut self, k: u32) -> Result<&RgbaImage, String> {
        let name = &self.name;
        if self.converted == Some(k) {
            return Ok(&self.rgba);
        }
        if k < self.decoded {
            return Err(format!(
                "Video {name}: frame {k} lies before the last decoded one ({})",
                self.decoded - 1
            ));
        }
        while self.decoded <= k {
            let i = self.decoded;
            let packets = self
                .reader
                .read_frame(i)
                .map_err(|e| format!("Video {name}: frame {i}: {e}"))?;
            let frame = self
                .decoder
                .decode(&packets.video)
                .map_err(|e| format!("Video {name}: frame {i}: {e}"))?;
            self.decoded += 1;
            if i == k {
                to_rgba(frame, &mut self.rgba);
                self.converted = Some(k);
                self.conversions += 1;
            }
        }
        Ok(&self.rgba)
    }

    fn track(&mut self) -> Option<Result<Box<dyn PcmStream>, String>> {
        if self.reader.header.audio.is_empty() {
            return None;
        }
        Some(bink_track_stream(&self.path))
    }
}

/// The default opener of the menu host: [`BinkVideoFile::open`].
pub fn open_bink(vfs: &Vfs, path: &str) -> Result<Box<dyn VideoFile>, String> {
    BinkVideoFile::open(vfs, path).map(|f| Box::new(f) as Box<dyn VideoFile>)
}

/// File name for messages (otherwise the whole path).
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Sound track 0 of the Bink file `path` as a stream for the mixer: own file access (independent of the frames), the
/// packets are read and decoded in sequence on demand, the rate (22,050, 44,100 or 48,000 Hz) is converted linearly to
/// 44,100 Hz. The length is the video duration (that of the samples is unknown beforehand). If the sound track is
/// missing or the file cannot be opened, an error message with the file name results.
pub fn bink_track_stream(path: &Path) -> Result<Box<dyn PcmStream>, String> {
    track_stream_counted(path).map(|(stream, _)| stream)
}

/// Like [`bink_track_stream`], plus a counter of the frames read so far (for tests).
fn track_stream_counted(path: &Path) -> Result<(Box<dyn PcmStream>, Arc<AtomicU32>), String> {
    let name = file_name(path);
    let file =
        File::open(path).map_err(|e| format!("Video {name}: file cannot be opened ({e})"))?;
    let mut reader =
        BinkReader::open(BufReader::new(file)).map_err(|e| format!("Video {name}: {e}"))?;
    let track = *reader
        .header
        .audio
        .first()
        .ok_or_else(|| format!("Video {name}: without a sound track"))?;
    let mut decoder = AudioDecoder::new(&track).map_err(|e| format!("Video {name}: {e}"))?;
    let (rate, channels) = (decoder.sample_rate(), decoder.channels());
    let length_ms = reader.header.duration_ms();
    let frames = reader.header.frames;
    let counter = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&counter);
    let mut next = 0u32;
    // Reads video frames until a sound packet yields at least one sample frame (frames without a packet occur
    // and must not count as "something appended", see the contract of `SampleSource`); end of file or an error end
    // the stream.
    let source: SampleSource = Box::new(move |out: &mut Vec<f32>| {
        while next < frames {
            let k = next;
            next += 1;
            let packets = match reader.read_frame(k) {
                Ok(packets) => packets,
                Err(e) => {
                    log::warn!("Video {name}: frame {k} not readable, the sound ends ({e})");
                    next = frames;
                    return false;
                }
            };
            count.fetch_add(1, Ordering::Relaxed);
            let Some(Some(packet)) = packets.audio.first() else {
                continue;
            };
            let before = out.len();
            if let Err(e) = decoder.decode_packet(packet, out) {
                out.truncate(before);
                log::warn!(
                    "Video {name}: sound packet in frame {k} not decodable, the sound ends ({e})"
                );
                next = frames;
                return false;
            }
            if out.len() > before {
                return true;
            }
        }
        false
    });
    let stream = ResampledStream::new(source, rate, channels, Some(length_ms));
    Ok((Box::new(stream), counter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Writes a synthetic `.bik` (revision `i`, 640 x 356, 25 frames/s) by hand: `tracks` sound tracks
    /// `(rate, flags)` and `frames` frames, per frame a sound packet size 0 per track and 4 bytes of video data.
    fn write_bik(dir: &Path, name: &str, frames: u32, tracks: &[(u16, u16)]) -> std::path::PathBuf {
        write_bik_with_audio(dir, name, frames, tracks, |_| Vec::new())
    }

    /// Like [`write_bik`], but with sound packet `audio(k)` (payload without size field) in frame `k` of every track.
    fn write_bik_with_audio(
        dir: &Path,
        name: &str,
        frames: u32,
        tracks: &[(u16, u16)],
        audio: impl Fn(u32) -> Vec<u8>,
    ) -> std::path::PathBuf {
        let header_len = 44 + 12 * tracks.len() as u32 + 4 * (frames + 1);
        let bodies: Vec<Vec<u8>> = (0..frames)
            .map(|k| {
                let packet = audio(k);
                let mut body = Vec::new();
                for _ in tracks {
                    body.extend_from_slice(&(packet.len() as u32).to_le_bytes());
                    body.extend_from_slice(&packet);
                }
                body.extend_from_slice(&[0u8; 4]);
                body
            })
            .collect();
        let total: u32 = bodies.iter().map(|b| b.len() as u32).sum();
        let largest = bodies.iter().map(|b| b.len() as u32).max().unwrap_or(0);
        let end = header_len + total;
        let mut v = Vec::new();
        v.extend_from_slice(b"BIKi");
        for x in [
            end - 8,
            frames,
            largest,
            0,
            640,
            356,
            25,
            1,
            0,
            tracks.len() as u32,
        ] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        for _ in tracks {
            v.extend_from_slice(&0x4000u32.to_le_bytes());
        }
        for &(rate, flags) in tracks {
            v.extend_from_slice(&rate.to_le_bytes());
            v.extend_from_slice(&flags.to_le_bytes());
        }
        for id in 0..tracks.len() as u32 {
            v.extend_from_slice(&id.to_le_bytes());
        }
        let mut offset = header_len;
        for body in &bodies {
            v.extend_from_slice(&offset.to_le_bytes());
            offset += body.len() as u32;
        }
        v.extend_from_slice(&offset.to_le_bytes());
        for body in &bodies {
            v.extend_from_slice(body);
        }
        let path = dir.join(name);
        std::fs::File::create(&path).unwrap().write_all(&v).unwrap();
        path
    }

    #[test]
    fn track_stream_ends_cleanly_without_packets() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_bik(dir.path(), "empty.bik", 3, &[(44_100, 0x7000)]);
        let (mut stream, counter) = track_stream_counted(&path).unwrap();
        assert_eq!(stream.channels(), 2);
        assert_eq!(stream.length_ms(), Some(120), "3 frames of 40 ms");
        // A hang (source reports `true` without frames) must not happen: reading runs in a thread with a time limit.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0i16; 512];
            let got = stream.read(&mut buf);
            let again = stream.read(&mut buf);
            tx.send((got, again)).unwrap();
        });
        let result = rx.recv_timeout(Duration::from_secs(10)).expect("hangs");
        assert_eq!(result, (0, 0), "no frames, no error");
        assert_eq!(counter.load(Ordering::Relaxed), 3, "all frames read");
    }

    #[test]
    fn track_stream_ends_when_a_packet_is_undecodable() {
        // Frame 2 of 5 carries a sound packet of zeros (size field plus 4 bytes): too short for a block.
        let dir = tempfile::tempdir().unwrap();
        let path = write_bik_with_audio(
            dir.path(),
            "broken-sound.bik",
            5,
            &[(44_100, 0x7000)],
            |k| {
                if k == 2 { vec![0u8; 8] } else { Vec::new() }
            },
        );
        let (mut stream, counter) = track_stream_counted(&path).unwrap();
        // Reading in a thread with a time limit (a hang must not happen); the warnings are per thread.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            crate::testutil::capture_warnings();
            let mut buf = [0i16; 512];
            let got = stream.read(&mut buf);
            let again = stream.read(&mut buf);
            tx.send((got, again, crate::testutil::take_warnings()))
                .unwrap();
        });
        let (got, again, warnings) = rx.recv_timeout(Duration::from_secs(10)).expect("hangs");
        assert_eq!((got, again), (0, 0), "sound ends, also on the second read");
        assert_eq!(
            counter.load(Ordering::Relaxed),
            3,
            "read up to the broken frame 2, frames 3 and 4 no longer"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("broken-sound.bik") && warnings[0].contains("frame 2"),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("the sound ends"), "{warnings:?}");
    }

    #[test]
    fn track_stream_without_audio_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_bik(dir.path(), "silent.bik", 3, &[]);
        let err = track_stream_counted(&path).err().expect("error");
        assert!(err.contains("without a sound track"), "{err}");
        assert!(err.contains("silent.bik"), "{err}");
    }

    #[test]
    fn track_stream_rejects_missing_and_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let err = bink_track_stream(&dir.path().join("missing.bik"))
            .err()
            .expect("error");
        assert!(err.contains("missing.bik"), "{err}");
        let broken = dir.path().join("broken.bik");
        std::fs::write(&broken, b"BIKi\x01\x00").unwrap();
        let err = bink_track_stream(&broken).err().expect("error");
        assert!(err.contains("broken.bik"), "{err}");
        // RDFT sound (without the DCT flag) is already rejected by the header.
        let rdft = write_bik(dir.path(), "rdft.bik", 2, &[(44_100, 0x6000)]);
        let err = bink_track_stream(&rdft).err().expect("error");
        assert!(err.contains("rdft.bik"), "{err}");
    }

    /// Installation in a temporary directory: an empty archive and `files` loose under `DATA/Videos`.
    fn video_install(files: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, Vfs) {
        let dir = tempfile::tempdir().unwrap();
        let rda = nml_rda::testutil::build_rda(&[nml_rda::testutil::TestFile {
            path: "data/txt/empty.txt",
            data: b"",
            compress: false,
        }]);
        std::fs::write(dir.path().join("NML0.RDA"), rda).unwrap();
        let videos = dir.path().join("DATA").join("Videos");
        std::fs::create_dir_all(&videos).unwrap();
        for (name, data) in files {
            std::fs::write(videos.join(name), data).unwrap();
        }
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    #[test]
    fn open_bink_reports_missing_and_broken_files() {
        let tmp = tempfile::tempdir().unwrap();
        let stumm = std::fs::read(write_bik(tmp.path(), "x.bik", 3, &[])).unwrap();
        let laut = std::fs::read(write_bik(tmp.path(), "y.bik", 3, &[(44_100, 0x7000)])).unwrap();
        let (_dir, vfs) = video_install(&[
            ("broken.bik", b"BIKi\x01\x00".to_vec()),
            ("Silent.bik", stumm),
            ("laut.bik", laut),
        ]);
        let err = open_bink(&vfs, "data/videos/missing.bik")
            .err()
            .expect("error");
        assert_eq!(
            err,
            "Video data/videos/missing.bik is missing from the installation"
        );
        let err = open_bink(&vfs, "data/videos/broken.bik")
            .err()
            .expect("error");
        assert!(err.starts_with("Video broken.bik: "), "{err}");
        // Case does not matter; header as in the file, without a sound track no stream.
        let mut f = open_bink(&vfs, "data/videos/silent.bik").unwrap();
        assert_eq!(
            f.info(),
            VideoInfo {
                width: 640,
                height: 356,
                frames: 3,
                fps_num: 25,
                fps_den: 1
            }
        );
        assert!(f.track().is_none());
        // A frame past the end: error with file name, no panic.
        let err = f.image_at(5).expect_err("error");
        assert!(err.contains("Silent.bik"), "{err}");
        let mut f = open_bink(&vfs, "data/videos/laut.bik").unwrap();
        assert!(matches!(f.track(), Some(Ok(_))));
    }

    #[test]
    fn installation_videos_open_and_show_frame_zero() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(dir.join("DATA/Videos"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_lowercase().ends_with(".bik"))
            .collect();
        names.sort();
        assert_eq!(names.len(), 12, "{names:?}");
        let mut with_track = 0;
        for name in &names {
            let path = format!("data/videos/{}", name.to_lowercase());
            let mut f = open_bink(&vfs, &path).unwrap_or_else(|e| panic!("{e}"));
            let info = f.info();
            assert_eq!(info.width, 640, "{name}");
            assert!(
                info.frames > 0 && info.fps_num > 0 && info.fps_den > 0,
                "{name}"
            );
            let img = f.image_at(0).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(img.dimensions(), (640, info.height), "{name}");
            // The logos have no sound track; where there is one, it opens.
            match f.track() {
                Some(Ok(_)) => with_track += 1,
                Some(Err(e)) => panic!("{e}"),
                None => {}
            }
        }
        assert!(with_track > 0, "no video with a sound track");
        // Clock jump 0 -> frame 100: all intermediate frames decoded, only frames 0 and 100 converted.
        let mut f = BinkVideoFile::open(&vfs, "data/videos/NML_Logo_Related.bik").unwrap();
        f.image_at(0).unwrap();
        assert_eq!((f.decoded, f.conversions), (1, 1));
        f.image_at(100).unwrap();
        assert_eq!(
            (f.decoded, f.conversions),
            (101, 2),
            "100 frames decoded, 2 converted"
        );
        // The same frame again: nothing new; an earlier one: error with file name.
        f.image_at(100).unwrap();
        assert_eq!((f.decoded, f.conversions), (101, 2));
        let err = f.image_at(50).expect_err("error");
        assert!(err.contains("NML_Logo_Related.bik"), "{err}");
    }

    #[test]
    fn eng_track_streams_lazily_at_44100() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let path = dir.join("DATA/Videos/eng.bik");
        let (mut stream, counter) = track_stream_counted(&path).unwrap();
        let (total, expected) = {
            let reader = nml_bink::BinkReader::open(std::fs::File::open(&path).unwrap()).unwrap();
            let h = &reader.header;
            assert_eq!(h.audio[0].sample_rate, 44_100);
            // Frames that carry 2 s of sound (rounded up).
            let two_s = (2 * u64::from(h.fps_num)).div_ceil(u64::from(h.fps_den));
            (u64::from(h.frames), two_s)
        };
        assert_eq!(stream.channels(), 2);
        assert!(stream.length_ms().unwrap() > 2000);
        let mut buf = vec![0i16; 88_200 * 2];
        assert_eq!(stream.read(&mut buf), 88_200, "2 s stereo");
        assert_eq!(stream.position_ms(), 2000);
        let read = u64::from(counter.load(Ordering::Relaxed));
        // The sound is ahead of the frame in the file (Bink interleaves larger sound blocks), so fewer frames than 2 s
        // of frame time suffice for 2 s; never more than frame time + 2.
        assert!(
            (1..=expected + 2).contains(&read),
            "read: {read} of {total} frames, frame time of 2 s: {expected}"
        );
        assert!(total > 10 * expected, "only a fraction read");
    }
}
