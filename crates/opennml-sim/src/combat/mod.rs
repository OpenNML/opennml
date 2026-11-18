//! Combat (re/combat): match state for combat (`GameWorld+0x222C`,
//! `+0x2250`, `+0x2294`, combat points `world+4`), damage, experience and ranks.

use serde::{Deserialize, Serialize};

pub mod damage;
#[cfg(test)]
mod flows;
pub mod hit_zone;
pub mod projectile;
pub mod rank;
#[cfg(test)]
pub(crate) mod testkit;

/// Slots with factors and behavior mode (0-8).
pub const PLACES: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatState {
    /// `m_AttackFactor[9]` (`+0x222C`): damage dealt in percent.
    pub attack_factor: [i32; PLACES],
    /// `m_HitFactor[9]` (`+0x2250`): damage taken in percent.
    pub hit_factor: [i32; PLACES],
    /// Player behavior mode 0...4 (`+0x2294`, `GameWorld_Init`: 2 "DEFENSIVE"; re/combat-attack 1).
    pub behavior_mode: [i32; PLACES],
    /// Combat points per target owner in the current double tick (re/combat-host 5).
    pub attack_points: [i32; 8],
}

impl Default for CombatState {
    fn default() -> Self {
        Self {
            attack_factor: [100; PLACES],
            hit_factor: [100; PLACES],
            behavior_mode: [2; PLACES],
            attack_points: [0; 8],
        }
    }
}

impl CombatState {
    /// Getter `0x00478590`: outside 0...8 -> 100.
    pub fn attack_factor(&self, slot: u8) -> i32 {
        self.attack_factor
            .get(usize::from(slot))
            .copied()
            .unwrap_or(100)
    }

    /// Getter `0x00478570`.
    pub fn hit_factor(&self, slot: u8) -> i32 {
        self.hit_factor
            .get(usize::from(slot))
            .copied()
            .unwrap_or(100)
    }

    /// `FUN_00478B00`: `0 ≤ slot < 9 ? +0x2294[slot] : 0`.
    pub fn behavior_mode(&self, slot: u8) -> i32 {
        self.behavior_mode
            .get(usize::from(slot))
            .copied()
            .unwrap_or(0)
    }
}

/// Attack kind `0x811` (`PropertyAttack_HandleMessage` `0x005021C0`, re/combat-damage 1.3): first
/// matching rule; `Module_HasFlags` checks without the low byte.
pub fn attack_art(flags: u32, status: u32) -> u32 {
    use opennml_formats::bonus::*;
    let has = |m: u32| flags & m & 0xFFFF_FF00 != 0;
    if status & crate::object::status::ISMOUNTED != 0 {
        ART_MOUNTY_SWORD
    } else if has(0x1_0000) {
        ART_PEEK_MOUNTY
    } else if has(0x4000) {
        ART_SWORD_PEEK
    } else if has(0x8000) {
        ART_MOUNTY_SWORD
    } else if has(0x2_0000) {
        ART_CANNON_BUILDING
    } else if has(0x4_0000) {
        ART_FIRE_BUILDING
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combat_state_defaults_and_bounds() {
        let c = CombatState::default();
        assert_eq!(c.attack_factor, [100; 9]);
        assert_eq!(c.hit_factor, [100; 9]);
        assert_eq!(
            c.behavior_mode, [2; 9],
            "GameWorld_Init sets all 9 entries to 2"
        );
        assert_eq!(c.attack_points, [0; 8]);
        assert_eq!(
            (c.attack_factor(9), c.hit_factor(200), c.behavior_mode(9)),
            (100, 100, 0)
        );
    }
}
