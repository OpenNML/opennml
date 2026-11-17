//! Module 40 `CPropertySelfHealing` (re/garrison-and-commands): heals hit points at
//! a fixed rate per minute. No messages, no timers; which types have it is decided by
//! the variant system (`PROPID 40 1` buildings with the upgrade "Prevents burning down", `PROPID 40 11`
//! WID/PRI units, heroes without a mask) - without a matching variant the module is off and not in
//! the tick list.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::filter::matches_class;
use crate::id::ObjectId;
use crate::object::status;
use crate::sim::Sim;

use super::ModuleKind;

pub const INDEX: usize = 40;
/// `VALUEID` of the healing rate (hit points per minute).
pub const RATE_VALUE: u32 = 410_001;
/// Hit points (attribute `0xE`).
const HITPOINTS: u32 = 14;
/// Object ticks per minute: scale of `AttributeSet_AddScaledToCurrent`.
const TICKS_PER_MINUTE: i32 = 600;
/// Class "finished building" (`CONSTRUCTED_FINISHED` of the building module).
const FINISHED_BUILDING: u32 = 0x403;

/// State (`0x74` bytes, `PropertySelfHealing_Construct` `0x005246B0`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHealing {
    /// `m_HealingSpeed` (`+0x70`): hit points per minute (600 object ticks).
    pub healing_speed: i32,
}

/// `PropertySelfHealing_ReadParams` (`0x005246E0`, slot 4): `m_HealingSpeed = 410001` (missing -> 0).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let rate = desc.value_int(RATE_VALUE).unwrap_or(0) as i32;
    if let Some(m) = sim.module_mut(id, INDEX)
        && let ModuleKind::SelfHealing(s) = &mut m.kind
    {
        s.healing_speed = rate;
    }
}

/// `Module_InitViaReadParams` (`0x00521920`, slot 9): only `ReadParams`; the module flags stay 0.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    read_params(sim, id, desc);
}

