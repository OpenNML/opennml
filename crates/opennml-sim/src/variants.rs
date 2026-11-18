//! Module variants (re/property-modules 1.6): `upgrades.ini` yields a bit mask per object
//! (`VariantBitTable_Evaluate` `0x0056F6E0`), which selects the active descriptor of each module via the
//! `PROPID n m...` masks of `property.ini` (`GameObject_SwitchModuleVariants` `0x00467B60`).

use crate::filter::matches;
use crate::id::{ObjectId, PLAYER_SLOTS, SLOT_COUNT};
use crate::module;
use crate::object::MODULE_SLOTS;
use crate::sim::Sim;

/// `GameWorld_GetPlayerVariantMask` (`0x00477130`) with `VariantBitTable_Evaluate`: for each rule all
/// terms (AND, no short circuit), several lines per bit OR. Terms: 0 false, 1 sets the AND result back
/// to true, 2 ... 999 `MY_MM` of the first `targets.ini` entry with this `BUTTONMSG` on the object,
/// values from 1000 up: applied upgrade of the player, negative = not (a negative filter term without an entry
/// leaves the result unchanged).
pub fn player_variant_mask(sim: &mut Sim, id: ObjectId) -> u32 {
    let slot = usize::from(id.slot());
    if slot >= SLOT_COUNT {
        return 0;
    }
    let rules = sim.rules;
    let mut mask = 0u32;
    for rule in &rules.variant_rules {
        let mut ok = true;
        for &term in &rule.terms {
            ok = match term {
                0 => false,
                1 => true,
                2..=999 => {
                    ok && rules
                        .target_entry(term)
                        .is_some_and(|e| matches(sim, id, &e.my, -1))
                }
                1000.. => ok && sim.state.applied_upgrades[slot].contains(&(term as u32)),
                _ => {
                    let w = term.wrapping_neg();
                    if w < 1000 {
                        match rules.target_entry(w) {
                            Some(e) if sim.object(id).is_some() => {
                                ok && !matches(sim, id, &e.my, -1)
                            }
                            _ => ok,
                        }
                    } else {
                        ok && !sim.state.applied_upgrades[slot].contains(&(w as u32))
                    }
                }
            };
        }
        if ok {
            mask |= 1u32 << (rule.bit as u32 & 31);
        }
    }
    mask
}

/// `GameObject_UpdateVariantMask(and, or)` (`0x00469D80`): only players 0-7; afterwards all modules
/// learn about the changed player data.
pub fn update_variant_mask(sim: &mut Sim, id: ObjectId, and: u32, or: u32) {
    if usize::from(id.slot()) >= PLAYER_SLOTS {
        return;
    }
    let mask = (player_variant_mask(sim, id) & and) | or;
    switch_module_variants(sim, id, mask);
    for n in 0..MODULE_SLOTS {
        if sim.module(id, n).is_some() {
            module::on_player_data_changed(sim, id, n);
        }
    }
}

/// `GameObject_SwitchModuleVariants`: for each existing module, if the current variant fits it is
/// switched on; otherwise the first descriptor with a matching bit (`ReadParams`, not `Init`); without
/// a matching descriptor it is switched off. Missing modules are never created.
pub fn switch_module_variants(sim: &mut Sim, id: ObjectId, mask: u32) {
    let rules = sim.rules;
    let Some(guid) = sim.object(id).map(|o| o.guid) else {
        return;
    };
    let ty = rules.types.get(&guid);
    for n in 0..MODULE_SLOTS {
        let Some(bits) = sim.module(id, n).map(|m| m.base.prop_bits) else {
            continue;
        };
        if bits & mask != 0 {
            if let Some(o) = sim.object_mut(id) {
                o.set_module_enabled(n, true);
            }
            continue;
        }
        let desc = ty.and_then(|t| {
            t.modules
                .iter()
                .find(|d| d.index == n as u32 && d.mask & mask != 0)
        });
        match desc {
            Some(d) => {
                module::read_params(sim, id, n, d);
                if let Some(o) = sim.object_mut(id) {
                    if let Some(m) = o.module_mut(n) {
                        m.base.prop_bits = d.mask;
                    }
                    o.set_module_enabled(n, true);
                }
            }
            None => {
                if let Some(o) = sim.object_mut(id) {
                    o.set_module_enabled(n, false);
                }
            }
        }
    }
}

