/*
 * Bink Audio decoder
 * Copyright (c) 2007-2011 Peter Ross (pross@xvid.org)
 * Copyright (c) 2009 Daniel Verkamp (daniel@drv.nu)
 *
 * This file is part of FFmpeg.
 *
 * FFmpeg is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 *
 * FFmpeg is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with FFmpeg; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 */

//! Bink sound decoder (DCT path).
//!
//! Ported from FFmpeg n9.0.2, `libavcodec/binkaudio.c` (`decode_init`, `get_float`, `decode_block`,
//! `binkaudio_receive_frame` for `AV_CODEC_ID_BINKAUDIO_DCT`). Not ported: the RDFT path and revision `b`
//! (`version_b`), also more than two channels (the container knows only mono and stereo).
//!
//! Every packet begins with a 32-bit size, which is skipped; blocks follow, each aligned to 32 bits, up to the
//! end of the packet. A block contains `frame_len` coefficients per channel and, after the inverse DCT and the
//! crossfade with the end of the previous block, yields `frame_len − frame_len/16` samples per channel. The
//! channels are decoded separately and interleaved only on output (FFmpeg outputs `AV_SAMPLE_FMT_FLTP`).
//!
//! Deviations from FFmpeg, effective only for corrupted or other data (none affects the audio tracks of the
//! game):
//!
//! - Reading past the end of the packet yields [`BinkError::Format`] (FFmpeg reads zero bits of the padding
//!   there). The state then stays as before the block; the samples of the complete blocks before it have
//!   already been appended, just as FFmpeg had already output them before the error.
//! - If the packet ends after the last block without being padded to 32 bits, this is `Ok` here (aligning ends
//!   at the end of the packet, the block counts); FFmpeg also outputs the block, but additionally reports
//!   "Incomplete packet" afterwards (after `get_bits_align32`, `get_bits_left` is negative, the packet is not
//!   released, and the next `decode_block` fails).
//! - [`AudioDecoder::new`] rejects an audio track with sample rate 0 with [`BinkError::Format`]; FFmpeg does
//!   not check the rate in `decode_init` (the band setup then divides by `sample_rate_half = 0`).
//!
//! When building the quantization table, FFmpeg uses `expf`, here `f32::exp`; the decoding itself computes in
//! `f32` in a fixed order.
//!
//! Modification notice (LGPL-2.1 section 2 b): Ported to Rust and modified for `opennml-bink`, 2026-10-25.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use crate::bits::BitReader;
use crate::container::AudioTrack;
use crate::dct::DctIII;
use crate::error::{BinkError, Result};
use crate::tables::{RLE_LENGTH_TAB, WMA_CRITICAL_FREQS};

/// `MAX_CHANNELS`: at most two channels per block.
const MAX_CHANNELS: usize = 2;

/// Decoder of a Bink audio track in the DCT path.
pub struct AudioDecoder {
    sample_rate: u32,
    channels: usize,
    /// Length of the transform in samples (`frame_len`).
    frame_len: usize,
    /// Length of the crossfade (`overlap_len = frame_len / 16`).
    overlap_len: usize,
    num_bands: usize,
    /// Band boundaries; `bands[num_bands] == frame_len`.
    bands: [usize; 26],
    root: f32,
    quant_table: [f32; 96],
    /// No block decoded yet (`s->first`): no crossfade.
    first: bool,
    /// Per channel the last `overlap_len` values of the previous block (`s->previous`).
    previous: [Vec<f32>; MAX_CHANNELS],
    dct: DctIII,
    /// Per channel the block after the transform.
    block: [Vec<f32>; MAX_CHANNELS],
}

