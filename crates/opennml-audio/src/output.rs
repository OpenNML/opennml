//! Device output via `cpal`: the mixer pushes stereo frames into a ring buffer that the
//! device thread drains. If the buffer runs empty (paused clock, stutter), the device fades the
//! last value out over [`FADE_FRAMES`] and then plays silence; it only continues, with a fade-in,
//! once [`PREFILL_FRAMES`] are queued. If the buffer overflows, the oldest frames are dropped.
//! Playback never changes the speed; the mixer and the capture see nothing of the fade-out.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::SAMPLE_RATE;
use crate::driver::{DriverKey, device_matches};

/// At most this many frames wait for the device (250 ms); older ones are dropped.
pub const RING_FRAMES: usize = SAMPLE_RATE as usize / 4;

/// Length of the fade-in and fade-out in frames (5 ms). If the supply breaks off (dragging the window,
/// mode switch, muting), the device would otherwise jump hard from the last sample to 0 and
/// hard back into the signal on restart; a short linear ramp avoids the click.
pub const FADE_FRAMES: usize = SAMPLE_RATE as usize / 200;

/// After an interruption (and at the start) the device only plays again once this many frames are
/// ready (40 ms). Without a prefill, after a gap every small chunk would arrive on its own and
/// break off again immediately, which would be heard as crackling.
pub const PREFILL_FRAMES: usize = SAMPLE_RATE as usize / 25;

/// Ring buffer of interleaved stereo samples between mixer and device.
#[derive(Debug, Default)]
pub struct Ring {
    samples: VecDeque<i16>,
    /// Frames that the device received as silence because nothing was ready.
    pub underrun_frames: u64,
    /// Discarded frames (overflow).
    pub dropped_frames: u64,
    /// The device is currently playing payload data (otherwise fade-out, silence or waiting for the prefill).
    playing: bool,
    /// Fade-in steps already played (0-[`FADE_FRAMES`]); the level is `fade_in / FADE_FRAMES`.
    fade_in: usize,
    /// Frame most recently given to the device, from which the fade-out starts.
    last: [f32; 2],
    /// Frames of the fade-out still missing; playback does not begin while it runs.
    fade_out_left: usize,
    /// Decrease per frame of the running fade-out.
    fade_out_step: [f32; 2],
}

impl Ring {
    /// Appends frames; beyond [`RING_FRAMES`] the oldest are dropped.
    pub fn push(&mut self, stereo: &[i16]) {
        self.samples.extend(stereo);
        let excess = self.samples.len().saturating_sub(RING_FRAMES * 2);
        if excess > 0 {
            self.samples.drain(..excess);
            self.dropped_frames += (excess / 2) as u64;
        }
    }

    /// Fills `out` (interleaved stereo, values -1 to 1). Without supply the last frame
    /// fades linearly to 0, then it is silent; playback only begins again with
    /// [`PREFILL_FRAMES`] queued and then fades in over [`FADE_FRAMES`].
    pub fn pull(&mut self, out: &mut [f32]) {
        let mut frames = out.chunks_exact_mut(2);
        for frame in &mut frames {
            frame.copy_from_slice(&self.next_frame());
        }
        frames.into_remainder().fill(0.0);
    }

    fn next_frame(&mut self) -> [f32; 2] {
        if !self.playing && self.fade_out_left == 0 && self.queued_frames() >= PREFILL_FRAMES {
            self.playing = true;
            self.fade_in = 0;
        }
        if self.playing {
            if let (Some(l), Some(r)) =
                (self.samples.front().copied(), self.samples.get(1).copied())
            {
                self.samples.drain(..2);
                self.fade_in = (self.fade_in + 1).min(FADE_FRAMES);
                let gain = self.fade_in as f32 / FADE_FRAMES as f32;
                self.last = [f32::from(l) / 32768.0 * gain, f32::from(r) / 32768.0 * gain];
                return self.last;
            }
            self.playing = false;
            self.fade_out_left = FADE_FRAMES;
            self.fade_out_step = self.last.map(|v| v / FADE_FRAMES as f32);
        }
        self.underrun_frames += 1;
        if self.fade_out_left > 0 {
            self.fade_out_left -= 1;
            self.last = if self.fade_out_left == 0 {
                [0.0; 2]
            } else {
                [
                    self.last[0] - self.fade_out_step[0],
                    self.last[1] - self.fade_out_step[1],
                ]
            };
        }
        self.last
    }

    pub fn queued_frames(&self) -> usize {
        self.samples.len() / 2
    }

