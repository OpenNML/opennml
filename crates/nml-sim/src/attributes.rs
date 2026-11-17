//! Attribute block of an object (`GameObject+0x334`, `AttributeSet`): 22 values with the IDs 13-34
//! (build time, hit points, extra energy, ...), see re/sim-core, section "Attributes".
//! Integer arithmetic as in the original: `i32` with overflow, division truncates toward 0.

use serde::{Deserialize, Serialize};

use crate::fixed_vec::FixedVec;

/// Smallest and largest attribute ID.
pub const FIRST_ATTRIBUTE: u32 = 13;
pub const LAST_ATTRIBUTE: u32 = 34;
/// Number of attributes.
pub const ATTRIBUTE_COUNT: usize = 22;
/// Fraction "full" (`×100 000`).
pub const FULL: i32 = 100_000;

/// Attributes without a "current value" (`GameObject_InitAttributes` `0x00466D90` via
/// `AttributeSet_SetTracksCurrent` `0x0048F780`): they always return their maximum.
const MAX_ONLY: [u32; 14] = [16, 18, 21, 34, 19, 20, 22, 27, 24, 25, 26, 33, 31, 32];

/// Entry of the modifier list (`AttributeSet_SetModifier` `0x0048FC20`); key 0 =
/// base value, otherwise upgrade GUID, or negative = "always append anew".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifier {
    pub key: i32,
    pub percent: i32,
    pub absolute: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeSet {
    /// Current value as a fraction of the maximum (`+0x08 + 4i`).
    fraction: [i32; ATTRIBUTE_COUNT],
    /// Cached sum `Σabs + base·Σpct/100` (`+0x60 + 4i`).
    bonus: [i32; ATTRIBUTE_COUNT],
    /// "tracks current value" (`+0xB8 + i`).
    tracks: [bool; ATTRIBUTE_COUNT],
    /// Regeneration rate per object tick (`+0xD0 + 4i`).
    regen: [i32; ATTRIBUTE_COUNT],
    /// Modifiers (`+0x128 + 0x1C·i`).
    modifiers: FixedVec<Vec<Modifier>, ATTRIBUTE_COUNT>,
    /// "present" (`+0x390 + i`).
    present: [bool; ATTRIBUTE_COUNT],
}

impl Default for AttributeSet {
    /// `AttributeSet_Construct` (`0x0048F840`): everything 0, "tracks" set everywhere.
    fn default() -> Self {
        Self {
            fraction: [0; ATTRIBUTE_COUNT],
            bonus: [0; ATTRIBUTE_COUNT],
            tracks: [true; ATTRIBUTE_COUNT],
            regen: [0; ATTRIBUTE_COUNT],
            modifiers: FixedVec::default(),
            present: [false; ATTRIBUTE_COUNT],
        }
    }
}

impl AttributeSet {
    /// "Tracks current value" (`AttributeSet+0xB8+i`, setter `AttributeSet_SetTracksCurrent` `0x0048F780`):
    /// attributes without this property always return their maximum and have no bar in the unit bar.
    /// Invalid ID: `false`. No state change.
    pub fn tracks(&self, id: u32) -> bool {
        index(id).is_some_and(|i| self.tracks[i])
    }
}

/// Index of an attribute ID (`Attribute_IdToIndex` `0x0048F520`).
pub fn index(id: u32) -> Option<usize> {
    (FIRST_ATTRIBUTE..=LAST_ATTRIBUTE)
        .contains(&id)
        .then(|| (id - FIRST_ATTRIBUTE) as usize)
}

