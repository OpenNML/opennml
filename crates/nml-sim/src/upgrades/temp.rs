//! Temporary upgrades per object (re/temp-upgrades 1.1, 1.2, 3.1, 3.2, 4.3): applying a record from
//! `tempupgrades.ini` (`GameObject_ApplyTempUpgradeRecord` `0x0046D470`), step 6 of the object tick
//! (`GameObject_TickTempUpgrades` `0x00472600`), the end (`GameObject_RemoveTempUpgrade` `0x0046FD00` with
//! `0x0046FC90`) and applying to the objects of a player (`GameWorld_ApplyTempUpgradeToPlayerObjects`
//! `0x0047B980`).
//!
//! Only the flags of the record decide, never whether a field is filled: a record can carry the path,
//! duration and messages of its predecessor (W-18). The effect on the object is presentation: the host gets
//! an event on every apply and end and only shows the effect with flag `EFFECT` with the path from the rules.

use nml_formats::temp_upgrades::{RecordMessage, flags};

use super::TempUpgradeEntry;
use crate::attributes::{FIRST_ATTRIBUTE, LAST_ATTRIBUTE};
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::Message;
use crate::messaging::send;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

/// Fixed deduction per object tick (`0x00472600`: `rest - 100`, not `dt`; K-59).
pub const TICK_MS: i32 = 100;

/// Message of a record line (`0x0046E800`: send `{id, argc, args}` synchronously).
fn message(m: &RecordMessage) -> Message {
    Message::new(m.code, &m.args)
}

/// `GameObject_ApplyTempUpgradeRecord(rec)` (`0x0046D470`, re/temp-upgrades 3.1): if the object already
/// carries the GUID (`0x0046D49C`, K-284: no stacking, no extension) or there is no
/// record, `false`. Otherwise duration `UPGRADETIME` raw in ms with flag `TIME`, otherwise config 13 of the
/// owner in seconds (`GetOwnerConfigValue` `0x00466930`, K-286); `TAKEUPGRADEVALUES`: for each
/// attribute 13...34 with config `100 + id` != 0 a percentage modifier with key GUID (clamped to +-100,
/// K-287); then the effect (event to the host), `MSGSTART` synchronously to the object and
/// not until after that the entry at the back of the list (`0x0046A520`).
pub fn apply(sim: &mut Sim, id: ObjectId, guid: u32) -> bool {
    let rules = sim.rules;
    let Some(o) = sim.object(id) else {
        return false;
    };
    if o.temp_upgrades.iter().any(|e| e.guid == guid) {
        return false;
    }
    let Some(rec) = rules.temp_upgrade(guid) else {
        return false;
    };
    let slot = o.owner();
    let rest_ms = if rec.flags & flags::TIME != 0 {
        rec.time_ms
    } else {
        rules.config_value(slot, guid, 13).wrapping_mul(1000)
    };
    if rec.flags & flags::TAKEUPGRADEVALUES != 0 {
        let values: Vec<(u32, i32)> = (FIRST_ATTRIBUTE..=LAST_ATTRIBUTE)
            .map(|a| (a, rules.config_value(slot, guid, 100 + a)))
            .filter(|&(_, v)| v != 0)
            .collect();
        if let Some(o) = sim.object_mut(id) {
            for (a, v) in values {
                o.attributes.set_modifier(a, v, true, guid as i32);
            }
        }
    }
    // `GameObject_AddOwnedEffect` only with flag `EFFECT` - presentation in the host.
    sim.state
        .events
        .push(SimEvent::TempUpgradeStarted { object: id, guid });
    if rec.flags & flags::MSGSTART != 0 {
        send(sim, id, &message(&rec.msg_start));
    }
    let Some(o) = sim.object_mut(id) else {
        return false;
    };
    o.temp_upgrades.push(TempUpgradeEntry {
        flags: rec.flags,
        guid,
        rest_ms,
    });
    true
}