    /// Discards the waiting frames; the device then fades out the last value.
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// Open output stream (44,100 Hz, stereo, `f32`; WASAPI converts by itself if needed).
pub struct Output {
    ring: Arc<Mutex<Ring>>,
    _stream: cpal::Stream,
    device: String,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl Output {
    /// Opens the device of the driver `key` ([`DriverKey::Default`]: the default output device); without a device or on
    /// an error `None` with a log line (the viewer then continues silently, the mixer and the capture keep
    /// running). If the specific device is missing, it takes the default device.
    pub fn open(key: &DriverKey) -> Option<Self> {
        match Self::try_open(key) {
            Ok(output) => {
                log::info!("Sound: output via {}", output.device);
                Some(output)
            }
            Err(e) => {
                log::warn!("Sound: no audio device, the viewer stays silent ({e:#})");
                None
            }
        }
    }

    fn try_open(key: &DriverKey) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = match key {
            DriverKey::Default => None,
            DriverKey::Device(k) => {
                let found = host.output_devices().ok().and_then(|mut devices| {
                    devices.find(|d| d.id().is_ok_and(|id| device_matches(id.id(), k)))
                });
                if found.is_none() {
                    log::warn!("Sound: device {k} not found, output through the default device");
                }
                found
            }
        };
        let device = match device {
            Some(d) => d,
            None => host
                .default_output_device()
                .ok_or_else(|| anyhow::anyhow!("no default output device"))?,
        };
        let name = device
            .description()
            .map_or_else(|_| "Default device".to_string(), |d| d.name().to_string());
        let config = cpal::StreamConfig {
            channels: 2,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };
        let ring = Arc::new(Mutex::new(Ring::default()));
        let shared = ring.clone();
        let stream = device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| match shared.lock() {
                Ok(mut ring) => ring.pull(data),
                Err(_) => data.fill(0.0),
            },
            |e| log::warn!("Sound: output error {e}"),
            None,
        )?;
        stream.play()?;
        Ok(Self {
            ring,
            _stream: stream,
            device: name,
        })
    }

