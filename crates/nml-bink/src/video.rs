/*
 * Bink video decoder
 * Copyright (c) 2009 Konstantin Shishkov
 * Copyright (C) 2011 Peter Ross <pross@xvid.org>
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

//! Bink 1 frame decoder (revision `i`, YUV 4:2:0 without alpha).
//!
//! Ported from FFmpeg n9.0.2, `libavcodec/bink.c` (`decode_frame`, `bink_decode_plane` and their
//! reading functions). Not ported: revision `b` (`binkb_*`), the branches of revision `k`, the alpha plane
//! and revisions before `h` (sign of the color values). For revision `i`, `swap_planes` applies: in the
//! bitstream the planes follow in the order Y, V, U.
//!
//! Deviations from FFmpeg, effective only for corrupted data: reading past the packet (FFmpeg reads zero bits of
//! the padding there), an exhausted bundle (FFmpeg then reads memory outside the bundle) and a motion vector outside
//! the
//! plane result in [`BinkError::Format`]; the frame is discarded and the previous one remains the reference, as
//! in FFmpeg after an error.
//!
//! Modification notice (LGPL-2.1 section 2 b): Ported to Rust and modified for `nml-bink`, 2026-10-25.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use crate::bits::BitReader;
use crate::container::BinkHeader;
use crate::dsp;
use crate::error::{BinkError, Result};
use crate::tables::{
    BINK_INTER_QUANT, BINK_INTRA_QUANT, BINK_PATTERNS, BINK_RLELENS, BINK_SCAN, BINK_TREE_BITS,
    BINK_TREE_LENS,
};

/// `BINK_FLAG_ALPHA`.
const BINK_FLAG_ALPHA: u32 = 0x0010_0000;
/// `BINK_FLAG_GRAY`.
const BINK_FLAG_GRAY: u32 = 0x0002_0000;
/// Upper limit for width and height (as in the container).
const MAX_DIMENSION: u32 = 4096;
/// Alignment of width and height of the Y plane (`utils.c` `avcodec_align_dimensions2`: for
/// `AV_PIX_FMT_YUV420P` `h_align = 16 * 2`, for `AV_CODEC_ID_BINKVIDEO` also `w_align = 16*2`); the color
/// planes have half.
const ALIGN: usize = 32;

// Bundles (`enum Sources`).
const SRC_BLOCK_TYPES: usize = 0;
const SRC_SUB_BLOCK_TYPES: usize = 1;
const SRC_COLORS: usize = 2;
const SRC_PATTERN: usize = 3;
const SRC_X_OFF: usize = 4;
const SRC_Y_OFF: usize = 5;
const SRC_INTRA_DC: usize = 6;
const SRC_INTER_DC: usize = 7;
const SRC_RUN: usize = 8;
const NB_SRC: usize = 9;

// Block kinds (`enum BlockTypes`).
const SKIP_BLOCK: i32 = 0;
const SCALED_BLOCK: i32 = 1;
const MOTION_BLOCK: i32 = 2;
const RUN_BLOCK: i32 = 3;
const RESIDUE_BLOCK: i32 = 4;
const INTRA_BLOCK: i32 = 5;
const FILL_BLOCK: i32 = 6;
const INTER_BLOCK: i32 = 7;
const PATTERN_BLOCK: i32 = 8;
const RAW_BLOCK: i32 = 9;

/// Bits of the first DC value of a bundle (`DC_START_BITS`).
const DC_START_BITS: u32 = 11;

/// Start value of the FNV-1a-64 checksum for [`yuv_hash`].
pub const YUV_HASH_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn bad(msg: impl Into<String>) -> BinkError {
    BinkError::Format(msg.into())
}

/// A decoded frame in YUV 4:2:0.
///
/// The planes are rounded up as in FFmpeg's frame buffer: Y to width and height in multiples of 32, U and V
/// to half (multiples of 16). This way every 16 x 16 block (`SCALED_BLOCK`, in the color planes 8 x 8 pixels
/// per block unit) fits completely into the plane even in the last block column or row with an odd number of
/// blocks, instead of writing past the end of the row into the next row.
/// `strides` are the row strides of Y, U, V (for 640 pixels width 640/320 as FFmpeg's `linesize`;
/// FFmpeg additionally rounds `linesize` up to `STRIDE_ALIGN`, which changes nothing at 640). The
/// visible pixels are provided by [`VideoFrame::plane`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// Visible width in pixels.
    pub width: u32,
    /// Visible height in pixels.
    pub height: u32,
    /// Brightness.
    pub y: Vec<u8>,
    /// Color U (Cb), half resolution.
    pub u: Vec<u8>,
    /// Color V (Cr), half resolution.
    pub v: Vec<u8>,
    /// Row strides of Y, U, V in bytes.
    pub strides: [usize; 3],
}

impl VideoFrame {
    /// Black (filled with 0) frame of size `width` x `height`, planes rounded up (see above).
    pub fn new(width: u32, height: u32) -> Self {
        let (w, h) = (width as usize, height as usize);
        let luma = (w.next_multiple_of(ALIGN), h.next_multiple_of(ALIGN));
        let chroma = (luma.0 / 2, luma.1 / 2);
        VideoFrame {
            width,
            height,
            y: vec![0; luma.0 * luma.1],
            u: vec![0; chroma.0 * chroma.1],
            v: vec![0; chroma.0 * chroma.1],
            strides: [luma.0, chroma.0, chroma.0],
        }
    }

    /// Visible size of plane `i` (0 = Y, 1 = U, 2 = V); color planes halved and rounded up like
    /// FFmpeg's `AV_CEIL_RSHIFT`.
    pub fn plane_size(&self, i: usize) -> (usize, usize) {
        let (w, h) = (self.width as usize, self.height as usize);
        if i == 0 {
            (w, h)
        } else {
            (w.div_ceil(2), h.div_ceil(2))
        }
    }

    /// The visible rows of plane `i` (0 = Y, 1 = U, 2 = V), cropped to [`Self::plane_size`];
    /// any other index yields no rows.
    pub fn plane(&self, i: usize) -> impl Iterator<Item = &[u8]> {
        let (data, stride): (&[u8], usize) = match i {
            0 => (&self.y, self.strides[0]),
            1 => (&self.u, self.strides[1]),
            2 => (&self.v, self.strides[2]),
            _ => (&[], 1),
        };
        let (w, h) = if i < 3 { self.plane_size(i) } else { (0, 0) };
        data.chunks(stride.max(1))
            .take(h)
            .map(move |row| &row[..w.min(row.len())])
    }
}

/// FNV-1a 64 over the visible planes Y, U, V of `frame`, continued from `hash` (start value
/// [`YUV_HASH_BASIS`]); chained frame by frame, this yields the FFmpeg reference checksums.
pub fn yuv_hash(frame: &VideoFrame, hash: &mut u64) {
    for i in 0..3 {
        for row in frame.plane(i) {
            for &b in row {
                *hash ^= u64::from(b);
                *hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
    }
}

/// Huffman table of one of the 16 fixed trees: index = the next `bits` bits (lowest first),
/// value = (leaf, code length). Replaces FFmpeg's `VLC` with `VLC_INIT_LE` and `get_vlc2(…, 1)`.
#[derive(Debug, Clone)]
struct Vlc {
    bits: u32,
    table: Vec<(u8, u8)>,
}

impl Vlc {
    /// From `bink.c` `bink_init_vlcs`.
    fn new(tree: usize) -> Self {
        let lens = &BINK_TREE_LENS[tree];
        let codes = &BINK_TREE_BITS[tree];
        let bits = u32::from(lens[15]);
        let mut table = vec![(0u8, 0u8); 1 << bits];
        for leaf in 0..16 {
            let len = u32::from(lens[leaf]);
            for fill in 0..1usize << (bits - len) {
                table[usize::from(codes[leaf]) | fill << len] = (leaf as u8, len as u8);
            }
        }
        Vlc { bits, table }
    }

    /// Reads a leaf (`get_vlc2`).
    fn read(&self, gb: &mut BitReader) -> Result<usize> {
        let idx = gb.peek(self.bits)? as usize;
        let (leaf, len) = self.table.get(idx).copied().unwrap_or((0, 0));
        if len == 0 {
            return Err(bad("invalid Huffman code"));
        }
        gb.skip(usize::from(len))?;
        Ok(usize::from(leaf))
    }
}

/// From `bink.c` `Tree`: number of the tree and mapping leaf -> symbol.
#[derive(Debug, Clone, Copy, Default)]
struct Tree {
    vlc_num: usize,
    syms: [u8; 16],
}

/// From `bink.c` `Bundle`. `cur_dec`/`cur_ptr` are byte positions in `data`; DC bundles store
/// `int16_t` little-endian as FFmpeg does in the same byte buffer.
#[derive(Debug, Clone)]
struct Bundle {
    len: u32,
    tree: Tree,
    data: Vec<u8>,
    /// `None` corresponds to `cur_dec = NULL` (bundle finished for this plane).
    cur_dec: Option<usize>,
    cur_ptr: usize,
}

/// From `bink.c` `GET_HUFF`.
#[inline]
fn get_huff(gb: &mut BitReader, vlc: &[Vlc; 16], tree: &Tree) -> Result<u8> {
    let leaf = vlc[tree.vlc_num].read(gb)?;
    Ok(tree.syms[leaf])
}

/// From `bink.c` `merge`: merge two consecutive lists of equal length according to read bits.
fn merge(gb: &mut BitReader, dst: &mut [u8], src: &[u8], size: usize) -> Result<()> {
    let (mut i1, mut i2) = (0, size);
    let (mut n1, mut n2) = (size, size);
    let mut d = 0;
    loop {
        if !gb.read_bit()? {
            dst[d] = src[i1];
            i1 += 1;
            n1 -= 1;
        } else {
            dst[d] = src[i2];
            i2 += 1;
            n2 -= 1;
        }
        d += 1;
        if n1 == 0 || n2 == 0 {
            break;
        }
    }
    for _ in 0..n1 {
        dst[d] = src[i1];
        d += 1;
        i1 += 1;
    }
    for _ in 0..n2 {
        dst[d] = src[i2];
        d += 1;
        i2 += 1;
    }
    Ok(())
}

/// From `bink.c` `read_tree`. As in FFmpeg, entries of `syms` that are not set anew are preserved.
fn read_tree(gb: &mut BitReader, tree: &mut Tree) -> Result<()> {
    if gb.remaining() < 4 {
        return Err(bad("bitstream ends before a Huffman tree"));
    }
    tree.vlc_num = gb.read(4)? as usize;
    if tree.vlc_num == 0 {
        tree.syms = std::array::from_fn(|i| i as u8);
        return Ok(());
    }
    if gb.read_bit()? {
        let mut len = gb.read(3)? as usize;
        let mut used = [false; 16];
        for i in 0..=len {
            let s = gb.read(4)? as u8;
            tree.syms[i] = s;
            used[usize::from(s)] = true;
        }
        for (i, &u) in used.iter().enumerate() {
            if len >= 15 {
                break;
            }
            if !u {
                len += 1;
                tree.syms[len] = i as u8;
            }
        }
    } else {
        let len = gb.read(2)? as usize;
        let mut cur: [u8; 16] = std::array::from_fn(|i| i as u8);
        let mut out = [0u8; 16];
        for i in 0..=len {
            let size = 1 << i;
            for t in (0..16).step_by(size << 1) {
                merge(gb, &mut out[t..], &cur[t..], size)?;
            }
            std::mem::swap(&mut cur, &mut out);
        }
        tree.syms = cur;
    }
    Ok(())
}

/// From `bink.c` `CHECK_READ_VAL`: number of new entries, or `None` if there is (still) nothing to read.
fn check_read_val(gb: &mut BitReader, b: &mut Bundle) -> Result<Option<(usize, usize)>> {
    let Some(dec) = b.cur_dec else {
        return Ok(None);
    };
    if dec > b.cur_ptr {
        return Ok(None);
    }
    let t = gb.read(b.len)? as usize;
    if t == 0 {
        b.cur_dec = None;
        return Ok(None);
    }
    Ok(Some((dec, t)))
}

/// End of the new entries, checked against the end of the bundle (`dec_end > b->data_end`).
fn dec_end(b: &Bundle, dec: usize, t: usize, what: &str) -> Result<usize> {
    let end = dec + t;
    if end > b.data.len() {
        return Err(bad(format!("too many {what}")));
    }
    Ok(end)
}

/// From `bink.c` `read_runs`.
fn read_runs(gb: &mut BitReader, vlc: &[Vlc; 16], b: &mut Bundle) -> Result<()> {
    let Some((dec, t)) = check_read_val(gb, b)? else {
        return Ok(());
    };
    let end = dec_end(b, dec, t, "run lengths")?;
    if gb.read_bit()? {
        let v = gb.read(4)? as u8;
        b.data[dec..end].fill(v);
    } else {
        for k in dec..end {
            b.data[k] = get_huff(gb, vlc, &b.tree)?;
        }
    }
    b.cur_dec = Some(end);
    Ok(())
}

/// From `bink.c` `read_motion_values`.
fn read_motion_values(gb: &mut BitReader, vlc: &[Vlc; 16], b: &mut Bundle) -> Result<()> {
    let Some((dec, t)) = check_read_val(gb, b)? else {
        return Ok(());
    };
    let end = dec_end(b, dec, t, "motion values")?;
    if gb.read_bit()? {
        let mut v = gb.read(4)? as i32;
        if v != 0 && gb.read_bit()? {
            v = -v;
        }
        b.data[dec..end].fill(v as u8);
    } else {
        for k in dec..end {
            let mut v = i32::from(get_huff(gb, vlc, &b.tree)?);
            if v != 0 && gb.read_bit()? {
                v = -v;
            }
            b.data[k] = v as u8;
        }
    }
    b.cur_dec = Some(end);
    Ok(())
}

/// From `bink.c` `read_block_types` (without the branch of revision `k`).
fn read_block_types(gb: &mut BitReader, vlc: &[Vlc; 16], b: &mut Bundle) -> Result<()> {
    let Some((dec, t)) = check_read_val(gb, b)? else {
        return Ok(());
    };
    let end = dec_end(b, dec, t, "block kinds")?;
    if gb.read_bit()? {
        let v = gb.read(4)? as u8;
        b.data[dec..end].fill(v);
    } else {
        let mut last = 0u8;
        let mut cur = dec;
        while cur < end {
            let v = get_huff(gb, vlc, &b.tree)?;
            if v < 12 {
                last = v;
                b.data[cur] = v;
                cur += 1;
            } else {
                let run = usize::from(BINK_RLELENS[usize::from(v - 12)]);
                if end - cur < run {
                    return Err(bad("run of block kinds past the end of the row"));
                }
                b.data[cur..cur + run].fill(last);
                cur += run;
            }
        }
    }
    b.cur_dec = Some(end);
    Ok(())
}

/// From `bink.c` `read_patterns`.
fn read_patterns(gb: &mut BitReader, vlc: &[Vlc; 16], b: &mut Bundle) -> Result<()> {
    let Some((dec, t)) = check_read_val(gb, b)? else {
        return Ok(());
    };
    let end = dec_end(b, dec, t, "pattern values")?;
    for k in dec..end {
        if gb.remaining() < 2 {
            return Err(bad("bitstream ends in the pattern values"));
        }
        let lo = get_huff(gb, vlc, &b.tree)?;
        let hi = get_huff(gb, vlc, &b.tree)?;
        b.data[k] = lo | hi << 4;
    }
    b.cur_dec = Some(end);
    Ok(())
}

/// From `bink.c` `read_colors` (revision >= `i`: without sign conversion).
fn read_colors(
    gb: &mut BitReader,
    vlc: &[Vlc; 16],
    b: &mut Bundle,
    col_high: &[Tree; 16],
    col_lastval: &mut usize,
) -> Result<()> {
    let Some((dec, t)) = check_read_val(gb, b)? else {
        return Ok(());
    };
    let end = dec_end(b, dec, t, "color values")?;
    if gb.read_bit()? {
        *col_lastval = usize::from(get_huff(gb, vlc, &col_high[*col_lastval])?);
        let v = get_huff(gb, vlc, &b.tree)?;
        b.data[dec..end].fill((*col_lastval as u8) << 4 | v);
    } else {
        for k in dec..end {
            if gb.remaining() < 2 {
                return Err(bad("bitstream ends in the color values"));
            }
            *col_lastval = usize::from(get_huff(gb, vlc, &col_high[*col_lastval])?);
            let v = get_huff(gb, vlc, &b.tree)?;
            b.data[k] = (*col_lastval as u8) << 4 | v;
        }
    }
    b.cur_dec = Some(end);
    Ok(())
}

/// From `bink.c` `read_dcs`.
fn read_dcs(gb: &mut BitReader, b: &mut Bundle, start_bits: u32, has_sign: bool) -> Result<()> {
    let Some((mut dst, len)) = check_read_val(gb, b)? else {
        return Ok(());
    };
    let cap = b.data.len();
    let first_bits = start_bits - u32::from(has_sign);
    if gb.remaining() < first_bits as usize {
        return Err(bad("bitstream ends in the DC values"));
    }
    let mut v = gb.read(first_bits)? as i32;
    if v != 0 && has_sign && gb.read_bit()? {
        v = -v;
    }
    let put = |data: &mut Vec<u8>, dst: &mut usize, v: i32| {
        data[*dst..*dst + 2].copy_from_slice(&(v as i16).to_le_bytes());
        *dst += 2;
    };
    if (cap - dst) / 2 < 1 {
        return Err(bad("too many DC values"));
    }
    put(&mut b.data, &mut dst, v);
    let len = len - 1;
    for i in (0..len).step_by(8) {
        let len2 = (len - i).min(8);
        if (cap - dst) / 2 < len2 {
            return Err(bad("too many DC values"));
        }
        let bsize = gb.read(4)?;
        if bsize != 0 {
            for _ in 0..len2 {
                let mut v2 = gb.read(bsize)? as i32;
                if v2 != 0 && gb.read_bit()? {
                    v2 = -v2;
                }
                v += v2;
                put(&mut b.data, &mut dst, v);
                if !(-32768..=32767).contains(&v) {
                    return Err(bad(format!("DC value out of range: {v}")));
                }
            }
        } else {
            for _ in 0..len2 {
                put(&mut b.data, &mut dst, v);
            }
        }
    }
    b.cur_dec = Some(dst);
    Ok(())
}

/// From `bink.c` `read_dct_coeffs` (quantizer always from the bitstream, `q == -1`): coefficients into
/// `block`, list of the set positions into `coef_idx`; result (quantizer, count).
fn read_dct_coeffs(
    gb: &mut BitReader,
    block: &mut [i32; 64],
    coef_idx: &mut [usize; 64],
) -> Result<(usize, usize)> {
    let overflow = || bad("coefficient list overflows");
    let mut coef_list = [0usize; 128];
    let mut mode_list = [0u8; 128];
    let mut list_start = 64usize;
    let mut list_end = 64usize;
    let mut coef_count = 0usize;

    if gb.remaining() < 4 {
        return Err(bad("bitstream ends before a DCT block"));
    }
    for (c, m) in [(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)] {
        coef_list[list_end] = c;
        mode_list[list_end] = m;
        list_end += 1;
    }

    let read_coef = |gb: &mut BitReader, bits: i32| -> Result<i32> {
        if bits == 0 {
            Ok(1 - (i32::from(gb.read_bit()?) << 1))
        } else {
            let t = gb.read(bits as u32)? as i32 | 1 << bits;
            Ok(if gb.read_bit()? { -t } else { t })
        }
    };
    let mut set = |block: &mut [i32; 64], ccoef: usize, t: i32, count: &mut usize| -> Result<()> {
        let pos = *BINK_SCAN.get(ccoef).ok_or_else(overflow)?;
        block[usize::from(pos)] = t;
        *coef_idx.get_mut(*count).ok_or_else(overflow)? = ccoef;
        *count += 1;
        Ok(())
    };

    let mut bits = gb.read(4)? as i32 - 1;
    while bits >= 0 {
        let mut list_pos = list_start;
        while list_pos < list_end {
            if (usize::from(mode_list[list_pos]) | coef_list[list_pos]) == 0 || !gb.read_bit()? {
                list_pos += 1;
                continue;
            }
            let mut ccoef = coef_list[list_pos];
            let mode = mode_list[list_pos];
            match mode {
                0 | 2 => {
                    if mode == 0 {
                        coef_list[list_pos] = ccoef + 4;
                        mode_list[list_pos] = 1;
                    } else {
                        coef_list[list_pos] = 0;
                        mode_list[list_pos] = 0;
                        list_pos += 1;
                    }
                    for _ in 0..4 {
                        if gb.read_bit()? {
                            list_start = list_start.checked_sub(1).ok_or_else(overflow)?;
                            coef_list[list_start] = ccoef;
                            mode_list[list_start] = 3;
                        } else {
                            let t = read_coef(gb, bits)?;
                            set(block, ccoef, t, &mut coef_count)?;
                        }
                        ccoef += 1;
                    }
                }
                1 => {
                    mode_list[list_pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        if list_end >= coef_list.len() {
                            return Err(overflow());
                        }
                        coef_list[list_end] = ccoef;
                        mode_list[list_end] = 2;
                        list_end += 1;
                    }
                }
                _ => {
                    // Mode 3: single coefficient.
                    let t = read_coef(gb, bits)?;
                    set(block, ccoef, t, &mut coef_count)?;
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                }
            }
        }
        bits -= 1;
    }

    let quant_idx = gb.read(4)? as usize;
    Ok((quant_idx, coef_count))
}

/// From `bink.c` `unquantize_dct_coeffs` (product unsigned as in C, then shifted arithmetically).
fn unquantize_dct_coeffs(block: &mut [i32; 64], quant: &[u32; 64], coef_idx: &[usize]) {
    let q = |v: i32, f: u32| ((v as u32).wrapping_mul(f) as i32) >> 11;
    block[0] = q(block[0], quant[0]);
    for &idx in coef_idx {
        let pos = usize::from(BINK_SCAN[idx]);
        block[pos] = q(block[pos], quant[idx]);
    }
}

/// From `bink.c` `read_residue`.
fn read_residue(gb: &mut BitReader, block: &mut [i16; 64], mut masks_count: i32) -> Result<()> {
    let overflow = || bad("residue list overflows");
    let mut coef_list = [0usize; 128];
    let mut mode_list = [0u8; 128];
    let mut list_start = 64usize;
    let mut list_end = 64usize;
    let mut nz_coeff = [0usize; 64];
    let mut nz_count = 0usize;

    for (c, m) in [(4, 0), (24, 0), (44, 0), (0, 2)] {
        coef_list[list_end] = c;
        mode_list[list_end] = m;
        list_end += 1;
    }

    // Sets a new coefficient to +/-mask; `true` if the mask count is exhausted.
    let put = |gb: &mut BitReader,
               block: &mut [i16; 64],
               ccoef: usize,
               mask: i32,
               nz: (&mut [usize; 64], &mut usize),
               masks_count: &mut i32|
     -> Result<bool> {
        let (nz_coeff, nz_count) = nz;
        let pos = usize::from(*BINK_SCAN.get(ccoef).ok_or_else(overflow)?);
        *nz_coeff.get_mut(*nz_count).ok_or_else(overflow)? = pos;
        *nz_count += 1;
        block[pos] = if gb.read_bit()? { -mask } else { mask } as i16;
        *masks_count -= 1;
        Ok(*masks_count < 0)
    };

    let mut mask = 1i32 << gb.read(3)?;
    while mask != 0 {
        for &pos in &nz_coeff[..nz_count] {
            if !gb.read_bit()? {
                continue;
            }
            block[pos] = if block[pos] < 0 {
                block[pos].wrapping_sub(mask as i16)
            } else {
                block[pos].wrapping_add(mask as i16)
            };
            masks_count -= 1;
            if masks_count < 0 {
                return Ok(());
            }
        }
        let mut list_pos = list_start;
        while list_pos < list_end {
            if (coef_list[list_pos] | usize::from(mode_list[list_pos])) == 0 || !gb.read_bit()? {
                list_pos += 1;
                continue;
            }
            let mut ccoef = coef_list[list_pos];
            let mode = mode_list[list_pos];
            match mode {
                0 | 2 => {
                    if mode == 0 {
                        coef_list[list_pos] = ccoef + 4;
                        mode_list[list_pos] = 1;
                    } else {
                        coef_list[list_pos] = 0;
                        mode_list[list_pos] = 0;
                        list_pos += 1;
                    }
                    for _ in 0..4 {
                        if gb.read_bit()? {
                            list_start = list_start.checked_sub(1).ok_or_else(overflow)?;
                            coef_list[list_start] = ccoef;
                            mode_list[list_start] = 3;
                        } else if put(
                            gb,
                            block,
                            ccoef,
                            mask,
                            (&mut nz_coeff, &mut nz_count),
                            &mut masks_count,
                        )? {
                            return Ok(());
                        }
                        ccoef += 1;
                    }
                }
                1 => {
                    mode_list[list_pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        if list_end >= coef_list.len() {
                            return Err(overflow());
                        }
                        coef_list[list_end] = ccoef;
                        mode_list[list_end] = 2;
                        list_end += 1;
                    }
                }
                _ => {
                    // Mode 3.
                    let done = put(
                        gb,
                        block,
                        ccoef,
                        mask,
                        (&mut nz_coeff, &mut nz_count),
                        &mut masks_count,
                    )?;
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                    if done {
                        return Ok(());
                    }
                }
            }
        }
        mask >>= 1;
    }
    Ok(())
}

/// `av_log2`.
fn log2(x: usize) -> u32 {
    usize::BITS - 1 - x.max(1).leading_zeros()
}

/// Checks that from `off` a block of `size` x `size` pixels lies within a plane of length `len`.
fn block_fits(off: usize, stride: usize, size: usize, len: usize) -> Result<()> {
    if off + (size - 1) * stride + size > len {
        return Err(bad("block lies outside the plane"));
    }
    Ok(())
}

/// A plane as `bink_decode_plane` sees it.
struct PlaneTarget<'a> {
    /// The frame being built.
    cur: &'a mut [u8],
    /// The previous frame; `None` for the first frame (FFmpeg then reads from the one being built).
    prev: Option<&'a [u8]>,
    stride: usize,
}

/// State of the decoder apart from the frames (`BinkContext`).
#[derive(Debug, Clone)]
struct State {
    width: usize,
    height: usize,
    vlc: [Vlc; 16],
    bundle: [Bundle; NB_SRC],
    col_high: [Tree; 16],
    col_lastval: usize,
    /// Intermediate buffer of the 16 x 16 blocks (`ublock`).
    ublock: [u8; 64],
}

impl State {
    /// From `bink.c` `get_value`: next value of a bundle; an exhausted bundle is an error.
    #[inline]
    fn get_value(&mut self, n: usize) -> Result<i32> {
        let b = &mut self.bundle[n];
        let p = b.cur_ptr;
        if n < SRC_X_OFF || n == SRC_RUN || n == SRC_X_OFF || n == SRC_Y_OFF {
            let v = *b
                .data
                .get(p)
                .ok_or_else(|| bad(format!("bundle {n} is exhausted")))?;
            b.cur_ptr += 1;
            return Ok(if n == SRC_X_OFF || n == SRC_Y_OFF {
                i32::from(v as i8)
            } else {
                i32::from(v)
            });
        }
        let bytes = b
            .data
            .get(p..p + 2)
            .ok_or_else(|| bad(format!("bundle {n} is exhausted")))?;
        let v = i16::from_le_bytes([bytes[0], bytes[1]]);
        b.cur_ptr += 2;
        Ok(i32::from(v))
    }

    /// From `bink.c` `init_lengths`.
    fn init_lengths(&mut self, width: usize, bw: usize) {
        let width = width.next_multiple_of(8);
        let blocks = log2((width >> 3) + 511) + 1;
        self.bundle[SRC_BLOCK_TYPES].len = blocks;
        self.bundle[SRC_SUB_BLOCK_TYPES].len = log2((width >> 4) + 511) + 1;
        self.bundle[SRC_COLORS].len = log2(bw * 64 + 511) + 1;
        self.bundle[SRC_INTRA_DC].len = blocks;
        self.bundle[SRC_INTER_DC].len = blocks;
        self.bundle[SRC_X_OFF].len = blocks;
        self.bundle[SRC_Y_OFF].len = blocks;
        self.bundle[SRC_PATTERN].len = log2((bw << 3) + 511) + 1;
        self.bundle[SRC_RUN].len = log2(bw * 48 + 511) + 1;
    }

    /// From `bink.c` `read_bundle`.
    fn read_bundle(&mut self, gb: &mut BitReader, n: usize) -> Result<()> {
        if n == SRC_COLORS {
            for tree in &mut self.col_high {
                read_tree(gb, tree)?;
            }
            self.col_lastval = 0;
        }
        if n != SRC_INTRA_DC && n != SRC_INTER_DC {
            read_tree(gb, &mut self.bundle[n].tree)?;
        }
        self.bundle[n].cur_dec = Some(0);
        self.bundle[n].cur_ptr = 0;
        Ok(())
    }

    /// Reads the bundles of a block row (loop head in `bink_decode_plane`).
    fn read_row_bundles(&mut self, gb: &mut BitReader) -> Result<()> {
        let vlc = &self.vlc;
        let [
            types,
            sub,
            colors,
            pattern,
            x_off,
            y_off,
            intra_dc,
            inter_dc,
            run,
        ] = &mut self.bundle;
        read_block_types(gb, vlc, types)?;
        read_block_types(gb, vlc, sub)?;
        read_colors(gb, vlc, colors, &self.col_high, &mut self.col_lastval)?;
        read_patterns(gb, vlc, pattern)?;
        read_motion_values(gb, vlc, x_off)?;
        read_motion_values(gb, vlc, y_off)?;
        read_dcs(gb, intra_dc, DC_START_BITS, false)?;
        read_dcs(gb, inter_dc, DC_START_BITS, true)?;
        read_runs(gb, vlc, run)
    }

    /// From `bink.c` `bink_put_pixels`: copy 8 x 8 from the previous frame with a motion vector.
    fn put_pixels(&mut self, t: &mut PlaneTarget, dst: usize, ref_end: usize) -> Result<()> {
        let xoff = self.get_value(SRC_X_OFF)? as isize;
        let yoff = self.get_value(SRC_Y_OFF)? as isize;
        let r = dst as isize + xoff + yoff * t.stride as isize;
        if r < 0 || r as usize > ref_end {
            return Err(bad(format!(
                "motion vector ({xoff}, {yoff}) points outside the plane"
            )));
        }
        let r = r as usize;
        block_fits(dst, t.stride, 8, t.cur.len())?;
        block_fits(r, t.stride, 8, t.cur.len())?;
        match t.prev {
            Some(prev) => {
                block_fits(r, t.stride, 8, prev.len())?;
                dsp::put_pixels8(&mut t.cur[dst..], &prev[r..], t.stride);
            }
            None => dsp::put_pixels8_within(t.cur, dst, r, t.stride),
        }
        Ok(())
    }

    /// Runs of a run-length block (`RUN_BLOCK`, 8 x 8 or in `ublock`); `put(k, v)` sets pixel
    /// `k` of the scan order.
    fn decode_runs(&mut self, gb: &mut BitReader, mut put: impl FnMut(usize, u8)) -> Result<()> {
        let scan = &BINK_PATTERNS[gb.read(4)? as usize];
        let mut i = 0usize;
        let mut k = 0usize;
        loop {
            let run = self.get_value(SRC_RUN)? as usize + 1;
            i += run;
            if i > 64 {
                return Err(bad("run past the end of the block"));
            }
            if gb.read_bit()? {
                let v = self.get_value(SRC_COLORS)? as u8;
                for _ in 0..run {
                    put(usize::from(scan[k]), v);
                    k += 1;
                }
            } else {
                for _ in 0..run {
                    let v = self.get_value(SRC_COLORS)? as u8;
                    put(usize::from(scan[k]), v);
                    k += 1;
                }
            }
            if i >= 63 {
                break;
            }
        }
        if i == 63 {
            let v = self.get_value(SRC_COLORS)? as u8;
            put(usize::from(scan[k]), v);
        }
        Ok(())
    }

    /// Reads and dequantizes a DCT block (common part of `INTRA_BLOCK` and `INTER_BLOCK`).
    fn read_dct_block(&mut self, gb: &mut BitReader, intra: bool) -> Result<[i32; 64]> {
        let mut block = [0i32; 64];
        block[0] = self.get_value(if intra { SRC_INTRA_DC } else { SRC_INTER_DC })?;
        let mut coef_idx = [0usize; 64];
        let (q, count) = read_dct_coeffs(gb, &mut block, &mut coef_idx)?;
        let quant = if intra {
            &BINK_INTRA_QUANT[q]
        } else {
            &BINK_INTER_QUANT[q]
        };
        unquantize_dct_coeffs(&mut block, quant, &coef_idx[..count]);
        Ok(block)
    }

    /// From `bink.c` `bink_decode_plane` (without the branch of revision `k`).
    fn decode_plane(
        &mut self,
        gb: &mut BitReader,
        t: &mut PlaneTarget,
        is_chroma: bool,
    ) -> Result<()> {
        let stride = t.stride;
        let (w, h) = (self.width, self.height);
        let (bw, bh) = if is_chroma {
            ((w + 15) >> 4, (h + 15) >> 4)
        } else {
            ((w + 7) >> 3, (h + 7) >> 3)
        };
        let width = w >> usize::from(is_chroma);

        self.init_lengths(width.max(8), bw);
        for n in 0..NB_SRC {
            self.read_bundle(gb, n)?;
        }

        // `c->last->linesize` is 0 for the first frame: motions then reach only across the first
        // block row (FFmpeg quirk, kept).
        let last_linesize = if t.prev.is_some() { stride } else { 0 };
        let ref_end = (bw - 1 + last_linesize * (bh - 1)) * 8;
        let coordmap: [usize; 64] = std::array::from_fn(|i| (i & 7) + (i >> 3) * stride);

        for by in 0..bh {
            self.read_row_bundles(gb)?;
            let mut bx = 0;
            while bx < bw {
                let dst = 8 * by * stride + 8 * bx;
                let blk = self.get_value(SRC_BLOCK_TYPES)?;
                // 16 x 16 block kind at an odd position: part of the block already decoded.
                if ((by & 1) != 0 || (bx & 1) != 0) && blk == SCALED_BLOCK {
                    bx += 2;
                    continue;
                }
                match blk {
                    SKIP_BLOCK => {
                        block_fits(dst, stride, 8, t.cur.len())?;
                        if let Some(prev) = t.prev {
                            block_fits(dst, stride, 8, prev.len())?;
                            dsp::put_pixels8(&mut t.cur[dst..], &prev[dst..], stride);
                        }
                    }
                    SCALED_BLOCK => {
                        block_fits(dst, stride, 16, t.cur.len())?;
                        let sub = self.get_value(SRC_SUB_BLOCK_TYPES)?;
                        match sub {
                            RUN_BLOCK => {
                                if gb.remaining() < 4 {
                                    return Err(bad("bitstream ends before a run-length block"));
                                }
                                let mut ub = self.ublock;
                                self.decode_runs(gb, |k, v| ub[k] = v)?;
                                self.ublock = ub;
                            }
                            INTRA_BLOCK => {
                                let block = self.read_dct_block(gb, true)?;
                                dsp::idct_put(&mut self.ublock, 8, &block);
                            }
                            FILL_BLOCK => {
                                let v = self.get_value(SRC_COLORS)? as u8;
                                dsp::fill_block(&mut t.cur[dst..], v, stride, 16);
                            }
                            PATTERN_BLOCK => {
                                let col = [
                                    self.get_value(SRC_COLORS)? as u8,
                                    self.get_value(SRC_COLORS)? as u8,
                                ];
                                for j in 0..8 {
                                    let mut v = self.get_value(SRC_PATTERN)?;
                                    for i in 0..8 {
                                        self.ublock[i + j * 8] = col[(v & 1) as usize];
                                        v >>= 1;
                                    }
                                }
                            }
                            RAW_BLOCK => {
                                for k in 0..64 {
                                    self.ublock[k] = self.get_value(SRC_COLORS)? as u8;
                                }
                            }
                            _ => {
                                return Err(bad(format!("unknown 16x16 block kind {sub}")));
                            }
                        }
                        if sub != FILL_BLOCK {
                            dsp::scale_block(&self.ublock, &mut t.cur[dst..], stride);
                        }
                        bx += 1;
                    }
                    MOTION_BLOCK => self.put_pixels(t, dst, ref_end)?,
                    RUN_BLOCK => {
                        block_fits(dst, stride, 8, t.cur.len())?;
                        let cur = &mut *t.cur;
                        self.decode_runs(gb, |k, v| cur[dst + coordmap[k]] = v)?;
                    }
                    RESIDUE_BLOCK => {
                        self.put_pixels(t, dst, ref_end)?;
                        let mut block = [0i16; 64];
                        let v = gb.read(7)? as i32;
                        read_residue(gb, &mut block, v)?;
                        dsp::add_pixels8(&mut t.cur[dst..], &block, stride);
                    }
                    INTRA_BLOCK => {
                        block_fits(dst, stride, 8, t.cur.len())?;
                        let block = self.read_dct_block(gb, true)?;
                        dsp::idct_put(&mut t.cur[dst..], stride, &block);
                    }
                    FILL_BLOCK => {
                        block_fits(dst, stride, 8, t.cur.len())?;
                        let v = self.get_value(SRC_COLORS)? as u8;
                        dsp::fill_block(&mut t.cur[dst..], v, stride, 8);
                    }
                    INTER_BLOCK => {
                        self.put_pixels(t, dst, ref_end)?;
                        let mut block = self.read_dct_block(gb, false)?;
                        dsp::idct_add(&mut t.cur[dst..], stride, &mut block);
                    }
                    PATTERN_BLOCK => {
                        block_fits(dst, stride, 8, t.cur.len())?;
                        let col = [
                            self.get_value(SRC_COLORS)? as u8,
                            self.get_value(SRC_COLORS)? as u8,
                        ];
                        for i in 0..8 {
                            let mut v = self.get_value(SRC_PATTERN)?;
                            for j in 0..8 {
                                t.cur[dst + i * stride + j] = col[(v & 1) as usize];
                                v >>= 1;
                            }
                        }
                    }
                    RAW_BLOCK => {
                        block_fits(dst, stride, 8, t.cur.len())?;
                        let b = &mut self.bundle[SRC_COLORS];
                        let src = b
                            .data
                            .get(b.cur_ptr..b.cur_ptr + 64)
                            .ok_or_else(|| bad("color bundle for raw block exhausted"))?;
                        for i in 0..8 {
                            t.cur[dst + i * stride..dst + i * stride + 8]
                                .copy_from_slice(&src[i * 8..i * 8 + 8]);
                        }
                        b.cur_ptr += 64;
                    }
                    _ => return Err(bad(format!("unknown block kind {blk}"))),
                }
                bx += 1;
            }
        }

        // The data of the next plane begins at a 32-bit boundary.
        gb.align32();
        Ok(())
    }
}

/// Bink frame decoder: one packet per frame, in file order; the previous frame is the reference.
#[derive(Debug, Clone)]
pub struct VideoDecoder {
    state: State,
    cur: VideoFrame,
    last: VideoFrame,
    /// There is a previous frame (`c->last->data[…]` set).
    has_last: bool,
}

impl VideoDecoder {
    /// From `bink.c` `decode_init` and `init_bundles`.
    pub fn new(header: &BinkHeader) -> Result<Self> {
        if header.revision != b'i' {
            return Err(BinkError::Unsupported(format!(
                "Bink revision \"{}\" (only \"i\" is supported)",
                char::from(header.revision).escape_default()
            )));
        }
        if header.video_flags & (BINK_FLAG_ALPHA | BINK_FLAG_GRAY) != 0 {
            return Err(BinkError::Unsupported(
                "alpha plane and grayscale are not supported".into(),
            ));
        }
        let (w, h) = (header.width, header.height);
        if w == 0 || h == 0 || w > MAX_DIMENSION || h > MAX_DIMENSION {
            return Err(bad(format!("frame size {w} x {h} invalid")));
        }
        let (w, h) = (w as usize, h as usize);
        let blocks = w.div_ceil(8) * h.div_ceil(8);
        let bundle = Bundle {
            len: 0,
            tree: Tree::default(),
            data: vec![0; blocks * 64],
            cur_dec: None,
            cur_ptr: 0,
        };
        Ok(VideoDecoder {
            state: State {
                width: w,
                height: h,
                vlc: std::array::from_fn(Vlc::new),
                bundle: std::array::from_fn(|_| bundle.clone()),
                col_high: [Tree::default(); 16],
                col_lastval: 0,
                ublock: [0; 64],
            },
            cur: VideoFrame::new(header.width, header.height),
            last: VideoFrame::new(header.width, header.height),
            has_last: false,
        })
    }

    /// From `bink.c` `decode_frame`: decodes the video packet of a frame. On an error, the previous
    /// frame remains the reference for the next one.
    pub fn decode(&mut self, packet: &[u8]) -> Result<&VideoFrame> {
        let swapped = self.has_last;
        if swapped {
            // The current frame becomes the reference, its predecessor the buffer for the new one.
            std::mem::swap(&mut self.cur, &mut self.last);
        }
        match self.decode_planes(packet) {
            Ok(()) => {
                self.has_last = true;
                Ok(&self.cur)
            }
            Err(e) => {
                if swapped {
                    std::mem::swap(&mut self.cur, &mut self.last);
                }
                Err(e)
            }
        }
    }

    fn decode_planes(&mut self, packet: &[u8]) -> Result<()> {
        let bits_count = packet.len() * 8;
        let mut gb = BitReader::new(packet);
        gb.skip(32)?;
        let prev = self.has_last.then_some(&self.last);
        for plane in 0..3 {
            // `swap_planes` (revision >= `h`): after Y follows V, then U.
            let (cur, prev, stride) = match plane {
                0 => (&mut self.cur.y, prev.map(|p| &p.y[..]), self.cur.strides[0]),
                1 => (&mut self.cur.v, prev.map(|p| &p.v[..]), self.cur.strides[2]),
                _ => (&mut self.cur.u, prev.map(|p| &p.u[..]), self.cur.strides[1]),
            };
            let mut target = PlaneTarget { cur, prev, stride };
            self.state.decode_plane(&mut gb, &mut target, plane != 0)?;
            if gb.position() >= bits_count {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::BinkHeader;

    fn header(width: u32, height: u32) -> BinkHeader {
        BinkHeader {
            revision: b'i',
            file_size: 0,
            frames: 1,
            largest_frame: 0,
            width,
            height,
            fps_num: 25,
            fps_den: 1,
            video_flags: 0,
            audio: Vec::new(),
            index: Vec::new(),
        }
    }

    /// Test writer: bits lowest first like the reader.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        bits: usize,
    }

    impl BitWriter {
        fn put(&mut self, v: u32, n: usize) {
            for i in 0..n {
                if self.bits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                if (v >> i) & 1 != 0 {
                    *self.bytes.last_mut().unwrap() |= 1 << (self.bits % 8);
                }
                self.bits += 1;
            }
        }

        fn align32(&mut self) {
            while !self.bits.is_multiple_of(32) {
                self.put(0, 1);
            }
        }
    }

    /// A plane of a small frame (all bundle lengths 10 bits except the sub-kinds with `sub_bits`),
    /// all blocks of kind `blk` with color `color`, motion (`x`, `y`) with 4-bit magnitude and sign.
    ///
    /// From the second row on, the decoder reads a bundle only if the previous row has consumed it
    /// (`CHECK_READ_VAL`): colors only for fill blocks, motion only for motion blocks.
    fn plane(
        w: &mut BitWriter,
        (rows, cols, sub_bits): (usize, u32, usize),
        blk: u32,
        color: u8,
        mv: (i32, i32),
    ) {
        trees(w);
        let motion = |w: &mut BitWriter, v: i32| {
            w.put(cols, 10);
            w.put(1, 1);
            w.put(v.unsigned_abs(), 4);
            if v != 0 {
                w.put(u32::from(v < 0), 1);
            }
        };
        for row in 0..rows {
            w.put(cols, 10); // Block kinds: all equal
            w.put(1, 1);
            w.put(blk, 4);
            if row == 0 {
                w.put(0, sub_bits); // Sub-kinds: none
            }
            if row == 0 || blk == 6 {
                w.put(cols, 10); // Colors: all equal
                w.put(1, 1);
                w.put(u32::from(color >> 4), 4);
                w.put(u32::from(color & 15), 4);
            }
            if row == 0 {
                w.put(0, 10); // Patterns: none
            }
            if row == 0 || blk == 2 {
                motion(w, mv.0);
                motion(w, mv.1);
            }
            if row == 0 {
                w.put(0, 10); // Intra-DC
                w.put(0, 10); // Inter-DC
                w.put(0, 10); // Run lengths
            }
        }
        w.align32();
    }

    /// Trees: block kind, sub-kind, 16 color trees + color tree, pattern, X, Y, run lengths (all tree 0, so
    /// every symbol as 4 bits).
    fn trees(w: &mut BitWriter) {
        for _ in 0..(7 + 16) {
            w.put(0, 4);
        }
    }

    /// Individually coded values of a bundle with tree 0 (10-bit count, bit 0, 4 bits per value).
    fn symbols(w: &mut BitWriter, values: &[u32]) {
        w.put(values.len() as u32, 10);
        w.put(0, 1);
        for &v in values {
            w.put(v, 4);
        }
    }

    /// Individually coded colors (per color the upper nibble via the color tree, then the lower one).
    fn colors(w: &mut BitWriter, values: &[u8]) {
        w.put(values.len() as u32, 10);
        w.put(0, 1);
        for &c in values {
            w.put(u32::from(c >> 4), 4);
            w.put(u32::from(c & 15), 4);
        }
    }

    fn visible(f: &VideoFrame) -> Vec<Vec<u8>> {
        (0..3)
            .flat_map(|i| f.plane(i).map(<[u8]>::to_vec))
            .collect()
    }

    fn frame(blk: u32, colors: [u8; 3], mv: (i32, i32)) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.put(0, 32);
        plane(&mut w, (2, 2, 10), blk, colors[0], mv);
        plane(&mut w, (1, 1, 9), blk, colors[1], mv);
        plane(&mut w, (1, 1, 9), blk, colors[2], mv);
        w.bytes
    }

    #[test]
    fn scaled_block_in_last_odd_column_stays_in_its_rows() {
        // 24 x 16: three block columns; the 16 x 16 block at bx = 2 reaches up to column 31. With a
        // row stride of 24 it would write past the end of the row into the pixels at bx = 0.
        let mut dec = VideoDecoder::new(&header(24, 16)).unwrap();
        let mut w = BitWriter::default();
        w.put(0, 32);
        trees(&mut w);
        // Block row 0: fill, fill, 16 x 16 (sub-kind fill).
        symbols(&mut w, &[6, 6, 1]);
        w.put(1, 10); // Sub-kinds: one, all equal
        w.put(1, 1);
        w.put(6, 4);
        colors(&mut w, &[100, 100, 200]);
        for _ in 0..6 {
            w.put(0, 10); // Pattern, X, Y, intra DC, inter DC, run lengths: none
        }
        // Block row 1: fill, fill, part of the 16 x 16 block.
        symbols(&mut w, &[6, 6, 1]);
        w.put(0, 10); // Sub-kinds: end
        colors(&mut w, &[100, 100]);
        w.align32();
        plane(&mut w, (1, 2, 10), 6, 50, (0, 0));
        plane(&mut w, (1, 2, 10), 6, 60, (0, 0));
        let f = dec.decode(&w.bytes).unwrap();
        let rows: Vec<&[u8]> = f.plane(0).collect();
        assert_eq!(rows.len(), 16);
        for (y, row) in rows.into_iter().enumerate() {
            assert_eq!(row[..16], [100; 16], "row {y}");
            assert_eq!(row[16..], [200; 8], "row {y}");
        }
        assert!(f.plane(1).all(|r| r == [60; 12]));
        assert!(f.plane(2).all(|r| r == [50; 12]));
        assert_eq!(f.strides, [32, 16, 16]);
    }

    #[test]
    fn scaled_block_in_last_odd_row_is_accepted() {
        // 16 x 8: one block row; the 16 x 16 block reaches 8 rows below the block grid.
        let mut dec = VideoDecoder::new(&header(16, 8)).unwrap();
        let mut w = BitWriter::default();
        w.put(0, 32);
        trees(&mut w);
        w.put(2, 10); // Block kinds: all 16 x 16
        w.put(1, 1);
        w.put(1, 4);
        w.put(1, 10); // Sub-kind fill
        w.put(1, 1);
        w.put(6, 4);
        colors(&mut w, &[77]);
        for _ in 0..6 {
            w.put(0, 10);
        }
        w.align32();
        plane(&mut w, (1, 1, 9), 6, 1, (0, 0));
        plane(&mut w, (1, 1, 9), 6, 2, (0, 0));
        let f = dec.decode(&w.bytes).unwrap();
        assert_eq!(f.plane(0).count(), 8);
        assert!(f.plane(0).all(|r| r == [77; 16]));
        assert!(f.plane(1).all(|r| r == [2; 8]));
    }

    #[test]
    fn fill_and_skip_frames_decode_like_ffmpeg() {
        let mut dec = VideoDecoder::new(&header(16, 16)).unwrap();
        // Bitstream order Y, V, U.
        let f = dec.decode(&frame(6, [100, 200, 50], (0, 0))).unwrap();
        assert!(
            f.plane(0)
                .all(|r| r.len() == 16 && r.iter().all(|&p| p == 100))
        );
        assert!(
            f.plane(1)
                .all(|r| r.len() == 8 && r.iter().all(|&p| p == 50))
        );
        assert!(f.plane(2).all(|r| r.iter().all(|&p| p == 200)));
        assert_eq!(f.plane(0).count(), 16);
        let first = f.clone();
        // A frame consisting only of skipped blocks takes over the previous one.
        let f = dec.decode(&frame(0, [0, 0, 0], (0, 0))).unwrap();
        assert_eq!(visible(f), visible(&first));
        let mut a = YUV_HASH_BASIS;
        let mut b = YUV_HASH_BASIS;
        yuv_hash(f, &mut a);
        yuv_hash(&first, &mut b);
        assert_eq!(a, b);
        assert_ne!(a, YUV_HASH_BASIS);
    }

    #[test]
    fn broken_frames_are_format_errors_and_keep_the_reference() {
        let mut dec = VideoDecoder::new(&header(16, 16)).unwrap();
        let good = dec.decode(&frame(6, [10, 20, 30], (0, 0))).unwrap().clone();
        // Motion vector outside the plane.
        let e = dec.decode(&frame(2, [0, 0, 0], (-1, 0))).unwrap_err();
        assert!(
            matches!(&e, BinkError::Format(m) if m.contains("motion vector")),
            "{e}"
        );
        // Unknown block kind.
        let e = dec.decode(&frame(10, [0, 0, 0], (0, 0))).unwrap_err();
        assert!(
            matches!(&e, BinkError::Format(m) if m.contains("block kind")),
            "{e}"
        );
        // Truncated packet.
        let full = frame(6, [1, 2, 3], (0, 0));
        assert!(matches!(dec.decode(&full[..9]), Err(BinkError::Format(_))));
        assert!(matches!(dec.decode(&[]), Err(BinkError::Format(_))));
        // After that, the good frame remains the reference.
        let f = dec.decode(&frame(0, [0, 0, 0], (0, 0))).unwrap();
        assert_eq!(visible(f), visible(&good));
        // A motion vector within the plane works.
        let f = dec.decode(&frame(2, [0, 0, 0], (0, 0))).unwrap();
        assert_eq!(visible(f), visible(&good));
    }

    #[test]
    fn decoder_rejects_foreign_headers() {
        let mut h = header(16, 16);
        h.revision = b'b';
        assert!(matches!(
            VideoDecoder::new(&h),
            Err(BinkError::Unsupported(_))
        ));
        let mut h = header(16, 16);
        h.video_flags = BINK_FLAG_ALPHA;
        assert!(matches!(
            VideoDecoder::new(&h),
            Err(BinkError::Unsupported(_))
        ));
        assert!(matches!(
            VideoDecoder::new(&header(0, 16)),
            Err(BinkError::Format(_))
        ));
    }

    #[test]
    fn corrupt_packets_never_panic() {
        let mut dec = VideoDecoder::new(&header(32, 16)).unwrap();
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut ok, mut err) = (0, 0);
        for _ in 0..1000 {
            let len = (next() % 4097) as usize;
            let packet: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            match dec.decode(&packet) {
                Ok(_) => ok += 1,
                Err(_) => err += 1,
            }
        }
        assert_eq!(ok + err, 1000);
    }
}
