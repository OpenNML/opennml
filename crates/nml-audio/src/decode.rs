//! Audio files of the game (`docs/formats/sounddatabase.md`, section "Audio files"): WAVs (RIFF, PCM
//! 16-bit, 44,100 Hz) completely in memory, Ogg Vorbis (music stereo, speech mono) streamed via
//! `symphonia`.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

use crate::SAMPLE_RATE;
use crate::resample::LinearResampler;

/// Decoded samples (interleaved, 16-bit).
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub channels: usize,
    pub samples: Arc<[i16]>,
}

impl Pcm {
    /// Number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    /// Length in ms like `FmodSound_GetLengthMs` (`length * 1000 / output rate`, truncated).
    pub fn length_ms(&self) -> u64 {
        self.frames() as u64 * 1000 / u64::from(SAMPLE_RATE)
    }
}

/// Reads a RIFF WAV with PCM 16-bit, 1 or 2 channels and 44,100 Hz. Unknown chunks (in the
/// shipped data `PAD `, `smpl`, `LIST`) are skipped; the viewer rejects other formats (the
/// game has none).
pub fn parse_wav(bytes: &[u8]) -> Result<Pcm> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF WAVE file");
    }
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        let end = body.saturating_add(size).min(bytes.len());
        let chunk = &bytes[body..end];
        match id {
            b"fmt " if chunk.len() >= 16 => {
                format = Some((
                    u16_at(chunk, 0),
                    u16_at(chunk, 2),
                    u32::from_le_bytes(chunk[4..8].try_into().unwrap()),
                    u16_at(chunk, 14),
                ));
            }
            b"data" => {
                let Some((tag, channels, rate, bits)) = format else {
                    bail!("data chunk before the fmt chunk");
                };
                if tag != 1 || bits != 16 || !(1..=2).contains(&channels) || rate != SAMPLE_RATE {
                    bail!(
                        "unsupported format (tag {tag}, {channels} channels, {rate} Hz, {bits} bit)"
                    );
                }
                let samples: Arc<[i16]> = chunk
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]))
                    .collect();
                return Ok(Pcm {
                    channels: usize::from(channels),
                    samples,
                });
            }
            _ => {}
        }
        // Chunks are padded to even lengths.
        offset = body.saturating_add(size).saturating_add(size & 1);
    }
    bail!("no data chunk")
}

/// Converts a decoded sample to 16-bit like `ov_read` (`*32768`, rounded half to even like
/// `fistp`, clamped).
pub fn to_i16(sample: f32) -> i16 {
    (sample * 32768.0)
        .round_ties_even()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

/// A stream of interleaved 16-bit frames at 44,100 Hz for a mixer channel (FMOD stream): Ogg music and
/// speech ([`OggStream`]) and streams of other rates via [`crate::resample::ResampledStream`] (Bink sound).
pub trait PcmStream: Send {
    /// Channels per frame (the mixer plays 1 or 2).
    fn channels(&self) -> usize;
    /// Fills `out` (interleaved, a multiple of the channel count) and returns the number of frames; fewer than
    /// requested means: end of the stream.
    fn read(&mut self, out: &mut [i16]) -> usize;
    /// Length in ms (`FSOUND_Stream_GetLengthMs`), if known.
    fn length_ms(&self) -> Option<u64>;
    /// Position of the next frame in ms (`FSOUND_Stream_GetTime`).
    fn position_ms(&self) -> u64;
    /// Back to the start (loop, `FSOUND_Stream_SetTime(0)`); `false` if the stream cannot do that.
    fn rewind(&mut self) -> bool;
}

/// An Ogg Vorbis stream: reads interleaved 16-bit frames at 44,100 Hz, knows its position and can jump to a
/// point in ms (`FSOUND_Stream_SetTime`). Like FMOD, it converts streams of any sample rate to the output rate
/// (`FSOUND_Stream_Open…`, re/audio, section "FMOD mixer"): through the core of
/// [`crate::resample::ResampledStream`], unchanged at 44,100 Hz. Position and length are in source time.
pub struct OggStream {
    source: OggSource,
    /// Sample rate of the source.
    rate: u32,
    /// Total length in source frames (from the container).
    frames_total: Option<u64>,
    /// Source frame at which the conversion last began (opening or seek).
    start: u64,
    core: LinearResampler,
}

/// Container and decoder of an [`OggStream`]: provides decoded `f32` samples as the source of the conversion core.
struct OggSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    channels: usize,
    /// Source frames still to be discarded after a seek.
    skip: u64,
    scratch: Vec<f32>,
}

impl OggSource {
    /// Appends the samples of the next packet to `out` (contract as in [`crate::resample::SampleSource`]); `false` at
    /// the end of the stream.
    fn pull(&mut self, out: &mut Vec<f32>) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return false,
                Err(e) => {
                    log::warn!("Ogg stream: {e}");
                    return false;
                }
            };
            if packet.track_id != self.track {
                continue;
            }
            let buffer = match self.decoder.decode(&packet) {
                Ok(buffer) => buffer,
                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                    log::debug!("Ogg packet discarded: {e}");
                    continue;
                }
                Err(e) => {
                    log::warn!("Ogg stream: {e}");
                    return false;
                }
            };
            self.scratch.resize(buffer.samples_interleaved(), 0.0);
            buffer.copy_to_slice_interleaved(&mut self.scratch);
            let frames = self.scratch.len() / self.channels;
            let drop = (self.skip.min(frames as u64)) as usize;
            self.skip -= drop as u64;
            if drop < frames {
                out.extend_from_slice(&self.scratch[drop * self.channels..]);
                return true;
            }
        }
    }
}

