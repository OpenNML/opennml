//! Fixed point 20.12 as in the original (`Fixed_FromInt` `0x00547960`, `Fixed_ToInt` `0x00547970`,
//! `Fixed_FromFloat` `0x00547980`, `Fixed_ToFloat` `0x00547990`): 4096 = one tile or one
//! degree. All computations run in `i32` with overflow like x86 (`wrapping_*`).

use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Fractional bits.
pub const FX_SHIFT: u32 = 12;

/// `f32` -> `i32` like `__ftol2`/`cvttss2si`: truncate toward 0; NaN and values outside
/// `i32` give `i32::MIN` (`0x80000000`, "indefinite"), not the saturated value.
pub fn ftol(value: f32) -> i32 {
    if !(-2_147_483_648.0..2_147_483_648.0).contains(&value) {
        i32::MIN
    } else {
        value as i32
    }
}

/// Number in the 20.12 fixed-point format.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Fx(pub i32);

impl Fx {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1 << FX_SHIFT);

    /// `Fixed_FromInt`: `v << 12` (overflow like x86 `SHL`).
    pub const fn from_int(v: i32) -> Self {
        Self(v.wrapping_shl(FX_SHIFT))
    }

    /// `Fixed_ToInt`: arithmetic `>> 12` (rounds toward -∞).
    pub const fn to_int(self) -> i32 {
        self.0 >> FX_SHIFT
    }

    /// `Fixed_FromFloat`: `trunc(v · 4096)` (the multiplication by 4096 is exact in `f32`).
    pub fn from_f32(v: f32) -> Self {
        Self(ftol(v * 4096.0))
    }

    /// `Fixed_ToFloat`: `v / 4096` in `f32`.
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / 4096.0
    }
}

impl Add for Fx {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

impl Sub for Fx {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}

impl Neg for Fx {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.wrapping_neg())
    }
}