impl AttributeSet {
    /// Initial assignment like `GameObject_InitAttributes` (`0x00466D90`): base values `base(id)` from the
    /// configuration, extra energy (15) full with rate 1, stamina (28) rate 4, hit points (14)
    /// full; control kind 2 (AI) gets 20 more living space (27) if the type provides any.
    pub fn initial(base: impl Fn(u32) -> i32, ai_owner: bool) -> Self {
        let mut set = Self::default();
        for id in FIRST_ATTRIBUTE..=LAST_ATTRIBUTE {
            set.set_modifier(id, base(id), false, 0);
            set.set_current_value(id, 0, 0);
        }
        for id in MAX_ONLY {
            set.tracks[(id - FIRST_ATTRIBUTE) as usize] = false;
        }
        set.set_regen_rate(15, 1);
        set.set_current_fraction(15, FULL);
        set.set_regen_rate(28, 4);
        set.fill_current(14);
        if ai_owner {
            let living = set.current_value(27, 0);
            if living != 0 {
                set.set_modifier(27, living.wrapping_add(20), false, 0);
            }
        }
        set
    }

    /// Base value = amount of the first modifier with key 0 (`AttributeSet_GetBaseValue`
    /// `0x0048F900`), otherwise 0.
    pub fn base(&self, id: u32) -> i32 {
        index(id)
            .and_then(|i| self.modifiers[i].iter().find(|m| m.key == 0))
            .map_or(0, |m| m.absolute)
    }

    fn recompute_bonus(&mut self, i: usize) {
        let (mut percent, mut absolute) = (0i32, 0i32);
        for m in &self.modifiers[i] {
            if m.percent != 0 {
                percent = percent.wrapping_add(m.percent);
            } else {
                absolute = absolute.wrapping_add(m.absolute);
            }
        }
        let base = self.base(i as u32 + FIRST_ATTRIBUTE);
        self.bonus[i] = absolute.wrapping_add(base.wrapping_mul(percent) / 100);
    }

    /// `AttributeSet_GetMaxValue` (`0x0048F910`): `base · P / 100 + bonus`; `upgrade_percent`
    /// = upgrade percentages of the owner for the type (`GameWorld_GetTypeUpgradePercent`).
    pub fn max_value(&self, id: u32, upgrade_percent: i32) -> i32 {
        let Some(i) = index(id) else { return 0 };
        (self.base(id).wrapping_mul(upgrade_percent) / 100).wrapping_add(self.bonus[i])
    }

    /// `AttributeSet_GetCurrentValue` (`0x0048F980`): `(Max · fraction + 50 000) / 100 000`, not
    /// negative; attributes without a "current value" return their maximum.
    pub fn current_value(&self, id: u32, upgrade_percent: i32) -> i32 {
        let Some(i) = index(id) else { return 0 };
        let max = self.max_value(id, upgrade_percent);
        if !self.tracks[i] {
            return max;
        }
        let v = max.wrapping_mul(self.fraction[i]).wrapping_add(50_000) / FULL;
        v.max(0)
    }

    /// `AttributeSet_SetModifier` (`0x0048FC20`): percentages clamped to ±100; an existing
    /// entry with key `key ≥ 0` is replaced, otherwise appended. Returns `key`.
    pub fn set_modifier(&mut self, id: u32, value: i32, percent: bool, key: i32) -> i32 {
        let Some(i) = index(id) else { return 0 };
        if value > 0 && !percent {
            self.present[i] = true;
        }
        let (pct, abs) = if percent {
            (value.clamp(-100, 100), 0)
        } else {
            (0, value)
        };
        if key >= 0
            && let Some(m) = self.modifiers[i].iter_mut().find(|m| m.key == key)
        {
            m.percent = pct;
            m.absolute = abs;
            self.recompute_bonus(i);
            return key;
        }
        self.modifiers[i].push(Modifier {
            key,
            percent: pct,
            absolute: abs,
        });
        self.recompute_bonus(i);
        key
    }

    /// `AttributeSet_RemoveModifier(id, key)` (`0x0048FD50`): remove the **first** entry with this
    /// key and recompute the bonus; if it is missing, nothing.
    pub fn remove_modifier(&mut self, id: u32, key: i32) {
        let Some(i) = index(id) else { return };
        if let Some(p) = self.modifiers[i].iter().position(|m| m.key == key) {
            self.modifiers[i].remove(p);
            self.recompute_bonus(i);
        }
    }

