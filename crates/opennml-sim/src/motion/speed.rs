//! Speeds of a unit (`SpeedInfo`, VTable `0x006318F0`, re/motion 3): all
//! values in fixed point per object tick (100 ms), turns in degrees*4096.

use serde::{Deserialize, Serialize};

use crate::fx::{fixed_div, fixed_mul};

/// "Turn radius invalid" (`0x006731C8` = -4096).
const NO_RADIUS: i32 = -4096;
/// Largest turn per tick (`0x006731CC` = 180 degrees).
pub const MAX_TURN: i32 = 180 << 12;
/// `2π` in fixed point (`0x006731A0` = `Fixed_FromFloat(0x40C90FDB)`).
const TWO_PI: i32 = 25735;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeedInfo {
    /// `+0x04 m_MinSpeed`: lower limit; 0 = may turn in place.
    pub min: i32,
    /// `+0x08` base speed (>= 10; reference for 2x and 1.5x). `Serialize` labels it
    /// `m_CruisingSpeed` (swapped).
    pub default: i32,
    /// `+0x0C` cruising speed (target of `ResetToCruising`).
    pub cruising: i32,
    /// `+0x10 m_TurnSpeed`: turn per tick while driving, `[1, 180°]`.
    pub turn: i32,
    /// `+0x14 m_TurnRadiusFwd`: turn radius, calculated lazily ([`NO_RADIUS`] = invalid).
    pub radius: i32,
    /// `+0x18 m_SpinSpeed` = `2 · turn`: turn per tick in place.
    pub spin: i32,
    /// `+0x1C m_CurSpeed`: current step size per object tick.
    pub cur: i32,
    /// `+0x20 m_IsRunning`.
    pub running: bool,
    /// `+0x24` spin threshold (90 degrees, only `0x2F00` changes it).
    pub spin_threshold: i32,
    /// `+0x28` instant turn.
    pub instant_turn: bool,
}

impl Default for SpeedInfo {
    /// `SpeedInfo_Construct` (`0x00553910`).
    fn default() -> Self {
        Self {
            min: 0,
            default: 10,
            cruising: 10,
            turn: 1,
            radius: NO_RADIUS,
            spin: 2,
            cur: 0,
            running: false,
            spin_threshold: 90 << 12,
            instant_turn: false,
        }
    }
}

impl SpeedInfo {
    /// `SpeedInfo_SetDefaultSpeed` (`0x00553960`): at least 10; also sets the
    /// cruising speed, `cur` stays (quirk).
    pub fn set_default_speed(&mut self, v: i32) -> bool {
        self.default = if v >= 10 { v } else { 10 };
        self.cruising = self.default;
        v >= 10
    }

    /// `SpeedInfo_SetTurnSpeed` (`0x005539A0`).
    pub fn set_turn_speed(&mut self, v: i32) -> bool {
        if v == self.turn {
            return true;
        }
        if v < 1 {
            self.turn = 1;
            self.radius = NO_RADIUS;
            self.spin = 2;
            return false;
        }
        self.turn = v.min(MAX_TURN);
        self.radius = NO_RADIUS;
        self.spin = self.turn.wrapping_shl(1);
        true
    }

    /// `SpeedInfo_GetTurnRadius` (`0x00553A00`): `R = cur · 360 / (2π · turn)`, cached.
    pub fn turn_radius(&mut self) -> i32 {
        if self.radius == NO_RADIUS {
            self.radius = fixed_div(self.cur.wrapping_mul(360), fixed_mul(TWO_PI, self.turn));
        }
        self.radius
    }

    /// `SpeedInfo_SetCurSpeed` (`0x00553AA0`): only in `[min, 2 · default]`.
    pub fn set_cur_speed(&mut self, v: i32) -> bool {
        if v == self.cur {
            return true;
        }
        if v < self.min || v > self.default.wrapping_mul(2) {
            return false;
        }
        self.cur = v;
        self.radius = NO_RADIUS;
        self.running = v > self.default;
        true
    }

    /// `SpeedInfo_SetCruisingSpeed` (`0x00553AE0`).
    pub fn set_cruising_speed(&mut self, v: i32) -> bool {
        if self.min <= v && v <= self.default.wrapping_mul(2) {
            self.cruising = v;
            true
        } else {
            false
        }
    }

    /// `SpeedInfo_StartSprint` (`0x00553B00`): `true`, even if setting fails.
    pub fn start_sprint(&mut self) -> bool {
        if self.running {
            return false;
        }
        self.set_cur_speed(self.default.wrapping_mul(2));
        self.running = true;
        true
    }

    /// `SpeedInfo_RaiseToOneAndHalf` (`0x00553B20`): `3 · default / 2` (direction 0).
    pub fn raise_to_one_and_half(&mut self) -> bool {
        let t = self.default.wrapping_mul(3) / 2;
        if t > self.cur {
            let r = self.set_cur_speed(t);
            self.running = false;
            r
        } else {
            false
        }
    }

