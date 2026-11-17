//! Software mixer like FMOD 3.71 with `FSOUND_MIXER_QUALITY_MMXP6` (`Mixer_QualityMMXP6_Mix`
//! `0x10030C10` in `fmod.dll`, measured bit-exactly against the original DLL): blocks of 1024 frames,
//! volume/pan/3D take effect from the next block, mono channels with a volume ramp over 128
//! frames (from 0 at the start), stereo channels without a ramp, sum as 16-bit with saturation after each
//! channel in start order. Samples and output both have 44,100 Hz (playback rate 1, no
//! interpolation).

use std::sync::Arc;

use crate::decode::{Pcm, PcmStream};
use crate::pool::CHANNELS;
use crate::spatial::{FmodListener, Pan, channel_gains, spatialize};

/// Frames per mix block (`(44100·25/1000) & !0xFF`).
pub const BLOCK_FRAMES: usize = 1024;
/// Length of the volume ramp in frames (`(rate << 7) / 44100`).
pub const RAMP_FRAMES: i32 = 128;

/// What a channel plays.
pub enum Source {
    /// Fully decoded sample (WAV).
    Sample { pcm: Arc<Pcm>, position: usize },
    /// Stream at 44,100 Hz (Ogg music and speech, converted Bink sound).
    Stream(Box<dyn PcmStream>),
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sample { pcm, position } => f
                .debug_struct("Sample")
                .field("pcm", pcm)
                .field("position", position)
                .finish(),
            Self::Stream(s) => f
                .debug_struct("Stream")
                .field("channels", &s.channels())
                .field("position_ms", &s.position_ms())
                .finish(),
        }
    }
}

impl Source {
    fn channels(&self) -> usize {
        match self {
            Self::Sample { pcm, .. } => pcm.channels,
            Self::Stream(s) => s.channels(),
        }
    }
}

/// An occupied FMOD channel.
#[derive(Debug)]
pub struct Channel {
    source: Source,
    looping: bool,
    /// `FSOUND_SetVolume` (0-255).
    volume: i32,
    pan: Pan,
    /// Source position of a 3D channel (mono samples only, loaded as 3D).
    spatial: Option<[f32; 3]>,
    vol3d: i32,
    /// Volume left/right in 8.8 format and ramp (mono only).
    current: [i32; 2],
    seen: [i32; 2],
    step: [i32; 2],
    ramp_left: i32,
    /// End reached (sample over, or stream at its end without a loop).
    finished: bool,
}

impl Channel {
    /// Target gain left/right from volume, 3D and pan.
    pub fn gains(&self) -> (i32, i32) {
        channel_gains(self.vol3d, self.volume, self.pan)
    }

    pub fn volume(&self) -> i32 {
        self.volume
    }

    pub fn pan(&self) -> Pan {
        self.pan
    }

    pub fn vol3d(&self) -> i32 {
        self.vol3d
    }

    pub fn spatial(&self) -> Option<[f32; 3]> {
        self.spatial
    }

    /// Position of a stream in ms (`FSOUND_Stream_GetTime`); for samples the time played.
    pub fn position_ms(&self) -> u64 {
        match &self.source {
            Source::Sample { position, .. } => {
                *position as u64 * 1000 / u64::from(crate::SAMPLE_RATE)
            }
            Source::Stream(s) => s.position_ms(),
        }
    }

    /// Length in ms (stream: if known).
    pub fn length_ms(&self) -> Option<u64> {
        match &self.source {
            Source::Sample { pcm, .. } => Some(pcm.length_ms()),
            Source::Stream(s) => s.length_ms(),
        }
    }

    /// Reads the frames of this block (interleaved, channel count of the source) into `buf`; returns the
    /// number of frames (fewer = end).
    fn fill(&mut self, buf: &mut Vec<i16>, frames: usize) -> usize {
        let channels = self.source.channels();
        buf.clear();
        buf.resize(frames * channels, 0);
        let mut done = 0;
        let mut empty_reads = 0;
        while done < frames {
            let got = match &mut self.source {
                Source::Sample { pcm, position } => {
                    let available = pcm.frames().saturating_sub(*position).min(frames - done);
                    buf[done * channels..(done + available) * channels].copy_from_slice(
                        &pcm.samples[*position * channels..(*position + available) * channels],
                    );
                    *position += available;
                    available
                }
                Source::Stream(s) => s.read(&mut buf[done * channels..frames * channels]),
            };
            done += got;
            if done == frames {
                break;
            }
            // Nothing read twice: empty source, no further even with a loop.
            empty_reads = if got == 0 { empty_reads + 1 } else { 0 };
            if !self.looping || empty_reads > 1 || !self.rewind() {
                break;
            }
        }
        if done < frames {
            self.finished = true;
        }
        done
    }

