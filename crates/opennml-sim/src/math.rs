//! Transcendental functions of the simulation without libm: `sin`/`cos` of an `f32` angle like x87
//! `FSIN`/`FCOS` followed by `FSTP dword` (core in `f64` after fdlibm, result rounded to `f32`), the fixed-point
//! tables of whole degrees of the original, and the fixed-point angle of a point (`Fixed_Atan2Deg`, re/motion 1.1).

use std::sync::LazyLock;

use crate::fx::Fx;

/// `sin` and `cos` of `theta` (radians), rounded to `f32`. Valid for |theta| < 10^6 (reduction with a two-part pi/2);
/// the world matrices of the objects stay far below that.
pub fn sin_cos(theta: f32) -> (f32, f32) {
    // pi/2 in two parts (fdlibm `pio2_1`, `pio2_1t`).
    const PIO2_1: f64 = 1.570_796_326_734_125_6;
    const PIO2_1T: f64 = 6.077_100_506_506_192e-11;
    let x = f64::from(theta);
    let n = (x * std::f64::consts::FRAC_2_PI).round();
    let r = (x - n * PIO2_1) - n * PIO2_1T;
    let (s, c) = (kernel_sin(r), kernel_cos(r));
    let (sin, cos) = match (n as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (sin as f32, cos as f32)
}

/// fdlibm `__kernel_sin` (|x| <= pi/4, without remainder).
fn kernel_sin(x: f64) -> f64 {
    const S1: f64 = -1.666_666_666_666_663_2e-1;
    const S2: f64 = 8.333_333_333_322_49e-3;
    const S3: f64 = -1.984_126_982_985_795e-4;
    const S4: f64 = 2.755_731_370_707_007e-6;
    const S5: f64 = -2.505_076_025_340_686_3e-8;
    const S6: f64 = 1.589_690_995_211_55e-10;
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    x + v * (S1 + z * r)
}

/// fdlibm `__kernel_cos` (|x| <= pi/4, without remainder).
fn kernel_cos(x: f64) -> f64 {
    const C1: f64 = 4.166_666_666_666_66e-2;
    const C2: f64 = -1.388_888_888_887_411e-3;
    const C3: f64 = 2.480_158_728_947_673e-5;
    const C4: f64 = -2.755_731_435_139_066_3e-7;
    const C5: f64 = 2.087_572_321_298_175e-9;
    const C6: f64 = -1.135_964_755_778_819_5e-11;
    let z = x * x;
    let r = z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6)))));
    let hz = 0.5 * z;
    let w = 1.0 - hz;
    w + (((1.0 - w) - hz) + z * r)
}

/// Sine table of whole degrees 0...180 (`0x0062CE10`): `trunc(4096 · sin k°)`.
pub const SIN_TABLE: [i32; 181] = [
    0, 71, 142, 214, 285, 356, 428, 499, 570, 640, 711, 781, 851, 921, 990, 1060, 1129, 1197, 1265,
    1333, 1400, 1467, 1534, 1600, 1665, 1731, 1795, 1859, 1922, 1985, 2047, 2109, 2170, 2230, 2290,
    2349, 2407, 2465, 2521, 2577, 2632, 2687, 2740, 2793, 2845, 2896, 2946, 2995, 3043, 3091, 3137,
    3183, 3227, 3271, 3313, 3355, 3395, 3435, 3473, 3510, 3547, 3582, 3616, 3649, 3681, 3712, 3741,
    3770, 3797, 3823, 3848, 3872, 3895, 3917, 3937, 3956, 3974, 3991, 4006, 4020, 4033, 4045, 4056,
    4065, 4073, 4080, 4086, 4090, 4093, 4095, 4096, 4095, 4093, 4090, 4086, 4080, 4073, 4065, 4056,
    4045, 4033, 4020, 4006, 3991, 3974, 3956, 3937, 3917, 3895, 3872, 3848, 3823, 3797, 3770, 3741,
    3712, 3681, 3649, 3616, 3582, 3547, 3510, 3473, 3435, 3395, 3355, 3313, 3271, 3227, 3183, 3137,
    3091, 3043, 2995, 2946, 2896, 2845, 2793, 2740, 2687, 2632, 2577, 2521, 2465, 2407, 2349, 2290,
    2230, 2170, 2109, 2047, 1985, 1922, 1859, 1795, 1731, 1665, 1600, 1534, 1467, 1400, 1333, 1265,
    1197, 1129, 1060, 990, 921, 851, 781, 711, 640, 570, 499, 428, 356, 285, 214, 142, 71, 0,
];

