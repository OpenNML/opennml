//! Influence and aura of module 44 (`CPropertyExecTempUpgrade`, re/evoke-aura 1.4, 2.3, 3.6-3.9;
//! re/temp-upgrades 3.4): `0x22C {idx}` starts an influence (`0x00512D20`), the aura tick `0x42C`
//! (`0x00512F30`) compares the hits in the surroundings against the set of influenced objects every 250 ms
//! (measured every three object ticks, C-285), `0x32C {idx}` ends an influence (`0x00512580`, no in-game sender).
//! Change of owner (slot 5 `0x005127C0`) and destructor (`0x00512C00`) take the set and remove the influences
//! from all objects in it ([`release_set`]). The host derives the influence effect 450003 (ring) from the state
//! ([`ring_shown`], deviation from the original, re/evoke-aura).
//!
//! The intermediate set `+0x8C` of the original is not a list of its own in the reimplementation: during the aura
//! tick it points to the same list as `+0x88` ([`ExecTempUpgrade::influenced`]); outside the tick it is `NULL`,
//! and `0x22C` with a running set would read it when removing (C-290, exception A7). The reimplementation
//! removes only from the passed list and once from the set.

use nml_formats::targets::MatchFilter;
use nml_formats::temp_upgrades::flags;

use crate::collision::query_radius;
use crate::filter;
use crate::id::ObjectId;
use crate::message::Message;
use crate::sim::Sim;
use crate::upgrades::temp;

use super::exec_temp::{ExecTempUpgrade, GROUP, INDEX, Influence, msg};
use super::{Module, ModuleKind};

/// Delay of the aura tick (ms): three calls of `update_events` (C-285).
pub const AURA_MS: i32 = 250;
/// Sight (attribute 18): a quarter of it is the aura radius.
const SIGHT: u32 = 18;
/// Search filter of the aura tick: `PLAYERLIST | ANDALL | PROPERTY` with module 0 (walk, even
/// when switched off), player -1 - that is, every object with walk (re/evoke-aura 3.7).
const SEARCH_MODE: u32 = 0x4_0014;
/// Filter of an entry (`0x00512D20`): `FRIENDLIST`, with `ENEMYUPGRADE` `ENEMYLIST`.
const FRIENDLIST: u32 = 0x800;
const ENEMYLIST: u32 = 0x400;

fn exec<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s ExecTempUpgrade> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::ExecTempUpgrade(e) => Some(e),
        _ => None,
    }
}

fn exec_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut ExecTempUpgrade> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::ExecTempUpgrade(e) => Some(e),
        _ => None,
    }
}

/// GUID `450001[idx]` (`IntList_GetAt` `0x00405840`: outside the list 0, so no record).
fn guid_at(e: &ExecTempUpgrade, idx: i32) -> u32 {
    usize::try_from(idx)
        .ok()
        .and_then(|i| e.exec_list.get(i).copied())
        .unwrap_or(0)
}

/// Slot of the owner (`player` of the filters).
fn owner_slot(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).map_or(-1, |o| i32::from(o.owner()))
}

fn post_aura_tick(sim: &mut Sim, id: ObjectId) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(AURA_MS, Message::new(msg::AURA_TICK, &[]), false, GROUP);
    }
}

/// `0x22C {idx}` (`0x00512D20`, re/evoke-aura 3.6): GUID `450001[idx]`; if it already runs ("upgradeGUID
/// %d still exist") or its record is missing -> 0 without effect. Otherwise the entry goes to the end of the
/// influence list (enemy flag from the record); without a set `0x42C` after 250 ms (group `0x80`,
/// several calls before the first tick start several chains, C-291), with a set [`apply_to_set`] at once
/// on a copy of the set (the missing intermediate set counts as empty, C-290).
pub fn start(sim: &mut Sim, id: ObjectId, idx: i32) -> i32 {
    let Some(e) = exec(sim, id) else { return 0 };
    let guid = guid_at(e, idx);
    if e.influences.iter().any(|i| i.guid == guid) {
        return 0;
    }
    let Some(rec_flags) = sim.rules.temp_upgrade(guid).map(|r| r.flags) else {
        return 0;
    };
    let influence = Influence {
        guid,
        enemy: rec_flags & flags::ENEMY != 0,
    };
    let Some(e) = exec_mut(sim, id) else { return 0 };
    e.influences.push(influence.clone());
    match e.influenced.clone() {
        None => post_aura_tick(sim, id),
        Some(mut set) => {
            let slot = owner_slot(sim, id);
            apply_to_set(sim, id, &mut set, &influence, true, slot);
        }
    }
    0
}

