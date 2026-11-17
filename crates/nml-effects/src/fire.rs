//! Fire at the hit zones of an object (`GameObject_UpdateDamageFires` `0x0046E590`,
//! `GameObject_SetHitZoneFire` `0x0046E3D0`, re/combat-host 4.4): the calculation that the
//! model browser and the map share. The browser specifies `d` from a burn intensity, the map from
//! the hit points of the building.

/// Effect of the hit zones.
pub const FIRE: &str = "data/effects/feuer.pef";
/// Action of the sound database that every call of `SetHitZoneFire` with `s > 0` plays.
pub const FIRE_SOUND_ACTION: i32 = 102;
/// Flags of this sound (`SoundManager_PlayObjectSound3D(102, 0x11, ...)`).
pub const FIRE_SOUND_FLAGS: u32 = 0x11;

/// Scaling of the fire per zone for the damage value `d` (`UpdateDamageFires`): `d` to 0...1000,
/// zone `i` with `d_i = d - i*floor(800/n)`, `s = (d_i < 1) ? 0 : min(0.01 * d_i, 3)`.
pub fn zone_scales(d: i32, zones: usize) -> Vec<f32> {
    if zones == 0 {
        return Vec::new();
    }
    let mut d = d.clamp(0, 1000);
    let step = 800 / zones as i32;
    (0..zones)
        .map(|_| {
            let s = if d < 1 {
                0.0
            } else {
                (d as f32 * 0.01).min(3.0)
            };
            d -= step;
            s
        })
        .collect()
}

/// What `SetHitZoneFire` does with the fire of a zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoneFire {
    /// No fire and `s = 0`: nothing.
    Idle,
    /// New fire with scaling 1; `s` does not apply until the next call.
    Start,
    /// Scale the existing fire to `s`.
    Scale(f32),
    /// `s = 0` with an existing fire: `Stop` (the ember disappears, smoke particles die out).
    Stop,
}

/// Step of a zone: `burning` = there is already a fire, `s` the scaling of this call.
pub fn zone_step(burning: bool, s: f32) -> ZoneFire {
    match (burning, s > 0.0) {
        (false, false) => ZoneFire::Idle,
        (false, true) => ZoneFire::Start,
        (true, true) => ZoneFire::Scale(s),
        (true, false) => ZoneFire::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_scales_follow_the_formula_of_the_original() {
        // n = 4: step 200. d = 500 -> 500, 300, 100, -100: 3, 3, 1, 0.
        assert_eq!(zone_scales(500, 4), [3.0, 3.0, 1.0, 0.0]);
        // d = 100 -> only zone 0 with 1.
        assert_eq!(zone_scales(100, 4), [1.0, 0.0, 0.0, 0.0]);
        // d is limited to 0...1000: 1000 -> 1000, 800, 600, 400 (all >= 300 -> 3).
        assert_eq!(zone_scales(5000, 4), [3.0; 4]);
        assert_eq!(zone_scales(-50, 4), [0.0; 4]);
        // n = 3: step 266; d = 250 -> 2.5, then -16 -> 0.
        assert_eq!(zone_scales(250, 3), [2.5, 0.0, 0.0]);
        assert!(zone_scales(10, 0).is_empty());
    }

    #[test]
    fn a_zone_starts_scales_and_stops() {
        assert_eq!(zone_step(false, 0.0), ZoneFire::Idle);
        assert_eq!(zone_step(false, 2.0), ZoneFire::Start);
        assert_eq!(zone_step(true, 2.0), ZoneFire::Scale(2.0));
        assert_eq!(zone_step(true, 0.0), ZoneFire::Stop);
    }
}