/// Cosine table of whole degrees 0...180 (`0x0062D0E8`): `trunc(4096 · cos k°)` (not the mirrored sine table:
/// `cos 60°` gives 2048, `sin 30°` 2047).
pub const COS_TABLE: [i32; 181] = [
    4096, 4095, 4093, 4090, 4086, 4080, 4073, 4065, 4056, 4045, 4033, 4020, 4006, 3991, 3974, 3956,
    3937, 3917, 3895, 3872, 3848, 3823, 3797, 3770, 3741, 3712, 3681, 3649, 3616, 3582, 3547, 3510,
    3473, 3435, 3395, 3355, 3313, 3271, 3227, 3183, 3137, 3091, 3043, 2995, 2946, 2896, 2845, 2793,
    2740, 2687, 2632, 2577, 2521, 2465, 2407, 2349, 2290, 2230, 2170, 2109, 2048, 1985, 1922, 1859,
    1795, 1731, 1665, 1600, 1534, 1467, 1400, 1333, 1265, 1197, 1129, 1060, 990, 921, 851, 781,
    711, 640, 570, 499, 428, 356, 285, 214, 142, 71, 0, -71, -142, -214, -285, -356, -428, -499,
    -570, -640, -711, -781, -851, -921, -990, -1060, -1129, -1197, -1265, -1333, -1400, -1467,
    -1534, -1600, -1665, -1731, -1795, -1859, -1922, -1985, -2047, -2109, -2170, -2230, -2290,
    -2349, -2407, -2465, -2521, -2577, -2632, -2687, -2740, -2793, -2845, -2896, -2946, -2995,
    -3043, -3091, -3137, -3183, -3227, -3271, -3313, -3355, -3395, -3435, -3473, -3510, -3547,
    -3582, -3616, -3649, -3681, -3712, -3741, -3770, -3797, -3823, -3848, -3872, -3895, -3917,
    -3937, -3956, -3974, -3991, -4006, -4020, -4033, -4045, -4056, -4065, -4073, -4080, -4086,
    -4090, -4093, -4095, -4096,
];

/// `Fixed_SinDeg` (`0x00547C40`): fold the angle in degrees*4096 to [-180 degrees, 180 degrees), index = absolute
/// value of the rounded-down degree count (so negative angles are rounded up in magnitude), sign of `a`.
pub fn sin_deg(a: i32) -> i32 {
    let w = crate::world::wrap_angle(a);
    let v = SIN_TABLE[(w >> 12).unsigned_abs() as usize];
    if w < 0 { -v } else { v }
}

/// `Fixed_CosDeg` (`0x00547C70`).
pub fn cos_deg(a: i32) -> i32 {
    COS_TABLE[(crate::world::wrap_angle(a) >> 12).unsigned_abs() as usize]
}

/// Rotates `v` about the vertical axis by the angle `direction + 90 degrees` (direction in degrees*4096): the
/// coupling and muzzle calculation of the original (`Matrix_RotationY` `0x005755A0` with the table-angle input, then
/// `Vector3_TransformCoord` `0x00575250`; re/train-multipart 3.1, re/ships 6). With theta = `wrap(direction + 90°)`,
/// `c`/`s` = `cos_deg`/`sin_deg(theta) / 4096` as `f32`, `x' = x·c − z·s`, `z' = x·s + z·c`, `y' = y` hold
/// (`Matrix_RotationY` writes m[2] = +s, m[8] = -s, `TransformCoord` computes x' = x*m0 + z*m8, z' = x*m2 + z*m10 -
/// like `footprint::world_matrix`). For multiples of 90 degrees the result is exact.
pub fn rotate_y_by_direction(v: [f32; 3], direction: Fx) -> [f32; 3] {
    let theta = crate::world::wrap_angle(direction.0.wrapping_add(90 << 12));
    let c = cos_deg(theta) as f32 / 4096.0;
    let s = sin_deg(theta) as f32 / 4096.0;
    [v[0] * c - v[2] * s, v[1], v[0] * s + v[2] * c]
}

