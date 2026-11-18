//! `bonus.ini` (`GameWorld_LoadBonusIni` `0x0048A230`, re/combat-damage 1.6):
//! attack bonuses per attack kind and the rider bonus `MOUNTY_BONUS`.

use std::collections::BTreeMap;

use crate::config::{game_lines, left, mid, scan_int};

pub const BONUS_PATH: &str = "data/gifcon/bonus.ini";

/// Attack kinds (`0x811`, re/combat-damage 1.3) = keys of the bonus table.
pub const ART_SWORD_PEEK: u32 = 0x1000_0000;
pub const ART_PEEK_MOUNTY: u32 = 0x2000_0000;
pub const ART_MOUNTY_SWORD: u32 = 0x3000_0000;
pub const ART_CANNON_BUILDING: u32 = 0x4000_0000;
pub const ART_FIRE_BUILDING: u32 = 0x5000_0000;
pub const ART_MELEE_ARTILLERY: u32 = 0x6000_0000;
pub const ART_MELEE_MELEEARTILLERY: u32 = 0x7000_0000;
pub const ART_UNIT_BUILDING: u32 = 0x8000_0000;

/// Keys in check order (`0x0061C654...0x0061C744`).
const KEYS: [(&str, u32); 8] = [
    ("ATTACKBONUS_SWORD_PEEK", ART_SWORD_PEEK),
    ("ATTACKBONUS_PEEK_MOUNTY", ART_PEEK_MOUNTY),
    ("ATTACKBONUS_MOUNTY_SWORD", ART_MOUNTY_SWORD),
    ("ATTACKBONUS_CANNON_BUILDING", ART_CANNON_BUILDING),
    ("ATTACKBONUS_FIRE_BUILDING", ART_FIRE_BUILDING),
    ("ATTACKBONUS_MELEE_ARTILLERY", ART_MELEE_ARTILLERY),
    ("ATTACKBONUS_MELEE_MELEEARTILLERY", ART_MELEE_MELEEARTILLERY),
    ("ATTACKBONUS_UNIT_BUILDING", ART_UNIT_BUILDING),
];

/// Contents of `bonus.ini` (`GameWorld+0x1F88` and `+0x1FA4`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BonusTable {
    /// Attack kind -> percent.
    pub attack: BTreeMap<u32, i32>,
    /// `MOUNTY_BONUS`: attribute -> percent (sorted by attribute, re/combat-damage 1.6).
    pub mounty: BTreeMap<i32, i32>,
}

impl BonusTable {
    /// `GameWorld_GetAttackBonus` (`0x0047DE60`): missing key -> 0.
    pub fn attack_bonus(&self, art: u32) -> i32 {
        self.attack.get(&art).copied().unwrap_or(0)
    }
}

/// Each line trimmed on both sides; empty or `#` at the start -> skipped; prefix comparison
/// `Left(len(key)) == key`, value `atol(Mid(len(key) + 1))`; later lines override.
/// `MOUNTY_BONUS` (`Left(12)`): rest from `Mid(13)` left-trimmed, split at the first space
/// (if there is none, the line is discarded), `attr = atol(links)`, `pct = atol(rest)`.
pub fn parse_bonus_ini(text: &str) -> BonusTable {
    let mut t = BonusTable::default();
    for line in game_lines(text) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(&(key, art)) = KEYS.iter().find(|(k, _)| left(line, k.len()) == *k) {
            t.attack
                .insert(art, scan_int(mid(line, key.len() + 1)).unwrap_or(0));
        } else if left(line, 12) == "MOUNTY_BONUS" {
            let rest = mid(line, 13).trim_start();
            if let Some((a, p)) = rest.split_once(' ') {
                t.mounty
                    .insert(scan_int(a).unwrap_or(0), scan_int(p).unwrap_or(0));
            }
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bonus_ini_reads_attack_bonuses_and_the_mounty_bonus() {
        let text = "# Comment\r\n\r\nATTACKBONUS_SWORD_PEEK 40\r\n  ATTACKBONUS_PEEK_MOUNTY 60  \r\n\
            ATTACKBONUS_MOUNTY_SWORD 50\r\nATTACKBONUS_CANNON_BUILDING 150\r\nATTACKBONUS_FIRE_BUILDING 100\r\n\
            ATTACKBONUS_MELEE_ARTILLERY 50\r\nATTACKBONUS_MELEE_MELEEARTILLERY 25\r\n\
            ATTACKBONUS_UNIT_BUILDING -20\r\nMOUNTY_BONUS 18 20 \r\nMOUNTY_BONUS 19\r\n\
            ATTACKBONUS_SWORD_PEEK 41\r\n#ATTACKBONUS_FIRE_BUILDING 7\r\nUNKNOWN 3\r\n";
        let b = parse_bonus_ini(text);
        let want = [
            (ART_SWORD_PEEK, 41), // later line overrides
            (ART_PEEK_MOUNTY, 60),
            (ART_MOUNTY_SWORD, 50),
            (ART_CANNON_BUILDING, 150),
            (ART_FIRE_BUILDING, 100),
            (ART_MELEE_ARTILLERY, 50),
            (ART_MELEE_MELEEARTILLERY, 25),
            (ART_UNIT_BUILDING, -20),
        ];
        assert_eq!(b.attack.len(), 8);
        for (art, v) in want {
            assert_eq!(b.attack_bonus(art), v, "{art:#x}");
        }
        assert_eq!(b.attack_bonus(0x9000_0000), 0, "missing key -> 0");
        // "MOUNTY_BONUS 19" without a space after the attribute is discarded.
        assert_eq!(b.mounty, [(18, 20)].into_iter().collect());
    }
}