    /// Loop: back to the start (sample sample-exact, stream by seeking).
    fn rewind(&mut self) -> bool {
        match &mut self.source {
            Source::Sample { pcm, position } => {
                *position = 0;
                pcm.frames() > 0
            }
            Source::Stream(s) => s.rewind(),
        }
    }
}

/// The 32 channels, the FMOD listener and the current mix block.
#[derive(Debug)]
pub struct Mixer {
    slots: Vec<Option<Channel>>,
    /// Start order of the occupied channels (mixing order).
    order: Vec<usize>,
    listener: FmodListener,
    min_distance: f32,
    max_distance: f32,
    block: Vec<i16>,
    /// Frames delivered from the current block (`BLOCK_FRAMES` = block used up).
    block_used: usize,
    scratch: Vec<i16>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Self {
        Self {
            slots: (0..CHANNELS).map(|_| None).collect(),
            order: Vec::new(),
            listener: FmodListener::default(),
            min_distance: 4.0,
            max_distance: 20.0,
            block: vec![0; BLOCK_FRAMES * 2],
            block_used: BLOCK_FRAMES,
            scratch: Vec::new(),
        }
    }

    /// Min/max distance of the 3D samples (`FSOUND_Sample_SetMinMaxDistance`, 4/20 in the game).
    pub fn set_min_max(&mut self, min: f32, max: f32) {
        self.min_distance = min;
        self.max_distance = max;
    }

    pub fn listener(&self) -> &FmodListener {
        &self.listener
    }

    /// `FSOUND_3D_Listener_SetAttributes` and `FSOUND_Update`: take over the listener and recompute all
    /// running 3D channels (effective from the next block).
    pub fn update_3d(&mut self, listener: FmodListener) {
        self.listener = listener;
        for channel in self.slots.iter_mut().flatten() {
            if let Some(position) = channel.spatial {
                let (vol3d, pan) =
                    spatialize(&listener, position, self.min_distance, self.max_distance);
                channel.vol3d = vol3d;
                channel.pan = Pan::Position(pan);
            }
        }
    }

    /// Starts `source` on channel `slot` like `FSOUND_PlaySoundEx(paused)`,
    /// `FSOUND_3D_SetAttributes`, `FSOUND_SetVolume`, `FSOUND_SetPaused(0)`: 3D immediately with the
    /// FMOD listener, pan center (mono) or stereo, volume ramp from 0; audible from the next
    /// block. An occupied channel is stopped first.
    pub fn start(
        &mut self,
        slot: usize,
        source: Source,
        looping: bool,
        volume: i32,
        spatial: Option<[f32; 3]>,
    ) {
        self.stop(slot);
        let mono = source.channels() == 1;
        let spatial = spatial.filter(|_| mono);
        let (vol3d, pan) = match spatial {
            Some(p) => {
                let (v, pan) = spatialize(&self.listener, p, self.min_distance, self.max_distance);
                (v, Pan::Position(pan))
            }
            None if mono => (255, Pan::Position(128)),
            None => (255, Pan::Stereo),
        };
        self.slots[slot] = Some(Channel {
            source,
            looping,
            volume: volume.clamp(0, 255),
            pan,
            spatial,
            vol3d,
            current: [0; 2],
            seen: [-1; 2],
            step: [0; 2],
            ramp_left: 0,
            finished: false,
        });
        self.order.push(slot);
    }

    /// `FSOUND_StopSound`: hard, without fading out.
    pub fn stop(&mut self, slot: usize) {
        if self.slots[slot].take().is_some() {
            self.order.retain(|&s| s != slot);
        }
    }

    /// `FSOUND_SetVolume` (effective from the next block, mono with a ramp).
    pub fn set_volume(&mut self, slot: usize, volume: i32) {
        if let Some(c) = &mut self.slots[slot] {
            c.volume = volume.clamp(0, 255);
        }
    }

    /// `FSOUND_3D_SetAttributes` on a running channel: new position, computed at the
    /// next [`Mixer::update_3d`].
    pub fn set_position(&mut self, slot: usize, position: [f32; 3]) {
        if let Some(c) = &mut self.slots[slot]
            && c.spatial.is_some()
        {
            c.spatial = Some(position);
        }
    }

