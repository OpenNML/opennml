//! 3D attenuation, panorama and pan law of the software mixer of FMOD 3.71
//! (re/audio, section "FMOD mixer"; `SWChan_Update3D` `0x10014078`, `SWChan_SetVolume`
//! `0x10013647`, `SWChan_SetPan` `0x100136b7` in `fmod.dll`). Measured bit-exactly against the
//! original DLL: inputs `f32`, intermediate computation like the x87 in `f64`, storage locations as `f32`,
//! truncating instead of rounding.

/// Pan table `g_PanTable` (`0x1003F538`): gain per side (about `255·√(p/255)`, but not
/// exact; center 128 gives 180 on the left and 181 on the right).
pub const PAN_TABLE: [u8; 256] = [
    0, 16, 22, 27, 32, 35, 39, 42, 45, 48, 50, 53, 55, 57, 59, 61, //
    64, 65, 67, 69, 71, 73, 75, 76, 78, 80, 81, 83, 84, 86, 87, 89, //
    90, 91, 93, 94, 96, 97, 98, 99, 101, 102, 103, 104, 106, 107, 108, 109, //
    110, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, //
    128, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, //
    143, 144, 144, 145, 146, 147, 148, 149, 150, 150, 151, 152, 153, 154, 155, 155, //
    156, 157, 158, 159, 160, 160, 161, 162, 163, 163, 164, 165, 166, 167, 167, 168, //
    169, 170, 170, 171, 172, 173, 173, 174, 175, 176, 176, 177, 178, 178, 179, 180, //
    181, 181, 182, 183, 183, 184, 185, 185, 186, 187, 187, 188, 189, 189, 190, 191, //
    192, 192, 193, 193, 194, 195, 195, 196, 197, 197, 198, 199, 199, 200, 201, 201, //
    202, 203, 203, 204, 204, 205, 206, 206, 207, 208, 208, 209, 209, 210, 211, 211, //
    212, 212, 213, 214, 214, 215, 215, 216, 217, 217, 218, 218, 219, 219, 220, 221, //
    221, 222, 222, 223, 224, 224, 225, 225, 226, 226, 227, 227, 228, 229, 229, 230, //
    230, 231, 231, 232, 232, 233, 234, 234, 235, 235, 236, 236, 237, 237, 238, 238, //
    239, 240, 240, 241, 241, 242, 242, 243, 243, 244, 244, 245, 245, 246, 246, 247, //
    247, 248, 248, 249, 249, 250, 250, 251, 251, 252, 252, 253, 253, 254, 254, 255, //
];

/// Pan of a channel: 0 = left, 128 = center, 255 = right; `Stereo` = `FSOUND_STEREOPAN` (-1),
/// both sides at full volume (default for stereo samples and streams).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pan {
    Position(u8),
    Stereo,
}

/// Center, default for mono samples and mono streams.
pub const PAN_CENTER: Pan = Pan::Position(128);

/// Gain left/right (0-255) from 3D volume, channel volume and pan
/// (`m = vol3d·vol/255`, `L = PAN[255−pan]·m/255`, `R = PAN[pan]·m/255`, integer). For
/// `Stereo`, `L = R = m` applies.
pub fn channel_gains(vol3d: i32, volume: i32, pan: Pan) -> (i32, i32) {
    let m = vol3d * volume.clamp(0, 255) / 255;
    match pan {
        Pan::Stereo => (m, m),
        Pan::Position(p) => (
            i32::from(PAN_TABLE[255 - usize::from(p)]) * m / 255,
            i32::from(PAN_TABLE[usize::from(p)]) * m / 255,
        ),
    }
}

/// Listener as FMOD stores it (`FSOUND_3D_Listener_SetAttributes` `0x100216CA`): position and
/// the **non-normalized** right vector `up × forward` as `f32`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FmodListener {
    pub position: [f32; 3],
    pub right: [f32; 3],
}

impl FmodListener {
    pub fn new(position: [f32; 3], forward: [f32; 3], up: [f32; 3]) -> Self {
        let [fx, fy, fz] = forward.map(f64::from);
        let [tx, ty, tz] = up.map(f64::from);
        Self {
            position,
            right: [
                (fz * ty - tz * fy) as f32,
                (tz * fx - fz * tx) as f32,
                (tx * fy - fx * ty) as f32,
            ],
        }
    }
}

