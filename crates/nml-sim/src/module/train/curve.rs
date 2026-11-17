//! Drive curve of the train (re/train-multipart 3.3, 9.1): uniformly accelerated movement over
//! time indices in ms, integer like `Train_TravelDistance` (`0x0052F0B0`), `Train_TimeForSpeed`
//! (`0x0052F140`) and `Train_BrakeDistance` (`0x0052F190`).

use super::MAX_ACC_TIME;

/// `Train_TravelDistance(a, b, k, d)` (`0x0052F0B0`): distance between the times `a` and `b` (ms,
/// each clamped to `0...2400`) as fixed point: `Fixed_FromInt(b^2*k/1000 - a^2*k/1000) / (d*1000)`, all
/// divisions 32-bit toward 0.
pub fn travel_distance(a: i32, b: i32, k: i32, d: i32) -> i32 {
    let a = a.clamp(0, MAX_ACC_TIME);
    let b = b.clamp(0, MAX_ACC_TIME);
    let diff = (b.wrapping_mul(b).wrapping_mul(k) / 1000)
        .wrapping_sub(a.wrapping_mul(a).wrapping_mul(k) / 1000);
    diff.wrapping_shl(12) / d.wrapping_mul(1000)
}

/// `Train_TimeForSpeed(v)` (`0x0052F140`): smallest `t = 100*i < 2400` with
/// `v < travel_distance(t, t + 100, 2, 1)`, otherwise 2400.
pub fn time_for_speed(v: i32) -> i32 {
    let mut t = 0;
    while t < MAX_ACC_TIME {
        if v < travel_distance(t, t + 100, 2, 1) {
            return t;
        }
        t += 100;
    }
    MAX_ACC_TIME
}

/// `Train_BrakeDistance(v)` (`0x0052F190`, called by the drive step with `k = 2, d = 1`):
/// `travel_distance(0, time_for_speed(v) + 100, 2, 1)`.
pub fn brake_distance(v: i32) -> i32 {
    travel_distance(0, time_for_speed(v) + 100, 2, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceleration_follows_the_24_step_curve() {
        // Acceleration step (0x0052F3F0) from A = 2400 to A < 1 (re/train-multipart 9.1).
        let expected = [
            0, 51, 153, 256, 358, 460, 563, 665, 768, 870, 972, 1075, 1177, 1280, 1382, 1484, 1587,
            1689, 1792, 1894, 1996, 2099, 2201, 2304,
        ];
        let mut a = MAX_ACC_TIME;
        let mut speeds = Vec::new();
        while a >= 1 {
            speeds.push(travel_distance(2300 - a, 2400 - a, 5, 4));
            a -= 100;
        }
        assert_eq!(speeds, expected);
        assert_eq!(speeds.iter().sum::<i32>(), 27_076);
    }

    #[test]
    fn braking_from_cruise_speed() {
        assert_eq!(time_for_speed(2304), 1400);
        assert_eq!(brake_distance(2304), 18_432);
        assert_eq!(time_for_speed(0), 0);
        // Start of braking (0x0052FEE0): A = 1400, v = Dist(1300, 1400).
        let mut a = time_for_speed(2304) / 100 * 100;
        assert_eq!(travel_distance(a - 100, a, 2, 1), 2211);
        // Braking ticks (0x0052F960): v = Dist(A - 100, A), A -= 100, until v < 20.
        let mut speeds = Vec::new();
        loop {
            let v = travel_distance(a - 100, a, 2, 1);
            a -= 100;
            if v < 20 {
                break;
            }
            speeds.push(v);
        }
        assert_eq!(
            speeds,
            [
                2211, 2048, 1884, 1720, 1556, 1392, 1228, 1064, 901, 737, 573, 409, 245, 81
            ]
        );
        // Every halt over the braking curve leaves A = -100.
        assert_eq!(a, -100);
    }
}
