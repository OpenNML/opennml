//! Streams of any sample rate for the mixer (a deviation from the original): like FMOD, the mixer runs at 44,100 Hz
//! without interpolation ([`crate::mixer`]); the Bink audio tracks of the videos have 22,050, 44,100 or 48,000 Hz.
//! In the original, `binkw32.dll` plays them itself through DirectSound (re/video, section "Opening", hypothesis); the
//! reimplementation converts them linearly to 44,100 Hz and feeds them as a stream into a mixer channel (re/audio,
//! section "Video sound").
//!
//! The order of computation is fixed: the phase is an integer fraction `rest / 44 100` (per output frame
//! `rest += rate`, whole multiples advance the source frame), the intermediate value
//! `a + (b − a) · (rest / 44 100)` in `f32`, then
//! [`to_i16`] (rounds and clamps; values above 1.0 are allowed). At phase 0 - hence always at 44,100 Hz - the
//! source value passes through unchanged.
//!
//! The core [`LinearResampler`] is shared by [`ResampledStream`] (Bink sound) and [`crate::decode::OggStream`]: FMOD
//! converts every stream to the output rate (`FSOUND_Stream_Open…`); the German patch brings speech at
//! 48,000 Hz (re/audio, sections "FMOD mixer" and "Streams of other sample rates").

use crate::SAMPLE_RATE;
use crate::decode::{PcmStream, to_i16};

/// Source of a [`ResampledStream`]: appends interleaved `f32` samples and reports `false` at the end (what it
/// still appends then counts). Contract: whoever reports `true` appends at least one frame - the stream calls
/// the source in a loop until enough frames are there, and an empty `true` source would hang the sound thread.
pub type SampleSource = Box<dyn FnMut(&mut Vec<f32>) -> bool + Send>;

/// Source frames read, from which on the buffer is trimmed at the front.
const COMPACT_FRAMES: usize = 4096;

/// Core of the linear conversion to [`SAMPLE_RATE`], shared by [`ResampledStream`] (Bink sound) and
/// [`crate::decode::OggStream`] (music and speech of any rate, like FMOD `FSOUND_Stream_Open…`). It holds the source
/// samples read and the phase; the caller passes the source in on every [`LinearResampler::read`].
#[derive(Debug)]
pub(crate) struct LinearResampler {
    rate: u32,
    channels: usize,
    /// Source samples read and still needed (interleaved); `buffer[0]` is source frame `base`.
    buffer: Vec<f32>,
    base: u64,
    /// Source frame and phase (numerator over the denominator 44,100) of the next output frame.
    index: u64,
    phase: u64,
    /// The source has reported its end.
    ended: bool,
}

impl LinearResampler {
    /// Core for `rate` Hz and `channels` channels; rate 0 or 0 channels yield nothing.
    pub(crate) fn new(rate: u32, channels: usize) -> Self {
        Self {
            rate,
            channels,
            buffer: Vec::new(),
            base: 0,
            index: 0,
            phase: 0,
            ended: rate == 0 || channels == 0,
        }
    }

    /// Back to the start (after a seek of the source): buffer, phase and end flag as new, so that no remnants
    /// of the old pass end up in the new one.
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.rate, self.channels);
    }

    /// Source frame of the next output frame, counted from [`Self::new`] or [`Self::reset`].
    pub(crate) fn source_frame(&self) -> u64 {
        self.index
    }

    /// Complete source frames in the buffer.
    fn buffered(&self) -> usize {
        self.buffer.len() / self.channels
    }

    /// Reads from `source` until `frames` source frames are in the buffer or the source ends.
    fn fill(&mut self, frames: usize, source: &mut dyn FnMut(&mut Vec<f32>) -> bool) {
        while !self.ended && self.buffered() < frames {
            if !source(&mut self.buffer) {
                self.ended = true;
            }
        }
    }

    /// Discards source frames already read at the front.
    fn compact(&mut self) {
        let used = ((self.index - self.base) as usize).min(self.buffered());
        if used >= COMPACT_FRAMES {
            self.buffer.drain(..used * self.channels);
            self.base += used as u64;
        }
    }

    /// Fills `out` (interleaved) with converted frames from `source` (contract as in [`SampleSource`]) and returns
    /// their number; fewer than requested means: end of the source.
    pub(crate) fn read(
        &mut self,
        out: &mut [i16],
        source: &mut dyn FnMut(&mut Vec<f32>) -> bool,
    ) -> usize {
        if self.rate == 0 || self.channels == 0 {
            return 0;
        }
        let ch = self.channels;
        let rate = u64::from(SAMPLE_RATE);
        let frames = out.len() / ch;
        let mut done = 0;
        while done < frames {
            let at = (self.index - self.base) as usize;
            // At phase 0 the source frame itself suffices, otherwise its successor too.
            self.fill(at + if self.phase == 0 { 1 } else { 2 }, source);
            let available = self.buffered();
            if at >= available {
                break;
            }
            let target = &mut out[done * ch..(done + 1) * ch];
            let a = &self.buffer[at * ch..(at + 1) * ch];
            if self.phase == 0 || at + 1 >= available {
                // At the end of the source the last frame holds.
                for (o, &s) in target.iter_mut().zip(a) {
                    *o = to_i16(s);
                }
            } else {
                let b = &self.buffer[(at + 1) * ch..(at + 2) * ch];
                let t = self.phase as f32 / SAMPLE_RATE as f32;
                for ((o, &s0), &s1) in target.iter_mut().zip(a).zip(b) {
                    *o = to_i16(s0 + (s1 - s0) * t);
                }
            }
            done += 1;
            let next = self.phase + u64::from(self.rate);
            self.index += next / rate;
            self.phase = next % rate;
        }
        self.compact();
        done
    }
}