impl Default for FmodListener {
    /// Origin, forward +z, up +y (right +x).
    fn default() -> Self {
        Self::new([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
    }
}

/// 3D volume (0-255) and pan (0-255) of a source (`SWChan_Update3D`): distance clamped to
/// `[min, max]` (from `max` on the volume stays at `255·min/max`, the source does not go
/// silent), rolloff 1, pan from the dot product of the direction with the right vector without a
/// front/back distinction; distance 0 gives the center.
pub fn spatialize(listener: &FmodListener, source: [f32; 3], min: f32, max: f32) -> (i32, u8) {
    let d: [f64; 3] =
        std::array::from_fn(|i| f64::from(source[i]) - f64::from(listener.position[i]));
    let stored = d.map(|v| v as f32);
    let sumsq = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f32;
    let dist = f64::from(sumsq).sqrt();
    let (min, max) = (f64::from(min), f64::from(max));
    let mut x = dist;
    // `!(x < max)` in the original: NaN also becomes `max`.
    if x.is_nan() || x >= max {
        x = max;
    }
    if x < min {
        x = min;
    }
    if x < f64::from(1e-5f32) {
        x = f64::from(1e-5f32);
    }
    let vol3d = ((min / x).clamp(0.0, 1.0) * 255.0) as i32;
    let n = if dist > 0.0 {
        let inv = f64::from((1.0 / dist) as f32);
        stored.map(|v| inv * f64::from(v))
    } else {
        [0.0; 3]
    };
    let r = listener.right.map(f64::from);
    let dot = n[2] * r[2] + n[1] * r[1] + n[0] * r[0];
    let pan = (128 - (dot * -128.0) as i32).clamp(0, 255);
    (vol3d, pan as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_pan_and_stereo_pan_match_the_measured_gains() {
        let mono: Vec<_> = [255, 127, 76, 44, 1, 0]
            .iter()
            .map(|&v| channel_gains(255, v, PAN_CENTER))
            .collect();
        assert_eq!(
            mono,
            [(180, 181), (89, 90), (53, 53), (31, 31), (0, 0), (0, 0)]
        );
        let pans: Vec<_> = [0, 64, 128, 192, 255]
            .iter()
            .map(|&p| channel_gains(255, 255, Pan::Position(p)))
            .collect();
        assert_eq!(
            pans,
            [(255, 0), (221, 128), (180, 181), (126, 221), (0, 255)]
        );
        assert_eq!(channel_gains(255, 255, Pan::Stereo), (255, 255));
        assert_eq!(channel_gains(255, 127, Pan::Stereo), (127, 127));
        assert_eq!(channel_gains(255, 76, Pan::Position(64)), (65, 38));
    }

    /// Listener at the origin, forward +z; source at `(d·sin a, 0, d·cos a)`, min/max 4/20, volume
    /// 255: measured gains left/right of the original DLL (72 of 72 cases).
    #[test]
    fn spatial_gains_match_the_original_dll() {
        let angles = [0, 45, 90, 135, 180, 225, 270, 315];
        let table: [(f32, [(i32, i32); 8]); 9] = [
            (0.0, [(180, 181); 8]),
            (
                2.0,
                [
                    (180, 181),
                    (97, 236),
                    (0, 255),
                    (97, 236),
                    (180, 181),
                    (235, 98),
                    (255, 0),
                    (235, 98),
                ],
            ),
            (
                4.0,
                [
                    (180, 181),
                    (97, 236),
                    (0, 255),
                    (97, 236),
                    (180, 181),
                    (235, 98),
                    (255, 0),
                    (235, 98),
                ],
            ),
            (
                6.0,
                [
                    (120, 120),
                    (64, 157),
                    (0, 170),
                    (64, 157),
                    (120, 120),
                    (156, 65),
                    (170, 0),
                    (156, 65),
                ],
            ),
            (
                8.0,
                [
                    (89, 90),
                    (48, 117),
                    (0, 127),
                    (48, 117),
                    (89, 90),
                    (117, 48),
                    (127, 0),
                    (117, 48),
                ],
            ),
            (
                10.0,
                [
                    (72, 72),
                    (38, 94),
                    (0, 102),
                    (38, 94),
                    (72, 72),
                    (94, 39),
                    (102, 0),
                    (94, 39),
                ],
            ),
            (
                20.0,
                [
                    (36, 36),
                    (19, 47),
                    (0, 51),
                    (19, 47),
                    (36, 36),
                    (47, 19),
                    (51, 0),
                    (47, 19),
                ],
            ),
            (
                25.0,
                [
                    (36, 36),
                    (19, 47),
                    (0, 51),
                    (19, 47),
                    (36, 36),
                    (47, 19),
                    (50, 3),
                    (47, 19),
                ],
            ),
            (
                40.0,
                [
                    (36, 36),
                    (19, 47),
                    (0, 51),
                    (19, 47),
                    (36, 36),
                    (47, 19),
                    (51, 0),
                    (47, 19),
                ],
            ),
        ];
        let listener = FmodListener::default();
        for (d, row) in table {
            for (a, expected) in angles.iter().zip(row) {
                let r = f64::from(*a).to_radians();
                let source = [
                    (f64::from(d) * r.sin()) as f32,
                    0.0,
                    (f64::from(d) * r.cos()) as f32,
                ];
                let (vol3d, pan) = spatialize(&listener, source, 4.0, 20.0);
                assert_eq!(
                    channel_gains(vol3d, 255, Pan::Position(pan)),
                    expected,
                    "distance {d}, angle {a}"
                );
            }
        }
    }

    #[test]
    fn spatial_volume_combines_with_the_channel_volume_and_a_tilted_listener() {
        let listener = FmodListener::default();
        for (d, g127, g76) in [
            (2.0f32, (56, 113), (33, 67)),
            (8.0, (28, 56), (16, 33)),
            (20.0, (11, 22), (6, 13)),
        ] {
            let (vol3d, pan) = spatialize(&listener, [0.6 * d, 0.0, 0.8 * d], 4.0, 20.0);
            assert_eq!(pan, 204);
            assert_eq!(channel_gains(vol3d, 127, Pan::Position(pan)), g127);
            assert_eq!(channel_gains(vol3d, 76, Pan::Position(pan)), g76);
        }
        let tilted = FmodListener::new([10.0, 0.0, 10.0], [0.6, 0.3, 0.8], [0.0, 1.0, 0.0]);
        let (vol3d, pan) = spatialize(&tilted, [16.0, 0.0, 10.0], 4.0, 20.0);
        assert_eq!((vol3d, pan), (170, 230));
        assert_eq!(channel_gains(vol3d, 255, Pan::Position(pan)), (53, 161));
    }
}