    /// `AttributeSet_SetCurrentValue` (`0x0048FA10`): fraction = `min(v, Max) · 100 000 / Max`, not
    /// negative; Max 0 gives 0.
    pub fn set_current_value(&mut self, id: u32, value: i32, upgrade_percent: i32) {
        let Some(i) = index(id) else { return };
        let max = self.max_value(id, upgrade_percent);
        self.fraction[i] = if max == 0 {
            0
        } else {
            let c = value.min(max);
            if c < 0 { 0 } else { c.wrapping_mul(FULL) / max }
        };
    }

    /// `AttributeSet_AddToCurrentValue` (`0x0048FAF0`): no lower limit (the fraction can become
    /// negative), capped at Max above.
    pub fn add_to_current_value(&mut self, id: u32, delta: i32, upgrade_percent: i32) {
        let Some(i) = index(id) else { return };
        let max = self.max_value(id, upgrade_percent);
        if max == 0 {
            return;
        }
        let c = self
            .current_value(id, upgrade_percent)
            .wrapping_add(delta)
            .min(max);
        self.fraction[i] = c.wrapping_mul(FULL) / max;
    }

    /// `AttributeSet_AddScaledToCurrent` (`0x0048FB50`): fraction += `v · 100 000 / (Max · s)`
    /// (32-bit product, then 64-bit division, truncated).
    pub fn add_scaled_to_current(&mut self, id: u32, value: i32, scale: i32, upgrade_percent: i32) {
        let product = i64::from(self.max_value(id, upgrade_percent).wrapping_mul(scale));
        if product == 0 {
            return;
        }
        self.add_fraction(id, (i64::from(value) * 100_000 / product) as i32);
    }

    /// `AttributeSet_AddFraction` (`0x0048F5F0`): capped at 100 000 above, open below.
    pub fn add_fraction(&mut self, id: u32, delta: i32) {
        if let Some(i) = index(id) {
            self.fraction[i] = self.fraction[i].wrapping_add(delta).min(FULL);
        }
    }

    /// `AttributeSet_SetCurrentFraction` (`0x0048F580`): clamped to 0-100 000.
    pub fn set_current_fraction(&mut self, id: u32, fraction: i32) {
        if let Some(i) = index(id) {
            self.fraction[i] = fraction.clamp(0, FULL);
        }
    }

    /// `AttributeSet_GetCurrentFraction` (`0x0048F640`): not negative.
    pub fn current_fraction(&self, id: u32) -> i32 {
        index(id).map_or(0, |i| self.fraction[i].max(0))
    }

    /// Fraction unclamped (`AttributeSet+0x08 + 4i`), as it is stored; after damage it can be
    /// negative (the read functions of the original clamp to 0).
    pub fn raw_fraction(&self, id: u32) -> i32 {
        index(id).map_or(0, |i| self.fraction[i])
    }

    /// `AttributeSet_IsFull` (`0x0048F5C0`).
    pub fn is_full(&self, id: u32) -> bool {
        index(id).is_some_and(|i| self.fraction[i] >= FULL)
    }

    /// `AttributeSet_IsPresent` (`0x0048F540`).
    pub fn is_present(&self, id: u32) -> bool {
        index(id).is_some_and(|i| self.present[i] || self.bonus[i] > 0)
    }

    /// Switches the tracking of the current value of the attribute on or off (original
    /// `0x0048F780`).
    pub fn set_tracks_current(&mut self, id: u32, on: bool) {
        if let Some(i) = index(id) {
            self.tracks[i] = on;
        }
    }

    /// `AttributeSet_FillCurrent` (`0x0048F620`).
    pub fn fill_current(&mut self, id: u32) {
        if let Some(i) = index(id) {
            self.fraction[i] = FULL;
        }
    }

    /// `AttributeSet_SetRegenRate` (`0x0048F760`).
    pub fn set_regen_rate(&mut self, id: u32, rate: i32) {
        if let Some(i) = index(id) {
            self.regen[i] = rate;
        }
    }

    /// Regeneration rate of the attribute (`AttributeSet+0xD0 + 4i`, counterpart to
    /// [`set_regen_rate`](Self::set_regen_rate)); 0 for an invalid ID.
    pub fn regen_rate(&self, id: u32) -> i32 {
        index(id).map_or(0, |i| self.regen[i])
    }