    pub fn channel(&self, slot: usize) -> Option<&Channel> {
        self.slots[slot].as_ref()
    }

    pub fn channel_mut(&mut self, slot: usize) -> Option<&mut Channel> {
        self.slots[slot].as_mut()
    }

    /// `FSOUND_IsPlaying`: occupied and not yet at the end.
    pub fn is_playing(&self, slot: usize) -> bool {
        self.slots[slot].as_ref().is_some_and(|c| !c.finished)
    }

    /// Occupied channels in mixing order.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// Fills `out` (interleaved stereo) with the next frames; new blocks are not mixed until
    /// their first frame is needed.
    pub fn render(&mut self, out: &mut [i16]) {
        let mut done = 0;
        while done < out.len() {
            if self.block_used == BLOCK_FRAMES {
                self.mix_block();
                self.block_used = 0;
            }
            let n = ((BLOCK_FRAMES - self.block_used) * 2).min(out.len() - done);
            let start = self.block_used * 2;
            out[done..done + n].copy_from_slice(&self.block[start..start + n]);
            self.block_used += n / 2;
            done += n;
        }
    }

    /// Mixes the next block from all occupied, unfinished channels.
    fn mix_block(&mut self) {
        self.block.fill(0);
        for &slot in &self.order {
            let Some(channel) = self.slots[slot].as_mut() else {
                continue;
            };
            if channel.finished {
                continue;
            }
            let (l, r) = channel.gains();
            let frames = channel.fill(&mut self.scratch, BLOCK_FRAMES);
            if channel.source.channels() == 2 {
                for (frame, out) in self
                    .scratch
                    .chunks_exact(2)
                    .zip(self.block.chunks_exact_mut(2))
                    .take(frames)
                {
                    out[0] = out[0].saturating_add(scale(frame[0], l << 8));
                    out[1] = out[1].saturating_add(scale(frame[1], r << 8));
                }
                continue;
            }
            // Mono: reset the ramp at the block start if the target has changed.
            let target = [l, r];
            if channel.ramp_left == 0 || target != channel.seen {
                channel.seen = target;
                channel.step =
                    [0, 1].map(|i| ((target[i] - (channel.current[i] >> 8)) << 8) / RAMP_FRAMES);
                if channel.step != [0, 0] {
                    channel.ramp_left = RAMP_FRAMES;
                }
            }
            for (&s, out) in self.scratch[..frames]
                .iter()
                .zip(self.block.chunks_exact_mut(2))
            {
                let gain = if channel.ramp_left > 0 {
                    let g = channel.current.map(|c| c & 0xFFFF);
                    channel.current = [0, 1].map(|i| channel.current[i] + channel.step[i]);
                    channel.ramp_left -= 1;
                    if channel.ramp_left == 0 {
                        channel.current = target.map(|t| t << 8);
                    }
                    g
                } else {
                    channel.current = target.map(|t| t << 8);
                    channel.current
                };
                out[0] = out[0].saturating_add(scale(s, gain[0]));
                out[1] = out[1].saturating_add(scale(s, gain[1]));
            }
        }
    }

    /// Releases finished channels (the sound thread on `FSOUND_IsPlaying` = false) and returns their
    /// slots.
    pub fn release_finished(&mut self) -> Vec<usize> {
        let done: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&s| self.slots[s].as_ref().is_some_and(|c| c.finished))
            .collect();
        for &slot in &done {
            self.stop(slot);
        }
        done
    }
}

