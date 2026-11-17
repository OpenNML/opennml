/*
 * Bink DSP routines
 * Copyright (c) 2009 Konstantin Shishkov
 *
 * Half-pel DSP functions.
 * Copyright (c) 2000, 2001 Fabrice Bellard
 * Copyright (c) 2002-2004 Michael Niedermayer <michaelni@gmx.at>
 *
 * gmc & q-pel & 32/64 bit based MC by Michael Niedermayer <michaelni@gmx.at>
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

//! Block computations of the Bink frame decoder: inverse DCT, enlarging 8 x 8 -> 16 x 16, adding the residual,
//! copying and filling 8 x 8.
//!
//! Ported from FFmpeg n9.0.2, `libavcodec/binkdsp.c` (C version; FFmpeg has no processor-specific paths for
//! Bink), plus `put_pixels8_8_c` from `libavcodec/pel_template.c` (included in `libavcodec/hpeldsp.c`; two
//! 4-byte copies per row) and `fill_block16_c`/`fill_block8_c` from `libavcodec/blockdsp.c`. The copyright lines
//! above come from `binkdsp.c` and `hpeldsp.c`; `pel_template.c` and `blockdsp.c` have none of their own.
//!
//! All computations run with 32-bit overflow as in C (`wrapping_*`); results are truncated like the C
//! assignment to `uint8_t`, not clamped. The caller checks the destination ranges (`video.rs`): an 8 x 8 block
//! needs `7 · stride + 8` bytes from its start, a 16 x 16 block `15 · stride + 16`.
//!
//! Modification notice (LGPL-2.1 section 2 b): Ported to Rust and modified for `nml-bink`, 2026-10-25.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

/// `(1/sqrt(2)) << 12`.
const A1: i32 = 2896;
const A2: i32 = 2217;
const A3: i32 = 3784;
const A4: i32 = -5352;

/// From `binkdsp.c` `MUL`: product unsigned (overflow as in C), then shifted arithmetically by 11.
#[inline(always)]
fn mul(x: i32, y: i32) -> i32 {
    ((x as u32).wrapping_mul(y as u32) as i32) >> 11
}

/// From `binkdsp.c` `IDCT_TRANSFORM` (without `munge`): eight input values `s0..s7` -> eight output values.
#[inline(always)]
fn idct_transform(s: [i32; 8]) -> [i32; 8] {
    let a0 = s[0].wrapping_add(s[4]);
    let a1 = s[0].wrapping_sub(s[4]);
    let a2 = s[2].wrapping_add(s[6]);
    let a3 = mul(A1, s[2].wrapping_sub(s[6]));
    let a4 = s[5].wrapping_add(s[3]);
    let a5 = s[5].wrapping_sub(s[3]);
    let a6 = s[1].wrapping_add(s[7]);
    let a7 = s[1].wrapping_sub(s[7]);
    let b0 = a4.wrapping_add(a6);
    let b1 = mul(A3, a5.wrapping_add(a7));
    let b2 = mul(A4, a5).wrapping_sub(b0).wrapping_add(b1);
    let b3 = mul(A1, a6.wrapping_sub(a4)).wrapping_sub(b2);
    let b4 = mul(A2, a7).wrapping_add(b3).wrapping_sub(b1);
    [
        a0.wrapping_add(a2).wrapping_add(b0),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_add(b2),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_add(b3),
        a0.wrapping_sub(a2).wrapping_sub(b4),
        a0.wrapping_sub(a2).wrapping_add(b4),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_sub(b3),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_sub(b2),
        a0.wrapping_add(a2).wrapping_sub(b0),
    ]
}

/// From `binkdsp.c` `MUNGE_ROW`.
#[inline(always)]
fn munge_row(x: i32) -> i32 {
    x.wrapping_add(0x7F) >> 8
}

/// From `binkdsp.c` `bink_idct_col` (via `IDCT_COL`): column `col` from `src` to `dest`; if the column has
/// only the first value, it is passed through.
#[inline(always)]
fn idct_col(dest: &mut [i32; 64], src: &[i32; 64], col: usize) {
    let s: [i32; 8] = std::array::from_fn(|k| src[col + 8 * k]);
    let out = if s[1..].iter().fold(0, |acc, &v| acc | v) == 0 {
        [s[0]; 8]
    } else {
        idct_transform(s)
    };
    for (k, v) in out.into_iter().enumerate() {
        dest[col + 8 * k] = v;
    }
}

/// Column pass of both IDCT versions (`bink_idct_c`, `bink_idct_put_c`).
#[inline(always)]
fn idct_cols(block: &[i32; 64]) -> [i32; 64] {
    let mut temp = [0i32; 64];
    for col in 0..8 {
        idct_col(&mut temp, block, col);
    }
    temp
}

/// Row `row` of the intermediate result through `IDCT_ROW` (with `MUNGE_ROW`).
#[inline(always)]
fn idct_row(temp: &[i32; 64], row: usize) -> [i32; 8] {
    let s: [i32; 8] = std::array::from_fn(|k| temp[8 * row + k]);
    idct_transform(s).map(munge_row)
}

/// From `binkdsp.c` `bink_idct_c`: inverse DCT in place.
pub fn idct(block: &mut [i32; 64]) {
    let temp = idct_cols(block);
    for row in 0..8 {
        block[8 * row..8 * row + 8].copy_from_slice(&idct_row(&temp, row));
    }
}

/// From `binkdsp.c` `bink_idct_put_c`: inverse DCT, result truncated to 8 bits into `dst`
/// (row stride `stride`).
pub fn idct_put(dst: &mut [u8], stride: usize, block: &[i32; 64]) {
    let temp = idct_cols(block);
    for row in 0..8 {
        let out = idct_row(&temp, row);
        let line = &mut dst[row * stride..row * stride + 8];
        for (d, v) in line.iter_mut().zip(out) {
            *d = v as u8;
        }
    }
}

/// From `binkdsp.c` `bink_idct_add_c`: inverse DCT of `block` (its content afterwards) added to `dst`,
/// truncated to 8 bits.
pub fn idct_add(dst: &mut [u8], stride: usize, block: &mut [i32; 64]) {
    idct(block);
    for row in 0..8 {
        let line = &mut dst[row * stride..row * stride + 8];
        for (d, v) in line.iter_mut().zip(&block[8 * row..8 * row + 8]) {
            *d = d.wrapping_add(*v as u8);
        }
    }
}

/// From `binkdsp.c` `scale_block_c`: 8 x 8 -> 16 x 16, every pixel as 2 x 2.
pub fn scale_block(src: &[u8; 64], dst: &mut [u8], stride: usize) {
    for j in 0..8 {
        for half in 0..2 {
            let start = (2 * j + half) * stride;
            let line = &mut dst[start..start + 16];
            for (pair, &v) in line.chunks_exact_mut(2).zip(&src[8 * j..8 * j + 8]) {
                pair.fill(v);
            }
        }
    }
}

/// From `binkdsp.c` `add_pixels8_c`: residual `block` added to `dst`, truncated to 8 bits.
pub fn add_pixels8(dst: &mut [u8], block: &[i16; 64], stride: usize) {
    for row in 0..8 {
        let line = &mut dst[row * stride..row * stride + 8];
        for (d, v) in line.iter_mut().zip(&block[8 * row..8 * row + 8]) {
            *d = d.wrapping_add(*v as u8);
        }
    }
}

/// From `pel_template.c` `put_pixels8_8_c`: copy 8 x 8 from another plane.
pub fn put_pixels8(dst: &mut [u8], src: &[u8], stride: usize) {
    for row in 0..8 {
        dst[row * stride..row * stride + 8].copy_from_slice(&src[row * stride..row * stride + 8]);
    }
}

/// From `pel_template.c` `put_pixels8_8_c` with source and destination in the same plane (first frame without
/// predecessor: `bink_decode_plane` then reads from the frame being built). Two 4-byte copies per row as in C:
/// with overlap the result of the C version applies.
pub fn put_pixels8_within(buf: &mut [u8], dst: usize, src: usize, stride: usize) {
    for row in 0..8 {
        for half in [0, 4] {
            let s = src + row * stride + half;
            let d = dst + row * stride + half;
            let word = [buf[s], buf[s + 1], buf[s + 2], buf[s + 3]];
            buf[d..d + 4].copy_from_slice(&word);
        }
    }
}

/// From `blockdsp.c` `fill_block8_c` (`size` 8) or `fill_block16_c` (`size` 16, height 16).
pub fn fill_block(dst: &mut [u8], value: u8, stride: usize, size: usize) {
    for row in 0..size {
        dst[row * stride..row * stride + size].fill(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idct_of_dc_only_block_is_flat() {
        // bink_idct_c: DC 256 yields 1 everywhere after (x + 0x7F) >> 8.
        let mut block = [0i32; 64];
        block[0] = 256;
        idct(&mut block);
        assert_eq!(block, [1; 64]);
        let mut block = [0i32; 64];
        block[0] = 256;
        let mut dst = [0u8; 8 * 9];
        idct_put(&mut dst, 9, &block);
        for row in dst.chunks(9).take(8) {
            assert_eq!(&row[..8], &[1; 8]);
        }
    }

    #[test]
    fn dsp_helpers_follow_c_semantics() {
        // idct_add and add_pixels8 truncate (overflow modulo 256) instead of clamping.
        let mut dst = [250u8; 64];
        let mut block = [0i32; 64];
        block[0] = 256 * 10;
        idct_add(&mut dst, 8, &mut block);
        assert_eq!(dst, [4; 64]);
        let mut res = [0i16; 64];
        res[9] = -5;
        add_pixels8(&mut dst, &res, 8);
        assert_eq!(dst[9], 255);
        // scale_block doubles in both directions.
        let src: [u8; 64] = std::array::from_fn(|i| i as u8);
        let mut big = [0u8; 16 * 16];
        scale_block(&src, &mut big, 16);
        assert_eq!(big[0..4], [0, 0, 1, 1]);
        assert_eq!(big[16..20], [0, 0, 1, 1]);
        assert_eq!(big[32..34], [8, 8]);
        // Copy in the same plane with overlap: for every 4 bytes, first read, then write.
        let mut buf: Vec<u8> = (0..80u8).collect();
        put_pixels8_within(&mut buf, 2, 0, 9);
        assert_eq!(buf[2..10], [0, 1, 2, 3, 2, 3, 6, 7]);
    }
}
