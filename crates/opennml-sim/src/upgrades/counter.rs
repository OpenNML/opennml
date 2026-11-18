//! Counter-upgrades (GEG 3370-3377, command `0x2F`; re/temp-upgrades 1.5, 3.5, 3.6, re/upgrades 3.6): the
//! world list `m_CounterUpgrades` (`GameWorld+0x990`, [`SimState::counter_upgrades`]) in insertion order,
//! the start (`GameWorld_CmdStartCounterUpgrade` `0x004848F0`), the expiry in the phase 0 step with the
//! revocation for all players, and the expiry on the death of the source.
//!
//! The effect runs through incremental player upgrades of the enemies
//! ([`super::apply_upgrade`]`(..., true)`: counter, type percentages, `m_ShowCounterUpgrade`); 3374 and 3377
//! only read the counter (production, throwing off).

use super::{CounterUpgrade, apply_upgrade, revoke_upgrade_all_players};
use crate::economy::{add_object_costs_percent, can_afford_object};
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};

/// Deduction per phase 0 step (100 ms, `0x004127F0`).
pub const STEP_MS: i32 = 100;
/// Duration of a counter-upgrade with config 13 under one second (`0x00484987`, W-229).
pub const SHORT_DURATION_MS: i32 = 10_000;

/// `FUN_00412620` (re/temp-upgrades 3.5): whether the GUID runs anywhere - for **all** players (W-226).
pub fn is_running(state: &SimState, guid: u32) -> bool {
    state.counter_upgrades.iter().any(|c| c.guid == guid)
}

/// `FUN_00479690(p)`: entries whose stored source HID carries the slot `slot`
/// (`(hid >> 11) & 0x1F`). After a change of owner of the source the entry keeps counting for the
/// old slot (W-230).
pub fn count_of_slot(state: &SimState, slot: u8) -> usize {
    state
        .counter_upgrades
        .iter()
        .filter(|c| c.source.slot() == slot)
        .count()
}

/// `GameWorld_CmdStartCounterUpgrade(guid, source)` (`0x004848F0`, re/temp-upgrades 3.5) without the
/// command check of the reimplementation ([`crate::command::upgrade::check_counter_upgrade`]): if the GUID
/// runs anywhere (W-226), the source is missing (`GetObjectById(..., 0)`: a dying one counts as present)
/// or the slot of the source already has an entry, nothing. Otherwise the cost of the GUID must be
/// affordable at the slot of the source (`Player_CanAffordObject`, with a message at the tile of the
/// source); then -100% of the cost, duration config 13 * 1000 ms (under 1000 -> 10,000, W-229), entry at the
/// back (`0x00412870`), message to the host and `Player_ApplyUpgrade(e, guid, incremental)` for every
/// enemy `e` of the slot (`GameWorld_HasEnemyRelation`, slots 0-7). Return value: started.
pub fn start(sim: &mut Sim, guid: u32, source: ObjectId) -> bool {
    if is_running(sim.state, guid) {
        return false;
    }
    let slot = source.slot();
    let Some(tile) = sim.object(source).map(|o| o.tile) else {
        return false;
    };
    if count_of_slot(sim.state, slot) >= 1 {
        return false;
    }
    let player = i32::from(slot);
    if !can_afford_object(sim, player, guid, false, tile) {
        return false;
    }
    add_object_costs_percent(sim, player, guid, -100);
    let mut rest_ms = sim.rules.config_value(slot, guid, 13).wrapping_mul(1000);
    if rest_ms < 1000 {
        rest_ms = SHORT_DURATION_MS;
    }
    sim.state.counter_upgrades.push(CounterUpgrade {
        guid,
        rest_ms,
        source,
    });
    sim.state.events.push(SimEvent::CounterUpgradeStarted {
        guid,
        player: slot,
        tile,
    });
    for enemy in 0..PLAYER_SLOTS as i32 {
        if sim.state.diplomacy.is_enemy(player, enemy) {
            apply_upgrade(sim, enemy, guid, true);
        }
    }
    true
}