/// `GameObject_RefreshModuleParams` (`0x00467BE0`, only caller `FUN_00479380` of script actions 384/385):
/// for each **enabled** module, find the first descriptor of its index whose mask fits the variant bits
/// of the module (`ObjectType_FindModuleDesc(n, prop_bits)`), and reread its values with `ReadParams`.
/// No change to the variant or to the enabled state (unlike [`switch_module_variants`]). This clears e.g.
/// the extra energy of a healer (C-241).
pub fn refresh_module_params(sim: &mut Sim, id: ObjectId) {
    let rules = sim.rules;
    let Some(guid) = sim.object(id).map(|o| o.guid) else {
        return;
    };
    let Some(ty) = rules.types.get(&guid) else {
        return;
    };
    for n in 0..MODULE_SLOTS {
        let Some(bits) = sim
            .module(id, n)
            .filter(|m| m.base.enabled)
            .map(|m| m.base.prop_bits)
        else {
            continue;
        };
        if let Some(d) = ty
            .modules
            .iter()
            .find(|d| d.index == n as u32 && d.mask & bits != 0)
        {
            module::read_params(sim, id, n, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};
    use opennml_formats::gifcon::parse_variant_rules;
    use opennml_formats::targets::{parse_target_defines, parse_targets};

    const TYPES: &str = "GUID 1100\nNAME Bauer\nVALUEID 100 6 1 0\nPROPID 0 6\nPROPID 0 7\nPROPID 2 6\nPROPID 2 7\nPROPID 39\nVALUEID 400001 6 1 0\nPROPID 16 7\nGUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3 2\nPROPID 3 1\nGUID 3602\nNAME Baum\nVALUEID 100 6 1 256\nPROPID 4 2\n";
    const TARGETS: &str = "BEGIN\nBUTTONMSG 485\nMY_MM\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MATCHBITS MID_PROP_ARMING_ISARMED\nMM_END\nEND\nBEGIN\nBUTTONMSG 486\nMY_MM\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_ARMING_ISARMED\nMM_END\nEND\n";
    const UPGRADES: &str = "BIT 0 = 1\nBIT 1 = 2813\nBIT 2 = -2813\nBIT 6 = 485\nBIT 7 = 2806 486\nBIT 8 = 0 1 2806\nBIT 9 = -999\n";

    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 16);
        let defines = parse_target_defines("MID_PROP_ARMING_ISARMED 295\n");
        r.targets = parse_targets(TARGETS, &defines).entries;
        r.variant_rules = parse_variant_rules(UPGRADES);
        r
    }

    // QUIRK W-16: kept
    #[test]
    fn masks_follow_upgrades_and_filters_and_switch_module_variants() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let farmer =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        // Bit 0 always; 2 (no deletion); 6 (unarmed); 8: "0 1 2806" -> 1 resets, 2806 is missing;
        // 9: a negative filter term without an entry leaves "true" in place.
        assert_eq!(
            player_variant_mask(&mut sim, farmer),
            1 | 1 << 2 | 1 << 6 | 1 << 9
        );
        let o = sim.object(farmer).unwrap();
        // Walk/Stand stay at variant 6; Dying (variant 7 only) is switched off.
        assert_eq!(o.module(0).unwrap().base.prop_bits, 1 << 6);
        assert!(!o.module(16).unwrap().base.enabled);
        // Arm + upgrade 2806 -> variant 7 via ReadParams.
        sim.object_mut(farmer)
            .unwrap()
            .module_mut(39)
            .unwrap()
            .base
            .flags = 0x100;
        sim.state.applied_upgrades[0].push(2806);
        update_variant_mask(&mut sim, farmer, u32::MAX, 0);
        let o = sim.object(farmer).unwrap();
        assert_eq!(o.module(2).unwrap().base.prop_bits, 1 << 7);
        assert!(o.module(16).unwrap().base.enabled);
        // Building: variant 2 without, variant 1 with upgrade 2813.
        let hq = create_object(&mut sim, &mut ObjectDesc::new(1000, 0, [50.0, 0.0, 50.0])).unwrap();
        assert_eq!(sim.module(hq, 3).unwrap().base.prop_bits, 1 << 2);
        sim.state.applied_upgrades[0].push(2813);
        update_variant_mask(&mut sim, hq, u32::MAX, 0);
        assert_eq!(sim.module(hq, 3).unwrap().base.prop_bits, 1 << 1);
        // Slots from 8 on get no variant selection.
        let tree =
            create_object(&mut sim, &mut ObjectDesc::new(3602, 0, [70.0, 0.0, 70.0])).unwrap();
        assert!(sim.module(tree, 4).unwrap().base.enabled);
    }
}