/// Aura tick `0x42C` (`0x00512F30`, re/evoke-aura 3.7): without influences 0 (the chain ends). Otherwise
/// reschedule (in the firing handler: every three object ticks, C-285), surroundings search with radius
/// `Fixed(Max(18)) >> 2` around the grid position, new/lost against the old set ("lost" only
/// if the old ID still exists, C-286), the hits become the new set; then first apply all
/// new ones, afterwards remove from all lost ones, with the slot of the carrier. Return value 1.
pub fn aura_tick(sim: &mut Sim, id: ObjectId) -> i32 {
    if exec(sim, id).is_none_or(|e| e.influences.is_empty()) {
        return 0;
    }
    post_aura_tick(sim, id);
    let Some(o) = sim.object(id) else { return 1 };
    let pos = (o.grid.x.0, o.grid.z.0);
    let slot = i32::from(o.owner());
    let p = sim.type_upgrade_percent(o.owner(), o.guid, SIGHT);
    let r = o.attributes.max_value(SIGHT, p).wrapping_shl(12) >> 2;
    let search = MatchFilter {
        mode: SEARCH_MODE,
        prop_id: 0,
        ..MatchFilter::constructed()
    };
    let found = query_radius(sim, pos, &search, r, -1, false);
    let old = exec(sim, id).and_then(|e| e.influenced.clone());
    let (mut gained, mut lost): (Vec<ObjectId>, Vec<ObjectId>) = match &old {
        None => (found.clone(), Vec::new()),
        Some(old) => (
            found
                .iter()
                .filter(|&h| !old.contains(h))
                .copied()
                .collect(),
            old.iter()
                .copied()
                .filter(|&h| !found.contains(&h) && sim.object(h).is_some())
                .collect(),
        ),
    };
    if let Some(e) = exec_mut(sim, id) {
        e.influenced = Some(found);
    }
    apply_all(sim, id, &mut gained, true, slot);
    apply_all(sim, id, &mut lost, false, slot);
    1
}

/// `0x32C {idx}` (`0x00512580`, re/evoke-aura 3.8; only the debug console of the original sends it):
/// if the GUID `450001[idx]` is not running -> 0 without change. Otherwise remove the entry and, with a
/// set present, remove from all objects in it; if it was the last one, the set becomes free (the
/// next `0x42C` ends the chain). Removing the influence effect **before** the search (C-292)
/// has no counterpart in the reimplementation: the ring follows from the state ([`ring_shown`]).
pub fn stop(sim: &mut Sim, id: ObjectId, idx: i32) -> i32 {
    let Some(e) = exec_mut(sim, id) else { return 0 };
    let guid = guid_at(e, idx);
    let Some(pos) = e.influences.iter().position(|i| i.guid == guid) else {
        return 0;
    };
    let influence = e.influences.remove(pos);
    let Some(mut set) = e.influenced.clone() else {
        return 0;
    };
    let slot = owner_slot(sim, id);
    apply_to_set(sim, id, &mut set, &influence, false, slot);
    if let Some(e) = exec_mut(sim, id)
        && e.influences.is_empty()
    {
        e.influenced = None;
    }
    0
}

/// Take the set and remove all influences from all objects in it, with the filter of the slot `slot`
/// (`0x00512700(set, 0, slot)`): on change of owner (slot 5 `0x005127C0`, the old slot,
/// after deleting the group `0x80` in [`super::exec_temp::on_owner_changed`]) and in the
/// destructor ([`destruct`], the current one). The influence list stays: after a
/// change of owner no aura tick starts any more, a new `0x22C` of the same GUID finds it
/// (C-223). Without a set nothing.
pub(crate) fn release_set(sim: &mut Sim, id: ObjectId, slot: i32) {
    let Some(mut set) = exec_mut(sim, id).and_then(|e| e.influenced.take()) else {
        return;
    };
    apply_all(sim, id, &mut set, false, slot);
}

/// Destructor (slot 1 `0x00513390` -> `0x00512C00`, re/evoke-aura 1.4, 3.9): set present -> remove with
/// the current slot, without group deletion. Takes effect only when destroying without dying
/// (when dying, the change of owner to slot 8 has already cleaned up).
pub fn destruct(sim: &mut Sim, id: ObjectId) {
    let slot = owner_slot(sim, id);
    release_set(sim, id, slot);
}

/// The aura ring (450003) is to be shown (deviation from the original, for host and control channel): non-empty
/// influence list and a set or a scheduled aura tick. After change of owner, death and
/// the last `0x32C` it is off.
pub fn ring_shown(module: &Module) -> bool {
    match &module.kind {
        ModuleKind::ExecTempUpgrade(e) => {
            !e.influences.is_empty()
                && (e.influenced.is_some() || module.base.has_pending_timer(msg::AURA_TICK))
        }
        _ => false,
    }
}

/// `0x00512700(list, on, slot)`: influences outside, targets inside - a target that an influence from
/// `list` removes is no longer seen by the next one.
fn apply_all(sim: &mut Sim, id: ObjectId, list: &mut Vec<ObjectId>, on: bool, slot: i32) {
    if list.is_empty() {
        return;
    }
    let influences = exec(sim, id)
        .map(|e| e.influences.clone())
        .unwrap_or_default();
    for influence in &influences {
        apply_to_set(sim, id, list, influence, on, slot);
    }
}