    /// `AttributeSet_CopyAttribute(id, source)` (`0x0048FDB0`): take over the share, regeneration rate, flag
    /// "tracks current value" and the cached bonus of an attribute verbatim, clear the
    /// modifier list and append the entries of the source in their order
    /// (without recomputing the bonus). "present" (`+0x390`) stays unchanged.
    pub fn copy_attribute(&mut self, id: u32, from: &AttributeSet) {
        let Some(i) = index(id) else { return };
        self.fraction[i] = from.fraction[i];
        self.regen[i] = from.regen[i];
        self.tracks[i] = from.tracks[i];
        self.bonus[i] = from.bonus[i];
        self.modifiers[i].clone_from(&from.modifiers[i]);
    }

    /// `AttributeSet_TickRegeneration` (`0x0048FBE0`): per object tick `rate` with scale 10, the
    /// tick length is not used.
    pub fn tick_regeneration(&mut self, upgrade_percent: impl Fn(u32) -> i32) {
        for i in 0..ATTRIBUTE_COUNT {
            let rate = self.regen[i];
            if rate != 0 {
                let id = i as u32 + FIRST_ATTRIBUTE;
                self.add_scaled_to_current(id, rate, 10, upgrade_percent(id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(id: u32) -> i32 {
        match id {
            14 => 120,
            15 => 50,
            17 => 20,
            18 => 32,
            27 => 10,
            _ => 0,
        }
    }

    #[test]
    fn initial_values_follow_init_attributes() {
        let a = AttributeSet::initial(base, false);
        // Hit points and extra energy full, load capacity (tracked) empty, sight = Max.
        assert_eq!(a.current_value(14, 0), 120);
        assert_eq!(a.current_value(15, 0), 50);
        assert_eq!(a.current_value(17, 0), 0);
        assert_eq!(a.current_value(18, 0), 32);
        assert!(a.is_full(14) && a.is_present(14) && !a.is_present(13));
        // AI: 20 more living space, only if the type has any.
        assert_eq!(AttributeSet::initial(base, true).current_value(27, 0), 30);
        let no_living = |id| if id == 27 { 0 } else { base(id) };
        assert_eq!(AttributeSet::initial(no_living, true).max_value(27, 0), 0);
    }

    #[test]
    fn modifiers_and_upgrade_percent_change_the_maximum() {
        let mut a = AttributeSet::initial(base, false);
        // Max = 120*25/100 + 120 + 0 = 150; fraction stays full.
        assert_eq!(a.max_value(14, 25), 150);
        assert_eq!(a.current_value(14, 25), 150);
        a.set_modifier(14, 30, true, 2840);
        a.set_modifier(14, 7, false, 4400);
        // Bonus = 120 + 7 + 120*30/100 = 163; replacing via the key, percent clamped to 100.
        assert_eq!(a.max_value(14, 0), 163);
        a.set_modifier(14, 500, true, 2840);
        assert_eq!(a.max_value(14, 0), 247);
        // A negative key always appends anew.
        a.set_modifier(14, 1, false, -1);
        a.set_modifier(14, 1, false, -1);
        assert_eq!(a.max_value(14, 0), 249);
    }

    #[test]
    fn current_values_round_and_have_no_lower_bound() {
        let mut a = AttributeSet::initial(base, false);
        a.add_to_current_value(14, -200, 0);
        // -80/120 -> fraction -66 666, read as 0; the fraction stays negative.
        assert_eq!(a.current_value(14, 0), 0);
        assert_eq!(a.current_fraction(14), 0);
        a.add_fraction(14, 200_000);
        assert_eq!(a.current_value(14, 0), 120);
        a.set_current_value(14, 61, 0);
        // 61*100000/120 = 50833 -> (120*50833 + 50000)/100000 = 61.
        assert_eq!(a.current_value(14, 0), 61);
        a.set_current_value(14, -5, 0);
        assert_eq!(a.current_fraction(14), 0);
    }

    #[test]
    fn copy_attribute_takes_fraction_rate_flag_bonus_and_modifiers_but_not_presence() {
        let mut from = AttributeSet::initial(|id| if id == 26 { 5000 } else { 0 }, false);
        from.set_tracks_current(26, true);
        from.set_modifier(26, 10, true, 77);
        from.set_regen_rate(26, 3);
        from.set_current_fraction(26, 40_000);
        let mut to = AttributeSet::initial(|id| if id == 26 { 9 } else { 0 }, false);
        to.copy_attribute(26, &from);
        // Max = 5000 + 5000*10/100 = 5500, 40 % of it; list in the order of the source.
        assert_eq!(to.current_value(26, 0), 2200);
        assert_eq!(to.base(26), 5000);
        assert_eq!(to.modifiers[13], from.modifiers[13]);
        assert_eq!((to.regen[13], to.tracks[13]), (3, true));
        // The bonus is taken over verbatim, not recomputed from the list.
        from.bonus[13] = 1;
        to.copy_attribute(26, &from);
        assert_eq!(to.max_value(26, 0), 1);
        // "present" stays with the target (set here by the base value 9).
        let mut empty = AttributeSet::default();
        empty.copy_attribute(26, &AttributeSet::initial(|_| 0, false));
        assert!(!empty.present[13]);
        let mut present = AttributeSet::initial(|id| if id == 26 { 9 } else { 0 }, false);
        present.copy_attribute(26, &AttributeSet::default());
        assert!(present.present[13] && present.max_value(26, 0) == 0);
        // Other attributes stay.
        assert_eq!(to.current_value(24, 0), 0);
    }

    #[test]
    fn remove_modifier_removes_the_first_entry_with_the_key_and_recomputes() {
        let mut a = AttributeSet::initial(|id| if id == 19 { 26 } else { 0 }, false);
        a.set_modifier(19, 5, true, 4400);
        assert_eq!(a.max_value(19, 0), 27);
        a.set_modifier(19, -3, false, -1);
        a.set_modifier(19, -3, false, -1);
        a.remove_modifier(19, 4400);
        assert_eq!(a.max_value(19, 0), 20, "26 - 3 - 3");
        a.remove_modifier(19, -1);
        assert_eq!(a.max_value(19, 0), 23, "only the first entry with -1 goes");
        a.remove_modifier(19, 4711);
        a.remove_modifier(99, 4400);
        assert_eq!(a.max_value(19, 0), 23, "missing key or attribute: nothing");
    }

    #[test]
    fn regeneration_ignores_the_tick_length_and_truncates() {
        let mut a = AttributeSet::initial(base, false);
        a.set_current_fraction(15, 0);
        // Rate 1, scale 10, Max 50: +200 fraction per tick; at Max 20 000 the increment becomes 0.
        a.tick_regeneration(|_| 0);
        assert_eq!(a.current_fraction(15), 200);
        let mut big = AttributeSet::initial(|id| if id == 15 { 20_000 } else { 0 }, false);
        big.set_current_fraction(15, 0);
        big.tick_regeneration(|_| 0);
        assert_eq!(big.current_fraction(15), 0);
    }

    #[test]
    fn tracks_reports_the_tracked_byte() {
        let mut a = AttributeSet::initial(base, false);
        // Without a current value: the maximum always applies (`MAX_ONLY`), so no bar in the unit bar.
        for id in [16, 18, 19, 20, 21, 22, 24, 25, 26, 27, 31, 32, 33, 34] {
            assert!(!a.tracks(id), "{id}");
        }
        // With a current value: hit points, extra energy, load capacity, stamina ...
        for id in [13, 14, 15, 17, 23, 28, 29, 30] {
            assert!(a.tracks(id), "{id}");
        }
        assert!(a.tracks(14));
        // A module switches the tracking on again (food/wood/gold on resource objects: 24-26).
        a.set_tracks_current(24, true);
        assert!(a.tracks(24));
        a.set_tracks_current(14, false);
        assert!(!a.tracks(14));
        // Invalid IDs: false, no panic.
        for id in [0, 12, 35, u32::MAX] {
            assert!(!a.tracks(id), "{id}");
        }
    }
}