/// `atan` after fdlibm (`s_atan.c`) for `0 ≤ x ≤ 1`, without libm.
fn atan01(x: f64) -> f64 {
    const ATAN_HI: [f64; 2] = [4.636_476_090_008_061e-1, std::f64::consts::FRAC_PI_4];
    const ATAN_LO: [f64; 2] = [2.269_877_745_296_168_7e-17, 3.061_616_997_868_383e-17];
    const AT: [f64; 11] = [
        3.333_333_333_333_293e-1,
        -1.999_999_999_987_648_3e-1,
        1.428_571_427_250_346_6e-1,
        -1.111_111_040_546_235_6e-1,
        9.090_887_133_436_507e-2,
        -7.691_876_205_044_83e-2,
        6.661_073_137_387_531e-2,
        -5.833_570_133_790_573_5e-2,
        4.976_877_994_615_932_4e-2,
        -3.653_157_274_421_691_6e-2,
        1.628_582_011_536_578_2e-2,
    ];
    let (id, x) = if x < 0.4375 {
        (None, x)
    } else if x < 0.6875 {
        (Some(0), (2.0 * x - 1.0) / (2.0 + x))
    } else {
        (Some(1), (x - 1.0) / (x + 1.0))
    };
    let z = x * x;
    let w = z * z;
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    match id {
        None => x - x * (s1 + s2),
        Some(i) => ATAN_HI[i] - ((x * (s1 + s2) - ATAN_LO[i]) - x),
    }
}

/// Arctangent table (`0x0062D3C0`, 4097 entries): `trunc(4096 · deg(atan(i / 4096)))`. No entry lies closer than
/// `1.8·10⁻⁵` to an integer, so any exact `f64` calculation hits the values of the EXE (stored there as data).
static ATAN_TABLE: LazyLock<Vec<i32>> = LazyLock::new(|| {
    (0..=4096)
        .map(|i| (atan01(f64::from(i) / 4096.0) * (180.0 / std::f64::consts::PI) * 4096.0) as i32)
        .collect()
});