    /// Gives frames to the device.
    pub fn push(&self, stereo: &[i16]) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.push(stereo);
        }
    }

    /// Discards waiting frames (e.g. when muting).
    pub fn clear(&self) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.clear();
        }
    }

    pub fn device(&self) -> &str {
        &self.device
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frames` stereo frames with the fixed value `v` on both channels.
    fn constant(frames: usize, v: i16) -> Vec<i16> {
        vec![v; frames * 2]
    }

    /// Pulls `frames` frames and returns the left channel (the right one is equal).
    fn pull_left(ring: &mut Ring, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames * 2];
        ring.pull(&mut out);
        for frame in out.chunks_exact(2) {
            assert_eq!(frame[0], frame[1], "both channels must be equal");
        }
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    /// Ring that has settled at level 0.5 and has just output everything.
    fn playing_ring_drained() -> Ring {
        let mut ring = Ring::default();
        ring.push(&constant(PREFILL_FRAMES, 16384));
        let out = pull_left(&mut ring, PREFILL_FRAMES);
        assert_eq!(*out.last().unwrap(), 0.5);
        ring
    }

    /// Checks a linear ramp from `from` to `to` over exactly [`FADE_FRAMES`] frames
    /// (`ramp[0]` is the first step, `ramp.last()` the target value), without a larger jump.
    fn assert_linear_ramp(from: f32, to: f32, ramp: &[f32]) {
        assert_eq!(ramp.len(), FADE_FRAMES);
        assert_eq!(*ramp.last().unwrap(), to);
        let max_step = (to - from).abs() / FADE_FRAMES as f32 * 1.01;
        let mut prev = from;
        for (i, &v) in ramp.iter().enumerate() {
            assert!(
                (v - prev).abs() <= max_step,
                "jump {} at frame {i}",
                v - prev
            );
            assert!(
                (to > from && v >= prev) || (to < from && v <= prev),
                "ramp not monotonic at frame {i}"
            );
            prev = v;
        }
    }

    #[test]
    fn underrun_fades_the_last_value_out_instead_of_jumping_to_zero() {
        let mut ring = playing_ring_drained();
        let out = pull_left(&mut ring, FADE_FRAMES + 50);
        assert_linear_ramp(0.5, 0.0, &out[..FADE_FRAMES]);
        assert!(out[0] > 0.4, "the first step stays close to the last value");
        assert!(out[FADE_FRAMES..].iter().all(|&v| v == 0.0));
        assert_eq!(ring.underrun_frames, (FADE_FRAMES + 50) as u64);
    }

    #[test]
    fn underrun_in_the_middle_of_a_buffer_continues_from_the_last_frame() {
        let mut ring = Ring::default();
        ring.push(&constant(PREFILL_FRAMES + 100, 16384));
        let out = pull_left(&mut ring, PREFILL_FRAMES + 100 + FADE_FRAMES);
        let (data, tail) = out.split_at(PREFILL_FRAMES + 100);
        assert_eq!(*data.last().unwrap(), 0.5);
        assert_linear_ramp(0.5, 0.0, tail);
    }

    #[test]
    fn resume_waits_for_the_prefill_and_then_fades_in() {
        let mut ring = playing_ring_drained();
        pull_left(&mut ring, FADE_FRAMES + 10);
        ring.push(&constant(PREFILL_FRAMES - 1, 16384));
        let out = pull_left(&mut ring, 300);
        assert!(
            out.iter().all(|&v| v == 0.0),
            "below the prefill it stays silent"
        );
        assert_eq!(ring.queued_frames(), PREFILL_FRAMES - 1);
        ring.push(&constant(1, 16384));
        let out = pull_left(&mut ring, FADE_FRAMES + 20);
        assert!(out[0] > 0.0 && out[0] <= 0.5 / FADE_FRAMES as f32 * 1.01);
        assert_linear_ramp(0.0, 0.5, &out[..FADE_FRAMES]);
        assert!(out[FADE_FRAMES..].iter().all(|&v| v == 0.5));
    }

    #[test]
    fn stream_start_is_silent_until_the_prefill_and_then_fades_in() {
        let mut ring = Ring::default();
        ring.push(&constant(PREFILL_FRAMES, 16384));
        let out = pull_left(&mut ring, FADE_FRAMES + 5);
        assert_linear_ramp(0.0, 0.5, &out[..FADE_FRAMES]);
        assert!(out[FADE_FRAMES..].iter().all(|&v| v == 0.5));
    }

    #[test]
    fn resume_during_the_fade_out_waits_for_its_end_without_a_jump() {
        let mut ring = playing_ring_drained();
        let mut out = pull_left(&mut ring, 20);
        ring.push(&constant(PREFILL_FRAMES, 16384));
        out.extend(pull_left(&mut ring, FADE_FRAMES * 3));
        let max_step = 0.5 / FADE_FRAMES as f32 * 1.01;
        let mut prev = 0.5;
        for (i, &v) in out.iter().enumerate() {
            assert!((v - prev).abs() <= max_step, "jump at frame {i}");
            prev = v;
        }
        assert_eq!(prev, 0.5, "afterwards the signal continues");
    }

    #[test]
    fn clear_while_playing_fades_out_and_unmute_fades_in_after_the_prefill() {
        let mut ring = Ring::default();
        ring.push(&constant(PREFILL_FRAMES + 500, 16384));
        pull_left(&mut ring, PREFILL_FRAMES);
        ring.clear();
        assert_eq!(ring.queued_frames(), 0);
        let out = pull_left(&mut ring, FADE_FRAMES + 10);
        assert_linear_ramp(0.5, 0.0, &out[..FADE_FRAMES]);
        assert!(out[FADE_FRAMES..].iter().all(|&v| v == 0.0));
        ring.push(&constant(PREFILL_FRAMES, 16384));
        let out = pull_left(&mut ring, FADE_FRAMES);
        assert_linear_ramp(0.0, 0.5, &out);
    }

    #[test]
    fn ring_counts_underruns_and_drops_the_oldest_frames_when_full() {
        let mut ring = Ring::default();
        ring.push(&[16384, -16384, 8192, 0]);
        let mut out = [9.0f32; 8];
        ring.pull(&mut out);
        assert_eq!(out, [0.0; 8], "below the prefill the device stays silent");
        assert_eq!(ring.underrun_frames, 4);
        assert_eq!(ring.queued_frames(), 2, "the frames stay queued for later");
        let many: Vec<i16> = (0..(RING_FRAMES as i32 + 10) * 2)
            .map(|i| (i / 2) as i16)
            .collect();
        ring.push(&many);
        assert_eq!(ring.queued_frames(), RING_FRAMES);
        assert_eq!(ring.dropped_frames, 12);
        let mut first = [0.0f32; 2];
        ring.pull(&mut first);
        let expected = 10.0 / 32768.0 / FADE_FRAMES as f32;
        assert!(
            (first[0] - expected).abs() < 1e-9 && first[0] == first[1],
            "the 2 old and 10 new frames are missing"
        );
        ring.clear();
        assert_eq!(ring.queued_frames(), 0);
    }
}