/// A stream with source rate `rate`, converted linearly to [`SAMPLE_RATE`].
pub struct ResampledStream {
    source: SampleSource,
    core: LinearResampler,
    rate: u32,
    channels: usize,
    length_ms: Option<u64>,
    /// Output frames delivered.
    delivered: u64,
}

impl std::fmt::Debug for ResampledStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResampledStream")
            .field("rate", &self.rate)
            .field("channels", &self.channels)
            .field("length_ms", &self.length_ms)
            .field("delivered", &self.delivered)
            .finish_non_exhaustive()
    }
}

impl ResampledStream {
    /// Stream from `source` with `rate` Hz and `channels` channels; `length_ms` is reported by [`PcmStream::length_ms`]
    /// (only the caller knows the length). Rate 0 or 0 channels yield an empty stream.
    pub fn new(source: SampleSource, rate: u32, channels: usize, length_ms: Option<u64>) -> Self {
        Self {
            source,
            core: LinearResampler::new(rate, channels),
            rate,
            channels,
            length_ms,
            delivered: 0,
        }
    }
}

impl PcmStream for ResampledStream {
    fn channels(&self) -> usize {
        self.channels
    }

    fn read(&mut self, out: &mut [i16]) -> usize {
        let done = self.core.read(out, &mut *self.source);
        self.delivered += done as u64;
        done
    }

    fn length_ms(&self) -> Option<u64> {
        self.length_ms
    }

    /// Frames delivered in ms (44,100 Hz).
    fn position_ms(&self) -> u64 {
        self.delivered * 1000 / u64::from(SAMPLE_RATE)
    }

    /// The source cannot seek back (video sound does not loop).
    fn rewind(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{PcmStream, to_i16};

    /// Sine from a fixed computation (Taylor series up to x^13 on [-pi, pi], without libm): `turns` in periods.
    fn sine(turns: f64) -> f32 {
        const PI: f64 = std::f64::consts::PI;
        let r = turns - (turns + 0.5).floor();
        let x = r * 2.0 * PI;
        let x2 = x * x;
        let mut term = x;
        let mut sum = x;
        for k in 1..7 {
            term *= -x2 / f64::from((2 * k) * (2 * k + 1));
            sum += term;
        }
        sum as f32
    }

    /// Source from a table of interleaved samples, in pieces of `chunk` frames.
    fn table_source(table: Vec<f32>, channels: usize, chunk: usize) -> SampleSource {
        let mut at = 0;
        Box::new(move |out: &mut Vec<f32>| {
            let end = (at + chunk * channels).min(table.len());
            out.extend_from_slice(&table[at..end]);
            at = end;
            at < table.len()
        })
    }

    /// 1 kHz sine with `n` frames at `rate` (left sine * 0.5, right its negation).
    fn sine_table(rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|k| {
                let s = sine(k as f64 * 1000.0 / f64::from(rate)) * 0.5;
                [s, -s]
            })
            .collect()
    }

    fn read_all(stream: &mut ResampledStream, chunk: usize) -> Vec<i16> {
        let channels = stream.channels();
        let mut all = Vec::new();
        let mut buf = vec![0i16; chunk * channels];
        loop {
            let got = stream.read(&mut buf);
            all.extend_from_slice(&buf[..got * channels]);
            if got < chunk {
                return all;
            }
        }
    }

    #[test]
    fn resampler_passes_44100_through() {
        let table: Vec<f32> = (0..3000)
            .map(|i| ((i * 7919) % 2001) as f32 / 1000.0 - 1.0)
            .collect();
        let mut stream =
            ResampledStream::new(table_source(table.clone(), 2, 37), 44_100, 2, Some(34));
        assert_eq!((stream.channels(), stream.length_ms()), (2, Some(34)));
        let out = read_all(&mut stream, 256);
        let expected: Vec<i16> = table.iter().map(|&s| to_i16(s)).collect();
        assert_eq!(out, expected, "unchanged at 44,100 Hz");
        assert_eq!(stream.position_ms(), 1500 * 1000 / 44_100);
        assert!(!stream.rewind(), "a source without seeking back");
    }