impl AddAssign for Fx {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Fx {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

/// `Fixed_Div` (`0x00547A80`): `((a << 10) / b) << 2`, division toward 0, overflow like x86.
pub fn fixed_div(a: i32, b: i32) -> i32 {
    a.wrapping_shl(10).wrapping_div(b).wrapping_shl(2)
}

/// `Fixed_Mul` (`0x00547A60`): halves the larger factor until the product fits in `i32`, and
/// shifts by the saved digits less (shift amount like x86 modulo 32).
///
/// Deviation (K-324): for `b == i32::MIN` the function ends immediately with 0. The original hangs there for negative
/// `a` (the absolute value of `INT_MIN` stays negative, `a` only halves down to -1) and likewise returns 0 in all
/// other cases with this `b`; for all terminating inputs the result is therefore bit-identical. So no value from
/// the savegame (formation distance, train speed, drawbar) hangs the tick.
pub fn fixed_mul(mut a: i32, mut b: i32) -> i32 {
    if b == 0 || b == i32::MIN {
        return 0;
    }
    let mut k = 0;
    while a.wrapping_abs() > i32::MAX / b.wrapping_abs() {
        if a.wrapping_abs() > b.wrapping_abs() {
            a >>= 1;
        } else {
            b >>= 1;
        }
        k += 1;
    }
    a.wrapping_mul(b).wrapping_shr((12 - k) as u32)
}

/// Table of `Fixed_Hypot` (`0x00628E08`, 4097 entries): `⌊√(4096² + i²)⌋`, i.e.
/// `hypot(1, i/4096)` in fixed point (checked against the table of the EXE).
static HYPOT_TABLE: [u32; 4097] = {
    let mut t = [0u32; 4097];
    let mut i = 0;
    while i < 4097 {
        t[i] = (4096u64 * 4096 + (i as u64) * (i as u64)).isqrt() as u32;
        i += 1;
    }
    t
};

/// Root table `0x00628600`: 514 words up to the hypot table (`0x00628E08`), of which 513 real entries
/// `⌊√i · 4096⌋ = isqrt(i << 24)` for `i = 0 … 512`; entry 513 is 0 in the EXE (re/groups-formations 12.5;
/// the first 16 entries and entry 513 compared with the EXE). Here entry 513 is exact too (K-255, A7).
static SQRT_TABLE: [i32; 514] = {
    let mut t = [0i32; 514];
    let mut i = 0;
    while i < 514 {
        t[i] = ((i as u64) << 24).isqrt() as i32;
        i += 1;
    }
    t
};

/// `Fixed_SqrtInt` (`0x00547AA0`): fixed-point root of an integer. From 512 recursive with
/// `k = n / 512`. For `n / k ≥ 513` the original reads the zero in entry 513 or hypot values behind the
/// table (K-255); exact here (A7).
pub fn fixed_sqrt_int(n: usize) -> i32 {
    if n < 512 {
        return SQRT_TABLE[n];
    }
    let k = n / 512;
    let i = n / k;
    let t = SQRT_TABLE
        .get(i)
        .copied()
        .unwrap_or_else(|| ((i as u64) << 24).isqrt() as i32);
    fixed_mul(t, fixed_sqrt_int(k))
}

/// `Fixed_Hypot` (`0x00547B90`): length of `(p, q)` via the table.
///
/// Deviation (E-118): if the divisor `s` becomes 0, the function ends with 0. The original divides by zero there and
/// crashes: in the first branch when the larger difference is congruent to 0-3 or 2^22 - 3 ... 2^22 - 1 modulo 2^22
/// (`Fixed_Div(hi, 0x1000)` runs to 0 in `i32`), in the second only for `(i32::MIN, i32::MIN)`. All other inputs
/// compute bit-identically. 0 instead of a large value because the callers tolerate a distance of 0 and
/// `walk::MAX_SEEK_DISTANCE` presupposes a result of at most about 1.415 * 2^21.
pub fn fixed_hypot(p: i32, q: i32) -> i32 {
    let (a, b) = (p.wrapping_abs(), q.wrapping_abs());
    let (lo, hi) = (a.min(b), a.max(b));
    if hi == 0 {
        return 0;
    }
    let entry = |i: i32| HYPOT_TABLE[i.clamp(0, 4096) as usize] as i32;
    if hi >= 0x1000 {
        let s = fixed_div(hi, 0x1000);
        if s == 0 {
            return 0;
        }
        fixed_mul(entry(fixed_div(lo, s)), s)
    } else {
        let s = fixed_div(0x1000, hi);
        if s == 0 {
            return 0;
        }
        fixed_div(entry(fixed_mul(lo, s)), s)
    }
}

/// Grid position of an object (`GameObject+0x19C` x, `+0x1A0` z, `+0x1A4` y) in tiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FxVec3 {
    pub x: Fx,
    pub y: Fx,
    pub z: Fx,
}

impl FxVec3 {
    pub const ZERO: Self = Self {
        x: Fx::ZERO,
        y: Fx::ZERO,
        z: Fx::ZERO,
    };

    pub const fn new(x: Fx, y: Fx, z: Fx) -> Self {
        Self { x, y, z }
    }

