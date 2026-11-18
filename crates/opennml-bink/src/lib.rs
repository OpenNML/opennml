//! Bink 1 decoder for the videos of "No Man's Land" (revision `i`, frames and DCT sound).
//!
//! Parts of this crate are ported from FFmpeg (tag n9.0.2); the crate is therefore licensed under
//! LGPL-2.1-or-later (see `README.md` and `COPYING.LGPLv2.1`).

pub mod audio;
pub mod bits;
pub mod color;
pub mod container;
mod dct;
mod dsp;
pub mod error;
mod tables;
pub mod video;

pub use audio::AudioDecoder;
pub use bits::BitReader;
pub use color::to_rgba;
pub use container::{AudioTrack, BinkHeader, BinkReader, FrameEntry, FramePackets};
pub use error::{BinkError, Result};
pub use video::{VideoDecoder, VideoFrame, YUV_HASH_BASIS, yuv_hash};