    #[test]
    fn resampler_converts_48000_and_22050() {
        // Linear between the source frames, the last one holds.
        let mut ramp =
            ResampledStream::new(table_source(vec![0.0, 0.5, 1.0], 1, 2), 22_050, 1, None);
        assert_eq!(
            read_all(&mut ramp, 16),
            [0, 8192, 16384, 24576, 32767, 32767]
        );
        for rate in [48_000u32, 22_050] {
            let n = rate as usize * 2 / 10;
            let table = sine_table(rate, n);
            let mut stream =
                ResampledStream::new(table_source(table.clone(), 2, 1000), rate, 2, None);
            let out = read_all(&mut stream, 4096);
            let frames = out.len() / 2;
            let expected = n * 44_100 / rate as usize;
            assert!(
                frames.abs_diff(expected) <= 1,
                "{rate} Hz: {frames} frames instead of {expected}"
            );
            // Rising zero crossings on the left at a distance of 44.1 +/- 1 frames.
            let left: Vec<i16> = out.iter().step_by(2).copied().collect();
            let rising: Vec<usize> = (1..left.len())
                .filter(|&j| left[j - 1] < 0 && left[j] >= 0)
                .collect();
            assert!(rising.len() >= 8, "{rate} Hz: {} crossings", rising.len());
            for w in rising.windows(2) {
                let d = (w[1] - w[0]) as f64;
                assert!((d - 44.1).abs() <= 1.0, "{rate} Hz: distance {d}");
            }
            // On the right the negation (apart from the rounding at -32768).
            for f in out.chunks_exact(2) {
                assert!((i32::from(f[0]) + i32::from(f[1])).abs() <= 1, "{f:?}");
            }
            // Same result with a different chunking of the source and of the read calls.
            let mut again = ResampledStream::new(table_source(table, 2, 333), rate, 2, None);
            assert_eq!(read_all(&mut again, 77), out, "{rate} Hz");
        }
    }

    #[test]
    fn resampler_clamps_values_above_one() {
        // Bink sound can exceed 1.0 (up to about 1.15); clamping happens only at `to_i16`.
        let table = [1.15f32, -1.2].repeat(480);
        let mut stream = ResampledStream::new(table_source(table, 2, 100), 48_000, 2, None);
        let out = read_all(&mut stream, 128);
        assert_eq!(out.len() / 2, 441, "480 frames at 48,000 Hz");
        assert!(out.chunks_exact(2).all(|f| f == [32767, -32768]), "clamped");
    }

    #[test]
    fn core_restarts_cleanly_after_reset() {
        // `OggStream` seeks to the start for looping (`FSOUND_Stream_SetTime(0)`) and resets the core:
        // afterwards the same frames as with a fresh core - in the middle of the buffer with phase != 0 as at the end.
        let table = sine_table(48_000, 2000);
        let first = |core: &mut LinearResampler| {
            let mut source = table_source(table.clone(), 2, 300);
            let mut out = vec![0i16; 500 * 2];
            assert_eq!(core.read(&mut out, &mut *source), 500);
            out
        };
        let expected = first(&mut LinearResampler::new(48_000, 2));
        let mut core = LinearResampler::new(48_000, 2);
        let mut source = table_source(table.clone(), 2, 300);
        let mut buf = vec![0i16; 333 * 2];
        assert_eq!(core.read(&mut buf, &mut *source), 333);
        assert_ne!(core.phase, 0, "in the middle between two source frames");
        core.reset();
        assert_eq!(core.source_frame(), 0);
        assert_eq!(
            first(&mut core),
            expected,
            "seek in the middle of the stream"
        );
        while core.read(&mut buf, &mut *source) == 333 {}
        assert!(core.ended);
        core.reset();
        assert_eq!(first(&mut core), expected, "seek after the end");
        assert_eq!(core.source_frame(), 500 * 48_000 / 44_100);
    }

    #[test]
    fn resampler_without_channels_or_rate_is_empty() {
        let mut empty = ResampledStream::new(table_source(vec![0.5; 10], 1, 4), 0, 1, None);
        assert_eq!(empty.read(&mut [0; 8]), 0);
        let mut none = ResampledStream::new(table_source(vec![0.5; 10], 1, 4), 44_100, 0, None);
        assert_eq!(none.read(&mut [0; 8]), 0);
        let mut ended = ResampledStream::new(Box::new(|_: &mut Vec<f32>| false), 22_050, 2, None);
        assert_eq!(ended.read(&mut [0; 8]), 0);
    }
}