/// `(s · vol88) >> 16` (arithmetic, hence rounded down; `pmulhw` with sign correction).
fn scale(sample: i16, vol88: i32) -> i16 {
    ((i32::from(sample) * vol88) >> 16) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(channels: usize, samples: Vec<i16>) -> Arc<Pcm> {
        Arc::new(Pcm {
            channels,
            samples: samples.into(),
        })
    }

    fn sample(pcm: &Arc<Pcm>) -> Source {
        Source::Sample {
            pcm: pcm.clone(),
            position: 0,
        }
    }

    fn render(mixer: &mut Mixer, frames: usize) -> Vec<i16> {
        let mut out = vec![0; frames * 2];
        mixer.render(&mut out);
        out
    }

    #[test]
    fn mono_sounds_ramp_in_from_zero_over_128_frames_then_play_exactly() {
        let mut mixer = Mixer::new();
        let dc = pcm(1, vec![30000; 4096]);
        mixer.start(0, sample(&dc), false, 255, None);
        let out = render(&mut mixer, 2048);
        // Ramp: vol88(n) = n * 2 * 180 (left) or n * 2 * 181 (right).
        for n in [0usize, 1, 64, 127] {
            assert_eq!(out[2 * n], ((30000 * (n as i32 * 360)) >> 16) as i16, "{n}");
            assert_eq!(
                out[2 * n + 1],
                ((30000 * (n as i32 * 362)) >> 16) as i16,
                "{n}"
            );
        }
        // Afterwards floor(s * 180/256) or floor(s * 181/256).
        assert_eq!(&out[256..260], &[21093, 21210, 21093, 21210]);
        assert_eq!(
            &out[4094..4096],
            &[21093, 21210],
            "second block without a new ramp"
        );
    }

    #[test]
    fn volume_changes_take_effect_at_the_next_block_with_a_ramp() {
        let mut mixer = Mixer::new();
        let dc = pcm(1, vec![-20000; 8192]);
        mixer.start(3, sample(&dc), false, 255, None);
        render(&mut mixer, 1000);
        mixer.set_volume(3, 76);
        let out = render(&mut mixer, 24 + 1024);
        let full = ((-20000 * (180 << 8)) >> 16) as i16;
        assert_eq!(out[0], full, "current block unchanged");
        // First frame of the new block: still old (180), then step 2 * (53 - 180).
        assert_eq!(out[48], full);
        assert_eq!(
            out[50],
            ((-20000 * (180 * 256 + 2 * (53 - 180))) >> 16) as i16
        );
        assert_eq!(out[48 + 2 * 200], ((-20000 * (53 << 8)) >> 16) as i16);
    }

    #[test]
    fn stereo_streams_play_without_ramp_and_sums_saturate_in_start_order() {
        let mut mixer = Mixer::new();
        let stereo = pcm(2, [10000, -10000].repeat(2048));
        mixer.start(5, sample(&stereo), false, 255, None);
        let out = render(&mut mixer, 2);
        assert_eq!(&out[..], &[9960, -9961, 9960, -9961]);
        let mut mixer = Mixer::new();
        let dc = pcm(1, vec![30000; 2048]);
        let neg = pcm(1, vec![-30000; 2048]);
        mixer.start(0, sample(&dc), false, 255, None);
        mixer.start(1, sample(&dc), false, 255, None);
        mixer.start(2, sample(&neg), false, 255, None);
        let out = render(&mut mixer, 1024);
        // 21093 + 21093 saturates at 32767, then - 21094.
        assert_eq!(out[2 * 500], 32767 - 21094);
    }

    #[test]
    fn samples_loop_exactly_or_end_hard_and_release_their_channel() {
        let mut mixer = Mixer::new();
        let ramp: Vec<i16> = (0..100).map(|i| i * 100).collect();
        let looped = pcm(1, ramp.clone());
        mixer.start(0, sample(&looped), true, 255, None);
        let out = render(&mut mixer, 3072);
        assert_eq!(out[2 * 2050], ((ramp[50] as i32 * (180 << 8)) >> 16) as i16);
        assert!(mixer.is_playing(0));
        let mut mixer = Mixer::new();
        let short = pcm(1, vec![8000; 1500]);
        mixer.start(7, sample(&short), false, 255, None);
        let out = render(&mut mixer, 2048);
        assert_ne!(out[2 * 1499], 0);
        assert_eq!(out[2 * 1500], 0, "end without fading out");
        assert!(!mixer.is_playing(7));
        assert_eq!(mixer.release_finished(), [7]);
        assert!(mixer.channel(7).is_none() && mixer.order().is_empty());
    }

    #[test]
    fn spatial_channels_use_the_listener_and_follow_updates() {
        let mut mixer = Mixer::new();
        let dc = pcm(1, vec![25600; 8192]);
        // Source 8 units to the right: vol3d 127, fully right.
        mixer.start(0, sample(&dc), false, 255, Some([8.0, 0.0, 0.0]));
        let c = mixer.channel(0).unwrap();
        assert_eq!(
            (c.vol3d(), c.pan(), c.gains()),
            (127, Pan::Position(255), (0, 127))
        );
        // The listener turns around: source now on the left.
        mixer.update_3d(FmodListener::new(
            [0.0; 3],
            [0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0],
        ));
        assert_eq!(mixer.channel(0).unwrap().gains(), (127, 0));
        // Stereo samples stay 2D.
        let stereo = pcm(2, vec![0; 64]);
        mixer.start(1, sample(&stereo), false, 255, Some([8.0, 0.0, 0.0]));
        assert_eq!(mixer.channel(1).unwrap().spatial(), None);
    }
}
