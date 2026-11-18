/*
 * Copyright (c) Lynne
 *
 * Power of two FFT:
 * Copyright (c) Lynne
 * Copyright (c) 2008 Loren Merritt
 * Copyright (c) 2002 Fabrice Bellard
 * Partly based on libdjbfft by D. J. Bernstein
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

//! Inverse DCT (DCT-III) for the Bink sound.
//!
//! Ported from FFmpeg n9.0.2, `libavutil/tx_template.c` (`ff_tx_dct_init` and `ff_tx_dctIII` for
//! `AV_TX_FLOAT_DCT` inverse, underneath `ff_tx_rdft_init` and `ff_tx_rdft_c2r`). The complex FFT
//! underneath is a custom radix-2 FFT (FFmpeg chooses a split-radix FFT, in assembler on x86); the
//! results agree up to rounding.
//!
//! The tables are built at setup with `f64::cos`/`f64::sin` as in FFmpeg (`cos`/`sin` in `double`,
//! then rounded to `float`); the transform itself computes in `f32` in a fixed order.
//!
//! Modification notice (LGPL-2.1 section 2 b): Ported to Rust and modified for `opennml-bink`, 2026-10-25.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use std::f64::consts::PI;

/// Inverse DCT of length `len` (a power of two from 8) via a real FFT of length `len`.
///
/// Corresponds to `av_tx_init(AV_TX_FLOAT_DCT, inv = 1, len / 2, &scale)`: FFmpeg doubles the length passed in
/// on the inverse path (`ff_tx_dct_init`); here `len` is equal to the number of values. Result:
/// `y[n] = scale · (x[0] + 2·Σ_{k≥1} x[k]·cos(π·k·(2n + 1)/(2·len)))`.
pub struct DctIII {
    len: usize,
    /// `s->exp` from `ff_tx_dct_init`: `cos(i·π/(2·len))` for `i < len`, followed by
    /// `0.5 / sin((2i + 1)·π/(2·len))` for `i < len/2`.
    exp: Vec<f32>,
    /// The eight factors from `ff_tx_rdft_init` (inverse path).
    fact: [f32; 8],
    /// `tcos` from `ff_tx_rdft_init`: `cos(2π·i/len)` for `i < len/4`.
    tcos: Vec<f32>,
    /// `tsin` from `ff_tx_rdft_init`: `cos((len − 4i)/4 · 2π/len)` for `i < len/4`.
    tsin: Vec<f32>,
    fft: Fft,
    /// Input with the two padding values that `ff_tx_dctIII` requires.
    src: Vec<f32>,
    /// Output of the FFT (`len/2` complex values).
    dst: Vec<f32>,
}

impl DctIII {
    /// Builds the tables (`ff_tx_dct_init` with `inv = 1`, including `ff_tx_rdft_init`). `scale` is the
    /// scaling value of `av_tx_init`. Panics for a length that is not a power of two from 8 (the sound decoder
    /// uses only 512, 1024 and 2048).
    pub fn new(len: usize, scale: f32) -> Self {
        assert!(
            len >= 8 && len.is_power_of_two(),
            "DCT length {len} is not a power of two from 8"
        );
        // ff_tx_dct_init: on the inverse path `rsc *= 0.5` (in `float`).
        let rsc = scale * 0.5;
        let freq = PI / (len as f64 * 2.0);
        let mut exp = Vec::with_capacity(len + len / 2);
        for i in 0..len {
            exp.push((i as f64 * freq).cos() as f32);
        }
        for i in 0..len / 2 {
            exp.push((0.5 / ((2 * i + 1) as f64 * freq).sin()) as f32);
        }

        // ff_tx_rdft_init with `inv = 1`: m = 2*scale_d.
        let m = 2.0 * f64::from(rsc);
        let fact = [
            (0.5 * m) as f32,
            (0.5 * m) as f32,
            m as f32,
            (-m) as f32,
            ((0.5 - 0.0) * m) as f32,
            ((0.0 - 0.5) * m) as f32,
            ((0.5 - 1.0) * m) as f32,
            (-(0.5 - 1.0) * m) as f32,
        ];
        let len4 = len / 4;
        let f = 2.0 * PI / len as f64;
        let tcos = (0..len4).map(|i| (i as f64 * f).cos() as f32).collect();
        let tsin = (0..len4)
            .map(|i| (((len - i * 4) as f64 / 4.0) * f).cos() as f32)
            .collect();

        DctIII {
            len,
            exp,
            fact,
            tcos,
            tsin,
            fft: Fft::new(len / 2),
            src: vec![0.0; len + 2],
            dst: vec![0.0; len],
        }
    }

    /// Transforms `data` (exactly `len` values) in place (`ff_tx_dctIII`).
    pub fn transform(&mut self, data: &mut [f32]) {
        let len = self.len;
        assert_eq!(data.len(), len, "DCT input has the wrong length");
        let src = &mut self.src;
        let exp = &self.exp;
        src[..len].copy_from_slice(data);

        // ff_tx_dctIII, preparation.
        src[len] = 2.0 * src[len - 1];
        let mut i = len - 2;
        while i >= 2 {
            let val1 = src[i];
            let val2 = src[i - 1] - src[i + 1];
            // CMUL(src[i + 1], src[i], exp[len - i], exp[i], val1, val2)
            src[i + 1] = exp[len - i] * val1 - exp[i] * val2;
            src[i] = exp[len - i] * val2 + exp[i] * val1;
            i -= 2;
        }

        self.rdft_c2r();

        // ff_tx_dctIII, finishing.
        let exp = &self.exp;
        let dst = &mut self.dst;
        for i in 0..len / 2 {
            let in1 = dst[i];
            let in2 = dst[len - i - 1];
            let c = exp[len + i];
            let tmp1 = in1 + in2;
            let mut tmp2 = in1 - in2;
            tmp2 *= c;
            dst[i] = tmp1 + tmp2;
            dst[len - i - 1] = tmp1 - tmp2;
        }
        data.copy_from_slice(dst);
    }

    /// Inverse real FFT of length `len` from `src` (`len/2 + 1` complex values, real and imaginary part
    /// alternating) to `dst` (`ff_tx_rdft_c2r`).
    fn rdft_c2r(&mut self) {
        let len2 = self.len / 2;
        let len4 = self.len / 4;
        let fact = &self.fact;
        let d = &mut self.src;
        let (re, im) = (|k: usize| 2 * k, |k: usize| 2 * k + 1);

        d[im(0)] = d[re(len2)];
        let t0re = d[re(0)];
        d[re(0)] = t0re + d[im(0)];
        d[im(0)] = t0re - d[im(0)];
        d[re(0)] *= fact[0];
        d[im(0)] *= fact[1];
        d[re(len4)] *= fact[2];
        d[im(len4)] *= fact[3];

        for i in 1..len4 {
            let j = len2 - i;
            // Separate the even and odd FFT.
            let t0re = fact[4] * (d[re(i)] + d[re(j)]);
            let t0im = fact[5] * (d[im(i)] - d[im(j)]);
            let t1re = fact[6] * (d[im(i)] + d[im(j)]);
            let t1im = fact[7] * (d[re(i)] - d[re(j)]);
            // CMUL(t2.re, t2.im, t1.re, t1.im, tcos[i], tsin[i])
            let t2re = t1re * self.tcos[i] - t1im * self.tsin[i];
            let t2im = t1re * self.tsin[i] + t1im * self.tcos[i];
            d[re(i)] = t0re + t2re;
            d[im(i)] = t2im - t0im;
            d[re(j)] = t0re - t2re;
            d[im(j)] = t2im + t0im;
        }

        self.fft.inverse(&self.src[..self.len], &mut self.dst);
    }
}

/// Complex FFT in the inverse direction (`e^{+2πi·jk/n}`) without scaling, radix 2.
struct Fft {
    n: usize,
    /// Bit reversal of the indices.
    rev: Vec<usize>,
    /// Twiddle factors `cos(2πk/n)`, `sin(2πk/n)` for `k < n/2`.
    twiddle: Vec<[f32; 2]>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| {
                if bits == 0 {
                    0
                } else {
                    i.reverse_bits() >> (usize::BITS - bits)
                }
            })
            .collect();
        let twiddle = (0..n / 2)
            .map(|k| {
                let a = 2.0 * PI * k as f64 / n as f64;
                [a.cos() as f32, a.sin() as f32]
            })
            .collect();
        Fft { n, rev, twiddle }
    }

    /// `dst[k] = Σ_j src[j] · e^{+2πi·jk/n}`; `src` and `dst` hold `n` complex values (real and
    /// imaginary part alternating). Bit reversal, then butterflies of size 2 up to `n`.
    fn inverse(&self, src: &[f32], dst: &mut [f32]) {
        let n = self.n;
        for (i, &j) in self.rev.iter().enumerate() {
            dst[2 * i] = src[2 * j];
            dst[2 * i + 1] = src[2 * j + 1];
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let [wr, wi] = self.twiddle[k * step];
                    let a = start + k;
                    let b = a + half;
                    let (br, bi) = (dst[2 * b], dst[2 * b + 1]);
                    let tr = br * wr - bi * wi;
                    let ti = br * wi + bi * wr;
                    let (ar, ai) = (dst[2 * a], dst[2 * a + 1]);
                    dst[2 * a] = ar + tr;
                    dst[2 * a + 1] = ai + ti;
                    dst[2 * b] = ar - tr;
                    dst[2 * b + 1] = ai - ti;
                }
            }
            size *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed random generator (xorshift64) for values in [-1, 1).
    fn values(len: usize, mut state: u64) -> Vec<f32> {
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 40) as f64 / (1u64 << 23) as f64 - 1.0) as f32
            })
            .collect()
    }

    /// The formula that `ff_tx_dctIII` computes with the scaling value `scale` (in `f64`):
    /// `y[n] = scale · (x[0] + 2·Σ_{k≥1} x[k]·cos(π·k·(2n + 1)/(2·len)))`.
    fn naive(x: &[f32], scale: f64) -> Vec<f64> {
        let len = x.len();
        (0..len)
            .map(|n| {
                let mut sum = f64::from(x[0]);
                for (k, &v) in x.iter().enumerate().skip(1) {
                    let a = PI * k as f64 * (2 * n + 1) as f64 / (2 * len) as f64;
                    sum += 2.0 * f64::from(v) * a.cos();
                }
                scale * sum
            })
            .collect()
    }

    #[test]
    fn dct_iii_matches_the_naive_formula() {
        for (len, seed) in [(16usize, 1u64), (512, 2), (1024, 3)] {
            let scale = 1.0 / len as f32;
            let x = values(len, 0x9E37_79B9_7F4A_7C15 ^ seed);
            let want = naive(&x, f64::from(scale));
            let mut got = x.clone();
            let mut dct = DctIII::new(len, scale);
            dct.transform(&mut got);
            let peak = want.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            for (n, (&g, &w)) in got.iter().zip(&want).enumerate() {
                let err = (f64::from(g) - w).abs() / peak;
                assert!(err <= 1e-4, "length {len}, value {n}: {g} instead of {w}");
            }
        }
    }

    #[test]
    fn dct_iii_is_repeatable() {
        let x = values(64, 7);
        let mut dct = DctIII::new(64, 0.25);
        let mut a = x.clone();
        let mut b = x;
        dct.transform(&mut a);
        dct.transform(&mut b);
        assert_eq!(a, b);
    }
}
