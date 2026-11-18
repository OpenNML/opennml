//! Module 30 `CPropertyGlobalUpgrade` (vtable `0x00624128`, `0x74` bytes, re/upgrades 1.4, 3.7): the
//! upgrade `m_UpgradeGUID` (`+0x70`, 310001) that init credits to the slot via the world queue and that
//! the owner change takes away from the old slot again. Shipped data: only `SPA_Missionar`
//! (1113) with 2840. Otherwise the module answers like an idle module (ReadParams and vf+0x20 empty).
//! The destructor and destroying without dying remove nothing; death removes via the
//! owner change to slot 8 (W-219).

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::sim::Sim;

use super::{ModuleKind, init_flags};

/// Module index of GlobalUpgrade.
pub const INDEX: usize = 0x1E;
/// `VALUEID` of the upgrade (`0x4BAF1`).
pub const VALUE_GUID: u32 = 310001;

/// `CPropertyGlobalUpgrade`; the savegame holds `m_UpgradeGUID` (`0x005164F0`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalUpgrade {
    /// `m_UpgradeGUID` (`+0x70`, 310001).
    pub upgrade_guid: u32,
}

/// `PropertyGlobalUpgrade_Init` (`0x005163E0`, re/upgrades 3.7): `m_UpgradeGUID = GetInt(310001)`,
/// then "Apply" at slot `slot` (`min(slot, 11)`) into the world queue (`FUN_0047E310`):
/// credited not until the next phase 0 step, for every missionary again (duplicates, K-69).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, slot: u8) {
    let flags = init_flags(INDEX, desc);
    let guid = desc.value_int(VALUE_GUID).unwrap_or(0) as u32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        if let ModuleKind::GlobalUpgrade(g) = &mut m.kind {
            g.upgrade_guid = guid;
        }
    }
    crate::upgrades::queue_upgrade(sim.state, i32::from(slot), guid, true);
}

/// Owner change (vf+0x14 `0x00516410`, re/upgrades 3.7): "Remove" at the **old** slot
/// (`FUN_0047E350`); the new owner gets nothing (W-219).
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId, old: ObjectId) {
    let Some(guid) = sim.module(id, INDEX).and_then(|m| match &m.kind {
        ModuleKind::GlobalUpgrade(g) => Some(g.upgrade_guid),
        _ => None,
    }) else {
        return;
    };
    crate::upgrades::queue_upgrade(sim.state, i32::from(old.slot()), guid, false);
}

#[cfg(test)]
mod tests {
    use crate::message::Message;
    use crate::messaging::send;
    use crate::rules::Rules;
    use crate::sim::Sim;
    use crate::state::SimState;
    use crate::upgrades::testkit::{MISSIONARY, rules, spawn, state};

    fn count(state: &SimState, slot: usize, guid: u32) -> i32 {
        state.guid_counts[slot].get(&guid).copied().unwrap_or(0)
    }

    /// Steps up to and including the next phase 0 step.
    fn next_phase_0(state: &mut SimState, rules: &Rules) {
        loop {
            crate::tick::step(state, rules, &[]);
            if state.phase == 0 {
                return;
            }
        }
    }

    // QUIRK W-219: kept
    #[test]
    fn a_missionary_credits_2840_in_the_next_phase_0_step() {
        let rules = rules();
        let mut state = state(&rules);
        let first = spawn(&mut Sim::new(&mut state, &rules), MISSIONARY, 0, (10, 10));
        // Init only enqueues (`FUN_0047E310`): neither counter nor list changes immediately.
        assert_eq!(count(&state, 0, 2840), 0);
        assert!(state.applied_upgrades[0].is_empty());
        assert_eq!(state.upgrade_queue.len(), 1);
        next_phase_0(&mut state, &rules);
        assert_eq!(count(&state, 0, 2840), 1);
        assert_eq!(state.applied_upgrades[0].last(), Some(&2840));
        assert!(state.upgrade_queue.is_empty());
        // Second missionary: credited again (duplicate, K-69).
        let second = spawn(&mut Sim::new(&mut state, &rules), MISSIONARY, 0, (14, 10));
        next_phase_0(&mut state, &rules);
        assert_eq!(count(&state, 0, 2840), 2);
        assert_eq!(state.applied_upgrades[0], vec![2840, 2840]);
        // Destroying without dying: the destructor `0x005163D0` takes nothing away (W-219).
        crate::world::destroy_object(&mut Sim::new(&mut state, &rules), first);
        next_phase_0(&mut state, &rules);
        assert!(state.object(first, false).is_none());
        assert_eq!(count(&state, 0, 2840), 2);
        assert_eq!(state.applied_upgrades[0], vec![2840, 2840]);
        assert!(state.upgrade_queue.is_empty());
        // Handing the second over to slot 1: "Remove" at the old slot, enqueued at the
        // owner change at the end of the phase 0 step, effective in the next one; slot 1 gets nothing.
        crate::world::queue_owner_change(&mut Sim::new(&mut state, &rules), second, 1);
        next_phase_0(&mut state, &rules);
        assert_eq!(state.upgrade_queue.len(), 1);
        assert_eq!(count(&state, 0, 2840), 2);
        next_phase_0(&mut state, &rules);
        assert_eq!(count(&state, 0, 2840), 1);
        assert_eq!(state.applied_upgrades[0], vec![2840]);
        assert_eq!(count(&state, 1, 2840), 0);
        assert!(state.applied_upgrades[1].is_empty());
        assert!(state.upgrade_queue.is_empty());
        // Death: dying hands the missionary to slot 8 (`QueueOwnerChange(hid, 8)`, C-142), and
        // `GameObject_OnOwnerChanged` (`0x0046F850`) calls vf+0x14 of all 60 modules without a
        // check: the old slot loses 2840 one phase 0 step after the owner change
        // (Ghidra check; corrects re/upgrades 3.7/9).
        let third = spawn(&mut Sim::new(&mut state, &rules), MISSIONARY, 0, (18, 10));
        next_phase_0(&mut state, &rules);
        assert_eq!(count(&state, 0, 2840), 2);
        send(
            &mut Sim::new(&mut state, &rules),
            third,
            &Message::new(0x110, &[]),
        );
        next_phase_0(&mut state, &rules);
        assert_eq!(count(&state, 0, 2840), 2, "owner change not until the end");
        assert_eq!(state.upgrade_queue.len(), 1);
        next_phase_0(&mut state, &rules);
        assert_eq!(count(&state, 0, 2840), 1);
        assert_eq!(state.applied_upgrades[0], vec![2840]);
        assert!(state.applied_upgrades[8].is_empty(), "slot 8 gets nothing");
        assert!(state.upgrade_queue.is_empty());
    }
}
