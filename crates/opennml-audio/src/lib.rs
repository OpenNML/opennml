//! Sound as in the original (re/audio): sound database, channel allocation, volume, 3D sound,
//! music state machine and a custom mixer (44,100 Hz, stereo) that feeds the same samples to the
//! audio device (`cpal`) and to the offline mix (WAV). It has no dependency on graphics or
//! windows; everything runs on the viewer's virtual clock.

pub mod capture;
pub mod db;
pub mod decode;
pub mod driver;
pub mod engine;
pub mod manager;
pub mod mixer;
pub mod music;
pub mod output;
pub mod pool;
pub mod resample;
pub mod spatial;
pub mod volume;

/// Mixing rate (`FSOUND_Init(44100, ...)`).
pub const SAMPLE_RATE: u32 = 44_100;

/// Seed of the random generator used for sound selection (`rand()` per subsystem).
pub const AUDIO_SEED: u32 = 3;