/// Phase 0 tick of the list (`0x004127F0`, from `GameWorld_Tick` after the extra list, before the
/// projectiles; re/temp-upgrades 3.6) in list order: an entry with `rest - 100 < 1` is - only the **first**
/// such one - removed and revoked for all players
/// ([`revoke_upgrade_all_players`], W-228); further expired ones stay unchanged until the
/// next step (W-227); all others lose 100 ms.
pub fn tick(sim: &mut Sim) {
    let list = &mut sim.state.counter_upgrades;
    let mut ended = None;
    let mut i = 0;
    while i < list.len() {
        if list[i].rest_ms.wrapping_sub(STEP_MS) < 1 {
            if ended.is_none() {
                ended = Some(list.remove(i).guid);
                continue;
            }
        } else {
            list[i].rest_ms = list[i].rest_ms.wrapping_sub(STEP_MS);
        }
        i += 1;
    }
    if let Some(guid) = ended {
        revoke_upgrade_all_players(sim, guid);
        sim.state
            .events
            .push(SimEvent::CounterUpgradeEnded { guid });
    }
}

/// `FUN_00412650(hid)` from `PropertyDying_StartDying` (`0x0050EE30`): all entries with this
/// source get rest 0 and expire in the next phase 0 step (W-218). A source that changed owner before
/// carries a new ID and no longer hits an entry (W-230).
pub fn expire_by_source(state: &mut SimState, source: ObjectId) {
    for c in state
        .counter_upgrades
        .iter_mut()
        .filter(|c| c.source == source)
    {
        c.rest_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::run;
    use crate::command::upgrade::check_counter_upgrade;
    use crate::command::validate::{button_locked, menu_button};
    use crate::command::{Command, CommandKind};
    use crate::message::Message;
    use crate::messaging::send;
    use crate::module::building::{self, msg as bmsg};
    use crate::rules::Rules;
    use crate::save::checksum;
    use crate::script::world::view::lock_button;
    use crate::tick::step;
    use crate::upgrades::testkit::*;

    /// Chastity belt (production half as fast), weakness (HP -40%), termites (HP -60%,
    /// not in menu 601 of the test world), prerequisite "research" (re/temp-upgrades 1.5).
    const CHASTITY: u32 = 3374;
    const WEAKNESS: u32 = 3375;
    const TERMITES: u32 = 3376;
    const RESEARCH: u32 = 3354;
    /// Dying (`PropertyDying_StartDying`, module 16).
    const DIE: u32 = 0x110;

    fn count(state: &SimState, slot: usize, guid: u32) -> i32 {
        state.guid_counts[slot].get(&guid).copied().unwrap_or(0)
    }

    /// Missionary of the slot with researched 3354 (buttons 3374/3375 free) and 1000 of every
    /// resource.
    fn missionary(sim: &mut Sim, slot: u8, tile: (i32, i32)) -> ObjectId {
        sim.state.add_guid_count(slot, RESEARCH, 1);
        for index in 0..3 {
            sim.state.players[usize::from(slot)].set_resource(index, 1000);
        }
        spawn(sim, MISSIONARY, i32::from(slot), tile)
    }

    fn counter(guid: u32, source: ObjectId) -> CommandKind {
        CommandKind::CounterUpgrade { guid, source }
    }

    /// List as `(guid, rest_ms, source)`.
    fn entries(state: &SimState) -> Vec<(u32, i32, ObjectId)> {
        state
            .counter_upgrades
            .iter()
            .map(|c| (c.guid, c.rest_ms, c.source))
            .collect()
    }

    fn ended(events: &[SimEvent]) -> Vec<u32> {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::CounterUpgradeEnded { guid } => Some(*guid),
                _ => None,
            })
            .collect()
    }

    /// Engine ticks up to and including the next phase 0 step (it runs in the `step` before the one with
    /// `phase == 1`); returns the events of all these ticks.
    fn phase_0_pass(state: &mut SimState, rules: &Rules) -> Vec<SimEvent> {
        let mut events = Vec::new();
        loop {
            let pass = state.phase == 1;
            events.extend(step(state, rules, &[]));
            if pass {
                return events;
            }
        }
    }

    /// HP (attribute 14) as `(max, current)` with the type percentage of the owner.
    fn hitpoints(sim: &Sim, id: ObjectId) -> (i32, i32) {
        let o = sim.object(id).unwrap();
        let p = sim.type_upgrade_percent(o.owner(), o.guid, 14);
        (
            o.attributes.max_value(14, p),
            o.attributes.current_value(14, p),
        )
    }

    fn show(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().show_counter_upgrade
    }

    // QUIRK W-19: kept
    #[test]
    fn chastity_belt_halves_the_enemy_production() {
        // re/temp-upgrades 9 V8: missionary (slot 0, 3354 researched, 150/150/150) starts 3374; player 1 enemy:
        // cost deducted, list {3374, 120000, missionary}, counter 1 at slot 1; colonist training (15 s) at
        // slot 1 takes 300 building ticks (-50 instead of -100, W-19).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m = missionary(&mut sim, 0, (10, 10));
        for index in 0..3 {
            sim.state.players[0].set_resource(index, 150);
        }
        let hq = spawn(&mut sim, HQ, 1, (30, 30));
        sim.state.events.clear();
        run(&mut sim, 0, counter(CHASTITY, m));
        for index in 0..3 {
            assert_eq!(sim.state.resource(0, index), 0, "resource {index}");
        }
        assert_eq!(entries(sim.state), [(CHASTITY, 120_000, m)]);
        assert!(is_running(sim.state, CHASTITY));
        assert_eq!(count_of_slot(sim.state, 0), 1);
        assert_eq!(count(sim.state, 1, CHASTITY), 1);
        assert_eq!(count(sim.state, 0, CHASTITY), 0, "only for enemies");
        assert!(sim.state.events.iter().any(|e| matches!(
            e,
            SimEvent::CounterUpgradeStarted {
                guid: CHASTITY,
                player: 0,
                tile: (10, 10)
            }
        )));
        // Training at slot 1: start in the first building tick, then 300 ticks of 50 ms each.
        let tick = |sim: &mut Sim| crate::module::tick(sim, hq, building::INDEX, 100);
        let made = |sim: &Sim| building::building(sim, hq).map(|b| (b.make_guid, b.make_time));
        assert_eq!(
            send(
                &mut sim,
                hq,
                &Message::new(bmsg::ENQUEUE, &[COLONIST as i32])
            ),
            1
        );
        tick(&mut sim);
        assert_eq!(made(&sim), Some((COLONIST, 15_000)));
        for _ in 1..300 {
            tick(&mut sim);
        }
        assert_eq!(made(&sim), Some((COLONIST, 50)));
        tick(&mut sim);
        assert_eq!(made(&sim), Some((0, 0)), "done after 300 ticks");
    }

    #[test]
    fn a_counter_upgrade_ends_in_the_1200th_phase_0_step() {
        // Command in step n (before the world tick): deduction in every phase 0 pass from this step on;
        // in the 1200th pass end, counter 0, CounterUpgradeEnded.
        let rules = rules();
        let mut state = state(&rules);
        let m = missionary(&mut Sim::new(&mut state, &rules), 0, (10, 10));
        if state.phase != 1 {
            step(&mut state, &rules, &[]);
        }
        assert_eq!(state.phase, 1, "the next step has a phase 0 pass");
        let command = Command {
            player: 0,
            kind: counter(CHASTITY, m),
        };
        let events = step(&mut state, &rules, &[command]);
        assert!(ended(&events).is_empty());
        assert_eq!(
            entries(&state),
            [(CHASTITY, 119_900, m)],
            "deduction in the command step"
        );
        let mut passes = 1;
        loop {
            let events = phase_0_pass(&mut state, &rules);
            passes += 1;
            if !ended(&events).is_empty() {
                assert_eq!(ended(&events), [CHASTITY]);
                break;
            }
            assert_eq!(count(&state, 1, CHASTITY), 1);
            assert!(passes < 1300, "no end");
        }
        assert_eq!(passes, 1200);
        assert!(state.counter_upgrades.is_empty());
        assert_eq!(count(&state, 1, CHASTITY), 0);
    }

    // QUIRK W-217, W-226: kept
    #[test]
    fn one_counter_upgrade_per_guid_and_per_slot() {
        // re/temp-upgrades 9 V9: second 0x2F {3375} from slot 0 -> rejected; slot 2 with {3374} -> rejected
        // (global), accepted with {3375} (W-226).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m0 = missionary(&mut sim, 0, (10, 10));
        let m2 = missionary(&mut sim, 2, (20, 10));
        run(&mut sim, 0, counter(CHASTITY, m0));
        let before = checksum(sim.state);
        run(&mut sim, 0, counter(WEAKNESS, m0));
        assert_eq!(checksum(sim.state), before, "one per slot");
        run(&mut sim, 2, counter(CHASTITY, m2));
        assert_eq!(checksum(sim.state), before, "GUID once globally");
        // The same locks in the start itself (original, behind the command check).
        assert!(!start(&mut sim, WEAKNESS, m0));
        assert!(!start(&mut sim, CHASTITY, m2));
        assert_eq!(checksum(sim.state), before);
        run(&mut sim, 2, counter(WEAKNESS, m2));
        assert_eq!(
            entries(sim.state),
            [(CHASTITY, 120_000, m0), (WEAKNESS, 60_000, m2)]
        );
        assert_eq!(
            (count_of_slot(sim.state, 0), count_of_slot(sim.state, 2)),
            (1, 1)
        );
        assert_eq!(count_of_slot(sim.state, 1), 0);
    }

    // QUIRK W-218: kept
    #[test]
    fn a_dying_source_ends_its_counter_upgrade_in_the_next_phase_0_step() {
        // re/temp-upgrades 9 V14, W-218: the source dies at rest 50,000 -> rest 0 (`ExpireBySource`), expiry in
        // the next phase 0 step; the state in between loads.
        let rules = rules();
        let mut state = state(&rules);
        let m = missionary(&mut Sim::new(&mut state, &rules), 0, (10, 10));
        run(&mut Sim::new(&mut state, &rules), 0, counter(CHASTITY, m));
        for _ in 0..700 {
            assert!(ended(&phase_0_pass(&mut state, &rules)).is_empty());
        }
        assert_eq!(entries(&state), [(CHASTITY, 50_000, m)]);
        send(
            &mut Sim::new(&mut state, &rules),
            m,
            &Message::new(DIE, &[]),
        );
        assert_eq!(entries(&state), [(CHASTITY, 0, m)]);
        assert_eq!(count(&state, 1, CHASTITY), 1, "runs until the phase 0 step");
        let bytes = crate::save::testkit::save_bytes(&state, &rules);
        crate::save::SaveGame::from_bytes(&bytes)
            .unwrap()
            .check(&rules)
            .unwrap();
        let events = phase_0_pass(&mut state, &rules);
        assert_eq!(ended(&events), [CHASTITY]);
        assert!(state.counter_upgrades.is_empty());
        assert_eq!(count(&state, 1, CHASTITY), 0);
    }

    // QUIRK W-217, W-227: kept
    #[test]
    fn two_expiries_in_one_step_end_one_after_the_other() {
        // re/temp-upgrades 9 V15, W-227: two sources die in the same tick; the first in list order ends in the
        // next step, the second one step later with unchanged rest; a third, non-expired entry keeps
        // counting in both steps.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m0 = missionary(&mut sim, 0, (10, 10));
        let m2 = missionary(&mut sim, 2, (20, 10));
        let m1 = missionary(&mut sim, 1, (30, 10));
        run(&mut sim, 0, counter(CHASTITY, m0));
        run(&mut sim, 2, counter(WEAKNESS, m2));
        assert!(start(&mut sim, TERMITES, m1));
        for m in [m0, m2] {
            send(&mut sim, m, &Message::new(DIE, &[]));
        }
        assert_eq!(
            entries(sim.state),
            [(CHASTITY, 0, m0), (WEAKNESS, 0, m2), (TERMITES, 60_000, m1)]
        );
        let events = phase_0_pass(&mut state, &rules);
        assert_eq!(ended(&events), [CHASTITY]);
        assert_eq!(entries(&state), [(WEAKNESS, 0, m2), (TERMITES, 59_900, m1)]);
        assert_eq!(count(&state, 1, CHASTITY), 0);
        assert_eq!(count(&state, 1, WEAKNESS), 1);
        let events = phase_0_pass(&mut state, &rules);
        assert_eq!(ended(&events), [WEAKNESS]);
        assert_eq!(entries(&state), [(TERMITES, 59_800, m1)]);
        assert_eq!(count(&state, 1, WEAKNESS), 0);
    }

    // QUIRK W-230: kept
    #[test]
    fn an_owner_change_of_the_source_decouples_the_counter_upgrade() {
        // W-230: the source changes owner (new HID); its death ends nothing, and
        // `count_of_slot` keeps counting at the old slot.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m = missionary(&mut sim, 0, (10, 10));
        run(&mut sim, 0, counter(CHASTITY, m));
        let new = crate::world::change_owner(&mut sim, m, 2).expect("change of owner");
        assert_ne!(new, m);
        assert_eq!(
            (count_of_slot(sim.state, 0), count_of_slot(sim.state, 2)),
            (1, 0)
        );
        send(&mut sim, new, &Message::new(DIE, &[]));
        assert_ne!(
            sim.object(new).unwrap().status & crate::object::status::DIED,
            0
        );
        assert_eq!(entries(sim.state), [(CHASTITY, 120_000, m)]);
        // A second missionary of slot 0 stays locked.
        let other = missionary(&mut sim, 0, (12, 10));
        assert!(check_counter_upgrade(&sim, 0, WEAKNESS, other).is_err());
        let events = phase_0_pass(&mut state, &rules);
        assert!(ended(&events).is_empty());
        assert_eq!(entries(&state), [(CHASTITY, 119_900, m)]);
    }

    // QUIRK W-217, W-228: kept
    #[test]
    fn revoking_ignores_diplomacy_changes_during_the_run() {
        // W-228: during the run slot 1 becomes a friend; the revocation lowers anyway. Slot 2 was
        // not an enemy at the start (counter 0) and becomes one afterwards: its counter stays 0.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.diplomacy.set_enemy(0, 2, false);
        let m = missionary(&mut sim, 0, (10, 10));
        run(&mut sim, 0, counter(WEAKNESS, m));
        let counts = |sim: &Sim| {
            (0..8)
                .map(|p| count(sim.state, p, WEAKNESS))
                .collect::<Vec<_>>()
        };
        assert_eq!(counts(&sim), [0, 1, 0, 1, 1, 1, 1, 1]);
        sim.state.diplomacy.set_enemy(0, 1, false);
        sim.state.diplomacy.set_friend(0, 1, true);
        sim.state.diplomacy.set_enemy(0, 2, true);
        for _ in 0..599 {
            tick(&mut sim);
        }
        assert_eq!(counts(&sim), [0, 1, 0, 1, 1, 1, 1, 1]);
        sim.state.events.clear();
        tick(&mut sim);
        assert_eq!(ended(&sim.state.events), [WEAKNESS]);
        assert_eq!(counts(&sim), [0; 8]);
        assert_eq!(sim.type_upgrade_percent(1, COLONIST, 14), 0);
    }

    // QUIRK W-228: kept
    #[test]
    fn weakness_shrinks_hitpoints_and_marks_the_start_objects() {
        // re/temp-upgrades 9 V11: 3375 against slot 1, colonist full: max 96, current 96, display 1; a colonist
        // created afterwards has the percentage but display 0; after 60 s 160/160, both display 0.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 1, (30, 30));
        let own = spawn(&mut sim, COLONIST, 0, (12, 12));
        assert_eq!(hitpoints(&sim, c), (160, 160));
        let m = missionary(&mut sim, 0, (10, 10));
        run(&mut sim, 0, counter(WEAKNESS, m));
        assert_eq!(hitpoints(&sim, c), (96, 96));
        assert_eq!(show(&sim, c), 1);
        assert_eq!((hitpoints(&sim, own), show(&sim, own)), ((160, 160), 0));
        let late = spawn(&mut sim, COLONIST, 1, (32, 30));
        assert_eq!((hitpoints(&sim, late), show(&sim, late)), ((96, 96), 0));
        for _ in 0..599 {
            tick(&mut sim);
        }
        assert_eq!(hitpoints(&sim, c), (96, 96));
        tick(&mut sim);
        assert!(sim.state.counter_upgrades.is_empty());
        for id in [c, late] {
            assert_eq!((hitpoints(&sim, id), show(&sim, id)), ((160, 160), 0));
        }
    }

    // QUIRK W-226: kept
    // QUIRK W-231: deviation
    #[test]
    fn counter_upgrade_commands_check_source_button_and_running() {
        // A8 (deviation from W-231): foreign source, dead source, button not in the menu, 3354 missing
        // (locked), locked by level data, GUID running, sender has one -> each a rejection with a
        // message, checksum equal. `button_locked` locks the buttons like the UI.
        let mut rules = rules();
        rules
            .level
            .slot_configs
            .entry(0)
            .or_default()
            .insert(WEAKNESS, [(1, 0)].into_iter().collect());
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m0 = spawn(&mut sim, MISSIONARY, 0, (10, 10));
        let m1 = missionary(&mut sim, 1, (40, 10));
        let dead = spawn(&mut sim, MISSIONARY, 0, (12, 10));
        send(&mut sim, dead, &Message::new(DIE, &[]));
        let colonist = spawn(&mut sim, COLONIST, 0, (14, 10));
        let reject = |sim: &mut Sim, player, guid, source, why: &str| {
            let err = check_counter_upgrade(sim, player, guid, source).unwrap_err();
            assert!(err.contains(why), "{guid}: {err}");
            let before = checksum(sim.state);
            run(sim, player, counter(guid, source));
            assert_eq!(checksum(sim.state), before, "{guid}: {err}");
        };
        reject(&mut sim, 0, CHASTITY, m1, "not an own");
        reject(&mut sim, 0, CHASTITY, dead, "not an own");
        reject(&mut sim, 0, CHASTITY, colonist, "not in the menu");
        reject(&mut sim, 0, 2800, m0, "not in the menu");
        reject(&mut sim, 0, CHASTITY, m0, "prerequisite");
        sim.state.add_guid_count(0, RESEARCH, 1);
        check_counter_upgrade(&sim, 0, CHASTITY, m0).unwrap();
        // Script lock 207: global, the check "like the command bar" sees it too.
        lock_button(&mut sim, CHASTITY, true);
        reject(&mut sim, 0, CHASTITY, m0, "locked");
        lock_button(&mut sim, CHASTITY, false);
        check_counter_upgrade(&sim, 0, CHASTITY, m0).unwrap();
        reject(&mut sim, 0, WEAKNESS, m0, "locked");
        // GUID running (slot 1 starts 3374); slot 1 then has one itself.
        run(&mut sim, 1, counter(CHASTITY, m1));
        assert!(is_running(sim.state, CHASTITY));
        reject(&mut sim, 0, CHASTITY, m0, "already running");
        reject(&mut sim, 1, WEAKNESS, m1, "already has a running");
        let b74 = menu_button(&rules, MISSIONARY, |b| b.msg == CHASTITY as i32).unwrap();
        let b75 = menu_button(&rules, MISSIONARY, |b| b.msg == WEAKNESS as i32).unwrap();
        assert!(button_locked(&sim, 0, b74), "GUID running");
        assert!(!button_locked(&sim, 0, b75));
        assert!(button_locked(&sim, 1, b75), "slot 1 has a running one");
        sim.state.counter_upgrades.clear();
        assert!(!button_locked(&sim, 0, b74) && !button_locked(&sim, 1, b75));
    }

    // QUIRK W-229: kept
    #[test]
    fn a_duration_under_one_second_becomes_ten() {
        // W-229: config 13 = 0 -> rest 10,000; 1 s stays 1000 (limit `< 1000`).
        for (secs, rest) in [(0, 10_000), (1, 1000)] {
            let mut rules = rules();
            rules.defaults.get_mut(&CHASTITY).unwrap().insert(13, secs);
            let mut state = state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let m = missionary(&mut sim, 0, (10, 10));
            run(&mut sim, 0, counter(CHASTITY, m));
            assert_eq!(
                entries(sim.state),
                [(CHASTITY, rest, m)],
                "Config 13 = {secs}"
            );
        }
    }
}
