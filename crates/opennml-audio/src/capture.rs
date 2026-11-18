//! Offline mix: writes the mixed samples as WAV (PCM 16-bit, stereo, 44,100 Hz) and tracks
//! levels (peak, RMS, RMS envelope in 100 ms windows). The same steps produce bit-identical
//! files.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::SAMPLE_RATE;

/// Frames per envelope window (100 ms).
pub const ENVELOPE_FRAMES: usize = SAMPLE_RATE as usize / 10;

/// Result of a capture; levels relative to full scale (32768 = 1).
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureSummary {
    pub path: PathBuf,
    pub frames: u64,
    pub length_ms: f64,
    /// Peak level left/right.
    pub peak: [f32; 2],
    /// RMS over the whole capture left/right.
    pub rms: [f32; 2],
    /// RMS per 100 ms window left/right (the last window may be shorter).
    pub envelope: Vec<[f32; 2]>,
}

/// Level statistics over interleaved stereo frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Levels {
    frames: u64,
    peak: [i32; 2],
    sum: [f64; 2],
    window_sum: [f64; 2],
    window_frames: usize,
    envelope: Vec<[f32; 2]>,
}

impl Levels {
    pub fn add(&mut self, stereo: &[i16]) {
        for frame in stereo.chunks_exact(2) {
            for (c, &sample) in frame.iter().enumerate() {
                let s = i32::from(sample);
                self.peak[c] = self.peak[c].max(s.abs());
                let sq = f64::from(s) * f64::from(s);
                self.sum[c] += sq;
                self.window_sum[c] += sq;
            }
            self.frames += 1;
            self.window_frames += 1;
            if self.window_frames == ENVELOPE_FRAMES {
                self.close_window();
            }
        }
    }

    fn close_window(&mut self) {
        let n = self.window_frames as f64;
        self.envelope
            .push(self.window_sum.map(|s| ((s / n).sqrt() / 32768.0) as f32));
        self.window_sum = [0.0; 2];
        self.window_frames = 0;
    }

    /// Completion: the numbers of the whole capture.
    pub fn finish(mut self, path: PathBuf) -> CaptureSummary {
        if self.window_frames > 0 {
            self.close_window();
        }
        let n = self.frames.max(1) as f64;
        CaptureSummary {
            path,
            frames: self.frames,
            length_ms: self.frames as f64 * 1000.0 / f64::from(SAMPLE_RATE),
            peak: self.peak.map(|p| p as f32 / 32768.0),
            rms: self.sum.map(|s| ((s / n).sqrt() / 32768.0) as f32),
            envelope: self.envelope,
        }
    }
}

/// Running capture into a WAV file.
pub struct Capture {
    path: PathBuf,
    /// `None` after completion (header written, file closed).
    file: Option<BufWriter<File>>,
    levels: Levels,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("path", &self.path)
            .field("frames", &self.levels.frames)
            .finish_non_exhaustive()
    }
}

/// WAV header for `frames` stereo frames of PCM 16-bit.
fn header(frames: u64) -> [u8; 44] {
    let data = (frames * 4).min(u64::from(u32::MAX - 36)) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

impl Capture {
    /// Creates the file (overwrites an existing one).
    pub fn create(path: &Path) -> Result<Self> {
        let mut file = BufWriter::new(
            File::create(path).with_context(|| format!("creating {}", path.display()))?,
        );
        file.write_all(&header(0))?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            levels: Levels::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.levels.frames
    }

    /// Appends interleaved stereo samples.
    pub fn write(&mut self, stereo: &[i16]) -> Result<()> {
        let bytes: Vec<u8> = stereo.iter().flat_map(|s| s.to_le_bytes()).collect();
        let file = self.file.as_mut().context("capture already finished")?;
        file.write_all(&bytes)
            .with_context(|| format!("writing {}", self.path.display()))?;
        self.levels.add(stereo);
        Ok(())
    }

    /// Flushes the buffer and writes the final header; the file is closed afterwards.
    /// A second call does nothing.
    fn finalize(&mut self) -> Result<()> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header(self.levels.frames))?;
        file.flush()
            .with_context(|| format!("writing {}", self.path.display()))
    }

    /// Writes the final header and returns the numbers.
    pub fn finish(mut self) -> Result<CaptureSummary> {
        self.finalize()?;
        Ok(std::mem::take(&mut self.levels).finish(std::mem::take(&mut self.path)))
    }
}

impl Drop for Capture {
    /// If the sound is aborted (viewer terminated, write error), the file still gets a
    /// valid header.
    fn drop(&mut self) {
        if let Err(e) = self.finalize() {
            log::warn!(
                "Sound capture {}: finalizing failed ({e:#})",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::parse_wav;

    #[test]
    fn captures_are_valid_wav_files_with_levels_and_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let mut capture = Capture::create(&path).unwrap();
        // 150 ms: left constant 16384 (half of full scale), right 0, then 50 ms of right -32768.
        let first: Vec<i16> = (0..ENVELOPE_FRAMES).flat_map(|_| [16384, 0]).collect();
        capture.write(&first).unwrap();
        let second: Vec<i16> = (0..ENVELOPE_FRAMES / 2)
            .flat_map(|_| [16384, -32768])
            .collect();
        capture.write(&second).unwrap();
        assert_eq!(capture.frames(), 6615);
        let summary = capture.finish().unwrap();
        assert_eq!(summary.frames, 6615);
        assert_eq!(summary.length_ms, 150.0);
        assert_eq!(summary.peak, [0.5, 1.0]);
        assert_eq!(summary.envelope, [[0.5, 0.0], [0.5, 1.0]]);
        assert!((summary.rms[1] - (1.0f32 / 3.0).sqrt()).abs() < 1e-6);
        let pcm = parse_wav(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((pcm.channels, pcm.frames()), (2, 6615));
        assert_eq!(&pcm.samples[8820..8822], &[16384, -32768]);
    }

    #[test]
    fn a_dropped_capture_still_gets_a_valid_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aborted.wav");
        let mut capture = Capture::create(&path).unwrap();
        let samples: Vec<i16> = (0..2000).collect();
        capture.write(&samples).unwrap();
        drop(capture);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes[40..44], 4000u32.to_le_bytes(), "data size in header");
        let pcm = parse_wav(&bytes).unwrap();
        assert_eq!((pcm.channels, pcm.frames()), (2, 1000));
        assert_eq!(&pcm.samples[..], &samples[..]);
    }

    #[test]
    fn an_empty_capture_has_no_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wav");
        let summary = Capture::create(&path).unwrap().finish().unwrap();
        assert_eq!((summary.frames, summary.envelope.len()), (0, 0));
        assert_eq!(std::fs::read(&path).unwrap().len(), 44);
    }
}