/// `GameObject_TickTempUpgrades` (`0x00472600`, re/temp-upgrades 3.2), step 6 of the object tick,
/// also during the animation lock (K-59): for each entry in list order send `MSGTICK`
/// (flag `0x400`, message from the shared space `msg_end`), with flag `TEMPORARY`
/// `rest - 100 < 1` -> mark, otherwise deduct (never without `TEMPORARY`, K-285); if the object is
/// `DIED`, mark every entry (re/temp-upgrades 9 V16). Afterwards [`remove`] for each marked GUID (duplicates
/// harmless: the second call finds nothing any more).
pub fn tick(sim: &mut Sim, id: ObjectId) {
    let rules = sim.rules;
    let mut ended: Vec<u32> = Vec::new();
    let mut i = 0;
    loop {
        let Some(o) = sim.object(id) else { return };
        let Some(&TempUpgradeEntry { flags: f, guid, .. }) = o.temp_upgrades.get(i) else {
            break;
        };
        if f & flags::MSGTICK != 0
            && let Some(rec) = rules.temp_upgrade(guid)
        {
            send(sim, id, &message(&rec.msg_end));
        }
        let Some(o) = sim.object_mut(id) else { return };
        if f & flags::TEMPORARY != 0
            && let Some(e) = o.temp_upgrades.get_mut(i)
        {
            let r = e.rest_ms.wrapping_sub(TICK_MS);
            if r < 1 {
                ended.push(guid);
            } else {
                e.rest_ms = r;
            }
        }
        if o.status & status::DIED != 0 {
            ended.push(guid);
        }
        i += 1;
    }
    for guid in ended {
        remove(sim, id, guid);
    }
}

/// `GameObject_RemoveTempUpgrade(g)` (`0x0046FD00`, re/temp-upgrades 3.2): **all** entries with
/// this GUID in list order; for each entry first the end with a copy (`0x0046FC90`):
/// flag `TAKEUPGRADEVALUES` -> `AttributeSet_RemoveModifier(id, g)` for 13...34, end the effect
/// (event to the host), flag `MSGEND` -> `MSGEND` to the object (absolute values, W-225); then
/// remove the node (`PtrList_RemoveAt`).
pub fn remove(sim: &mut Sim, id: ObjectId, guid: u32) {
    let rules = sim.rules;
    let mut i = 0;
    loop {
        let Some(o) = sim.object_mut(id) else { return };
        let Some(p) = o.temp_upgrades[i.min(o.temp_upgrades.len())..]
            .iter()
            .position(|e| e.guid == guid)
            .map(|p| p + i)
        else {
            return;
        };
        let entry = o.temp_upgrades[p].clone();
        if entry.flags & flags::TAKEUPGRADEVALUES != 0 {
            for a in FIRST_ATTRIBUTE..=LAST_ATTRIBUTE {
                o.attributes.remove_modifier(a, guid as i32);
            }
        }
        sim.state
            .events
            .push(SimEvent::TempUpgradeEnded { object: id, guid });
        if entry.flags & flags::MSGEND != 0
            && let Some(rec) = rules.temp_upgrade(guid)
        {
            send(sim, id, &message(&rec.msg_end));
        }
        let Some(o) = sim.object_mut(id) else { return };
        // The node is the same as long as `MSGEND` has not changed the list.
        match o.temp_upgrades.get(p) {
            Some(e) if *e == entry => {
                o.temp_upgrades.remove(p);
                i = p;
            }
            _ => i = p + 1,
        }
    }
}