/// `0x005119E0(list, entry, on, slot)` per target in list order (next node noted
/// beforehand; the reimplementation runs over an index): the object exists (even dead), matches the filter
/// of the entry (`FRIENDLIST`/`ENEMYLIST` with `slot`; a precheck rejects the dead and
/// the quartered, C-287) and its type is in `OBJ_UPGRADES` of the GUID. Removing:
/// [`temp::remove`]. Applying: [`temp::apply`]; if it fails (GUID already there, C-288), the
/// target is dropped from `list` and once from the set.
fn apply_to_set(
    sim: &mut Sim,
    id: ObjectId,
    list: &mut Vec<ObjectId>,
    influence: &Influence,
    on: bool,
    slot: i32,
) {
    let rules = sim.rules;
    let types = rules.types_with_upgrade(influence.guid);
    let entry = MatchFilter {
        mode: if influence.enemy {
            ENEMYLIST
        } else {
            FRIENDLIST
        },
        ..MatchFilter::constructed()
    };
    let mut i = 0;
    while let Some(&t) = list.get(i) {
        let Some(guid) = sim.object(t).map(|o| o.guid) else {
            i += 1;
            continue;
        };
        if !filter::matches(sim, t, &entry, slot) || !types.contains(&guid) {
            i += 1;
            continue;
        }
        if !on {
            temp::remove(sim, t, influence.guid);
            i += 1;
            continue;
        }
        if temp::apply(sim, t, influence.guid) {
            i += 1;
            continue;
        }
        list.remove(i);
        if let Some(set) = exec_mut(sim, id).and_then(|e| e.influenced.as_mut())
            && let Some(p) = set.iter().position(|&h| h == t)
        {
            set.remove(p);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Test vectors re/evoke-aura 9 V7 to V16 and quirks re/evoke-aura 8 no. 11-18, 20. "Tick" is the
    //! object tick (two `tick::step`); the animals arise directly (`tk::spawn` and `0x314`, as
    //! when appearing from module 32), only the timeline test really evokes.

    use std::cell::RefCell;

    use super::*;
    use crate::messaging::send;
    use crate::module::ability_testkit as tk;
    use crate::module::{spell, test_hook};
    use crate::object::status;
    use crate::state::SimEvent;
    use crate::world::{change_owner, destroy_object, set_grid_position};
    use nml_formats::temp_upgrades::parse_temp_upgrades;

    const HITPOINTS: u32 = 14;
    const SPEED: u32 = 16;
    const NEAR: u32 = 19;
    const FAR: u32 = 20;

    thread_local! {
        /// Delivered queries `(object, module, code)` during [`observed`].
        static SEEN: RefCell<Vec<(ObjectId, usize, u32)>> = const { RefCell::new(Vec::new()) };
    }

    fn record(_: &mut Sim, id: ObjectId, index: usize, m: &Message) {
        SEEN.with(|s| s.borrow_mut().push((id, index, m.code)));
    }

    fn observed(f: impl FnOnce()) -> Vec<(ObjectId, usize, u32)> {
        SEEN.with(|s| s.borrow_mut().clear());
        test_hook::observe(record, f);
        SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }

    /// Number of aura ticks (`0x42C` to module 44) in `seen`, only at `carrier` (or at all).
    fn auras(seen: &[(ObjectId, usize, u32)], carrier: Option<ObjectId>) -> usize {
        seen.iter()
            .filter(|&&(id, index, code)| {
                index == INDEX && code == msg::AURA_TICK && carrier.is_none_or(|c| c == id)
            })
            .count()
    }

    /// One object tick; return value: delivered queries.
    fn tick(sim: &mut Sim) -> Vec<(ObjectId, usize, u32)> {
        observed(|| tk::run_ticks(sim, 1))
    }

    /// Object ticks up to and including the next aura tick of `carrier` (at most four);
    /// return value: number of ticks.
    fn next_aura(sim: &mut Sim, carrier: ObjectId) -> usize {
        for n in 1..=4 {
            if auras(&tick(sim), Some(carrier)) > 0 {
                return n;
            }
        }
        panic!("no aura tick of {carrier:?} in four ticks");
    }

    /// Animal `guid` on slot `slot`, "evoked" like from `0x220` (`0x314` -> `0x22C {0}`).
    fn evoked(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let id = tk::spawn(sim, guid, slot, tile);
        send(sim, id, &Message::new(0x314, &[]));
        assert_eq!(influences(sim, id).len(), 1, "0x22C {{0}} started");
        id
    }

    fn exec_of<'s>(sim: &'s Sim, id: ObjectId) -> &'s ExecTempUpgrade {
        exec(sim, id).expect("module 44")
    }

    fn influences(sim: &Sim, id: ObjectId) -> Vec<u32> {
        exec_of(sim, id).influences.iter().map(|i| i.guid).collect()
    }

    fn influenced(sim: &Sim, id: ObjectId) -> Option<Vec<ObjectId>> {
        exec_of(sim, id).influenced.clone()
    }

    fn temp_guids(sim: &Sim, id: ObjectId) -> Vec<u32> {
        sim.object(id)
            .expect("object")
            .temp_upgrades
            .iter()
            .map(|e| e.guid)
            .collect()
    }

    fn aura_pending(sim: &Sim, id: ObjectId) -> bool {
        sim.module(id, INDEX)
            .is_some_and(|m| m.base.has_pending_timer(msg::AURA_TICK))
    }

    /// Number of timers with `code` (newly created and running, without group deletion).
    fn timers(sim: &Sim, id: ObjectId, code: u32) -> usize {
        let b = &sim.module(id, INDEX).expect("module 44").base;
        b.timers_ws.iter().filter(|e| e.msg.code == code).count()
            + b.events.iter().filter(|e| e.msg.code == code).count()
    }

    fn ring(sim: &Sim, id: ObjectId) -> bool {
        ring_shown(sim.module(id, INDEX).expect("module 44"))
    }

    /// `(current, max)` of an attribute with the upgrade percentages of the owner.
    fn value(sim: &Sim, id: ObjectId, a: u32) -> (i32, i32) {
        let o = sim.object(id).expect("object");
        let p = sim.type_upgrade_percent(o.owner(), o.guid, a);
        (
            o.attributes.current_value(a, p),
            o.attributes.max_value(a, p),
        )
    }

    fn move_to(sim: &mut Sim, id: ObjectId, tile: (i32, i32)) {
        let pos = crate::fx::FxVec3::new(
            crate::fx::Fx(tile.0 * 4096 + 2048),
            crate::fx::Fx::ZERO,
            crate::fx::Fx(tile.1 * 4096 + 2048),
        );
        set_grid_position(sim, id, pos, true);
    }

    fn kill(sim: &mut Sim, id: ObjectId) {
        let o = sim.object(id).expect("object");
        let p = sim.type_upgrade_percent(o.owner(), o.guid, HITPOINTS);
        sim.object_mut(id)
            .unwrap()
            .attributes
            .set_current_value(HITPOINTS, 0, p);
    }

    fn dead(sim: &Sim, id: ObjectId) -> bool {
        sim.object(id).is_some_and(|o| o.status & status::DIED != 0)
    }

    // QUIRK C-289: kept
    #[test]
    fn the_wolf_aura_reaches_own_and_allied_upgrade_types() {
        // V7, V15, C-289: radius `Fixed(20) >> 2` = 20,480 (5 tiles, center distance).
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.diplomacy.set_friend(0, 2, true);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (23, 20));
        let ranged = tk::spawn(&mut sim, tk::RANGED, 0, (20, 24));
        let evoker = tk::spawn(&mut sim, tk::ANIMAL_EVOKER, 0, (21, 21));
        let colonist = tk::spawn(&mut sim, tk::COLONIST, 0, (18, 20));
        let enemy = tk::spawn(&mut sim, tk::WARRIOR, 1, (20, 18));
        let far = tk::spawn(&mut sim, tk::WARRIOR, 0, (26, 20));
        let ally = tk::spawn(&mut sim, tk::WARRIOR, 2, (20, 22));
        assert_eq!(
            [
                sim.attribute(warrior, NEAR),
                sim.attribute(ranged, FAR),
                sim.attribute(evoker, NEAR)
            ],
            [9, 11, 20]
        );
        let wolf = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        assert_eq!(value(&sim, wolf, 18).1.wrapping_shl(12) >> 2, 20_480);
        assert!(ring(&sim, wolf), "timer 0x42C scheduled");
        next_aura(&mut sim, wolf);
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        assert_eq!(sim.attribute(ranged, FAR), 17);
        assert_eq!(sim.attribute(evoker, NEAR), 32);
        assert_eq!(sim.attribute(ally, NEAR), 14, "V15: ally");
        for id in [warrior, ranged, evoker, ally] {
            assert_eq!(temp_guids(&sim, id), [tk::WOLF_AURA], "{id:?}");
        }
        for id in [colonist, enemy, far] {
            assert!(temp_guids(&sim, id).is_empty(), "{id:?}");
        }
        assert_eq!(
            (sim.attribute(enemy, NEAR), sim.attribute(far, NEAR)),
            (9, 9)
        );
        // The set holds all hits (also the rejected ones: colonist, enemy, wolf itself), not
        // the distant warrior.
        let set = influenced(&sim, wolf).expect("set");
        for id in [warrior, ranged, evoker, ally, colonist, enemy, wolf] {
            assert!(set.contains(&id), "{id:?} in the set");
        }
        assert!(!set.contains(&far));
        assert!(ring(&sim, wolf));
    }

    // QUIRK C-285: kept
    #[test]
    fn the_aura_tick_runs_every_three_ticks() {
        // V10, C-285: real evocation; "E" is the tick in which the wolf appears (`0x314` ->
        // `0x22C`). Measured: the first aura tick comes in E+3 (the wolf already ticks in E),
        // then every +3 ticks.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ev = tk::spawn(&mut sim, tk::ANIMAL_EVOKER, 0, (10, 10));
        let p = spell::mana_percent(&sim, ev);
        sim.object_mut(ev)
            .unwrap()
            .attributes
            .set_current_value(spell::MANA, 100, p);
        assert!(crate::messaging::post_message(
            &mut sim,
            ev,
            Message::new(0x120, &[0, 441]),
            true
        ));
        let (mut appeared, mut aura_ticks) = (None, Vec::new());
        for t in 1..=100 {
            let mut events: Vec<SimEvent> = Vec::new();
            let seen = observed(|| {
                for _ in 0..2 {
                    events.extend(crate::tick::step(sim.state, sim.rules, &[]));
                }
            });
            let wolf = events.iter().find_map(|e| match e {
                SimEvent::Created { id } if sim.object(*id).is_some_and(|o| o.guid == tk::WOLF) => {
                    Some(*id)
                }
                _ => None,
            });
            if let Some(w) = wolf {
                appeared = Some((t, w));
            }
            if auras(&seen, None) > 0 {
                aura_ticks.push(t);
            }
        }
        let (e, wolf) = appeared.expect("wolf appeared");
        assert_eq!(e, 82, "N+81 with N = tick 1");
        assert_eq!(aura_ticks[0], e + 3, "first aura tick");
        for w in aura_ticks.windows(2) {
            assert_eq!(w[1] - w[0], 3, "{aura_ticks:?}");
        }
        assert_eq!(aura_ticks.len(), 6, "E+3 ... E+18");
        // The evoker stands one tile in front of the wolf: he gets the aura (C-289).
        assert_eq!(temp_guids(&sim, ev), [tk::WOLF_AURA]);
        assert_eq!(influences(&sim, wolf), [tk::WOLF_AURA]);
    }

    #[test]
    fn the_bear_aura_raises_max_hitpoints() {
        // V8: 4461 hit points +60 %, the fraction stays (160/160 -> 256/256); out of range -> 160/160.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let bear = evoked(&mut sim, tk::BEAR, 0, (20, 20));
        assert_eq!(value(&sim, warrior, HITPOINTS), (160, 160));
        next_aura(&mut sim, bear);
        assert_eq!(value(&sim, warrior, HITPOINTS), (256, 256));
        move_to(&mut sim, warrior, (30, 20));
        assert_eq!(next_aura(&mut sim, bear), 3);
        assert_eq!(value(&sim, warrior, HITPOINTS), (160, 160));
        assert!(temp_guids(&sim, warrior).is_empty());
    }

    #[test]
    fn the_cat_aura_speeds_up() {
        // V9: 4462 speed +60 %: 183 -> 292 (truncated).
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let cat = evoked(&mut sim, tk::CAT, 0, (20, 20));
        assert_eq!(sim.attribute(warrior, SPEED), 183);
        next_aura(&mut sim, cat);
        assert_eq!(sim.attribute(warrior, SPEED), 292);
    }

    // QUIRK C-288: kept
    #[test]
    fn two_wolves_do_not_stack() {
        // V11, C-288: B discards the warrior in each of its ticks (4460 already there); A runs away ->
        // 9, in the next tick of B 14 again. B appears one tick after the first tick of A,
        // so the chains lie in different ticks.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (21, 22));
        let a = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        next_aura(&mut sim, a);
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        tick(&mut sim);
        let b = evoked(&mut sim, tk::WOLF, 0, (23, 20));
        for _ in 0..2 {
            next_aura(&mut sim, b);
            assert_eq!(sim.attribute(warrior, NEAR), 14, "not 19");
            assert_eq!(temp_guids(&sim, warrior), [tk::WOLF_AURA]);
            assert!(
                !influenced(&sim, b).unwrap().contains(&warrior),
                "B has discarded him"
            );
            assert!(influenced(&sim, a).unwrap().contains(&warrior));
        }
        move_to(&mut sim, a, (40, 40));
        let seen_a = loop {
            let seen = tick(&mut sim);
            if auras(&seen, Some(a)) > 0 {
                break seen;
            }
        };
        assert_eq!(auras(&seen_a, Some(b)), 0, "different ticks");
        assert_eq!(sim.attribute(warrior, NEAR), 9, "A gone");
        let gap = next_aura(&mut sim, b);
        assert!(gap <= 3, "gap up to 3 ticks: {gap}");
        assert_eq!(sim.attribute(warrior, NEAR), 14, "from B");
        assert!(influenced(&sim, b).unwrap().contains(&warrior));
    }

    // QUIRK C-287: kept
    #[test]
    fn quartering_in_the_radius_keeps_the_bonus() {
        // V12, C-287: quartered within the radius -> not found ("lost"), the precheck
        // QUARTERED rejects the removal; left outside -> the bonus stays permanently.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let house = tk::spawn(&mut sim, tk::COLONIST, 0, (22, 21));
        let wolf = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        next_aura(&mut sim, wolf);
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        {
            let o = sim.object_mut(warrior).unwrap();
            o.status |= status::QUARTERED;
            o.carrier = house;
        }
        next_aura(&mut sim, wolf);
        assert!(!influenced(&sim, wolf).unwrap().contains(&warrior));
        assert_eq!(sim.attribute(warrior, NEAR), 14, "removal rejected");
        {
            let o = sim.object_mut(warrior).unwrap();
            o.status &= !status::QUARTERED;
            o.carrier = ObjectId::NONE;
        }
        move_to(&mut sim, warrior, (30, 20));
        for _ in 0..3 {
            next_aura(&mut sim, wolf);
        }
        assert_eq!(sim.attribute(warrior, NEAR), 14, "permanent");
        assert_eq!(temp_guids(&sim, warrior), [tk::WOLF_AURA]);
    }

    // QUIRK C-286: kept
    #[test]
    fn an_owner_change_of_the_beneficiary_keeps_the_bonus() {
        // C-286: the beneficiary switches to an ally (slot 2). The old ID
        // no longer exists, so it is never "lost"; the new one is "new", applying fails
        // (4460 already there) and it is dropped from the set. Outside the radius 4460 stays.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.diplomacy.set_friend(0, 2, true);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let wolf = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        next_aura(&mut sim, wolf);
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        let new = change_owner(&mut sim, warrior, 2).expect("new ID");
        assert_eq!(temp_guids(&sim, new), [tk::WOLF_AURA]);
        next_aura(&mut sim, wolf);
        let set = influenced(&sim, wolf).unwrap();
        assert!(!set.contains(&new) && !set.contains(&warrior));
        move_to(&mut sim, new, (30, 20));
        for _ in 0..3 {
            next_aura(&mut sim, wolf);
        }
        assert_eq!(sim.attribute(new, NEAR), 14, "permanent");
        assert_eq!(temp_guids(&sim, new), [tk::WOLF_AURA]);
    }

    // QUIRK C-223: kept
    #[test]
    fn the_aura_ends_with_the_wolf() {
        // V13: death in T (`check_death`), ownership on slot 8 in phase 0 after T -> slot 5 removes
        // 4460 from all three; the influence list stays, no aura tick any more, no ring.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let three = [
            tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20)),
            tk::spawn(&mut sim, tk::WARRIOR, 0, (20, 22)),
            tk::spawn(&mut sim, tk::RANGED, 0, (18, 20)),
        ];
        let wolf = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        next_aura(&mut sim, wolf);
        for id in three {
            assert_eq!(temp_guids(&sim, id), [tk::WOLF_AURA]);
        }
        kill(&mut sim, wolf);
        // T: the wolf dies in its tick (`check_death`), the change of owner to slot 8 runs
        // at the end of the phase-0 step of the same object tick.
        let mut events = Vec::new();
        for _ in 0..2 {
            events.extend(crate::tick::step(sim.state, sim.rules, &[]));
        }
        let corpse = events
            .iter()
            .find_map(|e| match e {
                SimEvent::OwnerChanged { old, new } if *old == wolf => Some(*new),
                _ => None,
            })
            .expect("ownership on slot 8");
        assert_eq!(corpse.slot(), 8);
        assert!(dead(&sim, corpse) && sim.object(wolf).is_none());
        for id in three {
            assert!(temp_guids(&sim, id).is_empty(), "{id:?} after phase 0");
        }
        assert_eq!(influences(&sim, corpse), [tk::WOLF_AURA], "list stays");
        assert_eq!(influenced(&sim, corpse), None);
        assert!(!aura_pending(&sim, corpse) && !ring(&sim, corpse));
        let mut seen = Vec::new();
        for _ in 0..10 {
            seen.extend(tick(&mut sim));
        }
        assert_eq!(auras(&seen, None), 0, "no 0x42C any more");
        for id in three {
            assert!(temp_guids(&sim, id).is_empty());
        }
    }

    // QUIRK C-223: kept
    #[test]
    fn a_living_owner_change_ends_the_aura_for_good() {
        // V14, C-223: removal with the filter of the **old** slot (with slot 1 the
        // own warrior would stay, he is its enemy); the new `0x22C` finds the GUID ("still exist"),
        // never an aura again - not even for the warriors of the new owner.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let theirs = tk::spawn(&mut sim, tk::WARRIOR, 1, (20, 22));
        let wolf = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        next_aura(&mut sim, wolf);
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        let new = change_owner(&mut sim, wolf, 1).expect("new ID");
        assert_eq!(sim.attribute(warrior, NEAR), 9, "old slot");
        assert_eq!(influences(&sim, new), [tk::WOLF_AURA]);
        assert_eq!(influenced(&sim, new), None);
        assert!(!aura_pending(&sim, new) && !ring(&sim, new));
        assert_eq!(
            send(&mut sim, new, &Message::new(msg::INFLUENCE_START, &[0])),
            0
        );
        assert!(!aura_pending(&sim, new), "still exist");
        let mut seen = Vec::new();
        for _ in 0..10 {
            seen.extend(tick(&mut sim));
        }
        assert_eq!(auras(&seen, None), 0);
        assert_eq!(
            (sim.attribute(warrior, NEAR), sim.attribute(theirs, NEAR)),
            (9, 9)
        );
        assert!(temp_guids(&sim, theirs).is_empty());
    }

    #[test]
    fn destroying_the_carrier_removes_the_bonus() {
        // Destructor `0x00512C00`: destroying without dying removes with the current slot.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let wolf = evoked(&mut sim, tk::WOLF, 0, (20, 20));
        next_aura(&mut sim, wolf);
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        destroy_object(&mut sim, wolf);
        assert!(sim.object(wolf).is_none());
        assert_eq!(sim.attribute(warrior, NEAR), 9);
        assert!(temp_guids(&sim, warrior).is_empty());
    }

    /// Wolf with the execution list `[4460, 4461]` (C-290, extended directly).
    fn two_aura_wolf(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
        let wolf = evoked(sim, tk::WOLF, 0, tile);
        if let Some(e) = exec_mut(sim, wolf) {
            e.exec_list.push(tk::BEAR_AURA);
        }
        wolf
    }

    // QUIRK C-290: deviation
    #[test]
    fn a_second_influence_applies_at_once() {
        // C-290 (A7): a second influence with a running set is applied at once;
        // if a target already carries the GUID, applying fails - the original would then read the
        // intermediate set `NULL`, the reimplementation removes it from the set, without panic.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let carrying = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let other = tk::spawn(&mut sim, tk::WARRIOR, 0, (20, 22));
        let wolf = two_aura_wolf(&mut sim, (20, 20));
        next_aura(&mut sim, wolf);
        assert!(temp::apply(&mut sim, carrying, tk::BEAR_AURA));
        assert_eq!(temp_guids(&sim, carrying), [tk::WOLF_AURA, tk::BEAR_AURA]);
        assert_eq!(
            send(&mut sim, wolf, &Message::new(msg::INFLUENCE_START, &[1])),
            0
        );
        assert_eq!(influences(&sim, wolf), [tk::WOLF_AURA, tk::BEAR_AURA]);
        let set = influenced(&sim, wolf).unwrap();
        assert!(!set.contains(&carrying), "removed");
        assert!(set.contains(&other));
        assert_eq!(
            temp_guids(&sim, carrying),
            [tk::WOLF_AURA, tk::BEAR_AURA],
            "4461 once"
        );
        assert_eq!(
            temp_guids(&sim, other),
            [tk::WOLF_AURA, tk::BEAR_AURA],
            "at once, without tick"
        );
        assert_eq!(value(&sim, other, HITPOINTS), (256, 256));
        assert_eq!(timers(&sim, wolf, msg::AURA_TICK), 1, "no second chain");
    }

    // QUIRK C-291: kept
    #[test]
    fn init_posts_direct_influences() {
        // C-291: type with 450001 = `4460 4461` and 450002 = `0 1` -> init posts two
        // `0x22C` after 100 ms (group `0x80`); created before the first `update_events` (tick 1
        // takes them over), they fire in tick 3, both before the first aura tick, and each creates
        // a `0x42C` - two chains, one tick per chain every three object ticks.
        const TWO: u32 = 3499;
        let extra = format!(
            "GUID {TWO}\nNAME ANI_Zweifach\nVALUEID 100 6 1 {}\nPROPID 0\nVALUEID 10001 6 1 1\n\
             VALUEID 10003 6 1 2119936\nVALUEID 10004 6 1 1024\nVALUEID 10005 6 1 2048\n\
             VALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 1\nPROPID 2\n\
             VALUEID 30001 1 1 0\nPROPID 44\nVALUEID 450001 1 1 4460 4461\n\
             VALUEID 450002 1 1 0 1\n",
            0x200_0000
        );
        let mut rules = tk::evoke_rules_from(&format!("{}{extra}", tk::evoke_types()));
        rules
            .defaults
            .insert(TWO, [(14, 80), (18, 20)].into_iter().collect());
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let carrier = tk::spawn(&mut sim, TWO, 0, (20, 20));
        assert_eq!(exec_of(&sim, carrier).influence_list, [0, 1]);
        assert_eq!(timers(&sim, carrier, msg::INFLUENCE_START), 2);
        assert!(influences(&sim, carrier).is_empty());
        let mut seen = Vec::new();
        for _ in 0..2 {
            seen.extend(tick(&mut sim));
        }
        assert!(
            influences(&sim, carrier).is_empty(),
            "nothing after two ticks"
        );
        seen.extend(tick(&mut sim));
        let starts = seen
            .iter()
            .filter(|&&(id, i, c)| id == carrier && i == INDEX && c == msg::INFLUENCE_START)
            .count();
        assert_eq!(starts, 2, "two 0x22C after 100 ms");
        // `Module_PostDelayedMessage` creates at the front of `m_TimersWS`: the second entry fires
        // first.
        assert_eq!(influences(&sim, carrier), [tk::BEAR_AURA, tk::WOLF_AURA]);
        assert_eq!(influenced(&sim, carrier), None, "before the first tick");
        assert_eq!(timers(&sim, carrier, msg::AURA_TICK), 2, "two chains");
        let mut seen = Vec::new();
        for _ in 0..9 {
            seen.extend(tick(&mut sim));
        }
        assert_eq!(auras(&seen, Some(carrier)), 6, "two per three ticks");
        assert_eq!(temp_guids(&sim, warrior), [tk::BEAR_AURA, tk::WOLF_AURA]);
        assert_eq!(value(&sim, warrior, HITPOINTS), (256, 256));
        assert_eq!(sim.attribute(warrior, NEAR), 14);
        assert_eq!(timers(&sim, carrier, msg::AURA_TICK), 2);
    }

    // QUIRK C-292: deviation
    #[test]
    fn stop_removes_the_influence() {
        // `0x32C` (re/evoke-aura 3.8, deviation from C-292): only the simulation part.
        // Unknown index -> 0 without change; entry gone and removed from the set; the last one
        // frees the set, the next `0x42C` ends the chain.
        let rules = tk::evoke_rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let warrior = tk::spawn(&mut sim, tk::WARRIOR, 0, (22, 20));
        let wolf = two_aura_wolf(&mut sim, (20, 20));
        next_aura(&mut sim, wolf);
        send(&mut sim, wolf, &Message::new(msg::INFLUENCE_START, &[1]));
        assert_eq!(temp_guids(&sim, warrior), [tk::WOLF_AURA, tk::BEAR_AURA]);
        let stop =
            |sim: &mut Sim, idx: i32| send(sim, wolf, &Message::new(msg::INFLUENCE_STOP, &[idx]));
        let before = exec_of(&sim, wolf).clone();
        for idx in [5, -1] {
            assert_eq!(stop(&mut sim, idx), 0);
            assert_eq!(exec_of(&sim, wolf), &before, "index {idx}");
        }
        assert_eq!(stop(&mut sim, 0), 0);
        assert_eq!(influences(&sim, wolf), [tk::BEAR_AURA]);
        assert_eq!(temp_guids(&sim, warrior), [tk::BEAR_AURA]);
        assert_eq!(sim.attribute(warrior, NEAR), 9);
        assert!(influenced(&sim, wolf).is_some() && ring(&sim, wolf));
        assert_eq!(stop(&mut sim, 1), 0);
        assert!(influences(&sim, wolf).is_empty());
        assert!(temp_guids(&sim, warrior).is_empty());
        assert_eq!(influenced(&sim, wolf), None);
        assert!(!ring(&sim, wolf));
        assert!(aura_pending(&sim, wolf), "the timer is still running");
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.extend(tick(&mut sim));
        }
        assert_eq!(auras(&seen, Some(wolf)), 1, "one last tick");
        assert!(!aura_pending(&sim, wolf), "chain ended");
    }

    // QUIRK K-64: kept
    #[test]
    fn the_0x67f_message_applies_both_records() {
        // V16, K-64 (`0x0046DF40...0x0046E00A`): first the fraction (A full, B 50 % -> A
        // 50 %, B 100 %), then record `arg2` on A and `arg3` on B. Without a record (GUID
        // unknown or missing argument = 0) nothing; if the target is missing, the whole branch is dropped.
        let mut rules = tk::evoke_rules();
        rules.temp_upgrades.extend(
            parse_temp_upgrades("TEMPUPGRADE 4452\nEND\nTEMPUPGRADE 4453\nEND\n")
                .expect("records")
                .into_iter()
                .map(|t| (t.guid, t)),
        );
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = tk::spawn(&mut sim, tk::WARRIOR, 0, (20, 20));
        let b = tk::spawn(&mut sim, tk::WARRIOR, 0, (24, 20));
        let p = sim.type_upgrade_percent(0, tk::WARRIOR, HITPOINTS);
        sim.object_mut(b)
            .unwrap()
            .attributes
            .set_current_value(HITPOINTS, 80, p);
        let fraction = |sim: &Sim, id| {
            sim.object(id)
                .unwrap()
                .attributes
                .current_fraction(HITPOINTS)
        };
        let transfer = |sim: &mut Sim, args: &[i32]| {
            send(sim, a, &Message::new(0x67F, args));
        };
        transfer(&mut sim, &[b.as_i32(), 14, 4452, 4453]);
        assert_eq!((fraction(&sim, a), fraction(&sim, b)), (50_000, 100_000));
        assert_eq!(temp_guids(&sim, a), [4452]);
        assert_eq!(temp_guids(&sim, b), [4453]);
        // Without a record or with missing arguments: only the fraction.
        transfer(&mut sim, &[b.as_i32(), 14, 9999, 0]);
        transfer(&mut sim, &[b.as_i32(), 14]);
        transfer(&mut sim, &[b.as_i32(), 14, 4453]);
        assert_eq!(temp_guids(&sim, a), [4452, 4453]);
        assert_eq!(temp_guids(&sim, b), [4453]);
        // Target missing: `arg2` not either.
        let c = tk::spawn(&mut sim, tk::WARRIOR, 0, (28, 20));
        let gone = c;
        destroy_object(&mut sim, c);
        send(
            &mut sim,
            b,
            &Message::new(0x67F, &[gone.as_i32(), 14, 4452, 4452]),
        );
        assert_eq!(temp_guids(&sim, b), [4453]);
        assert_eq!(fraction(&sim, b), 100_000);
    }
}