    /// Tile (`x >> 12`, `z >> 12`).
    pub const fn tile(self) -> (i32, i32) {
        (self.x.to_int(), self.z.to_int())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // QUIRK K-255: deviation
    #[test]
    fn fixed_sqrt_int_matches_the_table_of_the_exe() {
        // The first 16 entries of 0x00628600 (re/groups-formations 12.5).
        let exe = [
            0, 4096, 5792, 7094, 8192, 9158, 10033, 10836, 11585, 12288, 12952, 13584, 14188,
            14768, 15325, 15863,
        ];
        for (i, &v) in exe.iter().enumerate() {
            assert_eq!(fixed_sqrt_int(i), v, "SqrtTab[{i}]");
        }
        // Recursion from 512: k = 1024 / 512 = 2, Tab[512] * sqrt(2).
        assert_eq!(
            fixed_sqrt_int(1024),
            fixed_mul(fixed_sqrt_int(512), fixed_sqrt_int(2))
        );
        // K-255: beyond the end of the table exact instead of hypot values (A7), without a panic.
        assert_eq!(
            fixed_sqrt_int(600),
            fixed_mul((600u64 << 24).isqrt() as i32, 4096)
        );
        let _ = fixed_sqrt_int(1 << 30);
    }

    #[test]
    fn conversions_follow_the_original_rounding() {
        assert_eq!(Fx::from_int(3), Fx(12288));
        assert_eq!(Fx::from_int(-1).0, -4096);
        // `>> 12` rounds toward -∞, `from_f32` truncates toward 0.
        assert_eq!(Fx(-1).to_int(), -1);
        assert_eq!(Fx(4095).to_int(), 0);
        assert_eq!(Fx::from_f32(1.9999), Fx(8191));
        assert_eq!(Fx::from_f32(-1.9999), Fx(-8191));
        assert_eq!(Fx::from_f32(7.8125), Fx(32000));
        assert_eq!(Fx(32000).to_f32(), 7.8125);
        // Overflow and NaN like `cvttss2si`.
        assert_eq!(ftol(f32::NAN), i32::MIN);
        assert_eq!(ftol(3.0e9), i32::MIN);
        assert_eq!(ftol(-2_147_483_648.0), i32::MIN);
        assert_eq!(ftol(2_147_483_520.0), 2_147_483_520);
        assert_eq!(Fx::from_int(0x8_0000), Fx(i32::MIN));
        assert_eq!(Fx(i32::MAX) + Fx(1), Fx(i32::MIN));
        let p = FxVec3::new(Fx(32000), Fx(0), Fx(-1));
        assert_eq!(p.tile(), (7, -1));
    }

    #[test]
    fn table_hypot_matches_the_original_values() {
        // Values from the reimplementation against the table of the EXE (re/path-grid 4.8).
        assert_eq!(
            (HYPOT_TABLE[0], HYPOT_TABLE[2048], HYPOT_TABLE[4096]),
            (4096, 4579, 5792)
        );
        for (p, q, d) in [
            (4096, 0, 4096),
            (4096, 4096, 5792),
            (12288, 16384, 20480),
            (20480, 20480, 28960),
            (28672, 4096, 28959),
            (1_044_480, 1_044_480, 1_476_960),
            (2048, 0, 2048),
            (1000, 1000, 1412),
            (0, 0, 0),
            (-4096, 0, 4096),
        ] {
            assert_eq!(fixed_hypot(p, q), d, "hypot({p}, {q})");
        }
        assert_eq!(fixed_div(3 << 12, 2 << 12), 6144);
        assert_eq!(fixed_mul(3 << 12, 2 << 12), 6 << 12);
        assert_eq!(fixed_mul(5, 0), 0);
    }

    /// E-118: if the divisor `Fixed_Div(hi, 0x1000)` or `Fixed_Div(0x1000, hi)` becomes 0, `fixed_hypot` ends with 0
    /// instead of dividing by zero (larger difference congruent to 0-3 or 2^22 - 3 ... 2^22 - 1 modulo 2^22,
    /// plus `(MIN, MIN)`).
    // QUIRK E-118: deviation
    #[test]
    fn fixed_hypot_with_divisor_zero_ends_with_zero() {
        let mut his = Vec::new();
        for k in 0..512i64 {
            for r in [0i64, 1, 2, 3, (1 << 22) - 3, (1 << 22) - 2, (1 << 22) - 1] {
                let hi = (k << 22) + r;
                if (0x1000..=i64::from(i32::MAX)).contains(&hi) {
                    his.push(hi as i32);
                }
            }
        }
        assert!(his.contains(&(1 << 22)) && his.contains(&((1 << 22) - 1)));
        for hi in his {
            assert_eq!(fixed_div(hi, 0x1000), 0, "divisor for {hi}");
            for lo in [0, 1, 4096, hi / 2, hi] {
                for (p, q) in [(hi, lo), (lo, hi), (-hi, lo), (lo, -hi), (-hi, -lo)] {
                    assert_eq!(fixed_hypot(p, q), 0, "hypot({p}, {q})");
                }
            }
        }
        assert_eq!(fixed_hypot(i32::MIN, i32::MIN), 0);
        // Just beside it `Fixed_Div` does not divide by zero and computes unchanged.
        assert_ne!(fixed_hypot((1 << 22) + 4, 0), 0);
        assert_ne!(fixed_hypot((1 << 22) - 4, 0), 0);
    }

    // QUIRK K-324: deviation
    #[test]
    fn fixed_mul_with_int_min_ends_with_zero() {
        for a in [-1, -5, -4096, i32::MIN + 1, i32::MIN, 0, 1, 4096, i32::MAX] {
            assert_eq!(fixed_mul(a, i32::MIN), 0, "a = {a}");
        }
    }
}