impl AudioDecoder {
    /// Sets up the decoder for `track` (`decode_init`). RDFT sound is rejected.
    pub fn new(track: &AudioTrack) -> Result<Self> {
        if !track.dct() {
            return Err(BinkError::Unsupported(
                "audio track in the RDFT method (only DCT sound is decoded)".into(),
            ));
        }
        let sample_rate = u32::from(track.sample_rate);
        if sample_rate == 0 {
            return Err(BinkError::Format("audio track with sample rate 0".into()));
        }
        let channels = if track.stereo() { 2 } else { 1 };

        // Length of the transform according to the rate.
        let frame_len_bits = if sample_rate < 22050 {
            9
        } else if sample_rate < 44100 {
            10
        } else {
            11
        };
        let frame_len = 1usize << frame_len_bits;
        let overlap_len = frame_len / 16;
        let sample_rate_half = u64::from(sample_rate).div_ceil(2);
        // `frame_len / (sqrt(frame_len) * 32768.0)` in `double`, stored as `float`.
        let root = (frame_len as f64 / ((frame_len as f64).sqrt() * 32768.0)) as f32;
        let mut quant_table = [0.0f32; 96];
        for (i, q) in quant_table.iter_mut().enumerate() {
            // `expf(i * 0.15289164787221953823f) * root` (as `f32` the same number as the literal here);
            // the constant is 0.066399999 / log10(e).
            *q = (i as f32 * 0.152_891_65_f32).exp() * root;
        }

        // Number of bands: the first band whose cutoff frequency reaches half the rate.
        let mut num_bands = 1;
        while num_bands < 25 {
            if sample_rate_half <= u64::from(WMA_CRITICAL_FREQS[num_bands - 1]) {
                break;
            }
            num_bands += 1;
        }
        // Band boundaries; non-decreasing because the cutoff frequencies rise, and below `frame_len` because
        // their frequencies are below half the rate.
        let mut bands = [0usize; 26];
        bands[0] = 2;
        for i in 1..num_bands {
            let b = u64::from(WMA_CRITICAL_FREQS[i - 1]) * frame_len as u64 / sample_rate_half;
            bands[i] = (b & !1) as usize;
        }
        bands[num_bands] = frame_len;

        Ok(AudioDecoder {
            sample_rate,
            channels,
            frame_len,
            overlap_len,
            num_bands,
            bands,
            root,
            quant_table,
            first: true,
            previous: [vec![0.0; overlap_len], vec![0.0; overlap_len]],
            // `av_tx_init(AV_TX_FLOAT_DCT, inv, 1 << (frame_len_bits - 1), 1.0 / (1 << frame_len_bits))`.
            dct: DctIII::new(frame_len, 1.0 / frame_len as f32),
            block: [vec![0.0; frame_len], vec![0.0; frame_len]],
        })
    }