/// `GameWorld_ApplyTempUpgradeToPlayerObjects(p, rec)` (`0x0047B980`, re/temp-upgrades 3.3): only
/// player 0 <= `slot` < 8; every object of the slot table in table order whose type names the
/// upgrade in `OBJ_UPGRADES` gets the record ([`apply`]; W-224: taxes per
/// governor).
pub fn apply_to_player_objects(sim: &mut Sim, slot: i32, guid: u32) {
    let Some(p) = u8::try_from(slot)
        .ok()
        .filter(|&p| usize::from(p) < PLAYER_SLOTS)
    else {
        return;
    };
    let types = sim.rules.types_with_upgrade(guid);
    for id in super::objects_of_slot(sim.state, p) {
        if sim.object(id).is_some_and(|o| types.contains(&o.guid)) {
            apply(sim, id, guid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Envelope;
    use crate::messaging::{post_command, send_mut};
    use crate::module::building::building;
    use crate::module::construct::{self, construct};
    use crate::module::walk;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::{object_tick, step};
    use crate::upgrades::testkit::*;
    use crate::world::{set_direction, set_grid_position};

    /// Governor "Collect taxes" (`0x4E1`, `MSGTICK 57F {0, 1000}`), "reduce build time"
    /// (`0xE7`, `MSGSTART`/`MSGEND 601`), military advisor artillery (`0xD9`, ranged +175) and wolf
    /// (`0x49`, permanent); re/temp-upgrades 1.4.
    const TAX: u32 = 4450;
    const BUILD: u32 = 4451;
    const ARTILLERY: u32 = 4457;
    const WOLF: u32 = 4460;

    fn entries(sim: &Sim, id: ObjectId) -> Vec<(u32, u32, i32)> {
        sim.object(id)
            .expect("object present")
            .temp_upgrades
            .iter()
            .map(|e| (e.flags, e.guid, e.rest_ms))
            .collect()
    }

    /// Raw gold (20.12) of the slot.
    fn gold(sim: &Sim, slot: usize) -> i32 {
        sim.state.players[slot].resources_fx[0]
    }

    fn started(events: &[SimEvent]) -> Vec<(ObjectId, u32)> {
        events
            .iter()
            .filter_map(|e| match *e {
                SimEvent::TempUpgradeStarted { object, guid } => Some((object, guid)),
                _ => None,
            })
            .collect()
    }

    fn ended(events: &[SimEvent]) -> Vec<(ObjectId, u32)> {
        events
            .iter()
            .filter_map(|e| match *e {
                SimEvent::TempUpgradeEnded { object, guid } => Some((object, guid)),
                _ => None,
            })
            .collect()
    }

    fn ticks(sim: &mut Sim, id: ObjectId, n: usize) {
        for _ in 0..n {
            object_tick(sim, id, 100);
        }
    }

    fn assert_loads(state: &SimState, rules: &Rules) {
        let bytes = crate::save::testkit::save_bytes(state, rules);
        crate::save::SaveGame::from_bytes(&bytes)
            .expect("savegame readable")
            .check(rules)
            .expect("savegame valid");
    }

    // QUIRK K-286, W-224: kept
    #[test]
    fn taxes_pay_146_gold_per_governor_in_600_ticks() {
        // re/temp-upgrades 9 V1, W-224: only governors of slot 0 (not the colonist, not the governor of
        // slot 1); entry {0x4E1, 4450, 60000}, 600 object ticks of +1000 raw each, removed in the 600th tick,
        // no MSGEND (otherwise +1000 more).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        let c = spawn(&mut sim, COLONIST, 0, (12, 10));
        let other = spawn(&mut sim, GOVERNOR, 1, (30, 30));
        sim.state.events.clear();
        apply_to_player_objects(&mut sim, 0, TAX);
        assert_eq!(entries(&sim, g), [(0x4E1, TAX, 60_000)]);
        assert!(entries(&sim, c).is_empty() && entries(&sim, other).is_empty());
        assert_eq!(started(&sim.state.events), [(g, TAX)]);
        // Only players 0-7.
        for slot in [-1, 8, 10] {
            apply_to_player_objects(&mut sim, slot, TAX);
        }
        let before = gold(&sim, 0);
        ticks(&mut sim, g, 599);
        assert_eq!(entries(&sim, g), [(0x4E1, TAX, 100)]);
        assert!(ended(&sim.state.events).is_empty());
        ticks(&mut sim, g, 1);
        assert!(entries(&sim, g).is_empty(), "removed in the 600th tick");
        let paid = gold(&sim, 0) - before;
        assert_eq!(paid, 600_000);
        assert_eq!((paid >> 12, paid & 0xFFF), (146, 1984));
        assert_eq!(ended(&sim.state.events), [(g, TAX)]);
        ticks(&mut sim, g, 10);
        assert_eq!(gold(&sim, 0) - before, 600_000);
    }

    // QUIRK W-224: kept
    #[test]
    fn two_governors_pay_double_taxes_in_1200_steps() {
        // re/temp-upgrades 9 V2, W-224: two governors, via `tick::step` (one object tick = two steps):
        // +1,200,000 raw, step 6 runs in the ordinary object tick.
        let rules = rules();
        let mut state = state(&rules);
        let (g1, g2) = {
            let mut sim = Sim::new(&mut state, &rules);
            let g1 = spawn(&mut sim, GOVERNOR, 0, (10, 10));
            let g2 = spawn(&mut sim, GOVERNOR, 0, (20, 10));
            apply_to_player_objects(&mut sim, 0, TAX);
            (g1, g2)
        };
        let before = state.players[0].resources_fx[0];
        let mut events = Vec::new();
        for _ in 0..1200 {
            events.extend(step(&mut state, &rules, &[]));
        }
        assert_eq!(state.players[0].resources_fx[0] - before, 1_200_000);
        let mut ends = ended(&events);
        ends.sort_by_key(|&(o, _)| o.0);
        let mut expected = vec![(g1, TAX), (g2, TAX)];
        expected.sort_by_key(|&(o, _)| o.0);
        assert_eq!(ends, expected);
    }

    // QUIRK K-284: kept
    #[test]
    fn applying_the_same_temp_guid_twice_changes_nothing() {
        // re/temp-upgrades 9 V3, K-284: again after 30 s -> false, rest keeps running, no second event.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        assert!(apply(&mut sim, g, TAX));
        ticks(&mut sim, g, 300);
        sim.state.events.clear();
        assert!(!apply(&mut sim, g, TAX));
        apply_to_player_objects(&mut sim, 0, TAX);
        assert_eq!(entries(&sim, g), [(0x4E1, TAX, 30_000)]);
        assert!(started(&sim.state.events).is_empty());
        ticks(&mut sim, g, 1);
        assert_eq!(entries(&sim, g), [(0x4E1, TAX, 29_900)]);
        // Without a record or object: false.
        assert!(!apply(&mut sim, g, 4999));
        assert!(!apply(&mut sim, ObjectId::new(500, 0, 1), TAX));
    }

    /// Colonist on the docking point of the construction site (like the `construct` tests).
    fn place_on_dock(sim: &mut Sim, w: ObjectId, b: ObjectId) {
        let o = sim.object(w).unwrap();
        let layer = walk::walk(sim, w).unwrap().layer as i32;
        let mut m = Message::new(0xB03, &[o.grid.x.0, o.grid.z.0, w.as_i32(), layer]);
        assert!(send_mut(sim, b, &mut m) >= 0);
        send(sim, b, &Message::new(0xD03, &[w.as_i32()]));
        let y = sim.object(w).unwrap().grid.y;
        let p = crate::fx::FxVec3::new(crate::fx::Fx(m.arg(0)), y, crate::fx::Fx(m.arg(1)));
        set_grid_position(sim, w, p, true);
        set_direction(sim, w, m.arg(2) << 12);
    }

    /// House (`BuildTime 6000`) with one worker, with or without 4451: build steps after the
    /// two ticks up to the first build step (order, start).
    fn build_house(sim: &mut Sim, upgrade: bool) -> (ObjectId, usize) {
        let b = site(sim, HOUSE, 0, (20, 20));
        let w = spawn(sim, COLONIST, 0, (14, 20));
        if upgrade {
            assert!(apply(sim, w, BUILD));
            assert_eq!(
                construct(sim, w).unwrap().speed,
                300,
                "MSGSTART immediately"
            );
        }
        place_on_dock(sim, w, b);
        let order = Envelope::bare(Message::new(construct::msg::CONSTRUCT, &[b.as_i32()]));
        post_command(sim, w, order, true);
        ticks(sim, w, 2);
        let mut steps = 0;
        while building(sim, b).unwrap().build_counter >= 1 {
            ticks(sim, w, 1);
            steps += 1;
            assert!(steps < 1000);
        }
        (w, steps)
    }

    // QUIRK W-225: kept
    #[test]
    fn faster_building_sets_speed_300_and_resets_it_to_100() {
        // re/temp-upgrades 9 V4, W-225: 0x601 {300} on applying, build step 30 -> 200 instead of 600 steps; after
        // 60 s 0x601 {100} (absolute).
        let rules = rules();
        let mut state = state(&rules);
        let (_, plain) = build_house(&mut Sim::new(&mut state, &rules), false);
        assert_eq!(plain, 600);
        let mut state = super::super::testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (w, fast) = build_house(&mut sim, true);
        assert_eq!(fast, 200);
        ticks(&mut sim, w, 599 - (2 + fast));
        assert_eq!(construct(&sim, w).unwrap().speed, 300);
        assert_eq!(entries(&sim, w), [(0xE7, BUILD, 100)]);
        ticks(&mut sim, w, 1);
        assert_eq!(construct(&sim, w).unwrap().speed, 100, "MSGEND absolute");
        assert!(entries(&sim, w).is_empty());
    }

    // QUIRK K-287: kept
    #[test]
    fn percent_modifiers_are_clamped_to_100() {
        // re/temp-upgrades 9 V6, K-287: config 120 = 175 -> modifier +100 (ranged 20 -> 40, not 55); the end
        // takes it out again.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        let far = |sim: &Sim| sim.object(g).unwrap().attributes.max_value(20, 0);
        assert_eq!(far(&sim), 20);
        assert!(apply(&mut sim, g, ARTILLERY));
        assert_eq!(far(&sim), 40);
        remove(&mut sim, g, ARTILLERY);
        assert_eq!(far(&sim), 20);
        assert!(entries(&sim, g).is_empty());
    }

    // QUIRK K-285: kept
    #[test]
    fn entries_without_0x80_never_expire() {
        // K-285: 4460 (`0x49`, without config 13: rest 0) stays after 10,000 ticks, the values keep
        // acting (ranged 20 + 60%); the state loads (rest 0 without 0x80 is valid).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        assert!(apply(&mut sim, a, WOLF));
        ticks(&mut sim, a, 10_000);
        assert_eq!(entries(&sim, a), [(0x49, WOLF, 0)]);
        assert_eq!(sim.object(a).unwrap().attributes.max_value(20, 0), 32);
        assert!(ended(&sim.state.events).is_empty());
        assert_loads(&state, &rules);
    }

    // QUIRK W-225: kept
    #[test]
    fn a_dying_carrier_ends_its_temp_upgrades_with_msgend() {
        // re/temp-upgrades 9 V16: in the death tick (step 5 sets DIED) step 6 removes the entry, `0x601 {100}`
        // to itself, exactly one end.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        assert!(apply(&mut sim, w, BUILD));
        ticks(&mut sim, w, 10);
        sim.state.events.clear();
        sim.object_mut(w)
            .unwrap()
            .attributes
            .set_current_value(14, 0, 0);
        ticks(&mut sim, w, 1);
        assert_ne!(sim.object(w).unwrap().status & status::DIED, 0);
        assert!(entries(&sim, w).is_empty());
        assert_eq!(construct(&sim, w).unwrap().speed, 100);
        assert_eq!(ended(&sim.state.events), [(w, BUILD)]);
        ticks(&mut sim, w, 5);
        assert_eq!(
            ended(&sim.state.events),
            [(w, BUILD)],
            "MSGEND exactly once"
        );
    }

    // QUIRK K-288: kept
    #[test]
    fn an_owner_change_keeps_the_temp_list_and_pays_the_new_owner() {
        // K-288: the list moves along (rest keeps running), `MSGTICK 57F` pays the new
        // owner; the state loads.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        assert!(apply(&mut sim, g, TAX));
        ticks(&mut sim, g, 100);
        let new = crate::world::change_owner(&mut sim, g, 1).expect("change of owner");
        assert_eq!(entries(&sim, new), [(0x4E1, TAX, 50_000)]);
        let (old_gold, new_gold) = (gold(&sim, 0), gold(&sim, 1));
        ticks(&mut sim, new, 100);
        assert_eq!(gold(&sim, 0), old_gold);
        assert_eq!(gold(&sim, 1) - new_gold, 100_000);
        assert_eq!(entries(&sim, new), [(0x4E1, TAX, 40_000)]);
        assert_loads(&state, &rules);
    }

    // QUIRK K-59: kept
    #[test]
    fn the_tick_runs_during_the_animation_lock() {
        // K-59: step 6 lies outside the lock branch - MSGTICK and deduction run.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        assert!(apply(&mut sim, g, TAX));
        if let Some(o) = sim.object_mut(g) {
            o.anim_lock_ms = 1000;
            o.set_status_flags(status::ANIM_LOCKED, true);
        }
        let before = gold(&sim, 0);
        ticks(&mut sim, g, 1);
        assert_eq!(sim.object(g).unwrap().anim_lock_ms, 900, "locked");
        assert_eq!(gold(&sim, 0) - before, 1000);
        assert_eq!(entries(&sim, g), [(0x4E1, TAX, 59_900)]);
    }
}
