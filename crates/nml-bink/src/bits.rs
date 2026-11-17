/*
 * Copyright (c) 2004 Michael Niedermayer <michaelni@gmx.at>
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

//! Bit reader, little-endian (lowest bit first).
//!
//! Ported from FFmpeg n9.0.2, `libavcodec/get_bits.h` (reader with `BITSTREAM_READER_LE`:
//! `init_get_bits`, `get_bits`, `get_bits1`, `show_bits`, `skip_bits`, `get_bits_count`,
//! `get_bits_left`).
//! Unlike FFmpeg (which reads into the padding past the end or limits the position at the end), reading or
//! skipping past the end returns an error.
//!
//! Modification notice (LGPL-2.1 section 2 b): Ported to Rust and modified for `nml-bink`, 2026-10-25.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use crate::error::{BinkError, Result};

/// Reader over a byte buffer; bit 0 of the stream is the lowest bit of byte 0.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Bit position (`GetBitContext.index`).
    pos: usize,
}

impl<'a> BitReader<'a> {
    /// Starts at the beginning of `data` (`init_get_bits`).
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// Bits read so far (`get_bits_count`).
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bits not yet read (`get_bits_left`).
    pub fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    fn overrun(&self, want: usize) -> BinkError {
        BinkError::Format(format!(
            "bitstream ends too early: {want} bits requested, {} left",
            self.remaining()
        ))
    }

    /// Reads `n` bits (0 to 32) as a number; the bit read first is the lowest (`get_bits`).
    pub fn read(&mut self, n: u32) -> Result<u32> {
        let v = self.peek(n)?;
        let n = n as usize;
        if n > self.remaining() {
            return Err(self.overrun(n));
        }
        self.pos += n;
        Ok(v)
    }

    /// Shows the next `n` bits (0 to 32) without changing the position (`show_bits`); past the end of the
    /// buffer there are zero bits as in FFmpeg's padding (for the table lookup of the Huffman codes,
    /// `get_vlc2`). More than 32 bits is an error.
    pub fn peek(&self, n: u32) -> Result<u32> {
        if n > 32 {
            return Err(BinkError::Format(format!(
                "{n} bits requested at once (at most 32)"
            )));
        }
        let n = n as usize;
        if n == 0 {
            return Ok(0);
        }
        let first = self.pos / 8;
        let shift = self.pos % 8;
        // At most 32 + 7 bits fit in five bytes.
        let take = (shift + n).div_ceil(8);
        let mut v = 0u64;
        for (i, b) in self.data.iter().skip(first).take(take).enumerate() {
            v |= u64::from(*b) << (8 * i);
        }
        Ok(((v >> shift) & ((1u64 << n) - 1)) as u32)
    }

    /// Reads one bit (`get_bits1`).
    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read(1)? != 0)
    }

    /// Skips `n` bits (`skip_bits`).
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if n > self.remaining() {
            return Err(self.overrun(n));
        }
        self.pos += n;
        Ok(())
    }

    /// Advances to the next 32-bit boundary (`bink.c`, `bink_decode_plane` and `decode_frame`: the data of the
    /// next plane begins there); at the end of the buffer the position stays where it is.
    pub fn align32(&mut self) {
        let aligned = self.pos.next_multiple_of(32);
        self.pos = aligned.min(self.data.len() * 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_read_little_endian_order() {
        let data = [0b1010_0001u8, 0xFF];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(1).unwrap(), 1);
        assert_eq!(r.read(4).unwrap(), 0);
        assert_eq!(r.read(3).unwrap(), 5);
        assert_eq!(r.read(8).unwrap(), 0xFF);
        assert_eq!(r.remaining(), 0);
        assert!(matches!(r.read(1), Err(crate::BinkError::Format(_))));
        assert!(matches!(r.read_bit(), Err(crate::BinkError::Format(_))));
        assert!(matches!(r.skip(1), Err(crate::BinkError::Format(_))));
    }

    #[test]
    fn bits_read_crosses_bytes_and_full_width() {
        let data = [0x78, 0x56, 0x34, 0x12, 0xEF, 0xCD];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(4).unwrap(), 0x8);
        // 32 bits from bit position 4: the low nibble of byte 4 is on top.
        assert_eq!(r.read(32).unwrap(), 0xF123_4567);
        assert_eq!(r.position(), 36);
        assert_eq!(r.remaining(), 12);
        assert_eq!(r.read(0).unwrap(), 0);
        assert_eq!(r.read(12).unwrap(), 0xCDE);
        assert!(r.read(1).is_err());
        // A failed read does not change the position.
        assert_eq!(r.position(), 48);
    }

    #[test]
    fn bits_more_than_32_are_an_error() {
        let data = [0xFFu8; 8];
        let mut r = BitReader::new(&data);
        assert!(matches!(r.read(33), Err(crate::BinkError::Format(_))));
        assert!(matches!(r.peek(40), Err(crate::BinkError::Format(_))));
        assert_eq!(r.position(), 0);
        assert_eq!(r.read(32).unwrap(), 0xFFFF_FFFF);
    }

    #[test]
    fn bits_peek_pads_with_zeros_and_keeps_position() {
        let data = [0b1011_0110u8];
        let mut r = BitReader::new(&data);
        r.skip(4).unwrap();
        assert_eq!(r.peek(7).unwrap(), 0b1011);
        assert_eq!(r.position(), 4);
        assert_eq!(r.read(4).unwrap(), 0b1011);
        assert_eq!(r.peek(8).unwrap(), 0);
    }

    #[test]
    fn bits_align_to_32() {
        let data = [0u8; 12];
        let mut r = BitReader::new(&data);
        r.align32();
        assert_eq!(r.position(), 0);
        r.skip(1).unwrap();
        r.align32();
        assert_eq!(r.position(), 32);
        r.skip(31).unwrap();
        r.align32();
        assert_eq!(r.position(), 64);
        r.skip(30).unwrap();
        assert_eq!(r.position(), 94);
        r.align32();
        assert_eq!(r.position(), 96);
        assert_eq!(r.remaining(), 0);
        // Past the end, aligning stays at the end.
        let short = [0u8; 2];
        let mut r = BitReader::new(&short);
        r.skip(3).unwrap();
        r.align32();
        assert_eq!(r.position(), 16);
    }
}