    /// `SpeedInfo_ResetToCruising` (`0x00553B50`): only lower or start from 0.
    pub fn reset_to_cruising(&mut self) -> bool {
        if self.cur != 0 && self.cruising >= self.cur {
            return false;
        }
        self.set_cur_speed(self.cruising);
        self.running = false;
        true
    }

    /// `SpeedInfo_EnsureMoving` (`0x00553B70`).
    pub fn ensure_moving(&mut self) -> bool {
        if self.cur > 0 {
            false
        } else {
            self.reset_to_cruising()
        }
    }

    /// Values of a walk unit (`PropertyWalk_InitBase` `0x00532850`, re/motion 5): turn from
    /// VALUEID 10006 (`Fixed_Div(v << 12, 12800)`), `MinSpeed` from 10007 (!= 0 -> 0, otherwise 10),
    /// base speed from attribute 16 (`Fixed_Div(v, 1280)`), instant turn with hard radius
    /// <= 512, `MinSpeed` 0 and spin >= 80 degrees.
    pub fn for_unit(
        turn_param: i32,
        min_param: i32,
        speed_attribute: i32,
        hard_radius: i32,
    ) -> Self {
        let mut s = Self::default();
        s.apply_unit(turn_param, min_param, speed_attribute, hard_radius);
        s
    }

    /// Like [`SpeedInfo::for_unit`] on existing values (repeated `ReadParams`): `cur` stays.
    pub fn apply_unit(
        &mut self,
        turn_param: i32,
        min_param: i32,
        speed_attribute: i32,
        hard_radius: i32,
    ) {
        self.set_turn_speed(fixed_div(turn_param.wrapping_shl(12), TIME_UNIT * 10));
        self.min = if min_param != 0 { 0 } else { 10 };
        self.set_default_speed(fixed_div(speed_attribute, TIME_UNIT));
        self.instant_turn = hard_radius <= 0x200 && self.min == 0 && self.spin >= INSTANT_TURN_MIN;
    }
}

/// `0x500` (`0x00672E4C`): 128 world units per tile * 10 object ticks per second.
pub const TIME_UNIT: i32 = 0x500;
/// Minimum spin for the instant turn (`0x00672DD0` = `Fixed_Div(250 << 12, 12800)` = 80 degrees).
const INSTANT_TURN_MIN: i32 = 327_680;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_speeds_follow_the_property_values() {
        // Human (10006 = 200, 10007 = 1, "medium" 183, radius 512), cart, ship (re/motion 5.1).
        let mut man = SpeedInfo::for_unit(200, 1, 183, 512);
        man.reset_to_cruising();
        assert_eq!(
            (man.turn, man.spin, man.min, man.default),
            (262_144, 524_288, 0, 584)
        );
        assert!(man.instant_turn && man.cur == 584);
        assert_eq!(man.turn_radius(), 520);
        let mut cart = SpeedInfo::for_unit(100, 0, 183, 1900);
        cart.reset_to_cruising();
        assert_eq!(
            (cart.turn, cart.min, cart.cur, cart.turn_radius()),
            (131_072, 10, 584, 1044)
        );
        assert!(!cart.instant_turn);
        let mut ship = SpeedInfo::for_unit(7, 1, 128, 2100);
        ship.reset_to_cruising();
        assert_eq!(
            (ship.turn, ship.cur, ship.turn_radius()),
            (9172, 408, 10436)
        );
        // Without a speed value: base speed 10 (SPA_Silva).
        assert_eq!(SpeedInfo::for_unit(200, 1, 0, 512).default, 10);
        // Overflow from 10006 >= 512 as in the original (shipped data <= 350).
        assert!(SpeedInfo::for_unit(512, 1, 183, 512).turn < 1 << 12);
    }

    #[test]
    fn setters_clamp_and_keep_the_old_value_out_of_range() {
        let mut s = SpeedInfo::default();
        s.set_default_speed(100);
        assert!(!s.set_cur_speed(201) && s.cur == 0);
        assert!(s.set_cur_speed(200) && s.running);
        assert!(s.reset_to_cruising() && s.cur == 100 && !s.running);
        // Lowering only: cruising speed >= cur -> nothing.
        assert!(!s.reset_to_cruising());
        assert!(s.raise_to_one_and_half() && s.cur == 150 && !s.running);
        assert!(s.start_sprint() && s.cur == 200 && s.running);
        assert!(!s.start_sprint());
        s.min = 10;
        assert!(!s.set_cruising_speed(5) && s.cruising == 100);
        assert!(!s.set_turn_speed(0) && s.turn == 1 && s.spin == 2);
        assert!(s.set_turn_speed(200 << 12) && s.turn == 180 << 12);
        // Default below 10 -> 10, cur stays.
        assert!(!s.set_default_speed(3) && s.default == 10 && s.cur == 200);
        let mut e = SpeedInfo::default();
        assert!(e.ensure_moving() && e.cur == 10);
        assert!(!e.ensure_moving());
    }
}