/// `PropertySelfHealing_Tick` (`0x00524700`, slot 13; `dt` unread): dead (`0x40000`) -> nothing;
/// a building only when finished (`MatchesClass(0x403)`, so not during construction either, where the
/// fraction rises via the construction step); otherwise `AddScaledToCurrent(14, rate, 600)` - product `Max * 600` as
/// `i32` with overflow, 64-bit division with truncation, capped at full on top, **without
/// lower bound** (a negative fraction after damage heals up from below). No further
/// conditions: quartered units keep healing, combat and movement do not count.
pub fn tick(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    if o.status & status::DIED != 0 {
        return;
    }
    if o.has_module(super::building::INDEX, false) && !matches_class(sim, id, FINISHED_BUILDING) {
        return;
    }
    let rate = match sim.module(id, INDEX).map(|m| &m.kind) {
        Some(ModuleKind::SelfHealing(s)) => s.healing_speed,
        _ => return,
    };
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, HITPOINTS);
    if let Some(o) = sim.object_mut(id) {
        o.attributes
            .add_scaled_to_current(HITPOINTS, rate, TICKS_PER_MINUTE, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// Healing types: headquarters and house (buildings, rate 50), WID warrior (rate 100),
    /// hero (rate 350), warrior with max 20,000 (never heals).
    const TYPES: &str = "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nPROPID 40\nVALUEID 410001 6 1 50\nGUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nPROPID 40\nVALUEID 410001 6 1 50\nGUID 2900\nNAME WID_Krieger\nVALUEID 100 6 1 0\nPROPID 40\nVALUEID 410001 6 1 100\nGUID 2950\nNAME Held\nVALUEID 100 6 1 0\nPROPID 40\nVALUEID 410001 6 1 350\nGUID 2951\nNAME Riese\nVALUEID 100 6 1 0\nPROPID 40\nVALUEID 410001 6 1 50\nGUID 2952\nNAME Gebaeude_mit_Upgrade\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nPROPID 40 1\nVALUEID 410001 6 1 50\n";

    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        let hp = |v: i32| [(14, v)].into_iter().collect();
        r.defaults.insert(1000, hp(4000));
        r.defaults.insert(1001, hp(900));
        r.defaults.insert(2900, hp(160));
        r.defaults.insert(2950, hp(1000));
        r.defaults.insert(2951, hp(20_000));
        r.defaults.insert(2952, hp(900));
        // Bit 0 always (like `Rules::synthetic`), bit 1 with upgrade 2813 ("Prevents burning down").
        r.variant_rules = nml_formats::gifcon::parse_variant_rules("BIT 0 = 1\nBIT 1 = 2813\n");
        r
    }

    fn create(sim: &mut Sim, guid: u32, x: f32, ready: bool) -> ObjectId {
        let mut desc = ObjectDesc::new(guid, 0, [x, 0.0, 1000.0]);
        if ready {
            desc.flags |= desc_flags::PLACED;
        }
        create_object(sim, &mut desc).unwrap()
    }

    fn fraction(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().attributes.raw_fraction(HITPOINTS)
    }

    fn set_fraction(sim: &mut Sim, id: ObjectId, f: i32) {
        let a = &mut sim.object_mut(id).unwrap().attributes;
        a.set_current_fraction(HITPOINTS, 0);
        a.add_fraction(HITPOINTS, f);
    }

    /// Gain of the fraction in one tick from `from`.
    fn delta(sim: &mut Sim, id: ObjectId, from: i32) -> i32 {
        set_fraction(sim, id, from);
        tick(sim, id);
        fraction(sim, id) - from
    }

    // QUIRK W-135: kept
    #[test]
    fn healing_follows_the_rate_per_minute_table() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = create(&mut sim, 1000, 1000.0, true);
        let house = create(&mut sim, 1001, 2500.0, true);
        let warrior = create(&mut sim, 2900, 3000.0, true);
        let hero = create(&mut sim, 2950, 3200.0, true);
        let giant = create(&mut sim, 2951, 3400.0, true);
        assert_eq!(
            sim.module(hq, INDEX).map(|m| m.kind.clone()),
            Some(ModuleKind::SelfHealing(SelfHealing { healing_speed: 50 }))
        );
        // Delta per tick: HQ 50*100000/(4000*600) = 2, house 9, warrior 104, hero 58.
        assert_eq!(delta(&mut sim, hq, 50_000), 2);
        assert_eq!(delta(&mut sim, house, 50_000), 9);
        assert_eq!(delta(&mut sim, warrior, 50_000), 104);
        assert_eq!(delta(&mut sim, hero, 50_000), 58);
        // Max*600 > rate*100000 -> gain 0, never heals.
        assert_eq!(delta(&mut sim, giant, 50_000), 0);
        // Capped at full on top.
        assert_eq!(delta(&mut sim, warrior, 99_950), 50);
        // HQ from 50 % after 10 ticks: display 2001.
        set_fraction(&mut sim, hq, 50_000);
        for _ in 0..10 {
            tick(&mut sim, hq);
        }
        assert_eq!(sim.attribute(hq, HITPOINTS), 2001);
        // No lower bound: fraction -500 after two ticks of the warrior is -292.
        set_fraction(&mut sim, warrior, -500);
        tick(&mut sim, warrior);
        tick(&mut sim, warrior);
        assert_eq!(fraction(&sim, warrior), -292);
    }

    // QUIRK W-135: kept
    #[test]
    fn construction_sites_and_dead_objects_do_not_heal() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Construction site: 1 of 4000 HP (fraction 25), not finished.
        let site = create(&mut sim, 1000, 1000.0, false);
        assert_eq!(fraction(&sim, site), 25);
        tick(&mut sim, site);
        assert_eq!(fraction(&sim, site), 25);
        // Dead objects (buildings and units) do not heal.
        let warrior = create(&mut sim, 2900, 3000.0, true);
        let hq = create(&mut sim, 1000, 2000.0, true);
        for id in [warrior, hq] {
            set_fraction(&mut sim, id, 50_000);
            sim.object_mut(id)
                .unwrap()
                .set_status_flags(status::DIED, true);
            tick(&mut sim, id);
            assert_eq!(fraction(&sim, id), 50_000, "{id:?}");
        }
        // Quartered units keep healing.
        set_fraction(&mut sim, warrior, 50_000);
        let o = sim.object_mut(warrior).unwrap();
        o.status = status::QUARTERED;
        tick(&mut sim, warrior);
        assert_eq!(fraction(&sim, warrior), 50_104);
    }

    #[test]
    fn the_variant_switches_the_module_on_and_the_object_tick_runs_it() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // `PROPID 40 1`: off without upgrade 2813 (no longer in the list after the first tick).
        let id = create(&mut sim, 2952, 1000.0, true);
        assert!(!sim.module(id, INDEX).unwrap().base.enabled);
        crate::variants::update_variant_mask(&mut sim, id, u32::MAX, 0);
        sim.object_mut(id)
            .unwrap()
            .apply_module_activation_changes();
        assert!(!sim.object(id).unwrap().tick_list.contains(&(INDEX as u8)));
        set_fraction(&mut sim, id, 50_000);
        crate::tick::object_tick(&mut sim, id, 100);
        assert_eq!(fraction(&sim, id), 50_000);
        // With the upgrade on: the object tick heals (900 HP, rate 50 -> 9 per tick).
        sim.state.applied_upgrades[0].push(2813);
        crate::variants::update_variant_mask(&mut sim, id, u32::MAX, 0);
        sim.object_mut(id)
            .unwrap()
            .apply_module_activation_changes();
        assert!(sim.module(id, INDEX).unwrap().base.enabled);
        crate::tick::object_tick(&mut sim, id, 100);
        assert_eq!(fraction(&sim, id), 50_009);
    }
}