/// `Fixed_Atan2Deg(dz, dx)` (`0x00547C90`): angle of the vector `(dx, dz)` in degrees*4096, 0 degrees = +x,
/// 90 degrees = +z, folded to [-180 degrees, 180 degrees). From a distance of one tile on, the table index is always
/// a multiple of 4 because of `Fixed_Div` (quirk).
pub fn atan2_deg(dz: i32, dx: i32) -> i32 {
    use crate::fx::{fixed_div, fixed_mul};
    if dx == 0 && dz == 0 {
        return 0;
    }
    let (ax, az) = (dx.wrapping_abs(), dz.wrapping_abs());
    let swapped = az > ax;
    let (lo, hi) = if swapped { (ax, az) } else { (az, ax) };
    // Deviation (E-118): divisor 0 (larger component congruent to 0...3 or to 2^22 - 3...2^22 - 1 modulo 2^22, or 0
    // if `wrapping_abs(i32::MIN)` is negative and counts as the smaller one) crashes in the original; here index
    // 4096 for `lo != 0` (like the clamping of `lo / s`), otherwise 0. All other inputs calculate bit-identically.
    let at_zero_divisor = i32::from(lo != 0) << 12;
    let index = if hi > 0xFFF {
        match fixed_div(hi, 0x1000) {
            0 => at_zero_divisor,
            s => fixed_div(lo, s),
        }
    } else if hi == 0 {
        at_zero_divisor
    } else {
        fixed_mul(lo, fixed_div(0x1000, hi))
    };
    let mut a = ATAN_TABLE[index.clamp(0, 4096) as usize];
    if swapped {
        a = (90 << 12) - a;
    }
    if dx < 0 {
        a = (180 << 12) - a;
    }
    if dz < 0 {
        a = -a;
    }
    crate::world::wrap_angle(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E-118 in `Fixed_Atan2Deg`: if the divisor becomes 0 (larger component congruent to 0...3 or to
    /// 2^22 - 3...2^22 - 1 modulo 2^22, or larger component 0 after `wrapping_abs`, i.e. `dx` or `dz = i32::MIN`
    /// against 0), the index becomes 4096 (for `lo != 0`) or 0, like the clamping of `lo / s` to 4096.
    // QUIRK E-118: deviation
    #[test]
    fn atan2_deg_with_divisor_zero_does_not_divide_by_zero() {
        let t45 = ATAN_TABLE[4096];
        assert_eq!(atan2_deg(0, 1 << 22), 0);
        assert_eq!(atan2_deg(1, 1 << 22), t45);
        assert_eq!(atan2_deg(1 << 22, 1), (90 << 12) - t45);
        for hi in [
            1 << 22,
            (1 << 22) - 1,
            (1 << 22) + 3,
            3 << 22,
            (511 << 22) + 3,
        ] {
            for lo in [0, 1, -1, 4096, hi / 2, hi] {
                for (dz, dx) in [(lo, hi), (hi, lo), (lo, -hi), (-hi, lo), (-lo, -hi)] {
                    let a = atan2_deg(dz, dx);
                    assert!(
                        (-(180 << 12)..(180 << 12)).contains(&a),
                        "atan2({dz}, {dx}) = {a}"
                    );
                }
            }
        }
        for (dz, dx) in [
            (0, i32::MIN),
            (i32::MIN, 0),
            (i32::MIN, i32::MIN),
            (5, i32::MIN),
        ] {
            let a = atan2_deg(dz, dx);
            assert!(
                (-(180 << 12)..(180 << 12)).contains(&a),
                "atan2({dz}, {dx}) = {a}"
            );
        }
    }

    #[test]
    fn rotate_y_by_direction_is_matrix_rotation_y_with_table_values() {
        // theta = wrap(direction + 90 degrees), c/s = cos_deg/sin_deg(theta) / 4096 (exact for multiples of 90
        // degrees); `Matrix_RotationY` (0x005755A0) writes m[2] = +s, m[8] = -s, `Vector3_TransformCoord`
        // (0x00575250) computes x' = x*c - z*s, z' = x*s + z*c.
        use crate::fx::Fx;
        let v = [1.0, 2.0, 3.0];
        assert_eq!(rotate_y_by_direction(v, Fx(0)), [-3.0, 2.0, 1.0]);
        assert_eq!(rotate_y_by_direction(v, Fx(90 << 12)), [-1.0, 2.0, -3.0]);
        assert_eq!(rotate_y_by_direction(v, Fx(-90 << 12)), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn tables_truncate_the_degree_sines() {
        for k in 0..=180usize {
            let r = (k as f64).to_radians();
            assert_eq!(SIN_TABLE[k], (r.sin() * 4096.0) as i32, "sin {k}");
            assert_eq!(COS_TABLE[k], (r.cos() * 4096.0) as i32, "cos {k}");
        }
        assert_eq!((SIN_TABLE[30], COS_TABLE[60]), (2047, 2048));
        // `−0.5°` becomes -1 degree (index 1), `0.5 rad` (-117,341) becomes -29 degrees.
        assert_eq!(sin_deg(-2048), -71);
        assert_eq!((sin_deg(-117_341), cos_deg(-117_341)), (-1985, 3582));
        assert_eq!((sin_deg(-368_640), cos_deg(-368_640)), (-4096, 0));
        assert_eq!((sin_deg(737_280), cos_deg(737_280)), (0, -4096));
    }

    #[test]
    fn sine_and_cosine_round_like_the_fpu_to_f32() {
        // cos(f32(pi/2)) is not 0 (`0xB33BBD2E`).
        let (s, c) = sin_cos(std::f32::consts::FRAC_PI_2);
        assert_eq!((s, c.to_bits()), (1.0, 0xB33B_BD2E));
        assert_eq!(sin_cos(0.0), (0.0, 1.0));
        // Against the runtime library (correctly rounded except for rare edge cases).
        let mut x = -20.0f32;
        let mut differences = 0;
        while x < 20.0 {
            let (s, c) = sin_cos(x);
            let (rs, rc) = ((x as f64).sin() as f32, (x as f64).cos() as f32);
            differences += usize::from(s != rs) + usize::from(c != rc);
            x += 0.000_37;
        }
        assert_eq!(differences, 0);
    }

    #[test]
    fn the_arc_tangent_table_and_angles_match_the_original() {
        // Against the runtime library (in the test only) and samples of the EXE table.
        for i in 0..=4096usize {
            let r = ((i as f64 / 4096.0).atan().to_degrees() * 4096.0) as i32;
            assert_eq!(ATAN_TABLE[i], r, "atan {i}");
        }
        assert_eq!((ATAN_TABLE[1], ATAN_TABLE[4096]), (57, 184_320));
        // Test vectors `(dz, dx) → angle` from the reimplementation of the disassembly (re/motion 1.1).
        for (dz, dx, a) in [
            (0, 1, 0),
            (1, 0, 368_640),
            (0, -1, -737_280),
            (-1, 0, -368_640),
            (4096, 4096, 184_320),
            (-4096, 4096, -184_320),
            (4096, -4096, 552_960),
            (1, 4096, 0),
            (19115, 32768, 123_873),
            (100, 3, 361_652),
            (-7, -7000, -737_051),
            (0, 0, 0),
            (3, 4096, 0),
            (4, 4096, 229),
        ] {
            assert_eq!(atan2_deg(dz, dx), a, "atan2({dz}, {dx})");
        }
    }
}