impl std::fmt::Debug for OggStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OggStream")
            .field("channels", &self.source.channels)
            .field("rate", &self.rate)
            .field("frames_total", &self.frames_total)
            .field("position", &self.position())
            .finish_non_exhaustive()
    }
}

impl OggStream {
    /// Opens a stream from a file (`std::fs::File`) or from memory
    /// (`std::io::Cursor<Vec<u8>>`); 1 or 2 channels, any sample rate above 0.
    pub fn open(source: Box<dyn MediaSource>) -> Result<Self> {
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        hint.with_extension("ogg");
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .context("Ogg container not readable")?;
        let track = format
            .default_track(TrackType::Audio)
            .context("no audio track")?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .context("no audio parameters")?;
        let channels = params.channels.as_ref().map_or(0, |c| c.count());
        let rate = params.sample_rate.unwrap_or(0);
        if rate == 0 || !(1..=2).contains(&channels) {
            bail!(
                "unsupported format ({channels} channels, {:?} Hz)",
                params.sample_rate
            );
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .context("Vorbis decoder")?;
        Ok(Self {
            frames_total: track.num_frames,
            source: OggSource {
                track: track.id,
                format,
                decoder,
                channels,
                skip: 0,
                scratch: Vec::new(),
            },
            rate,
            start: 0,
            core: LinearResampler::new(rate, channels),
        })
    }

    /// Sample rate of the source in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Source frame of the next frame delivered.
    fn position(&self) -> u64 {
        self.start + self.core.source_frame()
    }

    /// Seeks to `ms` (`FSOUND_Stream_SetTime`, in source time); past the end the stream is at its end. The
    /// conversion starts over afterwards.
    pub fn seek_ms(&mut self, ms: u64) -> Result<()> {
        let frame = ms * u64::from(self.rate) / 1000;
        self.core.reset();
        let seeked = self.source.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: Timestamp::new(frame as i64),
                track_id: self.source.track,
            },
        );
        self.source.decoder.reset();
        match seeked {
            Ok(to) => {
                let actual = to.actual_ts.get().max(0) as u64;
                self.source.skip = frame.saturating_sub(actual);
                self.start = frame;
            }
            Err(e) => {
                // Past the end: like a finished stream.
                log::debug!("Seek to {ms} ms: {e}");
                self.source.skip = 0;
                self.start = self.frames_total.unwrap_or(frame);
                self.frames_total = Some(self.start);
            }
        }
        Ok(())
    }
}

impl PcmStream for OggStream {
    fn channels(&self) -> usize {
        self.source.channels
    }

    /// Length in ms, if the container knows it.
    fn length_ms(&self) -> Option<u64> {
        self.frames_total.map(|f| f * 1000 / u64::from(self.rate))
    }

    fn position_ms(&self) -> u64 {
        self.position() * 1000 / u64::from(self.rate)
    }

    /// By seeking to 0 ms ([`OggStream::seek_ms`]).
    fn rewind(&mut self) -> bool {
        self.seek_ms(0).is_ok()
    }

    fn read(&mut self, out: &mut [i16]) -> usize {
        let source = &mut self.source;
        self.core
            .read(out, &mut |buf: &mut Vec<f32>| source.pull(buf))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// WAV file from samples (PCM 16-bit, 44,100 Hz) with a `PAD ` chunk of odd length before it.
    pub(crate) fn wav_bytes(channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        out.extend_from_slice(&(SAMPLE_RATE * 2 * u32::from(channels)).to_le_bytes());
        out.extend_from_slice(&(2 * channels).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"PAD ");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        let size = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&size.to_le_bytes());
        out
    }

    #[test]
    fn wav_files_are_read_with_padding_chunks() {
        let pcm = parse_wav(&wav_bytes(1, &[1, -2, 32767, -32768])).unwrap();
        assert_eq!(pcm.channels, 1);
        assert_eq!(&*pcm.samples, &[1, -2, 32767, -32768]);
        let second: Vec<i16> = vec![0; 44_100 * 2];
        let stereo = parse_wav(&wav_bytes(2, &second)).unwrap();
        assert_eq!((stereo.frames(), stereo.length_ms()), (44_100, 1000));
        assert!(parse_wav(b"RIFF\0\0\0\0WAVX").is_err());
        let mut eight_bit = wav_bytes(1, &[0, 0]);
        eight_bit[34] = 8;
        assert!(format!("{:#}", parse_wav(&eight_bit).unwrap_err()).contains("8 bit"));
    }

    #[test]
    fn decoded_samples_round_like_ov_read() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32768);
        assert_eq!(to_i16(-2.0), -32768);
        assert_eq!(to_i16(0.5 / 32768.0), 0);
        assert_eq!(to_i16(1.5 / 32768.0), 2);
        assert_eq!(to_i16(-1.5 / 32768.0), -2);
    }
}