    /// Number of channels (1 or 2).
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Decodes an audio packet (with the 32-bit size up front) and appends the interleaved samples to
    /// `out` (`binkaudio_receive_frame`).
    pub fn decode_packet(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<()> {
        if packet.len() < 4 {
            return Err(BinkError::Format(format!(
                "audio packet too small ({} bytes)",
                packet.len()
            )));
        }
        let mut gb = BitReader::new(packet);
        // Skip the reported size.
        gb.skip(32)?;
        loop {
            self.decode_block(&mut gb)?;
            let n = self.frame_len - self.overlap_len;
            out.reserve(n * self.channels);
            for i in 0..n {
                for ch in 0..self.channels {
                    out.push(self.block[ch][i]);
                }
            }
            // get_bits_align32; the packet is finished if no bits are left afterwards.
            gb.align32();
            if gb.remaining() == 0 {
                return Ok(());
            }
        }
    }

    /// Decodes a block of all channels into `self.block` and crossfades it with the previous one
    /// (`decode_block` with `use_dct`). On an error, `first` and `previous` stay unchanged.
    fn decode_block(&mut self, gb: &mut BitReader<'_>) -> Result<()> {
        let frame_len = self.frame_len;
        gb.skip(2)?;
        for ch in 0..self.channels {
            let coeffs = &mut self.block[ch];
            if gb.remaining() < 58 {
                return Err(incomplete());
            }
            coeffs[0] = get_float(gb)? * self.root;
            coeffs[1] = get_float(gb)? * self.root;

            if gb.remaining() < self.num_bands * 8 {
                return Err(incomplete());
            }
            let mut quant = [0.0f32; 25];
            for q in quant.iter_mut().take(self.num_bands) {
                let value = gb.read(8)? as usize;
                *q = self.quant_table[value.min(95)];
            }

            let mut k = 0usize;
            let mut q = quant[0];
            // Read coefficients; `k` stays at most `num_bands` because `bands` does not decrease and
            // `bands[num_bands] == frame_len > i` holds.
            let mut i = 2usize;
            while i < frame_len {
                let j = if gb.read_bit()? {
                    let v = gb.read(4)? as usize;
                    i + usize::from(RLE_LENGTH_TAB[v]) * 8
                } else {
                    i + 8
                };
                let j = j.min(frame_len);
                let width = gb.read(4)?;
                if width == 0 {
                    coeffs[i..j].fill(0.0);
                    i = j;
                    while self.bands[k] < i {
                        q = quant[k];
                        k += 1;
                    }
                } else {
                    while i < j {
                        if self.bands[k] == i {
                            q = quant[k];
                            k += 1;
                        }
                        let coeff = gb.read(width)?;
                        coeffs[i] = if coeff != 0 {
                            if gb.read_bit()? {
                                -q * coeff as f32
                            } else {
                                q * coeff as f32
                            }
                        } else {
                            0.0
                        };
                        i += 1;
                    }
                }
            }
        }

        for coeffs in self.block.iter_mut().take(self.channels) {
            coeffs[0] /= 0.5;
            self.dct.transform(coeffs);
        }

        // Crossfade with the end of the previous block; the weights run over the interleaved
        // channels (`j = ch + i·channels`, `count = overlap_len·channels`).
        let count = (self.overlap_len * self.channels) as i32;
        for ch in 0..self.channels {
            let block = &mut self.block[ch];
            let previous = &mut self.previous[ch];
            if !self.first {
                let mut j = ch as i32;
                for i in 0..self.overlap_len {
                    block[i] =
                        (previous[i] * (count - j) as f32 + block[i] * j as f32) / count as f32;
                    j += self.channels as i32;
                }
            }
            previous.copy_from_slice(&block[frame_len - self.overlap_len..]);
        }
        self.first = false;
        Ok(())
    }
}

/// Error for a block that does not lie completely in the packet ("Incomplete packet").
fn incomplete() -> BinkError {
    BinkError::Format("audio packet ends in the middle of a block".into())
}

/// Floating-point number from 5 bits of exponent, 23 bits of mantissa and sign (`get_float`):
/// `ldexpf(mantissa, exponent − 23)`, exact.
fn get_float(gb: &mut BitReader<'_>) -> Result<f32> {
    let power = gb.read(5)? as i32;
    let mantissa = gb.read(23)? as f32;
    // 2^(power - 23) lies between 2^-23 and 2^8 and is exactly representable as `f32`.
    let scale = f32::from_bits(((power - 23 + 127) as u32) << 23);
    let f = mantissa * scale;
    Ok(if gb.read_bit()? { -f } else { f })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(sample_rate: u16, stereo: bool) -> AudioTrack {
        AudioTrack {
            sample_rate,
            flags: 0x4000 | 0x1000 | if stereo { 0x2000 } else { 0 },
            id: 0,
        }
    }

    #[test]
    fn decoder_parameters_follow_the_rate() {
        for (rate, frame_len, bands) in [
            (22050u16, 1024usize, 23usize),
            (44100, 2048, 25),
            (48000, 2048, 25),
        ] {
            let d = AudioDecoder::new(&track(rate, true)).unwrap();
            assert_eq!(d.sample_rate(), u32::from(rate));
            assert_eq!(d.channels(), 2);
            assert_eq!(d.frame_len, frame_len, "{rate}");
            assert_eq!(d.overlap_len, frame_len / 16, "{rate}");
            assert_eq!(d.num_bands, bands, "{rate}");
            assert_eq!(d.bands[0], 2, "{rate}");
            assert_eq!(d.bands[d.num_bands], frame_len, "{rate}");
            assert!(
                d.bands[..=d.num_bands].windows(2).all(|w| w[0] <= w[1]),
                "{rate}"
            );
            assert_eq!(d.block[0].len(), frame_len, "{rate}");
        }
        // 44,100 Hz: first band boundaries 100*2048/22050 = 9 -> 8 and 200*2048/22050 = 18.
        let d = AudioDecoder::new(&track(44100, false)).unwrap();
        assert_eq!(d.channels(), 1);
        assert_eq!(&d.bands[..3], &[2, 8, 18]);
        // root = 2048 / (sqrt(2048) * 32768).
        assert_eq!(d.root, (2048.0 / (2048f64.sqrt() * 32768.0)) as f32);
        assert_eq!(d.quant_table[0], d.root);
    }

    #[test]
    fn decoder_rejects_rdft_and_rate_zero() {
        let rdft = AudioTrack {
            sample_rate: 44100,
            flags: 0x6000,
            id: 0,
        };
        assert!(matches!(
            AudioDecoder::new(&rdft),
            Err(BinkError::Unsupported(_))
        ));
        assert!(matches!(
            AudioDecoder::new(&track(0, true)),
            Err(BinkError::Format(_))
        ));
    }

    /// Bits of a silent block: 2 bits, per channel two floating-point numbers (58 bits), per band 8 bits of
    /// quantization, per group of eight from coefficient 2 one bit "no run length" and 4 bits of width 0.
    fn silent_block_bits(d: &AudioDecoder) -> usize {
        let groups = (d.frame_len - 2).div_ceil(8);
        2 + d.channels * (58 + d.num_bands * 8 + groups * 5)
    }

    #[test]
    fn silent_packets_yield_blocks_of_zeros() {
        for (rate, stereo) in [(22050u16, true), (44100, false), (48000, true)] {
            let mut d = AudioDecoder::new(&track(rate, stereo)).unwrap();
            let block = silent_block_bits(&d).next_multiple_of(32) / 8;
            let per_block = (d.frame_len - d.overlap_len) * d.channels;
            let mut out = Vec::new();
            d.decode_packet(&vec![0u8; 4 + block], &mut out).unwrap();
            assert_eq!(out.len(), per_block, "{rate}");
            d.decode_packet(&vec![0u8; 4 + 3 * block], &mut out)
                .unwrap();
            assert_eq!(out.len(), 4 * per_block, "{rate}");
            assert!(out.iter().all(|&v| v == 0.0), "{rate}");
            // Without the padding to 32 bits the block still decodes (aligning ends at the end of the
            // packet); one byte less, and it ends in the middle of reading.
            let raw = silent_block_bits(&d).div_ceil(8);
            let mut unpadded = Vec::new();
            d.decode_packet(&vec![0u8; 4 + raw], &mut unpadded).unwrap();
            assert_eq!(unpadded.len(), per_block, "{rate}");
            let mut short = Vec::new();
            assert!(
                d.decode_packet(&vec![0u8; 4 + raw - 1], &mut short)
                    .is_err()
            );
            assert!(d.decode_packet(&[0u8; 3], &mut short).is_err());
            assert!(d.decode_packet(&[0u8; 4], &mut short).is_err());
            assert!(short.is_empty());
        }
    }

    /// A DC value in the first block yields a constant block; the second, silent block crossfades linearly in
    /// the first `overlap_len` values from the end of the first to 0.
    #[test]
    fn second_block_crossfades_from_the_first() {
        let mut d = AudioDecoder::new(&track(22050, false)).unwrap();
        let block = silent_block_bits(&d).next_multiple_of(32) / 8;
        let mut packet = vec![0u8; 4 + 2 * block];
        // First floating-point number after the 2 bits: exponent 23 (5 bits), mantissa 1 (23 bits) -> 1.0.
        let mut bits = 2usize;
        for (v, n) in [(23u32, 5usize), (1, 23)] {
            for i in 0..n {
                if v >> i & 1 != 0 {
                    packet[4 + bits / 8] |= 1 << (bits % 8);
                }
                bits += 1;
            }
        }
        let mut out = Vec::new();
        d.decode_packet(&packet, &mut out).unwrap();
        let n = d.frame_len - d.overlap_len;
        assert_eq!(out.len(), 2 * n);
        let dc = out[0];
        assert!(dc > 0.0);
        assert!(out[..n].iter().all(|&v| (v - dc).abs() <= dc * 1e-5));
        let count = d.overlap_len as f32;
        for i in 0..d.overlap_len {
            let want = dc * (count - i as f32) / count;
            assert!((out[n + i] - want).abs() <= dc * 1e-5, "Wert {i}");
        }
        assert!(out[n + d.overlap_len..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn corrupt_audio_packets_never_panic() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut decoders = [
            AudioDecoder::new(&track(22050, true)).unwrap(),
            AudioDecoder::new(&track(44100, false)).unwrap(),
            AudioDecoder::new(&track(48000, true)).unwrap(),
        ];
        let (mut ok, mut err) = (0, 0);
        for k in 0..600 {
            let len = (next() % 1200) as usize;
            let mut packet: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Some of the packets start like a silent packet and are only partly corrupted with noise.
            if k % 3 == 0 {
                for (i, b) in packet.iter_mut().enumerate() {
                    if i % 7 != 0 {
                        *b = 0;
                    }
                }
            }
            let mut out = Vec::new();
            match decoders[k % 3].decode_packet(&packet, &mut out) {
                Ok(()) => ok += 1,
                Err(_) => err += 1,
            }
            assert!(out.iter().all(|v| !v.is_nan()));
        }
        assert!(err > 0);
        let _ = ok;
    }
}
